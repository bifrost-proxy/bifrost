//! Convert standard CommonMark Markdown to Feishu Card JSON 2.0 compatible Markdown.
//!
//! The LLM agent emits standard Markdown, but Feishu cards support a subset with specific
//! differences. This module bridges the gap by transforming content before it's sent.

use std::borrow::Cow;

/// Convert standard Markdown to Feishu Card Markdown (JSON 2.0).
///
/// Handles:
/// - Code-block language alias normalisation
/// - Image URL → text link (Feishu only supports `image_key`)
/// - Horizontal-rule variants → `---`
/// - Task-list checkbox → emoji
/// - Unsupported HTML tag removal (preserve inner text)
/// - Bold+italic `***text***` → `**_text_**`
/// - Footnote removal
pub fn convert_to_feishu_markdown(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut inside_code_block = false;
    let mut code_fence: Option<String> = None; // the fence marker (e.g. "```" or "````")

    for line in input.split('\n') {
        if inside_code_block {
            // Check for closing fence
            if let Some(ref fence) = code_fence {
                let trimmed = line.trim_start();
                if trimmed.starts_with(fence.as_str()) && trimmed[fence.len()..].trim().is_empty() {
                    inside_code_block = false;
                    code_fence = None;
                    output.push_str(line);
                    output.push('\n');
                    continue;
                }
            }
            // Inside code block: pass through unchanged
            output.push_str(line);
            output.push('\n');
            continue;
        }

        // Detect opening code fence
        let trimmed = line.trim_start();
        if let Some(fence_info) = detect_code_fence_open(trimmed) {
            inside_code_block = true;
            code_fence = Some(fence_info.fence.clone());
            // Rewrite the opening line with normalised language
            let indent = &line[..line.len() - trimmed.len()];
            let normalised_lang = normalise_code_lang(&fence_info.lang);
            output.push_str(indent);
            output.push_str(&fence_info.fence);
            if !normalised_lang.is_empty() {
                output.push_str(&normalised_lang);
            }
            output.push('\n');
            continue;
        }

        // Skip footnote definitions  e.g. `[^1]: some text`
        if is_footnote_definition(trimmed) {
            continue;
        }

        let mut processed = Cow::Borrowed(line);

        // Horizontal rule normalisation (must be standalone line)
        if is_horizontal_rule(trimmed) {
            output.push_str("---\n");
            continue;
        }

        // Task list conversion
        processed = convert_task_list(processed);

        // Image URL → text link
        processed = convert_image_urls(processed);

        // Footnote references [^N] → remove
        processed = remove_footnote_refs(processed);

        // Bold+italic ***text*** → **_text_**
        processed = convert_bold_italic(processed);

        // Remove unsupported HTML tags (keep inner text)
        processed = remove_unsupported_html(processed);

        output.push_str(&processed);
        output.push('\n');
    }

    // Remove trailing newline added by the loop (match original behaviour)
    if output.ends_with('\n') && !input.ends_with('\n') {
        output.pop();
    }

    output
}

// ---------------------------------------------------------------------------
// Code block language normalisation
// ---------------------------------------------------------------------------

struct FenceInfo {
    fence: String, // e.g. "```" or "````"
    lang: String,  // e.g. "typescript" or "python {title=x}"
}

fn detect_code_fence_open(trimmed: &str) -> Option<FenceInfo> {
    // Must start with at least 3 backticks or tildes
    let fence_char = trimmed.chars().next()?;
    if fence_char != '`' && fence_char != '~' {
        return None;
    }
    let fence_len = trimmed.chars().take_while(|&c| c == fence_char).count();
    if fence_len < 3 {
        return None;
    }
    let fence = &trimmed[..fence_len];
    let rest = trimmed[fence_len..].trim();
    // For backtick fences, the info string must not contain backticks
    if fence_char == '`' && rest.contains('`') {
        return None;
    }
    Some(FenceInfo {
        fence: fence.to_string(),
        lang: rest.to_string(),
    })
}

fn normalise_code_lang(lang_info: &str) -> String {
    // Strip metadata like `{title="foo"}` after the language name
    let lang = lang_info
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_lowercase();

    match lang.as_str() {
        "" => String::new(),
        // Common aliases → Feishu-supported names
        "ts" => "typescript".into(),
        "js" => "javascript".into(),
        "py" => "python".into(),
        "rb" => "ruby".into(),
        "sh" => "shell".into(),
        "yml" => "yaml".into(),
        "cs" | "csharp" | "c#" => "c_sharp".into(),
        "objc" | "objective-c" => "objective_c".into(),
        "glsl" => "opengl_shading_language".into(),
        "vb" => "visual_basic".into(),
        "coffee" => "coffee_script".into(),
        "plaintext" | "text" | "txt" => "plain_text".into(),
        "dockerfile" => "docker_file".into(),
        "objectivec" => "objective_c".into(),
        "coffeescript" => "coffee_script".into(),
        // Already valid Feishu names, pass through
        _ => lang,
    }
}

// ---------------------------------------------------------------------------
// Image URL → text link
// ---------------------------------------------------------------------------

fn convert_image_urls(line: Cow<'_, str>) -> Cow<'_, str> {
    // Match ![alt](url) where url starts with http
    if !line.contains("![") {
        return line;
    }

    let s = line.as_ref();
    let mut result = String::with_capacity(s.len());
    let mut pos = 0;

    while pos < s.len() {
        // Look for `![` — both `!` and `[` are ASCII so byte check is safe
        if s.as_bytes()[pos] == b'!' && pos + 1 < s.len() && s.as_bytes()[pos + 1] == b'[' {
            // Try to parse ![alt](url)
            if let Some((alt, url, end)) = parse_image_syntax(s, pos + 2) {
                if url.starts_with("http://") || url.starts_with("https://") {
                    // Convert to text link
                    let display = if alt.is_empty() {
                        "🖼 图片链接".to_string()
                    } else {
                        format!("🖼 {}", alt)
                    };
                    result.push_str(&format!("[{}]({})", display, url));
                    pos = end;
                    continue;
                }
            }
        }
        // Advance by one *character* (handles multi-byte UTF-8)
        let ch = s[pos..].chars().next().unwrap();
        result.push(ch);
        pos += ch.len_utf8();
    }

    if result == s {
        line
    } else {
        Cow::Owned(result)
    }
}

/// Parse `alt](url)` starting after `![`.
/// Returns (alt, url, end_position) where end_position is one past the closing `)`.
pub(crate) fn parse_image_syntax(line: &str, start: usize) -> Option<(String, String, usize)> {
    let bytes = line.as_bytes();
    // Find closing `]`
    let mut depth = 1;
    let mut pos = start;
    while pos < bytes.len() {
        match bytes[pos] {
            b'[' => depth += 1,
            b']' => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            _ => {}
        }
        pos += 1;
    }
    if depth != 0 || pos >= bytes.len() {
        return None;
    }
    let alt = &line[start..pos];
    pos += 1; // skip `]`

    // Must be followed by `(`
    if pos >= bytes.len() || bytes[pos] != b'(' {
        return None;
    }
    pos += 1; // skip `(`

    // Find closing `)`
    let url_start = pos;
    let mut paren_depth = 1;
    while pos < bytes.len() {
        match bytes[pos] {
            b'(' => paren_depth += 1,
            b')' => {
                paren_depth -= 1;
                if paren_depth == 0 {
                    break;
                }
            }
            _ => {}
        }
        pos += 1;
    }
    if paren_depth != 0 {
        return None;
    }
    let url = line[url_start..pos].trim();
    Some((alt.to_string(), url.to_string(), pos + 1))
}

// ---------------------------------------------------------------------------
// Horizontal rule normalisation
// ---------------------------------------------------------------------------

fn is_horizontal_rule(trimmed: &str) -> bool {
    if trimmed.is_empty() {
        return false;
    }
    // Standard HR: three or more `-`, `*`, or `_` optionally separated by spaces
    let first_meaningful = trimmed.chars().find(|c| !c.is_whitespace());
    match first_meaningful {
        Some('-') | Some('*') | Some('_') => {}
        _ => return false,
    }
    let marker = first_meaningful.unwrap();
    let only_markers = trimmed.chars().all(|c| c == marker || c.is_whitespace());
    let marker_count = trimmed.chars().filter(|&c| c == marker).count();
    only_markers && marker_count >= 3
}

// ---------------------------------------------------------------------------
// Task list conversion
// ---------------------------------------------------------------------------

fn convert_task_list(line: Cow<'_, str>) -> Cow<'_, str> {
    let trimmed = line.trim_start();
    let indent = &line[..line.len() - trimmed.len()];
    if let Some(rest) = trimmed.strip_prefix("- [ ] ") {
        Cow::Owned(format!("{}- ⬜ {}", indent, rest))
    } else if let Some(rest) = trimmed
        .strip_prefix("- [x] ")
        .or_else(|| trimmed.strip_prefix("- [X] "))
    {
        Cow::Owned(format!("{}- ✅ {}", indent, rest))
    } else {
        line
    }
}

// ---------------------------------------------------------------------------
// Footnote handling
// ---------------------------------------------------------------------------

fn is_footnote_definition(trimmed: &str) -> bool {
    // Pattern: `[^...]:` at start of line
    if !trimmed.starts_with("[^") {
        return false;
    }
    if let Some(bracket_end) = trimmed.find("]:") {
        // Ensure the content between [^ and ]: is a simple reference
        let ref_content = &trimmed[2..bracket_end];
        !ref_content.is_empty() && ref_content.chars().all(|c| c.is_alphanumeric() || c == '-')
    } else {
        false
    }
}

fn remove_footnote_refs(line: Cow<'_, str>) -> Cow<'_, str> {
    if !line.contains("[^") {
        return line;
    }

    let s = line.as_ref();
    let mut result = String::with_capacity(s.len());
    let mut pos = 0;

    while pos < s.len() {
        // `[` and `^` are ASCII so byte check is safe for detecting the marker
        if s.as_bytes()[pos] == b'[' && pos + 1 < s.len() && s.as_bytes()[pos + 1] == b'^' {
            // Try to find closing `]`
            if let Some(end) = s[pos + 2..].find(']') {
                let ref_content = &s[pos + 2..pos + 2 + end];
                if !ref_content.is_empty()
                    && ref_content.chars().all(|c| c.is_alphanumeric() || c == '-')
                {
                    // Skip the footnote ref
                    pos = pos + 2 + end + 1;
                    continue;
                }
            }
        }
        // Advance by one *character* (handles multi-byte UTF-8)
        let ch = s[pos..].chars().next().unwrap();
        result.push(ch);
        pos += ch.len_utf8();
    }

    if result.len() == s.len() {
        line
    } else {
        Cow::Owned(result)
    }
}

// ---------------------------------------------------------------------------
// Bold+italic combo
// ---------------------------------------------------------------------------

fn convert_bold_italic(line: Cow<'_, str>) -> Cow<'_, str> {
    // ***text*** → **_text_** (avoid Feishu rendering issues with triple *)
    if !line.contains("***") {
        return line;
    }

    let s = line.as_ref();
    let mut result = String::with_capacity(s.len());
    let mut pos = 0;

    while pos < s.len() {
        // `*` is ASCII so byte-level check is safe for detecting the marker
        if pos + 3 <= s.len() && &s.as_bytes()[pos..pos + 3] == b"***" {
            // Find the closing ***
            if let Some(close) = s[pos + 3..].find("***") {
                let inner = &s[pos + 3..pos + 3 + close];
                // Only convert if inner doesn't contain newlines and isn't empty
                if !inner.is_empty() && !inner.contains('\n') {
                    result.push_str("**_");
                    result.push_str(inner);
                    result.push_str("_**");
                    pos = pos + 3 + close + 3;
                    continue;
                }
            }
        }
        // Advance by one *character* (handles multi-byte UTF-8)
        let ch = s[pos..].chars().next().unwrap();
        result.push(ch);
        pos += ch.len_utf8();
    }

    if result == s {
        line
    } else {
        Cow::Owned(result)
    }
}

// ---------------------------------------------------------------------------
// HTML tag cleanup
// ---------------------------------------------------------------------------

/// Tags supported by Feishu Card Markdown 2.0
const FEISHU_ALLOWED_TAGS: &[&str] = &[
    "br",
    "hr",
    "font",
    "a",
    "at",
    "person",
    "text_tag",
    "local_datetime",
    "link",
    "raw",
    "number_tag",
    "audio",
    "img",
];

fn remove_unsupported_html(line: Cow<'_, str>) -> Cow<'_, str> {
    if !line.contains('<') {
        return line;
    }

    let mut result = String::with_capacity(line.len());
    let s = line.as_ref();
    let mut pos = 0;

    while pos < s.len() {
        if s.as_bytes()[pos] == b'<' {
            if let Some((tag_name, is_close, end)) = parse_html_tag(s, pos) {
                let tag_lower = tag_name.to_lowercase();
                if is_tag_allowed(&tag_lower) {
                    // Keep the entire tag as-is
                    result.push_str(&s[pos..end]);
                    pos = end;
                    continue;
                } else {
                    // Strip the tag but keep content (for closing tags, nothing to keep)
                    if is_close {
                        // </tag> → skip
                    }
                    // For opening tags, just skip the tag itself; content follows naturally
                    pos = end;
                    continue;
                }
            }
        }
        // Safe: we're working byte-by-byte but only on ASCII '<'
        let ch = s[pos..].chars().next().unwrap();
        result.push(ch);
        pos += ch.len_utf8();
    }

    if result == s {
        line
    } else {
        Cow::Owned(result)
    }
}

fn is_tag_allowed(tag_lower: &str) -> bool {
    FEISHU_ALLOWED_TAGS.contains(&tag_lower)
}

/// Parse an HTML tag at position `pos` (which points to `<`).
/// Returns (tag_name, is_closing_tag, end_position).
fn parse_html_tag(s: &str, pos: usize) -> Option<(String, bool, usize)> {
    let rest = &s[pos..];
    if !rest.starts_with('<') {
        return None;
    }

    // Find closing `>`
    let gt = rest.find('>')?;
    let tag_content = &rest[1..gt]; // content between < and >

    if tag_content.is_empty() {
        return None;
    }

    let is_close = tag_content.starts_with('/');
    let content = if is_close {
        tag_content[1..].trim()
    } else {
        tag_content.trim_end_matches('/')
    };

    // Extract tag name (first word)
    let tag_name = content
        .split(|c: char| c.is_whitespace() || c == '/' || c == '>')
        .next()?
        .to_string();

    if tag_name.is_empty() || !tag_name.chars().next()?.is_alphabetic() {
        return None;
    }

    // Only treat as HTML tag if name is purely alphabetic + underscores
    if !tag_name
        .chars()
        .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
    {
        return None;
    }

    Some((tag_name, is_close, pos + gt + 1))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_code_block_lang_normalization() {
        let input = "```ts\nconst x = 1;\n```";
        let result = convert_to_feishu_markdown(input);
        assert!(result.starts_with("```typescript\n"));
        assert!(result.contains("const x = 1;"));

        let input2 = "```py\nprint('hello')\n```";
        let result2 = convert_to_feishu_markdown(input2);
        assert!(result2.starts_with("```python\n"));

        let input3 = "```csharp\nvar x = 1;\n```";
        let result3 = convert_to_feishu_markdown(input3);
        assert!(result3.starts_with("```c_sharp\n"));

        let input4 = "```yml\nkey: value\n```";
        let result4 = convert_to_feishu_markdown(input4);
        assert!(result4.starts_with("```yaml\n"));

        let input5 = "```sh\necho hello\n```";
        let result5 = convert_to_feishu_markdown(input5);
        assert!(result5.starts_with("```shell\n"));

        let input6 = "```js\nlet x = 1;\n```";
        let result6 = convert_to_feishu_markdown(input6);
        assert!(result6.starts_with("```javascript\n"));

        let input7 = "```plaintext\nhello\n```";
        let result7 = convert_to_feishu_markdown(input7);
        assert!(result7.starts_with("```plain_text\n"));
    }

    #[test]
    fn test_code_block_with_metadata() {
        let input = "```python {title=\"example.py\"}\nprint('hi')\n```";
        let result = convert_to_feishu_markdown(input);
        assert!(result.starts_with("```python\n"));
    }

    #[test]
    fn test_code_block_content_preserved() {
        let input = "```rust\n// ***bold*** and ![img](https://example.com/img.png)\nlet x = \"- [ ] task\";\n```";
        let result = convert_to_feishu_markdown(input);
        assert!(result.contains("***bold***"));
        assert!(result.contains("![img](https://example.com/img.png)"));
        assert!(result.contains("- [ ] task"));
    }

    #[test]
    fn test_image_url_to_link() {
        let input = "Check this: ![screenshot](https://example.com/img.png) here";
        let result = convert_to_feishu_markdown(input);
        assert_eq!(
            result,
            "Check this: [🖼 screenshot](https://example.com/img.png) here"
        );

        let input2 = "![](https://example.com/img.png)";
        let result2 = convert_to_feishu_markdown(input2);
        assert_eq!(result2, "[🖼 图片链接](https://example.com/img.png)");
    }

    #[test]
    fn test_image_key_preserved() {
        // Feishu image_key format should be preserved
        let input = "![hover](img_v2_abc123)";
        let result = convert_to_feishu_markdown(input);
        assert_eq!(result, "![hover](img_v2_abc123)");
    }

    #[test]
    fn test_hr_normalization() {
        assert_eq!(convert_to_feishu_markdown("***"), "---");
        assert_eq!(convert_to_feishu_markdown("___"), "---");
        assert_eq!(convert_to_feishu_markdown("* * *"), "---");
        assert_eq!(convert_to_feishu_markdown("- - -"), "---");
        assert_eq!(convert_to_feishu_markdown("----"), "---");
        // Keep `---` as is
        assert_eq!(convert_to_feishu_markdown("---"), "---");
    }

    #[test]
    fn test_task_list_conversion() {
        let input = "- [ ] todo item\n- [x] done item\n- [X] also done";
        let result = convert_to_feishu_markdown(input);
        assert!(result.contains("- ⬜ todo item"));
        assert!(result.contains("- ✅ done item"));
        assert!(result.contains("- ✅ also done"));
    }

    #[test]
    fn test_task_list_with_indent() {
        let input = "    - [ ] nested todo";
        let result = convert_to_feishu_markdown(input);
        assert_eq!(result, "    - ⬜ nested todo");
    }

    #[test]
    fn test_footnote_removal() {
        let input = "Some text[^1] and more[^note].\n\n[^1]: First note\n[^note]: Second note";
        let result = convert_to_feishu_markdown(input);
        assert!(!result.contains("[^1]"));
        assert!(!result.contains("[^note]"));
        assert!(!result.contains("First note"));
        assert!(!result.contains("Second note"));
        assert!(result.contains("Some text and more."));
    }

    #[test]
    fn test_bold_italic_combo() {
        let input = "This is ***important*** text";
        let result = convert_to_feishu_markdown(input);
        assert_eq!(result, "This is **_important_** text");
    }

    #[test]
    fn test_html_tag_cleanup() {
        // Unsupported tags get stripped
        let input = "Hello <div>world</div> end";
        let result = convert_to_feishu_markdown(input);
        assert_eq!(result, "Hello world end");

        let input2 = "<span style='color:red'>text</span>";
        let result2 = convert_to_feishu_markdown(input2);
        assert_eq!(result2, "text");
    }

    #[test]
    fn test_allowed_html_preserved() {
        let input = "<font color='red'>red text</font>";
        let result = convert_to_feishu_markdown(input);
        assert_eq!(result, "<font color='red'>red text</font>");

        let input2 = "Line 1<br>Line 2";
        let result2 = convert_to_feishu_markdown(input2);
        assert_eq!(result2, "Line 1<br>Line 2");

        let input3 = "<at id=all></at>";
        let result3 = convert_to_feishu_markdown(input3);
        assert_eq!(result3, "<at id=all></at>");
    }

    #[test]
    fn test_mixed_content() {
        let input = r#"# Title

Here's some **bold** and ***bold italic*** text.

```ts
const x = 1;
```

![screenshot](https://example.com/img.png)

- [ ] Task 1
- [x] Task 2

Some text[^1] with footnote.

[^1]: The footnote

---

<div class="note">Important note</div>"#;

        let result = convert_to_feishu_markdown(input);
        assert!(result.contains("# Title"));
        assert!(result.contains("**bold**"));
        assert!(result.contains("**_bold italic_**"));
        assert!(result.starts_with("# Title\n"));
        assert!(result.contains("```typescript\n"));
        assert!(result.contains("[🖼 screenshot](https://example.com/img.png)"));
        assert!(result.contains("- ⬜ Task 1"));
        assert!(result.contains("- ✅ Task 2"));
        assert!(!result.contains("[^1]"));
        assert!(result.contains("---"));
        assert!(!result.contains("<div"));
        assert!(result.contains("Important note"));
    }

    #[test]
    fn test_empty_input() {
        assert_eq!(convert_to_feishu_markdown(""), "");
    }

    #[test]
    fn test_plain_text_passthrough() {
        let input = "Just plain text without any markdown.";
        assert_eq!(convert_to_feishu_markdown(input), input);
    }

    #[test]
    fn test_regular_links_preserved() {
        let input = "[click here](https://example.com)";
        let result = convert_to_feishu_markdown(input);
        assert_eq!(result, input);
    }

    #[test]
    fn test_hr_not_matching_bold() {
        // `**bold**` should NOT be treated as HR
        let input = "**bold text**";
        let result = convert_to_feishu_markdown(input);
        assert_eq!(result, "**bold text**");
    }

    #[test]
    fn test_dockerfile_lang() {
        let input = "```dockerfile\nFROM ubuntu\n```";
        let result = convert_to_feishu_markdown(input);
        assert!(result.starts_with("```docker_file\n"));
    }

    #[test]
    fn test_tilde_code_fence() {
        let input = "~~~python\nprint(1)\n~~~";
        let result = convert_to_feishu_markdown(input);
        assert!(result.starts_with("~~~python\n"));
        assert!(result.contains("print(1)"));
    }

    // -----------------------------------------------------------------------
    // UTF-8 safety tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_utf8_chinese_text_preserved() {
        let input = "这是一段中文文本，包含**粗体**和*斜体*。";
        let result = convert_to_feishu_markdown(input);
        assert_eq!(result, input);
    }

    #[test]
    fn test_utf8_emoji_preserved() {
        let input = "标准emoji 😁😢🌞💼🏆❌✅ 和飞书表情 :DONE:";
        let result = convert_to_feishu_markdown(input);
        assert_eq!(result, input);
    }

    #[test]
    fn test_utf8_image_with_chinese_alt() {
        let input = "查看 ![截图说明](https://example.com/img.png) 效果";
        let result = convert_to_feishu_markdown(input);
        assert_eq!(
            result,
            "查看 [🖼 截图说明](https://example.com/img.png) 效果"
        );
    }

    #[test]
    fn test_utf8_image_with_japanese() {
        let input = "![スクリーンショット](https://example.com/a.png)";
        let result = convert_to_feishu_markdown(input);
        assert_eq!(result, "[🖼 スクリーンショット](https://example.com/a.png)");
    }

    #[test]
    fn test_utf8_bold_italic_chinese() {
        let input = "这是***重要内容***的说明";
        let result = convert_to_feishu_markdown(input);
        assert_eq!(result, "这是**_重要内容_**的说明");
    }

    #[test]
    fn test_utf8_bold_italic_mixed_scripts() {
        let input = "***αβγδ*** and ***日本語テスト***";
        let result = convert_to_feishu_markdown(input);
        assert_eq!(result, "**_αβγδ_** and **_日本語テスト_**");
    }

    #[test]
    fn test_utf8_task_list_chinese() {
        let input = "- [ ] 完成文档翻译\n- [x] 审核代码变更";
        let result = convert_to_feishu_markdown(input);
        assert_eq!(result, "- ⬜ 完成文档翻译\n- ✅ 审核代码变更");
    }

    #[test]
    fn test_utf8_footnote_with_surrounding_chinese() {
        let input = "这段话有脚注[^1]，需要处理。";
        let result = convert_to_feishu_markdown(input);
        assert_eq!(result, "这段话有脚注，需要处理。");
    }

    #[test]
    fn test_utf8_html_tags_with_chinese_content() {
        let input = "<div>这是中文内容</div>后面还有<font color='red'>红色文字</font>";
        let result = convert_to_feishu_markdown(input);
        assert_eq!(
            result,
            "这是中文内容后面还有<font color='red'>红色文字</font>"
        );
    }

    #[test]
    fn test_utf8_code_block_chinese_content() {
        let input = "```py\n# 中文注释\nprint('你好世界')\n```";
        let result = convert_to_feishu_markdown(input);
        assert!(result.starts_with("```python\n"));
        assert!(result.contains("# 中文注释"));
        assert!(result.contains("print('你好世界')"));
    }

    #[test]
    fn test_utf8_korean_text() {
        let input = "한국어 텍스트 with ***강조*** and ![이미지](https://example.com/k.png)";
        let result = convert_to_feishu_markdown(input);
        assert!(result.contains("**_강조_**"));
        assert!(result.contains("[🖼 이미지](https://example.com/k.png)"));
    }

    #[test]
    fn test_utf8_mixed_comprehensive() {
        let input = "# 标题 🎉\n\n中文**粗体**和***重点***内容\n\n```ts\n// 注释：处理中文\nconst msg = '你好';\n```\n\n![示意图](https://example.com/zh.png)\n\n- [ ] 任务一：完成翻译\n- [x] 任务二：代码审查\n\n引用[^1]已处理\n\n[^1]: 脚注内容\n\n---\n\n<div>移除标签</div>保留<font color='red'>红色</font>";
        let result = convert_to_feishu_markdown(input);
        assert!(result.contains("# 标题 🎉"));
        assert!(result.contains("中文**粗体**和**_重点_**内容"));
        assert!(result.contains("```typescript\n"));
        assert!(result.contains("const msg = '你好';"));
        assert!(result.contains("[🖼 示意图](https://example.com/zh.png)"));
        assert!(result.contains("- ⬜ 任务一：完成翻译"));
        assert!(result.contains("- ✅ 任务二：代码审查"));
        assert!(!result.contains("[^1]"));
        assert!(!result.contains("脚注内容"));
        assert!(result.contains("---"));
        assert!(!result.contains("<div>"));
        assert!(result.contains("移除标签保留<font color='red'>红色</font>"));
    }

    #[test]
    fn test_utf8_4byte_chars() {
        // 4-byte UTF-8 characters (emoji, rare CJK, etc.)
        let input = "𝄞 music note and 𠀀 rare CJK and 🇨🇳 flag";
        let result = convert_to_feishu_markdown(input);
        assert_eq!(result, input);
    }

    #[test]
    fn test_utf8_mixed_with_image_adjacent() {
        // Image syntax immediately adjacent to multi-byte chars
        let input = "中文![图](https://a.com/b.png)日本語";
        let result = convert_to_feishu_markdown(input);
        assert_eq!(result, "中文[🖼 图](https://a.com/b.png)日本語");
    }

    #[test]
    fn test_utf8_bold_italic_with_emoji_inside() {
        let input = "***🔥重要🔥***";
        let result = convert_to_feishu_markdown(input);
        assert_eq!(result, "**_🔥重要🔥_**");
    }

    #[test]
    fn test_utf8_html_tag_between_multibyte() {
        let input = "你好<span>世界</span>再见";
        let result = convert_to_feishu_markdown(input);
        assert_eq!(result, "你好世界再见");
    }
}
