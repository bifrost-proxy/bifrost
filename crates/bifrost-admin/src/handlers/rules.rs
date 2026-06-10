use bifrost_core::{
    expand_rule_references, extract_inline_variables, validate_rules_with_context, ParseError,
    ParseErrorSeverity, ScriptReference, VariableInfo,
};
use bifrost_storage::{ConfigChangeEvent, RuleFile, RuleSummary, RulesStorage};
use http_body_util::BodyExt;
use hyper::{body::Incoming, Method, Request, Response, StatusCode};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::time::Duration;

use super::{error_response, json_response, method_not_allowed, success_response, BoxBody};
use crate::push::SharedPushManager;
use crate::state::SharedAdminState;

#[derive(Debug, Serialize)]
struct RuleFileInfo {
    name: String,
    enabled: bool,
    sort_order: i32,
    rule_count: usize,
    created_at: String,
    updated_at: String,
}

#[derive(Debug, Serialize)]
struct RuleReferenceCandidate {
    name: String,
    rule_name: String,
    group_name: Option<String>,
    group_id: Option<String>,
}

#[derive(Debug, Serialize)]
struct RuleFileDetail {
    name: String,
    content: String,
    enabled: bool,
    sort_order: i32,
    created_at: String,
    updated_at: String,
    sync: RuleSyncInfo,
}

#[derive(Debug, Serialize)]
struct RuleSyncInfo {
    status: String,
    last_synced_at: Option<String>,
    remote_id: Option<String>,
    remote_updated_at: Option<String>,
}

fn sync_status_label(status: bifrost_storage::RuleSyncStatus) -> &'static str {
    match status {
        bifrost_storage::RuleSyncStatus::LocalOnly => "local_only",
        bifrost_storage::RuleSyncStatus::Synced => "synced",
        bifrost_storage::RuleSyncStatus::Modified => "modified",
    }
}

const GROUP_CACHE_RESOLUTION_TIMEOUT: Duration = Duration::from_millis(1_000);

#[derive(Debug, Deserialize)]
struct CreateRuleRequest {
    name: String,
    content: String,
    enabled: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct UpdateRuleRequest {
    content: Option<String>,
    enabled: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct ReorderRulesRequest {
    order: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct RenameRuleRequest {
    new_name: String,
}

#[derive(Debug, Serialize)]
struct ActiveRuleItem {
    name: String,
    rule_count: usize,
    group_id: Option<String>,
    group_name: Option<String>,
}

#[derive(Debug, Serialize)]
struct ActiveSummaryResponse {
    total: usize,
    rules: Vec<ActiveRuleItem>,
    variable_conflicts: Vec<VariableConflict>,
    merged_content: String,
}

#[derive(Debug, Serialize)]
struct VariableConflict {
    variable_name: String,
    definitions: Vec<VariableDefinition>,
}

#[derive(Debug, Serialize)]
struct VariableDefinition {
    rule_name: String,
    group_id: Option<String>,
    value_preview: String,
}

#[derive(Debug, Deserialize)]
struct ValidateRuleRequest {
    content: String,
    #[serde(default)]
    global_values: HashMap<String, String>,
    #[serde(default)]
    current_rule_name: Option<String>,
    #[serde(default)]
    current_group_name: Option<String>,
}

#[derive(Debug, Serialize)]
struct ValidateRuleResponse {
    valid: bool,
    rule_count: usize,
    errors: Vec<ParseError>,
    warnings: Vec<ParseError>,
    defined_variables: Vec<VariableInfo>,
    script_references: Vec<ScriptReference>,
}

const BUILTIN_DECODE_SCRIPTS: &[&str] = &["utf8", "default", "bp"];
pub async fn handle_rules(
    req: Request<Incoming>,
    state: SharedAdminState,
    push_manager: Option<SharedPushManager>,
    path: &str,
) -> Response<BoxBody> {
    let method = req.method().clone();

    if path == "/api/rules" || path == "/api/rules/" {
        match method {
            Method::GET => list_rules(state).await,
            Method::POST => create_rule(req, state, push_manager).await,
            _ => method_not_allowed(),
        }
    } else if path == "/api/rules/reorder" {
        match method {
            Method::PUT => reorder_rules(req, state, push_manager).await,
            _ => method_not_allowed(),
        }
    } else if path == "/api/rules/active-summary" {
        match method {
            Method::GET => active_summary(state).await,
            _ => method_not_allowed(),
        }
    } else if path == "/api/rules/reference-candidates" {
        match method {
            Method::GET => list_reference_candidates(state).await,
            _ => method_not_allowed(),
        }
    } else if path == "/api/rules/validate" {
        match method {
            Method::POST => validate_rule(req, state).await,
            _ => method_not_allowed(),
        }
    } else if let Some(name) = path.strip_prefix("/api/rules/") {
        let name = urlencoding::decode(name).unwrap_or_default();
        let name = name.as_ref();

        if let Some(name) = name.strip_suffix("/enable") {
            match method {
                Method::PUT => enable_rule(state, name, true, push_manager).await,
                _ => method_not_allowed(),
            }
        } else if let Some(name) = name.strip_suffix("/disable") {
            match method {
                Method::PUT => enable_rule(state, name, false, push_manager).await,
                _ => method_not_allowed(),
            }
        } else if let Some(name) = name.strip_suffix("/rename") {
            match method {
                Method::PUT => rename_rule(req, state, name, push_manager).await,
                _ => method_not_allowed(),
            }
        } else {
            match method {
                Method::GET => get_rule(state, name).await,
                Method::PUT => update_rule(req, state, name, push_manager).await,
                Method::DELETE => delete_rule(state, name, push_manager).await,
                _ => method_not_allowed(),
            }
        }
    } else {
        error_response(StatusCode::NOT_FOUND, "Not Found")
    }
}

async fn list_rules(state: SharedAdminState) -> Response<BoxBody> {
    match state.rules_storage.list_summaries() {
        Ok(rules) => {
            let infos: Vec<RuleFileInfo> = rules
                .into_iter()
                .map(|r: RuleSummary| RuleFileInfo {
                    name: r.name,
                    enabled: r.enabled,
                    sort_order: r.sort_order,
                    rule_count: r.rule_count,
                    created_at: r.created_at,
                    updated_at: r.updated_at,
                })
                .collect();
            json_response(&infos)
        }
        Err(e) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("Failed to list rules: {}", e),
        ),
    }
}

async fn list_reference_candidates(state: SharedAdminState) -> Response<BoxBody> {
    let reverse_cache = {
        let cache = state.group_name_cache();
        cache
            .entries()
            .into_iter()
            .map(|(group_id, group_name)| (group_name, group_id))
            .collect::<HashMap<_, _>>()
    };

    let rule_files = match state.rules_storage.load_all_with_subdirs() {
        Ok(rules) => rules,
        Err(e) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("Failed to list rule references: {}", e),
            )
        }
    };

    let mut candidates = rule_files
        .into_iter()
        .map(|rule| {
            let group_name = rule.group.clone();
            let group_id = group_name
                .as_deref()
                .and_then(|name| reverse_cache.get(name).cloned());
            RuleReferenceCandidate {
                name: bifrost_storage::rule_reference_key(&rule),
                rule_name: rule.name,
                group_name,
                group_id,
            }
        })
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| left.name.cmp(&right.name));

    json_response(&candidates)
}

struct InlineVarEntry {
    rule_name: String,
    group_id: Option<String>,
    value: String,
}

fn collect_enabled_from_storage(
    storage: &RulesStorage,
    group_id: Option<&str>,
    group_name: Option<&str>,
    var_map: &mut HashMap<String, Vec<InlineVarEntry>>,
    content_parts: &mut Vec<String>,
) -> Vec<ActiveRuleItem> {
    let rules = match storage.load_enabled() {
        Ok(r) => r,
        Err(_) => return Vec::new(),
    };
    let mut items = Vec::new();
    for rule in rules {
        let rule_count = rule
            .content
            .lines()
            .filter(|l| {
                let t = l.trim();
                !t.is_empty() && !t.starts_with('#')
            })
            .count();

        let inline_vars = extract_inline_variables(&rule.content);
        for (var_name, var_value) in inline_vars {
            var_map.entry(var_name).or_default().push(InlineVarEntry {
                rule_name: rule.name.clone(),
                group_id: group_id.map(|s| s.to_string()),
                value: var_value,
            });
        }

        content_parts.push(rule.content.clone());

        items.push(ActiveRuleItem {
            name: rule.name,
            rule_count,
            group_id: group_id.map(|s| s.to_string()),
            group_name: group_name.map(|s| s.to_string()),
        });
    }
    items
}

fn reverse_group_cache_for_dirs(
    state: &SharedAdminState,
    group_dirs: &[(String, std::path::PathBuf)],
) -> HashMap<String, String> {
    let cache = state.group_name_cache();
    let mut map = HashMap::new();
    for (dir_name, _) in group_dirs {
        if let Some(gid) = cache.reverse_lookup(dir_name) {
            map.insert(dir_name.clone(), gid);
        }
    }
    map
}

async fn active_summary(state: SharedAdminState) -> Response<BoxBody> {
    let mut all_rules = Vec::new();
    let mut var_map: HashMap<String, Vec<InlineVarEntry>> = HashMap::new();
    let mut content_parts: Vec<String> = Vec::new();

    match state.rules_storage.load_enabled() {
        Ok(rules) => {
            for rule in rules {
                let rule_count = rule
                    .content
                    .lines()
                    .filter(|l| {
                        let t = l.trim();
                        !t.is_empty() && !t.starts_with('#')
                    })
                    .count();

                let inline_vars = extract_inline_variables(&rule.content);
                for (var_name, var_value) in inline_vars {
                    var_map.entry(var_name).or_default().push(InlineVarEntry {
                        rule_name: rule.name.clone(),
                        group_id: None,
                        value: var_value,
                    });
                }

                content_parts.push(rule.content.clone());

                all_rules.push(ActiveRuleItem {
                    name: rule.name,
                    rule_count,
                    group_id: None,
                    group_name: None,
                });
            }
        }
        Err(e) => {
            tracing::warn!(
                target: "bifrost_admin::rules",
                error = %e,
                "Failed to list own rules for active summary"
            );
        }
    }

    let base_dir = state.rules_storage.base_dir();
    let has_group_session = state
        .sync_manager
        .as_ref()
        .map(|sm| sm.has_session())
        .unwrap_or(false);

    let group_dirs: Vec<(String, std::path::PathBuf)> = std::fs::read_dir(base_dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().is_dir())
        .map(|e| {
            let dir_name = e.file_name().to_string_lossy().to_string();
            (dir_name, e.path())
        })
        .collect();

    if !has_group_session && !group_dirs.is_empty() {
        tracing::info!(
            target: "bifrost_admin::rules",
            count = group_dirs.len(),
            "active summary skipped group rules without active sync session"
        );
        let variable_conflicts = build_variable_conflicts(var_map);
        let merged_content = content_parts.join("\n");
        let resp = ActiveSummaryResponse {
            total: all_rules.len(),
            rules: all_rules,
            variable_conflicts,
            merged_content,
        };
        return json_response(&resp);
    }

    let reverse_cache = reverse_group_cache_for_dirs(&state, &group_dirs);

    let uncached_dirs: Vec<String> = group_dirs
        .iter()
        .filter(|(d, _)| !reverse_cache.contains_key(d))
        .map(|(d, _)| d.clone())
        .collect();

    if !uncached_dirs.is_empty() {
        tracing::debug!(
            target: "bifrost_admin::rules",
            dirs = ?uncached_dirs,
            "active summary using local group directory names for uncached groups"
        );
        if !state.is_group_cache_resolved() {
            if let Some(sm) = state.sync_manager.clone() {
                if sm.has_session() {
                    state.set_group_cache_resolved();
                    let state_for_task = state.clone();
                    let dirs_for_task = uncached_dirs.clone();
                    tokio::spawn(async move {
                        let resolved =
                            tokio::time::timeout(GROUP_CACHE_RESOLUTION_TIMEOUT, async {
                                super::group_rules::resolve_missing_group_caches(
                                    &sm,
                                    &state_for_task,
                                    &dirs_for_task,
                                )
                                .await;
                                let cache = state_for_task.group_name_cache();
                                dirs_for_task
                                    .iter()
                                    .all(|dir| cache.reverse_lookup(dir).is_some())
                            })
                            .await;
                        let all_resolved = match resolved {
                            Ok(all_resolved) => all_resolved,
                            Err(_) => {
                                tracing::warn!(
                                    target: "bifrost_admin::rules",
                                    dirs = ?dirs_for_task,
                                    timeout_ms = GROUP_CACHE_RESOLUTION_TIMEOUT.as_millis(),
                                    "group cache resolution timed out; clearing retry gate"
                                );
                                false
                            }
                        };
                        if !all_resolved {
                            state_for_task.clear_group_cache_resolved();
                        }
                        state_for_task.refresh_badge_rules_cache();
                    });
                } else {
                    tracing::debug!(
                        target: "bifrost_admin::rules",
                        dirs = ?uncached_dirs,
                        "active summary skipped group id resolution without active sync session"
                    );
                }
            }
        }
    }

    for (dir_name, dir_path) in &group_dirs {
        let group_storage = match RulesStorage::with_dir(dir_path.clone()) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(
                    target: "bifrost_admin::rules",
                    error = %e,
                    dir = %dir_name,
                    "Failed to open group rules storage for active summary"
                );
                continue;
            }
        };

        let group_id = reverse_cache
            .get(dir_name)
            .map(String::as_str)
            .unwrap_or(dir_name.as_str());
        let group_rules = collect_enabled_from_storage(
            &group_storage,
            Some(group_id),
            Some(dir_name),
            &mut var_map,
            &mut content_parts,
        );
        all_rules.extend(group_rules);
    }

    let variable_conflicts = build_variable_conflicts(var_map);

    let merged_content = content_parts.join("\n");

    let resp = ActiveSummaryResponse {
        total: all_rules.len(),
        rules: all_rules,
        variable_conflicts,
        merged_content,
    };
    json_response(&resp)
}

fn truncate_preview(value: &str, max_len: usize) -> String {
    let single_line = value.replace('\n', "\\n");
    if single_line.len() <= max_len {
        single_line
    } else {
        let mut truncated: String = single_line.chars().take(max_len).collect();
        truncated.push_str("...");
        truncated
    }
}

fn build_variable_conflicts(
    var_map: HashMap<String, Vec<InlineVarEntry>>,
) -> Vec<VariableConflict> {
    let mut conflicts = Vec::new();
    for (var_name, entries) in &var_map {
        if entries.len() < 2 {
            continue;
        }
        let first_value = &entries[0].value;
        let has_conflict = entries.iter().skip(1).any(|e| &e.value != first_value);
        if !has_conflict {
            continue;
        }
        let definitions = entries
            .iter()
            .map(|e| VariableDefinition {
                rule_name: e.rule_name.clone(),
                group_id: e.group_id.clone(),
                value_preview: truncate_preview(&e.value, 80),
            })
            .collect();
        conflicts.push(VariableConflict {
            variable_name: var_name.clone(),
            definitions,
        });
    }
    conflicts.sort_by(|a, b| a.variable_name.cmp(&b.variable_name));
    conflicts
}

fn append_missing_script_reference_warnings(
    result: &mut bifrost_core::ValidationResult,
    req_scripts: &HashSet<String>,
    res_scripts: &HashSet<String>,
    decode_scripts: &HashSet<String>,
) {
    for script_ref in &result.script_references {
        let scripts = match script_ref.script_type.as_str() {
            "request" => req_scripts,
            "response" => res_scripts,
            "decode" => decode_scripts,
            _ => continue,
        };

        if scripts.contains(&script_ref.name) {
            continue;
        }

        let mut available = scripts.iter().cloned().collect::<Vec<_>>();
        available.sort();

        let warning = ParseError::with_range(
            script_ref.line,
            1,
            script_ref.name.len() + 15,
            format!(
                "Script '{}' not found. Available {} scripts: {}",
                script_ref.name,
                script_ref.script_type,
                if available.is_empty() {
                    "(none)".to_string()
                } else {
                    available.join(", ")
                }
            ),
        )
        .with_severity(ParseErrorSeverity::Warning)
        .with_code("W003")
        .with_suggestion(format!(
            "Create a {} script named '{}' in the scripts directory, or use an existing script.",
            script_ref.script_type, script_ref.name
        ));
        result.warnings.push(warning);
    }
}

async fn validate_rule(req: Request<Incoming>, state: SharedAdminState) -> Response<BoxBody> {
    let body = match req.collect().await {
        Ok(collected) => collected.to_bytes(),
        Err(e) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                &format!("Failed to read body: {}", e),
            )
        }
    };

    let request: ValidateRuleRequest = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => return error_response(StatusCode::BAD_REQUEST, &format!("Invalid JSON: {}", e)),
    };

    let current_rule_name = request
        .current_rule_name
        .as_deref()
        .filter(|name| !name.trim().is_empty())
        .unwrap_or("__current__");
    let current_group_name = request
        .current_group_name
        .as_deref()
        .filter(|name| !name.trim().is_empty());
    let source_name = current_group_name
        .map(|group| format!("{}/{}", group.trim(), current_rule_name))
        .unwrap_or_else(|| current_rule_name.to_string());
    let content = request.content.clone();
    let global_values = request.global_values.clone();
    let rule_files = state
        .rules_storage
        .load_all_with_subdirs()
        .unwrap_or_default();
    let mut reference_catalog = bifrost_storage::build_rule_reference_catalog(&rule_files);
    reference_catalog.insert(source_name.clone(), content.clone());

    let content = match expand_rule_references(&source_name, &content, &reference_catalog) {
        Ok(expanded) => expanded,
        Err(error) => {
            let reference_error = ParseError::with_range(
                error.line(),
                1,
                request
                    .content
                    .lines()
                    .nth(error.line().saturating_sub(1))
                    .map(|line| line.len().max(1))
                    .unwrap_or(1),
                error.to_string(),
            )
            .with_code("E020")
            .with_suggestion(
                "Create the referenced rule or remove the cycle before enabling this rule.",
            );
            let resp = ValidateRuleResponse {
                valid: false,
                rule_count: 0,
                errors: vec![reference_error],
                warnings: Vec::new(),
                defined_variables: Vec::new(),
                script_references: Vec::new(),
            };
            return json_response(&resp);
        }
    };

    let validation_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        validate_rules_with_context(&content, &global_values)
    }));

    let mut result = match validation_result {
        Ok(r) => r,
        Err(e) => {
            let panic_msg = if let Some(s) = e.downcast_ref::<&str>() {
                s.to_string()
            } else if let Some(s) = e.downcast_ref::<String>() {
                s.clone()
            } else {
                "Unknown panic during validation".to_string()
            };

            tracing::error!(
                target: "bifrost_admin::rules",
                error = %panic_msg,
                "Validation panic caught - returning safe error response"
            );

            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("Validation failed unexpectedly: {}", panic_msg),
            );
        }
    };

    if let Some(ref script_manager) = state.script_manager {
        let manager = script_manager.read().await;
        let engine = manager.engine();

        let req_scripts: std::collections::HashSet<String> = engine
            .list_scripts(bifrost_script::ScriptType::Request)
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|s| s.name)
            .collect();

        let res_scripts: std::collections::HashSet<String> = engine
            .list_scripts(bifrost_script::ScriptType::Response)
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|s| s.name)
            .collect();

        let mut decode_scripts: std::collections::HashSet<String> = BUILTIN_DECODE_SCRIPTS
            .iter()
            .map(|name| (*name).to_string())
            .collect();
        decode_scripts.extend(
            engine
                .list_scripts(bifrost_script::ScriptType::Decode)
                .await
                .unwrap_or_default()
                .into_iter()
                .map(|s| s.name),
        );

        append_missing_script_reference_warnings(
            &mut result,
            &req_scripts,
            &res_scripts,
            &decode_scripts,
        );
    }

    let response = ValidateRuleResponse {
        valid: result.valid,
        rule_count: result.rule_count,
        errors: result.errors,
        warnings: result.warnings,
        defined_variables: result.defined_variables,
        script_references: result.script_references,
    };

    json_response(&response)
}

async fn create_rule(
    req: Request<Incoming>,
    state: SharedAdminState,
    push_manager: Option<SharedPushManager>,
) -> Response<BoxBody> {
    let body = match req.collect().await {
        Ok(collected) => collected.to_bytes(),
        Err(e) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                &format!("Failed to read body: {}", e),
            )
        }
    };

    let request: CreateRuleRequest = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => return error_response(StatusCode::BAD_REQUEST, &format!("Invalid JSON: {}", e)),
    };

    if request.name.is_empty() {
        return error_response(StatusCode::BAD_REQUEST, "Rule name is required");
    }

    if state.rules_storage.exists(&request.name) {
        return error_response(StatusCode::CONFLICT, "Rule with this name already exists");
    }

    let highest_priority_sort_order = state
        .rules_storage
        .list_summaries()
        .ok()
        .and_then(|rules| rules.into_iter().map(|rule| rule.sort_order).min())
        .map(|value| value - 1)
        .unwrap_or(0);

    let rule = RuleFile::new(&request.name, &request.content)
        .with_enabled(request.enabled.unwrap_or(true))
        .with_sort_order(highest_priority_sort_order);

    match state.rules_storage.save(&rule) {
        Ok(_) => {
            if let Some(sync_manager) = state.sync_manager.clone() {
                if let Err(error) = sync_manager.clear_deleted_rule(&request.name).await {
                    return error_response(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        &format!("Failed to clear deleted sync intent for rule: {}", error),
                    );
                }
            }
            notify_rules_changed(&state);
            invalidate_overview_cache(&push_manager);
            success_response(&format!("Rule '{}' created successfully", request.name))
        }
        Err(e) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("Failed to create rule: {}", e),
        ),
    }
}

async fn get_rule(state: SharedAdminState, name: &str) -> Response<BoxBody> {
    match state.rules_storage.load(name) {
        Ok(rule) => {
            let detail = RuleFileDetail {
                name: rule.name,
                content: rule.content,
                enabled: rule.enabled,
                sort_order: rule.sort_order,
                created_at: rule.created_at,
                updated_at: rule.updated_at,
                sync: RuleSyncInfo {
                    status: sync_status_label(rule.sync.status).to_string(),
                    last_synced_at: rule.sync.last_synced_at,
                    remote_id: rule.sync.remote_id,
                    remote_updated_at: rule.sync.remote_updated_at,
                },
            };
            json_response(&detail)
        }
        Err(_) => error_response(StatusCode::NOT_FOUND, &format!("Rule '{}' not found", name)),
    }
}

async fn update_rule(
    req: Request<Incoming>,
    state: SharedAdminState,
    name: &str,
    push_manager: Option<SharedPushManager>,
) -> Response<BoxBody> {
    let existing = match state.rules_storage.load(name) {
        Ok(r) => r,
        Err(_) => {
            return error_response(StatusCode::NOT_FOUND, &format!("Rule '{}' not found", name))
        }
    };

    let body = match req.collect().await {
        Ok(collected) => collected.to_bytes(),
        Err(e) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                &format!("Failed to read body: {}", e),
            )
        }
    };

    let request: UpdateRuleRequest = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => return error_response(StatusCode::BAD_REQUEST, &format!("Invalid JSON: {}", e)),
    };
    let content_changed = request.content.is_some();
    let enabled_changed = request.enabled.is_some();

    let rule = RuleFile {
        name: existing.name,
        content: request.content.unwrap_or(existing.content),
        enabled: request.enabled.unwrap_or(existing.enabled),
        sort_order: existing.sort_order,
        description: existing.description,
        group: existing.group,
        version: existing.version,
        created_at: existing.created_at,
        updated_at: chrono::Utc::now().to_rfc3339(),
        sync: existing.sync,
    };
    let mut rule = rule;
    if content_changed || enabled_changed {
        rule.touch_local_change();
    }

    match state.rules_storage.save(&rule) {
        Ok(_) => {
            notify_rules_changed(&state);
            invalidate_overview_cache(&push_manager);
            success_response(&format!("Rule '{}' updated successfully", name))
        }
        Err(e) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("Failed to update rule: {}", e),
        ),
    }
}

async fn reorder_rules(
    req: Request<Incoming>,
    state: SharedAdminState,
    push_manager: Option<SharedPushManager>,
) -> Response<BoxBody> {
    let body = match req.collect().await {
        Ok(collected) => collected.to_bytes(),
        Err(e) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                &format!("Failed to read body: {}", e),
            )
        }
    };

    let request: ReorderRulesRequest = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => return error_response(StatusCode::BAD_REQUEST, &format!("Invalid JSON: {}", e)),
    };

    match state.rules_storage.reorder(&request.order) {
        Ok(_) => {
            notify_rules_changed(&state);
            invalidate_overview_cache(&push_manager);
            success_response("Rules reordered successfully")
        }
        Err(e) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("Failed to reorder rules: {}", e),
        ),
    }
}

async fn delete_rule(
    state: SharedAdminState,
    name: &str,
    push_manager: Option<SharedPushManager>,
) -> Response<BoxBody> {
    let rule = match state.rules_storage.load(name) {
        Ok(rule) => rule,
        Err(_) => {
            return error_response(StatusCode::NOT_FOUND, &format!("Rule '{}' not found", name))
        }
    };

    if rule.sync.remote_id.is_some() {
        if let Some(sync_manager) = state.sync_manager.clone() {
            if let Err(error) = sync_manager.record_deleted_rule(&rule).await {
                tracing::warn!(
                    target: "bifrost_admin::rules",
                    rule = %name,
                    error = %error,
                    "failed to record synced rule deletion tombstone, proceeding with local delete"
                );
            }
        } else {
            tracing::warn!(
                target: "bifrost_admin::rules",
                rule = %name,
                "sync manager not available, skipping remote deletion record for synced rule"
            );
        }
    }

    match state.rules_storage.delete(name) {
        Ok(_) => {
            if let Some(sync_manager) = state.sync_manager.clone() {
                sync_manager.trigger_sync();
            }
            notify_rules_changed(&state);
            invalidate_overview_cache(&push_manager);
            success_response(&format!("Rule '{}' deleted successfully", name))
        }
        Err(e) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("Failed to delete rule: {}", e),
        ),
    }
}

async fn enable_rule(
    state: SharedAdminState,
    name: &str,
    enabled: bool,
    push_manager: Option<SharedPushManager>,
) -> Response<BoxBody> {
    match state.rules_storage.set_enabled(name, enabled) {
        Ok(_) => {
            notify_rules_changed(&state);
            invalidate_overview_cache(&push_manager);
            let action = if enabled { "enabled" } else { "disabled" };
            success_response(&format!("Rule '{}' {} successfully", name, action))
        }
        Err(e) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("Failed to update rule: {}", e),
        ),
    }
}

async fn rename_rule(
    req: Request<Incoming>,
    state: SharedAdminState,
    name: &str,
    push_manager: Option<SharedPushManager>,
) -> Response<BoxBody> {
    if !state.rules_storage.exists(name) {
        return error_response(StatusCode::NOT_FOUND, &format!("Rule '{}' not found", name));
    }

    let body = match req.collect().await {
        Ok(collected) => collected.to_bytes(),
        Err(e) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                &format!("Failed to read body: {}", e),
            )
        }
    };

    let request: RenameRuleRequest = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => return error_response(StatusCode::BAD_REQUEST, &format!("Invalid JSON: {}", e)),
    };

    if request.new_name.is_empty() {
        return error_response(StatusCode::BAD_REQUEST, "New rule name is required");
    }

    if request.new_name == name {
        return error_response(
            StatusCode::BAD_REQUEST,
            "New name is the same as the old name",
        );
    }

    match state.rules_storage.rename(name, &request.new_name) {
        Ok(_) => {
            notify_rules_changed(&state);
            invalidate_overview_cache(&push_manager);
            success_response(&format!(
                "Rule '{}' renamed to '{}' successfully",
                name, request.new_name
            ))
        }
        Err(e) => {
            let status = if e.to_string().contains("already exists") {
                StatusCode::CONFLICT
            } else if e.to_string().contains("not found") {
                StatusCode::NOT_FOUND
            } else {
                StatusCode::INTERNAL_SERVER_ERROR
            };
            error_response(status, &format!("Failed to rename rule: {}", e))
        }
    }
}

fn invalidate_overview_cache(push_manager: &Option<SharedPushManager>) {
    if let Some(pm) = push_manager {
        pm.invalidate_overview_cache();
    }
}

pub(crate) fn notify_rules_changed_pub(state: &SharedAdminState) {
    notify_rules_changed(state);
}

fn notify_rules_changed(state: &SharedAdminState) {
    state.refresh_badge_rules_cache();
    if let Some(ref config_manager) = state.config_manager {
        match config_manager.notify(ConfigChangeEvent::RulesChanged) {
            Ok(count) => {
                tracing::info!(
                    target: "bifrost_admin::rules",
                    receivers = count,
                    "notified rules changed event"
                );
            }
            Err(e) => {
                tracing::warn!(
                    target: "bifrost_admin::rules",
                    error = %e,
                    "failed to notify rules changed event (no receivers)"
                );
            }
        }
    } else {
        tracing::warn!(
            target: "bifrost_admin::rules",
            "config_manager is not available, cannot notify rules changed"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_variable_conflicts_same_name_different_values() {
        let mut var_map: HashMap<String, Vec<InlineVarEntry>> = HashMap::new();
        var_map
            .entry("data".to_string())
            .or_default()
            .push(InlineVarEntry {
                rule_name: "rule-a".to_string(),
                group_id: None,
                value: "x-tt-env: boe_xxx".to_string(),
            });
        var_map
            .entry("data".to_string())
            .or_default()
            .push(InlineVarEntry {
                rule_name: "rule-b".to_string(),
                group_id: None,
                value: "x-debug: true".to_string(),
            });

        let conflicts = build_variable_conflicts(var_map);
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].variable_name, "data");
        assert_eq!(conflicts[0].definitions.len(), 2);
        assert_eq!(conflicts[0].definitions[0].rule_name, "rule-a");
        assert_eq!(conflicts[0].definitions[1].rule_name, "rule-b");
    }

    #[test]
    fn test_no_conflicts_when_values_match() {
        let mut var_map: HashMap<String, Vec<InlineVarEntry>> = HashMap::new();
        var_map
            .entry("data".to_string())
            .or_default()
            .push(InlineVarEntry {
                rule_name: "rule-a".to_string(),
                group_id: None,
                value: "same content".to_string(),
            });
        var_map
            .entry("data".to_string())
            .or_default()
            .push(InlineVarEntry {
                rule_name: "rule-b".to_string(),
                group_id: None,
                value: "same content".to_string(),
            });

        let conflicts = build_variable_conflicts(var_map);
        assert!(conflicts.is_empty());
    }

    #[test]
    fn test_no_conflicts_single_rule_file() {
        let mut var_map: HashMap<String, Vec<InlineVarEntry>> = HashMap::new();
        var_map
            .entry("data".to_string())
            .or_default()
            .push(InlineVarEntry {
                rule_name: "rule-a".to_string(),
                group_id: None,
                value: "some value".to_string(),
            });

        let conflicts = build_variable_conflicts(var_map);
        assert!(conflicts.is_empty());
    }

    #[test]
    fn test_truncate_preview_short() {
        assert_eq!(truncate_preview("short", 80), "short");
    }

    #[test]
    fn test_truncate_preview_long() {
        let long = "a".repeat(100);
        let result = truncate_preview(&long, 80);
        assert!(result.len() <= 83 + 3);
        assert!(result.ends_with("..."));
    }

    #[test]
    fn test_truncate_preview_newlines() {
        let result = truncate_preview("line1\nline2\nline3", 80);
        assert_eq!(result, r"line1\nline2\nline3");
    }

    #[test]
    fn test_conflicts_with_group_rules() {
        let mut var_map: HashMap<String, Vec<InlineVarEntry>> = HashMap::new();
        var_map
            .entry("headers".to_string())
            .or_default()
            .push(InlineVarEntry {
                rule_name: "my-rule".to_string(),
                group_id: None,
                value: "x-env: prod".to_string(),
            });
        var_map
            .entry("headers".to_string())
            .or_default()
            .push(InlineVarEntry {
                rule_name: "team-rule".to_string(),
                group_id: Some("group-123".to_string()),
                value: "x-env: staging".to_string(),
            });

        let conflicts = build_variable_conflicts(var_map);
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].definitions[0].group_id, None);
        assert_eq!(
            conflicts[0].definitions[1].group_id,
            Some("group-123".to_string())
        );
    }

    #[test]
    fn test_builtin_decode_script_references_do_not_warn() {
        let mut result = bifrost_core::ValidationResult {
            script_references: BUILTIN_DECODE_SCRIPTS
                .iter()
                .map(|name| ScriptReference {
                    name: (*name).to_string(),
                    script_type: "decode".to_string(),
                    line: 1,
                })
                .collect(),
            ..Default::default()
        };
        let req_scripts = HashSet::new();
        let res_scripts = HashSet::new();
        let decode_scripts = BUILTIN_DECODE_SCRIPTS
            .iter()
            .map(|name| (*name).to_string())
            .collect();

        append_missing_script_reference_warnings(
            &mut result,
            &req_scripts,
            &res_scripts,
            &decode_scripts,
        );

        assert!(
            result.warnings.is_empty(),
            "built-in decode scripts such as decode://bp must not be reported as missing"
        );
    }

    #[test]
    fn test_missing_decode_script_warning_uses_decode_inventory() {
        let mut result = bifrost_core::ValidationResult {
            script_references: vec![ScriptReference {
                name: "missing".to_string(),
                script_type: "decode".to_string(),
                line: 3,
            }],
            ..Default::default()
        };
        let req_scripts = HashSet::new();
        let res_scripts = HashSet::from(["response_only".to_string()]);
        let decode_scripts = HashSet::from(["bbb".to_string(), "default".to_string()]);

        append_missing_script_reference_warnings(
            &mut result,
            &req_scripts,
            &res_scripts,
            &decode_scripts,
        );

        assert_eq!(result.warnings.len(), 1);
        let warning = &result.warnings[0];
        assert_eq!(warning.code.as_deref(), Some("W003"));
        assert!(warning
            .message
            .contains("Available decode scripts: bbb, default"));
        assert!(!warning.message.contains("response_only"));
    }
}
