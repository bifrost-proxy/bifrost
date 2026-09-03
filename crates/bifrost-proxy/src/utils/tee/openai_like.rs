use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::Arc;

use bifrost_admin::{
    assemble_openai_like_response_body_from_text, AdminState, BodyRef,
    MAX_OPENAI_LIKE_SSE_ASSEMBLY_INPUT_BYTES,
};

use crate::transform::decompress::try_decompress_body_with_limit;

const MAX_DERIVED_OPENAI_LIKE_SSE_BODY_BYTES: usize = MAX_OPENAI_LIKE_SSE_ASSEMBLY_INPUT_BYTES;

fn derive_openai_like_sse_body_ref(
    state: &AdminState,
    record_id: &str,
    response_body_ref: &Option<BodyRef>,
    content_encoding: Option<&str>,
) -> Option<BodyRef> {
    if state.get_super_performance_mode() {
        return None;
    }
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
    let wire_body = body_store.read().load_bytes(body_ref)?;
    let max_decompress_output_bytes = state
        .config_manager
        .as_ref()
        .and_then(|manager| manager.try_config())
        .map(|config| config.sandbox.limits.max_decompress_output_bytes)
        .unwrap_or(10 * 1024 * 1024)
        .min(MAX_DERIVED_OPENAI_LIKE_SSE_BODY_BYTES);
    let content_encoding = content_encoding
        .map(str::to_string)
        .or_else(|| body_ref.content_encoding());
    let decoded = match content_encoding {
        Some(content_encoding) => match try_decompress_body_with_limit(
            &wire_body,
            &content_encoding,
            max_decompress_output_bytes,
        ) {
            Ok(decoded) => decoded,
            Err(error) => {
                tracing::debug!(%error, record_id, %content_encoding, "Skipping OpenAI-like SSE derivation because decoding failed");
                return None;
            }
        },
        None => wire_body,
    };
    let raw_body = String::from_utf8(decoded).ok()?;
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

pub(super) fn schedule_openai_like_sse_body_derivation(
    state: Arc<AdminState>,
    record_id: String,
    response_body_ref: Option<BodyRef>,
    content_encoding: Option<String>,
) {
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        return;
    };
    handle.spawn(async move {
        let state_for_work = state.clone();
        let record_id_for_work = record_id.clone();
        let derived = tokio::task::spawn_blocking(move || {
            derive_openai_like_sse_body_ref(
                &state_for_work,
                &record_id_for_work,
                &response_body_ref,
                content_encoding.as_deref(),
            )
        })
        .await
        .ok()
        .flatten();
        if let Some(derived) = derived {
            state.update_traffic_by_id(&record_id, move |record| {
                record.derived_response_body_ref = Some(derived.clone());
            });
        }
    });
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::sync::Arc;
    use std::time::Duration;

    use bifrost_admin::{AdminState, BodyStore};
    use parking_lot::RwLock;

    use super::{derive_openai_like_sse_body_ref, schedule_openai_like_sse_body_derivation};

    fn test_state_with_body_store(prefix: &str) -> (Arc<AdminState>, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "bifrost-tee-openai-{prefix}-{}-{}",
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
        (
            Arc::new(AdminState::new(0).with_body_store(body_store)),
            dir,
        )
    }

    #[test]
    fn malformed_compressed_sse_is_not_derived_as_openai_content() {
        let (state, dir) = test_state_with_body_store("malformed-sse-derive");
        let record_id = "malformed-sse-derive";
        let body_ref = state
            .body_store
            .as_ref()
            .unwrap()
            .read()
            .store(record_id, "sse_raw", b"not gzip")
            .unwrap()
            .with_content_encoding(Some("gzip"))
            .unwrap();

        assert!(
            derive_openai_like_sse_body_ref(&state, record_id, &Some(body_ref), Some("gzip"))
                .is_none()
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn compressed_sse_is_decoded_before_openai_like_derivation() {
        let (state, dir) = test_state_with_body_store("compressed-sse-derive");
        let record_id = "compressed-sse-derive";
        let raw = concat!(
            "data: {\"id\":\"chatcmpl-test\",\"object\":\"chat.completion.chunk\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"hello\"},\"finish_reason\":null}]}\n\n",
            "data: [DONE]\n\n"
        );
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(raw.as_bytes()).unwrap();
        let compressed = encoder.finish().unwrap();
        let body_ref = state
            .body_store
            .as_ref()
            .unwrap()
            .read()
            .store(record_id, "sse_raw", &compressed)
            .unwrap()
            .with_content_encoding(Some("gzip"))
            .unwrap();

        let derived =
            derive_openai_like_sse_body_ref(&state, record_id, &Some(body_ref), Some("gzip"))
                .expect("derive compressed SSE body");
        let body = state
            .body_store
            .as_ref()
            .unwrap()
            .read()
            .load(&derived)
            .expect("load derived body");
        let json: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(json["choices"][0]["message"]["content"], "hello");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn scheduling_derivation_without_runtime_is_a_noop() {
        schedule_openai_like_sse_body_derivation(
            Arc::new(AdminState::new(0)),
            "no-runtime".to_string(),
            None,
            None,
        );
    }
}
