use std::collections::BTreeMap;
use std::io::Write as _;
use std::process::Stdio;
use std::time::Duration;

use bifrost_core::text::truncate_chars_with_suffix;
use sha1::{Digest, Sha1};
use tokio::io::AsyncReadExt;
use tokio::process::Command as TokioCommand;
use tracing::{debug, info, warn};

use super::types::{ImTaskExecutionRequest, ImTaskRun, TaskRunStatus, TaskScript, TriggerSource};

/// Result of matching an event to a route, carrying named captures from regex.
/// Used by the event router to pass match context into the executor.
#[derive(Debug, Clone, Default)]
pub struct MatchContext {
    pub named_captures: BTreeMap<String, String>,
    pub message_text: Option<String>,
}

pub struct ImTaskExecutor;

impl ImTaskExecutor {
    /// Execute a task script and produce an `ImTaskRun` record.
    ///
    /// The executor:
    /// 1. Prepares the script (tempfile for `script_text`, direct path for `script_file`)
    /// 2. Builds environment variables (user-defined + IM-specific)
    /// 3. Spawns the child process with timeout
    /// 4. Captures stdout/stderr up to `max_output_bytes`
    /// 5. Computes SHA1 digests of full output
    /// 6. Returns a completed `ImTaskRun`
    pub async fn execute(
        request: &ImTaskExecutionRequest,
        run_id: String,
        route_id: Option<String>,
        schedule_id: Option<String>,
        match_context: &MatchContext,
    ) -> ImTaskRun {
        let now_ms = current_time_ms();
        let mut run = ImTaskRun {
            run_id,
            trigger_source: request.trigger_source,
            route_id,
            schedule_id,
            provider_id: Some(request.provider_id.clone()),
            target_id: None,
            status: TaskRunStatus::Running,
            started_at: now_ms,
            ended_at: None,
            duration_ms: None,
            exit_code: None,
            input_preview: Some(super::types::script_task_input_preview(&request.script)),
            stdout_preview: None,
            stderr_preview: None,
            stdout_digest: None,
            stderr_digest: None,
            error: None,
            task_type: Some(super::types::ScheduleTaskType::Script),
            runner_id: None,
            agent_final_response: None,
            agent_tool_calls: Vec::new(),
            agent_plan_steps: None,
        };

        // Prepare the script (tempfile or file path)
        let script_path = match Self::prepare_script(&request.script) {
            Ok(path) => path,
            Err(e) => {
                run.status = TaskRunStatus::Failed;
                run.error = Some(format!("script preparation failed: {e}"));
                run.ended_at = Some(current_time_ms());
                run.duration_ms = Some(run.ended_at.unwrap() - run.started_at);
                return run;
            }
        };

        // Build environment
        let env = Self::build_env(
            &request.script,
            &request.provider_id,
            request.trigger_source,
            match_context,
        );

        // Execute with timeout
        let timeout = Duration::from_millis(request.timeout_ms);
        let max_output = request.max_output_bytes as usize;

        match tokio::time::timeout(
            timeout,
            Self::run_script(&script_path, &request.script.cwd, &env, max_output),
        )
        .await
        {
            Ok(Ok((exit_code, stdout, stderr))) => {
                let ended = current_time_ms();
                run.exit_code = Some(exit_code);
                run.stdout_preview = truncate_preview(&stdout, 4096);
                run.stderr_preview = truncate_preview(&stderr, 4096);
                run.stdout_digest = (!stdout.is_empty()).then(|| sha1_hex(&stdout));
                run.stderr_digest = (!stderr.is_empty()).then(|| sha1_hex(&stderr));
                run.status = if exit_code == 0 {
                    TaskRunStatus::Success
                } else {
                    TaskRunStatus::Failed
                };
                run.ended_at = Some(ended);
                run.duration_ms = Some(ended - run.started_at);
            }
            Ok(Err(e)) => {
                let ended = current_time_ms();
                run.status = TaskRunStatus::Failed;
                run.error = Some(format!("execution error: {e}"));
                run.ended_at = Some(ended);
                run.duration_ms = Some(ended - run.started_at);
            }
            Err(_) => {
                // Timeout
                let ended = current_time_ms();
                warn!(timeout_ms = request.timeout_ms, "task execution timed out");
                run.status = TaskRunStatus::Timeout;
                run.error = Some(format!("timeout after {}ms", request.timeout_ms));
                run.ended_at = Some(ended);
                run.duration_ms = Some(ended - run.started_at);
            }
        }

        // Clean up tempfile if script_text was used
        if request.script.script_text.is_some() {
            let _ = std::fs::remove_file(&script_path);
        }

        debug!(
            run_id = %run.run_id,
            status = ?run.status,
            duration_ms = ?run.duration_ms,
            "task execution completed"
        );

        run
    }

    /// Prepare script for execution. Returns the path to the script file.
    fn prepare_script(script: &TaskScript) -> Result<String, String> {
        if let Some(ref text) = script.script_text {
            let extension = if cfg!(windows) { "cmd" } else { "sh" };
            let tmp_path = std::env::temp_dir().join(format!(
                "bifrost-im-script-{}.{}",
                uuid::Uuid::new_v4().as_simple(),
                extension
            ));
            let mut file = std::fs::File::create(&tmp_path)
                .map_err(|e| format!("failed to create temp script file: {e}"))?;
            file.write_all(text.as_bytes())
                .map_err(|e| format!("failed to write script to temp file: {e}"))?;
            file.flush()
                .map_err(|e| format!("failed to flush temp file: {e}"))?;

            // Make executable on Unix
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&tmp_path, std::fs::Permissions::from_mode(0o755));
            }

            Ok(tmp_path.to_string_lossy().into_owned())
        } else if let Some(ref file) = script.script_file {
            // Validate the script file exists
            if !std::path::Path::new(file).exists() {
                return Err(format!("script_file not found: {file}"));
            }
            Ok(file.clone())
        } else {
            Err("neither script_text nor script_file provided".to_string())
        }
    }

    /// Build environment variables for the script execution.
    fn build_env(
        script: &TaskScript,
        provider_id: &str,
        trigger_source: TriggerSource,
        match_context: &MatchContext,
    ) -> BTreeMap<String, String> {
        let mut env = script.env.clone();

        // IM-specific environment variables
        env.insert(
            "BIFROST_IM_PROVIDER_ID".to_string(),
            provider_id.to_string(),
        );
        env.insert(
            "BIFROST_IM_TRIGGER_SOURCE".to_string(),
            trigger_source_str(trigger_source).to_string(),
        );

        // Message text if available
        if let Some(ref text) = match_context.message_text {
            env.insert("BIFROST_IM_MESSAGE_TEXT".to_string(), text.clone());
        }

        // Named regex captures as BIFROST_IM_MATCH_{NAME}
        for (name, value) in &match_context.named_captures {
            let key = format!("BIFROST_IM_MATCH_{}", name.to_uppercase());
            env.insert(key, value.clone());
        }

        env
    }

    /// Spawn and run the script, returning (exit_code, stdout, stderr).
    async fn run_script(
        script_path: &str,
        cwd: &Option<String>,
        env: &BTreeMap<String, String>,
        max_output_bytes: usize,
    ) -> Result<(i32, String, String), String> {
        let mut command = if cfg!(windows) {
            let mut command = TokioCommand::new("cmd");
            command
                .arg("/C")
                .arg(format!("\"{}\"", script_path.replace('"', "\"\"")));
            command
        } else {
            let mut command = TokioCommand::new("sh");
            command.arg("-c").arg(script_path);
            command
        };

        if let Some(ref dir) = cwd {
            command.current_dir(dir);
        }

        // Set environment variables
        for (key, value) in env {
            command.env(key, value);
        }

        command.stdout(Stdio::piped());
        command.stderr(Stdio::piped());
        command.stdin(Stdio::null());

        // Kill on drop ensures child is terminated if future is dropped (timeout)
        command.kill_on_drop(true);

        let mut child = command
            .spawn()
            .map_err(|e| format!("failed to spawn process: {e}"))?;

        // Read stdout and stderr concurrently with size limits
        let mut stdout_buf = Vec::with_capacity(max_output_bytes.min(65536));
        let mut stderr_buf = Vec::with_capacity(max_output_bytes.min(65536));

        if let Some(mut stdout) = child.stdout.take() {
            let limit = max_output_bytes;
            let buf = &mut stdout_buf;
            let mut tmp = vec![0u8; 8192];
            loop {
                match stdout.read(&mut tmp).await {
                    Ok(0) => break,
                    Ok(n) => {
                        let remaining = limit.saturating_sub(buf.len());
                        if remaining == 0 {
                            break;
                        }
                        let take = n.min(remaining);
                        buf.extend_from_slice(&tmp[..take]);
                    }
                    Err(_) => break,
                }
            }
        }

        if let Some(mut stderr) = child.stderr.take() {
            let limit = max_output_bytes;
            let buf = &mut stderr_buf;
            let mut tmp = vec![0u8; 8192];
            loop {
                match stderr.read(&mut tmp).await {
                    Ok(0) => break,
                    Ok(n) => {
                        let remaining = limit.saturating_sub(buf.len());
                        if remaining == 0 {
                            break;
                        }
                        let take = n.min(remaining);
                        buf.extend_from_slice(&tmp[..take]);
                    }
                    Err(_) => break,
                }
            }
        }

        let status = child
            .wait()
            .await
            .map_err(|e| format!("failed to wait for process: {e}"))?;

        let exit_code = status.code().unwrap_or(-1);
        let stdout = String::from_utf8_lossy(&stdout_buf).to_string();
        let stderr = String::from_utf8_lossy(&stderr_buf).to_string();

        info!(
            exit_code,
            stdout_len = stdout.len(),
            stderr_len = stderr.len(),
            "script execution finished"
        );

        Ok((exit_code, stdout, stderr))
    }
}

/// Truncate output to a preview string with max length.
fn truncate_preview(output: &str, max_len: usize) -> Option<String> {
    if output.is_empty() {
        return None;
    }
    Some(truncate_chars_with_suffix(
        output,
        max_len,
        "...[truncated]",
    ))
}

/// Compute SHA1 hex digest of a string.
fn sha1_hex(input: &str) -> String {
    let mut hasher = Sha1::new();
    hasher.update(input.as_bytes());
    let result = hasher.finalize();
    result.iter().fold(String::with_capacity(40), |mut acc, b| {
        use std::fmt::Write;
        let _ = write!(acc, "{:02x}", b);
        acc
    })
}

fn current_time_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn trigger_source_str(source: TriggerSource) -> &'static str {
    match source {
        TriggerSource::InboundMessage => "inbound_message",
        TriggerSource::Schedule => "schedule",
        TriggerSource::ManualRun => "manual_run",
        TriggerSource::RemoteManualRun => "remote_manual_run",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn success_script_text() -> &'static str {
        "echo hello_from_test"
    }

    fn failure_script_text() -> &'static str {
        if cfg!(windows) {
            "exit /B 42"
        } else {
            "exit 42"
        }
    }

    fn long_running_script_text() -> &'static str {
        if cfg!(windows) {
            "ping -n 60 127.0.0.1 > nul"
        } else {
            "sleep 60"
        }
    }

    #[test]
    fn test_sha1_hex_known_value() {
        assert_eq!(
            sha1_hex("hello"),
            "aaf4c61ddcc5e8a2dabede0f3b482cd9aea9434d"
        );
    }

    #[test]
    fn test_sha1_hex_empty() {
        assert_eq!(sha1_hex(""), "da39a3ee5e6b4b0d3255bfef95601890afd80709");
    }

    #[test]
    fn test_truncate_preview_short() {
        let result = truncate_preview("short", 100);
        assert_eq!(result, Some("short".to_string()));
    }

    #[test]
    fn test_truncate_preview_long() {
        let long = "x".repeat(200);
        let result = truncate_preview(&long, 50);
        assert!(result.is_some());
        let r = result.unwrap();
        assert!(r.ends_with("...[truncated]"));
        assert!(r.len() < 200);
    }

    #[test]
    fn test_truncate_preview_empty() {
        assert_eq!(truncate_preview("", 100), None);
    }

    #[test]
    fn test_truncate_preview_multibyte_boundary() {
        let output = "前".repeat(20);
        let result = truncate_preview(&output, 5).expect("preview");

        assert_eq!(result, format!("{}...[truncated]", "前".repeat(5)));
    }

    #[test]
    fn test_build_env_basic() {
        let script = TaskScript {
            script_text: Some("echo hi".to_string()),
            script_file: None,
            cwd: None,
            env: BTreeMap::new(),
        };
        let ctx = MatchContext {
            named_captures: BTreeMap::from([("service".to_string(), "bifrost".to_string())]),
            message_text: Some("/check bifrost".to_string()),
        };
        let env = ImTaskExecutor::build_env(&script, "prov1", TriggerSource::InboundMessage, &ctx);
        assert_eq!(env.get("BIFROST_IM_PROVIDER_ID").unwrap(), "prov1");
        assert_eq!(
            env.get("BIFROST_IM_TRIGGER_SOURCE").unwrap(),
            "inbound_message"
        );
        assert_eq!(env.get("BIFROST_IM_MATCH_SERVICE").unwrap(), "bifrost");
        assert_eq!(
            env.get("BIFROST_IM_MESSAGE_TEXT").unwrap(),
            "/check bifrost"
        );
    }

    #[test]
    fn test_trigger_source_str() {
        assert_eq!(
            trigger_source_str(TriggerSource::InboundMessage),
            "inbound_message"
        );
        assert_eq!(trigger_source_str(TriggerSource::Schedule), "schedule");
        assert_eq!(trigger_source_str(TriggerSource::ManualRun), "manual_run");
        assert_eq!(
            trigger_source_str(TriggerSource::RemoteManualRun),
            "remote_manual_run"
        );
    }

    #[tokio::test]
    async fn test_execute_script_text_success() {
        let request = ImTaskExecutionRequest {
            provider_id: "test_provider".to_string(),
            trigger_source: TriggerSource::ManualRun,
            policy_id: None,
            script_policy_binding: None,
            script: TaskScript {
                script_text: Some(success_script_text().to_string()),
                script_file: None,
                cwd: None,
                env: BTreeMap::new(),
            },
            timeout_ms: 5000,
            max_output_bytes: 4096,
        };

        let run = ImTaskExecutor::execute(
            &request,
            "run_001".to_string(),
            None,
            None,
            &MatchContext::default(),
        )
        .await;

        assert_eq!(run.run_id, "run_001");
        assert_eq!(run.status, TaskRunStatus::Success);
        assert_eq!(run.exit_code, Some(0));
        assert!(run
            .stdout_preview
            .as_deref()
            .unwrap_or("")
            .contains("hello_from_test"));
        assert!(run.ended_at.is_some());
        assert!(run.duration_ms.is_some());
    }

    #[tokio::test]
    async fn test_execute_script_text_failure() {
        let request = ImTaskExecutionRequest {
            provider_id: "test_provider".to_string(),
            trigger_source: TriggerSource::Schedule,
            policy_id: None,
            script_policy_binding: None,
            script: TaskScript {
                script_text: Some(failure_script_text().to_string()),
                script_file: None,
                cwd: None,
                env: BTreeMap::new(),
            },
            timeout_ms: 5000,
            max_output_bytes: 4096,
        };

        let run = ImTaskExecutor::execute(
            &request,
            "run_002".to_string(),
            None,
            Some("sched_1".to_string()),
            &MatchContext::default(),
        )
        .await;

        assert_eq!(run.status, TaskRunStatus::Failed);
        assert_eq!(run.exit_code, Some(42));
    }

    #[tokio::test]
    async fn test_execute_timeout() {
        let request = ImTaskExecutionRequest {
            provider_id: "test_provider".to_string(),
            trigger_source: TriggerSource::ManualRun,
            policy_id: None,
            script_policy_binding: None,
            script: TaskScript {
                script_text: Some(long_running_script_text().to_string()),
                script_file: None,
                cwd: None,
                env: BTreeMap::new(),
            },
            timeout_ms: 100,
            max_output_bytes: 4096,
        };

        let run = ImTaskExecutor::execute(
            &request,
            "run_003".to_string(),
            None,
            None,
            &MatchContext::default(),
        )
        .await;

        assert_eq!(run.status, TaskRunStatus::Timeout);
        assert!(run.error.as_deref().unwrap_or("").contains("timeout"));
    }

    #[test]
    fn test_prepare_script_no_source() {
        let script = TaskScript {
            script_text: None,
            script_file: None,
            cwd: None,
            env: BTreeMap::new(),
        };
        let result = ImTaskExecutor::prepare_script(&script);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("neither"));
    }

    #[test]
    fn test_prepare_script_file_not_found() {
        let script = TaskScript {
            script_text: None,
            script_file: Some("/nonexistent/path/script.sh".to_string()),
            cwd: None,
            env: BTreeMap::new(),
        };
        let result = ImTaskExecutor::prepare_script(&script);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not found"));
    }
}
