use crate::curl::CurlCommand;
use crate::mock::EnhancedMockServer;
use crate::proxy::ProxyInstance;
use crate::runner::TestCase;

pub fn get_all_tests() -> Vec<TestCase> {
    vec![
        TestCase::standalone(
            "priority_host_order",
            "Priority: first host rule takes effect",
            "rule_priority",
            test_priority_host_order,
        ),
        TestCase::standalone(
            "priority_xhost_over_host",
            "Priority: xhost overrides host",
            "rule_priority",
            test_priority_xhost_over_host,
        ),
        TestCase::standalone(
            "priority_host_vs_proxy",
            "Priority: host takes precedence over proxy",
            "rule_priority",
            test_priority_host_vs_proxy,
        ),
        TestCase::standalone(
            "priority_header_override",
            "Priority: later header overrides earlier (same key)",
            "rule_priority",
            test_priority_header_override,
        ),
        TestCase::standalone(
            "priority_header_merge",
            "Priority: different headers are merged",
            "rule_priority",
            test_priority_header_merge,
        ),
        TestCase::standalone(
            "priority_cookie_override",
            "Priority: later cookie overrides earlier (same key)",
            "rule_priority",
            test_priority_cookie_override,
        ),
        TestCase::standalone(
            "priority_cookie_merge",
            "Priority: different cookies are merged",
            "rule_priority",
            test_priority_cookie_merge,
        ),
        TestCase::standalone(
            "priority_urlparams_override",
            "Priority: later urlParams overrides earlier (same key)",
            "rule_priority",
            test_priority_urlparams_override,
        ),
        TestCase::standalone(
            "priority_urlparams_merge",
            "Priority: different urlParams are merged",
            "rule_priority",
            test_priority_urlparams_merge,
        ),
        TestCase::standalone(
            "priority_resbody_last_wins",
            "Priority: last resBody rule wins",
            "rule_priority",
            test_priority_resbody_last_wins,
        ),
        TestCase::standalone(
            "priority_forward_with_modify",
            "Priority: forward and modify rules both apply",
            "rule_priority",
            test_priority_forward_with_modify,
        ),
        TestCase::standalone(
            "priority_mixed_rules",
            "Priority: complex mixed rules scenario",
            "rule_priority",
            test_priority_mixed_rules,
        ),
    ]
}

async fn test_priority_host_order() -> Result<(), String> {
    let mock1 = EnhancedMockServer::start().await;
    mock1.set_response(200, "first_server");

    let mock2 = EnhancedMockServer::start().await;
    mock2.set_response(200, "second_server");

    let port = portpicker::pick_unused_port().unwrap();
    let _proxy = ProxyInstance::start(
        port,
        vec![
            &format!("test.local host://127.0.0.1:{}", mock1.port),
            &format!("test.local host://127.0.0.1:{}", mock2.port),
        ],
    )
    .await
    .map_err(|e| format!("Failed to start proxy: {}", e))?;

    _proxy.wait_for_ready().await?;

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

async fn test_priority_xhost_over_host() -> Result<(), String> {
    let mock1 = EnhancedMockServer::start().await;
    mock1.set_response(200, "host_server");

    let mock2 = EnhancedMockServer::start().await;
    mock2.set_response(200, "xhost_server");

    let port = portpicker::pick_unused_port().unwrap();
    let _proxy = ProxyInstance::start(
        port,
        vec![
            &format!("test.local host://127.0.0.1:{}", mock1.port),
            &format!("test.local xhost://127.0.0.1:{}", mock2.port),
        ],
    )
    .await
    .map_err(|e| format!("Failed to start proxy: {}", e))?;

    _proxy.wait_for_ready().await?;

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

async fn test_priority_host_vs_proxy() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;
    mock.set_response(200, "host_wins");

    let port = portpicker::pick_unused_port().unwrap();
    let proxy_port = portpicker::pick_unused_port().unwrap();
    let _proxy = ProxyInstance::start(
        port,
        vec![
            &format!("test.local host://127.0.0.1:{}", mock.port),
            &format!("test.local proxy://127.0.0.1:{}", proxy_port),
        ],
    )
    .await
    .map_err(|e| format!("Failed to start proxy: {}", e))?;

    _proxy.wait_for_ready().await?;

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

async fn test_priority_header_override() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;

    let port = portpicker::pick_unused_port().unwrap();
    let _proxy = ProxyInstance::start(
        port,
        vec![
            &format!("test.local host://127.0.0.1:{}", mock.port),
            "test.local reqHeaders://X-Priority=first",
            "test.local reqHeaders://X-Priority=second",
        ],
    )
    .await
    .map_err(|e| format!("Failed to start proxy: {}", e))?;

    _proxy.wait_for_ready().await?;

    let result = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "http://test.local/api",
    )
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result.assert_success()?;
    mock.assert_header_received("x-priority", "second")?;

    Ok(())
}

async fn test_priority_header_merge() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;

    let port = portpicker::pick_unused_port().unwrap();
    let _proxy = ProxyInstance::start(
        port,
        vec![
            &format!("test.local host://127.0.0.1:{}", mock.port),
            "test.local reqHeaders://X-Header-A=value-a",
            "test.local reqHeaders://X-Header-B=value-b",
        ],
    )
    .await
    .map_err(|e| format!("Failed to start proxy: {}", e))?;

    _proxy.wait_for_ready().await?;

    let result = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "http://test.local/api",
    )
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result.assert_success()?;
    mock.assert_header_received("x-header-a", "value-a")?;
    mock.assert_header_received("x-header-b", "value-b")?;

    Ok(())
}

async fn test_priority_cookie_override() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;

    let port = portpicker::pick_unused_port().unwrap();
    let _proxy = ProxyInstance::start(
        port,
        vec![
            &format!("test.local host://127.0.0.1:{}", mock.port),
            "test.local reqCookies://session=first",
            "test.local reqCookies://session=second",
        ],
    )
    .await
    .map_err(|e| format!("Failed to start proxy: {}", e))?;

    _proxy.wait_for_ready().await?;

    let result = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "http://test.local/api",
    )
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result.assert_success()?;
    let req = mock.last_request().ok_or("No request received")?;
    let cookie_header = req.headers.get("cookie").ok_or("No cookie header")?;
    if !cookie_header.contains("session=second") {
        return Err(format!(
            "Expected cookie session=second, got: {}",
            cookie_header
        ));
    }

    Ok(())
}

async fn test_priority_cookie_merge() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;

    let port = portpicker::pick_unused_port().unwrap();
    let _proxy = ProxyInstance::start(
        port,
        vec![
            &format!("test.local host://127.0.0.1:{}", mock.port),
            "test.local reqCookies://cookie_a=value1",
            "test.local reqCookies://cookie_b=value2",
        ],
    )
    .await
    .map_err(|e| format!("Failed to start proxy: {}", e))?;

    _proxy.wait_for_ready().await?;

    let result = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "http://test.local/api",
    )
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result.assert_success()?;
    mock.assert_header_contains("cookie", "cookie_a")?;
    mock.assert_header_contains("cookie", "cookie_b")?;

    Ok(())
}

async fn test_priority_urlparams_override() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;

    let port = portpicker::pick_unused_port().unwrap();
    let _proxy = ProxyInstance::start(
        port,
        vec![
            &format!("test.local host://127.0.0.1:{}", mock.port),
            "test.local urlParams://(key:first)",
            "test.local urlParams://(key:second)",
        ],
    )
    .await
    .map_err(|e| format!("Failed to start proxy: {}", e))?;

    _proxy.wait_for_ready().await?;

    let result = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "http://test.local/api",
    )
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result.assert_success()?;
    let req = mock.last_request().ok_or("No request received")?;
    if let Some(query) = &req.query {
        if !query.contains("key=second") {
            return Err(format!("Expected key=second in query, got: {}", query));
        }
    } else {
        return Err("Query string missing".to_string());
    }

    Ok(())
}

async fn test_priority_urlparams_merge() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;

    let port = portpicker::pick_unused_port().unwrap();
    let _proxy = ProxyInstance::start(
        port,
        vec![
            &format!("test.local host://127.0.0.1:{}", mock.port),
            "test.local urlParams://(param_x:value1)",
            "test.local urlParams://(param_y:value2)",
        ],
    )
    .await
    .map_err(|e| format!("Failed to start proxy: {}", e))?;

    _proxy.wait_for_ready().await?;

    let result = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "http://test.local/api",
    )
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result.assert_success()?;
    let req = mock.last_request().ok_or("No request received")?;
    if let Some(query) = &req.query {
        if !query.contains("param_x") || !query.contains("param_y") {
            return Err(format!(
                "Expected both param_x and param_y in query, got: {}",
                query
            ));
        }
    } else {
        return Err("Query string missing".to_string());
    }

    Ok(())
}

async fn test_priority_resbody_last_wins() -> Result<(), String> {
    let port = portpicker::pick_unused_port().unwrap();
    let _proxy = ProxyInstance::start(
        port,
        vec![
            "test.local file://(body_first)",
            "test.local file://(body_second)",
            "test.local file://(body_last)",
        ],
    )
    .await
    .map_err(|e| format!("Failed to start proxy: {}", e))?;

    _proxy.wait_for_ready().await?;

    let result = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "http://test.local/api",
    )
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result.assert_success()?;
    result.assert_body_contains("body_last")?;

    Ok(())
}

async fn test_priority_forward_with_modify() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;
    mock.set_response(200, "forwarded_response");

    let port = portpicker::pick_unused_port().unwrap();
    let _proxy = ProxyInstance::start(
        port,
        vec![
            &format!("test.local host://127.0.0.1:{}", mock.port),
            "test.local reqHeaders://X-Forward=true",
            "test.local resHeaders://X-Modified=true",
        ],
    )
    .await
    .map_err(|e| format!("Failed to start proxy: {}", e))?;

    _proxy.wait_for_ready().await?;

    let result = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "http://test.local/api",
    )
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result.assert_success()?;
    result.assert_body_contains("forwarded_response")?;
    mock.assert_header_received("x-forward", "true")?;
    result.assert_header("x-modified", "true")?;

    Ok(())
}

async fn test_priority_mixed_rules() -> Result<(), String> {
    let mock1 = EnhancedMockServer::start().await;
    mock1.set_response(200, "server1_response");

    let mock2 = EnhancedMockServer::start().await;
    mock2.set_response(200, "server2_response");

    let port = portpicker::pick_unused_port().unwrap();
    let _proxy = ProxyInstance::start(
        port,
        vec![
            &format!(
                "test.local host://127.0.0.1:{} reqHeaders://X-Test=override1 reqHeaders://X-Extra=added resHeaders://X-Response=modified urlParams://debug=true",
                mock1.port
            ),
            &format!("test.local host://127.0.0.1:{}", mock2.port),
            "test.local reqHeaders://X-Test=override2",
        ],
    )
    .await
    .map_err(|e| format!("Failed to start proxy: {}", e))?;

    _proxy.wait_for_ready().await?;

    let result = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "http://test.local/api",
    )
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result.assert_success()?;
    result.assert_body_contains("server1_response")?;
    mock1.assert_header_received("x-test", "override2")?;
    mock1.assert_header_received("x-extra", "added")?;
    result.assert_header("x-response", "modified")?;

    let req = mock1.last_request().ok_or("No request received")?;
    if let Some(query) = &req.query {
        if !query.contains("debug=true") {
            return Err(format!("Expected debug=true in query, got: {}", query));
        }
    } else {
        return Err("Query string missing".to_string());
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_host_order() {
        let result = test_priority_host_order().await;
        assert!(result.is_ok(), "Test failed: {:?}", result.err());
    }

    #[tokio::test]
    async fn test_xhost_over_host() {
        let result = test_priority_xhost_over_host().await;
        assert!(result.is_ok(), "Test failed: {:?}", result.err());
    }

    #[tokio::test]
    async fn test_header_override() {
        let result = test_priority_header_override().await;
        assert!(result.is_ok(), "Test failed: {:?}", result.err());
    }

    #[tokio::test]
    async fn test_header_merge() {
        let result = test_priority_header_merge().await;
        assert!(result.is_ok(), "Test failed: {:?}", result.err());
    }

    #[tokio::test]
    async fn test_mixed_rules() {
        let result = test_priority_mixed_rules().await;
        assert!(result.is_ok(), "Test failed: {:?}", result.err());
    }
}
