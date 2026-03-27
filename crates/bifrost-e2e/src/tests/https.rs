use crate::curl::CurlCommand;
use crate::mock::{EnhancedMockServer, HttpbinMockServer, HttpsMockServer};
use crate::proxy::ProxyInstance;
use crate::runner::TestCase;
use std::time::Duration;

pub fn get_all_tests() -> Vec<TestCase> {
    vec![
        TestCase::standalone(
            "https_tunnel_passthrough",
            "HTTPS tunnel direct passthrough (no interception)",
            "https",
            test_https_tunnel_passthrough,
        ),
        TestCase::standalone(
            "https_tunnel_with_host_rule",
            "HTTPS tunnel with host redirect rule",
            "https",
            test_https_tunnel_with_host_rule,
        ),
        TestCase::standalone(
            "https_curl_insecure_flag",
            "HTTPS with curl -k insecure flag",
            "https",
            test_https_curl_insecure_flag,
        ),
        TestCase::standalone(
            "https_tunnel_public_site",
            "HTTPS tunnel to public site (httpbin)",
            "https",
            test_https_tunnel_public_site,
        ),
        TestCase::standalone(
            "https_without_ca_should_fail",
            "HTTPS without CA cert should fail verification",
            "https",
            test_https_without_ca_should_fail,
        ),
        TestCase::standalone(
            "https_with_ca_should_pass",
            "HTTPS with CA cert should pass verification",
            "https",
            test_https_with_ca_should_pass,
        ),
        TestCase::standalone(
            "https_cert_hostname_validation",
            "HTTPS certificate hostname validation",
            "https",
            test_https_cert_hostname_validation,
        ),
        TestCase::standalone(
            "https_interception_rules_applied",
            "HTTPS interception with rules applied",
            "https",
            test_https_interception_rules_applied,
        ),
    ]
}

async fn test_https_tunnel_passthrough() -> Result<(), String> {
    let mock = HttpbinMockServer::start().await;
    let port = portpicker::pick_unused_port().unwrap();
    let rules = mock.http_rules();
    let rule_refs: Vec<&str> = rules.iter().map(String::as_str).collect();
    let _proxy = ProxyInstance::start(port, rule_refs)
        .await
        .map_err(|e| format!("Failed to start proxy: {}", e))?;

    tokio::time::sleep(Duration::from_millis(100)).await;

    let result = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "https://httpbin.org/get",
    )
    .insecure()
    .connect_timeout(10)
    .max_time(30)
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result.assert_success()?;
    result.assert_body_contains("\"Host\":\"httpbin.org\"")?;
    result.assert_body_contains("\"User-Agent\":\"curl/")?;

    Ok(())
}

async fn test_https_tunnel_with_host_rule() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;
    mock.set_response(200, "https_redirected");

    let port = portpicker::pick_unused_port().unwrap();
    let _proxy = ProxyInstance::start(
        port,
        vec![&format!("secure.test.local host://127.0.0.1:{}", mock.port)],
    )
    .await
    .map_err(|e| format!("Failed to start proxy: {}", e))?;

    tokio::time::sleep(Duration::from_millis(100)).await;

    let result = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "http://secure.test.local/api",
    )
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result.assert_success()?;
    result.assert_body_contains("https_redirected")?;

    mock.assert_request_received()?;

    Ok(())
}

async fn test_https_curl_insecure_flag() -> Result<(), String> {
    let mock = HttpsMockServer::start("insecure-flag.test").await;
    mock.set_response(200, "insecure_flag_ok");

    let port = portpicker::pick_unused_port().unwrap();
    let _proxy = ProxyInstance::start(
        port,
        vec![&format!(
            "insecure-flag.test host://127.0.0.1:{}",
            mock.port
        )],
    )
    .await
    .map_err(|e| format!("Failed to start proxy: {}", e))?;

    tokio::time::sleep(Duration::from_millis(100)).await;

    let result = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "https://insecure-flag.test/headers",
    )
    .insecure()
    .connect_timeout(10)
    .max_time(30)
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result.assert_success()?;
    result.assert_body_contains("insecure_flag_ok")?;

    mock.assert_request_received()?;

    Ok(())
}

async fn test_https_tunnel_public_site() -> Result<(), String> {
    let mock = HttpbinMockServer::start().await;
    let port = portpicker::pick_unused_port().unwrap();
    let rules = mock.http_rules();
    let rule_refs: Vec<&str> = rules.iter().map(String::as_str).collect();
    let _proxy = ProxyInstance::start(port, rule_refs)
        .await
        .map_err(|e| format!("Failed to start proxy: {}", e))?;

    tokio::time::sleep(Duration::from_millis(100)).await;

    let result = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "https://httpbin.org/user-agent",
    )
    .insecure()
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result.assert_success()?;
    result.assert_body_contains("user-agent")?;

    Ok(())
}

async fn test_https_without_ca_should_fail() -> Result<(), String> {
    Err("SKIPPED: TLS interception not yet implemented - requires CA setup".to_string())
}

async fn test_https_with_ca_should_pass() -> Result<(), String> {
    Err("SKIPPED: TLS interception not yet implemented - requires CA setup".to_string())
}

async fn test_https_cert_hostname_validation() -> Result<(), String> {
    Err("SKIPPED: TLS interception not yet implemented - requires CA setup".to_string())
}

async fn test_https_interception_rules_applied() -> Result<(), String> {
    Err("SKIPPED: TLS interception not yet implemented - requires CA setup".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore]
    async fn test_https_passthrough() {
        let result = test_https_tunnel_passthrough().await;
        assert!(result.is_ok(), "Test failed: {:?}", result.err());
    }

    #[tokio::test]
    async fn test_https_host_rule() {
        let result = test_https_tunnel_with_host_rule().await;
        assert!(result.is_ok(), "Test failed: {:?}", result.err());
    }
}
