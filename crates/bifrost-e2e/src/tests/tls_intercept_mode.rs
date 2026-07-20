use crate::curl::CurlCommand;
use crate::mock::EnhancedMockServer;
use crate::proxy::ProxyInstance;
use crate::runner::TestCase;
use std::time::Duration;

const START_PROXY_MAX_ATTEMPTS: usize = 10;

async fn start_proxy_with_owned_rules(rules: Vec<String>) -> Result<(u16, ProxyInstance), String> {
    for attempt in 1..=START_PROXY_MAX_ATTEMPTS {
        let port = portpicker::pick_unused_port().ok_or("Failed to pick unused port")?;
        let rule_refs: Vec<&str> = rules.iter().map(String::as_str).collect();
        match ProxyInstance::start(port, rule_refs).await {
            Ok(proxy) => return Ok((port, proxy)),
            Err(error)
                if is_bind_race(&error.to_string()) && attempt < START_PROXY_MAX_ATTEMPTS =>
            {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            Err(error) => return Err(format!("Failed to start proxy: {error}")),
        }
    }

    Err("Failed to start proxy after retrying port bind races".to_string())
}

fn is_bind_race(error: &str) -> bool {
    error.contains("Failed to bind") || error.contains("already listening on this port")
}

pub fn get_all_tests() -> Vec<TestCase> {
    vec![
        TestCase::standalone(
            "tls_rule_intercept_override",
            "tlsIntercept:// rule forces TLS interception",
            "tls_intercept_mode",
            test_tls_rule_intercept_override,
        ),
        TestCase::standalone(
            "tls_rule_passthrough_override",
            "tlsPassthrough:// rule forces TLS passthrough",
            "tls_intercept_mode",
            test_tls_rule_passthrough_override,
        ),
        TestCase::standalone(
            "tls_rule_intercept_with_modification",
            "tlsIntercept:// combined with request modification",
            "tls_intercept_mode",
            test_tls_rule_intercept_with_modification,
        ),
    ]
}

async fn test_tls_rule_intercept_override() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;
    mock.set_response(200, "intercepted_ok");

    let (port, _proxy) = start_proxy_with_owned_rules(vec![
        "*.force-intercept.test tlsIntercept://".to_string(),
        format!("force-intercept.test host://127.0.0.1:{}", mock.port),
    ])
    .await?;

    tokio::time::sleep(Duration::from_millis(100)).await;

    let result = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "http://force-intercept.test/api/test",
    )
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result.assert_success()?;
    result.assert_body_contains("intercepted_ok")?;
    mock.assert_request_received()?;

    Ok(())
}

async fn test_tls_rule_passthrough_override() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;
    mock.set_response(200, "passthrough_ok");

    let (port, _proxy) = start_proxy_with_owned_rules(vec![
        "*.passthrough.test tlsPassthrough://".to_string(),
        format!("passthrough.test host://127.0.0.1:{}", mock.port),
    ])
    .await?;

    tokio::time::sleep(Duration::from_millis(100)).await;

    let result = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "http://passthrough.test/api/test",
    )
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result.assert_success()?;
    result.assert_body_contains("passthrough_ok")?;
    mock.assert_request_received()?;

    Ok(())
}

async fn test_tls_rule_intercept_with_modification() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;
    mock.set_response(200, "modified_ok");

    let (port, _proxy) = start_proxy_with_owned_rules(vec![
        "*.api.test tlsIntercept:// reqHeaders://(X-Intercepted: true)".to_string(),
        format!("api.test host://127.0.0.1:{}", mock.port),
    ])
    .await?;

    tokio::time::sleep(Duration::from_millis(100)).await;

    let result = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "http://api.test/api/test",
    )
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result.assert_success()?;
    result.assert_body_contains("modified_ok")?;
    mock.assert_request_received()?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_intercept_rule() {
        let result = test_tls_rule_intercept_override().await;
        assert!(result.is_ok(), "Test failed: {:?}", result.err());
    }

    #[tokio::test]
    async fn test_passthrough_rule() {
        let result = test_tls_rule_passthrough_override().await;
        assert!(result.is_ok(), "Test failed: {:?}", result.err());
    }

    #[tokio::test]
    async fn test_intercept_with_modification() {
        let result = test_tls_rule_intercept_with_modification().await;
        assert!(result.is_ok(), "Test failed: {:?}", result.err());
    }
}
