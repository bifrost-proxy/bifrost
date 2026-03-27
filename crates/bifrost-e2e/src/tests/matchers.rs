use crate::curl::CurlCommand;
use crate::mock::EnhancedMockServer;
use crate::proxy::ProxyInstance;
use crate::runner::TestCase;
use std::time::Duration;

pub fn get_all_tests() -> Vec<TestCase> {
    vec![
        TestCase::standalone(
            "matcher_domain_exact",
            "Domain matcher: exact domain match",
            "matchers",
            test_matcher_domain_exact,
        ),
        TestCase::standalone(
            "matcher_domain_with_protocol",
            "Domain matcher: with http:// prefix",
            "matchers",
            test_matcher_domain_with_protocol,
        ),
        TestCase::standalone(
            "matcher_domain_with_port",
            "Domain matcher: domain:port",
            "matchers",
            test_matcher_domain_with_port,
        ),
        TestCase::standalone(
            "matcher_domain_with_path",
            "Domain matcher: domain/path",
            "matchers",
            test_matcher_domain_with_path,
        ),
        TestCase::standalone(
            "matcher_wildcard_single_star",
            "Wildcard matcher: *.example.com",
            "matchers",
            test_matcher_wildcard_single_star,
        ),
        TestCase::standalone(
            "matcher_wildcard_double_star",
            "Wildcard matcher: **.example.com",
            "matchers",
            test_matcher_wildcard_double_star,
        ),
        TestCase::standalone(
            "matcher_wildcard_path",
            "Wildcard matcher: path/*",
            "matchers",
            test_matcher_wildcard_path,
        ),
        TestCase::standalone(
            "matcher_regex_basic",
            "Regex matcher: /pattern/",
            "matchers",
            test_matcher_regex_basic,
        ),
        TestCase::standalone(
            "matcher_regex_case_insensitive",
            "Regex matcher: /pattern/i",
            "matchers",
            test_matcher_regex_case_insensitive,
        ),
        TestCase::standalone(
            "matcher_priority_exact_over_wildcard",
            "Priority: exact domain > wildcard",
            "matchers",
            test_matcher_priority_exact_over_wildcard,
        ),
        TestCase::standalone(
            "matcher_no_match_passthrough",
            "No match: request passes through unchanged",
            "matchers",
            test_matcher_no_match_passthrough,
        ),
        TestCase::standalone(
            "matcher_protocol_first_regex_pattern",
            "Protocol-first host rule: regex pattern stays split correctly",
            "matchers",
            test_matcher_protocol_first_regex_pattern,
        ),
    ]
}

async fn test_matcher_domain_exact() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;
    mock.set_response(200, "exact_match");

    let port = portpicker::pick_unused_port().unwrap();
    let _proxy = ProxyInstance::start(
        port,
        vec![&format!("exact.domain.test host://127.0.0.1:{}", mock.port)],
    )
    .await
    .map_err(|e| format!("Failed to start proxy: {}", e))?;

    tokio::time::sleep(Duration::from_millis(100)).await;

    let result = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "http://exact.domain.test/path",
    )
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result.assert_success()?;
    result.assert_body_contains("exact_match")?;

    Ok(())
}

async fn test_matcher_domain_with_protocol() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;
    mock.set_response(200, "protocol_match");

    let port = portpicker::pick_unused_port().unwrap();
    let _proxy = ProxyInstance::start(
        port,
        vec![&format!(
            "http://protocol.test host://127.0.0.1:{}",
            mock.port
        )],
    )
    .await
    .map_err(|e| format!("Failed to start proxy: {}", e))?;

    tokio::time::sleep(Duration::from_millis(100)).await;

    let result = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "http://protocol.test/api",
    )
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result.assert_success()?;
    result.assert_body_contains("protocol_match")?;

    Ok(())
}

async fn test_matcher_domain_with_port() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;
    mock.set_response(200, "port_match");

    let port = portpicker::pick_unused_port().unwrap();
    let _proxy = ProxyInstance::start(
        port,
        vec![&format!("port.test:9090 host://127.0.0.1:{}", mock.port)],
    )
    .await
    .map_err(|e| format!("Failed to start proxy: {}", e))?;

    tokio::time::sleep(Duration::from_millis(100)).await;

    let result = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "http://port.test:9090/api",
    )
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result.assert_success()?;
    result.assert_body_contains("port_match")?;

    Ok(())
}

async fn test_matcher_domain_with_path() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;
    mock.set_response(200, "path_match");

    let port = portpicker::pick_unused_port().unwrap();
    let _proxy = ProxyInstance::start(
        port,
        vec![&format!("path.test/api/v1 host://127.0.0.1:{}", mock.port)],
    )
    .await
    .map_err(|e| format!("Failed to start proxy: {}", e))?;

    tokio::time::sleep(Duration::from_millis(100)).await;

    let result = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "http://path.test/api/v1/users",
    )
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result.assert_success()?;
    result.assert_body_contains("path_match")?;

    Ok(())
}

async fn test_matcher_wildcard_single_star() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;
    mock.set_response(200, "wildcard_single");

    let port = portpicker::pick_unused_port().unwrap();
    let _proxy = ProxyInstance::start(
        port,
        vec![&format!("*.wildcard.test host://127.0.0.1:{}", mock.port)],
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
    result.assert_body_contains("wildcard_single")?;

    mock.clear_requests();

    let result2 = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "http://admin.wildcard.test/dashboard",
    )
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result2.assert_success()?;
    result2.assert_body_contains("wildcard_single")?;

    Ok(())
}

async fn test_matcher_wildcard_double_star() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;
    mock.set_response(200, "wildcard_double");

    let port = portpicker::pick_unused_port().unwrap();
    let _proxy = ProxyInstance::start(
        port,
        vec![&format!(
            "**.multi.level.test host://127.0.0.1:{}",
            mock.port
        )],
    )
    .await
    .map_err(|e| format!("Failed to start proxy: {}", e))?;

    tokio::time::sleep(Duration::from_millis(100)).await;

    let result = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "http://deep.nested.sub.multi.level.test/path",
    )
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result.assert_success()?;
    result.assert_body_contains("wildcard_double")?;

    Ok(())
}

async fn test_matcher_wildcard_path() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;
    mock.set_response(200, "path_wildcard");

    let port = portpicker::pick_unused_port().unwrap();
    let _proxy = ProxyInstance::start(
        port,
        vec![&format!(
            "pathwild.test/api/* host://127.0.0.1:{}",
            mock.port
        )],
    )
    .await
    .map_err(|e| format!("Failed to start proxy: {}", e))?;

    tokio::time::sleep(Duration::from_millis(100)).await;

    let result = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "http://pathwild.test/api/users",
    )
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result.assert_success()?;
    result.assert_body_contains("path_wildcard")?;

    mock.clear_requests();

    let result2 = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "http://pathwild.test/api/posts/123",
    )
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result2.assert_success()?;
    result2.assert_body_contains("path_wildcard")?;

    Ok(())
}

async fn test_matcher_regex_basic() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;
    mock.set_response(200, "regex_basic");

    let port = portpicker::pick_unused_port().unwrap();
    let _proxy = ProxyInstance::start(
        port,
        vec![&format!(
            "/regex\\d+\\.test/ host://127.0.0.1:{}",
            mock.port
        )],
    )
    .await
    .map_err(|e| format!("Failed to start proxy: {}", e))?;

    tokio::time::sleep(Duration::from_millis(100)).await;

    let result = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "http://regex123.test/path",
    )
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result.assert_success()?;
    result.assert_body_contains("regex_basic")?;

    mock.clear_requests();

    let result2 = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "http://regex999.test/another",
    )
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result2.assert_success()?;
    result2.assert_body_contains("regex_basic")?;

    Ok(())
}

async fn test_matcher_regex_case_insensitive() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;
    mock.set_response(200, "case_insensitive");

    let port = portpicker::pick_unused_port().unwrap();
    let _proxy = ProxyInstance::start(
        port,
        vec![&format!(
            "/casematch\\.test/i host://127.0.0.1:{}",
            mock.port
        )],
    )
    .await
    .map_err(|e| format!("Failed to start proxy: {}", e))?;

    tokio::time::sleep(Duration::from_millis(100)).await;

    let result_lower = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "http://casematch.test/path",
    )
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result_lower.assert_success()?;
    result_lower.assert_body_contains("case_insensitive")?;

    mock.clear_requests();

    let result_upper = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "http://CASEMATCH.TEST/path",
    )
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result_upper.assert_success()?;
    result_upper.assert_body_contains("case_insensitive")?;

    Ok(())
}

async fn test_matcher_priority_exact_over_wildcard() -> Result<(), String> {
    let mock_exact = EnhancedMockServer::start().await;
    mock_exact.set_response(200, "EXACT_MATCHED");

    let mock_wildcard = EnhancedMockServer::start().await;
    mock_wildcard.set_response(200, "WILDCARD_MATCHED");

    let port = portpicker::pick_unused_port().unwrap();
    let _proxy = ProxyInstance::start(
        port,
        vec![
            &format!("*.priority.test host://127.0.0.1:{}", mock_wildcard.port),
            &format!("exact.priority.test host://127.0.0.1:{}", mock_exact.port),
        ],
    )
    .await
    .map_err(|e| format!("Failed to start proxy: {}", e))?;

    tokio::time::sleep(Duration::from_millis(100)).await;

    let result = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "http://exact.priority.test/path",
    )
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result.assert_success()?;
    result.assert_body_contains("EXACT_MATCHED")?;

    Ok(())
}

async fn test_matcher_no_match_passthrough() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;
    mock.set_response(200, "should_not_reach");
    let origin = EnhancedMockServer::start().await;
    origin.set_response(200, "passthrough_ok");

    let port = portpicker::pick_unused_port().unwrap();
    let _proxy = ProxyInstance::start(
        port,
        vec![&format!("configured.domain host://127.0.0.1:{}", mock.port)],
    )
    .await
    .map_err(|e| format!("Failed to start proxy: {}", e))?;

    tokio::time::sleep(Duration::from_millis(100)).await;

    let result =
        CurlCommand::with_proxy(&format!("http://127.0.0.1:{}", port), &origin.url("/get"))
            .connect_timeout(10)
            .max_time(30)
            .execute()
            .await
            .map_err(|e| format!("curl failed: {}", e))?;

    result.assert_success()?;
    result.assert_body_contains("passthrough_ok")?;

    if mock.request_count() > 0 {
        return Err("Request should not have reached mock server".to_string());
    }

    Ok(())
}

async fn test_matcher_protocol_first_regex_pattern() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;
    mock.set_response(200, "protocol_first_regex");

    let port = portpicker::pick_unused_port().unwrap();
    let _proxy = ProxyInstance::start(
        port,
        vec![&format!(
            r#"host://127.0.0.1:{} /^http:\/\/regex-merge\.test\/api\/v\d+/"#,
            mock.port
        )],
    )
    .await
    .map_err(|e| format!("Failed to start proxy: {}", e))?;

    tokio::time::sleep(Duration::from_millis(100)).await;

    let result = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "http://regex-merge.test/api/v2/users",
    )
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result.assert_success()?;
    result.assert_body_contains("protocol_first_regex")?;
    mock.assert_path("/api/v2/users")?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_domain_exact() {
        let result = test_matcher_domain_exact().await;
        assert!(result.is_ok(), "Test failed: {:?}", result.err());
    }

    #[tokio::test]
    async fn test_wildcard() {
        let result = test_matcher_wildcard_single_star().await;
        assert!(result.is_ok(), "Test failed: {:?}", result.err());
    }

    #[tokio::test]
    #[ignore]
    async fn test_priority() {
        let result = test_matcher_priority_exact_over_wildcard().await;
        assert!(result.is_ok(), "Test failed: {:?}", result.err());
    }
}
