use std::sync::Arc;

use bifrost_admin::{AdminState, FrameDirection, TrafficType};
use bifrost_core::{BifrostError, Result};
use hyper::upgrade::Upgraded;
use hyper_util::rt::TokioIo;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tracing::debug;

pub async fn tunnel_bidirectional(
    upgraded: Upgraded,
    target: TcpStream,
    verbose_logging: bool,
    req_id: &str,
    admin_state: Option<&Arc<AdminState>>,
) -> Result<()> {
    tunnel_bidirectional_io(
        TokioIo::new(upgraded),
        target,
        verbose_logging,
        req_id,
        admin_state,
    )
    .await
}

async fn tunnel_bidirectional_io<C, T>(
    client: C,
    target: T,
    verbose_logging: bool,
    req_id: &str,
    admin_state: Option<&Arc<AdminState>>,
) -> Result<()>
where
    C: AsyncRead + AsyncWrite + Unpin,
    T: AsyncRead + AsyncWrite + Unpin,
{
    let (mut target_read, mut target_write) = tokio::io::split(target);

    let (client_read, client_write) = tokio::io::split(client);
    let mut client_read = client_read;
    let mut client_write = client_write;

    let admin_state_clone = admin_state.cloned();
    let admin_state_clone2 = admin_state.cloned();

    let client_to_target = async move {
        let mut buf = [0u8; 16384];
        loop {
            let n = client_read.read(&mut buf).await?;
            if n == 0 {
                break;
            }
            target_write.write_all(&buf[..n]).await?;

            if let Some(ref state) = admin_state_clone {
                state
                    .metrics_collector
                    .add_bytes_sent_by_type(TrafficType::Tunnel, n as u64);
            }
        }
        target_write.shutdown().await?;
        Ok::<_, std::io::Error>(())
    };

    let target_to_client = async move {
        let mut buf = [0u8; 16384];
        loop {
            let n = target_read.read(&mut buf).await?;
            if n == 0 {
                break;
            }
            client_write.write_all(&buf[..n]).await?;

            if let Some(ref state) = admin_state_clone2 {
                state
                    .metrics_collector
                    .add_bytes_received_by_type(TrafficType::Tunnel, n as u64);
            }
        }
        Ok::<_, std::io::Error>(())
    };

    let result = tokio::try_join!(client_to_target, target_to_client);

    match result {
        Ok(_) => {
            if verbose_logging {
                debug!("[{}] Tunnel closed normally", req_id);
            } else {
                debug!("Tunnel closed normally");
            }
            Ok(())
        }
        Err(e) => {
            if e.kind() == std::io::ErrorKind::ConnectionReset
                || e.kind() == std::io::ErrorKind::BrokenPipe
            {
                if verbose_logging {
                    debug!("[{}] Tunnel closed: {}", req_id, e);
                } else {
                    debug!("Tunnel closed: {}", e);
                }
                Ok(())
            } else {
                Err(BifrostError::Network(format!("Tunnel error: {}", e)))
            }
        }
    }
}

#[derive(Debug)]
pub struct TunnelStats {
    pub bytes_sent: u64,
    pub bytes_received: u64,
    pub cancelled: bool,
}

fn shutdown_tunnel_task<T>(task: &mut JoinHandle<std::io::Result<T>>) {
    task.abort();
}

fn tunnel_result_from_error(
    verbose_logging: bool,
    req_id: &str,
    error: std::io::Error,
) -> Result<TunnelStats> {
    if error.kind() == std::io::ErrorKind::ConnectionReset
        || error.kind() == std::io::ErrorKind::BrokenPipe
    {
        if verbose_logging {
            debug!("[{}] Tunnel closed: {}", req_id, error);
        } else {
            debug!("Tunnel closed: {}", error);
        }
        Ok(TunnelStats {
            bytes_sent: 0,
            bytes_received: 0,
            cancelled: false,
        })
    } else {
        Err(BifrostError::Network(format!("Tunnel error: {}", error)))
    }
}

pub async fn tunnel_bidirectional_with_cancel(
    upgraded: Upgraded,
    target: TcpStream,
    verbose_logging: bool,
    req_id: &str,
    admin_state: Option<&Arc<AdminState>>,
    cancel_rx: oneshot::Receiver<()>,
) -> Result<TunnelStats> {
    tunnel_bidirectional_with_cancel_io(
        TokioIo::new(upgraded),
        target,
        verbose_logging,
        req_id,
        admin_state,
        cancel_rx,
    )
    .await
}

async fn tunnel_bidirectional_with_cancel_io<C, T>(
    client: C,
    target: T,
    verbose_logging: bool,
    req_id: &str,
    admin_state: Option<&Arc<AdminState>>,
    cancel_rx: oneshot::Receiver<()>,
) -> Result<TunnelStats>
where
    C: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    T: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (mut target_read, mut target_write) = tokio::io::split(target);
    let (client_read, client_write) = tokio::io::split(client);
    let mut client_read = client_read;
    let mut client_write = client_write;

    let admin_state_clone = admin_state.cloned();
    let admin_state_clone2 = admin_state.cloned();
    let req_id_owned = req_id.to_string();
    let req_id_owned2 = req_id.to_string();

    let client_to_target = async move {
        let mut buf = [0u8; 16384];
        let mut total_sent: u64 = 0;
        loop {
            let n = client_read.read(&mut buf).await?;
            if n == 0 {
                break;
            }
            target_write.write_all(&buf[..n]).await?;
            total_sent += n as u64;

            if let Some(ref state) = admin_state_clone {
                state
                    .metrics_collector
                    .add_bytes_sent_by_type(TrafficType::Tunnel, n as u64);
                // 对于隧道连接，只更新流量统计，不记录详细帧
                state.connection_monitor.update_traffic(
                    &req_id_owned,
                    FrameDirection::Send,
                    n as u64,
                );
            }
        }
        target_write.shutdown().await?;
        Ok::<_, std::io::Error>(total_sent)
    };

    let target_to_client = async move {
        let mut buf = [0u8; 16384];
        let mut total_received: u64 = 0;
        loop {
            let n = target_read.read(&mut buf).await?;
            if n == 0 {
                break;
            }
            client_write.write_all(&buf[..n]).await?;
            total_received += n as u64;

            if let Some(ref state) = admin_state_clone2 {
                state
                    .metrics_collector
                    .add_bytes_received_by_type(TrafficType::Tunnel, n as u64);
                // 对于隧道连接，只更新流量统计，不记录详细帧
                state.connection_monitor.update_traffic(
                    &req_id_owned2,
                    FrameDirection::Receive,
                    n as u64,
                );
            }
        }

        Ok::<_, std::io::Error>(total_received)
    };

    let mut client_to_target_task = tokio::spawn(client_to_target);
    let mut target_to_client_task = tokio::spawn(target_to_client);

    tokio::select! {
        result = &mut client_to_target_task => {
            shutdown_tunnel_task(&mut target_to_client_task);
            match result {
                Ok(Ok(bytes_sent)) => {
                    if verbose_logging {
                        debug!("[{}] Tunnel closed after client stream ended", req_id);
                    } else {
                        debug!("Tunnel closed after client stream ended");
                    }
                    Ok(TunnelStats { bytes_sent, bytes_received: 0, cancelled: false })
                }
                Ok(Err(error)) => tunnel_result_from_error(verbose_logging, req_id, error),
                Err(join_error) if join_error.is_cancelled() => Ok(TunnelStats {
                    bytes_sent: 0,
                    bytes_received: 0,
                    cancelled: false,
                }),
                Err(join_error) => Err(BifrostError::Network(format!(
                    "Tunnel task join error: {}",
                    join_error
                ))),
            }
        }
        result = &mut target_to_client_task => {
            shutdown_tunnel_task(&mut client_to_target_task);
            match result {
                Ok(Ok(bytes_received)) => {
                    if verbose_logging {
                        debug!("[{}] Tunnel closed after target stream ended", req_id);
                    } else {
                        debug!("Tunnel closed after target stream ended");
                    }
                    Ok(TunnelStats { bytes_sent: 0, bytes_received, cancelled: false })
                }
                Ok(Err(error)) => tunnel_result_from_error(verbose_logging, req_id, error),
                Err(join_error) if join_error.is_cancelled() => Ok(TunnelStats {
                    bytes_sent: 0,
                    bytes_received: 0,
                    cancelled: false,
                }),
                Err(join_error) => Err(BifrostError::Network(format!(
                    "Tunnel task join error: {}",
                    join_error
                ))),
            }
        }
        _ = cancel_rx => {
            shutdown_tunnel_task(&mut client_to_target_task);
            shutdown_tunnel_task(&mut target_to_client_task);
            if verbose_logging {
                debug!("[{}] Tunnel cancelled by config change", req_id);
            }
            Ok(TunnelStats { bytes_sent: 0, bytes_received: 0, cancelled: true })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tunnel_error_classification_handles_expected_disconnects_and_other_errors() {
        for kind in [
            std::io::ErrorKind::ConnectionReset,
            std::io::ErrorKind::BrokenPipe,
        ] {
            let stats =
                tunnel_result_from_error(true, "REQ-test", std::io::Error::new(kind, "closed"))
                    .unwrap();
            assert_eq!(stats.bytes_sent, 0);
            assert_eq!(stats.bytes_received, 0);
            assert!(!stats.cancelled);
        }
        let error = tunnel_result_from_error(
            false,
            "REQ-test",
            std::io::Error::new(std::io::ErrorKind::InvalidData, "bad"),
        )
        .unwrap_err();
        assert!(error.to_string().contains("Tunnel error"));
    }

    #[tokio::test]
    async fn cancelable_tunnel_reports_client_to_target_bytes() {
        let (mut client_peer, client) = tokio::io::duplex(128);
        let (target, mut target_peer) = tokio::io::duplex(128);
        let (_cancel_tx, cancel_rx) = oneshot::channel();
        let task = tokio::spawn(async move {
            tunnel_bidirectional_with_cancel_io(
                client,
                target,
                true,
                "REQ-client-end",
                None,
                cancel_rx,
            )
            .await
        });
        client_peer.write_all(b"client-data").await.unwrap();
        client_peer.shutdown().await.unwrap();
        let mut received = Vec::new();
        target_peer.read_to_end(&mut received).await.unwrap();
        let stats = task.await.unwrap().unwrap();
        assert_eq!(received, b"client-data");
        assert_eq!(stats.bytes_sent, 11);
        assert!(!stats.cancelled);
    }

    #[tokio::test]
    async fn cancelable_tunnel_reports_target_to_client_bytes_with_metrics() {
        let state = Arc::new(AdminState::new(0));
        let (client, mut client_peer) = tokio::io::duplex(128);
        let (mut target_peer, target) = tokio::io::duplex(128);
        let (_cancel_tx, cancel_rx) = oneshot::channel();
        let task = tokio::spawn({
            let state = state.clone();
            async move {
                tunnel_bidirectional_with_cancel_io(
                    client,
                    target,
                    false,
                    "REQ-target-end",
                    Some(&state),
                    cancel_rx,
                )
                .await
            }
        });
        target_peer.write_all(b"target-data").await.unwrap();
        target_peer.shutdown().await.unwrap();
        let mut received = Vec::new();
        client_peer.read_to_end(&mut received).await.unwrap();
        let stats = task.await.unwrap().unwrap();
        assert_eq!(received, b"target-data");
        assert_eq!(stats.bytes_received, 11);
        assert!(!stats.cancelled);
    }

    #[tokio::test]
    async fn cancelable_tunnel_stops_both_tasks_on_signal() {
        let (_client_peer, client) = tokio::io::duplex(64);
        let (target, _target_peer) = tokio::io::duplex(64);
        let (cancel_tx, cancel_rx) = oneshot::channel();
        cancel_tx.send(()).unwrap();
        let stats = tunnel_bidirectional_with_cancel_io(
            client,
            target,
            true,
            "REQ-cancel",
            None,
            cancel_rx,
        )
        .await
        .unwrap();
        assert!(stats.cancelled);
        assert_eq!(stats.bytes_sent, 0);
        assert_eq!(stats.bytes_received, 0);
    }

    #[tokio::test]
    async fn basic_tunnel_forwards_both_directions_and_updates_metrics() {
        let state = Arc::new(AdminState::new(0));
        let (client, mut client_peer) = tokio::io::duplex(128);
        let (target, mut target_peer) = tokio::io::duplex(128);
        let task = tokio::spawn({
            let state = state.clone();
            async move { tunnel_bidirectional_io(client, target, true, "REQ-basic", Some(&state)).await }
        });
        client_peer.write_all(b"client").await.unwrap();
        target_peer.write_all(b"target").await.unwrap();
        let mut from_client = [0u8; 6];
        target_peer.read_exact(&mut from_client).await.unwrap();
        let mut from_target = [0u8; 6];
        client_peer.read_exact(&mut from_target).await.unwrap();
        assert_eq!(&from_client, b"client");
        assert_eq!(&from_target, b"target");
        client_peer.shutdown().await.unwrap();
        target_peer.shutdown().await.unwrap();
        task.await.unwrap().unwrap();
    }

    struct ErrorIo(std::io::ErrorKind);

    impl AsyncRead for ErrorIo {
        fn poll_read(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
            _buf: &mut tokio::io::ReadBuf<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            std::task::Poll::Ready(Err(std::io::Error::new(self.0, "covered")))
        }
    }

    impl AsyncWrite for ErrorIo {
        fn poll_write(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
            _buf: &[u8],
        ) -> std::task::Poll<std::io::Result<usize>> {
            std::task::Poll::Ready(Err(std::io::Error::new(self.0, "covered")))
        }
        fn poll_flush(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            std::task::Poll::Ready(Ok(()))
        }
        fn poll_shutdown(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            std::task::Poll::Ready(Ok(()))
        }
    }

    #[tokio::test]
    async fn basic_tunnel_treats_disconnect_as_success_and_other_errors_as_failure() {
        let (_, peer) = tokio::io::duplex(16);
        tunnel_bidirectional_io(
            ErrorIo(std::io::ErrorKind::ConnectionReset),
            peer,
            false,
            "REQ-reset",
            None,
        )
        .await
        .unwrap();

        let (_, peer) = tokio::io::duplex(16);
        let error = tunnel_bidirectional_io(
            ErrorIo(std::io::ErrorKind::InvalidData),
            peer,
            true,
            "REQ-error",
            None,
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("Tunnel error"));
    }
}
