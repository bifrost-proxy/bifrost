//! Memory system local telemetry (JSONL file).

use crate::memory::layout::memory_root;
use crate::memory::utils::now_secs;
use fs2::FileExt;
use std::fs;
use std::io::Write;

/// Append a single JSONL telemetry line to `agent/memory/.telemetry.jsonl`.
///
/// Swallows all errors — telemetry must never break the memory hot path.
pub(crate) fn telemetry_event(event: &str, value: u64, success: bool, detail: Option<&str>) {
    let root = memory_root();
    if fs::create_dir_all(&root).is_err() {
        return;
    }
    let path = root.join(".telemetry.jsonl");
    let line = serde_json::json!({
        "ts": now_secs(),
        "event": event,
        "value": value,
        "success": success,
        "detail": detail,
    });
    if let Ok(mut file) = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .read(true)
        .open(&path)
    {
        if file.lock_exclusive().is_ok() {
            let _ = writeln!(file, "{line}");
            let _ = file.flush();
            let _ = FileExt::unlock(&file);
        }
    }
}
