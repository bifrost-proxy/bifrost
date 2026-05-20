use http_body_util::BodyExt;
use hyper::{body::Incoming, Method, Request, Response, StatusCode};
use tracing::debug;

use super::{error_response, full_body, BoxBody};
use crate::state::SharedAdminState;
use bifrost_storage::RulesStorage;
use bifrost_sync::SharedSyncManager;

fn is_group_list_request(method: &Method, api_path: &str) -> bool {
    *method == Method::GET && (api_path == "/api/group" || api_path == "/api/group/")
}

fn cache_group_ids_from_response(state: &SharedAdminState, response_body: &[u8]) {
    let parsed: serde_json::Value = match serde_json::from_slice(response_body) {
        Ok(v) => v,
        Err(_) => return,
    };

    let mut entries = Vec::new();
    if let Some(list) = parsed
        .pointer("/data/list")
        .and_then(|v| v.as_array())
        .or_else(|| parsed.pointer("/data").and_then(|v| v.as_array()))
    {
        for group in list {
            if let (Some(id), Some(name)) = (
                group
                    .get("id")
                    .or_else(|| group.get("group_id"))
                    .and_then(|v| v.as_str()),
                group.get("name").and_then(|v| v.as_str()),
            ) {
                entries.push((id.to_string(), name.to_string()));
            }
        }
    } else if let Some(group) = parsed.pointer("/data").and_then(|v| v.as_object()) {
        if let (Some(id), Some(name)) = (
            group
                .get("id")
                .or_else(|| group.get("group_id"))
                .and_then(|v| v.as_str()),
            group.get("name").and_then(|v| v.as_str()),
        ) {
            entries.push((id.to_string(), name.to_string()));
        }
    }

    if entries.is_empty() {
        return;
    }

    {
        let mut cache = state.group_name_cache();
        for (id, name) in entries {
            cache.insert(id, name);
        }
    }
    state.persist_group_name_cache();
}

fn cleanup_orphaned_group_dirs(state: &SharedAdminState, response_body: &[u8]) {
    let local_entries: Vec<(String, String)> = {
        let cache = state.group_name_cache();
        let entries = cache.entries();
        if entries.is_empty() {
            return;
        }
        entries
    };

    let parsed: serde_json::Value = match serde_json::from_slice(response_body) {
        Ok(v) => v,
        Err(_) => return,
    };

    let groups = match parsed.pointer("/data").and_then(|d| d.as_array()) {
        Some(arr) => arr,
        None => return,
    };

    let remote_group_ids: std::collections::HashSet<String> = groups
        .iter()
        .filter_map(|g| {
            g.get("group_id")
                .and_then(|id| id.as_str())
                .map(String::from)
        })
        .collect();

    let orphaned: Vec<&(String, String)> = local_entries
        .iter()
        .filter(|(gid, _)| !remote_group_ids.contains(gid))
        .collect();

    if orphaned.is_empty() {
        return;
    }

    let rules_base_dir = state.rules_storage.base_dir();
    let mut any_had_enabled = false;

    for (group_id, dir_name) in &orphaned {
        let dir_path = rules_base_dir.join(dir_name);
        if !dir_path.exists() {
            continue;
        }

        tracing::warn!(
            target: "bifrost_admin::group",
            group_id = %group_id,
            dir = %dir_name,
            "group no longer exists remotely, cleaning up local directory"
        );

        if let Ok(storage) = RulesStorage::with_dir(dir_path.clone()) {
            if let Ok(rules) = storage.load_all() {
                if rules.iter().any(|r| r.enabled) {
                    for rule in &rules {
                        if rule.enabled {
                            let _ = storage.set_enabled(&rule.name, false);
                        }
                    }
                    any_had_enabled = true;
                }
            }
        }

        if let Err(e) = std::fs::remove_dir_all(&dir_path) {
            tracing::warn!(
                target: "bifrost_admin::group",
                error = %e,
                dir = %dir_name,
                "failed to remove orphaned group directory"
            );
        } else {
            tracing::info!(
                target: "bifrost_admin::group",
                group_id = %group_id,
                dir = %dir_name,
                "removed orphaned group directory"
            );
        }
    }

    {
        let mut cache = state.group_name_cache();
        for (group_id, _) in &orphaned {
            cache.remove(group_id);
        }
        cache.persist(rules_base_dir);
    }

    if any_had_enabled {
        super::rules::notify_rules_changed_pub(state);
    }
}

fn is_single_group_detail(method: &Method, api_path: &str) -> Option<String> {
    if *method != Method::GET {
        return None;
    }
    let sub = api_path.strip_prefix("/api/group/")?;
    if sub.is_empty() || sub.contains('/') {
        return None;
    }
    Some(sub.to_string())
}

async fn enrich_group_detail(
    sync_manager: &SharedSyncManager,
    group_id: &str,
    mut response_body: Vec<u8>,
) -> Vec<u8> {
    let parsed: serde_json::Value = match serde_json::from_slice(&response_body) {
        Ok(v) => v,
        Err(_) => return response_body,
    };

    let has_visibility = parsed
        .pointer("/data/visibility")
        .is_some_and(|v| !v.is_null());

    if has_visibility {
        return response_body;
    }

    let setting_path = format!("/v4/group/{}/setting", group_id);
    let setting_result = sync_manager
        .proxy_forward(reqwest::Method::GET, &setting_path, None, None)
        .await;

    if let Ok((200, _, setting_bytes)) = setting_result {
        if let Ok(setting_json) = serde_json::from_slice::<serde_json::Value>(&setting_bytes) {
            let level = setting_json
                .pointer("/data/level")
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            let visibility = if level == 1 { 1 } else { 0 };

            if let Ok(mut root) = serde_json::from_slice::<serde_json::Value>(&response_body) {
                if let Some(data) = root.get_mut("data").and_then(|d| d.as_object_mut()) {
                    data.insert(
                        "visibility".to_string(),
                        serde_json::Value::Number(visibility.into()),
                    );
                    if let Ok(enriched) = serde_json::to_vec(&root) {
                        debug!(group_id = %group_id, visibility = visibility, "enriched group detail with visibility from setting");
                        response_body = enriched;
                    }
                }
            }
        }
    }

    response_body
}

pub async fn handle_group(
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

    let method = req.method().clone();
    let query = req.uri().query().map(|q| q.to_string());
    let remote_path = path.replacen("/api/group", "/v4/group", 1);
    let detail_group_id = is_single_group_detail(&method, path);
    let is_list_request = is_group_list_request(&method, path);

    let body = match req.collect().await {
        Ok(collected) => {
            let bytes = collected.to_bytes();
            if bytes.is_empty() {
                None
            } else {
                Some(bytes.to_vec())
            }
        }
        Err(error) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                &format!("Failed to read body: {error}"),
            )
        }
    };

    let reqwest_method = match method {
        Method::GET => reqwest::Method::GET,
        Method::POST => reqwest::Method::POST,
        Method::PUT => reqwest::Method::PUT,
        Method::PATCH => reqwest::Method::PATCH,
        Method::DELETE => reqwest::Method::DELETE,
        _ => {
            return error_response(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed");
        }
    };

    match sync_manager
        .proxy_forward(reqwest_method, &remote_path, query.as_deref(), body)
        .await
    {
        Ok((status, content_type, mut response_body)) => {
            if status == 200 {
                if let Some(ref group_id) = detail_group_id {
                    response_body =
                        enrich_group_detail(&sync_manager, group_id, response_body).await;
                }
                if is_list_request || detail_group_id.is_some() {
                    cache_group_ids_from_response(&state, &response_body);
                }
                if is_list_request {
                    cleanup_orphaned_group_dirs(&state, &response_body);
                }
            }

            let status_code =
                StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
            Response::builder()
                .status(status_code)
                .header("Content-Type", content_type)
                .body(full_body(response_body))
                .unwrap()
        }
        Err(error) => error_response(
            StatusCode::BAD_GATEWAY,
            &format!("Failed to proxy group request: {error}"),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AdminState;
    use std::sync::Arc;

    #[test]
    fn cache_group_ids_from_response_persists_to_rules_data_dir() {
        let temp_dir = tempfile::tempdir().unwrap();
        let rules_dir = temp_dir.path().join("rules");
        let storage = RulesStorage::with_dir(rules_dir.clone()).unwrap();
        let state = Arc::new(AdminState::new_for_test(19910, storage));

        cache_group_ids_from_response(
            &state,
            br#"{"code":0,"data":{"list":[{"id":"gid-next","name":"next-agent"}],"total":1}}"#,
        );

        let cache_file = rules_dir.join(".group_cache.json");
        assert!(cache_file.exists(), "group cache should be persisted");

        let reloaded = Arc::new(AdminState::new_for_test(
            19911,
            RulesStorage::with_dir(rules_dir).unwrap(),
        ));
        reloaded.load_group_name_cache();

        let cache = reloaded.group_name_cache();
        assert_eq!(cache.get("gid-next"), Some("next-agent".to_string()));
        assert_eq!(
            cache.reverse_lookup("next-agent"),
            Some("gid-next".to_string())
        );
    }
}
