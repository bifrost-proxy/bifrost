use crate::error::{Result, ScriptError};
use crate::types::*;
use base64::Engine as _;
use rquickjs::function::Rest;
use rquickjs::{Context, Ctx, Function, Object, Runtime, Value};
use serde_json::Value as JsonValue;
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;
use tracing::{debug, error, info, warn};

const DEFAULT_NETWORK_TIMEOUT_MS: u64 = 5_000;
const DEFAULT_MAX_FILE_BYTES: usize = 1024 * 1024; // 1MiB
const DEFAULT_MAX_NET_REQUEST_BYTES: usize = 256 * 1024; // 256KiB
const DEFAULT_MAX_NET_RESPONSE_BYTES: usize = 1024 * 1024; // 1MiB

pub struct SandboxConfig {
    pub timeout_ms: u64,
    pub max_memory: usize,
    /// 当为 Some 时，启用 `file.xxx`，并将所有路径限制在该目录下（相对路径）。
    pub file_root: Option<PathBuf>,
    /// 额外允许访问的系统目录（绝对路径）。
    pub file_allowed_dirs: Vec<PathBuf>,
    /// 是否允许脚本发起网络请求（`net.xxx`）。
    pub allow_network: bool,
    pub network_timeout_ms: u64,
    pub max_file_bytes: usize,
    pub max_net_request_bytes: usize,
    pub max_net_response_bytes: usize,
}

fn bytes_to_hex_limited(bytes: &[u8], max_bytes: usize) -> (String, bool) {
    let truncated = bytes.len() > max_bytes;
    let slice = &bytes[..bytes.len().min(max_bytes)];
    let mut out = String::with_capacity(slice.len().saturating_mul(2));
    for b in slice {
        use std::fmt::Write;
        let _ = write!(&mut out, "{:02x}", b);
    }
    (out, truncated)
}

impl Default for SandboxConfig {
    fn default() -> Self {
        Self {
            timeout_ms: 10000,
            max_memory: 32 * 1024 * 1024,
            file_root: None,
            file_allowed_dirs: Vec::new(),
            allow_network: false,
            network_timeout_ms: DEFAULT_NETWORK_TIMEOUT_MS,
            max_file_bytes: DEFAULT_MAX_FILE_BYTES,
            max_net_request_bytes: DEFAULT_MAX_NET_REQUEST_BYTES,
            max_net_response_bytes: DEFAULT_MAX_NET_RESPONSE_BYTES,
        }
    }
}

struct InterruptState {
    start_time: Instant,
    timeout_ms: u64,
    timed_out: AtomicBool,
}

impl InterruptState {
    fn new(timeout_ms: u64) -> Self {
        Self {
            start_time: Instant::now(),
            timeout_ms,
            timed_out: AtomicBool::new(false),
        }
    }

    fn check_timeout(&self) -> bool {
        let elapsed = self.start_time.elapsed().as_millis() as u64;
        if elapsed > self.timeout_ms {
            self.timed_out.store(true, Ordering::SeqCst);
            true
        } else {
            false
        }
    }

    fn is_timed_out(&self) -> bool {
        self.timed_out.load(Ordering::SeqCst)
    }
}

pub struct Sandbox {
    runtime: Runtime,
    context: Context,
    config: SandboxConfig,
    interrupt_state: Arc<InterruptState>,
}

impl Sandbox {
    pub fn new(config: SandboxConfig) -> Result<Self> {
        let runtime = Runtime::new().map_err(|e| ScriptError::QuickJsError(e.to_string()))?;
        runtime.set_memory_limit(config.max_memory);

        if let Some(ref root) = config.file_root {
            std::fs::create_dir_all(root)?;
        }

        let interrupt_state = Arc::new(InterruptState::new(config.timeout_ms));
        let interrupt_state_clone = interrupt_state.clone();

        runtime.set_interrupt_handler(Some(Box::new(move || {
            interrupt_state_clone.check_timeout()
        })));

        let context =
            Context::full(&runtime).map_err(|e| ScriptError::QuickJsError(e.to_string()))?;

        debug!(
            target: "bifrost::script",
            timeout_ms = config.timeout_ms,
            max_memory = config.max_memory,
            "Sandbox created with timeout and memory limits"
        );

        Ok(Self {
            runtime,
            context,
            config,
            interrupt_state,
        })
    }

    fn reset_interrupt_state(&mut self) {
        let new_state = Arc::new(InterruptState::new(self.config.timeout_ms));
        let state_clone = new_state.clone();
        self.runtime
            .set_interrupt_handler(Some(Box::new(move || state_clone.check_timeout())));
        self.interrupt_state = new_state;
    }

    fn check_and_return_timeout_error<T>(&self) -> Option<Result<T>> {
        if self.interrupt_state.is_timed_out() {
            Some(Err(ScriptError::Timeout(self.config.timeout_ms)))
        } else {
            None
        }
    }

    pub fn execute_request_script(
        &mut self,
        script: &str,
        request: &RequestData,
        ctx: &ScriptContext,
    ) -> Result<(RequestModifications, Vec<ScriptLogEntry>)> {
        self.reset_interrupt_state();

        let logs = Rc::new(RefCell::new(Vec::new()));
        let modifications = Rc::new(RefCell::new(RequestModifications::default()));
        let request_headers = Rc::new(RefCell::new(request.headers.clone()));
        let request_body = Rc::new(RefCell::new(request.body.clone()));
        let request_method = Rc::new(RefCell::new(request.method.clone()));

        let eval_result = self.context.with(|js_ctx| {
            self.setup_log_object(
                &js_ctx,
                logs.clone(),
                ctx.script_name.clone(),
                ctx.request_id.clone(),
            )?;
            self.setup_ctx_object(&js_ctx, ctx, "request")?;
            self.setup_request_object(
                &js_ctx,
                request,
                request_headers.clone(),
                request_body.clone(),
                request_method.clone(),
            )?;
            self.setup_file_object(&js_ctx)?;
            self.setup_net_object(&js_ctx)?;
            self.remove_dangerous_globals(&js_ctx)?;

            if let Err(e) = js_ctx.eval::<(), _>(script) {
                let exc = js_ctx.catch();
                let err_detail = if exc.is_exception() || exc.is_error() {
                    if let Some(exc_obj) = exc.as_object() {
                        let msg = exc_obj.get::<_, String>("message").ok().unwrap_or_default();
                        let stack = exc_obj.get::<_, String>("stack").ok().unwrap_or_default();
                        if !msg.is_empty() {
                            if !stack.is_empty() {
                                format!("{}\n{}", msg, stack)
                            } else {
                                msg
                            }
                        } else {
                            format!("{:?}", exc)
                        }
                    } else if let Some(exc_str) = exc.clone().into_string() {
                        exc_str.to_string().unwrap_or_else(|_| format!("{:?}", exc))
                    } else {
                        format!("{:?}", exc)
                    }
                } else {
                    format!("{:?}", exc)
                };
                return Err(ScriptError::ExecutionFailed(format!(
                    "{}: {}",
                    e, err_detail
                )));
            }

            let request_snapshot: String = js_ctx
                .eval("JSON.stringify(request)")
                .map_err(|e| ScriptError::ExecutionFailed(e.to_string()))?;
            if let Ok(snapshot) = serde_json::from_str::<JsonValue>(&request_snapshot) {
                if let Some(obj) = snapshot.as_object() {
                    if let Some(method) = obj.get("method").and_then(|v| v.as_str()) {
                        *request_method.borrow_mut() = method.to_string();
                    }
                    match obj.get("body") {
                        Some(JsonValue::Null) | None => {
                            *request_body.borrow_mut() = None;
                        }
                        Some(value) => {
                            *request_body.borrow_mut() =
                                Some(value.to_string().trim_matches('"').to_string());
                        }
                    }
                    if let Some(headers) = obj.get("headers").and_then(|v| v.as_object()) {
                        let mut new_headers = HashMap::new();
                        for (k, v) in headers {
                            let value = v
                                .as_str()
                                .map(|s| s.to_string())
                                .unwrap_or_else(|| v.to_string());
                            new_headers.insert(k.to_string(), value);
                        }
                        *request_headers.borrow_mut() = new_headers;
                    }
                }
            }

            Ok::<(), ScriptError>(())
        });

        if let Some(timeout_err) = self.check_and_return_timeout_error() {
            warn!(
                target: "bifrost::script",
                script_name = %ctx.script_name,
                timeout_ms = self.config.timeout_ms,
                "Script execution timed out"
            );
            return timeout_err;
        }

        eval_result?;

        let mut mods = modifications.borrow_mut();
        let headers = request_headers.borrow();
        let body = request_body.borrow();
        let method = request_method.borrow();

        if *headers != request.headers {
            mods.headers = Some(headers.clone());
        }
        if *body != request.body {
            mods.body = body.clone();
        }
        if *method != request.method {
            mods.method = Some(method.clone());
        }

        let logs_result = logs.borrow().clone();
        let mods_result = mods.clone();

        Ok((mods_result, logs_result))
    }

    pub fn execute_response_script(
        &mut self,
        script: &str,
        response: &ResponseData,
        ctx: &ScriptContext,
    ) -> Result<(ResponseModifications, Vec<ScriptLogEntry>)> {
        self.reset_interrupt_state();

        let logs = Rc::new(RefCell::new(Vec::new()));
        let modifications = Rc::new(RefCell::new(ResponseModifications::default()));
        let response_headers = Rc::new(RefCell::new(response.headers.clone()));
        let response_body = Rc::new(RefCell::new(response.body.clone()));
        let response_status = Rc::new(RefCell::new(response.status));
        let response_status_text = Rc::new(RefCell::new(response.status_text.clone()));

        let eval_result = self.context.with(|js_ctx| {
            self.setup_log_object(
                &js_ctx,
                logs.clone(),
                ctx.script_name.clone(),
                ctx.request_id.clone(),
            )?;
            self.setup_ctx_object(&js_ctx, ctx, "response")?;
            self.setup_response_object(
                &js_ctx,
                response,
                response_headers.clone(),
                response_body.clone(),
                response_status.clone(),
                response_status_text.clone(),
            )?;
            self.setup_file_object(&js_ctx)?;
            self.setup_net_object(&js_ctx)?;
            self.remove_dangerous_globals(&js_ctx)?;

            if let Err(e) = js_ctx.eval::<(), _>(script) {
                let exc = js_ctx.catch();
                let err_detail = if exc.is_exception() || exc.is_error() {
                    if let Some(exc_obj) = exc.as_object() {
                        let msg = exc_obj.get::<_, String>("message").ok().unwrap_or_default();
                        let stack = exc_obj.get::<_, String>("stack").ok().unwrap_or_default();
                        if !msg.is_empty() {
                            if !stack.is_empty() {
                                format!("{}\n{}", msg, stack)
                            } else {
                                msg
                            }
                        } else {
                            format!("{:?}", exc)
                        }
                    } else if let Some(exc_str) = exc.clone().into_string() {
                        exc_str.to_string().unwrap_or_else(|_| format!("{:?}", exc))
                    } else {
                        format!("{:?}", exc)
                    }
                } else {
                    format!("{:?}", exc)
                };
                return Err(ScriptError::ExecutionFailed(format!(
                    "{}: {}",
                    e, err_detail
                )));
            }

            let response_snapshot: String = js_ctx
                .eval("JSON.stringify(response)")
                .map_err(|e| ScriptError::ExecutionFailed(e.to_string()))?;
            if let Ok(snapshot) = serde_json::from_str::<JsonValue>(&response_snapshot) {
                if let Some(obj) = snapshot.as_object() {
                    if let Some(status) = obj.get("status").and_then(|v| v.as_u64()) {
                        *response_status.borrow_mut() = status as u16;
                    }
                    if let Some(status_text) = obj.get("statusText").and_then(|v| v.as_str()) {
                        *response_status_text.borrow_mut() = status_text.to_string();
                    }
                    match obj.get("body") {
                        Some(JsonValue::Null) | None => {
                            *response_body.borrow_mut() = None;
                        }
                        Some(value) => {
                            *response_body.borrow_mut() =
                                Some(value.to_string().trim_matches('"').to_string());
                        }
                    }
                    if let Some(headers) = obj.get("headers").and_then(|v| v.as_object()) {
                        let mut new_headers = HashMap::new();
                        for (k, v) in headers {
                            let value = v
                                .as_str()
                                .map(|s| s.to_string())
                                .unwrap_or_else(|| v.to_string());
                            new_headers.insert(k.to_string(), value);
                        }
                        *response_headers.borrow_mut() = new_headers;
                    }
                }
            }

            Ok::<(), ScriptError>(())
        });

        if let Some(timeout_err) = self.check_and_return_timeout_error() {
            warn!(
                target: "bifrost::script",
                script_name = %ctx.script_name,
                timeout_ms = self.config.timeout_ms,
                "Script execution timed out"
            );
            return timeout_err;
        }

        eval_result?;

        let mut mods = modifications.borrow_mut();
        let headers = response_headers.borrow();
        let body = response_body.borrow();
        let status = *response_status.borrow();
        let status_text = response_status_text.borrow();

        if *headers != response.headers {
            mods.headers = Some(headers.clone());
        }
        if *body != response.body {
            mods.body = body.clone();
        }
        if status != response.status {
            mods.status = Some(status);
        }
        if *status_text != response.status_text {
            mods.status_text = Some(status_text.clone());
        }

        let logs_result = logs.borrow().clone();
        let mods_result = mods.clone();

        Ok((mods_result, logs_result))
    }

    #[allow(clippy::too_many_arguments)]
    pub fn execute_decode_script(
        &mut self,
        script: &str,
        phase: &str,
        request: &RequestData,
        request_body_bytes: &[u8],
        response: &ResponseData,
        response_body_bytes: &[u8],
        ctx: &ScriptContext,
    ) -> Result<(DecodeOutput, Vec<ScriptLogEntry>)> {
        self.reset_interrupt_state();

        // 为避免把大体积二进制直接塞进 JS（尤其是 hex），这里对展示型字段做截断。
        const MAX_HEX_BYTES: usize = 256 * 1024;

        let logs = Rc::new(RefCell::new(Vec::new()));

        let eval_result = self.context.with(|js_ctx| {
            self.setup_log_object(
                &js_ctx,
                logs.clone(),
                ctx.script_name.clone(),
                ctx.request_id.clone(),
            )?;
            self.setup_ctx_object(&js_ctx, ctx, phase)?;

            if phase.eq_ignore_ascii_case("response") {
                // 响应阶段：仅提供 response（并在 response.request 中携带 request 快照）
                // 为兼容性，额外把 request 快照挂到全局 request（不包含二进制 body）
                self.setup_decode_request_snapshot_object(&js_ctx, &response.request)?;
                self.setup_decode_response_object(
                    &js_ctx,
                    response,
                    response_body_bytes,
                    MAX_HEX_BYTES,
                )?;
            } else {
                // 请求阶段：仅提供 request
                self.setup_decode_request_object(
                    &js_ctx,
                    request,
                    request_body_bytes,
                    MAX_HEX_BYTES,
                )?;
                js_ctx
                    .globals()
                    .set("response", Value::new_null(js_ctx.clone()))
                    .map_err(|e| ScriptError::QuickJsError(e.to_string()))?;
            }

            self.setup_file_object(&js_ctx)?;
            self.setup_net_object(&js_ctx)?;
            self.remove_dangerous_globals(&js_ctx)?;

            let out_val = match js_ctx.eval::<Value, _>(script) {
                Ok(v) => v,
                Err(e) => {
                    let exc = js_ctx.catch();
                    let err_detail = if exc.is_exception() || exc.is_error() {
                        if let Some(exc_obj) = exc.as_object() {
                            let msg = exc_obj.get::<_, String>("message").ok().unwrap_or_default();
                            let stack = exc_obj.get::<_, String>("stack").ok().unwrap_or_default();
                            if !msg.is_empty() {
                                if !stack.is_empty() {
                                    format!("{}\n{}", msg, stack)
                                } else {
                                    msg
                                }
                            } else {
                                format!("{:?}", exc)
                            }
                        } else if let Some(exc_str) = exc.clone().into_string() {
                            exc_str.to_string().unwrap_or_else(|_| format!("{:?}", exc))
                        } else {
                            format!("{:?}", exc)
                        }
                    } else {
                        format!("{:?}", exc)
                    };
                    return Err(ScriptError::ExecutionFailed(format!(
                        "{}: {}",
                        e, err_detail
                    )));
                }
            };

            let output = self.parse_decode_output_from_js(&js_ctx, out_val)?;
            Ok::<DecodeOutput, ScriptError>(output)
        });

        if let Some(timeout_err) = self.check_and_return_timeout_error() {
            warn!(
                target: "bifrost::script",
                script_name = %ctx.script_name,
                timeout_ms = self.config.timeout_ms,
                "Script execution timed out"
            );
            return timeout_err;
        }

        let output = eval_result?;
        let logs_result = logs.borrow().clone();
        Ok((output, logs_result))
    }

    fn setup_log_object(
        &self,
        js_ctx: &Ctx,
        logs: Rc<RefCell<Vec<ScriptLogEntry>>>,
        script_name: String,
        request_id: String,
    ) -> Result<()> {
        let log =
            Object::new(js_ctx.clone()).map_err(|e| ScriptError::QuickJsError(e.to_string()))?;

        for level in ["log", "debug", "info", "warn", "error"] {
            let logs_clone = logs.clone();
            let script_name_clone = script_name.clone();
            let request_id_clone = request_id.clone();
            let level_enum = match level {
                "log" => ScriptLogLevel::Info,
                "debug" => ScriptLogLevel::Debug,
                "info" => ScriptLogLevel::Info,
                "warn" => ScriptLogLevel::Warn,
                "error" => ScriptLogLevel::Error,
                _ => unreachable!(),
            };

            let func = Function::new(js_ctx.clone(), move |args: Rest<Value>| {
                let message = args
                    .0
                    .iter()
                    .map(|v| {
                        if let Some(s) = v.as_string() {
                            if let Ok(s) = s.to_string() {
                                return s;
                            }
                        }
                        format!("{:?}", v)
                    })
                    .collect::<Vec<_>>()
                    .join(" ");

                match level_enum {
                    ScriptLogLevel::Debug => {
                        debug!(
                            target: "bifrost::script",
                            script = %script_name_clone,
                            request_id = %request_id_clone,
                            "{}",
                            message
                        );
                    }
                    ScriptLogLevel::Info => {
                        info!(
                            target: "bifrost::script",
                            script = %script_name_clone,
                            request_id = %request_id_clone,
                            "{}",
                            message
                        );
                    }
                    ScriptLogLevel::Warn => {
                        warn!(
                            target: "bifrost::script",
                            script = %script_name_clone,
                            request_id = %request_id_clone,
                            "{}",
                            message
                        );
                    }
                    ScriptLogLevel::Error => {
                        error!(
                            target: "bifrost::script",
                            script = %script_name_clone,
                            request_id = %request_id_clone,
                            "{}",
                            message
                        );
                    }
                }

                logs_clone.borrow_mut().push(ScriptLogEntry {
                    timestamp: chrono::Utc::now().timestamp_millis() as u64,
                    level: level_enum,
                    message,
                    args: None,
                });
            })
            .map_err(|e| ScriptError::QuickJsError(e.to_string()))?;

            log.set(level, func)
                .map_err(|e| ScriptError::QuickJsError(e.to_string()))?;
        }

        let globals = js_ctx.globals();
        globals
            .set("log", log.clone())
            .map_err(|e| ScriptError::QuickJsError(e.to_string()))?;
        globals
            .set("console", log)
            .map_err(|e| ScriptError::QuickJsError(e.to_string()))?;

        Ok(())
    }

    fn setup_ctx_object(
        &self,
        js_ctx: &Ctx,
        script_ctx: &ScriptContext,
        phase: &str,
    ) -> Result<()> {
        let ctx_obj =
            Object::new(js_ctx.clone()).map_err(|e| ScriptError::QuickJsError(e.to_string()))?;

        ctx_obj
            .set("requestId", script_ctx.request_id.clone())
            .map_err(|e| ScriptError::QuickJsError(e.to_string()))?;
        ctx_obj
            .set("scriptName", script_ctx.script_name.clone())
            .map_err(|e| ScriptError::QuickJsError(e.to_string()))?;
        ctx_obj
            .set("scriptType", script_ctx.script_type.to_string())
            .map_err(|e| ScriptError::QuickJsError(e.to_string()))?;

        ctx_obj
            .set("phase", phase.to_string())
            .map_err(|e| ScriptError::QuickJsError(e.to_string()))?;

        let values_obj =
            Object::new(js_ctx.clone()).map_err(|e| ScriptError::QuickJsError(e.to_string()))?;
        for (k, v) in &script_ctx.values {
            values_obj
                .set(k.as_str(), v.clone())
                .map_err(|e| ScriptError::QuickJsError(e.to_string()))?;
        }
        ctx_obj
            .set("values", values_obj)
            .map_err(|e| ScriptError::QuickJsError(e.to_string()))?;

        let rules_array = rquickjs::Array::new(js_ctx.clone())
            .map_err(|e| ScriptError::QuickJsError(e.to_string()))?;
        for (i, rule) in script_ctx.matched_rules.iter().enumerate() {
            let rule_obj = Object::new(js_ctx.clone())
                .map_err(|e| ScriptError::QuickJsError(e.to_string()))?;
            rule_obj
                .set("pattern", rule.pattern.clone())
                .map_err(|e| ScriptError::QuickJsError(e.to_string()))?;
            rule_obj
                .set("protocol", rule.protocol.clone())
                .map_err(|e| ScriptError::QuickJsError(e.to_string()))?;
            rule_obj
                .set("value", rule.value.clone())
                .map_err(|e| ScriptError::QuickJsError(e.to_string()))?;
            rules_array
                .set(i, rule_obj)
                .map_err(|e| ScriptError::QuickJsError(e.to_string()))?;
        }
        ctx_obj
            .set("matchedRules", rules_array)
            .map_err(|e| ScriptError::QuickJsError(e.to_string()))?;

        let globals = js_ctx.globals();
        globals
            .set("ctx", ctx_obj)
            .map_err(|e| ScriptError::QuickJsError(e.to_string()))?;

        Ok(())
    }

    fn setup_request_object(
        &self,
        js_ctx: &Ctx,
        request: &RequestData,
        _headers: Rc<RefCell<HashMap<String, String>>>,
        _body: Rc<RefCell<Option<String>>>,
        _method: Rc<RefCell<String>>,
    ) -> Result<()> {
        let req =
            Object::new(js_ctx.clone()).map_err(|e| ScriptError::QuickJsError(e.to_string()))?;

        req.set("url", request.url.clone())
            .map_err(|e| ScriptError::QuickJsError(e.to_string()))?;
        req.set("host", request.host.clone())
            .map_err(|e| ScriptError::QuickJsError(e.to_string()))?;
        req.set("path", request.path.clone())
            .map_err(|e| ScriptError::QuickJsError(e.to_string()))?;
        req.set("protocol", request.protocol.clone())
            .map_err(|e| ScriptError::QuickJsError(e.to_string()))?;
        req.set("clientIp", request.client_ip.clone())
            .map_err(|e| ScriptError::QuickJsError(e.to_string()))?;
        req.set("clientApp", request.client_app.clone())
            .map_err(|e| ScriptError::QuickJsError(e.to_string()))?;
        req.set("method", request.method.clone())
            .map_err(|e| ScriptError::QuickJsError(e.to_string()))?;

        let headers_obj =
            Object::new(js_ctx.clone()).map_err(|e| ScriptError::QuickJsError(e.to_string()))?;
        for (k, v) in &request.headers {
            headers_obj
                .set(k.as_str(), v.clone())
                .map_err(|e| ScriptError::QuickJsError(e.to_string()))?;
        }
        req.set("headers", headers_obj)
            .map_err(|e| ScriptError::QuickJsError(e.to_string()))?;

        if let Some(b) = &request.body {
            req.set("body", b.clone())
                .map_err(|e| ScriptError::QuickJsError(e.to_string()))?;
        } else {
            req.set("body", Value::new_null(js_ctx.clone()))
                .map_err(|e| ScriptError::QuickJsError(e.to_string()))?;
        }

        let globals = js_ctx.globals();
        globals
            .set("request", req)
            .map_err(|e| ScriptError::QuickJsError(e.to_string()))?;

        Ok(())
    }

    fn setup_response_object(
        &self,
        js_ctx: &Ctx,
        response: &ResponseData,
        _headers: Rc<RefCell<HashMap<String, String>>>,
        _body: Rc<RefCell<Option<String>>>,
        _status: Rc<RefCell<u16>>,
        _status_text: Rc<RefCell<String>>,
    ) -> Result<()> {
        let res =
            Object::new(js_ctx.clone()).map_err(|e| ScriptError::QuickJsError(e.to_string()))?;

        res.set("status", response.status)
            .map_err(|e| ScriptError::QuickJsError(e.to_string()))?;
        res.set("statusText", response.status_text.clone())
            .map_err(|e| ScriptError::QuickJsError(e.to_string()))?;

        let headers_obj =
            Object::new(js_ctx.clone()).map_err(|e| ScriptError::QuickJsError(e.to_string()))?;
        for (k, v) in &response.headers {
            headers_obj
                .set(k.as_str(), v.clone())
                .map_err(|e| ScriptError::QuickJsError(e.to_string()))?;
        }
        res.set("headers", headers_obj)
            .map_err(|e| ScriptError::QuickJsError(e.to_string()))?;

        if let Some(b) = &response.body {
            res.set("body", b.clone())
                .map_err(|e| ScriptError::QuickJsError(e.to_string()))?;
        } else {
            res.set("body", Value::new_null(js_ctx.clone()))
                .map_err(|e| ScriptError::QuickJsError(e.to_string()))?;
        }

        let req_obj =
            Object::new(js_ctx.clone()).map_err(|e| ScriptError::QuickJsError(e.to_string()))?;
        req_obj
            .set("url", response.request.url.clone())
            .map_err(|e| ScriptError::QuickJsError(e.to_string()))?;
        req_obj
            .set("method", response.request.method.clone())
            .map_err(|e| ScriptError::QuickJsError(e.to_string()))?;
        req_obj
            .set("host", response.request.host.clone())
            .map_err(|e| ScriptError::QuickJsError(e.to_string()))?;
        req_obj
            .set("path", response.request.path.clone())
            .map_err(|e| ScriptError::QuickJsError(e.to_string()))?;

        let req_headers_obj =
            Object::new(js_ctx.clone()).map_err(|e| ScriptError::QuickJsError(e.to_string()))?;
        for (k, v) in &response.request.headers {
            req_headers_obj
                .set(k.as_str(), v.clone())
                .map_err(|e| ScriptError::QuickJsError(e.to_string()))?;
        }
        req_obj
            .set("headers", req_headers_obj)
            .map_err(|e| ScriptError::QuickJsError(e.to_string()))?;

        res.set("request", req_obj)
            .map_err(|e| ScriptError::QuickJsError(e.to_string()))?;

        let globals = js_ctx.globals();
        globals
            .set("response", res)
            .map_err(|e| ScriptError::QuickJsError(e.to_string()))?;

        Ok(())
    }

    fn setup_decode_request_object(
        &self,
        js_ctx: &Ctx,
        request: &RequestData,
        body_bytes: &[u8],
        max_inline_bytes: usize,
    ) -> Result<()> {
        let req =
            Object::new(js_ctx.clone()).map_err(|e| ScriptError::QuickJsError(e.to_string()))?;

        req.set("url", request.url.clone())
            .map_err(|e| ScriptError::QuickJsError(e.to_string()))?;
        req.set("host", request.host.clone())
            .map_err(|e| ScriptError::QuickJsError(e.to_string()))?;
        req.set("path", request.path.clone())
            .map_err(|e| ScriptError::QuickJsError(e.to_string()))?;
        req.set("protocol", request.protocol.clone())
            .map_err(|e| ScriptError::QuickJsError(e.to_string()))?;
        req.set("clientIp", request.client_ip.clone())
            .map_err(|e| ScriptError::QuickJsError(e.to_string()))?;
        req.set("clientApp", request.client_app.clone())
            .map_err(|e| ScriptError::QuickJsError(e.to_string()))?;
        req.set("method", request.method.clone())
            .map_err(|e| ScriptError::QuickJsError(e.to_string()))?;

        let headers_obj =
            Object::new(js_ctx.clone()).map_err(|e| ScriptError::QuickJsError(e.to_string()))?;
        for (k, v) in &request.headers {
            headers_obj
                .set(k.as_str(), v.clone())
                .map_err(|e| ScriptError::QuickJsError(e.to_string()))?;
        }
        req.set("headers", headers_obj)
            .map_err(|e| ScriptError::QuickJsError(e.to_string()))?;

        let (body_hex, body_hex_truncated) = bytes_to_hex_limited(body_bytes, max_inline_bytes);
        let body_text =
            String::from_utf8_lossy(&body_bytes[..body_bytes.len().min(max_inline_bytes)])
                .to_string();
        let body_text_truncated = body_bytes.len() > max_inline_bytes;

        req.set("body", body_text)
            .map_err(|e| ScriptError::QuickJsError(e.to_string()))?;
        req.set("bodySize", body_bytes.len() as u64)
            .map_err(|e| ScriptError::QuickJsError(e.to_string()))?;
        req.set("bodyHex", body_hex)
            .map_err(|e| ScriptError::QuickJsError(e.to_string()))?;
        req.set(
            "bodyBase64",
            base64::engine::general_purpose::STANDARD.encode(body_bytes),
        )
        .map_err(|e| ScriptError::QuickJsError(e.to_string()))?;
        req.set("bodyHexTruncated", body_hex_truncated)
            .map_err(|e| ScriptError::QuickJsError(e.to_string()))?;
        req.set("bodyTextTruncated", body_text_truncated)
            .map_err(|e| ScriptError::QuickJsError(e.to_string()))?;

        let globals = js_ctx.globals();
        globals
            .set("request", req)
            .map_err(|e| ScriptError::QuickJsError(e.to_string()))?;

        Ok(())
    }

    fn setup_decode_request_snapshot_object(
        &self,
        js_ctx: &Ctx,
        request: &RequestData,
    ) -> Result<()> {
        let req =
            Object::new(js_ctx.clone()).map_err(|e| ScriptError::QuickJsError(e.to_string()))?;

        req.set("url", request.url.clone())
            .map_err(|e| ScriptError::QuickJsError(e.to_string()))?;
        req.set("host", request.host.clone())
            .map_err(|e| ScriptError::QuickJsError(e.to_string()))?;
        req.set("path", request.path.clone())
            .map_err(|e| ScriptError::QuickJsError(e.to_string()))?;
        req.set("protocol", request.protocol.clone())
            .map_err(|e| ScriptError::QuickJsError(e.to_string()))?;
        req.set("clientIp", request.client_ip.clone())
            .map_err(|e| ScriptError::QuickJsError(e.to_string()))?;
        req.set("clientApp", request.client_app.clone())
            .map_err(|e| ScriptError::QuickJsError(e.to_string()))?;
        req.set("method", request.method.clone())
            .map_err(|e| ScriptError::QuickJsError(e.to_string()))?;

        let headers_obj =
            Object::new(js_ctx.clone()).map_err(|e| ScriptError::QuickJsError(e.to_string()))?;
        for (k, v) in &request.headers {
            headers_obj
                .set(k.as_str(), v.clone())
                .map_err(|e| ScriptError::QuickJsError(e.to_string()))?;
        }
        req.set("headers", headers_obj)
            .map_err(|e| ScriptError::QuickJsError(e.to_string()))?;

        req.set("body", Value::new_null(js_ctx.clone()))
            .map_err(|e| ScriptError::QuickJsError(e.to_string()))?;
        req.set("bodySize", 0u64)
            .map_err(|e| ScriptError::QuickJsError(e.to_string()))?;
        req.set("bodyHex", "".to_string())
            .map_err(|e| ScriptError::QuickJsError(e.to_string()))?;
        req.set("bodyBase64", "".to_string())
            .map_err(|e| ScriptError::QuickJsError(e.to_string()))?;
        req.set("bodyHexTruncated", false)
            .map_err(|e| ScriptError::QuickJsError(e.to_string()))?;
        req.set("bodyTextTruncated", false)
            .map_err(|e| ScriptError::QuickJsError(e.to_string()))?;

        let globals = js_ctx.globals();
        globals
            .set("request", req)
            .map_err(|e| ScriptError::QuickJsError(e.to_string()))?;

        Ok(())
    }

    fn setup_decode_response_object(
        &self,
        js_ctx: &Ctx,
        response: &ResponseData,
        body_bytes: &[u8],
        max_inline_bytes: usize,
    ) -> Result<()> {
        let res =
            Object::new(js_ctx.clone()).map_err(|e| ScriptError::QuickJsError(e.to_string()))?;

        res.set("status", response.status)
            .map_err(|e| ScriptError::QuickJsError(e.to_string()))?;
        res.set("statusText", response.status_text.clone())
            .map_err(|e| ScriptError::QuickJsError(e.to_string()))?;

        let headers_obj =
            Object::new(js_ctx.clone()).map_err(|e| ScriptError::QuickJsError(e.to_string()))?;
        for (k, v) in &response.headers {
            headers_obj
                .set(k.as_str(), v.clone())
                .map_err(|e| ScriptError::QuickJsError(e.to_string()))?;
        }
        res.set("headers", headers_obj)
            .map_err(|e| ScriptError::QuickJsError(e.to_string()))?;

        let (body_hex, body_hex_truncated) = bytes_to_hex_limited(body_bytes, max_inline_bytes);
        let body_text =
            String::from_utf8_lossy(&body_bytes[..body_bytes.len().min(max_inline_bytes)])
                .to_string();
        let body_text_truncated = body_bytes.len() > max_inline_bytes;

        res.set("body", body_text)
            .map_err(|e| ScriptError::QuickJsError(e.to_string()))?;
        res.set("bodySize", body_bytes.len() as u64)
            .map_err(|e| ScriptError::QuickJsError(e.to_string()))?;
        res.set("bodyHex", body_hex)
            .map_err(|e| ScriptError::QuickJsError(e.to_string()))?;
        res.set(
            "bodyBase64",
            base64::engine::general_purpose::STANDARD.encode(body_bytes),
        )
        .map_err(|e| ScriptError::QuickJsError(e.to_string()))?;
        res.set("bodyHexTruncated", body_hex_truncated)
            .map_err(|e| ScriptError::QuickJsError(e.to_string()))?;
        res.set("bodyTextTruncated", body_text_truncated)
            .map_err(|e| ScriptError::QuickJsError(e.to_string()))?;

        let req_obj =
            Object::new(js_ctx.clone()).map_err(|e| ScriptError::QuickJsError(e.to_string()))?;
        req_obj
            .set("url", response.request.url.clone())
            .map_err(|e| ScriptError::QuickJsError(e.to_string()))?;
        req_obj
            .set("method", response.request.method.clone())
            .map_err(|e| ScriptError::QuickJsError(e.to_string()))?;
        req_obj
            .set("host", response.request.host.clone())
            .map_err(|e| ScriptError::QuickJsError(e.to_string()))?;
        req_obj
            .set("path", response.request.path.clone())
            .map_err(|e| ScriptError::QuickJsError(e.to_string()))?;
        req_obj
            .set("protocol", response.request.protocol.clone())
            .map_err(|e| ScriptError::QuickJsError(e.to_string()))?;
        req_obj
            .set("clientIp", response.request.client_ip.clone())
            .map_err(|e| ScriptError::QuickJsError(e.to_string()))?;
        req_obj
            .set("clientApp", response.request.client_app.clone())
            .map_err(|e| ScriptError::QuickJsError(e.to_string()))?;

        let req_headers_obj =
            Object::new(js_ctx.clone()).map_err(|e| ScriptError::QuickJsError(e.to_string()))?;
        for (k, v) in &response.request.headers {
            req_headers_obj
                .set(k.as_str(), v.clone())
                .map_err(|e| ScriptError::QuickJsError(e.to_string()))?;
        }
        req_obj
            .set("headers", req_headers_obj)
            .map_err(|e| ScriptError::QuickJsError(e.to_string()))?;

        res.set("request", req_obj)
            .map_err(|e| ScriptError::QuickJsError(e.to_string()))?;

        let globals = js_ctx.globals();
        globals
            .set("response", res)
            .map_err(|e| ScriptError::QuickJsError(e.to_string()))?;

        Ok(())
    }

    fn parse_decode_output_from_js<'js>(
        &self,
        js_ctx: &Ctx<'js>,
        out_val: Value<'js>,
    ) -> Result<DecodeOutput> {
        let mut v = out_val;
        if v.is_undefined() || v.is_null() {
            if let Ok(v2) = js_ctx.eval::<Value, _>("ctx.output") {
                if !v2.is_undefined() && !v2.is_null() {
                    v = v2;
                }
            }
        }
        if v.is_undefined() || v.is_null() {
            if let Ok(v2) = js_ctx.eval::<Value, _>("output") {
                if !v2.is_undefined() && !v2.is_null() {
                    v = v2;
                }
            }
        }

        if v.is_undefined() || v.is_null() {
            return Err(ScriptError::ExecutionFailed(
                "decode 脚本未输出结果：请返回 {data,code,msg} 或设置 ctx.output/output"
                    .to_string(),
            ));
        }

        // 允许直接返回 JSON 字符串
        let json_text = if let Some(s) = v.clone().into_string() {
            s.to_string()
                .map_err(|e| ScriptError::ExecutionFailed(e.to_string()))?
        } else {
            let globals = js_ctx.globals();
            globals
                .set("__bifrost_decode_output", v)
                .map_err(|e| ScriptError::QuickJsError(e.to_string()))?;
            js_ctx
                .eval::<String, _>("JSON.stringify(__bifrost_decode_output)")
                .map_err(|e| ScriptError::ExecutionFailed(e.to_string()))?
        };

        let parsed: JsonValue = serde_json::from_str(&json_text).map_err(|e| {
            ScriptError::ExecutionFailed(format!("decode 输出不是有效 JSON: {}", e))
        })?;
        let obj = parsed.as_object().ok_or_else(|| {
            ScriptError::ExecutionFailed("decode 输出必须是 JSON object".to_string())
        })?;

        let code = obj
            .get("code")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                ScriptError::ExecutionFailed("decode 输出缺少字符串字段 code".to_string())
            })?
            .to_string();
        let data = obj
            .get("data")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let msg = obj
            .get("msg")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        Ok(DecodeOutput { data, code, msg })
    }

    fn remove_dangerous_globals(&self, js_ctx: &Ctx) -> Result<()> {
        let globals = js_ctx.globals();

        let dangerous = ["eval", "Function"];
        for name in dangerous {
            let _ = globals.remove(name);
        }

        Ok(())
    }

    fn setup_file_object(&self, js_ctx: &Ctx) -> Result<()> {
        let file_obj =
            Object::new(js_ctx.clone()).map_err(|e| ScriptError::QuickJsError(e.to_string()))?;

        let enabled = self.config.file_root.is_some() || !self.config.file_allowed_dirs.is_empty();
        file_obj
            .set("enabled", enabled)
            .map_err(|e| ScriptError::QuickJsError(e.to_string()))?;

        let file_root = self.config.file_root.clone();
        let file_allowed_dirs = self.config.file_allowed_dirs.clone();
        let max_file_bytes = self.config.max_file_bytes;

        let read_text = Function::new(
            js_ctx.clone(),
            move |path: String| -> rquickjs::Result<String> {
                let full =
                    resolve_file_path(file_root.as_deref(), &file_allowed_dirs, &path, false)
                        .map_err(|e| js_err("file", "readText", e))?;
                let metadata =
                    std::fs::metadata(&full).map_err(|e| js_err("file", "readText", e))?;
                let size = metadata.len() as usize;
                if size > max_file_bytes {
                    return Err(rquickjs::Error::new_from_js_message(
                        "file",
                        "readText",
                        format!("文件过大: {} bytes (limit {})", size, max_file_bytes),
                    ));
                }
                std::fs::read_to_string(&full).map_err(|e| js_err("file", "readText", e))
            },
        );
        file_obj
            .set("readText", read_text)
            .map_err(|e| ScriptError::QuickJsError(e.to_string()))?;

        let file_root = self.config.file_root.clone();
        let file_allowed_dirs = self.config.file_allowed_dirs.clone();
        let write_text = Function::new(
            js_ctx.clone(),
            move |path: String, content: String| -> rquickjs::Result<bool> {
                if content.len() > max_file_bytes {
                    return Err(rquickjs::Error::new_from_js_message(
                        "file",
                        "writeText",
                        format!(
                            "写入内容过大: {} bytes (limit {})",
                            content.len(),
                            max_file_bytes
                        ),
                    ));
                }
                let full = resolve_file_path(file_root.as_deref(), &file_allowed_dirs, &path, true)
                    .map_err(|e| js_err("file", "writeText", e))?;
                if let Some(parent) = full.parent() {
                    std::fs::create_dir_all(parent).map_err(|e| js_err("file", "writeText", e))?;
                }
                std::fs::write(&full, content).map_err(|e| js_err("file", "writeText", e))?;
                Ok(true)
            },
        );
        file_obj
            .set("writeText", write_text)
            .map_err(|e| ScriptError::QuickJsError(e.to_string()))?;

        let file_root = self.config.file_root.clone();
        let file_allowed_dirs = self.config.file_allowed_dirs.clone();
        let append_text = Function::new(
            js_ctx.clone(),
            move |path: String, content: String| -> rquickjs::Result<bool> {
                use std::io::Write;

                if content.len() > max_file_bytes {
                    return Err(rquickjs::Error::new_from_js_message(
                        "file",
                        "appendText",
                        format!(
                            "追加内容过大: {} bytes (limit {})",
                            content.len(),
                            max_file_bytes
                        ),
                    ));
                }

                let full = resolve_file_path(file_root.as_deref(), &file_allowed_dirs, &path, true)
                    .map_err(|e| js_err("file", "appendText", e))?;
                if let Some(parent) = full.parent() {
                    std::fs::create_dir_all(parent).map_err(|e| js_err("file", "appendText", e))?;
                }
                let mut f = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&full)
                    .map_err(|e| js_err("file", "appendText", e))?;
                f.write_all(content.as_bytes())
                    .map_err(|e| js_err("file", "appendText", e))?;
                Ok(true)
            },
        );
        file_obj
            .set("appendText", append_text)
            .map_err(|e| ScriptError::QuickJsError(e.to_string()))?;

        let file_root = self.config.file_root.clone();
        let file_allowed_dirs = self.config.file_allowed_dirs.clone();
        let exists = Function::new(
            js_ctx.clone(),
            move |path: String| -> rquickjs::Result<bool> {
                let full =
                    resolve_file_path(file_root.as_deref(), &file_allowed_dirs, &path, false)
                        .map_err(|e| js_err("file", "exists", e))?;
                Ok(full.exists())
            },
        );
        file_obj
            .set("exists", exists)
            .map_err(|e| ScriptError::QuickJsError(e.to_string()))?;

        let file_root = self.config.file_root.clone();
        let file_allowed_dirs = self.config.file_allowed_dirs.clone();
        let remove = Function::new(
            js_ctx.clone(),
            move |path: String| -> rquickjs::Result<bool> {
                let full =
                    resolve_file_path(file_root.as_deref(), &file_allowed_dirs, &path, false)
                        .map_err(|e| js_err("file", "remove", e))?;
                if full.is_dir() {
                    std::fs::remove_dir_all(&full).map_err(|e| js_err("file", "remove", e))?;
                } else if full.exists() {
                    std::fs::remove_file(&full).map_err(|e| js_err("file", "remove", e))?;
                }
                Ok(true)
            },
        );
        file_obj
            .set("remove", remove)
            .map_err(|e| ScriptError::QuickJsError(e.to_string()))?;

        let file_root = self.config.file_root.clone();
        let file_allowed_dirs = self.config.file_allowed_dirs.clone();
        let list_dir = Function::new(
            js_ctx.clone(),
            move |path: Option<String>| -> rquickjs::Result<Vec<String>> {
                let rel = path.unwrap_or_else(|| ".".to_string());
                let full = resolve_file_path(file_root.as_deref(), &file_allowed_dirs, &rel, false)
                    .map_err(|e| js_err("file", "listDir", e))?;
                let mut out = Vec::new();
                for entry in std::fs::read_dir(&full).map_err(|e| js_err("file", "listDir", e))? {
                    let entry = entry.map_err(|e| js_err("file", "listDir", e))?;
                    if let Some(name) = entry.file_name().to_str() {
                        out.push(name.to_string());
                    }
                }
                out.sort();
                Ok(out)
            },
        );
        file_obj
            .set("listDir", list_dir)
            .map_err(|e| ScriptError::QuickJsError(e.to_string()))?;

        let globals = js_ctx.globals();
        globals
            .set("file", file_obj)
            .map_err(|e| ScriptError::QuickJsError(e.to_string()))?;

        Ok(())
    }

    fn setup_net_object(&self, js_ctx: &Ctx) -> Result<()> {
        let net_obj =
            Object::new(js_ctx.clone()).map_err(|e| ScriptError::QuickJsError(e.to_string()))?;

        net_obj
            .set("enabled", self.config.allow_network)
            .map_err(|e| ScriptError::QuickJsError(e.to_string()))?;

        let allow_network = self.config.allow_network;
        let timeout_ms = self.config.network_timeout_ms;
        let max_req = self.config.max_net_request_bytes;
        let max_resp = self.config.max_net_response_bytes;

        // 说明：由于 QuickJS 在当前模式下为同步执行，网络接口采用阻塞请求，并返回 JSON 字符串。
        // JS 侧可用 JSON.parse(...) 获取结构化结果。
        let fetch = Function::new(
            js_ctx.clone(),
            move |url: String, options_json: Option<String>| -> rquickjs::Result<String> {
                if !allow_network {
                    return Err(rquickjs::Error::new_from_js_message(
                        "net",
                        "fetch",
                        "net API 未启用",
                    ));
                }

                let parsed = reqwest::Url::parse(&url).map_err(|e| js_err("net", "fetch", e))?;
                let scheme = parsed.scheme();
                if scheme != "http" && scheme != "https" {
                    return Err(rquickjs::Error::new_from_js_message(
                        "net",
                        "fetch",
                        format!("仅允许 http/https，当前为: {}", scheme),
                    ));
                }

                let mut method = "GET".to_string();
                let mut headers = HashMap::<String, String>::new();
                let mut body: Option<String> = None;
                let mut body_bytes: Option<Vec<u8>> = None;
                let mut req_timeout_ms = timeout_ms;

                if let Some(json) = options_json {
                    let v: JsonValue =
                        serde_json::from_str(&json).map_err(|e| js_err("net", "fetch", e))?;
                    if let Some(m) = v.get("method").and_then(|x| x.as_str()) {
                        method = m.to_string();
                    }
                    if let Some(t) = v.get("timeoutMs").and_then(|x| x.as_u64()) {
                        req_timeout_ms = t;
                    }
                    if let Some(h) = v.get("headers").and_then(|x| x.as_object()) {
                        for (k, vv) in h {
                            let val = vv
                                .as_str()
                                .map(|s| s.to_string())
                                .unwrap_or_else(|| vv.to_string());
                            headers.insert(k.to_string(), val);
                        }
                    }
                    if let Some(b) = v.get("body") {
                        match b {
                            JsonValue::Null => body = None,
                            JsonValue::String(s) => body = Some(s.clone()),
                            other => body = Some(other.to_string()),
                        }
                    }
                    if let Some(b64) = v.get("bodyBase64").and_then(|x| x.as_str()) {
                        let decoded = base64::engine::general_purpose::STANDARD
                            .decode(b64)
                            .map_err(|e| js_err("net", "fetch", e))?;
                        body_bytes = Some(decoded);
                        body = None;
                    }
                }

                let body_len = body_bytes
                    .as_ref()
                    .map(|b| b.len())
                    .or_else(|| body.as_ref().map(|b| b.len()));
                if let Some(len) = body_len {
                    if len > max_req {
                        return Err(rquickjs::Error::new_from_js_message(
                            "net",
                            "fetch",
                            format!("请求体过大: {} bytes (limit {})", len, max_req),
                        ));
                    }
                }

                let m = reqwest::Method::from_bytes(method.as_bytes())
                    .map_err(|e| js_err("net", "fetch", e))?;
                let client = bifrost_core::direct_blocking_reqwest_client_builder()
                    .timeout(std::time::Duration::from_millis(req_timeout_ms))
                    .build()
                    .map_err(|e| js_err("net", "fetch", e))?;

                let mut req = client.request(m, parsed);
                for (k, v) in headers.iter() {
                    req = req.header(k, v);
                }
                if let Some(b) = body_bytes {
                    req = req.body(b);
                } else if let Some(b) = body {
                    req = req.body(b);
                }

                let resp = req.send().map_err(|e| js_err("net", "fetch", e))?;
                let status = resp.status();

                let mut resp_headers = HashMap::<String, String>::new();
                for (k, v) in resp.headers().iter() {
                    resp_headers.insert(k.to_string(), v.to_str().unwrap_or("").to_string());
                }

                use std::io::Read;
                let mut buf = Vec::new();
                let mut limited = resp.take(max_resp as u64);
                std::io::Read::read_to_end(&mut limited, &mut buf)
                    .map_err(|e| js_err("net", "fetch", e))?;
                if buf.len() >= max_resp {
                    return Err(rquickjs::Error::new_from_js_message(
                        "net",
                        "fetch",
                        format!("响应体过大，已达到限制: {} bytes", max_resp),
                    ));
                }
                let body_text = String::from_utf8_lossy(&buf).to_string();

                let out = serde_json::json!({
                    "status": status.as_u16(),
                    "ok": status.is_success(),
                    "headers": resp_headers,
                    "body": body_text,
                });
                Ok(out.to_string())
            },
        );

        net_obj
            .set("fetch", fetch)
            .map_err(|e| ScriptError::QuickJsError(e.to_string()))?;
        // alias
        let fetch_alias: Function = net_obj
            .get("fetch")
            .map_err(|e| ScriptError::QuickJsError(e.to_string()))?;
        net_obj
            .set("request", fetch_alias)
            .map_err(|e| ScriptError::QuickJsError(e.to_string()))?;

        let globals = js_ctx.globals();
        globals
            .set("net", net_obj)
            .map_err(|e| ScriptError::QuickJsError(e.to_string()))?;

        Ok(())
    }
}

fn js_err<E: std::fmt::Display>(from: &'static str, to: &'static str, e: E) -> rquickjs::Error {
    rquickjs::Error::new_from_js_message(from, to, e.to_string())
}

fn resolve_file_path(
    primary_dir: Option<&Path>,
    allowed_dirs: &[PathBuf],
    user_path: &str,
    for_write: bool,
) -> std::io::Result<PathBuf> {
    if user_path.is_empty() {
        return Err(std::io::Error::other("路径不能为空"));
    }

    // 允许访问的根目录集合：primary_dir + allowed_dirs
    let mut roots: Vec<PathBuf> = Vec::new();
    if let Some(p) = primary_dir {
        roots.push(p.to_path_buf());
    }
    roots.extend(allowed_dirs.iter().cloned());

    if roots.is_empty() {
        return Err(std::io::Error::other("file API 未启用"));
    }

    let p = Path::new(user_path);

    if p.is_absolute() {
        return resolve_under_roots(p, &roots, for_write);
    }

    // 相对路径：禁止 .. 等跳转
    for c in p.components() {
        match c {
            Component::Normal(_) | Component::CurDir => {}
            Component::ParentDir => return Err(std::io::Error::other("不允许使用 ..")),
            Component::RootDir | Component::Prefix(_) => {
                return Err(std::io::Error::other("不允许根路径/前缀"))
            }
        }
    }

    // 相对路径的落点目录：优先 primary_dir，否则使用 allowed_dirs 的第一个
    let base = primary_dir
        .or_else(|| allowed_dirs.first().map(|p| p.as_path()))
        .ok_or_else(|| std::io::Error::other("未配置可用目录"))?;

    resolve_under_roots(&base.join(p), &roots, for_write)
}

fn resolve_under_roots(
    path: &Path,
    roots: &[PathBuf],
    for_write: bool,
) -> std::io::Result<PathBuf> {
    let root_canons: Vec<PathBuf> = roots
        .iter()
        .map(|r| r.canonicalize().unwrap_or_else(|_| r.to_path_buf()))
        .collect();

    if path.exists() {
        let canon = path.canonicalize()?;
        if !root_canons.iter().any(|r| canon.starts_with(r)) {
            return Err(std::io::Error::other("路径超出允许目录范围"));
        }
        return Ok(canon);
    }

    // 不存在时：通过最近存在的父目录做一次 canonicalize 校验（防止符号链接逃逸）
    let mut parent = path.parent();
    while let Some(p) = parent {
        if p.exists() {
            let canon_parent = p.canonicalize()?;
            if !root_canons.iter().any(|r| canon_parent.starts_with(r)) {
                return Err(std::io::Error::other("路径超出允许目录范围"));
            }
            return Ok(path.to_path_buf());
        }
        parent = p.parent();
    }

    // 没有任何已存在的父目录，无法判断
    if for_write {
        Err(std::io::Error::other("目标路径父目录不存在"))
    } else {
        Ok(path.to_path_buf())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sandbox_creation() {
        let sandbox = Sandbox::new(SandboxConfig::default());
        assert!(sandbox.is_ok());
    }

    #[test]
    fn test_simple_script() {
        let mut sandbox = Sandbox::new(SandboxConfig::default()).unwrap();
        let request = RequestData {
            url: "https://example.com/api".to_string(),
            method: "GET".to_string(),
            host: "example.com".to_string(),
            path: "/api".to_string(),
            protocol: "https".to_string(),
            client_ip: "127.0.0.1".to_string(),
            client_app: Some("test".to_string()),
            headers: HashMap::new(),
            body: None,
        };

        let ctx = ScriptContext {
            request_id: "test-123".to_string(),
            script_name: "test".to_string(),
            script_type: ScriptType::Request,
            values: HashMap::new(),
            matched_rules: vec![],
        };

        let script = r#"
            log.info("Processing request: " + request.url);
        "#;

        let result = sandbox.execute_request_script(script, &request, &ctx);
        assert!(result.is_ok());

        let (_, logs) = result.unwrap();
        assert_eq!(logs.len(), 1);
        assert!(logs[0].message.contains("example.com"));
    }

    #[test]
    fn test_ctx_values_access() {
        let mut sandbox = Sandbox::new(SandboxConfig::default()).unwrap();
        let request = RequestData {
            url: "https://example.com/api".to_string(),
            method: "GET".to_string(),
            host: "example.com".to_string(),
            path: "/api".to_string(),
            protocol: "https".to_string(),
            client_ip: "127.0.0.1".to_string(),
            client_app: None,
            headers: HashMap::new(),
            body: None,
        };

        let mut values = HashMap::new();
        values.insert("API_TOKEN".to_string(), "secret-token".to_string());

        let matched_rules = vec![MatchedRuleInfo {
            pattern: "/api/*".to_string(),
            protocol: "https".to_string(),
            value: "backend".to_string(),
        }];

        let ctx = ScriptContext {
            request_id: "test-123".to_string(),
            script_name: "test".to_string(),
            script_type: ScriptType::Request,
            values,
            matched_rules,
        };

        let script = r#"
            var token = ctx.values.API_TOKEN;
            var rule = ctx.matchedRules[0];
            log.info("Token: " + token + ", rule=" + rule.pattern + "/" + rule.protocol + "/" + rule.value);
        "#;

        let result = sandbox.execute_request_script(script, &request, &ctx);
        assert!(result.is_ok());

        let (_, logs) = result.unwrap();
        assert_eq!(logs.len(), 1);
        let msg = &logs[0].message;
        assert!(msg.contains("secret-token"));
        assert!(msg.contains("rule=/api/*"));
    }

    #[test]
    fn test_infinite_loop_timeout() {
        let mut sandbox = Sandbox::new(SandboxConfig {
            timeout_ms: 100,
            max_memory: 16 * 1024 * 1024,
            ..Default::default()
        })
        .unwrap();

        let request = RequestData {
            url: "https://example.com/api".to_string(),
            method: "GET".to_string(),
            host: "example.com".to_string(),
            path: "/api".to_string(),
            protocol: "https".to_string(),
            client_ip: "127.0.0.1".to_string(),
            client_app: None,
            headers: HashMap::new(),
            body: None,
        };

        let ctx = ScriptContext {
            request_id: "test-timeout".to_string(),
            script_name: "timeout-test".to_string(),
            script_type: ScriptType::Request,
            values: HashMap::new(),
            matched_rules: vec![],
        };

        let script = r#"
            while(true) {}
        "#;

        let start = std::time::Instant::now();
        let result = sandbox.execute_request_script(script, &request, &ctx);
        let elapsed = start.elapsed();

        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), ScriptError::Timeout(_)));
        assert!(
            elapsed.as_millis() < 500,
            "Timeout should trigger within 500ms, took {:?}",
            elapsed
        );
    }

    #[test]
    fn test_cpu_intensive_timeout() {
        let mut sandbox = Sandbox::new(SandboxConfig {
            timeout_ms: 100,
            max_memory: 16 * 1024 * 1024,
            ..Default::default()
        })
        .unwrap();

        let request = RequestData {
            url: "https://example.com/api".to_string(),
            method: "GET".to_string(),
            host: "example.com".to_string(),
            path: "/api".to_string(),
            protocol: "https".to_string(),
            client_ip: "127.0.0.1".to_string(),
            client_app: None,
            headers: HashMap::new(),
            body: None,
        };

        let ctx = ScriptContext {
            request_id: "test-cpu".to_string(),
            script_name: "cpu-test".to_string(),
            script_type: ScriptType::Request,
            values: HashMap::new(),
            matched_rules: vec![],
        };

        let script = r#"
            var sum = 0;
            for (var i = 0; i < 1000000000; i++) {
                sum += i;
            }
        "#;

        let start = std::time::Instant::now();
        let result = sandbox.execute_request_script(script, &request, &ctx);
        let elapsed = start.elapsed();

        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), ScriptError::Timeout(_)));
        assert!(
            elapsed.as_millis() < 500,
            "Timeout should trigger within 500ms, took {:?}",
            elapsed
        );
    }

    #[test]
    fn test_file_read_write_in_sandbox() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut sandbox = Sandbox::new(SandboxConfig {
            file_root: Some(tmp.path().to_path_buf()),
            file_allowed_dirs: vec![],
            ..Default::default()
        })
        .unwrap();

        let request = RequestData {
            url: "https://example.com/api".to_string(),
            method: "GET".to_string(),
            host: "example.com".to_string(),
            path: "/api".to_string(),
            protocol: "https".to_string(),
            client_ip: "127.0.0.1".to_string(),
            client_app: Some("test".to_string()),
            headers: HashMap::new(),
            body: None,
        };
        let ctx = ScriptContext {
            request_id: "test-file".to_string(),
            script_name: "file-test".to_string(),
            script_type: ScriptType::Request,
            values: HashMap::new(),
            matched_rules: vec![],
        };

        let script = r#"
            file.writeText("state/hello.txt", "hello");
            var v = file.readText("state/hello.txt");
            log.info("file:", v);
        "#;

        let result = sandbox.execute_request_script(script, &request, &ctx);
        assert!(result.is_ok());
        let (_, logs) = result.unwrap();
        assert!(logs.iter().any(|l| l.message.contains("hello")));
        assert!(tmp.path().join("state/hello.txt").exists());
    }

    #[test]
    fn test_file_path_traversal_denied() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut sandbox = Sandbox::new(SandboxConfig {
            file_root: Some(tmp.path().to_path_buf()),
            file_allowed_dirs: vec![],
            ..Default::default()
        })
        .unwrap();

        let request = RequestData {
            url: "https://example.com/api".to_string(),
            method: "GET".to_string(),
            host: "example.com".to_string(),
            path: "/api".to_string(),
            protocol: "https".to_string(),
            client_ip: "127.0.0.1".to_string(),
            client_app: None,
            headers: HashMap::new(),
            body: None,
        };
        let ctx = ScriptContext {
            request_id: "test-file".to_string(),
            script_name: "file-test".to_string(),
            script_type: ScriptType::Request,
            values: HashMap::new(),
            matched_rules: vec![],
        };

        // 说明：当前实现禁止相对路径的 ..，避免逃逸
        let script = r#"
            file.readText("../evil.txt");
        "#;
        let result = sandbox.execute_request_script(script, &request, &ctx);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("不允许") || err.contains("sandbox"));
    }

    #[test]
    fn test_file_absolute_path_allowed_when_configured() {
        let tmp = tempfile::TempDir::new().unwrap();
        let allow_dir = tempfile::TempDir::new().unwrap();
        std::fs::write(allow_dir.path().join("a.txt"), "ok").unwrap();

        let mut sandbox = Sandbox::new(SandboxConfig {
            file_root: Some(tmp.path().to_path_buf()),
            file_allowed_dirs: vec![allow_dir.path().to_path_buf()],
            ..Default::default()
        })
        .unwrap();

        let request = RequestData {
            url: "https://example.com/api".to_string(),
            method: "GET".to_string(),
            host: "example.com".to_string(),
            path: "/api".to_string(),
            protocol: "https".to_string(),
            client_ip: "127.0.0.1".to_string(),
            client_app: None,
            headers: HashMap::new(),
            body: None,
        };
        let ctx = ScriptContext {
            request_id: "test-file".to_string(),
            script_name: "file-test".to_string(),
            script_type: ScriptType::Request,
            values: HashMap::new(),
            matched_rules: vec![],
        };

        let abs = allow_dir.path().join("a.txt").to_string_lossy().to_string();
        let script = format!(
            r#"
            var v = file.readText("{}");
            log.info("abs:", v);
            "#,
            abs.replace('\\', "\\\\")
        );

        let result = sandbox.execute_request_script(&script, &request, &ctx);
        assert!(result.is_ok());
        let (_, logs) = result.unwrap();
        assert!(logs.iter().any(|l| l.message.contains("ok")));
    }

    // ---------------------------------------------------------------------
    // Shared helpers for the expanded sandbox coverage tests below.
    // ---------------------------------------------------------------------
    fn req(method: &str) -> RequestData {
        RequestData {
            url: "https://example.com/api".to_string(),
            method: method.to_string(),
            host: "example.com".to_string(),
            path: "/api".to_string(),
            protocol: "https".to_string(),
            client_ip: "127.0.0.1".to_string(),
            client_app: Some("test".to_string()),
            headers: HashMap::new(),
            body: None,
        }
    }

    fn resp(status: u16) -> ResponseData {
        ResponseData {
            status,
            status_text: "OK".to_string(),
            headers: HashMap::new(),
            body: Some("original".to_string()),
            request: req("GET"),
        }
    }

    fn ctx(ty: ScriptType) -> ScriptContext {
        ScriptContext {
            request_id: "rid".to_string(),
            script_name: "s".to_string(),
            script_type: ty,
            values: HashMap::new(),
            matched_rules: vec![],
        }
    }

    // ----- request script mutations -----
    #[test]
    fn test_request_script_mutates_method_body_headers() {
        let mut sb = Sandbox::new(SandboxConfig::default()).unwrap();
        let r = req("GET");
        let script = r#"
            request.method = "POST";
            request.body = "hello";
            request.headers["X-Test"] = "1";
        "#;
        let (mods, _logs) = sb
            .execute_request_script(script, &r, &ctx(ScriptType::Request))
            .unwrap();
        assert_eq!(mods.method.as_deref(), Some("POST"));
        assert_eq!(mods.body.as_deref(), Some("hello"));
        assert_eq!(
            mods.headers
                .as_ref()
                .and_then(|h| h.get("X-Test"))
                .map(|s| s.as_str()),
            Some("1")
        );
    }

    #[test]
    fn test_request_script_clears_body() {
        let mut sb = Sandbox::new(SandboxConfig::default()).unwrap();
        let mut r = req("POST");
        r.body = Some("had body".to_string());
        let script = r#"request.body = null;"#;
        let (mods, _logs) = sb
            .execute_request_script(script, &r, &ctx(ScriptType::Request))
            .unwrap();
        assert_eq!(mods.body, None);
    }

    #[test]
    fn test_request_script_runtime_error_is_reported() {
        let mut sb = Sandbox::new(SandboxConfig::default()).unwrap();
        let r = req("GET");
        let script = r#"throw new Error("boom");"#;
        let err = sb
            .execute_request_script(script, &r, &ctx(ScriptType::Request))
            .unwrap_err();
        assert!(matches!(err, ScriptError::ExecutionFailed(_)));
        assert!(err.to_string().contains("boom"));
    }

    #[test]
    fn test_log_levels_and_console_alias() {
        let mut sb = Sandbox::new(SandboxConfig::default()).unwrap();
        let r = req("GET");
        let script = r#"
            log.debug("d");
            log.warn("w");
            log.error("e");
            console.info("c");
        "#;
        let (_mods, logs) = sb
            .execute_request_script(script, &r, &ctx(ScriptType::Request))
            .unwrap();
        assert_eq!(logs.len(), 4);
        assert!(logs
            .iter()
            .any(|l| matches!(l.level, ScriptLogLevel::Debug)));
        assert!(logs.iter().any(|l| matches!(l.level, ScriptLogLevel::Warn)));
        assert!(logs
            .iter()
            .any(|l| matches!(l.level, ScriptLogLevel::Error)));
        assert!(logs.iter().any(|l| l.message == "c"));
    }

    #[test]
    fn test_ctx_matched_rules_visible_in_js() {
        let mut sb = Sandbox::new(SandboxConfig::default()).unwrap();
        let r = req("GET");
        let mut c = ctx(ScriptType::Request);
        c.matched_rules = vec![MatchedRuleInfo {
            pattern: "*.example.com".to_string(),
            protocol: "https".to_string(),
            value: "v1".to_string(),
        }];
        let script = r#"
            log.info("rule:" + ctx.matchedRules[0].pattern + ctx.matchedRules[0].value);
            log.info("phase:" + ctx.phase + " type:" + ctx.scriptType);
        "#;
        let (_mods, logs) = sb.execute_request_script(script, &r, &c).unwrap();
        assert!(logs.iter().any(|l| l.message.contains("*.example.com")));
        assert!(logs.iter().any(|l| l.message.contains("phase:request")));
    }

    // ----- response script mutations -----
    #[test]
    fn test_response_script_mutates_status_and_body() {
        let mut sb = Sandbox::new(SandboxConfig::default()).unwrap();
        let r = resp(200);
        let script = r#"
            response.status = 503;
            response.statusText = "Down";
            response.body = "new body";
            response.headers["X-Mod"] = "yes";
        "#;
        let (mods, _logs) = sb
            .execute_response_script(script, &r, &ctx(ScriptType::Response))
            .unwrap();
        assert_eq!(mods.status, Some(503));
        assert_eq!(mods.status_text.as_deref(), Some("Down"));
        assert_eq!(mods.body.as_deref(), Some("new body"));
        assert_eq!(
            mods.headers
                .as_ref()
                .and_then(|h| h.get("X-Mod"))
                .map(|s| s.as_str()),
            Some("yes")
        );
    }

    #[test]
    fn test_response_script_no_change_yields_empty_mods() {
        let mut sb = Sandbox::new(SandboxConfig::default()).unwrap();
        let r = resp(200);
        let script = r#"log.info("status=" + response.status + " " + response.request.method);"#;
        let (mods, logs) = sb
            .execute_response_script(script, &r, &ctx(ScriptType::Response))
            .unwrap();
        assert_eq!(mods.status, None);
        assert_eq!(mods.body, None);
        assert_eq!(mods.headers, None);
        assert!(logs.iter().any(|l| l.message.contains("status=200")));
    }

    #[test]
    fn test_response_script_clears_body() {
        let mut sb = Sandbox::new(SandboxConfig::default()).unwrap();
        let r = resp(200);
        let script = r#"response.body = null;"#;
        let (mods, _logs) = sb
            .execute_response_script(script, &r, &ctx(ScriptType::Response))
            .unwrap();
        assert_eq!(mods.body, None);
    }

    #[test]
    fn test_response_script_runtime_error() {
        let mut sb = Sandbox::new(SandboxConfig::default()).unwrap();
        let r = resp(200);
        let err = sb
            .execute_response_script(
                r#"throw new Error("rerr");"#,
                &r,
                &ctx(ScriptType::Response),
            )
            .unwrap_err();
        assert!(matches!(err, ScriptError::ExecutionFailed(_)));
        assert!(err.to_string().contains("rerr"));
    }

    // ----- decode scripts (request phase) -----
    #[test]
    fn test_decode_request_phase_reads_body_bytes_and_returns_output() {
        let mut sb = Sandbox::new(SandboxConfig::default()).unwrap();
        let r = req("POST");
        let resp_empty = ResponseData::default();
        let body = b"abc";
        let script = r#"
            log.info("size=" + request.bodySize + " hex=" + request.bodyHex);
            ({ code: "0", data: request.bodyHex, msg: "ok" });
        "#;
        let (out, logs) = sb
            .execute_decode_script(
                script,
                "request",
                &r,
                body,
                &resp_empty,
                &[],
                &ctx(ScriptType::Decode),
            )
            .unwrap();
        assert_eq!(out.code, "0");
        assert_eq!(out.data, "616263"); // hex of "abc"
        assert!(logs.iter().any(|l| l.message.contains("size=3")));
    }

    #[test]
    fn test_decode_request_phase_response_is_null() {
        let mut sb = Sandbox::new(SandboxConfig::default()).unwrap();
        let r = req("GET");
        let resp_empty = ResponseData::default();
        let script =
            r#"({ code: "0", data: (response === null) ? "isnull" : "notnull", msg: "" });"#;
        let (out, _logs) = sb
            .execute_decode_script(
                script,
                "request",
                &r,
                b"",
                &resp_empty,
                &[],
                &ctx(ScriptType::Decode),
            )
            .unwrap();
        assert_eq!(out.data, "isnull");
    }

    // ----- decode scripts (response phase) -----
    #[test]
    fn test_decode_response_phase_reads_response_and_request_snapshot() {
        let mut sb = Sandbox::new(SandboxConfig::default()).unwrap();
        let r = req("GET");
        let mut response = resp(201);
        response.request = req("PUT");
        let body = b"hello";
        let script = r#"
            ({ code: "0", data: response.bodyBase64 + "|" + request.method, msg: "" });
        "#;
        let (out, _logs) = sb
            .execute_decode_script(
                script,
                "response",
                &r,
                &[],
                &response,
                body,
                &ctx(ScriptType::Decode),
            )
            .unwrap();
        // base64("hello") = aGVsbG8=, request snapshot method is PUT
        assert!(out.data.starts_with("aGVsbG8="));
        assert!(out.data.ends_with("|PUT"));
    }

    #[test]
    fn test_decode_via_ctx_output_global() {
        let mut sb = Sandbox::new(SandboxConfig::default()).unwrap();
        let r = req("GET");
        let resp_empty = ResponseData::default();
        let script = r#"ctx.output = { code: "0", data: "viactx", msg: "" };"#;
        let (out, _logs) = sb
            .execute_decode_script(
                script,
                "request",
                &r,
                b"",
                &resp_empty,
                &[],
                &ctx(ScriptType::Decode),
            )
            .unwrap();
        assert_eq!(out.data, "viactx");
    }

    #[test]
    fn test_decode_via_output_global() {
        let mut sb = Sandbox::new(SandboxConfig::default()).unwrap();
        let r = req("GET");
        let resp_empty = ResponseData::default();
        let script = r#"var output = { code: "7", data: "viaglobal", msg: "m" };"#;
        let (out, _logs) = sb
            .execute_decode_script(
                script,
                "request",
                &r,
                b"",
                &resp_empty,
                &[],
                &ctx(ScriptType::Decode),
            )
            .unwrap();
        assert_eq!(out.code, "7");
        assert_eq!(out.data, "viaglobal");
        assert_eq!(out.msg, "m");
    }

    #[test]
    fn test_decode_returns_json_string() {
        let mut sb = Sandbox::new(SandboxConfig::default()).unwrap();
        let r = req("GET");
        let resp_empty = ResponseData::default();
        let script = r#"JSON.stringify({ code: "0", data: "fromstr", msg: "" });"#;
        let (out, _logs) = sb
            .execute_decode_script(
                script,
                "request",
                &r,
                b"",
                &resp_empty,
                &[],
                &ctx(ScriptType::Decode),
            )
            .unwrap();
        assert_eq!(out.data, "fromstr");
    }

    #[test]
    fn test_decode_missing_output_errors() {
        let mut sb = Sandbox::new(SandboxConfig::default()).unwrap();
        let r = req("GET");
        let resp_empty = ResponseData::default();
        let script = r#"log.info("no output");"#;
        let err = sb
            .execute_decode_script(
                script,
                "request",
                &r,
                b"",
                &resp_empty,
                &[],
                &ctx(ScriptType::Decode),
            )
            .unwrap_err();
        assert!(matches!(err, ScriptError::ExecutionFailed(_)));
    }

    #[test]
    fn test_decode_output_missing_code_errors() {
        let mut sb = Sandbox::new(SandboxConfig::default()).unwrap();
        let r = req("GET");
        let resp_empty = ResponseData::default();
        let script = r#"({ data: "x", msg: "y" });"#;
        let err = sb
            .execute_decode_script(
                script,
                "request",
                &r,
                b"",
                &resp_empty,
                &[],
                &ctx(ScriptType::Decode),
            )
            .unwrap_err();
        assert!(err.to_string().contains("code"));
    }

    #[test]
    fn test_decode_output_not_object_errors() {
        let mut sb = Sandbox::new(SandboxConfig::default()).unwrap();
        let r = req("GET");
        let resp_empty = ResponseData::default();
        let script = r#""[1,2,3]";"#;
        let err = sb
            .execute_decode_script(
                script,
                "request",
                &r,
                b"",
                &resp_empty,
                &[],
                &ctx(ScriptType::Decode),
            )
            .unwrap_err();
        assert!(matches!(err, ScriptError::ExecutionFailed(_)));
    }

    #[test]
    fn test_decode_runtime_error() {
        let mut sb = Sandbox::new(SandboxConfig::default()).unwrap();
        let r = req("GET");
        let resp_empty = ResponseData::default();
        let err = sb
            .execute_decode_script(
                r#"throw new Error("derr");"#,
                "request",
                &r,
                b"",
                &resp_empty,
                &[],
                &ctx(ScriptType::Decode),
            )
            .unwrap_err();
        assert!(err.to_string().contains("derr"));
    }

    // ----- file API extended -----
    #[test]
    fn test_file_append_exists_listdir_remove() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut sb = Sandbox::new(SandboxConfig {
            file_root: Some(tmp.path().to_path_buf()),
            ..Default::default()
        })
        .unwrap();
        let r = req("GET");
        let script = r#"
            file.appendText("log.txt", "a");
            file.appendText("log.txt", "b");
            log.info("exists=" + file.exists("log.txt"));
            log.info("content=" + file.readText("log.txt"));
            file.writeText("other.txt", "x");
            var entries = file.listDir(".");
            log.info("list=" + entries.join(","));
            file.remove("other.txt");
            log.info("after=" + file.exists("other.txt"));
        "#;
        let (_m, logs) = sb
            .execute_request_script(script, &r, &ctx(ScriptType::Request))
            .unwrap();
        let joined: String = logs
            .iter()
            .map(|l| l.message.clone())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(joined.contains("exists=true"));
        assert!(joined.contains("content=ab"));
        assert!(joined.contains("log.txt"));
        assert!(joined.contains("other.txt"));
        assert!(joined.contains("after=false"));
    }

    #[test]
    fn test_file_remove_directory() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut sb = Sandbox::new(SandboxConfig {
            file_root: Some(tmp.path().to_path_buf()),
            ..Default::default()
        })
        .unwrap();
        let r = req("GET");
        let script = r#"
            file.writeText("d/inside.txt", "x");
            file.remove("d");
            log.info("dir_exists=" + file.exists("d"));
        "#;
        let (_m, logs) = sb
            .execute_request_script(script, &r, &ctx(ScriptType::Request))
            .unwrap();
        assert!(logs.iter().any(|l| l.message.contains("dir_exists=false")));
    }

    #[test]
    fn test_file_write_too_large_errors() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut sb = Sandbox::new(SandboxConfig {
            file_root: Some(tmp.path().to_path_buf()),
            max_file_bytes: 4,
            ..Default::default()
        })
        .unwrap();
        let r = req("GET");
        let script = r#"file.writeText("big.txt", "abcdefgh");"#;
        let err = sb
            .execute_request_script(script, &r, &ctx(ScriptType::Request))
            .unwrap_err();
        assert!(matches!(err, ScriptError::ExecutionFailed(_)));
    }

    #[test]
    fn test_file_read_too_large_errors() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::write(tmp.path().join("big.txt"), "abcdefghij").unwrap();
        let mut sb = Sandbox::new(SandboxConfig {
            file_root: Some(tmp.path().to_path_buf()),
            max_file_bytes: 4,
            ..Default::default()
        })
        .unwrap();
        let r = req("GET");
        let err = sb
            .execute_request_script(
                r#"file.readText("big.txt");"#,
                &r,
                &ctx(ScriptType::Request),
            )
            .unwrap_err();
        assert!(matches!(err, ScriptError::ExecutionFailed(_)));
    }

    #[test]
    fn test_file_disabled_reports_not_enabled() {
        let mut sb = Sandbox::new(SandboxConfig::default()).unwrap();
        let r = req("GET");
        let script = r#"log.info("file_enabled=" + file.enabled);"#;
        let (_m, logs) = sb
            .execute_request_script(script, &r, &ctx(ScriptType::Request))
            .unwrap();
        assert!(logs
            .iter()
            .any(|l| l.message.contains("file_enabled=false")));
    }

    #[test]
    fn test_file_read_missing_errors() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut sb = Sandbox::new(SandboxConfig {
            file_root: Some(tmp.path().to_path_buf()),
            ..Default::default()
        })
        .unwrap();
        let r = req("GET");
        let err = sb
            .execute_request_script(
                r#"file.readText("nope.txt");"#,
                &r,
                &ctx(ScriptType::Request),
            )
            .unwrap_err();
        assert!(matches!(err, ScriptError::ExecutionFailed(_)));
    }

    // ----- net API -----
    #[test]
    fn test_net_disabled_by_default() {
        let mut sb = Sandbox::new(SandboxConfig::default()).unwrap();
        let r = req("GET");
        let script = r#"log.info("net_enabled=" + net.enabled);"#;
        let (_m, logs) = sb
            .execute_request_script(script, &r, &ctx(ScriptType::Request))
            .unwrap();
        assert!(logs.iter().any(|l| l.message.contains("net_enabled=false")));
    }

    #[test]
    fn test_net_fetch_disabled_errors() {
        let mut sb = Sandbox::new(SandboxConfig::default()).unwrap();
        let r = req("GET");
        let err = sb
            .execute_request_script(
                r#"net.fetch("https://x.test/");"#,
                &r,
                &ctx(ScriptType::Request),
            )
            .unwrap_err();
        assert!(matches!(err, ScriptError::ExecutionFailed(_)));
    }

    #[test]
    fn test_net_fetch_rejects_non_http_scheme() {
        let mut sb = Sandbox::new(SandboxConfig {
            allow_network: true,
            ..Default::default()
        })
        .unwrap();
        let r = req("GET");
        let err = sb
            .execute_request_script(
                r#"net.fetch("ftp://x.test/");"#,
                &r,
                &ctx(ScriptType::Request),
            )
            .unwrap_err();
        assert!(err.to_string().contains("http") || matches!(err, ScriptError::ExecutionFailed(_)));
    }

    #[test]
    fn test_net_fetch_rejects_invalid_url() {
        let mut sb = Sandbox::new(SandboxConfig {
            allow_network: true,
            ..Default::default()
        })
        .unwrap();
        let r = req("GET");
        let err = sb
            .execute_request_script(r#"net.fetch("not a url");"#, &r, &ctx(ScriptType::Request))
            .unwrap_err();
        assert!(matches!(err, ScriptError::ExecutionFailed(_)));
    }

    #[test]
    fn test_net_fetch_request_body_too_large() {
        let mut sb = Sandbox::new(SandboxConfig {
            allow_network: true,
            max_net_request_bytes: 2,
            ..Default::default()
        })
        .unwrap();
        let r = req("GET");
        let script =
            r#"net.fetch("https://x.test/", JSON.stringify({ method: "POST", body: "toolong" }));"#;
        let err = sb
            .execute_request_script(script, &r, &ctx(ScriptType::Request))
            .unwrap_err();
        assert!(matches!(err, ScriptError::ExecutionFailed(_)));
    }

    #[test]
    fn test_net_request_is_alias_for_fetch() {
        let mut sb = Sandbox::new(SandboxConfig::default()).unwrap();
        let r = req("GET");
        // net.request exists and, being disabled, errors the same way as fetch.
        let err = sb
            .execute_request_script(
                r#"net.request("https://x.test/");"#,
                &r,
                &ctx(ScriptType::Request),
            )
            .unwrap_err();
        assert!(matches!(err, ScriptError::ExecutionFailed(_)));
    }

    // ----- dangerous globals removed -----
    #[test]
    fn test_dangerous_globals_removed() {
        let mut sb = Sandbox::new(SandboxConfig::default()).unwrap();
        let r = req("GET");
        let script = r#"log.info("eval=" + (typeof eval) + " Function=" + (typeof Function));"#;
        let (_m, logs) = sb
            .execute_request_script(script, &r, &ctx(ScriptType::Request))
            .unwrap();
        assert!(logs
            .iter()
            .any(|l| l.message.contains("eval=undefined")
                && l.message.contains("Function=undefined")));
    }

    // ----- absolute path outside allowed dirs denied -----
    #[test]
    fn test_file_absolute_path_outside_allowed_denied() {
        let tmp = tempfile::TempDir::new().unwrap();
        let outside = tempfile::TempDir::new().unwrap();
        std::fs::write(outside.path().join("secret.txt"), "s").unwrap();
        let mut sb = Sandbox::new(SandboxConfig {
            file_root: Some(tmp.path().to_path_buf()),
            ..Default::default()
        })
        .unwrap();
        let r = req("GET");
        let abs = outside
            .path()
            .join("secret.txt")
            .to_string_lossy()
            .replace('\\', "\\\\");
        let script = format!(r#"file.readText("{}");"#, abs);
        let err = sb
            .execute_request_script(&script, &r, &ctx(ScriptType::Request))
            .unwrap_err();
        assert!(matches!(err, ScriptError::ExecutionFailed(_)));
    }

    // ----- decode with default request (covers ResponseData/RequestData defaults) -----
    #[test]
    fn test_decode_request_phase_with_empty_body() {
        let mut sb = Sandbox::new(SandboxConfig::default()).unwrap();
        let r = req("GET");
        let resp_empty = ResponseData::default();
        let script = r#"({ code: "0", data: "size=" + request.bodySize, msg: "" });"#;
        let (out, _logs) = sb
            .execute_decode_script(
                script,
                "request",
                &r,
                &[],
                &resp_empty,
                &[],
                &ctx(ScriptType::Decode),
            )
            .unwrap();
        assert_eq!(out.data, "size=0");
    }

    // One-shot loopback HTTP fixture (same pattern used in engine/tests.rs) to
    // drive net.fetch's full request/response path: header injection, body
    // upload, status + headers + body readback, and JSON result shaping.
    #[test]
    fn test_net_fetch_success_round_trip() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = std::thread::spawn(move || {
            use std::io::{Read, Write};
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = Vec::new();
                let mut tmp = [0u8; 1024];
                loop {
                    let n = stream.read(&mut tmp).unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    buf.extend_from_slice(&tmp[..n]);
                    if let Some(header_end) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                        let headers = String::from_utf8_lossy(&buf[..header_end + 4]);
                        let content_length = headers
                            .lines()
                            .find_map(|line| {
                                line.strip_prefix("Content-Length:")
                                    .or_else(|| line.strip_prefix("content-length:"))
                                    .and_then(|v| v.trim().parse::<usize>().ok())
                            })
                            .unwrap_or(0);
                        let body_start = header_end + 4;
                        if buf.len() >= body_start + content_length {
                            break;
                        }
                    }
                }
                let payload = "{\"ok\":true}";
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                    payload.len(),
                    payload
                );
                let _ = stream.write_all(response.as_bytes());
            }
        });

        let mut sb = Sandbox::new(SandboxConfig {
            allow_network: true,
            ..Default::default()
        })
        .unwrap();
        let r = req("GET");
        let script = format!(
            r#"
            var resp = JSON.parse(net.fetch("http://{addr}/x", JSON.stringify({{
                method: "POST",
                headers: {{ "X-Probe": "1" }},
                body: "payload"
            }})));
            log.info("status=" + resp.status + " ok=" + resp.ok + " body=" + resp.body);
            "#
        );
        let (_m, logs) = sb
            .execute_request_script(&script, &r, &ctx(ScriptType::Request))
            .unwrap();
        let _ = handle.join();
        assert!(logs
            .iter()
            .any(|l| l.message.contains("status=200") && l.message.contains("ok=true")));
        assert!(logs
            .iter()
            .any(|l| l.message.contains("\\\"ok\\\":true") || l.message.contains("ok\":true")));
    }

    // Populated headers exercise every "for (k, v) in &headers" injection loop
    // across request/response/decode setup, plus the snapshot header rebuild.
    #[test]
    fn test_request_script_with_existing_headers_round_trip() {
        let mut sb = Sandbox::new(SandboxConfig::default()).unwrap();
        let mut r = req("POST");
        r.headers.insert("X-In".to_string(), "v".to_string());
        r.body = Some("b".to_string());
        let script = r#"
            log.info("in=" + request.headers["X-In"]);
            request.headers["X-Added"] = "2";
        "#;
        let (mods, logs) = sb
            .execute_request_script(script, &r, &ctx(ScriptType::Request))
            .unwrap();
        assert!(logs.iter().any(|l| l.message.contains("in=v")));
        let h = mods.headers.expect("headers changed");
        assert_eq!(h.get("X-Added").map(|s| s.as_str()), Some("2"));
        assert_eq!(h.get("X-In").map(|s| s.as_str()), Some("v"));
    }

    #[test]
    fn test_response_script_with_existing_headers() {
        let mut sb = Sandbox::new(SandboxConfig::default()).unwrap();
        let mut r = resp(200);
        r.headers
            .insert("Server".to_string(), "bifrost".to_string());
        r.request
            .headers
            .insert("X-Req".to_string(), "rq".to_string());
        let script = r#"
            log.info("srv=" + response.headers["Server"] + " req=" + response.request.headers["X-Req"]);
        "#;
        let (_mods, logs) = sb
            .execute_response_script(script, &r, &ctx(ScriptType::Response))
            .unwrap();
        assert!(logs
            .iter()
            .any(|l| l.message.contains("srv=bifrost") && l.message.contains("req=rq")));
    }

    #[test]
    fn test_decode_request_phase_with_headers() {
        let mut sb = Sandbox::new(SandboxConfig::default()).unwrap();
        let mut r = req("POST");
        r.headers
            .insert("Content-Type".to_string(), "app/x".to_string());
        let resp_empty = ResponseData::default();
        let script = r#"({ code: "0", data: request.headers["Content-Type"], msg: "" });"#;
        let (out, _logs) = sb
            .execute_decode_script(
                script,
                "request",
                &r,
                b"x",
                &resp_empty,
                &[],
                &ctx(ScriptType::Decode),
            )
            .unwrap();
        assert_eq!(out.data, "app/x");
    }

    #[test]
    fn test_decode_response_phase_with_headers_and_no_client_app() {
        let mut sb = Sandbox::new(SandboxConfig::default()).unwrap();
        let r = req("GET");
        let mut response = resp(200);
        response.headers.insert("X-R".to_string(), "rv".to_string());
        response.request = req("GET");
        response.request.client_app = None;
        response
            .request
            .headers
            .insert("X-RR".to_string(), "rrv".to_string());
        let script = r#"
            ({ code: "0", data: response.headers["X-R"] + "|" + response.request.headers["X-RR"], msg: "" });
        "#;
        let (out, _logs) = sb
            .execute_decode_script(
                script,
                "response",
                &r,
                &[],
                &response,
                b"body",
                &ctx(ScriptType::Decode),
            )
            .unwrap();
        assert_eq!(out.data, "rv|rrv");
    }

    // Throwing a non-Error value drives the string/fallback exception-detail
    // extraction branches (else-if `into_string`).
    #[test]
    fn test_request_script_throws_string_value() {
        let mut sb = Sandbox::new(SandboxConfig::default()).unwrap();
        let r = req("GET");
        let err = sb
            .execute_request_script(
                r#"throw "plain string error";"#,
                &r,
                &ctx(ScriptType::Request),
            )
            .unwrap_err();
        assert!(matches!(err, ScriptError::ExecutionFailed(_)));
    }

    #[test]
    fn test_response_script_throws_string_value() {
        let mut sb = Sandbox::new(SandboxConfig::default()).unwrap();
        let r = resp(200);
        let err = sb
            .execute_response_script(r#"throw "rstr";"#, &r, &ctx(ScriptType::Response))
            .unwrap_err();
        assert!(matches!(err, ScriptError::ExecutionFailed(_)));
    }

    #[test]
    fn test_decode_script_throws_string_value() {
        let mut sb = Sandbox::new(SandboxConfig::default()).unwrap();
        let r = req("GET");
        let resp_empty = ResponseData::default();
        let err = sb
            .execute_decode_script(
                r#"throw "dstr";"#,
                "request",
                &r,
                b"",
                &resp_empty,
                &[],
                &ctx(ScriptType::Decode),
            )
            .unwrap_err();
        assert!(matches!(err, ScriptError::ExecutionFailed(_)));
    }
}

#[cfg(test)]
mod more_tests {
    use super::*;

    #[test]
    fn test_bytes_to_hex_limited_basic_and_truncated() {
        let (hex, truncated) = bytes_to_hex_limited(&[0x00, 0x01, 0xff], 10);
        assert_eq!(hex, "0001ff");
        assert!(!truncated);

        let (hex2, truncated2) = bytes_to_hex_limited(b"abcdef", 2);
        assert_eq!(hex2, "6162");
        assert!(truncated2);
    }

    #[test]
    fn test_resolve_file_path_basic_and_errors() {
        // 空路径
        let err = resolve_file_path(None, &[], "", false).unwrap_err();
        assert!(err.to_string().contains("路径不能为空"));

        // file API 未启用
        let err = resolve_file_path(None, &[], "a.txt", false).unwrap_err();
        assert!(err.to_string().contains("file API 未启用"));

        // 相对路径在 primary_dir 下正常解析
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::write(tmp.path().join("a.txt"), "ok").unwrap();
        let p = resolve_file_path(Some(tmp.path()), &[], "a.txt", false).unwrap();
        assert_eq!(p.file_name().unwrap().to_string_lossy(), "a.txt");

        // 绝对路径不在允许目录下时被拒绝
        let allowed = tempfile::TempDir::new().unwrap();
        let other = tempfile::TempDir::new().unwrap();
        let err = resolve_file_path(
            Some(allowed.path()),
            &[],
            other.path().join("evil.txt").to_string_lossy().as_ref(),
            false,
        )
        .unwrap_err();
        assert!(err.to_string().contains("路径超出允许目录范围"));

        // for_write=true 且父目录链上没有任何已存在目录时返回专用错误
        let roots = vec![PathBuf::from("nonexistent-root")];
        let err =
            resolve_under_roots(Path::new("nonexistent-root/child.txt"), &roots, true).unwrap_err();
        assert!(err.to_string().contains("目标路径父目录不存在"));

        // 只读场景下则允许返回原始路径
        let path =
            resolve_under_roots(Path::new("nonexistent-root/child.txt"), &roots, false).unwrap();
        assert_eq!(path, PathBuf::from("nonexistent-root/child.txt"));
    }

    #[test]
    fn test_net_fetch_disabled() {
        let mut sandbox = Sandbox::new(SandboxConfig::default()).unwrap();

        let request = RequestData {
            url: "https://example.com/api".to_string(),
            method: "GET".to_string(),
            host: "example.com".to_string(),
            path: "/api".to_string(),
            protocol: "https".to_string(),
            client_ip: "127.0.0.1".to_string(),
            client_app: None,
            headers: HashMap::new(),
            body: None,
        };
        let ctx = ScriptContext {
            request_id: "net-disabled".to_string(),
            script_name: "net-disabled".to_string(),
            script_type: ScriptType::Request,
            values: HashMap::new(),
            matched_rules: vec![],
        };

        let script = r#"
            net.fetch("http://127.0.0.1/", null);
        "#;

        let result = sandbox.execute_request_script(script, &request, &ctx);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("net API 未启用"));
    }

    #[test]
    fn test_net_fetch_invalid_scheme() {
        let mut sandbox = Sandbox::new(SandboxConfig {
            allow_network: true,
            ..Default::default()
        })
        .unwrap();

        let request = RequestData {
            url: "https://example.com/api".to_string(),
            method: "GET".to_string(),
            host: "example.com".to_string(),
            path: "/api".to_string(),
            protocol: "https".to_string(),
            client_ip: "127.0.0.1".to_string(),
            client_app: None,
            headers: HashMap::new(),
            body: None,
        };
        let ctx = ScriptContext {
            request_id: "net-invalid-scheme".to_string(),
            script_name: "net-invalid-scheme".to_string(),
            script_type: ScriptType::Request,
            values: HashMap::new(),
            matched_rules: vec![],
        };

        let script = r#"
            net.fetch("ftp://example.com/resource", null);
        "#;

        let result = sandbox.execute_request_script(script, &request, &ctx);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("仅允许 http/https"));
    }

    #[test]
    fn test_net_fetch_request_body_too_large() {
        let mut sandbox = Sandbox::new(SandboxConfig {
            allow_network: true,
            max_net_request_bytes: 8,
            ..Default::default()
        })
        .unwrap();

        let request = RequestData {
            url: "https://example.com/api".to_string(),
            method: "GET".to_string(),
            host: "example.com".to_string(),
            path: "/api".to_string(),
            protocol: "https".to_string(),
            client_ip: "127.0.0.1".to_string(),
            client_app: None,
            headers: HashMap::new(),
            body: None,
        };
        let ctx = ScriptContext {
            request_id: "net-body-too-large".to_string(),
            script_name: "net-body-too-large".to_string(),
            script_type: ScriptType::Request,
            values: HashMap::new(),
            matched_rules: vec![],
        };

        let script = r#"
            var opt = { method: "POST", body: "0123456789" };
            net.fetch("http://127.0.0.1/", JSON.stringify(opt));
        "#;

        let result = sandbox.execute_request_script(script, &request, &ctx);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("请求体过大"));
    }

    #[test]
    fn test_net_fetch_body_null_and_object_request_too_large() {
        let mut sandbox = Sandbox::new(SandboxConfig {
            allow_network: true,
            max_net_request_bytes: 8,
            ..Default::default()
        })
        .unwrap();

        let request = RequestData {
            url: "https://example.com/api".to_string(),
            method: "GET".to_string(),
            host: "example.com".to_string(),
            path: "/api".to_string(),
            protocol: "https".to_string(),
            client_ip: "127.0.0.1".to_string(),
            client_app: None,
            headers: HashMap::new(),
            body: None,
        };
        let ctx = ScriptContext {
            request_id: "net-body-variants".to_string(),
            script_name: "net-body-variants".to_string(),
            script_type: ScriptType::Request,
            values: HashMap::new(),
            matched_rules: vec![],
        };

        let big_b64 = base64::engine::general_purpose::STANDARD.encode(b"0123456789");

        let script = format!(
            r#"
            try {{
                var opt1 = {{ method: "POST", body: null, bodyBase64: "{big_b64}" }};
                net.fetch("http://127.0.0.1/", JSON.stringify(opt1));
            }} catch (e) {{
                log.error("nullBody:" + e.message);
            }}
            try {{
                var opt2 = {{ method: "POST", body: {{ nested: "0123456789" }} }};
                net.fetch("http://127.0.0.1/", JSON.stringify(opt2));
            }} catch (e) {{
                log.error("objBody:" + e.message);
            }}
            "#
        );

        let result = sandbox.execute_request_script(&script, &request, &ctx);
        assert!(result.is_ok());
        let (_, logs) = result.unwrap();
        let msgs: Vec<&str> = logs.iter().map(|l| l.message.as_str()).collect();
        assert!(msgs.iter().any(|m| m.contains("nullBody:")));
        assert!(msgs.iter().any(|m| m.contains("objBody:")));
        for msg in msgs {
            assert!(msg.contains("请求体过大"));
        }
    }

    #[test]
    fn test_net_fetch_response_body_too_large() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            use std::io::{Read, Write};

            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 1024];
                let _ = stream.read(&mut buf);
                let body = "A".repeat(64);
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(response.as_bytes());
            }
        });

        let mut sandbox = Sandbox::new(SandboxConfig {
            allow_network: true,
            max_net_response_bytes: 16,
            ..Default::default()
        })
        .unwrap();

        let request = RequestData {
            url: "https://example.com/api".to_string(),
            method: "GET".to_string(),
            host: "example.com".to_string(),
            path: "/api".to_string(),
            protocol: "https".to_string(),
            client_ip: "127.0.0.1".to_string(),
            client_app: None,
            headers: HashMap::new(),
            body: None,
        };
        let ctx = ScriptContext {
            request_id: "net-resp-too-large".to_string(),
            script_name: "net-resp-too-large".to_string(),
            script_type: ScriptType::Request,
            values: HashMap::new(),
            matched_rules: vec![],
        };

        let script = format!(
            r#"
            net.fetch("http://{addr}/big", null);
            "#
        );

        let result = sandbox.execute_request_script(&script, &request, &ctx);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("响应体过大"));
    }

    #[test]
    fn test_file_append_exists_remove_and_list_dir() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut sandbox = Sandbox::new(SandboxConfig {
            file_root: Some(tmp.path().to_path_buf()),
            file_allowed_dirs: vec![],
            ..Default::default()
        })
        .unwrap();

        let request = RequestData {
            url: "https://example.com/api".to_string(),
            method: "GET".to_string(),
            host: "example.com".to_string(),
            path: "/api".to_string(),
            protocol: "https".to_string(),
            client_ip: "127.0.0.1".to_string(),
            client_app: Some("test".to_string()),
            headers: HashMap::new(),
            body: None,
        };
        let ctx = ScriptContext {
            request_id: "file-append".to_string(),
            script_name: "file-append".to_string(),
            script_type: ScriptType::Request,
            values: HashMap::new(),
            matched_rules: vec![],
        };

        let script = r#"
            if (!file.enabled) {
                throw new Error("file.disabled");
            }
            file.writeText("state/log.txt", "hello");
            file.appendText("state/log.txt", "-world");
            var names = file.listDir("state");
            log.info("names:" + names.join(","));
            var exists_before = file.exists("state/log.txt");
            log.info("exists_before:" + exists_before);
            file.remove("state/log.txt");
            var exists_after = file.exists("state/log.txt");
            log.info("exists_after:" + exists_after);
        "#;

        let result = sandbox.execute_request_script(script, &request, &ctx);
        assert!(result.is_ok());
        let (_, logs) = result.unwrap();
        let msgs: Vec<&str> = logs.iter().map(|l| l.message.as_str()).collect();
        assert!(msgs.iter().any(|m| m.contains("names:log.txt")));
        assert!(msgs.iter().any(|m| m.contains("exists_before:true")));
        assert!(msgs.iter().any(|m| m.contains("exists_after:false")));
        assert!(!tmp.path().join("state/log.txt").exists());
    }

    #[test]
    fn test_decode_script_error_cases() {
        let mut sandbox = Sandbox::new(SandboxConfig::default()).unwrap();

        let request = RequestData {
            url: "https://example.com/api".to_string(),
            method: "GET".to_string(),
            host: "example.com".to_string(),
            path: "/api".to_string(),
            protocol: "https".to_string(),
            client_ip: "127.0.0.1".to_string(),
            client_app: None,
            headers: HashMap::new(),
            body: None,
        };
        let response = ResponseData::default();
        let ctx = ScriptContext {
            request_id: "decode-errors".to_string(),
            script_name: "decode-errors".to_string(),
            script_type: ScriptType::Decode,
            values: HashMap::new(),
            matched_rules: vec![],
        };

        // 未输出任何结果
        let script_no_output = "";
        let result = sandbox.execute_decode_script(
            script_no_output,
            "request",
            &request,
            b"body",
            &response,
            b"",
            &ctx,
        );
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("未输出结果"));

        // 输出不是合法 JSON
        let script_invalid_json = r#"ctx.output = "not-json";"#;
        let result = sandbox.execute_decode_script(
            script_invalid_json,
            "request",
            &request,
            b"body",
            &response,
            b"",
            &ctx,
        );
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("decode 输出不是有效 JSON"));

        // 缺少 code 字段
        let script_missing_code = r#"ctx.output = { data: "x", msg: "y" };"#;
        let result = sandbox.execute_decode_script(
            script_missing_code,
            "request",
            &request,
            b"body",
            &response,
            b"",
            &ctx,
        );
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("decode 输出缺少字符串字段 code"));
    }

    #[test]
    fn test_execute_response_script_js_exception_produces_error() {
        let mut sandbox = Sandbox::new(SandboxConfig::default()).unwrap();

        let response = ResponseData {
            status: 200,
            status_text: "OK".to_string(),
            headers: HashMap::new(),
            body: Some("body".to_string()),
            request: RequestData::default(),
        };
        let ctx = ScriptContext {
            request_id: "resp-js-exc".to_string(),
            script_name: "resp-js-exc".to_string(),
            script_type: ScriptType::Response,
            values: HashMap::new(),
            matched_rules: vec![],
        };

        let script = r#"
            function boom() { throw new Error("response boom"); }
            boom();
        "#;

        let result = sandbox.execute_response_script(script, &response, &ctx);
        assert!(result.is_err());
        let err = result.unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("response boom"));
    }

    #[test]
    fn test_execute_decode_script_js_exception_produces_error() {
        let mut sandbox = Sandbox::new(SandboxConfig::default()).unwrap();

        let request = RequestData::default();
        let response = ResponseData::default();
        let ctx = ScriptContext {
            request_id: "decode-js-exc".to_string(),
            script_name: "decode-js-exc".to_string(),
            script_type: ScriptType::Decode,
            values: HashMap::new(),
            matched_rules: vec![],
        };

        let script = r#"
            function broken() { throw new Error("decode boom"); }
            broken();
        "#;

        let result =
            sandbox.execute_decode_script(script, "request", &request, b"", &response, b"", &ctx);
        assert!(result.is_err());
        let err = result.unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("decode boom"));
    }

    #[test]
    fn test_decode_script_non_object_json_error() {
        let mut sandbox = Sandbox::new(SandboxConfig::default()).unwrap();

        let request = RequestData::default();
        let response = ResponseData::default();
        let ctx = ScriptContext {
            request_id: "decode-non-object".to_string(),
            script_name: "decode-non-object".to_string(),
            script_type: ScriptType::Decode,
            values: HashMap::new(),
            matched_rules: vec![],
        };

        let script = r#"ctx.output = "123";"#;

        let result =
            sandbox.execute_decode_script(script, "request", &request, b"", &response, b"", &ctx);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("decode 输出必须是 JSON object"));
    }

    #[test]
    fn test_decode_script_uses_global_output_variable() {
        let mut sandbox = Sandbox::new(SandboxConfig::default()).unwrap();

        let request = RequestData::default();
        let response = ResponseData::default();
        let ctx = ScriptContext {
            request_id: "decode-global-output".to_string(),
            script_name: "decode-global-output".to_string(),
            script_type: ScriptType::Decode,
            values: HashMap::new(),
            matched_rules: vec![],
        };

        let script = r#"
            var output = { code: "0", data: "from-global", msg: "ok" };
        "#;

        let (out, _logs) = sandbox
            .execute_decode_script(script, "request", &request, b"", &response, b"", &ctx)
            .unwrap();

        assert_eq!(out.code, "0");
        assert_eq!(out.data, "from-global");
        assert_eq!(out.msg, "ok");
    }
}

#[cfg(test)]
mod more_tests_trunc_and_limits {
    use super::*;

    #[test]
    fn test_file_read_text_respects_max_file_bytes() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut sandbox = Sandbox::new(SandboxConfig {
            file_root: Some(tmp.path().to_path_buf()),
            max_file_bytes: 8,
            ..Default::default()
        })
        .unwrap();

        std::fs::write(tmp.path().join("big.txt"), "0123456789").unwrap();

        let request = RequestData {
            url: "https://example.com/api".to_string(),
            method: "GET".to_string(),
            host: "example.com".to_string(),
            path: "/api".to_string(),
            protocol: "https".to_string(),
            client_ip: "127.0.0.1".to_string(),
            client_app: None,
            headers: HashMap::new(),
            body: None,
        };
        let ctx = ScriptContext {
            request_id: "file-read-too-large".to_string(),
            script_name: "file-read-too-large".to_string(),
            script_type: ScriptType::Request,
            values: HashMap::new(),
            matched_rules: vec![],
        };

        let script = r#"
            file.readText("big.txt");
        "#;

        let result = sandbox.execute_request_script(script, &request, &ctx);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("文件过大"));
    }

    #[test]
    fn test_file_write_and_append_respects_max_file_bytes() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut sandbox = Sandbox::new(SandboxConfig {
            file_root: Some(tmp.path().to_path_buf()),
            max_file_bytes: 8,
            ..Default::default()
        })
        .unwrap();

        let request = RequestData {
            url: "https://example.com/api".to_string(),
            method: "GET".to_string(),
            host: "example.com".to_string(),
            path: "/api".to_string(),
            protocol: "https".to_string(),
            client_ip: "127.0.0.1".to_string(),
            client_app: Some("test".to_string()),
            headers: HashMap::new(),
            body: None,
        };
        let ctx = ScriptContext {
            request_id: "file-write-append".to_string(),
            script_name: "file-write-append".to_string(),
            script_type: ScriptType::Request,
            values: HashMap::new(),
            matched_rules: vec![],
        };

        let script = r#"
            try {
                file.writeText("big.txt", "0123456789");
            } catch (e) {
                log.error("writeError:" + e.message);
            }
            try {
                file.appendText("ok.txt", "0123456789");
            } catch (e) {
                log.error("appendError:" + e.message);
            }
        "#;

        let result = sandbox.execute_request_script(script, &request, &ctx);
        assert!(result.is_ok());
        let (_, logs) = result.unwrap();
        let msgs: Vec<&str> = logs.iter().map(|l| l.message.as_str()).collect();
        assert!(msgs.iter().any(|m| m.contains("writeError")));
        assert!(msgs.iter().any(|m| m.contains("写入内容过大")));
        assert!(msgs.iter().any(|m| m.contains("appendError")));
        assert!(msgs.iter().any(|m| m.contains("追加内容过大")));
    }

    #[test]
    fn test_file_remove_directory() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = tmp.path().join("dir");
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("sub").join("a.txt"), "ok").unwrap();

        let mut sandbox = Sandbox::new(SandboxConfig {
            file_root: Some(tmp.path().to_path_buf()),
            file_allowed_dirs: vec![],
            ..Default::default()
        })
        .unwrap();

        let request = RequestData {
            url: "https://example.com/api".to_string(),
            method: "GET".to_string(),
            host: "example.com".to_string(),
            path: "/api".to_string(),
            protocol: "https".to_string(),
            client_ip: "127.0.0.1".to_string(),
            client_app: None,
            headers: HashMap::new(),
            body: None,
        };
        let ctx = ScriptContext {
            request_id: "file-remove-dir".to_string(),
            script_name: "file-remove-dir".to_string(),
            script_type: ScriptType::Request,
            values: HashMap::new(),
            matched_rules: vec![],
        };

        assert!(dir.exists());

        let script = r#"
            file.remove("dir");
        "#;
        let result = sandbox.execute_request_script(script, &request, &ctx);
        assert!(result.is_ok());
        assert!(!tmp.path().join("dir").exists());
    }

    #[test]
    fn test_decode_request_body_truncation_flags() {
        let mut sandbox = Sandbox::new(SandboxConfig::default()).unwrap();

        let request = RequestData {
            url: "https://example.com/api".to_string(),
            method: "GET".to_string(),
            host: "example.com".to_string(),
            path: "/api".to_string(),
            protocol: "https".to_string(),
            client_ip: "127.0.0.1".to_string(),
            client_app: None,
            headers: HashMap::new(),
            body: None,
        };
        let response = ResponseData::default();
        let ctx = ScriptContext {
            request_id: "decode-trunc-req".to_string(),
            script_name: "decode-trunc-req".to_string(),
            script_type: ScriptType::Decode,
            values: HashMap::new(),
            matched_rules: vec![],
        };

        let script = r#"
            ctx.output = {
              code: "0",
              data: JSON.stringify({
                bodySize: request.bodySize,
                hexTruncated: request.bodyHexTruncated,
                textTruncated: request.bodyTextTruncated
              }),
              msg: ""
            };
        "#;

        let big_body = vec![b'A'; 300 * 1024];
        let (out, _) = sandbox
            .execute_decode_script(script, "request", &request, &big_body, &response, &[], &ctx)
            .unwrap();

        let data: serde_json::Value = serde_json::from_str(&out.data).unwrap();
        assert_eq!(data["bodySize"].as_u64().unwrap(), big_body.len() as u64);
        assert!(data["hexTruncated"].as_bool().unwrap());
        assert!(data["textTruncated"].as_bool().unwrap());
    }

    #[test]
    fn test_decode_response_body_truncation_flags() {
        let mut sandbox = Sandbox::new(SandboxConfig::default()).unwrap();

        let mut req_headers = HashMap::new();
        req_headers.insert("X-Req".to_string(), "1".to_string());
        let request = RequestData {
            headers: req_headers.clone(),
            ..Default::default()
        };
        let mut resp_headers = HashMap::new();
        resp_headers.insert("X-Resp".to_string(), "2".to_string());
        let response = ResponseData {
            status: 200,
            status_text: "OK".to_string(),
            headers: resp_headers,
            body: None,
            request: RequestData {
                headers: req_headers,
                ..Default::default()
            },
        };
        let ctx = ScriptContext {
            request_id: "decode-trunc-resp".to_string(),
            script_name: "decode-trunc-resp".to_string(),
            script_type: ScriptType::Decode,
            values: HashMap::new(),
            matched_rules: vec![],
        };

        let script = r#"
            ctx.output = {
              code: "0",
              data: JSON.stringify({
                bodySize: response.bodySize,
                hexTruncated: response.bodyHexTruncated,
                textTruncated: response.bodyTextTruncated
              }),
              msg: ""
            };
        "#;

        let big_body = vec![b'B'; 300 * 1024];
        let (out, _) = sandbox
            .execute_decode_script(
                script,
                "response",
                &request,
                &[],
                &response,
                &big_body,
                &ctx,
            )
            .unwrap();

        let data: serde_json::Value = serde_json::from_str(&out.data).unwrap();
        assert_eq!(data["bodySize"].as_u64().unwrap(), big_body.len() as u64);
        assert!(data["hexTruncated"].as_bool().unwrap());
        assert!(data["textTruncated"].as_bool().unwrap());
    }
}
