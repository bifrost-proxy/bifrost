use bifrost_agent::config::agent_home_dir;
use http_body_util::BodyExt;
use hyper::{body::Incoming, Method, Request, Response, StatusCode};
use memory::{
    GcPolicy, MemoryId, MemoryKind, MemoryPatch, MemoryScope, MemorySearchQuery, MemorySource,
    MemoryStore, NewMemoryRecord, SqliteMemoryStore,
};
use serde::{Deserialize, Serialize};
use std::io::BufReader;

use super::{
    error_response, full_body, json_response, json_response_with_status, method_not_allowed,
    BoxBody,
};

#[derive(Debug, Deserialize)]
struct ListQuery {
    query: Option<String>,
    scope_type: Option<String>,
    scope_value: Option<String>,
    kind: Option<String>,
    tag: Option<String>,
    limit: Option<usize>,
    offset: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct CreateMemoryRequest {
    scope: Option<MemoryScope>,
    kind: Option<MemoryKind>,
    content: String,
    tags: Option<Vec<String>>,
    pinned: Option<bool>,
    confidence: Option<f32>,
    expires_at: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct SearchMemoryRequest {
    query: Option<String>,
    scopes: Option<Vec<MemoryScope>>,
    kind: Option<MemoryKind>,
    tag: Option<String>,
    limit: Option<usize>,
    offset: Option<usize>,
}

#[derive(Debug, Serialize)]
struct ExportResponse {
    content: String,
    count: usize,
}

/// 处理长期记忆管理 API。
pub async fn handle_agent_memories(req: Request<Incoming>, path: &str) -> Response<BoxBody> {
    let suffix = path.strip_prefix("/api/agent/memories").unwrap_or("");
    match (req.method(), suffix) {
        (&Method::GET, "/stats") => handle_stats().await,
        (&Method::GET, "/export") => handle_export().await,
        (&Method::POST, "/import") => handle_import(req).await,
        (&Method::POST, "/search") => handle_search(req).await,
        (&Method::GET, "") | (&Method::GET, "/") => handle_list(req).await,
        (&Method::POST, "") | (&Method::POST, "/") => handle_create(req).await,
        (&Method::PATCH, id_path) if id_path.starts_with('/') => {
            handle_patch(req, id_path.trim_start_matches('/')).await
        }
        (&Method::DELETE, id_path) if id_path.starts_with('/') => {
            handle_delete(id_path.trim_start_matches('/')).await
        }
        _ => method_not_allowed(),
    }
}

async fn handle_list(req: Request<Incoming>) -> Response<BoxBody> {
    let query = req.uri().query().unwrap_or("");
    let params = match serde_urlencoded::from_str::<ListQuery>(query) {
        Ok(params) => params,
        Err(error) => {
            return error_response(StatusCode::BAD_REQUEST, &format!("Invalid query: {error}"));
        }
    };
    let store = match open_store() {
        Ok(store) => store,
        Err(error) => return memory_store_error(error),
    };
    let search = MemorySearchQuery {
        query: params.query,
        scopes: scope_filter(params.scope_type, params.scope_value),
        kind: params.kind.and_then(|kind| kind.parse().ok()),
        tag: params.tag,
        include_deleted: false,
        limit: params.limit.unwrap_or(50),
        offset: params.offset.unwrap_or(0),
    };
    match store.search(search) {
        Ok(records) => json_response(&records),
        Err(error) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string()),
    }
}

async fn handle_create(req: Request<Incoming>) -> Response<BoxBody> {
    let body = match read_json::<CreateMemoryRequest>(req).await {
        Ok(body) => body,
        Err(resp) => return resp,
    };
    if body.content.trim().is_empty() {
        return error_response(StatusCode::BAD_REQUEST, "content must not be empty");
    }
    let store = match open_store() {
        Ok(store) => store,
        Err(error) => return memory_store_error(error),
    };
    let result = store.insert(NewMemoryRecord {
        scope: body.scope.unwrap_or(MemoryScope::Global),
        kind: body.kind.unwrap_or(MemoryKind::Fact),
        content: body.content,
        source: MemorySource::UserExplicit,
        tags: body.tags.unwrap_or_default(),
        pinned: body.pinned.unwrap_or(false),
        confidence: body.confidence.unwrap_or(1.0),
        expires_at: body.expires_at,
    });
    match result {
        Ok(record) => json_response_with_status(StatusCode::CREATED, &record),
        Err(error) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string()),
    }
}

async fn handle_patch(req: Request<Incoming>, id: &str) -> Response<BoxBody> {
    let patch = match read_json::<MemoryPatch>(req).await {
        Ok(patch) => patch,
        Err(resp) => return resp,
    };
    let store = match open_store() {
        Ok(store) => store,
        Err(error) => return memory_store_error(error),
    };
    match store.update(&MemoryId::from_string(id), patch) {
        Ok(record) => json_response(&record),
        Err(error) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string()),
    }
}

async fn handle_delete(id: &str) -> Response<BoxBody> {
    let store = match open_store() {
        Ok(store) => store,
        Err(error) => return memory_store_error(error),
    };
    match store.soft_delete(&MemoryId::from_string(id)) {
        Ok(true) => json_response(&serde_json::json!({ "deleted": true })),
        Ok(false) => error_response(StatusCode::NOT_FOUND, "memory not found"),
        Err(error) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string()),
    }
}

async fn handle_search(req: Request<Incoming>) -> Response<BoxBody> {
    let body = match read_json::<SearchMemoryRequest>(req).await {
        Ok(body) => body,
        Err(resp) => return resp,
    };
    let store = match open_store() {
        Ok(store) => store,
        Err(error) => return memory_store_error(error),
    };
    match store.search(MemorySearchQuery {
        query: body.query,
        scopes: body.scopes.unwrap_or_default(),
        kind: body.kind,
        tag: body.tag,
        include_deleted: false,
        limit: body.limit.unwrap_or(50),
        offset: body.offset.unwrap_or(0),
    }) {
        Ok(records) => json_response(&records),
        Err(error) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string()),
    }
}

async fn handle_import(req: Request<Incoming>) -> Response<BoxBody> {
    let body = match read_body(req).await {
        Ok(body) => body,
        Err(resp) => return resp,
    };
    let store = match open_store() {
        Ok(store) => store,
        Err(error) => return memory_store_error(error),
    };
    match store.import_jsonl(BufReader::new(body.as_bytes())) {
        Ok(report) => json_response(&report),
        Err(error) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string()),
    }
}

async fn handle_export() -> Response<BoxBody> {
    let store = match open_store() {
        Ok(store) => store,
        Err(error) => return memory_store_error(error),
    };
    let mut output = Vec::new();
    match store.export_jsonl(&mut output) {
        Ok(count) => {
            let content = String::from_utf8(output).unwrap_or_default();
            Response::builder()
                .status(StatusCode::OK)
                .header("Content-Type", "application/json")
                .body(full_body(
                    serde_json::to_string(&ExportResponse { content, count })
                        .unwrap_or_else(|_| "{}".to_string()),
                ))
                .unwrap()
        }
        Err(error) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string()),
    }
}

async fn handle_stats() -> Response<BoxBody> {
    let store = match open_store() {
        Ok(store) => store,
        Err(error) => return memory_store_error(error),
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let _ = store.gc(GcPolicy {
        now,
        max_unused_days: None,
        tombstone_path: None,
    });
    match store.stats(now) {
        Ok(stats) => json_response(&stats),
        Err(error) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string()),
    }
}

fn open_store() -> Result<SqliteMemoryStore, String> {
    SqliteMemoryStore::open(agent_home_dir()).map_err(|error| error.to_string())
}

fn memory_store_error(error: String) -> Response<BoxBody> {
    error_response(
        StatusCode::INTERNAL_SERVER_ERROR,
        &format!("Failed to open memory store: {error}"),
    )
}

fn scope_filter(scope_type: Option<String>, scope_value: Option<String>) -> Vec<MemoryScope> {
    match scope_type.as_deref() {
        Some("global") => vec![MemoryScope::Global],
        Some("user") => scope_value.into_iter().map(MemoryScope::User).collect(),
        Some("project") => scope_value.into_iter().map(MemoryScope::Project).collect(),
        Some("session") => scope_value.into_iter().map(MemoryScope::Session).collect(),
        _ => Vec::new(),
    }
}

async fn read_body(req: Request<Incoming>) -> Result<String, Response<BoxBody>> {
    let body_bytes = req
        .into_body()
        .collect()
        .await
        .map_err(|error| {
            error_response(
                StatusCode::BAD_REQUEST,
                &format!("Failed to read body: {error}"),
            )
        })?
        .to_bytes();
    String::from_utf8(body_bytes.to_vec()).map_err(|error| {
        error_response(StatusCode::BAD_REQUEST, &format!("Invalid UTF-8: {error}"))
    })
}

async fn read_json<T: for<'de> Deserialize<'de>>(
    req: Request<Incoming>,
) -> Result<T, Response<BoxBody>> {
    let body = read_body(req).await?;
    serde_json::from_str(&body)
        .map_err(|error| error_response(StatusCode::BAD_REQUEST, &format!("Invalid JSON: {error}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_filter_builds_global_scope() {
        assert_eq!(
            scope_filter(Some("global".to_string()), None),
            vec![MemoryScope::Global]
        );
    }

    #[test]
    fn scope_filter_requires_value_for_user_scope() {
        assert!(scope_filter(Some("user".to_string()), None).is_empty());
        assert_eq!(
            scope_filter(Some("user".to_string()), Some("u1".to_string())),
            vec![MemoryScope::User("u1".to_string())]
        );
    }
}
