use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, OnceLock};
use std::task::{Context, Poll};

use bifrost_admin::{
    AdminState, BodyRef, BodyStreamWriter, FrameDirection, SharedBodyStore, TrafficType,
};
use bytes::{Bytes, BytesMut};
use http_body_util::BodyExt;
use hyper::body::{Body, Frame, Incoming};
use memchr::memchr;
use tokio::sync::Semaphore;
use tokio::time::Sleep;

use crate::server::BoxBody;
use crate::transform::decompress::{decompress_body_with_limit, try_decompress_body_with_limit};

mod openai_like;

use openai_like::derive_openai_like_sse_body_ref;

// Keep hot-path metrics updates coarse-grained so high-throughput relays do
// not burn CPU on bookkeeping.
const BODY_TRAFFIC_FLUSH_BYTES: usize = 1024 * 1024;
const DEFAULT_BODY_STORE_BACKGROUND_CONCURRENCY: usize = 1;

fn record_first_downstream_byte(state: &AdminState, record_id: &str) {
    if state.get_super_performance_mode() {
        return;
    }
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

#[derive(Default)]
struct StoredResponseBodies {
    primary: Option<BodyRef>,
    raw: Option<BodyRef>,
}

fn stores_decoded_http_body(content_encoding: Option<&str>) -> bool {
    content_encoding.is_some_and(|value| {
        value
            .split(',')
            .map(str::trim)
            .any(|coding| !coding.is_empty() && !coding.eq_ignore_ascii_case("identity"))
    })
}

fn store_buffered_response_bodies(
    store: &bifrost_admin::BodyStore,
    record_id: &str,
    body: &[u8],
    content_encoding: Option<&str>,
    max_decompress_output_bytes: usize,
) -> StoredResponseBodies {
    let should_decode = stores_decoded_http_body(content_encoding);
    let decoded = if max_decompress_output_bytes > 0 {
        content_encoding.and_then(|encoding| {
            try_decompress_body_with_limit(body, encoding, max_decompress_output_bytes).ok()
        })
    } else {
        None
    };
    if should_decode {
        if let Some(decoded) = decoded {
            return StoredResponseBodies {
                primary: store.store(record_id, "res", &decoded),
                raw: store.store(record_id, "res_raw", body),
            };
        }
    }

    let primary = store.store(record_id, "res", body).and_then(|body_ref| {
        let cleanup_ref = body_ref.clone();
        match body_ref.with_content_encoding(content_encoding) {
            Ok(body_ref) => Some(body_ref),
            Err(error) => {
                store.remove(&cleanup_ref);
                tracing::warn!(%error, %record_id, "failed to persist buffered response content encoding");
                None
            }
        }
    });
    StoredResponseBodies { primary, raw: None }
}

fn schedule_decompressed_response_body_store(
    state: Arc<AdminState>,
    body_store: SharedBodyStore,
    record_id: String,
    body: Vec<u8>,
    content_encoding: Option<String>,
    max_decompress_output_bytes: usize,
) -> StoredResponseBodies {
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        return store_buffered_response_bodies(
            &body_store.read(),
            &record_id,
            &body,
            content_encoding.as_deref(),
            max_decompress_output_bytes,
        );
    };

    handle.spawn(async move {
        let semaphore = body_store_background_semaphore();
        let _permit = semaphore.acquire_owned().await.ok();
        let record_id_for_store = record_id.clone();
        let stored = tokio::task::spawn_blocking(move || {
            store_buffered_response_bodies(
                &body_store.read(),
                &record_id_for_store,
                &body,
                content_encoding.as_deref(),
                max_decompress_output_bytes,
            )
        })
        .await
        .unwrap_or_default();

        if stored.primary.is_some() || stored.raw.is_some() {
            state.update_traffic_by_id(&record_id, move |record| {
                if stored.primary.is_some() {
                    record.response_body_ref = stored.primary.clone();
                }
                if stored.raw.is_some() {
                    record.raw_response_body_ref = stored.raw.clone();
                }
            });
        }
    });

    StoredResponseBodies::default()
}

fn store_response_body_or_schedule(
    state: Arc<AdminState>,
    body_store: SharedBodyStore,
    record_id: String,
    body: Vec<u8>,
    content_encoding: Option<String>,
    max_decompress_output_bytes: usize,
) -> StoredResponseBodies {
    if state.get_super_performance_mode() {
        return StoredResponseBodies::default();
    }
    if let Some(store) = body_store.try_read() {
        return store_buffered_response_bodies(
            &store,
            &record_id,
            &body,
            content_encoding.as_deref(),
            max_decompress_output_bytes,
        );
    }

    schedule_decompressed_response_body_store(
        state,
        body_store,
        record_id,
        body,
        content_encoding,
        max_decompress_output_bytes,
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
    if state.get_super_performance_mode() {
        return;
    }
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
            if state.get_super_performance_mode() {
                self.finished = true;
                return;
            }
            let stored_response_bodies = if let Some(writer) = self.file_writer.take() {
                match writer
                    .finish()
                    .with_content_encoding(self.content_encoding.as_deref())
                {
                    Ok(body_ref) => StoredResponseBodies {
                        primary: Some(body_ref),
                        raw: None,
                    },
                    Err(error) => {
                        tracing::warn!(%error, record_id = %self.record_id, "failed to persist response content encoding");
                        StoredResponseBodies::default()
                    }
                }
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
                    StoredResponseBodies::default()
                }
            } else {
                StoredResponseBodies::default()
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
                if stored_response_bodies.primary.is_some() {
                    record.response_body_ref = stored_response_bodies.primary.clone();
                }
                if stored_response_bodies.raw.is_some() {
                    record.raw_response_body_ref = stored_response_bodies.raw.clone();
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
    if admin_state
        .as_ref()
        .map(|state| state.get_super_performance_mode())
        .unwrap_or(false)
    {
        return create_metrics_body(body, admin_state, options.traffic_type);
    }

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
    content_encoding: Option<String>,
    capture: BodyCaptureHandle,
}

impl Drop for RequestTeeBodyDropGuard {
    fn drop(&mut self) {
        if let Some(writer) = self.file_writer.take() {
            let body_ref = writer
                .finish()
                .with_content_encoding(self.content_encoding.as_deref());
            if let Err(ref error) = body_ref {
                tracing::warn!(%error, record_id = %self.record_id, "failed to persist request content encoding");
            }
            if let (Ok(mut slot), Ok(body_ref)) = (self.capture.body_ref.lock(), body_ref) {
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
    content_encoding: Option<String>,
) -> (BoxBody, BodyCaptureHandle) {
    let capture = BodyCaptureHandle {
        body_ref: Arc::new(Mutex::new(None)),
    };
    if admin_state
        .as_ref()
        .map(|state| state.get_super_performance_mode())
        .unwrap_or(false)
    {
        return (BodyExt::boxed(body), capture);
    }

    let guard = RequestTeeBodyDropGuard {
        admin_state,
        record_id,
        file_writer: None,
        content_encoding,
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
    content_encoding: Option<String>,
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
            let response_body_ref = self.file_writer.take().and_then(|writer| {
                match writer
                    .finish()
                    .with_content_encoding(self.content_encoding.as_deref())
                {
                    Ok(body_ref) => Some(body_ref),
                    Err(error) => {
                        tracing::warn!(%error, record_id = %self.record_id, "failed to persist SSE content encoding");
                        None
                    }
                }
            });
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
        content_encoding: Option<String>,
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
                content_encoding,
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
    content_encoding: Option<String>,
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
        content_encoding,
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
        if state.get_super_performance_mode() {
            return None;
        }
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
        if state.get_super_performance_mode() {
            return None;
        }
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

    use bifrost_admin::{BodyStore, TrafficDbStore};
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
        let (state, dir) = test_state_with_body_store("body-store-eventual");
        let body_store = state.body_store.as_ref().unwrap().clone();
        state.record_traffic(bifrost_admin::TrafficRecord::new(
            "eventual-body".to_string(),
            "GET".to_string(),
            "http://example.test/eventual".to_string(),
        ));
        let busy_writer = body_store.write();

        let stored = store_response_body_or_schedule(
            state.clone(),
            body_store.clone(),
            "eventual-body".to_string(),
            b"body".to_vec(),
            None,
            1024 * 1024,
        );

        assert!(stored.primary.is_none());
        assert!(stored.raw.is_none());
        assert!(!dir.join("eventual-body_res").exists());
        drop(busy_writer);

        for _ in 0..50 {
            let body_ref = state
                .traffic_db_store
                .as_ref()
                .and_then(|store| store.get_by_id("eventual-body"))
                .and_then(|record| record.response_body_ref);
            if dir.join("eventual-body_res").exists() && body_ref.is_some() {
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

    #[test]
    fn super_performance_mode_skips_request_and_response_body_storage() {
        let dir = std::env::temp_dir().join(format!(
            "bifrost-tee-super-performance-{}",
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
        let state = Arc::new(
            AdminState::new(0)
                .with_body_store(body_store)
                .with_super_performance_mode(true),
        );
        let admin_state = Some(state);

        let req_ref = store_request_body(&admin_state, "super-mode-body", b"request", None);
        let res_ref = store_response_body(&admin_state, "super-mode-body", b"response");

        assert!(req_ref.is_none());
        assert!(res_ref.is_none());
        assert!(!dir.join("super-mode-body_req").exists());
        assert!(!dir.join("super-mode-body_res").exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn super_performance_mode_skips_streaming_body_tee_storage() {
        let dir = std::env::temp_dir().join(format!(
            "bifrost-tee-super-performance-streaming-{}",
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
        let state = Arc::new(
            AdminState::new(0)
                .with_body_store(body_store)
                .with_super_performance_mode(true),
        );

        let (request_body, capture) = create_request_tee_body(
            crate::server::full_body(Bytes::from_static(b"streaming-request")),
            Some(Arc::clone(&state)),
            "super-mode-stream".to_string(),
            None,
        );
        let request_bytes = request_body.collect().await.unwrap().to_bytes();

        let response_body = create_tee_body_with_store(
            crate::server::full_body(Bytes::from_static(b"streaming-response")),
            Some(state),
            "super-mode-stream".to_string(),
            TeeBodyCaptureOptions {
                max_body_size: Some(4),
                content_encoding: None,
                traffic_type: None,
                monitor_connection: false,
                response_headers_size: 0,
            },
        );
        let response_bytes = response_body.collect().await.unwrap().to_bytes();

        assert_eq!(request_bytes, Bytes::from_static(b"streaming-request"));
        assert_eq!(response_bytes, Bytes::from_static(b"streaming-response"));
        assert!(capture.clone_ref().is_none());
        assert!(capture.take().is_none());
        assert!(!dir.join("super-mode-stream_req").exists());
        assert!(!dir.join("super-mode-stream_res").exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    fn test_state_with_body_store(prefix: &str) -> (Arc<AdminState>, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "bifrost-tee-{prefix}-{}-{}",
            std::process::id(),
            rand::random::<u64>()
        ));
        let body_store = Arc::new(RwLock::new(BodyStore::new(
            dir.clone(),
            1024 * 1024,
            1,
            64 * 1024,
            Duration::from_millis(1),
        )));
        let traffic_store = TrafficDbStore::new(dir.join("traffic-db"), 100, 0, None).unwrap();
        (
            Arc::new(
                AdminState::new(0)
                    .with_body_store(body_store)
                    .with_traffic_db_store(traffic_store),
            ),
            dir,
        )
    }

    #[tokio::test]
    async fn sse_tee_persists_raw_and_derived_bodies_and_socket_summary() {
        let (state, dir) = test_state_with_body_store("sse");
        let record_id = "sse-covered";
        state.record_traffic(bifrost_admin::TrafficRecord::new(
            record_id.into(),
            "GET".into(),
            "http://example.test/events".into(),
        ));
        let writer =
            start_body_stream(state.body_store.as_ref().unwrap(), record_id, "sse_raw").unwrap();
        let payload = Bytes::from_static(
            b"data: {\"choices\":[{\"delta\":{\"content\":\"Hello\"}}]}\n\ndata: [DONE]\n\n",
        );
        let body = create_sse_tee_body(
            crate::server::full_body(payload.clone()),
            Some(state.clone()),
            record_id.into(),
            Some(TrafficType::Http),
            Some(writer),
            None,
            1024,
        );
        assert!(!body.is_end_stream());
        assert_eq!(body.size_hint().lower(), payload.len() as u64);
        let collected = body.boxed().collect().await.unwrap().to_bytes();
        assert_eq!(collected, payload);

        let record = state
            .traffic_db_store
            .as_ref()
            .and_then(|store| store.get_by_id(record_id))
            .expect("traffic record");
        assert!(record.response_body_ref.is_some());
        assert!(record.derived_response_body_ref.is_some());
        assert_eq!(record.frame_count, 2);
        assert_eq!(record.download_bytes, payload.len());
        assert!(state.sse_hub.get_socket_status(record_id).is_none());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn sse_chunk_parser_covers_split_boundaries_overflow_and_no_state() {
        let body = crate::server::full_body(Bytes::new());
        let mut tee = SseTeeBody::new(body, None, "none".into(), None, None, None, 3);
        tee.process_sse_chunk(b"abcd");
        assert!(tee.overflowed);
        tee.process_sse_chunk(b"\n");
        tee.process_sse_chunk(b"\n");
        assert!(!tee.overflowed);
        tee.process_sse_chunk(b"x\n\n");
        assert_eq!(tee.event_size, 0);
        let collected = tee.boxed().collect().await.unwrap().to_bytes();
        assert!(collected.is_empty());

        let capped = create_sse_tee_body(
            crate::server::full_body(Bytes::new()),
            None,
            "capped".into(),
            None,
            None,
            None,
            usize::MAX,
        );
        assert_eq!(capped.max_buffer_size, DEFAULT_MAX_SSE_EVENT_BUFFER_BYTES);
    }

    #[tokio::test]
    async fn normal_request_response_and_metrics_tee_paths_store_and_forward() {
        let (state, dir) = test_state_with_body_store("normal");
        let record_id = "normal-covered";
        state.record_traffic(bifrost_admin::TrafficRecord::new(
            record_id.into(),
            "POST".into(),
            "http://example.test/".into(),
        ));

        let (request, capture) = create_request_tee_body(
            crate::server::full_body(Bytes::from_static(b"request-body")),
            Some(state.clone()),
            record_id.into(),
            None,
        );
        assert_eq!(request.size_hint().lower(), 12);
        assert_eq!(
            request.collect().await.unwrap().to_bytes(),
            Bytes::from_static(b"request-body")
        );
        // The drop guard transfers the captured body reference into the traffic
        // record, so the caller-side handle is intentionally empty afterwards.
        assert!(capture.clone_ref().or_else(|| capture.take()).is_none());

        let response = create_tee_body_with_store(
            crate::server::full_body(Bytes::from_static(b"response-body")),
            Some(state.clone()),
            record_id.into(),
            TeeBodyCaptureOptions {
                max_body_size: Some(1024),
                content_encoding: None,
                traffic_type: None,
                monitor_connection: false,
                response_headers_size: 10,
            },
        );
        assert_eq!(
            response.collect().await.unwrap().to_bytes(),
            Bytes::from_static(b"response-body")
        );

        let metrics = create_metrics_body(
            crate::server::full_body(Bytes::from_static(b"metrics")),
            Some(state.clone()),
            Some(TrafficType::Http),
        );
        assert_eq!(
            metrics.collect().await.unwrap().to_bytes(),
            Bytes::from_static(b"metrics")
        );
        let record = state
            .traffic_db_store
            .as_ref()
            .and_then(|store| store.get_by_id(record_id))
            .expect("traffic record");
        assert!(record.request_body_ref.is_some());
        assert!(record.response_body_ref.is_some());
        assert!(store_request_body(&Some(state.clone()), "direct", b"request", None).is_some());
        assert!(store_response_body(&Some(state), "direct", b"response").is_some());
        assert!(store_request_body(&None, "none", b"request", None).is_none());
        assert!(store_response_body(&None, "none", b"response").is_none());
        assert!(store_request_body(&None, "empty", b"", None).is_none());
        assert!(store_response_body(&None, "empty", b"").is_none());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn streaming_body_refs_persist_their_content_encoding() {
        let (state, dir) = test_state_with_body_store("content-encoded");
        let record_id = "content-encoded-stream";
        state.record_traffic(bifrost_admin::TrafficRecord::new(
            record_id.into(),
            "POST".into(),
            "http://example.test/".into(),
        ));

        let request_wire = Bytes::from_static(b"request-wire");
        let (request, _) = create_request_tee_body(
            crate::server::full_body(request_wire.clone()),
            Some(state.clone()),
            record_id.into(),
            Some("gzip".to_string()),
        );
        assert_eq!(request.collect().await.unwrap().to_bytes(), request_wire);

        let response_wire = Bytes::from_static(b"response-wire");
        let response = create_tee_body_with_store(
            crate::server::full_body(response_wire.clone()),
            Some(state.clone()),
            record_id.into(),
            TeeBodyCaptureOptions {
                max_body_size: Some(1),
                content_encoding: Some("br".to_string()),
                traffic_type: None,
                monitor_connection: false,
                response_headers_size: 0,
            },
        );
        assert_eq!(response.collect().await.unwrap().to_bytes(), response_wire);

        let record = state
            .traffic_db_store
            .as_ref()
            .and_then(|store| store.get_by_id(record_id))
            .expect("traffic record");
        let request_ref = record.request_body_ref.as_ref().expect("request body ref");
        let response_ref = record
            .response_body_ref
            .as_ref()
            .expect("response body ref");
        assert_eq!(request_ref.content_encoding().as_deref(), Some("gzip"));
        assert_eq!(response_ref.content_encoding().as_deref(), Some("br"));
        let store = state.body_store.as_ref().expect("body store").read();
        assert_eq!(
            store.load_bytes(request_ref).as_deref(),
            Some(request_wire.as_ref())
        );
        assert_eq!(
            store.load_bytes(response_ref).as_deref(),
            Some(response_wire.as_ref())
        );
        drop(store);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn buffered_compressed_response_keeps_plaintext_and_wire_bytes() {
        use flate2::{write::GzEncoder, Compression};
        use std::io::Write;

        let (state, dir) = test_state_with_body_store("buffered-content-encoded");
        let record_id = "buffered-content-encoded";
        state.record_traffic(bifrost_admin::TrafficRecord::new(
            record_id.into(),
            "GET".into(),
            "http://example.test/".into(),
        ));
        let plaintext = b"buffered gzip plaintext";
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(plaintext).unwrap();
        let wire = encoder.finish().unwrap();

        let response = create_tee_body_with_store(
            crate::server::full_body(Bytes::from(wire.clone())),
            Some(state.clone()),
            record_id.into(),
            TeeBodyCaptureOptions {
                max_body_size: Some(1024),
                content_encoding: Some("gzip".to_string()),
                traffic_type: None,
                monitor_connection: false,
                response_headers_size: 0,
            },
        );
        response.collect().await.unwrap();

        let record = state
            .traffic_db_store
            .as_ref()
            .and_then(|store| store.get_by_id(record_id))
            .unwrap();
        let store = state.body_store.as_ref().unwrap().read();
        assert_eq!(
            store
                .load_bytes(record.response_body_ref.as_ref().unwrap())
                .as_deref(),
            Some(plaintext.as_slice())
        );
        assert_eq!(
            store
                .load_bytes(record.raw_response_body_ref.as_ref().unwrap())
                .as_deref(),
            Some(wire.as_slice())
        );
        drop(store);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn buffered_response_falls_back_to_encoded_wire_when_decode_is_unavailable() {
        let dir = std::env::temp_dir().join(format!(
            "bifrost-tee-buffered-fallback-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let store = BodyStore::new(
            dir.clone(),
            1024 * 1024,
            1,
            64 * 1024,
            Duration::from_secs(1),
        );
        let stored = store_buffered_response_bodies(
            &store,
            "decode-disabled",
            b"wire body",
            Some("gzip"),
            0,
        );

        let primary = stored.primary.expect("wire fallback should be stored");
        assert!(stored.raw.is_none());
        assert_eq!(primary.content_encoding().as_deref(), Some("gzip"));
        assert_eq!(
            store.load_bytes(&primary).as_deref(),
            Some(b"wire body".as_slice())
        );

        let invalid_encoding = "gzip,".repeat(80);
        let invalid = store_buffered_response_bodies(
            &store,
            "invalid-metadata",
            b"wire body",
            Some(&invalid_encoding),
            0,
        );
        assert!(invalid.primary.is_none());
        assert!(!dir.join("invalid-metadata_res").exists());

        let state = Arc::new(
            AdminState::new(0)
                .with_body_store(Arc::new(RwLock::new(store)))
                .with_super_performance_mode(true),
        );
        let skipped = store_response_body_or_schedule(
            state.clone(),
            state.body_store.as_ref().unwrap().clone(),
            "super-performance".to_string(),
            b"not stored".to_vec(),
            None,
            1024,
        );
        assert!(skipped.primary.is_none());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn buffered_response_store_works_without_an_async_runtime() {
        use flate2::{write::GzEncoder, Compression};
        use std::io::Write;

        let (state, dir) = test_state_with_body_store("sync-buffered-store");
        let body_store = state.body_store.as_ref().unwrap().clone();
        let plaintext = b"synchronous buffered response";
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(plaintext).unwrap();
        let wire = encoder.finish().unwrap();

        let stored = schedule_decompressed_response_body_store(
            state,
            body_store.clone(),
            "sync-buffered-store".to_string(),
            wire.clone(),
            Some("gzip".to_string()),
            1024,
        );

        let store = body_store.read();
        assert_eq!(
            store
                .load_bytes(stored.primary.as_ref().unwrap())
                .as_deref(),
            Some(plaintext.as_slice())
        );
        assert_eq!(
            store.load_bytes(stored.raw.as_ref().unwrap()).as_deref(),
            Some(wire.as_slice())
        );
        drop(store);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn streaming_body_refs_are_not_published_when_encoding_metadata_fails() {
        let (state, dir) = test_state_with_body_store("invalid-content-encoding");
        let record_id = "invalid-content-encoding";
        state.record_traffic(bifrost_admin::TrafficRecord::new(
            record_id.into(),
            "POST".into(),
            "http://example.test/events".into(),
        ));
        let invalid_encoding = "gzip,".repeat(80);

        let (request, capture) = create_request_tee_body(
            crate::server::full_body(Bytes::from_static(b"request-wire")),
            Some(state.clone()),
            record_id.into(),
            Some(invalid_encoding.clone()),
        );
        request.collect().await.unwrap();
        assert!(capture.take().is_none());

        let response = create_tee_body_with_store(
            crate::server::full_body(Bytes::from_static(b"response-wire")),
            Some(state.clone()),
            record_id.into(),
            TeeBodyCaptureOptions {
                max_body_size: Some(1),
                content_encoding: Some(invalid_encoding.clone()),
                traffic_type: None,
                monitor_connection: false,
                response_headers_size: 0,
            },
        );
        response.collect().await.unwrap();

        let writer =
            start_body_stream(state.body_store.as_ref().unwrap(), record_id, "sse_raw").unwrap();
        let sse = create_sse_tee_body(
            crate::server::full_body(Bytes::from_static(b"data: event\n\n")),
            Some(state.clone()),
            record_id.into(),
            Some(TrafficType::Http),
            Some(writer),
            Some(invalid_encoding),
            1024,
        );
        sse.boxed().collect().await.unwrap();

        let record = state
            .traffic_db_store
            .as_ref()
            .and_then(|store| store.get_by_id(record_id))
            .unwrap();
        assert!(record.request_body_ref.is_none());
        assert!(record.response_body_ref.is_none());
        let _ = std::fs::remove_dir_all(dir);
    }
}
