use super::*;

fn test_mcp_config() -> McpServerConfig {
    McpServerConfig {
        command: None,
        args: None,
        env: None,
        cwd: None,
        url: None,
        bearer_token_env_var: None,
        enabled: true,
        startup_timeout_sec: Some(1),
        tool_timeout_sec: Some(1),
        enabled_tools: None,
        disabled_tools: None,
        required: None,
        supports_parallel_tool_calls: None,
        default_tools_approval_mode: None,
        scopes: None,
        oauth_resource: None,
        http_headers: None,
        env_http_headers: None,
        tools: None,
    }
}

#[test]
fn test_is_valid_tool_name() {
    assert!(is_valid_tool_name("foo"));
    assert!(is_valid_tool_name("foo_bar-123"));
    assert!(!is_valid_tool_name(""));
    assert!(!is_valid_tool_name("x".repeat(65).as_str()));
    assert!(!is_valid_tool_name("foo bar"));
    assert!(!is_valid_tool_name("foo.bar"));
}

#[test]
fn test_sanitise_server_prefix_short_passthrough() {
    let s = sanitise_server_prefix("lark", 10);
    assert_eq!(s, "lark");
}

#[test]
fn test_sanitise_server_prefix_illegal_chars() {
    let s = sanitise_server_prefix("lark.docs/skill", 8);
    assert_eq!(s, "lark_docs_skill");
}

#[test]
fn test_sanitise_server_prefix_long_gets_hashed() {
    let s = sanitise_server_prefix("a_very_long_server_name_that_exceeds_budget", 30);
    assert!(s.len() <= 29);
    let s2 = sanitise_server_prefix("a_very_long_server_name_that_exceeds_budget", 30);
    assert_eq!(s, s2);
}

#[test]
fn mcp_tool_exposure_keeps_below_threshold_direct() {
    let tools = (0..(DIRECT_MCP_TOOL_EXPOSURE_THRESHOLD - 1))
        .map(|idx| {
            ToolDefinition::function(
                format!("mcp_test__tool_{idx}"),
                "test tool".to_string(),
                Some(serde_json::json!({"type":"object"})),
            )
        })
        .collect::<Vec<_>>();

    let exposure = mcp_tool_exposure_from_definitions(tools);
    assert_eq!(
        exposure.direct_tools.len(),
        DIRECT_MCP_TOOL_EXPOSURE_THRESHOLD - 1
    );
    assert!(exposure.deferred_tools.is_empty());
}

#[test]
fn mcp_tool_exposure_defers_at_codex_threshold() {
    let tools = (0..DIRECT_MCP_TOOL_EXPOSURE_THRESHOLD)
        .map(|idx| {
            ToolDefinition::function(
                format!("mcp_test__tool_{idx}"),
                "test tool".to_string(),
                Some(serde_json::json!({"type":"object"})),
            )
        })
        .collect::<Vec<_>>();

    let exposure = mcp_tool_exposure_from_definitions(tools);
    assert!(exposure.direct_tools.is_empty());
    assert_eq!(
        exposure.deferred_tools.len(),
        DIRECT_MCP_TOOL_EXPOSURE_THRESHOLD
    );
}

#[test]
fn mcp_resource_tools_stay_direct_when_server_tools_are_deferred() {
    let tools = (0..DIRECT_MCP_TOOL_EXPOSURE_THRESHOLD)
        .map(|idx| {
            ToolDefinition::function(
                format!("mcp_test__tool_{idx}"),
                "test tool".to_string(),
                Some(serde_json::json!({"type":"object"})),
            )
        })
        .collect::<Vec<_>>();

    let exposure = mcp_tool_exposure_with_resource_tools(tools, true);
    let direct_names = exposure
        .direct_tools
        .iter()
        .map(ToolDefinition::name)
        .collect::<Vec<_>>();
    assert_eq!(
        direct_names,
        vec![
            "list_mcp_resources",
            "list_mcp_resource_templates",
            "read_mcp_resource"
        ]
    );
    assert_eq!(
        exposure.deferred_tools.len(),
        DIRECT_MCP_TOOL_EXPOSURE_THRESHOLD
    );
}

#[test]
fn test_split_sse_events_single() {
    let body = "event: message\ndata: {\"a\":1}\n\n";
    let events = split_sse_events(body);
    assert_eq!(events, vec!["{\"a\":1}"]);
}

#[test]
fn test_split_sse_events_multi() {
    let body = "data: {\"a\":1}\n\ndata: {\"b\":2}\n\n";
    let events = split_sse_events(body);
    assert_eq!(events, vec!["{\"a\":1}", "{\"b\":2}"]);
}

#[test]
fn test_split_sse_events_multiline_data() {
    let body = "data: {\"a\":1,\ndata: \"b\":2}\n\n";
    let events = split_sse_events(body);
    assert_eq!(events, vec!["{\"a\":1,\"b\":2}"]);
}

#[test]
fn test_split_sse_events_empty() {
    assert!(split_sse_events("").is_empty());
    assert!(split_sse_events("\n\n").is_empty());
}

#[test]
fn test_validate_schema_passes_valid() {
    let s = serde_json::json!({"type": "object", "properties": {}});
    let out = validate_or_default_schema("s", "t", Some(s.clone()));
    assert_eq!(out, s);
}

#[test]
fn test_validate_schema_substitutes_invalid() {
    let s = serde_json::json!({"type": "string"});
    let out = validate_or_default_schema("s", "t", Some(s));
    assert_eq!(out, serde_json::json!({"type": "object"}));
}

#[test]
fn test_validate_schema_substitutes_non_object() {
    let out = validate_or_default_schema("s", "t", Some(serde_json::json!(["x"])));
    assert_eq!(out, serde_json::json!({"type": "object"}));
}

#[test]
fn test_validate_schema_accepts_ref() {
    let s = serde_json::json!({"$ref": "#/definitions/Foo"});
    let out = validate_or_default_schema("s", "t", Some(s.clone()));
    assert_eq!(out, s);
}

#[test]
fn test_mcp_manager_empty() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let configs = HashMap::new();
        let manager = McpManager::new(&configs).await;
        assert!(manager.list_tools().is_empty());
        assert!(!manager.is_mcp_tool("anything"));
    });
}

#[test]
fn test_mcp_manager_disabled_server() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let mut configs = HashMap::new();
        configs.insert(
            "test".to_string(),
            McpServerConfig {
                command: Some("echo".to_string()),
                args: None,
                env: None,
                cwd: None,
                url: None,
                bearer_token_env_var: None,
                enabled: false,
                startup_timeout_sec: None,
                tool_timeout_sec: None,
                enabled_tools: None,
                disabled_tools: None,
                required: None,
                supports_parallel_tool_calls: None,
                default_tools_approval_mode: None,
                scopes: None,
                oauth_resource: None,
                http_headers: None,
                env_http_headers: None,
                tools: None,
            },
        );
        let manager = McpManager::new(&configs).await;
        assert!(manager.list_tools().is_empty());
    });
}

#[test]
fn test_mcp_availability_disabled_server_is_not_started() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let mut configs = HashMap::new();
        let mut config = test_mcp_config();
        config.command = Some("missing-command-that-should-not-run".to_string());
        config.enabled = false;
        configs.insert("disabled".to_string(), config);

        let statuses = check_server_availability(&configs).await;
        assert_eq!(statuses.len(), 1);
        assert_eq!(statuses[0].name, "disabled");
        assert_eq!(statuses[0].status, McpServerAvailabilityStatus::Disabled);
        assert_eq!(statuses[0].tool_count, 0);
        assert!(statuses[0].error.is_none());
    });
}

#[test]
fn test_mcp_availability_reports_invalid_enabled_server() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let mut configs = HashMap::new();
        configs.insert("broken".to_string(), test_mcp_config());

        let statuses = check_server_availability(&configs).await;
        assert_eq!(statuses.len(), 1);
        assert_eq!(statuses[0].name, "broken");
        assert_eq!(statuses[0].status, McpServerAvailabilityStatus::Unavailable);
        assert_eq!(statuses[0].tool_count, 0);
        assert!(statuses[0]
            .error
            .as_deref()
            .is_some_and(|error| error.contains("neither 'url' nor 'command'")));
    });
}

#[test]
fn test_mcp_default_timeout_constants_match_runtime_policy() {
    assert_eq!(DEFAULT_STARTUP_TIMEOUT_SEC, 600);
    assert_eq!(DEFAULT_TOOL_TIMEOUT_SEC, 600);
}

#[test]
fn test_json_rpc_request_serialization() {
    let req = JsonRpcRequest {
        jsonrpc: "2.0",
        id: 1,
        method: "tools/list".to_string(),
        params: None,
    };
    let json = serde_json::to_string(&req).unwrap();
    assert!(json.contains("\"jsonrpc\":\"2.0\""));
    assert!(json.contains("\"method\":\"tools/list\""));
    assert!(!json.contains("params"));
}

#[test]
fn test_json_rpc_response_parsing_result() {
    let json = r#"{"jsonrpc":"2.0","id":1,"result":{"tools":[]}}"#;
    let resp: JsonRpcResponse = serde_json::from_str(json).unwrap();
    assert_eq!(resp.id, Some(1));
    assert!(resp.result.is_some());
    assert!(resp.error.is_none());
    assert!(resp.method.is_none());
}

#[test]
fn test_json_rpc_response_parsing_notification() {
    let json = r#"{"jsonrpc":"2.0","method":"notifications/progress","params":{"x":1}}"#;
    let resp: JsonRpcResponse = serde_json::from_str(json).unwrap();
    assert!(resp.id.is_none());
    assert_eq!(resp.method.as_deref(), Some("notifications/progress"));
}

#[test]
fn test_json_rpc_error_response_parsing() {
    let json = r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32601,"message":"Method not found"}}"#;
    let resp: JsonRpcResponse = serde_json::from_str(json).unwrap();
    assert!(resp.error.is_some());
    assert_eq!(resp.error.unwrap().message, "Method not found");
}

#[test]
fn test_http_transport_detection_no_panic() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let mut configs = HashMap::new();
        configs.insert(
            "http_server".to_string(),
            McpServerConfig {
                command: None,
                args: None,
                env: None,
                cwd: None,
                url: Some("http://127.0.0.1:1/mcp".to_string()),
                bearer_token_env_var: None,
                enabled: true,
                startup_timeout_sec: Some(1),
                tool_timeout_sec: Some(1),
                enabled_tools: None,
                disabled_tools: None,
                required: None,
                supports_parallel_tool_calls: None,
                default_tools_approval_mode: None,
                scopes: None,
                oauth_resource: None,
                http_headers: None,
                env_http_headers: None,
                tools: None,
            },
        );
        let manager = McpManager::new(&configs).await;
        assert!(manager.list_tools().is_empty());
    });
}

#[tokio::test]
async fn test_route_inbound_frame_matches_pending() {
    let pending: PendingMap = Arc::new(Mutex::new(HashMap::new()));
    let (tx, rx) = oneshot::channel();
    pending.lock().await.insert(42, tx);
    route_inbound_frame(
        "srv",
        &pending,
        r#"{"jsonrpc":"2.0","id":42,"result":{"ok":true}}"#,
    )
    .await;
    let got = rx.await.unwrap().unwrap();
    assert_eq!(got, serde_json::json!({"ok": true}));
    assert!(pending.lock().await.is_empty());
}

#[tokio::test]
async fn test_route_inbound_frame_drops_unknown_id() {
    let pending: PendingMap = Arc::new(Mutex::new(HashMap::new()));
    route_inbound_frame("srv", &pending, r#"{"jsonrpc":"2.0","id":99,"result":{}}"#).await;
    assert!(pending.lock().await.is_empty());
}

#[tokio::test]
async fn test_route_inbound_frame_ignores_notification() {
    let pending: PendingMap = Arc::new(Mutex::new(HashMap::new()));
    let (tx, mut rx) = oneshot::channel::<Result<serde_json::Value, String>>();
    pending.lock().await.insert(1, tx);
    route_inbound_frame(
        "srv",
        &pending,
        r#"{"jsonrpc":"2.0","method":"notifications/tools/list_changed"}"#,
    )
    .await;
    assert!(pending.lock().await.contains_key(&1));
    assert!(rx.try_recv().is_err());
}

#[tokio::test]
async fn test_route_inbound_frame_delivers_error() {
    let pending: PendingMap = Arc::new(Mutex::new(HashMap::new()));
    let (tx, rx) = oneshot::channel();
    pending.lock().await.insert(7, tx);
    route_inbound_frame(
        "srv",
        &pending,
        r#"{"jsonrpc":"2.0","id":7,"error":{"code":-1,"message":"boom"}}"#,
    )
    .await;
    let got = rx.await.unwrap();
    assert!(got.is_err());
    assert!(got.err().unwrap().contains("boom"));
}
