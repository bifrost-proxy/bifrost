use std::collections::HashMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use bifrost_core::{
    rule_share::{append_rule_share_query, new_rule_share_payload, share_payload_name_from_rule},
    RuleSyntaxReport, ValueStore,
};
use bifrost_storage::{
    build_rule_reference_catalog, ConfigManager, RuleFile, RulesStorage, ValuesStorage,
};
use bifrost_sync::{SyncAction, SyncManager};

use crate::cli::RuleCommands;

pub fn handle_rule_command(action: RuleCommands) -> bifrost_core::Result<()> {
    match action {
        RuleCommands::Sync => handle_rule_sync(),
        RuleCommands::Rename { name, new_name } => handle_rule_rename(&name, &new_name),
        RuleCommands::Reorder { names } => handle_rule_reorder(&names),
        RuleCommands::Active => handle_rule_active(),
        other => handle_rule_local(other),
    }
}

fn handle_rule_local(action: RuleCommands) -> bifrost_core::Result<()> {
    let storage = RulesStorage::new()?;

    match action {
        RuleCommands::List => {
            let rules = storage.list_summaries()?;
            if rules.is_empty() {
                println!("No rules found.");
            } else {
                println!("Rules ({}):", rules.len());
                for rule in rules {
                    let status = if rule.enabled { "enabled" } else { "disabled" };
                    println!("  {} [{}]", rule.name, status);
                }
            }
        }
        RuleCommands::Add {
            name,
            content,
            file,
            allow_invalid,
            json,
        } => {
            let rule_content = load_rule_content(content, file)?;
            let syntax = validate_local_rule_syntax(&storage, &name, &rule_content)?;
            if !syntax.valid && !allow_invalid {
                print_rule_syntax_rejection(&name, &syntax, json)?;
                std::process::exit(2);
            }

            let rule = RuleFile::new(&name, rule_content);
            storage.save(&rule)?;
            print_rule_mutation_success(&name, "added", &syntax, json)?;
        }
        RuleCommands::Update {
            name,
            content,
            file,
            allow_invalid,
            json,
        } => {
            let rule_content = load_rule_content(content, file)?;
            let syntax = validate_local_rule_syntax(&storage, &name, &rule_content)?;
            if !syntax.valid && !allow_invalid {
                print_rule_syntax_rejection(&name, &syntax, json)?;
                std::process::exit(2);
            }
            storage.update_content(&name, rule_content)?;
            print_rule_mutation_success(&name, "updated", &syntax, json)?;
        }
        RuleCommands::Delete { name } => {
            storage.delete(&name)?;
            println!("Rule '{}' deleted successfully.", name);
        }
        RuleCommands::Enable { name } => {
            storage.set_enabled(&name, true)?;
            println!("Rule '{}' enabled.", name);
        }
        RuleCommands::Disable { name } => {
            storage.set_enabled(&name, false)?;
            println!("Rule '{}' disabled.", name);
        }
        RuleCommands::Show { name } => {
            let rule = storage.load(&name)?;
            println!("Rule: {}", rule.name);
            println!(
                "Status: {}",
                if rule.enabled { "enabled" } else { "disabled" }
            );
            println!("Content:");
            println!("{}", rule.content);
        }
        RuleCommands::Share {
            name,
            target_url,
            content,
            file,
            exclusive_scope: _,
        } => {
            let rule_content = match (content, file) {
                (Some(content), file) => load_rule_content(Some(content), file)?,
                (None, Some(file)) => load_rule_content(None, Some(file))?,
                (None, None) => storage.load(&name)?.content,
            };
            let source_name = storage
                .load(&name)
                .map(|rule| share_payload_name_from_rule(&rule.name, rule.description.as_deref()))
                .unwrap_or_else(|_| share_payload_name_from_rule(&name, None));
            let payload = new_rule_share_payload(source_name, rule_content)?;
            let share_url = append_rule_share_query(&target_url, &payload)?;
            println!("{}", share_url);
        }
        RuleCommands::Sync
        | RuleCommands::Rename { .. }
        | RuleCommands::Reorder { .. }
        | RuleCommands::Active => {
            unreachable!()
        }
    }

    Ok(())
}

#[derive(Debug, Serialize)]
struct RuleMutationCliResponse<'a> {
    success: bool,
    message: String,
    saved: bool,
    syntax: &'a RuleSyntaxReport,
}

fn validate_local_rule_syntax(
    storage: &RulesStorage,
    name: &str,
    content: &str,
) -> bifrost_core::Result<RuleSyntaxReport> {
    let global_values = ValuesStorage::new()
        .ok()
        .map(|storage| storage.as_hashmap())
        .unwrap_or_default();
    Ok(validate_local_rule_syntax_with_values(
        storage,
        name,
        content,
        &global_values,
    ))
}

fn validate_local_rule_syntax_with_values(
    storage: &RulesStorage,
    name: &str,
    content: &str,
    global_values: &HashMap<String, String>,
) -> RuleSyntaxReport {
    let rule_files = storage.load_all_with_subdirs().unwrap_or_default();
    let mut reference_catalog = build_rule_reference_catalog(&rule_files);
    reference_catalog.insert(name.to_string(), content.to_string());
    bifrost_core::validate_rule_syntax_report(name, content, &reference_catalog, global_values)
}

fn print_rule_syntax_rejection(
    name: &str,
    syntax: &RuleSyntaxReport,
    json: bool,
) -> bifrost_core::Result<()> {
    let message = format!("Rule '{name}' was not saved because syntax validation failed");
    if json {
        print_rule_mutation_json(false, message, false, syntax)?;
        return Ok(());
    }

    eprintln!("Rule '{name}' was not saved: {}", syntax.guidance.summary);
    for error in &syntax.errors {
        let code = error.code.as_deref().unwrap_or("syntax");
        eprintln!(
            "{code} line {}:{}-{}: {}",
            error.line, error.start_column, error.end_column, error.message
        );
        if let Some(suggestion) = &error.suggestion {
            eprintln!("Suggestion: {suggestion}");
        }
    }
    for action in &syntax.guidance.next_actions {
        eprintln!("Next: {action}");
    }
    Ok(())
}

fn print_rule_mutation_success(
    name: &str,
    action: &str,
    syntax: &RuleSyntaxReport,
    json: bool,
) -> bifrost_core::Result<()> {
    let message = format!("Rule '{name}' {action} successfully.");
    if json {
        print_rule_mutation_json(true, message, true, syntax)?;
    } else {
        println!("{message}");
        for warning in &syntax.warnings {
            let code = warning.code.as_deref().unwrap_or("warning");
            println!("{code} line {}: {}", warning.line, warning.message);
            if let Some(suggestion) = &warning.suggestion {
                println!("Suggestion: {suggestion}");
            }
        }
    }
    Ok(())
}

fn print_rule_mutation_json(
    success: bool,
    message: String,
    saved: bool,
    syntax: &RuleSyntaxReport,
) -> bifrost_core::Result<()> {
    let response = RuleMutationCliResponse {
        success,
        message,
        saved,
        syntax,
    };
    println!("{}", serde_json::to_string_pretty(&response)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_active_summary_deserialization_full() {
        let json = r#"{
            "total": 3,
            "rules": [
                {"name": "dev-proxy", "rule_count": 5, "group_id": null, "group_name": null},
                {"name": "shared-mock", "rule_count": 2, "group_id": "g1", "group_name": "Team A"},
                {"name": "team-headers", "rule_count": 1, "group_id": "g1", "group_name": "Team A"}
            ],
            "variable_conflicts": [
                {
                    "variable_name": "API_HOST",
                    "definitions": [
                        {"rule_name": "dev-proxy", "value_preview": "localhost:3000"},
                        {"rule_name": "shared-mock", "value_preview": "mock.example.com"}
                    ]
                }
            ],
            "merged_content": "example.com 127.0.0.1:3000\napi.example.com proxy://proxy:8080"
        }"#;
        let resp: ActiveSummaryResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.total, 3);
        assert_eq!(resp.rules.len(), 3);
        assert!(resp.rules[0].group_id.is_none());
        assert_eq!(resp.rules[1].group_id.as_deref(), Some("g1"));
        assert_eq!(resp.rules[1].group_name.as_deref(), Some("Team A"));
        assert_eq!(resp.variable_conflicts.len(), 1);
        assert_eq!(resp.variable_conflicts[0].variable_name, "API_HOST");
        assert_eq!(resp.variable_conflicts[0].definitions.len(), 2);
        assert!(resp.merged_content.contains("example.com"));
        assert!(resp.merged_content.contains("proxy://proxy:8080"));
    }

    #[test]
    fn test_active_summary_deserialization_empty() {
        let json = r#"{"total": 0, "rules": [], "variable_conflicts": [], "merged_content": ""}"#;
        let resp: ActiveSummaryResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.total, 0);
        assert!(resp.rules.is_empty());
        assert!(resp.variable_conflicts.is_empty());
        assert!(resp.merged_content.is_empty());
    }

    #[test]
    fn test_active_summary_deserialization_missing_optional_fields() {
        let json = r#"{"total": 1, "rules": [{"name": "test", "rule_count": 3}]}"#;
        let resp: ActiveSummaryResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.total, 1);
        assert!(resp.rules[0].group_id.is_none());
        assert!(resp.rules[0].group_name.is_none());
        assert!(resp.variable_conflicts.is_empty());
        assert!(resp.merged_content.is_empty());
    }

    #[test]
    fn validate_local_rule_syntax_reports_valid_rules() {
        let dir = tempfile::tempdir().unwrap();
        let storage = RulesStorage::with_dir(dir.path().join("rules")).unwrap();

        let report = validate_local_rule_syntax_with_values(
            &storage,
            "valid",
            "example.com host://127.0.0.1:3000",
            &HashMap::new(),
        );

        assert!(report.valid);
        assert_eq!(report.rule_count, 1);
        assert!(report.errors.is_empty());
        assert_eq!(report.guidance.summary, "Rule syntax is valid.");
    }

    #[test]
    fn validate_local_rule_syntax_reports_missing_reference() {
        let dir = tempfile::tempdir().unwrap();
        let storage = RulesStorage::with_dir(dir.path().join("rules")).unwrap();

        let report = validate_local_rule_syntax_with_values(
            &storage,
            "entry",
            "@missing-shared",
            &HashMap::new(),
        );

        assert!(!report.valid);
        assert_eq!(report.errors.len(), 1);
        assert_eq!(report.errors[0].code.as_deref(), Some("E020"));
        assert!(!report.guidance.next_actions.is_empty());
    }

    #[test]
    fn validate_local_rule_syntax_rejects_invalid_port() {
        // Exercises the CLI add/update validation path against the host:port
        // validator. A non-numeric / zero / out-of-range port must be rejected
        // so the CLI exits non-zero instead of saving a broken rule.
        let dir = tempfile::tempdir().unwrap();
        let storage = RulesStorage::with_dir(dir.path().join("rules")).unwrap();

        let report = validate_local_rule_syntax_with_values(
            &storage,
            "bad-port",
            "example.com host://example.com:0",
            &HashMap::new(),
        );

        assert!(!report.valid, "port 0 must be rejected by CLI validation");
        assert!(
            report
                .errors
                .iter()
                .any(|e| e.code.as_deref() == Some("E017")),
            "expected E017 port error, got {:?}",
            report.errors
        );
    }

    #[test]
    fn validate_local_rule_syntax_rejects_invalid_status_code() {
        let dir = tempfile::tempdir().unwrap();
        let storage = RulesStorage::with_dir(dir.path().join("rules")).unwrap();

        let report = validate_local_rule_syntax_with_values(
            &storage,
            "bad-status",
            "example.com statusCode://999",
            &HashMap::new(),
        );

        assert!(
            !report.valid,
            "status 999 must be rejected by CLI validation"
        );
        assert!(
            report
                .errors
                .iter()
                .any(|e| e.code.as_deref() == Some("E010")),
            "expected E010 status error, got {:?}",
            report.errors
        );
    }

    #[test]
    fn validate_local_rule_syntax_allows_warning_only_rules() {
        // A warning (unknown HTTP method, E015) must not block the save: the
        // report stays valid while still surfacing the warning for the user.
        let dir = tempfile::tempdir().unwrap();
        let storage = RulesStorage::with_dir(dir.path().join("rules")).unwrap();

        let report = validate_local_rule_syntax_with_values(
            &storage,
            "warn-only",
            "example.com method://FOOBAR",
            &HashMap::new(),
        );

        assert!(report.valid, "warning-only rule should remain savable");
        assert!(
            report
                .warnings
                .iter()
                .any(|w| w.code.as_deref() == Some("E015")),
            "expected E015 method warning, got {:?}",
            report.warnings
        );
    }

    #[test]
    fn rule_mutation_cli_response_serializes_syntax_report() {
        // The JSON feedback emitted by `rule add --json` / `rule update --json`
        // must carry the saved flag and the full syntax report.
        let dir = tempfile::tempdir().unwrap();
        let storage = RulesStorage::with_dir(dir.path().join("rules")).unwrap();
        let syntax = validate_local_rule_syntax_with_values(
            &storage,
            "json-rule",
            "example.com host://127.0.0.1:3000",
            &HashMap::new(),
        );

        let response = RuleMutationCliResponse {
            success: true,
            message: "Rule 'json-rule' added successfully.".to_string(),
            saved: true,
            syntax: &syntax,
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("\"saved\":true"));
        assert!(json.contains("\"success\":true"));
        assert!(json.contains("json-rule"));
    }

    #[test]
    fn validate_local_rule_syntax_expands_existing_references() {
        let dir = tempfile::tempdir().unwrap();
        let storage = RulesStorage::with_dir(dir.path().join("rules")).unwrap();
        storage
            .save(&RuleFile::new(
                "shared",
                "api.example.com reqHeaders://X-Shared=ok",
            ))
            .unwrap();

        let report = validate_local_rule_syntax_with_values(
            &storage,
            "entry",
            "@shared\nexample.com host://127.0.0.1:3000",
            &HashMap::new(),
        );

        assert!(report.valid);
        assert_eq!(report.rule_count, 2);
    }

    #[test]
    fn format_active_summary_lines_groups_rules_and_variables() {
        let resp = ActiveSummaryResponse {
            total: 3,
            rules: vec![
                ActiveRuleItem {
                    name: "own".to_string(),
                    rule_count: 1,
                    group_id: None,
                    group_name: None,
                },
                ActiveRuleItem {
                    name: "team-a".to_string(),
                    rule_count: 2,
                    group_id: Some("g1".to_string()),
                    group_name: Some("Team A".to_string()),
                },
            ],
            variable_conflicts: vec![VariableConflict {
                variable_name: "API_HOST".to_string(),
                definitions: vec![VariableDefinition {
                    rule_name: "own".to_string(),
                    value_preview: "localhost".to_string(),
                }],
            }],
            merged_content: "example.com host://127.0.0.1:3000".to_string(),
        };

        let lines = format_active_summary_lines(&resp);
        assert!(lines.iter().any(|l| l.contains("My Rules")));
        assert!(lines.iter().any(|l| l.contains("Group: Team A (g1)")));
        assert!(lines.iter().any(|l| l.contains("Variable Conflicts (1)")));
        assert!(lines.iter().any(|l| l.contains("example.com")));
    }

    #[test]
    fn format_active_summary_lines_handles_empty_summary() {
        let resp = ActiveSummaryResponse {
            total: 0,
            rules: Vec::new(),
            variable_conflicts: Vec::new(),
            merged_content: String::new(),
        };
        let lines = format_active_summary_lines(&resp);
        assert!(lines.iter().any(|l| l.contains("No active rules.")));
    }
}

fn load_rule_content(
    content: Option<String>,
    file: Option<std::path::PathBuf>,
) -> bifrost_core::Result<String> {
    if let Some(c) = content {
        Ok(c)
    } else if let Some(path) = file {
        Ok(std::fs::read_to_string(&path)?)
    } else {
        Err(bifrost_core::BifrostError::Config(
            "Either --content or --file must be provided".to_string(),
        ))
    }
}

fn handle_rule_sync() -> bifrost_core::Result<()> {
    println!("🔄 Starting rules sync...");

    let data_dir = bifrost_storage::data_dir();
    let config_manager = Arc::new(ConfigManager::new(data_dir)?);
    let config = futures::executor::block_on(config_manager.config());

    println!("   Remote: {}", config.sync.remote_base_url);
    println!("   Enabled: {}", config.sync.enabled);

    let sync_manager = SyncManager::new(config_manager, 0)?;

    let rt = tokio::runtime::Runtime::new().map_err(|e| {
        bifrost_core::BifrostError::Config(format!("failed to create tokio runtime: {e}"))
    })?;

    let result = rt.block_on(sync_manager.sync_once())?;

    if result.success {
        println!("✅ {}", result.message);
        if let Some(user) = &result.user {
            println!("   User: {} ({})", user.nickname, user.user_id);
        }
        println!("   Local rules: {}", result.local_rules);
        println!("   Remote rules: {}", result.remote_rules);
        if let Some(action) = &result.action {
            let action_str = match action {
                SyncAction::LocalPushed => "Local → Remote (pushed local changes)",
                SyncAction::RemotePulled => "Remote → Local (pulled remote changes)",
                SyncAction::Bidirectional => "Bidirectional (both pushed and pulled)",
                SyncAction::NoChange => "No changes needed",
            };
            println!("   Action: {}", action_str);
        }
    } else {
        println!("❌ {}", result.message);
        if let Some(user) = &result.user {
            println!("   User: {} ({})", user.nickname, user.user_id);
        }
    }

    Ok(())
}

fn handle_rule_rename(name: &str, new_name: &str) -> bifrost_core::Result<()> {
    let port = crate::process::read_runtime_port().unwrap_or(9900);
    let client = super::config::client::ConfigApiClient::new("127.0.0.1", port);

    client
        .rename_rule(name, new_name)
        .map_err(bifrost_core::BifrostError::Config)?;

    println!("Rule '{}' renamed to '{}'.", name, new_name);
    Ok(())
}

fn handle_rule_reorder(names: &[String]) -> bifrost_core::Result<()> {
    let port = crate::process::read_runtime_port().unwrap_or(9900);
    let client = super::config::client::ConfigApiClient::new("127.0.0.1", port);

    client
        .reorder_rules(names)
        .map_err(bifrost_core::BifrostError::Config)?;

    println!("Rules reordered successfully:");
    for (i, name) in names.iter().enumerate() {
        println!("  {}. {}", i + 1, name);
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
pub(crate) struct ActiveRuleItem {
    pub(crate) name: String,
    pub(crate) rule_count: usize,
    pub(crate) group_id: Option<String>,
    pub(crate) group_name: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct VariableDefinition {
    pub(crate) rule_name: String,
    pub(crate) value_preview: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct VariableConflict {
    pub(crate) variable_name: String,
    pub(crate) definitions: Vec<VariableDefinition>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ActiveSummaryResponse {
    pub(crate) total: usize,
    pub(crate) rules: Vec<ActiveRuleItem>,
    #[serde(default)]
    pub(crate) variable_conflicts: Vec<VariableConflict>,
    #[serde(default)]
    pub(crate) merged_content: String,
}

pub(crate) fn fetch_active_summary_from_api(
    port: u16,
) -> bifrost_core::Result<ActiveSummaryResponse> {
    let url = format!(
        "http://127.0.0.1:{}/_bifrost/api/rules/active-summary",
        port
    );
    let response = bifrost_core::direct_ureq_agent_builder()
        .timeout(std::time::Duration::from_secs(3))
        .build()
        .get(&url)
        .call();

    let resp: ActiveSummaryResponse = match response {
        Ok(r) => r.into_json::<ActiveSummaryResponse>().map_err(|e| {
            bifrost_core::BifrostError::Config(format!("Failed to parse response: {e}"))
        })?,
        Err(e) => {
            return Err(bifrost_core::BifrostError::Config(format!(
                "Failed to connect to server on port {port}: {e}\nIs bifrost running? Try 'bifrost status' to check."
            )));
        }
    };

    Ok(resp)
}

pub(crate) fn format_active_summary_lines(resp: &ActiveSummaryResponse) -> Vec<String> {
    let mut lines = vec![
        "Active Rules Summary".to_string(),
        "====================".to_string(),
        String::new(),
    ];

    if resp.total == 0 {
        lines.push("No active rules.".to_string());
        return lines;
    }

    lines.push(format!("Total active: {} rule file(s)", resp.total));
    lines.push(String::new());

    let own_rules: Vec<_> = resp.rules.iter().filter(|r| r.group_id.is_none()).collect();
    let mut group_map: std::collections::BTreeMap<String, (String, Vec<&ActiveRuleItem>)> =
        std::collections::BTreeMap::new();
    for r in resp.rules.iter().filter(|r| r.group_id.is_some()) {
        let gid = r.group_id.as_deref().unwrap_or_default();
        let entry = group_map.entry(gid.to_string()).or_insert_with(|| {
            let display = r.group_name.as_deref().unwrap_or(gid);
            (display.to_string(), Vec::new())
        });
        entry.1.push(r);
    }

    if !own_rules.is_empty() {
        lines.push(format!("My Rules ({}):", own_rules.len()));
        for rule in &own_rules {
            lines.push(format!("  ⚡ {} ({} rules)", rule.name, rule.rule_count));
        }
        lines.push(String::new());
    }

    for (gid, (group_name, rules)) in &group_map {
        let label = if group_name != gid {
            format!("{group_name} ({gid})")
        } else {
            gid.clone()
        };
        lines.push(format!("Group: {} ({} file(s)):", label, rules.len()));
        for rule in rules {
            lines.push(format!("  ⚡ {} ({} rules)", rule.name, rule.rule_count));
        }
        lines.push(String::new());
    }

    if !resp.variable_conflicts.is_empty() {
        lines.push(format!(
            "⚠  Variable Conflicts ({}):",
            resp.variable_conflicts.len()
        ));
        for conflict in &resp.variable_conflicts {
            lines.push(format!("  {{{}}}", conflict.variable_name));
            for def in &conflict.definitions {
                lines.push(format!("    - {}: {}", def.rule_name, def.value_preview));
            }
        }
        lines.push(String::new());
    }

    lines.push("Merged Rules (in parsing order)".to_string());
    lines.push("-------------------------------".to_string());
    let content = resp.merged_content.trim();
    if content.is_empty() {
        lines.push("(empty)".to_string());
    } else {
        lines.push(content.to_string());
    }
    lines.push(String::new());

    lines
}

fn handle_rule_active() -> bifrost_core::Result<()> {
    let port = crate::process::read_runtime_port().unwrap_or(9900);
    let resp = fetch_active_summary_from_api(port)?;

    for line in format_active_summary_lines(&resp) {
        println!("{}", line);
    }

    Ok(())
}
