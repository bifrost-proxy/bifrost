use crate::handlers::{error_response, json_response, success_response, BoxBody};
use bifrost_script::{ScriptDetail, ScriptEngine, ScriptEngineConfig, ScriptInfo, ScriptType};
use bifrost_storage::{ConfigChangeEvent, SharedConfigManager, UnifiedConfig};
use http_body_util::BodyExt;
use hyper::{Method, Request, Response, StatusCode};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{error, info};

fn is_builtin_decode_script(name: &str) -> bool {
    matches!(name, "utf8" | "default")
}

fn builtin_decode_script_description(name: &str) -> Option<String> {
    match name {
        "utf8" => Some("Built-in UTF-8 (lossy) decoder".to_string()),
        "default" => Some("Built-in default decoder (alias of utf8)".to_string()),
        _ => None,
    }
}

fn builtin_decode_script_content(name: &str) -> Option<String> {
    let desc = builtin_decode_script_description(name)?;
    Some(format!(
        "// ============================================================\n// {}\n// ============================================================\n// 说明：这是 Bifrost 内置 decode 解码器（由 Rust 实现），不会执行 JS。\n// 作用：将输入 bytes 以 UTF-8 lossy 方式转换为文本，用于落库展示与搜索。\n// 使用：在规则中写 decode://{}\n//\n// 如果你需要自定义解码/脱敏/格式化：请新建 decode 脚本文件。\n",
        desc, name
    ))
}

pub struct ScriptManager {
    engine: ScriptEngine,
}

impl ScriptManager {
    fn inline_script<'a>(
        script_ref: &str,
        ctx: &'a bifrost_script::ScriptContext,
    ) -> Option<(&'a str, String)> {
        let name = script_ref.strip_prefix('{')?.strip_suffix('}')?;
        ctx.values
            .get(name)
            .map(|content| (content.as_str(), name.to_string()))
    }

    fn apply_request_test_result(
        request: &mut bifrost_script::RequestData,
        result: &bifrost_script::ScriptExecutionResult,
    ) {
        if !result.success {
            return;
        }
        if let Some(mods) = &result.request_modifications {
            if let Some(method) = &mods.method {
                request.method = method.clone();
            }
            if let Some(headers) = &mods.headers {
                request.headers = headers.clone();
            }
            if mods.body.is_some() {
                request.body = mods.body.clone();
            }
        }
    }

    fn apply_response_test_result(
        response: &mut bifrost_script::ResponseData,
        result: &bifrost_script::ScriptExecutionResult,
    ) {
        if !result.success {
            return;
        }
        if let Some(mods) = &result.response_modifications {
            if let Some(status) = mods.status {
                response.status = status;
            }
            if let Some(status_text) = &mods.status_text {
                response.status_text = status_text.clone();
            }
            if let Some(headers) = &mods.headers {
                response.headers = headers.clone();
            }
            if mods.body.is_some() {
                response.body = mods.body.clone();
            }
        }
    }

    pub fn new(scripts_dir: PathBuf) -> Self {
        Self {
            engine: ScriptEngine::new(ScriptEngineConfig {
                scripts_dir,
                timeout_ms: 10000,
                max_memory: 32 * 1024 * 1024,
            }),
        }
    }

    pub async fn init(&self) -> Result<(), bifrost_script::ScriptError> {
        self.engine.init().await
    }

    pub fn engine(&self) -> &ScriptEngine {
        &self.engine
    }

    pub async fn execute_request_scripts(
        &self,
        script_names: &[String],
        request: &mut bifrost_script::RequestData,
        ctx: &bifrost_script::ScriptContext,
    ) -> Vec<bifrost_script::ScriptExecutionResult> {
        if !bifrost_core::scripts_allowed() {
            return Vec::new();
        }
        let mut results = Vec::new();
        for script_name in script_names {
            let mut result = if let Some((content, name)) = Self::inline_script(script_name, ctx) {
                let mut result = self
                    .engine
                    .test_script(ScriptType::Request, content, Some(request), None, ctx)
                    .await;
                result.script_name = format!("{{{name}}}");
                Self::apply_request_test_result(request, &result);
                result
            } else {
                self.engine
                    .execute_request_script(script_name, request, ctx)
                    .await
            };
            result.script_type = ScriptType::Request;
            results.push(result);
        }
        results
    }

    pub async fn execute_request_scripts_with_config(
        &self,
        script_names: &[String],
        request: &mut bifrost_script::RequestData,
        ctx: &bifrost_script::ScriptContext,
        cfg: &UnifiedConfig,
    ) -> Vec<bifrost_script::ScriptExecutionResult> {
        if !bifrost_core::scripts_allowed() {
            return Vec::new();
        }
        let mut results = Vec::new();
        for script_name in script_names {
            let mut result = if let Some((content, name)) = Self::inline_script(script_name, ctx) {
                let mut result = self
                    .engine
                    .test_script_with_config(
                        ScriptType::Request,
                        content,
                        Some(request),
                        None,
                        ctx,
                        cfg,
                    )
                    .await;
                result.script_name = format!("{{{name}}}");
                Self::apply_request_test_result(request, &result);
                result
            } else {
                self.engine
                    .execute_request_script_with_config(script_name, request, ctx, cfg)
                    .await
            };
            result.script_type = ScriptType::Request;
            results.push(result);
        }
        results
    }

    pub async fn execute_response_scripts(
        &self,
        script_names: &[String],
        response: &mut bifrost_script::ResponseData,
        ctx: &bifrost_script::ScriptContext,
    ) -> Vec<bifrost_script::ScriptExecutionResult> {
        if !bifrost_core::scripts_allowed() {
            return Vec::new();
        }
        let mut results = Vec::new();
        for script_name in script_names {
            let mut result = if let Some((content, name)) = Self::inline_script(script_name, ctx) {
                let mut result = self
                    .engine
                    .test_script(ScriptType::Response, content, None, Some(response), ctx)
                    .await;
                result.script_name = format!("{{{name}}}");
                Self::apply_response_test_result(response, &result);
                result
            } else {
                self.engine
                    .execute_response_script(script_name, response, ctx)
                    .await
            };
            result.script_type = ScriptType::Response;
            results.push(result);
        }
        results
    }

    pub async fn execute_response_scripts_with_config(
        &self,
        script_names: &[String],
        response: &mut bifrost_script::ResponseData,
        ctx: &bifrost_script::ScriptContext,
        cfg: &UnifiedConfig,
    ) -> Vec<bifrost_script::ScriptExecutionResult> {
        if !bifrost_core::scripts_allowed() {
            return Vec::new();
        }
        let mut results = Vec::new();
        for script_name in script_names {
            let mut result = if let Some((content, name)) = Self::inline_script(script_name, ctx) {
                let mut result = self
                    .engine
                    .test_script_with_config(
                        ScriptType::Response,
                        content,
                        None,
                        Some(response),
                        ctx,
                        cfg,
                    )
                    .await;
                result.script_name = format!("{{{name}}}");
                Self::apply_response_test_result(response, &result);
                result
            } else {
                self.engine
                    .execute_response_script_with_config(script_name, response, ctx, cfg)
                    .await
            };
            result.script_type = ScriptType::Response;
            results.push(result);
        }
        results
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn execute_decode_scripts(
        &self,
        script_names: &[String],
        phase: &str,
        request: &bifrost_script::RequestData,
        request_body_bytes: &[u8],
        response: &bifrost_script::ResponseData,
        response_body_bytes: &[u8],
        ctx: &bifrost_script::ScriptContext,
    ) -> Vec<
        std::result::Result<
            (
                bifrost_script::DecodeOutput,
                Vec<bifrost_script::ScriptLogEntry>,
            ),
            bifrost_script::ScriptError,
        >,
    > {
        if !bifrost_core::scripts_allowed() {
            return Vec::new();
        }
        let mut results = Vec::new();
        for script_name in script_names {
            let result = self
                .engine
                .execute_decode_script(
                    script_name,
                    phase,
                    request,
                    request_body_bytes,
                    response,
                    response_body_bytes,
                    ctx,
                )
                .await;
            results.push(result);
        }
        results
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn execute_decode_scripts_with_config(
        &self,
        script_names: &[String],
        phase: &str,
        request: &bifrost_script::RequestData,
        request_body_bytes: &[u8],
        response: &bifrost_script::ResponseData,
        response_body_bytes: &[u8],
        ctx: &bifrost_script::ScriptContext,
        cfg: &UnifiedConfig,
    ) -> Vec<
        std::result::Result<
            (
                bifrost_script::DecodeOutput,
                Vec<bifrost_script::ScriptLogEntry>,
            ),
            bifrost_script::ScriptError,
        >,
    > {
        if !bifrost_core::scripts_allowed() {
            return Vec::new();
        }
        let mut results = Vec::new();
        for script_name in script_names {
            let result = self
                .engine
                .execute_decode_script_with_config(
                    script_name,
                    phase,
                    request,
                    request_body_bytes,
                    response,
                    response_body_bytes,
                    ctx,
                    cfg,
                )
                .await;
            results.push(result);
        }
        results
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn execute_parser_script_with_config(
        &self,
        script_ref: &str,
        phase: &str,
        request: &bifrost_script::RequestData,
        request_body_bytes: &[u8],
        response: &bifrost_script::ResponseData,
        response_body_bytes: &[u8],
        ctx: &bifrost_script::ScriptContext,
        cfg: &UnifiedConfig,
    ) -> std::result::Result<
        (
            bifrost_script::DecodeOutput,
            Vec<bifrost_script::ScriptLogEntry>,
        ),
        bifrost_script::ScriptError,
    > {
        if !bifrost_core::scripts_allowed() {
            return Ok((
                bifrost_script::DecodeOutput {
                    data: String::new(),
                    code: "resource_pressure".into(),
                    msg: "parser script paused by resource pressure".into(),
                },
                Vec::new(),
            ));
        }
        self.engine
            .execute_parser_script_with_config(
                script_ref,
                phase,
                request,
                request_body_bytes,
                response,
                response_body_bytes,
                ctx,
                cfg,
            )
            .await
    }
}

#[derive(Serialize)]
struct ScriptsListResponse {
    request: Vec<ScriptInfo>,
    response: Vec<ScriptInfo>,
    decode: Vec<ScriptInfo>,
    parser: Vec<ScriptInfo>,
}

#[derive(Deserialize)]
pub struct SaveScriptRequest {
    pub content: String,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Deserialize)]
pub struct RenameScriptRequest {
    pub new_name: String,
}

#[derive(Deserialize)]
pub struct TestScriptRequest {
    #[serde(rename = "type")]
    pub script_type: ScriptType,
    pub content: String,
    #[serde(default)]
    pub mock_request: Option<MockRequestData>,
    #[serde(default)]
    pub mock_response: Option<MockResponseData>,
}

#[derive(Deserialize, Default)]
pub struct MockRequestData {
    #[serde(default = "default_url")]
    pub url: String,
    #[serde(default = "default_method")]
    pub method: String,
    #[serde(default)]
    pub headers: std::collections::HashMap<String, String>,
    #[serde(default)]
    pub body: Option<String>,
}

fn default_url() -> String {
    "https://example.com/api".to_string()
}

fn default_method() -> String {
    "GET".to_string()
}

#[derive(Deserialize, Default)]
pub struct MockResponseData {
    #[serde(default = "default_status")]
    pub status: u16,
    #[serde(default)]
    pub headers: std::collections::HashMap<String, String>,
    #[serde(default)]
    pub body: Option<String>,
}

fn default_status() -> u16 {
    200
}

pub async fn handle_scripts_request(
    req: Request<hyper::body::Incoming>,
    script_manager: Arc<RwLock<ScriptManager>>,
    config_manager: Option<SharedConfigManager>,
    admin_path: &str,
) -> Response<BoxBody> {
    let method = req.method().clone();
    let path = admin_path.to_string();

    match method {
        Method::GET => handle_get(path, script_manager).await,
        Method::PUT => handle_put(req, path, script_manager, config_manager.clone()).await,
        Method::DELETE => handle_delete(path, script_manager, config_manager.clone()).await,
        Method::POST => handle_post(req, path, script_manager, config_manager).await,
        _ => error_response(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed"),
    }
}

async fn handle_get(path: String, script_manager: Arc<RwLock<ScriptManager>>) -> Response<BoxBody> {
    let manager = script_manager.read().await;

    if path == "/api/scripts" || path == "/api/scripts/" {
        let request_scripts = manager
            .engine()
            .list_scripts(ScriptType::Request)
            .await
            .unwrap_or_default();
        let response_scripts = manager
            .engine()
            .list_scripts(ScriptType::Response)
            .await
            .unwrap_or_default();
        let decode_scripts = manager
            .engine()
            .list_scripts(ScriptType::Decode)
            .await
            .unwrap_or_default();
        let parser_scripts = manager
            .engine()
            .list_scripts(ScriptType::Parser)
            .await
            .unwrap_or_default();

        // 为了与规则层（decode://utf8 / decode://default）以及 WebSocket decode 行为保持一致，
        // 在脚本列表中暴露内置 decode 解码器（只读）。
        let mut decode_scripts = decode_scripts;
        for name in ["utf8", "default"] {
            if decode_scripts.iter().any(|s| s.name == name) {
                continue;
            }
            decode_scripts.push(ScriptInfo {
                name: name.to_string(),
                script_type: ScriptType::Decode,
                description: builtin_decode_script_description(name),
                created_at: 0,
                updated_at: 0,
            });
        }

        return json_response(&ScriptsListResponse {
            request: request_scripts,
            response: response_scripts,
            decode: decode_scripts,
            parser: parser_scripts,
        });
    }

    let remaining = path.trim_start_matches("/api/scripts/");
    let first_slash = remaining.find('/');
    if first_slash.is_none() {
        return error_response(StatusCode::BAD_REQUEST, "Invalid path: missing script name");
    }

    let (type_str, name) = remaining.split_at(first_slash.unwrap());
    let name = name.trim_start_matches('/');

    if name.is_empty() {
        return error_response(StatusCode::BAD_REQUEST, "Invalid path: empty script name");
    }

    let script_type = match type_str {
        "request" => ScriptType::Request,
        "response" => ScriptType::Response,
        "decode" => ScriptType::Decode,
        "parser" => ScriptType::Parser,
        _ => return error_response(StatusCode::BAD_REQUEST, "Invalid script type"),
    };

    // 内置 decode 脚本（只读）
    if script_type == ScriptType::Decode && is_builtin_decode_script(name) {
        let detail = ScriptDetail {
            info: ScriptInfo {
                name: name.to_string(),
                script_type,
                description: builtin_decode_script_description(name),
                created_at: 0,
                updated_at: 0,
            },
            content: builtin_decode_script_content(name).unwrap_or_default(),
        };
        return json_response(&detail);
    }

    match manager.engine().load_script(script_type, name).await {
        Ok(content) => {
            let detail = ScriptDetail {
                info: ScriptInfo {
                    name: name.to_string(),
                    script_type,
                    description: None,
                    created_at: 0,
                    updated_at: 0,
                },
                content,
            };
            json_response(&detail)
        }
        Err(e) => {
            error!("Failed to load script {}/{}: {}", script_type, name, e);
            error_response(StatusCode::NOT_FOUND, &format!("Script not found: {}", e))
        }
    }
}

async fn handle_put(
    req: Request<hyper::body::Incoming>,
    path: String,
    script_manager: Arc<RwLock<ScriptManager>>,
    config_manager: Option<SharedConfigManager>,
) -> Response<BoxBody> {
    let remaining = path.trim_start_matches("/api/scripts/");
    let first_slash = remaining.find('/');
    if first_slash.is_none() {
        return error_response(StatusCode::BAD_REQUEST, "Invalid path: missing script name");
    }

    let (type_str, name) = remaining.split_at(first_slash.unwrap());
    let name = name.trim_start_matches('/');

    if name.is_empty() {
        return error_response(StatusCode::BAD_REQUEST, "Invalid path: empty script name");
    }

    let script_type = match type_str {
        "request" => ScriptType::Request,
        "response" => ScriptType::Response,
        "decode" => ScriptType::Decode,
        "parser" => ScriptType::Parser,
        _ => return error_response(StatusCode::BAD_REQUEST, "Invalid script type"),
    };

    if script_type == ScriptType::Decode && is_builtin_decode_script(name) {
        return error_response(
            StatusCode::BAD_REQUEST,
            "Built-in decode script is read-only",
        );
    }

    let body = match req.collect().await {
        Ok(b) => b.to_bytes(),
        Err(e) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                &format!("Failed to read body: {}", e),
            );
        }
    };

    let save_req: SaveScriptRequest = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => {
            return error_response(StatusCode::BAD_REQUEST, &format!("Invalid JSON: {}", e));
        }
    };

    let manager = script_manager.read().await;
    match manager
        .engine()
        .save_script(script_type, name, &save_req.content)
        .await
    {
        Ok(()) => {
            info!("Saved {} script: {}", script_type, name);
            if let Some(ref cm) = config_manager {
                let _ = cm.notify(ConfigChangeEvent::ScriptsChanged);
            }
            let detail = ScriptDetail {
                info: ScriptInfo {
                    name: name.to_string(),
                    script_type,
                    description: save_req.description,
                    created_at: chrono::Utc::now().timestamp_millis() as u64,
                    updated_at: chrono::Utc::now().timestamp_millis() as u64,
                },
                content: save_req.content,
            };
            json_response(&detail)
        }
        Err(e) => {
            error!("Failed to save script {}/{}: {}", script_type, name, e);
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("Failed to save script: {}", e),
            )
        }
    }
}

async fn handle_delete(
    path: String,
    script_manager: Arc<RwLock<ScriptManager>>,
    config_manager: Option<SharedConfigManager>,
) -> Response<BoxBody> {
    let remaining = path.trim_start_matches("/api/scripts/");
    let first_slash = remaining.find('/');
    if first_slash.is_none() {
        return error_response(StatusCode::BAD_REQUEST, "Invalid path: missing script name");
    }

    let (type_str, name) = remaining.split_at(first_slash.unwrap());
    let name = name.trim_start_matches('/');

    if name.is_empty() {
        return error_response(StatusCode::BAD_REQUEST, "Invalid path: empty script name");
    }

    let script_type = match type_str {
        "request" => ScriptType::Request,
        "response" => ScriptType::Response,
        "decode" => ScriptType::Decode,
        "parser" => ScriptType::Parser,
        _ => return error_response(StatusCode::BAD_REQUEST, "Invalid script type"),
    };

    if script_type == ScriptType::Decode && is_builtin_decode_script(name) {
        return error_response(
            StatusCode::BAD_REQUEST,
            "Built-in decode script is read-only",
        );
    }

    let manager = script_manager.read().await;
    match manager.engine().delete_script(script_type, name).await {
        Ok(()) => {
            info!("Deleted {} script: {}", script_type, name);
            if let Some(ref cm) = config_manager {
                let _ = cm.notify(ConfigChangeEvent::ScriptsChanged);
            }
            success_response(&format!("Script {} deleted", name))
        }
        Err(e) => {
            error!("Failed to delete script {}/{}: {}", script_type, name, e);
            error_response(StatusCode::NOT_FOUND, &format!("Failed to delete: {}", e))
        }
    }
}

async fn handle_post(
    req: Request<hyper::body::Incoming>,
    path: String,
    script_manager: Arc<RwLock<ScriptManager>>,
    config_manager: Option<SharedConfigManager>,
) -> Response<BoxBody> {
    if path.starts_with("/api/scripts/rename/") {
        return handle_rename(req, path, script_manager, config_manager).await;
    }

    if path != "/api/scripts/test" && path != "/api/scripts/test/" {
        return error_response(StatusCode::NOT_FOUND, "Not found");
    }

    let body = match req.collect().await {
        Ok(b) => b.to_bytes(),
        Err(e) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                &format!("Failed to read body: {}", e),
            );
        }
    };

    let test_req: TestScriptRequest = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => {
            return error_response(StatusCode::BAD_REQUEST, &format!("Invalid JSON: {}", e));
        }
    };

    let manager = script_manager.read().await;

    let mock_request = test_req.mock_request.unwrap_or_default();
    let mock_response_opt = test_req.mock_response;

    let request_data = bifrost_script::RequestData {
        url: mock_request.url.clone(),
        method: mock_request.method.clone(),
        host: url::Url::parse(&mock_request.url)
            .map(|u| u.host_str().unwrap_or("").to_string())
            .unwrap_or_default(),
        path: url::Url::parse(&mock_request.url)
            .map(|u| u.path().to_string())
            .unwrap_or_default(),
        protocol: url::Url::parse(&mock_request.url)
            .map(|u| u.scheme().to_string())
            .unwrap_or_else(|_| "https".to_string()),
        client_ip: "127.0.0.1".to_string(),
        client_app: Some("test".to_string()),
        headers: mock_request.headers,
        body: mock_request.body,
    };

    let response_data = if let Some(mock_response) = mock_response_opt {
        Some(bifrost_script::ResponseData {
            status: mock_response.status,
            status_text: "OK".to_string(),
            headers: mock_response.headers,
            body: mock_response.body,
            request: request_data.clone(),
        })
    } else {
        None
    };

    let ctx = bifrost_script::ScriptContext {
        request_id: "test".to_string(),
        script_name: "test".to_string(),
        script_type: test_req.script_type,
        values: std::collections::HashMap::new(),
        matched_rules: vec![],
    };

    let cfg = if let Some(cm) = config_manager.as_ref() {
        Some(cm.config().await)
    } else {
        None
    };

    let result = if let Some(ref cfg) = cfg {
        manager
            .engine()
            .test_script_with_config(
                test_req.script_type,
                &test_req.content,
                Some(&request_data),
                response_data.as_ref(),
                &ctx,
                cfg,
            )
            .await
    } else {
        manager
            .engine()
            .test_script(
                test_req.script_type,
                &test_req.content,
                Some(&request_data),
                response_data.as_ref(),
                &ctx,
            )
            .await
    };

    json_response(&result)
}

async fn handle_rename(
    req: Request<hyper::body::Incoming>,
    path: String,
    script_manager: Arc<RwLock<ScriptManager>>,
    config_manager: Option<SharedConfigManager>,
) -> Response<BoxBody> {
    let remaining = path.trim_start_matches("/api/scripts/rename/");
    let first_slash = remaining.find('/');
    if first_slash.is_none() {
        return error_response(StatusCode::BAD_REQUEST, "Invalid path: missing script name");
    }

    let (type_str, name) = remaining.split_at(first_slash.unwrap());
    let name = name.trim_start_matches('/');

    if name.is_empty() {
        return error_response(StatusCode::BAD_REQUEST, "Invalid path: empty script name");
    }

    let script_type = match type_str {
        "request" => ScriptType::Request,
        "response" => ScriptType::Response,
        "decode" => ScriptType::Decode,
        "parser" => ScriptType::Parser,
        _ => return error_response(StatusCode::BAD_REQUEST, "Invalid script type"),
    };

    if script_type == ScriptType::Decode && is_builtin_decode_script(name) {
        return error_response(
            StatusCode::BAD_REQUEST,
            "Built-in decode script is read-only",
        );
    }

    let body = match req.collect().await {
        Ok(b) => b.to_bytes(),
        Err(e) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                &format!("Failed to read body: {}", e),
            );
        }
    };

    let rename_req: RenameScriptRequest = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => {
            return error_response(StatusCode::BAD_REQUEST, &format!("Invalid JSON: {}", e));
        }
    };

    let manager = script_manager.read().await;
    match manager
        .engine()
        .rename_script(script_type, name, &rename_req.new_name)
        .await
    {
        Ok(()) => {
            info!(
                "Renamed {} script: {} -> {}",
                script_type, name, rename_req.new_name
            );
            if let Some(ref cm) = config_manager {
                let _ = cm.notify(ConfigChangeEvent::ScriptsChanged);
            }
            success_response(&format!(
                "Script {} renamed to {}",
                name, rename_req.new_name
            ))
        }
        Err(e) => {
            error!("Failed to rename script {}/{}: {}", script_type, name, e);
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("Failed to rename script: {}", e),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bifrost_script::{RequestData, ResponseData, ScriptContext};
    use http_body_util::BodyExt;
    use std::collections::HashMap;
    use std::sync::Arc;
    use tokio::sync::RwLock;

    #[test]
    fn builtin_decode_script_helpers_match_supported_names() {
        assert!(is_builtin_decode_script("utf8"));
        assert!(is_builtin_decode_script("default"));
        assert!(!is_builtin_decode_script("other"));

        let utf8_desc = builtin_decode_script_description("utf8").unwrap();
        assert!(utf8_desc.to_lowercase().contains("utf-8"));

        assert!(builtin_decode_script_description("other").is_none());

        let utf8_content = builtin_decode_script_content("utf8").unwrap();
        assert!(utf8_content.contains("decode://utf8"));
    }

    #[tokio::test]
    async fn handle_get_exposes_builtin_decode_script_detail() {
        let dir = tempfile::tempdir().unwrap();
        let manager = Arc::new(RwLock::new(ScriptManager::new(dir.path().to_path_buf())));

        let resp = super::handle_get("/api/scripts/decode/utf8".to_string(), manager).await;
        assert_eq!(resp.status(), StatusCode::OK);

        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let detail: ScriptDetail = serde_json::from_slice(&body).unwrap();

        assert_eq!(detail.info.name, "utf8");
        assert_eq!(detail.info.script_type, ScriptType::Decode);
        assert!(detail.content.contains("decode://utf8"));
    }

    #[tokio::test]
    async fn inline_request_script_executes_block_and_preserves_string_body() {
        let dir = tempfile::tempdir().unwrap();
        let manager = ScriptManager::new(dir.path().to_path_buf());
        let expected = "quoted: \"yes\"\npath: C:\\tmp";
        let mut values = HashMap::new();
        values.insert(
            "request_bridge".to_string(),
            r#"
                request.headers["X-Inline"] = "yes";
                request.body = "quoted: \"yes\"\npath: C:\\tmp";
            "#
            .to_string(),
        );
        let ctx = ScriptContext {
            request_id: "inline-request-test".to_string(),
            script_name: "{request_bridge}".to_string(),
            script_type: ScriptType::Request,
            values,
            matched_rules: vec![],
        };
        let mut request = RequestData::default();

        let results = manager
            .execute_request_scripts(&["{request_bridge}".to_string()], &mut request, &ctx)
            .await;

        assert_eq!(results.len(), 1);
        assert!(results[0].success, "{:?}", results[0].error);
        assert_eq!(results[0].script_name, "{request_bridge}");
        assert_eq!(
            request.headers.get("X-Inline").map(String::as_str),
            Some("yes")
        );
        assert_eq!(request.body.as_deref(), Some(expected));
    }

    #[tokio::test]
    async fn critical_pressure_pauses_every_script_execution_surface() {
        const CHILD: &str = "BIFROST_SCRIPT_PRESSURE_CHILD";
        if std::env::var_os(CHILD).is_none() {
            let status = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "handlers::scripts::tests::critical_pressure_pauses_every_script_execution_surface",
                    "--nocapture",
                ])
                .env(CHILD, "1")
                .status()
                .unwrap();
            assert!(status.success());
            return;
        }

        bifrost_core::publish_resource_pressure(bifrost_core::ResourcePressureLevel::Critical);
        let dir = tempfile::tempdir().unwrap();
        let manager = ScriptManager::new(dir.path().to_path_buf());
        let mut request = RequestData::default();
        let mut response = ResponseData::default();
        let ctx = ScriptContext {
            request_id: "pressure".into(),
            script_name: "pressure".into(),
            script_type: ScriptType::Request,
            values: HashMap::new(),
            matched_rules: Vec::new(),
        };
        let config = UnifiedConfig::default();
        let names = vec!["must-not-run".to_string()];

        assert!(manager
            .execute_request_scripts(&names, &mut request, &ctx)
            .await
            .is_empty());
        assert!(manager
            .execute_request_scripts_with_config(&names, &mut request, &ctx, &config)
            .await
            .is_empty());
        assert!(manager
            .execute_response_scripts(&names, &mut response, &ctx)
            .await
            .is_empty());
        assert!(manager
            .execute_response_scripts_with_config(&names, &mut response, &ctx, &config)
            .await
            .is_empty());
        assert!(manager
            .execute_decode_scripts(&names, "response", &request, b"", &response, b"", &ctx)
            .await
            .is_empty());
        assert!(manager
            .execute_decode_scripts_with_config(
                &names, "response", &request, b"", &response, b"", &ctx, &config,
            )
            .await
            .is_empty());
        let (output, logs) = manager
            .execute_parser_script_with_config(
                "must-not-run",
                "response",
                &request,
                b"",
                &response,
                b"",
                &ctx,
                &config,
            )
            .await
            .unwrap();
        assert_eq!(output.code, "resource_pressure");
        assert!(logs.is_empty());
        bifrost_core::publish_resource_pressure(bifrost_core::ResourcePressureLevel::Normal);
    }

    #[tokio::test]
    async fn inline_response_script_executes_block_and_preserves_sse_body() {
        let dir = tempfile::tempdir().unwrap();
        let manager = ScriptManager::new(dir.path().to_path_buf());
        let expected = "event: delta\ndata: {\"text\":\"a\\\\b\"}\n\n";
        let mut values = HashMap::new();
        values.insert(
            "response_bridge".to_string(),
            r#"
                response.headers["Content-Type"] = "text/event-stream";
                response.body = "event: delta\ndata: {\"text\":\"a\\\\b\"}\n\n";
            "#
            .to_string(),
        );
        let ctx = ScriptContext {
            request_id: "inline-response-test".to_string(),
            script_name: "{response_bridge}".to_string(),
            script_type: ScriptType::Response,
            values,
            matched_rules: vec![],
        };
        let mut response = ResponseData::default();

        let results = manager
            .execute_response_scripts(&["{response_bridge}".to_string()], &mut response, &ctx)
            .await;

        assert_eq!(results.len(), 1);
        assert!(results[0].success, "{:?}", results[0].error);
        assert_eq!(results[0].script_name, "{response_bridge}");
        assert_eq!(
            response.headers.get("Content-Type").map(String::as_str),
            Some("text/event-stream")
        );
        assert_eq!(response.body.as_deref(), Some(expected));
    }
}
