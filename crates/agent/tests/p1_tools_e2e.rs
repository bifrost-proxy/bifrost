use bifrost_agent::tools::goal::{
    execute_goal_tool, CREATE_GOAL_TOOL_NAME, GET_GOAL_TOOL_NAME, UPDATE_GOAL_TOOL_NAME,
};
use bifrost_agent::{AgentSession, ToolRegistry};
use std::fs;

fn session_id_from_output(output: &str) -> String {
    output
        .lines()
        .find(|line| line.starts_with("session_id: "))
        .expect("session_id line")
        .trim_start_matches("session_id: ")
        .to_string()
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
    assert!(created.output.contains("\"goalId\":"));
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
async fn pty_tools_work_end_to_end() {
    let registry = ToolRegistry::with_defaults(5);
    let work_dir = tempfile::tempdir().expect("work dir");

    let create_session = registry
        .execute(
            "shell_pty",
            r#"{"command":"export P1_TOOL_TEST=bifrost_ok && echo ready"}"#,
            work_dir.path(),
        )
        .await;
    assert!(create_session.success, "{}", create_session.output);
    assert!(create_session.output.contains("exit_indicator: done"));
    assert!(create_session.output.contains("exit_code: 0"));
    assert!(create_session.output.contains("ready"));
    let session_id = session_id_from_output(&create_session.output);

    let reuse = registry
        .execute(
            "shell_pty",
            &serde_json::json!({
                "command": "echo $P1_TOOL_TEST",
                "session_id": session_id,
            })
            .to_string(),
            work_dir.path(),
        )
        .await;
    assert!(reuse.success, "{}", reuse.output);
    assert!(reuse.output.contains("bifrost_ok"));

    let interactive = registry
        .execute(
            "shell_pty",
            &serde_json::json!({
                "command": "python3 -u -c 'import sys; print(\"ready\"); print(sys.stdin.readline().strip())'",
                "wait_for_completion": false,
                "yield_time_ms": 5000,
            })
            .to_string(),
            work_dir.path(),
        )
        .await;
    assert!(interactive.success, "{}", interactive.output);
    assert!(interactive.output.contains("exit_indicator: running"));
    assert!(interactive.output.contains("ready"));
    let interactive_session_id = session_id_from_output(&interactive.output);

    let stdin_result = registry
        .execute(
            "write_stdin",
            &serde_json::json!({
                "session_id": interactive_session_id,
                "input": "hello pty\n",
                "yield_time_ms": 1000,
            })
            .to_string(),
            work_dir.path(),
        )
        .await;
    assert!(stdin_result.success, "{}", stdin_result.output);
    assert!(stdin_result.output.contains("hello pty"));
}
