//! Memory parsing utilities: JSON extraction, markdown fences, and schemas.

use crate::memory::types::{ConsolidatedMemory, ExtractedMemories};

/// JSON Schema for Phase 1 structured output.
pub(crate) fn phase1_output_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "rollout_summary": { "type": "string" },
            "rollout_slug": { "type": ["string", "null"] },
            "raw_memory": { "type": "string" }
        },
        "required": ["rollout_summary", "rollout_slug", "raw_memory"],
        "additionalProperties": false
    })
}

/// Parse Phase 1 extracted memories from raw model output.
pub(crate) fn parse_extracted_memories(content: &str) -> ExtractedMemories {
    let trimmed = strip_markdown_fences(content.trim());
    serde_json::from_str::<ExtractedMemories>(trimmed)
        .or_else(|_| {
            extract_balanced_json(trimmed)
                .ok_or_else(|| serde_json::Error::io(std::io::Error::other("missing json")))
                .and_then(serde_json::from_str::<ExtractedMemories>)
        })
        .unwrap_or(ExtractedMemories {
            raw_memory: None,
            rollout_summary: None,
            rollout_slug: None,
        })
}

/// Parse Phase 2 consolidated memory output.
pub(crate) fn parse_consolidated_memory(content: &str) -> Result<ConsolidatedMemory, String> {
    let trimmed = strip_markdown_fences(content.trim());
    serde_json::from_str::<ConsolidatedMemory>(trimmed)
        .or_else(|_| {
            extract_balanced_json(trimmed)
                .ok_or_else(|| "no JSON object found".to_string())
                .and_then(|json| {
                    serde_json::from_str::<ConsolidatedMemory>(json)
                        .map_err(|e| format!("parse consolidated memory: {e}"))
                })
        })
        .map_err(|e| format!("parse consolidated memory: {e}"))
}

/// Strip a leading ```json / ```JSON / ``` fence and its trailing ``` if present.
pub(crate) fn strip_markdown_fences(content: &str) -> &str {
    let trimmed = content.trim();
    if let Some(rest) = trimmed.strip_prefix("```") {
        let after_lang = rest.find('\n').map(|idx| &rest[idx + 1..]).unwrap_or(rest);
        if let Some(end) = after_lang.rfind("```") {
            return after_lang[..end].trim();
        }
        return after_lang.trim();
    }
    trimmed
}

/// Find the first balanced JSON object/array in the given text.
pub(crate) fn extract_balanced_json(content: &str) -> Option<&str> {
    let bytes = content.as_bytes();
    let mut start = None;
    let mut depth: i32 = 0;
    let mut in_string = false;
    let mut escape = false;
    let mut opener = b'{';
    for (i, &b) in bytes.iter().enumerate() {
        if in_string {
            if escape {
                escape = false;
            } else if b == b'\\' {
                escape = true;
            } else if b == b'"' {
                in_string = false;
            }
            continue;
        }
        match b {
            b'"' => in_string = true,
            b'{' | b'[' => {
                if start.is_none() {
                    start = Some(i);
                    opener = b;
                }
                depth += 1;
            }
            b'}' | b']' if start.is_some() => {
                let closer = if opener == b'{' { b'}' } else { b']' };
                depth -= 1;
                if depth == 0 {
                    if b == closer {
                        let s = start.unwrap();
                        return Some(&content[s..=i]);
                    }
                    start = None;
                    depth = 0;
                }
            }
            _ => {}
        }
    }
    None
}
