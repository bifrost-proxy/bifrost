use crate::curl::CurlCommand;
use crate::mock::EnhancedMockServer;
use crate::proxy::ProxyInstance;
use crate::runner::TestCase;
use bytes::Bytes;
use http_body_util::Full;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use std::collections::HashMap;
use std::convert::Infallible;
use std::time::Duration;
use tokio::net::TcpListener;

pub fn get_all_tests() -> Vec<TestCase> {
    vec![
        TestCase::standalone(
            "body_file_inline_json",
            "file 规则返回内联 JSON",
            "body",
            test_file_inline_json,
        ),
        TestCase::standalone(
            "body_rawfile_inline",
            "rawfile 规则返回内联内容（无自动响应头）",
            "body",
            test_rawfile_inline,
        ),
        TestCase::standalone(
            "body_tpl_timestamp",
            "tpl 规则返回包含时间戳的模板",
            "body",
            test_tpl_timestamp,
        ),
        TestCase::standalone(
            "body_tpl_uuid",
            "tpl 规则返回包含 UUID 的模板",
            "body",
            test_tpl_uuid,
        ),
        TestCase::standalone(
            "body_tpl_request_info",
            "tpl 规则返回请求信息（method/path）",
            "body",
            test_tpl_request_info,
        ),
        TestCase::standalone(
            "body_reqBody_set_json",
            "reqBody 规则设置请求 Body",
            "body",
            test_reqbody_set_json,
        ),
        TestCase::standalone(
            "body_resBody_set_json",
            "resBody 规则设置响应 Body",
            "body",
            test_resbody_set_json,
        ),
        TestCase::standalone(
            "body_reqReplace_simple",
            "reqReplace 规则简单替换请求 Body",
            "body",
            test_reqreplace_simple,
        ),
        TestCase::standalone(
            "body_resReplace_simple",
            "resReplace 规则简单替换响应 Body",
            "body",
            test_resreplace_simple,
        ),
        TestCase::standalone(
            "body_resReplace_regex_global",
            "resReplace 正则全局替换",
            "body",
            test_resreplace_regex_global,
        ),
        TestCase::standalone(
            "body_reqMerge_add_field",
            "reqMerge 规则添加 JSON 字段",
            "body",
            test_reqmerge_add_field,
        ),
        TestCase::standalone(
            "body_resMerge_add_field",
            "resMerge 规则添加响应 JSON 字段",
            "body",
            test_resmerge_add_field,
        ),
        TestCase::standalone(
            "body_resAppend_content",
            "resAppend 规则在响应末尾追加内容",
            "body",
            test_resappend_content,
        ),
        TestCase::standalone(
            "body_resPrepend_content",
            "resPrepend 规则在响应开头插入内容",
            "body",
            test_resprepend_content,
        ),
        TestCase::standalone(
            "body_htmlAppend_script",
            "htmlAppend 规则在 HTML 文档结束标签前注入脚本",
            "body",
            test_htmlappend_script,
        ),
        TestCase::standalone(
            "matcher_wildcard_root_path_html_append",
            "通配域名根路径规则匹配子路径并执行 htmlAppend",
            "body",
            test_wildcard_root_path_html_append,
        ),
        TestCase::standalone(
            "body_htmlAppend_gzip_response",
            "htmlAppend 规则处理 gzip HTML 响应后仍保持有效 gzip",
            "body",
            test_htmlappend_gzip_response,
        ),
        TestCase::standalone(
            "body_htmlAppend_gzip_response_delete_encoding",
            "htmlAppend 规则处理 gzip HTML 响应且删除 Content-Encoding 后返回明文",
            "body",
            test_htmlappend_gzip_response_delete_encoding,
        ),
        TestCase::standalone(
            "body_jsAppend_code",
            "jsAppend 规则在 JS 末尾追加代码",
            "body",
            test_jsappend_code,
        ),
        TestCase::standalone(
            "body_content_injection_protocol_matrix",
            "html/js/css 内容注入协议按响应类型执行 prepend/append/body",
            "body",
            test_content_injection_protocol_matrix,
        ),
        TestCase::standalone(
            "body_content_injection_mock_resources",
            "mock/file/template 生成的响应也执行 html/js/css 内容注入",
            "body",
            test_content_injection_mock_resources,
        ),
        TestCase::standalone(
            "body_https_htmlAppend_forwarded_http",
            "HTTPS 解包转发到 HTTP 上游时 htmlAppend 仍生效",
            "body",
            test_https_htmlappend_forwarded_http,
        ),
        TestCase::standalone(
            "body_replace_skip_binary",
            "resReplace 跳过二进制类型内容",
            "body",
            test_replace_skip_binary,
        ),
    ]
}

async fn test_file_inline_json() -> Result<(), String> {
    let port = portpicker::pick_unused_port().unwrap();
    let _proxy = ProxyInstance::start(port, vec!["test.local/api file://({\"ok\":true})"])
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
    result.assert_body_contains("{\"ok\":true}")?;
    Ok(())
}

async fn test_rawfile_inline() -> Result<(), String> {
    let port = portpicker::pick_unused_port().unwrap();
    let _proxy = ProxyInstance::start(port, vec!["test.local/raw rawfile://(raw-content-here)"])
        .await
        .map_err(|e| format!("Failed to start proxy: {}", e))?;

    tokio::time::sleep(Duration::from_millis(100)).await;

    let result = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "http://test.local/raw",
    )
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result.assert_success()?;
    result.assert_body_contains("raw-content-here")?;
    Ok(())
}

async fn test_tpl_timestamp() -> Result<(), String> {
    let port = portpicker::pick_unused_port().unwrap();
    let _proxy = ProxyInstance::start(port, vec!["test.local/tpl tpl://`({\"time\":${now}})`"])
        .await
        .map_err(|e| format!("Failed to start proxy: {}", e))?;

    tokio::time::sleep(Duration::from_millis(100)).await;

    let result = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "http://test.local/tpl",
    )
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result.assert_success()?;
    result.assert_body_contains("\"time\":")?;
    let body = &result.body;
    if !body.contains(|c: char| c.is_ascii_digit()) {
        return Err("Expected timestamp in response".to_string());
    }
    Ok(())
}

async fn test_tpl_uuid() -> Result<(), String> {
    let port = portpicker::pick_unused_port().unwrap();
    let _proxy = ProxyInstance::start(
        port,
        vec!["test.local/uuid tpl://`({\"id\":\"${randomUUID}\"})`"],
    )
    .await
    .map_err(|e| format!("Failed to start proxy: {}", e))?;

    tokio::time::sleep(Duration::from_millis(100)).await;

    let result = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "http://test.local/uuid",
    )
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result.assert_success()?;
    result.assert_body_contains("\"id\":\"")?;
    if !result.body.contains("-") {
        return Err("Expected UUID format with dashes".to_string());
    }
    Ok(())
}

async fn test_tpl_request_info() -> Result<(), String> {
    let port = portpicker::pick_unused_port().unwrap();
    let _proxy = ProxyInstance::start(
        port,
        vec!["test.local/info tpl://`({\"method\":\"${method}\"})`"],
    )
    .await
    .map_err(|e| format!("Failed to start proxy: {}", e))?;

    tokio::time::sleep(Duration::from_millis(100)).await;

    let result = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "http://test.local/info",
    )
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result.assert_success()?;
    result.assert_body_contains("\"method\":\"GET\"")?;
    Ok(())
}

async fn test_reqbody_set_json() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;
    mock.set_response(200, "ok");

    let port = portpicker::pick_unused_port().unwrap();
    let _proxy = ProxyInstance::start(
        port,
        vec![&format!(
            "test.local host://127.0.0.1:{} reqBody://({{\"injected\":true}})",
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
    .method("POST")
    .data("{\"original\":1}")
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result.assert_success()?;

    let req = mock.last_request().ok_or("No request received")?;
    let body = req.body.ok_or("No body in request")?;
    if !body.contains("{\"injected\":true}") {
        return Err(format!("Expected injected body, got: {}", body));
    }
    Ok(())
}

async fn test_resbody_set_json() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;
    mock.set_response(200, "original-response");

    let port = portpicker::pick_unused_port().unwrap();
    let _proxy = ProxyInstance::start(
        port,
        vec![&format!(
            "test.local host://127.0.0.1:{} resBody://({{\"mocked\":true}})",
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
    result.assert_body_contains("{\"mocked\":true}")?;
    Ok(())
}

async fn test_reqreplace_simple() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;
    mock.set_response(200, "ok");

    let port = portpicker::pick_unused_port().unwrap();
    let _proxy = ProxyInstance::start(
        port,
        vec![&format!(
            "test.local host://127.0.0.1:{} reqReplace://old=new",
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
    .method("POST")
    .data("this is old value")
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result.assert_success()?;

    let req = mock.last_request().ok_or("No request received")?;
    let body = req.body.ok_or("No body in request")?;
    if !body.contains("this is new value") {
        return Err(format!("Expected replaced body, got: {}", body));
    }
    Ok(())
}

async fn test_resreplace_simple() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;
    mock.set_response(200, "production mode active");

    let port = portpicker::pick_unused_port().unwrap();
    let _proxy = ProxyInstance::start(
        port,
        vec![&format!(
            "test.local host://127.0.0.1:{} resReplace://production=development",
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
    result.assert_body_contains("development mode active")?;
    Ok(())
}

async fn test_resreplace_regex_global() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;
    mock.set_response(200, "aaa-bbb-aaa");

    let port = portpicker::pick_unused_port().unwrap();
    let _proxy = ProxyInstance::start(
        port,
        vec![&format!(
            "test.local host://127.0.0.1:{} resReplace://(/a/g=x)",
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
    result.assert_body_contains("xxx-bbb-xxx")?;
    Ok(())
}

async fn test_reqmerge_add_field() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;
    mock.set_response(200, "ok");

    let port = portpicker::pick_unused_port().unwrap();
    let _proxy = ProxyInstance::start(
        port,
        vec![&format!(
            "test.local host://127.0.0.1:{} reqMerge://(extra:\"added\")",
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
    .method("POST")
    .header("Content-Type", "application/json")
    .data("{\"original\":1}")
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result.assert_success()?;

    let req = mock.last_request().ok_or("No request received")?;
    let body = req.body.ok_or("No body in request")?;
    if !body.contains("extra") {
        return Err(format!("Expected merged body with 'extra', got: {}", body));
    }
    Ok(())
}

async fn test_resmerge_add_field() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;
    let mut headers = HashMap::new();
    headers.insert("Content-Type".to_string(), "application/json".to_string());
    mock.set_response_with_headers(200, "{\"data\":[]}", headers);

    let port = portpicker::pick_unused_port().unwrap();
    let _proxy = ProxyInstance::start(
        port,
        vec![&format!(
            "test.local host://127.0.0.1:{} resMerge://(proxy:true)",
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
    result.assert_body_contains("proxy")?;
    Ok(())
}

async fn test_resappend_content() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;
    mock.set_response(200, "original");

    let port = portpicker::pick_unused_port().unwrap();
    let _proxy = ProxyInstance::start(
        port,
        vec![&format!(
            "test.local host://127.0.0.1:{} resAppend://(--appended)",
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
    result.assert_body_contains("original")?;
    result.assert_body_contains("--appended")?;
    Ok(())
}

async fn test_resprepend_content() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;
    mock.set_response(200, "original");

    let port = portpicker::pick_unused_port().unwrap();
    let _proxy = ProxyInstance::start(
        port,
        vec![&format!(
            "test.local host://127.0.0.1:{} resPrepend://(prefix--)",
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
    result.assert_body_contains("prefix--")?;
    result.assert_body_contains("original")?;
    Ok(())
}

async fn test_htmlappend_script() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;
    let mut headers = HashMap::new();
    headers.insert("Content-Type".to_string(), "text/html".to_string());
    mock.set_response_with_headers(200, "<html><body>page</body></html>", headers);

    let port = portpicker::pick_unused_port().unwrap();
    let _proxy = ProxyInstance::start(
        port,
        vec![&format!(
            "test.local host://127.0.0.1:{} htmlAppend://(<script>console.log('injected')</script>)",
            mock.port
        )],
    ).await.map_err(|e| format!("Failed to start proxy: {}", e))?;

    tokio::time::sleep(Duration::from_millis(100)).await;

    let result = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "http://test.local/page.html",
    )
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result.assert_success()?;
    let script = "<script>console.log('injected')</script>";
    let script_index = result
        .body
        .rfind(script)
        .ok_or_else(|| format!("missing injected script in response: {}", result.body))?;
    let html_close_index = result
        .body
        .to_lowercase()
        .rfind("</html>")
        .ok_or_else(|| format!("missing </html> in response: {}", result.body))?;

    if script_index >= html_close_index {
        return Err(format!(
            "htmlAppend should inject before </html>; got {:?}",
            result.body
        ));
    }
    Ok(())
}

async fn test_wildcard_root_path_html_append() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;
    let mut headers = HashMap::new();
    headers.insert("Content-Type".to_string(), "text/html".to_string());
    mock.set_response_with_headers(
        200,
        "<!doctype html><html><body>qq page</body></html>",
        headers,
    );

    let port = portpicker::pick_unused_port().unwrap();
    let _proxy = ProxyInstance::start(
        port,
        vec![&format!(
            "*.qq.com/ host://127.0.0.1:{} htmlAppend://(<script>wildcardRootPath()</script>)",
            mock.port
        )],
    )
    .await
    .map_err(|e| format!("Failed to start proxy: {}", e))?;

    tokio::time::sleep(Duration::from_millis(100)).await;

    for url in [
        "http://www.qq.com",
        "http://www.qq.com/",
        "http://news.qq.com/rain/a/20260428A07D9U00",
    ] {
        let result = CurlCommand::with_proxy(&format!("http://127.0.0.1:{}", port), url)
            .execute()
            .await
            .map_err(|e| format!("curl failed for {url}: {}", e))?;

        result.assert_success()?;
        let marker = "<script>wildcardRootPath()</script>";
        let marker_index = result
            .body
            .rfind(marker)
            .ok_or_else(|| format!("missing wildcard htmlAppend for {url}: {}", result.body))?;
        let html_close_index = result
            .body
            .to_lowercase()
            .rfind("</html>")
            .ok_or_else(|| format!("missing </html> for {url}: {}", result.body))?;

        if marker_index >= html_close_index {
            return Err(format!(
                "wildcard htmlAppend should inject before </html> for {url}: {}",
                result.body
            ));
        }
    }

    Ok(())
}

async fn test_htmlappend_gzip_response() -> Result<(), String> {
    let upstream_port = start_gzip_html_server(
        "<!doctype html><html><head><title>gzip</title></head><body>page</body></html>",
    )
    .await?;

    let port = portpicker::pick_unused_port().unwrap();
    let _proxy = ProxyInstance::start(
        port,
        vec![&format!(
            "gzip-html.local host://127.0.0.1:{} htmlAppend://(<script>gzipHtmlAppend()</script>)",
            upstream_port
        )],
    )
    .await
    .map_err(|e| format!("Failed to start proxy: {}", e))?;

    tokio::time::sleep(Duration::from_millis(100)).await;

    let result = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "http://gzip-html.local/page.html",
    )
    .compressed()
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result.assert_success()?;
    result.assert_header("content-encoding", "gzip")?;
    let script = "<script>gzipHtmlAppend()</script>";
    let script_index = result
        .body
        .rfind(script)
        .ok_or_else(|| format!("missing injected script in gzip response: {}", result.body))?;
    let html_close_index = result
        .body
        .to_lowercase()
        .rfind("</html>")
        .ok_or_else(|| format!("missing </html> in gzip response: {}", result.body))?;

    if script_index >= html_close_index {
        return Err(format!(
            "gzip htmlAppend should inject before </html>: {}",
            result.body
        ));
    }

    Ok(())
}

async fn test_htmlappend_gzip_response_delete_encoding() -> Result<(), String> {
    let upstream_port = start_gzip_html_server(
        "<!doctype html><html><head><title>gzip</title></head><body>page</body></html>",
    )
    .await?;

    let port = portpicker::pick_unused_port().unwrap();
    let _proxy = ProxyInstance::start(
        port,
        vec![&format!(
            "gzip-html.local host://127.0.0.1:{} htmlAppend://(<script>gzipHtmlAppend()</script>) delete://resHeaders.Content-Encoding",
            upstream_port
        )],
    )
    .await
    .map_err(|e| format!("Failed to start proxy: {}", e))?;

    tokio::time::sleep(Duration::from_millis(100)).await;

    let result = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "http://gzip-html.local/page.html",
    )
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result.assert_success()?;
    result.assert_header_missing("content-encoding")?;
    result.assert_body_contains("<body>page")?;
    result.assert_body_contains("<script>gzipHtmlAppend()</script>")?;

    Ok(())
}

async fn start_gzip_html_server(html: &'static str) -> Result<u16, String> {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|e| format!("failed to bind gzip html server: {}", e))?;
    let port = listener
        .local_addr()
        .map_err(|e| format!("failed to read gzip html server addr: {}", e))?
        .port();
    let encoded = bifrost_proxy::transform::compress_body(html.as_bytes(), "gzip")
        .map_err(|e| format!("failed to gzip html fixture: {}", e))?;

    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                break;
            };
            let io = TokioIo::new(stream);
            let encoded = encoded.clone();

            tokio::spawn(async move {
                let service = service_fn(move |_req: Request<hyper::body::Incoming>| {
                    let encoded = encoded.clone();
                    async move {
                        Ok::<_, Infallible>(
                            Response::builder()
                                .status(StatusCode::OK)
                                .header("content-type", "text/html; charset=utf-8")
                                .header("content-encoding", "gzip")
                                .body(Full::new(Bytes::from(encoded)))
                                .unwrap(),
                        )
                    }
                });

                let _ = http1::Builder::new().serve_connection(io, service).await;
            });
        }
    });

    Ok(port)
}

async fn test_jsappend_code() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;
    let mut headers = HashMap::new();
    headers.insert(
        "Content-Type".to_string(),
        "application/javascript".to_string(),
    );
    mock.set_response_with_headers(200, "var app = {};", headers);

    let port = portpicker::pick_unused_port().unwrap();
    let _proxy = ProxyInstance::start(
        port,
        vec![&format!(
            "test.local host://127.0.0.1:{} jsAppend://(;console.log('loaded');)",
            mock.port
        )],
    )
    .await
    .map_err(|e| format!("Failed to start proxy: {}", e))?;

    tokio::time::sleep(Duration::from_millis(100)).await;

    let result = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "http://test.local/app.js",
    )
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result.assert_success()?;
    result.assert_body_contains("var app = {};")?;
    result.assert_body_contains(";console.log('loaded');")?;
    Ok(())
}

async fn test_content_injection_protocol_matrix() -> Result<(), String> {
    let upstream = ContentTypeMockServer::start().await?;
    let port = portpicker::pick_unused_port().unwrap();
    let rules = vec![
        format!(
            "html-append.local host://127.0.0.1:{}\nhtml-append.local htmlAppend://(<script>htmlAppend()</script>)",
            upstream.port
        ),
        format!(
            "html-prepend.local host://127.0.0.1:{}\nhtml-prepend.local htmlPrepend://(<!--HTML_PREPEND-->)",
            upstream.port
        ),
        format!(
            "html-body.local host://127.0.0.1:{}\nhtml-body.local htmlBody://(<main>HTML_REPLACED</main>)",
            upstream.port
        ),
        format!(
            "js-append.local host://127.0.0.1:{}\njs-append.local jsAppend://(window.appended=1;)",
            upstream.port
        ),
        format!(
            "js-prepend.local host://127.0.0.1:{}\njs-prepend.local jsPrepend://(window.prepended=1;)",
            upstream.port
        ),
        format!(
            "js-body.local host://127.0.0.1:{}\njs-body.local jsBody://(window.replaced=1;)",
            upstream.port
        ),
        format!(
            "css-append.local host://127.0.0.1:{}\ncss-append.local cssAppend://(.appended{{color:red;}})",
            upstream.port
        ),
        format!(
            "css-prepend.local host://127.0.0.1:{}\ncss-prepend.local cssPrepend://(:root{{--prepended:1;}})",
            upstream.port
        ),
        format!(
            "css-body.local host://127.0.0.1:{}\ncss-body.local cssBody://(.replaced{{color:red;}})",
            upstream.port
        ),
    ];
    let rule_refs: Vec<&str> = rules.iter().map(String::as_str).collect();
    let _proxy = ProxyInstance::start(port, rule_refs)
        .await
        .map_err(|e| format!("Failed to start proxy: {}", e))?;

    tokio::time::sleep(Duration::from_millis(100)).await;

    let html_append = fetch_body(port, "http://html-append.local/html.html").await?;
    let html_script = "<script>htmlAppend()</script>";
    let html_script_index = html_append
        .rfind(html_script)
        .ok_or_else(|| format!("missing htmlAppend script: {}", html_append))?;
    let html_close_index = html_append
        .to_lowercase()
        .rfind("</html>")
        .ok_or_else(|| format!("missing </html>: {}", html_append))?;
    if html_script_index >= html_close_index {
        return Err(format!(
            "htmlAppend should inject before </html>: {}",
            html_append
        ));
    }

    let html_prepend = fetch_body(port, "http://html-prepend.local/html.html").await?;
    if !html_prepend.starts_with("<!doctype html><html><!--HTML_PREPEND-->") {
        return Err(format!(
            "htmlPrepend should inject after <html> open tag: {}",
            html_prepend
        ));
    }

    let html_body = fetch_body(port, "http://html-body.local/html.html").await?;
    let main_index = html_body
        .find("<body><main>HTML_REPLACED</main>")
        .ok_or_else(|| {
            format!(
                "htmlBody replacement should be first body content: {}",
                html_body
            )
        })?;
    let body_close_index = html_body
        .to_lowercase()
        .rfind("</body>")
        .ok_or_else(|| format!("missing </body>: {}", html_body))?;
    if !html_body.contains("<head><title>original</title></head>")
        || main_index >= body_close_index
        || html_body.contains("HTML_ORIGINAL")
    {
        return Err(format!(
            "htmlBody should replace only the body inner HTML: {}",
            html_body
        ));
    }

    assert_body_eq(
        port,
        "http://js-append.local/js.js",
        "window.original=1;window.appended=1;",
    )
    .await?;
    assert_body_eq(
        port,
        "http://js-prepend.local/js.js",
        "window.prepended=1;window.original=1;",
    )
    .await?;
    assert_body_eq(port, "http://js-body.local/js.js", "window.replaced=1;").await?;
    assert_body_eq(
        port,
        "http://css-append.local/css.css",
        ".original{color:black;}.appended{color:red;}",
    )
    .await?;
    assert_body_eq(
        port,
        "http://css-prepend.local/css.css",
        ":root{--prepended:1;}.original{color:black;}",
    )
    .await?;
    assert_body_eq(
        port,
        "http://css-body.local/css.css",
        ".replaced{color:red;}",
    )
    .await?;

    Ok(())
}

async fn test_content_injection_mock_resources() -> Result<(), String> {
    let fixture_dir = std::env::temp_dir().join(format!(
        "bifrost-content-injection-mock-{}",
        portpicker::pick_unused_port().unwrap_or(0)
    ));
    std::fs::create_dir_all(&fixture_dir).map_err(|e| format!("create fixture dir: {}", e))?;
    let html_path = fixture_dir.join("mock.html");
    let js_path = fixture_dir.join("mock.js");
    let css_path = fixture_dir.join("mock.css");
    std::fs::write(&html_path, "<html><body>mock</body></html>")
        .map_err(|e| format!("write html fixture: {}", e))?;
    std::fs::write(&js_path, "window.mock=1;").map_err(|e| format!("write js fixture: {}", e))?;
    std::fs::write(&css_path, ".mock{color:black;}")
        .map_err(|e| format!("write css fixture: {}", e))?;

    let port = portpicker::pick_unused_port().unwrap();
    let rules = [
        format!(
            "mock-html.local file://{}\nmock-html.local htmlAppend://(<script>mockHtml()</script>)",
            html_path.display()
        ),
        format!(
            "mock-js.local file://{}\nmock-js.local jsAppend://(window.mockAppend=1;)",
            js_path.display()
        ),
        format!(
            "mock-css.local file://{}\nmock-css.local cssAppend://(.mockAppend{{color:red;}})",
            css_path.display()
        ),
    ];
    let rule_refs: Vec<&str> = rules.iter().map(String::as_str).collect();
    let _proxy = ProxyInstance::start(port, rule_refs)
        .await
        .map_err(|e| format!("Failed to start proxy: {}", e))?;

    tokio::time::sleep(Duration::from_millis(100)).await;

    let html = fetch_body(port, "http://mock-html.local/index.html").await?;
    let injected = "<script>mockHtml()</script>";
    let injected_index = html
        .rfind(injected)
        .ok_or_else(|| format!("missing mock html injection: {}", html))?;
    let html_close_index = html
        .to_lowercase()
        .rfind("</html>")
        .ok_or_else(|| format!("missing mock </html>: {}", html))?;
    if injected_index >= html_close_index {
        return Err(format!(
            "mock htmlAppend should inject before </html>: {}",
            html
        ));
    }

    assert_body_eq(
        port,
        "http://mock-js.local/app.js",
        "window.mock=1;window.mockAppend=1;",
    )
    .await?;
    assert_body_eq(
        port,
        "http://mock-css.local/style.css",
        ".mock{color:black;}.mockAppend{color:red;}",
    )
    .await?;

    Ok(())
}

struct ContentTypeMockServer {
    port: u16,
}

impl ContentTypeMockServer {
    async fn start() -> Result<Self, String> {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|e| format!("bind failed: {}", e))?;
        let port = listener
            .local_addr()
            .map_err(|e| format!("local_addr failed: {}", e))?
            .port();

        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    break;
                };
                tokio::spawn(async move {
                    let io = TokioIo::new(stream);
                    let service = service_fn(handle_content_type_request);
                    let _ = http1::Builder::new().serve_connection(io, service).await;
                });
            }
        });

        tokio::time::sleep(Duration::from_millis(50)).await;
        Ok(Self { port })
    }
}

async fn handle_content_type_request(
    req: Request<hyper::body::Incoming>,
) -> Result<Response<Full<Bytes>>, Infallible> {
    let (status, content_type, body) = match req.uri().path() {
        "/html.html" => (
            StatusCode::OK,
            "text/html",
            "<!doctype html><html><head><title>original</title></head><body>HTML_ORIGINAL</body></html>",
        ),
        "/js.js" => (
            StatusCode::OK,
            "application/javascript",
            "window.original=1;",
        ),
        "/css.css" => (StatusCode::OK, "text/css", ".original{color:black;}"),
        _ => (StatusCode::NOT_FOUND, "text/plain", "not found"),
    };

    Ok(Response::builder()
        .status(status)
        .header("Content-Type", content_type)
        .body(Full::new(Bytes::from(body)))
        .unwrap())
}

async fn fetch_body(port: u16, url: &str) -> Result<String, String> {
    let result = CurlCommand::with_proxy(&format!("http://127.0.0.1:{}", port), url)
        .execute()
        .await
        .map_err(|e| format!("curl failed: {}", e))?;
    result.assert_success()?;
    Ok(result.body)
}

async fn assert_body_eq(port: u16, url: &str, expected: &str) -> Result<(), String> {
    let body = fetch_body(port, url).await?;
    if body != expected {
        return Err(format!(
            "unexpected body for {}: expected {:?}, got {:?}",
            url, expected, body
        ));
    }
    Ok(())
}

async fn test_https_htmlappend_forwarded_http() -> Result<(), String> {
    let upstream = ContentTypeMockServer::start().await?;
    let port = portpicker::pick_unused_port().unwrap();
    let rules = [
        format!(
            "https://html-tunnel.local/ http://127.0.0.1:{}",
            upstream.port
        ),
        "https://html-tunnel.local/ htmlAppend://(<script>httpsHtmlAppend()</script>)".to_string(),
    ];
    let rule_refs: Vec<&str> = rules.iter().map(String::as_str).collect();
    let (_proxy, _admin_state) = ProxyInstance::start_with_admin(port, rule_refs, true, true)
        .await
        .map_err(|e| format!("Failed to start proxy: {}", e))?;

    tokio::time::sleep(Duration::from_millis(100)).await;

    let result = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "https://html-tunnel.local/html.html",
    )
    .insecure()
    .connect_timeout(10)
    .max_time(30)
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result.assert_success()?;
    let body = result.body.as_str();
    let injected = "<script>httpsHtmlAppend()</script>";
    let injected_index = body.rfind(injected).ok_or_else(|| {
        format!(
            "missing htmlAppend injection in HTTPS tunnel body: {}",
            body
        )
    })?;
    let html_close_index = body
        .to_lowercase()
        .rfind("</html>")
        .ok_or_else(|| format!("missing </html> in HTTPS tunnel body: {}", body))?;
    if injected_index >= html_close_index {
        return Err(format!(
            "HTTPS tunnel htmlAppend should inject before </html>: {}",
            body
        ));
    }

    Ok(())
}

async fn test_replace_skip_binary() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;
    let mut headers = HashMap::new();
    headers.insert("Content-Type".to_string(), "image/png".to_string());
    mock.set_response_with_headers(200, "PNG_IMAGE_DATA_hello_world", headers);

    let port = portpicker::pick_unused_port().unwrap();
    let _proxy = ProxyInstance::start(
        port,
        vec![&format!(
            "test.local host://127.0.0.1:{} resReplace://hello=replaced",
            mock.port
        )],
    )
    .await
    .map_err(|e| format!("Failed to start proxy: {}", e))?;

    tokio::time::sleep(Duration::from_millis(100)).await;

    let result = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "http://test.local/image.png",
    )
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result.assert_success()?;
    result.assert_body_contains("hello_world")?;
    if result.body.contains("replaced") {
        return Err("Binary content should not be replaced".to_string());
    }

    Ok(())
}
