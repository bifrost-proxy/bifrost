use crate::curl::CurlCommand;
use crate::mock::EnhancedMockServer;
use crate::proxy::ProxyInstance;
use crate::runner::TestCase;
use std::time::Duration;

pub fn get_all_tests() -> Vec<TestCase> {
    vec![
        TestCase::standalone(
            "curl_mock_host_redirect",
            "Host redirect to mock server",
            "curl_mock",
            test_host_redirect,
        ),
        TestCase::standalone(
            "curl_mock_reqheaders_injection",
            "Request headers injection via proxy",
            "curl_mock",
            test_reqheaders_injection,
        ),
        TestCase::standalone(
            "curl_mock_resheaders_injection",
            "Response headers injection via proxy",
            "curl_mock",
            test_resheaders_injection,
        ),
        TestCase::standalone(
            "curl_mock_ua_modification",
            "User-Agent modification",
            "curl_mock",
            test_ua_modification,
        ),
        TestCase::standalone(
            "curl_mock_referer_injection",
            "Referer header injection",
            "curl_mock",
            test_referer_injection,
        ),
        TestCase::standalone(
            "curl_mock_cookie_injection",
            "Cookie injection via proxy",
            "curl_mock",
            test_cookie_injection,
        ),
        TestCase::standalone(
            "curl_mock_host_plus_reqheaders",
            "Combined host redirect + request headers",
            "curl_mock",
            test_host_plus_reqheaders,
        ),
        TestCase::standalone(
            "curl_mock_host_plus_resheaders",
            "Combined host redirect + response headers",
            "curl_mock",
            test_host_plus_resheaders,
        ),
        TestCase::standalone(
            "curl_mock_multi_headers",
            "Multiple request headers injection",
            "curl_mock",
            test_multi_headers,
        ),
        TestCase::standalone(
            "curl_mock_wildcard_match",
            "Wildcard pattern matching",
            "curl_mock",
            test_wildcard_match,
        ),
        TestCase::standalone(
            "curl_mock_status_code",
            "Status code modification",
            "curl_mock",
            test_status_code,
        ),
        TestCase::standalone(
            "curl_mock_cors_headers",
            "CORS headers injection",
            "curl_mock",
            test_cors_headers,
        ),
    ]
}

async fn test_host_redirect() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;
    mock.set_response(200, "mock_response_ok");

    let port = portpicker::pick_unused_port().unwrap();
    let _proxy = ProxyInstance::start(
        port,
        vec![&format!("example.com host://127.0.0.1:{}", mock.port)],
    )
    .await
    .map_err(|e| format!("Failed to start proxy: {}", e))?;

    tokio::time::sleep(Duration::from_millis(100)).await;

    let result = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "http://example.com/test/path",
    )
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result.assert_success()?;
    result.assert_body_contains("mock_response")?;

    mock.assert_path("/test/path")?;
    mock.assert_method("GET")?;

    Ok(())
}

async fn test_reqheaders_injection() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;

    let port = portpicker::pick_unused_port().unwrap();
    let _proxy = ProxyInstance::start(
        port,
        vec![
            &format!("test.local host://127.0.0.1:{}", mock.port),
            "test.local reqHeaders://X-Custom-Header=test-value-123",
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

    mock.assert_header_received("x-custom-header", "test-value-123")?;

    let json = result.parse_json()?;
    let received_headers = json
        .get("received")
        .and_then(|r| r.get("headers"))
        .ok_or("No received headers in response")?;

    if !received_headers.to_string().contains("x-custom-header") {
        return Err("Header not echoed in mock response".to_string());
    }

    Ok(())
}

async fn test_resheaders_injection() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;

    let port = portpicker::pick_unused_port().unwrap();
    let _proxy = ProxyInstance::start(
        port,
        vec![
            &format!("test.local host://127.0.0.1:{}", mock.port),
            "test.local resHeaders://X-Response-Custom=response-value",
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
    result.assert_header("X-Response-Custom", "response-value")?;

    Ok(())
}

async fn test_ua_modification() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;

    let port = portpicker::pick_unused_port().unwrap();
    let _proxy = ProxyInstance::start(
        port,
        vec![
            &format!("test.local host://127.0.0.1:{}", mock.port),
            "test.local ua://CustomUserAgent/1.0",
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
    mock.assert_header_received("user-agent", "CustomUserAgent/1.0")?;

    Ok(())
}

async fn test_referer_injection() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;

    let port = portpicker::pick_unused_port().unwrap();
    let _proxy = ProxyInstance::start(
        port,
        vec![
            &format!("test.local host://127.0.0.1:{}", mock.port),
            "test.local referer://https://custom-referer.com/page",
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
    mock.assert_header_received("referer", "https://custom-referer.com/page")?;

    Ok(())
}

async fn test_cookie_injection() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;

    let port = portpicker::pick_unused_port().unwrap();
    let _proxy = ProxyInstance::start(
        port,
        vec![
            &format!("test.local host://127.0.0.1:{}", mock.port),
            "test.local reqCookies://session_id=abc123, user_token: xyz789",
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
    mock.assert_header_contains("cookie", "session_id")?;

    Ok(())
}

async fn test_host_plus_reqheaders() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;

    let port = portpicker::pick_unused_port().unwrap();
    let _proxy = ProxyInstance::start(
        port,
        vec![
            &format!("test.local host://127.0.0.1:{}", mock.port),
            "test.local reqHeaders://X-Test=hello",
            "test.local reqHeaders://X-Another=world",
        ],
    )
    .await
    .map_err(|e| format!("Failed to start proxy: {}", e))?;

    tokio::time::sleep(Duration::from_millis(100)).await;

    let result = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "http://test.local/api/test",
    )
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result.assert_success()?;

    mock.assert_path("/api/test")?;
    mock.assert_header_received("x-test", "hello")?;
    mock.assert_header_received("x-another", "world")?;

    Ok(())
}

async fn test_host_plus_resheaders() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;

    let port = portpicker::pick_unused_port().unwrap();
    let _proxy = ProxyInstance::start(
        port,
        vec![
            &format!("test.local host://127.0.0.1:{}", mock.port),
            "test.local resHeaders://X-Powered-By=BifrostProxy",
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

    mock.assert_request_received()?;
    result.assert_header("X-Powered-By", "BifrostProxy")?;

    Ok(())
}

async fn test_multi_headers() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;

    let port = portpicker::pick_unused_port().unwrap();
    let _proxy = ProxyInstance::start(
        port,
        vec![
            &format!("test.local host://127.0.0.1:{}", mock.port),
            "test.local reqHeaders://X-Header-One=value1, X-Header-Two: value2",
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

    mock.assert_header_received("x-header-one", "value1")?;
    mock.assert_header_received("x-header-two", "value2")?;

    Ok(())
}

async fn test_wildcard_match() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;

    let port = portpicker::pick_unused_port().unwrap();
    let _proxy = ProxyInstance::start(
        port,
        vec![
            &format!("*.wildcard.test host://127.0.0.1:{}", mock.port),
            "*.wildcard.test reqHeaders://X-Wildcard-Match=true",
        ],
    )
    .await
    .map_err(|e| format!("Failed to start proxy: {}", e))?;

    tokio::time::sleep(Duration::from_millis(100)).await;

    let result = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "http://api.wildcard.test/path",
    )
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result.assert_success()?;
    mock.assert_header_received("x-wildcard-match", "true")?;

    let result2 = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "http://sub.wildcard.test/another",
    )
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result2.assert_success()?;

    Ok(())
}

async fn test_status_code() -> Result<(), String> {
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

async fn test_cors_headers() -> Result<(), String> {
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
    .header("Origin", "http://example.com")
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result.assert_success()?;
    result.assert_header("Access-Control-Allow-Origin", "http://example.com")?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_curl_mock_host_redirect() {
        let result = test_host_redirect().await;
        assert!(result.is_ok(), "Test failed: {:?}", result.err());
    }

    #[tokio::test]
    async fn test_curl_mock_reqheaders() {
        let result = test_reqheaders_injection().await;
        assert!(result.is_ok(), "Test failed: {:?}", result.err());
    }

    #[tokio::test]
    async fn test_curl_mock_combined() {
        let result = test_host_plus_reqheaders().await;
        assert!(result.is_ok(), "Test failed: {:?}", result.err());
    }
}
