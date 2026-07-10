mod common;

use bifrost_core::Protocol;
use bifrost_proxy::ProxyConfig;
use common::{
    add_test_rule, clear_test_rules, create_proxy_client, start_test_proxy, MockHttpServer,
};

#[tokio::test]
async fn test_simple_http_request() {
    let mock_server = MockHttpServer::start().await;
    let proxy = start_test_proxy().await;
    let client = create_proxy_client(&proxy);

    let response = client
        .get(mock_server.url("/api/test"))
        .send()
        .await
        .expect("Failed to send request");

    assert!(response.status().is_success());
    let body = response.text().await.unwrap();
    assert!(body.contains("\"path\":\"/api/test\""));
    assert!(body.contains("\"method\":\"GET\""));
}

#[tokio::test]
async fn test_http_post_request() {
    let mock_server = MockHttpServer::start().await;
    let proxy = start_test_proxy().await;
    let client = create_proxy_client(&proxy);

    let response = client
        .post(mock_server.url("/api/data"))
        .body("test body")
        .send()
        .await
        .expect("Failed to send POST request");

    assert!(response.status().is_success());
    let body = response.text().await.unwrap();
    assert!(body.contains("\"method\":\"POST\""));
}

#[tokio::test]
async fn test_http_with_rules() {
    let mock_server = MockHttpServer::start().await;
    let proxy = start_test_proxy().await;

    add_test_rule(
        &proxy,
        "*",
        Protocol::ReqHeaders,
        "X-Custom-Header=TestValue",
    );

    let client = create_proxy_client(&proxy);
    let response = client
        .get(mock_server.url("/api/test"))
        .send()
        .await
        .expect("Failed to send request");

    assert!(response.status().is_success());
    let body = response.text().await.unwrap();
    assert!(body.contains("x-custom-header: TestValue") || body.contains("X-Custom-Header"));
}

#[tokio::test]
async fn test_host_replacement() {
    let mock_server = MockHttpServer::start().await;
    let proxy = start_test_proxy().await;

    let target = format!("127.0.0.1:{}", mock_server.port);
    add_test_rule(&proxy, "example.com", Protocol::Host, &target);

    let client = create_proxy_client(&proxy);
    let response = client
        .get(format!("http://example.com:{}/api/test", mock_server.port))
        .send()
        .await
        .expect("Failed to send request with host replacement");

    assert!(response.status().is_success());
}

#[tokio::test]
async fn test_req_headers_injection() {
    let mock_server = MockHttpServer::start().await;
    let proxy = start_test_proxy().await;

    add_test_rule(
        &proxy,
        "*",
        Protocol::ReqHeaders,
        "Authorization=Bearer test-token",
    );
    add_test_rule(&proxy, "*", Protocol::ReqHeaders, "X-Request-ID=12345");

    let client = create_proxy_client(&proxy);
    let response = client
        .get(mock_server.url("/api/protected"))
        .send()
        .await
        .expect("Failed to send request");

    assert!(response.status().is_success());
    let body = response.text().await.unwrap();
    assert!(
        body.to_lowercase().contains("authorization") || body.to_lowercase().contains("bearer")
    );
}

#[tokio::test]
async fn test_res_headers_injection() {
    let mock_server = MockHttpServer::start().await;
    let proxy = start_test_proxy().await;

    add_test_rule(
        &proxy,
        "*",
        Protocol::ResHeaders,
        "X-Response-Header=InjectedValue",
    );
    add_test_rule(&proxy, "*", Protocol::ResHeaders, "Cache-Control=no-cache");

    let client = create_proxy_client(&proxy);
    let response = client
        .get(mock_server.url("/api/test"))
        .send()
        .await
        .expect("Failed to send request");

    assert!(response.status().is_success());

    let x_response_header = response.headers().get("X-Response-Header");
    let cache_control = response.headers().get("Cache-Control");

    assert!(
        x_response_header.is_some() || cache_control.is_some(),
        "Response headers should be injected"
    );
}

#[tokio::test]
async fn test_multiple_rules_combined() {
    let mock_server = MockHttpServer::start().await;
    let proxy = start_test_proxy().await;

    add_test_rule(&proxy, "*", Protocol::ReqHeaders, "X-Test=1");
    add_test_rule(&proxy, "*", Protocol::ResHeaders, "X-Proxy=bifrost");
    add_test_rule(&proxy, "*", Protocol::Ua, "TestBot/1.0");

    let client = create_proxy_client(&proxy);
    let response = client
        .get(mock_server.url("/api/test"))
        .send()
        .await
        .expect("Failed to send request");

    assert!(response.status().is_success());
}

#[tokio::test]
async fn test_clear_rules() {
    let mock_server = MockHttpServer::start().await;
    let proxy = start_test_proxy().await;

    add_test_rule(&proxy, "*", Protocol::ReqHeaders, "X-Should-Exist=yes");

    let client = create_proxy_client(&proxy);
    let response1 = client
        .get(mock_server.url("/api/test"))
        .send()
        .await
        .unwrap();
    assert!(response1.status().is_success());

    clear_test_rules(&proxy);

    let response2 = client
        .get(mock_server.url("/api/test"))
        .send()
        .await
        .unwrap();
    assert!(response2.status().is_success());
}

#[tokio::test]
async fn test_http_request_with_query_params() {
    let mock_server = MockHttpServer::start().await;
    let proxy = start_test_proxy().await;
    let client = create_proxy_client(&proxy);

    let response = client
        .get(mock_server.url("/api/search?q=test&page=1"))
        .send()
        .await
        .expect("Failed to send request");

    assert!(response.status().is_success());
}

#[tokio::test]
async fn test_http_request_with_custom_headers() {
    let mock_server = MockHttpServer::start().await;
    let proxy = start_test_proxy().await;
    let client = create_proxy_client(&proxy);

    let response = client
        .get(mock_server.url("/api/test"))
        .header("Accept", "application/json")
        .header("X-Custom", "value")
        .send()
        .await
        .expect("Failed to send request");

    assert!(response.status().is_success());
    let body = response.text().await.unwrap();
    assert!(body.to_lowercase().contains("accept"));
}

#[tokio::test]
async fn test_http_put_request() {
    let mock_server = MockHttpServer::start().await;
    let proxy = start_test_proxy().await;
    let client = create_proxy_client(&proxy);

    let response = client
        .put(mock_server.url("/api/resource/1"))
        .body(r#"{"name": "updated"}"#)
        .send()
        .await
        .expect("Failed to send PUT request");

    assert!(response.status().is_success());
    let body = response.text().await.unwrap();
    assert!(body.contains("\"method\":\"PUT\""));
}

#[tokio::test]
async fn test_http_delete_request() {
    let mock_server = MockHttpServer::start().await;
    let proxy = start_test_proxy().await;
    let client = create_proxy_client(&proxy);

    let response = client
        .delete(mock_server.url("/api/resource/1"))
        .send()
        .await
        .expect("Failed to send DELETE request");

    assert!(response.status().is_success());
    let body = response.text().await.unwrap();
    assert!(body.contains("\"method\":\"DELETE\""));
}

#[tokio::test]
async fn test_proxy_keeps_connection_alive() {
    let mock_server = MockHttpServer::start().await;
    let proxy = start_test_proxy().await;
    let client = create_proxy_client(&proxy);

    for i in 0..5 {
        let response = client
            .get(mock_server.url(&format!("/api/test/{}", i)))
            .send()
            .await
            .expect("Failed to send request");
        assert!(response.status().is_success());
    }
}

#[test]
fn test_proxy_config_default() {
    let config = ProxyConfig::default();
    assert_eq!(config.port, 9900);
    assert_eq!(config.host, "127.0.0.1");
    assert!(!config.enable_tls_interception);
    assert!(config.intercept_exclude.is_empty());
    assert!(config.intercept_include.is_empty());
    assert_eq!(config.http1_max_header_size, 64 * 1024);
    assert_eq!(config.http2_max_header_list_size, 256 * 1024);
    assert_eq!(config.websocket_handshake_max_header_size, 64 * 1024);
    assert!(!config.super_performance_mode);
    assert!(config.binary_traffic_performance_mode);
}

#[test]
fn test_proxy_config_custom() {
    let config = ProxyConfig {
        port: 9000,
        host: "0.0.0.0".to_string(),
        enable_tls_interception: true,
        intercept_exclude: vec!["*.internal.com".to_string()],
        intercept_include: vec![],
        app_intercept_exclude: vec![],
        app_intercept_include: vec![],
        ip_intercept_exclude: vec![],
        ip_intercept_include: vec![],
        timeout_secs: 60,
        http1_max_header_size: 128 * 1024,
        http2_max_header_list_size: 512 * 1024,
        websocket_handshake_max_header_size: 96 * 1024,
        socks5_port: Some(1080),
        socks5_auth_required: false,
        socks5_username: None,
        socks5_password: None,
        verbose_logging: false,
        access_mode: bifrost_core::AccessMode::AllowAll,
        client_whitelist: vec![],
        allow_lan: true,
        unsafe_ssl: false,
        max_body_buffer_size: 10 * 1024 * 1024,
        max_body_probe_size: 64 * 1024,
        super_performance_mode: false,
        binary_traffic_performance_mode: true,
        inject_bifrost_badge: true,
        enable_socks: true,
        userpass_auth: None,
        userpass_last_connected_at: std::collections::HashMap::new(),
    };
    assert_eq!(config.port, 9000);
    assert!(config.enable_tls_interception);
    assert_eq!(config.socks5_port, Some(1080));
    assert_eq!(config.http1_max_header_size, 128 * 1024);
    assert_eq!(config.http2_max_header_list_size, 512 * 1024);
    assert_eq!(config.websocket_handshake_max_header_size, 96 * 1024);
}
