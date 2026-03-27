use std::time::Duration;

use crate::client::DirectClient;
use crate::mock::HttpbinMockServer;
use crate::{ProxyClient, ProxyInstance, TestCase};

use bifrost_cli::commands::{
    run_traffic_get, run_traffic_list, OutputFormat, TrafficGetOptions, TrafficListOptions,
};

pub fn get_all_tests() -> Vec<TestCase> {
    vec![TestCase::standalone(
        "traffic_cli_list_get",
        "Validate traffic list/get against admin API (and CLI wrappers)",
        "traffic",
        || async move {
            let port = portpicker::pick_unused_port().ok_or("Failed to pick unused port")?;
            let mock = HttpbinMockServer::start().await;
            let rules = mock.http_rules();
            let rule_refs: Vec<&str> = rules.iter().map(String::as_str).collect();
            let (_proxy, _admin_state) =
                ProxyInstance::start_with_admin(port, rule_refs, false, true)
                    .await
                    .map_err(|e| format!("Failed to start proxy with admin: {}", e))?;

            let proxy_url = format!("http://127.0.0.1:{}", port);
            let client = ProxyClient::new(&proxy_url).map_err(|e| e.to_string())?;

            let _ = client
                .post("http://httpbin.org/post", r#"{"hello":"world"}"#)
                .await;
            tokio::time::sleep(Duration::from_millis(200)).await;

            let direct = DirectClient::new().map_err(|e| e.to_string())?;
            let list_url = format!("http://127.0.0.1:{}/_bifrost/api/traffic?limit=20", port);
            let list_json = direct
                .get_json(&list_url)
                .await
                .map_err(|e| e.to_string())?;

            let records = list_json
                .get("records")
                .and_then(|v| v.as_array())
                .ok_or("Expected records array in traffic list")?;
            if records.is_empty() {
                return Err("Expected at least one traffic record".to_string());
            }

            let seq = records[0]
                .get("seq")
                .or_else(|| records[0].get("sequence"))
                .and_then(|v| v.as_u64())
                .ok_or("Expected sequence in traffic record")?;

            let list_opts = TrafficListOptions {
                port,
                limit: 10,
                cursor: None,
                direction: "backward".to_string(),
                method: None,
                status: None,
                status_min: None,
                status_max: None,
                protocol: None,
                host: None,
                url: None,
                path: None,
                content_type: None,
                client_ip: None,
                client_app: None,
                has_rule_hit: None,
                is_websocket: None,
                is_sse: None,
                is_tunnel: None,
                format: OutputFormat::Json,
                no_color: true,
            };
            run_traffic_list(list_opts).map_err(|e| e.to_string())?;

            let get_opts = TrafficGetOptions {
                port,
                id: Some(seq.to_string()),
                request_body: true,
                response_body: true,
                format: OutputFormat::Json,
            };
            run_traffic_get(get_opts).map_err(|e| e.to_string())?;

            Ok(())
        },
    )]
}
