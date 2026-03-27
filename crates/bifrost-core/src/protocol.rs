use std::collections::HashMap;
use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Protocol {
    // 基础路由
    Host,
    XHost,
    Http,
    Https,
    Ws,
    Wss,
    Proxy,
    Http3,
    Pac,
    Redirect,
    File,
    Tpl,
    RawFile,
    Delete,
    Skip,

    // 请求修改
    ReqHeaders,
    ReqBody,
    ReqPrepend,
    ReqAppend,
    ReqCookies,
    ReqCors,
    ReqDelay,
    ReqSpeed,
    ReqType,
    ReqCharset,
    ReqReplace,
    ForwardedFor,
    Method,
    Auth,
    Ua,
    Referer,
    UrlParams,
    Params,

    // 响应修改
    ResHeaders,
    ResBody,
    ResPrepend,
    ResAppend,
    ResCookies,
    ResCors,
    ResDelay,
    ResSpeed,
    ResType,
    ResCharset,
    ResReplace,
    ReplaceStatus,
    StatusCode,
    Cache,
    Attachment,
    ResponseFor,
    Trailers,
    ResMerge,
    HeaderReplace,

    // 内容注入
    HtmlAppend,
    HtmlPrepend,
    HtmlBody,
    JsAppend,
    JsPrepend,
    JsBody,
    CssAppend,
    CssPrepend,
    CssBody,

    // URL处理
    UrlReplace,

    // 脚本插件
    ReqScript,
    ResScript,
    Decode,

    // DNS 解析
    Dns,

    // TLS 拦截控制
    TlsIntercept,
    TlsPassthrough,
    TlsOptions,
    SniCallback,

    // 直连/透传
    Passthrough,
    Tunnel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtocolCategory {
    Request,
    Response,
    Both,
    Control,
}

pub const MULTI_MATCH_PROTOCOLS: &[Protocol] = &[
    Protocol::Trailers,
    Protocol::UrlParams,
    Protocol::Params,
    Protocol::HeaderReplace,
    Protocol::ReqHeaders,
    Protocol::ResHeaders,
    Protocol::ReqCors,
    Protocol::ResCors,
    Protocol::ReqCookies,
    Protocol::ResCookies,
    Protocol::ReqReplace,
    Protocol::UrlReplace,
    Protocol::ResReplace,
    Protocol::ResMerge,
    Protocol::ReqBody,
    Protocol::ReqPrepend,
    Protocol::ResPrepend,
    Protocol::ReqAppend,
    Protocol::ResAppend,
    Protocol::ResBody,
    Protocol::HtmlAppend,
    Protocol::JsAppend,
    Protocol::CssAppend,
    Protocol::HtmlBody,
    Protocol::JsBody,
    Protocol::CssBody,
    Protocol::HtmlPrepend,
    Protocol::JsPrepend,
    Protocol::CssPrepend,
    Protocol::ReqScript,
    Protocol::ResScript,
    Protocol::Decode,
    Protocol::Delete,
    Protocol::Skip,
];

pub fn protocol_aliases() -> HashMap<&'static str, &'static str> {
    let mut map = HashMap::new();
    map.insert("ignore", "passthrough");
    map.insert("pathReplace", "urlReplace");
    map.insert("download", "attachment");
    map.insert("http-proxy", "proxy");
    map.insert("h3", "http3");
    map.insert("status", "statusCode");
    map.insert("hosts", "host");
    map.insert("html", "htmlAppend");
    map.insert("js", "jsAppend");
    map.insert("reqMerge", "params");
    map.insert("css", "cssAppend");
    map
}

fn normalize_legacy_rule_line(line: &str) -> String {
    let comment_start = line.find('#').unwrap_or(line.len());
    let (body, comment) = line.split_at(comment_start);
    let trimmed = body.trim();

    if trimmed.is_empty() {
        return line.to_string();
    }

    if let Some(pattern) = trimmed.strip_prefix("ignore://") {
        let pattern = pattern.trim();
        if pattern.is_empty() {
            return line.to_string();
        }
        let mut normalized = format!("{pattern} passthrough://");
        if !comment.is_empty() {
            normalized.push_str(comment);
        }
        return normalized;
    }

    let mut changed = false;
    let tokens: Vec<String> = body
        .split_whitespace()
        .map(|token| {
            if token.starts_with("ignore://") {
                changed = true;
                "passthrough://".to_string()
            } else {
                token.to_string()
            }
        })
        .collect();

    if !changed {
        return line.to_string();
    }

    let mut normalized = tokens.join(" ");
    if !comment.is_empty() {
        normalized.push_str(comment);
    }
    normalized
}

pub fn normalize_rule_content(content: &str) -> String {
    let mut normalized = Vec::new();
    let mut in_fenced_block = false;
    let mut in_line_block = false;

    for line in content.lines() {
        let trimmed = line.trim();

        if in_line_block {
            normalized.push(line.to_string());
            if trimmed == "`" {
                in_line_block = false;
            }
            continue;
        }

        if !in_fenced_block && trimmed == "line`" {
            in_line_block = true;
            normalized.push(line.to_string());
            continue;
        }

        if trimmed.starts_with("```") {
            in_fenced_block = !in_fenced_block;
            normalized.push(line.to_string());
            continue;
        }

        if in_fenced_block {
            normalized.push(line.to_string());
            continue;
        }

        normalized.push(normalize_legacy_rule_line(line));
    }

    let mut result = normalized.join("\n");
    if content.ends_with('\n') {
        result.push('\n');
    }
    result
}

lazy_static::lazy_static! {
    pub static ref PROTOCOL_ALIASES: HashMap<&'static str, &'static str> = protocol_aliases();
}

const PURE_RES_PROTOCOLS: &[Protocol] = &[
    Protocol::ReplaceStatus,
    Protocol::StatusCode,
    Protocol::Cache,
    Protocol::Attachment,
    Protocol::ResMerge,
    Protocol::ResDelay,
    Protocol::ResSpeed,
    Protocol::ResType,
    Protocol::ResCharset,
    Protocol::ResCookies,
    Protocol::ResCors,
    Protocol::ResHeaders,
    Protocol::Trailers,
    Protocol::ResPrepend,
    Protocol::ResBody,
    Protocol::ResAppend,
    Protocol::ResReplace,
    Protocol::CssAppend,
    Protocol::HtmlAppend,
    Protocol::JsAppend,
    Protocol::CssBody,
    Protocol::HtmlBody,
    Protocol::JsBody,
    Protocol::CssPrepend,
    Protocol::HtmlPrepend,
    Protocol::JsPrepend,
];

impl Protocol {
    pub fn parse(s: &str) -> Option<Protocol> {
        let resolved = Self::resolve_alias(s);
        match resolved {
            "host" => Some(Protocol::Host),
            "xhost" => Some(Protocol::XHost),
            "http" => Some(Protocol::Http),
            "https" => Some(Protocol::Https),
            "ws" => Some(Protocol::Ws),
            "wss" => Some(Protocol::Wss),
            "proxy" => Some(Protocol::Proxy),
            "http3" => Some(Protocol::Http3),
            "pac" => Some(Protocol::Pac),
            "redirect" => Some(Protocol::Redirect),
            "file" => Some(Protocol::File),
            "tpl" => Some(Protocol::Tpl),
            "rawfile" => Some(Protocol::RawFile),
            "delete" => Some(Protocol::Delete),
            "skip" => Some(Protocol::Skip),
            "referer" => Some(Protocol::Referer),
            "auth" => Some(Protocol::Auth),
            "ua" => Some(Protocol::Ua),
            "urlParams" => Some(Protocol::UrlParams),
            "params" => Some(Protocol::Params),
            "resMerge" => Some(Protocol::ResMerge),
            "replaceStatus" => Some(Protocol::ReplaceStatus),
            "statusCode" => Some(Protocol::StatusCode),
            "method" => Some(Protocol::Method),
            "cache" => Some(Protocol::Cache),
            "attachment" => Some(Protocol::Attachment),
            "reqScript" => Some(Protocol::ReqScript),
            "resScript" => Some(Protocol::ResScript),
            "decode" => Some(Protocol::Decode),
            "reqDelay" => Some(Protocol::ReqDelay),
            "resDelay" => Some(Protocol::ResDelay),
            "headerReplace" => Some(Protocol::HeaderReplace),
            "reqSpeed" => Some(Protocol::ReqSpeed),
            "resSpeed" => Some(Protocol::ResSpeed),
            "reqType" => Some(Protocol::ReqType),
            "resType" => Some(Protocol::ResType),
            "reqCharset" => Some(Protocol::ReqCharset),
            "resCharset" => Some(Protocol::ResCharset),
            "reqCookies" => Some(Protocol::ReqCookies),
            "resCookies" => Some(Protocol::ResCookies),
            "forwardedFor" => Some(Protocol::ForwardedFor),
            "responseFor" => Some(Protocol::ResponseFor),
            "reqCors" => Some(Protocol::ReqCors),
            "resCors" => Some(Protocol::ResCors),
            "reqHeaders" => Some(Protocol::ReqHeaders),
            "resHeaders" => Some(Protocol::ResHeaders),
            "trailers" => Some(Protocol::Trailers),
            "reqPrepend" => Some(Protocol::ReqPrepend),
            "resPrepend" => Some(Protocol::ResPrepend),
            "reqBody" => Some(Protocol::ReqBody),
            "resBody" => Some(Protocol::ResBody),
            "reqAppend" => Some(Protocol::ReqAppend),
            "resAppend" => Some(Protocol::ResAppend),
            "urlReplace" => Some(Protocol::UrlReplace),
            "reqReplace" => Some(Protocol::ReqReplace),
            "resReplace" => Some(Protocol::ResReplace),
            "cssAppend" => Some(Protocol::CssAppend),
            "htmlAppend" => Some(Protocol::HtmlAppend),
            "jsAppend" => Some(Protocol::JsAppend),
            "cssBody" => Some(Protocol::CssBody),
            "htmlBody" => Some(Protocol::HtmlBody),
            "jsBody" => Some(Protocol::JsBody),
            "cssPrepend" => Some(Protocol::CssPrepend),
            "htmlPrepend" => Some(Protocol::HtmlPrepend),
            "jsPrepend" => Some(Protocol::JsPrepend),
            "dns" => Some(Protocol::Dns),
            "tlsIntercept" => Some(Protocol::TlsIntercept),
            "tlsPassthrough" => Some(Protocol::TlsPassthrough),
            "tlsOptions" => Some(Protocol::TlsOptions),
            "sniCallback" => Some(Protocol::SniCallback),
            "passthrough" => Some(Protocol::Passthrough),
            "tunnel" => Some(Protocol::Tunnel),
            _ => None,
        }
    }

    pub fn to_str(&self) -> &'static str {
        match self {
            Protocol::Host => "host",
            Protocol::XHost => "xhost",
            Protocol::Http => "http",
            Protocol::Https => "https",
            Protocol::Ws => "ws",
            Protocol::Wss => "wss",
            Protocol::Proxy => "proxy",
            Protocol::Http3 => "http3",
            Protocol::Pac => "pac",
            Protocol::Redirect => "redirect",
            Protocol::File => "file",
            Protocol::Tpl => "tpl",
            Protocol::RawFile => "rawfile",
            Protocol::Delete => "delete",
            Protocol::Skip => "skip",
            Protocol::Referer => "referer",
            Protocol::Auth => "auth",
            Protocol::Ua => "ua",
            Protocol::UrlParams => "urlParams",
            Protocol::Params => "params",
            Protocol::ResMerge => "resMerge",
            Protocol::ReplaceStatus => "replaceStatus",
            Protocol::StatusCode => "statusCode",
            Protocol::Method => "method",
            Protocol::Cache => "cache",
            Protocol::Attachment => "attachment",
            Protocol::ReqScript => "reqScript",
            Protocol::ResScript => "resScript",
            Protocol::Decode => "decode",
            Protocol::ReqDelay => "reqDelay",
            Protocol::ResDelay => "resDelay",
            Protocol::HeaderReplace => "headerReplace",
            Protocol::ReqSpeed => "reqSpeed",
            Protocol::ResSpeed => "resSpeed",
            Protocol::ReqType => "reqType",
            Protocol::ResType => "resType",
            Protocol::ReqCharset => "reqCharset",
            Protocol::ResCharset => "resCharset",
            Protocol::ReqCookies => "reqCookies",
            Protocol::ResCookies => "resCookies",
            Protocol::ForwardedFor => "forwardedFor",
            Protocol::ResponseFor => "responseFor",
            Protocol::ReqCors => "reqCors",
            Protocol::ResCors => "resCors",
            Protocol::ReqHeaders => "reqHeaders",
            Protocol::ResHeaders => "resHeaders",
            Protocol::Trailers => "trailers",
            Protocol::ReqPrepend => "reqPrepend",
            Protocol::ResPrepend => "resPrepend",
            Protocol::ReqBody => "reqBody",
            Protocol::ResBody => "resBody",
            Protocol::ReqAppend => "reqAppend",
            Protocol::ResAppend => "resAppend",
            Protocol::UrlReplace => "urlReplace",
            Protocol::ReqReplace => "reqReplace",
            Protocol::ResReplace => "resReplace",
            Protocol::CssAppend => "cssAppend",
            Protocol::HtmlAppend => "htmlAppend",
            Protocol::JsAppend => "jsAppend",
            Protocol::CssBody => "cssBody",
            Protocol::HtmlBody => "htmlBody",
            Protocol::JsBody => "jsBody",
            Protocol::CssPrepend => "cssPrepend",
            Protocol::HtmlPrepend => "htmlPrepend",
            Protocol::JsPrepend => "jsPrepend",
            Protocol::Dns => "dns",
            Protocol::TlsIntercept => "tlsIntercept",
            Protocol::TlsPassthrough => "tlsPassthrough",
            Protocol::TlsOptions => "tlsOptions",
            Protocol::SniCallback => "sniCallback",
            Protocol::Passthrough => "passthrough",
            Protocol::Tunnel => "tunnel",
        }
    }

    pub fn category(&self) -> ProtocolCategory {
        match self {
            Protocol::TlsIntercept
            | Protocol::TlsPassthrough
            | Protocol::TlsOptions
            | Protocol::SniCallback
            | Protocol::Passthrough
            | Protocol::Tunnel
            | Protocol::Skip => ProtocolCategory::Control,

            Protocol::ReplaceStatus
            | Protocol::StatusCode
            | Protocol::Cache
            | Protocol::Attachment
            | Protocol::ResponseFor
            | Protocol::ResMerge
            | Protocol::ResDelay
            | Protocol::ResSpeed
            | Protocol::ResType
            | Protocol::ResCharset
            | Protocol::ResCookies
            | Protocol::ResCors
            | Protocol::ResHeaders
            | Protocol::Trailers
            | Protocol::ResPrepend
            | Protocol::ResBody
            | Protocol::ResAppend
            | Protocol::ResReplace
            | Protocol::CssAppend
            | Protocol::HtmlAppend
            | Protocol::JsAppend
            | Protocol::CssBody
            | Protocol::HtmlBody
            | Protocol::JsBody
            | Protocol::CssPrepend
            | Protocol::HtmlPrepend
            | Protocol::JsPrepend
            | Protocol::ResScript => ProtocolCategory::Response,

            Protocol::ReqHeaders
            | Protocol::ReqBody
            | Protocol::ReqPrepend
            | Protocol::ReqAppend
            | Protocol::ReqCookies
            | Protocol::ReqCors
            | Protocol::ReqDelay
            | Protocol::ReqSpeed
            | Protocol::ReqType
            | Protocol::ReqCharset
            | Protocol::ReqReplace
            | Protocol::ForwardedFor
            | Protocol::Method
            | Protocol::Auth
            | Protocol::Ua
            | Protocol::Referer
            | Protocol::UrlParams
            | Protocol::Params
            | Protocol::ReqScript
            | Protocol::Dns
            | Protocol::Http3 => ProtocolCategory::Request,

            Protocol::Host
            | Protocol::XHost
            | Protocol::Http
            | Protocol::Https
            | Protocol::Ws
            | Protocol::Wss
            | Protocol::Proxy
            | Protocol::Pac
            | Protocol::Redirect
            | Protocol::File
            | Protocol::Tpl
            | Protocol::RawFile
            | Protocol::Delete
            | Protocol::HeaderReplace
            | Protocol::UrlReplace
            | Protocol::Decode => ProtocolCategory::Both,
        }
    }

    pub fn is_multi_match(&self) -> bool {
        MULTI_MATCH_PROTOCOLS.contains(self)
    }

    pub fn resolve_alias(name: &str) -> &str {
        PROTOCOL_ALIASES.get(name).copied().unwrap_or(name)
    }

    pub fn is_res_protocol(&self) -> bool {
        PURE_RES_PROTOCOLS.contains(self) || matches!(self, Protocol::HeaderReplace)
    }

    pub fn is_req_protocol(&self) -> bool {
        !PURE_RES_PROTOCOLS.contains(self)
    }

    pub fn all() -> &'static [Protocol] {
        &ALL_PROTOCOLS
    }
}

impl FromStr for Protocol {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Protocol::parse(s).ok_or(())
    }
}

impl std::fmt::Display for Protocol {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.to_str())
    }
}

pub const ALL_PROTOCOLS: [Protocol; 72] = [
    Protocol::Host,
    Protocol::XHost,
    Protocol::Http,
    Protocol::Https,
    Protocol::Ws,
    Protocol::Wss,
    Protocol::Proxy,
    Protocol::Http3,
    Protocol::Pac,
    Protocol::Redirect,
    Protocol::File,
    Protocol::Tpl,
    Protocol::RawFile,
    Protocol::Delete,
    Protocol::Skip,
    Protocol::Referer,
    Protocol::Auth,
    Protocol::Ua,
    Protocol::UrlParams,
    Protocol::Params,
    Protocol::ResMerge,
    Protocol::ReplaceStatus,
    Protocol::StatusCode,
    Protocol::Method,
    Protocol::Cache,
    Protocol::Attachment,
    Protocol::ReqScript,
    Protocol::ResScript,
    Protocol::Decode,
    Protocol::ReqDelay,
    Protocol::ResDelay,
    Protocol::HeaderReplace,
    Protocol::ReqSpeed,
    Protocol::ResSpeed,
    Protocol::ReqType,
    Protocol::ResType,
    Protocol::ReqCharset,
    Protocol::ResCharset,
    Protocol::ReqCookies,
    Protocol::ResCookies,
    Protocol::ForwardedFor,
    Protocol::ResponseFor,
    Protocol::ReqCors,
    Protocol::ResCors,
    Protocol::ReqHeaders,
    Protocol::ResHeaders,
    Protocol::Trailers,
    Protocol::ReqPrepend,
    Protocol::ResPrepend,
    Protocol::ReqBody,
    Protocol::ResBody,
    Protocol::ReqAppend,
    Protocol::ResAppend,
    Protocol::UrlReplace,
    Protocol::ReqReplace,
    Protocol::ResReplace,
    Protocol::CssAppend,
    Protocol::HtmlAppend,
    Protocol::JsAppend,
    Protocol::CssBody,
    Protocol::HtmlBody,
    Protocol::JsBody,
    Protocol::CssPrepend,
    Protocol::HtmlPrepend,
    Protocol::JsPrepend,
    Protocol::Dns,
    Protocol::TlsIntercept,
    Protocol::TlsPassthrough,
    Protocol::TlsOptions,
    Protocol::SniCallback,
    Protocol::Passthrough,
    Protocol::Tunnel,
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_protocol_count() {
        assert_eq!(ALL_PROTOCOLS.len(), 72);
    }

    #[test]
    fn test_all_protocols_parse() {
        let protocol_names = [
            "host",
            "xhost",
            "http",
            "https",
            "ws",
            "wss",
            "proxy",
            "http3",
            "pac",
            "redirect",
            "file",
            "tpl",
            "rawfile",
            "delete",
            "skip",
            "referer",
            "auth",
            "ua",
            "urlParams",
            "params",
            "resMerge",
            "replaceStatus",
            "statusCode",
            "method",
            "cache",
            "attachment",
            "reqScript",
            "resScript",
            "decode",
            "reqDelay",
            "resDelay",
            "headerReplace",
            "reqSpeed",
            "resSpeed",
            "reqType",
            "resType",
            "reqCharset",
            "resCharset",
            "reqCookies",
            "resCookies",
            "forwardedFor",
            "responseFor",
            "reqCors",
            "resCors",
            "reqHeaders",
            "resHeaders",
            "trailers",
            "reqPrepend",
            "resPrepend",
            "reqBody",
            "resBody",
            "reqAppend",
            "resAppend",
            "urlReplace",
            "reqReplace",
            "resReplace",
            "cssAppend",
            "htmlAppend",
            "jsAppend",
            "cssBody",
            "htmlBody",
            "jsBody",
            "cssPrepend",
            "htmlPrepend",
            "jsPrepend",
            "dns",
            "tlsIntercept",
            "tlsPassthrough",
            "tlsOptions",
            "sniCallback",
            "passthrough",
            "tunnel",
        ];

        for name in &protocol_names {
            let result = Protocol::parse(name);
            assert!(result.is_some(), "Failed to parse protocol: {}", name);
        }

        assert_eq!(protocol_names.len(), 72);
    }

    #[test]
    fn test_protocol_roundtrip() {
        for protocol in ALL_PROTOCOLS.iter() {
            let name = protocol.to_str();
            let parsed = Protocol::parse(name);
            assert_eq!(parsed, Some(*protocol), "Roundtrip failed for: {}", name);
        }
    }

    #[test]
    fn test_alias_resolution() {
        assert_eq!(Protocol::resolve_alias("ignore"), "passthrough");
        assert_eq!(Protocol::resolve_alias("pathReplace"), "urlReplace");
        assert_eq!(Protocol::resolve_alias("download"), "attachment");
        assert_eq!(Protocol::resolve_alias("http-proxy"), "proxy");
        assert_eq!(Protocol::resolve_alias("h3"), "http3");
        assert_eq!(Protocol::resolve_alias("status"), "statusCode");
        assert_eq!(Protocol::resolve_alias("hosts"), "host");
        assert_eq!(Protocol::resolve_alias("xhost"), "xhost");
        assert_eq!(Protocol::resolve_alias("html"), "htmlAppend");
        assert_eq!(Protocol::resolve_alias("js"), "jsAppend");
        assert_eq!(Protocol::resolve_alias("reqMerge"), "params");
        assert_eq!(Protocol::resolve_alias("css"), "cssAppend");
        assert_eq!(Protocol::resolve_alias("reqScript"), "reqScript");
    }

    #[test]
    fn test_alias_parse() {
        let resolved = Protocol::resolve_alias("hosts");
        assert_eq!(Protocol::parse(resolved), Some(Protocol::Host));

        let resolved = Protocol::resolve_alias("download");
        assert_eq!(Protocol::parse(resolved), Some(Protocol::Attachment));

        let resolved = Protocol::resolve_alias("html");
        assert_eq!(Protocol::parse(resolved), Some(Protocol::HtmlAppend));

        let resolved = Protocol::resolve_alias("css");
        assert_eq!(Protocol::parse(resolved), Some(Protocol::CssAppend));

        let resolved = Protocol::resolve_alias("js");
        assert_eq!(Protocol::parse(resolved), Some(Protocol::JsAppend));

        let resolved = Protocol::resolve_alias("h3");
        assert_eq!(Protocol::parse(resolved), Some(Protocol::Http3));

        let resolved = Protocol::resolve_alias("ignore");
        assert_eq!(Protocol::parse(resolved), Some(Protocol::Passthrough));
    }

    #[test]
    fn test_normalize_rule_content_converts_ignore_operations() {
        let content = "example.com ignore://host|rule\nignore://*.png\n";
        let normalized = normalize_rule_content(content);
        assert_eq!(
            normalized,
            "example.com passthrough://\n*.png passthrough://\n"
        );
    }

    #[test]
    fn test_normalize_rule_content_keeps_blocks_unchanged() {
        let content = "``` example\nignore://*.png\n```\nexample.com ignore://*\n";
        let normalized = normalize_rule_content(content);
        assert_eq!(
            normalized,
            "``` example\nignore://*.png\n```\nexample.com passthrough://\n"
        );
    }

    #[test]
    fn test_multi_match_protocols() {
        assert!(Protocol::Trailers.is_multi_match());
        assert!(Protocol::UrlParams.is_multi_match());
        assert!(Protocol::Params.is_multi_match());
        assert!(Protocol::HeaderReplace.is_multi_match());
        assert!(Protocol::ReqHeaders.is_multi_match());
        assert!(Protocol::ResHeaders.is_multi_match());
        assert!(Protocol::ReqCors.is_multi_match());
        assert!(Protocol::ResCors.is_multi_match());
        assert!(Protocol::ReqCookies.is_multi_match());
        assert!(Protocol::ResCookies.is_multi_match());
        assert!(Protocol::ReqReplace.is_multi_match());
        assert!(Protocol::UrlReplace.is_multi_match());
        assert!(Protocol::ResReplace.is_multi_match());
        assert!(Protocol::ResMerge.is_multi_match());
        assert!(Protocol::ReqBody.is_multi_match());
        assert!(Protocol::ReqPrepend.is_multi_match());
        assert!(Protocol::ResPrepend.is_multi_match());
        assert!(Protocol::ReqAppend.is_multi_match());
        assert!(Protocol::ResAppend.is_multi_match());
        assert!(Protocol::ResBody.is_multi_match());
        assert!(Protocol::HtmlAppend.is_multi_match());
        assert!(Protocol::JsAppend.is_multi_match());
        assert!(Protocol::CssAppend.is_multi_match());
        assert!(Protocol::HtmlBody.is_multi_match());
        assert!(Protocol::JsBody.is_multi_match());
        assert!(Protocol::CssBody.is_multi_match());
        assert!(Protocol::HtmlPrepend.is_multi_match());
        assert!(Protocol::JsPrepend.is_multi_match());
        assert!(Protocol::CssPrepend.is_multi_match());
        assert!(Protocol::ReqScript.is_multi_match());
        assert!(Protocol::ResScript.is_multi_match());
        assert!(Protocol::Delete.is_multi_match());

        assert!(!Protocol::Host.is_multi_match());
        assert!(!Protocol::Proxy.is_multi_match());
        assert!(!Protocol::Method.is_multi_match());
        assert!(!Protocol::Auth.is_multi_match());
    }

    #[test]
    fn test_protocol_category_control() {
        assert_eq!(Protocol::TlsIntercept.category(), ProtocolCategory::Control);
        assert_eq!(
            Protocol::TlsPassthrough.category(),
            ProtocolCategory::Control
        );
    }

    #[test]
    fn test_protocol_category_request() {
        assert_eq!(Protocol::ReqHeaders.category(), ProtocolCategory::Request);
        assert_eq!(Protocol::ReqBody.category(), ProtocolCategory::Request);
        assert_eq!(Protocol::ReqPrepend.category(), ProtocolCategory::Request);
        assert_eq!(Protocol::ReqAppend.category(), ProtocolCategory::Request);
        assert_eq!(Protocol::ReqCookies.category(), ProtocolCategory::Request);
        assert_eq!(Protocol::ReqCors.category(), ProtocolCategory::Request);
        assert_eq!(Protocol::ReqDelay.category(), ProtocolCategory::Request);
        assert_eq!(Protocol::ReqSpeed.category(), ProtocolCategory::Request);
        assert_eq!(Protocol::ReqType.category(), ProtocolCategory::Request);
        assert_eq!(Protocol::ReqCharset.category(), ProtocolCategory::Request);
        assert_eq!(Protocol::ReqReplace.category(), ProtocolCategory::Request);
        assert_eq!(Protocol::Method.category(), ProtocolCategory::Request);
        assert_eq!(Protocol::Auth.category(), ProtocolCategory::Request);
        assert_eq!(Protocol::Ua.category(), ProtocolCategory::Request);
        assert_eq!(Protocol::Referer.category(), ProtocolCategory::Request);
        assert_eq!(Protocol::UrlParams.category(), ProtocolCategory::Request);
        assert_eq!(Protocol::Params.category(), ProtocolCategory::Request);
        assert_eq!(Protocol::ReqScript.category(), ProtocolCategory::Request);
        assert_eq!(Protocol::Http3.category(), ProtocolCategory::Request);
    }

    #[test]
    fn test_protocol_category_response() {
        assert_eq!(Protocol::ResHeaders.category(), ProtocolCategory::Response);
        assert_eq!(Protocol::ResBody.category(), ProtocolCategory::Response);
        assert_eq!(Protocol::ResPrepend.category(), ProtocolCategory::Response);
        assert_eq!(Protocol::ResAppend.category(), ProtocolCategory::Response);
        assert_eq!(Protocol::ResCookies.category(), ProtocolCategory::Response);
        assert_eq!(Protocol::ResCors.category(), ProtocolCategory::Response);
        assert_eq!(Protocol::ResDelay.category(), ProtocolCategory::Response);
        assert_eq!(Protocol::ResSpeed.category(), ProtocolCategory::Response);
        assert_eq!(Protocol::ResType.category(), ProtocolCategory::Response);
        assert_eq!(Protocol::ResCharset.category(), ProtocolCategory::Response);
        assert_eq!(Protocol::ResReplace.category(), ProtocolCategory::Response);
        assert_eq!(
            Protocol::ReplaceStatus.category(),
            ProtocolCategory::Response
        );
        assert_eq!(Protocol::Cache.category(), ProtocolCategory::Response);
        assert_eq!(Protocol::Attachment.category(), ProtocolCategory::Response);
        assert_eq!(Protocol::Trailers.category(), ProtocolCategory::Response);
        assert_eq!(Protocol::ResMerge.category(), ProtocolCategory::Response);
        assert_eq!(Protocol::HtmlAppend.category(), ProtocolCategory::Response);
        assert_eq!(Protocol::HtmlPrepend.category(), ProtocolCategory::Response);
        assert_eq!(Protocol::HtmlBody.category(), ProtocolCategory::Response);
        assert_eq!(Protocol::JsAppend.category(), ProtocolCategory::Response);
        assert_eq!(Protocol::JsPrepend.category(), ProtocolCategory::Response);
        assert_eq!(Protocol::JsBody.category(), ProtocolCategory::Response);
        assert_eq!(Protocol::CssAppend.category(), ProtocolCategory::Response);
        assert_eq!(Protocol::CssPrepend.category(), ProtocolCategory::Response);
        assert_eq!(Protocol::CssBody.category(), ProtocolCategory::Response);
        assert_eq!(Protocol::ResScript.category(), ProtocolCategory::Response);
    }

    #[test]
    fn test_protocol_category_both_decode() {
        assert_eq!(Protocol::Decode.category(), ProtocolCategory::Both);
    }

    #[test]
    fn test_protocol_category_both() {
        assert_eq!(Protocol::Host.category(), ProtocolCategory::Both);
        assert_eq!(Protocol::Proxy.category(), ProtocolCategory::Both);
        assert_eq!(Protocol::HeaderReplace.category(), ProtocolCategory::Both);
        assert_eq!(Protocol::UrlReplace.category(), ProtocolCategory::Both);
    }

    #[test]
    fn test_is_res_protocol() {
        assert!(Protocol::ResHeaders.is_res_protocol());
        assert!(Protocol::ResBody.is_res_protocol());
        assert!(Protocol::ReplaceStatus.is_res_protocol());
        assert!(Protocol::Cache.is_res_protocol());
        assert!(Protocol::HeaderReplace.is_res_protocol());
    }

    #[test]
    fn test_is_req_protocol() {
        assert!(Protocol::ReqHeaders.is_req_protocol());
        assert!(Protocol::ReqBody.is_req_protocol());
        assert!(Protocol::Method.is_req_protocol());
        assert!(Protocol::Auth.is_req_protocol());
        assert!(Protocol::Ua.is_req_protocol());
        assert!(Protocol::Host.is_req_protocol());
        assert!(Protocol::Proxy.is_req_protocol());
        assert!(Protocol::Http3.is_req_protocol());
    }

    #[test]
    fn test_protocol_display() {
        assert_eq!(format!("{}", Protocol::Host), "host");
        assert_eq!(format!("{}", Protocol::Proxy), "proxy");
        assert_eq!(format!("{}", Protocol::Http3), "http3");
        assert_eq!(format!("{}", Protocol::ReqHeaders), "reqHeaders");
        assert_eq!(format!("{}", Protocol::ResHeaders), "resHeaders");
    }

    #[test]
    fn test_protocol_from_str_trait() {
        let result: Result<Protocol, _> = "host".parse();
        assert_eq!(result, Ok(Protocol::Host));

        let result: Result<Protocol, _> = "invalid".parse();
        assert!(result.is_err());
    }

    #[test]
    fn test_unknown_protocol_returns_none() {
        assert!(Protocol::parse("unknown").is_none());
        assert!(Protocol::parse("").is_none());
        assert!(Protocol::parse("HOST").is_none());
    }

    #[test]
    fn test_all_protocols_function() {
        let all = Protocol::all();
        assert_eq!(all, &ALL_PROTOCOLS);
        assert!(all.contains(&Protocol::Host));
        assert!(all.contains(&Protocol::Http));
        assert!(all.contains(&Protocol::Https));
        assert!(all.contains(&Protocol::Ws));
        assert!(all.contains(&Protocol::Wss));
        assert!(all.contains(&Protocol::Proxy));
        assert!(all.contains(&Protocol::Http3));
        assert!(all.contains(&Protocol::Pac));
        assert!(all.contains(&Protocol::Passthrough));
        assert!(all.contains(&Protocol::ReqScript));
        assert!(all.contains(&Protocol::Decode));
    }

    #[test]
    fn test_protocol_aliases_are_parseable() {
        for (alias, canonical) in PROTOCOL_ALIASES.iter() {
            assert_ne!(alias, canonical);
            assert!(
                Protocol::parse(canonical).is_some(),
                "Alias {alias} resolves to unknown protocol {canonical}"
            );
        }
    }

    #[test]
    fn test_multi_match_protocols_are_unique() {
        let unique: std::collections::HashSet<_> = MULTI_MATCH_PROTOCOLS.iter().copied().collect();
        assert_eq!(unique.len(), MULTI_MATCH_PROTOCOLS.len());
        assert!(MULTI_MATCH_PROTOCOLS.iter().all(Protocol::is_multi_match));
    }
}
