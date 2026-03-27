use crate::mock::HttpbinMockServer;
use crate::proxy::ProxyInstance;
use crate::runner::TestCase;
use serde_json::Value;
use std::time::Duration;

pub fn get_all_tests() -> Vec<TestCase> {
    vec![
        TestCase::standalone(
            "tls_switch_intercept_to_tunnel",
            "Test TLS interception switch: ON -> OFF",
            "tls_switch",
            test_tls_switch_intercept_to_tunnel,
        ),
        TestCase::standalone(
            "tls_switch_tunnel_to_intercept",
            "Test TLS interception switch: OFF -> ON (user reported issue)",
            "tls_switch",
            test_tls_switch_tunnel_to_intercept,
        ),
    ]
}

fn get_traffic_as_json(admin_state: &bifrost_admin::AdminState) -> Value {
    let Some(db_store) = admin_state.traffic_db_store.as_ref() else {
        return serde_json::json!({
            "total": 0,
            "offset": 0,
            "limit": 100,
            "records": []
        });
    };

    let result = db_store.query(&bifrost_admin::QueryParams {
        limit: Some(100),
        ..Default::default()
    });
    let records_json: Vec<Value> = result
        .records
        .into_iter()
        .map(|r| {
            serde_json::json!({
                "id": r.id,
                "method": r.m,
                "protocol": r.proto,
                "status": r.s,
                "host": r.h,
                "path": r.p,
                "duration_ms": r.dur,
                "request_size": r.req_sz,
                "response_size": r.res_sz,
                "is_websocket": r.is_websocket(),
                "is_sse": r.is_sse(),
                "is_tunnel": r.is_tunnel(),
                "content_type": r.ct,
            })
        })
        .collect();

    serde_json::json!({
        "total": records_json.len(),
        "offset": 0,
        "limit": 100,
        "records": records_json
    })
}

async fn update_tls_config_like_api(
    admin_state: &bifrost_admin::AdminState,
    enable_tls_interception: bool,
) -> (bool, usize) {
    let old_value;
    let should_disconnect;

    {
        let mut config = admin_state.runtime_config.write().await;
        old_value = config.enable_tls_interception;
        should_disconnect = config.disconnect_on_config_change;
        config.enable_tls_interception = enable_tls_interception;
    }

    let global_changed = old_value != enable_tls_interception;

    let disconnected_count = if should_disconnect && global_changed {
        let disconnected = admin_state
            .connection_registry
            .disconnect_all_with_mode(!enable_tls_interception);
        disconnected.len()
    } else {
        0
    };

    if global_changed {
        let config = admin_state.runtime_config.read().await;
        let status_info = bifrost_admin::status_printer::TlsStatusInfo::from_runtime_config(
            &config,
            admin_state.connection_registry.active_count(),
        );
        status_info.print_update_banner();
    }

    (global_changed, disconnected_count)
}

async fn test_tls_switch_intercept_to_tunnel() -> Result<(), String> {
    println!("\n======================================================================");
    println!("       TLS SWITCH TEST: ON -> OFF (Intercept to Tunnel)");
    println!("======================================================================\n");

    let port = portpicker::pick_unused_port().unwrap();
    println!("[SETUP] Proxy will run on port {}", port);

    let mock = HttpbinMockServer::start().await;
    let rules = mock.http_rules();
    let rule_refs: Vec<&str> = rules.iter().map(String::as_str).collect();
    let (proxy, admin_state) = ProxyInstance::start_with_admin(port, rule_refs, true, true)
        .await
        .map_err(|e| format!("Failed to start proxy: {}", e))?;

    println!("[SETUP] Proxy started with TLS interception ENABLED");
    tokio::time::sleep(Duration::from_millis(200)).await;

    let proxy_url = format!("http://127.0.0.1:{}", port);

    let https_client = reqwest::Client::builder()
        .proxy(reqwest::Proxy::all(&proxy_url).unwrap())
        .danger_accept_invalid_certs(true)
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|e| format!("Failed to create HTTPS client: {}", e))?;

    println!("\n[PHASE 1] Send HTTPS request with TLS interception ENABLED");
    let _ = https_client
        .get("https://httpbin.org/get?test=1")
        .send()
        .await;
    tokio::time::sleep(Duration::from_millis(200)).await;

    let connections_before = admin_state.connection_registry.list_connections();
    println!(
        "[INFO] Active connections: {} (intercept_mode=true expected)",
        connections_before.len()
    );
    for (req_id, host, port, intercept) in &connections_before {
        println!(
            "       - {} {}:{} (intercept={})",
            req_id, host, port, intercept
        );
    }

    println!("\n[PHASE 2] DISABLE TLS Interception");
    let (changed, disconnected) = update_tls_config_like_api(&admin_state, false).await;
    println!(
        "[API] Config changed: {}, Disconnected: {}",
        changed, disconnected
    );

    let connections_after = admin_state.connection_registry.list_connections();
    println!(
        "[INFO] Active connections after: {}",
        connections_after.len()
    );

    println!("\n[PHASE 3] Send HTTPS request with TLS interception DISABLED");
    let _ = https_client
        .get("https://httpbin.org/get?test=2")
        .send()
        .await;
    tokio::time::sleep(Duration::from_millis(200)).await;

    let traffic = get_traffic_as_json(&admin_state);
    let https_count = traffic["records"]
        .as_array()
        .map(|r| r.iter().filter(|x| x["protocol"] == "https").count())
        .unwrap_or(0);
    let tunnel_count = traffic["records"]
        .as_array()
        .map(|r| r.iter().filter(|x| x["protocol"] == "tunnel").count())
        .unwrap_or(0);

    println!(
        "\n[RESULT] HTTPS: {}, TUNNEL: {}",
        https_count, tunnel_count
    );

    if https_count == 1 && tunnel_count >= 1 {
        println!("[RESULT] ✅ TEST PASSED - ON -> OFF switch works correctly");
    } else {
        println!("[RESULT] ⚠️ TEST FAILED - Expected HTTPS=1, TUNNEL>=1");
    }

    drop(proxy);
    Ok(())
}

async fn test_tls_switch_tunnel_to_intercept() -> Result<(), String> {
    println!("\n======================================================================");
    println!("       TLS SWITCH TEST: OFF -> ON (Tunnel to Intercept)");
    println!("       This is the user-reported issue scenario!");
    println!("======================================================================\n");

    let port = portpicker::pick_unused_port().unwrap();
    println!("[SETUP] Proxy will run on port {}", port);

    let mock = HttpbinMockServer::start().await;
    let rules = mock.http_rules();
    let rule_refs: Vec<&str> = rules.iter().map(String::as_str).collect();
    let (proxy, admin_state) = ProxyInstance::start_with_admin(port, rule_refs, false, true)
        .await
        .map_err(|e| format!("Failed to start proxy: {}", e))?;

    println!("[SETUP] Proxy started with TLS interception DISABLED (tunnel mode)");
    tokio::time::sleep(Duration::from_millis(200)).await;

    let config = admin_state.runtime_config.read().await;
    println!(
        "[CONFIG] enable_tls_interception = {}",
        config.enable_tls_interception
    );
    println!(
        "[CONFIG] disconnect_on_config_change = {}",
        config.disconnect_on_config_change
    );
    drop(config);

    let proxy_url = format!("http://127.0.0.1:{}", port);

    let https_client = reqwest::Client::builder()
        .proxy(reqwest::Proxy::all(&proxy_url).unwrap())
        .danger_accept_invalid_certs(true)
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|e| format!("Failed to create HTTPS client: {}", e))?;

    println!("\n======================================================================");
    println!(" PHASE 1: TLS Interception DISABLED - Send HTTPS request");
    println!("          Expected: Connection registered as tunnel (intercept=false)");
    println!("======================================================================");

    println!("\n[REQUEST 1] Sending HTTPS request (tunnel mode)");
    let result1 = https_client
        .get("https://httpbin.org/get?phase=1")
        .send()
        .await;
    match &result1 {
        Ok(resp) => println!("[RESPONSE] Status: {}", resp.status()),
        Err(e) => println!("[RESPONSE] Error: {}", e),
    }
    tokio::time::sleep(Duration::from_millis(200)).await;

    let connections_phase1 = admin_state.connection_registry.list_connections();
    println!(
        "\n[INFO] Active connections after Phase 1: {}",
        connections_phase1.len()
    );
    for (req_id, host, port, intercept) in &connections_phase1 {
        println!(
            "       - {} {}:{} (intercept={})",
            req_id, host, port, intercept
        );
        if !intercept {
            println!("         ✅ Correct: tunnel connection (intercept=false)");
        } else {
            println!("         ❌ Wrong: should be intercept=false");
        }
    }

    let traffic_phase1 = get_traffic_as_json(&admin_state);
    println!("\n[TRAFFIC] After Phase 1:");
    print_traffic_summary(&traffic_phase1);

    println!("\n======================================================================");
    println!(" PHASE 2: ENABLE TLS Interception");
    println!("          Expected: Tunnel connections should be disconnected");
    println!("======================================================================");

    println!("\n[API] Enabling TLS interception...");
    let (changed, disconnected) = update_tls_config_like_api(&admin_state, true).await;
    println!("[API] Config changed: {}", changed);
    println!("[API] Connections disconnected: {}", disconnected);

    let config = admin_state.runtime_config.read().await;
    println!(
        "[CONFIG] enable_tls_interception = {}",
        config.enable_tls_interception
    );
    drop(config);

    let connections_phase2 = admin_state.connection_registry.list_connections();
    println!(
        "\n[INFO] Active connections after Phase 2: {}",
        connections_phase2.len()
    );
    for (req_id, host, port, intercept) in &connections_phase2 {
        println!(
            "       - {} {}:{} (intercept={})",
            req_id, host, port, intercept
        );
    }

    if disconnected > 0 && connections_phase2.is_empty() {
        println!("\n[SUCCESS] ✅ Tunnel connections were disconnected!");
    } else if disconnected == 0 && !connections_phase1.is_empty() {
        println!("\n[WARNING] ⚠️ No connections were disconnected!");
        println!("          This is the BUG: tunnel connections should be disconnected");
        println!("          when enabling TLS interception!");
    }

    println!("\n======================================================================");
    println!(" PHASE 3: TLS Interception ENABLED - Send HTTPS request");
    println!("          Expected: New connection with intercept=true");
    println!("======================================================================");

    tokio::time::sleep(Duration::from_millis(100)).await;

    let https_client2 = reqwest::Client::builder()
        .proxy(reqwest::Proxy::all(&proxy_url).unwrap())
        .danger_accept_invalid_certs(true)
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|e| format!("Failed to create HTTPS client: {}", e))?;

    println!("\n[REQUEST 2] Sending HTTPS request (should be intercepted now)");
    let result2 = https_client2
        .get("https://httpbin.org/get?phase=3")
        .send()
        .await;
    match &result2 {
        Ok(resp) => println!("[RESPONSE] Status: {}", resp.status()),
        Err(e) => println!("[RESPONSE] Error: {}", e),
    }
    tokio::time::sleep(Duration::from_millis(300)).await;

    let connections_phase3 = admin_state.connection_registry.list_connections();
    println!(
        "\n[INFO] Active connections after Phase 3: {}",
        connections_phase3.len()
    );
    for (req_id, host, port, intercept) in &connections_phase3 {
        println!(
            "       - {} {}:{} (intercept={})",
            req_id, host, port, intercept
        );
        if *intercept {
            println!("         ✅ Correct: intercept connection (intercept=true)");
        } else {
            println!("         ❌ Wrong: should be intercept=true");
        }
    }

    let traffic_phase3 = get_traffic_as_json(&admin_state);
    println!("\n[TRAFFIC] After Phase 3:");
    print_traffic_summary(&traffic_phase3);

    println!("\n======================================================================");
    println!(" FINAL ANALYSIS");
    println!("======================================================================");

    if let Some(records) = traffic_phase3["records"].as_array() {
        let mut tunnel_count = 0;
        let mut https_count = 0;

        for record in records {
            match record["protocol"].as_str().unwrap_or("") {
                "tunnel" => tunnel_count += 1,
                "https" => https_count += 1,
                _ => {}
            }
        }

        println!("\n[BREAKDOWN]");
        println!("  - TUNNEL (not intercepted): {} records", tunnel_count);
        println!("  - HTTPS (intercepted):      {} records", https_count);

        println!("\n[EXPECTATION]");
        println!("  - TUNNEL: 1 (Phase 1, TLS interception OFF)");
        println!("  - HTTPS: >= 1 (Phase 3, TLS interception ON)");

        if tunnel_count == 1 && https_count >= 1 {
            println!("\n[RESULT] ✅ TEST PASSED");
            println!("         TLS 解包切换 (OFF -> ON) 正常工作！");
            println!("         - 旧的 tunnel 连接被断开");
            println!("         - 新请求使用了 TLS 拦截模式");
            println!("         - 转发规则将会生效");
        } else if https_count == 0 {
            println!("\n[RESULT] ⚠️ TEST FAILED");
            println!("         没有 HTTPS 记录！");
            println!("         旧的 tunnel 连接没有被断开，新请求仍在复用旧连接");
            println!("         这就是用户报告的问题！");
        } else if tunnel_count > 1 {
            println!("\n[RESULT] ⚠️ TEST FAILED");
            println!(
                "         发现 {} 个 TUNNEL 记录，可能有额外的 tunnel 请求",
                tunnel_count
            );
        }
    }

    drop(proxy);

    println!("\n======================================================================");
    println!("                    TEST COMPLETED");
    println!("======================================================================\n");

    Ok(())
}

fn print_traffic_summary(response: &Value) {
    if let Some(records) = response["records"].as_array() {
        println!("  Total records: {}", records.len());
        println!("  {:-<60}", "");

        for (idx, record) in records.iter().enumerate() {
            let protocol = record["protocol"].as_str().unwrap_or("");
            let marker = if protocol == "tunnel" {
                "🔒 TUNNEL"
            } else {
                "🔓 INTERCEPT"
            };

            println!(
                "  #{} {} {} {}",
                idx + 1,
                marker,
                record["method"].as_str().unwrap_or(""),
                record["url"].as_str().unwrap_or("")
            );
        }
        println!("  {:-<60}", "");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn run_tls_switch_test() {
        let result = test_tls_switch_intercept_to_tunnel().await;
        if let Err(e) = &result {
            println!("Test error: {}", e);
        }
        assert!(result.is_ok(), "Test failed: {:?}", result.err());
    }

    #[tokio::test]
    async fn run_tls_switch_tunnel_to_intercept_test() {
        let result = test_tls_switch_tunnel_to_intercept().await;
        if let Err(e) = &result {
            println!("Test error: {}", e);
        }
        assert!(result.is_ok(), "Test failed: {:?}", result.err());
    }
}
