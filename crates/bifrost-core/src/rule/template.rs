use chrono::Utc;
use rand::Rng;
use regex::{Captures, Regex};
use std::collections::HashMap;

use super::context::RequestContext;

const VERSION: &str = env!("CARGO_PKG_VERSION");

lazy_static::lazy_static! {
    static ref CAPTURE_VAR_RE: Regex = Regex::new(r"\$(\d+)").unwrap();

    static ref NAMED_VAR_RE: Regex = Regex::new(r"\$\{([a-zA-Z_][a-zA-Z0-9_]*)\}").unwrap();

    static ref INLINE_FILE_RE: Regex = Regex::new(r"\{([^{}\s]+\.[a-zA-Z0-9]+)\}").unwrap();

    static ref VALUE_REF_RE: Regex = Regex::new(r"\{([a-zA-Z_][a-zA-Z0-9_]*)\}").unwrap();

    static ref TPL_VAR_RE: Regex = Regex::new(r"(?ix)
        (\$)?                                   # $1: escape marker ($$)
        \$\{                                    # literal ${
        (\{)?                                   # $2: url encode marker {
        (                                       # $3: variable name
            id|reqId|
            now|random(?:Int\(\d{1,15}(?:-\d{1,15})?\))?|randomUUID|
            version|
            host|port|hostname|
            realPort|realHost|realUrl|
            url|query|search|queryString|searchString|path|pathname|
            clientId|localClientId|
            ip|clientIp|clientPort|remoteAddress|remotePort|
            serverIp|serverPort|
            method|status(?:Code)?|
            reqCookies?|resCookies?|
            reqH(?:eaders?)?|resH(?:eaders?)?|
            env
        )
        (?:\.([^{}]+))?                         # $4: property key (e.g., .key or .replace(...))
        \}                                      # literal }
        (\})?                                   # $5: url encode end marker }
    ").unwrap();

    static ref RANDOM_INT_RE: Regex = Regex::new(r"(?i)randomInt\((\d{1,15})(?:-(\d{1,15}))?\)").unwrap();

    static ref REPLACE_PATTERN_RE: Regex = Regex::new(r"(?s)^(.*?)replace\((.+),([^)]*)\)$").unwrap();
}

pub struct TemplateEngine;

impl TemplateEngine {
    pub fn expand(
        template: &str,
        captures: Option<&[String]>,
        values: &HashMap<String, String>,
    ) -> String {
        let ctx = RequestContext::default();
        Self::expand_with_context(template, &ctx, captures, values)
    }

    pub fn expand_with_context(
        template: &str,
        ctx: &RequestContext,
        captures: Option<&[String]>,
        values: &HashMap<String, String>,
    ) -> String {
        let mut result = template.to_string();

        result = Self::expand_captures(&result, captures);
        result = Self::expand_builtin_vars(&result, ctx, values);
        result = Self::expand_named_vars(&result, values);
        result = Self::expand_value_refs(&result, values);
        result = Self::expand_inline_files(&result, values);

        result
    }

    fn expand_captures(template: &str, captures: Option<&[String]>) -> String {
        let Some(caps) = captures else {
            return template.to_string();
        };

        CAPTURE_VAR_RE
            .replace_all(template, |cap: &Captures| {
                let index: usize = cap.get(1).unwrap().as_str().parse().unwrap_or(0);
                if index > 0 && index <= caps.len() {
                    caps[index - 1].clone()
                } else {
                    cap.get(0).unwrap().as_str().to_string()
                }
            })
            .to_string()
    }

    fn expand_builtin_vars(
        template: &str,
        ctx: &RequestContext,
        values: &HashMap<String, String>,
    ) -> String {
        TPL_VAR_RE
            .replace_all(template, |cap: &Captures| {
                let escape = cap.get(1).map(|m| m.as_str()) == Some("$");
                if escape {
                    let rest = &cap.get(0).unwrap().as_str()[1..];
                    return rest.to_string();
                }

                let url_encode = cap.get(2).is_some();
                let var_name = cap.get(3).map(|m| m.as_str()).unwrap_or("");
                let property_key = cap.get(4).map(|m| m.as_str());
                let url_encode_end = cap.get(5).is_some();

                if url_encode && !url_encode_end {
                    return cap.get(0).unwrap().as_str().to_string();
                }

                // Response-phase variables are resolved against data that does not exist
                // during request-phase resolution (status code, response headers/cookies).
                // When that data is absent, leave the token literal so a later response-phase
                // pass can fill it in, instead of collapsing it to an empty string.
                if Self::is_deferred_response_var(var_name, ctx) {
                    return cap.get(0).unwrap().as_str().to_string();
                }

                let (actual_key, replace_pattern) = Self::parse_property_key(property_key);

                let mut value =
                    Self::resolve_builtin_var(ctx, var_name, actual_key.as_deref(), values);

                if let Some((pattern, replacement)) = replace_pattern {
                    value = Self::apply_replace(&value, &pattern, &replacement);
                }

                if url_encode && !value.is_empty() {
                    value = urlencoding::encode(&value).into_owned();
                }

                if !url_encode && url_encode_end {
                    value.push('}');
                }

                value
            })
            .to_string()
    }

    fn parse_property_key(key: Option<&str>) -> (Option<String>, Option<(String, String)>) {
        let Some(key) = key else {
            return (None, None);
        };

        if let Some(caps) = REPLACE_PATTERN_RE.captures(key) {
            let prefix = caps.get(1).map(|m| m.as_str().to_string());
            let pattern = caps
                .get(2)
                .map(|m| m.as_str().to_string())
                .unwrap_or_default();
            let replacement = caps
                .get(3)
                .map(|m| m.as_str().to_string())
                .unwrap_or_default();

            let actual_key = if prefix.as_ref().map(|s| s.is_empty()).unwrap_or(true) {
                None
            } else {
                prefix
            };

            return (actual_key, Some((pattern, replacement)));
        }

        (Some(key.to_string()), None)
    }

    fn apply_replace(value: &str, pattern: &str, replacement: &str) -> String {
        lazy_static::lazy_static! {
            static ref ORIG_REG_EXP: Regex = Regex::new(r"^/(.+)/([igmu]{0,4})$").unwrap();
        }

        if let Some(caps) = ORIG_REG_EXP.captures(pattern) {
            let regex_pattern = caps.get(1).map(|m| m.as_str()).unwrap_or("");
            let flags = caps.get(2).map(|m| m.as_str()).unwrap_or("");

            let case_insensitive = flags.contains('i');
            let global = flags.contains('g');

            let regex_str = if case_insensitive {
                format!("(?i){}", regex_pattern)
            } else {
                regex_pattern.to_string()
            };

            if let Ok(re) = Regex::new(&regex_str) {
                return if global {
                    re.replace_all(value, replacement).to_string()
                } else {
                    re.replace(value, replacement).to_string()
                };
            }
        }

        value.replace(pattern, replacement)
    }

    /// Whether `name` is a response-phase variable whose backing data is not yet
    /// populated on `ctx`. Such tokens must be left literal at request phase so the
    /// proxy can expand them once the upstream response is available.
    fn is_deferred_response_var(name: &str, ctx: &RequestContext) -> bool {
        match name.to_ascii_lowercase().as_str() {
            "status" | "statuscode" => ctx.status_code.is_none(),
            "resh" | "resheader" | "resheaders" => ctx.res_headers.is_none(),
            _ => false,
        }
    }

    fn resolve_builtin_var(
        ctx: &RequestContext,
        name: &str,
        key: Option<&str>,
        values: &HashMap<String, String>,
    ) -> String {
        let lname = name.to_lowercase();

        if let Some(caps) = RANDOM_INT_RE.captures(name) {
            return Self::resolve_random_int(&caps);
        }

        match lname.as_str() {
            "now" => Utc::now().timestamp_millis().to_string(),
            "random" => rand::thread_rng().gen::<f64>().to_string(),
            "randomuuid" => uuid::Uuid::new_v4().to_string(),
            "id" | "reqid" => ctx.req_id.clone(),
            "version" => VERSION.to_string(),

            "url" => Self::resolve_url_var(ctx, key, values),
            "host" => ctx.host.clone(),
            "hostname" => ctx.hostname.clone(),
            "port" => ctx.port.to_string(),
            "path" => ctx.path.clone(),
            "pathname" => ctx.pathname.clone(),
            "query" => Self::resolve_query_var(ctx, key),
            "search" => ctx.search.clone().unwrap_or_default(),
            "querystring" | "searchstring" => ctx.search.clone().unwrap_or_else(|| "?".to_string()),

            "realurl" => ctx.real_url.clone().unwrap_or_default(),
            "realhost" => ctx.real_host.clone().unwrap_or_default(),
            "realport" => ctx.real_port.map(|p| p.to_string()).unwrap_or_default(),

            "ip" | "clientip" => ctx.client_ip.clone(),
            "clientport" => ctx.client_port.to_string(),
            "remoteaddress" => ctx.remote_address.clone(),
            "remoteport" => ctx.remote_port.to_string(),

            "serverip" => ctx
                .server_ip
                .clone()
                .unwrap_or_else(|| "127.0.0.1".to_string()),
            "serverport" => ctx.server_port.map(|p| p.to_string()).unwrap_or_default(),
            "status" | "statuscode" => ctx.status_code.map(|c| c.to_string()).unwrap_or_default(),

            "method" => ctx.method.clone(),

            "clientid" => ctx.client_id.clone().unwrap_or_default(),
            "localclientid" => ctx.local_client_id.clone().unwrap_or_default(),

            "reqcookie" | "reqcookies" => Self::resolve_req_cookies(ctx, key),
            "rescookie" | "rescookies" => Self::resolve_res_cookies(ctx, key),
            "reqh" | "reqheader" | "reqheaders" => Self::resolve_req_headers(ctx, key),
            "resh" | "resheader" | "resheaders" => Self::resolve_res_headers(ctx, key),

            "env" => Self::resolve_env_var(key),

            _ => String::new(),
        }
    }

    fn resolve_random_int(caps: &Captures) -> String {
        let first: i64 = caps.get(1).unwrap().as_str().parse().unwrap_or(0);

        if let Some(second_match) = caps.get(2) {
            let second: i64 = second_match.as_str().parse().unwrap_or(0);
            let (min, max) = if first < second {
                (first, second)
            } else {
                (second, first)
            };
            let range = max - min + 1;
            let result = min + (rand::thread_rng().gen::<i64>().abs() % range);
            result.to_string()
        } else {
            let max = first;
            (rand::thread_rng().gen::<i64>().abs() % (max + 1)).to_string()
        }
    }

    fn resolve_url_var(
        ctx: &RequestContext,
        key: Option<&str>,
        values: &HashMap<String, String>,
    ) -> String {
        if let Some(k) = key {
            values.get(k).cloned().unwrap_or_else(|| ctx.url.clone())
        } else {
            ctx.url.clone()
        }
    }

    fn resolve_query_var(ctx: &RequestContext, key: Option<&str>) -> String {
        if let Some(k) = key {
            ctx.get_query_param(k).unwrap_or_default()
        } else {
            ctx.query.clone().unwrap_or_default()
        }
    }

    fn resolve_req_cookies(ctx: &RequestContext, key: Option<&str>) -> String {
        if let Some(k) = key {
            ctx.get_cookie(k).cloned().unwrap_or_default()
        } else {
            ctx.req_cookies
                .iter()
                .map(|(k, v)| format!("{}={}", k, v))
                .collect::<Vec<_>>()
                .join("; ")
        }
    }

    fn resolve_res_cookies(ctx: &RequestContext, key: Option<&str>) -> String {
        if let Some(k) = key {
            ctx.res_cookies
                .as_ref()
                .and_then(|c| c.get(k))
                .cloned()
                .unwrap_or_default()
        } else {
            ctx.res_cookies
                .as_ref()
                .map(|c| {
                    c.iter()
                        .map(|(k, v)| format!("{}={}", k, v))
                        .collect::<Vec<_>>()
                        .join("; ")
                })
                .unwrap_or_default()
        }
    }

    fn resolve_req_headers(ctx: &RequestContext, key: Option<&str>) -> String {
        if let Some(k) = key {
            ctx.get_header(k).cloned().unwrap_or_default()
        } else {
            ctx.req_headers
                .iter()
                .map(|(k, v)| format!("{}: {}", k, v))
                .collect::<Vec<_>>()
                .join("\r\n")
        }
    }

    fn resolve_res_headers(ctx: &RequestContext, key: Option<&str>) -> String {
        if let Some(k) = key {
            ctx.res_headers
                .as_ref()
                .and_then(|h| h.get(&k.to_lowercase()))
                .cloned()
                .unwrap_or_default()
        } else {
            ctx.res_headers
                .as_ref()
                .map(|h| {
                    h.iter()
                        .map(|(k, v)| format!("{}: {}", k, v))
                        .collect::<Vec<_>>()
                        .join("\r\n")
                })
                .unwrap_or_default()
        }
    }

    fn resolve_env_var(key: Option<&str>) -> String {
        if let Some(k) = key {
            std::env::var(k)
                .or_else(|_| match k {
                    "USER" => std::env::var("USERNAME"),
                    "USERNAME" => std::env::var("USER"),
                    "HOME" => std::env::var("USERPROFILE"),
                    "USERPROFILE" => std::env::var("HOME"),
                    _ => Err(std::env::VarError::NotPresent),
                })
                .unwrap_or_default()
        } else {
            String::new()
        }
    }

    fn expand_named_vars(template: &str, values: &HashMap<String, String>) -> String {
        NAMED_VAR_RE
            .replace_all(template, |cap: &Captures| {
                let name = cap.get(1).unwrap().as_str();
                values
                    .get(name)
                    .cloned()
                    .unwrap_or_else(|| cap.get(0).unwrap().as_str().to_string())
            })
            .to_string()
    }

    fn expand_value_refs(template: &str, values: &HashMap<String, String>) -> String {
        VALUE_REF_RE
            .replace_all(template, |cap: &Captures| {
                let name = cap.get(1).unwrap().as_str();
                values
                    .get(name)
                    .cloned()
                    .unwrap_or_else(|| cap.get(0).unwrap().as_str().to_string())
            })
            .to_string()
    }

    fn expand_inline_files(template: &str, values: &HashMap<String, String>) -> String {
        INLINE_FILE_RE
            .replace_all(template, |cap: &Captures| {
                let filename = cap.get(1).unwrap().as_str();
                values
                    .get(filename)
                    .cloned()
                    .unwrap_or_else(|| cap.get(0).unwrap().as_str().to_string())
            })
            .to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx_with_url(url: &str) -> RequestContext {
        RequestContext::builder()
            .url(url)
            .host("example.com")
            .hostname("example.com")
            .path("/test")
            .pathname("/test")
            .build()
    }

    #[test]
    fn test_expand_no_variables() {
        let result = TemplateEngine::expand("hello world", None, &HashMap::new());
        assert_eq!(result, "hello world");
    }

    #[test]
    fn test_expand_captures() {
        let captures = vec!["foo".to_string(), "bar".to_string()];
        let result = TemplateEngine::expand("$1-$2", Some(&captures), &HashMap::new());
        assert_eq!(result, "foo-bar");
    }

    #[test]
    fn test_expand_named_vars() {
        let mut values = HashMap::new();
        values.insert("name".to_string(), "world".to_string());
        let result = TemplateEngine::expand("hello ${name}", None, &values);
        assert_eq!(result, "hello world");
    }

    #[test]
    fn test_expand_inline_file() {
        let mut values = HashMap::new();
        values.insert("data.json".to_string(), r#"{"key":"value"}"#.to_string());
        let result = TemplateEngine::expand("{data.json}", None, &values);
        assert_eq!(result, r#"{"key":"value"}"#);
    }

    #[test]
    fn test_builtin_now() {
        let ctx = ctx_with_url("http://example.com/test");
        let result = TemplateEngine::expand_with_context("${now}", &ctx, None, &HashMap::new());
        let parsed: Result<i64, _> = result.parse();
        assert!(parsed.is_ok());
    }

    #[test]
    fn test_builtin_random() {
        let ctx = ctx_with_url("http://example.com/test");
        let result = TemplateEngine::expand_with_context("${random}", &ctx, None, &HashMap::new());
        let parsed: Result<f64, _> = result.parse();
        assert!(parsed.is_ok());
    }

    #[test]
    fn test_builtin_random_uuid() {
        let ctx = ctx_with_url("http://example.com/test");
        let result =
            TemplateEngine::expand_with_context("${randomUUID}", &ctx, None, &HashMap::new());
        assert!(uuid::Uuid::parse_str(&result).is_ok());
    }

    #[test]
    fn test_builtin_url() {
        let ctx = RequestContext::builder()
            .url("http://example.com/api/test?foo=bar")
            .host("example.com")
            .hostname("example.com")
            .path("/api/test?foo=bar")
            .pathname("/api/test")
            .query("foo=bar")
            .search("?foo=bar")
            .build();
        let result = TemplateEngine::expand_with_context("${url}", &ctx, None, &HashMap::new());
        assert_eq!(result, "http://example.com/api/test?foo=bar");
    }

    #[test]
    fn test_builtin_host() {
        let ctx = RequestContext::builder()
            .url("http://www.example.com/test")
            .host("www.example.com")
            .hostname("www.example.com")
            .path("/test")
            .pathname("/test")
            .build();
        let result = TemplateEngine::expand_with_context("${host}", &ctx, None, &HashMap::new());
        assert_eq!(result, "www.example.com");
    }

    #[test]
    fn test_builtin_hostname() {
        let ctx = RequestContext::builder()
            .url("http://api.example.com/test")
            .host("api.example.com:8080")
            .hostname("api.example.com")
            .path("/test")
            .pathname("/test")
            .port(8080)
            .build();
        let result =
            TemplateEngine::expand_with_context("${hostname}", &ctx, None, &HashMap::new());
        assert_eq!(result, "api.example.com");
    }

    #[test]
    fn test_builtin_port() {
        let ctx = RequestContext::builder()
            .url("http://example.com:8080/test")
            .host("example.com:8080")
            .hostname("example.com")
            .path("/test")
            .pathname("/test")
            .port(8080)
            .build();
        let result = TemplateEngine::expand_with_context("${port}", &ctx, None, &HashMap::new());
        assert_eq!(result, "8080");
    }

    #[test]
    fn test_builtin_path() {
        let ctx = RequestContext::builder()
            .url("http://example.com/api/v1/users")
            .host("example.com")
            .hostname("example.com")
            .path("/api/v1/users")
            .pathname("/api/v1/users")
            .build();
        let result = TemplateEngine::expand_with_context("${path}", &ctx, None, &HashMap::new());
        assert_eq!(result, "/api/v1/users");
    }

    #[test]
    fn test_builtin_pathname() {
        let ctx = RequestContext::builder()
            .url("http://example.com/api?foo=bar")
            .host("example.com")
            .hostname("example.com")
            .path("/api?foo=bar")
            .pathname("/api")
            .query("foo=bar")
            .build();
        let result =
            TemplateEngine::expand_with_context("${pathname}", &ctx, None, &HashMap::new());
        assert_eq!(result, "/api");
    }

    #[test]
    fn test_builtin_query_with_key() {
        let ctx = RequestContext::builder()
            .url("http://example.com/api?name=test&value=123")
            .host("example.com")
            .hostname("example.com")
            .path("/api?name=test&value=123")
            .pathname("/api")
            .query("name=test&value=123")
            .build();
        let result =
            TemplateEngine::expand_with_context("${query.name}", &ctx, None, &HashMap::new());
        assert_eq!(result, "test");
    }

    #[test]
    fn test_builtin_method() {
        let ctx = RequestContext::builder()
            .url("http://example.com/api")
            .host("example.com")
            .hostname("example.com")
            .path("/api")
            .pathname("/api")
            .method("POST")
            .build();
        let result = TemplateEngine::expand_with_context("${method}", &ctx, None, &HashMap::new());
        assert_eq!(result, "POST");
    }

    #[test]
    fn test_builtin_client_ip() {
        let ctx = RequestContext::builder()
            .url("http://example.com/api")
            .host("example.com")
            .hostname("example.com")
            .path("/api")
            .pathname("/api")
            .client_ip("192.168.1.100")
            .build();
        let result =
            TemplateEngine::expand_with_context("${clientIp}", &ctx, None, &HashMap::new());
        assert_eq!(result, "192.168.1.100");
    }

    #[test]
    fn test_builtin_req_headers() {
        let mut headers = HashMap::new();
        headers.insert("content-type".to_string(), "application/json".to_string());
        headers.insert("authorization".to_string(), "Bearer token".to_string());

        let ctx = RequestContext::builder()
            .url("http://example.com/api")
            .host("example.com")
            .hostname("example.com")
            .path("/api")
            .pathname("/api")
            .req_headers(headers)
            .build();

        let result = TemplateEngine::expand_with_context(
            "${reqHeaders.content-type}",
            &ctx,
            None,
            &HashMap::new(),
        );
        assert_eq!(result, "application/json");

        let result = TemplateEngine::expand_with_context(
            "${reqH.authorization}",
            &ctx,
            None,
            &HashMap::new(),
        );
        assert_eq!(result, "Bearer token");
    }

    #[test]
    fn test_builtin_req_cookies() {
        let mut cookies = HashMap::new();
        cookies.insert("session".to_string(), "abc123".to_string());
        cookies.insert("user".to_string(), "test".to_string());

        let ctx = RequestContext::builder()
            .url("http://example.com/api")
            .host("example.com")
            .hostname("example.com")
            .path("/api")
            .pathname("/api")
            .req_cookies(cookies)
            .build();

        let result = TemplateEngine::expand_with_context(
            "${reqCookies.session}",
            &ctx,
            None,
            &HashMap::new(),
        );
        assert_eq!(result, "abc123");
    }

    #[test]
    fn test_builtin_env() {
        std::env::set_var("TEST_VAR_FOR_TEMPLATE", "test_value");
        let ctx = ctx_with_url("http://example.com/test");
        let result = TemplateEngine::expand_with_context(
            "${env.TEST_VAR_FOR_TEMPLATE}",
            &ctx,
            None,
            &HashMap::new(),
        );
        assert_eq!(result, "test_value");
        std::env::remove_var("TEST_VAR_FOR_TEMPLATE");
    }

    #[test]
    fn test_url_encode_syntax() {
        let ctx = RequestContext::builder()
            .url("http://example.com/test")
            .host("example.com")
            .hostname("example.com")
            .path("/test")
            .pathname("/test")
            .method("hello world")
            .build();
        let result =
            TemplateEngine::expand_with_context("${{method}}", &ctx, None, &HashMap::new());
        assert_eq!(result, "hello%20world");
    }

    #[test]
    fn test_escape_syntax() {
        let ctx = ctx_with_url("http://example.com/test");
        let result = TemplateEngine::expand_with_context("$${host}", &ctx, None, &HashMap::new());
        assert_eq!(result, "${host}");
    }

    #[test]
    fn test_replace_simple() {
        let ctx = RequestContext::builder()
            .url("http://example.com/test")
            .host("example.com")
            .hostname("example.com")
            .path("/test")
            .pathname("/test")
            .build();
        let result = TemplateEngine::expand_with_context(
            "${hostname.replace(example,test)}",
            &ctx,
            None,
            &HashMap::new(),
        );
        assert_eq!(result, "test.com");
    }

    #[test]
    fn test_replace_regex() {
        let ctx = RequestContext::builder()
            .url("http://example.com/test")
            .host("example.com")
            .hostname("example.com")
            .path("/test")
            .pathname("/test")
            .build();
        let result = TemplateEngine::expand_with_context(
            "${hostname.replace(/\\./,-)}",
            &ctx,
            None,
            &HashMap::new(),
        );
        assert_eq!(result, "example-com");
    }

    #[test]
    fn test_replace_regex_global() {
        let ctx = RequestContext::builder()
            .url("http://example.com/test")
            .host("a.b.c.d")
            .hostname("a.b.c.d")
            .path("/test")
            .pathname("/test")
            .build();
        let result = TemplateEngine::expand_with_context(
            "${hostname.replace(/\\./g,-)}",
            &ctx,
            None,
            &HashMap::new(),
        );
        assert_eq!(result, "a-b-c-d");
    }

    #[test]
    fn test_version() {
        let ctx = ctx_with_url("http://example.com/test");
        let result = TemplateEngine::expand_with_context("${version}", &ctx, None, &HashMap::new());
        assert!(!result.is_empty());
    }

    #[test]
    fn test_status_code() {
        let ctx = RequestContext::builder()
            .url("http://example.com/test")
            .host("example.com")
            .hostname("example.com")
            .path("/test")
            .pathname("/test")
            .status_code(200)
            .build();
        let result =
            TemplateEngine::expand_with_context("${statusCode}", &ctx, None, &HashMap::new());
        assert_eq!(result, "200");
    }

    #[test]
    fn test_response_vars_passthrough_when_absent() {
        // Request phase: no response data yet. Response variables must survive as
        // literal tokens so the proxy can expand them once the response arrives,
        // rather than collapsing to empty strings.
        let ctx = RequestContext::builder()
            .url("http://example.com/test")
            .build();
        let result = TemplateEngine::expand_with_context(
            "s=${statusCode} st=${status} h=${resHeaders.x-up}",
            &ctx,
            None,
            &HashMap::new(),
        );
        assert_eq!(result, "s=${statusCode} st=${status} h=${resHeaders.x-up}");
    }

    #[test]
    fn test_response_vars_expand_when_present() {
        // Response phase: status/headers populated -> tokens expand for real.
        let mut headers = HashMap::new();
        headers.insert("x-up".to_string(), "mock".to_string());
        let mut ctx = RequestContext::builder()
            .url("http://example.com/test")
            .build();
        ctx.set_response(503, headers);

        let result = TemplateEngine::expand_with_context(
            "s=${statusCode} st=${status} h=${resHeaders.x-up}",
            &ctx,
            None,
            &HashMap::new(),
        );
        assert_eq!(result, "s=503 st=503 h=mock");
    }

    #[test]
    fn test_combined_variables() {
        let mut values = HashMap::new();
        values.insert("target".to_string(), "127.0.0.1".to_string());

        let ctx = RequestContext::builder()
            .url("http://example.com:8080/api")
            .host("example.com:8080")
            .hostname("example.com")
            .port(8080)
            .path("/api")
            .pathname("/api")
            .method("GET")
            .build();

        let result = TemplateEngine::expand_with_context(
            "${target}:${port} ${method} ${pathname}",
            &ctx,
            None,
            &values,
        );
        assert_eq!(result, "127.0.0.1:8080 GET /api");
    }

    #[test]
    fn test_expand_value_refs() {
        let mut values = HashMap::new();
        values.insert("mockResponse".to_string(), "hello world".to_string());
        values.insert("authHeaders".to_string(), "X-Test: value".to_string());

        let result = TemplateEngine::expand("{mockResponse}", None, &values);
        assert_eq!(result, "hello world");

        let result2 = TemplateEngine::expand("{authHeaders}", None, &values);
        assert_eq!(result2, "X-Test: value");

        let result3 = TemplateEngine::expand("{unknownValue}", None, &values);
        assert_eq!(result3, "{unknownValue}");
    }

    #[test]
    fn test_capture_out_of_range_left_intact() {
        // $3 with only two captures → left as-is; $0 → left as-is
        let captures = vec!["a".to_string(), "b".to_string()];
        let result = TemplateEngine::expand("$1-$3-$0", Some(&captures), &HashMap::new());
        assert_eq!(result, "a-$3-$0");
    }

    #[test]
    fn test_url_encode_mismatched_markers_pass_through() {
        // Opening url-encode brace without closing one → original text returned.
        let ctx = ctx_with_url("http://example.com/test");
        let result = TemplateEngine::expand_with_context("${{host}", &ctx, None, &HashMap::new());
        assert_eq!(result, "${{host}");
    }

    #[test]
    fn test_url_encode_end_without_start_appends_brace() {
        // Closing marker present but not opening: value followed by literal '}'.
        let ctx = RequestContext::builder()
            .url("http://example.com/test")
            .host("example.com")
            .hostname("example.com")
            .path("/test")
            .pathname("/test")
            .method("GET")
            .build();
        let result = TemplateEngine::expand_with_context("${method}}", &ctx, None, &HashMap::new());
        assert_eq!(result, "GET}");
    }

    #[test]
    fn test_querystring_defaults_to_question_mark() {
        let ctx = ctx_with_url("http://example.com/test");
        let result =
            TemplateEngine::expand_with_context("${queryString}", &ctx, None, &HashMap::new());
        assert_eq!(result, "?");
        let result =
            TemplateEngine::expand_with_context("${searchString}", &ctx, None, &HashMap::new());
        assert_eq!(result, "?");
    }

    #[test]
    fn test_search_present_and_default_empty() {
        let ctx = RequestContext::builder()
            .url("http://example.com/api?x=1")
            .host("example.com")
            .hostname("example.com")
            .path("/api?x=1")
            .pathname("/api")
            .search("?x=1")
            .build();
        let result = TemplateEngine::expand_with_context("${search}", &ctx, None, &HashMap::new());
        assert_eq!(result, "?x=1");

        let empty = ctx_with_url("http://example.com/test");
        let result =
            TemplateEngine::expand_with_context("${search}", &empty, None, &HashMap::new());
        assert_eq!(result, "");
    }

    #[test]
    fn test_random_int_single_and_range() {
        let ctx = ctx_with_url("http://example.com/test");
        // single bound: result in [0, 5]
        let r = TemplateEngine::expand_with_context("${randomInt(5)}", &ctx, None, &HashMap::new());
        let n: i64 = r.parse().unwrap();
        assert!((0..=5).contains(&n), "got {n}");

        // range bound: result in [10, 20]
        let r =
            TemplateEngine::expand_with_context("${randomInt(10-20)}", &ctx, None, &HashMap::new());
        let n: i64 = r.parse().unwrap();
        assert!((10..=20).contains(&n), "got {n}");

        // reversed range still normalized: [3, 8]
        let r =
            TemplateEngine::expand_with_context("${randomInt(8-3)}", &ctx, None, &HashMap::new());
        let n: i64 = r.parse().unwrap();
        assert!((3..=8).contains(&n), "got {n}");
    }

    #[test]
    fn test_real_url_host_port() {
        let ctx = RequestContext::builder()
            .url("http://example.com/test")
            .host("example.com")
            .hostname("example.com")
            .path("/test")
            .pathname("/test")
            .real_url("http://backend.internal/real")
            .real_host("backend.internal")
            .real_port(9000)
            .build();
        assert_eq!(
            TemplateEngine::expand_with_context("${realUrl}", &ctx, None, &HashMap::new()),
            "http://backend.internal/real"
        );
        assert_eq!(
            TemplateEngine::expand_with_context("${realHost}", &ctx, None, &HashMap::new()),
            "backend.internal"
        );
        assert_eq!(
            TemplateEngine::expand_with_context("${realPort}", &ctx, None, &HashMap::new()),
            "9000"
        );
    }

    #[test]
    fn test_remote_and_client_port_and_address() {
        let mut ctx = RequestContext::builder()
            .url("http://example.com/test")
            .host("example.com")
            .hostname("example.com")
            .path("/test")
            .pathname("/test")
            .client_ip("10.0.0.1")
            .client_port(54321)
            .build();
        // client_ip/client_port set remote_* to the same values; override remote
        // fields directly to exercise their distinct resolution paths.
        ctx.remote_address = "10.0.0.2".to_string();
        ctx.remote_port = 443;
        assert_eq!(
            TemplateEngine::expand_with_context("${clientPort}", &ctx, None, &HashMap::new()),
            "54321"
        );
        assert_eq!(
            TemplateEngine::expand_with_context("${remoteAddress}", &ctx, None, &HashMap::new()),
            "10.0.0.2"
        );
        assert_eq!(
            TemplateEngine::expand_with_context("${remotePort}", &ctx, None, &HashMap::new()),
            "443"
        );
    }

    #[test]
    fn test_server_ip_port_defaults() {
        let ctx = ctx_with_url("http://example.com/test");
        // serverIp defaults to 127.0.0.1 when unset
        assert_eq!(
            TemplateEngine::expand_with_context("${serverIp}", &ctx, None, &HashMap::new()),
            "127.0.0.1"
        );
        // serverPort defaults to empty when unset
        assert_eq!(
            TemplateEngine::expand_with_context("${serverPort}", &ctx, None, &HashMap::new()),
            ""
        );
    }

    #[test]
    fn test_client_id_and_local_client_id() {
        let ctx = RequestContext::builder()
            .url("http://example.com/test")
            .host("example.com")
            .hostname("example.com")
            .path("/test")
            .pathname("/test")
            .client_id("client-42")
            .local_client_id("local-7")
            .build();
        assert_eq!(
            TemplateEngine::expand_with_context("${clientId}", &ctx, None, &HashMap::new()),
            "client-42"
        );
        assert_eq!(
            TemplateEngine::expand_with_context("${localClientId}", &ctx, None, &HashMap::new()),
            "local-7"
        );
    }

    #[test]
    fn test_id_req_id() {
        let ctx = ctx_with_url("http://example.com/test");
        let id = TemplateEngine::expand_with_context("${id}", &ctx, None, &HashMap::new());
        let req_id = TemplateEngine::expand_with_context("${reqId}", &ctx, None, &HashMap::new());
        assert_eq!(id, req_id);
    }

    #[test]
    fn test_req_headers_no_key_joins_all() {
        let mut headers = HashMap::new();
        headers.insert("x-test".to_string(), "v1".to_string());
        let ctx = RequestContext::builder()
            .url("http://example.com/test")
            .host("example.com")
            .hostname("example.com")
            .path("/test")
            .pathname("/test")
            .req_headers(headers)
            .build();
        let result =
            TemplateEngine::expand_with_context("${reqHeaders}", &ctx, None, &HashMap::new());
        assert_eq!(result, "x-test: v1");
    }

    #[test]
    fn test_req_cookies_no_key_joins_all() {
        let mut cookies = HashMap::new();
        cookies.insert("session".to_string(), "abc".to_string());
        let ctx = RequestContext::builder()
            .url("http://example.com/test")
            .host("example.com")
            .hostname("example.com")
            .path("/test")
            .pathname("/test")
            .req_cookies(cookies)
            .build();
        let result =
            TemplateEngine::expand_with_context("${reqCookies}", &ctx, None, &HashMap::new());
        assert_eq!(result, "session=abc");
    }

    #[test]
    fn test_res_headers_with_and_without_key() {
        let mut headers = HashMap::new();
        headers.insert("content-type".to_string(), "text/html".to_string());
        let mut ctx = RequestContext::builder()
            .url("http://example.com/test")
            .host("example.com")
            .hostname("example.com")
            .path("/test")
            .pathname("/test")
            .build();
        ctx.res_headers = Some(headers);
        // keyed (lowercased lookup)
        let result = TemplateEngine::expand_with_context(
            "${resHeaders.Content-Type}",
            &ctx,
            None,
            &HashMap::new(),
        );
        assert_eq!(result, "text/html");
        // no key joins all
        let result =
            TemplateEngine::expand_with_context("${resHeaders}", &ctx, None, &HashMap::new());
        assert_eq!(result, "content-type: text/html");
    }

    #[test]
    fn test_res_cookies_with_and_without_key() {
        let mut cookies = HashMap::new();
        cookies.insert("token".to_string(), "xyz".to_string());
        let mut ctx = RequestContext::builder()
            .url("http://example.com/test")
            .host("example.com")
            .hostname("example.com")
            .path("/test")
            .pathname("/test")
            .build();
        ctx.res_cookies = Some(cookies);
        let result =
            TemplateEngine::expand_with_context("${resCookies.token}", &ctx, None, &HashMap::new());
        assert_eq!(result, "xyz");
        let result =
            TemplateEngine::expand_with_context("${resCookies}", &ctx, None, &HashMap::new());
        assert_eq!(result, "token=xyz");
    }

    #[test]
    fn test_res_headers_deferred_and_res_cookies_unset_are_empty() {
        let ctx = ctx_with_url("http://example.com/test");
        // Response-phase header tokens are deferred at request phase (res_headers
        // is None) and must be left literal so the proxy can expand them once the
        // upstream response is available.
        assert_eq!(
            TemplateEngine::expand_with_context("${resHeaders}", &ctx, None, &HashMap::new()),
            "${resHeaders}"
        );
        assert_eq!(
            TemplateEngine::expand_with_context("${resHeaders.x}", &ctx, None, &HashMap::new()),
            "${resHeaders.x}"
        );
        // resCookies is not a deferred response var, so an unset value resolves to
        // the empty string.
        assert_eq!(
            TemplateEngine::expand_with_context("${resCookies}", &ctx, None, &HashMap::new()),
            ""
        );
        assert_eq!(
            TemplateEngine::expand_with_context("${resCookies.x}", &ctx, None, &HashMap::new()),
            ""
        );
    }

    #[test]
    fn test_env_no_key_and_alias_fallback() {
        let ctx = ctx_with_url("http://example.com/test");
        // ${env} with no key → empty string
        assert_eq!(
            TemplateEngine::expand_with_context("${env}", &ctx, None, &HashMap::new()),
            ""
        );

        // Alias fallback: set USER, read it directly (primary path); and exercise
        // the fallback chain by querying USERNAME which maps back to USER.
        std::env::set_var("USER", "tester");
        std::env::remove_var("USERNAME");
        assert_eq!(
            TemplateEngine::expand_with_context("${env.USER}", &ctx, None, &HashMap::new()),
            "tester"
        );
        assert_eq!(
            TemplateEngine::expand_with_context("${env.USERNAME}", &ctx, None, &HashMap::new()),
            "tester"
        );
        // Unknown var with no alias → empty
        std::env::remove_var("DEFINITELY_UNSET_VAR_XYZ");
        assert_eq!(
            TemplateEngine::expand_with_context(
                "${env.DEFINITELY_UNSET_VAR_XYZ}",
                &ctx,
                None,
                &HashMap::new()
            ),
            ""
        );
    }

    #[test]
    fn test_env_home_userprofile_alias() {
        let ctx = ctx_with_url("http://example.com/test");
        std::env::set_var("HOME", "/home/tester");
        std::env::remove_var("USERPROFILE");
        // USERPROFILE not set → falls back to HOME
        assert_eq!(
            TemplateEngine::expand_with_context("${env.USERPROFILE}", &ctx, None, &HashMap::new()),
            "/home/tester"
        );
    }

    #[test]
    fn test_url_with_key_lookup_and_fallback() {
        let mut values = HashMap::new();
        values.insert("alt".to_string(), "http://alt.example/x".to_string());
        let ctx = ctx_with_url("http://example.com/test");
        // key present in values
        assert_eq!(
            TemplateEngine::expand_with_context("${url.alt}", &ctx, None, &values),
            "http://alt.example/x"
        );
        // key absent → falls back to ctx.url
        assert_eq!(
            TemplateEngine::expand_with_context("${url.missing}", &ctx, None, &values),
            "http://example.com/test"
        );
    }

    #[test]
    fn test_replace_unmatched_pattern_returns_value() {
        let ctx = RequestContext::builder()
            .url("http://example.com/test")
            .host("example.com")
            .hostname("example.com")
            .path("/test")
            .pathname("/test")
            .build();
        // pattern not found → plain replace is a no-op, value unchanged
        let result = TemplateEngine::expand_with_context(
            "${hostname.replace(zzz,qqq)}",
            &ctx,
            None,
            &HashMap::new(),
        );
        assert_eq!(result, "example.com");
    }

    #[test]
    fn test_inline_file_unknown_left_intact() {
        let result = TemplateEngine::expand("{missing.json}", None, &HashMap::new());
        assert_eq!(result, "{missing.json}");
    }
}
