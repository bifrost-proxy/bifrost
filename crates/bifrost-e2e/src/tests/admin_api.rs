use crate::assertions::{assert_header_contains, assert_header_value, assert_status};
use crate::{ProxyInstance, TestCase};
use bifrost_storage::RuleFile;
use std::net::TcpListener;

pub fn get_all_tests() -> Vec<TestCase> {
    vec![
        TestCase::standalone(
            "admin_api_cors_preflight_allows_client_id",
            "Validate admin CORS preflight allows desktop client headers",
            "admin",
            || async move {
                let port = pick_unused_port()?;
                let (_proxy, _admin_state) =
                    ProxyInstance::start_with_admin(port, vec![], false, true)
                        .await
                        .map_err(|e| format!("Failed to start proxy with admin: {}", e))?;

                let client = reqwest::Client::builder()
                    .danger_accept_invalid_certs(true)
                    .no_proxy()
                    .build()
                    .map_err(|e| format!("Failed to create client: {}", e))?;

                let response = client
                    .request(
                        reqwest::Method::OPTIONS,
                        format!("http://127.0.0.1:{}/_bifrost/api/system/info", port),
                    )
                    .header("Origin", "http://127.0.0.1:3000")
                    .header("Access-Control-Request-Method", "GET")
                    .header("Access-Control-Request-Headers", "X-Client-Id")
                    .send()
                    .await
                    .map_err(|e| format!("Preflight request failed: {}", e))?;

                assert_status(&response, 204)?;
                assert_header_value(&response, "access-control-allow-origin", "http://127.0.0.1:3000")?;
                assert_header_contains(&response, "access-control-allow-methods", "OPTIONS")?;
                assert_header_contains(&response, "access-control-allow-headers", "X-Client-Id")?;
                Ok(())
            },
        ),
        TestCase::standalone(
            "admin_public_proxy_qrcode_allows_cors",
            "Validate public proxy qrcode endpoint supports cross-origin access",
            "admin",
            || async move {
                let port = pick_unused_port()?;
                let (_proxy, _admin_state) =
                    ProxyInstance::start_with_admin(port, vec![], false, true)
                        .await
                        .map_err(|e| format!("Failed to start proxy with admin: {}", e))?;

                let client = reqwest::Client::builder()
                    .danger_accept_invalid_certs(true)
                    .no_proxy()
                    .build()
                    .map_err(|e| format!("Failed to create client: {}", e))?;

                let get_response = client
                    .get(format!(
                        "http://127.0.0.1:{}/_bifrost/public/proxy/qrcode?ip=127.0.0.1",
                        port
                    ))
                    .header("Origin", "http://127.0.0.1:3000")
                    .send()
                    .await
                    .map_err(|e| format!("GET qrcode request failed: {}", e))?;

                assert_status(&get_response, 200)?;
                assert_header_value(&get_response, "access-control-allow-origin", "http://127.0.0.1:3000")?;
                assert_header_contains(&get_response, "access-control-allow-methods", "GET")?;
                assert_header_contains(&get_response, "access-control-allow-methods", "OPTIONS")?;
                assert_header_contains(
                    &get_response,
                    "access-control-allow-headers",
                    "X-Client-Id",
                )?;
                assert_header_contains(&get_response, "content-type", "image/svg+xml")?;

                let preflight_response = client
                    .request(
                        reqwest::Method::OPTIONS,
                        format!(
                            "http://127.0.0.1:{}/_bifrost/public/proxy/qrcode?ip=127.0.0.1",
                            port
                        ),
                    )
                    .header("Origin", "http://127.0.0.1:3000")
                    .header("Access-Control-Request-Method", "GET")
                    .header("Access-Control-Request-Headers", "X-Client-Id")
                    .send()
                    .await
                    .map_err(|e| format!("OPTIONS qrcode request failed: {}", e))?;

                assert_status(&preflight_response, 204)?;
                assert_header_value(&preflight_response, "access-control-allow-origin", "http://127.0.0.1:3000")?;
                assert_header_contains(
                    &preflight_response,
                    "access-control-allow-methods",
                    "OPTIONS",
                )?;
                assert_header_contains(
                    &preflight_response,
                    "access-control-allow-headers",
                    "X-Client-Id",
                )?;
                Ok(())
            },
        ),
        TestCase::standalone(
            "admin_api_config_connections",
            "Validate GET /api/config/connections returns valid JSON with connections array",
            "admin",
            || async move {
                let port = pick_unused_port()?;
                let (_proxy, _admin_state) =
                    ProxyInstance::start_with_admin(port, vec![], false, true)
                        .await
                        .map_err(|e| format!("Failed to start proxy with admin: {}", e))?;

                let client = reqwest::Client::builder()
                    .danger_accept_invalid_certs(true)
                    .no_proxy()
                    .build()
                    .map_err(|e| format!("Failed to create client: {}", e))?;

                let response = client
                    .get(format!(
                        "http://127.0.0.1:{}/_bifrost/api/config/connections",
                        port
                    ))
                    .send()
                    .await
                    .map_err(|e| format!("GET connections failed: {}", e))?;

                assert_status(&response, 200)?;

                let json: serde_json::Value = response
                    .json()
                    .await
                    .map_err(|e| format!("Failed to parse connections JSON: {}", e))?;

                if json.get("total").is_none() && json.get("connections").is_none() {
                    return Err(format!(
                        "Expected 'total' or 'connections' field in response, got: {}",
                        serde_json::to_string_pretty(&json).unwrap_or_default()
                    ));
                }

                Ok(())
            },
        ),
        TestCase::standalone(
            "admin_api_mobile_devices_lists_android_ios_discovery",
            "Validate mobile device discovery API returns Android ADB status, iOS USB status, and product notices",
            "admin",
            || async move {
                let port = pick_unused_port()?;
                let (_proxy, _admin_state) =
                    ProxyInstance::start_with_admin(port, vec![], false, true)
                        .await
                        .map_err(|e| format!("Failed to start proxy with admin: {}", e))?;

                let client = reqwest::Client::builder()
                    .danger_accept_invalid_certs(true)
                    .no_proxy()
                    .build()
                    .map_err(|e| format!("Failed to create client: {}", e))?;

                let response = client
                    .get(format!(
                        "http://127.0.0.1:{}/_bifrost/api/mobile-devices",
                        port
                    ))
                    .send()
                    .await
                    .map_err(|e| format!("GET mobile devices failed: {}", e))?;

                assert_status(&response, 200)?;
                let json: serde_json::Value = response
                    .json()
                    .await
                    .map_err(|e| format!("Failed to parse mobile devices JSON: {}", e))?;

                if json.pointer("/android/adb_available").is_none() {
                    return Err(format!("Expected android.adb_available field, got: {json}"));
                }
                if json.pointer("/ios/supported").is_none() {
                    return Err(format!("Expected ios.supported field, got: {json}"));
                }
                if !json
                    .pointer("/ios/devices")
                    .map(|value| value.is_array())
                    .unwrap_or(false)
                {
                    return Err(format!("Expected ios.devices array, got: {json}"));
                }
                let ordinary_notice = json
                    .get("ordinary_device_notice")
                    .and_then(|value| value.as_str())
                    .unwrap_or_default();
                if !ordinary_notice.contains("require final confirmation") {
                    return Err(format!(
                        "Expected ordinary device notice to explain phone confirmation, got: {ordinary_notice}"
                    ));
                }
                Ok(())
            },
        ),
        TestCase::standalone(
            "admin_public_ios_mobileconfig_uses_current_ca",
            "Validate iOS mobileconfig and QR endpoints are generated from the current CA",
            "admin",
            || async move {
                let port = pick_unused_port()?;
                let (_proxy, _admin_state) =
                    ProxyInstance::start_with_admin(port, vec![], false, true)
                        .await
                        .map_err(|e| format!("Failed to start proxy with admin: {}", e))?;

                let client = reqwest::Client::builder()
                    .danger_accept_invalid_certs(true)
                    .no_proxy()
                    .build()
                    .map_err(|e| format!("Failed to create client: {}", e))?;

                let profile_response = client
                    .get(format!(
                        "http://127.0.0.1:{}/_bifrost/public/mobile/ios-profile.mobileconfig",
                        port
                    ))
                    .send()
                    .await
                    .map_err(|e| format!("GET iOS mobileconfig failed: {}", e))?;

                assert_status(&profile_response, 200)?;
                assert_header_contains(
                    &profile_response,
                    "content-type",
                    "application/x-apple-aspen-config",
                )?;
                let profile = profile_response
                    .text()
                    .await
                    .map_err(|e| format!("Failed to read iOS mobileconfig body: {}", e))?;
                if !profile.contains("com.apple.security.root")
                    || !profile.contains("Certificate Trust Settings")
                {
                    return Err(format!("Unexpected mobileconfig body: {profile}"));
                }

                let qrcode_response = client
                    .get(format!(
                        "http://127.0.0.1:{}/_bifrost/public/mobile/ios-profile.mobileconfig/qrcode?ip=127.0.0.1",
                        port
                    ))
                    .send()
                    .await
                    .map_err(|e| format!("GET iOS mobileconfig QR failed: {}", e))?;
                assert_status(&qrcode_response, 200)?;
                assert_header_contains(&qrcode_response, "content-type", "image/svg+xml")?;
                Ok(())
            },
        ),
        TestCase::standalone(
            "admin_api_mobile_install_requires_explicit_confirmation",
            "Validate Android CA install API rejects requests that do not confirm phone-side manual trust",
            "admin",
            || async move {
                let port = pick_unused_port()?;
                let (_proxy, _admin_state) =
                    ProxyInstance::start_with_admin(port, vec![], false, true)
                        .await
                        .map_err(|e| format!("Failed to start proxy with admin: {}", e))?;

                let client = reqwest::Client::builder()
                    .danger_accept_invalid_certs(true)
                    .no_proxy()
                    .build()
                    .map_err(|e| format!("Failed to create client: {}", e))?;

                let response = client
                    .post(format!(
                        "http://127.0.0.1:{}/_bifrost/api/mobile-devices/test-device/install-ca",
                        port
                    ))
                    .json(&serde_json::json!({ "mode": "normal_guide" }))
                    .send()
                    .await
                    .map_err(|e| format!("POST mobile install failed: {}", e))?;

                assert_status(&response, 400)?;
                let body = response
                    .text()
                    .await
                    .map_err(|e| format!("Failed to read mobile install error: {}", e))?;
                if !body.contains("Missing install confirmation") {
                    return Err(format!("Expected confirmation error, got: {body}"));
                }
                Ok(())
            },
        ),
        TestCase::standalone(
            "admin_api_local_ca_install_requires_explicit_confirmation",
            "Validate local CA install API rejects requests without explicit confirmation",
            "admin",
            || async move {
                let port = pick_unused_port()?;
                let (_proxy, _admin_state) =
                    ProxyInstance::start_with_admin(port, vec![], false, true)
                        .await
                        .map_err(|e| format!("Failed to start proxy with admin: {}", e))?;

                let client = reqwest::Client::builder()
                    .danger_accept_invalid_certs(true)
                    .no_proxy()
                    .build()
                    .map_err(|e| format!("Failed to create client: {}", e))?;

                let response = client
                    .post(format!(
                        "http://127.0.0.1:{}/_bifrost/api/cert/install",
                        port
                    ))
                    .json(&serde_json::json!({}))
                    .send()
                    .await
                    .map_err(|e| format!("POST local CA install failed: {}", e))?;

                assert_status(&response, 400)?;
                let body = response
                    .text()
                    .await
                    .map_err(|e| format!("Failed to read local CA install error: {}", e))?;
                if !body.contains("Missing local CA install confirmation") {
                    return Err(format!("Expected local confirmation error, got: {body}"));
                }
                Ok(())
            },
        ),
        TestCase::standalone(
            "admin_api_system_memory",
            "Validate GET /api/system/memory returns memory diagnostics with process info",
            "admin",
            || async move {
                let port = pick_unused_port()?;
                let (_proxy, _admin_state) =
                    ProxyInstance::start_with_admin(port, vec![], false, true)
                        .await
                        .map_err(|e| format!("Failed to start proxy with admin: {}", e))?;

                let client = reqwest::Client::builder()
                    .danger_accept_invalid_certs(true)
                    .no_proxy()
                    .build()
                    .map_err(|e| format!("Failed to create client: {}", e))?;

                let response = client
                    .get(format!(
                        "http://127.0.0.1:{}/_bifrost/api/system/memory",
                        port
                    ))
                    .send()
                    .await
                    .map_err(|e| format!("GET system/memory failed: {}", e))?;

                assert_status(&response, 200)?;

                let json: serde_json::Value = response
                    .json()
                    .await
                    .map_err(|e| format!("Failed to parse memory JSON: {}", e))?;

                if json.get("process").is_none() {
                    return Err(format!(
                        "Expected 'process' field in memory response, got: {}",
                        serde_json::to_string_pretty(&json).unwrap_or_default()
                    ));
                }

                let process = &json["process"];
                if process.get("pid").is_none() {
                    return Err("Expected 'pid' in process info".to_string());
                }
                if process.get("rss_kib").is_none() {
                    return Err("Expected 'rss_kib' in process info".to_string());
                }

                if json.get("stores").is_none() {
                    return Err("Expected 'stores' field in memory response".to_string());
                }

                Ok(())
            },
        ),
        TestCase::standalone(
            "admin_api_config_show_server_section",
            "Validate GET /api/config returns server configuration section",
            "admin",
            || async move {
                let port = pick_unused_port()?;
                let (_proxy, _admin_state) =
                    ProxyInstance::start_with_admin(port, vec![], false, true)
                        .await
                        .map_err(|e| format!("Failed to start proxy with admin: {}", e))?;

                let client = reqwest::Client::builder()
                    .danger_accept_invalid_certs(true)
                    .no_proxy()
                    .build()
                    .map_err(|e| format!("Failed to create client: {}", e))?;

                let response = client
                    .get(format!("http://127.0.0.1:{}/_bifrost/api/config", port))
                    .send()
                    .await
                    .map_err(|e| format!("GET config failed: {}", e))?;

                assert_status(&response, 200)?;

                let json: serde_json::Value = response
                    .json()
                    .await
                    .map_err(|e| format!("Failed to parse config JSON: {}", e))?;

                if json.get("port").is_none() && json.get("server").is_none() {
                    return Err(format!(
                        "Expected server config fields (port or server), got: {}",
                        serde_json::to_string_pretty(&json).unwrap_or_default()
                    ));
                }

                Ok(())
            },
        ),
        TestCase::standalone(
            "admin_api_proxy_address_with_preferred_ip",
            "Validate GET /api/proxy/address returns addresses with is_preferred field and preferred IP first",
            "admin",
            || async move {
                let port = pick_unused_port()?;
                let (_proxy, _admin_state) =
                    ProxyInstance::start_with_admin(port, vec![], false, true)
                        .await
                        .map_err(|e| format!("Failed to start proxy with admin: {}", e))?;

                let client = reqwest::Client::builder()
                    .danger_accept_invalid_certs(true)
                    .no_proxy()
                    .build()
                    .map_err(|e| format!("Failed to create client: {}", e))?;

                let response = client
                    .get(format!(
                        "http://127.0.0.1:{}/_bifrost/api/proxy/address",
                        port
                    ))
                    .send()
                    .await
                    .map_err(|e| format!("GET proxy/address failed: {}", e))?;

                assert_status(&response, 200)?;

                let json: serde_json::Value = response
                    .json()
                    .await
                    .map_err(|e| format!("Failed to parse proxy address JSON: {}", e))?;

                let port_val = json
                    .get("port")
                    .and_then(|v| v.as_u64())
                    .ok_or("Missing 'port' field in proxy address response")?;
                if port_val != port as u64 {
                    return Err(format!(
                        "Expected port {}, got {}",
                        port, port_val
                    ));
                }

                let addresses = json
                    .get("addresses")
                    .and_then(|v| v.as_array())
                    .ok_or("Missing 'addresses' array in proxy address response")?;

                if addresses.is_empty() {
                    return Err("Expected at least one address entry".to_string());
                }

                for (i, addr) in addresses.iter().enumerate() {
                    if addr.get("ip").and_then(|v| v.as_str()).is_none() {
                        return Err(format!("addresses[{}] missing 'ip' field", i));
                    }
                    if addr.get("address").and_then(|v| v.as_str()).is_none() {
                        return Err(format!("addresses[{}] missing 'address' field", i));
                    }
                    if addr.get("is_preferred").is_none() {
                        return Err(format!("addresses[{}] missing 'is_preferred' field", i));
                    }
                }

                let has_preferred = addresses
                    .iter()
                    .any(|a| a.get("is_preferred").and_then(|v| v.as_bool()) == Some(true));
                if has_preferred {
                    let first_preferred = addresses[0]
                        .get("is_preferred")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    if !first_preferred {
                        return Err(
                            "Preferred IP should be sorted first in the addresses list".to_string(),
                        );
                    }
                }

                let local_ips = json
                    .get("local_ips")
                    .and_then(|v| v.as_array())
                    .ok_or("Missing 'local_ips' array")?;
                if local_ips.len() != addresses.len() {
                    return Err(format!(
                        "local_ips length ({}) doesn't match addresses length ({})",
                        local_ips.len(),
                        addresses.len()
                    ));
                }

                Ok(())
            },
        ),
        TestCase::standalone(
            "admin_api_proxy_address_contains_lan_ip",
            "Validate GET /api/proxy/address returns at least one non-loopback LAN IP for external devices",
            "admin",
            || async move {
                let port = pick_unused_port()?;
                let (_proxy, _admin_state) =
                    ProxyInstance::start_with_admin(port, vec![], false, true)
                        .await
                        .map_err(|e| format!("Failed to start proxy with admin: {}", e))?;

                let client = reqwest::Client::builder()
                    .danger_accept_invalid_certs(true)
                    .no_proxy()
                    .build()
                    .map_err(|e| format!("Failed to create client: {}", e))?;

                let response = client
                    .get(format!(
                        "http://127.0.0.1:{}/_bifrost/api/proxy/address",
                        port
                    ))
                    .send()
                    .await
                    .map_err(|e| format!("GET proxy/address failed: {}", e))?;

                assert_status(&response, 200)?;

                let json: serde_json::Value = response
                    .json()
                    .await
                    .map_err(|e| format!("Failed to parse proxy address JSON: {}", e))?;

                let addresses = json
                    .get("addresses")
                    .and_then(|v| v.as_array())
                    .ok_or("Missing 'addresses' array in proxy address response")?;

                let has_non_loopback = addresses.iter().any(|addr| {
                    addr.get("ip")
                        .and_then(|v| v.as_str())
                        .is_some_and(|ip| ip != "127.0.0.1" && ip != "::1")
                });

                if !has_non_loopback {
                    let ips: Vec<&str> = addresses
                        .iter()
                        .filter_map(|a| a.get("ip").and_then(|v| v.as_str()))
                        .collect();
                    return Err(format!(
                        "Expected at least one non-loopback LAN IP for external device access, got only: {:?}",
                        ips
                    ));
                }

                Ok(())
            },
        ),
        TestCase::standalone(
            "admin_api_delete_synced_rule_succeeds_without_sync_manager",
            "Validate DELETE /api/rules/{name} succeeds for a synced rule even when sync_manager is unavailable",
            "admin",
            || async move {
                let port = pick_unused_port()?;
                let (_proxy, admin_state) =
                    ProxyInstance::start_with_admin(port, vec![], false, true)
                        .await
                        .map_err(|e| format!("Failed to start proxy with admin: {}", e))?;

                let mut rule = RuleFile::new("delete-synced-test", "example.com host://127.0.0.1:3000");
                rule.mark_synced(
                    "remote-1",
                    "user-1",
                    "2026-03-20T09:00:00Z",
                    "2026-03-20T10:00:00Z",
                );
                admin_state
                    .rules_storage
                    .save(&rule)
                    .map_err(|e| format!("Failed to save rule: {}", e))?;

                assert!(
                    admin_state.rules_storage.exists("delete-synced-test"),
                    "Rule should exist before deletion"
                );

                let client = reqwest::Client::builder()
                    .danger_accept_invalid_certs(true)
                    .no_proxy()
                    .build()
                    .map_err(|e| format!("Failed to create client: {}", e))?;

                let response = client
                    .delete(format!(
                        "http://127.0.0.1:{}/_bifrost/api/rules/delete-synced-test",
                        port
                    ))
                    .send()
                    .await
                    .map_err(|e| format!("DELETE request failed: {}", e))?;

                assert_status(&response, 200)?;

                assert!(
                    !admin_state.rules_storage.exists("delete-synced-test"),
                    "Rule should not exist after deletion"
                );

                let list_response = client
                    .get(format!(
                        "http://127.0.0.1:{}/_bifrost/api/rules",
                        port
                    ))
                    .send()
                    .await
                    .map_err(|e| format!("GET rules failed: {}", e))?;

                assert_status(&list_response, 200)?;
                let json: serde_json::Value = list_response
                    .json()
                    .await
                    .map_err(|e| format!("Failed to parse JSON: {}", e))?;
                let rules = json
                    .as_array()
                    .ok_or("Expected rules to be an array")?;
                let found = rules.iter().any(|r| {
                    r.get("name").and_then(|n| n.as_str()) == Some("delete-synced-test")
                });
                if found {
                    return Err("Deleted rule still appears in the rules list".to_string());
                }

                Ok(())
            },
        ),
    ]
}

fn pick_unused_port() -> Result<u16, String> {
    TcpListener::bind("127.0.0.1:0")
        .map_err(|e| format!("Failed to bind ephemeral port: {}", e))?
        .local_addr()
        .map(|addr| addr.port())
        .map_err(|e| format!("Failed to read ephemeral port: {}", e))
}
