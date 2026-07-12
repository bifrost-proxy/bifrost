use bytes::Bytes;
use serde_json::Value;
use tracing::debug;

use super::compress::compress_body;
use super::decompress::try_decompress_body_with_limit;
use crate::server::{RegexReplace, ResolvedRules};
use crate::utils::logging::RequestContext;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Phase {
    Request,
    Response,
}

fn is_binary_content_type(content_type: &str) -> bool {
    let ct = content_type.to_lowercase();
    ct.starts_with("image/")
        || ct.starts_with("audio/")
        || ct.starts_with("video/")
        || ct.starts_with("application/octet-stream")
        || ct.starts_with("application/pdf")
        || ct.starts_with("application/zip")
        || ct.starts_with("application/gzip")
        || ct.starts_with("application/x-tar")
        || ct.starts_with("application/x-rar")
        || ct.starts_with("application/x-7z")
        || ct.starts_with("application/wasm")
        || ct.starts_with("font/")
        || ct.contains("protobuf")
        || ct.contains("grpc")
}

pub fn apply_body_rules(
    body: Bytes,
    rules: &ResolvedRules,
    phase: Phase,
    content_type: Option<&str>,
    verbose_logging: bool,
    ctx: &RequestContext,
) -> Bytes {
    let skip_text_operations = content_type.map(is_binary_content_type).unwrap_or(false);
    let mut result = body;

    let (prepend, append, replace, replace_regex, merge, body_override) = match phase {
        Phase::Request => (
            &rules.req_prepend,
            &rules.req_append,
            &rules.req_replace,
            &rules.req_replace_regex,
            &rules.req_merge,
            &rules.req_body,
        ),
        Phase::Response => (
            &rules.res_prepend,
            &rules.res_append,
            &rules.res_replace,
            &rules.res_replace_regex,
            &rules.res_merge,
            &rules.res_body,
        ),
    };

    if let Some(override_body) = body_override {
        let mut replacement = override_body.clone();
        // Expand response-phase template variables (e.g. ${statusCode}, ${resHeaders.x})
        // left literal by the core engine at request phase. Only for a response-phase
        // body override whose ctx carries real response data, and only for UTF-8 text.
        if phase == Phase::Response && ctx.res_status.is_some() {
            if let Ok(text) = std::str::from_utf8(&replacement) {
                if text.contains("${") {
                    let expanded = crate::transform::response::expand_response_vars(text, ctx);
                    replacement = Bytes::from(expanded);
                }
            }
        }
        if verbose_logging {
            debug!(
                "[{}] [{:?}_BODY] replaced: {} bytes -> {} bytes",
                ctx.id_str(),
                phase,
                result.len(),
                replacement.len()
            );
        }
        result = replacement;
    }

    if let Some(prepend_data) = prepend {
        let new_len = prepend_data.len() + result.len();
        let mut new_body = Vec::with_capacity(new_len);
        new_body.extend_from_slice(prepend_data);
        new_body.extend_from_slice(&result);
        result = new_body.into();
        if verbose_logging {
            debug!(
                "[{}] [{:?}_PREPEND] prepended {} bytes",
                ctx.id_str(),
                phase,
                prepend_data.len()
            );
        }
    }

    if let Some(append_data) = append {
        let new_len = result.len() + append_data.len();
        let mut new_body = Vec::with_capacity(new_len);
        new_body.extend_from_slice(&result);
        new_body.extend_from_slice(append_data);
        result = new_body.into();
        if verbose_logging {
            debug!(
                "[{}] [{:?}_APPEND] appended {} bytes",
                ctx.id_str(),
                phase,
                append_data.len()
            );
        }
    }

    if !replace.is_empty() && !skip_text_operations {
        let mut body_str = String::from_utf8_lossy(&result).into_owned();
        for (from, to) in replace {
            body_str = body_str.replace(from.as_str(), to.as_str());
        }
        result = body_str.into_bytes().into();
        if verbose_logging {
            debug!(
                "[{}] [{:?}_REPLACE] applied {} string replacements",
                ctx.id_str(),
                phase,
                replace.len()
            );
        }
    } else if !replace.is_empty() && skip_text_operations && verbose_logging {
        debug!(
            "[{}] [{:?}_REPLACE] skipped {} string replacements for binary content type",
            ctx.id_str(),
            phase,
            replace.len()
        );
    }

    if !replace_regex.is_empty() && !skip_text_operations {
        let mut body_str = String::from_utf8_lossy(&result).into_owned();
        for regex_rule in replace_regex {
            body_str = apply_regex_replace(&body_str, regex_rule);
        }
        result = body_str.into_bytes().into();
        if verbose_logging {
            debug!(
                "[{}] [{:?}_REPLACE_REGEX] applied {} regex replacements",
                ctx.id_str(),
                phase,
                replace_regex.len()
            );
        }
    } else if !replace_regex.is_empty() && skip_text_operations && verbose_logging {
        debug!(
            "[{}] [{:?}_REPLACE_REGEX] skipped {} regex replacements for binary content type",
            ctx.id_str(),
            phase,
            replace_regex.len()
        );
    }

    if let Some(merge_value) = merge {
        let content_type_lower = content_type.unwrap_or_default().to_ascii_lowercase();
        if content_type_lower.starts_with("application/x-www-form-urlencoded") {
            if let Some(merged_form) = merge_form_urlencoded(&result, merge_value) {
                result = merged_form;
                if verbose_logging {
                    debug!("[{}] [{:?}_MERGE] merged form body", ctx.id_str(), phase);
                }
            }
        } else if let Ok(original) = serde_json::from_slice::<Value>(&result) {
            let merged = merge_json(original, merge_value.clone());
            if let Ok(merged_str) = serde_json::to_string(&merged) {
                result = merged_str.into_bytes().into();
                if verbose_logging {
                    debug!("[{}] [{:?}_MERGE] merged JSON", ctx.id_str(), phase);
                }
            }
        }
    }

    result
}

fn apply_regex_replace(input: &str, rule: &RegexReplace) -> String {
    if rule.global {
        rule.pattern
            .replace_all(input, rule.replacement.as_str())
            .into_owned()
    } else {
        rule.pattern
            .replace(input, rule.replacement.as_str())
            .into_owned()
    }
}

fn merge_json(base: Value, patch: Value) -> Value {
    match (base, patch) {
        (Value::Object(mut base_map), Value::Object(patch_map)) => {
            for (key, value) in patch_map {
                let base_value = base_map.remove(&key).unwrap_or(Value::Null);
                base_map.insert(key, merge_json(base_value, value));
            }
            Value::Object(base_map)
        }
        (_, patch) => patch,
    }
}

fn merge_form_urlencoded(body: &[u8], patch: &Value) -> Option<Bytes> {
    let Value::Object(patch_map) = patch else {
        return None;
    };

    let mut pairs: Vec<(String, String)> = url::form_urlencoded::parse(body).into_owned().collect();

    for (key, value) in patch_map {
        let merged_value = match value {
            Value::String(s) => s.clone(),
            Value::Number(n) => n.to_string(),
            Value::Bool(v) => v.to_string(),
            Value::Null => String::new(),
            other => other.to_string(),
        };

        if let Some(existing) = pairs
            .iter_mut()
            .find(|(existing_key, _)| existing_key == key)
        {
            existing.1 = merged_value;
        } else {
            pairs.push((key.clone(), merged_value));
        }
    }

    let encoded = pairs
        .into_iter()
        .fold(
            url::form_urlencoded::Serializer::new(String::new()),
            |mut serializer, (k, v)| {
                serializer.append_pair(&k, &v);
                serializer
            },
        )
        .finish();

    Some(Bytes::from(encoded))
}

pub fn apply_content_injection(
    body: Bytes,
    content_type: &str,
    rules: &ResolvedRules,
    verbose_logging: bool,
    ctx: &RequestContext,
) -> Bytes {
    let content_type_lower = content_type.to_lowercase();

    if content_type_lower.contains("text/html") || content_type_lower.contains("application/xhtml")
    {
        return apply_html_injection(body, rules, verbose_logging, ctx);
    }

    if content_type_lower.contains("javascript")
        || content_type_lower.contains("text/js")
        || content_type_lower.contains("application/x-javascript")
    {
        return apply_js_injection(body, rules, verbose_logging, ctx);
    }

    if content_type_lower.contains("text/css") {
        return apply_css_injection(body, rules, verbose_logging, ctx);
    }

    body
}

pub struct ContentInjectionResult {
    pub body: Bytes,
    pub content_encoding: Option<String>,
}

pub struct ContentInjectionEncoding<'a> {
    pub source: Option<&'a str>,
    pub output: Option<&'a str>,
    pub max_decompress_output_bytes: usize,
}

pub fn apply_body_rules_preserving_encoding(
    body: Bytes,
    rules: &ResolvedRules,
    phase: Phase,
    content_type: Option<&str>,
    encoding: ContentInjectionEncoding<'_>,
    verbose_logging: bool,
    ctx: &RequestContext,
) -> ContentInjectionResult {
    let source_content_encoding = encoding
        .source
        .filter(|encoding| !encoding.eq_ignore_ascii_case("identity"));
    let output_content_encoding = encoding
        .output
        .filter(|encoding| !encoding.eq_ignore_ascii_case("identity"));

    let Some(source_content_encoding) = source_content_encoding else {
        let processed = apply_body_rules(body, rules, phase, content_type, verbose_logging, ctx);
        return match output_content_encoding {
            Some(output_content_encoding) => {
                match compress_body(processed.as_ref(), output_content_encoding) {
                    Ok(compressed) => ContentInjectionResult {
                        body: Bytes::from(compressed),
                        content_encoding: Some(output_content_encoding.to_string()),
                    },
                    Err(e) => {
                        debug!(
                            "[{}] [{:?}_BODY] Failed to compress {} body ({}), fallback to identity",
                            ctx.id_str(),
                            phase,
                            output_content_encoding,
                            e
                        );
                        ContentInjectionResult {
                            body: processed,
                            content_encoding: None,
                        }
                    }
                }
            }
            None => ContentInjectionResult {
                body: processed,
                content_encoding: None,
            },
        };
    };

    match try_decompress_body_with_limit(
        body.as_ref(),
        source_content_encoding,
        encoding.max_decompress_output_bytes,
    ) {
        Ok(decompressed) => {
            let processed = apply_body_rules(
                Bytes::from(decompressed),
                rules,
                phase,
                content_type,
                verbose_logging,
                ctx,
            );

            match output_content_encoding {
                Some(output_content_encoding) => {
                    match compress_body(processed.as_ref(), output_content_encoding) {
                        Ok(compressed) => ContentInjectionResult {
                            body: Bytes::from(compressed),
                            content_encoding: Some(output_content_encoding.to_string()),
                        },
                        Err(e) => {
                            debug!(
                                "[{}] [{:?}_BODY] Failed to recompress {} body ({}), fallback to identity",
                                ctx.id_str(),
                                phase,
                                output_content_encoding,
                                e
                            );
                            ContentInjectionResult {
                                body: processed,
                                content_encoding: None,
                            }
                        }
                    }
                }
                None => ContentInjectionResult {
                    body: processed,
                    content_encoding: None,
                },
            }
        }
        Err(e) => {
            debug!(
                "[{}] [{:?}_BODY] Skip encoded body rules: failed to decompress {} body ({}).",
                ctx.id_str(),
                phase,
                source_content_encoding,
                e
            );
            ContentInjectionResult {
                body,
                content_encoding: Some(source_content_encoding.to_string()),
            }
        }
    }
}

pub fn apply_content_injection_preserving_encoding(
    body: Bytes,
    content_type: &str,
    encoding: ContentInjectionEncoding<'_>,
    rules: &ResolvedRules,
    verbose_logging: bool,
    ctx: &RequestContext,
) -> ContentInjectionResult {
    let source_content_encoding = encoding
        .source
        .filter(|encoding| !encoding.eq_ignore_ascii_case("identity"));
    let output_content_encoding = encoding
        .output
        .filter(|encoding| !encoding.eq_ignore_ascii_case("identity"));

    let Some(source_content_encoding) = source_content_encoding else {
        let injected_body =
            apply_content_injection(body, content_type, rules, verbose_logging, ctx);
        return match output_content_encoding {
            Some(output_content_encoding) => {
                match compress_body(injected_body.as_ref(), output_content_encoding) {
                    Ok(compressed) => ContentInjectionResult {
                        body: Bytes::from(compressed),
                        content_encoding: Some(output_content_encoding.to_string()),
                    },
                    Err(e) => {
                        debug!(
                        "[{}] [CONTENT_INJECTION] Failed to compress {} response body ({}), fallback to identity",
                        ctx.id_str(),
                        output_content_encoding,
                        e
                    );
                        ContentInjectionResult {
                            body: injected_body,
                            content_encoding: None,
                        }
                    }
                }
            }
            None => ContentInjectionResult {
                body: injected_body,
                content_encoding: None,
            },
        };
    };

    match try_decompress_body_with_limit(
        body.as_ref(),
        source_content_encoding,
        encoding.max_decompress_output_bytes,
    ) {
        Ok(decompressed) => {
            let injected_body = apply_content_injection(
                Bytes::from(decompressed),
                content_type,
                rules,
                verbose_logging,
                ctx,
            );

            match output_content_encoding {
                Some(output_content_encoding) => {
                    match compress_body(injected_body.as_ref(), output_content_encoding) {
                        Ok(compressed) => ContentInjectionResult {
                            body: Bytes::from(compressed),
                            content_encoding: Some(output_content_encoding.to_string()),
                        },
                        Err(e) => {
                            debug!(
                                "[{}] [CONTENT_INJECTION] Failed to recompress {} response body ({}), fallback to identity",
                                ctx.id_str(),
                                output_content_encoding,
                                e
                            );
                            ContentInjectionResult {
                                body: injected_body,
                                content_encoding: None,
                            }
                        }
                    }
                }
                None => ContentInjectionResult {
                    body: injected_body,
                    content_encoding: None,
                },
            }
        }
        Err(e) => {
            debug!(
                "[{}] [CONTENT_INJECTION] Skip encoded content injection: failed to decompress {} response body ({}).",
                ctx.id_str(),
                source_content_encoding,
                e
            );
            ContentInjectionResult {
                body,
                content_encoding: Some(source_content_encoding.to_string()),
            }
        }
    }
}

const HTML_DOCTYPE: &str = "<!DOCTYPE html>";
const HTML_CLOSE_TAG: &str = "</html>";
const BODY_OPEN_TAG: &str = "<body";
const BODY_CLOSE_TAG: &str = "</body>";

fn insert_before_html_close(html: &mut String, content: &str) -> bool {
    let lower = html.to_lowercase();
    if let Some(index) = lower.rfind(HTML_CLOSE_TAG) {
        html.insert_str(index, content);
        return true;
    }

    false
}

fn insert_after_html_open(html: &mut String, content: &str) -> bool {
    let lower = html.to_lowercase();
    let Some(html_open_start) = lower.find("<html") else {
        return false;
    };
    let Some(html_open_end_offset) = lower[html_open_start..].find('>') else {
        return false;
    };
    let html_inner_start = html_open_start + html_open_end_offset + 1;
    html.insert_str(html_inner_start, content);
    true
}

fn replace_html_body_inner(html: &mut String, content: &str) -> bool {
    let lower = html.to_lowercase();
    let Some(body_open_start) = lower.find(BODY_OPEN_TAG) else {
        return false;
    };
    let Some(body_open_end_offset) = lower[body_open_start..].find('>') else {
        return false;
    };
    let body_inner_start = body_open_start + body_open_end_offset + 1;

    let Some(body_close) = lower[body_inner_start..].rfind(BODY_CLOSE_TAG) else {
        return false;
    };
    let body_inner_end = body_inner_start + body_close;

    html.replace_range(body_inner_start..body_inner_end, content);
    true
}

fn apply_html_injection(
    body: Bytes,
    rules: &ResolvedRules,
    verbose_logging: bool,
    ctx: &RequestContext,
) -> Bytes {
    let (prepend, append, body_replace) =
        (&rules.html_prepend, &rules.html_append, &rules.html_body);

    if prepend.is_none() && append.is_none() && body_replace.is_none() {
        return body;
    }

    let mut html = String::from_utf8_lossy(&body).into_owned();

    if let Some(replace_body) = body_replace {
        if replace_html_body_inner(&mut html, replace_body) {
            if verbose_logging {
                debug!("[{}] [HTML_BODY] replaced body inner HTML", ctx.id_str());
            }
            return html.into_bytes().into();
        }
        if verbose_logging {
            debug!(
                "[{}] [HTML_BODY] replaced entire HTML because no body element was found",
                ctx.id_str()
            );
        }
        return replace_body.clone().into_bytes().into();
    }

    if let Some(prepend_content) = prepend {
        if !insert_after_html_open(&mut html, prepend_content) {
            let has_doctype = html.trim_start().to_lowercase().starts_with("<!doctype");
            if has_doctype {
                html = format!("{}{}", prepend_content, html);
            } else {
                html = format!("{}\n{}{}", HTML_DOCTYPE, prepend_content, html);
            }
            if verbose_logging && !has_doctype {
                debug!(
                    "[{}] [HTML_PREPEND] added DOCTYPE automatically",
                    ctx.id_str()
                );
            }
            if has_doctype && verbose_logging {
                debug!(
                    "[{}] [HTML_PREPEND] no html element found; prepended before document",
                    ctx.id_str()
                );
            }
        }
        if verbose_logging {
            debug!(
                "[{}] [HTML_PREPEND] prepended {} chars",
                ctx.id_str(),
                prepend_content.len()
            );
        }
    }

    if let Some(append_content) = append {
        if !insert_before_html_close(&mut html, append_content) {
            html = format!("{}{}", html, append_content);
        }
        if verbose_logging {
            debug!(
                "[{}] [HTML_APPEND] appended {} chars",
                ctx.id_str(),
                append_content.len()
            );
        }
    }

    html.into_bytes().into()
}

fn apply_js_injection(
    body: Bytes,
    rules: &ResolvedRules,
    verbose_logging: bool,
    ctx: &RequestContext,
) -> Bytes {
    let (prepend, append, body_replace) = (&rules.js_prepend, &rules.js_append, &rules.js_body);

    if prepend.is_none() && append.is_none() && body_replace.is_none() {
        return body;
    }

    if let Some(replace_body) = body_replace {
        if verbose_logging {
            debug!("[{}] [JS_BODY] replaced entire JS", ctx.id_str());
        }
        return replace_body.clone().into_bytes().into();
    }

    let mut js = String::from_utf8_lossy(&body).into_owned();

    if let Some(prepend_content) = prepend {
        js = format!("{}{}", prepend_content, js);
        if verbose_logging {
            debug!(
                "[{}] [JS_PREPEND] prepended {} chars",
                ctx.id_str(),
                prepend_content.len()
            );
        }
    }

    if let Some(append_content) = append {
        js = format!("{}{}", js, append_content);
        if verbose_logging {
            debug!(
                "[{}] [JS_APPEND] appended {} chars",
                ctx.id_str(),
                append_content.len()
            );
        }
    }

    js.into_bytes().into()
}

fn apply_css_injection(
    body: Bytes,
    rules: &ResolvedRules,
    verbose_logging: bool,
    ctx: &RequestContext,
) -> Bytes {
    let (prepend, append, body_replace) = (&rules.css_prepend, &rules.css_append, &rules.css_body);

    if prepend.is_none() && append.is_none() && body_replace.is_none() {
        return body;
    }

    if let Some(replace_body) = body_replace {
        if verbose_logging {
            debug!("[{}] [CSS_BODY] replaced entire CSS", ctx.id_str());
        }
        return replace_body.clone().into_bytes().into();
    }

    let mut css = String::from_utf8_lossy(&body).into_owned();

    if let Some(prepend_content) = prepend {
        css = format!("{}{}", prepend_content, css);
        if verbose_logging {
            debug!(
                "[{}] [CSS_PREPEND] prepended {} chars",
                ctx.id_str(),
                prepend_content.len()
            );
        }
    }

    if let Some(append_content) = append {
        css = format!("{}{}", css, append_content);
        if verbose_logging {
            debug!(
                "[{}] [CSS_APPEND] appended {} chars",
                ctx.id_str(),
                append_content.len()
            );
        }
    }

    css.into_bytes().into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mock_ctx() -> RequestContext {
        RequestContext::new()
    }

    #[test]
    fn test_apply_body_prepend() {
        let body = Bytes::from("original");
        let rules = ResolvedRules {
            req_prepend: Some(Bytes::from("prefix-")),
            ..Default::default()
        };

        let result = apply_body_rules(body, &rules, Phase::Request, None, false, &mock_ctx());
        assert_eq!(result, Bytes::from("prefix-original"));
    }

    #[test]
    fn test_apply_body_append() {
        let body = Bytes::from("original");
        let rules = ResolvedRules {
            req_append: Some(Bytes::from("-suffix")),
            ..Default::default()
        };

        let result = apply_body_rules(body, &rules, Phase::Request, None, false, &mock_ctx());
        assert_eq!(result, Bytes::from("original-suffix"));
    }

    #[test]
    fn test_apply_body_replace() {
        let body = Bytes::from("hello world");
        let rules = ResolvedRules {
            req_replace: vec![("world".to_string(), "rust".to_string())],
            ..Default::default()
        };

        let result = apply_body_rules(body, &rules, Phase::Request, None, false, &mock_ctx());
        assert_eq!(result, Bytes::from("hello rust"));
    }

    #[test]
    fn test_apply_body_merge_json() {
        let body = Bytes::from(r#"{"a":1,"b":2}"#);
        let rules = ResolvedRules {
            req_merge: Some(serde_json::json!({"b": 3, "c": 4})),
            ..Default::default()
        };

        let result = apply_body_rules(body, &rules, Phase::Request, None, false, &mock_ctx());
        let parsed: Value = serde_json::from_slice(&result).unwrap();
        assert_eq!(parsed["a"], 1);
        assert_eq!(parsed["b"], 3);
        assert_eq!(parsed["c"], 4);
    }

    #[test]
    fn test_apply_req_merge_preserving_gzip_encoding() {
        let compressed = compress_body(br#"{"a":1,"b":2}"#, "gzip").unwrap();
        let rules = ResolvedRules {
            req_merge: Some(serde_json::json!({"b": 3, "c": 4})),
            ..Default::default()
        };

        let result = apply_body_rules_preserving_encoding(
            Bytes::from(compressed),
            &rules,
            Phase::Request,
            Some("application/json"),
            ContentInjectionEncoding {
                source: Some("gzip"),
                output: Some("gzip"),
                max_decompress_output_bytes: 1024,
            },
            false,
            &mock_ctx(),
        );

        assert_eq!(result.content_encoding.as_deref(), Some("gzip"));
        let decompressed =
            try_decompress_body_with_limit(result.body.as_ref(), "gzip", 1024).unwrap();
        let parsed: Value = serde_json::from_slice(&decompressed).unwrap();
        assert_eq!(parsed["a"], 1);
        assert_eq!(parsed["b"], 3);
        assert_eq!(parsed["c"], 4);
    }

    #[test]
    fn test_apply_res_merge_preserving_gzip_encoding() {
        let compressed = compress_body(br#"{"ok":true,"test":"old"}"#, "gzip").unwrap();
        let rules = ResolvedRules {
            res_merge: Some(serde_json::json!({"test": "qwe"})),
            ..Default::default()
        };

        let result = apply_body_rules_preserving_encoding(
            Bytes::from(compressed),
            &rules,
            Phase::Response,
            Some("application/json"),
            ContentInjectionEncoding {
                source: Some("gzip"),
                output: Some("gzip"),
                max_decompress_output_bytes: 1024,
            },
            false,
            &mock_ctx(),
        );

        assert_eq!(result.content_encoding.as_deref(), Some("gzip"));
        let decompressed =
            try_decompress_body_with_limit(result.body.as_ref(), "gzip", 1024).unwrap();
        let parsed: Value = serde_json::from_slice(&decompressed).unwrap();
        assert_eq!(parsed["ok"], true);
        assert_eq!(parsed["test"], "qwe");
    }

    #[test]
    fn test_apply_res_merge_deletes_gzip_encoding_when_output_identity() {
        let compressed = compress_body(br#"{"test":"old"}"#, "gzip").unwrap();
        let rules = ResolvedRules {
            res_merge: Some(serde_json::json!({"test": "qwe"})),
            ..Default::default()
        };

        let result = apply_body_rules_preserving_encoding(
            Bytes::from(compressed),
            &rules,
            Phase::Response,
            Some("application/json"),
            ContentInjectionEncoding {
                source: Some("gzip"),
                output: None,
                max_decompress_output_bytes: 1024,
            },
            false,
            &mock_ctx(),
        );

        assert_eq!(result.content_encoding, None);
        let parsed: Value = serde_json::from_slice(&result.body).unwrap();
        assert_eq!(parsed["test"], "qwe");
    }

    #[test]
    fn test_apply_body_override() {
        let body = Bytes::from("original");
        let rules = ResolvedRules {
            req_body: Some(Bytes::from("replaced")),
            ..Default::default()
        };

        let result = apply_body_rules(body, &rules, Phase::Request, None, false, &mock_ctx());
        assert_eq!(result, Bytes::from("replaced"));
    }

    #[test]
    fn test_skip_replace_for_binary() {
        let body = Bytes::from("hello world");
        let rules = ResolvedRules {
            req_replace: vec![("world".to_string(), "rust".to_string())],
            ..Default::default()
        };

        let result = apply_body_rules(
            body,
            &rules,
            Phase::Request,
            Some("image/png"),
            false,
            &mock_ctx(),
        );
        assert_eq!(result, Bytes::from("hello world"));
    }

    #[test]
    fn test_merge_json_objects() {
        let base = serde_json::json!({"a": 1, "b": {"c": 2}});
        let patch = serde_json::json!({"b": {"d": 3}, "e": 4});

        let result = merge_json(base, patch);
        assert_eq!(result["a"], 1);
        assert_eq!(result["b"]["c"], 2);
        assert_eq!(result["b"]["d"], 3);
        assert_eq!(result["e"], 4);
    }

    #[test]
    fn test_html_injection_append() {
        let body = Bytes::from("<html><body>Hello</body></html>");
        let rules = ResolvedRules {
            html_append: Some("<script>alert(1)</script>".to_string()),
            ..Default::default()
        };

        let result = apply_content_injection(body, "text/html", &rules, false, &mock_ctx());
        assert_eq!(
            String::from_utf8_lossy(&result),
            "<html><body>Hello</body><script>alert(1)</script></html>"
        );
    }

    #[test]
    fn test_html_injection_gzip_preserves_encoding() {
        let body = Bytes::from("<html><body>Hello</body></html>");
        let encoded = compress_body(body.as_ref(), "gzip").unwrap();
        let rules = ResolvedRules {
            html_append: Some("<script>new VConsole();</script>".to_string()),
            ..Default::default()
        };

        let result = apply_content_injection_preserving_encoding(
            Bytes::from(encoded),
            "text/html; charset=utf-8",
            ContentInjectionEncoding {
                source: Some("gzip"),
                output: Some("gzip"),
                max_decompress_output_bytes: 1024,
            },
            &rules,
            false,
            &mock_ctx(),
        );

        assert_eq!(result.content_encoding.as_deref(), Some("gzip"));
        let decoded = try_decompress_body_with_limit(result.body.as_ref(), "gzip", 1024).unwrap();
        assert_eq!(
            String::from_utf8_lossy(&decoded),
            "<html><body>Hello</body><script>new VConsole();</script></html>"
        );
    }

    #[test]
    fn test_html_injection_gzip_respects_removed_encoding_header() {
        let body = Bytes::from("<html><body>Hello</body></html>");
        let encoded = compress_body(body.as_ref(), "gzip").unwrap();
        let rules = ResolvedRules {
            html_append: Some("<script>new VConsole();</script>".to_string()),
            ..Default::default()
        };

        let result = apply_content_injection_preserving_encoding(
            Bytes::from(encoded),
            "text/html; charset=utf-8",
            ContentInjectionEncoding {
                source: Some("gzip"),
                output: None,
                max_decompress_output_bytes: 1024,
            },
            &rules,
            false,
            &mock_ctx(),
        );

        assert_eq!(result.content_encoding, None);
        assert_eq!(
            String::from_utf8_lossy(&result.body),
            "<html><body>Hello</body><script>new VConsole();</script></html>"
        );
    }

    #[test]
    fn test_html_injection_identity_source_respects_encoded_output_header() {
        let body = Bytes::from("<html><body>Hello</body></html>");
        let rules = ResolvedRules {
            html_append: Some("<script>new VConsole();</script>".to_string()),
            ..Default::default()
        };

        let result = apply_content_injection_preserving_encoding(
            body,
            "text/html; charset=utf-8",
            ContentInjectionEncoding {
                source: Some("identity"),
                output: Some("gzip"),
                max_decompress_output_bytes: 1024,
            },
            &rules,
            false,
            &mock_ctx(),
        );

        assert_eq!(result.content_encoding.as_deref(), Some("gzip"));
        let decoded = try_decompress_body_with_limit(result.body.as_ref(), "gzip", 1024).unwrap();
        assert_eq!(
            String::from_utf8_lossy(&decoded),
            "<html><body>Hello</body><script>new VConsole();</script></html>"
        );
    }

    #[test]
    fn coverage_90_encoding_fallbacks_preserve_or_drop_headers_correctly() {
        let ctx = mock_ctx();
        let rules = ResolvedRules {
            req_append: Some(Bytes::from_static(b"-request")),
            html_append: Some("<em>injected</em>".to_string()),
            ..Default::default()
        };

        let identity = apply_body_rules_preserving_encoding(
            Bytes::from_static(b"body"),
            &rules,
            Phase::Request,
            Some("text/plain"),
            ContentInjectionEncoding {
                source: Some("identity"),
                output: None,
                max_decompress_output_bytes: 1024,
            },
            true,
            &ctx,
        );
        assert_eq!(identity.body, Bytes::from_static(b"body-request"));
        assert_eq!(identity.content_encoding, None);

        let invalid_output = apply_body_rules_preserving_encoding(
            Bytes::from_static(b"body"),
            &rules,
            Phase::Request,
            Some("text/plain"),
            ContentInjectionEncoding {
                source: None,
                output: Some("not-an-encoding"),
                max_decompress_output_bytes: 1024,
            },
            true,
            &ctx,
        );
        assert_eq!(invalid_output.body, Bytes::from_static(b"body-request"));
        assert_eq!(invalid_output.content_encoding, None);

        let invalid_source = apply_body_rules_preserving_encoding(
            Bytes::from_static(b"encoded-looking"),
            &rules,
            Phase::Request,
            Some("text/plain"),
            ContentInjectionEncoding {
                source: Some("not-an-encoding"),
                output: None,
                max_decompress_output_bytes: 1024,
            },
            true,
            &ctx,
        );
        assert_eq!(invalid_source.body, Bytes::from_static(b"encoded-looking"));
        assert_eq!(
            invalid_source.content_encoding.as_deref(),
            Some("not-an-encoding")
        );

        let html = Bytes::from_static(b"<html><body>x</body></html>");
        let invalid_html_output = apply_content_injection_preserving_encoding(
            html.clone(),
            "text/html",
            ContentInjectionEncoding {
                source: None,
                output: Some("not-an-encoding"),
                max_decompress_output_bytes: 1024,
            },
            &rules,
            true,
            &ctx,
        );
        assert!(String::from_utf8_lossy(&invalid_html_output.body).contains("injected"));
        assert_eq!(invalid_html_output.content_encoding, None);

        let invalid_html_source = apply_content_injection_preserving_encoding(
            html,
            "text/html",
            ContentInjectionEncoding {
                source: Some("not-an-encoding"),
                output: None,
                max_decompress_output_bytes: 1024,
            },
            &rules,
            true,
            &ctx,
        );
        assert_eq!(
            invalid_html_source.content_encoding.as_deref(),
            Some("not-an-encoding")
        );
    }

    #[test]
    fn test_html_injection_append_uses_last_html_close_case_insensitive() {
        let body = Bytes::from("<html><body>outer</body><template></html></template></HTML>");
        let rules = ResolvedRules {
            html_append: Some("<script>new VConsole();</script>".to_string()),
            ..Default::default()
        };

        let result = apply_content_injection(body, "text/html", &rules, false, &mock_ctx());
        assert_eq!(
            String::from_utf8_lossy(&result),
            "<html><body>outer</body><template></html></template><script>new VConsole();</script></HTML>"
        );
    }

    #[test]
    fn test_html_injection_append_falls_back_to_document_end_without_html_close() {
        let body = Bytes::from("<section>Hello</section>");
        let rules = ResolvedRules {
            html_append: Some("<script>alert(1)</script>".to_string()),
            ..Default::default()
        };

        let result = apply_content_injection(body, "text/html", &rules, false, &mock_ctx());
        assert_eq!(
            String::from_utf8_lossy(&result),
            "<section>Hello</section><script>alert(1)</script>"
        );
    }

    #[test]
    fn test_html_injection_prepend_inserts_after_html_open() {
        let body = Bytes::from(
            r#"<!doctype html><html lang="en"><head><title>Original</title></head><body>Hello</body></html>"#,
        );
        let rules = ResolvedRules {
            html_prepend: Some("<!--HTML_PREPEND-->".to_string()),
            ..Default::default()
        };

        let result = apply_content_injection(body, "text/html", &rules, false, &mock_ctx());
        assert_eq!(
            String::from_utf8_lossy(&result),
            r#"<!doctype html><html lang="en"><!--HTML_PREPEND--><head><title>Original</title></head><body>Hello</body></html>"#
        );
    }

    #[test]
    fn test_html_injection_prepend_uses_html_open_case_insensitive() {
        let body = Bytes::from("<!doctype html><HTML><body>Hello</body></HTML>");
        let rules = ResolvedRules {
            html_prepend: Some("<!--HTML_PREPEND-->".to_string()),
            ..Default::default()
        };

        let result = apply_content_injection(body, "text/html", &rules, false, &mock_ctx());
        assert_eq!(
            String::from_utf8_lossy(&result),
            "<!doctype html><HTML><!--HTML_PREPEND--><body>Hello</body></HTML>"
        );
    }

    #[test]
    fn test_html_injection_body_replaces_body_inner_html() {
        let body = Bytes::from(
            r#"<!doctype html><html><head><title>Original</title></head><body class="app">HTML_ORIGINAL</body></html>"#,
        );
        let rules = ResolvedRules {
            html_body: Some("<main>HTML_REPLACED</main>".to_string()),
            ..Default::default()
        };

        let result = apply_content_injection(body, "text/html", &rules, false, &mock_ctx());
        assert_eq!(
            String::from_utf8_lossy(&result),
            r#"<!doctype html><html><head><title>Original</title></head><body class="app"><main>HTML_REPLACED</main></body></html>"#
        );
    }

    #[test]
    fn test_html_injection_body_falls_back_to_entire_replace_without_body_element() {
        let body = Bytes::from("<section>HTML_ORIGINAL</section>");
        let rules = ResolvedRules {
            html_body: Some("<main>HTML_REPLACED</main>".to_string()),
            ..Default::default()
        };

        let result = apply_content_injection(body, "text/html", &rules, false, &mock_ctx());
        assert_eq!(
            String::from_utf8_lossy(&result),
            "<main>HTML_REPLACED</main>"
        );
    }

    #[test]
    fn test_js_injection_prepend() {
        let body = Bytes::from("console.log('hello');");
        let rules = ResolvedRules {
            js_prepend: Some("var x = 1;".to_string()),
            ..Default::default()
        };

        let result =
            apply_content_injection(body, "application/javascript", &rules, false, &mock_ctx());
        assert!(String::from_utf8_lossy(&result).starts_with("var x = 1;"));
    }

    #[test]
    fn test_css_injection_body_replace() {
        let body = Bytes::from("body { color: black; }");
        let rules = ResolvedRules {
            css_body: Some("body { color: red; }".to_string()),
            ..Default::default()
        };

        let result = apply_content_injection(body, "text/css", &rules, false, &mock_ctx());
        assert_eq!(String::from_utf8_lossy(&result), "body { color: red; }");
    }

    #[test]
    fn test_js_injection_append_prepend_and_body_replace() {
        let body = Bytes::from("window.app = 1;");
        let append_rules = ResolvedRules {
            js_append: Some("window.loaded = true;".to_string()),
            ..Default::default()
        };
        let prepend_rules = ResolvedRules {
            js_prepend: Some("window.before = true;".to_string()),
            ..Default::default()
        };
        let body_rules = ResolvedRules {
            js_body: Some("window.replaced = true;".to_string()),
            ..Default::default()
        };

        let appended = apply_content_injection(
            body.clone(),
            "application/javascript",
            &append_rules,
            false,
            &mock_ctx(),
        );
        assert_eq!(
            String::from_utf8_lossy(&appended),
            "window.app = 1;window.loaded = true;"
        );

        let prepended = apply_content_injection(
            body.clone(),
            "text/javascript",
            &prepend_rules,
            false,
            &mock_ctx(),
        );
        assert_eq!(
            String::from_utf8_lossy(&prepended),
            "window.before = true;window.app = 1;"
        );

        let replaced = apply_content_injection(
            body,
            "application/x-javascript",
            &body_rules,
            false,
            &mock_ctx(),
        );
        assert_eq!(
            String::from_utf8_lossy(&replaced),
            "window.replaced = true;"
        );
    }

    #[test]
    fn test_css_injection_append_prepend_and_body_replace() {
        let body = Bytes::from(".app{color:black;}");
        let append_rules = ResolvedRules {
            css_append: Some(".loaded{display:block;}".to_string()),
            ..Default::default()
        };
        let prepend_rules = ResolvedRules {
            css_prepend: Some(":root{--ok:1;}".to_string()),
            ..Default::default()
        };
        let body_rules = ResolvedRules {
            css_body: Some(".replaced{color:red;}".to_string()),
            ..Default::default()
        };

        let appended =
            apply_content_injection(body.clone(), "text/css", &append_rules, false, &mock_ctx());
        assert_eq!(
            String::from_utf8_lossy(&appended),
            ".app{color:black;}.loaded{display:block;}"
        );

        let prepended = apply_content_injection(
            body.clone(),
            "text/css; charset=utf-8",
            &prepend_rules,
            false,
            &mock_ctx(),
        );
        assert_eq!(
            String::from_utf8_lossy(&prepended),
            ":root{--ok:1;}.app{color:black;}"
        );

        let replaced = apply_content_injection(body, "text/css", &body_rules, false, &mock_ctx());
        assert_eq!(String::from_utf8_lossy(&replaced), ".replaced{color:red;}");
    }

    #[test]
    fn test_content_injection_ignores_protocols_when_response_type_differs() {
        let body = Bytes::from("window.app = 1;");
        let rules = ResolvedRules {
            html_append: Some("<script>html</script>".to_string()),
            css_append: Some(".bad{display:none;}".to_string()),
            js_append: Some("window.loaded = true;".to_string()),
            ..Default::default()
        };

        let result =
            apply_content_injection(body, "application/javascript", &rules, false, &mock_ctx());
        assert_eq!(
            String::from_utf8_lossy(&result),
            "window.app = 1;window.loaded = true;"
        );
    }

    #[test]
    fn test_no_injection_for_other_types() {
        let body = Bytes::from("some data");
        let rules = ResolvedRules {
            html_append: Some("<script></script>".to_string()),
            ..Default::default()
        };

        let result =
            apply_content_injection(body.clone(), "application/json", &rules, false, &mock_ctx());
        assert_eq!(result, body);
    }

    #[test]
    fn coverage_90_form_merge_all_value_types_and_non_object_patch() {
        let rules = ResolvedRules {
            req_merge: Some(serde_json::json!({
                "text": "updated",
                "number": 42,
                "enabled": true,
                "empty": null,
                "nested": {"ok": true}
            })),
            ..Default::default()
        };
        let result = apply_body_rules(
            Bytes::from("text=old&keep=yes"),
            &rules,
            Phase::Request,
            Some("application/x-www-form-urlencoded; charset=utf-8"),
            true,
            &mock_ctx(),
        );
        let pairs: std::collections::HashMap<_, _> =
            url::form_urlencoded::parse(&result).into_owned().collect();
        assert_eq!(pairs.get("text").map(String::as_str), Some("updated"));
        assert_eq!(pairs.get("number").map(String::as_str), Some("42"));
        assert_eq!(pairs.get("enabled").map(String::as_str), Some("true"));
        assert_eq!(pairs.get("empty").map(String::as_str), Some(""));
        assert_eq!(
            pairs.get("nested").map(String::as_str),
            Some("{\"ok\":true}")
        );

        assert!(merge_form_urlencoded(b"a=1", &serde_json::json!([1, 2])).is_none());
    }

    #[test]
    fn coverage_90_verbose_body_rules_cover_text_binary_and_response_paths() {
        let rules = ResolvedRules {
            res_body: Some(Bytes::from("${statusCode}")),
            res_prepend: Some(Bytes::from("[")),
            res_append: Some(Bytes::from("]")),
            res_replace: vec![("never".into(), "changed".into())],
            res_replace_regex: vec![RegexReplace {
                pattern: regex::Regex::new("n.+r").unwrap(),
                replacement: "changed".into(),
                global: false,
            }],
            ..Default::default()
        };
        let mut ctx = mock_ctx();
        ctx.res_status = Some(201);
        let result = apply_body_rules(
            Bytes::from("original"),
            &rules,
            Phase::Response,
            Some("text/plain"),
            true,
            &ctx,
        );
        assert_eq!(result, Bytes::from("[201]"));

        let binary = apply_body_rules(
            Bytes::from_static(b"never"),
            &rules,
            Phase::Response,
            Some("application/protobuf"),
            true,
            &ctx,
        );
        assert_eq!(binary, Bytes::from("[201]"));
    }

    #[test]
    fn coverage_90_encoding_failures_and_identity_paths_are_lossless() {
        let rules = ResolvedRules {
            req_append: Some(Bytes::from("!")),
            html_append: Some("<footer>ok</footer>".into()),
            ..Default::default()
        };
        let invalid = Bytes::from_static(b"not compressed");
        let result = apply_body_rules_preserving_encoding(
            invalid.clone(),
            &rules,
            Phase::Request,
            Some("text/plain"),
            ContentInjectionEncoding {
                source: Some("gzip"),
                output: Some("br"),
                max_decompress_output_bytes: 1024,
            },
            true,
            &mock_ctx(),
        );
        assert_eq!(result.body, invalid);
        assert_eq!(result.content_encoding.as_deref(), Some("gzip"));

        let unsupported = apply_body_rules_preserving_encoding(
            Bytes::from_static(b"plain"),
            &rules,
            Phase::Request,
            Some("text/plain"),
            ContentInjectionEncoding {
                source: Some("identity"),
                output: Some("unsupported"),
                max_decompress_output_bytes: 1024,
            },
            true,
            &mock_ctx(),
        );
        assert_eq!(unsupported.body, Bytes::from_static(b"plain!"));
        assert_eq!(unsupported.content_encoding, None);

        let injected = apply_content_injection_preserving_encoding(
            Bytes::from_static(b"bad gzip"),
            "text/html",
            ContentInjectionEncoding {
                source: Some("gzip"),
                output: None,
                max_decompress_output_bytes: 1024,
            },
            &rules,
            true,
            &mock_ctx(),
        );
        assert_eq!(injected.body, Bytes::from_static(b"bad gzip"));
        assert_eq!(injected.content_encoding.as_deref(), Some("gzip"));
    }

    #[test]
    fn coverage_90_verbose_html_js_css_fallbacks() {
        let ctx = mock_ctx();
        let html_prepend = ResolvedRules {
            html_prepend: Some("<meta name=covered>".into()),
            html_append: Some("<footer>covered</footer>".into()),
            ..Default::default()
        };
        let without_html = apply_content_injection(
            Bytes::from_static(b"fragment"),
            "application/xhtml+xml",
            &html_prepend,
            true,
            &ctx,
        );
        assert!(String::from_utf8_lossy(&without_html).starts_with("<!DOCTYPE html>"));

        let with_doctype = apply_content_injection(
            Bytes::from_static(b"<!doctype html>fragment"),
            "text/html",
            &html_prepend,
            true,
            &ctx,
        );
        assert!(String::from_utf8_lossy(&with_doctype).starts_with("<meta name=covered>"));

        for (content_type, rules) in [
            (
                "text/js",
                ResolvedRules {
                    js_prepend: Some("before;".into()),
                    js_append: Some("after;".into()),
                    ..Default::default()
                },
            ),
            (
                "text/css",
                ResolvedRules {
                    css_prepend: Some("before{}".into()),
                    css_append: Some("after{}".into()),
                    ..Default::default()
                },
            ),
        ] {
            let result = apply_content_injection(
                Bytes::from_static(b"middle"),
                content_type,
                &rules,
                true,
                &ctx,
            );
            assert!(String::from_utf8_lossy(&result).contains("middle"));
        }
    }

    #[test]
    fn coverage_90_successful_encoding_and_full_content_replacement_paths() {
        let rules = ResolvedRules {
            req_append: Some(Bytes::from_static(b"!")),
            html_body: Some("<html>replacement</html>".to_string()),
            js_body: Some("replacement();".to_string()),
            css_body: Some("replacement{}".to_string()),
            ..Default::default()
        };
        let encoded = apply_body_rules_preserving_encoding(
            Bytes::from_static(b"plain"),
            &rules,
            Phase::Request,
            Some("text/plain"),
            ContentInjectionEncoding {
                source: None,
                output: Some("gzip"),
                max_decompress_output_bytes: 1024,
            },
            true,
            &mock_ctx(),
        );
        assert_eq!(encoded.content_encoding.as_deref(), Some("gzip"));
        assert_eq!(
            try_decompress_body_with_limit(&encoded.body, "gzip", 1024).unwrap(),
            b"plain!"
        );
        for (content_type, expected) in [
            ("text/html", "<html>replacement</html>"),
            ("application/javascript", "replacement();"),
            ("text/css", "replacement{}"),
        ] {
            let result = apply_content_injection(
                Bytes::from_static(b"original"),
                content_type,
                &rules,
                true,
                &mock_ctx(),
            );
            assert_eq!(String::from_utf8_lossy(&result), expected);
        }
        assert_eq!(
            apply_content_injection(
                Bytes::from_static(b"unchanged"),
                "application/octet-stream",
                &rules,
                true,
                &mock_ctx(),
            ),
            Bytes::from_static(b"unchanged")
        );

        let compressed = compress_body(b"encoded", "gzip").unwrap();
        let decoded_identity = apply_body_rules_preserving_encoding(
            Bytes::from(compressed.clone()),
            &ResolvedRules::default(),
            Phase::Request,
            Some("text/plain"),
            ContentInjectionEncoding {
                source: Some("gzip"),
                output: None,
                max_decompress_output_bytes: 1024,
            },
            true,
            &mock_ctx(),
        );
        assert_eq!(decoded_identity.body, Bytes::from_static(b"encoded"));
        assert_eq!(decoded_identity.content_encoding, None);
        let invalid_recompression = apply_body_rules_preserving_encoding(
            Bytes::from(compressed.clone()),
            &ResolvedRules::default(),
            Phase::Request,
            Some("text/plain"),
            ContentInjectionEncoding {
                source: Some("gzip"),
                output: Some("unsupported"),
                max_decompress_output_bytes: 1024,
            },
            true,
            &mock_ctx(),
        );
        assert_eq!(invalid_recompression.body, Bytes::from_static(b"encoded"));
        assert_eq!(invalid_recompression.content_encoding, None);

        let html = compress_body(b"<html><body>x</body></html>", "gzip").unwrap();
        let injected_identity = apply_content_injection_preserving_encoding(
            Bytes::from(html.clone()),
            "text/html",
            ContentInjectionEncoding {
                source: Some("gzip"),
                output: None,
                max_decompress_output_bytes: 1024,
            },
            &ResolvedRules::default(),
            true,
            &mock_ctx(),
        );
        assert_eq!(injected_identity.content_encoding, None);
        let invalid_injection_recompression = apply_content_injection_preserving_encoding(
            Bytes::from(html),
            "text/html",
            ContentInjectionEncoding {
                source: Some("gzip"),
                output: Some("unsupported"),
                max_decompress_output_bytes: 1024,
            },
            &ResolvedRules::default(),
            true,
            &mock_ctx(),
        );
        assert_eq!(invalid_injection_recompression.content_encoding, None);
    }
}
