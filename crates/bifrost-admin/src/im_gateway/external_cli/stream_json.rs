use super::*;

struct PendingStreamJsonGuide {
    command: live_guide::LiveGuideCommand,
    request_id: String,
    interrupt_acknowledged: bool,
}

struct StreamJsonRunCleanup {
    run_id: String,
    session_key: Option<String>,
    pid: u32,
    armed: bool,
}

impl StreamJsonRunCleanup {
    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for StreamJsonRunCleanup {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        if let Some(session_key) = self.session_key.as_deref() {
            live_guide::remove_session(session_key, &self.run_id);
        }
        if self.pid != 0 {
            let _ = terminate_process(self.pid);
        }
        ACTIVE_RUNS.remove(&self.run_id);
        remove_active_sessions_for_run(&self.run_id);
    }
}

pub(super) async fn run_command(
    run_id: &str,
    session_key: Option<&str>,
    spec: CommandSpec,
    prompt: String,
    stop_marker_path: PathBuf,
    progress_tx: Option<mpsc::Sender<ExternalCliProgressEvent>>,
) -> Result<CommandOutput, String> {
    let (stdout_path, stderr_path) = external_cli_log_paths(&stop_marker_path);
    let mut command = Command::new(&spec.executable);
    command
        .args(&spec.args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    if let Some(work_dir) = spec.work_dir.as_ref() {
        command.current_dir(work_dir);
    }
    apply_command_environment(&mut command, &spec.env);

    let mut child = command
        .spawn()
        .map_err(|error| format!("spawn stream-json external cli failed: {error}"))?;
    let pid = child.id().unwrap_or(0);
    if pid != 0 {
        ACTIVE_RUNS.insert(run_id.to_string(), pid);
    }
    if let Some(session_key) = session_key {
        ACTIVE_SESSIONS.insert(session_key.to_string(), run_id.to_string());
    }
    let mut cleanup = StreamJsonRunCleanup {
        run_id: run_id.to_string(),
        session_key: session_key.map(str::to_string),
        pid,
        armed: true,
    };

    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| "stream-json stdin unavailable".to_string())?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "stream-json stdout unavailable".to_string())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "stream-json stderr unavailable".to_string())?;
    let (stdout, stdout_tee_task) =
        tee_external_cli_output(stdout, stdout_path, "stream-json stdout").await?;
    let (stderr, stderr_tee_task) =
        tee_external_cli_output(stderr, stderr_path, "stream-json stderr").await?;
    write_user_frame(&mut stdin, &prompt).await?;

    let mut lines = tokio::io::BufReader::new(stdout).lines();
    let stderr_task = tokio::spawn(read_stderr_lines(stderr));
    let (guide_tx, mut guide_rx) = mpsc::unbounded_channel::<live_guide::LiveGuideCommand>();
    let mut pending_guide = None::<PendingStreamJsonGuide>;
    let mut interrupted_results_to_ignore = 0usize;
    let mut thread_id = None::<String>;
    let mut initial_prompt_replayed = false;
    let mut guide_registered = false;
    let mut stdout_bytes = Vec::new();
    let mut events = Vec::new();
    let mut parse_state = ExternalCliParseState::default();
    let mut tool_started_at = HashMap::<String, u64>::new();
    let timeout_secs = spec.timeout_secs;
    let timeout_sleep = async move {
        match timeout_secs {
            Some(seconds) => sleep(Duration::from_secs(seconds)).await,
            None => std::future::pending::<()>().await,
        }
    };
    tokio::pin!(timeout_sleep);
    let mut terminal_status = None::<ExternalCliRunStatus>;
    let mut terminal_error = None::<String>;
    let mut stop_request_id = None::<String>;
    let mut stop_deadline = None::<std::pin::Pin<Box<tokio::time::Sleep>>>;

    while terminal_status.is_none() {
        tokio::select! {
            line = lines.next_line() => {
                let Some(line) = line.map_err(|error| format!("read stream-json stdout failed: {error}"))? else {
                    terminal_error = Some("stream-json runner exited before a result frame".to_string());
                    terminal_status = Some(ExternalCliRunStatus::Failed);
                    break;
                };
                append_tail(&mut stdout_bytes, line.as_bytes(), MAX_CAPTURED_STREAM_BYTES);
                append_tail(&mut stdout_bytes, b"\n", MAX_CAPTURED_STREAM_BYTES);
                let raw = serde_json::from_str::<serde_json::Value>(&line).ok();
                if thread_id.is_none() {
                    thread_id = raw.as_ref().and_then(stream_json_session_id);
                }
                if let Some(raw) = raw.as_ref() {
                    if let Some((request_id, succeeded, reason)) = control_response(raw) {
                        if stop_request_id.as_deref() == Some(request_id.as_str()) {
                            if !succeeded {
                                terminal_error = Some(reason.unwrap_or_else(|| {
                                    "Claude Code rejected the stop interrupt".to_string()
                                }));
                            }
                            terminal_status = Some(ExternalCliRunStatus::Stopped);
                            continue;
                        }
                        if pending_guide
                            .as_ref()
                            .is_some_and(|pending| pending.request_id == request_id)
                        {
                            let mut pending = pending_guide.take().expect("pending guide exists");
                            if !succeeded {
                                let _ = pending.command.ack_tx.send(live_guide::rejected_guide(
                                    pending.command.guide_id,
                                    thread_id.clone(),
                                    None,
                                    reason.unwrap_or_else(|| {
                                        "Claude Code rejected the interrupt request".to_string()
                                    }),
                                ));
                            } else if pending.command.ack_tx.is_closed() {
                                // The caller already fell back to the FIFO queue. Do not also send
                                // the same text through Claude's stream-json session.
                            } else {
                                match write_user_frame(&mut stdin, &pending.command.message).await {
                                    Ok(()) => {
                                        pending.interrupt_acknowledged = true;
                                        interrupted_results_to_ignore += 1;
                                        pending_guide = Some(pending);
                                    }
                                    Err(error) => {
                                        let _ = pending.command.ack_tx.send(live_guide::rejected_guide(
                                            pending.command.guide_id,
                                            thread_id.clone(),
                                            None,
                                            error,
                                        ));
                                    }
                                }
                            }
                        }
                    }
                }
                if let Some(replayed) = raw.as_ref().and_then(replayed_user_text) {
                    if !initial_prompt_replayed && replayed == prompt {
                        initial_prompt_replayed = true;
                    } else if pending_guide.as_ref().is_some_and(|pending| {
                        pending.interrupt_acknowledged && pending.command.message == replayed
                    }) {
                        let pending = pending_guide.take().expect("pending guide exists");
                        let _ = pending.command.ack_tx.send(live_guide::accepted_guide(
                            pending.command.guide_id,
                            thread_id.clone(),
                            None,
                        ));
                    }
                }
                if !guide_registered && initial_prompt_replayed {
                    if let (Some(session_key), Some(thread_id)) = (session_key, thread_id.as_ref()) {
                        guide_registered = live_guide::register_session(
                            session_key,
                            run_id,
                            live_guide::ActiveGuideHandle {
                                run_id: run_id.to_string(),
                                thread_id: Some(thread_id.clone()),
                                turn_id: None,
                                guide_tx: guide_tx.clone(),
                            },
                        );
                    }
                }
                let interrupted_result = raw.as_ref().is_some_and(is_interrupted_result)
                    && interrupted_results_to_ignore > 0;
                if interrupted_result {
                    interrupted_results_to_ignore -= 1;
                }
                if !interrupted_result {
                    if let Some(event) =
                        parse_progress_event_line_with_state(&line, &mut parse_state)
                    {
                        let observed_at = now_ms();
                        for mut event in expand_subagent_progress_event(event) {
                            enrich_progress_event_observation(
                                &mut event,
                                observed_at,
                                &mut tool_started_at,
                            );
                            if let Some(progress_tx) = progress_tx.as_ref() {
                                let _ = progress_tx.try_send(event.clone());
                            }
                            events.push(event);
                        }
                    }
                }
                if !interrupted_result {
                    if let Some(raw) = raw.as_ref().filter(|raw| {
                        raw.get("type").and_then(serde_json::Value::as_str) == Some("result")
                    }) {
                        if stop_request_id.is_some() && is_interrupted_result(raw) {
                            terminal_status = Some(ExternalCliRunStatus::Stopped);
                            continue;
                        }
                        let succeeded = !raw
                            .get("is_error")
                            .and_then(serde_json::Value::as_bool)
                            .unwrap_or(false)
                            && raw
                                .get("subtype")
                                .and_then(serde_json::Value::as_str)
                                .is_none_or(|value| value == "success");
                        terminal_status = Some(if succeeded {
                            ExternalCliRunStatus::Succeeded
                        } else {
                            ExternalCliRunStatus::Failed
                        });
                    }
                }
            }
            guide = guide_rx.recv() => {
                let Some(guide) = guide else { continue; };
                if stop_request_id.is_some() {
                    let _ = guide.ack_tx.send(live_guide::rejected_guide(
                        guide.guide_id,
                        thread_id.clone(),
                        None,
                        "Claude Code session is stopping".to_string(),
                    ));
                    continue;
                }
                if session_key.is_some_and(|session_key| !live_guide::active_session_is_owned_by(session_key, run_id)) {
                    let _ = guide.ack_tx.send(live_guide::rejected_guide(
                        guide.guide_id,
                        thread_id.clone(),
                        None,
                        "active session was replaced before guide delivery".to_string(),
                    ));
                    continue;
                }
                if pending_guide.is_some() {
                    let _ = guide.ack_tx.send(live_guide::rejected_guide(
                        guide.guide_id,
                        thread_id.clone(),
                        None,
                        "another Claude Code guide redirect is awaiting acknowledgement".to_string(),
                    ));
                    continue;
                }
                let request_id = format!("bifrost-guide-{}", uuid::Uuid::new_v4());
                match write_interrupt_frame(&mut stdin, &request_id).await {
                    Ok(()) => {
                        pending_guide = Some(PendingStreamJsonGuide {
                            command: guide,
                            request_id,
                            interrupt_acknowledged: false,
                        });
                    }
                    Err(error) => {
                        let _ = guide.ack_tx.send(live_guide::rejected_guide(
                            guide.guide_id,
                            thread_id.clone(),
                            None,
                            error,
                        ));
                    }
                }
            }
            _ = wait_for_stop_marker(stop_marker_path.clone()), if stop_request_id.is_none() => {
                let request_id = format!("bifrost-stop-{}", uuid::Uuid::new_v4());
                match write_interrupt_frame(&mut stdin, &request_id).await {
                    Ok(()) => {
                        stop_request_id = Some(request_id);
                        stop_deadline = Some(Box::pin(sleep(Duration::from_millis(
                            WORKER_TRANSPORT_STOP_GRACE_MS,
                        ))));
                    }
                    Err(error) => {
                        terminal_error = Some(format!(
                            "failed to send Claude Code stop interrupt: {error}"
                        ));
                        terminal_status = Some(ExternalCliRunStatus::Stopped);
                    }
                }
            }
            _ = &mut timeout_sleep, if stop_request_id.is_none() => {
                let request_id = format!("bifrost-timeout-{}", uuid::Uuid::new_v4());
                if let Err(error) = write_interrupt_frame(&mut stdin, &request_id).await {
                    terminal_error = Some(format!(
                        "failed to interrupt timed-out Claude Code run: {error}"
                    ));
                }
                let timeout_error = format!(
                    "stream-json runner timed out after {} seconds",
                    timeout_secs.unwrap_or_default(),
                );
                terminal_error = Some(match terminal_error.take() {
                    Some(interrupt_error) => format!("{timeout_error}; {interrupt_error}"),
                    None => timeout_error,
                });
                terminal_status = Some(ExternalCliRunStatus::TimedOut);
            }
            _ = async {
                match stop_deadline.as_mut() {
                    Some(deadline) => deadline.await,
                    None => std::future::pending::<()>().await,
                }
            } => {
                terminal_error = Some(
                    "Claude Code did not acknowledge the stop interrupt before termination"
                        .to_string(),
                );
                terminal_status = Some(ExternalCliRunStatus::Stopped);
            }
        }
    }

    if let Some(session_key) = session_key {
        live_guide::remove_session(session_key, run_id);
    }
    if let Some(pending) = pending_guide {
        let _ = pending.command.ack_tx.send(live_guide::rejected_guide(
            pending.command.guide_id,
            thread_id.clone(),
            None,
            "Claude Code session completed before guide redirect acknowledgement".to_string(),
        ));
    }
    reject_queued_guides(&mut guide_rx, thread_id.clone());

    drop(stdin);
    let status = terminal_status.unwrap_or(ExternalCliRunStatus::Failed);
    if !matches!(
        status,
        ExternalCliRunStatus::Succeeded | ExternalCliRunStatus::Failed
    ) && pid != 0
    {
        let _ = terminate_process(pid);
    }
    let exit_status = wait_for_stream_json_child(&mut child, pid).await;
    ACTIVE_RUNS.remove(run_id);
    remove_active_sessions_for_run(run_id);
    cleanup.disarm();
    let mut stderr = if exit_status.is_some() {
        stderr_task
            .await
            .map_err(|error| format!("join stream-json stderr task failed: {error}"))??
    } else {
        stderr_task.abort();
        let _ = stderr_task.await;
        b"stream-json runner did not exit after termination\n".to_vec()
    };
    join_external_cli_tee(stdout_tee_task, "stream-json stdout").await?;
    join_external_cli_tee(stderr_tee_task, "stream-json stderr").await?;
    if let Some(error) = terminal_error {
        if !stderr.is_empty() && !stderr.ends_with(b"\n") {
            stderr.push(b'\n');
        }
        stderr.extend_from_slice(error.as_bytes());
        stderr.push(b'\n');
    }
    Ok(CommandOutput {
        status,
        exit_code: exit_status.and_then(|status| status.code()),
        stdout: stdout_bytes,
        stderr,
        events,
    })
}

fn reject_queued_guides(
    guide_rx: &mut mpsc::UnboundedReceiver<live_guide::LiveGuideCommand>,
    thread_id: Option<String>,
) {
    while let Ok(command) = guide_rx.try_recv() {
        let _ = command.ack_tx.send(live_guide::rejected_guide(
            command.guide_id,
            thread_id.clone(),
            None,
            "turn is no longer active".to_string(),
        ));
    }
}

async fn wait_for_stream_json_child(
    child: &mut tokio::process::Child,
    pid: u32,
) -> Option<std::process::ExitStatus> {
    match timeout(Duration::from_millis(WORKER_STOP_GRACE_MS), child.wait()).await {
        Ok(Ok(status)) => return Some(status),
        Ok(Err(_)) => return None,
        Err(_) => {}
    }
    if pid != 0 {
        let _ = terminate_process(pid);
    }
    timeout(Duration::from_millis(WORKER_STOP_GRACE_MS), child.wait())
        .await
        .ok()
        .and_then(Result::ok)
}

async fn write_user_frame(
    stdin: &mut tokio::process::ChildStdin,
    message: &str,
) -> Result<(), String> {
    let frame = build_user_frame(message);
    let mut bytes = serde_json::to_vec(&frame)
        .map_err(|error| format!("serialize stream-json user frame failed: {error}"))?;
    bytes.push(b'\n');
    stdin
        .write_all(&bytes)
        .await
        .map_err(|error| format!("write stream-json user frame failed: {error}"))?;
    stdin
        .flush()
        .await
        .map_err(|error| format!("flush stream-json user frame failed: {error}"))
}

async fn write_interrupt_frame(
    stdin: &mut tokio::process::ChildStdin,
    request_id: &str,
) -> Result<(), String> {
    write_stream_json_frame(stdin, &build_interrupt_frame(request_id)).await
}

async fn write_stream_json_frame(
    stdin: &mut tokio::process::ChildStdin,
    frame: &serde_json::Value,
) -> Result<(), String> {
    let mut bytes = serde_json::to_vec(frame)
        .map_err(|error| format!("serialize stream-json frame failed: {error}"))?;
    bytes.push(b'\n');
    stdin
        .write_all(&bytes)
        .await
        .map_err(|error| format!("write stream-json frame failed: {error}"))?;
    stdin
        .flush()
        .await
        .map_err(|error| format!("flush stream-json frame failed: {error}"))
}

fn build_interrupt_frame(request_id: &str) -> serde_json::Value {
    serde_json::json!({
        "type": "control_request",
        "request_id": request_id,
        "request": {"subtype": "interrupt"}
    })
}

fn control_response(raw: &serde_json::Value) -> Option<(String, bool, Option<String>)> {
    if raw.get("type").and_then(serde_json::Value::as_str) != Some("control_response") {
        return None;
    }
    let response = raw.get("response")?;
    let request_id = response.get("request_id")?.as_str()?.to_string();
    let succeeded = response.get("subtype").and_then(serde_json::Value::as_str) == Some("success");
    let reason = response
        .get("error")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    Some((request_id, succeeded, reason))
}

fn is_interrupted_result(raw: &serde_json::Value) -> bool {
    raw.get("type").and_then(serde_json::Value::as_str) == Some("result")
        && raw.get("subtype").and_then(serde_json::Value::as_str) == Some("error_during_execution")
}

fn build_user_frame(message: &str) -> serde_json::Value {
    serde_json::json!({
        "type": "user",
        "message": {
            "role": "user",
            "content": [{"type": "text", "text": message}]
        }
    })
}

fn stream_json_session_id(raw: &serde_json::Value) -> Option<String> {
    raw.get("session_id")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn replayed_user_text(raw: &serde_json::Value) -> Option<String> {
    if raw.get("type").and_then(serde_json::Value::as_str) != Some("user") {
        return None;
    }
    let content = raw.get("message")?.get("content")?;
    if let Some(text) = content.as_str() {
        return Some(text.to_string());
    }
    Some(
        content
            .as_array()?
            .iter()
            .filter_map(|block| {
                (block.get("type").and_then(serde_json::Value::as_str) == Some("text"))
                    .then(|| block.get("text").and_then(serde_json::Value::as_str))
                    .flatten()
            })
            .collect::<Vec<_>>()
            .join(""),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    const MOCK_RUN_TIMEOUT_SECS: u64 = 15;

    #[cfg(unix)]
    fn mock_executable(temp_dir: &tempfile::TempDir, name: &str, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;

        let executable = temp_dir.path().join(name);
        std::fs::write(&executable, body).unwrap();
        let mut permissions = std::fs::metadata(&executable).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&executable, permissions).unwrap();
        executable
    }

    #[cfg(unix)]
    fn mock_spec(executable: &Path, timeout_secs: Option<u64>) -> CommandSpec {
        CommandSpec {
            executable: executable.display().to_string(),
            args: Vec::new(),
            env: BTreeMap::new(),
            work_dir: None,
            timeout_secs,
        }
    }

    #[cfg(unix)]
    async fn wait_for_active_handle(session_key: &str) -> live_guide::ActiveGuideHandle {
        timeout(Duration::from_secs(10), async {
            loop {
                if let Some(handle) = live_guide::active_handle(session_key) {
                    break handle;
                }
                sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("live guide handle")
    }

    #[cfg(unix)]
    async fn wait_for_path(path: &Path) {
        timeout(Duration::from_secs(10), async {
            while !path.exists() {
                sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("mock runner marker");
    }

    #[test]
    fn user_frame_contains_stream_json_message() {
        assert_eq!(
            build_user_frame("focus")["message"]["content"][0]["text"],
            "focus"
        );
        let interrupt = build_interrupt_frame("request-1");
        assert_eq!(interrupt["type"], "control_request");
        assert_eq!(interrupt["request_id"], "request-1");
        assert_eq!(interrupt["request"]["subtype"], "interrupt");
    }

    #[test]
    fn replayed_user_text_supports_string_and_blocks() {
        assert_eq!(
            replayed_user_text(&serde_json::json!({
                "type": "user",
                "message": {"content": "focus"}
            }))
            .as_deref(),
            Some("focus")
        );
        assert_eq!(
            replayed_user_text(&build_user_frame("focus")).as_deref(),
            Some("focus")
        );
        assert_eq!(
            control_response(&serde_json::json!({
                "type": "control_response",
                "response": {
                    "subtype": "error",
                    "request_id": "request-2",
                    "error": "not active"
                }
            })),
            Some((
                "request-2".to_string(),
                false,
                Some("not active".to_string())
            ))
        );
        assert!(is_interrupted_result(&serde_json::json!({
            "type": "result",
            "subtype": "error_during_execution"
        })));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn mock_stream_json_runner_redirects_live_guide_in_same_process() {
        use std::os::unix::fs::PermissionsExt;

        let temp_dir = tempfile::tempdir().unwrap();
        let executable = temp_dir.path().join("mock-claude");
        let pid_log = temp_dir.path().join("pid.log");
        std::fs::write(
            &executable,
            r#"#!/usr/bin/env python3
import json
import os
import sys

def send(value):
    print(json.dumps(value, separators=(",", ":")), flush=True)

with open(os.environ["PID_LOG"], "a", encoding="utf-8") as handle:
    handle.write(str(os.getpid()) + "\n")

first = json.loads(sys.stdin.readline())
send({"type":"system","subtype":"init","session_id":"claude-stream-session"})
send(first)
interrupt = json.loads(sys.stdin.readline())
assert interrupt["type"] == "control_request"
assert interrupt["request"]["subtype"] == "interrupt"
send({"type":"control_response","response":{"subtype":"success","request_id":interrupt["request_id"],"response":{}}})
guide = json.loads(sys.stdin.readline())
send(guide)
send({"type":"result","subtype":"error_during_execution","is_error":True,"session_id":"claude-stream-session"})
send({"type":"system","subtype":"init","session_id":"claude-stream-session"})
send({"type":"assistant","message":{"content":[{"type":"text","text":"guided result"}]},"session_id":"claude-stream-session"})
send({"type":"result","subtype":"success","is_error":False,"result":"guided result","session_id":"claude-stream-session"})
"#,
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&executable).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&executable, permissions).unwrap();

        let session_key = "mock-stream-json-session";
        let run_id = "mock-stream-json-run";
        let spec = CommandSpec {
            executable: executable.display().to_string(),
            args: Vec::new(),
            env: BTreeMap::from([("PID_LOG".to_string(), pid_log.display().to_string())]),
            work_dir: None,
            timeout_secs: Some(10),
        };
        ACTIVE_SESSIONS.insert(session_key.to_string(), run_id.to_string());
        let stop_marker = temp_dir.path().join("stop");
        let run_task = tokio::spawn(run_command(
            run_id,
            Some(session_key),
            spec,
            "initial task".to_string(),
            stop_marker,
            None,
        ));
        let guided = live_guide::request_session_guide(
            session_key,
            "guide-stream".to_string(),
            "focus on tests".to_string(),
        )
        .await;
        assert!(guided.accepted, "guide rejected: {guided:?}");
        assert_eq!(guided.thread_id.as_deref(), Some("claude-stream-session"));
        assert_eq!(guided.turn_id, None);

        let output = run_task.await.unwrap().unwrap();
        assert_eq!(output.status, ExternalCliRunStatus::Succeeded);
        assert!(output.events.iter().any(|event| {
            event.event_type == ExternalCliProgressEventType::AssistantFinal
                && event.content == "guided result"
        }));
        assert!(!output
            .events
            .iter()
            .any(|event| event.event_type == ExternalCliProgressEventType::RunFailed));
        assert_eq!(
            output
                .events
                .iter()
                .filter(|event| event.event_type == ExternalCliProgressEventType::RunFinished)
                .count(),
            1
        );
        assert_eq!(std::fs::read_to_string(pid_log).unwrap().lines().count(), 1);
        assert!(!ACTIVE_RUNS.contains_key(run_id));
        assert!(!ACTIVE_SESSIONS.contains_key(session_key));
        assert!(live_guide::active_handle(session_key).is_none());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn result_frame_force_kills_runner_that_does_not_exit() {
        use std::os::unix::fs::PermissionsExt;

        let temp_dir = tempfile::tempdir().unwrap();
        let executable = temp_dir.path().join("mock-claude-stuck");
        std::fs::write(
            &executable,
            r#"#!/usr/bin/env python3
import json
import signal
import sys
import time

json.loads(sys.stdin.readline())
print(json.dumps({"type":"result","subtype":"success","is_error":False,"result":"done"}), flush=True)
signal.signal(signal.SIGTERM, signal.SIG_IGN)
while True:
    time.sleep(1)
"#,
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&executable).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&executable, permissions).unwrap();

        let run_id = "mock-stream-json-stuck-run";
        let started = tokio::time::Instant::now();
        let output = tokio::time::timeout(
            // The cleanup path has two 1.5s grace windows. Leave scheduler
            // headroom for full-workspace test contention while still proving
            // that a SIGTERM-ignoring child cannot hang the runner forever.
            Duration::from_secs(15),
            run_command(
                run_id,
                None,
                CommandSpec {
                    executable: executable.display().to_string(),
                    args: Vec::new(),
                    env: BTreeMap::new(),
                    work_dir: None,
                    timeout_secs: Some(10),
                },
                "initial task".to_string(),
                temp_dir.path().join("stop"),
                None,
            ),
        )
        .await
        .expect("stuck runner cleanup must be bounded")
        .unwrap();

        assert_eq!(output.status, ExternalCliRunStatus::Succeeded);
        assert!(
            started.elapsed() < Duration::from_secs(15),
            "SIGTERM-ignoring runner cleanup exceeded the bounded timeout"
        );
        assert!(!ACTIVE_RUNS.contains_key(run_id));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn runner_eof_before_result_reports_failure_and_stderr() {
        let temp_dir = tempfile::tempdir().unwrap();
        let executable = mock_executable(
            &temp_dir,
            "mock-claude-eof",
            r#"#!/usr/bin/env python3
import sys
sys.stdin.readline()
sys.stderr.write("mock stderr without newline")
"#,
        );
        let (progress_tx, mut progress_rx) = mpsc::channel(EXTERNAL_CLI_PROGRESS_CHANNEL_CAPACITY);
        let output = run_command(
            "mock-stream-json-eof-run",
            None,
            CommandSpec {
                work_dir: Some(temp_dir.path().to_path_buf()),
                env: BTreeMap::from([("BIFROST_STREAM_JSON_TEST".to_string(), "1".to_string())]),
                ..mock_spec(&executable, Some(MOCK_RUN_TIMEOUT_SECS))
            },
            "initial task".to_string(),
            temp_dir.path().join("stop"),
            Some(progress_tx),
        )
        .await
        .unwrap();

        assert_eq!(output.status, ExternalCliRunStatus::Failed);
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(stderr.contains("mock stderr without newline\nstream-json runner exited"));
        assert!(progress_rx.try_recv().is_err());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn runner_timeout_and_stop_marker_terminate_child() {
        let temp_dir = tempfile::tempdir().unwrap();
        let executable = mock_executable(
            &temp_dir,
            "mock-claude-wait",
            r#"#!/usr/bin/env python3
import sys
import time
sys.stdin.readline()
time.sleep(30)
"#,
        );

        let timed_out = run_command(
            "mock-stream-json-timeout-run",
            None,
            mock_spec(&executable, Some(1)),
            "initial task".to_string(),
            temp_dir.path().join("timeout-stop"),
            None,
        )
        .await
        .unwrap();
        assert_eq!(timed_out.status, ExternalCliRunStatus::TimedOut);
        assert!(String::from_utf8_lossy(&timed_out.stderr)
            .contains("stream-json runner timed out after 1 seconds"));

        let stop_marker = temp_dir.path().join("requested-stop");
        let stop_marker_for_task = stop_marker.clone();
        let stop_run = tokio::spawn(run_command(
            "mock-stream-json-stop-run",
            None,
            mock_spec(&executable, None),
            "initial task".to_string(),
            stop_marker_for_task,
            None,
        ));
        sleep(Duration::from_millis(100)).await;
        tokio::fs::write(&stop_marker, b"stop").await.unwrap();
        let stopped = stop_run.await.unwrap().unwrap();
        assert_eq!(stopped.status, ExternalCliRunStatus::Stopped);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn stop_protocol_covers_ack_rejection_interrupted_result_and_deadline() {
        let temp_dir = tempfile::tempdir().unwrap();
        let executable = mock_executable(
            &temp_dir,
            "mock-claude-stop-protocol",
            r#"#!/usr/bin/env python3
import json
import os
import sys
import time

mode = os.environ["STOP_MODE"]
marker = os.environ["STOP_SEEN"]

def send(value):
    print(json.dumps(value, separators=(",", ":")), flush=True)

first = json.loads(sys.stdin.readline())
first["session_id"] = "stop-protocol-session"
send(first)
for line in sys.stdin:
    frame = json.loads(line)
    if frame.get("type") != "control_request":
        continue
    with open(marker, "w", encoding="utf-8") as handle:
        handle.write(frame["request_id"])
    if mode == "ack":
        send({"type":"control_response","response":{"subtype":"success","request_id":frame["request_id"],"response":{}}})
    elif mode == "reject":
        time.sleep(0.2)
        send({"type":"control_response","response":{"subtype":"error","request_id":frame["request_id"],"error":"not active"}})
    elif mode == "result":
        send({"type":"result","subtype":"error_during_execution","is_error":True,"result":"interrupted","session_id":"stop-protocol-session"})
    elif mode == "ignore":
        pass
"#,
        );

        for mode in ["ack", "reject", "result", "ignore"] {
            let run_id = format!("stream-json-stop-{mode}-run");
            let session_key = format!("stream-json-stop-{mode}-session");
            let stop_marker = temp_dir.path().join(format!("stop-{mode}"));
            let stop_seen = temp_dir.path().join(format!("seen-{mode}"));
            let mut spec = mock_spec(&executable, Some(MOCK_RUN_TIMEOUT_SECS));
            spec.env = BTreeMap::from([
                ("STOP_MODE".to_string(), mode.to_string()),
                ("STOP_SEEN".to_string(), stop_seen.display().to_string()),
            ]);
            ACTIVE_SESSIONS.insert(session_key.clone(), run_id.clone());
            let stop_marker_for_run = stop_marker.clone();
            let run_id_for_run = run_id.clone();
            let session_key_for_run = session_key.clone();
            let run = tokio::spawn(async move {
                run_command(
                    &run_id_for_run,
                    Some(&session_key_for_run),
                    spec,
                    "wait".to_string(),
                    stop_marker_for_run,
                    None,
                )
                .await
            });
            wait_for_active_handle(&session_key).await;
            tokio::fs::write(&stop_marker, b"stop").await.unwrap();
            wait_for_path(&stop_seen).await;
            if mode == "reject" {
                let rejected = live_guide::request_session_guide(
                    &session_key,
                    "late-guide".to_string(),
                    "too late".to_string(),
                )
                .await;
                assert_eq!(
                    rejected.reason.as_deref(),
                    Some("Claude Code session is stopping")
                );
            }
            let output = timeout(Duration::from_secs(8), run)
                .await
                .expect("stream-json stop should finish")
                .unwrap()
                .unwrap();
            assert_eq!(output.status, ExternalCliRunStatus::Stopped);
            let stderr = String::from_utf8_lossy(&output.stderr);
            match mode {
                "reject" => assert!(stderr.contains("not active"), "{stderr}"),
                "ignore" => assert!(stderr.contains("did not acknowledge"), "{stderr}"),
                _ => {}
            }
            assert!(!ACTIVE_RUNS.contains_key(&run_id));
            assert!(!ACTIVE_SESSIONS.contains_key(&session_key));
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn stop_and_timeout_report_closed_interrupt_stdin() {
        let temp_dir = tempfile::tempdir().unwrap();
        let executable = mock_executable(
            &temp_dir,
            "mock-claude-closed-stop-stdin",
            r#"#!/usr/bin/env python3
import json
import os
import sys
import time

first = json.loads(sys.stdin.readline())
first["session_id"] = "closed-stop-session"
print(json.dumps(first, separators=(",", ":")), flush=True)
with open(os.environ["CLOSED_STDIN_READY"], "w", encoding="utf-8") as handle:
    handle.write("ready")
os.close(0)
time.sleep(30)
"#,
        );

        for trigger in ["stop", "timeout"] {
            let ready = temp_dir.path().join(format!("ready-{trigger}"));
            let stop_marker = temp_dir.path().join(format!("stop-{trigger}"));
            let mut spec = mock_spec(&executable, (trigger == "timeout").then_some(1));
            spec.env = BTreeMap::from([(
                "CLOSED_STDIN_READY".to_string(),
                ready.display().to_string(),
            )]);
            let run_id = format!("stream-json-closed-{trigger}-run");
            let stop_marker_for_run = stop_marker.clone();
            let run = tokio::spawn(async move {
                run_command(
                    &run_id,
                    None,
                    spec,
                    "wait".to_string(),
                    stop_marker_for_run,
                    None,
                )
                .await
            });
            wait_for_path(&ready).await;
            if trigger == "stop" {
                tokio::fs::write(&stop_marker, b"stop").await.unwrap();
            }
            let output = timeout(Duration::from_secs(8), run)
                .await
                .expect("closed-stdin stream-json should finish")
                .unwrap()
                .unwrap();
            let stderr = String::from_utf8_lossy(&output.stderr);
            if trigger == "stop" {
                assert_eq!(output.status, ExternalCliRunStatus::Stopped);
                assert!(
                    stderr.contains("failed to send Claude Code stop interrupt"),
                    "{stderr}"
                );
            } else {
                assert_eq!(output.status, ExternalCliRunStatus::TimedOut);
                assert!(
                    stderr.contains("failed to interrupt timed-out Claude Code run"),
                    "{stderr}"
                );
            }
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn interrupt_rejection_uses_default_reason_and_run_can_finish() {
        let temp_dir = tempfile::tempdir().unwrap();
        let executable = mock_executable(
            &temp_dir,
            "mock-claude-reject-interrupt",
            r#"#!/usr/bin/env python3
import json
import sys

def send(value):
    print(json.dumps(value, separators=(",", ":")), flush=True)

first = json.loads(sys.stdin.readline())
send({"type":"system","subtype":"init","session_id":"reject-session"})
send(first)
interrupt = json.loads(sys.stdin.readline())
send({"type":"control_response","response":{"subtype":"error","request_id":interrupt["request_id"]}})
send({"type":"result","subtype":"success","is_error":False,"result":"done","session_id":"reject-session"})
"#,
        );
        let session_key = "mock-stream-json-reject-session";
        let run_id = "mock-stream-json-reject-run";
        ACTIVE_SESSIONS.insert(session_key.to_string(), run_id.to_string());
        let run_task = tokio::spawn(run_command(
            run_id,
            Some(session_key),
            mock_spec(&executable, Some(MOCK_RUN_TIMEOUT_SECS)),
            "initial task".to_string(),
            temp_dir.path().join("stop"),
            None,
        ));

        let rejected = live_guide::request_session_guide(
            session_key,
            "guide-rejected".to_string(),
            "focus tests".to_string(),
        )
        .await;
        assert_eq!(
            rejected.reason.as_deref(),
            Some("Claude Code rejected the interrupt request")
        );
        assert_eq!(
            run_task.await.unwrap().unwrap().status,
            ExternalCliRunStatus::Succeeded
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn rejected_stop_interrupt_is_stopped_with_diagnostic() {
        let temp_dir = tempfile::tempdir().unwrap();
        let executable = mock_executable(
            &temp_dir,
            "mock-claude-reject-stop",
            r#"#!/usr/bin/env python3
import json
import sys

def send(value):
    print(json.dumps(value, separators=(",", ":")), flush=True)

first = json.loads(sys.stdin.readline())
send({"type":"system","subtype":"init","session_id":"reject-stop-session"})
send(first)
interrupt = json.loads(sys.stdin.readline())
send({"type":"control_response","response":{"subtype":"error","request_id":interrupt["request_id"],"error":"run already completed"}})
"#,
        );
        let session_key = format!("mock-stream-json-reject-stop-{}", uuid::Uuid::new_v4());
        let run_id = format!("mock-stream-json-reject-stop-run-{}", uuid::Uuid::new_v4());
        let stop_marker = temp_dir.path().join("stop");
        let run = tokio::spawn({
            let run_id = run_id.clone();
            let session_key = session_key.clone();
            let stop_marker = stop_marker.clone();
            async move {
                run_command(
                    &run_id,
                    Some(&session_key),
                    mock_spec(&executable, Some(MOCK_RUN_TIMEOUT_SECS)),
                    "initial task".to_string(),
                    stop_marker,
                    None,
                )
                .await
            }
        });

        wait_for_active_handle(&session_key).await;
        tokio::fs::write(&stop_marker, b"stop").await.unwrap();
        let output = timeout(Duration::from_secs(8), run)
            .await
            .expect("rejected stop interrupt should terminate")
            .unwrap()
            .unwrap();

        assert_eq!(output.status, ExternalCliRunStatus::Stopped);
        assert!(String::from_utf8_lossy(&output.stderr).contains("run already completed"));
        assert!(!ACTIVE_RUNS.contains_key(&run_id));
        assert!(live_guide::active_handle(&session_key).is_none());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn rejected_stop_interrupt_without_reason_uses_default_diagnostic() {
        let temp_dir = tempfile::tempdir().unwrap();
        let executable = mock_executable(
            &temp_dir,
            "mock-claude-reject-stop-default",
            r#"#!/usr/bin/env python3
import json
import sys

def send(value):
    print(json.dumps(value, separators=(",", ":")), flush=True)

first = json.loads(sys.stdin.readline())
send({"type":"system","subtype":"init","session_id":"reject-stop-default"})
send(first)
interrupt = json.loads(sys.stdin.readline())
send({"type":"control_response","response":{"subtype":"error","request_id":interrupt["request_id"]}})
"#,
        );
        let session_key = format!(
            "mock-stream-json-reject-stop-default-{}",
            uuid::Uuid::new_v4()
        );
        let stop_marker = temp_dir.path().join("stop");
        let run = tokio::spawn({
            let session_key = session_key.clone();
            let stop_marker = stop_marker.clone();
            async move {
                run_command(
                    "mock-stream-json-reject-stop-default-run",
                    Some(&session_key),
                    mock_spec(&executable, Some(MOCK_RUN_TIMEOUT_SECS)),
                    "initial task".to_string(),
                    stop_marker,
                    None,
                )
                .await
            }
        });

        wait_for_active_handle(&session_key).await;
        tokio::fs::write(&stop_marker, b"stop").await.unwrap();
        let output = run.await.unwrap().unwrap();
        assert_eq!(output.status, ExternalCliRunStatus::Stopped);
        assert!(String::from_utf8_lossy(&output.stderr)
            .contains("Claude Code rejected the stop interrupt"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn interrupted_result_after_stop_marker_is_terminal_stop() {
        let temp_dir = tempfile::tempdir().unwrap();
        let executable = mock_executable(
            &temp_dir,
            "mock-claude-interrupted-result",
            r#"#!/usr/bin/env python3
import json
import sys

def send(value):
    print(json.dumps(value, separators=(",", ":")), flush=True)

first = json.loads(sys.stdin.readline())
send({"type":"system","subtype":"init","session_id":"interrupted-result"})
send(first)
json.loads(sys.stdin.readline())
send({"type":"result","subtype":"error_during_execution","is_error":True})
"#,
        );
        let session_key = format!(
            "mock-stream-json-interrupted-result-{}",
            uuid::Uuid::new_v4()
        );
        let stop_marker = temp_dir.path().join("stop");
        let run = tokio::spawn({
            let session_key = session_key.clone();
            let stop_marker = stop_marker.clone();
            async move {
                run_command(
                    "mock-stream-json-interrupted-result-run",
                    Some(&session_key),
                    mock_spec(&executable, Some(MOCK_RUN_TIMEOUT_SECS)),
                    "initial task".to_string(),
                    stop_marker,
                    None,
                )
                .await
            }
        });

        wait_for_active_handle(&session_key).await;
        tokio::fs::write(&stop_marker, b"stop").await.unwrap();
        assert_eq!(
            run.await.unwrap().unwrap().status,
            ExternalCliRunStatus::Stopped
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn unacknowledged_stop_interrupt_hits_bounded_fallback() {
        let temp_dir = tempfile::tempdir().unwrap();
        let executable = mock_executable(
            &temp_dir,
            "mock-claude-no-stop-ack",
            r#"#!/usr/bin/env python3
import json
import os
import sys
import time

def send(value):
    print(json.dumps(value, separators=(",", ":")), flush=True)

first = json.loads(sys.stdin.readline())
send({"type":"system","subtype":"init","session_id":"no-stop-ack-session"})
send(first)
json.loads(sys.stdin.readline())
with open(os.environ["STOP_SEEN"], "w", encoding="utf-8") as handle:
    handle.write("seen")
time.sleep(30)
"#,
        );
        let session_key = format!("mock-stream-json-no-stop-ack-{}", uuid::Uuid::new_v4());
        let run_id = format!("mock-stream-json-no-stop-ack-run-{}", uuid::Uuid::new_v4());
        let stop_marker = temp_dir.path().join("stop");
        let stop_seen = temp_dir.path().join("stop-seen");
        let mut spec = mock_spec(&executable, Some(MOCK_RUN_TIMEOUT_SECS));
        spec.env
            .insert("STOP_SEEN".to_string(), stop_seen.display().to_string());
        let run = tokio::spawn({
            let run_id = run_id.clone();
            let session_key = session_key.clone();
            let stop_marker = stop_marker.clone();
            async move {
                run_command(
                    &run_id,
                    Some(&session_key),
                    spec,
                    "initial task".to_string(),
                    stop_marker,
                    None,
                )
                .await
            }
        });

        wait_for_active_handle(&session_key).await;
        tokio::fs::write(&stop_marker, b"stop").await.unwrap();
        timeout(Duration::from_secs(5), async {
            while !stop_seen.is_file() {
                sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("Claude stop interrupt observed");
        let rejected = live_guide::request_session_guide(
            &session_key,
            "guide-during-stop".to_string(),
            "must not be delivered".to_string(),
        )
        .await;
        assert_eq!(
            rejected.reason.as_deref(),
            Some("Claude Code session is stopping")
        );
        let output = timeout(Duration::from_secs(8), run)
            .await
            .expect("unacknowledged stop interrupt should hit fallback")
            .unwrap()
            .unwrap();

        assert_eq!(output.status, ExternalCliRunStatus::Stopped);
        assert!(String::from_utf8_lossy(&output.stderr)
            .contains("Claude Code did not acknowledge the stop interrupt before termination"));
        assert!(!ACTIVE_RUNS.contains_key(&run_id));
        assert!(live_guide::active_handle(&session_key).is_none());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn closed_stdin_stop_records_interrupt_write_failure() {
        let temp_dir = tempfile::tempdir().unwrap();
        let executable = mock_executable(
            &temp_dir,
            "mock-claude-closed-stop",
            r#"#!/usr/bin/env python3
import json
import os
import sys
import time

def send(value):
    print(json.dumps(value, separators=(",", ":")), flush=True)

first = json.loads(sys.stdin.readline())
send({"type":"system","subtype":"init","session_id":"closed-stop-session"})
send(first)
os.close(0)
with open(os.environ["STDIN_CLOSED"], "w", encoding="utf-8") as handle:
    handle.write("closed")
time.sleep(30)
"#,
        );
        let session_key = format!("mock-stream-json-closed-stop-{}", uuid::Uuid::new_v4());
        let run_id = format!("mock-stream-json-closed-stop-run-{}", uuid::Uuid::new_v4());
        let stop_marker = temp_dir.path().join("stop");
        let stdin_closed = temp_dir.path().join("stdin-closed");
        let mut spec = mock_spec(&executable, Some(MOCK_RUN_TIMEOUT_SECS));
        spec.env.insert(
            "STDIN_CLOSED".to_string(),
            stdin_closed.display().to_string(),
        );
        let run = tokio::spawn({
            let run_id = run_id.clone();
            let session_key = session_key.clone();
            let stop_marker = stop_marker.clone();
            async move {
                run_command(
                    &run_id,
                    Some(&session_key),
                    spec,
                    "initial task".to_string(),
                    stop_marker,
                    None,
                )
                .await
            }
        });

        wait_for_active_handle(&session_key).await;
        timeout(Duration::from_secs(5), async {
            while !stdin_closed.is_file() {
                sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("Claude stdin closed");
        tokio::fs::write(&stop_marker, b"stop").await.unwrap();
        let output = timeout(Duration::from_secs(8), run)
            .await
            .expect("closed stdin stop should terminate")
            .unwrap()
            .unwrap();

        assert_eq!(output.status, ExternalCliRunStatus::Stopped);
        assert!(String::from_utf8_lossy(&output.stderr)
            .contains("failed to send Claude Code stop interrupt"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn closed_stdin_timeout_records_interrupt_write_failure() {
        let temp_dir = tempfile::tempdir().unwrap();
        let executable = mock_executable(
            &temp_dir,
            "mock-claude-closed-timeout",
            r#"#!/usr/bin/env python3
import json
import os
import sys
import time

def send(value):
    print(json.dumps(value, separators=(",", ":")), flush=True)

first = json.loads(sys.stdin.readline())
send({"type":"system","subtype":"init","session_id":"closed-timeout-session"})
send(first)
os.close(0)
with open(os.environ["STDIN_CLOSED"], "w", encoding="utf-8") as handle:
    handle.write("closed")
time.sleep(30)
"#,
        );
        let stdin_closed = temp_dir.path().join("stdin-closed");
        let mut spec = mock_spec(&executable, Some(1));
        spec.env.insert(
            "STDIN_CLOSED".to_string(),
            stdin_closed.display().to_string(),
        );
        let run = tokio::spawn(run_command(
            "mock-stream-json-closed-timeout-run",
            None,
            spec,
            "initial task".to_string(),
            temp_dir.path().join("stop"),
            None,
        ));
        timeout(Duration::from_secs(5), async {
            while !stdin_closed.is_file() {
                sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("Claude timeout stdin closed");
        let output = timeout(Duration::from_secs(8), run)
            .await
            .expect("closed stdin timeout should terminate")
            .unwrap()
            .unwrap();

        assert_eq!(output.status, ExternalCliRunStatus::TimedOut);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("stream-json runner timed out after 1 seconds"));
        assert!(stderr.contains("failed to interrupt timed-out Claude Code run"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn pending_guide_rejects_parallel_redirect() {
        let temp_dir = tempfile::tempdir().unwrap();
        let executable = mock_executable(
            &temp_dir,
            "mock-claude-parallel-guide",
            r#"#!/usr/bin/env python3
import json
import os
import sys
import time

def send(value):
    print(json.dumps(value, separators=(",", ":")), flush=True)

first = json.loads(sys.stdin.readline())
send({"type":"system","subtype":"init","session_id":"parallel-session"})
send(first)
interrupt = json.loads(sys.stdin.readline())
with open(os.environ["INTERRUPT_MARKER"], "w", encoding="utf-8") as handle:
    handle.write("ready")
time.sleep(0.5)
send({"type":"control_response","response":{"subtype":"success","request_id":interrupt["request_id"]}})
guide = json.loads(sys.stdin.readline())
send(guide)
send({"type":"result","subtype":"error_during_execution","is_error":True,"session_id":"parallel-session"})
send({"type":"result","subtype":"success","is_error":False,"result":"done","session_id":"parallel-session"})
"#,
        );
        let session_key = "mock-stream-json-parallel-session";
        let run_id = "mock-stream-json-parallel-run";
        ACTIVE_SESSIONS.insert(session_key.to_string(), run_id.to_string());
        let interrupt_marker = temp_dir.path().join("interrupt-ready");
        let run_task = tokio::spawn(run_command(
            run_id,
            Some(session_key),
            CommandSpec {
                env: BTreeMap::from([(
                    "INTERRUPT_MARKER".to_string(),
                    interrupt_marker.display().to_string(),
                )]),
                ..mock_spec(&executable, Some(MOCK_RUN_TIMEOUT_SECS))
            },
            "initial task".to_string(),
            temp_dir.path().join("stop"),
            None,
        ));

        let first_guide = tokio::spawn(live_guide::request_session_guide(
            session_key,
            "guide-first".to_string(),
            "first guide".to_string(),
        ));
        wait_for_path(&interrupt_marker).await;
        let handle = wait_for_active_handle(session_key).await;
        let (ack_tx, ack_rx) = oneshot::channel();
        handle
            .guide_tx
            .send(live_guide::LiveGuideCommand {
                guide_id: "guide-second".to_string(),
                message: "second guide".to_string(),
                ack_tx,
            })
            .unwrap();
        let second = ack_rx.await.unwrap();
        assert_eq!(
            second.reason.as_deref(),
            Some("another Claude Code guide redirect is awaiting acknowledgement")
        );
        assert!(first_guide.await.unwrap().accepted);
        assert_eq!(
            run_task.await.unwrap().unwrap().status,
            ExternalCliRunStatus::Succeeded
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn replaced_session_rejects_direct_guide_command() {
        let temp_dir = tempfile::tempdir().unwrap();
        let executable = mock_executable(
            &temp_dir,
            "mock-claude-replaced-session",
            r#"#!/usr/bin/env python3
import json
import sys
import time

def send(value):
    print(json.dumps(value, separators=(",", ":")), flush=True)

first = json.loads(sys.stdin.readline())
send({"type":"system","subtype":"init","session_id":"replaced-session"})
send(first)
time.sleep(2)
send({"type":"result","subtype":"success","is_error":False,"result":"done","session_id":"replaced-session"})
"#,
        );
        let session_key = "mock-stream-json-replaced-session";
        let run_id = "mock-stream-json-replaced-run";
        ACTIVE_SESSIONS.insert(session_key.to_string(), run_id.to_string());
        let run_task = tokio::spawn(run_command(
            run_id,
            Some(session_key),
            mock_spec(&executable, Some(MOCK_RUN_TIMEOUT_SECS)),
            "initial task".to_string(),
            temp_dir.path().join("stop"),
            None,
        ));
        let handle = wait_for_active_handle(session_key).await;
        ACTIVE_SESSIONS.insert(session_key.to_string(), "replacement-run".to_string());
        let (ack_tx, ack_rx) = oneshot::channel();
        handle
            .guide_tx
            .send(live_guide::LiveGuideCommand {
                guide_id: "guide-replaced".to_string(),
                message: "should reject".to_string(),
                ack_tx,
            })
            .unwrap();
        let rejected = ack_rx.await.unwrap();
        assert_eq!(
            rejected.reason.as_deref(),
            Some("active session was replaced before guide delivery")
        );
        assert_eq!(
            run_task.await.unwrap().unwrap().status,
            ExternalCliRunStatus::Succeeded
        );
        ACTIVE_SESSIONS.remove(session_key);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn run_completion_rejects_pending_guide() {
        let temp_dir = tempfile::tempdir().unwrap();
        let executable = mock_executable(
            &temp_dir,
            "mock-claude-pending-result",
            r#"#!/usr/bin/env python3
import json
import sys

def send(value):
    print(json.dumps(value, separators=(",", ":")), flush=True)

first = json.loads(sys.stdin.readline())
send({"type":"system","subtype":"init","session_id":"pending-session"})
send(first)
json.loads(sys.stdin.readline())
send({"type":"result","subtype":"success","is_error":False,"result":"done","session_id":"pending-session"})
"#,
        );
        let session_key = "mock-stream-json-pending-session";
        let run_id = "mock-stream-json-pending-run";
        ACTIVE_SESSIONS.insert(session_key.to_string(), run_id.to_string());
        let run_task = tokio::spawn(run_command(
            run_id,
            Some(session_key),
            mock_spec(&executable, Some(MOCK_RUN_TIMEOUT_SECS)),
            "initial task".to_string(),
            temp_dir.path().join("stop"),
            None,
        ));
        let rejected = live_guide::request_session_guide(
            session_key,
            "guide-pending".to_string(),
            "pending guide".to_string(),
        )
        .await;
        assert_eq!(
            rejected.reason.as_deref(),
            Some("Claude Code session completed before guide redirect acknowledgement")
        );
        assert_eq!(
            run_task.await.unwrap().unwrap().status,
            ExternalCliRunStatus::Succeeded
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn closed_stdin_rejects_interrupt_write() {
        let temp_dir = tempfile::tempdir().unwrap();
        let executable = mock_executable(
            &temp_dir,
            "mock-claude-closed-stdin",
            r#"#!/usr/bin/env python3
import json
import os
import sys
import time

def send(value):
    print(json.dumps(value, separators=(",", ":")), flush=True)

first = json.loads(sys.stdin.readline())
send({"type":"system","subtype":"init","session_id":"closed-stdin-session"})
send(first)
os.close(0)
time.sleep(0.5)
send({"type":"result","subtype":"success","is_error":False,"result":"done","session_id":"closed-stdin-session"})
"#,
        );
        let session_key = "mock-stream-json-closed-stdin-session";
        let run_id = "mock-stream-json-closed-stdin-run";
        ACTIVE_SESSIONS.insert(session_key.to_string(), run_id.to_string());
        let run_task = tokio::spawn(run_command(
            run_id,
            Some(session_key),
            mock_spec(&executable, Some(MOCK_RUN_TIMEOUT_SECS)),
            "initial task".to_string(),
            temp_dir.path().join("stop"),
            None,
        ));
        let rejected = live_guide::request_session_guide(
            session_key,
            "guide-closed-stdin".to_string(),
            "cannot write".to_string(),
        )
        .await;
        assert!(!rejected.accepted);
        assert!(rejected.reason.as_deref().is_some_and(|reason| {
            reason.contains("write stream-json frame") || reason.contains("flush stream-json frame")
        }));
        assert_eq!(
            run_task.await.unwrap().unwrap().status,
            ExternalCliRunStatus::Succeeded
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn cancelled_guide_request_is_not_replayed_after_interrupt_ack() {
        let temp_dir = tempfile::tempdir().unwrap();
        let executable = mock_executable(
            &temp_dir,
            "mock-claude-cancelled-guide",
            r#"#!/usr/bin/env python3
import json
import os
import sys
import time

def send(value):
    print(json.dumps(value, separators=(",", ":")), flush=True)

first = json.loads(sys.stdin.readline())
send({"type":"system","subtype":"init","session_id":"cancelled-guide-session"})
send(first)
interrupt = json.loads(sys.stdin.readline())
with open(os.environ["INTERRUPT_MARKER"], "w", encoding="utf-8") as handle:
    handle.write("ready")
while not os.path.exists(os.environ["CONTINUE_MARKER"]):
    time.sleep(0.01)
send({"type":"control_response","response":{"subtype":"success","request_id":interrupt["request_id"]}})
time.sleep(0.1)
send({"type":"result","subtype":"success","is_error":False,"result":"done","session_id":"cancelled-guide-session"})
"#,
        );
        let session_key = "mock-stream-json-cancelled-guide-session";
        let run_id = "mock-stream-json-cancelled-guide-run";
        let interrupt_marker = temp_dir.path().join("interrupt-ready");
        let continue_marker = temp_dir.path().join("continue");
        ACTIVE_SESSIONS.insert(session_key.to_string(), run_id.to_string());
        let run_task = tokio::spawn(run_command(
            run_id,
            Some(session_key),
            CommandSpec {
                env: BTreeMap::from([
                    (
                        "INTERRUPT_MARKER".to_string(),
                        interrupt_marker.display().to_string(),
                    ),
                    (
                        "CONTINUE_MARKER".to_string(),
                        continue_marker.display().to_string(),
                    ),
                ]),
                ..mock_spec(&executable, Some(MOCK_RUN_TIMEOUT_SECS))
            },
            "initial task".to_string(),
            temp_dir.path().join("stop"),
            None,
        ));
        let guide_task = tokio::spawn(live_guide::request_session_guide(
            session_key,
            "guide-cancelled".to_string(),
            "do not replay".to_string(),
        ));
        wait_for_path(&interrupt_marker).await;
        guide_task.abort();
        std::fs::write(&continue_marker, b"continue").unwrap();

        assert_eq!(
            run_task.await.unwrap().unwrap().status,
            ExternalCliRunStatus::Succeeded
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn closed_stdin_after_interrupt_ack_rejects_user_frame() {
        let temp_dir = tempfile::tempdir().unwrap();
        let executable = mock_executable(
            &temp_dir,
            "mock-claude-user-frame-failure",
            r#"#!/usr/bin/env python3
import json
import os
import sys
import time

def send(value):
    print(json.dumps(value, separators=(",", ":")), flush=True)

first = json.loads(sys.stdin.readline())
send({"type":"system","subtype":"init","session_id":"user-frame-failure-session"})
send(first)
interrupt = json.loads(sys.stdin.readline())
os.close(0)
send({"type":"control_response","response":{"subtype":"success","request_id":interrupt["request_id"]}})
time.sleep(0.2)
send({"type":"result","subtype":"success","is_error":False,"result":"done","session_id":"user-frame-failure-session"})
"#,
        );
        let session_key = "mock-stream-json-user-frame-failure-session";
        let run_id = "mock-stream-json-user-frame-failure-run";
        ACTIVE_SESSIONS.insert(session_key.to_string(), run_id.to_string());
        let run_task = tokio::spawn(run_command(
            run_id,
            Some(session_key),
            mock_spec(&executable, Some(MOCK_RUN_TIMEOUT_SECS)),
            "initial task".to_string(),
            temp_dir.path().join("stop"),
            None,
        ));
        let rejected = live_guide::request_session_guide(
            session_key,
            "guide-user-frame-failure".to_string(),
            "cannot replay".to_string(),
        )
        .await;
        assert!(!rejected.accepted);
        assert!(rejected.reason.as_deref().is_some_and(|reason| {
            reason.contains("write stream-json user frame")
                || reason.contains("flush stream-json user frame")
        }));
        assert_eq!(
            run_task.await.unwrap().unwrap().status,
            ExternalCliRunStatus::Succeeded
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn failed_result_is_forwarded_to_progress_channel() {
        let temp_dir = tempfile::tempdir().unwrap();
        let executable = mock_executable(
            &temp_dir,
            "mock-claude-failed-result",
            r#"#!/usr/bin/env python3
import json
import sys

json.loads(sys.stdin.readline())
print(json.dumps({"type":"assistant","message":{"content":[{"type":"text","text":"partial"}]}}), flush=True)
print(json.dumps({"type":"result","subtype":"error_max_turns","is_error":True,"result":"failed"}), flush=True)
"#,
        );
        let (progress_tx, mut progress_rx) = mpsc::channel(EXTERNAL_CLI_PROGRESS_CHANNEL_CAPACITY);
        let output = run_command(
            "mock-stream-json-failed-result-run",
            None,
            mock_spec(&executable, Some(MOCK_RUN_TIMEOUT_SECS)),
            "initial task".to_string(),
            temp_dir.path().join("stop"),
            Some(progress_tx),
        )
        .await
        .unwrap();
        assert_eq!(output.status, ExternalCliRunStatus::Failed);
        assert!(progress_rx.try_recv().is_ok());
    }

    #[tokio::test]
    async fn terminal_cleanup_rejects_queued_guide_commands() {
        let (guide_tx, mut guide_rx) = mpsc::unbounded_channel();
        let mut acknowledgements = Vec::new();
        for index in 0..3 {
            let (ack_tx, ack_rx) = oneshot::channel();
            guide_tx
                .send(live_guide::LiveGuideCommand {
                    guide_id: format!("guide-drain-{index}"),
                    message: "queued".to_string(),
                    ack_tx,
                })
                .unwrap();
            acknowledgements.push(ack_rx);
        }

        reject_queued_guides(&mut guide_rx, Some("thread-drain".to_string()));
        for ack_rx in acknowledgements {
            let result = ack_rx.await.unwrap();
            assert_eq!(result.thread_id.as_deref(), Some("thread-drain"));
            assert_eq!(result.reason.as_deref(), Some("turn is no longer active"));
        }
        assert!(guide_rx.try_recv().is_err());
    }
}
