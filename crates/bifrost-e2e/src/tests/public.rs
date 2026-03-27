use crate::mock::HttpbinMockServer;
use crate::proxy::ProxyInstance;
use crate::{assert_body_contains, assert_header_value, assert_status, ProxyClient, TestCase};
use std::time::Duration;

pub fn tests() -> Vec<TestCase> {
    vec![
        TestCase::new(
            "public_baidu_basic",
            "public",
            vec![],
            |client: ProxyClient| async move {
                let resp = client
                    .get("http://www.baidu.com/")
                    .await
                    .map_err(|e| e.to_string())?;
                assert_status(&resp, 200)?;
                let body = resp.text().await.unwrap_or_default();
                assert_body_contains(&body, "baidu")?;
                Ok(())
            },
        ),
        TestCase::new(
            "public_baidu_with_header",
            "public",
            vec!["www.baidu.com reqHeaders://X-Test-Header=bifrost-e2e"],
            |client: ProxyClient| async move {
                let resp = client
                    .get("http://www.baidu.com/")
                    .await
                    .map_err(|e| e.to_string())?;
                assert_status(&resp, 200)?;
                let body = resp.text().await.unwrap_or_default();
                assert_body_contains(&body, "baidu")?;
                Ok(())
            },
        ),
        TestCase::new(
            "public_qq_basic",
            "public",
            vec![],
            |client: ProxyClient| async move {
                let resp = client
                    .get("http://www.qq.com/")
                    .await
                    .map_err(|e| e.to_string())?;
                let status = resp.status().as_u16();
                if status == 200 || status == 301 || status == 302 {
                    Ok(())
                } else {
                    Err(format!("Expected 200/301/302, got {}", status))
                }
            },
        ),
        TestCase::new(
            "public_sina_basic",
            "public",
            vec![],
            |client: ProxyClient| async move {
                let resp = client
                    .get("http://www.sina.com.cn/")
                    .await
                    .map_err(|e| e.to_string())?;
                let status = resp.status().as_u16();
                if status == 200 || status == 301 || status == 302 {
                    Ok(())
                } else {
                    Err(format!("Expected 200/301/302, got {}", status))
                }
            },
        ),
        TestCase::standalone(
            "public_httpbin_ip",
            "Public IP endpoint via local httpbin mock",
            "public",
            test_public_httpbin_ip,
        ),
        TestCase::standalone(
            "public_httpbin_host_redirect",
            "Host redirect to local httpbin mock",
            "public",
            test_public_httpbin_host_redirect,
        ),
        TestCase::new(
            "public_cors_injection",
            "public",
            vec!["www.baidu.com resHeaders://Access-Control-Allow-Origin=*"],
            |client: ProxyClient| async move {
                let resp = client
                    .get("http://www.baidu.com/")
                    .await
                    .map_err(|e| e.to_string())?;
                assert_status(&resp, 200)?;
                assert_header_value(&resp, "access-control-allow-origin", "*")?;
                Ok(())
            },
        ),
        TestCase::new(
            "public_custom_header_injection",
            "public",
            vec!["www.baidu.com resHeaders://X-Proxy-By=bifrost, X-Test-Time: 2024"],
            |client: ProxyClient| async move {
                let resp = client
                    .get("http://www.baidu.com/")
                    .await
                    .map_err(|e| e.to_string())?;
                assert_status(&resp, 200)?;
                assert_header_value(&resp, "x-proxy-by", "bifrost")?;
                assert_header_value(&resp, "x-test-time", "2024")?;
                Ok(())
            },
        ),
        TestCase::new(
            "public_wildcard_pattern",
            "public",
            vec!["*.baidu.com resHeaders://X-Matched=wildcard"],
            |client: ProxyClient| async move {
                let resp = client
                    .get("http://www.baidu.com/")
                    .await
                    .map_err(|e| e.to_string())?;
                assert_status(&resp, 200)?;
                assert_header_value(&resp, "x-matched", "wildcard")?;
                Ok(())
            },
        ),
        TestCase::new(
            "public_multi_site_rule",
            "public",
            vec![
                "www.baidu.com resHeaders://X-Site=baidu",
                "httpbin.org resHeaders://X-Site=httpbin",
            ],
            |client: ProxyClient| async move {
                let resp1 = client
                    .get("http://www.baidu.com/")
                    .await
                    .map_err(|e| e.to_string())?;
                assert_status(&resp1, 200)?;
                assert_header_value(&resp1, "x-site", "baidu")?;

                let resp2 = client
                    .get("http://httpbin.org/get")
                    .await
                    .map_err(|e| e.to_string())?;
                assert_status(&resp2, 200)?;
                assert_header_value(&resp2, "x-site", "httpbin")?;
                Ok(())
            },
        ),
    ]
}

async fn test_public_httpbin_ip() -> Result<(), String> {
    let mock = HttpbinMockServer::start().await;
    let port = portpicker::pick_unused_port().unwrap();
    let rules = mock.http_rules();
    let rule_refs: Vec<&str> = rules.iter().map(String::as_str).collect();
    let _proxy = ProxyInstance::start(port, rule_refs)
        .await
        .map_err(|e| format!("Failed to start proxy: {e}"))?;

    tokio::time::sleep(Duration::from_millis(100)).await;

    let client =
        ProxyClient::new(&format!("http://127.0.0.1:{port}")).map_err(|e| e.to_string())?;
    let json = client
        .get_json("http://httpbin.org/ip")
        .await
        .map_err(|e| e.to_string())?;

    if json.get("origin").is_some() {
        Ok(())
    } else {
        Err("Expected 'origin' field in response".to_string())
    }
}

async fn test_public_httpbin_host_redirect() -> Result<(), String> {
    let mock = HttpbinMockServer::start().await;
    let port = portpicker::pick_unused_port().unwrap();
    let mut rules = mock.http_rules();
    rules.push("my-test-domain.local host://httpbin.org".to_string());
    let rule_refs: Vec<&str> = rules.iter().map(String::as_str).collect();
    let _proxy = ProxyInstance::start(port, rule_refs)
        .await
        .map_err(|e| format!("Failed to start proxy: {e}"))?;

    tokio::time::sleep(Duration::from_millis(100)).await;

    let client =
        ProxyClient::new(&format!("http://127.0.0.1:{port}")).map_err(|e| e.to_string())?;
    let json = client
        .get_json("http://my-test-domain.local/get")
        .await
        .map_err(|e| e.to_string())?;

    if json.get("url").is_some() {
        Ok(())
    } else {
        Err("Expected httpbin response with 'url' field".to_string())
    }
}
