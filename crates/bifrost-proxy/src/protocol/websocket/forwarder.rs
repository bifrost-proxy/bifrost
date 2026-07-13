use tokio::io::{AsyncRead, AsyncWrite};

use super::{WebSocketFrame, WebSocketReader, WebSocketWriter};

pub struct WebSocketForwarder;

pub type WebSocketFrameCallback =
    Box<dyn Fn(&WebSocketFrame) -> Option<WebSocketFrame> + Send + Sync>;

impl WebSocketForwarder {
    pub async fn bidirectional<R1, W1, R2, W2>(
        mut client_reader: R1,
        mut client_writer: W1,
        mut server_reader: R2,
        mut server_writer: W2,
        on_client_frame: Option<WebSocketFrameCallback>,
        on_server_frame: Option<WebSocketFrameCallback>,
    ) -> std::io::Result<(u64, u64)>
    where
        R1: AsyncRead + Unpin + Send + 'static,
        W1: AsyncWrite + Unpin + Send + 'static,
        R2: AsyncRead + Unpin + Send + 'static,
        W2: AsyncWrite + Unpin + Send + 'static,
    {
        use futures_util::StreamExt;

        let client_to_server = async move {
            let mut reader = WebSocketReader::new(&mut client_reader);
            let mut writer = WebSocketWriter::new(&mut server_writer, true);
            let mut count = 0u64;

            while let Some(result) = reader.next().await {
                let frame = result?;

                let frame_to_write = if let Some(ref transform) = on_client_frame {
                    transform(&frame)
                } else {
                    Some(frame)
                };

                if let Some(f) = frame_to_write {
                    writer.write_frame(f).await?;
                    count += 1;
                }
            }

            Ok::<_, std::io::Error>(count)
        };

        let server_to_client = async move {
            let mut reader = WebSocketReader::new(&mut server_reader);
            let mut writer = WebSocketWriter::new(&mut client_writer, false);
            let mut count = 0u64;

            while let Some(result) = reader.next().await {
                let frame = result?;

                let frame_to_write = if let Some(ref transform) = on_server_frame {
                    transform(&frame)
                } else {
                    Some(frame)
                };

                if let Some(f) = frame_to_write {
                    writer.write_frame(f).await?;
                    count += 1;
                }
            }

            Ok::<_, std::io::Error>(count)
        };

        let (r1, r2) = tokio::try_join!(client_to_server, server_to_client)?;
        Ok((r1, r2))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use tokio::io::{duplex, empty, sink, AsyncWriteExt};

    #[tokio::test]
    async fn bidirectional_forwards_client_frames_and_counts() {
        let (forwarder_side, mut client_side) = duplex(1024);

        // Prepare two simple frames from client to server.
        let frames = vec![
            WebSocketFrame::text("hello"),
            WebSocketFrame::binary(b"world"),
        ];
        let mut encoded = Vec::new();
        for frame in &frames {
            encoded.extend_from_slice(&frame.encode());
        }

        // Feed encoded frames into the client-side connection.
        let writer = tokio::spawn(async move {
            client_side.write_all(&encoded).await.unwrap();
        });

        let (client_to_server, server_to_client) =
            WebSocketForwarder::bidirectional(forwarder_side, sink(), empty(), sink(), None, None)
                .await
                .unwrap();

        writer.await.unwrap();

        assert_eq!(client_to_server, 2);
        assert_eq!(server_to_client, 0);
    }

    #[tokio::test]
    async fn bidirectional_applies_callbacks_and_can_drop_frames() {
        let (forwarder_side, mut client_side) = duplex(1024);

        let frames = vec![WebSocketFrame::text("keep"), WebSocketFrame::text("drop")];
        let mut encoded = Vec::new();
        for frame in &frames {
            encoded.extend_from_slice(&frame.encode());
        }

        let writer = tokio::spawn(async move {
            client_side.write_all(&encoded).await.unwrap();
        });

        let on_client_frame: WebSocketFrameCallback = Box::new(|frame: &WebSocketFrame| {
            if frame.payload == Bytes::from_static(b"drop") {
                None
            } else {
                Some(frame.clone())
            }
        });

        let (client_to_server, server_to_client) = WebSocketForwarder::bidirectional(
            forwarder_side,
            sink(),
            empty(),
            sink(),
            Some(on_client_frame),
            None,
        )
        .await
        .unwrap();

        writer.await.unwrap();

        // Only one frame should be forwarded due to the callback dropping the second.
        assert_eq!(client_to_server, 1);
        assert_eq!(server_to_client, 0);
    }

    #[tokio::test]
    async fn coverage_90_server_callback_transforms_and_counts_frame() {
        let (server_forwarder, mut server_peer) = duplex(1024);
        let frame = WebSocketFrame::text("server").encode();
        let writer = tokio::spawn(async move {
            server_peer.write_all(&frame).await.unwrap();
        });
        let callback: WebSocketFrameCallback = Box::new(|_| Some(WebSocketFrame::text("changed")));
        let (client_count, server_count) = WebSocketForwarder::bidirectional(
            empty(),
            sink(),
            server_forwarder,
            sink(),
            None,
            Some(callback),
        )
        .await
        .unwrap();
        writer.await.unwrap();
        assert_eq!(client_count, 0);
        assert_eq!(server_count, 1);
    }
}
