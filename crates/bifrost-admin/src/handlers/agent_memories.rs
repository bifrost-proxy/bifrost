use base64::Engine;
use bifrost_agent::memory_runtime;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression;
use http_body_util::BodyExt;
use hyper::{body::Incoming, Method, Request, Response, StatusCode};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};

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
struct PatchMemoryRequest {
    content: String,
}

#[derive(Debug, Deserialize)]
struct SearchMemoryRequest {
    query: Option<String>,
    limit: Option<usize>,
}

#[derive(Debug, Serialize)]
struct ExportResponse {
    /// Base64-encoded tar.gz archive of the full memory root.
    archive_b64: String,
    /// Archive byte size (pre-base64).
    bytes: u64,
    /// Number of files included in the archive.
    file_count: usize,
    /// Absolute memory root on disk (for debugging).
    memory_root: String,
}

#[derive(Debug, Serialize)]
struct ImportReport {
    imported_files: usize,
    imported_bytes: u64,
    memory_root: String,
}

#[derive(Debug, Deserialize)]
struct ImportRequest {
    /// Base64-encoded tar.gz archive.
    archive_b64: String,
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

async fn handle_patch(req: Request<Incoming>, id: &str) -> Response<BoxBody> {
    if id.is_empty() {
        return error_response(StatusCode::BAD_REQUEST, "memory id required in path");
    }
    let body = match read_json::<PatchMemoryRequest>(req).await {
        Ok(body) => body,
        Err(resp) => return resp,
    };
    if body.content.trim().is_empty() {
        return error_response(StatusCode::BAD_REQUEST, "content must not be empty");
    }
    let session = bifrost_agent::AgentSession::new("admin-api");
    match memory_runtime::replace_memory(
        &bifrost_agent::AgentConfig::default(),
        &session,
        id,
        &body.content,
    ) {
        Ok(Some(id)) => json_response(&serde_json::json!({
            "updated": true,
            "id": id,
            "content": body.content,
        })),
        Ok(None) => error_response(StatusCode::NOT_FOUND, "memory entry not found"),
        Err(error) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &error),
    }
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

/// Import the full Codex-style memory tree from a base64-encoded tar.gz.
///
/// The payload must be a JSON object `{"archive_b64":"..."}`. Entries whose
/// paths escape the memory root are rejected. Existing files with the same
/// relative path are overwritten in place (atomic rename is not guaranteed,
/// but Bifrost memory APIs are append/replace tolerant).
async fn handle_import(req: Request<Incoming>) -> Response<BoxBody> {
    let body = match read_json::<ImportRequest>(req).await {
        Ok(body) => body,
        Err(resp) => return resp,
    };
    let root = match memory_runtime::ensure_memory_layout() {
        Ok(root) => root,
        Err(error) => return memory_store_error(error),
    };
    let archive =
        match base64::engine::general_purpose::STANDARD.decode(body.archive_b64.as_bytes()) {
            Ok(bytes) => bytes,
            Err(error) => {
                return error_response(
                    StatusCode::BAD_REQUEST,
                    &format!("invalid base64 archive: {error}"),
                );
            }
        };
    let mut decoder = GzDecoder::new(Cursor::new(archive));
    let mut expanded = Vec::new();
    if let Err(error) = decoder.read_to_end(&mut expanded) {
        return error_response(
            StatusCode::BAD_REQUEST,
            &format!("failed to gunzip archive: {error}"),
        );
    }
    let mut archive = tar::Archive::new(Cursor::new(expanded));
    let mut imported_files = 0usize;
    let mut imported_bytes = 0u64;
    let entries = match archive.entries() {
        Ok(entries) => entries,
        Err(error) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                &format!("failed to read tar entries: {error}"),
            );
        }
    };
    for entry in entries {
        let mut entry = match entry {
            Ok(e) => e,
            Err(error) => {
                return error_response(
                    StatusCode::BAD_REQUEST,
                    &format!("tar entry read error: {error}"),
                );
            }
        };
        let rel_path = match entry.path() {
            Ok(p) => p.into_owned(),
            Err(error) => {
                return error_response(
                    StatusCode::BAD_REQUEST,
                    &format!("tar entry path error: {error}"),
                );
            }
        };
        if !is_safe_relative(&rel_path) {
            return error_response(
                StatusCode::BAD_REQUEST,
                &format!("tar entry escapes archive root: {}", rel_path.display()),
            );
        }
        let target = root.join(&rel_path);
        if !target.starts_with(&root) {
            return error_response(
                StatusCode::BAD_REQUEST,
                &format!("tar entry writes outside root: {}", target.display()),
            );
        }
        let entry_type = entry.header().entry_type();
        if entry_type.is_dir() {
            if let Err(error) = fs::create_dir_all(&target) {
                return error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    &format!("create {}: {error}", target.display()),
                );
            }
            continue;
        }
        if !entry_type.is_file() {
            continue;
        }
        if let Some(parent) = target.parent() {
            if let Err(error) = fs::create_dir_all(parent) {
                return error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    &format!("create {}: {error}", parent.display()),
                );
            }
        }
        let mut buf = Vec::new();
        if let Err(error) = entry.read_to_end(&mut buf) {
            return error_response(
                StatusCode::BAD_REQUEST,
                &format!("read tar entry {}: {error}", rel_path.display()),
            );
        }
        if let Err(error) = fs::write(&target, &buf) {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("write {}: {error}", target.display()),
            );
        }
        imported_bytes += buf.len() as u64;
        imported_files += 1;
    }
    json_response(&ImportReport {
        imported_files,
        imported_bytes,
        memory_root: root.display().to_string(),
    })
}

/// Export the full memory root as a base64-encoded tar.gz archive.
///
/// Returning the archive inline keeps the endpoint JSON-only and avoids having
/// to introduce multipart or streaming download paths elsewhere in the admin
/// API. Callers that need a raw blob can pipe the decoded bytes through
/// `base64 -d | tar -xz`.
async fn handle_export() -> Response<BoxBody> {
    let root = match memory_runtime::ensure_memory_layout() {
        Ok(root) => root,
        Err(error) => return memory_store_error(error),
    };
    let encoder = GzEncoder::new(Vec::new(), Compression::default());
    let mut builder = tar::Builder::new(encoder);
    builder.mode(tar::HeaderMode::Deterministic);
    let file_count = match append_tree(&mut builder, &root, &root) {
        Ok(count) => count,
        Err(error) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("failed to archive memory root: {error}"),
            );
        }
    };
    let encoder = match builder.into_inner() {
        Ok(enc) => enc,
        Err(error) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("finalize tar: {error}"),
            );
        }
    };
    let compressed = match encoder.finish() {
        Ok(bytes) => bytes,
        Err(error) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("finalize gzip: {error}"),
            );
        }
    };
    let bytes = compressed.len() as u64;
    let archive_b64 = base64::engine::general_purpose::STANDARD.encode(&compressed);
    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "application/json")
        .body(full_body(
            serde_json::to_string(&ExportResponse {
                archive_b64,
                bytes,
                file_count,
                memory_root: root.display().to_string(),
            })
            .unwrap_or_else(|_| "{}".to_string()),
        ))
        .unwrap()
}

fn append_tree<W: Write>(
    builder: &mut tar::Builder<W>,
    root: &Path,
    current: &Path,
) -> std::io::Result<usize> {
    let mut count = 0usize;
    for entry in fs::read_dir(current)? {
        let entry = entry?;
        let path = entry.path();
        let rel = match path.strip_prefix(root) {
            Ok(rel) => rel.to_path_buf(),
            Err(_) => continue,
        };
        if rel.as_os_str().is_empty() {
            continue;
        }
        // skip transient lock/telemetry files by default to keep archives small
        if let Some(name) = rel.file_name().and_then(|n| n.to_str()) {
            if name == ".phase2.lock" {
                continue;
            }
        }
        let meta = entry.metadata()?;
        if meta.is_dir() {
            builder.append_dir(&rel, &path)?;
            count += append_tree(builder, root, &path)?;
        } else if meta.is_file() {
            let mut file = fs::File::open(&path)?;
            let mut header = tar::Header::new_gnu();
            header.set_metadata(&meta);
            header.set_size(meta.len());
            header.set_cksum();
            builder.append_data(&mut header, &rel, &mut file)?;
            count += 1;
        }
    }
    Ok(count)
}

fn is_safe_relative(path: &Path) -> bool {
    if path.is_absolute() {
        return false;
    }
    for component in path.components() {
        match component {
            std::path::Component::ParentDir => return false,
            std::path::Component::Prefix(_) | std::path::Component::RootDir => return false,
            _ => {}
        }
    }
    let _ = PathBuf::from(path);
    true
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
