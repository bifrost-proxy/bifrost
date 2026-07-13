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
    progress_tx: Option<mpsc::UnboundedSender<ExternalCliProgressEvent>>,
) -> Result<CommandOutput, String> {
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
    for (key, value) in &spec.env {
        command.env(key, value);
    }

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

    while terminal_status.is_none() {
        tokio::select! {
            line = lines.next_line() => {
                let Some(line) = line.map_err(|error| format!("read stream-json stdout failed: {error}"))? else {
                    terminal_error = Some("stream-json runner exited before a result frame".to_string());
                    terminal_status = Some(ExternalCliRunStatus::Failed);
                    break;
                };
                stdout_bytes.extend_from_slice(line.as_bytes());
                stdout_bytes.push(b'\n');
                let raw = serde_json::from_str::<serde_json::Value>(&line).ok();
                if thread_id.is_none() {
                    thread_id = raw.as_ref().and_then(stream_json_session_id);
                }
                if let Some(raw) = raw.as_ref() {
                    if let Some((request_id, succeeded, reason)) = control_response(raw) {
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
                    if let Some(mut event) =
                        parse_progress_event_line_with_state(&line, &mut parse_state)
                    {
                        enrich_progress_event_observation(
                            &mut event,
                            now_ms(),
                            &mut tool_started_at,
                        );
                        if let Some(progress_tx) = progress_tx.as_ref() {
                            let _ = progress_tx.send(event.clone());
                        }
                        events.push(event);
                    }
                }
                if !interrupted_result {
                    if let Some(raw) = raw.as_ref().filter(|raw| {
                        raw.get("type").and_then(serde_json::Value::as_str) == Some("result")
                    }) {
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
            _ = wait_for_stop_marker(stop_marker_path.clone()) => {
                terminal_status = Some(ExternalCliRunStatus::Stopped);
            }
            _ = &mut timeout_sleep => {
                terminal_error = Some(format!(
                    "stream-json runner timed out after {} seconds",
                    timeout_secs.unwrap_or_default(),
                ));
                terminal_status = Some(ExternalCliRunStatus::TimedOut);
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
    while let Ok(command) = guide_rx.try_recv() {
        let _ = command.ack_tx.send(live_guide::rejected_guide(
            command.guide_id,
            thread_id.clone(),
            None,
            "turn is no longer active".to_string(),
        ));
    }

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
            Duration::from_secs(5),
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
        assert!(started.elapsed() < Duration::from_secs(4));
        assert!(!ACTIVE_RUNS.contains_key(run_id));
    }
}
