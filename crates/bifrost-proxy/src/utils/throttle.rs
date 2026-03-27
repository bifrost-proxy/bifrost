use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use bytes::Bytes;
use http_body_util::BodyExt;
use hyper::body::{Body, Frame};
use tokio::time::Sleep;

use crate::server::BoxBody;

pub struct ThrottledBoxBody {
    inner: BoxBody,
    bytes_per_second: u64,
    bytes_sent_this_window: u64,
    window_start: Instant,
    sleep: Option<Pin<Box<Sleep>>>,
    pending_data: Option<Bytes>,
}

impl ThrottledBoxBody {
    pub fn new(inner: BoxBody, bytes_per_second: u64) -> Self {
        Self {
            inner,
            bytes_per_second,
            bytes_sent_this_window: 0,
            window_start: Instant::now(),
            sleep: None,
            pending_data: None,
        }
    }

    fn refresh_window(&mut self) {
        if self.window_start.elapsed() >= Duration::from_secs(1) {
            self.bytes_sent_this_window = 0;
            self.window_start = Instant::now();
        }
    }

    fn available_budget(&mut self) -> u64 {
        self.refresh_window();
        self.bytes_per_second
            .saturating_sub(self.bytes_sent_this_window)
    }

    fn schedule_sleep_if_needed(&mut self) {
        if self.bytes_sent_this_window < self.bytes_per_second {
            return;
        }

        let elapsed = self.window_start.elapsed();
        if elapsed >= Duration::from_secs(1) {
            self.bytes_sent_this_window = 0;
            self.window_start = Instant::now();
            self.sleep = None;
            return;
        }

        let sleep_duration = Duration::from_secs(1) - elapsed;
        self.sleep = Some(Box::pin(tokio::time::sleep(sleep_duration)));
    }

    fn poll_pending_data(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, hyper::Error>>> {
        let Some(mut data) = self.pending_data.take() else {
            return Poll::Pending;
        };

        let budget = self.available_budget();
        if budget == 0 {
            self.pending_data = Some(data);
            self.schedule_sleep_if_needed();
            if let Some(ref mut sleep) = self.sleep {
                let _ = sleep.as_mut().poll(cx);
            }
            return Poll::Pending;
        }

        let chunk_len = budget.min(data.len() as u64) as usize;
        let chunk = data.split_to(chunk_len);
        if !data.is_empty() {
            self.pending_data = Some(data);
        }

        self.bytes_sent_this_window += chunk_len as u64;
        let has_more_buffered_or_upstream_data =
            self.pending_data.is_some() || !self.inner.is_end_stream();
        if has_more_buffered_or_upstream_data {
            self.schedule_sleep_if_needed();
        } else {
            self.sleep = None;
        }
        Poll::Ready(Some(Ok(Frame::data(chunk))))
    }
}

impl Body for ThrottledBoxBody {
    type Data = Bytes;
    type Error = hyper::Error;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        if let Some(ref mut sleep) = self.sleep {
            match sleep.as_mut().poll(cx) {
                Poll::Ready(()) => {
                    self.sleep = None;
                    self.bytes_sent_this_window = 0;
                    self.window_start = Instant::now();
                }
                Poll::Pending => return Poll::Pending,
            }
        }

        if self.pending_data.is_some() {
            return self.poll_pending_data(cx);
        }

        match Pin::new(&mut self.inner).poll_frame(cx) {
            Poll::Ready(Some(Ok(frame))) => match frame.into_data() {
                Ok(data) => {
                    self.pending_data = Some(data);
                    self.poll_pending_data(cx)
                }
                Err(frame) => Poll::Ready(Some(Ok(frame))),
            },
            other => other,
        }
    }

    fn is_end_stream(&self) -> bool {
        self.pending_data.is_none() && self.sleep.is_none() && self.inner.is_end_stream()
    }

    fn size_hint(&self) -> hyper::body::SizeHint {
        self.inner.size_hint()
    }
}

pub fn wrap_throttled_body(body: BoxBody, bytes_per_second: Option<u64>) -> BoxBody {
    match bytes_per_second {
        Some(speed) if speed > 0 => BodyExt::boxed(ThrottledBoxBody::new(body, speed)),
        _ => body,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::full_body;

    #[tokio::test]
    async fn throttled_body_splits_large_frames() {
        let body = wrap_throttled_body(full_body(Bytes::from(vec![b'a'; 10])), Some(4));
        let collected = body.collect().await.expect("body should collect");
        let bytes = collected.to_bytes();
        assert_eq!(bytes.len(), 10);
    }

    #[tokio::test(start_paused = true)]
    async fn throttled_body_waits_for_next_window_before_releasing_remaining_data() {
        let mut body = Box::pin(ThrottledBoxBody::new(
            full_body(Bytes::from(vec![b'a'; 10])),
            4,
        ));
        let waker = std::task::Waker::noop();
        let mut cx = Context::from_waker(waker);

        let first = match body.as_mut().poll_frame(&mut cx) {
            Poll::Ready(Some(Ok(frame))) => frame.into_data().expect("data frame"),
            other => panic!("unexpected first poll result: {:?}", other),
        };
        assert_eq!(first.len(), 4);

        assert!(matches!(body.as_mut().poll_frame(&mut cx), Poll::Pending));

        tokio::time::advance(Duration::from_secs(1)).await;

        let second = match body.as_mut().poll_frame(&mut cx) {
            Poll::Ready(Some(Ok(frame))) => frame.into_data().expect("data frame"),
            other => panic!("unexpected second poll result: {:?}", other),
        };
        assert_eq!(second.len(), 4);

        assert!(matches!(body.as_mut().poll_frame(&mut cx), Poll::Pending));

        tokio::time::advance(Duration::from_secs(1)).await;

        let third = match body.as_mut().poll_frame(&mut cx) {
            Poll::Ready(Some(Ok(frame))) => frame.into_data().expect("data frame"),
            other => panic!("unexpected third poll result: {:?}", other),
        };
        assert_eq!(third.len(), 2);

        assert!(matches!(
            body.as_mut().poll_frame(&mut cx),
            Poll::Ready(None)
        ));
    }

    #[tokio::test]
    async fn throttled_body_is_not_end_stream_while_data_is_buffered() {
        let mut body = Box::pin(ThrottledBoxBody::new(
            full_body(Bytes::from(vec![b'a'; 10])),
            4,
        ));
        let waker = std::task::Waker::noop();
        let mut cx = Context::from_waker(waker);

        let first = match body.as_mut().poll_frame(&mut cx) {
            Poll::Ready(Some(Ok(frame))) => frame.into_data().expect("data frame"),
            other => panic!("unexpected first poll result: {:?}", other),
        };
        assert_eq!(first.len(), 4);
        assert!(
            !body.is_end_stream(),
            "body should not report end-of-stream while throttled data is still buffered"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn throttled_body_does_not_wait_an_extra_window_after_last_exact_chunk() {
        let mut body = Box::pin(ThrottledBoxBody::new(
            full_body(Bytes::from(vec![b'a'; 8])),
            4,
        ));
        let waker = std::task::Waker::noop();
        let mut cx = Context::from_waker(waker);

        let first = match body.as_mut().poll_frame(&mut cx) {
            Poll::Ready(Some(Ok(frame))) => frame.into_data().expect("data frame"),
            other => panic!("unexpected first poll result: {:?}", other),
        };
        assert_eq!(first.len(), 4);
        assert!(matches!(body.as_mut().poll_frame(&mut cx), Poll::Pending));

        tokio::time::advance(Duration::from_secs(1)).await;

        let second = match body.as_mut().poll_frame(&mut cx) {
            Poll::Ready(Some(Ok(frame))) => frame.into_data().expect("data frame"),
            other => panic!("unexpected second poll result: {:?}", other),
        };
        assert_eq!(second.len(), 4);
        assert!(
            body.is_end_stream(),
            "exact final chunk should finish the stream immediately"
        );
        assert!(matches!(
            body.as_mut().poll_frame(&mut cx),
            Poll::Ready(None)
        ));
    }

    #[tokio::test(start_paused = true)]
    async fn throttled_body_registers_a_waker_when_waiting_for_next_window() {
        let body = wrap_throttled_body(full_body(Bytes::from(vec![b'a'; 8])), Some(4));
        let handle = tokio::spawn(async move {
            body.collect()
                .await
                .expect("body should collect")
                .to_bytes()
        });

        tokio::task::yield_now().await;
        assert!(
            !handle.is_finished(),
            "body should still be throttled before the next window opens"
        );

        tokio::time::advance(Duration::from_secs(1)).await;
        tokio::task::yield_now().await;

        assert!(
            handle.is_finished(),
            "scheduled sleep should wake the body task once the next window opens"
        );

        let bytes = handle.await.expect("join should succeed");
        assert_eq!(bytes.len(), 8);
    }
}
