use crate::curl::CurlCommand;
use crate::mock::EnhancedMockServer;
use crate::proxy::ProxyInstance;
use crate::runner::TestCase;
use std::collections::HashMap;
use std::time::Duration;

pub fn get_all_tests() -> Vec<TestCase> {
    vec![
        TestCase::standalone(
            "protocol_host_basic",
            "Host protocol: basic redirect",
            "protocols",
            test_protocol_host_basic,
        ),
        TestCase::standalone(
            "protocol_host_with_port",
            "Host protocol: redirect with port",
            "protocols",
            test_protocol_host_with_port,
        ),
        TestCase::standalone(
            "protocol_reqheaders_single",
            "ReqHeaders protocol: single header",
            "protocols",
            test_protocol_reqheaders_single,
        ),
        TestCase::standalone(
            "protocol_reqheaders_multiple",
            "ReqHeaders protocol: multiple headers",
            "protocols",
            test_protocol_reqheaders_multiple,
        ),
        TestCase::standalone(
            "protocol_resheaders_single",
            "ResHeaders protocol: single header",
            "protocols",
            test_protocol_resheaders_single,
        ),
        TestCase::standalone(
            "protocol_resheaders_multiple",
            "ResHeaders protocol: multiple headers",
            "protocols",
            test_protocol_resheaders_multiple,
        ),
        TestCase::standalone(
            "protocol_statuscode",
            "StatusCode protocol: modify response status",
            "protocols",
            test_protocol_statuscode,
        ),
        TestCase::standalone(
            "protocol_ua",
            "UA protocol: modify User-Agent",
            "protocols",
            test_protocol_ua,
        ),
        TestCase::standalone(
            "protocol_referer",
            "Referer protocol: inject referer header",
            "protocols",
            test_protocol_referer,
        ),
        TestCase::standalone(
            "protocol_method",
            "Method protocol: change request method",
            "protocols",
            test_protocol_method,
        ),
        TestCase::standalone(
            "protocol_reqcookies",
            "ReqCookies protocol: inject request cookies",
            "protocols",
            test_protocol_reqcookies,
        ),
        TestCase::standalone(
            "protocol_rescookies",
            "ResCookies protocol: inject response cookies",
            "protocols",
            test_protocol_rescookies,
        ),
        TestCase::standalone(
            "protocol_rescors",
            "ResCors protocol: enable CORS",
            "protocols",
            test_protocol_rescors,
        ),
        TestCase::standalone(
            "protocol_proxy_upstream",
            "Proxy protocol: forward to upstream proxy",
            "protocols",
            test_protocol_proxy_upstream,
        ),
        TestCase::standalone(
            "protocol_full_url_reqheaders_spaces",
            "Full URL pattern: spaced reqHeaders value stays separate from host target",
            "protocols",
            test_protocol_full_url_reqheaders_spaces,
        ),
        TestCase::standalone(
            "protocol_full_url_bare_target_reqheaders_spaces",
            "Full URL pattern + bare target: spaced reqHeaders value stays active",
            "protocols",
            test_protocol_full_url_bare_target_reqheaders_spaces,
        ),
        TestCase::standalone(
            "protocol_full_url_reqheaders_value_ref",
            "Full URL pattern: value-ref reqHeaders still applies after host target",
            "protocols",
            test_protocol_full_url_reqheaders_value_ref,
        ),
        TestCase::standalone(
            "protocol_full_url_bare_target_reqheaders_value_ref",
            "Full URL pattern + bare target: value-ref reqHeaders still applies",
            "protocols",
            test_protocol_full_url_bare_target_reqheaders_value_ref,
        ),
        TestCase::standalone(
            "protocol_combined_pipeline",
            "Combined: full request/response pipeline",
            "protocols",
            test_protocol_combined_pipeline,
        ),
    ]
}

async fn test_protocol_host_basic() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;
    mock.set_response(200, "host_redirected");

    let port = portpicker::pick_unused_port().unwrap();
    let _proxy = ProxyInstance::start(
        port,
        vec![&format!("original.host host://127.0.0.1:{}", mock.port)],
    )
    .await
    .map_err(|e| format!("Failed to start proxy: {}", e))?;

    tokio::time::sleep(Duration::from_millis(100)).await;

    let result = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "http://original.host/path",
    )
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result.assert_success()?;
    result.assert_body_contains("host_redirected")?;
    mock.assert_path("/path")?;

    Ok(())
}

async fn test_protocol_host_with_port() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;
    mock.set_response(200, "port_redirected");

    let port = portpicker::pick_unused_port().unwrap();
    let _proxy = ProxyInstance::start(
        port,
        vec![&format!(
            "original.host:8080 host://127.0.0.1:{}",
            mock.port
        )],
    )
    .await
    .map_err(|e| format!("Failed to start proxy: {}", e))?;

    tokio::time::sleep(Duration::from_millis(100)).await;

    let result = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "http://original.host:8080/api",
    )
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result.assert_success()?;
    result.assert_body_contains("port_redirected")?;

    Ok(())
}

async fn test_protocol_reqheaders_single() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;

    let port = portpicker::pick_unused_port().unwrap();
    let _proxy = ProxyInstance::start(
        port,
        vec![
            &format!("test.local host://127.0.0.1:{}", mock.port),
            "test.local reqHeaders://X-Single-Header=single-value",
        ],
    )
    .await
    .map_err(|e| format!("Failed to start proxy: {}", e))?;

    tokio::time::sleep(Duration::from_millis(100)).await;

    let result = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "http://test.local/api",
    )
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result.assert_success()?;
    mock.assert_header_received("x-single-header", "single-value")?;

    Ok(())
}

async fn test_protocol_reqheaders_multiple() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;

    let port = portpicker::pick_unused_port().unwrap();
    let _proxy = ProxyInstance::start(
        port,
        vec![
            &format!("test.local host://127.0.0.1:{}", mock.port),
            "test.local reqHeaders://X-First=one, X-Second: two, X-Third: three",
        ],
    )
    .await
    .map_err(|e| format!("Failed to start proxy: {}", e))?;

    tokio::time::sleep(Duration::from_millis(100)).await;

    let result = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "http://test.local/api",
    )
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result.assert_success()?;
    mock.assert_header_received("x-first", "one")?;
    mock.assert_header_received("x-second", "two")?;
    mock.assert_header_received("x-third", "three")?;

    Ok(())
}

async fn test_protocol_resheaders_single() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;

    let port = portpicker::pick_unused_port().unwrap();
    let _proxy = ProxyInstance::start(
        port,
        vec![
            &format!("test.local host://127.0.0.1:{}", mock.port),
            "test.local resHeaders://X-Response-Header=response-value",
        ],
    )
    .await
    .map_err(|e| format!("Failed to start proxy: {}", e))?;

    tokio::time::sleep(Duration::from_millis(100)).await;

    let result = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "http://test.local/api",
    )
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result.assert_success()?;
    result.assert_header("X-Response-Header", "response-value")?;

    Ok(())
}

async fn test_protocol_resheaders_multiple() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;

    let port = portpicker::pick_unused_port().unwrap();
    let _proxy = ProxyInstance::start(
        port,
        vec![
            &format!("test.local host://127.0.0.1:{}", mock.port),
            "test.local resHeaders://X-Res-One=val1, X-Res-Two: val2",
        ],
    )
    .await
    .map_err(|e| format!("Failed to start proxy: {}", e))?;

    tokio::time::sleep(Duration::from_millis(100)).await;

    let result = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "http://test.local/api",
    )
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result.assert_success()?;
    result.assert_header("X-Res-One", "val1")?;
    result.assert_header("X-Res-Two", "val2")?;

    Ok(())
}

async fn test_protocol_statuscode() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;
    mock.set_response(200, "original");

    let port = portpicker::pick_unused_port().unwrap();
    let _proxy = ProxyInstance::start(
        port,
        vec![
            &format!("test.local host://127.0.0.1:{}", mock.port),
            "test.local statusCode://201",
        ],
    )
    .await
    .map_err(|e| format!("Failed to start proxy: {}", e))?;

    tokio::time::sleep(Duration::from_millis(100)).await;

    let result = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "http://test.local/api",
    )
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result.assert_status(201)?;

    Ok(())
}

async fn test_protocol_ua() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;

    let port = portpicker::pick_unused_port().unwrap();
    let _proxy = ProxyInstance::start(
        port,
        vec![
            &format!("test.local host://127.0.0.1:{}", mock.port),
            "test.local ua://BifrostProxy/1.0 (Test)",
        ],
    )
    .await
    .map_err(|e| format!("Failed to start proxy: {}", e))?;

    tokio::time::sleep(Duration::from_millis(100)).await;

    let result = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "http://test.local/api",
    )
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result.assert_success()?;
    mock.assert_header_received("user-agent", "BifrostProxy/1.0 (Test)")?;

    Ok(())
}

async fn test_protocol_referer() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;

    let port = portpicker::pick_unused_port().unwrap();
    let _proxy = ProxyInstance::start(
        port,
        vec![
            &format!("test.local host://127.0.0.1:{}", mock.port),
            "test.local referer://https://example.com/referrer-page",
        ],
    )
    .await
    .map_err(|e| format!("Failed to start proxy: {}", e))?;

    tokio::time::sleep(Duration::from_millis(100)).await;

    let result = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "http://test.local/api",
    )
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result.assert_success()?;
    mock.assert_header_received("referer", "https://example.com/referrer-page")?;

    Ok(())
}

async fn test_protocol_method() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;

    let port = portpicker::pick_unused_port().unwrap();
    let _proxy = ProxyInstance::start(
        port,
        vec![
            &format!("test.local host://127.0.0.1:{}", mock.port),
            "test.local method://PUT",
        ],
    )
    .await
    .map_err(|e| format!("Failed to start proxy: {}", e))?;

    tokio::time::sleep(Duration::from_millis(100)).await;

    let result = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "http://test.local/api",
    )
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result.assert_success()?;
    mock.assert_method("PUT")?;

    Ok(())
}

async fn test_protocol_reqcookies() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;

    let port = portpicker::pick_unused_port().unwrap();
    let _proxy = ProxyInstance::start(
        port,
        vec![
            &format!("test.local host://127.0.0.1:{}", mock.port),
            "test.local reqCookies://session=sess123, token: tok456",
        ],
    )
    .await
    .map_err(|e| format!("Failed to start proxy: {}", e))?;

    tokio::time::sleep(Duration::from_millis(100)).await;

    let result = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "http://test.local/api",
    )
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result.assert_success()?;
    mock.assert_header_contains("cookie", "session")?;

    Ok(())
}

async fn test_protocol_rescookies() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;

    let port = portpicker::pick_unused_port().unwrap();
    let _proxy = ProxyInstance::start(
        port,
        vec![
            &format!("test.local host://127.0.0.1:{}", mock.port),
            "test.local resCookies://tracking_id=abc123",
        ],
    )
    .await
    .map_err(|e| format!("Failed to start proxy: {}", e))?;

    tokio::time::sleep(Duration::from_millis(100)).await;

    let result = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "http://test.local/api",
    )
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result.assert_success()?;
    result.assert_header_contains("Set-Cookie", "tracking_id")?;

    Ok(())
}

async fn test_protocol_rescors() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;

    let port = portpicker::pick_unused_port().unwrap();
    let _proxy = ProxyInstance::start(
        port,
        vec![
            &format!("test.local host://127.0.0.1:{}", mock.port),
            "test.local resCors://*",
        ],
    )
    .await
    .map_err(|e| format!("Failed to start proxy: {}", e))?;

    tokio::time::sleep(Duration::from_millis(100)).await;

    let result = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "http://test.local/api",
    )
    .header("Origin", "http://other-domain.com")
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result.assert_success()?;
    result.assert_header("Access-Control-Allow-Origin", "http://other-domain.com")?;

    Ok(())
}

async fn test_protocol_proxy_upstream() -> Result<(), String> {
    Err("SKIPPED: Upstream proxy test requires external proxy setup".to_string())
}

async fn test_protocol_full_url_reqheaders_spaces() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;

    let port = portpicker::pick_unused_port().unwrap();
    let _proxy = ProxyInstance::start(
        port,
        vec![&format!(
            "http://space.rule.test/api host://127.0.0.1:{} reqHeaders://(X-Trace: note.example.com:8443 stays text)",
            mock.port
        )],
    )
    .await
    .map_err(|e| format!("Failed to start proxy: {}", e))?;

    tokio::time::sleep(Duration::from_millis(100)).await;

    let result = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "http://space.rule.test/api/v1/users",
    )
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result.assert_success()?;
    mock.assert_path("/api/v1/users")?;
    mock.assert_header_received("x-trace", "note.example.com:8443 stays text")?;

    Ok(())
}

async fn test_protocol_full_url_bare_target_reqheaders_spaces() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;

    let port = portpicker::pick_unused_port().unwrap();
    let _proxy = ProxyInstance::start_with_rules_text(
        port,
        &format!(
            "http://space-bare.rule.test/api 127.0.0.1:{} reqHeaders://(X-Trace: note.example.com:8443 stays text)",
            mock.port
        ),
    )
    .await
    .map_err(|e| format!("Failed to start proxy: {}", e))?;

    tokio::time::sleep(Duration::from_millis(100)).await;

    let result = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "http://space-bare.rule.test/api/v1/users",
    )
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result.assert_success()?;
    mock.assert_path("/api/v1/users")?;
    mock.assert_header_received("x-trace", "note.example.com:8443 stays text")?;

    Ok(())
}

async fn test_protocol_full_url_reqheaders_value_ref() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;

    let port = portpicker::pick_unused_port().unwrap();
    let mut values = HashMap::new();
    values.insert(
        "customHeaders".to_string(),
        "X-Upstream=api.example.com:9443".to_string(),
    );

    let _proxy = ProxyInstance::start_with_values(
        port,
        vec![
            &format!(
                "http://value-ref.rule.test/api host://127.0.0.1:{}",
                mock.port
            ),
            "http://value-ref.rule.test/api reqHeaders://{customHeaders}",
        ],
        values,
    )
    .await
    .map_err(|e| format!("Failed to start proxy: {}", e))?;

    tokio::time::sleep(Duration::from_millis(100)).await;

    let result = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "http://value-ref.rule.test/api/users",
    )
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result.assert_success()?;
    mock.assert_path("/api/users")?;
    mock.assert_header_received("x-upstream", "api.example.com:9443")?;

    Ok(())
}

async fn test_protocol_full_url_bare_target_reqheaders_value_ref() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;

    let port = portpicker::pick_unused_port().unwrap();
    let rules_text = format!(
        r#"http://value-ref-bare.rule.test/api 127.0.0.1:{} reqHeaders://{{customHeaders}}

``` customHeaders
X-Upstream: api.example.com:9443
```"#,
        mock.port
    );
    let _proxy = ProxyInstance::start_with_rules_text(port, &rules_text)
        .await
        .map_err(|e| format!("Failed to start proxy: {}", e))?;

    tokio::time::sleep(Duration::from_millis(100)).await;

    let result = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "http://value-ref-bare.rule.test/api/users",
    )
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result.assert_success()?;
    mock.assert_path("/api/users")?;
    mock.assert_header_received("x-upstream", "api.example.com:9443")?;

    Ok(())
}

async fn test_protocol_combined_pipeline() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;

    let port = portpicker::pick_unused_port().unwrap();
    let _proxy = ProxyInstance::start(
        port,
        vec![
            &format!("pipeline.test host://127.0.0.1:{}", mock.port),
            "pipeline.test reqHeaders://X-Request-Added=yes",
            "pipeline.test resHeaders://X-Response-Added=yes",
            "pipeline.test ua://Pipeline-UA/1.0",
            "pipeline.test resCors://*",
        ],
    )
    .await
    .map_err(|e| format!("Failed to start proxy: {}", e))?;

    tokio::time::sleep(Duration::from_millis(100)).await;

    let result = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "http://pipeline.test/api/full",
    )
    .header("Origin", "http://client.com")
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result.assert_success()?;

    mock.assert_path("/api/full")?;
    mock.assert_header_received("x-request-added", "yes")?;
    mock.assert_header_received("user-agent", "Pipeline-UA/1.0")?;

    result.assert_header("X-Response-Added", "yes")?;
    result.assert_header("Access-Control-Allow-Origin", "http://client.com")?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_host_basic() {
        let result = test_protocol_host_basic().await;
        assert!(result.is_ok(), "Test failed: {:?}", result.err());
    }

    #[tokio::test]
    async fn test_combined() {
        let result = test_protocol_combined_pipeline().await;
        assert!(result.is_ok(), "Test failed: {:?}", result.err());
    }
}
