use std::future::Future;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::pin::Pin;
use std::sync::{Arc, Mutex, OnceLock};
use std::task::{Context, Poll};

use bifrost_admin::{
    assemble_openai_like_response_body_from_text, AdminState, BodyRef, BodyStreamWriter,
    FrameDirection, SharedBodyStore, TrafficType, MAX_OPENAI_LIKE_SSE_ASSEMBLY_INPUT_BYTES,
};
use bytes::{Bytes, BytesMut};
use http_body_util::BodyExt;
use hyper::body::{Body, Frame, Incoming};
use memchr::memchr;
use tokio::sync::Semaphore;
use tokio::time::Sleep;

use crate::server::BoxBody;
use crate::transform::decompress::decompress_body_with_limit;

// Keep hot-path metrics updates coarse-grained so high-throughput relays do
// not burn CPU on bookkeeping.
const BODY_TRAFFIC_FLUSH_BYTES: usize = 1024 * 1024;
const MAX_DERIVED_OPENAI_LIKE_SSE_BODY_BYTES: usize = MAX_OPENAI_LIKE_SSE_ASSEMBLY_INPUT_BYTES;
const DEFAULT_BODY_STORE_BACKGROUND_CONCURRENCY: usize = 1;

fn record_first_downstream_byte(state: &AdminState, record_id: &str) {
    state.update_traffic_by_id(record_id, move |record| {
        let now_ms = chrono::Utc::now().timestamp_millis().max(0) as u64;
        let first_byte_ms = now_ms.saturating_sub(record.timestamp);
        if let Some(ref mut timing) = record.timing {
            if timing.first_byte_ms.is_none() {
                timing.first_byte_ms = Some(first_byte_ms);
            }
        }
    });
}

fn store_body_sync(
    body_store: &SharedBodyStore,
    record_id: &str,
    kind: &str,
    data: &[u8],
) -> Option<BodyRef> {
    body_store.read().store(record_id, kind, data)
}

fn body_store_background_semaphore() -> Arc<Semaphore> {
    static SEMAPHORE: OnceLock<Arc<Semaphore>> = OnceLock::new();
    SEMAPHORE
        .get_or_init(|| {
            let permits = std::env::var("BIFROST_BODY_STORE_BACKGROUND_CONCURRENCY")
                .ok()
                .and_then(|value| value.parse::<usize>().ok())
                .filter(|value| *value > 0)
                .unwrap_or(DEFAULT_BODY_STORE_BACKGROUND_CONCURRENCY);
            Arc::new(Semaphore::new(permits))
        })
        .clone()
}

fn schedule_decompressed_response_body_store(
    state: Arc<AdminState>,
    body_store: SharedBodyStore,
    record_id: String,
    body: Vec<u8>,
) -> Option<BodyRef> {
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        return store_body_sync(&body_store, &record_id, "res", &body);
    };

    handle.spawn(async move {
        let semaphore = body_store_background_semaphore();
        let _permit = semaphore.acquire_owned().await.ok();
        let record_id_for_store = record_id.clone();
        let body_ref = tokio::task::spawn_blocking(move || {
            store_body_sync(&body_store, &record_id_for_store, "res", &body)
        })
        .await
        .ok()
        .flatten();

        if let Some(body_ref) = body_ref {
            state.update_traffic_by_id(&record_id, move |record| {
                record.response_body_ref = Some(body_ref.clone());
            });
        }
    });

    None
}

fn store_response_body_or_schedule(
    state: Arc<AdminState>,
    body_store: SharedBodyStore,
    record_id: String,
    body: Vec<u8>,
    content_encoding: Option<String>,
    max_decompress_output_bytes: usize,
) -> Option<BodyRef> {
    let decompressed = decompress_body_with_limit(
        &body,
        content_encoding.as_deref(),
        max_decompress_output_bytes,
    );
    if let Some(store) = body_store.try_read() {
        return store.store(&record_id, "res", decompressed.as_ref());
    }

    schedule_decompressed_response_body_store(
        state,
        body_store,
        record_id,
        decompressed.as_ref().to_vec(),
    )
}

fn start_body_stream(
    body_store: &SharedBodyStore,
    record_id: &str,
    kind: &str,
) -> std::io::Result<BodyStreamWriter> {
    body_store.read().start_stream(record_id, kind)
}

fn persist_socket_summary(state: &AdminState, record_id: &str, total_bytes: usize) {
    let status = state.sse_hub.get_socket_status(record_id).map(|mut s| {
        s.is_open = false;
        s
    });
    let frame_count = status.as_ref().map(|s| s.frame_count).unwrap_or(0);
    let last_frame_id = frame_count as u64;
    let mut response_size = status.as_ref().map(|s| s.receive_bytes).unwrap_or(0) as usize;
    if response_size == 0 {
        response_size = total_bytes;
    }
    state.update_traffic_by_id(record_id, move |record| {
        record.response_size = response_size;
        record.download_bytes = response_size;
        record.frame_count = frame_count;
        record.last_frame_id = last_frame_id;
        if let Some(ref s) = status {
            record.socket_status = Some(s.clone());
        }
    });
}

fn derive_openai_like_sse_body_ref(
    state: &AdminState,
    record_id: &str,
    response_body_ref: &Option<BodyRef>,
) -> Option<BodyRef> {
    let body_ref = response_body_ref.as_ref()?;
    if body_ref.size() > MAX_DERIVED_OPENAI_LIKE_SSE_BODY_BYTES {
        tracing::debug!(
            record_id,
            body_size = body_ref.size(),
            max_size = MAX_DERIVED_OPENAI_LIKE_SSE_BODY_BYTES,
            "Skipping OpenAI-like SSE body derivation because payload exceeded limit"
        );
        return None;
    }
    let body_store = state.body_store.as_ref()?;
    let raw_body = body_store.read().load(body_ref)?;
    let assembled = match catch_unwind(AssertUnwindSafe(|| {
        assemble_openai_like_response_body_from_text(&raw_body)
    })) {
        Ok(Some(body)) => body,
        Ok(None) => return None,
        Err(_) => {
            tracing::warn!(
                record_id,
                "OpenAI-like SSE body derivation panicked; falling back to raw body only"
            );
            return None;
        }
    };
    body_store
        .read()
        .store(record_id, "res_openai_like", assembled.as_bytes())
}

struct TeeBodyDropGuard {
    admin_state: Option<Arc<AdminState>>,
    record_id: String,
    total_bytes: usize,
    pending_traffic_bytes: usize,
    finished: bool,
    buffer: BytesMut,
    max_body_size: usize,
    content_encoding: Option<String>,
    traffic_type: Option<TrafficType>,
    monitor_connection: bool,
    response_headers_size: usize,
    file_writer: Option<BodyStreamWriter>,
}

impl Drop for TeeBodyDropGuard {
    fn drop(&mut self) {
        if !self.finished {
            self.store_body_and_update_record();
        }
    }
}

impl TeeBodyDropGuard {
    fn flush_pending_traffic(&mut self) {
        if self.pending_traffic_bytes == 0 {
            return;
        }

        if let Some(ref state) = self.admin_state {
            let bytes = self.pending_traffic_bytes as u64;
            if let Some(traffic_type) = self.traffic_type {
                state
                    .metrics_collector
                    .add_bytes_received_by_type(traffic_type, bytes);
            } else {
                state.metrics_collector.add_bytes_received(bytes);
            }
            if self.monitor_connection {
                state.connection_monitor.update_traffic(
                    &self.record_id,
                    FrameDirection::Receive,
                    bytes,
                );
            }
        }

        self.pending_traffic_bytes = 0;
    }

    fn store_body_and_update_record(&mut self) {
        self.flush_pending_traffic();
        if let Some(ref state) = self.admin_state {
            let response_body_ref = if let Some(writer) = self.file_writer.take() {
                Some(writer.finish())
            } else if !self.buffer.is_empty() {
                if let Some(ref body_store) = state.body_store {
                    let max_decompress_output_bytes = state
                        .config_manager
                        .as_ref()
                        .and_then(|cm| cm.try_config())
                        .map(|cfg| cfg.sandbox.limits.max_decompress_output_bytes)
                        .unwrap_or(10 * 1024 * 1024);
                    store_response_body_or_schedule(
                        state.clone(),
                        body_store.clone(),
                        self.record_id.clone(),
                        self.buffer.split().freeze().to_vec(),
                        self.content_encoding.clone(),
                        max_decompress_output_bytes,
                    )
                } else {
                    None
                }
            } else {
                None
            };

            let body_bytes = self.total_bytes;
            let total_bytes = body_bytes + self.response_headers_size;
            state.update_traffic_by_id(&self.record_id, move |record| {
                record.response_size = total_bytes;
                record.download_bytes = body_bytes;
                if let Some(ref mut timing) = record.timing {
                    if timing.first_byte_ms.is_none() {
                        timing.first_byte_ms = Some(record.duration_ms);
                    }
                }
                if response_body_ref.is_some() {
                    record.response_body_ref = response_body_ref.clone();
                }
            });

            if self.monitor_connection {
                let socket_status = state.connection_monitor.close_connection(
                    &self.record_id,
                    None,
                    None,
                    state.frame_store.as_ref(),
                    state.ws_payload_store.as_ref(),
                );

                if let Some(socket_status) = socket_status {
                    let record_id = self.record_id.clone();
                    state.update_traffic_by_id(&record_id, move |record| {
                        record.socket_status = Some(socket_status.clone());
                    });
                }
            }
        }
        self.finished = true;
    }
}

struct TeeBody<B> {
    inner: Pin<Box<B>>,
    guard: TeeBodyDropGuard,
}

const DEFAULT_MAX_BODY_BUFFER_SIZE: usize = 10 * 1024 * 1024;

pub struct TeeBodyCaptureOptions {
    pub max_body_size: Option<usize>,
    pub content_encoding: Option<String>,
    pub traffic_type: Option<TrafficType>,
    pub monitor_connection: bool,
    pub response_headers_size: usize,
}

impl<B> TeeBody<B> {
    pub fn new(
        inner: B,
        admin_state: Option<Arc<AdminState>>,
        record_id: String,
        options: TeeBodyCaptureOptions,
    ) -> Self {
        let max_size = options
            .max_body_size
            .unwrap_or(DEFAULT_MAX_BODY_BUFFER_SIZE);
        Self {
            inner: Box::pin(inner),
            guard: TeeBodyDropGuard {
                admin_state,
                record_id,
                total_bytes: 0,
                pending_traffic_bytes: 0,
                finished: false,
                buffer: BytesMut::with_capacity(8192),
                max_body_size: max_size,
                content_encoding: options.content_encoding,
                traffic_type: options.traffic_type,
                monitor_connection: options.monitor_connection,
                response_headers_size: options.response_headers_size,
                file_writer: None,
            },
        }
    }

    pub fn boxed(self) -> BoxBody
    where
        B: Body<Data = Bytes, Error = hyper::Error> + Send + Sync + 'static,
    {
        BodyExt::boxed(self)
    }
}

impl<B> Body for TeeBody<B>
where
    B: Body<Data = Bytes, Error = hyper::Error>,
{
    type Data = Bytes;
    type Error = hyper::Error;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        if self.guard.finished {
            return Poll::Ready(None);
        }

        match self.inner.as_mut().poll_frame(cx) {
            Poll::Ready(Some(Ok(frame))) => {
                if let Some(data) = frame.data_ref() {
                    let had_body_bytes = self.guard.total_bytes > 0;
                    let len = data.len();
                    self.guard.total_bytes += len;
                    self.guard.pending_traffic_bytes =
                        self.guard.pending_traffic_bytes.saturating_add(len);
                    if !had_body_bytes && len > 0 {
                        if let Some(ref state) = self.guard.admin_state {
                            record_first_downstream_byte(state, &self.guard.record_id);
                        }
                    }

                    let mut new_writer: Option<BodyStreamWriter> = None;
                    if self.guard.file_writer.is_none()
                        && self.guard.buffer.len() + len > self.guard.max_body_size
                    {
                        if let Some(ref state) = self.guard.admin_state {
                            if let Some(ref body_store) = state.body_store {
                                new_writer =
                                    start_body_stream(body_store, &self.guard.record_id, "res")
                                        .ok();
                            }
                        }
                    }

                    if let Some(mut writer) = new_writer {
                        if !self.guard.buffer.is_empty() {
                            let _ = writer.write_chunk(&self.guard.buffer);
                            self.guard.buffer.clear();
                        }
                        let _ = writer.write_chunk(data);
                        self.guard.file_writer = Some(writer);
                    } else if self.guard.file_writer.is_some() {
                        if let Some(writer) = self.guard.file_writer.as_mut() {
                            let _ = writer.write_chunk(data);
                        }
                    } else if self.guard.buffer.len() + len <= self.guard.max_body_size {
                        self.guard.buffer.extend_from_slice(data);
                    } else {
                        self.guard.buffer.clear();
                    }

                    if self.guard.pending_traffic_bytes >= BODY_TRAFFIC_FLUSH_BYTES {
                        self.guard.flush_pending_traffic();
                    }
                }
                Poll::Ready(Some(Ok(frame)))
            }
            Poll::Ready(Some(Err(e))) => {
                self.guard.finished = true;
                self.guard.store_body_and_update_record();
                Poll::Ready(Some(Err(e)))
            }
            Poll::Ready(None) => {
                self.guard.finished = true;
                self.guard.store_body_and_update_record();
                Poll::Ready(None)
            }
            Poll::Pending => Poll::Pending,
        }
    }

    fn is_end_stream(&self) -> bool {
        self.guard.finished || self.inner.is_end_stream()
    }

    fn size_hint(&self) -> hyper::body::SizeHint {
        self.inner.size_hint()
    }
}

pub fn create_tee_body_with_store(
    body: impl Body<Data = Bytes, Error = hyper::Error> + Send + Sync + 'static,
    admin_state: Option<Arc<AdminState>>,
    record_id: String,
    options: TeeBodyCaptureOptions,
) -> BoxBody {
    TeeBody::new(body, admin_state, record_id, options).boxed()
}

struct MetricsBodyDropGuard {
    admin_state: Option<Arc<AdminState>>,
    traffic_type: Option<TrafficType>,
    pending_traffic_bytes: usize,
}

impl MetricsBodyDropGuard {
    fn flush_pending_traffic(&mut self) {
        if self.pending_traffic_bytes == 0 {
            return;
        }

        if let Some(ref state) = self.admin_state {
            let bytes = self.pending_traffic_bytes as u64;
            if let Some(traffic_type) = self.traffic_type {
                state
                    .metrics_collector
                    .add_bytes_received_by_type(traffic_type, bytes);
            } else {
                state.metrics_collector.add_bytes_received(bytes);
            }
        }

        self.pending_traffic_bytes = 0;
    }
}

impl Drop for MetricsBodyDropGuard {
    fn drop(&mut self) {
        self.flush_pending_traffic();
    }
}

struct MetricsBody<B> {
    inner: Pin<Box<B>>,
    guard: MetricsBodyDropGuard,
}

impl<B> MetricsBody<B> {
    fn new(
        inner: B,
        admin_state: Option<Arc<AdminState>>,
        traffic_type: Option<TrafficType>,
    ) -> Self {
        Self {
            inner: Box::pin(inner),
            guard: MetricsBodyDropGuard {
                admin_state,
                traffic_type,
                pending_traffic_bytes: 0,
            },
        }
    }

    fn boxed(self) -> BoxBody
    where
        B: Body<Data = Bytes, Error = hyper::Error> + Send + Sync + 'static,
    {
        BodyExt::boxed(self)
    }
}

impl<B> Body for MetricsBody<B>
where
    B: Body<Data = Bytes, Error = hyper::Error>,
{
    type Data = Bytes;
    type Error = hyper::Error;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        match self.inner.as_mut().poll_frame(cx) {
            Poll::Ready(Some(Ok(frame))) => {
                if let Some(data) = frame.data_ref() {
                    self.guard.pending_traffic_bytes =
                        self.guard.pending_traffic_bytes.saturating_add(data.len());
                    if self.guard.pending_traffic_bytes >= BODY_TRAFFIC_FLUSH_BYTES {
                        self.guard.flush_pending_traffic();
                    }
                }
                Poll::Ready(Some(Ok(frame)))
            }
            Poll::Ready(Some(Err(e))) => {
                self.guard.flush_pending_traffic();
                Poll::Ready(Some(Err(e)))
            }
            Poll::Ready(None) => {
                self.guard.flush_pending_traffic();
                Poll::Ready(None)
            }
            Poll::Pending => Poll::Pending,
        }
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> hyper::body::SizeHint {
        self.inner.size_hint()
    }
}

pub fn create_metrics_body(
    body: impl Body<Data = Bytes, Error = hyper::Error> + Send + Sync + 'static,
    admin_state: Option<Arc<AdminState>>,
    traffic_type: Option<TrafficType>,
) -> BoxBody {
    MetricsBody::new(body, admin_state, traffic_type).boxed()
}

#[derive(Clone)]
pub struct BodyCaptureHandle {
    body_ref: Arc<Mutex<Option<BodyRef>>>,
}

impl BodyCaptureHandle {
    pub fn take(&self) -> Option<BodyRef> {
        self.body_ref.lock().ok()?.take()
    }

    pub fn clone_ref(&self) -> Option<BodyRef> {
        self.body_ref.lock().ok()?.clone()
    }
}

struct RequestTeeBodyDropGuard {
    admin_state: Option<Arc<AdminState>>,
    record_id: String,
    file_writer: Option<BodyStreamWriter>,
    capture: BodyCaptureHandle,
}

impl Drop for RequestTeeBodyDropGuard {
    fn drop(&mut self) {
        if let Some(writer) = self.file_writer.take() {
            let body_ref = writer.finish();
            if let Ok(mut slot) = self.capture.body_ref.lock() {
                *slot = Some(body_ref);
            }
            if let Some(ref state) = self.admin_state {
                let capture = self.capture.clone();
                state.update_traffic_by_id(&self.record_id, move |record| {
                    if let Some(body_ref) = capture.take() {
                        record.request_body_ref = Some(body_ref);
                    }
                });
            }
        }
    }
}

pub struct RequestTeeBody {
    inner: Pin<Box<dyn Body<Data = Bytes, Error = hyper::Error> + Send + Sync>>,
    guard: RequestTeeBodyDropGuard,
}

impl Body for RequestTeeBody {
    type Data = Bytes;
    type Error = hyper::Error;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        match self.inner.as_mut().poll_frame(cx) {
            Poll::Ready(Some(Ok(frame))) => {
                if let Some(data) = frame.data_ref() {
                    if self.guard.file_writer.is_none() {
                        let mut new_writer: Option<BodyStreamWriter> = None;
                        if let Some(ref state) = self.guard.admin_state {
                            if let Some(ref body_store) = state.body_store {
                                new_writer =
                                    start_body_stream(body_store, &self.guard.record_id, "req")
                                        .ok();
                            }
                        }
                        if let Some(writer) = new_writer {
                            self.guard.file_writer = Some(writer);
                        }
                    }
                    if let Some(writer) = self.guard.file_writer.as_mut() {
                        let _ = writer.write_chunk(data);
                    }
                }
                Poll::Ready(Some(Ok(frame)))
            }
            Poll::Ready(Some(Err(e))) => Poll::Ready(Some(Err(e))),
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Pending => Poll::Pending,
        }
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> hyper::body::SizeHint {
        self.inner.size_hint()
    }
}

pub fn create_request_tee_body(
    body: impl Body<Data = Bytes, Error = hyper::Error> + Send + Sync + 'static,
    admin_state: Option<Arc<AdminState>>,
    record_id: String,
) -> (BoxBody, BodyCaptureHandle) {
    let capture = BodyCaptureHandle {
        body_ref: Arc::new(Mutex::new(None)),
    };
    let guard = RequestTeeBodyDropGuard {
        admin_state,
        record_id,
        file_writer: None,
        capture: BodyCaptureHandle {
            body_ref: capture.body_ref.clone(),
        },
    };
    let body = RequestTeeBody {
        inner: Box::pin(body),
        guard,
    };
    (BodyExt::boxed(body), capture)
}

struct SseTeeBodyDropGuard {
    admin_state: Option<Arc<AdminState>>,
    record_id: String,
    total_bytes: usize,
    finished: bool,
    traffic_type: Option<TrafficType>,
    file_writer: Option<BodyStreamWriter>,
}

impl Drop for SseTeeBodyDropGuard {
    fn drop(&mut self) {
        if !self.finished {
            self.store_body_and_update_record();
        }
    }
}

impl SseTeeBodyDropGuard {
    fn store_body_and_update_record(&mut self) {
        if let Some(ref state) = self.admin_state {
            let response_body_ref = self.file_writer.take().map(|w| w.finish());
            let derived_response_body_ref =
                derive_openai_like_sse_body_ref(state, &self.record_id, &response_body_ref);
            let had_body_bytes = self.total_bytes > 0;
            state.sse_hub.set_closed(&self.record_id);
            state.update_traffic_by_id(&self.record_id, move |record| {
                let now_ms = chrono::Utc::now().timestamp_millis().max(0) as u64;
                let total_ms = now_ms.saturating_sub(record.timestamp);
                record.duration_ms = record.duration_ms.max(total_ms);
                if let Some(ref mut timing) = record.timing {
                    if timing.first_byte_ms.is_none() && had_body_bytes {
                        timing.first_byte_ms = Some(record.duration_ms);
                    }
                    let first_response_ms = timing
                        .dns_ms
                        .unwrap_or(0)
                        .saturating_add(timing.connect_ms.unwrap_or(0))
                        .saturating_add(timing.tls_ms.unwrap_or(0))
                        .saturating_add(timing.send_ms.unwrap_or(0))
                        .saturating_add(timing.wait_ms.unwrap_or(0));
                    timing.receive_ms = Some(record.duration_ms.saturating_sub(first_response_ms));
                    timing.total_ms = record.duration_ms;
                }
                record.response_body_ref = response_body_ref.clone();
                record.derived_response_body_ref = derived_response_body_ref.clone();
            });
            persist_socket_summary(state, &self.record_id, self.total_bytes);
            state.sse_hub.unregister(&self.record_id);
        }
        self.finished = true;
    }
}

const DEFAULT_MAX_SSE_EVENT_BUFFER_BYTES: usize = 256 * 1024;

pub struct SseTeeBody<B = Incoming> {
    inner: B,
    guard: SseTeeBodyDropGuard,
    prev_byte: Option<u8>,
    event_size: usize,
    max_buffer_size: usize,
    overflowed: bool,
    flush_interval: Option<std::time::Duration>,
    flush_sleep: Option<Pin<Box<Sleep>>>,
}

impl<B> SseTeeBody<B>
where
    B: Body<Data = Bytes, Error = hyper::Error> + Unpin + Send + Sync + 'static,
{
    pub fn new(
        inner: B,
        admin_state: Option<Arc<AdminState>>,
        record_id: String,
        traffic_type: Option<TrafficType>,
        file_writer: Option<BodyStreamWriter>,
        max_buffer_size: usize,
    ) -> Self {
        let flush_interval = file_writer
            .as_ref()
            .map(|w| w.flush_interval())
            .filter(|d| !d.is_zero());
        let flush_sleep = flush_interval.map(|d| Box::pin(tokio::time::sleep(d)));

        if let Some(ref state) = admin_state {
            state.sse_hub.register(&record_id);
        }

        Self {
            inner,
            guard: SseTeeBodyDropGuard {
                admin_state,
                record_id,
                total_bytes: 0,
                finished: false,
                traffic_type,
                file_writer,
            },
            prev_byte: None,
            event_size: 0,
            max_buffer_size,
            overflowed: false,
            flush_interval,
            flush_sleep,
        }
    }

    pub fn boxed(self) -> BoxBody {
        BodyExt::boxed(self)
    }

    fn process_sse_chunk(&mut self, data: &[u8]) {
        if data.is_empty() {
            return;
        }

        let mut i = 0;
        while i < data.len() {
            let Some(rel) = memchr(b'\n', &data[i..]) else {
                if !self.overflowed {
                    self.event_size = self.event_size.saturating_add(data.len() - i);
                    if self.max_buffer_size > 0 && self.event_size > self.max_buffer_size {
                        self.overflowed = true;
                    }
                }
                self.prev_byte = Some(*data.last().unwrap());
                return;
            };

            let pos = i + rel;
            if pos > i {
                if !self.overflowed {
                    self.event_size = self.event_size.saturating_add(pos - i);
                    if self.max_buffer_size > 0 && self.event_size > self.max_buffer_size {
                        self.overflowed = true;
                    }
                }
                self.prev_byte = Some(data[pos - 1]);
            }

            if self.prev_byte == Some(b'\n') {
                if self.event_size > 0 {
                    if let Some(ref state) = self.guard.admin_state {
                        state.sse_hub.add_receive_event(&self.guard.record_id);
                    }
                }
                self.event_size = 0;
                self.overflowed = false;
                self.prev_byte = Some(b'\n');
            } else {
                if !self.overflowed {
                    self.event_size = self.event_size.saturating_add(1);
                    if self.max_buffer_size > 0 && self.event_size > self.max_buffer_size {
                        self.overflowed = true;
                    }
                }
                self.prev_byte = Some(b'\n');
            }

            i = pos + 1;
        }
    }
}

impl<B> Body for SseTeeBody<B>
where
    B: Body<Data = Bytes, Error = hyper::Error> + Unpin + Send + Sync + 'static,
{
    type Data = Bytes;
    type Error = hyper::Error;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        if self.guard.finished {
            return Poll::Ready(None);
        }

        if let (Some(interval), Some(mut sleep_fut)) =
            (self.flush_interval, self.flush_sleep.take())
        {
            if sleep_fut.as_mut().poll(cx).is_ready() {
                if let Some(ref mut writer) = self.guard.file_writer {
                    let _ = writer.flush_buffered();
                }
                self.flush_sleep = Some(Box::pin(tokio::time::sleep(interval)));
            } else {
                self.flush_sleep = Some(sleep_fut);
            }
        }

        match Pin::new(&mut self.inner).poll_frame(cx) {
            Poll::Ready(Some(Ok(frame))) => {
                if let Some(data) = frame.data_ref() {
                    let had_body_bytes = self.guard.total_bytes > 0;
                    let len = data.len();
                    self.guard.total_bytes += len;
                    if !had_body_bytes && len > 0 {
                        if let Some(ref state) = self.guard.admin_state {
                            record_first_downstream_byte(state, &self.guard.record_id);
                        }
                    }

                    if let Some(ref state) = self.guard.admin_state {
                        state.sse_hub.add_receive_bytes(&self.guard.record_id, len);
                        if let Some(traffic_type) = self.guard.traffic_type {
                            state
                                .metrics_collector
                                .add_bytes_received_by_type(traffic_type, len as u64);
                        } else {
                            state.metrics_collector.add_bytes_received(len as u64);
                        }
                    }

                    let should_force_flush = self
                        .guard
                        .admin_state
                        .as_ref()
                        .map(|state| state.sse_hub.should_force_flush(&self.guard.record_id))
                        .unwrap_or(false);

                    if let Some(ref mut writer) = self.guard.file_writer {
                        let _ = writer.write_chunk(data);
                        if should_force_flush {
                            let _ = writer.flush_buffered();
                        }
                    }

                    self.process_sse_chunk(data);
                }
                Poll::Ready(Some(Ok(frame)))
            }
            Poll::Ready(Some(Err(e))) => {
                self.guard.store_body_and_update_record();
                Poll::Ready(Some(Err(e)))
            }
            Poll::Ready(None) => {
                self.guard.store_body_and_update_record();
                Poll::Ready(None)
            }
            Poll::Pending => Poll::Pending,
        }
    }

    fn is_end_stream(&self) -> bool {
        self.guard.finished || self.inner.is_end_stream()
    }

    fn size_hint(&self) -> hyper::body::SizeHint {
        self.inner.size_hint()
    }
}

pub fn create_sse_tee_body<B>(
    body: B,
    admin_state: Option<Arc<AdminState>>,
    record_id: String,
    traffic_type: Option<TrafficType>,
    file_writer: Option<BodyStreamWriter>,
    max_buffer_size: usize,
) -> SseTeeBody<B>
where
    B: Body<Data = Bytes, Error = hyper::Error> + Unpin + Send + Sync + 'static,
{
    let max_buffer_size = max_buffer_size.min(DEFAULT_MAX_SSE_EVENT_BUFFER_BYTES);
    SseTeeBody::new(
        body,
        admin_state,
        record_id,
        traffic_type,
        file_writer,
        max_buffer_size,
    )
}

pub fn store_request_body(
    admin_state: &Option<Arc<AdminState>>,
    record_id: &str,
    body_data: &[u8],
    content_encoding: Option<&str>,
) -> Option<BodyRef> {
    if body_data.is_empty() {
        return None;
    }

    if let Some(ref state) = admin_state {
        if let Some(ref body_store) = state.body_store {
            let max_decompress_output_bytes = state
                .config_manager
                .as_ref()
                .and_then(|cm| cm.try_config())
                .map(|cfg| cfg.sandbox.limits.max_decompress_output_bytes)
                .unwrap_or(10 * 1024 * 1024);
            let decompressed = decompress_body_with_limit(
                body_data,
                content_encoding,
                max_decompress_output_bytes,
            );
            return store_body_sync(body_store, record_id, "req", decompressed.as_ref());
        }
    }
    None
}

pub fn store_response_body(
    admin_state: &Option<Arc<AdminState>>,
    record_id: &str,
    body_data: &[u8],
) -> Option<BodyRef> {
    if body_data.is_empty() {
        return None;
    }

    if let Some(ref state) = admin_state {
        if let Some(ref body_store) = state.body_store {
            return store_body_sync(body_store, record_id, "res", body_data);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    use bifrost_admin::BodyStore;
    use parking_lot::RwLock;

    #[test]
    fn tee_body_skips_connection_monitor_when_tracking_disabled() {
        let state = Arc::new(AdminState::new(0));
        let mut guard = TeeBodyDropGuard {
            admin_state: Some(state.clone()),
            record_id: "plain-http".to_string(),
            total_bytes: 128,
            pending_traffic_bytes: 64,
            finished: false,
            buffer: BytesMut::new(),
            max_body_size: DEFAULT_MAX_BODY_BUFFER_SIZE,
            content_encoding: None,
            traffic_type: Some(TrafficType::Http),
            monitor_connection: false,
            response_headers_size: 0,
            file_writer: None,
        };

        guard.store_body_and_update_record();

        assert_eq!(state.connection_monitor.connection_count(), 0);
        assert!(state.connection_monitor.get_status("plain-http").is_none());
    }

    #[test]
    fn tee_body_updates_connection_monitor_when_tracking_enabled() {
        let state = Arc::new(AdminState::new(0));
        state.connection_monitor.register_connection("streaming");
        let mut guard = TeeBodyDropGuard {
            admin_state: Some(state.clone()),
            record_id: "streaming".to_string(),
            total_bytes: 128,
            pending_traffic_bytes: 64,
            finished: false,
            buffer: BytesMut::new(),
            max_body_size: DEFAULT_MAX_BODY_BUFFER_SIZE,
            content_encoding: None,
            traffic_type: Some(TrafficType::Http),
            monitor_connection: true,
            response_headers_size: 0,
            file_writer: None,
        };

        guard.store_body_and_update_record();

        let status = state
            .connection_monitor
            .get_status("streaming")
            .expect("registered streaming connection should keep a final status");
        assert!(!status.is_open);
        assert_eq!(status.receive_bytes, 64);
    }

    #[tokio::test]
    async fn response_body_storage_waits_in_background_when_body_store_is_busy() {
        let dir = std::env::temp_dir().join(format!(
            "bifrost-tee-body-store-eventual-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let body_store = Arc::new(RwLock::new(BodyStore::new(
            dir.clone(),
            1024 * 1024,
            1,
            64 * 1024,
            Duration::from_secs(1),
        )));
        let state = Arc::new(AdminState::new(0).with_body_store(body_store.clone()));
        let busy_writer = body_store.write();

        let body_ref = store_response_body_or_schedule(
            state,
            body_store.clone(),
            "eventual-body".to_string(),
            b"body".to_vec(),
            None,
            1024 * 1024,
        );

        assert!(body_ref.is_none());
        assert!(!dir.join("eventual-body_res").exists());
        drop(busy_writer);

        for _ in 0..50 {
            if dir.join("eventual-body_res").exists() {
                let saved = std::fs::read(dir.join("eventual-body_res")).unwrap();
                assert_eq!(saved, b"body");
                let _ = std::fs::remove_dir_all(dir);
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }

        let _ = std::fs::remove_dir_all(dir);
        panic!("background body storage did not finish");
    }
}
