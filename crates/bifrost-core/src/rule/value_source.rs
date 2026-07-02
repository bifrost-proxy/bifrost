use std::collections::HashMap;
use std::time::Duration;

use super::ValueStore;

#[derive(Debug, Clone, PartialEq)]
pub enum ValueSource {
    Inline(String),
    InlineParams(Vec<(String, String)>),
    ParenContent(String),
    ValueRef(String),
    FilePath(String),
    RemoteUrl(String),
}

impl ValueSource {
    pub fn parse(value: &str) -> Self {
        if value.is_empty() {
            return ValueSource::Inline(String::new());
        }

        if value.starts_with("http://") || value.starts_with("https://") {
            return ValueSource::RemoteUrl(value.to_string());
        }

        if value.starts_with('/') && !value.starts_with("//") {
            return ValueSource::FilePath(value.to_string());
        }

        if value.starts_with('(') && value.ends_with(')') && value.len() > 2 {
            let content = &value[1..value.len() - 1];
            return ValueSource::ParenContent(content.to_string());
        }

        if value.starts_with('{') && value.ends_with('}') && value.len() > 2 {
            let var_name = &value[1..value.len() - 1];
            if !var_name.contains('{') && !var_name.contains('}') {
                return ValueSource::ValueRef(var_name.to_string());
            }
        }

        if value.contains('=') && !value.contains('/') && !value.contains('{') {
            let params = parse_inline_params(value);
            if !params.is_empty() {
                return ValueSource::InlineParams(params);
            }
        }

        ValueSource::Inline(value.to_string())
    }

    pub fn as_inline(&self) -> Option<&str> {
        match self {
            ValueSource::Inline(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_paren_content(&self) -> Option<&str> {
        match self {
            ValueSource::ParenContent(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_value_ref(&self) -> Option<&str> {
        match self {
            ValueSource::ValueRef(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_file_path(&self) -> Option<&str> {
        match self {
            ValueSource::FilePath(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_remote_url(&self) -> Option<&str> {
        match self {
            ValueSource::RemoteUrl(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_inline_params(&self) -> Option<&[(String, String)]> {
        match self {
            ValueSource::InlineParams(params) => Some(params),
            _ => None,
        }
    }

    pub fn get_raw_value(&self) -> String {
        match self {
            ValueSource::Inline(s) => s.clone(),
            ValueSource::InlineParams(params) => params
                .iter()
                .map(|(k, v)| format!("{}={}", k, v))
                .collect::<Vec<_>>()
                .join("&"),
            ValueSource::ParenContent(s) => format!("({})", s),
            ValueSource::ValueRef(s) => format!("{{{}}}", s),
            ValueSource::FilePath(s) => s.clone(),
            ValueSource::RemoteUrl(s) => s.clone(),
        }
    }

    pub fn is_content_source(&self) -> bool {
        matches!(
            self,
            ValueSource::ParenContent(_)
                | ValueSource::ValueRef(_)
                | ValueSource::FilePath(_)
                | ValueSource::RemoteUrl(_)
        )
    }

    pub fn to_params_map(&self) -> HashMap<String, String> {
        match self {
            ValueSource::InlineParams(params) => params.iter().cloned().collect(),
            _ => HashMap::new(),
        }
    }

    pub fn resolve(&self, store: &dyn ValueStore) -> Option<String> {
        match self {
            ValueSource::Inline(s) => Some(s.clone()),
            ValueSource::InlineParams(params) => Some(
                params
                    .iter()
                    .map(|(k, v)| format!("{}={}", k, v))
                    .collect::<Vec<_>>()
                    .join("&"),
            ),
            ValueSource::ParenContent(s) => Some(s.clone()),
            ValueSource::ValueRef(var_name) => store.get(var_name),
            ValueSource::FilePath(path) => std::fs::read_to_string(path).ok(),
            ValueSource::RemoteUrl(url) => {
                let client = crate::outbound_blocking_reqwest_client_builder()
                    .timeout(Duration::from_secs(5))
                    .build()
                    .ok()?;
                let response = client.get(url).send().ok()?;
                if !response.status().is_success() {
                    return None;
                }
                response.text().ok()
            }
        }
    }

    pub fn resolve_with_fallback(&self, store: &dyn ValueStore) -> String {
        self.resolve(store).unwrap_or_else(|| self.get_raw_value())
    }
}

fn parse_inline_params(value: &str) -> Vec<(String, String)> {
    let mut params = Vec::new();

    for pair in value.split('&') {
        let pair = pair.trim();
        if pair.is_empty() {
            continue;
        }

        if let Some(eq_pos) = pair.find('=') {
            let key = pair[..eq_pos].trim();
            let val = pair[eq_pos + 1..].trim();
            if !key.is_empty() {
                params.push((key.to_string(), val.to_string()));
            }
        } else if !pair.is_empty() {
            params.push((pair.to_string(), String::new()));
        }
    }

    params
}

pub fn expand_value_ref(value_ref: &str, values: &HashMap<String, String>) -> Option<String> {
    values.get(value_ref).cloned()
}

pub fn expand_value_ref_from_store(value_ref: &str, store: &dyn ValueStore) -> Option<String> {
    store.get(value_ref)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_inline_value() {
        let source = ValueSource::parse("127.0.0.1:8080");
        assert_eq!(source, ValueSource::Inline("127.0.0.1:8080".to_string()));
    }

    #[test]
    fn test_parse_empty_value() {
        let source = ValueSource::parse("");
        assert_eq!(source, ValueSource::Inline(String::new()));
    }

    #[test]
    fn test_parse_inline_params() {
        let source = ValueSource::parse("X-Custom=test&X-Another=value");
        match source {
            ValueSource::InlineParams(params) => {
                assert_eq!(params.len(), 2);
                assert_eq!(params[0], ("X-Custom".to_string(), "test".to_string()));
                assert_eq!(params[1], ("X-Another".to_string(), "value".to_string()));
            }
            _ => panic!("Expected InlineParams"),
        }
    }

    #[test]
    fn test_parse_inline_params_single() {
        let source = ValueSource::parse("key=value");
        match source {
            ValueSource::InlineParams(params) => {
                assert_eq!(params.len(), 1);
                assert_eq!(params[0], ("key".to_string(), "value".to_string()));
            }
            _ => panic!("Expected InlineParams"),
        }
    }

    #[test]
    fn test_parse_paren_content() {
        let source = ValueSource::parse("({\"ok\":true,\"code\":0})");
        assert_eq!(
            source,
            ValueSource::ParenContent("{\"ok\":true,\"code\":0}".to_string())
        );
    }

    #[test]
    fn test_parse_paren_content_simple() {
        let source = ValueSource::parse("(hello world)");
        assert_eq!(source, ValueSource::ParenContent("hello world".to_string()));
    }

    #[test]
    fn test_parse_value_ref() {
        let source = ValueSource::parse("{myResponse}");
        assert_eq!(source, ValueSource::ValueRef("myResponse".to_string()));
    }

    #[test]
    fn test_parse_value_ref_with_extension() {
        let source = ValueSource::parse("{config.json}");
        assert_eq!(source, ValueSource::ValueRef("config.json".to_string()));
    }

    #[test]
    fn test_parse_file_path() {
        let source = ValueSource::parse("/etc/mock.json");
        assert_eq!(source, ValueSource::FilePath("/etc/mock.json".to_string()));
    }

    #[test]
    fn test_parse_file_path_absolute() {
        let source = ValueSource::parse("/Users/test/data.json");
        assert_eq!(
            source,
            ValueSource::FilePath("/Users/test/data.json".to_string())
        );
    }

    #[test]
    fn test_parse_remote_url_http() {
        let source = ValueSource::parse("http://example.com/data.json");
        assert_eq!(
            source,
            ValueSource::RemoteUrl("http://example.com/data.json".to_string())
        );
    }

    #[test]
    fn test_parse_remote_url_https() {
        let source = ValueSource::parse("https://api.example.com/config");
        assert_eq!(
            source,
            ValueSource::RemoteUrl("https://api.example.com/config".to_string())
        );
    }

    #[test]
    fn test_as_inline() {
        let source = ValueSource::Inline("test".to_string());
        assert_eq!(source.as_inline(), Some("test"));

        let source2 = ValueSource::FilePath("/path".to_string());
        assert_eq!(source2.as_inline(), None);
    }

    #[test]
    fn test_as_paren_content() {
        let source = ValueSource::ParenContent("content".to_string());
        assert_eq!(source.as_paren_content(), Some("content"));
    }

    #[test]
    fn test_as_value_ref() {
        let source = ValueSource::ValueRef("varName".to_string());
        assert_eq!(source.as_value_ref(), Some("varName"));
    }

    #[test]
    fn test_as_file_path() {
        let source = ValueSource::FilePath("/etc/config".to_string());
        assert_eq!(source.as_file_path(), Some("/etc/config"));
    }

    #[test]
    fn test_as_remote_url() {
        let source = ValueSource::RemoteUrl("http://example.com".to_string());
        assert_eq!(source.as_remote_url(), Some("http://example.com"));
    }

    #[test]
    fn test_as_inline_params() {
        let params = vec![("key".to_string(), "value".to_string())];
        let source = ValueSource::InlineParams(params.clone());
        assert_eq!(source.as_inline_params(), Some(params.as_slice()));
    }

    #[test]
    fn test_get_raw_value_inline() {
        let source = ValueSource::Inline("test".to_string());
        assert_eq!(source.get_raw_value(), "test");
    }

    #[test]
    fn test_get_raw_value_params() {
        let source = ValueSource::InlineParams(vec![
            ("a".to_string(), "1".to_string()),
            ("b".to_string(), "2".to_string()),
        ]);
        assert_eq!(source.get_raw_value(), "a=1&b=2");
    }

    #[test]
    fn test_get_raw_value_paren() {
        let source = ValueSource::ParenContent("content".to_string());
        assert_eq!(source.get_raw_value(), "(content)");
    }

    #[test]
    fn test_get_raw_value_ref() {
        let source = ValueSource::ValueRef("var".to_string());
        assert_eq!(source.get_raw_value(), "{var}");
    }

    #[test]
    fn test_is_content_source() {
        assert!(!ValueSource::Inline("test".to_string()).is_content_source());
        assert!(!ValueSource::InlineParams(vec![]).is_content_source());
        assert!(ValueSource::ParenContent("test".to_string()).is_content_source());
        assert!(ValueSource::ValueRef("var".to_string()).is_content_source());
        assert!(ValueSource::FilePath("/path".to_string()).is_content_source());
        assert!(ValueSource::RemoteUrl("http://test".to_string()).is_content_source());
    }

    #[test]
    fn test_to_params_map() {
        let source = ValueSource::InlineParams(vec![
            ("key1".to_string(), "val1".to_string()),
            ("key2".to_string(), "val2".to_string()),
        ]);
        let map = source.to_params_map();
        assert_eq!(map.get("key1"), Some(&"val1".to_string()));
        assert_eq!(map.get("key2"), Some(&"val2".to_string()));
    }

    #[test]
    fn test_to_params_map_non_params() {
        let source = ValueSource::Inline("test".to_string());
        let map = source.to_params_map();
        assert!(map.is_empty());
    }

    #[test]
    fn test_expand_value_ref() {
        let mut values = HashMap::new();
        values.insert("myVar".to_string(), "resolved_value".to_string());

        assert_eq!(
            expand_value_ref("myVar", &values),
            Some("resolved_value".to_string())
        );
        assert_eq!(expand_value_ref("unknown", &values), None);
    }

    #[test]
    fn test_parse_inline_params_func() {
        let params = parse_inline_params("a=1&b=2&c=3");
        assert_eq!(params.len(), 3);
        assert_eq!(params[0], ("a".to_string(), "1".to_string()));
        assert_eq!(params[1], ("b".to_string(), "2".to_string()));
        assert_eq!(params[2], ("c".to_string(), "3".to_string()));
    }

    #[test]
    fn test_parse_inline_params_empty_value() {
        let params = parse_inline_params("key=");
        assert_eq!(params.len(), 1);
        assert_eq!(params[0], ("key".to_string(), String::new()));
    }

    #[test]
    fn test_parse_inline_params_no_value() {
        let params = parse_inline_params("flag");
        assert_eq!(params.len(), 1);
        assert_eq!(params[0], ("flag".to_string(), String::new()));
    }

    #[test]
    fn test_parse_url_with_query_as_inline() {
        let source = ValueSource::parse("example.com/path?query=1");
        assert_eq!(
            source,
            ValueSource::Inline("example.com/path?query=1".to_string())
        );
    }

    #[test]
    fn test_parse_host_port_as_inline() {
        let source = ValueSource::parse("localhost:3000");
        assert_eq!(source, ValueSource::Inline("localhost:3000".to_string()));
    }

    #[test]
    fn test_value_source_clone() {
        let source = ValueSource::FilePath("/test".to_string());
        let cloned = source.clone();
        assert_eq!(source, cloned);
    }

    #[test]
    fn test_value_source_debug() {
        let source = ValueSource::Inline("test".to_string());
        let debug_str = format!("{:?}", source);
        assert!(debug_str.contains("Inline"));
    }

    #[test]
    fn test_resolve_inline() {
        use super::super::MemoryValueStore;
        let store = MemoryValueStore::new();
        let source = ValueSource::Inline("test_value".to_string());
        assert_eq!(source.resolve(&store), Some("test_value".to_string()));
    }

    #[test]
    fn test_resolve_paren_content() {
        use super::super::MemoryValueStore;
        let store = MemoryValueStore::new();
        let source = ValueSource::ParenContent(r#"{"ok":true}"#.to_string());
        assert_eq!(source.resolve(&store), Some(r#"{"ok":true}"#.to_string()));
    }

    #[test]
    fn test_resolve_value_ref() {
        use super::super::MemoryValueStore;
        let mut store = MemoryValueStore::new();
        store.set("myVar", "resolved_content".to_string());

        let source = ValueSource::ValueRef("myVar".to_string());
        assert_eq!(source.resolve(&store), Some("resolved_content".to_string()));

        let source2 = ValueSource::ValueRef("unknown".to_string());
        assert_eq!(source2.resolve(&store), None);
    }

    #[test]
    fn test_resolve_with_fallback() {
        use super::super::MemoryValueStore;
        let store = MemoryValueStore::new();

        let source = ValueSource::ValueRef("unknown".to_string());
        assert_eq!(source.resolve_with_fallback(&store), "{unknown}");
    }

    #[test]
    fn test_expand_value_ref_from_store() {
        use super::super::MemoryValueStore;
        let mut store = MemoryValueStore::new();
        store.set("key", "value".to_string());

        assert_eq!(
            expand_value_ref_from_store("key", &store),
            Some("value".to_string())
        );
        assert_eq!(expand_value_ref_from_store("missing", &store), None);
    }
}
