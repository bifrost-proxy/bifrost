use std::collections::{HashMap, HashSet};

use bifrost_core::normalize_rule_content;

use crate::types::RemoteEnv;

pub fn normalize_remote_rule(env: &RemoteEnv, remote_envs: &[RemoteEnv]) -> String {
    let mut env_map: HashMap<String, &RemoteEnv> = HashMap::new();
    for item in remote_envs {
        env_map.insert(format!("{}/{}", item.user_id, item.name), item);
        env_map.entry(item.name.clone()).or_insert(item);
    }

    let mut visiting = HashSet::new();
    normalize_remote_rule_inner(env, &env_map, &mut visiting)
}

fn normalize_remote_rule_inner(
    env: &RemoteEnv,
    env_map: &HashMap<String, &RemoteEnv>,
    visiting: &mut HashSet<String>,
) -> String {
    let visit_key = format!("{}/{}", env.user_id, env.name);
    if !visiting.insert(visit_key) {
        return String::new();
    }

    let result = if needs_legacy_normalization(&env.rule) {
        let values = collect_legacy_values(&env.rule);
        normalize_text(&env.rule, &env.user_id, &values, env_map, visiting)
    } else {
        env.rule.clone()
    };
    visiting.remove(&format!("{}/{}", env.user_id, env.name));
    result
}

fn collect_legacy_values(rule: &str) -> HashMap<String, String> {
    let mut values = HashMap::new();
    let mut in_code_block = false;
    let mut in_line_block = false;

    for line in rule.lines() {
        let trimmed = line.trim();

        if in_line_block {
            if trimmed == "`" {
                in_line_block = false;
            }
            continue;
        }

        if !in_code_block && trimmed == "line`" {
            in_line_block = true;
            continue;
        }

        if trimmed.starts_with("```") {
            if !in_code_block && trimmed == "```" {
                continue;
            }
            in_code_block = !in_code_block;
            continue;
        }

        if in_code_block {
            continue;
        }

        let clean = strip_legacy_comment(line).trim();
        if clean.is_empty() {
            continue;
        }

        if let Some((key, value)) = parse_legacy_value_assignment(clean) {
            let expanded = expand_legacy_templates(&value, &values);
            values.insert(key, expanded);
        }
    }

    values
}

fn needs_legacy_normalization(rule: &str) -> bool {
    let mut in_code_block = false;
    let mut in_line_block = false;

    for line in rule.lines() {
        let trimmed = line.trim();

        if in_line_block {
            if trimmed == "`" {
                in_line_block = false;
            }
            continue;
        }

        if !in_code_block && trimmed == "line`" {
            in_line_block = true;
            continue;
        }

        if trimmed.starts_with("```") {
            in_code_block = !in_code_block;
            continue;
        }

        if in_code_block {
            continue;
        }

        if trimmed.starts_with('#') || trimmed.is_empty() {
            continue;
        }

        if trimmed.contains("${") {
            return true;
        }

        if parse_legacy_import(trimmed).is_some() {
            return true;
        }

        if parse_legacy_value_assignment(trimmed).is_some() {
            return true;
        }

        if trimmed.contains(" ignore://")
            || trimmed.starts_with("ignore://")
            || trimmed.contains(" enable://intercept")
            || trimmed.contains(" disable://intercept")
            || trimmed.contains(" enable://https")
            || trimmed.contains(" disable://https")
        {
            return true;
        }
    }

    false
}

fn normalize_text(
    rule: &str,
    owner_user_id: &str,
    values: &HashMap<String, String>,
    env_map: &HashMap<String, &RemoteEnv>,
    visiting: &mut HashSet<String>,
) -> String {
    let mut output = Vec::new();
    let mut in_code_block = false;
    let mut in_line_block = false;

    for line in rule.lines() {
        let trimmed = line.trim();

        if in_line_block {
            output.push(line.to_string());
            if trimmed == "`" {
                in_line_block = false;
            }
            continue;
        }

        if !in_code_block && trimmed == "line`" {
            in_line_block = true;
            output.push(trimmed.to_string());
            continue;
        }

        if trimmed.starts_with("```") {
            if !in_code_block && trimmed == "```" {
                continue;
            }
            in_code_block = !in_code_block;
            output.push(trimmed.to_string());
            continue;
        }

        if in_code_block {
            output.push(line.to_string());
            continue;
        }

        let clean = strip_legacy_comment(line).trim().to_string();
        if clean.is_empty() {
            output.push(String::new());
            continue;
        }

        if parse_legacy_value_assignment(&clean).is_some() {
            continue;
        }

        if let Some(import_name) = parse_legacy_import(&clean) {
            let lookup_keys = [
                import_name.clone(),
                format!(
                    "{owner_user_id}/{}",
                    import_name.split_once('/').map(|(_, n)| n).unwrap_or("")
                ),
            ];

            let import_env = lookup_keys.iter().find_map(|key| env_map.get(key)).copied();

            if let Some(import_env) = import_env {
                let expanded = normalize_remote_rule_inner(import_env, env_map, visiting);
                if !expanded.trim().is_empty() {
                    output.push(format!("# import {}", clean));
                    output.push(expanded);
                }
            } else {
                output.push(format!("# unresolved import {}", clean));
            }
            continue;
        }

        let expanded = expand_legacy_templates(&clean, values);
        if let Some(normalized) = normalize_legacy_rule_line(&expanded) {
            output.push(normalized);
        }
    }

    output.join("\n")
}

fn strip_legacy_comment(line: &str) -> &str {
    match line.find('#') {
        Some(index) => &line[..index],
        None => line,
    }
}

fn parse_legacy_value_assignment(line: &str) -> Option<(String, String)> {
    if let Some(index) = line.find('=') {
        let key = line[..index].trim();
        let value = line[index + 1..].trim();
        if is_legacy_key(key) {
            return Some((key.to_string(), value.to_string()));
        }
    }

    if let Some(index) = line.find(':') {
        if line[index + 1..].starts_with('/') {
            return None;
        }
        let value = &line[index + 1..];
        if !value.starts_with(' ') && !value.starts_with('\t') {
            return None;
        }
        let key = line[..index].trim();
        if is_legacy_key(key) {
            return Some((key.to_string(), value.trim().to_string()));
        }
    }

    None
}

fn is_legacy_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= 64
        && key
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.' | '-'))
}

fn expand_legacy_templates(input: &str, values: &HashMap<String, String>) -> String {
    let mut current = input.to_string();

    for _ in 0..10 {
        let mut changed = false;
        let mut result = String::new();
        let mut rest = current.as_str();

        while let Some(start) = rest.find("${") {
            result.push_str(&rest[..start]);
            let after_start = &rest[start + 2..];
            if let Some(end) = after_start.find('}') {
                let key = &after_start[..end];
                if let Some(value) = values.get(key) {
                    result.push_str(value);
                    changed = true;
                } else {
                    result.push_str("${");
                    result.push_str(key);
                    result.push('}');
                }
                rest = &after_start[end + 1..];
            } else {
                result.push_str(&rest[start..]);
                rest = "";
                break;
            }
        }

        result.push_str(rest);
        if !changed {
            return result;
        }
        current = result;
    }

    current
}

fn parse_legacy_import(line: &str) -> Option<String> {
    let rest = line.strip_prefix('@')?;
    let (user_id, name) = rest.split_once('/')?;
    if user_id.is_empty() || name.is_empty() || name.contains(char::is_whitespace) {
        return None;
    }
    Some(format!("{user_id}/{name}"))
}

fn normalize_legacy_rule_line(line: &str) -> Option<String> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return Some(String::new());
    }

    let normalized = trimmed
        .replace(" ignore://host|rule", " passthrough://")
        .replace(" ignore://*", " passthrough://")
        .replace(" enable://intercept", " tlsIntercept://")
        .replace(" disable://intercept", " tlsPassthrough://")
        .replace(" enable://https", " tlsIntercept://")
        .replace(" disable://https", " tlsPassthrough://");

    Some(normalize_rule_content(&normalized))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn remote_env(name: &str, rule: &str) -> RemoteEnv {
        RemoteEnv {
            id: format!("id-{name}"),
            user_id: "liuhua.jia".to_string(),
            name: name.to_string(),
            rule: rule.to_string(),
            create_time: "2024-01-01T00:00:00.000Z".to_string(),
            update_time: "2024-01-01T00:00:00.000Z".to_string(),
        }
    }

    #[test]
    fn normalizes_legacy_variables_and_aliases() {
        let env = remote_env(
            "apaas/base",
            r#"
exclude=
port=6310 # 定义端口
*/ae/application/api ignore://host|rule
wss://localhost:${port}/ ws://localhost:${port}/ reqHeaders://(Origin=http://localhost:${port})
"#,
        );

        let actual = normalize_remote_rule(&env, std::slice::from_ref(&env));
        assert!(actual.contains("*/ae/application/api passthrough://"));
        assert!(actual.contains("wss://localhost:6310/ ws://localhost:6310/ reqHeaders://(Origin=http://localhost:6310)"));
        assert!(!actual.contains("port=6310"));
        assert!(!actual.contains("exclude="));
    }

    #[test]
    fn expands_legacy_imports() {
        let base = remote_env("shared/base", "*.example.com host://127.0.0.1\n");
        let env = remote_env(
            "syntax/test",
            r#"
@liuhua.jia/shared/base
${unknown} http://example.com:3000
"#,
        );

        let actual = normalize_remote_rule(&env, &[base.clone(), env.clone()]);
        assert!(actual.contains("# import @liuhua.jia/shared/base"));
        assert!(actual.contains("*.example.com host://127.0.0.1"));
    }

    #[test]
    fn keeps_indented_code_blocks_parseable() {
        let env = remote_env(
            "服务台联调",
            "nextoncall.bifrost.local reqHeaders://{block_var}\n \n ```block_var\n x-tt-env: ppe_online_ticket_tag\n ```\n",
        );

        let actual = normalize_remote_rule(&env, std::slice::from_ref(&env));
        assert!(actual.contains("```block_var"));
        assert!(actual.contains("x-tt-env: ppe_online_ticket_tag"));
        assert!(actual.contains("```"));
    }

    #[test]
    fn preserves_markdown_value_blocks_with_hash_lines_during_normalization() {
        let rule = r#"
https://app.example.test/api/ https://app.example.test/api/
https://app.example.test/api/ reqHeaders://{env_block}
```env_block
X-Test-Env:test_env
#comment_marker
X-Test-Flag:1
## X-Ignored-Env: ignored comment
```

https://app.example.test http://localhost:8000/
wss://app.example.test/ ws://localhost:8000/
"#;
        let env = remote_env("example/env-block", rule);

        let actual = normalize_remote_rule(&env, std::slice::from_ref(&env));

        assert!(actual.contains("https://app.example.test/api/ https://app.example.test/api/"));
        assert!(actual.contains("https://app.example.test/api/ reqHeaders://{env_block}"));
        assert!(actual.contains("#comment_marker"));
        assert!(actual.contains("## X-Ignored-Env: ignored comment"));
        assert!(actual.contains("https://app.example.test http://localhost:8000/"));
        assert!(actual.contains("wss://app.example.test/ ws://localhost:8000/"));
    }

    #[test]
    fn converts_unsupported_ignore_rules_to_passthrough() {
        let env = remote_env("jlcj", "verify.zijieapi.com ignore://htmlAppend\n");
        let actual = normalize_remote_rule(&env, std::slice::from_ref(&env));
        assert_eq!(actual.trim(), "verify.zijieapi.com passthrough://");
    }

    #[test]
    fn preserves_blank_lines_in_normal_rules() {
        let rule =
            "example.com proxy://localhost:3000\n\n# section two\napi.test.com host://127.0.0.1\n";
        let env = remote_env("my-rules", rule);
        let actual = normalize_remote_rule(&env, std::slice::from_ref(&env));
        assert_eq!(actual, rule);
    }

    #[test]
    fn preserves_blank_lines_in_legacy_rules() {
        let rule =
            "port=8080\n\nexample.com http://localhost:${port}\n\napi.test.com host://127.0.0.1\n";
        let env = remote_env("legacy", rule);
        let actual = normalize_remote_rule(&env, std::slice::from_ref(&env));
        assert!(actual.contains("\n\n"));
        assert!(actual.contains("example.com http://localhost:8080"));
    }

    #[test]
    fn skips_normalization_for_modern_rules() {
        let rule = "  example.com   proxy://localhost:3000  \n\n# my comment\napi.test.com host://127.0.0.1";
        let env = remote_env("modern", rule);
        let actual = normalize_remote_rule(&env, std::slice::from_ref(&env));
        assert_eq!(actual, rule);
    }
}
