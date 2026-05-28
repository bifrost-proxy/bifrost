use bifrost_storage::RulesStorage;
use http_body_util::BodyExt;
use hyper::{body::Incoming, Method, Request, Response, StatusCode};
use serde::{Deserialize, Deserializer, Serialize};

use super::{error_response, json_response, method_not_allowed, success_response, BoxBody};
use crate::state::SharedAdminState;
use bifrost_storage::ConfigChangeEvent;

fn nullable_string<'de, D>(deserializer: D) -> std::result::Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(Option::<String>::deserialize(deserializer)?.unwrap_or_default())
}

#[derive(Debug, Deserialize)]
struct RemoteResponse<T> {
    #[allow(dead_code)]
    code: i32,
    #[allow(dead_code)]
    message: String,
    data: T,
}

#[derive(Debug, Deserialize)]
struct RemoteListPayload<T> {
    list: Option<Vec<T>>,
    #[allow(dead_code)]
    total: Option<u64>,
}

#[derive(Debug, Deserialize, Clone)]
struct RemotePeer {
    user_id: String,
    channel: i32,
    group_id: Option<String>,
    editable: Option<bool>,
}

#[derive(Debug, Deserialize, Clone)]
#[allow(dead_code)]
struct RemoteRoom {
    user_id: String,
    level: i32,
    group_id: String,
}

#[derive(Debug, Deserialize, Clone)]
struct RemoteEnv {
    #[serde(default, deserialize_with = "nullable_string")]
    id: String,
    #[serde(default, deserialize_with = "nullable_string")]
    user_id: String,
    #[serde(default, deserialize_with = "nullable_string")]
    name: String,
    #[serde(default, deserialize_with = "nullable_string")]
    rule: String,
    #[serde(default, deserialize_with = "nullable_string")]
    create_time: String,
    #[serde(default, deserialize_with = "nullable_string")]
    update_time: String,
}

#[derive(Debug, Deserialize, Clone)]
struct RemoteGroup {
    #[allow(dead_code)]
    #[serde(default, deserialize_with = "nullable_string")]
    id: String,
    #[serde(default, deserialize_with = "nullable_string")]
    name: String,
    #[allow(dead_code)]
    visibility: Option<serde_json::Value>,
}

#[derive(Debug, Serialize)]
struct GroupRuleInfo {
    name: String,
    enabled: bool,
    sort_order: i32,
    rule_count: usize,
    created_at: String,
    updated_at: String,
    remote_env_id: Option<String>,
    remote_user_id: Option<String>,
}

#[derive(Debug, Serialize)]
struct GroupRuleDetail {
    name: String,
    content: String,
    enabled: bool,
    sort_order: i32,
    created_at: String,
    updated_at: String,
    sync: GroupRuleSyncInfo,
}

#[derive(Debug, Serialize)]
struct GroupRuleSyncInfo {
    status: String,
    remote_id: Option<String>,
    remote_updated_at: Option<String>,
}

#[derive(Debug, Serialize)]
struct GroupRulesResponse {
    group_id: String,
    group_name: String,
    writable: bool,
    rules: Vec<GroupRuleInfo>,
}

#[derive(Debug, Deserialize)]
struct CreateGroupRuleRequest {
    name: String,
    content: Option<String>,
}

#[derive(Debug, Deserialize)]
struct UpdateGroupRuleRequest {
    content: String,
}

fn sanitize_group_dir_name(name: &str) -> String {
    name.replace(['/', '\\', '\0', ':'], "_")
}

async fn proxy_get_json<T: serde::de::DeserializeOwned>(
    sync_manager: &bifrost_sync::SyncManager,
    path: &str,
    query: Option<&str>,
) -> Result<T, String> {
    let (status, _content_type, body) = sync_manager
        .proxy_forward(reqwest::Method::GET, path, query, None)
        .await
        .map_err(|e| format!("proxy_forward failed: {e}"))?;
    if status != 200 {
        let body_str = String::from_utf8_lossy(&body);
        return Err(format!("Remote API returned status {status}: {body_str}"));
    }
    serde_json::from_slice(&body).map_err(|e| format!("JSON parse error: {e}"))
}

async fn proxy_post_json<T: serde::de::DeserializeOwned>(
    sync_manager: &bifrost_sync::SyncManager,
    path: &str,
    body: &serde_json::Value,
) -> Result<T, String> {
    let body_bytes = serde_json::to_vec(body).map_err(|e| format!("JSON encode: {e}"))?;
    let (status, _ct, resp_body) = sync_manager
        .proxy_forward(reqwest::Method::POST, path, None, Some(body_bytes))
        .await
        .map_err(|e| format!("proxy_forward failed: {e}"))?;
    if status != 200 {
        let s = String::from_utf8_lossy(&resp_body);
        return Err(format!("Remote API returned status {status}: {s}"));
    }
    serde_json::from_slice(&resp_body).map_err(|e| format!("JSON parse error: {e}"))
}

async fn proxy_patch_json<T: serde::de::DeserializeOwned>(
    sync_manager: &bifrost_sync::SyncManager,
    path: &str,
    body: &serde_json::Value,
) -> Result<T, String> {
    let body_bytes = serde_json::to_vec(body).map_err(|e| format!("JSON encode: {e}"))?;
    let (status, _ct, resp_body) = sync_manager
        .proxy_forward(reqwest::Method::PATCH, path, None, Some(body_bytes))
        .await
        .map_err(|e| format!("proxy_forward failed: {e}"))?;
    if status != 200 {
        let s = String::from_utf8_lossy(&resp_body);
        return Err(format!("Remote API returned status {status}: {s}"));
    }
    serde_json::from_slice(&resp_body).map_err(|e| format!("JSON parse error: {e}"))
}

async fn proxy_delete(sync_manager: &bifrost_sync::SyncManager, path: &str) -> Result<(), String> {
    let (status, _ct, resp_body) = sync_manager
        .proxy_forward(reqwest::Method::DELETE, path, None, None)
        .await
        .map_err(|e| format!("proxy_forward failed: {e}"))?;
    if status != 200 {
        let s = String::from_utf8_lossy(&resp_body);
        return Err(format!("Remote API returned status {status}: {s}"));
    }
    Ok(())
}

async fn fetch_group_info(
    sync_manager: &bifrost_sync::SyncManager,
    group_id: &str,
) -> Result<RemoteGroup, String> {
    let resp: RemoteResponse<RemoteGroup> =
        proxy_get_json(sync_manager, &format!("/v4/group/{group_id}"), None).await?;
    Ok(resp.data)
}

async fn fetch_room_members(
    sync_manager: &bifrost_sync::SyncManager,
    group_id: &str,
) -> Result<Vec<RemoteRoom>, String> {
    let query = format!("group_id={group_id}&offset=0&limit=500");
    let resp: RemoteResponse<RemoteListPayload<RemoteRoom>> =
        proxy_get_json(sync_manager, "/v4/room", Some(&query)).await?;
    Ok(resp.data.list.unwrap_or_default())
}

async fn fetch_user_envs(
    sync_manager: &bifrost_sync::SyncManager,
    user_id: &str,
) -> Result<Vec<RemoteEnv>, String> {
    let query = format!("user_id={user_id}&offset=0&limit=500");
    let resp: RemoteResponse<RemoteListPayload<RemoteEnv>> =
        proxy_get_json(sync_manager, "/v4/env", Some(&query)).await?;
    Ok(resp.data.list.unwrap_or_default())
}

async fn fetch_peers(sync_manager: &bifrost_sync::SyncManager) -> Result<Vec<RemotePeer>, String> {
    let resp: RemoteResponse<RemoteListPayload<RemotePeer>> =
        proxy_get_json(sync_manager, "/v4/user/peer", Some("offset=0&limit=500")).await?;
    Ok(resp.data.list.unwrap_or_default())
}

fn find_virtual_user_for_group<'a>(
    peers: &'a [RemotePeer],
    group_id: &str,
) -> Option<&'a RemotePeer> {
    peers.iter().find(|p| {
        p.channel == 3
            && p.group_id
                .as_deref()
                .map(|gid| gid == group_id)
                .unwrap_or(false)
    })
}

fn sync_envs_to_local(
    rules_storage: &RulesStorage,
    envs: &[RemoteEnv],
    group_name: &str,
) -> Result<bool, String> {
    let existing_names: std::collections::HashSet<String> = rules_storage
        .list()
        .unwrap_or_default()
        .into_iter()
        .collect();

    let mut active_rules_changed = false;
    let mut remote_names = std::collections::HashSet::new();
    for env in envs {
        let rule_name = env.name.clone();
        remote_names.insert(rule_name.clone());

        let existing = rules_storage.load(&rule_name).ok();
        let existing_enabled = existing.as_ref().map(|r| r.enabled).unwrap_or(false);
        if existing_enabled
            && existing
                .as_ref()
                .map(|r| r.content.as_str() != env.rule.as_str())
                .unwrap_or(false)
        {
            active_rules_changed = true;
        }

        let mut rule_file = bifrost_storage::RuleFile::new(&rule_name, &env.rule);
        rule_file.enabled = existing_enabled;
        rule_file.group = Some(group_name.to_string());
        rule_file.created_at = env.create_time.clone();
        rule_file.updated_at = env.update_time.clone();
        rule_file.mark_synced(&env.id, &env.user_id, &env.create_time, &env.update_time);

        rules_storage
            .save(&rule_file)
            .map_err(|e| format!("Failed to save rule: {e}"))?;
    }

    for name in &existing_names {
        if !remote_names.contains(name) {
            if rules_storage
                .load(name)
                .ok()
                .map(|r| r.enabled)
                .unwrap_or(false)
            {
                active_rules_changed = true;
            }
            let _ = rules_storage.delete(name);
        }
    }

    Ok(active_rules_changed)
}

fn build_rule_info_from_storage(rules_storage: &RulesStorage) -> Vec<GroupRuleInfo> {
    let names = rules_storage.list().unwrap_or_default();
    let mut rules = Vec::new();
    for (i, name) in names.iter().enumerate() {
        if let Ok(rule) = rules_storage.load(name) {
            let rule_count = rule
                .content
                .lines()
                .filter(|l| {
                    let t = l.trim();
                    !t.is_empty() && !t.starts_with('#')
                })
                .count();
            rules.push(GroupRuleInfo {
                name: rule.name.clone(),
                enabled: rule.enabled,
                sort_order: i as i32,
                rule_count,
                created_at: rule.created_at,
                updated_at: rule.updated_at,
                remote_env_id: rule.sync.remote_id,
                remote_user_id: rule.sync.remote_user_id,
            });
        }
    }
    rules
}

pub async fn handle_group_rules(
    req: Request<Incoming>,
    state: SharedAdminState,
    path: &str,
) -> Response<BoxBody> {
    let Some(sync_manager) = state.sync_manager.clone() else {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "Sync manager not available",
        );
    };

    let sub_path = path
        .strip_prefix("/api/group-rules")
        .unwrap_or("")
        .trim_start_matches('/');

    let parts: Vec<&str> = sub_path.split('/').filter(|s| !s.is_empty()).collect();
    if parts.is_empty() {
        return error_response(StatusCode::BAD_REQUEST, "group_id is required");
    }

    let group_ref = urlencoding::decode(parts[0])
        .map(|v| v.into_owned())
        .unwrap_or_else(|_| parts[0].to_string());
    let rule_name_segment = if parts.len() > 1 {
        Some(parts[1..].join("/"))
    } else {
        None
    };

    let method = req.method().clone();

    if let Some(ref seg) = rule_name_segment {
        let decoded = urlencoding::decode(seg)
            .map(|v| v.into_owned())
            .unwrap_or_else(|_| seg.clone());

        if let Some(rule_name) = decoded.strip_suffix("/enable") {
            return match method {
                Method::PUT => {
                    handle_enable_rule(sync_manager, state, &group_ref, rule_name, true).await
                }
                _ => method_not_allowed(),
            };
        }
        if let Some(rule_name) = decoded.strip_suffix("/disable") {
            return match method {
                Method::PUT => {
                    handle_enable_rule(sync_manager, state, &group_ref, rule_name, false).await
                }
                _ => method_not_allowed(),
            };
        }
    }

    match method {
        Method::GET if rule_name_segment.is_none() => {
            handle_list_and_sync(sync_manager, state, &group_ref).await
        }
        Method::GET if rule_name_segment.is_some() => {
            let rule_name = urlencoding::decode(rule_name_segment.as_ref().unwrap())
                .map(|v| v.into_owned())
                .unwrap_or_else(|_| rule_name_segment.unwrap());
            handle_get_rule(state, &group_ref, &rule_name).await
        }
        Method::POST => handle_create_rule(req, sync_manager, state, &group_ref).await,
        Method::PUT if rule_name_segment.is_some() => {
            let rule_name = urlencoding::decode(rule_name_segment.as_ref().unwrap())
                .map(|v| v.into_owned())
                .unwrap_or_else(|_| rule_name_segment.unwrap());
            handle_update_rule(req, sync_manager, state, &group_ref, &rule_name).await
        }
        Method::DELETE if rule_name_segment.is_some() => {
            let rule_name = urlencoding::decode(rule_name_segment.as_ref().unwrap())
                .map(|v| v.into_owned())
                .unwrap_or_else(|_| rule_name_segment.unwrap());
            handle_delete_rule(sync_manager, state, &group_ref, &rule_name).await
        }
        _ => method_not_allowed(),
    }
}

fn get_group_rules_storage(
    state: &SharedAdminState,
    group_name: &str,
) -> Result<RulesStorage, String> {
    let base = state.rules_storage.base_dir();
    let dir = base.join(sanitize_group_dir_name(group_name));
    RulesStorage::with_dir(dir).map_err(|e| format!("Failed to create group rules dir: {e}"))
}

struct ResolvedGroupRef {
    id: Option<String>,
    name: String,
}

fn local_group_dir_exists(state: &SharedAdminState, group_name: &str) -> bool {
    state
        .rules_storage
        .base_dir()
        .join(sanitize_group_dir_name(group_name))
        .is_dir()
}

async fn resolve_group_ref(
    sync_manager: Option<&bifrost_sync::SyncManager>,
    state: &SharedAdminState,
    group_ref: &str,
) -> Result<ResolvedGroupRef, String> {
    {
        let cache = state.group_name_cache();
        if let Some(name) = cache.get(group_ref) {
            return Ok(ResolvedGroupRef {
                id: Some(group_ref.to_string()),
                name: name.clone(),
            });
        }
        if let Some(id) = cache.reverse_lookup(group_ref) {
            return Ok(ResolvedGroupRef {
                id: Some(id),
                name: group_ref.to_string(),
            });
        }
    }

    if local_group_dir_exists(state, group_ref) {
        if let Some(sm) = sync_manager {
            let dir_name = sanitize_group_dir_name(group_ref);
            resolve_missing_group_caches(sm, state, std::slice::from_ref(&dir_name)).await;
            let cache = state.group_name_cache();
            if let Some(id) = cache.reverse_lookup(group_ref) {
                return Ok(ResolvedGroupRef {
                    id: Some(id),
                    name: group_ref.to_string(),
                });
            }
            if dir_name != group_ref {
                if let Some(id) = cache.reverse_lookup(&dir_name) {
                    return Ok(ResolvedGroupRef {
                        id: Some(id),
                        name: group_ref.to_string(),
                    });
                }
            }
        }
        return Ok(ResolvedGroupRef {
            id: None,
            name: group_ref.to_string(),
        });
    }

    let Some(sm) = sync_manager else {
        return Err("Group not loaded yet, list first".to_string());
    };
    if !sm.has_session() {
        return Err("Group not loaded yet, list first".to_string());
    }
    let group = fetch_group_info(sm, group_ref).await?;
    let name = group.name.clone();
    {
        let mut cache = state.group_name_cache();
        cache.insert(group_ref.to_string(), name.clone());
    }
    state.persist_group_name_cache();
    Ok(ResolvedGroupRef {
        id: Some(group_ref.to_string()),
        name,
    })
}

pub(crate) async fn resolve_missing_group_caches(
    sync_manager: &bifrost_sync::SyncManager,
    state: &SharedAdminState,
    uncached_dir_names: &[String],
) {
    if uncached_dir_names.is_empty() {
        return;
    }

    let peers = match fetch_peers(sync_manager).await {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(
                target: "bifrost_admin::group_rules",
                error = %e,
                "failed to fetch peers for group cache resolution"
            );
            return;
        }
    };

    let group_ids: Vec<String> = peers
        .iter()
        .filter_map(|p| p.group_id.clone())
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();

    let mut resolved = false;
    for gid in &group_ids {
        {
            let cache = state.group_name_cache();
            if cache.get(gid).is_some() {
                continue;
            }
        }
        match fetch_group_info(sync_manager, gid).await {
            Ok(group) => {
                let dir_name = sanitize_group_dir_name(&group.name);
                if uncached_dir_names.contains(&dir_name) {
                    let mut cache = state.group_name_cache();
                    cache.insert(gid.clone(), group.name);
                    resolved = true;
                    tracing::debug!(
                        target: "bifrost_admin::group_rules",
                        group_id = %gid,
                        dir_name = %dir_name,
                        "resolved missing group cache entry"
                    );
                }
            }
            Err(e) => {
                tracing::warn!(
                    target: "bifrost_admin::group_rules",
                    error = %e,
                    group_id = %gid,
                    "failed to fetch group info for cache resolution"
                );
            }
        }
    }

    if resolved {
        state.persist_group_name_cache();
    }
}

async fn resolve_writable_from_room(
    sync_manager: &bifrost_sync::SyncManager,
    group_id: &str,
) -> bool {
    let current_user_id = match sync_manager.current_user_id() {
        Some(id) => id,
        None => return false,
    };
    match fetch_room_members(sync_manager, group_id).await {
        Ok(members) => members
            .iter()
            .any(|m| m.user_id == current_user_id && m.level >= 1),
        Err(e) => {
            tracing::warn!(
                error = %e,
                group_id = %group_id,
                "Failed to fetch room members for writable check"
            );
            false
        }
    }
}

async fn ensure_virtual_user(sync_manager: &bifrost_sync::SyncManager, group_id: &str) {
    let path = format!("/v4/group/{group_id}/setting");
    match proxy_get_json::<serde_json::Value>(sync_manager, &path, None).await {
        Ok(_) => {
            tracing::info!(
                group_id = %group_id,
                "Triggered ensureVirtualUser via readSetting"
            );
        }
        Err(e) => {
            tracing::warn!(
                error = %e,
                group_id = %group_id,
                "Failed to trigger ensureVirtualUser via readSetting"
            );
        }
    }
}

async fn handle_list_and_sync(
    sync_manager: std::sync::Arc<bifrost_sync::SyncManager>,
    state: SharedAdminState,
    group_ref: &str,
) -> Response<BoxBody> {
    let resolved = match resolve_group_ref(Some(&sync_manager), &state, group_ref).await {
        Ok(r) => r,
        Err(e) => return error_response(StatusCode::BAD_GATEWAY, &e),
    };
    let group_name = resolved.name.clone();

    let group_storage = match get_group_rules_storage(&state, &group_name) {
        Ok(s) => s,
        Err(e) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, &e),
    };

    let Some(group_id) = resolved.id.as_deref() else {
        let rules = build_rule_info_from_storage(&group_storage);
        return json_response(&GroupRulesResponse {
            group_id: group_ref.to_string(),
            group_name,
            writable: false,
            rules,
        });
    };

    let mut peers = match fetch_peers(&sync_manager).await {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(
                error = %e,
                group_ref = %group_ref,
                group_id = %group_id,
                group_name = %group_name,
                "Failed to fetch peers, using local group rules only"
            );
            let rules = build_rule_info_from_storage(&group_storage);
            return json_response(&GroupRulesResponse {
                group_id: group_id.to_string(),
                group_name,
                writable: false,
                rules,
            });
        }
    };

    let mut virtual_user = find_virtual_user_for_group(&peers, group_id);

    if virtual_user.is_none() {
        tracing::info!(
            group_id = %group_id,
            group_name = %group_name,
            "No virtual user peer found, triggering ensureVirtualUser and retrying"
        );
        ensure_virtual_user(&sync_manager, group_id).await;

        if let Ok(new_peers) = fetch_peers(&sync_manager).await {
            peers = new_peers;
            virtual_user = find_virtual_user_for_group(&peers, group_id);
        }
    }

    let (virtual_user_id, writable) = match virtual_user {
        Some(p) => (p.user_id.clone(), p.editable.unwrap_or(false)),
        None => {
            tracing::info!(
                group_id = %group_id,
                group_name = %group_name,
                "Still no virtual user peer after ensureVirtualUser, using group name as fallback"
            );
            let writable = resolve_writable_from_room(&sync_manager, group_id).await;
            (group_name.clone(), writable)
        }
    };

    tracing::debug!(
        group_id = %group_id,
        group_name = %group_name,
        virtual_user_id = %virtual_user_id,
        writable = %writable,
        "Fetching group envs via virtual user"
    );

    match fetch_user_envs(&sync_manager, &virtual_user_id).await {
        Ok(envs) => match sync_envs_to_local(&group_storage, &envs, &group_name) {
            Ok(active_rules_changed) => {
                if active_rules_changed {
                    notify_rules_changed(&state);
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "Failed to sync envs to local storage");
            }
        },
        Err(e) => {
            tracing::warn!(
                error = %e,
                group_id = %group_id,
                virtual_user_id = %virtual_user_id,
                "Failed to fetch envs for virtual user, using local rules only"
            );
        }
    }

    let rules = build_rule_info_from_storage(&group_storage);

    json_response(&GroupRulesResponse {
        group_id: group_id.to_string(),
        group_name,
        writable,
        rules,
    })
}

async fn handle_get_rule(
    state: SharedAdminState,
    group_ref: &str,
    rule_name: &str,
) -> Response<BoxBody> {
    let group_name = match resolve_group_ref(None, &state, group_ref).await {
        Ok(r) => r.name,
        Err(e) => return error_response(StatusCode::BAD_REQUEST, &e),
    };

    let group_storage = match get_group_rules_storage(&state, &group_name) {
        Ok(s) => s,
        Err(e) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, &e),
    };

    match group_storage.load(rule_name) {
        Ok(rule) => json_response(&GroupRuleDetail {
            name: rule.name,
            content: rule.content,
            enabled: rule.enabled,
            sort_order: rule.sort_order,
            created_at: rule.created_at,
            updated_at: rule.updated_at,
            sync: GroupRuleSyncInfo {
                status: "synced".to_string(),
                remote_id: rule.sync.remote_id,
                remote_updated_at: rule.sync.remote_updated_at,
            },
        }),
        Err(_) => error_response(StatusCode::NOT_FOUND, "Rule not found"),
    }
}

async fn handle_create_rule(
    req: Request<Incoming>,
    sync_manager: std::sync::Arc<bifrost_sync::SyncManager>,
    state: SharedAdminState,
    group_ref: &str,
) -> Response<BoxBody> {
    let resolved = match resolve_group_ref(Some(&sync_manager), &state, group_ref).await {
        Ok(r) => r,
        Err(e) => return error_response(StatusCode::BAD_GATEWAY, &e),
    };
    let Some(group_id) = resolved.id.as_deref() else {
        return error_response(
            StatusCode::BAD_REQUEST,
            "Group id is not resolved yet; reload group list before creating remote group rules",
        );
    };
    let group_name = resolved.name;

    let mut peers = match fetch_peers(&sync_manager).await {
        Ok(p) => p,
        Err(e) => return error_response(StatusCode::BAD_GATEWAY, &e),
    };

    let mut virtual_user = find_virtual_user_for_group(&peers, group_id);

    if virtual_user.is_none() {
        ensure_virtual_user(&sync_manager, group_id).await;
        if let Ok(new_peers) = fetch_peers(&sync_manager).await {
            peers = new_peers;
            virtual_user = find_virtual_user_for_group(&peers, group_id);
        }
    }

    let (virtual_user_id, writable) = match virtual_user {
        Some(p) => (p.user_id.clone(), p.editable.unwrap_or(false)),
        None => {
            let writable = resolve_writable_from_room(&sync_manager, group_id).await;
            (group_name.clone(), writable)
        }
    };

    if !writable {
        return error_response(StatusCode::FORBIDDEN, "No write permission for this group");
    }

    let body = match req.collect().await {
        Ok(collected) => collected.to_bytes().to_vec(),
        Err(e) => return error_response(StatusCode::BAD_REQUEST, &format!("Read body: {e}")),
    };

    let create_req: CreateGroupRuleRequest = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => return error_response(StatusCode::BAD_REQUEST, &format!("Invalid JSON: {e}")),
    };

    let remote_body = serde_json::json!({
        "user_id": virtual_user_id,
        "name": create_req.name,
        "rule": create_req.content.unwrap_or_default(),
    });

    let env_resp: RemoteResponse<RemoteEnv> =
        match proxy_post_json(&sync_manager, "/v4/env", &remote_body).await {
            Ok(r) => r,
            Err(e) => return error_response(StatusCode::BAD_GATEWAY, &e),
        };
    let env = env_resp.data;

    let group_storage = match get_group_rules_storage(&state, &group_name) {
        Ok(s) => s,
        Err(e) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, &e),
    };

    let rule_name = env.name.clone();
    let mut rule_file = bifrost_storage::RuleFile::new(&rule_name, &env.rule);
    rule_file.enabled = false;
    rule_file.group = Some(group_name.clone());
    rule_file.created_at = env.create_time.clone();
    rule_file.updated_at = env.update_time.clone();
    rule_file.mark_synced(&env.id, &env.user_id, &env.create_time, &env.update_time);

    if let Err(e) = group_storage.save(&rule_file) {
        tracing::warn!(error = %e, "Failed to save created rule locally");
    }

    json_response(&GroupRuleDetail {
        name: rule_file.name,
        content: rule_file.content,
        enabled: rule_file.enabled,
        sort_order: 0,
        created_at: rule_file.created_at,
        updated_at: rule_file.updated_at,
        sync: GroupRuleSyncInfo {
            status: "synced".to_string(),
            remote_id: Some(env.id),
            remote_updated_at: Some(env.update_time),
        },
    })
}

async fn handle_update_rule(
    req: Request<Incoming>,
    sync_manager: std::sync::Arc<bifrost_sync::SyncManager>,
    state: SharedAdminState,
    group_ref: &str,
    rule_name: &str,
) -> Response<BoxBody> {
    let group_name = match resolve_group_ref(Some(&sync_manager), &state, group_ref).await {
        Ok(r) => r.name,
        Err(e) => return error_response(StatusCode::BAD_GATEWAY, &e),
    };

    let group_storage = match get_group_rules_storage(&state, &group_name) {
        Ok(s) => s,
        Err(e) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, &e),
    };

    let existing = match group_storage.load(rule_name) {
        Ok(r) => r,
        Err(_) => return error_response(StatusCode::NOT_FOUND, "Rule not found locally"),
    };

    let remote_id = match &existing.sync.remote_id {
        Some(id) => id.clone(),
        None => return error_response(StatusCode::BAD_REQUEST, "Rule has no remote_id"),
    };

    let body = match req.collect().await {
        Ok(collected) => collected.to_bytes().to_vec(),
        Err(e) => return error_response(StatusCode::BAD_REQUEST, &format!("Read body: {e}")),
    };

    let update_req: UpdateGroupRuleRequest = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => return error_response(StatusCode::BAD_REQUEST, &format!("Invalid JSON: {e}")),
    };

    let remote_body = serde_json::json!({
        "id": remote_id,
        "rule": update_req.content,
    });

    let env_resp: RemoteResponse<RemoteEnv> = match proxy_patch_json(
        &sync_manager,
        &format!("/v4/env/{remote_id}"),
        &remote_body,
    )
    .await
    {
        Ok(r) => r,
        Err(e) => return error_response(StatusCode::BAD_GATEWAY, &e),
    };
    let env = env_resp.data;

    let mut rule_file = bifrost_storage::RuleFile::new(rule_name, &env.rule);
    rule_file.enabled = existing.enabled;
    rule_file.group = Some(group_name.clone());
    rule_file.created_at = env.create_time.clone();
    rule_file.updated_at = env.update_time.clone();
    rule_file.mark_synced(&env.id, &env.user_id, &env.create_time, &env.update_time);

    if let Err(e) = group_storage.save(&rule_file) {
        tracing::warn!(error = %e, "Failed to save updated rule locally");
    }

    if existing.enabled {
        notify_rules_changed(&state);
    }

    json_response(&GroupRuleDetail {
        name: rule_file.name,
        content: rule_file.content,
        enabled: rule_file.enabled,
        sort_order: rule_file.sort_order,
        created_at: rule_file.created_at,
        updated_at: rule_file.updated_at,
        sync: GroupRuleSyncInfo {
            status: "synced".to_string(),
            remote_id: Some(env.id),
            remote_updated_at: Some(env.update_time),
        },
    })
}

async fn handle_delete_rule(
    sync_manager: std::sync::Arc<bifrost_sync::SyncManager>,
    state: SharedAdminState,
    group_ref: &str,
    rule_name: &str,
) -> Response<BoxBody> {
    let group_name = match resolve_group_ref(Some(&sync_manager), &state, group_ref).await {
        Ok(r) => r.name,
        Err(e) => return error_response(StatusCode::BAD_GATEWAY, &e),
    };

    let group_storage = match get_group_rules_storage(&state, &group_name) {
        Ok(s) => s,
        Err(e) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, &e),
    };

    let existing = match group_storage.load(rule_name) {
        Ok(r) => r,
        Err(_) => return error_response(StatusCode::NOT_FOUND, "Rule not found locally"),
    };

    if let Some(remote_id) = &existing.sync.remote_id {
        if let Err(e) = proxy_delete(&sync_manager, &format!("/v4/env/{remote_id}")).await {
            tracing::warn!(
                target: "bifrost_admin::group_rules",
                rule = %rule_name,
                group = %group_ref,
                error = %e,
                "failed to delete remote rule, proceeding with local delete"
            );
        }
    }

    if let Err(e) = group_storage.delete(rule_name) {
        tracing::warn!(error = %e, "Failed to delete rule locally");
    }

    if existing.enabled {
        notify_rules_changed(&state);
    }

    success_response("Rule deleted")
}

async fn handle_enable_rule(
    sync_manager: std::sync::Arc<bifrost_sync::SyncManager>,
    state: SharedAdminState,
    group_ref: &str,
    rule_name: &str,
    enabled: bool,
) -> Response<BoxBody> {
    let group_name = match resolve_group_ref(Some(&sync_manager), &state, group_ref).await {
        Ok(r) => r.name,
        Err(e) => return error_response(StatusCode::BAD_REQUEST, &e),
    };

    let group_storage = match get_group_rules_storage(&state, &group_name) {
        Ok(s) => s,
        Err(e) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, &e),
    };

    match group_storage.set_enabled(rule_name, enabled) {
        Ok(_) => {
            notify_rules_changed(&state);
            let action = if enabled { "enabled" } else { "disabled" };
            tracing::info!(
                target: "bifrost_admin::group_rules",
                group = %group_ref,
                group_name = %group_name,
                rule_name = %rule_name,
                enabled = %enabled,
                "group rule {action}"
            );
            success_response(&format!("Rule '{}' {} successfully", rule_name, action))
        }
        Err(e) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("Failed to update rule: {}", e),
        ),
    }
}

fn notify_rules_changed(state: &SharedAdminState) {
    state.refresh_badge_rules_cache();
    if let Some(ref config_manager) = state.config_manager {
        match config_manager.notify(ConfigChangeEvent::RulesChanged) {
            Ok(count) => {
                tracing::info!(
                    target: "bifrost_admin::group_rules",
                    receivers = count,
                    "notified rules changed event"
                );
            }
            Err(e) => {
                tracing::warn!(
                    target: "bifrost_admin::group_rules",
                    error = %e,
                    "failed to notify rules changed event (no receivers)"
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bifrost_storage::RuleFile;

    fn remote_env(name: &str, rule: &str) -> RemoteEnv {
        RemoteEnv {
            id: format!("env-{name}"),
            user_id: "virtual-user".to_string(),
            name: name.to_string(),
            rule: rule.to_string(),
            create_time: "2026-05-28T00:00:00Z".to_string(),
            update_time: "2026-05-28T00:00:00Z".to_string(),
        }
    }

    fn storage() -> RulesStorage {
        let dir = tempfile::tempdir().expect("temp dir");
        RulesStorage::with_dir(dir.keep()).expect("rules storage")
    }

    #[test]
    fn sync_envs_to_local_reports_enabled_rule_content_changes() {
        let storage = storage();
        let mut local_rule = RuleFile::new("badge-cache-rule", "old.example.com status://201");
        local_rule.enabled = true;
        local_rule.group = Some("cache-group".to_string());
        storage.save(&local_rule).expect("save local rule");

        let changed = sync_envs_to_local(
            &storage,
            &[remote_env(
                "badge-cache-rule",
                "new.example.com status://202",
            )],
            "cache-group",
        )
        .expect("sync envs");

        assert!(
            changed,
            "enabled rule content changes must refresh Badge cache"
        );
        let saved = storage.load("badge-cache-rule").expect("load synced rule");
        assert!(saved.enabled);
        assert_eq!(saved.content, "new.example.com status://202");
    }

    #[test]
    fn sync_envs_to_local_reports_enabled_rule_removal() {
        let storage = storage();
        let mut removed_rule =
            RuleFile::new("removed-active-rule", "gone.example.com status://410");
        removed_rule.enabled = true;
        removed_rule.group = Some("cache-group".to_string());
        storage.save(&removed_rule).expect("save local rule");

        let changed = sync_envs_to_local(&storage, &[], "cache-group").expect("sync envs");

        assert!(changed, "removing an enabled rule must refresh Badge cache");
        assert!(storage.load("removed-active-rule").is_err());
    }

    #[test]
    fn sync_envs_to_local_ignores_inactive_remote_metadata_changes() {
        let storage = storage();
        let mut disabled_rule = RuleFile::new("inactive-rule", "inactive.example.com status://200");
        disabled_rule.enabled = false;
        disabled_rule.group = Some("cache-group".to_string());
        storage.save(&disabled_rule).expect("save local rule");

        let changed = sync_envs_to_local(
            &storage,
            &[remote_env(
                "inactive-rule",
                "inactive.example.com status://204",
            )],
            "cache-group",
        )
        .expect("sync envs");

        assert!(
            !changed,
            "disabled rule updates do not affect active Badge/runtime state"
        );
        let saved = storage.load("inactive-rule").expect("load synced rule");
        assert!(!saved.enabled);
        assert_eq!(saved.content, "inactive.example.com status://204");
    }
}
