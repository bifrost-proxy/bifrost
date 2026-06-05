use crate::curl::CurlCommand;
use crate::mock::EnhancedMockServer;
use crate::proxy::ProxyInstance;
use crate::runner::TestCase;
use bifrost_admin::{AdminState, QueryParams, TrafficRecord};
use std::sync::Arc;
use std::time::Duration;

pub fn get_all_tests() -> Vec<TestCase> {
    vec![
        TestCase::standalone(
            "status_statusCode_404",
            "statusCode 返回 404",
            "status",
            test_statuscode_404,
        ),
        TestCase::standalone(
            "status_statusCode_direct_no_upstream",
            "statusCode 命中后不请求后端",
            "status",
            test_statuscode_direct_no_upstream,
        ),
        TestCase::standalone(
            "status_statusCode_500",
            "statusCode 返回 500",
            "status",
            test_statuscode_500,
        ),
        TestCase::standalone(
            "status_statusCode_200",
            "statusCode 返回 200",
            "status",
            test_statuscode_200,
        ),
        TestCase::standalone(
            "status_statusCode_with_body",
            "statusCode 配合 resBody",
            "status",
            test_statuscode_with_body,
        ),
        TestCase::standalone(
            "status_statusCode_preserves_rule_pipeline",
            "statusCode 直接响应不短路其他规则",
            "status",
            test_statuscode_preserves_rule_pipeline,
        ),
        TestCase::standalone(
            "status_replaceStatus_200",
            "replaceStatus 将后端状态码替换为 200",
            "status",
            test_replacestatus_200,
        ),
        TestCase::standalone(
            "status_redirect_302",
            "redirect 默认 302 重定向",
            "status",
            test_redirect_302,
        ),
        TestCase::standalone(
            "status_redirect_301",
            "redirect 指定 301 永久重定向",
            "status",
            test_redirect_301,
        ),
        TestCase::standalone(
            "status_redirect_307",
            "redirect 307 保持请求方法",
            "status",
            test_redirect_307,
        ),
        TestCase::standalone(
            "status_locationHref",
            "locationHref JavaScript 跳转",
            "status",
            test_locationhref,
        ),
        TestCase::standalone(
            "status_combined_statusCode_headers",
            "statusCode + resHeaders 组合",
            "status",
            test_combined_statuscode_headers,
        ),
    ]
}

async fn test_statuscode_404() -> Result<(), String> {
    let port = portpicker::pick_unused_port().unwrap();
    let _proxy = ProxyInstance::start(port, vec!["test.local statusCode://404"])
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

    result.assert_status(404)?;
    Ok(())
}

async fn test_statuscode_direct_no_upstream() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;
    mock.set_response(200, "should_not_reach");

    let port = portpicker::pick_unused_port().unwrap();
    let _proxy = ProxyInstance::start(
        port,
        vec![&format!(
            "test.local host://127.0.0.1:{} statusCode://451 resBody://(blocked)",
            mock.port
        )],
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

    result.assert_status(451)?;
    result.assert_body_contains("blocked")?;
    tokio::time::sleep(Duration::from_millis(100)).await;
    if mock.request_count() != 0 {
        return Err(format!(
            "statusCode should not contact upstream, got {} requests",
            mock.request_count()
        ));
    }

    Ok(())
}

async fn test_statuscode_500() -> Result<(), String> {
    let port = portpicker::pick_unused_port().unwrap();
    let _proxy = ProxyInstance::start(port, vec!["test.local statusCode://500"])
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

    result.assert_status(500)?;
    Ok(())
}

async fn test_statuscode_200() -> Result<(), String> {
    let port = portpicker::pick_unused_port().unwrap();
    let _proxy = ProxyInstance::start(port, vec!["test.local statusCode://200"])
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
    Ok(())
}

async fn test_statuscode_with_body() -> Result<(), String> {
    let port = portpicker::pick_unused_port().unwrap();
    let _proxy = ProxyInstance::start(
        port,
        vec!["test.local statusCode://404 resBody://(not-found)"],
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

    result.assert_status(404)?;
    result.assert_body_contains("not-found")?;
    Ok(())
}

async fn test_statuscode_preserves_rule_pipeline() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;
    mock.set_response(200, "should_not_reach");

    let port = portpicker::pick_unused_port().unwrap();
    let rules = [format!(
        "test.local host://127.0.0.1:{} statusCode://451 reqHeaders://X-Status-Pipeline=applied reqBody://(rewritten-request-body) resHeaders://X-Status-Response=applied resBody://(base) resAppend://(-tail)",
        mock.port
    )];
    let rule_refs: Vec<&str> = rules.iter().map(String::as_str).collect();
    let (_proxy, admin_state) = ProxyInstance::start_with_admin(port, rule_refs, false, false)
        .await
        .map_err(|e| format!("Failed to start proxy: {}", e))?;

    tokio::time::sleep(Duration::from_millis(100)).await;

    let result = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "http://test.local/api",
    )
    .method("POST")
    .header("Content-Type", "text/plain")
    .data("original-request-body")
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result.assert_status(451)?;
    result.assert_header("x-status-response", "applied")?;
    if result.body != "base-tail" {
        return Err(format!(
            "Expected response body rules to produce base-tail, got {:?}",
            result.body
        ));
    }
    tokio::time::sleep(Duration::from_millis(200)).await;
    if mock.request_count() != 0 {
        return Err(format!(
            "statusCode should not contact upstream, got {} requests",
            mock.request_count()
        ));
    }

    let record = get_latest_record(&admin_state).await?;
    let request_headers = record
        .request_headers
        .as_ref()
        .ok_or("traffic record missing request headers")?;
    if !request_headers
        .iter()
        .any(|(name, value)| name.eq_ignore_ascii_case("x-status-pipeline") && value == "applied")
    {
        return Err(format!(
            "Expected transformed request header in traffic record, got {:?}",
            request_headers
        ));
    }

    let body_ref = record
        .request_body_ref
        .ok_or("traffic record missing transformed request_body_ref")?;
    let Some(body_store) = admin_state.body_store.as_ref() else {
        return Err("Body store not configured".to_string());
    };
    let stored_body = body_store
        .read()
        .load(&body_ref)
        .ok_or("Failed to load transformed request body")?;
    if !stored_body.contains("rewritten-request-body") {
        return Err(format!(
            "Expected transformed request body in traffic record, got {:?}",
            stored_body
        ));
    }

    Ok(())
}

async fn test_replacestatus_200() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;
    mock.set_response(500, "server error");

    let port = portpicker::pick_unused_port().unwrap();
    let _proxy = ProxyInstance::start(
        port,
        vec![&format!(
            "test.local host://127.0.0.1:{} replaceStatus://200",
            mock.port
        )],
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
    result.assert_body_contains("server error")?;
    Ok(())
}

async fn test_redirect_302() -> Result<(), String> {
    let port = portpicker::pick_unused_port().unwrap();
    let _proxy = ProxyInstance::start(port, vec!["test.local redirect://http://new.example.com/"])
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

    result.assert_status(302)?;
    result.assert_header("location", "http://new.example.com/")?;
    Ok(())
}

async fn test_redirect_301() -> Result<(), String> {
    let port = portpicker::pick_unused_port().unwrap();
    let _proxy = ProxyInstance::start(
        port,
        vec!["test.local redirect://301:http://new.example.com/"],
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

    result.assert_status(301)?;
    result.assert_header("location", "http://new.example.com/")?;
    Ok(())
}

async fn test_redirect_307() -> Result<(), String> {
    let port = portpicker::pick_unused_port().unwrap();
    let _proxy = ProxyInstance::start(
        port,
        vec!["test.local redirect://307:http://new.example.com/"],
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

    result.assert_status(307)?;
    result.assert_header("location", "http://new.example.com/")?;
    Ok(())
}

async fn test_locationhref() -> Result<(), String> {
    let port = portpicker::pick_unused_port().unwrap();
    let _proxy = ProxyInstance::start(
        port,
        vec!["test.local locationHref://http://new.example.com/"],
    )
    .await
    .map_err(|e| format!("Failed to start proxy: {}", e))?;

    tokio::time::sleep(Duration::from_millis(100)).await;

    let result = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "http://test.local/page",
    )
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result.assert_success()?;
    result.assert_body_contains("location.href")?;
    result.assert_body_contains("http://new.example.com/")?;
    Ok(())
}

async fn test_combined_statuscode_headers() -> Result<(), String> {
    let port = portpicker::pick_unused_port().unwrap();
    let _proxy = ProxyInstance::start(
        port,
        vec!["test.local statusCode://503 resHeaders://X-Retry-After=3600"],
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

    result.assert_status(503)?;
    result.assert_header("x-retry-after", "3600")?;
    Ok(())
}

async fn get_latest_record(admin_state: &Arc<AdminState>) -> Result<TrafficRecord, String> {
    let Some(db_store) = admin_state.traffic_db_store.clone() else {
        return Err("Traffic DB not configured".to_string());
    };

    tokio::task::spawn_blocking(move || {
        let result = db_store.query(&QueryParams {
            limit: Some(1),
            ..Default::default()
        });
        let id = result
            .records
            .first()
            .map(|record| record.id.clone())
            .ok_or_else(|| "No traffic records found".to_string())?;
        db_store
            .get_by_id(&id)
            .ok_or_else(|| "Failed to get traffic record detail".to_string())
    })
    .await
    .map_err(|e| format!("spawn_blocking failed: {}", e))?
}
