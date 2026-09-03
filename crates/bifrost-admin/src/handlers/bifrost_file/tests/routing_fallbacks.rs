use super::super::*;

#[test]
fn network_record_import_falls_back_to_url_when_host_and_path_missing() {
    let record = NetworkRecord {
        id: "REQ-fallback".to_string(),
        method: "POST".to_string(),
        url: "https://example.test/api?x=1".to_string(),
        status: 201,
        host: None,
        path: None,
        protocol: None,
        actual_url: None,
        actual_host: None,
        listener_port: None,
        has_rule_hit: None,
        error_message: None,
        client_app: None,
        client_path: None,
        request_headers: None,
        response_headers: None,
        original_response_headers: None,
        request_body: None,
        request_body_base64: None,
        response_body: None,
        response_body_base64: None,
        duration_ms: 0,
        timestamp: 0,
        matched_rules: None,
        active_rules: None,
    };

    let traffic = network_record_to_traffic_record(&record);

    assert_eq!(traffic.host, "example.test");
    assert_eq!(traffic.path, "/api?x=1");
    assert_eq!(traffic.protocol, "HTTPS");
}

#[test]
fn network_record_import_uses_default_protocol_for_invalid_url() {
    let record = NetworkRecord {
        id: "REQ-invalid-url".to_string(),
        method: "GET".to_string(),
        url: "not a valid url".to_string(),
        status: 0,
        host: None,
        path: None,
        protocol: None,
        actual_url: None,
        actual_host: None,
        listener_port: None,
        has_rule_hit: None,
        error_message: None,
        client_app: None,
        client_path: None,
        request_headers: None,
        response_headers: None,
        original_response_headers: None,
        request_body: None,
        request_body_base64: None,
        response_body: None,
        response_body_base64: None,
        duration_ms: 0,
        timestamp: 0,
        matched_rules: None,
        active_rules: None,
    };

    let traffic = network_record_to_traffic_record(&record);

    assert_eq!(traffic.host, "");
    assert_eq!(traffic.path, "");
    assert_eq!(traffic.protocol, "HTTP");
}
