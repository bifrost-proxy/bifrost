use bifrost_core::{Protocol, ResolvedRules};
use bytes::Bytes;
use serde_json::Value;

pub(crate) fn apply_response_body_rules(
    resolved_rules: &ResolvedRules,
    headers: &[(String, String)],
    body: Option<String>,
) -> Option<String> {
    let mut result = body.clone().map(String::into_bytes).unwrap_or_default();
    let mut changed = false;

    for rule in &resolved_rules.rules {
        match rule.rule.protocol {
            Protocol::ResBody | Protocol::HtmlBody | Protocol::JsBody | Protocol::CssBody => {
                result = extract_inline_content(&rule.resolved_value).into_bytes();
                changed = true;
            }
            Protocol::ResPrepend
            | Protocol::HtmlPrepend
            | Protocol::JsPrepend
            | Protocol::CssPrepend => {
                let prepend = extract_inline_content(&rule.resolved_value).into_bytes();
                let mut next = Vec::with_capacity(prepend.len() + result.len());
                next.extend_from_slice(&prepend);
                next.extend_from_slice(&result);
                result = next;
                changed = true;
            }
            Protocol::ResAppend
            | Protocol::HtmlAppend
            | Protocol::JsAppend
            | Protocol::CssAppend => {
                result.extend_from_slice(extract_inline_content(&rule.resolved_value).as_bytes());
                changed = true;
            }
            Protocol::ResReplace => {
                let mut body_str = String::from_utf8_lossy(&result).into_owned();
                let parsed = parse_replace_value(&rule.resolved_value);
                for (from, to) in parsed.string_rules {
                    body_str = body_str.replace(&from, &to);
                }
                for regex_rule in parsed.regex_rules {
                    body_str = if regex_rule.global {
                        regex_rule
                            .pattern
                            .replace_all(&body_str, regex_rule.replacement.as_str())
                            .into_owned()
                    } else {
                        regex_rule
                            .pattern
                            .replace(&body_str, regex_rule.replacement.as_str())
                            .into_owned()
                    };
                }
                result = body_str.into_bytes();
                changed = true;
            }
            Protocol::ResMerge => {
                if let Some(patch) = parse_merge_value(&rule.resolved_value) {
                    if let Some(merged) = merge_body(&result, headers, &patch) {
                        result = merged.to_vec();
                        changed = true;
                    }
                }
            }
            _ => {}
        }
    }

    if changed {
        Some(String::from_utf8_lossy(&result).into_owned())
    } else {
        body
    }
}

fn extract_inline_content(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.starts_with('(') && trimmed.ends_with(')') && trimmed.len() >= 2 {
        trimmed[1..trimmed.len() - 1].to_string()
    } else {
        value.to_string()
    }
}

fn parse_merge_value(value: &str) -> Option<Value> {
    if let Ok(json_value) = serde_json::from_str(value) {
        return Some(json_value);
    }

    let content = extract_inline_content(value);
    if content.contains('=') {
        let mut object = serde_json::Map::new();
        for pair in content.split('&') {
            if let Some((key, value)) = pair.split_once('=') {
                object.insert(
                    key.trim().to_string(),
                    Value::String(value.trim().to_string()),
                );
            }
        }
        if !object.is_empty() {
            return Some(Value::Object(object));
        }
    }

    None
}

fn merge_body(body: &[u8], headers: &[(String, String)], patch: &Value) -> Option<Bytes> {
    let content_type = headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case("content-type"))
        .map(|(_, value)| value.to_ascii_lowercase())
        .unwrap_or_default();

    if content_type.starts_with("application/x-www-form-urlencoded") {
        return merge_form_urlencoded(body, patch);
    }

    let original = serde_json::from_slice::<Value>(body).ok()?;
    let merged = merge_json(original, patch.clone());
    serde_json::to_vec(&merged).ok().map(Bytes::from)
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
        let value_string = match value {
            Value::String(s) => s.clone(),
            Value::Number(n) => n.to_string(),
            Value::Bool(b) => b.to_string(),
            Value::Null => String::new(),
            other => other.to_string(),
        };

        if let Some(existing) = pairs
            .iter_mut()
            .find(|(existing_key, _)| existing_key == key)
        {
            existing.1 = value_string;
        } else {
            pairs.push((key.clone(), value_string));
        }
    }

    let mut serializer = url::form_urlencoded::Serializer::new(String::new());
    for (key, value) in pairs {
        serializer.append_pair(&key, &value);
    }
    Some(Bytes::from(serializer.finish()))
}

struct ParsedReplaceRules {
    string_rules: Vec<(String, String)>,
    regex_rules: Vec<ReplayRegexReplace>,
}

struct ReplayRegexReplace {
    pattern: regex::Regex,
    replacement: String,
    global: bool,
}

fn parse_replace_value(value: &str) -> ParsedReplaceRules {
    let mut string_rules = Vec::new();
    let mut regex_rules = Vec::new();

    if let Some(pairs) = bifrost_core::parse_json_replace_pairs(value) {
        for (from, to) in pairs {
            push_replace_pair(&mut string_rules, &mut regex_rules, from, to);
        }
        return ParsedReplaceRules {
            string_rules,
            regex_rules,
        };
    }

    for pair in value.split('&') {
        let pair = pair.trim();
        if pair.is_empty() {
            continue;
        }

        let (from, to) = pair.split_once('=').unwrap_or((pair, ""));
        let from = url_decode(from);
        let to = url_decode(to);
        push_replace_pair(&mut string_rules, &mut regex_rules, from, to);
    }

    ParsedReplaceRules {
        string_rules,
        regex_rules,
    }
}

fn push_replace_pair(
    string_rules: &mut Vec<(String, String)>,
    regex_rules: &mut Vec<ReplayRegexReplace>,
    from: String,
    to: String,
) {
    if let Some((pattern, global)) = parse_regex_pattern(&from) {
        regex_rules.push(ReplayRegexReplace {
            pattern,
            replacement: to,
            global,
        });
    } else {
        string_rules.push((from, to));
    }
}

fn url_decode(value: &str) -> String {
    urlencoding::decode(value)
        .map(|decoded| decoded.into_owned())
        .unwrap_or_else(|_| value.to_string())
}

fn parse_regex_pattern(value: &str) -> Option<(regex::Regex, bool)> {
    let value = value.trim();
    if !value.starts_with('/') {
        return None;
    }

    let global = value.ends_with("/g") || value.ends_with("/gi") || value.ends_with("/ig");
    let case_insensitive =
        value.ends_with("/i") || value.ends_with("/gi") || value.ends_with("/ig");

    let end_pos = if global && case_insensitive {
        value.len().checked_sub(3)?
    } else if global || case_insensitive {
        value.len().checked_sub(2)?
    } else if value.len() > 1 && value.ends_with('/') {
        value.len() - 1
    } else {
        return None;
    };

    let pattern = &value[1..end_pos];
    if pattern.is_empty() {
        return None;
    }

    let regex = if case_insensitive {
        regex::RegexBuilder::new(pattern)
            .case_insensitive(true)
            .build()
            .ok()?
    } else {
        regex::Regex::new(pattern).ok()?
    };

    Some((regex, global))
}

#[cfg(test)]
mod tests {
    use super::{apply_response_body_rules, parse_replace_value};
    use bifrost_core::{RequestContext, RuleParser, RulesResolver};

    fn resolve(rule_text: &str) -> bifrost_core::ResolvedRules {
        let parser = RuleParser::new();
        let rules = parser.parse_line(rule_text).unwrap();
        let resolver = RulesResolver::new(rules);
        let ctx = RequestContext::from_url("https://example.test/api").with_method("GET");
        resolver.resolve(&ctx)
    }

    #[test]
    fn replay_response_res_merge_merges_json_body() {
        let rules = resolve(r#"https://example.test/api resMerge://({"test":"qwe","n":2})"#);
        let body = Some(r#"{"ok":true,"test":"old"}"#.to_string());

        let merged = apply_response_body_rules(
            &rules,
            &[("content-type".to_string(), "application/json".to_string())],
            body,
        )
        .unwrap();
        let value: serde_json::Value = serde_json::from_str(&merged).unwrap();

        assert_eq!(value["ok"], true);
        assert_eq!(value["test"], "qwe");
        assert_eq!(value["n"], 2);
    }

    #[test]
    fn replay_response_body_rules_apply_text_mutations() {
        let rules = resolve(
            "https://example.test/api resPrepend://(pre-) resReplace://world=relay resAppend://(-post)",
        );

        let result =
            apply_response_body_rules(&rules, &[], Some("hello world".to_string())).unwrap();

        assert_eq!(result, "pre-hello relay-post");
    }

    #[test]
    fn replay_response_replace_parser_supports_json_object() {
        let parsed = parse_replace_value(
            r#"{".doupay.com\"":".nodoupay.com\"","\"inf.baohuaxia.com\"":"\"inf.nobaohuaxia.com\""}"#,
        );

        assert_eq!(
            parsed.string_rules,
            vec![
                (r#".doupay.com""#.into(), r#".nodoupay.com""#.into()),
                (
                    r#""inf.baohuaxia.com""#.into(),
                    r#""inf.nobaohuaxia.com""#.into(),
                ),
            ]
        );
        assert!(parsed.regex_rules.is_empty());
    }

    #[test]
    fn replay_response_body_rules_apply_content_injection_mutations() {
        let rules = resolve(
            "https://example.test/api htmlPrepend://(<head>) jsAppend://(<script>ok()</script>)",
        );

        let result =
            apply_response_body_rules(&rules, &[], Some("<main></main>".to_string())).unwrap();

        assert_eq!(result, "<head><main></main><script>ok()</script>");
    }

    #[test]
    fn replay_response_body_rules_apply_content_injection_body_override() {
        let rules = resolve("https://example.test/api cssBody://(body{color:red})");

        let result =
            apply_response_body_rules(&rules, &[], Some("<main></main>".to_string())).unwrap();

        assert_eq!(result, "body{color:red}");
    }
}
