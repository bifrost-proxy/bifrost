use bifrost_agent::session::{run_turn, AgentSession};
use bifrost_agent::{AgentClient, AgentConfig, ToolRegistry};
use std::fs;

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
        &ToolRegistry::with_defaults(1),
        "/skill list",
        None,
    )
    .await
    .expect("run turn");

    assert!(!result.response.contains("未知命令"));
    assert!(result.response.contains("Skill 命令:"));
    assert!(result.response.contains("/weather"));
}

#[tokio::test]
async fn goal_slash_command_runs_through_session_router() {
    let mut session = AgentSession::new("goal-session");
    let result = run_turn(
        &AgentClient::new(),
        &AgentConfig::default(),
        &mut session,
        &ToolRegistry::with_defaults(1),
        "/goal set --budget 128 finish the p1 work",
        None,
    )
    .await
    .expect("run turn");

    assert!(result.response.contains("finish the p1 work"));
    assert!(result.response.contains("\"active\""));
}
