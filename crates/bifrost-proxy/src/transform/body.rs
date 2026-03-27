use bytes::Bytes;
use serde_json::Value;
use tracing::debug;

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
        if verbose_logging {
            debug!(
                "[{}] [{:?}_BODY] replaced: {} bytes -> {} bytes",
                ctx.id_str(),
                phase,
                result.len(),
                override_body.len()
            );
        }
        result = override_body.clone();
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

const HTML_DOCTYPE: &str = "<!DOCTYPE html>";

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

    if let Some(replace_body) = body_replace {
        if verbose_logging {
            debug!("[{}] [HTML_BODY] replaced entire HTML", ctx.id_str());
        }
        return replace_body.clone().into_bytes().into();
    }

    let mut html = String::from_utf8_lossy(&body).into_owned();

    if let Some(prepend_content) = prepend {
        let has_doctype = html.trim_start().to_lowercase().starts_with("<!doctype");
        if has_doctype {
            html = format!("{}{}", prepend_content, html);
        } else {
            html = format!("{}\n{}{}", HTML_DOCTYPE, prepend_content, html);
            if verbose_logging {
                debug!(
                    "[{}] [HTML_PREPEND] added DOCTYPE automatically",
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
        html = format!("{}{}", html, append_content);
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
        assert!(String::from_utf8_lossy(&result).ends_with("<script>alert(1)</script>"));
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
}
