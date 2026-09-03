use std::sync::{Arc, OnceLock};

use bifrost_admin::{AdminState, BodyRef, SharedBodyStore};
use tokio::sync::Semaphore;

use crate::transform::decompress::try_decompress_body_with_limit;

const DEFAULT_BODY_STORE_BACKGROUND_CONCURRENCY: usize = 1;

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
