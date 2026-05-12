use base64::Engine;
use bifrost_agent::tools::goal::{
    execute_goal_tool, CREATE_GOAL_TOOL_NAME, GET_GOAL_TOOL_NAME, UPDATE_GOAL_TOOL_NAME,
};
use bifrost_agent::tools::tool_search::{
    parse_loadable_tool_definitions, ToolSearchEntry, ToolSearchTool,
};
use bifrost_agent::tools::ToolHandler;
use bifrost_agent::types::ToolDefinition;
use bifrost_agent::{AgentSession, ToolRegistry};
use std::fs;

fn session_id_from_exec_json(output: &str) -> String {
    let value: serde_json::Value = serde_json::from_str(output).expect("exec json output");
    session_id_value_to_string(&value["session_id"])
}

fn session_id_value_to_string(value: &serde_json::Value) -> String {
    if let Some(session_id) = value.as_i64() {
        return session_id.to_string();
    }
    value.as_str().expect("session id").to_string()
}

#[tokio::test]
async fn goal_tools_work_end_to_end() {
    let mut session = AgentSession::new("goal-e2e");
    session.total_tokens_used = Some(64);

    let initial = execute_goal_tool(&mut session, GET_GOAL_TOOL_NAME, "{}").expect("get goal");
    assert!(initial.success);
    assert!(initial.output.contains("\"goal\": null"));

    let created = execute_goal_tool(
        &mut session,
        CREATE_GOAL_TOOL_NAME,
        r#"{"objective":"close the p1 gap","token_budget":2048}"#,
    )
    .expect("create goal");
    assert!(created.success, "{}", created.output);
    assert!(created.output.contains("close the p1 gap"));
    assert!(created.output.contains("\"active\""));
    assert!(created.output.contains("\"threadId\": \"goal-e2e\""));
    assert!(!created.output.contains("\"goalId\":"));
    assert!(created.output.contains("\"remainingTokens\": 2048"));

    session.total_tokens_used = Some(512);
    let updated = execute_goal_tool(
        &mut session,
        UPDATE_GOAL_TOOL_NAME,
        r#"{"status":"complete"}"#,
    )
    .expect("update goal");
    assert!(updated.success, "{}", updated.output);
    assert!(updated.output.contains("\"complete\""));
    assert!(updated
        .output
        .contains("Goal achieved. Report final budget usage to the user"));

    let final_goal = execute_goal_tool(&mut session, GET_GOAL_TOOL_NAME, "{}").expect("get goal");
    assert!(final_goal.success);
    assert!(final_goal.output.contains("\"complete\""));
}

#[tokio::test]
async fn apply_patch_tool_works_end_to_end() {
    let registry = ToolRegistry::with_defaults(5);
    let work_dir = tempfile::tempdir().expect("work dir");

    fs::write(
        work_dir.path().join("main.rs"),
        "fn main() {\n    println!(\"old\");\n}\n",
    )
    .expect("seed main");
    fs::write(work_dir.path().join("obsolete.txt"), "bye\n").expect("seed obsolete");

    let patch = r#"*** Begin Patch
*** Update File: main.rs
*** Move to: src/main.rs
@@ fn main() {
-    println!("old");
+    println!("new");
*** Delete File: obsolete.txt
*** Add File: notes.txt
+structured patch works
*** End Patch"#;

    let args = serde_json::json!({ "input": patch }).to_string();
    let result = registry
        .execute("apply_patch", &args, work_dir.path())
        .await;
    assert!(result.success, "{}", result.output);
    assert!(result.output.contains("3 file(s) changed"));
    assert!(!work_dir.path().join("main.rs").exists());
    assert!(!work_dir.path().join("obsolete.txt").exists());
    assert_eq!(
        fs::read_to_string(work_dir.path().join("src/main.rs")).expect("renamed main"),
        "fn main() {\n    println!(\"new\");\n}\n"
    );
    assert_eq!(
        fs::read_to_string(work_dir.path().join("notes.txt")).expect("notes"),
        "structured patch works\n"
    );
}

#[tokio::test]
async fn legacy_shell_pty_is_not_registered_by_default() {
    let registry = ToolRegistry::with_defaults(5);
    let work_dir = tempfile::tempdir().expect("work dir");

    assert!(!registry.contains_tool("shell_pty"));
    assert!(!registry
        .definitions()
        .iter()
        .any(|definition| definition.name() == "shell_pty"));

    let result = registry
        .execute(
            "shell_pty",
            r#"{"command":"export P1_TOOL_TEST=bifrost_ok && echo ready","wait_for_completion":false}"#,
            work_dir.path(),
        )
        .await;
    assert!(!result.success);
    assert!(result.output.contains("unknown tool: shell_pty"));
}

#[tokio::test]
async fn exec_command_tool_works_end_to_end() {
    let registry = ToolRegistry::with_defaults(5);
    let work_dir = tempfile::tempdir().expect("work dir");

    let completed = registry
        .execute(
            "exec_command",
            r#"{"cmd":"printf exec-ok","yield_time_ms":500,"max_output_tokens":1000}"#,
            work_dir.path(),
        )
        .await;
    assert!(completed.success, "{}", completed.output);
    let completed_json: serde_json::Value =
        serde_json::from_str(&completed.output).expect("completed exec json");
    assert_eq!(completed_json["exit_code"], 0);
    assert_eq!(completed_json["output"], "exec-ok");
    assert!(completed_json["session_id"].is_null());

    let long_running = registry
        .execute(
            "exec_command",
            &serde_json::json!({
                "cmd": "printf long-start; sleep 0.3; printf long-end",
                "yield_time_ms": 50,
                "max_output_tokens": 1000,
            })
            .to_string(),
            work_dir.path(),
        )
        .await;
    assert!(long_running.success, "{}", long_running.output);
    let long_running_json: serde_json::Value =
        serde_json::from_str(&long_running.output).expect("long-running exec json");
    let long_session_id = session_id_value_to_string(&long_running_json["session_id"]);

    let mut final_poll_json = serde_json::Value::Null;
    for _ in 0..20 {
        let poll = registry
            .execute(
                "write_stdin",
                &serde_json::json!({
                    "session_id": long_session_id,
                    "chars": "",
                    "yield_time_ms": 100,
                    "max_output_tokens": 1000,
                })
                .to_string(),
                work_dir.path(),
            )
            .await;
        assert!(poll.success, "{}", poll.output);
        final_poll_json = serde_json::from_str(&poll.output).expect("long poll json");
        if !final_poll_json["exit_code"].is_null() {
            break;
        }
    }
    assert_eq!(final_poll_json["exit_code"], 0);
    assert!(final_poll_json["output"]
        .as_str()
        .unwrap_or("")
        .contains("long-end"));

    let interactive = registry
        .execute(
            "exec_command",
            &serde_json::json!({
                "cmd": "python3 -u -c 'import os,sys; print(os.isatty(0), os.isatty(1)); print(\"exec-ready\"); print(sys.stdin.readline().strip())'",
                "tty": true,
                "yield_time_ms": 5000,
            })
            .to_string(),
            work_dir.path(),
        )
        .await;
    assert!(interactive.success, "{}", interactive.output);
    assert!(interactive.output.contains("True True"));
    assert!(interactive.output.contains("exec-ready"));
    let session_id = session_id_from_exec_json(&interactive.output);

    let stdin_result = registry
        .execute(
            "write_stdin",
            &serde_json::json!({
                "session_id": session_id,
                "chars": "hello exec\n",
                "yield_time_ms": 1000,
                "max_output_tokens": 1000,
            })
            .to_string(),
            work_dir.path(),
        )
        .await;
    assert!(stdin_result.success, "{}", stdin_result.output);
    assert!(stdin_result.output.contains("hello exec"));
}

#[tokio::test]
#[ignore = "requires a locally installed and authenticated Codex CLI; run manually for real PTY regression"]
async fn codex_cli_interactive_starts_in_real_pty() {
    let registry = ToolRegistry::with_defaults(10);
    let work_dir = tempfile::tempdir().expect("work dir");

    let started = registry
        .execute(
            "exec_command",
            &serde_json::json!({
                "cmd": "codex --sandbox read-only",
                "tty": true,
                "yield_time_ms": 3000,
                "max_output_tokens": 4000,
            })
            .to_string(),
            work_dir.path(),
        )
        .await;
    assert!(started.success, "{}", started.output);
    let started_json: serde_json::Value =
        serde_json::from_str(&started.output).expect("codex exec json");
    let output = started_json["output"].as_str().unwrap_or("");
    assert!(
        output.contains("Codex")
            || output.contains("codex")
            || output.contains("login")
            || output.contains("OpenAI"),
        "{}",
        started_json
    );
    let session_id = session_id_value_to_string(&started_json["session_id"]);

    let interrupted = registry
        .execute(
            "write_stdin",
            &serde_json::json!({
                "session_id": session_id,
                "chars": "\u{3}",
                "yield_time_ms": 1000,
                "max_output_tokens": 1000,
            })
            .to_string(),
            work_dir.path(),
        )
        .await;
    assert!(interrupted.success, "{}", interrupted.output);
}

#[tokio::test]
async fn view_image_tool_works_end_to_end() {
    let registry = ToolRegistry::with_defaults(5);
    let work_dir = tempfile::tempdir().expect("work dir");
    fs::write(
        work_dir.path().join("tiny.png"),
        base64::engine::general_purpose::STANDARD
            .decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+/p9sAAAAASUVORK5CYII=")
            .expect("tiny png"),
    )
    .expect("write png");

    let result = registry
        .execute(
            "view_image",
            r#"{"path":"tiny.png","detail":"original"}"#,
            work_dir.path(),
        )
        .await;
    assert!(result.success, "{}", result.output);
    let value: serde_json::Value = serde_json::from_str(&result.output).expect("view image json");
    assert!(value["image_url"]
        .as_str()
        .expect("data url")
        .starts_with("data:image/png;base64,"));
    assert_eq!(value["detail"], "original");
}

#[tokio::test]
async fn tool_search_is_hidden_without_deferred_tools() {
    let registry = ToolRegistry::with_defaults(5);
    assert!(!registry
        .tool_names()
        .iter()
        .any(|name| name == "tool_search"));
    assert!(!registry
        .definitions()
        .iter()
        .any(|definition| definition.name() == "tool_search"));
}

#[tokio::test]
async fn tool_search_returns_deferred_mcp_tools() {
    let deferred = ToolDefinition::function(
        "mcp_test__exec_command".to_string(),
        "Run a command on the MCP side".to_string(),
        Some(serde_json::json!({"type":"object"})),
    );
    let tool = ToolSearchTool::new(vec![ToolSearchEntry::new(
        deferred.clone(),
        "MCP tools: test",
    )]);
    let work_dir = tempfile::tempdir().expect("work dir");
    let result = tool
        .execute(r#"{"query":"exec command","limit":5}"#, work_dir.path())
        .await;
    assert!(result.success, "{}", result.output);
    let loaded = parse_loadable_tool_definitions(&result.output);
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].name(), deferred.name());
}
