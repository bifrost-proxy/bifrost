use bifrost_agent::memory_runtime;
use http_body_util::BodyExt;
use hyper::{body::Incoming, Method, Request, Response, StatusCode};
use serde::{Deserialize, Serialize};
use std::fs;

use super::{
    error_response, full_body, json_response, json_response_with_status, method_not_allowed,
    BoxBody,
};

#[derive(Debug, Deserialize)]
struct ListQuery {
    query: Option<String>,
    limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct CreateMemoryRequest {
    content: String,
}

#[derive(Debug, Deserialize)]
struct SearchMemoryRequest {
    query: Option<String>,
    limit: Option<usize>,
}

#[derive(Debug, Serialize)]
struct ExportResponse {
    content: String,
    count: usize,
}

#[derive(Debug, Serialize)]
struct ImportReport {
    imported: usize,
}

/// 处理 Codex-style 文件长期记忆管理 API。
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
    let root = match memory_runtime::ensure_memory_layout() {
        Ok(root) => root,
        Err(error) => return memory_store_error(error),
    };
    match memory_runtime::search_memory_files(
        params.query.as_deref().unwrap_or(""),
        params.limit.unwrap_or(100),
        &root,
    ) {
        Ok(records) => json_response(&records),
        Err(error) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &error),
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
    let mut session = bifrost_agent::AgentSession::new("admin-api");
    session.source = "admin-api".to_string();
    match memory_runtime::remember_explicit(
        &bifrost_agent::AgentConfig::default(),
        &session,
        &body.content,
    ) {
        Ok(record) => json_response_with_status(StatusCode::CREATED, &record),
        Err(error) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &error),
    }
}

async fn handle_patch(_req: Request<Incoming>, _id: &str) -> Response<BoxBody> {
    error_response(
        StatusCode::METHOD_NOT_ALLOWED,
        "file-backed memories are append-oriented; edit MEMORY.md directly under agent/memory",
    )
}

async fn handle_delete(id: &str) -> Response<BoxBody> {
    let session = bifrost_agent::AgentSession::new("admin-api");
    match memory_runtime::forget_memory(&bifrost_agent::AgentConfig::default(), &session, id) {
        Ok(Some(id)) => json_response(&serde_json::json!({ "deleted": true, "id": id })),
        Ok(None) => error_response(StatusCode::NOT_FOUND, "memory entry not found"),
        Err(error) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &error),
    }
}

async fn handle_search(req: Request<Incoming>) -> Response<BoxBody> {
    let body = match read_json::<SearchMemoryRequest>(req).await {
        Ok(body) => body,
        Err(resp) => return resp,
    };
    let root = match memory_runtime::ensure_memory_layout() {
        Ok(root) => root,
        Err(error) => return memory_store_error(error),
    };
    match memory_runtime::search_memory_files(
        body.query.as_deref().unwrap_or(""),
        body.limit.unwrap_or(50),
        &root,
    ) {
        Ok(records) => json_response(&records),
        Err(error) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &error),
    }
}

async fn handle_import(req: Request<Incoming>) -> Response<BoxBody> {
    let body = match read_body(req).await {
        Ok(body) => body,
        Err(resp) => return resp,
    };
    let root = match memory_runtime::ensure_memory_layout() {
        Ok(root) => root,
        Err(error) => return memory_store_error(error),
    };
    let path = root.join("MEMORY.md");
    if let Err(error) = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .and_then(|mut file| std::io::Write::write_all(&mut file, body.as_bytes()))
    {
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("append {}: {error}", path.display()),
        );
    }
    json_response(&ImportReport {
        imported: body.lines().filter(|line| !line.trim().is_empty()).count(),
    })
}

async fn handle_export() -> Response<BoxBody> {
    let root = match memory_runtime::ensure_memory_layout() {
        Ok(root) => root,
        Err(error) => return memory_store_error(error),
    };
    const MAX_MEMORY_FILE_BYTES: u64 = 8 * 1024 * 1024;
    let summary_path = root.join("memory_summary.md");
    let summary = if std::fs::metadata(&summary_path)
        .map(|m| m.len())
        .unwrap_or(0)
        <= MAX_MEMORY_FILE_BYTES
    {
        fs::read_to_string(&summary_path).unwrap_or_default()
    } else {
        String::new()
    };
    let memory_path = root.join("MEMORY.md");
    let memory = if std::fs::metadata(&memory_path)
        .map(|m| m.len())
        .unwrap_or(0)
        <= MAX_MEMORY_FILE_BYTES
    {
        fs::read_to_string(&memory_path).unwrap_or_default()
    } else {
        String::new()
    };
    let content = format!("--- memory_summary.md ---\n{summary}\n--- MEMORY.md ---\n{memory}");
    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "application/json")
        .body(full_body(
            serde_json::to_string(&ExportResponse {
                count: content.lines().count(),
                content,
            })
            .unwrap_or_else(|_| "{}".to_string()),
        ))
        .unwrap()
}

async fn handle_stats() -> Response<BoxBody> {
    match memory_runtime::memory_stats() {
        Ok(stats) => json_response(&stats),
        Err(error) => memory_store_error(error),
    }
}

fn memory_store_error(error: String) -> Response<BoxBody> {
    error_response(
        StatusCode::INTERNAL_SERVER_ERROR,
        &format!("Failed to access memory files: {error}"),
    )
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
