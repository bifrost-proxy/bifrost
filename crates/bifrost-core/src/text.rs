//! UTF-8 safe text truncation helpers.

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
}
