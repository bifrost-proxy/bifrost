use crate::error::{Result, ScriptError};
use crate::sandbox::Sandbox;
use crate::types::*;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::Read;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

use bifrost_storage::UnifiedConfig;

const MAX_SCRIPT_FILE_BYTES: u64 = 8 * 1024 * 1024;

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
            allow_private_network: false,
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
            allow_private_network: cfg.sandbox.net.allow_private_network,
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

    fn parser_scripts_dir(&self) -> PathBuf {
        self.config.scripts_dir.join("parser")
    }

    fn remote_parser_cache_dir(&self) -> PathBuf {
        self.config.scripts_dir.join("_remote-cache").join("parser")
    }

    pub async fn init(&self) -> Result<()> {
        let request_dir = self.request_scripts_dir();
        let response_dir = self.response_scripts_dir();
        let decode_dir = self.decode_scripts_dir();
        let parser_dir = self.parser_scripts_dir();
        let sandbox_dir = self.config.scripts_dir.join("_sandbox");
        let remote_parser_cache_dir = self.remote_parser_cache_dir();

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

        if !parser_dir.exists() {
            std::fs::create_dir_all(&parser_dir)?;
            info!("Created parser scripts directory: {:?}", parser_dir);
        }
        let released_parser_scripts = crate::builtins::release_parser_scripts(&parser_dir)?;

        if !remote_parser_cache_dir.exists() {
            std::fs::create_dir_all(&remote_parser_cache_dir)?;
            info!(
                "Created remote parser cache directory: {:?}",
                remote_parser_cache_dir
            );
        }

        if !sandbox_dir.exists() {
            std::fs::create_dir_all(&sandbox_dir)?;
            info!("Created sandbox directory: {:?}", sandbox_dir);
        }

        if !released_parser_scripts.is_empty() {
            let mut cache = self.script_cache.write().await;
            for name in released_parser_scripts {
                cache.remove(&format!("{}:{}", ScriptType::Parser, name));
            }
        }

        Ok(())
    }

    fn get_script_path(&self, script_type: ScriptType, name: &str) -> PathBuf {
        let dir = match script_type {
            ScriptType::Request => self.request_scripts_dir(),
            ScriptType::Response => self.response_scripts_dir(),
            ScriptType::Decode => self.decode_scripts_dir(),
            ScriptType::Parser => self.parser_scripts_dir(),
        };
        dir.join(format!("{}.js", name))
    }

    fn is_remote_script_ref(script_ref: &str) -> bool {
        reqwest::Url::parse(script_ref)
            .map(|url| matches!(url.scheme(), "https" | "http"))
            .unwrap_or(false)
    }

    fn validate_remote_script_url(script_ref: &str) -> Result<()> {
        let parsed = reqwest::Url::parse(script_ref)
            .map_err(|e| ScriptError::InvalidName(format!("invalid remote parser URL: {e}")))?;
        match parsed.scheme() {
            "https" => Ok(()),
            "http" => {
                let host = parsed.host_str().unwrap_or_default();
                if matches!(host, "127.0.0.1" | "localhost" | "::1") {
                    Ok(())
                } else {
                    Err(ScriptError::InvalidName(
                        "remote bp parser script only allows http for localhost".to_string(),
                    ))
                }
            }
            _ => Err(ScriptError::InvalidName(
                "remote bp parser script URL must use https".to_string(),
            )),
        }
    }

    fn sha256_hex(bytes: &[u8]) -> String {
        let digest = Sha256::digest(bytes);
        digest.iter().map(|b| format!("{b:02x}")).collect()
    }

    fn source_id_for_remote_ref(script_ref: &str) -> String {
        let hash = Self::sha256_hex(script_ref.as_bytes());
        hash[..16].to_string()
    }

    fn expected_sha_from_remote_ref(script_ref: &str) -> Option<String> {
        let parsed = reqwest::Url::parse(script_ref).ok()?;
        if let Some((_, value)) = parsed
            .query_pairs()
            .find(|(key, _)| key == "sha256" || key == "checksum")
        {
            return Some(value.to_string().to_ascii_lowercase());
        }
        let fragment = parsed.fragment()?;
        for part in fragment.split('&') {
            if let Some(value) = part
                .strip_prefix("sha256=")
                .or_else(|| part.strip_prefix("checksum="))
            {
                return Some(value.to_string().to_ascii_lowercase());
            }
        }
        None
    }

    fn remote_download_url(script_ref: &str) -> String {
        if let Ok(mut parsed) = reqwest::Url::parse(script_ref) {
            let filtered: Vec<(String, String)> = parsed
                .query_pairs()
                .filter(|(key, _)| key != "sha256" && key != "checksum")
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect();
            parsed.query_pairs_mut().clear().extend_pairs(filtered);
            parsed.set_fragment(None);
            parsed.to_string()
        } else {
            script_ref.to_string()
        }
    }

    fn local_parser_script_name(script_ref: &str) -> &str {
        let query_pos = script_ref.find('?');
        let fragment_pos = script_ref.find('#');
        let split_at = match (query_pos, fragment_pos) {
            (Some(q), Some(f)) => q.min(f),
            (Some(q), None) => q,
            (None, Some(f)) => f,
            (None, None) => script_ref.len(),
        };
        &script_ref[..split_at]
    }

    fn load_remote_parser_script_blocking(
        &self,
        script_ref: &str,
        network_timeout_ms: u64,
    ) -> Result<String> {
        let expected_sha = Self::expected_sha_from_remote_ref(script_ref).ok_or_else(|| {
            ScriptError::InvalidName(
                "remote bp parser script requires sha256=<hex> in query or fragment".to_string(),
            )
        })?;
        Self::validate_remote_script_url(script_ref)?;
        if expected_sha.len() != 64 || !expected_sha.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(ScriptError::InvalidName(
                "remote bp parser sha256 must be a 64-character hex string".to_string(),
            ));
        }

        let source_id = Self::source_id_for_remote_ref(script_ref);
        let cache_dir = self.remote_parser_cache_dir().join(source_id);
        let cache_path = cache_dir.join(format!("{expected_sha}.js"));
        if cache_path.exists() {
            return Ok(std::fs::read_to_string(cache_path)?);
        }

        std::fs::create_dir_all(&cache_dir)?;
        let download_url = Self::remote_download_url(script_ref);
        let client = bifrost_core::direct_blocking_reqwest_client_builder()
            .timeout(Duration::from_millis(network_timeout_ms))
            .build()
            .map_err(|e| {
                ScriptError::ExecutionFailed(format!("remote parser client init failed: {e}"))
            })?;
        let response = client.get(&download_url).send().map_err(|e| {
            ScriptError::ExecutionFailed(format!("remote parser download failed: {e}"))
        })?;
        if !response.status().is_success() {
            return Err(ScriptError::ExecutionFailed(format!(
                "remote parser download failed with status {}",
                response.status()
            )));
        }
        let bytes = Self::read_remote_parser_body_with_limit(response)?;
        let actual_sha = Self::sha256_hex(&bytes);
        if actual_sha != expected_sha {
            return Err(ScriptError::ExecutionFailed(format!(
                "remote parser sha256 mismatch: expected {expected_sha}, got {actual_sha}"
            )));
        }
        let content = std::str::from_utf8(&bytes)
            .map_err(|e| ScriptError::ExecutionFailed(format!("remote parser is not UTF-8: {e}")))?
            .to_string();
        let tmp_path = cache_dir.join(format!("{expected_sha}.tmp"));
        std::fs::write(&tmp_path, &content)?;
        std::fs::rename(&tmp_path, &cache_path)?;
        Ok(content)
    }

    fn read_remote_parser_body_with_limit(
        response: reqwest::blocking::Response,
    ) -> Result<Vec<u8>> {
        if let Some(len) = response.content_length() {
            if len > MAX_SCRIPT_FILE_BYTES {
                return Err(Self::script_too_large_error(
                    "remote parser script",
                    len,
                    MAX_SCRIPT_FILE_BYTES,
                ));
            }
        }

        let mut bytes = Vec::new();
        let mut limited = response.take(MAX_SCRIPT_FILE_BYTES + 1);
        limited.read_to_end(&mut bytes).map_err(|e| {
            ScriptError::ExecutionFailed(format!("remote parser body read failed: {e}"))
        })?;

        if bytes.len() as u64 > MAX_SCRIPT_FILE_BYTES {
            return Err(Self::script_too_large_error(
                "remote parser script",
                bytes.len() as u64,
                MAX_SCRIPT_FILE_BYTES,
            ));
        }

        Ok(bytes)
    }

    fn script_too_large_error(label: &str, size: u64, limit: u64) -> ScriptError {
        ScriptError::IoError(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("{label} too large ({size} bytes, limit {limit} bytes)"),
        ))
    }

    async fn load_parser_script_ref(
        &self,
        script_ref: &str,
        network_timeout_ms: u64,
    ) -> Result<String> {
        if !Self::is_remote_script_ref(script_ref) {
            return self
                .load_script(
                    ScriptType::Parser,
                    Self::local_parser_script_name(script_ref),
                )
                .await;
        }

        let cache_key = format!("parser-remote:{script_ref}");
        {
            let cache = self.script_cache.read().await;
            if let Some(content) = cache.get(&cache_key) {
                return Ok(content.clone());
            }
        }

        let engine = self.clone_for_blocking();
        let script_ref_owned = script_ref.to_string();
        let content = tokio::task::spawn_blocking(move || {
            engine.load_remote_parser_script_blocking(&script_ref_owned, network_timeout_ms)
        })
        .await
        .map_err(|e| ScriptError::ExecutionFailed(format!("remote parser task failed: {e}")))??;

        let mut cache = self.script_cache.write().await;
        cache.insert(cache_key, content.clone());
        Ok(content)
    }

    fn clone_for_blocking(&self) -> Self {
        Self {
            config: ScriptEngineConfig {
                scripts_dir: self.config.scripts_dir.clone(),
                timeout_ms: self.config.timeout_ms,
                max_memory: self.config.max_memory,
            },
            script_cache: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub async fn load_script(&self, script_type: ScriptType, name: &str) -> Result<String> {
        Self::validate_script_name(name)?;

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

        if let Ok(meta) = std::fs::metadata(&path) {
            if meta.len() > MAX_SCRIPT_FILE_BYTES {
                return Err(Self::script_too_large_error(
                    "script file",
                    meta.len(),
                    MAX_SCRIPT_FILE_BYTES,
                ));
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
        Self::validate_script_name(name)?;

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
        Self::validate_script_name(old_name)?;
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
            ScriptType::Parser => self.parser_scripts_dir(),
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
            ScriptType::Parser => self.parser_scripts_dir(),
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

    #[allow(clippy::too_many_arguments)]
    pub async fn execute_parser_script(
        &self,
        script_ref: &str,
        phase: &str,
        request: &RequestData,
        request_body_bytes: &[u8],
        response: &ResponseData,
        response_body_bytes: &[u8],
        ctx: &ScriptContext,
    ) -> std::result::Result<(DecodeOutput, Vec<ScriptLogEntry>), ScriptError> {
        self.execute_parser_script_with_sandbox(
            script_ref,
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
    pub async fn execute_parser_script_with_config(
        &self,
        script_ref: &str,
        phase: &str,
        request: &RequestData,
        request_body_bytes: &[u8],
        response: &ResponseData,
        response_body_bytes: &[u8],
        ctx: &ScriptContext,
        cfg: &UnifiedConfig,
    ) -> std::result::Result<(DecodeOutput, Vec<ScriptLogEntry>), ScriptError> {
        let sandbox = self.sandbox_config_from_unified(cfg);
        self.execute_parser_script_with_sandbox(
            script_ref,
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
    pub async fn execute_parser_script_with_sandbox(
        &self,
        script_ref: &str,
        phase: &str,
        request: &RequestData,
        request_body_bytes: &[u8],
        response: &ResponseData,
        response_body_bytes: &[u8],
        ctx: &ScriptContext,
        sandbox_config: crate::sandbox::SandboxConfig,
    ) -> std::result::Result<(DecodeOutput, Vec<ScriptLogEntry>), ScriptError> {
        let script = self
            .load_parser_script_ref(script_ref, sandbox_config.network_timeout_ms)
            .await?;

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
                "Parser script execution thread panicked: {}",
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
            ScriptType::Decode | ScriptType::Parser => {
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
mod tests;
