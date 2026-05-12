use bifrost_agent::persistence::ConversationRecorder;
use bifrost_agent::session::{run_turn, AgentSession};
use bifrost_agent::tools::goal::{
    execute_goal_tool, GoalPauseReason, GoalState, GoalStatus, GET_GOAL_TOOL_NAME,
};
use bifrost_agent::{AgentClient, AgentConfig, ToolRegistry};
use std::fs;
use std::path::{Path, PathBuf};

struct EnvGuard {
    old_data_dir: Option<String>,
}

impl EnvGuard {
    fn set_data_dir(path: &Path) -> Self {
        let guard = Self {
            old_data_dir: std::env::var("BIFROST_DATA_DIR").ok(),
        };
        unsafe {
            std::env::set_var("BIFROST_DATA_DIR", path);
        }
        guard
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        unsafe {
            match &self.old_data_dir {
                Some(value) => std::env::set_var("BIFROST_DATA_DIR", value),
                None => std::env::remove_var("BIFROST_DATA_DIR"),
            }
        }
    }
}

#[tokio::test]
async fn new_with_work_dir_attaches_skill_registry_for_skill_list() {
    let work_dir = tempfile::tempdir().expect("work dir");
    let skill_dir = work_dir.path().join(".agents/skills/weather");
    fs::create_dir_all(&skill_dir).expect("skill dir");
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: weather\ndescription: Weather helper\nslash_command: /weather\n---\n# Weather",
    )
    .expect("skill md");

    let mut session = AgentSession::new_with_work_dir(
        "skill-session",
        Some(work_dir.path().to_string_lossy().to_string()),
    );
    let result = run_turn(
        &AgentClient::new(),
        &AgentConfig::default(),
        &mut session,
        &ToolRegistry::with_defaults(),
        "/skill list",
        None,
    )
    .await
    .expect("run turn");

    assert!(!result.response.contains("未知命令"));
    assert!(result.response.contains("已注册的 skill slash 命令:"));
    assert!(result.response.contains("/weather"));
}

#[tokio::test]
async fn reinitialize_work_dir_reloads_repo_local_skills_for_new_session_context() {
    let first_work_dir = tempfile::tempdir().expect("first work dir");
    let second_work_dir = tempfile::tempdir().expect("second work dir");

    let first_skill_dir = first_work_dir.path().join(".agents/skills/first");
    fs::create_dir_all(&first_skill_dir).expect("first skill dir");
    fs::write(
        first_skill_dir.join("SKILL.md"),
        "---\nname: first\ndescription: First helper\nslash_command: /first\n---\n# First",
    )
    .expect("first skill md");

    let second_skill_dir = second_work_dir.path().join(".agents/skills/second");
    fs::create_dir_all(&second_skill_dir).expect("second skill dir");
    fs::write(
        second_skill_dir.join("SKILL.md"),
        "---\nname: second\ndescription: Second helper\nslash_command: /second\n---\n# Second",
    )
    .expect("second skill md");

    let mut session = AgentSession::new_with_work_dir(
        "skill-session",
        Some(first_work_dir.path().to_string_lossy().to_string()),
    );
    session
        .history
        .push(bifrost_agent::types::ChatMessage::user("old context"));
    let second_work_dir_string = second_work_dir.path().to_string_lossy().to_string();
    session.reinitialize_work_dir(second_work_dir_string.clone());

    assert!(session.history.is_empty());
    assert_eq!(
        session.work_dir.as_deref(),
        Some(second_work_dir_string.as_str())
    );

    let result = run_turn(
        &AgentClient::new(),
        &AgentConfig::default(),
        &mut session,
        &ToolRegistry::with_defaults(),
        "/skill list",
        None,
    )
    .await
    .expect("run turn");

    assert!(result.response.contains("/second"));
    assert!(!result.response.contains("/first"));
}

#[tokio::test]
async fn goal_slash_command_runs_through_session_router() {
    let mut session = AgentSession::new("goal-session");
    let result = run_turn(
        &AgentClient::new(),
        &AgentConfig::default(),
        &mut session,
        &ToolRegistry::with_defaults(),
        "/goal set --budget 128 finish the p1 work",
        None,
    )
    .await
    .expect("run turn");

    assert!(result.response.contains("finish the p1 work"));
    assert!(result.response.contains("\"status\": \"active\""));
    assert!(result.response.contains("\"threadId\": \"goal-session\""));
    assert!(!result.response.contains("\"goalId\":"));
    assert!(result.response.contains("\"remainingTokens\": 128"));
}

#[tokio::test]
async fn goal_slash_command_is_session_scoped() {
    let mut session_a = AgentSession::new("goal-session-a");
    let mut session_b = AgentSession::new("goal-session-b");
    let tools = ToolRegistry::with_defaults();

    let set_goal = run_turn(
        &AgentClient::new(),
        &AgentConfig::default(),
        &mut session_a,
        &tools,
        "/goal set isolate session a",
        None,
    )
    .await
    .expect("set goal");
    assert!(set_goal.response.contains("isolate session a"));

    let show_other = run_turn(
        &AgentClient::new(),
        &AgentConfig::default(),
        &mut session_b,
        &tools,
        "/goal show",
        None,
    )
    .await
    .expect("show goal");
    assert!(show_other.response.contains("\"goal\": null"));
}

#[tokio::test]
async fn goal_slash_command_pause_and_resume_updates_status() {
    let mut session = AgentSession::new("goal-session-pause-resume");
    let tools = ToolRegistry::with_defaults();

    let created = run_turn(
        &AgentClient::new(),
        &AgentConfig::default(),
        &mut session,
        &tools,
        "/goal set --budget 64 verify pause resume",
        None,
    )
    .await
    .expect("create goal");
    assert!(created.response.contains("\"status\": \"active\""));

    let paused = run_turn(
        &AgentClient::new(),
        &AgentConfig::default(),
        &mut session,
        &tools,
        "/goal pause",
        None,
    )
    .await
    .expect("pause goal");
    assert!(paused.response.contains("\"status\": \"paused\""));

    let resumed = run_turn(
        &AgentClient::new(),
        &AgentConfig::default(),
        &mut session,
        &tools,
        "/goal resume",
        None,
    )
    .await
    .expect("resume goal");
    assert!(resumed.response.contains("\"status\": \"active\""));
}

#[tokio::test]
async fn resume_restores_total_tokens_and_reactivates_interrupted_goal() {
    let data_dir = tempfile::tempdir().expect("agent data dir");
    let _guard = EnvGuard::set_data_dir(data_dir.path());
    let agent_home = bifrost_agent::config::agent_home_dir();

    let goal = GoalState {
        goal_id: "goal-resume-1".to_string(),
        objective: "resume interrupted work".to_string(),
        status: GoalStatus::Paused,
        pause_reason: Some(GoalPauseReason::Interrupted),
        token_budget: Some(100),
        created_at: 1,
        updated_at: 2,
        accumulated_tokens_used: 50,
        accumulated_time_used_seconds: 12,
        active_total_tokens_baseline: None,
        active_started_at: None,
        start_total_tokens: 100,
        completed_total_tokens: None,
        completed_time_used_seconds: None,
    };

    let recorder = {
        let mut recorder = ConversationRecorder::new(&agent_home, "resume-goal-session");
        recorder
            .record_user_message("resume-goal-session", "continue")
            .expect("record user");
        recorder
            .record_goal_updated("resume-goal-session", &goal)
            .expect("record goal");
        let recorder_path = PathBuf::from(recorder.file_path());
        recorder.close();
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&recorder_path)
            .expect("open recorder file");
        use std::io::Write;
        writeln!(
            file,
            r#"{{"timestamp":3,"event_type":"session_end","session_key":"resume-goal-session","content":{{"total_tokens":150}}}}"#
        )
        .expect("append session end");
        recorder_path
    };

    assert!(recorder.exists());

    let mut session = AgentSession::new("resume-goal-session");
    let result = run_turn(
        &AgentClient::new(),
        &AgentConfig::default(),
        &mut session,
        &ToolRegistry::with_defaults(),
        "/resume",
        None,
    )
    .await
    .expect("resume turn");

    assert!(result.response.contains("已恢复会话历史"));
    assert_eq!(session.total_tokens_used, Some(150));
    let current_goal = session.current_goal.as_ref().expect("restored goal");
    assert_eq!(current_goal.status, GoalStatus::Active);
    assert_eq!(current_goal.pause_reason, None);

    session.total_tokens_used = Some(session.total_tokens_used.unwrap_or(0) + 25);
    let goal_snapshot = execute_goal_tool(&mut session, GET_GOAL_TOOL_NAME, "{}").unwrap();
    assert!(goal_snapshot.output.contains("\"tokensUsed\": 75"));
}
