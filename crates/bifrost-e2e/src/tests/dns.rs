use crate::curl::CurlCommand;
use crate::mock::{EnhancedMockServer, HttpsMockServer, MockDnsServer};
use crate::proxy::ProxyInstance;
use crate::runner::TestCase;
use bifrost_admin::AdminState;
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr};
use std::sync::Arc;
use std::time::Duration;

const START_PROXY_MAX_ATTEMPTS: usize = 5;

pub fn get_all_tests() -> Vec<TestCase> {
    vec![
        TestCase::standalone(
            "dns_basic_rule_parsing",
            "DNS protocol: rule parsing and resolution",
            "dns",
            test_dns_basic_rule_parsing,
        ),
        TestCase::standalone(
            "dns_with_host_protocol",
            "DNS protocol: combined with host protocol (host takes priority)",
            "dns",
            test_dns_with_host_protocol,
        ),
        TestCase::standalone(
            "dns_wildcard_domain",
            "DNS protocol: wildcard domain matching",
            "dns",
            test_dns_wildcard_domain,
        ),
        TestCase::standalone(
            "dns_multiple_servers",
            "DNS protocol: multiple DNS servers configuration",
            "dns",
            test_dns_multiple_servers,
        ),
        TestCase::standalone(
            "dns_rule_priority",
            "DNS protocol: first matching rule takes priority",
            "dns",
            test_dns_rule_priority,
        ),
        TestCase::standalone(
            "dns_http_forward_custom_resolver",
            "DNS protocol: plain HTTP forwarding uses custom resolver end-to-end",
            "dns",
            test_dns_http_forward_custom_resolver,
        ),
        TestCase::standalone(
            "dns_https_tunnel_custom_resolver",
            "DNS protocol: HTTPS CONNECT tunnel uses custom resolver end-to-end",
            "dns",
            test_dns_https_tunnel_custom_resolver,
        ),
        TestCase::standalone(
            "dns_https_intercept_custom_resolver",
            "DNS protocol: HTTPS interception upstream uses custom resolver end-to-end",
            "dns",
            test_dns_https_intercept_custom_resolver,
        ),
    ]
}

async fn test_dns_basic_rule_parsing() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;
    mock.set_response(200, "dns_rule_ok");

    let (port, _proxy) = start_proxy_with_owned_rules(vec![
        "test.local dns://8.8.8.8".to_string(),
        format!("test.local host://127.0.0.1:{}", mock.port),
    ])
    .await?;

    tokio::time::sleep(Duration::from_millis(100)).await;

    let result = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "http://test.local/api/test",
    )
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result.assert_success()?;
    result.assert_body_contains("dns_rule_ok")?;

    Ok(())
}

async fn test_dns_with_host_protocol() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;
    mock.set_response(200, "host_priority_ok");

    let (port, _proxy) = start_proxy_with_owned_rules(vec![
        "api.example.com dns://8.8.8.8".to_string(),
        format!("api.example.com host://127.0.0.1:{}", mock.port),
    ])
    .await?;

    tokio::time::sleep(Duration::from_millis(100)).await;

    let result = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "http://api.example.com/api",
    )
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result.assert_success()?;
    result.assert_body_contains("host_priority_ok")?;

    Ok(())
}

async fn test_dns_wildcard_domain() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;
    mock.set_response(200, "wildcard_dns_ok");

    let (port, _proxy) = start_proxy_with_owned_rules(vec![
        "*.example.com dns://8.8.8.8,8.8.4.4".to_string(),
        format!("*.example.com host://127.0.0.1:{}", mock.port),
    ])
    .await?;

    tokio::time::sleep(Duration::from_millis(100)).await;

    let result = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "http://sub1.example.com/api",
    )
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result.assert_success()?;
    result.assert_body_contains("wildcard_dns_ok")?;

    let result2 = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "http://sub2.example.com/api",
    )
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result2.assert_success()?;
    result2.assert_body_contains("wildcard_dns_ok")?;

    Ok(())
}

async fn test_dns_multiple_servers() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;
    mock.set_response(200, "multi_dns_ok");

    let (port, _proxy) = start_proxy_with_owned_rules(vec![
        "test.local dns://8.8.8.8,8.8.4.4,1.1.1.1".to_string(),
        format!("test.local host://127.0.0.1:{}", mock.port),
    ])
    .await?;

    tokio::time::sleep(Duration::from_millis(100)).await;

    let result = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "http://test.local/api",
    )
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result.assert_success()?;
    result.assert_body_contains("multi_dns_ok")?;

    Ok(())
}

async fn test_dns_rule_priority() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;
    mock.set_response(200, "priority_ok");

    let (port, _proxy) = start_proxy_with_owned_rules(vec![
        "test.local dns://8.8.8.8".to_string(),
        "test.local dns://1.1.1.1".to_string(),
        format!("test.local host://127.0.0.1:{}", mock.port),
    ])
    .await?;

    tokio::time::sleep(Duration::from_millis(100)).await;

    let result = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "http://test.local/api",
    )
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result.assert_success()?;
    result.assert_body_contains("priority_ok")?;

    Ok(())
}

async fn test_dns_http_forward_custom_resolver() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;
    mock.set_response(200, "dns_http_forward_ok");

    let dns = MockDnsServer::start(HashMap::from([(
        "mapped.test".to_string(),
        IpAddr::V4(Ipv4Addr::LOCALHOST),
    )]))
    .await;

    let dns_rule = format!("mapped.test dns://{}", dns.server());
    let (port, _proxy) = start_proxy_with_owned_rules(vec![dns_rule]).await?;

    tokio::time::sleep(Duration::from_millis(100)).await;

    let result = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        &format!("http://mapped.test:{}/api/http-dns", mock.port),
    )
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result.assert_success()?;
    result.assert_body_contains("dns_http_forward_ok")?;
    dns.assert_query_received("mapped.test")?;
    mock.assert_request_received()?;

    Ok(())
}

async fn test_dns_https_tunnel_custom_resolver() -> Result<(), String> {
    let https_mock = HttpsMockServer::start("mapped.test").await;
    https_mock.set_response(200, "dns_https_tunnel_ok");

    let dns = MockDnsServer::start(HashMap::from([(
        "mapped.test".to_string(),
        IpAddr::V4(Ipv4Addr::LOCALHOST),
    )]))
    .await;

    let dns_rule = format!("mapped.test dns://{}", dns.server());
    let (port, _proxy) = start_proxy_with_owned_rules(vec![dns_rule]).await?;

    tokio::time::sleep(Duration::from_millis(100)).await;

    let result = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        &format!(
            "https://mapped.test:{}/api/https-tunnel-dns",
            https_mock.port
        ),
    )
    .insecure()
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result.assert_success()?;
    result.assert_body_contains("dns_https_tunnel_ok")?;
    dns.assert_query_received("mapped.test")?;
    https_mock.assert_request_received()?;

    Ok(())
}

async fn test_dns_https_intercept_custom_resolver() -> Result<(), String> {
    let https_mock = HttpsMockServer::start("mapped.test").await;
    https_mock.set_response(200, "dns_https_intercept_ok");

    let dns = MockDnsServer::start(HashMap::from([(
        "mapped.test".to_string(),
        IpAddr::V4(Ipv4Addr::LOCALHOST),
    )]))
    .await;

    let dns_rule = format!("mapped.test dns://{}", dns.server());
    let (port, _proxy, _admin_state) = start_proxy_with_admin_rules(vec![dns_rule]).await?;

    tokio::time::sleep(Duration::from_millis(100)).await;

    let result = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        &format!("https://mapped.test:{}/api/https-mitm-dns", https_mock.port),
    )
    .insecure()
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result.assert_success()?;
    result.assert_body_contains("dns_https_intercept_ok")?;
    dns.assert_query_received("mapped.test")?;
    https_mock.assert_request_received()?;

    Ok(())
}

async fn start_proxy_with_owned_rules(rules: Vec<String>) -> Result<(u16, ProxyInstance), String> {
    for attempt in 1..=START_PROXY_MAX_ATTEMPTS {
        let port = portpicker::pick_unused_port().ok_or("Failed to pick unused port")?;
        let rule_refs: Vec<&str> = rules.iter().map(String::as_str).collect();
        match ProxyInstance::start(port, rule_refs).await {
            Ok(proxy) => return Ok((port, proxy)),
            Err(e) if is_bind_race(&e.to_string()) && attempt < START_PROXY_MAX_ATTEMPTS => {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            Err(e) => return Err(format!("Failed to start proxy: {e}")),
        }
    }

    Err("Failed to start proxy after retrying port bind races".to_string())
}

async fn start_proxy_with_admin_rules(
    rules: Vec<String>,
) -> Result<(u16, ProxyInstance, Arc<AdminState>), String> {
    for attempt in 1..=START_PROXY_MAX_ATTEMPTS {
        let port = portpicker::pick_unused_port().ok_or("Failed to pick unused port")?;
        let rule_refs: Vec<&str> = rules.iter().map(String::as_str).collect();
        match ProxyInstance::start_with_admin(port, rule_refs, true, true).await {
            Ok((proxy, admin_state)) => return Ok((port, proxy, admin_state)),
            Err(e) if is_bind_race(&e.to_string()) && attempt < START_PROXY_MAX_ATTEMPTS => {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            Err(e) => return Err(format!("Failed to start proxy: {e}")),
        }
    }

    Err("Failed to start proxy after retrying port bind races".to_string())
}

fn is_bind_race(error: &str) -> bool {
    error.contains("Failed to bind") || error.contains("already listening on this port")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_dns_basic() {
        let result = test_dns_basic_rule_parsing().await;
        assert!(result.is_ok(), "Test failed: {:?}", result.err());
    }

    #[tokio::test]
    async fn test_dns_with_host() {
        let result = test_dns_with_host_protocol().await;
        assert!(result.is_ok(), "Test failed: {:?}", result.err());
    }

    #[tokio::test]
    async fn test_dns_wildcard() {
        let result = test_dns_wildcard_domain().await;
        assert!(result.is_ok(), "Test failed: {:?}", result.err());
    }

    #[tokio::test]
    async fn test_dns_http_forward_custom_dns() {
        let result = test_dns_http_forward_custom_resolver().await;
        assert!(result.is_ok(), "Test failed: {:?}", result.err());
    }

    #[tokio::test]
    async fn test_dns_https_tunnel_custom_dns() {
        let result = test_dns_https_tunnel_custom_resolver().await;
        assert!(result.is_ok(), "Test failed: {:?}", result.err());
    }

    #[tokio::test]
    async fn test_dns_https_intercept_custom_dns() {
        let result = test_dns_https_intercept_custom_resolver().await;
        assert!(result.is_ok(), "Test failed: {:?}", result.err());
    }
}
