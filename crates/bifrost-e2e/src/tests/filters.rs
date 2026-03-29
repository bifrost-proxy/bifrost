use crate::curl::CurlCommand;
use crate::mock::EnhancedMockServer;
use crate::proxy::ProxyInstance;
use crate::runner::TestCase;
use std::collections::HashMap;

pub fn get_all_tests() -> Vec<TestCase> {
    vec![
        TestCase::standalone(
            "filters_includeFilter_method_post",
            "includeFilter 方法过滤 - POST 请求应用规则",
            "filters",
            test_includefilter_method_post,
        ),
        TestCase::standalone(
            "filters_includeFilter_method_get_not_match",
            "includeFilter 方法过滤 - GET 请求不应用",
            "filters",
            test_includefilter_method_get_not_match,
        ),
        TestCase::standalone(
            "filters_includeFilter_status_500",
            "includeFilter 状态码过滤 - 500 时替换状态",
            "filters",
            test_includefilter_status_500,
        ),
        TestCase::standalone(
            "filters_includeFilter_header",
            "includeFilter 请求头过滤",
            "filters",
            test_includefilter_header,
        ),
        TestCase::standalone(
            "filters_excludeFilter_method_get",
            "excludeFilter 排除 GET 请求",
            "filters",
            test_excludefilter_method_get,
        ),
        TestCase::standalone(
            "filters_excludeFilter_method_post_apply",
            "excludeFilter 排除 GET - POST 请求应用",
            "filters",
            test_excludefilter_method_post_apply,
        ),
        TestCase::standalone(
            "filters_delete_reqHeader",
            "delete 删除请求头",
            "filters",
            test_delete_reqheader,
        ),
        TestCase::standalone(
            "filters_delete_resHeader",
            "delete 删除响应头",
            "filters",
            test_delete_resheader,
        ),
        TestCase::standalone(
            "filters_delete_urlParams",
            "delete 删除 URL 参数",
            "filters",
            test_delete_urlparams,
        ),
        TestCase::standalone(
            "filters_enable_abort",
            "enable://abort 中断请求",
            "filters",
            test_enable_abort,
        ),
    ]
}

async fn test_includefilter_method_post() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;
    mock.set_response(200, "ok");

    let port = portpicker::pick_unused_port().unwrap();
    let _proxy = ProxyInstance::start(
        port,
        vec![&format!(
            "test.local host://127.0.0.1:{} resHeaders://X-Method=POST includeFilter://m:POST",
            mock.port
        )],
    )
    .await
    .map_err(|e| format!("Failed to start proxy: {}", e))?;

    let result = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "http://test.local/api",
    )
    .method("POST")
    .data("test")
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result.assert_success()?;
    result.assert_header("x-method", "POST")?;
    Ok(())
}

async fn test_includefilter_method_get_not_match() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;
    mock.set_response(200, "ok");

    let port = portpicker::pick_unused_port().unwrap();
    let _proxy = ProxyInstance::start(
        port,
        vec![&format!(
            "test.local host://127.0.0.1:{} resHeaders://X-Method=POST includeFilter://m:POST",
            mock.port
        )],
    )
    .await
    .map_err(|e| format!("Failed to start proxy: {}", e))?;

    let result = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "http://test.local/api",
    )
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result.assert_success()?;
    if result.headers.contains_key("x-method") {
        return Err("Header X-Method should not be present for GET request".to_string());
    }
    Ok(())
}

async fn test_includefilter_status_500() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;
    mock.set_response(500, "server error");

    let port = portpicker::pick_unused_port().unwrap();
    let _proxy = ProxyInstance::start(
        port,
        vec![&format!(
            "test.local host://127.0.0.1:{} replaceStatus://200 includeFilter://s:500",
            mock.port
        )],
    )
    .await
    .map_err(|e| format!("Failed to start proxy: {}", e))?;

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

async fn test_includefilter_header() -> Result<(), String> {
    let mock1 = EnhancedMockServer::start().await;
    mock1.set_response(200, "debug_server");
    let mock2 = EnhancedMockServer::start().await;
    mock2.set_response(200, "normal_server");

    let port = portpicker::pick_unused_port().unwrap();
    let _proxy = ProxyInstance::start(
        port,
        vec![
            &format!(
                "test.local host://127.0.0.1:{} includeFilter://h:X-Debug=true",
                mock1.port
            ),
            &format!("test.local host://127.0.0.1:{}", mock2.port),
        ],
    )
    .await
    .map_err(|e| format!("Failed to start proxy: {}", e))?;

    let result_with_header = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "http://test.local/api",
    )
    .header("X-Debug", "true")
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result_with_header.assert_success()?;
    result_with_header.assert_body_contains("debug_server")?;

    let result_without_header = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "http://test.local/api",
    )
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result_without_header.assert_success()?;
    result_without_header.assert_body_contains("normal_server")?;

    Ok(())
}

async fn test_excludefilter_method_get() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;
    mock.set_response(200, "ok");

    let port = portpicker::pick_unused_port().unwrap();
    let _proxy = ProxyInstance::start(
        port,
        vec![&format!(
            "test.local host://127.0.0.1:{} resHeaders://X-Applied=true excludeFilter://m:GET",
            mock.port
        )],
    )
    .await
    .map_err(|e| format!("Failed to start proxy: {}", e))?;

    let result = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "http://test.local/api",
    )
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result.assert_success()?;
    if result.headers.contains_key("x-applied") {
        return Err("Header X-Applied should not be present for GET (excluded)".to_string());
    }
    Ok(())
}

async fn test_excludefilter_method_post_apply() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;
    mock.set_response(200, "ok");

    let port = portpicker::pick_unused_port().unwrap();
    let _proxy = ProxyInstance::start(
        port,
        vec![&format!(
            "test.local host://127.0.0.1:{} resHeaders://X-Applied=true excludeFilter://m:GET",
            mock.port
        )],
    )
    .await
    .map_err(|e| format!("Failed to start proxy: {}", e))?;

    let result = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "http://test.local/api",
    )
    .method("POST")
    .data("test")
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result.assert_success()?;
    result.assert_header("x-applied", "true")?;
    Ok(())
}

async fn test_delete_reqheader() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;
    mock.set_response(200, "ok");

    let port = portpicker::pick_unused_port().unwrap();
    let _proxy = ProxyInstance::start(
        port,
        vec![&format!(
            "test.local host://127.0.0.1:{} delete://reqHeaders.X-Custom",
            mock.port
        )],
    )
    .await
    .map_err(|e| format!("Failed to start proxy: {}", e))?;

    let _result = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "http://test.local/api",
    )
    .header("X-Custom", "should-be-deleted")
    .header("X-Keep", "should-remain")
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    let request = mock
        .last_request()
        .ok_or_else(|| "No request received by mock server".to_string())?;

    if request
        .headers
        .iter()
        .any(|(k, _)| k.to_lowercase() == "x-custom")
    {
        return Err("X-Custom header should be deleted".to_string());
    }
    if !request
        .headers
        .iter()
        .any(|(k, _)| k.to_lowercase() == "x-keep")
    {
        return Err("X-Keep header should remain".to_string());
    }
    Ok(())
}

async fn test_delete_resheader() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;
    let mut headers = HashMap::new();
    headers.insert("X-Powered-By".to_string(), "Whistle".to_string());
    headers.insert("X-Keep".to_string(), "remain".to_string());
    mock.set_response_with_headers(200, "ok", headers);

    let port = portpicker::pick_unused_port().unwrap();
    let _proxy = ProxyInstance::start(
        port,
        vec![&format!(
            "test.local host://127.0.0.1:{} delete://resHeaders.X-Powered-By",
            mock.port
        )],
    )
    .await
    .map_err(|e| format!("Failed to start proxy: {}", e))?;

    let result = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "http://test.local/api",
    )
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result.assert_success()?;
    if result.headers.contains_key("x-powered-by") {
        return Err("X-Powered-By header should be deleted".to_string());
    }
    result.assert_header("x-keep", "remain")?;
    Ok(())
}

async fn test_delete_urlparams() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;
    mock.set_response(200, "ok");

    let port = portpicker::pick_unused_port().unwrap();
    let _proxy = ProxyInstance::start(
        port,
        vec![&format!(
            "test.local host://127.0.0.1:{} delete://urlParams.debug",
            mock.port
        )],
    )
    .await
    .map_err(|e| format!("Failed to start proxy: {}", e))?;

    let _result = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "http://test.local/api?debug=true&keep=yes",
    )
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    let request = mock
        .last_request()
        .ok_or_else(|| "No request received by mock server".to_string())?;

    let query = request.query.unwrap_or_default();
    if query.contains("debug=true") {
        return Err("debug parameter should be deleted".to_string());
    }
    if !query.contains("keep=yes") {
        return Err("keep parameter should remain".to_string());
    }
    Ok(())
}

async fn test_enable_abort() -> Result<(), String> {
    let port = portpicker::pick_unused_port().unwrap();
    let _proxy = ProxyInstance::start(port, vec!["test.local enable://abort"])
        .await
        .map_err(|e| format!("Failed to start proxy: {}", e))?;

    let result = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "http://test.local/api",
    )
    .execute()
    .await;

    match result {
        Err(_) => Ok(()),
        Ok(res) => {
            if res.http_code.map(|c| c == 0 || c >= 500).unwrap_or(true) {
                Ok(())
            } else {
                Err(format!(
                    "Expected request to be aborted, got status {:?}",
                    res.http_code
                ))
            }
        }
    }
}
