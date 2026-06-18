//! Minimal JSON Path subset used by Bifrost search filters.
//!
//! Supported syntax:
//!
//! - `$` — root
//! - `.name` — object member (name must match `[A-Za-z0-9_\-]+` characters, no quoting)
//! - `[N]` — array index (non-negative integer)
//! - `[*]` — array wildcard (diverges into every element)
//!
//! Not supported (returns empty result):
//!
//! - Recursive descent `..`
//! - Filter expressions `[?(@.x > 1)]`
//! - Slice expressions `[1:3]`
//! - Quoted member access `["x.y"]`
//!
//! The evaluator is intentionally permissive: a malformed path simply
//! produces an empty result so callers can treat it as "no match" without
//! propagating errors.

use serde_json::Value;

/// Parsed token of the JSON path expression.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Token {
    /// `.name`
    Member(String),
    /// `[N]`
    Index(usize),
    /// `[*]`
    Wildcard,
}

/// Evaluate `path` against `value` and return every matching subvalue.
///
/// Returns an empty vector when the path is invalid or no element matches.
pub fn eval<'a>(value: &'a Value, path: &str) -> Vec<&'a Value> {
    let tokens = match parse(path) {
        Some(tokens) => tokens,
        None => return Vec::new(),
    };

    let mut current: Vec<&Value> = vec![value];
    for token in tokens {
        let mut next: Vec<&Value> = Vec::new();
        for node in current.drain(..) {
            match &token {
                Token::Member(name) => {
                    if let Some(obj) = node.as_object() {
                        if let Some(child) = obj.get(name) {
                            next.push(child);
                        }
                    }
                }
                Token::Index(idx) => {
                    if let Some(arr) = node.as_array() {
                        if let Some(child) = arr.get(*idx) {
                            next.push(child);
                        }
                    }
                }
                Token::Wildcard => {
                    if let Some(arr) = node.as_array() {
                        for child in arr {
                            next.push(child);
                        }
                    }
                }
            }
        }
        current = next;
        if current.is_empty() {
            return current;
        }
    }
    current
}

/// Parse `$` followed by zero or more tokens. Returns `None` on syntax error.
fn parse(path: &str) -> Option<Vec<Token>> {
    let bytes = path.as_bytes();
    if bytes.is_empty() || bytes[0] != b'$' {
        return None;
    }

    let mut tokens = Vec::new();
    let mut i = 1;
    while i < bytes.len() {
        match bytes[i] {
            b'.' => {
                i += 1;
                let start = i;
                while i < bytes.len() && is_member_char(bytes[i]) {
                    i += 1;
                }
                if start == i {
                    return None;
                }
                let name = path.get(start..i)?.to_string();
                tokens.push(Token::Member(name));
            }
            b'[' => {
                i += 1;
                let start = i;
                while i < bytes.len() && bytes[i] != b']' {
                    i += 1;
                }
                if i >= bytes.len() {
                    return None;
                }
                let inner = path.get(start..i)?;
                i += 1; // consume ]
                if inner == "*" {
                    tokens.push(Token::Wildcard);
                } else {
                    let idx: usize = inner.parse().ok()?;
                    tokens.push(Token::Index(idx));
                }
            }
            _ => return None,
        }
    }
    Some(tokens)
}

fn is_member_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'-'
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn root_returns_full_document() {
        let v = json!({"a": 1});
        let hits = eval(&v, "$");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0], &v);
    }

    #[test]
    fn nested_member_access() {
        let v = json!({"a": {"b": {"c": 42}}});
        let hits = eval(&v, "$.a.b.c");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0], &json!(42));
    }

    #[test]
    fn array_index_access() {
        let v = json!({"xs": [10, 20, 30]});
        let hits = eval(&v, "$.xs[1]");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0], &json!(20));
    }

    #[test]
    fn array_wildcard_returns_all() {
        let v = json!({"xs": [{"k": 1}, {"k": 2}, {"k": 3}]});
        let hits = eval(&v, "$.xs[*].k");
        assert_eq!(hits.len(), 3);
        let nums: Vec<i64> = hits.iter().map(|n| n.as_i64().unwrap()).collect();
        assert_eq!(nums, vec![1, 2, 3]);
    }

    #[test]
    fn missing_member_returns_empty() {
        let v = json!({"a": 1});
        assert!(eval(&v, "$.b").is_empty());
        assert!(eval(&v, "$.a.b").is_empty());
    }

    #[test]
    fn out_of_bounds_index_returns_empty() {
        let v = json!({"xs": [1, 2]});
        assert!(eval(&v, "$.xs[7]").is_empty());
    }

    #[test]
    fn invalid_path_returns_empty() {
        let v = json!({"a": 1});
        assert!(eval(&v, "a").is_empty());
        assert!(eval(&v, "$.").is_empty());
        assert!(eval(&v, "$[a]").is_empty());
        assert!(eval(&v, "$[1").is_empty());
    }

    #[test]
    fn wildcard_on_empty_array_returns_empty() {
        let v = json!({"xs": []});
        assert!(eval(&v, "$.xs[*]").is_empty());
        assert!(eval(&v, "$.xs[*].k").is_empty());
    }

    #[test]
    fn deep_array_member_chain() {
        let v = json!({
            "choices": [
                {"index": 0, "delta": {"content": "hello"}},
                {"index": 1, "delta": {"content": "world"}}
            ]
        });
        let hits = eval(&v, "$.choices[*].delta.content");
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0], &json!("hello"));
        assert_eq!(hits[1], &json!("world"));
    }

    #[test]
    fn member_with_dash_and_underscore() {
        let v = json!({"x-y": {"a_b": 7}});
        let hits = eval(&v, "$.x-y.a_b");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0], &json!(7));
    }
}
