use std::net::{IpAddr, ToSocketAddrs};
use std::sync::Arc;
use std::time::{Duration, Instant};

use rquickjs::{Context, Function, Runtime};

use crate::{Result, ScriptError};

const DEFAULT_TIMEOUT_MS: u64 = 50;
const DEFAULT_MAX_MEMORY: usize = 4 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PacDecision {
    Direct,
    Proxy {
        scheme: PacProxyScheme,
        host_port: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PacProxyScheme {
    Http,
    Https,
    Socks,
    Socks5,
}

impl PacProxyScheme {
    pub fn as_proxy_url_scheme(self) -> Option<&'static str> {
        match self {
            Self::Http => Some("http"),
            Self::Https => Some("https"),
            Self::Socks | Self::Socks5 => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct PacEngineConfig {
    pub timeout_ms: u64,
    pub max_memory: usize,
}

impl Default for PacEngineConfig {
    fn default() -> Self {
        Self {
            timeout_ms: DEFAULT_TIMEOUT_MS,
            max_memory: DEFAULT_MAX_MEMORY,
        }
    }
}

pub struct PacEngine {
    config: PacEngineConfig,
}

impl PacEngine {
    pub fn new(config: PacEngineConfig) -> Self {
        Self { config }
    }

    pub fn evaluate(&self, script: &str, url: &str, host: &str) -> Result<PacDecision> {
        if script.len() > 1024 * 1024 {
            return Err(ScriptError::RuntimeError(
                "PAC script exceeds 1 MiB limit".to_string(),
            ));
        }

        let runtime = Runtime::new().map_err(|e| ScriptError::QuickJsError(e.to_string()))?;
        runtime.set_memory_limit(self.config.max_memory);
        let deadline = Arc::new(Instant::now() + Duration::from_millis(self.config.timeout_ms));
        let interrupt_deadline = deadline.clone();
        runtime.set_interrupt_handler(Some(Box::new(move || {
            Instant::now() >= *interrupt_deadline
        })));

        let context =
            Context::full(&runtime).map_err(|e| ScriptError::QuickJsError(e.to_string()))?;

        let raw = context.with(|js_ctx| {
            install_pac_helpers(&js_ctx)?;
            remove_dangerous_globals(&js_ctx);
            js_ctx.eval::<(), _>(script).map_err(|e| {
                if Instant::now() >= *deadline {
                    ScriptError::Timeout(self.config.timeout_ms)
                } else {
                    ScriptError::ExecutionFailed(format_js_error(&js_ctx, e))
                }
            })?;
            if Instant::now() >= *deadline {
                return Err(ScriptError::Timeout(self.config.timeout_ms));
            }
            let globals = js_ctx.globals();
            let finder: Function = globals.get("FindProxyForURL").map_err(|_| {
                ScriptError::RuntimeError("FindProxyForURL is not defined".to_string())
            })?;
            let result: String = finder
                .call((url.to_string(), host.to_string()))
                .map_err(|e| {
                    if Instant::now() >= *deadline {
                        ScriptError::Timeout(self.config.timeout_ms)
                    } else {
                        ScriptError::ExecutionFailed(format_js_error(&js_ctx, e))
                    }
                })?;
            Ok::<String, ScriptError>(result)
        })?;

        if Instant::now() >= *deadline {
            return Err(ScriptError::Timeout(self.config.timeout_ms));
        }

        parse_pac_decision(&raw)
    }
}

pub fn parse_pac_decision(raw: &str) -> Result<PacDecision> {
    for candidate in raw.split(';') {
        let candidate = candidate.trim();
        if candidate.is_empty() {
            continue;
        }
        let mut parts = candidate.split_whitespace();
        let Some(kind) = parts.next() else {
            continue;
        };
        if kind.eq_ignore_ascii_case("DIRECT") {
            return Ok(PacDecision::Direct);
        }
        let scheme = if kind.eq_ignore_ascii_case("PROXY") || kind.eq_ignore_ascii_case("HTTP") {
            Some(PacProxyScheme::Http)
        } else if kind.eq_ignore_ascii_case("HTTPS") {
            Some(PacProxyScheme::Https)
        } else if kind.eq_ignore_ascii_case("SOCKS") {
            Some(PacProxyScheme::Socks)
        } else if kind.eq_ignore_ascii_case("SOCKS5") {
            Some(PacProxyScheme::Socks5)
        } else {
            None
        };
        if let Some(scheme) = scheme {
            let host_port = parts
                .next()
                .unwrap_or("")
                .trim_matches('"')
                .trim_matches('\'');
            if host_port.is_empty() {
                return Err(ScriptError::RuntimeError(format!(
                    "PAC {} result is missing host:port",
                    kind
                )));
            }
            return Ok(PacDecision::Proxy {
                scheme,
                host_port: host_port.to_string(),
            });
        }
    }

    Err(ScriptError::RuntimeError(format!(
        "unsupported PAC result: {}",
        raw
    )))
}

fn install_pac_helpers(js_ctx: &rquickjs::Ctx<'_>) -> Result<()> {
    let globals = js_ctx.globals();

    globals
        .set(
            "isPlainHostName",
            Function::new(js_ctx.clone(), |host: String| -> bool {
                !host.contains('.')
            }),
        )
        .map_err(|e| ScriptError::QuickJsError(e.to_string()))?;
    globals
        .set(
            "dnsDomainIs",
            Function::new(js_ctx.clone(), |host: String, domain: String| -> bool {
                host.eq_ignore_ascii_case(domain.trim_start_matches('.'))
                    || host
                        .to_ascii_lowercase()
                        .ends_with(&domain.to_ascii_lowercase())
            }),
        )
        .map_err(|e| ScriptError::QuickJsError(e.to_string()))?;
    globals
        .set(
            "localHostOrDomainIs",
            Function::new(js_ctx.clone(), |host: String, hostdom: String| -> bool {
                host.eq_ignore_ascii_case(&hostdom)
                    || (!host.contains('.')
                        && hostdom
                            .to_ascii_lowercase()
                            .starts_with(&format!("{}.", host.to_ascii_lowercase())))
            }),
        )
        .map_err(|e| ScriptError::QuickJsError(e.to_string()))?;
    globals
        .set(
            "shExpMatch",
            Function::new(js_ctx.clone(), |value: String, pattern: String| -> bool {
                shell_match(&pattern, &value)
            }),
        )
        .map_err(|e| ScriptError::QuickJsError(e.to_string()))?;
    globals
        .set(
            "dnsResolve",
            Function::new(js_ctx.clone(), |host: String| -> String {
                resolve_first_ip(&host).unwrap_or_default()
            }),
        )
        .map_err(|e| ScriptError::QuickJsError(e.to_string()))?;
    globals
        .set(
            "isResolvable",
            Function::new(js_ctx.clone(), |host: String| -> bool {
                resolve_first_ip(&host).is_some()
            }),
        )
        .map_err(|e| ScriptError::QuickJsError(e.to_string()))?;
    globals
        .set(
            "isInNet",
            Function::new(
                js_ctx.clone(),
                |host: String, pattern: String, mask: String| -> bool {
                    is_in_net(&host, &pattern, &mask)
                },
            ),
        )
        .map_err(|e| ScriptError::QuickJsError(e.to_string()))?;
    globals
        .set(
            "myIpAddress",
            Function::new(js_ctx.clone(), || -> String { "127.0.0.1".to_string() }),
        )
        .map_err(|e| ScriptError::QuickJsError(e.to_string()))?;
    globals
        .set(
            "alert",
            Function::new(js_ctx.clone(), |_message: String| {}),
        )
        .map_err(|e| ScriptError::QuickJsError(e.to_string()))?;

    Ok(())
}

fn remove_dangerous_globals(js_ctx: &rquickjs::Ctx<'_>) {
    let globals = js_ctx.globals();
    for name in [
        "eval",
        "Function",
        "fetch",
        "XMLHttpRequest",
        "require",
        "process",
    ] {
        let _ = globals.remove(name);
    }
}

fn format_js_error(js_ctx: &rquickjs::Ctx<'_>, err: rquickjs::Error) -> String {
    let exc = js_ctx.catch();
    if exc.is_exception() || exc.is_error() {
        if let Some(obj) = exc.as_object() {
            let msg = obj.get::<_, String>("message").ok().unwrap_or_default();
            let stack = obj.get::<_, String>("stack").ok().unwrap_or_default();
            if !msg.is_empty() {
                if stack.is_empty() {
                    return format!("{}: {}", err, msg);
                }
                return format!("{}: {}\n{}", err, msg, stack);
            }
        }
    }
    err.to_string()
}

fn resolve_first_ip(host: &str) -> Option<String> {
    if let Ok(ip) = host.parse::<IpAddr>() {
        return Some(ip.to_string());
    }
    (host, 0)
        .to_socket_addrs()
        .ok()?
        .next()
        .map(|addr| addr.ip().to_string())
}

fn is_in_net(host: &str, pattern: &str, mask: &str) -> bool {
    let Some(ip) =
        resolve_first_ip(host).and_then(|value| value.parse::<std::net::Ipv4Addr>().ok())
    else {
        return false;
    };
    let Ok(pattern) = pattern.parse::<std::net::Ipv4Addr>() else {
        return false;
    };
    let Ok(mask) = mask.parse::<std::net::Ipv4Addr>() else {
        return false;
    };

    let ip = u32::from(ip);
    let pattern = u32::from(pattern);
    let mask = u32::from(mask);
    (ip & mask) == (pattern & mask)
}

fn shell_match(pattern: &str, value: &str) -> bool {
    shell_match_bytes(pattern.as_bytes(), value.as_bytes())
}

fn shell_match_bytes(pattern: &[u8], value: &[u8]) -> bool {
    if pattern.is_empty() {
        return value.is_empty();
    }
    match pattern[0] {
        b'*' => {
            shell_match_bytes(&pattern[1..], value)
                || (!value.is_empty() && shell_match_bytes(pattern, &value[1..]))
        }
        b'?' => !value.is_empty() && shell_match_bytes(&pattern[1..], &value[1..]),
        ch => !value.is_empty() && ch == value[0] && shell_match_bytes(&pattern[1..], &value[1..]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_direct_result() {
        assert_eq!(parse_pac_decision("DIRECT").unwrap(), PacDecision::Direct);
    }

    #[test]
    fn parses_proxy_result() {
        assert_eq!(
            parse_pac_decision("PROXY proxy.example:8080").unwrap(),
            PacDecision::Proxy {
                scheme: PacProxyScheme::Http,
                host_port: "proxy.example:8080".to_string()
            }
        );
    }

    #[test]
    fn parses_first_supported_candidate() {
        assert_eq!(
            parse_pac_decision("DIRECT; PROXY proxy.example:8080").unwrap(),
            PacDecision::Direct
        );
    }

    #[test]
    fn parses_proxy_scheme_variants_and_quotes() {
        assert_eq!(
            parse_pac_decision(r#"HTTPS "secure-proxy.example:8443""#).unwrap(),
            PacDecision::Proxy {
                scheme: PacProxyScheme::Https,
                host_port: "secure-proxy.example:8443".to_string()
            }
        );
        assert_eq!(
            parse_pac_decision("SOCKS 'socks-proxy.example:1080'").unwrap(),
            PacDecision::Proxy {
                scheme: PacProxyScheme::Socks,
                host_port: "socks-proxy.example:1080".to_string()
            }
        );
        assert_eq!(
            parse_pac_decision("SOCKS5 socks5-proxy.example:1080").unwrap(),
            PacDecision::Proxy {
                scheme: PacProxyScheme::Socks5,
                host_port: "socks5-proxy.example:1080".to_string()
            }
        );
    }

    #[test]
    fn proxy_scheme_maps_only_http_and_https_to_proxy_urls() {
        assert_eq!(PacProxyScheme::Http.as_proxy_url_scheme(), Some("http"));
        assert_eq!(PacProxyScheme::Https.as_proxy_url_scheme(), Some("https"));
        assert_eq!(PacProxyScheme::Socks.as_proxy_url_scheme(), None);
        assert_eq!(PacProxyScheme::Socks5.as_proxy_url_scheme(), None);
    }

    #[test]
    fn parse_rejects_missing_host_and_unsupported_results() {
        let missing = parse_pac_decision("PROXY").unwrap_err().to_string();
        assert!(missing.contains("missing host:port"));

        let unsupported = parse_pac_decision("; FOO bar ;").unwrap_err().to_string();
        assert!(unsupported.contains("unsupported PAC result"));
    }

    #[test]
    fn evaluates_find_proxy_for_url_with_helpers() {
        let engine = PacEngine::new(PacEngineConfig::default());
        let decision = engine
            .evaluate(
                r#"
function FindProxyForURL(url, host) {
  if (dnsDomainIs(host, ".example.com") && shExpMatch(url, "https://*.example.com/api/*")) {
    return "PROXY proxy.example:8080";
  }
  return "DIRECT";
}
"#,
                "https://www.example.com/api/v1",
                "www.example.com",
            )
            .unwrap();

        assert_eq!(
            decision,
            PacDecision::Proxy {
                scheme: PacProxyScheme::Http,
                host_port: "proxy.example:8080".to_string()
            }
        );
    }

    #[test]
    fn evaluates_direct_and_browser_global_sandboxing() {
        let engine = PacEngine::new(PacEngineConfig::default());
        let decision = engine
            .evaluate(
                r#"
function FindProxyForURL(url, host) {
  if (typeof eval !== "undefined") return "PROXY unsafe.example:1";
  if (typeof Function !== "undefined") return "PROXY unsafe.example:2";
  if (typeof fetch !== "undefined") return "PROXY unsafe.example:3";
  if (typeof XMLHttpRequest !== "undefined") return "PROXY unsafe.example:4";
  if (typeof require !== "undefined") return "PROXY unsafe.example:5";
  if (typeof process !== "undefined") return "PROXY unsafe.example:6";
  return "DIRECT";
}
"#,
                "https://plainhost/",
                "plainhost",
            )
            .unwrap();

        assert_eq!(decision, PacDecision::Direct);
    }

    #[test]
    fn evaluates_network_related_pac_helpers() {
        let engine = PacEngine::new(PacEngineConfig::default());
        let decision = engine
            .evaluate(
                r#"
function FindProxyForURL(url, host) {
  alert("ignored");
  if (!isPlainHostName("intranet")) return "DIRECT";
  if (!localHostOrDomainIs("www", "www.example.com")) return "DIRECT";
  if (dnsResolve("127.0.0.1") !== "127.0.0.1") return "DIRECT";
  if (!isResolvable("127.0.0.1")) return "DIRECT";
  if (!isInNet("127.0.0.1", "127.0.0.0", "255.0.0.0")) return "DIRECT";
  if (myIpAddress() !== "127.0.0.1") return "DIRECT";
  return "HTTPS secure-proxy.example:8443";
}
"#,
                "https://www.example.com/",
                "www.example.com",
            )
            .unwrap();

        assert_eq!(
            decision,
            PacDecision::Proxy {
                scheme: PacProxyScheme::Https,
                host_port: "secure-proxy.example:8443".to_string()
            }
        );
    }

    #[test]
    fn invalid_network_helpers_return_false_or_empty() {
        let engine = PacEngine::new(PacEngineConfig::default());
        let decision = engine
            .evaluate(
                r#"
function FindProxyForURL(url, host) {
  if (isInNet("::1", "127.0.0.0", "255.0.0.0")) return "PROXY bad.example:1";
  if (isInNet("127.0.0.1", "bad-pattern", "255.0.0.0")) return "PROXY bad.example:2";
  if (isInNet("127.0.0.1", "127.0.0.0", "bad-mask")) return "PROXY bad.example:3";
  return "DIRECT";
}
"#,
                "https://www.example.com/",
                "www.example.com",
            )
            .unwrap();

        assert_eq!(decision, PacDecision::Direct);
    }

    #[test]
    fn shell_match_supports_empty_star_question_and_literals() {
        assert!(shell_match("", ""));
        assert!(!shell_match("", "x"));
        assert!(shell_match(
            "*.example.com/api/??",
            "www.example.com/api/v1"
        ));
        assert!(!shell_match(
            "*.example.com/api/??",
            "www.example.com/api/v123"
        ));
    }

    #[test]
    fn rejects_oversized_script_before_runtime_creation() {
        let engine = PacEngine::new(PacEngineConfig::default());
        let script = " ".repeat(1024 * 1024 + 1);
        let err = engine
            .evaluate(&script, "https://example.com/", "example.com")
            .unwrap_err()
            .to_string();

        assert!(err.contains("exceeds 1 MiB"));
    }

    #[test]
    fn reports_javascript_errors() {
        let engine = PacEngine::new(PacEngineConfig::default());
        let err = engine
            .evaluate(
                r#"
function FindProxyForURL(url, host) {
  throw new Error("pac boom");
}
"#,
                "https://example.com/",
                "example.com",
            )
            .unwrap_err()
            .to_string();

        assert!(err.contains("pac boom"));
    }

    #[test]
    fn times_out_cpu_intensive_scripts() {
        let engine = PacEngine::new(PacEngineConfig {
            timeout_ms: 1,
            max_memory: DEFAULT_MAX_MEMORY,
        });
        let err = engine
            .evaluate(
                r#"
function FindProxyForURL(url, host) {
  while (true) {}
}
"#,
                "https://example.com/",
                "example.com",
            )
            .unwrap_err();

        assert!(matches!(err, ScriptError::Timeout(_)));
    }

    #[test]
    fn rejects_missing_find_proxy_for_url() {
        let engine = PacEngine::new(PacEngineConfig::default());
        let err = engine.evaluate("var x = 1;", "https://example.com/", "example.com");
        assert!(err.is_err());
    }
}
