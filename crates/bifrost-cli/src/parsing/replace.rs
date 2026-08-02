use super::url_decode;

pub struct ParsedReplaceRules {
    pub string_rules: Vec<(String, String)>,
    pub regex_rules: Vec<bifrost_proxy::RegexReplace>,
}

pub fn parse_regex_pattern(s: &str) -> Option<(regex::Regex, bool)> {
    let s = s.trim();
    if !s.starts_with('/') {
        return None;
    }

    let global = s.ends_with("/g") || s.ends_with("/gi") || s.ends_with("/ig");
    let case_insensitive = s.ends_with("/i") || s.ends_with("/gi") || s.ends_with("/ig");

    let end_pos = if global && case_insensitive {
        s.len() - 3
    } else if global || case_insensitive {
        s.len() - 2
    } else if s.len() > 1 && s.ends_with('/') {
        s.len() - 1
    } else {
        return None;
    };

    let pattern_str = &s[1..end_pos];
    if pattern_str.is_empty() {
        return None;
    }

    let regex_result = if case_insensitive {
        regex::RegexBuilder::new(pattern_str)
            .case_insensitive(true)
            .build()
    } else {
        regex::Regex::new(pattern_str)
    };

    match regex_result {
        Ok(re) => Some((re, global)),
        Err(e) => {
            tracing::warn!("Invalid regex pattern '{}': {}", pattern_str, e);
            None
        }
    }
}

pub fn parse_replace_value(value: &str) -> ParsedReplaceRules {
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

        if let Some((from, to)) = pair.split_once('=') {
            let from = url_decode(from);
            let to = url_decode(to);

            push_replace_pair(&mut string_rules, &mut regex_rules, from, to);
        } else {
            let from = url_decode(pair);
            push_replace_pair(&mut string_rules, &mut regex_rules, from, String::new());
        }
    }

    ParsedReplaceRules {
        string_rules,
        regex_rules,
    }
}

fn push_replace_pair(
    string_rules: &mut Vec<(String, String)>,
    regex_rules: &mut Vec<bifrost_proxy::RegexReplace>,
    from: String,
    to: String,
) {
    if let Some((regex, global)) = parse_regex_pattern(&from) {
        regex_rules.push(bifrost_proxy::RegexReplace {
            pattern: regex,
            replacement: to,
            global,
        });
    } else {
        string_rules.push((from, to));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_replace_value_supports_json_object_and_regex_keys() {
        let parsed = parse_replace_value(
            r#"{".doupay.com\"":".nodoupay.com\"","/baohuaxia\\.com/g":"nobaohuaxia.com"}"#,
        );

        assert_eq!(
            parsed.string_rules,
            vec![(r#".doupay.com""#.into(), r#".nodoupay.com""#.into())]
        );
        assert_eq!(parsed.regex_rules.len(), 1);
        assert!(parsed.regex_rules[0].global);
        assert_eq!(parsed.regex_rules[0].replacement, "nobaohuaxia.com");
        assert!(parsed.regex_rules[0].pattern.is_match("baohuaxia.com"));
    }

    #[test]
    fn parse_replace_value_keeps_legacy_ampersand_and_url_decode_behavior() {
        let parsed = parse_replace_value("old%20value=new%20value&remove=");

        assert_eq!(
            parsed.string_rules,
            vec![
                ("old value".into(), "new value".into()),
                ("remove".into(), String::new()),
            ]
        );
        assert!(parsed.regex_rules.is_empty());
    }

    #[test]
    fn parse_replace_value_empty_json_object_creates_no_rules() {
        let parsed = parse_replace_value("{}");
        assert!(parsed.string_rules.is_empty());
        assert!(parsed.regex_rules.is_empty());
    }

    #[test]
    fn parse_replace_value_malformed_json_keeps_legacy_fallback() {
        let parsed = parse_replace_value(r#"{"old":}"#);
        assert_eq!(
            parsed.string_rules,
            vec![(r#"{"old":}"#.into(), String::new())]
        );
        assert!(parsed.regex_rules.is_empty());
    }
}
