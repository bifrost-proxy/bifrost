use super::*;

#[tokio::test]
async fn terminal_result_closes_client_writer_with_stdin_still_open() {
    let _lock = super::tests::BROKER_TEST_ENV_LOCK.lock().await;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    std::env::set_var(BROKER_ADDR_ENV, listener.local_addr().unwrap().to_string());
    std::env::set_var(BROKER_TOKEN_ENV, "close-handshake-token");
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let (reader, mut writer) = stream.into_split();
        let mut reader = BufReader::new(reader);
        let start = super::super::read_limited_async_line(&mut reader, BROKER_MAX_FRAME_BYTES)
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(
            serde_json::from_str::<BrokerRequest>(&start).unwrap(),
            BrokerRequest::Start { .. }
        ));
        write_terminal_response(
            &mut reader,
            &mut writer,
            &BrokerResponse::Result {
                response: RemoteInvokeResponse::default(),
            },
        )
        .await
        .unwrap();
    });
    let (stdin_tx, stdin_rx) = mpsc::channel(1);
    execute_via_main_broker(&RemoteCommand::default(), Some(stdin_rx), &mut |_| {
        std::future::ready(Ok(()))
    })
    .await
    .unwrap();
    tokio::time::timeout(Duration::from_secs(1), server)
        .await
        .expect("terminal result must abort the client writer without waiting for stdin EOF")
        .unwrap();
    assert!(stdin_tx.is_closed());
    std::env::remove_var(BROKER_ADDR_ENV);
    std::env::remove_var(BROKER_TOKEN_ENV);
}

#[tokio::test]
async fn terminal_response_survives_unread_stdin_without_tcp_reset() {
    use tokio::io::AsyncReadExt;
    use tokio::net::TcpSocket;

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let socket = TcpSocket::new_v4().unwrap();
    // Keep response bytes in flight when the server finishes its writes.
    socket.set_recv_buffer_size(4096).unwrap();
    let (client, accepted) = tokio::join!(
        socket.connect(listener.local_addr().unwrap()),
        listener.accept()
    );
    let mut client = client.unwrap();
    let (server_read, mut server_write) = accepted.unwrap().0.into_split();
    let mut server_read = BufReader::new(server_read);
    // Model completion winning the select before pending stdin is read.
    write_request(&mut client, &BrokerRequest::StdinClose)
        .await
        .unwrap();
    let expected = "file-chunk".repeat(128 * 1024);
    let frame = BrokerResponse::Result {
        response: RemoteInvokeResponse {
            stdout: Some(expected.clone()),
            ..Default::default()
        },
    };
    let server = tokio::spawn(async move {
        write_terminal_response(&mut server_read, &mut server_write, &frame)
            .await
            .unwrap();
    });

    let mut bytes = Vec::new();
    tokio::time::timeout(Duration::from_secs(10), client.read_to_end(&mut bytes))
        .await
        .expect("broker must half-close without waiting for stdin EOF")
        .expect("unread stdin must not reset the response stream");
    match serde_json::from_slice::<BrokerResponse>(&bytes).unwrap() {
        BrokerResponse::Result { response } => assert_eq!(response.stdout, Some(expected)),
        other => panic!("expected terminal result, got {other:?}"),
    }
    // The broker drains until its peer has consumed the result and closes.
    assert!(!server.is_finished());
    client.shutdown().await.unwrap();
    tokio::time::timeout(Duration::from_secs(1), server)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn terminal_response_bounds_wait_for_stalled_peer() {
    use tokio::io::AsyncReadExt;

    let (mut client, server) = tokio::io::duplex(1024);
    let (mut reader, mut writer) = tokio::io::split(server);
    let server = tokio::spawn(async move {
        write_terminal_response(
            &mut reader,
            &mut writer,
            &BrokerResponse::Error {
                error: "rejected".to_string(),
            },
        )
        .await
        .unwrap();
    });
    let mut bytes = Vec::new();
    client.read_to_end(&mut bytes).await.unwrap();
    assert!(matches!(
        serde_json::from_slice::<BrokerResponse>(&bytes).unwrap(),
        BrokerResponse::Error { error } if error == "rejected"
    ));
    assert!(!server.is_finished());
    tokio::time::timeout(BROKER_CLOSE_TIMEOUT + Duration::from_secs(1), server)
        .await
        .expect("stalled peer must not retain a broker connection")
        .unwrap();
}

#[tokio::test]
async fn terminal_response_handles_peer_reset_and_shutdown_failure() {
    use std::io;
    use std::pin::Pin;
    use std::task::{Context, Poll};
    use tokio::io::ReadBuf;

    struct BrokenPeer;

    impl AsyncRead for BrokenPeer {
        fn poll_read(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            _buf: &mut ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            Poll::Ready(Err(io::ErrorKind::ConnectionReset.into()))
        }
    }

    impl AsyncWrite for BrokenPeer {
        fn poll_write(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            bytes: &[u8],
        ) -> Poll<io::Result<usize>> {
            Poll::Ready(Ok(bytes.len()))
        }

        fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }

        fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Err(io::ErrorKind::BrokenPipe.into()))
        }
    }

    let frame = BrokerResponse::Result {
        response: RemoteInvokeResponse::default(),
    };
    // A peer reset after the terminal response does not invalidate that result.
    write_terminal_response(&mut BrokenPeer, &mut tokio::io::sink(), &frame)
        .await
        .unwrap();
    let error = write_terminal_response(&mut tokio::io::empty(), &mut BrokenPeer, &frame)
        .await
        .unwrap_err();
    assert!(error.contains("shutdown Remote Execution broker response"));
}
