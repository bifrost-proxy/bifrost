use crate::curl::CurlCommand;
use crate::mock::{EnhancedMockServer, ProxyEchoServer};
use crate::proxy::ProxyInstance;
use crate::runner::TestCase;
use bifrost_core::{UserPassAccountConfig, UserPassAuthConfig};
use std::time::Duration;

const START_PROXY_MAX_ATTEMPTS: usize = 10;

macro_rules! start_proxy_with_rules {
    ($($rule:expr),+ $(,)?) => {{
        start_proxy_with_owned_rules(vec![$($rule.to_string()),+]).await
    }};
}

pub fn get_all_tests() -> Vec<TestCase> {
    vec![
        TestCase::standalone(
            "routing_host_basic",
            "Host protocol: basic redirect to target server",
            "routing",
            test_routing_host_basic,
        ),
        TestCase::standalone(
            "routing_host_with_port",
            "Host protocol: redirect with port matching",
            "routing",
            test_routing_host_with_port,
        ),
        TestCase::standalone(
            "routing_host_path_matching",
            "Host protocol: path preserved after redirect",
            "routing",
            test_routing_host_path_matching,
        ),
        TestCase::standalone(
            "routing_xhost_priority",
            "XHost protocol: priority over host",
            "routing",
            test_routing_xhost_priority,
        ),
        TestCase::standalone(
            "routing_redirect_302",
            "Redirect protocol: 302 redirect",
            "routing",
            test_routing_redirect_302,
        ),
        TestCase::standalone(
            "routing_redirect_301",
            "Redirect protocol: 301 permanent redirect",
            "routing",
            test_routing_redirect_301,
        ),
        TestCase::standalone(
            "routing_file_inline",
            "File protocol: inline content response",
            "routing",
            test_routing_file_inline,
        ),
        TestCase::standalone(
            "routing_file_json",
            "File protocol: JSON response",
            "routing",
            test_routing_file_json,
        ),
        TestCase::standalone(
            "routing_tpl_template",
            "Tpl protocol: template rendering",
            "routing",
            test_routing_tpl_template,
        ),
        TestCase::standalone(
            "routing_rawfile_inline",
            "Rawfile protocol: raw HTTP response",
            "routing",
            test_routing_rawfile_inline,
        ),
        TestCase::standalone(
            "routing_multiple_host_order",
            "Host protocol: first rule takes priority",
            "routing",
            test_routing_multiple_host_order,
        ),
        TestCase::standalone(
            "routing_host_vs_proxy",
            "Host vs Proxy: host takes priority",
            "routing",
            test_routing_host_vs_proxy,
        ),
        TestCase::standalone(
            "routing_proxy_chain_with_auth",
            "Proxy protocol: chain to another bifrost proxy with auth",
            "routing",
            test_routing_proxy_chain_with_auth,
        ),
        TestCase::standalone(
            "routing_host_preserve_query",
            "Host protocol: preserve query string",
            "routing",
            test_routing_host_preserve_query,
        ),
        TestCase::standalone(
            "routing_host_with_path_prefix",
            "Host protocol: path prefix matching",
            "routing",
            test_routing_host_with_path_prefix,
        ),
        TestCase::standalone(
            "routing_statuscode_with_file",
            "Combined: statusCode + file response",
            "routing",
            test_routing_statuscode_with_file,
        ),
        TestCase::standalone(
            "routing_proxy_chain_upstream_auth_correct",
            "Proxy chain: upstream requires auth, correct credentials succeed",
            "routing",
            test_routing_proxy_chain_upstream_auth_correct,
        ),
        TestCase::standalone(
            "routing_proxy_chain_upstream_auth_wrong",
            "Proxy chain: upstream requires auth, wrong credentials get 407",
            "routing",
            test_routing_proxy_chain_upstream_auth_wrong,
        ),
        TestCase::standalone(
            "routing_proxy_chain_upstream_auth_missing",
            "Proxy chain: upstream requires auth, no credentials get 407",
            "routing",
            test_routing_proxy_chain_upstream_auth_missing,
        ),
    ]
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

async fn start_proxy_with_userpass(
    rules: Vec<String>,
    userpass_auth: UserPassAuthConfig,
) -> Result<(u16, ProxyInstance), String> {
    for attempt in 1..=START_PROXY_MAX_ATTEMPTS {
        let port = portpicker::pick_unused_port().ok_or("Failed to pick unused port")?;
        let rule_refs: Vec<&str> = rules.iter().map(String::as_str).collect();
        match ProxyInstance::start_with_userpass(port, rule_refs, userpass_auth.clone()).await {
            Ok(proxy) => return Ok((port, proxy)),
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

async fn test_routing_host_basic() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;
    mock.set_response(200, "host_basic_ok");

    let (port, _proxy) =
        start_proxy_with_rules!(format!("test.local host://127.0.0.1:{}", mock.port))?;

    tokio::time::sleep(Duration::from_millis(100)).await;

    let result = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "http://test.local/api/test",
    )
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result.assert_success()?;
    result.assert_body_contains("host_basic_ok")?;
    mock.assert_path("/api/test")?;

    Ok(())
}

async fn test_routing_host_with_port() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;
    mock.set_response(200, "port_match_ok");

    let (port, _proxy) =
        start_proxy_with_rules!(format!("test.local:8080 host://127.0.0.1:{}", mock.port))?;

    tokio::time::sleep(Duration::from_millis(100)).await;

    let result = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "http://test.local:8080/api",
    )
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result.assert_success()?;
    result.assert_body_contains("port_match_ok")?;

    Ok(())
}

async fn test_routing_host_path_matching() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;
    mock.set_response(200, "path_match_ok");

    let (port, _proxy) =
        start_proxy_with_rules!(format!("test.local/api host://127.0.0.1:{}", mock.port))?;

    tokio::time::sleep(Duration::from_millis(100)).await;

    let result = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "http://test.local/api/users/123",
    )
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result.assert_success()?;
    result.assert_body_contains("path_match_ok")?;
    mock.assert_path("/api/users/123")?;

    Ok(())
}

async fn test_routing_xhost_priority() -> Result<(), String> {
    let mock1 = EnhancedMockServer::start().await;
    mock1.set_response(200, "host_server");

    let mock2 = EnhancedMockServer::start().await;
    mock2.set_response(200, "xhost_server");

    let (port, _proxy) = start_proxy_with_rules!(
        format!("test.local host://127.0.0.1:{}", mock1.port),
        format!("test.local xhost://127.0.0.1:{}", mock2.port),
    )?;

    tokio::time::sleep(Duration::from_millis(100)).await;

    let result = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "http://test.local/api",
    )
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result.assert_success()?;
    result.assert_body_contains("xhost_server")?;

    Ok(())
}

async fn test_routing_redirect_302() -> Result<(), String> {
    let (port, _proxy) = start_proxy_with_rules!("test.local redirect://http://new.example.com/")?;

    tokio::time::sleep(Duration::from_millis(100)).await;

    let result = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "http://test.local/old-path",
    )
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result.assert_status(302)?;
    result.assert_header("Location", "http://new.example.com/")?;

    Ok(())
}

async fn test_routing_redirect_301() -> Result<(), String> {
    let (port, _proxy) =
        start_proxy_with_rules!("test.local redirect://http://example.com/permanent?301")?;

    tokio::time::sleep(Duration::from_millis(100)).await;

    let result = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "http://test.local/temp",
    )
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result.assert_status(301)?;
    result.assert_header_contains("Location", "example.com/permanent")?;

    Ok(())
}

async fn test_routing_file_inline() -> Result<(), String> {
    let (port, _proxy) = start_proxy_with_rules!("test.local file://(inline_content_test)")?;

    tokio::time::sleep(Duration::from_millis(100)).await;

    let result = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "http://test.local/any",
    )
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result.assert_success()?;
    result.assert_body_contains("inline_content_test")?;

    Ok(())
}

async fn test_routing_file_json() -> Result<(), String> {
    let (port, _proxy) =
        start_proxy_with_rules!(r#"test.local file://({"status":"ok","code":200})"#)?;

    tokio::time::sleep(Duration::from_millis(100)).await;

    let result = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "http://test.local/api/status",
    )
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result.assert_success()?;
    result.assert_body_contains(r#""status":"ok""#)?;
    result.assert_body_contains(r#""code":200"#)?;

    Ok(())
}

async fn test_routing_tpl_template() -> Result<(), String> {
    let (port, _proxy) =
        start_proxy_with_rules!(r#"test.local tpl://({"timestamp":{{now}},"host":"{{host}}"})"#)?;

    tokio::time::sleep(Duration::from_millis(100)).await;

    let result = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "http://test.local/tpl",
    )
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result.assert_success()?;
    result.assert_body_contains("timestamp")?;
    result.assert_body_contains("test.local")?;

    Ok(())
}

async fn test_routing_rawfile_inline() -> Result<(), String> {
    let raw_response = "HTTP/1.1 201 Created\r\nX-Custom:rawfile\r\nContent-Type:text/plain\r\n\r\nRaw Response Body";
    let (port, _proxy) = start_proxy_with_rules!(format!(
        "test.local rawfile://({})",
        raw_response.replace("\r\n", "\\r\\n")
    ))?;

    tokio::time::sleep(Duration::from_millis(100)).await;

    let result = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "http://test.local/raw",
    )
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result.assert_status(201)?;
    result.assert_body_contains("Raw Response Body")?;

    Ok(())
}

async fn test_routing_multiple_host_order() -> Result<(), String> {
    let mock1 = EnhancedMockServer::start().await;
    mock1.set_response(200, "first_server");

    let mock2 = EnhancedMockServer::start().await;
    mock2.set_response(200, "second_server");

    let (port, _proxy) = start_proxy_with_rules!(
        format!("test.local host://127.0.0.1:{}", mock1.port),
        format!("test.local host://127.0.0.1:{}", mock2.port),
    )?;

    tokio::time::sleep(Duration::from_millis(100)).await;

    let result = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "http://test.local/api",
    )
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result.assert_success()?;
    result.assert_body_contains("first_server")?;

    Ok(())
}

async fn test_routing_host_vs_proxy() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;
    mock.set_response(200, "host_wins");

    let proxy_port = portpicker::pick_unused_port().unwrap();
    let (port, _proxy) = start_proxy_with_rules!(
        format!("test.local host://127.0.0.1:{}", mock.port),
        format!("test.local proxy://127.0.0.1:{}", proxy_port),
    )?;

    tokio::time::sleep(Duration::from_millis(100)).await;

    let result = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "http://test.local/api",
    )
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result.assert_success()?;
    result.assert_body_contains("host_wins")?;

    Ok(())
}

async fn test_routing_proxy_chain_with_auth() -> Result<(), String> {
    let proxy_echo = ProxyEchoServer::start().await;
    proxy_echo.set_response(200, "proxy_chain_ok");

    let (upstream_port, _upstream_proxy) =
        start_proxy_with_rules!(format!("chain.test host://127.0.0.1:{}", proxy_echo.port()))?;

    let (entry_port, _entry_proxy) = start_proxy_with_rules!(format!(
        "chain.test proxy://user:pass@127.0.0.1:{}",
        upstream_port
    ))?;

    tokio::time::sleep(Duration::from_millis(100)).await;

    let result = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", entry_port),
        "http://chain.test/api?via=entry",
    )
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result.assert_success()?;
    result.assert_body_contains("proxy_chain_ok")?;
    proxy_echo.assert_path("/api")?;
    proxy_echo.assert_proxy_auth_received("Basic dXNlcjpwYXNz")?;

    let request = proxy_echo
        .last_request()
        .ok_or_else(|| "No request received by final mock server".to_string())?;
    let query = request
        .query
        .ok_or_else(|| "Query string missing from chained request".to_string())?;
    if !query.contains("via=entry") {
        return Err(format!(
            "Unexpected query string after proxy chain: {}",
            query
        ));
    }

    Ok(())
}

async fn test_routing_host_preserve_query() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;
    mock.set_response(200, "query_ok");

    let (port, _proxy) =
        start_proxy_with_rules!(format!("test.local host://127.0.0.1:{}", mock.port))?;

    tokio::time::sleep(Duration::from_millis(100)).await;

    let result = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "http://test.local/api?foo=bar&baz=123",
    )
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result.assert_success()?;

    let req = mock.last_request().ok_or("No request received")?;
    if let Some(query) = &req.query {
        if !query.contains("foo=bar") || !query.contains("baz=123") {
            return Err(format!("Query string not preserved: {}", query));
        }
    } else {
        return Err("Query string missing".to_string());
    }

    Ok(())
}

async fn test_routing_host_with_path_prefix() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;
    mock.set_response(200, "path_prefix_ok");

    let (port, _proxy) =
        start_proxy_with_rules!(format!("test.local/v1 host://127.0.0.1:{}/v2", mock.port))?;

    tokio::time::sleep(Duration::from_millis(100)).await;

    let result = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "http://test.local/v1/users",
    )
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result.assert_success()?;
    result.assert_body_contains("path_prefix_ok")?;

    Ok(())
}

async fn test_routing_statuscode_with_file() -> Result<(), String> {
    let (port, _proxy) = start_proxy_with_rules!(
        r#"test.local statusCode://404"#,
        r#"test.local file://({"error":"not_found"})"#,
    )?;

    tokio::time::sleep(Duration::from_millis(100)).await;

    let result = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "http://test.local/missing",
    )
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result.assert_status(404)?;
    result.assert_body_contains("not_found")?;

    Ok(())
}

fn make_upstream_auth_config() -> UserPassAuthConfig {
    UserPassAuthConfig {
        enabled: true,
        accounts: vec![UserPassAccountConfig {
            username: "proxyuser".to_string(),
            password: Some("proxypass".to_string()),
            enabled: true,
        }],
        loopback_requires_auth: true,
    }
}

async fn test_routing_proxy_chain_upstream_auth_correct() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;
    mock.set_response(200, "upstream_auth_ok");

    let (upstream_port, _upstream_proxy) = start_proxy_with_userpass(
        vec![format!("authchain.test host://127.0.0.1:{}", mock.port)],
        make_upstream_auth_config(),
    )
    .await?;

    let (entry_port, _entry_proxy) = start_proxy_with_rules!(format!(
        "authchain.test proxy://proxyuser:proxypass@127.0.0.1:{}",
        upstream_port
    ))?;

    tokio::time::sleep(Duration::from_millis(200)).await;

    let result = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", entry_port),
        "http://authchain.test/api/data?key=value",
    )
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result.assert_success()?;
    result.assert_body_contains("upstream_auth_ok")?;

    let req = mock
        .last_request()
        .ok_or_else(|| "No request received by mock server".to_string())?;
    if req.path != "/api/data" {
        return Err(format!("Expected path /api/data, got: {}", req.path));
    }
    let query = req
        .query
        .ok_or_else(|| "Query string missing".to_string())?;
    if !query.contains("key=value") {
        return Err(format!("Query string not preserved: {}", query));
    }

    Ok(())
}

async fn test_routing_proxy_chain_upstream_auth_wrong() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;
    mock.set_response(200, "should_not_reach");

    let (upstream_port, _upstream_proxy) = start_proxy_with_userpass(
        vec![format!(
            "authchain-wrong.test host://127.0.0.1:{}",
            mock.port
        )],
        make_upstream_auth_config(),
    )
    .await?;

    let (entry_port, _entry_proxy) = start_proxy_with_rules!(format!(
        "authchain-wrong.test proxy://proxyuser:wrongpass@127.0.0.1:{}",
        upstream_port
    ))?;

    tokio::time::sleep(Duration::from_millis(200)).await;

    let result = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", entry_port),
        "http://authchain-wrong.test/api/data",
    )
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result.assert_status(407)?;

    Ok(())
}

async fn test_routing_proxy_chain_upstream_auth_missing() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;
    mock.set_response(200, "should_not_reach");

    let (upstream_port, _upstream_proxy) = start_proxy_with_userpass(
        vec![format!(
            "authchain-none.test host://127.0.0.1:{}",
            mock.port
        )],
        make_upstream_auth_config(),
    )
    .await?;

    let (entry_port, _entry_proxy) = start_proxy_with_rules!(format!(
        "authchain-none.test proxy://127.0.0.1:{}",
        upstream_port
    ))?;

    tokio::time::sleep(Duration::from_millis(200)).await;

    let result = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", entry_port),
        "http://authchain-none.test/api/data",
    )
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result.assert_status(407)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_host_basic() {
        let result = test_routing_host_basic().await;
        assert!(result.is_ok(), "Test failed: {:?}", result.err());
    }

    #[tokio::test]
    async fn test_xhost_priority() {
        let result = test_routing_xhost_priority().await;
        assert!(result.is_ok(), "Test failed: {:?}", result.err());
    }

    #[tokio::test]
    async fn test_redirect_302() {
        let result = test_routing_redirect_302().await;
        assert!(result.is_ok(), "Test failed: {:?}", result.err());
    }

    #[tokio::test]
    async fn test_file_inline() {
        let result = test_routing_file_inline().await;
        assert!(result.is_ok(), "Test failed: {:?}", result.err());
    }

    #[tokio::test]
    async fn test_multiple_host_order() {
        let result = test_routing_multiple_host_order().await;
        assert!(result.is_ok(), "Test failed: {:?}", result.err());
    }

    #[tokio::test]
    async fn test_proxy_chain_upstream_auth_correct() {
        let result = test_routing_proxy_chain_upstream_auth_correct().await;
        assert!(result.is_ok(), "Test failed: {:?}", result.err());
    }

    #[tokio::test]
    async fn test_proxy_chain_upstream_auth_wrong() {
        let result = test_routing_proxy_chain_upstream_auth_wrong().await;
        assert!(result.is_ok(), "Test failed: {:?}", result.err());
    }

    #[tokio::test]
    async fn test_proxy_chain_upstream_auth_missing() {
        let result = test_routing_proxy_chain_upstream_auth_missing().await;
        assert!(result.is_ok(), "Test failed: {:?}", result.err());
    }
}
