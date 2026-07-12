use std::pin::Pin;
use std::task::{Context, Poll};

use bytes::{Bytes, BytesMut};
use futures_util::Stream;
use pin_project_lite::pin_project;
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt, ReadBuf};

#[derive(Debug, Clone)]
pub struct SseEvent {
    pub id: Option<String>,
    pub event: Option<String>,
    pub data: String,
    pub retry: Option<u64>,
}

impl SseEvent {
    pub fn new(data: impl Into<String>) -> Self {
        Self {
            id: None,
            event: None,
            data: data.into(),
            retry: None,
        }
    }

    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }

    pub fn with_event(mut self, event: impl Into<String>) -> Self {
        self.event = Some(event.into());
        self
    }

    pub fn with_retry(mut self, retry: u64) -> Self {
        self.retry = Some(retry);
        self
    }

    pub fn encode(&self) -> Bytes {
        let mut buf = BytesMut::new();

        if let Some(ref id) = self.id {
            buf.extend_from_slice(b"id: ");
            buf.extend_from_slice(id.as_bytes());
            buf.extend_from_slice(b"\n");
        }

        if let Some(ref event) = self.event {
            buf.extend_from_slice(b"event: ");
            buf.extend_from_slice(event.as_bytes());
            buf.extend_from_slice(b"\n");
        }

        if let Some(retry) = self.retry {
            buf.extend_from_slice(b"retry: ");
            buf.extend_from_slice(retry.to_string().as_bytes());
            buf.extend_from_slice(b"\n");
        }

        for line in self.data.lines() {
            buf.extend_from_slice(b"data: ");
            buf.extend_from_slice(line.as_bytes());
            buf.extend_from_slice(b"\n");
        }

        buf.extend_from_slice(b"\n");
        buf.freeze()
    }

    pub fn parse(raw: &str) -> Option<Self> {
        let mut id = None;
        let mut event = None;
        let mut data_lines = Vec::new();
        let mut retry = None;

        for line in raw.lines() {
            if line.is_empty() {
                continue;
            }

            if let Some(value) = line.strip_prefix("id:") {
                id = Some(value.trim().to_string());
            } else if let Some(value) = line.strip_prefix("event:") {
                event = Some(value.trim().to_string());
            } else if let Some(value) = line.strip_prefix("data:") {
                data_lines.push(value.trim().to_string());
            } else if let Some(value) = line.strip_prefix("retry:") {
                retry = value.trim().parse().ok();
            } else if line.starts_with(':') {
            }
        }

        if data_lines.is_empty() {
            return None;
        }

        Some(Self {
            id,
            event,
            data: data_lines.join("\n"),
            retry,
        })
    }
}

pin_project! {
    pub struct SseReader<R> {
        #[pin]
        inner: R,
        buffer: BytesMut,
        last_event_id: Option<String>,
    }
}

impl<R> SseReader<R> {
    pub fn new(inner: R) -> Self {
        Self {
            inner,
            buffer: BytesMut::with_capacity(8192),
            last_event_id: None,
        }
    }

    pub fn last_event_id(&self) -> Option<&str> {
        self.last_event_id.as_deref()
    }

    pub fn into_inner(self) -> R {
        self.inner
    }
}

impl<R: AsyncRead + Unpin> Stream for SseReader<R> {
    type Item = std::io::Result<SseEvent>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let mut this = self.project();

        loop {
            if let Some(event) = try_parse_event(this.buffer, this.last_event_id) {
                return Poll::Ready(Some(Ok(event)));
            }

            let mut buf = [0u8; 4096];
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

fn try_parse_event(buffer: &mut BytesMut, last_event_id: &mut Option<String>) -> Option<SseEvent> {
    if let Some(pos) = find_event_boundary(buffer) {
        let event_data = buffer.split_to(pos + 2);
        let event_str = String::from_utf8_lossy(&event_data);

        if let Some(event) = SseEvent::parse(&event_str) {
            if let Some(ref id) = event.id {
                *last_event_id = Some(id.clone());
            }
            return Some(event);
        }
    }
    None
}

fn find_event_boundary(buffer: &BytesMut) -> Option<usize> {
    (0..buffer.len().saturating_sub(1)).find(|&i| buffer[i] == b'\n' && buffer[i + 1] == b'\n')
}

pub struct SseWriter<W> {
    inner: W,
}

impl<W> SseWriter<W> {
    pub fn new(inner: W) -> Self {
        Self { inner }
    }

    pub fn into_inner(self) -> W {
        self.inner
    }
}

impl<W: AsyncWrite + Unpin> SseWriter<W> {
    pub async fn write_event(&mut self, event: &SseEvent) -> std::io::Result<()> {
        let encoded = event.encode();
        self.inner.write_all(&encoded).await?;
        self.inner.flush().await?;
        Ok(())
    }

    pub async fn write_comment(&mut self, comment: &str) -> std::io::Result<()> {
        let mut buf = BytesMut::new();
        for line in comment.lines() {
            buf.extend_from_slice(b": ");
            buf.extend_from_slice(line.as_bytes());
            buf.extend_from_slice(b"\n");
        }
        self.inner.write_all(&buf).await?;
        self.inner.flush().await?;
        Ok(())
    }

    pub async fn send_keepalive(&mut self) -> std::io::Result<()> {
        self.inner.write_all(b": keepalive\n\n").await?;
        self.inner.flush().await?;
        Ok(())
    }
}

pub struct SseForwarder;

pub type SseEventCallback = Box<dyn Fn(&SseEvent) -> Option<SseEvent> + Send + Sync>;

impl SseForwarder {
    pub async fn forward<R, W>(
        mut reader: R,
        mut writer: W,
        on_event: Option<SseEventCallback>,
    ) -> std::io::Result<u64>
    where
        R: AsyncRead + Unpin,
        W: AsyncWrite + Unpin,
    {
        use futures_util::StreamExt;

        let mut sse_reader = SseReader::new(&mut reader);
        let mut sse_writer = SseWriter::new(&mut writer);
        let mut count = 0u64;

        while let Some(result) = sse_reader.next().await {
            let event = result?;

            let event_to_write = if let Some(ref transform) = on_event {
                transform(&event)
            } else {
                Some(event)
            };

            if let Some(event) = event_to_write {
                sse_writer.write_event(&event).await?;
                count += 1;
            }
        }

        Ok(count)
    }

    pub async fn forward_raw<R, W>(
        mut reader: R,
        mut writer: W,
        buffer_size: usize,
    ) -> std::io::Result<u64>
    where
        R: AsyncRead + Unpin,
        W: AsyncWrite + Unpin,
    {
        use tokio::io::AsyncReadExt;

        let mut buf = vec![0u8; buffer_size];
        let mut total = 0u64;

        loop {
            let n = reader.read(&mut buf).await?;
            if n == 0 {
                break;
            }
            writer.write_all(&buf[..n]).await?;
            writer.flush().await?;
            total += n as u64;
        }

        Ok(total)
    }
}

pub fn build_sse_response_headers() -> Vec<(&'static str, &'static str)> {
    vec![
        ("Content-Type", "text/event-stream"),
        ("Cache-Control", "no-cache"),
        ("Connection", "keep-alive"),
        ("X-Accel-Buffering", "no"),
    ]
}

pub fn is_sse_request(accept_header: &str) -> bool {
    accept_header.contains("text/event-stream")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sse_event_encode() {
        let event = SseEvent::new("hello world")
            .with_id("1")
            .with_event("message");

        let encoded = event.encode();
        let expected = "id: 1\nevent: message\ndata: hello world\n\n";
        assert_eq!(String::from_utf8_lossy(&encoded), expected);
    }

    #[test]
    fn test_sse_event_encode_multiline() {
        let event = SseEvent::new("line1\nline2\nline3");

        let encoded = event.encode();
        let expected = "data: line1\ndata: line2\ndata: line3\n\n";
        assert_eq!(String::from_utf8_lossy(&encoded), expected);
    }

    #[test]
    fn test_sse_event_encode_with_retry() {
        let event = SseEvent::new("data").with_retry(5000);

        let encoded = event.encode();
        let expected = "retry: 5000\ndata: data\n\n";
        assert_eq!(String::from_utf8_lossy(&encoded), expected);
    }

    #[test]
    fn test_sse_event_parse() {
        let raw = "id: 1\nevent: message\ndata: hello world";
        let event = SseEvent::parse(raw).unwrap();

        assert_eq!(event.id, Some("1".to_string()));
        assert_eq!(event.event, Some("message".to_string()));
        assert_eq!(event.data, "hello world");
    }

    #[test]
    fn test_sse_event_parse_multiline() {
        let raw = "data: line1\ndata: line2\ndata: line3";
        let event = SseEvent::parse(raw).unwrap();

        assert_eq!(event.data, "line1\nline2\nline3");
    }

    #[test]
    fn test_sse_event_parse_with_retry() {
        let raw = "retry: 5000\ndata: hello";
        let event = SseEvent::parse(raw).unwrap();

        assert_eq!(event.retry, Some(5000));
        assert_eq!(event.data, "hello");
    }

    #[test]
    fn test_sse_event_parse_with_comment() {
        let raw = ": this is a comment\ndata: hello";
        let event = SseEvent::parse(raw).unwrap();

        assert_eq!(event.data, "hello");
    }

    #[test]
    fn test_sse_event_parse_no_data() {
        let raw = "id: 1\nevent: message";
        let event = SseEvent::parse(raw);

        assert!(event.is_none());
    }

    #[test]
    fn test_is_sse_request() {
        assert!(is_sse_request("text/event-stream"));
        assert!(is_sse_request("text/event-stream, application/json"));
        assert!(!is_sse_request("application/json"));
    }

    #[test]
    fn test_build_sse_response_headers() {
        let headers = build_sse_response_headers();
        assert!(headers
            .iter()
            .any(|(k, v)| *k == "Content-Type" && *v == "text/event-stream"));
        assert!(headers
            .iter()
            .any(|(k, v)| *k == "Cache-Control" && *v == "no-cache"));
        assert!(headers
            .iter()
            .any(|(k, v)| *k == "Connection" && *v == "keep-alive"));
    }

    #[test]
    fn test_find_event_boundary() {
        let buffer = BytesMut::from("data: hello\n\ndata: world");
        let pos = find_event_boundary(&buffer);
        assert_eq!(pos, Some(11));
    }

    #[test]
    fn test_find_event_boundary_not_found() {
        let buffer = BytesMut::from("data: hello\n");
        let pos = find_event_boundary(&buffer);
        assert!(pos.is_none());
    }

    #[tokio::test]
    async fn test_sse_reader() {
        use futures_util::StreamExt;

        let data = b"data: hello\n\ndata: world\n\n";
        let cursor = std::io::Cursor::new(data.to_vec());
        let tokio_cursor = tokio_util::compat::FuturesAsyncReadCompatExt::compat(
            futures_util::io::AllowStdIo::new(cursor),
        );

        let mut reader = SseReader::new(tokio_cursor);
        let mut events = Vec::new();

        while let Some(result) = reader.next().await {
            events.push(result.unwrap());
        }

        assert_eq!(events.len(), 2);
        assert_eq!(events[0].data, "hello");
        assert_eq!(events[1].data, "world");
    }

    #[tokio::test]
    async fn test_sse_writer() {
        let mut output = Vec::new();
        {
            let mut writer = SseWriter::new(&mut output);

            let event = SseEvent::new("hello").with_id("1");
            writer.write_event(&event).await.unwrap();
        }

        let result = String::from_utf8(output).unwrap();
        assert!(result.contains("id: 1"));
        assert!(result.contains("data: hello"));
    }

    #[tokio::test]
    async fn test_sse_writer_comment() {
        let mut output = Vec::new();
        {
            let mut writer = SseWriter::new(&mut output);
            writer.write_comment("keepalive").await.unwrap();
        }

        let result = String::from_utf8(output).unwrap();
        assert_eq!(result, ": keepalive\n");
    }

    #[tokio::test]
    async fn test_sse_writer_keepalive() {
        let mut output = Vec::new();
        {
            let mut writer = SseWriter::new(&mut output);
            writer.send_keepalive().await.unwrap();
        }

        let result = String::from_utf8(output).unwrap();
        assert_eq!(result, ": keepalive\n\n");
    }

    #[tokio::test]
    async fn coverage_90_forward_transforms_filters_and_tracks_events() {
        use tokio::io::AsyncReadExt;

        let input = b"id: 1\ndata: first\n\nid: 2\ndata: second\n\n".to_vec();
        let (mut reader, mut input_writer) = tokio::io::duplex(256);
        tokio::spawn(async move {
            input_writer.write_all(&input).await.unwrap();
        });
        let (output_writer, mut output_reader) = tokio::io::duplex(256);
        let count = SseForwarder::forward(
            &mut reader,
            output_writer,
            Some(Box::new(|event| {
                (event.id.as_deref() == Some("2"))
                    .then(|| SseEvent::new(event.data.to_uppercase()).with_id("changed"))
            })),
        )
        .await
        .unwrap();
        assert_eq!(count, 1);
        let mut output = Vec::new();
        output_reader.read_to_end(&mut output).await.unwrap();
        assert!(String::from_utf8_lossy(&output).contains("data: SECOND"));

        let reader = SseReader::new(tokio::io::empty());
        assert_eq!(reader.last_event_id(), None);
        let _ = reader.into_inner();
        let writer = SseWriter::new(tokio::io::sink());
        let _ = writer.into_inner();
    }

    #[tokio::test]
    async fn coverage_90_forward_raw_handles_multiple_chunks() {
        use tokio::io::AsyncReadExt;
        let (writer, mut reader) = tokio::io::duplex(64);
        let total = SseForwarder::forward_raw(&b"abcdef"[..], writer, 2)
            .await
            .unwrap();
        assert_eq!(total, 6);
        let mut output = Vec::new();
        reader.read_to_end(&mut output).await.unwrap();
        assert_eq!(output, b"abcdef");
    }
}
