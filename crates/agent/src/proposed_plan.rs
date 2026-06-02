//! Extraction helpers for Plan Mode `<proposed_plan>` blocks.

const OPEN_TAG: &str = "<proposed_plan>";
const CLOSE_TAG: &str = "</proposed_plan>";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProposedPlanExtraction {
    pub visible_text: String,
    pub proposed_plan: Option<String>,
}

pub fn extract_proposed_plan(text: &str) -> ProposedPlanExtraction {
    let mut visible_text = String::new();
    let mut proposed_plan = None;
    let mut remaining = text;

    while let Some(open_index) = find_line_tag(remaining, OPEN_TAG) {
        visible_text.push_str(&remaining[..open_index]);
        let after_open = &remaining[open_index + OPEN_TAG.len()..];
        let after_open = after_open.strip_prefix('\n').unwrap_or(after_open);
        match find_line_tag(after_open, CLOSE_TAG) {
            Some(close_index) => {
                proposed_plan = Some(after_open[..close_index].to_string());
                remaining = &after_open[close_index + CLOSE_TAG.len()..];
                if let Some(rest) = remaining.strip_prefix('\n') {
                    remaining = rest;
                }
            }
            None => {
                proposed_plan = Some(after_open.to_string());
                remaining = "";
                break;
            }
        }
    }

    visible_text.push_str(remaining);
    ProposedPlanExtraction {
        visible_text,
        proposed_plan,
    }
}

fn find_line_tag(text: &str, tag: &str) -> Option<usize> {
    let mut offset = 0;
    for segment in text.split_inclusive('\n') {
        let line = segment.strip_suffix('\n').unwrap_or(segment);
        if line == tag {
            return Some(offset);
        }
        offset += segment.len();
    }
    if text
        .rsplit_once('\n')
        .map(|(_, line)| line == tag)
        .unwrap_or_else(|| text == tag)
    {
        return Some(text.len().saturating_sub(tag.len()));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_plan_and_strips_visible_text() {
        let result = extract_proposed_plan(
            "before\n<proposed_plan>\n- inspect\n- implement\n</proposed_plan>\nafter",
        );
        assert_eq!(result.visible_text, "before\nafter");
        assert_eq!(
            result.proposed_plan,
            Some("- inspect\n- implement\n".to_string())
        );
    }

    #[test]
    fn ignores_tags_with_extra_text() {
        let result = extract_proposed_plan("before\n<proposed_plan> extra\ninside");
        assert_eq!(result.visible_text, "before\n<proposed_plan> extra\ninside");
        assert_eq!(result.proposed_plan, None);
    }

    #[test]
    fn closes_unterminated_plan_at_end() {
        let result = extract_proposed_plan("<proposed_plan>\n- step\n");
        assert_eq!(result.visible_text, "");
        assert_eq!(result.proposed_plan, Some("- step\n".to_string()));
    }
}
