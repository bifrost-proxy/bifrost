//! UTF-8 safe text truncation helpers.
//!
//! Inspired by Codex's `codex_utils_string::truncate` module.

/// Return the closest byte index at or before `index` that is a UTF-8
/// character boundary in `s`.
pub fn floor_char_boundary(s: &str, index: usize) -> usize {
    if index >= s.len() {
        return s.len();
    }

    let mut boundary = index;
    while boundary > 0 && !s.is_char_boundary(boundary) {
        boundary -= 1;
    }
    boundary
}

/// Return the closest byte index at or after `index` that is a UTF-8
/// character boundary in `s`.
pub fn ceil_char_boundary(s: &str, index: usize) -> usize {
    if index >= s.len() {
        return s.len();
    }

    let mut boundary = index;
    while boundary < s.len() && !s.is_char_boundary(boundary) {
        boundary += 1;
    }
    boundary
}

/// Return a prefix whose byte length is at most `max_bytes` without splitting a
/// UTF-8 character.
pub fn truncate_bytes(s: &str, max_bytes: usize) -> String {
    let end = floor_char_boundary(s, max_bytes);
    s[..end].to_string()
}

/// Return a byte-budgeted prefix plus `suffix` when truncation is needed.
pub fn truncate_bytes_with_suffix(s: &str, max_bytes: usize, suffix: &str) -> String {
    if s.len() <= max_bytes {
        return s.to_string();
    }

    let end = floor_char_boundary(s, max_bytes);
    format!("{}{}", &s[..end], suffix)
}

/// Return a prefix containing at most `max_chars` Unicode scalar values.
pub fn truncate_chars(s: &str, max_chars: usize) -> String {
    s.chars().take(max_chars).collect()
}

/// Return a character-budgeted prefix plus `suffix` when truncation is needed.
pub fn truncate_chars_with_suffix(s: &str, max_chars: usize, suffix: &str) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }

    format!("{}{}", truncate_chars(s, max_chars), suffix)
}

/// Return a character-budgeted prefix plus `...` when truncation is needed.
pub fn truncate_chars_with_ellipsis(s: &str, max_chars: usize) -> String {
    truncate_chars_with_suffix(s, max_chars, "...")
}

/// Truncate a string in the middle, keeping a prefix and suffix on UTF-8
/// character boundaries — inspired by Codex's `split_string` +
/// `truncate_middle_chars`.
///
/// Returns the original string unchanged when it fits within `max_bytes`.
pub fn truncate_middle_bytes(s: &str, max_bytes: usize) -> String {
    if s.len() <= max_bytes || max_bytes == 0 {
        if max_bytes == 0 && !s.is_empty() {
            let total_chars = s.chars().count();
            return format!("…{total_chars} chars truncated…");
        }
        return s.to_string();
    }

    let left_budget = max_bytes / 2;
    let right_budget = max_bytes.saturating_sub(left_budget);

    let head_end = floor_char_boundary(s, left_budget);
    let tail_start = ceil_char_boundary(s, s.len().saturating_sub(right_budget));
    // Ensure tail_start is not before head_end (overlapping budgets)
    let tail_start = tail_start.max(head_end);

    let removed_chars = s[head_end..tail_start].chars().count();
    let head = &s[..head_end];
    let tail = &s[tail_start..];
    format!("{head}…{removed_chars} chars truncated…{tail}")
}

/// Maximum file size allowed for read operations (512 MiB, matching Codex).
pub const MAX_READ_FILE_BYTES: u64 = 512 * 1024 * 1024;

/// Check file size before reading. Returns an error string if the file
/// exceeds the limit.
pub fn check_file_size(path: &std::path::Path, max_bytes: u64) -> Result<u64, String> {
    let metadata = std::fs::metadata(path).map_err(|e| format!("stat file: {e}"))?;
    let size = metadata.len();
    if size > max_bytes {
        return Err(format!(
            "file is too large to read ({} bytes): limit is {} bytes",
            size, max_bytes
        ));
    }
    Ok(size)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floor_char_boundary_handles_multibyte_middle() {
        let s = "ab前cd";
        assert_eq!(floor_char_boundary(s, 0), 0);
        assert_eq!(floor_char_boundary(s, s.len()), s.len());
        assert_eq!(floor_char_boundary(s, 3), 2);
        assert_eq!(&s[..floor_char_boundary(s, 3)], "ab");
    }

    #[test]
    fn ceil_char_boundary_handles_multibyte_middle() {
        let s = "ab前cd";
        // '前' is 3 bytes at position 2..5
        assert_eq!(ceil_char_boundary(s, 3), 5);
        assert_eq!(ceil_char_boundary(s, 0), 0);
        assert_eq!(ceil_char_boundary(s, s.len()), s.len());
        assert_eq!(ceil_char_boundary(s, s.len() + 10), s.len());
    }

    #[test]
    fn truncate_bytes_with_suffix_does_not_split_chinese() {
        let s = "ab前cd";
        assert_eq!(truncate_bytes_with_suffix(s, 3, "..."), "ab...");
    }

    #[test]
    fn truncate_bytes_with_suffix_keeps_ascii_boundary() {
        assert_eq!(truncate_bytes_with_suffix("abcdef", 3, "..."), "abc...");
    }

    #[test]
    fn truncate_chars_with_suffix_counts_multibyte_as_one_char() {
        assert_eq!(
            truncate_chars_with_suffix("ab前🙂cd", 4, "..."),
            "ab前🙂..."
        );
    }

    #[test]
    fn truncate_chars_with_suffix_returns_original_when_short() {
        assert_eq!(truncate_chars_with_suffix("前🙂", 2, "..."), "前🙂");
    }

    #[test]
    fn truncate_middle_bytes_returns_original_when_under_limit() {
        assert_eq!(truncate_middle_bytes("short", 100), "short");
    }

    #[test]
    fn truncate_middle_bytes_removes_middle_ascii() {
        let out = truncate_middle_bytes("abcdefghij", 6);
        // left_budget = 3, right_budget = 3 → keeps "abc" + "hij"
        assert!(out.contains("abc"));
        assert!(out.contains("hij"));
        assert!(out.contains("chars truncated"));
    }

    #[test]
    fn truncate_middle_bytes_respects_utf8_boundaries() {
        // "😀" is 4 bytes
        let s = "😀😀😀😀😀";
        let out = truncate_middle_bytes(s, 10);
        // left_budget = 5, right_budget = 5
        // floor_char_boundary(s, 5) = 4 (end of first 😀)
        // ceil_char_boundary(s, 15) = 16 (start of last 😀)
        assert!(out.contains("😀"));
        assert!(out.contains("chars truncated"));
        // Must not panic
    }

    #[test]
    fn truncate_middle_bytes_zero_budget() {
        let out = truncate_middle_bytes("hello", 0);
        assert!(out.contains("5 chars truncated"));
    }

    #[test]
    fn check_file_size_returns_error_for_missing_file() {
        let result = check_file_size(
            std::path::Path::new("/nonexistent/file"),
            MAX_READ_FILE_BYTES,
        );
        assert!(result.is_err());
    }
}
