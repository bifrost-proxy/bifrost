use crate::error::{Result, ScriptError};
use crate::sandbox::Sandbox;
use crate::types::*;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

use bifrost_storage::UnifiedConfig;

pub struct ScriptEngineConfig {
    pub scripts_dir: PathBuf,
    pub timeout_ms: u64,
    pub max_memory: usize,
}

impl Default for ScriptEngineConfig {
    fn default() -> Self {
        Self {
            scripts_dir: PathBuf::from("scripts"),
            timeout_ms: 10000,
            max_memory: 32 * 1024 * 1024,
        }
    }
}

pub struct ScriptEngine {
    config: ScriptEngineConfig,
    script_cache: Arc<RwLock<HashMap<String, String>>>,
}

impl ScriptEngine {
    pub fn new(config: ScriptEngineConfig) -> Self {
        Self {
            config,
            script_cache: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn scripts_dir(&self) -> &PathBuf {
        &self.config.scripts_dir
    }

    fn default_sandbox_config(&self) -> crate::sandbox::SandboxConfig {
        crate::sandbox::SandboxConfig {
            timeout_ms: self.config.timeout_ms,
            max_memory: self.config.max_memory,
            file_root: Some(self.config.scripts_dir.join("_sandbox")),
            file_allowed_dirs: Vec::new(),
            allow_network: true,
            ..Default::default()
        }
    }

    fn sandbox_config_from_unified(&self, cfg: &UnifiedConfig) -> crate::sandbox::SandboxConfig {
        let file_root = if cfg.sandbox.file.sandbox_dir.trim().is_empty() {
            self.config.scripts_dir.join("_sandbox")
        } else {
            let p = PathBuf::from(cfg.sandbox.file.sandbox_dir.clone());
            if p.is_absolute() {
                p
            } else {
                self.config.scripts_dir.join(p)
            }
        };

        let allowed_dirs = cfg
            .sandbox
            .file
            .allowed_dirs
            .iter()
            .filter(|s| !s.trim().is_empty())
            .map(PathBuf::from)
            .collect::<Vec<_>>();

        crate::sandbox::SandboxConfig {
            timeout_ms: cfg.sandbox.limits.timeout_ms,
            max_memory: cfg.sandbox.limits.max_memory_bytes,
            file_root: Some(file_root),
            file_allowed_dirs: allowed_dirs,
            allow_network: cfg.sandbox.net.enabled,
            network_timeout_ms: cfg.sandbox.net.timeout_ms,
            max_file_bytes: cfg.sandbox.file.max_bytes,
            max_net_request_bytes: cfg.sandbox.net.max_request_bytes,
            max_net_response_bytes: cfg.sandbox.net.max_response_bytes,
        }
    }

    pub async fn execute_request_script_with_config(
        &self,
        script_name: &str,
        request: &mut RequestData,
        ctx: &ScriptContext,
        cfg: &UnifiedConfig,
    ) -> ScriptExecutionResult {
        let sandbox = self.sandbox_config_from_unified(cfg);
        self.execute_request_script_with_sandbox(script_name, request, ctx, sandbox)
            .await
    }

    pub async fn execute_response_script_with_config(
        &self,
        script_name: &str,
        response: &mut ResponseData,
        ctx: &ScriptContext,
        cfg: &UnifiedConfig,
    ) -> ScriptExecutionResult {
        let sandbox = self.sandbox_config_from_unified(cfg);
        self.execute_response_script_with_sandbox(script_name, response, ctx, sandbox)
            .await
    }

    fn request_scripts_dir(&self) -> PathBuf {
        self.config.scripts_dir.join("request")
    }

    fn response_scripts_dir(&self) -> PathBuf {
        self.config.scripts_dir.join("response")
    }

    fn decode_scripts_dir(&self) -> PathBuf {
        self.config.scripts_dir.join("decode")
    }

    pub async fn init(&self) -> Result<()> {
        let request_dir = self.request_scripts_dir();
        let response_dir = self.response_scripts_dir();
        let decode_dir = self.decode_scripts_dir();
        let sandbox_dir = self.config.scripts_dir.join("_sandbox");

        if !request_dir.exists() {
            std::fs::create_dir_all(&request_dir)?;
            info!("Created request scripts directory: {:?}", request_dir);
        }

        if !response_dir.exists() {
            std::fs::create_dir_all(&response_dir)?;
            info!("Created response scripts directory: {:?}", response_dir);
        }

        if !decode_dir.exists() {
            std::fs::create_dir_all(&decode_dir)?;
            info!("Created decode scripts directory: {:?}", decode_dir);
        }

        if !sandbox_dir.exists() {
            std::fs::create_dir_all(&sandbox_dir)?;
            info!("Created sandbox directory: {:?}", sandbox_dir);
        }

        Ok(())
    }

    fn get_script_path(&self, script_type: ScriptType, name: &str) -> PathBuf {
        let dir = match script_type {
            ScriptType::Request => self.request_scripts_dir(),
            ScriptType::Response => self.response_scripts_dir(),
            ScriptType::Decode => self.decode_scripts_dir(),
        };
        dir.join(format!("{}.js", name))
    }

    pub async fn load_script(&self, script_type: ScriptType, name: &str) -> Result<String> {
        let cache_key = format!("{}:{}", script_type, name);

        {
            let cache = self.script_cache.read().await;
            if let Some(content) = cache.get(&cache_key) {
                return Ok(content.clone());
            }
        }

        let path = self.get_script_path(script_type, name);
        if !path.exists() {
            return Err(ScriptError::NotFound(format!(
                "{} script '{}' not found at {:?}",
                script_type, name, path
            )));
        }

        const MAX_SCRIPT_FILE_BYTES: u64 = 8 * 1024 * 1024;
        if let Ok(meta) = std::fs::metadata(&path) {
            if meta.len() > MAX_SCRIPT_FILE_BYTES {
                return Err(ScriptError::IoError(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!(
                        "script file too large ({} bytes, limit {} bytes)",
                        meta.len(),
                        MAX_SCRIPT_FILE_BYTES
                    ),
                )));
            }
        }
        let content = std::fs::read_to_string(&path)?;

        {
            let mut cache = self.script_cache.write().await;
            cache.insert(cache_key, content.clone());
        }

        Ok(content)
    }

    pub async fn save_script(
        &self,
        script_type: ScriptType,
        name: &str,
        content: &str,
    ) -> Result<()> {
        Self::validate_script_name(name)?;

        let path = self.get_script_path(script_type, name);

        if let Some(parent) = path.parent() {
            if !parent.exists() {
                std::fs::create_dir_all(parent)?;
            }
        }

        std::fs::write(&path, content)?;

        let cache_key = format!("{}:{}", script_type, name);
        let mut cache = self.script_cache.write().await;
        cache.insert(cache_key, content.to_string());

        info!("Saved {} script '{}' to {:?}", script_type, name, path);
        Ok(())
    }

    pub async fn delete_script(&self, script_type: ScriptType, name: &str) -> Result<()> {
        let path = self.get_script_path(script_type, name);
        if !path.exists() {
            return Err(ScriptError::NotFound(format!(
                "{} script '{}' not found",
                script_type, name
            )));
        }

        std::fs::remove_file(&path)?;

        let cache_key = format!("{}:{}", script_type, name);
        let mut cache = self.script_cache.write().await;
        cache.remove(&cache_key);

        info!("Deleted {} script '{}'", script_type, name);
        Ok(())
    }

    pub async fn rename_script(
        &self,
        script_type: ScriptType,
        old_name: &str,
        new_name: &str,
    ) -> Result<()> {
        Self::validate_script_name(new_name)?;

        let old_path = self.get_script_path(script_type, old_name);
        let new_path = self.get_script_path(script_type, new_name);

        if !old_path.exists() {
            return Err(ScriptError::NotFound(format!(
                "{} script '{}' not found",
                script_type, old_name
            )));
        }

        if new_path.exists() {
            return Err(ScriptError::InvalidName(format!(
                "{} script '{}' already exists",
                script_type, new_name
            )));
        }

        if let Some(parent) = new_path.parent() {
            if !parent.exists() {
                std::fs::create_dir_all(parent)?;
            }
        }

        std::fs::rename(&old_path, &new_path)?;

        let old_cache_key = format!("{}:{}", script_type, old_name);
        let new_cache_key = format!("{}:{}", script_type, new_name);
        let mut cache = self.script_cache.write().await;
        let content = cache.remove(&old_cache_key);
        if let Some(content) = content {
            cache.insert(new_cache_key, content);
        }
        drop(cache);

        let type_base_dir = match script_type {
            ScriptType::Request => self.request_scripts_dir(),
            ScriptType::Response => self.response_scripts_dir(),
            ScriptType::Decode => self.decode_scripts_dir(),
        };
        let mut dir = old_path.parent().map(|p| p.to_path_buf());
        while let Some(d) = dir {
            if d == type_base_dir {
                break;
            }
            if d.read_dir()
                .map(|mut rd| rd.next().is_none())
                .unwrap_or(false)
            {
                std::fs::remove_dir(&d)?;
                dir = d.parent().map(|p| p.to_path_buf());
            } else {
                break;
            }
        }

        info!(
            "Renamed {} script '{}' to '{}'",
            script_type, old_name, new_name
        );
        Ok(())
    }

    pub async fn list_scripts(&self, script_type: ScriptType) -> Result<Vec<ScriptInfo>> {
        let dir = match script_type {
            ScriptType::Request => self.request_scripts_dir(),
            ScriptType::Response => self.response_scripts_dir(),
            ScriptType::Decode => self.decode_scripts_dir(),
        };

        if !dir.exists() {
            return Ok(vec![]);
        }

        let mut scripts = Vec::new();
        Self::collect_scripts_recursive(&dir, &dir, script_type, &mut scripts)?;

        scripts.sort_by_key(|s| std::cmp::Reverse(s.updated_at));
        Ok(scripts)
    }

    fn collect_scripts_recursive(
        base_dir: &std::path::Path,
        current_dir: &std::path::Path,
        script_type: ScriptType,
        scripts: &mut Vec<ScriptInfo>,
    ) -> Result<()> {
        for entry in std::fs::read_dir(current_dir)? {
            let entry = entry?;
            let path = entry.path();

            if path.is_dir() {
                Self::collect_scripts_recursive(base_dir, &path, script_type, scripts)?;
            } else if path.extension().and_then(|s| s.to_str()) == Some("js") {
                let relative_path = path
                    .strip_prefix(base_dir)
                    .map_err(|e| ScriptError::IoError(std::io::Error::other(e.to_string())))?;
                let name = relative_path
                    .with_extension("")
                    .to_string_lossy()
                    .replace('\\', "/");

                let metadata = entry.metadata()?;
                let created_at = metadata
                    .created()
                    .map(|t| {
                        t.duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_millis() as u64)
                            .unwrap_or(0)
                    })
                    .unwrap_or(0);
                let updated_at = metadata
                    .modified()
                    .map(|t| {
                        t.duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_millis() as u64)
                            .unwrap_or(0)
                    })
                    .unwrap_or(0);

                scripts.push(ScriptInfo {
                    name,
                    script_type,
                    description: None,
                    created_at,
                    updated_at,
                });
            }
        }
        Ok(())
    }

    pub async fn execute_request_script(
        &self,
        script_name: &str,
        request: &mut RequestData,
        ctx: &ScriptContext,
    ) -> ScriptExecutionResult {
        self.execute_request_script_with_sandbox(
            script_name,
            request,
            ctx,
            self.default_sandbox_config(),
        )
        .await
    }

    pub async fn execute_request_script_with_sandbox(
        &self,
        script_name: &str,
        request: &mut RequestData,
        ctx: &ScriptContext,
        sandbox_config: crate::sandbox::SandboxConfig,
    ) -> ScriptExecutionResult {
        let start = Instant::now();

        let script = match self.load_script(ScriptType::Request, script_name).await {
            Ok(s) => s,
            Err(e) => {
                return ScriptExecutionResult {
                    script_name: script_name.to_string(),
                    script_type: ScriptType::Request,
                    success: false,
                    error: Some(e.to_string()),
                    duration_ms: start.elapsed().as_millis() as u64,
                    logs: vec![],
                    request_modifications: None,
                    response_modifications: None,
                    decode_output: None,
                };
            }
        };

        let timeout_ms = sandbox_config.timeout_ms;
        let request_clone = request.clone();
        let ctx_clone = ctx.clone();
        let script_name_owned = script_name.to_string();

        debug!(
            target: "bifrost::script",
            script_name = %script_name,
            "Executing request script in isolated thread"
        );

        let result = tokio::task::spawn_blocking(move || {
            let mut sandbox = Sandbox::new(sandbox_config)?;
            sandbox.execute_request_script(&script, &request_clone, &ctx_clone)
        })
        .await;

        match result {
            Ok(Ok((modifications, logs))) => {
                if let Some(ref method) = modifications.method {
                    request.method = method.clone();
                }
                if let Some(ref headers) = modifications.headers {
                    request.headers = headers.clone();
                }
                if modifications.body.is_some() {
                    request.body = modifications.body.clone();
                }

                debug!(
                    target: "bifrost::script",
                    script_name = %script_name_owned,
                    duration_ms = start.elapsed().as_millis() as u64,
                    "Request script executed successfully"
                );

                ScriptExecutionResult {
                    script_name: script_name_owned,
                    script_type: ScriptType::Request,
                    success: true,
                    error: None,
                    duration_ms: start.elapsed().as_millis() as u64,
                    logs,
                    request_modifications: None,
                    response_modifications: None,
                    decode_output: None,
                }
            }
            Ok(Err(e)) => {
                let is_timeout = matches!(e, ScriptError::Timeout(_));
                if is_timeout {
                    warn!(
                        target: "bifrost::script",
                        script_name = %script_name_owned,
                        timeout_ms = timeout_ms,
                        "Request script execution timed out"
                    );
                } else {
                    error!(
                        target: "bifrost::script",
                        script_name = %script_name_owned,
                        error = %e,
                        "Request script execution failed"
                    );
                }
                ScriptExecutionResult {
                    script_name: script_name_owned,
                    script_type: ScriptType::Request,
                    success: false,
                    error: Some(e.to_string()),
                    duration_ms: start.elapsed().as_millis() as u64,
                    logs: vec![],
                    request_modifications: None,
                    response_modifications: None,
                    decode_output: None,
                }
            }
            Err(e) => {
                error!(
                    target: "bifrost::script",
                    script_name = %script_name_owned,
                    error = %e,
                    "Script execution thread panicked"
                );
                ScriptExecutionResult {
                    script_name: script_name_owned,
                    script_type: ScriptType::Request,
                    success: false,
                    error: Some(format!("Script execution thread panicked: {}", e)),
                    duration_ms: start.elapsed().as_millis() as u64,
                    logs: vec![],
                    request_modifications: None,
                    response_modifications: None,
                    decode_output: None,
                }
            }
        }
    }

    pub async fn execute_response_script(
        &self,
        script_name: &str,
        response: &mut ResponseData,
        ctx: &ScriptContext,
    ) -> ScriptExecutionResult {
        self.execute_response_script_with_sandbox(
            script_name,
            response,
            ctx,
            self.default_sandbox_config(),
        )
        .await
    }

    pub async fn execute_response_script_with_sandbox(
        &self,
        script_name: &str,
        response: &mut ResponseData,
        ctx: &ScriptContext,
        sandbox_config: crate::sandbox::SandboxConfig,
    ) -> ScriptExecutionResult {
        let start = Instant::now();

        let script = match self.load_script(ScriptType::Response, script_name).await {
            Ok(s) => s,
            Err(e) => {
                return ScriptExecutionResult {
                    script_name: script_name.to_string(),
                    script_type: ScriptType::Response,
                    success: false,
                    error: Some(e.to_string()),
                    duration_ms: start.elapsed().as_millis() as u64,
                    logs: vec![],
                    request_modifications: None,
                    response_modifications: None,
                    decode_output: None,
                };
            }
        };

        let timeout_ms = sandbox_config.timeout_ms;
        let response_clone = response.clone();
        let ctx_clone = ctx.clone();
        let script_name_owned = script_name.to_string();

        debug!(
            target: "bifrost::script",
            script_name = %script_name,
            "Executing response script in isolated thread"
        );

        let result = tokio::task::spawn_blocking(move || {
            let mut sandbox = Sandbox::new(sandbox_config)?;
            sandbox.execute_response_script(&script, &response_clone, &ctx_clone)
        })
        .await;

        match result {
            Ok(Ok((modifications, logs))) => {
                if let Some(status) = modifications.status {
                    response.status = status;
                }
                if let Some(ref status_text) = modifications.status_text {
                    response.status_text = status_text.clone();
                }
                if let Some(ref headers) = modifications.headers {
                    response.headers = headers.clone();
                }
                if modifications.body.is_some() {
                    response.body = modifications.body.clone();
                }

                debug!(
                    target: "bifrost::script",
                    script_name = %script_name_owned,
                    duration_ms = start.elapsed().as_millis() as u64,
                    "Response script executed successfully"
                );

                ScriptExecutionResult {
                    script_name: script_name_owned,
                    script_type: ScriptType::Response,
                    success: true,
                    error: None,
                    duration_ms: start.elapsed().as_millis() as u64,
                    logs,
                    request_modifications: None,
                    response_modifications: None,
                    decode_output: None,
                }
            }
            Ok(Err(e)) => {
                let is_timeout = matches!(e, ScriptError::Timeout(_));
                if is_timeout {
                    warn!(
                        target: "bifrost::script",
                        script_name = %script_name_owned,
                        timeout_ms = timeout_ms,
                        "Response script execution timed out"
                    );
                } else {
                    error!(
                        target: "bifrost::script",
                        script_name = %script_name_owned,
                        error = %e,
                        "Response script execution failed"
                    );
                }
                ScriptExecutionResult {
                    script_name: script_name_owned,
                    script_type: ScriptType::Response,
                    success: false,
                    error: Some(e.to_string()),
                    duration_ms: start.elapsed().as_millis() as u64,
                    logs: vec![],
                    request_modifications: None,
                    response_modifications: None,
                    decode_output: None,
                }
            }
            Err(e) => {
                error!(
                    target: "bifrost::script",
                    script_name = %script_name_owned,
                    error = %e,
                    "Script execution thread panicked"
                );
                ScriptExecutionResult {
                    script_name: script_name_owned,
                    script_type: ScriptType::Response,
                    success: false,
                    error: Some(format!("Script execution thread panicked: {}", e)),
                    duration_ms: start.elapsed().as_millis() as u64,
                    logs: vec![],
                    request_modifications: None,
                    response_modifications: None,
                    decode_output: None,
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn execute_decode_script(
        &self,
        script_name: &str,
        phase: &str,
        request: &RequestData,
        request_body_bytes: &[u8],
        response: &ResponseData,
        response_body_bytes: &[u8],
        ctx: &ScriptContext,
    ) -> std::result::Result<(DecodeOutput, Vec<ScriptLogEntry>), ScriptError> {
        self.execute_decode_script_with_sandbox(
            script_name,
            phase,
            request,
            request_body_bytes,
            response,
            response_body_bytes,
            ctx,
            self.default_sandbox_config(),
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn execute_decode_script_with_config(
        &self,
        script_name: &str,
        phase: &str,
        request: &RequestData,
        request_body_bytes: &[u8],
        response: &ResponseData,
        response_body_bytes: &[u8],
        ctx: &ScriptContext,
        cfg: &UnifiedConfig,
    ) -> std::result::Result<(DecodeOutput, Vec<ScriptLogEntry>), ScriptError> {
        let sandbox = self.sandbox_config_from_unified(cfg);
        self.execute_decode_script_with_sandbox(
            script_name,
            phase,
            request,
            request_body_bytes,
            response,
            response_body_bytes,
            ctx,
            sandbox,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn execute_decode_script_with_sandbox(
        &self,
        script_name: &str,
        phase: &str,
        request: &RequestData,
        request_body_bytes: &[u8],
        response: &ResponseData,
        response_body_bytes: &[u8],
        ctx: &ScriptContext,
        sandbox_config: crate::sandbox::SandboxConfig,
    ) -> std::result::Result<(DecodeOutput, Vec<ScriptLogEntry>), ScriptError> {
        let script = self.load_script(ScriptType::Decode, script_name).await?;

        let phase = phase.to_string();
        let request_clone = request.clone();
        let response_clone = response.clone();
        let req_bytes = request_body_bytes.to_vec();
        let res_bytes = response_body_bytes.to_vec();
        let ctx_clone = ctx.clone();

        let result = tokio::task::spawn_blocking(move || {
            let mut sandbox = Sandbox::new(sandbox_config)?;
            sandbox.execute_decode_script(
                &script,
                &phase,
                &request_clone,
                &req_bytes,
                &response_clone,
                &res_bytes,
                &ctx_clone,
            )
        })
        .await;

        match result {
            Ok(r) => r,
            Err(e) => Err(ScriptError::ExecutionFailed(format!(
                "Script execution thread panicked: {}",
                e
            ))),
        }
    }

    pub async fn test_script(
        &self,
        script_type: ScriptType,
        content: &str,
        request: Option<&RequestData>,
        response: Option<&ResponseData>,
        ctx: &ScriptContext,
    ) -> ScriptExecutionResult {
        self.test_script_with_sandbox(
            script_type,
            content,
            request,
            response,
            ctx,
            self.default_sandbox_config(),
        )
        .await
    }

    pub async fn test_script_with_config(
        &self,
        script_type: ScriptType,
        content: &str,
        request: Option<&RequestData>,
        response: Option<&ResponseData>,
        ctx: &ScriptContext,
        cfg: &UnifiedConfig,
    ) -> ScriptExecutionResult {
        let sandbox = self.sandbox_config_from_unified(cfg);
        self.test_script_with_sandbox(script_type, content, request, response, ctx, sandbox)
            .await
    }

    pub async fn test_script_with_sandbox(
        &self,
        script_type: ScriptType,
        content: &str,
        request: Option<&RequestData>,
        response: Option<&ResponseData>,
        ctx: &ScriptContext,
        sandbox_config: crate::sandbox::SandboxConfig,
    ) -> ScriptExecutionResult {
        let start = Instant::now();

        match script_type {
            ScriptType::Request => {
                let content_owned = content.to_string();
                let ctx_clone = ctx.clone();
                let request_clone = request.cloned().unwrap_or_default();

                let result = tokio::task::spawn_blocking(move || {
                    let mut sandbox = Sandbox::new(sandbox_config)?;

                    sandbox.execute_request_script(&content_owned, &request_clone, &ctx_clone)
                })
                .await;

                match result {
                    Ok(Ok((mods, logs))) => {
                        let request_mods = if mods.method.is_some()
                            || mods.headers.is_some()
                            || mods.body.is_some()
                        {
                            Some(TestRequestModifications {
                                method: mods.method,
                                headers: mods.headers,
                                body: mods.body,
                            })
                        } else {
                            None
                        };
                        ScriptExecutionResult {
                            script_name: "test".to_string(),
                            script_type,
                            success: true,
                            error: None,
                            duration_ms: start.elapsed().as_millis() as u64,
                            logs,
                            request_modifications: request_mods,
                            response_modifications: None,
                            decode_output: None,
                        }
                    }
                    Ok(Err(e)) => ScriptExecutionResult {
                        script_name: "test".to_string(),
                        script_type,
                        success: false,
                        error: Some(e.to_string()),
                        duration_ms: start.elapsed().as_millis() as u64,
                        logs: vec![],
                        request_modifications: None,
                        response_modifications: None,
                        decode_output: None,
                    },
                    Err(e) => ScriptExecutionResult {
                        script_name: "test".to_string(),
                        script_type,
                        success: false,
                        error: Some(format!("Script execution thread panicked: {}", e)),
                        duration_ms: start.elapsed().as_millis() as u64,
                        logs: vec![],
                        request_modifications: None,
                        response_modifications: None,
                        decode_output: None,
                    },
                }
            }
            ScriptType::Response => {
                let content_owned = content.to_string();
                let ctx_clone = ctx.clone();
                let response_clone = response.cloned().unwrap_or_default();

                let result = tokio::task::spawn_blocking(move || {
                    let mut sandbox = Sandbox::new(sandbox_config)?;

                    sandbox.execute_response_script(&content_owned, &response_clone, &ctx_clone)
                })
                .await;

                match result {
                    Ok(Ok((mods, logs))) => {
                        let response_mods = if mods.status.is_some()
                            || mods.status_text.is_some()
                            || mods.headers.is_some()
                            || mods.body.is_some()
                        {
                            Some(TestResponseModifications {
                                status: mods.status,
                                status_text: mods.status_text,
                                headers: mods.headers,
                                body: mods.body,
                            })
                        } else {
                            None
                        };
                        ScriptExecutionResult {
                            script_name: "test".to_string(),
                            script_type,
                            success: true,
                            error: None,
                            duration_ms: start.elapsed().as_millis() as u64,
                            logs,
                            request_modifications: None,
                            response_modifications: response_mods,
                            decode_output: None,
                        }
                    }
                    Ok(Err(e)) => ScriptExecutionResult {
                        script_name: "test".to_string(),
                        script_type,
                        success: false,
                        error: Some(e.to_string()),
                        duration_ms: start.elapsed().as_millis() as u64,
                        logs: vec![],
                        request_modifications: None,
                        response_modifications: None,
                        decode_output: None,
                    },
                    Err(e) => ScriptExecutionResult {
                        script_name: "test".to_string(),
                        script_type,
                        success: false,
                        error: Some(format!("Script execution thread panicked: {}", e)),
                        duration_ms: start.elapsed().as_millis() as u64,
                        logs: vec![],
                        request_modifications: None,
                        response_modifications: None,
                        decode_output: None,
                    },
                }
            }
            ScriptType::Decode => {
                // 说明：decode 脚本需要区分阶段；这里按优先级选择：有 response 就跑 response 阶段，否则跑 request 阶段。
                let content_owned = content.to_string();
                let ctx_clone = ctx.clone();
                let request_clone = request.cloned().unwrap_or_default();
                let response_clone = response.cloned().unwrap_or_default();

                let phase = if response.is_some() {
                    "response"
                } else {
                    "request"
                };
                let req_bytes = request_clone.body.clone().unwrap_or_default().into_bytes();
                let res_bytes = response_clone.body.clone().unwrap_or_default().into_bytes();

                let result = tokio::task::spawn_blocking(move || {
                    let mut sandbox = Sandbox::new(sandbox_config)?;

                    sandbox.execute_decode_script(
                        &content_owned,
                        phase,
                        &request_clone,
                        &req_bytes,
                        &response_clone,
                        &res_bytes,
                        &ctx_clone,
                    )
                })
                .await;

                match result {
                    Ok(Ok((decoded, logs))) => ScriptExecutionResult {
                        script_name: "test".to_string(),
                        script_type,
                        success: true,
                        error: None,
                        duration_ms: start.elapsed().as_millis() as u64,
                        logs,
                        request_modifications: None,
                        response_modifications: None,
                        decode_output: Some(decoded),
                    },
                    Ok(Err(e)) => ScriptExecutionResult {
                        script_name: "test".to_string(),
                        script_type,
                        success: false,
                        error: Some(e.to_string()),
                        duration_ms: start.elapsed().as_millis() as u64,
                        logs: vec![],
                        request_modifications: None,
                        response_modifications: None,
                        decode_output: None,
                    },
                    Err(e) => ScriptExecutionResult {
                        script_name: "test".to_string(),
                        script_type,
                        success: false,
                        error: Some(format!("Script execution thread panicked: {}", e)),
                        duration_ms: start.elapsed().as_millis() as u64,
                        logs: vec![],
                        request_modifications: None,
                        response_modifications: None,
                        decode_output: None,
                    },
                }
            }
        }
    }

    pub async fn invalidate_cache(&self) {
        let mut cache = self.script_cache.write().await;
        cache.clear();
        info!("Script cache invalidated");
    }

    pub async fn invalidate_script_cache(&self, script_type: ScriptType, name: &str) {
        let cache_key = format!("{}:{}", script_type, name);
        let mut cache = self.script_cache.write().await;
        cache.remove(&cache_key);
    }

    fn validate_script_name(name: &str) -> Result<()> {
        if name.is_empty() {
            return Err(ScriptError::InvalidName(
                "Script name cannot be empty".to_string(),
            ));
        }

        if name.len() > 128 {
            return Err(ScriptError::InvalidName(
                "Script name cannot exceed 128 characters".to_string(),
            ));
        }

        if name.starts_with('/') || name.ends_with('/') {
            return Err(ScriptError::InvalidName(
                "Script name cannot start or end with '/'".to_string(),
            ));
        }

        if name.contains("..") {
            return Err(ScriptError::InvalidName(
                "Script name cannot contain '..'".to_string(),
            ));
        }

        if name.contains("//") {
            return Err(ScriptError::InvalidName(
                "Script name cannot contain consecutive slashes".to_string(),
            ));
        }

        let valid_chars = name
            .chars()
            .all(|c| c.is_alphanumeric() || c == '-' || c == '_' || c == '/');
        if !valid_chars {
            return Err(ScriptError::InvalidName(
                "Script name can only contain alphanumeric characters, hyphens, underscores, and slashes"
                    .to_string(),
            ));
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_engine_init() {
        let temp_dir = TempDir::new().unwrap();
        let engine = ScriptEngine::new(ScriptEngineConfig {
            scripts_dir: temp_dir.path().to_path_buf(),
            ..Default::default()
        });

        assert!(engine.init().await.is_ok());
        assert!(temp_dir.path().join("request").exists());
        assert!(temp_dir.path().join("response").exists());
    }

    #[tokio::test]
    async fn test_save_and_load_script() {
        let temp_dir = TempDir::new().unwrap();
        let engine = ScriptEngine::new(ScriptEngineConfig {
            scripts_dir: temp_dir.path().to_path_buf(),
            ..Default::default()
        });
        engine.init().await.unwrap();

        let script_content = r#"log.info("Hello from test script");"#;
        engine
            .save_script(ScriptType::Request, "test-script", script_content)
            .await
            .unwrap();

        let loaded = engine
            .load_script(ScriptType::Request, "test-script")
            .await
            .unwrap();
        assert_eq!(loaded, script_content);
    }

    #[tokio::test]
    async fn test_list_scripts() {
        let temp_dir = TempDir::new().unwrap();
        let engine = ScriptEngine::new(ScriptEngineConfig {
            scripts_dir: temp_dir.path().to_path_buf(),
            ..Default::default()
        });
        engine.init().await.unwrap();

        engine
            .save_script(ScriptType::Request, "script-a", "// A")
            .await
            .unwrap();
        engine
            .save_script(ScriptType::Request, "script-b", "// B")
            .await
            .unwrap();

        let scripts = engine.list_scripts(ScriptType::Request).await.unwrap();
        assert_eq!(scripts.len(), 2);
    }

    #[tokio::test]
    async fn test_decode_test_returns_output() {
        let temp_dir = TempDir::new().unwrap();
        let engine = ScriptEngine::new(ScriptEngineConfig {
            scripts_dir: temp_dir.path().to_path_buf(),
            ..Default::default()
        });
        engine.init().await.unwrap();

        let mut headers = HashMap::new();
        headers.insert("Content-Type".to_string(), "text/plain".to_string());
        let request = RequestData {
            url: "https://example.com/".to_string(),
            method: "GET".to_string(),
            host: "example.com".to_string(),
            path: "/".to_string(),
            protocol: "https".to_string(),
            client_ip: "127.0.0.1".to_string(),
            client_app: None,
            headers,
            body: Some("hello".to_string()),
        };
        let ctx = ScriptContext {
            request_id: "test".to_string(),
            script_name: "test".to_string(),
            script_type: ScriptType::Decode,
            values: HashMap::new(),
            matched_rules: vec![],
        };

        let script = r#"
log.info("decode phase:", ctx.phase);
ctx.output = { code: "0", data: request.body, msg: "" };
"#;

        let result = engine
            .test_script(ScriptType::Decode, script, Some(&request), None, &ctx)
            .await;

        assert!(result.success);
        assert!(result.decode_output.is_some());
        let out = result.decode_output.unwrap();
        assert_eq!(out.code, "0");
        assert_eq!(out.data, "hello");
        assert_eq!(out.msg, "");
        assert!(!result.logs.is_empty());
    }

    #[test]
    fn test_validate_script_name() {
        assert!(ScriptEngine::validate_script_name("valid-name").is_ok());
        assert!(ScriptEngine::validate_script_name("valid_name").is_ok());
        assert!(ScriptEngine::validate_script_name("validName123").is_ok());
        assert!(ScriptEngine::validate_script_name("api/auth/add-token").is_ok());
        assert!(ScriptEngine::validate_script_name("folder/script").is_ok());
        assert!(ScriptEngine::validate_script_name("").is_err());
        assert!(ScriptEngine::validate_script_name("invalid name").is_err());
        assert!(ScriptEngine::validate_script_name("invalid.name").is_err());
        assert!(ScriptEngine::validate_script_name("/leading-slash").is_err());
        assert!(ScriptEngine::validate_script_name("trailing-slash/").is_err());
        assert!(ScriptEngine::validate_script_name("double//slash").is_err());
        assert!(ScriptEngine::validate_script_name("../path-traversal").is_err());
    }

    #[tokio::test]
    async fn test_script_timeout_in_engine() {
        let temp_dir = TempDir::new().unwrap();
        let engine = ScriptEngine::new(ScriptEngineConfig {
            scripts_dir: temp_dir.path().to_path_buf(),
            timeout_ms: 100,
            max_memory: 16 * 1024 * 1024,
        });
        engine.init().await.unwrap();

        let infinite_loop_script = r#"while(true) {}"#;
        engine
            .save_script(ScriptType::Request, "infinite-loop", infinite_loop_script)
            .await
            .unwrap();

        let mut request = RequestData::default();
        let ctx = ScriptContext {
            request_id: "test-timeout".to_string(),
            script_name: "infinite-loop".to_string(),
            script_type: ScriptType::Request,
            values: HashMap::new(),
            matched_rules: vec![],
        };

        let start = std::time::Instant::now();
        let result = engine
            .execute_request_script("infinite-loop", &mut request, &ctx)
            .await;
        let elapsed = start.elapsed();

        assert!(!result.success);
        assert!(result.error.is_some());
        assert!(
            result.error.as_ref().unwrap().contains("timeout"),
            "Error should mention timeout: {:?}",
            result.error
        );
        assert!(
            elapsed.as_millis() < 500,
            "Should timeout within 500ms, took {:?}",
            elapsed
        );
    }
}
