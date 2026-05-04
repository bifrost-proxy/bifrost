//! Memory citation parsing for the Bifrost Agent memory system.
//!
//! Parses `<oai-mem-citation>` blocks from model responses to extract
//! citation entries and rollout IDs for memory usage tracking.

/// A single citation entry referencing a file and line range.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CitationEntry {
    /// Relative file path (e.g., "MEMORY.md", "rollout_summaries/...")
    pub file: String,
    /// Start line number (1-based)
    pub line_start: usize,
    /// End line number (1-based, inclusive)
    pub line_end: usize,
    /// Short note describing how the memory was used
    pub note: String,
}

/// Parsed memory citation block.
#[derive(Debug, Clone, Default)]
pub struct MemoryCitation {
    /// Citation entries for rendering
    pub entries: Vec<CitationEntry>,
    /// Rollout IDs referenced in the citation
    pub rollout_ids: Vec<String>,
}

const CITATION_OPEN: &str = "<oai-mem-citation>";
const CITATION_CLOSE: &str = "</oai-mem-citation>";
const ENTRIES_OPEN: &str = "<citation_entries>";
const ENTRIES_CLOSE: &str = "</citation_entries>";
const ROLLOUT_IDS_OPEN: &str = "<rollout_ids>";
const ROLLOUT_IDS_CLOSE: &str = "</rollout_ids>";

/// Parse an `<oai-mem-citation>` block from model output text.
/// If multiple citation blocks exist, takes the last one (matching Codex behavior).
/// Returns `None` if no citation block is found or if parsed content is empty.
pub fn parse_memory_citation(text: &str) -> Option<MemoryCitation> {
    let block = extract_last_block(text, CITATION_OPEN, CITATION_CLOSE)?;

    let mut entries = Vec::new();
    let mut rollout_ids = Vec::new();

    if let Some(entries_block) = extract_block(block, ENTRIES_OPEN, ENTRIES_CLOSE) {
        entries.extend(entries_block.lines().filter_map(parse_citation_entry));
    }

    if let Some(ids_block) = extract_block(block, ROLLOUT_IDS_OPEN, ROLLOUT_IDS_CLOSE) {
        for line in ids_block.lines().map(str::trim).filter(|l| !l.is_empty()) {
            if looks_like_uuid(line) {
                rollout_ids.push(line.to_string());
            }
        }
    }

    if entries.is_empty() && rollout_ids.is_empty() {
        None
    } else {
        Some(MemoryCitation {
            entries,
            rollout_ids,
        })
    }
}

/// Extract just the rollout IDs from a citation block.
/// Convenience method for tracking which rollouts the model found useful.
pub fn rollout_ids_from_citation(text: &str) -> Vec<String> {
    parse_memory_citation(text)
        .map(|c| c.rollout_ids)
        .unwrap_or_default()
}

/// Strip citation blocks from text, returning clean content.
/// Useful for displaying responses without citation metadata.
pub fn strip_citations(text: &str) -> String {
    let mut result = text.to_string();
    while let Some(start) = result.find(CITATION_OPEN) {
        if let Some(end_tag_start) = result[start..].find(CITATION_CLOSE) {
            let end = start + end_tag_start + CITATION_CLOSE.len();
            result.replace_range(start..end, "");
        } else {
            // Malformed: no closing tag, remove from open tag to end
            result.truncate(start);
            break;
        }
    }
    result.trim_end().to_string()
}

fn parse_citation_entry(line: &str) -> Option<CitationEntry> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }

    let (location, note_raw) = line.rsplit_once("|note=[")?;
    let note = note_raw.strip_suffix(']')?.trim().to_string();
    let (path, line_range) = location.rsplit_once(':')?;
    let path = path.trim();
    // Skip entries with empty file path (e.g., ":1-2|note=[...]")
    if path.is_empty() {
        return None;
    }
    let (start_str, end_str) = line_range.split_once('-')?;

    Some(CitationEntry {
        file: path.to_string(),
        line_start: start_str.trim().parse().ok()?,
        line_end: end_str.trim().parse().ok()?,
        note,
    })
}

fn extract_block<'a>(text: &'a str, open: &str, close: &str) -> Option<&'a str> {
    let (_, rest) = text.split_once(open)?;
    let (body, _) = rest.split_once(close)?;
    Some(body)
}

fn extract_last_block<'a>(text: &'a str, open: &str, close: &str) -> Option<&'a str> {
    let start = text.rfind(open)?;
    let rest = &text[start + open.len()..];
    let (body, _) = rest.split_once(close)?;
    Some(body)
}

fn looks_like_uuid(s: &str) -> bool {
    // UUID format: 8-4-4-4-12 hex chars (with dashes)
    let s = s.trim();
    if s.len() < 32 {
        return false;
    }
    s.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_CITATION: &str = r#"Here is my response about the feature.

<oai-mem-citation>
<citation_entries>
MEMORY.md:234-236|note=[responsesapi citation extraction code pointer]
rollout_summaries/2026-02-17T21-23-02-LN3m-weekly_memory_report.md:10-12|note=[weekly report format]
</citation_entries>
<rollout_ids>
019c6e27-e55b-73d1-87d8-4e01f1f75043
019c7714-3b77-74d1-9866-e1f484aae2ab
</rollout_ids>
</oai-mem-citation>"#;

    #[test]
    fn test_parse_valid_citation() {
        let result = parse_memory_citation(SAMPLE_CITATION).unwrap();
        assert_eq!(result.entries.len(), 2);

        assert_eq!(result.entries[0].file, "MEMORY.md");
        assert_eq!(result.entries[0].line_start, 234);
        assert_eq!(result.entries[0].line_end, 236);
        assert_eq!(
            result.entries[0].note,
            "responsesapi citation extraction code pointer"
        );

        assert_eq!(
            result.entries[1].file,
            "rollout_summaries/2026-02-17T21-23-02-LN3m-weekly_memory_report.md"
        );
        assert_eq!(result.entries[1].line_start, 10);
        assert_eq!(result.entries[1].line_end, 12);
        assert_eq!(result.entries[1].note, "weekly report format");

        assert_eq!(result.rollout_ids.len(), 2);
        assert_eq!(
            result.rollout_ids[0],
            "019c6e27-e55b-73d1-87d8-4e01f1f75043"
        );
        assert_eq!(
            result.rollout_ids[1],
            "019c7714-3b77-74d1-9866-e1f484aae2ab"
        );
    }

    #[test]
    fn test_parse_citation_empty_rollout_ids() {
        let text = r#"<oai-mem-citation>
<citation_entries>
MEMORY.md:1-5|note=[some note]
</citation_entries>
<rollout_ids>
</rollout_ids>
</oai-mem-citation>"#;

        let result = parse_memory_citation(text).unwrap();
        assert_eq!(result.entries.len(), 1);
        assert!(result.rollout_ids.is_empty());
    }

    #[test]
    fn test_parse_citation_malformed_entries_skipped() {
        let text = r#"<oai-mem-citation>
<citation_entries>
MEMORY.md:234-236|note=[valid entry]
this is not a valid line
another:bad
:1-2|note=[missing file path before colon]
good_file.md:10-20|note=[another valid one]
</citation_entries>
<rollout_ids>
019c6e27-e55b-73d1-87d8-4e01f1f75043
</rollout_ids>
</oai-mem-citation>"#;

        let result = parse_memory_citation(text).unwrap();
        assert_eq!(result.entries.len(), 2);
        assert_eq!(result.entries[0].file, "MEMORY.md");
        assert_eq!(result.entries[1].file, "good_file.md");
    }

    #[test]
    fn test_no_citation_block_returns_none() {
        let text = "This is just a regular response with no citation.";
        assert!(parse_memory_citation(text).is_none());
    }

    #[test]
    fn test_strip_citations() {
        let text = "Hello world.\n\n<oai-mem-citation>\n<citation_entries>\nMEMORY.md:1-2|note=[test]\n</citation_entries>\n<rollout_ids>\n</rollout_ids>\n</oai-mem-citation>";
        let stripped = strip_citations(text);
        assert_eq!(stripped, "Hello world.");
        assert!(!stripped.contains("oai-mem-citation"));
    }

    #[test]
    fn test_strip_citations_preserves_content_before_and_after() {
        let text = "Before.\n<oai-mem-citation>\n<citation_entries>\nf.md:1-1|note=[x]\n</citation_entries>\n<rollout_ids>\n</rollout_ids>\n</oai-mem-citation>\nAfter.";
        let stripped = strip_citations(text);
        assert!(stripped.contains("Before."));
        assert!(stripped.contains("After."));
        assert!(!stripped.contains("oai-mem-citation"));
    }

    #[test]
    fn test_multiple_citation_blocks_takes_last() {
        let text = r#"<oai-mem-citation>
<citation_entries>
first.md:1-1|note=[first]
</citation_entries>
<rollout_ids>
</rollout_ids>
</oai-mem-citation>
Some middle text.
<oai-mem-citation>
<citation_entries>
second.md:2-3|note=[second]
</citation_entries>
<rollout_ids>
aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee
</rollout_ids>
</oai-mem-citation>"#;

        let result = parse_memory_citation(text).unwrap();
        assert_eq!(result.entries.len(), 1);
        assert_eq!(result.entries[0].file, "second.md");
        assert_eq!(result.entries[0].note, "second");
        assert_eq!(result.rollout_ids.len(), 1);
        assert_eq!(
            result.rollout_ids[0],
            "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"
        );
    }

    #[test]
    fn test_rollout_ids_from_citation() {
        let ids = rollout_ids_from_citation(SAMPLE_CITATION);
        assert_eq!(ids.len(), 2);
        assert_eq!(ids[0], "019c6e27-e55b-73d1-87d8-4e01f1f75043");
    }

    #[test]
    fn test_rollout_ids_filters_non_uuid() {
        let text = r#"<oai-mem-citation>
<citation_entries>
f.md:1-1|note=[x]
</citation_entries>
<rollout_ids>
019c6e27-e55b-73d1-87d8-4e01f1f75043
not a uuid at all!
another-bad-value
</rollout_ids>
</oai-mem-citation>"#;

        let result = parse_memory_citation(text).unwrap();
        assert_eq!(result.rollout_ids.len(), 1);
        assert_eq!(
            result.rollout_ids[0],
            "019c6e27-e55b-73d1-87d8-4e01f1f75043"
        );
    }
}
