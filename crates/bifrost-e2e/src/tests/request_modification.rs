use crate::curl::CurlCommand;
use crate::mock::EnhancedMockServer;
use crate::proxy::ProxyInstance;
use crate::runner::TestCase;
use std::collections::HashMap;
use std::time::Duration;

pub fn get_all_tests() -> Vec<TestCase> {
    vec![
        TestCase::standalone(
            "req_headers_single",
            "ReqHeaders protocol: add single header",
            "request_modification",
            test_req_headers_single,
        ),
        TestCase::standalone(
            "req_headers_multiple",
            "ReqHeaders protocol: add multiple headers",
            "request_modification",
            test_req_headers_multiple,
        ),
        TestCase::standalone(
            "req_headers_json_object",
            "ReqHeaders protocol: JSON object header map",
            "request_modification",
            test_req_headers_json_object,
        ),
        TestCase::standalone(
            "req_headers_ampersand_separated",
            "ReqHeaders protocol: ampersand-separated inline header map",
            "request_modification",
            test_req_headers_ampersand_separated,
        ),
        TestCase::standalone(
            "req_headers_referer_equals_url",
            "ReqHeaders protocol: equals delimiter before a URL colon",
            "request_modification",
            test_req_headers_referer_equals_url,
        ),
        TestCase::standalone(
            "req_headers_override",
            "ReqHeaders protocol: later rule overrides earlier",
            "request_modification",
            test_req_headers_override,
        ),
        TestCase::standalone(
            "req_headers_value_ref",
            "ReqHeaders protocol: value reference {name} expansion",
            "request_modification",
            test_req_headers_value_ref,
        ),
        TestCase::standalone(
            "req_headers_value_ref_literal_ampersand",
            "ReqHeaders protocol: preserve literal ampersand in a referenced one-line value",
            "request_modification",
            test_req_headers_value_ref_literal_ampersand,
        ),
        TestCase::standalone(
            "req_headers_template_literal_ampersand",
            "ReqHeaders protocol: template-produced ampersand remains inside one header",
            "request_modification",
            test_req_headers_template_literal_ampersand,
        ),
        TestCase::standalone(
            "req_headers_inline_markdown",
            "ReqHeaders protocol: inline markdown code block values",
            "request_modification",
            test_req_headers_inline_markdown,
        ),
        TestCase::standalone(
            "req_cookies_add",
            "ReqCookies protocol: add request cookies",
            "request_modification",
            test_req_cookies_add,
        ),
        TestCase::standalone(
            "req_cookies_ampersand_separated",
            "ReqCookies protocol: ampersand-separated inline cookie map",
            "request_modification",
            test_req_cookies_ampersand_separated,
        ),
        TestCase::standalone(
            "req_cookies_value_ref_literal_ampersand",
            "ReqCookies protocol: referenced ampersand remains cookie value data",
            "request_modification",
            test_req_cookies_value_ref_literal_ampersand,
        ),
        TestCase::standalone(
            "req_cookies_template_literal_ampersand",
            "ReqCookies protocol: template-produced ampersand remains cookie data",
            "request_modification",
            test_req_cookies_template_literal_ampersand,
        ),
        TestCase::standalone(
            "req_cookies_merge",
            "ReqCookies protocol: merge multiple cookies",
            "request_modification",
            test_req_cookies_merge,
        ),
        TestCase::standalone(
            "req_ua_modify",
            "UA protocol: modify User-Agent",
            "request_modification",
            test_req_ua_modify,
        ),
        TestCase::standalone(
            "req_referer_set",
            "Referer protocol: set referer header",
            "request_modification",
            test_req_referer_set,
        ),
        TestCase::standalone(
            "req_auth_basic",
            "Auth protocol: set basic authentication",
            "request_modification",
            test_req_auth_basic,
        ),
        TestCase::standalone(
            "req_method_change",
            "Method protocol: change request method",
            "request_modification",
            test_req_method_change,
        ),
        TestCase::standalone(
            "req_type_json",
            "ReqType protocol: set content-type to json",
            "request_modification",
            test_req_type_json,
        ),
        TestCase::standalone(
            "req_charset_modify",
            "ReqCharset protocol: modify charset",
            "request_modification",
            test_req_charset_modify,
        ),
        TestCase::standalone(
            "req_combined_modifications",
            "Combined: multiple request modification rules",
            "request_modification",
            test_req_combined_modifications,
        ),
        TestCase::standalone(
            "req_multiple_cookie_headers_merge",
            "Multiple Cookie headers: merged into single Cookie header for upstream",
            "request_modification",
            test_req_multiple_cookie_headers_merge,
        ),
    ]
}

async fn start_proxy_with_rules(rules: Vec<String>) -> Result<(u16, ProxyInstance), String> {
    for attempt in 1..=10 {
        let port = portpicker::pick_unused_port().ok_or("Failed to pick unused port")?;
        let rule_refs: Vec<&str> = rules.iter().map(String::as_str).collect();
        match ProxyInstance::start(port, rule_refs).await {
            Ok(proxy) => return Ok((port, proxy)),
            Err(e) if is_bind_race(&e.to_string()) && attempt < 10 => {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            Err(e) => return Err(format!("Failed to start proxy: {e}")),
        }
    }

    Err("Failed to start proxy after retrying port bind races".to_string())
}

async fn start_proxy_with_values(
    rules: Vec<String>,
    values: HashMap<String, String>,
) -> Result<(u16, ProxyInstance), String> {
    for attempt in 1..=10 {
        let port = portpicker::pick_unused_port().ok_or("Failed to pick unused port")?;
        let rule_refs: Vec<&str> = rules.iter().map(String::as_str).collect();
        match ProxyInstance::start_with_values(port, rule_refs, values.clone()).await {
            Ok(proxy) => return Ok((port, proxy)),
            Err(e) if is_bind_race(&e.to_string()) && attempt < 10 => {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            Err(e) => return Err(format!("Failed to start proxy: {e}")),
        }
    }

    Err("Failed to start proxy after retrying port bind races".to_string())
}

async fn start_proxy_with_rules_text(rules_text: &str) -> Result<(u16, ProxyInstance), String> {
    for attempt in 1..=10 {
        let port = portpicker::pick_unused_port().ok_or("Failed to pick unused port")?;
        match ProxyInstance::start_with_rules_text(port, rules_text).await {
            Ok(proxy) => return Ok((port, proxy)),
            Err(e) if is_bind_race(&e.to_string()) && attempt < 10 => {
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

async fn test_req_headers_single() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;

    let (port, _proxy) = start_proxy_with_rules(vec![
        format!("test.local host://127.0.0.1:{}", mock.port),
        "test.local reqHeaders://X-Custom-Header=test-value".to_string(),
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
    mock.assert_header_received("x-custom-header", "test-value")?;

    Ok(())
}

async fn test_req_headers_multiple() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;

    let (port, _proxy) = start_proxy_with_rules(vec![
        format!("test.local host://127.0.0.1:{}", mock.port),
        "test.local reqHeaders://X-Header-A=value-a".to_string(),
        "test.local reqHeaders://X-Header-B=value-b".to_string(),
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
    mock.assert_header_received("x-header-a", "value-a")?;
    mock.assert_header_received("x-header-b", "value-b")?;

    Ok(())
}

async fn test_req_headers_json_object() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;

    let (port, _proxy) = start_proxy_with_rules(vec![
        format!("test.local host://127.0.0.1:{}", mock.port),
        r#"test.local reqHeaders://{"x-tt-env":"ppe_next_agent_new","x-use-ppe":"1","x-tt-env-fe":"dev"}"#.to_string(),
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
    mock.assert_header_received("x-tt-env", "ppe_next_agent_new")?;
    mock.assert_header_received("x-use-ppe", "1")?;
    mock.assert_header_received("x-tt-env-fe", "dev")?;

    Ok(())
}

async fn test_req_headers_ampersand_separated() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;

    let (port, _proxy) = start_proxy_with_rules(vec![
        format!("test.local host://127.0.0.1:{}", mock.port),
        "test.local reqHeaders://(x-tt-env=ppe_doubao_connect_lark&x-flow-env=ppe_doubao_connect_lark&x-use-ppe=1)".to_string(),
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
    mock.assert_header_received("x-tt-env", "ppe_doubao_connect_lark")?;
    mock.assert_header_received("x-flow-env", "ppe_doubao_connect_lark")?;
    mock.assert_header_received("x-use-ppe", "1")?;

    Ok(())
}

async fn test_req_headers_referer_equals_url() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;

    let (port, _proxy) = start_proxy_with_rules(vec![
        format!("test.local host://127.0.0.1:{}", mock.port),
        "test.local reqHeaders://Referer=https://example.test/".to_string(),
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
    mock.assert_header_received("referer", "https://example.test/")?;

    Ok(())
}

async fn test_req_headers_override() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;

    let (port, _proxy) = start_proxy_with_rules(vec![
        format!("test.local host://127.0.0.1:{}", mock.port),
        "test.local reqHeaders://X-Override=first".to_string(),
        "test.local reqHeaders://X-Override=second".to_string(),
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
    mock.assert_header_received("x-override", "second")?;

    Ok(())
}

async fn test_req_headers_value_ref() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;

    let mut values = HashMap::new();
    values.insert(
        "customHeaders".to_string(),
        "X-Custom-Token=secret-12345".to_string(),
    );

    let (port, _proxy) = start_proxy_with_values(
        vec![
            format!("test.local host://127.0.0.1:{}", mock.port),
            "test.local reqHeaders://{customHeaders}".to_string(),
        ],
        values,
    )
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
    mock.assert_header_received("x-custom-token", "secret-12345")?;

    Ok(())
}

async fn test_req_headers_value_ref_literal_ampersand() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;

    let mut values = HashMap::new();
    values.insert("queryHeader".to_string(), "X-Query: a=1&b=2".to_string());

    let (port, _proxy) = start_proxy_with_values(
        vec![
            format!("test.local host://127.0.0.1:{}", mock.port),
            "test.local reqHeaders://{queryHeader}".to_string(),
        ],
        values,
    )
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
    mock.assert_header_received("x-query", "a=1&b=2")?;

    Ok(())
}

async fn test_req_headers_template_literal_ampersand() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;

    let (port, _proxy) = start_proxy_with_rules(vec![
        format!("test.local host://127.0.0.1:{}", mock.port),
        "test.local reqHeaders://X-Full-Url=${url}".to_string(),
        "test.local reqHeaders://X-Copied=${reqHeaders.x-source}".to_string(),
    ])
    .await?;

    tokio::time::sleep(Duration::from_millis(100)).await;

    let result = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "http://test.local/api?a=1&b=2",
    )
    .header("X-Source", "safe&X-Injected=yes")
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result.assert_success()?;
    mock.assert_header_received("x-full-url", "http://test.local/api?a=1&b=2")?;
    mock.assert_header_received("x-copied", "safe&X-Injected=yes")?;
    let request = mock.last_request().ok_or("No request received")?;
    if request.headers.keys().any(|name| name == "x-injected") {
        return Err("template output injected an unintended X-Injected header".to_string());
    }

    Ok(())
}

async fn test_req_headers_inline_markdown() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;

    let rules_text = format!(
        r#"
test.local host://127.0.0.1:{}
test.local reqHeaders://{{ppeHeaders}}

```ppeHeaders
X-Use-PPE: 1
X-TT-Env: ppe_test_env
```
"#,
        mock.port
    );

    let (port, _proxy) = start_proxy_with_rules_text(&rules_text).await?;

    tokio::time::sleep(Duration::from_millis(100)).await;

    let result = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "http://test.local/api",
    )
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result.assert_success()?;
    mock.assert_header_received("x-use-ppe", "1")?;
    mock.assert_header_received("x-tt-env", "ppe_test_env")?;

    Ok(())
}

async fn test_req_cookies_add() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;

    let (port, _proxy) = start_proxy_with_rules(vec![
        format!("test.local host://127.0.0.1:{}", mock.port),
        "test.local reqCookies://session=abc123".to_string(),
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
    mock.assert_header_contains("cookie", "session")?;

    Ok(())
}

async fn test_req_cookies_ampersand_separated() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;
    let (port, _proxy) = start_proxy_with_rules(vec![
        format!("test.local host://127.0.0.1:{}", mock.port),
        "test.local reqCookies://(sessionid=xxx&a=c&b=two=parts)".to_string(),
    ])
    .await?;

    tokio::time::sleep(Duration::from_millis(100)).await;
    CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "http://test.local/api",
    )
    .execute()
    .await
    .map_err(|e| format!("curl failed: {e}"))?
    .assert_success()?;

    mock.assert_header_contains("cookie", "sessionid=xxx")?;
    mock.assert_header_contains("cookie", "a=c")?;
    mock.assert_header_contains("cookie", "b=two=parts")?;
    Ok(())
}

async fn test_req_cookies_value_ref_literal_ampersand() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;
    let mut values = HashMap::new();
    values.insert(
        "cookieValue".to_string(),
        "session=safe&injected=yes".to_string(),
    );
    let (port, _proxy) = start_proxy_with_values(
        vec![
            format!("test.local host://127.0.0.1:{}", mock.port),
            "test.local reqCookies://{cookieValue}".to_string(),
        ],
        values,
    )
    .await?;

    tokio::time::sleep(Duration::from_millis(100)).await;
    CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "http://test.local/api",
    )
    .execute()
    .await
    .map_err(|e| format!("curl failed: {e}"))?
    .assert_success()?;

    mock.assert_header_contains("cookie", "session=safe&injected=yes")?;
    Ok(())
}

async fn test_req_cookies_template_literal_ampersand() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;
    let (port, _proxy) = start_proxy_with_rules(vec![
        format!("test.local host://127.0.0.1:{}", mock.port),
        "test.local reqCookies://session=${reqHeaders.x-source}".to_string(),
    ])
    .await?;

    tokio::time::sleep(Duration::from_millis(100)).await;
    CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "http://test.local/api",
    )
    .header("X-Source", "safe&injected=yes")
    .execute()
    .await
    .map_err(|e| format!("curl failed: {e}"))?
    .assert_success()?;

    mock.assert_header_contains("cookie", "session=safe&injected=yes")?;
    Ok(())
}

async fn test_req_cookies_merge() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;

    let (port, _proxy) = start_proxy_with_rules(vec![
        format!("test.local host://127.0.0.1:{}", mock.port),
        "test.local reqCookies://cookie_a=value1".to_string(),
        "test.local reqCookies://cookie_b=value2".to_string(),
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
    mock.assert_header_contains("cookie", "cookie_a")?;
    mock.assert_header_contains("cookie", "cookie_b")?;

    Ok(())
}

async fn test_req_ua_modify() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;

    let (port, _proxy) = start_proxy_with_rules(vec![
        format!("test.local host://127.0.0.1:{}", mock.port),
        "test.local ua://BifrostTestAgent/2.0".to_string(),
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
    mock.assert_header_received("user-agent", "BifrostTestAgent/2.0")?;

    Ok(())
}

async fn test_req_referer_set() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;

    let (port, _proxy) = start_proxy_with_rules(vec![
        format!("test.local host://127.0.0.1:{}", mock.port),
        "test.local referer://https://referrer.example.com/page".to_string(),
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
    mock.assert_header_received("referer", "https://referrer.example.com/page")?;

    Ok(())
}

async fn test_req_auth_basic() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;

    let (port, _proxy) = start_proxy_with_rules(vec![
        format!("test.local host://127.0.0.1:{}", mock.port),
        "test.local auth://testuser:testpass".to_string(),
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
    mock.assert_header_contains("authorization", "Basic")?;

    Ok(())
}

async fn test_req_method_change() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;

    let (port, _proxy) = start_proxy_with_rules(vec![
        format!("test.local host://127.0.0.1:{}", mock.port),
        "test.local method://PUT".to_string(),
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
    mock.assert_method("PUT")?;

    Ok(())
}

async fn test_req_type_json() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;

    let (port, _proxy) = start_proxy_with_rules(vec![
        format!("test.local host://127.0.0.1:{}", mock.port),
        "test.local reqType://json".to_string(),
    ])
    .await?;

    tokio::time::sleep(Duration::from_millis(100)).await;

    let result = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "http://test.local/api",
    )
    .method("POST")
    .data(r#"{"test":"data"}"#)
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result.assert_success()?;
    mock.assert_header_contains("content-type", "application/json")?;

    Ok(())
}

async fn test_req_charset_modify() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;

    let (port, _proxy) = start_proxy_with_rules(vec![
        format!("test.local host://127.0.0.1:{}", mock.port),
        "test.local reqCharset://gbk".to_string(),
    ])
    .await?;

    tokio::time::sleep(Duration::from_millis(100)).await;

    let result = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "http://test.local/api",
    )
    .method("POST")
    .header("Content-Type", "text/plain")
    .data("test data")
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result.assert_success()?;
    mock.assert_header_contains("content-type", "gbk")?;

    Ok(())
}

async fn test_req_combined_modifications() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;

    let (port, _proxy) = start_proxy_with_rules(vec![
        format!("test.local host://127.0.0.1:{}", mock.port),
        "test.local reqHeaders://X-Custom=combined-test".to_string(),
        "test.local ua://CombinedAgent/1.0".to_string(),
        "test.local referer://https://combined.test.com".to_string(),
        "test.local method://POST".to_string(),
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
    mock.assert_header_received("x-custom", "combined-test")?;
    mock.assert_header_received("user-agent", "CombinedAgent/1.0")?;
    mock.assert_header_received("referer", "https://combined.test.com")?;
    mock.assert_method("POST")?;

    Ok(())
}

async fn test_req_multiple_cookie_headers_merge() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;

    let (port, _proxy) =
        start_proxy_with_rules(vec![format!("test.local host://127.0.0.1:{}", mock.port)]).await?;

    tokio::time::sleep(Duration::from_millis(100)).await;

    let result = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "http://test.local/api",
    )
    .header("Cookie", "monitor_web_id=123456")
    .header("Cookie", "session_flag=1")
    .header("Cookie", "people-lang=zh")
    .header("Cookie", "x-token=16289f27-f342-4f5b-b95a-e5291cfe1577")
    .header("Cookie", "bd_sso=eyJhbGciOiJSUzI1NiJ9.eyJleHAiOjE3NzZ9.sig")
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result.assert_success()?;

    let req = mock.last_request().ok_or("No request received")?;
    let cookie_header = req
        .headers
        .get("cookie")
        .ok_or("No cookie header forwarded to upstream")?;

    if !cookie_header.contains("monitor_web_id=123456") {
        return Err(format!(
            "Missing monitor_web_id in merged cookie: {}",
            cookie_header
        ));
    }
    if !cookie_header.contains("session_flag=1") {
        return Err(format!(
            "Missing session_flag in merged cookie: {}",
            cookie_header
        ));
    }
    if !cookie_header.contains("people-lang=zh") {
        return Err(format!(
            "Missing people-lang in merged cookie: {}",
            cookie_header
        ));
    }
    if !cookie_header.contains("x-token=16289f27-f342-4f5b-b95a-e5291cfe1577") {
        return Err(format!(
            "Missing x-token in merged cookie: {}",
            cookie_header
        ));
    }
    if !cookie_header.contains("bd_sso=eyJhbGciOiJSUzI1NiJ9.eyJleHAiOjE3NzZ9.sig") {
        return Err(format!(
            "Missing bd_sso JWT in merged cookie: {}",
            cookie_header
        ));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_headers_single() {
        let result = test_req_headers_single().await;
        assert!(result.is_ok(), "Test failed: {:?}", result.err());
    }

    #[tokio::test]
    async fn test_headers_override() {
        let result = test_req_headers_override().await;
        assert!(result.is_ok(), "Test failed: {:?}", result.err());
    }

    #[tokio::test]
    async fn test_headers_ampersand_separated() {
        let result = test_req_headers_ampersand_separated().await;
        assert!(result.is_ok(), "Test failed: {:?}", result.err());
    }

    #[tokio::test]
    async fn test_headers_referer_equals_url() {
        let result = test_req_headers_referer_equals_url().await;
        assert!(result.is_ok(), "Test failed: {:?}", result.err());
    }

    #[tokio::test]
    async fn test_headers_value_ref() {
        let result = test_req_headers_value_ref().await;
        assert!(result.is_ok(), "Test failed: {:?}", result.err());
    }

    #[tokio::test]
    async fn test_headers_value_ref_literal_ampersand() {
        let result = test_req_headers_value_ref_literal_ampersand().await;
        assert!(result.is_ok(), "Test failed: {:?}", result.err());
    }

    #[tokio::test]
    async fn test_headers_template_literal_ampersand() {
        let result = test_req_headers_template_literal_ampersand().await;
        assert!(result.is_ok(), "Test failed: {:?}", result.err());
    }

    #[tokio::test]
    async fn test_headers_inline_markdown() {
        let result = test_req_headers_inline_markdown().await;
        assert!(result.is_ok(), "Test failed: {:?}", result.err());
    }

    #[tokio::test]
    async fn test_ua() {
        let result = test_req_ua_modify().await;
        assert!(result.is_ok(), "Test failed: {:?}", result.err());
    }

    #[tokio::test]
    async fn test_method() {
        let result = test_req_method_change().await;
        assert!(result.is_ok(), "Test failed: {:?}", result.err());
    }

    #[tokio::test]
    async fn test_combined() {
        let result = test_req_combined_modifications().await;
        assert!(result.is_ok(), "Test failed: {:?}", result.err());
    }

    #[tokio::test]
    async fn test_multiple_cookie_headers_merge() {
        let result = test_req_multiple_cookie_headers_merge().await;
        assert!(result.is_ok(), "Test failed: {:?}", result.err());
    }
}
