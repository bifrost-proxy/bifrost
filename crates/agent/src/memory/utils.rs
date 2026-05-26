//! Memory system utility functions.

use crate::memory::constants::*;
use fs2::FileExt;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::io::Write;
use std::path::Path;
use std::{fs, io};

/// Return the current time as unix seconds.
pub(crate) fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Sanitize a string into a safe filesystem ID.
pub(crate) fn sanitize_id(input: &str) -> String {
    input
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}

/// Produce a short hex hash of a string.
pub(crate) fn short_hash(input: &str) -> String {
    let mut hasher = DefaultHasher::new();
    input.hash(&mut hasher);
    format!("{:08x}", hasher.finish() as u32)
}

/// Sanitize a skill name for filesystem use.
pub(crate) fn sanitize_skill_name(input: &str) -> String {
    input
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}

/// Truncate a string by character count, adding `[truncated]` marker.
#[allow(dead_code)]
pub(crate) fn truncate_chars(input: &str, limit: usize) -> String {
    if input.chars().count() <= limit {
        return input.to_string();
    }
    let mut output = input.chars().take(limit).collect::<String>();
    output.push_str("\n[truncated]");
    output
}

/// Approximate token count using `bytes / 4` heuristic.
pub(crate) fn approx_token_count(text: &str) -> usize {
    text.len().saturating_add(APPROX_BYTES_PER_TOKEN - 1) / APPROX_BYTES_PER_TOKEN
}

/// Truncate text to approximately `max_tokens` tokens, preserving the beginning
/// and end. Middle content is replaced with a `…N tokens truncated…` marker.
pub(crate) fn truncate_middle_approx_tokens(text: &str, max_tokens: usize) -> String {
    if text.is_empty() {
        return String::new();
    }
    if max_tokens == 0 {
        let total = approx_token_count(text);
        return format!("…{total} tokens truncated…");
    }
    let max_bytes = max_tokens.saturating_mul(APPROX_BYTES_PER_TOKEN);
    if text.len() <= max_bytes {
        return text.to_string();
    }
    let total_tokens = approx_token_count(text);
    let keep_bytes = max_bytes.saturating_sub(40);
    let left_budget = keep_bytes / 2;
    let right_budget = keep_bytes - left_budget;
    let mut left_end = left_budget.min(text.len());
    while left_end > 0 && !text.is_char_boundary(left_end) {
        left_end -= 1;
    }
    let mut right_start = text.len().saturating_sub(right_budget);
    while right_start < text.len() && !text.is_char_boundary(right_start) {
        right_start += 1;
    }
    if right_start <= left_end {
        right_start = left_end;
    }
    let removed_tokens = total_tokens.saturating_sub(max_tokens);
    format!(
        "{}…{} tokens truncated…{}",
        &text[..left_end],
        removed_tokens,
        &text[right_start..]
    )
}

/// Atomically write content to a file (write temp then rename).
pub(crate) fn atomic_write(path: &Path, content: &str) -> io::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("file");
    let tmp = parent.join(format!(
        ".{file_name}.{}.{}.tmp",
        std::process::id(),
        now_secs()
    ));
    fs::write(&tmp, content)?;
    if let Ok(file) = fs::OpenOptions::new().read(true).open(&tmp) {
        let _ = file.sync_data();
    }
    fs::rename(&tmp, path).inspect_err(|_| {
        let _ = fs::remove_file(&tmp);
    })
}

/// Append a line to `path` under an exclusive advisory lock.
pub(crate) fn append_line(path: &Path, line: &str) -> Result<(), String> {
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .read(true)
        .open(path)
        .map_err(|error| format!("open {}: {error}", path.display()))?;
    file.lock_exclusive()
        .map_err(|error| format!("lock {}: {error}", path.display()))?;
    let write_result = file
        .write_all(line.as_bytes())
        .and_then(|_| file.flush())
        .and_then(|_| file.sync_data());
    let _ = FileExt::unlock(&file);
    write_result.map_err(|error| format!("append {}: {error}", path.display()))
}

/// Get file length in bytes, returning 0 if file doesn't exist.
pub(crate) fn file_len(path: std::path::PathBuf) -> u64 {
    fs::metadata(path).map(|meta| meta.len()).unwrap_or(0)
}

/// Count directory entries, returning 0 if directory doesn't exist.
pub(crate) fn dir_entry_count(path: std::path::PathBuf) -> usize {
    fs::read_dir(path)
        .map(|entries| entries.filter_map(Result::ok).count())
        .unwrap_or(0)
}
