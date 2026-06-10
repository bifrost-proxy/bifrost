use crate::{BifrostError, Result};
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuleReferenceError {
    Missing {
        source: String,
        line: usize,
        name: String,
    },
    Cycle {
        source: String,
        line: usize,
        chain: Vec<String>,
    },
}

impl RuleReferenceError {
    pub fn source(&self) -> &str {
        match self {
            RuleReferenceError::Missing { source, .. }
            | RuleReferenceError::Cycle { source, .. } => source,
        }
    }

    pub fn line(&self) -> usize {
        match self {
            RuleReferenceError::Missing { line, .. } | RuleReferenceError::Cycle { line, .. } => {
                *line
            }
        }
    }
}

impl std::fmt::Display for RuleReferenceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RuleReferenceError::Missing { source, line, name } => write!(
                f,
                "rule reference '@{name}' in '{source}' line {line} was not found"
            ),
            RuleReferenceError::Cycle {
                source,
                line,
                chain,
            } => write!(
                f,
                "cyclic rule reference in '{source}' line {line}: {}",
                chain.join(" -> ")
            ),
        }
    }
}

impl std::error::Error for RuleReferenceError {}

pub fn rule_reference_name(line: &str) -> Option<&str> {
    let trimmed = line.trim();
    if !trimmed.starts_with('@') || is_comment_directive(trimmed) {
        return None;
    }
    let rest = trimmed.strip_prefix('@')?.trim();
    if rest.is_empty() {
        return None;
    }
    let name = strip_inline_comment(rest).trim();
    if name.is_empty() {
        None
    } else {
        Some(name)
    }
}

fn is_comment_directive(trimmed: &str) -> bool {
    let Some(rest) = trimmed.strip_prefix("@comment") else {
        return false;
    };

    rest.is_empty()
        || rest
            .chars()
            .next()
            .is_some_and(|ch| ch.is_whitespace() || ch == '#')
}

fn strip_inline_comment(value: &str) -> &str {
    let comment_start = value.char_indices().find_map(|(idx, ch)| {
        if ch != '#' {
            return None;
        }
        if idx > 0 && value[..idx].chars().last().is_some_and(char::is_whitespace) {
            Some(idx)
        } else {
            None
        }
    });

    match comment_start {
        Some(idx) => &value[..idx],
        None => value,
    }
}

pub fn expand_rule_references(
    source_name: &str,
    content: &str,
    catalog: &HashMap<String, String>,
) -> std::result::Result<String, RuleReferenceError> {
    expand_rule_references_with_missing_behavior(source_name, content, catalog, false)
}

pub fn expand_rule_references_strict(
    source_name: &str,
    content: &str,
    catalog: &HashMap<String, String>,
) -> std::result::Result<String, RuleReferenceError> {
    expand_rule_references_with_missing_behavior(source_name, content, catalog, true)
}

fn expand_rule_references_with_missing_behavior(
    source_name: &str,
    content: &str,
    catalog: &HashMap<String, String>,
    error_on_missing: bool,
) -> std::result::Result<String, RuleReferenceError> {
    let mut stack = vec![source_name.to_string()];
    expand_rule_references_inner(source_name, content, catalog, &mut stack, error_on_missing)
}

pub fn expand_rule_references_to_result(
    source_name: &str,
    content: &str,
    catalog: &HashMap<String, String>,
) -> Result<String> {
    expand_rule_references(source_name, content, catalog)
        .map_err(|error| BifrostError::Rule(error.to_string()))
}

fn expand_rule_references_inner(
    source_name: &str,
    content: &str,
    catalog: &HashMap<String, String>,
    stack: &mut Vec<String>,
    error_on_missing: bool,
) -> std::result::Result<String, RuleReferenceError> {
    let mut expanded = String::new();

    for (idx, line) in content.lines().enumerate() {
        let line_number = idx + 1;
        let Some(reference_name) = rule_reference_name(line) else {
            expanded.push_str(line);
            expanded.push('\n');
            continue;
        };

        if stack.iter().any(|item| item == reference_name) {
            let mut chain = stack.clone();
            chain.push(reference_name.to_string());
            return Err(RuleReferenceError::Cycle {
                source: source_name.to_string(),
                line: line_number,
                chain,
            });
        }

        let Some(referenced_content) = catalog.get(reference_name) else {
            if error_on_missing {
                return Err(RuleReferenceError::Missing {
                    source: source_name.to_string(),
                    line: line_number,
                    name: reference_name.to_string(),
                });
            }
            continue;
        };

        stack.push(reference_name.to_string());
        let nested = expand_rule_references_inner(
            reference_name,
            referenced_content,
            catalog,
            stack,
            error_on_missing,
        )?;
        stack.pop();

        expanded.push_str(&nested);
        if !nested.ends_with('\n') {
            expanded.push('\n');
        }
    }

    Ok(expanded)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn catalog(entries: &[(&str, &str)]) -> HashMap<String, String> {
        entries
            .iter()
            .map(|(name, content)| ((*name).to_string(), (*content).to_string()))
            .collect()
    }

    #[test]
    fn detects_reference_directive() {
        assert_eq!(rule_reference_name("@shared"), Some("shared"));
        assert_eq!(
            rule_reference_name("  @folder/shared  # reuse"),
            Some("folder/shared")
        );
        assert_eq!(
            rule_reference_name("  @folder/shared\t# reuse"),
            Some("folder/shared")
        );
        assert_eq!(rule_reference_name("@comment not a reference"), None);
        assert_eq!(rule_reference_name("@comment"), None);
        assert_eq!(rule_reference_name("@comment# hidden"), None);
        assert_eq!(
            rule_reference_name("@commented-shared"),
            Some("commented-shared")
        );
        assert_eq!(
            rule_reference_name("@comment/group-shared"),
            Some("comment/group-shared")
        );
        assert_eq!(
            rule_reference_name("@#hash-prefixed"),
            Some("#hash-prefixed")
        );
        assert_eq!(rule_reference_name("# @shared"), None);
        assert_eq!(rule_reference_name("example.com host://127.0.0.1"), None);
    }

    #[test]
    fn expands_references_in_place() {
        let catalog = catalog(&[
            ("base", "base.test reqHeaders://X-Base=1"),
            (
                "outer",
                "before.test statusCode://204\n@base\nafter.test statusCode://205",
            ),
        ]);

        let expanded =
            expand_rule_references("outer", catalog.get("outer").unwrap(), &catalog).unwrap();

        assert_eq!(
            expanded,
            "before.test statusCode://204\nbase.test reqHeaders://X-Base=1\nafter.test statusCode://205\n"
        );
    }

    #[test]
    fn expands_nested_references() {
        let catalog = catalog(&[
            ("base", "base.test statusCode://200"),
            ("middle", "@base\nmiddle.test statusCode://201"),
            ("outer", "@middle\nouter.test statusCode://202"),
        ]);

        let expanded =
            expand_rule_references("outer", catalog.get("outer").unwrap(), &catalog).unwrap();

        assert!(expanded.contains("base.test statusCode://200"));
        assert!(expanded.contains("middle.test statusCode://201"));
        assert!(expanded.contains("outer.test statusCode://202"));
    }

    #[test]
    fn ignores_missing_reference_by_default() {
        let catalog = catalog(&[("outer", "@missing\nouter.test statusCode://204")]);
        let expanded =
            expand_rule_references("outer", catalog.get("outer").unwrap(), &catalog).unwrap();

        assert_eq!(expanded, "outer.test statusCode://204\n");
    }

    #[test]
    fn strict_reports_missing_reference() {
        let catalog = catalog(&[("outer", "@missing")]);
        let error = expand_rule_references_strict("outer", catalog.get("outer").unwrap(), &catalog)
            .unwrap_err();

        assert_eq!(
            error,
            RuleReferenceError::Missing {
                source: "outer".to_string(),
                line: 1,
                name: "missing".to_string(),
            }
        );
    }

    #[test]
    fn reports_reference_cycle() {
        let catalog = catalog(&[("a", "@b"), ("b", "@a")]);
        let error = expand_rule_references("a", catalog.get("a").unwrap(), &catalog).unwrap_err();

        assert_eq!(
            error,
            RuleReferenceError::Cycle {
                source: "b".to_string(),
                line: 1,
                chain: vec!["a".to_string(), "b".to_string(), "a".to_string()],
            }
        );
    }
}
