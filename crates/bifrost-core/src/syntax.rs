use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::protocol::{protocol_aliases, Protocol, ProtocolCategory};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValueHint {
    pub prefix: String,
    pub description: String,
    pub example: String,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub completions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProtocolValueSpec {
    pub protocol: String,
    pub value_format: String,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub hints: Vec<ValueHint>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub validation_regex: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProtocolInfo {
    pub name: String,
    pub category: String,
    pub description: String,
    pub value_type: String,
    pub example: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alias_of: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub aliases: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemplateVariableInfo {
    pub name: String,
    pub description: String,
    pub example: String,
    pub category: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PatternInfo {
    pub name: String,
    pub description: String,
    pub example: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyntaxInfo {
    pub protocols: Vec<ProtocolInfo>,
    pub template_variables: Vec<TemplateVariableInfo>,
    pub patterns: Vec<PatternInfo>,
    pub protocol_aliases: HashMap<String, String>,
}

fn category_to_string(category: ProtocolCategory) -> &'static str {
    match category {
        ProtocolCategory::Request => "request",
        ProtocolCategory::Response => "response",
        ProtocolCategory::Both => "both",
        ProtocolCategory::Control => "control",
    }
}

fn get_protocol_description(protocol: Protocol) -> &'static str {
    match protocol {
        Protocol::Host => "Forward request to specified host",
        Protocol::XHost => "Extended host forwarding with path rewrite",
        Protocol::Http => "HTTP protocol forwarding",
        Protocol::Https => "HTTPS protocol forwarding",
        Protocol::Ws => "WebSocket forwarding",
        Protocol::Wss => "Secure WebSocket forwarding",
        Protocol::Proxy => "HTTP proxy forwarding",
        Protocol::Http3 => "Enable upstream HTTP/3 attempt for matched requests",
        Protocol::Pac => "PAC script routing",
        Protocol::Redirect => "URL redirect (301/302)",
        Protocol::File => "Return file content as response",
        Protocol::Tpl => "Template response with variable substitution",
        Protocol::RawFile => "Return raw file content",
        Protocol::Delete => "Delete/block the request",
        Protocol::Skip => "Skip matched rules by pattern or operation and continue matching",
        Protocol::ReqHeaders => "Modify request headers",
        Protocol::ReqBody => "Set request body",
        Protocol::ReqPrepend => "Prepend content to request body",
        Protocol::ReqAppend => "Append content to request body",
        Protocol::ReqCookies => "Set request cookies",
        Protocol::ReqCors => "Add CORS headers to request",
        Protocol::ReqDelay => "Delay request (milliseconds)",
        Protocol::ReqSpeed => "Limit request speed (kb/s)",
        Protocol::ReqType => "Set request Content-Type",
        Protocol::ReqCharset => "Set request charset",
        Protocol::ReqReplace => "Replace content in request body",
        Protocol::ForwardedFor => "Set X-Forwarded-For header",
        Protocol::Method => "Change request method",
        Protocol::Auth => "Set Authorization header",
        Protocol::Ua => "Set User-Agent header",
        Protocol::Referer => "Set Referer header",
        Protocol::UrlParams => "Add/modify URL parameters",
        Protocol::Params => "Merge parameters",
        Protocol::ResHeaders => "Modify response headers",
        Protocol::ResBody => "Set response body",
        Protocol::ResPrepend => "Prepend content to response body",
        Protocol::ResAppend => "Append content to response body",
        Protocol::ResCookies => "Set response cookies",
        Protocol::ResCors => "Add CORS headers to response",
        Protocol::ResDelay => "Delay response (milliseconds)",
        Protocol::ResSpeed => "Limit response speed (kb/s)",
        Protocol::ResType => "Set response Content-Type",
        Protocol::ResCharset => "Set response charset",
        Protocol::ResReplace => "Replace content in response body",
        Protocol::ReplaceStatus => "Replace status code after request",
        Protocol::StatusCode => "Return status code directly",
        Protocol::Cache => "Set cache control (seconds)",
        Protocol::Attachment => "Set Content-Disposition for download",
        Protocol::ResponseFor => "Set x-bifrost-response-for response header",
        Protocol::Trailers => "Set response trailers",
        Protocol::ResMerge => "Merge JSON into response",
        Protocol::HeaderReplace => "Replace header content",
        Protocol::HtmlAppend => "Append content to HTML",
        Protocol::HtmlPrepend => "Prepend content to HTML",
        Protocol::HtmlBody => "Replace HTML body",
        Protocol::JsAppend => "Append content to JavaScript",
        Protocol::JsPrepend => "Prepend content to JavaScript",
        Protocol::JsBody => "Replace JavaScript body",
        Protocol::CssAppend => "Append content to CSS",
        Protocol::CssPrepend => "Prepend content to CSS",
        Protocol::CssBody => "Replace CSS body",
        Protocol::UrlReplace => "Replace URL path",
        Protocol::ReqScript => "Execute request script",
        Protocol::ResScript => "Execute response script",
        Protocol::Decode => "Execute decode script (for request/response decode)",
        Protocol::Dns => "Custom DNS resolution",
        Protocol::TlsIntercept => "Enable TLS interception",
        Protocol::TlsPassthrough => "Disable TLS interception",
        Protocol::TlsOptions => "Configure CONNECT upstream TLS options",
        Protocol::SniCallback => "Configure SNI callback metadata for CONNECT requests",
        Protocol::Passthrough => "Pass through without modification",
        Protocol::Tunnel => "Redirect CONNECT tunnel target",
    }
}

fn get_protocol_value_type(protocol: Protocol) -> &'static str {
    match protocol {
        Protocol::Host | Protocol::XHost | Protocol::Proxy => "host:port",
        Protocol::Pac => "pac_url_or_script",
        Protocol::Http | Protocol::Https | Protocol::Ws | Protocol::Wss | Protocol::Redirect => {
            "url"
        }
        Protocol::File | Protocol::Tpl | Protocol::RawFile => "file_path",
        Protocol::ReqScript | Protocol::ResScript | Protocol::Decode => "script_name",
        Protocol::ReqHeaders
        | Protocol::ResHeaders
        | Protocol::ReqCookies
        | Protocol::ResCookies
        | Protocol::Trailers => "headers",
        Protocol::ReqBody | Protocol::ResBody | Protocol::ResMerge => "body_content",
        Protocol::ReqDelay | Protocol::ResDelay => "milliseconds",
        Protocol::ReqSpeed | Protocol::ResSpeed => "kb_per_second",
        Protocol::StatusCode | Protocol::ReplaceStatus => "status_code",
        Protocol::Cache => "seconds",
        Protocol::UrlParams | Protocol::Params => "key=value",
        Protocol::ForwardedFor | Protocol::ResponseFor => "string",
        Protocol::ReqReplace
        | Protocol::ResReplace
        | Protocol::UrlReplace
        | Protocol::HeaderReplace => "old/new/",
        Protocol::Method => "method_name",
        Protocol::Auth => "user:password",
        Protocol::Ua | Protocol::Referer => "string",
        Protocol::ReqType | Protocol::ResType => "content_type",
        Protocol::ReqCharset | Protocol::ResCharset => "charset",
        Protocol::Attachment => "filename",
        Protocol::Dns => "dns_server",
        Protocol::TlsOptions => "tls_options",
        Protocol::SniCallback => "callback_spec",
        Protocol::Delete
        | Protocol::Skip
        | Protocol::ReqCors
        | Protocol::ResCors
        | Protocol::Http3
        | Protocol::TlsIntercept
        | Protocol::TlsPassthrough
        | Protocol::Passthrough => "empty",
        Protocol::Tunnel => "host:port",
        Protocol::HtmlAppend
        | Protocol::HtmlPrepend
        | Protocol::HtmlBody
        | Protocol::JsAppend
        | Protocol::JsPrepend
        | Protocol::JsBody
        | Protocol::CssAppend
        | Protocol::CssPrepend
        | Protocol::CssBody => "content",
        Protocol::ReqPrepend | Protocol::ReqAppend | Protocol::ResPrepend | Protocol::ResAppend => {
            "content"
        }
    }
}

fn get_protocol_example(protocol: Protocol) -> &'static str {
    match protocol {
        Protocol::Host => "host://127.0.0.1:8080",
        Protocol::XHost => "xhost://127.0.0.1:8080/api",
        Protocol::Http => "http://localhost:3000/",
        Protocol::Https => "https://api.example.com/",
        Protocol::Ws => "ws://localhost:8080/socket",
        Protocol::Wss => "wss://api.example.com/socket",
        Protocol::Proxy => "proxy://127.0.0.1:8888",
        Protocol::Http3 => "http3://",
        Protocol::Pac => "pac://http://127.0.0.1:8000/proxy.pac",
        Protocol::Redirect => "redirect://https://new-site.com/",
        Protocol::File => "file:///path/to/response.json",
        Protocol::Tpl => "tpl:///path/to/template.tpl",
        Protocol::RawFile => "rawfile:///path/to/raw.txt",
        Protocol::Delete => "delete://",
        Protocol::Skip => "skip://pattern=example.com/api",
        Protocol::ReqHeaders => "reqHeaders://(X-Custom: value)",
        Protocol::ReqBody => "reqBody://{\"key\": \"value\"}",
        Protocol::ReqPrepend => "reqPrepend://prefix-",
        Protocol::ReqAppend => "reqAppend://-suffix",
        Protocol::ReqCookies => "reqCookies://(session=abc123)",
        Protocol::ReqCors => "reqCors://",
        Protocol::ReqDelay => "reqDelay://1000",
        Protocol::ReqSpeed => "reqSpeed://1024",
        Protocol::ReqType => "reqType://application/json",
        Protocol::ReqCharset => "reqCharset://utf-8",
        Protocol::ReqReplace => "reqReplace://old/new/",
        Protocol::ForwardedFor => "forwardedFor://1.2.3.4",
        Protocol::Method => "method://POST",
        Protocol::Auth => "auth://user:password",
        Protocol::Ua => "ua://CustomAgent/1.0",
        Protocol::Referer => "referer://https://origin.com/",
        Protocol::UrlParams => "urlParams://(key=value)",
        Protocol::Params => "params://(param1=val1)",
        Protocol::ResHeaders => "resHeaders://(X-Response: value)",
        Protocol::ResBody => "resBody://{\"response\": \"data\"}",
        Protocol::ResPrepend => "resPrepend://prefix-",
        Protocol::ResAppend => "resAppend://-suffix",
        Protocol::ResCookies => "resCookies://(token=xyz789)",
        Protocol::ResCors => "resCors://",
        Protocol::ResDelay => "resDelay://500",
        Protocol::ResSpeed => "resSpeed://512",
        Protocol::ResType => "resType://text/html",
        Protocol::ResCharset => "resCharset://gbk",
        Protocol::ResReplace => "resReplace://old/new/",
        Protocol::ReplaceStatus => "replaceStatus://200",
        Protocol::StatusCode => "statusCode://404",
        Protocol::Cache => "cache://3600",
        Protocol::Attachment => "attachment://filename.zip",
        Protocol::ResponseFor => "responseFor://1.1.1.1",
        Protocol::Trailers => "trailers://(X-Checksum: abc123)",
        Protocol::ResMerge => "resMerge://{\"extra\": \"data\"}",
        Protocol::HeaderReplace => "headerReplace://OldHeader/NewHeader/",
        Protocol::HtmlAppend => "htmlAppend://<script>console.log('hi')</script>",
        Protocol::HtmlPrepend => "htmlPrepend://<!-- comment -->",
        Protocol::HtmlBody => "htmlBody://<html><body>content</body></html>",
        Protocol::JsAppend => "jsAppend://console.log('appended');",
        Protocol::JsPrepend => "jsPrepend:///* prepended */",
        Protocol::JsBody => "jsBody://function replaced() {}",
        Protocol::CssAppend => "cssAppend://body { color: red; }",
        Protocol::CssPrepend => "cssPrepend:///* prepended */",
        Protocol::CssBody => "cssBody://* { margin: 0; }",
        Protocol::UrlReplace => "urlReplace://old-path/new-path/",
        Protocol::ReqScript => "reqScript:///path/to/script.js",
        Protocol::ResScript => "resScript:///path/to/script.js",
        Protocol::Decode => "decode://my-decode-script",
        Protocol::Dns => "dns://8.8.8.8",
        Protocol::TlsIntercept => "tlsIntercept://",
        Protocol::TlsPassthrough => "tlsPassthrough://",
        Protocol::TlsOptions => "tlsOptions://minVersion=TLSv1.2&maxVersion=TLSv1.3",
        Protocol::SniCallback => "sniCallback://plugin(custom-sni)",
        Protocol::Passthrough => "passthrough://",
        Protocol::Tunnel => "tunnel://127.0.0.1:443",
    }
}

pub fn get_all_protocols() -> Vec<ProtocolInfo> {
    let aliases = protocol_aliases();
    let mut alias_map: HashMap<String, Vec<String>> = HashMap::new();

    for (alias, target) in &aliases {
        alias_map
            .entry(target.to_string())
            .or_default()
            .push(alias.to_string());
    }

    let protocols = vec![
        Protocol::Host,
        Protocol::XHost,
        Protocol::Http,
        Protocol::Https,
        Protocol::Ws,
        Protocol::Wss,
        Protocol::Proxy,
        Protocol::Redirect,
        Protocol::File,
        Protocol::Tpl,
        Protocol::RawFile,
        Protocol::Delete,
        Protocol::Skip,
        Protocol::ReqHeaders,
        Protocol::ReqBody,
        Protocol::ReqPrepend,
        Protocol::ReqAppend,
        Protocol::ReqCookies,
        Protocol::ReqCors,
        Protocol::ReqDelay,
        Protocol::ReqSpeed,
        Protocol::ReqType,
        Protocol::ReqCharset,
        Protocol::ReqReplace,
        Protocol::Method,
        Protocol::Auth,
        Protocol::Ua,
        Protocol::Referer,
        Protocol::UrlParams,
        Protocol::Params,
        Protocol::ResHeaders,
        Protocol::ResBody,
        Protocol::ResPrepend,
        Protocol::ResAppend,
        Protocol::ResCookies,
        Protocol::ResCors,
        Protocol::ResDelay,
        Protocol::ResSpeed,
        Protocol::ResType,
        Protocol::ResCharset,
        Protocol::ResReplace,
        Protocol::ReplaceStatus,
        Protocol::StatusCode,
        Protocol::Cache,
        Protocol::Attachment,
        Protocol::Trailers,
        Protocol::ResMerge,
        Protocol::HeaderReplace,
        Protocol::HtmlAppend,
        Protocol::HtmlPrepend,
        Protocol::HtmlBody,
        Protocol::JsAppend,
        Protocol::JsPrepend,
        Protocol::JsBody,
        Protocol::CssAppend,
        Protocol::CssPrepend,
        Protocol::CssBody,
        Protocol::UrlReplace,
        Protocol::ReqScript,
        Protocol::ResScript,
        Protocol::Dns,
        Protocol::TlsIntercept,
        Protocol::TlsPassthrough,
        Protocol::TlsOptions,
        Protocol::SniCallback,
        Protocol::Tunnel,
        Protocol::Passthrough,
    ];

    protocols
        .into_iter()
        .map(|p| {
            let name = p.to_str().to_string();
            ProtocolInfo {
                name: name.clone(),
                category: category_to_string(p.category()).to_string(),
                description: get_protocol_description(p).to_string(),
                value_type: get_protocol_value_type(p).to_string(),
                example: get_protocol_example(p).to_string(),
                alias_of: None,
                aliases: alias_map.remove(&name).unwrap_or_default(),
            }
        })
        .collect()
}

pub fn get_template_variables() -> Vec<TemplateVariableInfo> {
    vec![
        TemplateVariableInfo {
            name: "now".to_string(),
            description: "Current timestamp (Date.now())".to_string(),
            example: "1704067200000".to_string(),
            category: "time".to_string(),
        },
        TemplateVariableInfo {
            name: "random".to_string(),
            description: "Random number (Math.random())".to_string(),
            example: "0.8234567891".to_string(),
            category: "random".to_string(),
        },
        TemplateVariableInfo {
            name: "randomUUID".to_string(),
            description: "Random UUID (crypto.randomUUID())".to_string(),
            example: "a1b2c3d4-e5f6-7890-abcd-ef1234567890".to_string(),
            category: "random".to_string(),
        },
        TemplateVariableInfo {
            name: "randomInt(n)".to_string(),
            description: "Random integer from 0 to n".to_string(),
            example: "${randomInt(100)} -> 42".to_string(),
            category: "random".to_string(),
        },
        TemplateVariableInfo {
            name: "randomInt(n1-n2)".to_string(),
            description: "Random integer from n1 to n2".to_string(),
            example: "${randomInt(10-100)} -> 57".to_string(),
            category: "random".to_string(),
        },
        TemplateVariableInfo {
            name: "reqId".to_string(),
            description: "Unique request ID".to_string(),
            example: "1752301623294-339".to_string(),
            category: "request".to_string(),
        },
        TemplateVariableInfo {
            name: "url".to_string(),
            description: "Full request URL".to_string(),
            example: "http://example.com/api?a=1".to_string(),
            category: "url".to_string(),
        },
        TemplateVariableInfo {
            name: "url.protocol".to_string(),
            description: "URL protocol".to_string(),
            example: "https:".to_string(),
            category: "url".to_string(),
        },
        TemplateVariableInfo {
            name: "url.hostname".to_string(),
            description: "URL hostname".to_string(),
            example: "example.com".to_string(),
            category: "url".to_string(),
        },
        TemplateVariableInfo {
            name: "url.host".to_string(),
            description: "URL host (hostname:port)".to_string(),
            example: "example.com:8080".to_string(),
            category: "url".to_string(),
        },
        TemplateVariableInfo {
            name: "url.port".to_string(),
            description: "URL port".to_string(),
            example: "8080".to_string(),
            category: "url".to_string(),
        },
        TemplateVariableInfo {
            name: "url.path".to_string(),
            description: "URL path with query".to_string(),
            example: "/api/users?a=1".to_string(),
            category: "url".to_string(),
        },
        TemplateVariableInfo {
            name: "url.pathname".to_string(),
            description: "URL path without query".to_string(),
            example: "/api/users".to_string(),
            category: "url".to_string(),
        },
        TemplateVariableInfo {
            name: "url.search".to_string(),
            description: "URL query string".to_string(),
            example: "?a=1".to_string(),
            category: "url".to_string(),
        },
        TemplateVariableInfo {
            name: "query.xxx".to_string(),
            description: "Query parameter value".to_string(),
            example: "${query.id} -> 123".to_string(),
            category: "url".to_string(),
        },
        TemplateVariableInfo {
            name: "querystring".to_string(),
            description: "Query string with ?".to_string(),
            example: "?a=1&b=2".to_string(),
            category: "url".to_string(),
        },
        TemplateVariableInfo {
            name: "method".to_string(),
            description: "Request method".to_string(),
            example: "GET".to_string(),
            category: "request".to_string(),
        },
        TemplateVariableInfo {
            name: "reqHeaders.xxx".to_string(),
            description: "Request header value".to_string(),
            example: "${reqHeaders.content-type} -> application/json".to_string(),
            category: "request".to_string(),
        },
        TemplateVariableInfo {
            name: "resHeaders.xxx".to_string(),
            description: "Response header value".to_string(),
            example: "${resHeaders.content-type} -> text/html".to_string(),
            category: "response".to_string(),
        },
        TemplateVariableInfo {
            name: "reqCookies.xxx".to_string(),
            description: "Request cookie value".to_string(),
            example: "${reqCookies.session} -> abc123".to_string(),
            category: "request".to_string(),
        },
        TemplateVariableInfo {
            name: "resCookies.xxx".to_string(),
            description: "Response cookie value".to_string(),
            example: "${resCookies.token} -> xyz789".to_string(),
            category: "response".to_string(),
        },
        TemplateVariableInfo {
            name: "statusCode".to_string(),
            description: "Response status code".to_string(),
            example: "200".to_string(),
            category: "response".to_string(),
        },
        TemplateVariableInfo {
            name: "clientIp".to_string(),
            description: "Client IP address".to_string(),
            example: "192.168.1.1".to_string(),
            category: "connection".to_string(),
        },
        TemplateVariableInfo {
            name: "clientPort".to_string(),
            description: "Client port".to_string(),
            example: "60582".to_string(),
            category: "connection".to_string(),
        },
        TemplateVariableInfo {
            name: "serverIp".to_string(),
            description: "Server IP address".to_string(),
            example: "10.0.0.1".to_string(),
            category: "connection".to_string(),
        },
        TemplateVariableInfo {
            name: "serverPort".to_string(),
            description: "Server port".to_string(),
            example: "443".to_string(),
            category: "connection".to_string(),
        },
        TemplateVariableInfo {
            name: "version".to_string(),
            description: "Proxy version".to_string(),
            example: "1.0.0".to_string(),
            category: "system".to_string(),
        },
        TemplateVariableInfo {
            name: "port".to_string(),
            description: "Proxy port".to_string(),
            example: "9900".to_string(),
            category: "system".to_string(),
        },
        TemplateVariableInfo {
            name: "env.xxx".to_string(),
            description: "Environment variable".to_string(),
            example: "${env.NODE_ENV} -> production".to_string(),
            category: "system".to_string(),
        },
    ]
}

pub fn get_pattern_types() -> Vec<PatternInfo> {
    vec![
        PatternInfo {
            name: "exact".to_string(),
            description: "Exact domain match".to_string(),
            example: "www.example.com".to_string(),
        },
        PatternInfo {
            name: "path".to_string(),
            description: "Domain with path prefix".to_string(),
            example: "www.example.com/api".to_string(),
        },
        PatternInfo {
            name: "port".to_string(),
            description: "Domain with port".to_string(),
            example: "www.example.com:8080".to_string(),
        },
        PatternInfo {
            name: "single_wildcard".to_string(),
            description: "Single level subdomain wildcard".to_string(),
            example: "*.example.com".to_string(),
        },
        PatternInfo {
            name: "multi_wildcard".to_string(),
            description: "Multi level subdomain wildcard".to_string(),
            example: "**.example.com".to_string(),
        },
        PatternInfo {
            name: "path_wildcard".to_string(),
            description: "Path prefix wildcard".to_string(),
            example: "example.com/api/*".to_string(),
        },
        PatternInfo {
            name: "regex".to_string(),
            description: "Regular expression match".to_string(),
            example: "/api\\/v\\d+/".to_string(),
        },
        PatternInfo {
            name: "regex_insensitive".to_string(),
            description: "Case insensitive regex".to_string(),
            example: "/example/i".to_string(),
        },
        PatternInfo {
            name: "ip".to_string(),
            description: "IP address match".to_string(),
            example: "192.168.1.1".to_string(),
        },
        PatternInfo {
            name: "cidr".to_string(),
            description: "CIDR range match".to_string(),
            example: "192.168.1.0/24".to_string(),
        },
    ]
}

pub fn get_filter_value_specs() -> Vec<ProtocolValueSpec> {
    vec![
        ProtocolValueSpec {
            protocol: "includeFilter".to_string(),
            value_format: "prefix:value".to_string(),
            hints: get_filter_hints(),
            validation_regex: None,
        },
        ProtocolValueSpec {
            protocol: "excludeFilter".to_string(),
            value_format: "prefix:value".to_string(),
            hints: get_filter_hints(),
            validation_regex: None,
        },
    ]
}

fn get_filter_hints() -> Vec<ValueHint> {
    vec![
        ValueHint {
            prefix: "m:".to_string(),
            description: "Filter by HTTP method".to_string(),
            example: "m:GET or m:GET,POST,PUT".to_string(),
            completions: vec![
                "m:GET".to_string(),
                "m:POST".to_string(),
                "m:PUT".to_string(),
                "m:DELETE".to_string(),
                "m:PATCH".to_string(),
                "m:OPTIONS".to_string(),
                "m:HEAD".to_string(),
                "m:GET,POST".to_string(),
            ],
        },
        ValueHint {
            prefix: "s:".to_string(),
            description: "Filter by status code".to_string(),
            example: "s:200 or s:200-299 or s:4xx".to_string(),
            completions: vec![
                "s:200".to_string(),
                "s:200-299".to_string(),
                "s:2xx".to_string(),
                "s:3xx".to_string(),
                "s:4xx".to_string(),
                "s:5xx".to_string(),
                "s:301,302".to_string(),
                "s:400,401,403,404".to_string(),
            ],
        },
        ValueHint {
            prefix: "h:".to_string(),
            description: "Filter by header existence".to_string(),
            example: "h:X-Custom-Header".to_string(),
            completions: vec![
                "h:Content-Type".to_string(),
                "h:Authorization".to_string(),
                "h:X-Request-Id".to_string(),
            ],
        },
        ValueHint {
            prefix: "reqH:".to_string(),
            description: "Filter by request header value (supports regex)".to_string(),
            example: "reqH:Content-Type=/json/".to_string(),
            completions: vec![
                "reqH:Content-Type=/json/".to_string(),
                "reqH:Content-Type=/xml/".to_string(),
                "reqH:Authorization=/Bearer/".to_string(),
                "reqH:User-Agent=/Chrome/".to_string(),
            ],
        },
        ValueHint {
            prefix: "resH:".to_string(),
            description: "Filter by response header value (supports regex)".to_string(),
            example: "resH:Content-Type=/json/".to_string(),
            completions: vec![
                "resH:Content-Type=/json/".to_string(),
                "resH:Content-Type=/html/".to_string(),
                "resH:Cache-Control=/no-cache/".to_string(),
            ],
        },
        ValueHint {
            prefix: "i:".to_string(),
            description: "Filter by client IP (supports CIDR)".to_string(),
            example: "i:192.168.1.1 or i:192.168.0.0/16".to_string(),
            completions: vec![
                "i:127.0.0.1".to_string(),
                "i:192.168.0.0/16".to_string(),
                "i:10.0.0.0/8".to_string(),
            ],
        },
        ValueHint {
            prefix: "b:".to_string(),
            description: "Filter by request body (regex)".to_string(),
            example: "b:/error/ or b:/\"code\":\\s*0/".to_string(),
            completions: vec![
                "b:/error/".to_string(),
                "b:/success/".to_string(),
                "b:/\"code\":\\s*0/".to_string(),
            ],
        },
        ValueHint {
            prefix: "/".to_string(),
            description: "Filter by path (contains or regex)".to_string(),
            example: "/api/ or /^\\/api\\/v\\d+/".to_string(),
            completions: vec![
                "/api/".to_string(),
                "/admin/".to_string(),
                "/static/".to_string(),
                "/^\\/api\\/v\\d+/".to_string(),
            ],
        },
        ValueHint {
            prefix: "*".to_string(),
            description: "Match all requests".to_string(),
            example: "*".to_string(),
            completions: vec!["*".to_string()],
        },
    ]
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FilterValidationError {
    pub filter_type: String,
    pub value: String,
    pub message: String,
    pub suggestion: Option<String>,
}

pub fn validate_filter_value(filter_str: &str) -> Result<(), FilterValidationError> {
    let filter_str = filter_str.trim();

    if filter_str == "*" {
        return Ok(());
    }

    if filter_str.starts_with("m:") || filter_str.starts_with("M:") {
        let methods = &filter_str[2..];
        if methods.is_empty() {
            return Err(FilterValidationError {
                filter_type: "method".to_string(),
                value: filter_str.to_string(),
                message: "Method filter value is empty".to_string(),
                suggestion: Some("Use m:GET, m:POST, or m:GET,POST".to_string()),
            });
        }
        let valid_methods = [
            "GET", "POST", "PUT", "DELETE", "PATCH", "OPTIONS", "HEAD", "CONNECT", "TRACE",
        ];
        for method in methods.split(',') {
            let m = method.trim().to_uppercase();
            if !valid_methods.contains(&m.as_str()) {
                return Err(FilterValidationError {
                    filter_type: "method".to_string(),
                    value: filter_str.to_string(),
                    message: format!("Unknown HTTP method: '{}'", method.trim()),
                    suggestion: Some(format!("Valid methods: {}", valid_methods.join(", "))),
                });
            }
        }
        return Ok(());
    }

    if filter_str.starts_with("s:") || filter_str.starts_with("S:") {
        let status = &filter_str[2..];
        if status.is_empty() {
            return Err(FilterValidationError {
                filter_type: "status".to_string(),
                value: filter_str.to_string(),
                message: "Status code filter value is empty".to_string(),
                suggestion: Some("Use s:200, s:200-299, s:2xx, or s:200,404,500".to_string()),
            });
        }
        if !validate_status_code_filter(status) {
            return Err(FilterValidationError {
                filter_type: "status".to_string(),
                value: filter_str.to_string(),
                message: format!("Invalid status code format: '{}'", status),
                suggestion: Some("Use s:200, s:200-299, s:2xx, or s:200,404,500".to_string()),
            });
        }
        return Ok(());
    }

    if filter_str.starts_with("h:") || filter_str.starts_with("H:") {
        let header = &filter_str[2..];
        if header.is_empty() {
            return Err(FilterValidationError {
                filter_type: "header".to_string(),
                value: filter_str.to_string(),
                message: "Header name is empty".to_string(),
                suggestion: Some("Use h:Header-Name".to_string()),
            });
        }
        return Ok(());
    }

    if filter_str.starts_with("reqH:") || filter_str.starts_with("reqh:") {
        return validate_header_match_filter(&filter_str[5..], "reqH");
    }

    if filter_str.starts_with("resH:") || filter_str.starts_with("resh:") {
        return validate_header_match_filter(&filter_str[5..], "resH");
    }

    if filter_str.starts_with("i:") || filter_str.starts_with("I:") {
        let ip = &filter_str[2..];
        if ip.is_empty() {
            return Err(FilterValidationError {
                filter_type: "ip".to_string(),
                value: filter_str.to_string(),
                message: "IP address is empty".to_string(),
                suggestion: Some("Use i:192.168.1.1 or i:192.168.0.0/16".to_string()),
            });
        }
        if !validate_ip_filter(ip) {
            return Err(FilterValidationError {
                filter_type: "ip".to_string(),
                value: filter_str.to_string(),
                message: format!("Invalid IP address or CIDR: '{}'", ip),
                suggestion: Some("Use i:192.168.1.1 or i:192.168.0.0/16".to_string()),
            });
        }
        return Ok(());
    }

    if filter_str.starts_with("b:") || filter_str.starts_with("B:") {
        let body = &filter_str[2..];
        if body.is_empty() {
            return Err(FilterValidationError {
                filter_type: "body".to_string(),
                value: filter_str.to_string(),
                message: "Body filter pattern is empty".to_string(),
                suggestion: Some("Use b:/pattern/".to_string()),
            });
        }
        if let Err(e) = validate_regex_filter(body) {
            return Err(FilterValidationError {
                filter_type: "body".to_string(),
                value: filter_str.to_string(),
                message: format!("Invalid regex pattern: {}", e),
                suggestion: Some("Check regex syntax".to_string()),
            });
        }
        return Ok(());
    }

    if filter_str.starts_with('/') {
        if filter_str.len() < 2 {
            return Err(FilterValidationError {
                filter_type: "path".to_string(),
                value: filter_str.to_string(),
                message: "Path filter is too short".to_string(),
                suggestion: Some("Use /path/ or /regex/".to_string()),
            });
        }
        if filter_str.ends_with('/') && filter_str.len() > 2 {
            let pattern = &filter_str[1..filter_str.len() - 1];
            if let Err(e) = regex::Regex::new(pattern) {
                return Err(FilterValidationError {
                    filter_type: "path".to_string(),
                    value: filter_str.to_string(),
                    message: format!("Invalid regex pattern: {}", e),
                    suggestion: Some("Check regex syntax".to_string()),
                });
            }
        }
        return Ok(());
    }

    Err(FilterValidationError {
        filter_type: "unknown".to_string(),
        value: filter_str.to_string(),
        message: format!("Unknown filter format: '{}'", filter_str),
        suggestion: Some("Valid prefixes: m:, s:, h:, reqH:, resH:, i:, b:, /path/".to_string()),
    })
}

fn validate_status_code_filter(status: &str) -> bool {
    if status.ends_with("xx") && status.len() == 3 {
        let first = status.chars().next().unwrap();
        return first.is_ascii_digit() && ('1'..='5').contains(&first);
    }

    if status.contains('-') {
        let parts: Vec<&str> = status.split('-').collect();
        if parts.len() == 2 {
            return parts[0].parse::<u16>().is_ok() && parts[1].parse::<u16>().is_ok();
        }
        return false;
    }

    if status.contains(',') {
        return status.split(',').all(|s| s.trim().parse::<u16>().is_ok());
    }

    status.parse::<u16>().is_ok()
}

fn validate_header_match_filter(
    value: &str,
    filter_type: &str,
) -> Result<(), FilterValidationError> {
    if value.is_empty() {
        return Err(FilterValidationError {
            filter_type: filter_type.to_string(),
            value: value.to_string(),
            message: "Header match filter is empty".to_string(),
            suggestion: Some(format!("Use {}:Header-Name=/pattern/", filter_type)),
        });
    }

    if !value.contains('=') {
        return Err(FilterValidationError {
            filter_type: filter_type.to_string(),
            value: value.to_string(),
            message: "Missing '=' in header match filter".to_string(),
            suggestion: Some(format!("Use {}:Header-Name=/pattern/", filter_type)),
        });
    }

    let parts: Vec<&str> = value.splitn(2, '=').collect();
    if parts.len() != 2 || parts[0].is_empty() || parts[1].is_empty() {
        return Err(FilterValidationError {
            filter_type: filter_type.to_string(),
            value: value.to_string(),
            message: "Invalid header match format".to_string(),
            suggestion: Some(format!("Use {}:Header-Name=/pattern/", filter_type)),
        });
    }

    if let Err(e) = validate_regex_filter(parts[1]) {
        return Err(FilterValidationError {
            filter_type: filter_type.to_string(),
            value: value.to_string(),
            message: format!("Invalid regex pattern: {}", e),
            suggestion: Some("Check regex syntax".to_string()),
        });
    }

    Ok(())
}

fn validate_ip_filter(ip: &str) -> bool {
    if ip.contains('/') {
        let parts: Vec<&str> = ip.split('/').collect();
        if parts.len() != 2 {
            return false;
        }
        let prefix_len: Result<u8, _> = parts[1].parse();
        if prefix_len.is_err() {
            return false;
        }
        let prefix = prefix_len.unwrap();
        if ip.contains(':') {
            if prefix > 128 {
                return false;
            }
        } else if prefix > 32 {
            return false;
        }
        return parts[0].parse::<std::net::IpAddr>().is_ok();
    }

    ip.parse::<std::net::IpAddr>().is_ok()
}

fn validate_regex_filter(pattern: &str) -> Result<(), String> {
    let regex_str = if pattern.starts_with('/') && pattern.ends_with('/') && pattern.len() > 2 {
        &pattern[1..pattern.len() - 1]
    } else {
        pattern
    };

    regex::Regex::new(regex_str)
        .map(|_| ())
        .map_err(|e| e.to_string())
}

pub fn get_syntax_info() -> SyntaxInfo {
    let aliases = protocol_aliases();
    SyntaxInfo {
        protocols: get_all_protocols(),
        template_variables: get_template_variables(),
        patterns: get_pattern_types(),
        protocol_aliases: aliases
            .into_iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_all_protocols() {
        let protocols = get_all_protocols();
        assert!(!protocols.is_empty());

        let host = protocols.iter().find(|p| p.name == "host");
        assert!(host.is_some());
        let host = host.unwrap();
        assert_eq!(host.category, "both");
        assert!(!host.description.is_empty());

        let status_code = protocols.iter().find(|p| p.name == "statusCode");
        assert!(status_code.is_some());
        let status_code = status_code.unwrap();
        assert!(status_code.aliases.contains(&"status".to_string()));
    }

    #[test]
    fn test_get_template_variables() {
        let vars = get_template_variables();
        assert!(!vars.is_empty());

        let now = vars.iter().find(|v| v.name == "now");
        assert!(now.is_some());
    }

    #[test]
    fn test_get_syntax_info() {
        let info = get_syntax_info();
        assert!(!info.protocols.is_empty());
        assert!(!info.template_variables.is_empty());
        assert!(!info.patterns.is_empty());
        assert!(!info.protocol_aliases.is_empty());
    }

    #[test]
    fn test_filter_value_validation() {
        assert!(validate_filter_value("m:GET").is_ok());
        assert!(validate_filter_value("m:GET,POST").is_ok());
        assert!(validate_filter_value("m:INVALID").is_err());

        assert!(validate_filter_value("s:200").is_ok());
        assert!(validate_filter_value("s:200-299").is_ok());
        assert!(validate_filter_value("s:2xx").is_ok());
        assert!(validate_filter_value("s:invalid").is_err());

        assert!(validate_filter_value("h:Content-Type").is_ok());
        assert!(validate_filter_value("h:").is_err());

        assert!(validate_filter_value("reqH:Content-Type=/json/").is_ok());
        assert!(validate_filter_value("reqH:missing-equals").is_err());

        assert!(validate_filter_value("i:192.168.1.1").is_ok());
        assert!(validate_filter_value("i:192.168.0.0/16").is_ok());
        assert!(validate_filter_value("i:invalid").is_err());

        assert!(validate_filter_value("/api/").is_ok());
        assert!(validate_filter_value("/^\\/api\\/v\\d+/").is_ok());
        assert!(validate_filter_value("/[invalid/").is_err());

        assert!(validate_filter_value("*").is_ok());

        assert!(validate_filter_value("unknown:value").is_err());
    }

    #[test]
    fn test_get_filter_value_specs() {
        let specs = get_filter_value_specs();
        assert_eq!(specs.len(), 2);

        let include_spec = specs.iter().find(|s| s.protocol == "includeFilter");
        assert!(include_spec.is_some());
        let include_spec = include_spec.unwrap();
        assert!(!include_spec.hints.is_empty());

        let method_hint = include_spec.hints.iter().find(|h| h.prefix == "m:");
        assert!(method_hint.is_some());
        assert!(!method_hint.unwrap().completions.is_empty());
    }
}
