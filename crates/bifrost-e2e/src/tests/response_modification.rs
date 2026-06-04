use crate::curl::CurlCommand;
use crate::mock::EnhancedMockServer;
use crate::proxy::ProxyInstance;
use crate::runner::TestCase;
use std::collections::HashMap;
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
            "res_headers_single",
            "ResHeaders protocol: add single response header",
            "response_modification",
            test_res_headers_single,
        ),
        TestCase::standalone(
            "res_headers_multiple",
            "ResHeaders protocol: add multiple response headers",
            "response_modification",
            test_res_headers_multiple,
        ),
        TestCase::standalone(
            "res_headers_override",
            "ResHeaders protocol: later rule overrides earlier",
            "response_modification",
            test_res_headers_override,
        ),
        TestCase::standalone(
            "res_cookies_set",
            "ResCookies protocol: set response cookies",
            "response_modification",
            test_res_cookies_set,
        ),
        TestCase::standalone(
            "res_cookies_with_attrs",
            "ResCookies protocol: with advanced attributes",
            "response_modification",
            test_res_cookies_with_attrs,
        ),
        TestCase::standalone(
            "res_cors_all",
            "ResCors protocol: allow all origins",
            "response_modification",
            test_res_cors_all,
        ),
        TestCase::standalone(
            "res_cors_specific",
            "ResCors protocol: allow specific origin",
            "response_modification",
            test_res_cors_specific,
        ),
        TestCase::standalone(
            "res_cors_json_config",
            "ResCors protocol: JSON config with custom settings",
            "response_modification",
            test_res_cors_json_config,
        ),
        TestCase::standalone(
            "res_cors_max_age",
            "ResCors protocol: with max-age setting",
            "response_modification",
            test_res_cors_max_age,
        ),
        TestCase::standalone(
            "res_type_json",
            "ResType protocol: set content-type to json",
            "response_modification",
            test_res_type_json,
        ),
        TestCase::standalone(
            "res_charset_utf8",
            "ResCharset protocol: set charset",
            "response_modification",
            test_res_charset_utf8,
        ),
        TestCase::standalone(
            "res_attachment_download",
            "Attachment protocol: set download headers",
            "response_modification",
            test_res_attachment_download,
        ),
        TestCase::standalone(
            "res_header_delete",
            "ResHeaders protocol: delete header with empty value",
            "response_modification",
            test_res_header_delete,
        ),
        TestCase::standalone(
            "res_cache_control",
            "Cache protocol: set cache-control",
            "response_modification",
            test_res_cache_control,
        ),
        TestCase::standalone(
            "res_disable_cache",
            "DisableCache protocol: disable caching",
            "response_modification",
            test_res_disable_cache,
        ),
        TestCase::standalone(
            "res_combined_modifications",
            "Combined: multiple response modification rules",
            "response_modification",
            test_res_combined_modifications,
        ),
        TestCase::standalone(
            "res_delete_header_x_prefix",
            "DeleteResHeaders protocol: delete specific headers",
            "response_modification",
            test_res_delete_header_x_prefix,
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

fn is_bind_race(error: &str) -> bool {
    error.contains("Failed to bind") || error.contains("already listening on this port")
}

async fn test_res_headers_single() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;
    mock.set_response(200, "ok");

    let (port, _proxy) = start_proxy_with_rules!(
        &format!("test.local host://127.0.0.1:{}", mock.port),
        "test.local resHeaders://X-Custom-Response=test-value",
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
    result.assert_header("x-custom-response", "test-value")?;

    Ok(())
}

async fn test_res_headers_multiple() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;
    mock.set_response(200, "ok");

    let (port, _proxy) = start_proxy_with_rules!(
        &format!("test.local host://127.0.0.1:{}", mock.port),
        "test.local resHeaders://X-Header-A=value-a",
        "test.local resHeaders://X-Header-B=value-b",
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
    result.assert_header("x-header-a", "value-a")?;
    result.assert_header("x-header-b", "value-b")?;

    Ok(())
}

async fn test_res_headers_override() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;
    mock.set_response(200, "ok");

    let (port, _proxy) = start_proxy_with_rules!(
        &format!("test.local host://127.0.0.1:{}", mock.port),
        "test.local resHeaders://X-Override=first",
        "test.local resHeaders://X-Override=second",
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
    result.assert_header("x-override", "second")?;

    Ok(())
}

async fn test_res_cookies_set() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;
    mock.set_response(200, "ok");

    let (port, _proxy) = start_proxy_with_rules!(
        &format!("test.local host://127.0.0.1:{}", mock.port),
        "test.local resCookies://session_id=abc123",
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
    result.assert_header_contains("set-cookie", "session_id")?;

    Ok(())
}

async fn test_res_cookies_with_attrs() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;
    mock.set_response(200, "ok");

    let cookie_config =
        r#"{"auth_token":{"value":"xyz789","maxAge":3600,"secure":true,"httpOnly":true}}"#;
    let (port, _proxy) = start_proxy_with_rules!(
        &format!("test.local host://127.0.0.1:{}", mock.port),
        &format!("test.local resCookies://{}", cookie_config),
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
    result.assert_header_contains("set-cookie", "auth_token=xyz789")?;
    result.assert_header_contains("set-cookie", "Max-Age=3600")?;
    result.assert_header_contains("set-cookie", "Secure")?;
    result.assert_header_contains("set-cookie", "HttpOnly")?;

    Ok(())
}

async fn test_res_cors_all() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;
    mock.set_response(200, "ok");

    let (port, _proxy) = start_proxy_with_rules!(
        &format!("test.local host://127.0.0.1:{}", mock.port),
        "test.local resCors://*",
    )?;

    tokio::time::sleep(Duration::from_millis(100)).await;

    let result = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "http://test.local/api",
    )
    .header("Origin", "http://other.example.com")
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result.assert_success()?;
    result.assert_header_contains("access-control-allow-origin", "http://other.example.com")?;

    Ok(())
}

async fn test_res_cors_specific() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;
    mock.set_response(200, "ok");

    let (port, _proxy) = start_proxy_with_rules!(
        &format!("test.local host://127.0.0.1:{}", mock.port),
        "test.local resCors://http://allowed.example.com",
    )?;

    tokio::time::sleep(Duration::from_millis(100)).await;

    let result = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "http://test.local/api",
    )
    .header("Origin", "http://allowed.example.com")
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result.assert_success()?;
    result.assert_header_contains("access-control-allow-origin", "allowed.example.com")?;

    Ok(())
}

async fn test_res_cors_json_config() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;
    mock.set_response(200, "ok");

    let cors_config = r#"{"origin":"https://custom.example.com","methods":"GET,POST","headers":"X-Custom-Header"}"#;
    let (port, _proxy) = start_proxy_with_rules!(
        &format!("test.local host://127.0.0.1:{}", mock.port),
        &format!("test.local resCors://{}", cors_config),
    )?;

    tokio::time::sleep(Duration::from_millis(100)).await;

    let result = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "http://test.local/api",
    )
    .header("Origin", "https://custom.example.com")
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result.assert_success()?;
    result.assert_header_contains("access-control-allow-origin", "custom.example.com")?;
    result.assert_header_contains("access-control-allow-methods", "GET,POST")?;
    result.assert_header_contains("access-control-allow-headers", "X-Custom-Header")?;

    Ok(())
}

async fn test_res_cors_max_age() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;
    mock.set_response(200, "ok");

    let cors_config = r#"{"origin":"*","maxAge":86400}"#;
    let (port, _proxy) = start_proxy_with_rules!(
        &format!("test.local host://127.0.0.1:{}", mock.port),
        &format!("test.local resCors://{}", cors_config),
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
    result.assert_header_contains("access-control-allow-origin", "*")?;
    result.assert_header_contains("access-control-max-age", "86400")?;

    Ok(())
}

async fn test_res_type_json() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;
    mock.set_response(200, r#"{"data":"test"}"#);

    let (port, _proxy) = start_proxy_with_rules!(
        &format!("test.local host://127.0.0.1:{}", mock.port),
        "test.local resType://json",
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
    result.assert_header_contains("content-type", "application/json")?;

    Ok(())
}

async fn test_res_charset_utf8() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;
    mock.set_response(200, "test content");

    let (port, _proxy) = start_proxy_with_rules!(
        &format!("test.local host://127.0.0.1:{}", mock.port),
        "test.local resCharset://utf-8",
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
    result.assert_header_contains("content-type", "utf-8")?;

    Ok(())
}

async fn test_res_attachment_download() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;
    mock.set_response(200, "file content");

    let (port, _proxy) = start_proxy_with_rules!(
        &format!("test.local host://127.0.0.1:{}", mock.port),
        "test.local attachment://{document.pdf}",
    )?;

    tokio::time::sleep(Duration::from_millis(100)).await;

    let result = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "http://test.local/file",
    )
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result.assert_success()?;
    result.assert_header_contains("content-disposition", "attachment")?;
    result.assert_header_contains("content-disposition", "document.pdf")?;

    Ok(())
}

async fn test_res_header_delete() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;
    mock.set_response_with_headers(
        200,
        "ok",
        HashMap::from([("X-To-Delete".to_string(), "value".to_string())]),
    );

    let (port, _proxy) = start_proxy_with_rules!(
        &format!("test.local host://127.0.0.1:{}", mock.port),
        "test.local resHeaders://X-To-Delete=",
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
    result.assert_header_missing("x-to-delete")?;

    Ok(())
}

async fn test_res_cache_control() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;
    mock.set_response(200, "cacheable content");

    let (port, _proxy) = start_proxy_with_rules!(
        &format!("test.local host://127.0.0.1:{}", mock.port),
        "test.local cache://3600",
    )?;

    tokio::time::sleep(Duration::from_millis(100)).await;

    let result = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "http://test.local/static/file.js",
    )
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result.assert_success()?;
    result.assert_header_contains("cache-control", "max-age=3600")?;

    Ok(())
}

async fn test_res_disable_cache() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;
    mock.set_response(200, "no-cache content");

    let (port, _proxy) = start_proxy_with_rules!(
        &format!("test.local host://127.0.0.1:{}", mock.port),
        "test.local disable://cache",
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
    result.assert_header_contains("cache-control", "no-cache")?;

    Ok(())
}

async fn test_res_combined_modifications() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;
    mock.set_response(200, r#"{"combined":"test"}"#);

    let (port, _proxy) = start_proxy_with_rules!(
        &format!("test.local host://127.0.0.1:{}", mock.port),
        "test.local resHeaders://X-Custom=combined-test",
        "test.local resType://json",
        "test.local resCors://*",
    )?;

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
    result.assert_header("x-custom", "combined-test")?;
    result.assert_header_contains("content-type", "application/json")?;
    result.assert_header_contains("access-control-allow-origin", "http://example.com")?;

    Ok(())
}

async fn test_res_delete_header_x_prefix() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;
    mock.set_response_with_headers(
        200,
        "ok",
        HashMap::from([
            ("X-Server-Info".to_string(), "server1".to_string()),
            ("X-Version".to_string(), "1.0".to_string()),
            ("Content-Type".to_string(), "text/plain".to_string()),
        ]),
    );

    let (port, _proxy) = start_proxy_with_rules!(
        &format!("test.local host://127.0.0.1:{}", mock.port),
        "test.local deleteResHeaders://X-Server-Info",
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
    result.assert_header_missing("x-server-info")?;
    result.assert_header("x-version", "1.0")?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_headers_single() {
        let result = test_res_headers_single().await;
        assert!(result.is_ok(), "Test failed: {:?}", result.err());
    }

    #[tokio::test]
    async fn test_headers_override() {
        let result = test_res_headers_override().await;
        assert!(result.is_ok(), "Test failed: {:?}", result.err());
    }

    #[tokio::test]
    async fn test_cors_all() {
        let result = test_res_cors_all().await;
        assert!(result.is_ok(), "Test failed: {:?}", result.err());
    }

    #[tokio::test]
    async fn test_type_json() {
        let result = test_res_type_json().await;
        assert!(result.is_ok(), "Test failed: {:?}", result.err());
    }

    #[tokio::test]
    async fn test_combined() {
        let result = test_res_combined_modifications().await;
        assert!(result.is_ok(), "Test failed: {:?}", result.err());
    }
}
