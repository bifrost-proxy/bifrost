use crate::curl::CurlCommand;
use crate::mock::EnhancedMockServer;
use crate::proxy::ProxyInstance;
use crate::runner::TestCase;
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
            "merge_host_first_match_wins",
            "Merge strategy: first host rule wins",
            "rule_merge_strategy",
            test_merge_host_first_match_wins_e2e,
        ),
        TestCase::standalone(
            "merge_host_with_passthrough",
            "Merge strategy: passthrough blocks host rule",
            "rule_merge_strategy",
            test_merge_host_with_passthrough_e2e,
        ),
        TestCase::standalone(
            "merge_mock_file_first_wins",
            "Merge strategy: first file:// rule wins",
            "rule_merge_strategy",
            test_merge_mock_file_first_wins_e2e,
        ),
        TestCase::standalone(
            "merge_scalar_method_single_match",
            "Merge strategy: first method:// rule applies",
            "rule_merge_strategy",
            test_merge_scalar_method_single_match_e2e,
        ),
        TestCase::standalone(
            "merge_scalar_ua_single_match",
            "Merge strategy: first ua:// rule applies",
            "rule_merge_strategy",
            test_merge_scalar_ua_single_match_e2e,
        ),
        TestCase::standalone(
            "merge_scalar_statuscode",
            "Merge strategy: statusCode:// with file:// mock",
            "rule_merge_strategy",
            test_merge_scalar_statuscode_e2e,
        ),
        TestCase::standalone(
            "merge_reqheaders_different_keys_merge",
            "Merge strategy: reqHeaders with different keys are merged",
            "rule_merge_strategy",
            test_merge_reqheaders_different_keys_merge_e2e,
        ),
        TestCase::standalone(
            "merge_resheaders_different_keys_merge",
            "Merge strategy: resHeaders with different keys are merged",
            "rule_merge_strategy",
            test_merge_resheaders_different_keys_merge_e2e,
        ),
        TestCase::standalone(
            "merge_body_last_wins",
            "Merge strategy: last resBody rule wins",
            "rule_merge_strategy",
            test_merge_body_last_wins_e2e,
        ),
        TestCase::standalone(
            "merge_req_replace_accumulate",
            "Merge strategy: multiple reqReplace rules accumulate",
            "rule_merge_strategy",
            test_merge_req_replace_accumulate_e2e,
        ),
        TestCase::standalone(
            "merge_res_cors_last_wins",
            "Merge strategy: last resCors rule wins",
            "rule_merge_strategy",
            test_merge_res_cors_last_wins_e2e,
        ),
        TestCase::standalone(
            "merge_forward_with_multiple_modifiers",
            "Merge strategy: host + reqHeaders + resHeaders + urlParams all applied",
            "rule_merge_strategy",
            test_merge_forward_with_multiple_modifiers_e2e,
        ),
        TestCase::standalone(
            "merge_cookies_accumulate",
            "Merge strategy: multiple reqCookies accumulate",
            "rule_merge_strategy",
            test_merge_cookies_accumulate_e2e,
        ),
        TestCase::standalone(
            "merge_redirect_with_status",
            "Merge strategy: redirect://301 returns 301 with Location",
            "rule_merge_strategy",
            test_merge_redirect_with_status_e2e,
        ),
        TestCase::standalone(
            "merge_tls_intercept",
            "Merge strategy: tlsIntercept rule is applied",
            "rule_merge_strategy",
            test_merge_tls_intercept_e2e,
        ),
        TestCase::standalone(
            "merge_reqheaders_same_key_override",
            "Merge strategy: two reqHeaders rules with same key, later overrides earlier, client header also overridden",
            "rule_merge_strategy",
            test_merge_reqheaders_same_key_override_e2e,
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

async fn test_merge_host_first_match_wins_e2e() -> Result<(), String> {
    let mock1 = EnhancedMockServer::start().await;
    mock1.set_response(200, "first_server_wins");

    let mock2 = EnhancedMockServer::start().await;
    mock2.set_response(200, "second_server_loses");

    let (port, _proxy) = start_proxy_with_rules!(
        format!("merge.test host://127.0.0.1:{}", mock1.port),
        format!("merge.test host://127.0.0.1:{}", mock2.port),
    )?;

    _proxy.wait_for_ready().await?;

    let result = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "http://merge.test/api",
    )
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result.assert_success()?;
    result.assert_body_contains("first_server_wins")?;

    Ok(())
}

async fn test_merge_host_with_passthrough_e2e() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;
    mock.set_response(200, "should_not_reach");

    let (port, _proxy) = start_proxy_with_rules!(
        "merge-pt.test file://(passthrough_active)",
        format!("merge-pt.test host://127.0.0.1:{}", mock.port),
    )?;

    _proxy.wait_for_ready().await?;

    let result = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "http://merge-pt.test/api",
    )
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result.assert_success()?;
    result.assert_body_contains("passthrough_active")?;

    Ok(())
}

async fn test_merge_mock_file_first_wins_e2e() -> Result<(), String> {
    let (port, _proxy) = start_proxy_with_rules!(
        "merge-file.test file://(mock_content_from_file)",
        "merge-file.test resHeaders://X-Source=file-mock",
    )?;

    _proxy.wait_for_ready().await?;

    let result = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "http://merge-file.test/api",
    )
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result.assert_success()?;
    result.assert_body_contains("mock_content_from_file")?;

    Ok(())
}

async fn test_merge_scalar_method_single_match_e2e() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;

    let (port, _proxy) = start_proxy_with_rules!(
        format!("merge-method.test host://127.0.0.1:{}", mock.port),
        "merge-method.test method://PUT",
        "merge-method.test method://DELETE",
    )?;

    _proxy.wait_for_ready().await?;

    let result = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "http://merge-method.test/api",
    )
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result.assert_success()?;
    mock.assert_method("PUT")?;

    Ok(())
}

async fn test_merge_scalar_ua_single_match_e2e() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;

    let (port, _proxy) = start_proxy_with_rules!(
        format!("merge-ua.test host://127.0.0.1:{}", mock.port),
        "merge-ua.test ua://FirstAgent/1.0",
        "merge-ua.test ua://SecondAgent/2.0",
    )?;

    _proxy.wait_for_ready().await?;

    let result = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "http://merge-ua.test/api",
    )
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result.assert_success()?;
    mock.assert_header_received("user-agent", "FirstAgent/1.0")?;

    Ok(())
}

async fn test_merge_scalar_statuscode_e2e() -> Result<(), String> {
    let (port, _proxy) = start_proxy_with_rules!(
        "merge-status.test file://(custom_body)",
        "merge-status.test statusCode://201",
    )?;

    _proxy.wait_for_ready().await?;

    let result = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "http://merge-status.test/api",
    )
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result.assert_status(201)?;
    result.assert_body_contains("custom_body")?;

    Ok(())
}

async fn test_merge_reqheaders_different_keys_merge_e2e() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;

    let (port, _proxy) = start_proxy_with_rules!(
        format!("merge-reqh.test host://127.0.0.1:{}", mock.port),
        "merge-reqh.test reqHeaders://X-First-Header=alpha",
        "merge-reqh.test reqHeaders://X-Second-Header=beta",
    )?;

    _proxy.wait_for_ready().await?;

    let result = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "http://merge-reqh.test/api",
    )
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result.assert_success()?;
    mock.assert_header_received("x-first-header", "alpha")?;
    mock.assert_header_received("x-second-header", "beta")?;

    Ok(())
}

async fn test_merge_resheaders_different_keys_merge_e2e() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;
    mock.set_response(200, "ok");

    let (port, _proxy) = start_proxy_with_rules!(
        format!("merge-resh.test host://127.0.0.1:{}", mock.port),
        "merge-resh.test resHeaders://X-Res-Alpha=one",
        "merge-resh.test resHeaders://X-Res-Beta=two",
    )?;

    _proxy.wait_for_ready().await?;

    let result = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "http://merge-resh.test/api",
    )
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result.assert_success()?;
    result.assert_header("x-res-alpha", "one")?;
    result.assert_header("x-res-beta", "two")?;

    Ok(())
}

async fn test_merge_body_last_wins_e2e() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;
    mock.set_response(200, "original_body");

    let (port, _proxy) = start_proxy_with_rules!(
        format!("merge-body.test host://127.0.0.1:{}", mock.port),
        "merge-body.test resBody://(first_override)",
        "merge-body.test resBody://(last_override_wins)",
    )?;

    _proxy.wait_for_ready().await?;

    let result = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "http://merge-body.test/api",
    )
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result.assert_success()?;
    result.assert_body_contains("last_override_wins")?;

    Ok(())
}

async fn test_merge_req_replace_accumulate_e2e() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;
    mock.set_response(200, "ok");

    let (port, _proxy) = start_proxy_with_rules!(format!(
        "merge-reqrepl.test host://127.0.0.1:{} reqReplace://aaa=bbb reqReplace://ccc=ddd",
        mock.port
    ),)?;

    _proxy.wait_for_ready().await?;

    let result = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "http://merge-reqrepl.test/api",
    )
    .method("POST")
    .data("aaa and ccc content")
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result.assert_success()?;

    let req = mock.last_request().ok_or("No request received")?;
    let body = req.body.ok_or("No body in request")?;
    if !body.contains("bbb") {
        return Err(format!(
            "Expected first replacement 'aaa'->'bbb' applied, got: {}",
            body
        ));
    }
    if !body.contains("ddd") {
        return Err(format!(
            "Expected second replacement 'ccc'->'ddd' applied, got: {}",
            body
        ));
    }

    Ok(())
}

async fn test_merge_res_cors_last_wins_e2e() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;
    mock.set_response(200, "ok");

    let (port, _proxy) = start_proxy_with_rules!(
        format!("merge-cors.test host://127.0.0.1:{}", mock.port),
        "merge-cors.test resCors://http://first.example.com",
        "merge-cors.test resCors://http://last.example.com",
    )?;

    _proxy.wait_for_ready().await?;

    let result = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "http://merge-cors.test/api",
    )
    .header("Origin", "http://last.example.com")
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result.assert_success()?;
    result.assert_header_contains("access-control-allow-origin", "last.example.com")?;

    Ok(())
}

async fn test_merge_forward_with_multiple_modifiers_e2e() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;
    mock.set_response(200, "forwarded_with_mods");

    let (port, _proxy) = start_proxy_with_rules!(
        format!("merge-fwd.test host://127.0.0.1:{}", mock.port),
        "merge-fwd.test reqHeaders://X-Forwarded-Tag=merge-test",
        "merge-fwd.test resHeaders://X-Proxy-Applied=true",
        "merge-fwd.test urlParams://(trace_id:abc123)",
    )?;

    _proxy.wait_for_ready().await?;

    let result = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "http://merge-fwd.test/api",
    )
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result.assert_success()?;
    result.assert_body_contains("forwarded_with_mods")?;
    mock.assert_header_received("x-forwarded-tag", "merge-test")?;
    result.assert_header("x-proxy-applied", "true")?;

    let req = mock.last_request().ok_or("No request received")?;
    if let Some(query) = &req.query {
        if !query.contains("trace_id=abc123") {
            return Err(format!("Expected trace_id=abc123 in query, got: {}", query));
        }
    } else {
        return Err("Query string missing".to_string());
    }

    Ok(())
}

async fn test_merge_cookies_accumulate_e2e() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;

    let (port, _proxy) = start_proxy_with_rules!(
        format!("merge-cookie.test host://127.0.0.1:{}", mock.port),
        "merge-cookie.test reqCookies://session_id=abc111",
        "merge-cookie.test reqCookies://tracking_id=xyz999",
    )?;

    _proxy.wait_for_ready().await?;

    let result = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "http://merge-cookie.test/api",
    )
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result.assert_success()?;
    mock.assert_header_contains("cookie", "session_id")?;
    mock.assert_header_contains("cookie", "tracking_id")?;

    Ok(())
}

async fn test_merge_redirect_with_status_e2e() -> Result<(), String> {
    let (port, _proxy) = start_proxy_with_rules!(
        "merge-redir.test redirect://301:http://target.example.com/landing"
    )?;

    _proxy.wait_for_ready().await?;

    let result = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "http://merge-redir.test/old-page",
    )
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result.assert_status(301)?;
    result.assert_header("location", "http://target.example.com/landing")?;

    Ok(())
}

async fn test_merge_tls_intercept_e2e() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;
    mock.set_response(200, "tls_intercepted_ok");

    let (port, _proxy) = start_proxy_with_rules!(
        "*.merge-tls.test tlsIntercept://",
        format!("merge-tls.test host://127.0.0.1:{}", mock.port),
    )?;

    _proxy.wait_for_ready().await?;

    let result = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "http://merge-tls.test/api/secure",
    )
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result.assert_success()?;
    result.assert_body_contains("tls_intercepted_ok")?;
    mock.assert_request_received()?;

    Ok(())
}

async fn test_merge_reqheaders_same_key_override_e2e() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;

    let (port, _proxy) = start_proxy_with_rules!(
        format!("merge-sameh.test host://127.0.0.1:{}", mock.port),
        "merge-sameh.test reqHeaders://X-Same-Key=first",
        "merge-sameh.test reqHeaders://X-Same-Key=second",
    )?;

    _proxy.wait_for_ready().await?;

    let result = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "http://merge-sameh.test/api",
    )
    .header("X-Same-Key", "client-original")
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result.assert_success()?;
    mock.assert_header_received("x-same-key", "second")?;

    let req = mock.last_request().ok_or("No request received")?;
    let same_key_count = req
        .header_pairs
        .iter()
        .filter(|(k, _)| k.eq_ignore_ascii_case("x-same-key"))
        .count();
    if same_key_count != 1 {
        return Err(format!(
            "Expected exactly 1 x-same-key header, found {}",
            same_key_count
        ));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_host_first_match_wins() {
        let result = test_merge_host_first_match_wins_e2e().await;
        assert!(result.is_ok(), "Test failed: {:?}", result.err());
    }

    #[tokio::test]
    async fn test_mock_file_first_wins() {
        let result = test_merge_mock_file_first_wins_e2e().await;
        assert!(result.is_ok(), "Test failed: {:?}", result.err());
    }

    #[tokio::test]
    async fn test_reqheaders_merge() {
        let result = test_merge_reqheaders_different_keys_merge_e2e().await;
        assert!(result.is_ok(), "Test failed: {:?}", result.err());
    }

    #[tokio::test]
    async fn test_resheaders_merge() {
        let result = test_merge_resheaders_different_keys_merge_e2e().await;
        assert!(result.is_ok(), "Test failed: {:?}", result.err());
    }

    #[tokio::test]
    async fn test_forward_with_modifiers() {
        let result = test_merge_forward_with_multiple_modifiers_e2e().await;
        assert!(result.is_ok(), "Test failed: {:?}", result.err());
    }

    #[tokio::test]
    async fn test_cookies_accumulate() {
        let result = test_merge_cookies_accumulate_e2e().await;
        assert!(result.is_ok(), "Test failed: {:?}", result.err());
    }

    #[tokio::test]
    async fn test_redirect_with_status() {
        let result = test_merge_redirect_with_status_e2e().await;
        assert!(result.is_ok(), "Test failed: {:?}", result.err());
    }

    #[tokio::test]
    async fn test_reqheaders_same_key_override() {
        let result = test_merge_reqheaders_same_key_override_e2e().await;
        assert!(result.is_ok(), "Test failed: {:?}", result.err());
    }
}
