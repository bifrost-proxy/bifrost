use std::sync::{Arc, OnceLock};

use bifrost_admin::{AdminState, BodyRef, BodyStreamWriter, SharedBodyStore, TrafficRecord};
use tokio::sync::Semaphore;

use crate::transform::decompress::try_decompress_body_with_limit;

const DEFAULT_BODY_STORE_BACKGROUND_CONCURRENCY: usize = 1;

#[derive(Default)]
pub struct StoredRequestBodies {
    primary: Option<BodyRef>,
    raw: Option<BodyRef>,
}

impl StoredRequestBodies {
    pub fn apply_to(self, record: &mut TrafficRecord) {
        record.request_body_ref = self.primary;
        record.raw_request_body_ref = self.raw;
    }

    pub fn into_primary(self) -> Option<BodyRef> {
        self.primary
    }

    pub fn is_empty(&self) -> bool {
        self.primary.is_none() && self.raw.is_none()
    }
}

pub(super) fn finish_body_stream_with_encoding(
    body_store: Option<&SharedBodyStore>,
    writer: BodyStreamWriter,
    content_encoding: Option<&str>,
    record_id: &str,
    body_kind: &str,
) -> Option<BodyRef> {
    let body_ref = writer.finish();
    let cleanup_ref = body_ref.clone();
    match body_ref.with_content_encoding(content_encoding) {
        Ok(body_ref) => Some(body_ref),
        Err(error) => {
            if let Some(body_store) = body_store {
                body_store.read().remove(&cleanup_ref);
            }
            tracing::warn!(%error, %record_id, %body_kind, "failed to persist streamed body content encoding");
            None
        }
    }
}

#[derive(Default)]
pub(super) struct StoredResponseBodies {
    pub(super) primary: Option<BodyRef>,
    pub(super) raw: Option<BodyRef>,
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

fn stores_decoded_http_body(content_encoding: Option<&str>) -> bool {
    content_encoding.is_some_and(|value| {
        value
            .split(',')
            .map(str::trim)
            .any(|coding| !coding.is_empty() && !coding.eq_ignore_ascii_case("identity"))
    })
}

fn body_ref_is_lossless(body_ref: &BodyRef, bytes: &[u8]) -> bool {
    match body_ref {
        BodyRef::File { size, .. } | BodyRef::FileRange { size, .. } => *size == bytes.len(),
        BodyRef::Inline { data } => data.as_bytes() == bytes,
    }
}

fn remove_file_backed_ref(store: &bifrost_admin::BodyStore, body_ref: Option<&BodyRef>) {
    if let Some(body_ref) = body_ref.filter(|body_ref| body_ref.is_file()) {
        store.remove(body_ref);
    }
}

fn store_decoded_and_raw(
    store: &bifrost_admin::BodyStore,
    record_id: &str,
    primary_kind: &str,
    raw_kind: &str,
    decoded: &[u8],
    wire: &[u8],
) -> Option<(BodyRef, BodyRef)> {
    let primary = store.store(record_id, primary_kind, decoded);
    let raw = store.store(record_id, raw_kind, wire);
    if primary
        .as_ref()
        .is_some_and(|body_ref| body_ref_is_lossless(body_ref, decoded))
        && raw
            .as_ref()
            .is_some_and(|body_ref| body_ref_is_lossless(body_ref, wire))
    {
        return Some((primary.unwrap(), raw.unwrap()));
    }

    remove_file_backed_ref(store, primary.as_ref());
    remove_file_backed_ref(store, raw.as_ref());
    None
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
            if let Some((primary, raw)) =
                store_decoded_and_raw(store, record_id, "res", "res_raw", &decoded, body)
            {
                return StoredResponseBodies {
                    primary: Some(primary),
                    raw: Some(raw),
                };
            }
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

pub(super) fn store_buffered_request_bodies(
    store: &bifrost_admin::BodyStore,
    record_id: &str,
    body: &[u8],
    content_encoding: Option<&str>,
    max_decompress_output_bytes: usize,
) -> StoredRequestBodies {
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
            if let Some((primary, raw)) =
                store_decoded_and_raw(store, record_id, "req", "req_raw", &decoded, body)
            {
                return StoredRequestBodies {
                    primary: Some(primary),
                    raw: Some(raw),
                };
            }
        }
    }

    let primary = store.store(record_id, "req", body).and_then(|body_ref| {
        let cleanup_ref = body_ref.clone();
        match body_ref.with_content_encoding(content_encoding) {
            Ok(body_ref) => Some(body_ref),
            Err(error) => {
                store.remove(&cleanup_ref);
                tracing::warn!(%error, %record_id, "failed to persist buffered request content encoding");
                None
            }
        }
    });
    StoredRequestBodies { primary, raw: None }
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

pub(super) fn store_response_body_or_schedule(
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

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::time::Duration;

    use bifrost_admin::{BodyStore, TrafficDbStore};
    use bytes::Bytes;
    use flate2::{write::GzEncoder, Compression};
    use http_body_util::BodyExt;
    use parking_lot::RwLock;

    use super::*;
    use crate::utils::tee::{create_tee_body_with_store, TeeBodyCaptureOptions};

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
    async fn buffered_compressed_response_keeps_plaintext_and_wire_bytes() {
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
    fn buffered_compressed_request_keeps_plaintext_and_wire_bytes() {
        let dir = std::env::temp_dir().join(format!(
            "bifrost-tee-buffered-request-{}-{}",
            std::process::id(),
            rand::random::<u64>()
        ));
        let store = BodyStore::new(
            dir.clone(),
            1024 * 1024,
            1,
            64 * 1024,
            Duration::from_secs(1),
        );
        let plaintext = b"buffered request plaintext";
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(plaintext).unwrap();
        let wire = encoder.finish().unwrap();

        let stored = store_buffered_request_bodies(&store, "request", &wire, Some("gzip"), 1024);

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
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn buffered_compressed_bodies_never_publish_lossy_inline_raw_refs() {
        let parent = std::env::temp_dir().join(format!(
            "bifrost-tee-lossy-raw-{}-{}",
            std::process::id(),
            rand::random::<u64>()
        ));
        std::fs::create_dir_all(&parent).unwrap();
        let not_a_directory = parent.join("body-store-file");
        std::fs::write(&not_a_directory, b"occupied").unwrap();
        let store = BodyStore::new(
            not_a_directory,
            1024 * 1024,
            1,
            64 * 1024,
            Duration::from_secs(1),
        );
        let plaintext = vec![b'x'; 16 * 1024];
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&plaintext).unwrap();
        let wire = encoder.finish().unwrap();

        let response =
            store_buffered_response_bodies(&store, "response", &wire, Some("gzip"), 32 * 1024);
        assert!(response.raw.is_none());
        assert!(response.primary.is_none());

        let request =
            store_buffered_request_bodies(&store, "request", &wire, Some("gzip"), 32 * 1024);
        assert!(request.raw.is_none());
        assert!(request.primary.is_none());
        let _ = std::fs::remove_dir_all(parent);
    }

    #[test]
    fn file_backed_cleanup_removes_abandoned_body() {
        let dir = std::env::temp_dir().join(format!(
            "bifrost-tee-abandoned-body-{}-{}",
            std::process::id(),
            rand::random::<u64>()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("abandoned");
        std::fs::write(&path, b"wire").unwrap();
        let store = BodyStore::new(
            dir.clone(),
            1024 * 1024,
            1,
            64 * 1024,
            Duration::from_secs(1),
        );
        let body_ref = BodyRef::File {
            path: path.to_string_lossy().into_owned(),
            size: 4,
        };

        remove_file_backed_ref(&store, Some(&body_ref));

        assert!(!path.exists());
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

        let invalid_request = store_buffered_request_bodies(
            &store,
            "invalid-request-metadata",
            b"wire body",
            Some(&invalid_encoding),
            0,
        );
        assert!(invalid_request.primary.is_none());
        assert!(!dir.join("invalid-request-metadata_req").exists());

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
}
