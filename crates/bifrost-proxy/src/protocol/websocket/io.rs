use std::pin::Pin;
use std::task::{Context, Poll};

use bytes::{Buf, Bytes, BytesMut};
use futures_util::Stream;
use pin_project_lite::pin_project;
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt, ReadBuf};

use super::{Opcode, WebSocketFrame};

const DEFAULT_MAX_FRAGMENT_BUFFER_SIZE: usize = 16 * 1024 * 1024;

pin_project! {
    pub struct WebSocketReader<R> {
        #[pin]
        inner: R,
        buffer: BytesMut,
        fragment_buffer: Vec<u8>,
        fragment_opcode: Option<Opcode>,
        fragment_rsv1: bool,
        fragment_rsv2: bool,
        fragment_rsv3: bool,
        max_fragment_size: usize,
    }
}

impl<R> WebSocketReader<R> {
    pub fn new(inner: R) -> Self {
        Self::with_max_fragment_size(inner, DEFAULT_MAX_FRAGMENT_BUFFER_SIZE)
    }

    pub fn with_initial_buffer(inner: R, buffer: BytesMut) -> Self {
        Self {
            inner,
            buffer,
            fragment_buffer: Vec::new(),
            fragment_opcode: None,
            fragment_rsv1: false,
            fragment_rsv2: false,
            fragment_rsv3: false,
            max_fragment_size: DEFAULT_MAX_FRAGMENT_BUFFER_SIZE,
        }
    }

    pub fn with_max_fragment_size(inner: R, max_fragment_size: usize) -> Self {
        Self {
            inner,
            buffer: BytesMut::with_capacity(8192),
            fragment_buffer: Vec::new(),
            fragment_opcode: None,
            fragment_rsv1: false,
            fragment_rsv2: false,
            fragment_rsv3: false,
            max_fragment_size,
        }
    }

    pub fn into_inner(self) -> R {
        self.inner
    }
}

impl<R: AsyncRead + Unpin> Stream for WebSocketReader<R> {
    type Item = std::io::Result<WebSocketFrame>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let mut this = self.project();

        loop {
            if let Some((frame, consumed)) = WebSocketFrame::parse(this.buffer) {
                this.buffer.advance(consumed);

                if frame.opcode.is_control() {
                    return Poll::Ready(Some(Ok(frame)));
                }

                if frame.opcode == Opcode::Continuation {
                    let new_size = this.fragment_buffer.len() + frame.payload.len();
                    if new_size > *this.max_fragment_size {
                        tracing::warn!(
                            "[WS] Fragment buffer overflow: {} bytes exceeds limit of {} bytes, dropping fragments",
                            new_size,
                            *this.max_fragment_size
                        );
                        this.fragment_buffer.clear();
                        *this.fragment_opcode = None;
                        *this.fragment_rsv1 = false;
                        *this.fragment_rsv2 = false;
                        *this.fragment_rsv3 = false;
                        continue;
                    }
                    this.fragment_buffer.extend_from_slice(&frame.payload);
                    if frame.fin {
                        let opcode = this.fragment_opcode.take().unwrap_or(Opcode::Text);
                        let complete_frame = WebSocketFrame {
                            fin: true,
                            rsv1: *this.fragment_rsv1,
                            rsv2: *this.fragment_rsv2,
                            rsv3: *this.fragment_rsv3,
                            opcode,
                            mask: None,
                            payload: Bytes::from(std::mem::take(this.fragment_buffer)),
                        };
                        *this.fragment_rsv1 = false;
                        *this.fragment_rsv2 = false;
                        *this.fragment_rsv3 = false;
                        return Poll::Ready(Some(Ok(complete_frame)));
                    }
                } else if !frame.fin {
                    let new_size = frame.payload.len();
                    if new_size > *this.max_fragment_size {
                        tracing::warn!(
                            "[WS] Initial fragment too large: {} bytes exceeds limit of {} bytes",
                            new_size,
                            *this.max_fragment_size
                        );
                        this.fragment_buffer.clear();
                        *this.fragment_opcode = None;
                        *this.fragment_rsv1 = false;
                        *this.fragment_rsv2 = false;
                        *this.fragment_rsv3 = false;
                        continue;
                    }
                    *this.fragment_opcode = Some(frame.opcode);
                    *this.fragment_rsv1 = frame.rsv1;
                    *this.fragment_rsv2 = frame.rsv2;
                    *this.fragment_rsv3 = frame.rsv3;
                    this.fragment_buffer.clear();
                    this.fragment_buffer.extend_from_slice(&frame.payload);
                    // The read buffer may already contain continuation or
                    // control frames. Parse those before polling the socket
                    // again; an EOF immediately after a complete wire buffer
                    // must not discard the buffered fragments.
                    continue;
                } else {
                    return Poll::Ready(Some(Ok(frame)));
                }
            }

            let mut buf = [0u8; 8192];
            let mut read_buf = ReadBuf::new(&mut buf);

            match this.inner.as_mut().poll_read(cx, &mut read_buf) {
                Poll::Ready(Ok(())) => {
                    let n = read_buf.filled().len();
                    if n == 0 {
                        return Poll::Ready(None);
                    }
                    this.buffer.extend_from_slice(read_buf.filled());
                }
                Poll::Ready(Err(e)) => return Poll::Ready(Some(Err(e))),
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

pub struct WebSocketWriter<W> {
    inner: W,
    is_client: bool,
}

impl<W> WebSocketWriter<W> {
    pub fn new(inner: W, is_client: bool) -> Self {
        Self { inner, is_client }
    }

    pub fn into_inner(self) -> W {
        self.inner
    }
}

impl<W: AsyncWrite + Unpin> WebSocketWriter<W> {
    pub async fn write_frame(&mut self, mut frame: WebSocketFrame) -> std::io::Result<()> {
        if self.is_client && frame.mask.is_none() {
            frame = frame.with_mask(generate_mask());
        }
        let encoded = frame.encode();
        self.inner.write_all(&encoded).await?;
        self.inner.flush().await?;
        Ok(())
    }

    pub async fn write_text(&mut self, text: &str) -> std::io::Result<()> {
        self.write_frame(WebSocketFrame::text(text)).await
    }

    pub async fn write_binary(&mut self, data: &[u8]) -> std::io::Result<()> {
        self.write_frame(WebSocketFrame::binary(data)).await
    }

    pub async fn write_ping(&mut self, data: &[u8]) -> std::io::Result<()> {
        self.write_frame(WebSocketFrame::ping(data)).await
    }

    pub async fn write_pong(&mut self, data: &[u8]) -> std::io::Result<()> {
        self.write_frame(WebSocketFrame::pong(data)).await
    }

    pub async fn write_close(&mut self, code: Option<u16>, reason: &str) -> std::io::Result<()> {
        self.write_frame(WebSocketFrame::close(code, reason)).await
    }
}

fn generate_mask() -> [u8; 4] {
    use std::time::{SystemTime, UNIX_EPOCH};
    let seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u32;
    seed.to_be_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::StreamExt;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn encoded(mut frame: WebSocketFrame) -> Vec<u8> {
        frame.mask = None;
        frame.encode().to_vec()
    }

    #[tokio::test]
    async fn reader_reassembles_fragments_preserves_rsv_and_emits_control_frames() {
        let mut first = WebSocketFrame::text("hel");
        first.fin = false;
        first.rsv1 = true;
        first.rsv2 = true;
        let ping = WebSocketFrame::ping(b"p");
        let continuation = WebSocketFrame {
            fin: true,
            rsv1: false,
            rsv2: false,
            rsv3: false,
            opcode: Opcode::Continuation,
            mask: None,
            payload: Bytes::from_static(b"lo"),
        };
        let mut wire = encoded(first);
        wire.extend(encoded(ping));
        wire.extend(encoded(continuation));
        let mut reader = WebSocketReader::with_initial_buffer(
            tokio::io::empty(),
            BytesMut::from(wire.as_slice()),
        );
        let control = reader.next().await.unwrap().unwrap();
        assert_eq!(control.opcode, Opcode::Ping);
        let complete = reader.next().await.unwrap().unwrap();
        assert_eq!(complete.opcode, Opcode::Text);
        assert_eq!(complete.payload, Bytes::from_static(b"hello"));
        assert!(complete.rsv1);
        assert!(complete.rsv2);
        assert!(reader.next().await.is_none());
        let _ = reader.into_inner();
    }

    #[tokio::test]
    async fn reader_drops_oversized_initial_and_continuation_fragments_then_recovers() {
        let mut oversized = WebSocketFrame::text("toolong");
        oversized.fin = false;
        let valid = WebSocketFrame::text("ok");
        let mut wire = encoded(oversized);
        wire.extend(encoded(valid.clone()));
        let mut reader = WebSocketReader::with_max_fragment_size(&wire[..], 3);
        assert_eq!(reader.next().await.unwrap().unwrap().payload, valid.payload);

        let mut first = WebSocketFrame::text("ab");
        first.fin = false;
        let continuation = WebSocketFrame {
            fin: true,
            rsv1: false,
            rsv2: false,
            rsv3: false,
            opcode: Opcode::Continuation,
            mask: None,
            payload: Bytes::from_static(b"cd"),
        };
        let fallback = WebSocketFrame::binary(b"z");
        let mut wire = encoded(first);
        wire.extend(encoded(continuation));
        wire.extend(encoded(fallback.clone()));
        let mut reader = WebSocketReader::with_max_fragment_size(&wire[..], 3);
        let frame = reader.next().await.unwrap().unwrap();
        assert_eq!(frame.payload, fallback.payload);
    }

    #[tokio::test]
    async fn reader_handles_split_reads_and_propagates_io_errors() {
        let wire = encoded(WebSocketFrame::text("split"));
        let (mut tx, rx) = tokio::io::duplex(64);
        tokio::spawn(async move {
            for byte in wire {
                tx.write_all(&[byte]).await.unwrap();
                tokio::task::yield_now().await;
            }
        });
        let mut reader = WebSocketReader::new(rx);
        assert_eq!(
            reader.next().await.unwrap().unwrap().payload,
            Bytes::from_static(b"split")
        );

        struct ErrorReader;
        impl AsyncRead for ErrorReader {
            fn poll_read(
                self: Pin<&mut Self>,
                _cx: &mut Context<'_>,
                _buf: &mut ReadBuf<'_>,
            ) -> Poll<std::io::Result<()>> {
                Poll::Ready(Err(std::io::Error::other("covered")))
            }
        }
        let mut reader = WebSocketReader::new(ErrorReader);
        assert!(reader.next().await.unwrap().is_err());
    }

    #[tokio::test]
    async fn writer_covers_all_frame_helpers_and_client_masking() {
        let (writer_io, mut reader_io) = tokio::io::duplex(4096);
        let task = tokio::spawn(async move {
            let mut writer = WebSocketWriter::new(writer_io, true);
            writer.write_text("text").await.unwrap();
            writer.write_binary(b"bin").await.unwrap();
            writer.write_ping(b"ping").await.unwrap();
            writer.write_pong(b"pong").await.unwrap();
            writer.write_close(Some(1000), "done").await.unwrap();
            let _ = writer.into_inner();
        });
        let mut bytes = Vec::new();
        reader_io.read_to_end(&mut bytes).await.unwrap();
        task.await.unwrap();
        let mut reader = WebSocketReader::with_initial_buffer(
            tokio::io::empty(),
            BytesMut::from(bytes.as_slice()),
        );
        let mut opcodes = Vec::new();
        while let Some(frame) = reader.next().await {
            opcodes.push(frame.unwrap().opcode);
        }
        assert_eq!(
            opcodes,
            vec![
                Opcode::Text,
                Opcode::Binary,
                Opcode::Ping,
                Opcode::Pong,
                Opcode::Close
            ]
        );
    }
}
