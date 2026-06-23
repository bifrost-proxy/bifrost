use bifrost_core::{
    extract_inline_variables,
    rule_share::{
        append_rule_share_query, new_rule_share_payload, share_payload_name_from_rule,
        RuleShareExclusiveScope, RULE_SHARE_PROTOCOL_VERSION, RULE_SHARE_QUERY_PARAM,
    },
    validate_rule_syntax_report, ParseError, ParseErrorSeverity, RuleSyntaxReport, ValueStore,
};
use bifrost_storage::{ConfigChangeEvent, RuleFile, RuleSummary, RulesChangeOrigin, RulesStorage};
use http_body_util::BodyExt;
use hyper::{body::Incoming, Method, Request, Response, StatusCode};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::time::Duration;

use super::{
    error_response, full_body, json_response, json_response_with_status, method_not_allowed,
    success_response, BoxBody,
};
use crate::push::SharedPushManager;
use crate::rule_share_import::exit_rule_share_env;
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
    #[serde(default)]
    allow_invalid: bool,
}

#[derive(Debug, Deserialize)]
struct UpdateRuleRequest {
    content: Option<String>,
    enabled: Option<bool>,
    #[serde(default)]
    allow_invalid: bool,
}

#[derive(Debug, Deserialize)]
struct ReorderRulesRequest {
    order: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct RenameRuleRequest {
    new_name: String,
}

#[derive(Debug, Deserialize)]
struct CreateRuleShareLinkRequest {
    name: String,
    target_url: String,
    #[serde(default)]
    exclusive_scope: Option<RuleShareExclusiveScope>,
}

#[derive(Debug, Serialize)]
struct CreateRuleShareLinkResponse {
    url: String,
    query_param: &'static str,
    payload_version: u8,
    rule_name: String,
    content_hash: String,
}

#[derive(Debug, Serialize)]
struct ShareEnvStatusResponse {
    active: bool,
    imported_rule_name: Option<String>,
    requested_name: Option<String>,
    content_hash: Option<String>,
    enabled_rule_names: Vec<String>,
    entered_at: Option<String>,
}

fn html_escape(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            _ => escaped.push(ch),
        }
    }
    escaped
}

fn html_response(body: String) -> Response<BoxBody> {
    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "text/html; charset=utf-8")
        .header("Cache-Control", "no-store")
        .header("Referrer-Policy", "no-referrer")
        .header("X-Frame-Options", "DENY")
        .body(full_body(body))
        .unwrap()
}

pub fn share_env_exit_page(state: SharedAdminState) -> Response<BoxBody> {
    let share_state = match state.rules_storage.load_share_env_state() {
        Ok(Some(mut share_state)) if share_state.active => {
            if share_state.exit_token.trim().is_empty() {
                share_state.exit_token = uuid::Uuid::new_v4().to_string();
                if let Err(error) = state.rules_storage.save_share_env_state(&share_state) {
                    return error_response(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        &format!("Failed to save share env exit token: {}", error),
                    );
                }
            }
            Some(share_state)
        }
        Ok(_) => None,
        Err(error) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("Failed to load share env state: {}", error),
            );
        }
    };

    let (status_class, title, description, button, script) = if let Some(share_state) = share_state
    {
        let token_json =
            serde_json::to_string(&share_state.exit_token).unwrap_or_else(|_| "\"\"".to_string());
        let requested_name = html_escape(&share_state.requested_name);
        let imported_rule_name = html_escape(&share_state.imported_rule_name);
        let token_attr = html_escape(&share_state.exit_token);
        (
            "active",
            "Share preview active".to_string(),
            format!(
                "Exit <strong>{}</strong> and restore the rules that were enabled before <strong>{}</strong> was imported.",
                imported_rule_name, requested_name
            ),
            format!(
                r#"<form id="exit-form" method="post" action="/_bifrost/api/rules/share-env/exit"><input type="hidden" name="token" value="{token_attr}"><button id="confirm-exit" type="submit">Exit share preview</button></form>"#
            ),
            format!(
                r#"<script>
(function(){{
  var token={token_json};
  var button=document.getElementById('confirm-exit');
  var status=document.getElementById('status');
  var form=document.getElementById('exit-form');
  function setStatus(text, className){{status.textContent=text;status.className=className||'';}}
  form.addEventListener('submit', function(ev){{
    ev.preventDefault();
    if(ev.isTrusted===false)return;
    button.disabled=true;
    button.textContent='Exiting';
    setStatus('Restoring rules...', '');
    if(typeof fetch!=='function'){{form.submit();return;}}
    fetch('/_bifrost/api/rules/share-env/exit', {{
      method:'POST',
      credentials:'same-origin',
      headers:{{'Content-Type':'application/json'}},
      body:JSON.stringify({{token:token}})
    }}).then(function(resp){{
      if(!resp.ok)throw new Error('Exit failed');
      return resp.json();
    }}).then(function(data){{
      if(!data||data.was_active!==true)throw new Error('Share preview is not active');
      setStatus('Share preview exited. Returning to your page...', 'ok');
      try{{if(window.opener&&!window.opener.closed)window.opener.location.reload();}}catch(e){{}}
      setTimeout(function(){{window.close();}},700);
    }}).catch(function(){{
      button.disabled=false;
      button.textContent='Exit share preview';
      setStatus('Exit failed. Refresh and try again.', 'err');
    }});
  }});
}})();
</script>"#
            ),
        )
    } else {
        (
            "inactive",
            "No active Share preview".to_string(),
            "There is no active Share preview to exit.".to_string(),
            r#"<button type="button" disabled>Exit share preview</button>"#.to_string(),
            String::new(),
        )
    };

    html_response(format!(
        r#"<!doctype html>
<html>
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>{title}</title>
  <style>
    :root{{color-scheme:light dark;font-family:-apple-system,BlinkMacSystemFont,'Segoe UI',Roboto,sans-serif}}
    body{{margin:0;min-height:100vh;display:grid;place-items:center;background:#f7f9fa;color:#1f2a2e}}
    main{{width:min(360px,calc(100vw - 32px));border:1px solid rgba(35,140,156,.18);border-radius:12px;background:#fff;box-shadow:0 18px 48px rgba(31,42,46,.14);padding:24px}}
    .mark{{width:10px;height:10px;border-radius:999px;background:#ff4d4f;box-shadow:0 0 0 5px rgba(255,77,79,.12);margin-bottom:14px}}
    .mark.inactive{{background:#9aa3a7;box-shadow:0 0 0 5px rgba(154,163,167,.12)}}
    h1{{font-size:20px;line-height:1.2;margin:0 0 10px;font-weight:700}}
    p{{font-size:14px;line-height:1.55;margin:0 0 18px;color:#506167}}
    button{{height:34px;border:1px solid rgba(35,140,156,.36);border-radius:6px;background:#238c9c;color:#fff;font-size:14px;font-weight:700;padding:0 14px;cursor:pointer}}
    button:disabled{{cursor:not-allowed;opacity:.55}}
    #status{{min-height:20px;margin-top:12px;font-size:13px}}
    #status.ok{{color:#168a42}}
    #status.err{{color:#c93535}}
    @media(prefers-color-scheme:dark){{
      body{{background:#15191b;color:#e9f1f2}}
      main{{background:#202628;border-color:rgba(123,235,192,.22);box-shadow:0 18px 48px rgba(0,0,0,.36)}}
      p{{color:#b9c7ca}}
      button{{background:#2f9aae;border-color:rgba(123,235,192,.36)}}
    }}
  </style>
</head>
<body>
  <main>
    <div class="mark {status_class}"></div>
    <h1>{title}</h1>
    <p>{description}</p>
    {button}
    <p id="status" aria-live="polite"></p>
  </main>
  {script}
</body>
</html>"#
    ))
}

async fn share_exit_token_from_request(
    req: Request<Incoming>,
) -> Result<String, Response<BoxBody>> {
    let (parts, body) = req.into_parts();

    let bytes = body.collect().await.map_err(|error| {
        error_response(
            StatusCode::BAD_REQUEST,
            &format!("Failed to read share env exit request: {}", error),
        )
    })?;
    let bytes = bytes.to_bytes();
    Ok(share_exit_token_from_headers_and_body(
        &parts.headers,
        bytes.as_ref(),
    ))
}

fn share_exit_token_from_headers_and_body(headers: &hyper::HeaderMap, bytes: &[u8]) -> String {
    if let Some(token) = headers
        .get("X-Bifrost-Share-Exit-Token")
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return token.to_string();
    }

    if bytes.is_empty() {
        return String::new();
    }

    if let Ok(value) = serde_json::from_slice::<serde_json::Value>(bytes) {
        return value
            .get("token")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .unwrap_or_default()
            .to_string();
    }

    let body = String::from_utf8_lossy(bytes);
    serde_urlencoded::from_str::<HashMap<String, String>>(&body)
        .ok()
        .and_then(|mut form| form.remove("token"))
        .map(|token| token.trim().to_string())
        .unwrap_or_default()
}

#[derive(Debug, Serialize, Deserialize)]
struct ActiveRuleItem {
    name: String,
    rule_count: usize,
    group_id: Option<String>,
    group_name: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct ActiveSummaryResponse {
    total: usize,
    rules: Vec<ActiveRuleItem>,
    variable_conflicts: Vec<VariableConflict>,
    merged_content: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct VariableConflict {
    variable_name: String,
    definitions: Vec<VariableDefinition>,
}

#[derive(Debug, Serialize, Deserialize)]
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

type ValidateRuleResponse = RuleSyntaxReport;

#[derive(Debug, Serialize)]
struct RuleMutationResponse {
    success: bool,
    message: String,
    saved: bool,
    syntax: RuleSyntaxReport,
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
    } else if path == "/api/rules/share-link" {
        match method {
            Method::POST => create_rule_share_link(req, state).await,
            _ => method_not_allowed(),
        }
    } else if path == "/api/rules/share-env/status" {
        match method {
            Method::GET => share_env_status(state).await,
            _ => method_not_allowed(),
        }
    } else if path == "/api/rules/share-env/exit" {
        match method {
            Method::POST => exit_share_env(req, state, push_manager).await,
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

async fn share_env_status(state: SharedAdminState) -> Response<BoxBody> {
    match state.rules_storage.load_share_env_state() {
        Ok(Some(share_state)) if share_state.active => json_response(&ShareEnvStatusResponse {
            active: true,
            imported_rule_name: Some(share_state.imported_rule_name),
            requested_name: Some(share_state.requested_name),
            content_hash: Some(share_state.content_hash),
            enabled_rule_names: share_state.enabled_rule_names,
            entered_at: Some(share_state.entered_at),
        }),
        Ok(_) => json_response(&ShareEnvStatusResponse {
            active: false,
            imported_rule_name: None,
            requested_name: None,
            content_hash: None,
            enabled_rule_names: Vec::new(),
            entered_at: None,
        }),
        Err(error) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("Failed to load share env state: {}", error),
        ),
    }
}

async fn exit_share_env(
    req: Request<Incoming>,
    state: SharedAdminState,
    push_manager: Option<SharedPushManager>,
) -> Response<BoxBody> {
    let provided_token = match share_exit_token_from_request(req).await {
        Ok(token) => token,
        Err(response) => return response,
    };
    match state.rules_storage.load_share_env_state() {
        Ok(Some(mut share_state)) if share_state.active => {
            if share_state.exit_token.trim().is_empty() {
                share_state.exit_token = uuid::Uuid::new_v4().to_string();
                if let Err(error) = state.rules_storage.save_share_env_state(&share_state) {
                    return error_response(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        &format!("Failed to refresh share env exit token: {}", error),
                    );
                }
                return error_response(
                    StatusCode::CONFLICT,
                    "Share env exit token was refreshed; open the exit confirmation page again",
                );
            }
            if provided_token.is_empty() || provided_token != share_state.exit_token {
                return error_response(StatusCode::FORBIDDEN, "Invalid share env exit token");
            }
        }
        Ok(_) => {}
        Err(error) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("Failed to load share env state: {}", error),
            );
        }
    }

    match exit_rule_share_env(state, push_manager.as_ref()).await {
        Ok(outcome) => json_response(&outcome),
        Err(error) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("Failed to exit share env: {}", error),
        ),
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
    result: &mut RuleSyntaxReport,
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
    result.refresh_guidance();
}

pub(crate) async fn build_rule_syntax_report(
    state: &SharedAdminState,
    source_name: &str,
    content: &str,
    global_values: &HashMap<String, String>,
) -> Result<RuleSyntaxReport, String> {
    let rule_files = state
        .rules_storage
        .load_all_with_subdirs()
        .unwrap_or_default();
    let mut reference_catalog = bifrost_storage::build_rule_reference_catalog(&rule_files);
    reference_catalog.insert(source_name.to_string(), content.to_string());

    let validation_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        validate_rule_syntax_report(source_name, content, &reference_catalog, global_values)
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

            return Err(format!("Validation failed unexpectedly: {}", panic_msg));
        }
    };

    if let Some(ref script_manager) = state.script_manager {
        let manager = script_manager.read().await;
        let engine = manager.engine();

        let req_scripts: HashSet<String> = engine
            .list_scripts(bifrost_script::ScriptType::Request)
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|s| s.name)
            .collect();

        let res_scripts: HashSet<String> = engine
            .list_scripts(bifrost_script::ScriptType::Response)
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|s| s.name)
            .collect();

        let mut decode_scripts: HashSet<String> = BUILTIN_DECODE_SCRIPTS
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

    Ok(result)
}

pub(crate) fn global_values_from_state(state: &SharedAdminState) -> HashMap<String, String> {
    state
        .values_storage
        .as_ref()
        .map(|storage| storage.read().as_hashmap())
        .unwrap_or_default()
}

fn syntax_rejection_response(rule_name: &str, syntax: RuleSyntaxReport) -> Response<BoxBody> {
    let message = format!(
        "Rule '{}' was not saved because syntax validation failed",
        rule_name
    );
    let response = RuleMutationResponse {
        success: false,
        message,
        saved: false,
        syntax,
    };
    json_response_with_status(StatusCode::UNPROCESSABLE_ENTITY, &response)
}

fn syntax_saved_response(
    rule_name: &str,
    action: &str,
    syntax: RuleSyntaxReport,
) -> Response<BoxBody> {
    let response = RuleMutationResponse {
        success: true,
        message: format!("Rule '{}' {} successfully", rule_name, action),
        saved: true,
        syntax,
    };
    json_response(&response)
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
    let response: ValidateRuleResponse = match build_rule_syntax_report(
        &state,
        &source_name,
        &request.content,
        &request.global_values,
    )
    .await
    {
        Ok(report) => report,
        Err(error) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, &error),
    };

    json_response(&response)
}

async fn create_rule_share_link(
    req: Request<Incoming>,
    state: SharedAdminState,
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

    let request: CreateRuleShareLinkRequest = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => return error_response(StatusCode::BAD_REQUEST, &format!("Invalid JSON: {}", e)),
    };
    if request.name.trim().is_empty() {
        return error_response(StatusCode::BAD_REQUEST, "Rule name is required");
    }
    if request.target_url.trim().is_empty() {
        return error_response(StatusCode::BAD_REQUEST, "Target URL is required");
    }
    let _exclusive_scope = request.exclusive_scope.unwrap_or_default();

    let rule = match state.rules_storage.load(&request.name) {
        Ok(rule) => rule,
        Err(bifrost_core::BifrostError::NotFound(_)) => {
            return error_response(StatusCode::NOT_FOUND, "Rule not found")
        }
        Err(error) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("Failed to load rule: {}", error),
            )
        }
    };

    let payload_name = share_payload_name_from_rule(&rule.name, rule.description.as_deref());
    let payload = match new_rule_share_payload(&payload_name, &rule.content) {
        Ok(payload) => payload,
        Err(error) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                &format!("Failed to create rule share payload: {}", error),
            )
        }
    };
    let url = match append_rule_share_query(&request.target_url, &payload) {
        Ok(url) => url,
        Err(error) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                &format!("Failed to create rule share URL: {}", error),
            )
        }
    };

    json_response(&CreateRuleShareLinkResponse {
        url,
        query_param: RULE_SHARE_QUERY_PARAM,
        payload_version: RULE_SHARE_PROTOCOL_VERSION,
        rule_name: payload.name,
        content_hash: payload.content_hash,
    })
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

    let syntax = match build_rule_syntax_report(
        &state,
        &request.name,
        &request.content,
        &global_values_from_state(&state),
    )
    .await
    {
        Ok(report) => report,
        Err(error) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, &error),
    };
    if !syntax.valid && !request.allow_invalid {
        return syntax_rejection_response(&request.name, syntax);
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
            syntax_saved_response(&request.name, "created", syntax)
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
    let next_content = request
        .content
        .clone()
        .unwrap_or_else(|| existing.content.clone());
    let should_validate = content_changed || (!existing.enabled && request.enabled == Some(true));
    let syntax = match build_rule_syntax_report(
        &state,
        name,
        &next_content,
        &global_values_from_state(&state),
    )
    .await
    {
        Ok(report) => report,
        Err(error) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, &error),
    };
    if should_validate && !syntax.valid && !request.allow_invalid {
        return syntax_rejection_response(name, syntax);
    }

    let rule = RuleFile {
        name: existing.name,
        content: next_content,
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
            syntax_saved_response(name, "updated", syntax)
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
        match config_manager.notify(ConfigChangeEvent::rules_changed(
            RulesChangeOrigin::LocalApi,
        )) {
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
    use hyper::header::HeaderValue;

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

    #[tokio::test]
    async fn active_summary_includes_local_group_rules_without_sync_session() {
        let temp_dir = tempfile::tempdir().unwrap();
        let storage = RulesStorage::with_dir(temp_dir.path().join("rules")).unwrap();
        storage
            .save(
                &RuleFile::new("personal", "personal.example.com statusCode://201")
                    .with_enabled(false),
            )
            .unwrap();

        let group_storage = RulesStorage::with_dir(storage.base_dir().join("Tray Team")).unwrap();
        group_storage
            .save(&RuleFile::new(
                "group-rule",
                "group.example.com statusCode://203",
            ))
            .unwrap();

        let state =
            std::sync::Arc::new(crate::state::AdminState::new(0).with_rules_storage(storage));
        let response = active_summary(state).await;
        assert_eq!(response.status(), StatusCode::OK);

        let body = response.into_body().collect().await.unwrap().to_bytes();
        let summary: ActiveSummaryResponse = serde_json::from_slice(&body).unwrap();

        // active-summary is a read-only UI/tray summary. The proxy runtime keeps
        // its own no-session group protection in start::resolve_valid_group_dirs.
        assert_eq!(summary.total, 1);
        assert_eq!(summary.rules[0].name, "group-rule");
        assert_eq!(summary.rules[0].group_id.as_deref(), Some("Tray Team"));
        assert_eq!(summary.rules[0].group_name.as_deref(), Some("Tray Team"));
        assert_eq!(summary.merged_content, "group.example.com statusCode://203");
    }

    #[test]
    fn share_exit_token_prefers_header_over_body() {
        let mut headers = hyper::HeaderMap::new();
        headers.insert(
            "X-Bifrost-Share-Exit-Token",
            HeaderValue::from_static(" header-token "),
        );

        assert_eq!(
            share_exit_token_from_headers_and_body(&headers, br#"{"token":"body-token"}"#),
            "header-token"
        );
    }

    #[test]
    fn share_exit_token_reads_json_body() {
        let headers = hyper::HeaderMap::new();

        assert_eq!(
            share_exit_token_from_headers_and_body(&headers, br#"{"token":" json-token "}"#),
            "json-token"
        );
    }

    #[test]
    fn share_exit_token_reads_form_body() {
        let headers = hyper::HeaderMap::new();

        assert_eq!(
            share_exit_token_from_headers_and_body(&headers, b"token=form-token"),
            "form-token"
        );
    }

    #[tokio::test]
    async fn share_env_exit_page_regenerates_empty_token_and_sets_security_headers() {
        let temp_dir = tempfile::tempdir().unwrap();
        let storage = RulesStorage::with_dir(temp_dir.path().join("rules")).unwrap();
        storage
            .save_share_env_state(&bifrost_storage::ShareEnvState {
                active: true,
                imported_rule_name: "share/demo".to_string(),
                requested_name: "demo".to_string(),
                content_hash: "hash".to_string(),
                enabled_rule_names: vec!["before".to_string()],
                entered_at: "2026-06-22T00:00:00Z".to_string(),
                exit_token: String::new(),
            })
            .unwrap();
        let state =
            std::sync::Arc::new(crate::state::AdminState::new(0).with_rules_storage(storage));

        let response = share_env_exit_page(state.clone());
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get("Referrer-Policy"),
            Some(&HeaderValue::from_static("no-referrer"))
        );
        assert_eq!(
            response.headers().get("X-Frame-Options"),
            Some(&HeaderValue::from_static("DENY"))
        );

        let body = response.into_body().collect().await.unwrap().to_bytes();
        let body = String::from_utf8(body.to_vec()).unwrap();
        assert!(body.contains("Share preview active"));
        assert!(body.contains("method=\"post\" action=\"/_bifrost/api/rules/share-env/exit\""));
        assert!(!body.contains("share-env/exit?token="));

        let refreshed = state
            .rules_storage
            .load_share_env_state()
            .unwrap()
            .expect("share env state");
        assert!(!refreshed.exit_token.is_empty());
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
        let mut result = RuleSyntaxReport::from_validation_result(bifrost_core::ValidationResult {
            script_references: BUILTIN_DECODE_SCRIPTS
                .iter()
                .map(|name| bifrost_core::ScriptReference {
                    name: (*name).to_string(),
                    script_type: "decode".to_string(),
                    line: 1,
                })
                .collect(),
            ..Default::default()
        });
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
        let mut result = RuleSyntaxReport::from_validation_result(bifrost_core::ValidationResult {
            script_references: vec![bifrost_core::ScriptReference {
                name: "missing".to_string(),
                script_type: "decode".to_string(),
                line: 3,
            }],
            ..Default::default()
        });
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
