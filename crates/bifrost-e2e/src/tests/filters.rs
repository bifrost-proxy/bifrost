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
            "filters_includeFilter_header_route_split_cache_regression",
            "includeFilter 请求头分流缓存回归",
            "filters",
            test_includefilter_header_route_split_cache_regression,
        ),
        TestCase::standalone(
            "filters_includeFilter_header_modify_cache_regression",
            "includeFilter 请求头修改缓存回归",
            "filters",
            test_includefilter_header_modify_cache_regression,
        ),
        TestCase::standalone(
            "filters_includeFilter_header_redirect_cache_regression",
            "includeFilter 请求头重定向缓存回归",
            "filters",
            test_includefilter_header_redirect_cache_regression,
        ),
        TestCase::standalone(
            "filters_excludeFilter_header_cache_regression",
            "excludeFilter 请求头缓存回归",
            "filters",
            test_excludefilter_header_cache_regression,
        ),
        TestCase::standalone(
            "filters_skip_header_cache_regression",
            "skip 请求头缓存回归",
            "filters",
            test_skip_header_cache_regression,
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

async fn test_includefilter_header_route_split_cache_regression() -> Result<(), String> {
    let web_mock = EnhancedMockServer::start().await;
    web_mock.set_response(200, "feature_web_server");
    let mobile_mock = EnhancedMockServer::start().await;
    mobile_mock.set_response(200, "feature_mobile_server");
    let fallback_mock = EnhancedMockServer::start().await;
    fallback_mock.set_response(200, "feature_fallback_server");

    let port = portpicker::pick_unused_port().unwrap();
    let _proxy = ProxyInstance::start(
        port,
        vec![
            &format!(
                "multi-demand.local host://127.0.0.1:{} includeFilter://h:x-feature-env=feature-web",
                web_mock.port
            ),
            &format!(
                "multi-demand.local host://127.0.0.1:{} includeFilter://h:x-feature-env=feature-mobile",
                mobile_mock.port
            ),
            &format!("multi-demand.local host://127.0.0.1:{}", fallback_mock.port),
        ],
    )
    .await
    .map_err(|e| format!("Failed to start proxy: {}", e))?;

    let proxy_url = format!("http://127.0.0.1:{}", port);
    let target_url = "http://multi-demand.local/api";

    let mobile_result = CurlCommand::with_proxy(&proxy_url, target_url)
        .header("x-feature-env", "feature-mobile")
        .execute()
        .await
        .map_err(|e| format!("curl failed: {}", e))?;

    mobile_result.assert_success()?;
    mobile_result.assert_body_contains("feature_mobile_server")?;

    let fallback_result = CurlCommand::with_proxy(&proxy_url, target_url)
        .header("x-feature-env", "feature-unknown")
        .execute()
        .await
        .map_err(|e| format!("curl failed: {}", e))?;

    fallback_result.assert_success()?;
    fallback_result.assert_body_contains("feature_fallback_server")?;

    let web_result = CurlCommand::with_proxy(&proxy_url, target_url)
        .header("x-feature-env", "feature-web")
        .execute()
        .await
        .map_err(|e| format!("curl failed: {}", e))?;

    web_result.assert_success()?;
    web_result.assert_body_contains("feature_web_server")?;

    let mobile_again = CurlCommand::with_proxy(&proxy_url, target_url)
        .header("x-feature-env", "feature-mobile")
        .execute()
        .await
        .map_err(|e| format!("curl failed: {}", e))?;

    mobile_again.assert_success()?;
    mobile_again.assert_body_contains("feature_mobile_server")?;

    Ok(())
}

async fn test_includefilter_header_modify_cache_regression() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;
    mock.set_response(200, "modify_server");

    let port = portpicker::pick_unused_port().unwrap();
    let _proxy = ProxyInstance::start(
        port,
        vec![
            &format!("multi-modify.local host://127.0.0.1:{}", mock.port),
            "multi-modify.local reqHeaders://X-Selected-Env=web includeFilter://h:x-feature-env=feature-web",
            "multi-modify.local reqHeaders://X-Selected-Env=mobile includeFilter://h:x-feature-env=feature-mobile",
            "multi-modify.local resHeaders://X-Selected-Env=web includeFilter://h:x-feature-env=feature-web",
            "multi-modify.local resHeaders://X-Selected-Env=mobile includeFilter://h:x-feature-env=feature-mobile",
            "multi-modify.local replaceStatus://201 includeFilter://h:x-feature-env=feature-web",
            "multi-modify.local replaceStatus://202 includeFilter://h:x-feature-env=feature-mobile",
        ],
    )
    .await
    .map_err(|e| format!("Failed to start proxy: {}", e))?;

    let proxy_url = format!("http://127.0.0.1:{}", port);
    let target_url = "http://multi-modify.local/api";

    let mobile_result = CurlCommand::with_proxy(&proxy_url, target_url)
        .header("x-feature-env", "feature-mobile")
        .execute()
        .await
        .map_err(|e| format!("curl failed: {}", e))?;

    mobile_result.assert_status(202)?;
    mobile_result.assert_header("x-selected-env", "mobile")?;
    mock.assert_header_received("x-selected-env", "mobile")?;

    let web_result = CurlCommand::with_proxy(&proxy_url, target_url)
        .header("x-feature-env", "feature-web")
        .execute()
        .await
        .map_err(|e| format!("curl failed: {}", e))?;

    web_result.assert_status(201)?;
    web_result.assert_header("x-selected-env", "web")?;
    mock.assert_header_received("x-selected-env", "web")?;

    let mobile_again = CurlCommand::with_proxy(&proxy_url, target_url)
        .header("x-feature-env", "feature-mobile")
        .execute()
        .await
        .map_err(|e| format!("curl failed: {}", e))?;

    mobile_again.assert_status(202)?;
    mobile_again.assert_header("x-selected-env", "mobile")?;
    mock.assert_header_received("x-selected-env", "mobile")?;

    Ok(())
}

async fn test_includefilter_header_redirect_cache_regression() -> Result<(), String> {
    let port = portpicker::pick_unused_port().unwrap();
    let _proxy = ProxyInstance::start(
        port,
        vec![
            "multi-direct.local redirect://302:https://web.example.test/landing includeFilter://h:x-feature-env=feature-web",
            "multi-direct.local redirect://302:https://mobile.example.test/landing includeFilter://h:x-feature-env=feature-mobile",
            "multi-status.local statusCode://201 includeFilter://h:x-feature-env=feature-web",
            "multi-status.local statusCode://202 includeFilter://h:x-feature-env=feature-mobile",
        ],
    )
    .await
    .map_err(|e| format!("Failed to start proxy: {}", e))?;

    let proxy_url = format!("http://127.0.0.1:{}", port);
    let target_url = "http://multi-direct.local/api";

    let mobile_result = CurlCommand::with_proxy(&proxy_url, target_url)
        .header("x-feature-env", "feature-mobile")
        .execute()
        .await
        .map_err(|e| format!("curl failed: {}", e))?;

    mobile_result.assert_status(302)?;
    mobile_result.assert_header("location", "https://mobile.example.test/landing")?;

    let web_result = CurlCommand::with_proxy(&proxy_url, target_url)
        .header("x-feature-env", "feature-web")
        .execute()
        .await
        .map_err(|e| format!("curl failed: {}", e))?;

    web_result.assert_status(302)?;
    web_result.assert_header("location", "https://web.example.test/landing")?;

    let mobile_again = CurlCommand::with_proxy(&proxy_url, target_url)
        .header("x-feature-env", "feature-mobile")
        .execute()
        .await
        .map_err(|e| format!("curl failed: {}", e))?;

    mobile_again.assert_status(302)?;
    mobile_again.assert_header("location", "https://mobile.example.test/landing")?;

    let mobile_status = CurlCommand::with_proxy(&proxy_url, "http://multi-status.local/api")
        .header("x-feature-env", "feature-mobile")
        .execute()
        .await
        .map_err(|e| format!("curl failed: {}", e))?;

    mobile_status.assert_status(202)?;

    let web_status = CurlCommand::with_proxy(&proxy_url, "http://multi-status.local/api")
        .header("x-feature-env", "feature-web")
        .execute()
        .await
        .map_err(|e| format!("curl failed: {}", e))?;

    web_status.assert_status(201)?;

    let mobile_status_again = CurlCommand::with_proxy(&proxy_url, "http://multi-status.local/api")
        .header("x-feature-env", "feature-mobile")
        .execute()
        .await
        .map_err(|e| format!("curl failed: {}", e))?;

    mobile_status_again.assert_status(202)?;

    Ok(())
}

async fn test_excludefilter_header_cache_regression() -> Result<(), String> {
    let primary_mock = EnhancedMockServer::start().await;
    primary_mock.set_response(200, "exclude_primary_server");
    let fallback_mock = EnhancedMockServer::start().await;
    fallback_mock.set_response(200, "exclude_fallback_server");

    let port = portpicker::pick_unused_port().unwrap();
    let _proxy = ProxyInstance::start(
        port,
        vec![
            &format!(
                "exclude-cache.local host://127.0.0.1:{} excludeFilter://h:x-block=true",
                primary_mock.port
            ),
            &format!(
                "exclude-cache.local host://127.0.0.1:{}",
                fallback_mock.port
            ),
        ],
    )
    .await
    .map_err(|e| format!("Failed to start proxy: {}", e))?;

    let proxy_url = format!("http://127.0.0.1:{}", port);
    let target_url = "http://exclude-cache.local/api";

    let blocked = CurlCommand::with_proxy(&proxy_url, target_url)
        .header("x-block", "true")
        .execute()
        .await
        .map_err(|e| format!("curl failed: {}", e))?;
    blocked.assert_success()?;
    blocked.assert_body_contains("exclude_fallback_server")?;

    let allowed = CurlCommand::with_proxy(&proxy_url, target_url)
        .execute()
        .await
        .map_err(|e| format!("curl failed: {}", e))?;
    allowed.assert_success()?;
    allowed.assert_body_contains("exclude_primary_server")?;

    let blocked_again = CurlCommand::with_proxy(&proxy_url, target_url)
        .header("x-block", "true")
        .execute()
        .await
        .map_err(|e| format!("curl failed: {}", e))?;
    blocked_again.assert_success()?;
    blocked_again.assert_body_contains("exclude_fallback_server")?;

    Ok(())
}

async fn test_skip_header_cache_regression() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;
    mock.set_response(200, "skip_server");

    let port = portpicker::pick_unused_port().unwrap();
    let _proxy = ProxyInstance::start(
        port,
        vec![
            &format!("skip-cache.local host://127.0.0.1:{}", mock.port),
            "skip-cache.local resHeaders://X-Skip-Target=selected",
            "skip-cache.local skip://operation=resHeaders://X-Skip-Target=selected includeFilter://h:x-skip=true",
        ],
    )
    .await
    .map_err(|e| format!("Failed to start proxy: {}", e))?;

    let proxy_url = format!("http://127.0.0.1:{}", port);
    let target_url = "http://skip-cache.local/api";

    let skipped = CurlCommand::with_proxy(&proxy_url, target_url)
        .header("x-skip", "true")
        .execute()
        .await
        .map_err(|e| format!("curl failed: {}", e))?;
    skipped.assert_success()?;
    skipped.assert_header_missing("x-skip-target")?;

    let selected = CurlCommand::with_proxy(&proxy_url, target_url)
        .execute()
        .await
        .map_err(|e| format!("curl failed: {}", e))?;
    selected.assert_success()?;
    selected.assert_header("x-skip-target", "selected")?;

    let skipped_again = CurlCommand::with_proxy(&proxy_url, target_url)
        .header("x-skip", "true")
        .execute()
        .await
        .map_err(|e| format!("curl failed: {}", e))?;
    skipped_again.assert_success()?;
    skipped_again.assert_header_missing("x-skip-target")?;

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
