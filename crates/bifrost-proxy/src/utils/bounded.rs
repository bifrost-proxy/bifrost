use std::collections::VecDeque;
use std::pin::Pin;
use std::task::{Context, Poll};

use bytes::{Bytes, BytesMut};
use http_body_util::BodyExt;
use hyper::body::{Body, Frame};

pub enum BoundedBody<B> {
    Complete(Bytes),
    Exceeded(PrefixReplayBody<B>),
}

pub struct PrefixReplayBody<B> {
    prefix: VecDeque<Frame<Bytes>>,
    inner: Pin<Box<B>>,
}

impl<B> PrefixReplayBody<B> {
    pub fn new(prefix: VecDeque<Frame<Bytes>>, inner: B) -> Self {
        Self {
            prefix,
            inner: Box::pin(inner),
        }
    }

    pub fn boxed(self) -> crate::server::BoxBody
    where
        B: Body<Data = Bytes, Error = hyper::Error> + Send + Sync + 'static,
    {
        http_body_util::BodyExt::boxed(self)
    }
}

impl<B> Body for PrefixReplayBody<B>
where
    B: Body<Data = Bytes, Error = hyper::Error>,
{
    type Data = Bytes;
    type Error = hyper::Error;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        if let Some(frame) = self.prefix.pop_front() {
            return Poll::Ready(Some(Ok(frame)));
        }
        self.inner.as_mut().poll_frame(cx)
    }

    fn is_end_stream(&self) -> bool {
        self.prefix.is_empty() && self.inner.is_end_stream()
    }

    fn size_hint(&self) -> hyper::body::SizeHint {
        self.inner.size_hint()
    }
}

pub async fn read_body_bounded<B>(
    mut body: B,
    max_bytes: usize,
) -> Result<BoundedBody<B>, hyper::Error>
where
    B: Body<Data = Bytes, Error = hyper::Error> + Unpin + Send + Sync + 'static,
{
    let mut frames: VecDeque<Frame<Bytes>> = VecDeque::new();
    let mut seen: usize = 0;

    while let Some(frame) = body.frame().await {
        let frame = frame?;
        if let Some(data) = frame.data_ref() {
            seen = seen.saturating_add(data.len());
        }
        frames.push_back(frame);
        if seen > max_bytes {
            return Ok(BoundedBody::Exceeded(PrefixReplayBody::new(frames, body)));
        }
    }

    let mut buf = BytesMut::with_capacity(seen);
    for frame in &frames {
        if let Some(data) = frame.data_ref() {
            buf.extend_from_slice(data);
        }
    }
    Ok(BoundedBody::Complete(buf.freeze()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use http_body_util::BodyExt;
    use hyper::body::{Body, Frame, SizeHint};
    use std::collections::VecDeque;
    use std::pin::Pin;
    use std::task::{Context, Poll};

    #[derive(Debug)]
    struct TestBody {
        frames: VecDeque<Bytes>,
    }

    impl TestBody {
        fn from_chunks(chunks: &[&[u8]]) -> Self {
            let frames = chunks
                .iter()
                .map(|c| Bytes::copy_from_slice(c))
                .collect::<VecDeque<_>>();
            Self { frames }
        }
    }

    impl Body for TestBody {
        type Data = Bytes;
        type Error = hyper::Error;

        fn poll_frame(
            mut self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
        ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
            if let Some(chunk) = self.frames.pop_front() {
                return Poll::Ready(Some(Ok(Frame::data(chunk))));
            }
            Poll::Ready(None)
        }

        fn is_end_stream(&self) -> bool {
            self.frames.is_empty()
        }

        fn size_hint(&self) -> SizeHint {
            SizeHint::default()
        }
    }

    #[tokio::test]
    async fn read_body_bounded_collects_complete_body_when_under_limit() {
        let body = TestBody::from_chunks(&[b"hello", b" ", b"world"]);
        let result = read_body_bounded(body, 1024).await.unwrap();

        match result {
            BoundedBody::Complete(bytes) => {
                assert_eq!(bytes.as_ref(), b"hello world");
            }
            BoundedBody::Exceeded(_) => panic!("expected Complete variant"),
        }
    }

    #[tokio::test]
    async fn read_body_bounded_returns_prefix_replay_when_exceeded() {
        let body = TestBody::from_chunks(&[b"hello", b"world"]);
        let result = read_body_bounded(body, 5).await.unwrap();

        let prefix_body = match result {
            BoundedBody::Exceeded(prefix) => prefix,
            BoundedBody::Complete(_) => panic!("expected Exceeded variant"),
        };

        // PrefixReplayBody should replay the frames we saw before exceeding the limit.
        let mut collected = Vec::new();
        let mut replay = prefix_body;
        while let Some(frame) = replay.frame().await {
            let frame = frame.expect("frame ok");
            if let Some(data) = frame.data_ref() {
                collected.extend_from_slice(data);
            }
        }

        assert_eq!(collected.as_slice(), b"helloworld");
    }
}
