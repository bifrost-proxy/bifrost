#!/usr/bin/env python3
"""PR #3b-executor: add execute_shell_exec_streaming method.

Additive only — existing execute / execute_shell_exec paths are unchanged.
The streaming method:
  - writes StreamFrame::Stdout/Stderr/Heartbeat/Reconnect/Done/Error into a
    bounded mpsc::Sender<StreamFrame> provided by the caller
  - honors wall_clock + idle timeouts (same logic as PR#3a)
  - emits exactly one Reconnect frame at 27 min wall-clock
  - streams SHA-256 via ring::digest::Context (no buffering)
  - applies natural back-pressure via tx.send().await blocking
"""
import pathlib, sys

p = pathlib.Path("crates/bifrost-admin/src/remote_invoke/executor.rs")
s = p.read_text()
orig = s

# Add new consts near the existing heartbeat const.
old_consts = (
    "/// PR#3a: heartbeat tick interval (floor of idle-timeout granularity).\n"
    "const HEARTBEAT_INTERVAL_MS: u64 = 10_000;"
)
assert old_consts in s
new_consts = (
    "/// PR#3a: heartbeat tick interval (floor of idle-timeout granularity).\n"
    "const HEARTBEAT_INTERVAL_MS: u64 = 10_000;\n"
    "/// PR#3b: elapsed wall-clock after which the executor emits a single\n"
    "/// StreamFrame::Reconnect advisory so receivers can swap relay\n"
    "/// connections BEFORE the relay's 30-minute hard limit hits. 27 min\n"
    "/// gives receivers ~3 min to tear down + re-establish.\n"
    "const RELAY_RECONNECT_HINT_MS: u64 = 27 * 60 * 1000;"
)
s = s.replace(old_consts, new_consts)

# Insert execute_shell_exec_streaming just before execute_shell_exec.
anchor = "    async fn execute_shell_exec<F, Fut>("
assert anchor in s
new_method = '''    /// PR#3b: streaming variant of shell.exec. Instead of returning a
    /// RemoteInvokeResponse with buffered stdout/stderr, this method pushes
    /// `StreamFrame`s into `frame_tx` as the child produces output. The
    /// caller is responsible for pulling frames off the receiver; if the
    /// caller is slow, `frame_tx.send().await` blocks the read loop, giving
    /// natural back-pressure without unbounded in-process buffering.
    ///
    /// Lifecycle (exactly one of the terminal variants is sent):
    ///   - Stdout{seq, offset, data_b64}* / Stderr{...}*
    ///   - Heartbeat{ts, stdout_offset, stderr_offset}*   every 10s
    ///   - Reconnect{reason, stdout_offset, stderr_offset} once at ~27min
    ///   - Done{...} on normal exit
    ///   - Error{code, message} on any failure
    ///
    /// When `frame_tx` is closed by the receiver (downstream gone), the
    /// method aborts the child and returns Ok(()) — the assumption is the
    /// receiver no longer cares about this call.
    pub async fn execute_shell_exec_streaming(
        &self,
        command: &RemoteCommand,
        frame_tx: tokio::sync::mpsc::Sender<crate::remote_invoke::types::StreamFrame>,
    ) -> Result<()> {
        use crate::remote_invoke::types::StreamFrame;
        use base64::Engine as _;

        // Helper: best-effort send; if receiver is gone we abort.
        async fn send_frame(
            tx: &tokio::sync::mpsc::Sender<StreamFrame>,
            frame: StreamFrame,
        ) -> std::result::Result<(), ()> {
            tx.send(frame).await.map_err(|_| ())
        }

        async fn send_error(
            tx: &tokio::sync::mpsc::Sender<StreamFrame>,
            code: &str,
            message: String,
        ) {
            let _ = tx
                .send(StreamFrame::Error {
                    code: code.to_string(),
                    message,
                })
                .await;
        }

        let policy = match self.resolve_shell_policy(command) {
            Ok(p) => p,
            Err(e) => {
                send_error(&frame_tx, "policy_rejected", format!("{e}")).await;
                return Ok(());
            }
        };

        if command.pty.as_ref().map(|pty| pty.enabled).unwrap_or(false)
            && !policy.interactive_allowed
        {
            send_error(
                &frame_tx,
                "policy_rejected",
                format!(
                    "policy '{}' does not allow PTY/interactive shell execution",
                    policy.policy_id
                ),
            )
            .await;
            return Ok(());
        }

        // Timeout split identical to PR#3a.
        let wall_clock_timeout_ms: Option<u64> =
            match (command.timeout_ms, policy.max_timeout_ms) {
                (Some(c), Some(p)) => Some(c.min(p)),
                (Some(c), None) => Some(c),
                (None, Some(p)) => Some(p),
                (None, None) => None,
            };
        let idle_timeout_ms: u64 = policy.max_idle_ms.unwrap_or(DEFAULT_IDLE_TIMEOUT_MS);
        let timeout_ms = wall_clock_timeout_ms.unwrap_or(u64::MAX);

        let start = Instant::now();
        let mut process = match self.build_shell_exec_process(command, &policy) {
            Ok(p) => p,
            Err(e) => {
                send_error(&frame_tx, "spawn_failed", format!("{e}")).await;
                return Ok(());
            }
        };
        process.stdout(Stdio::piped());
        process.stderr(Stdio::piped());
        process.stdin(Stdio::null());
        process.kill_on_drop(true);

        let mut child = match process.spawn() {
            Ok(c) => c,
            Err(e) => {
                send_error(
                    &frame_tx,
                    "spawn_failed",
                    format!("spawn shell.exec failed: {e}"),
                )
                .await;
                return Ok(());
            }
        };
        let mut stdout_reader = match child.stdout.take() {
            Some(r) => r,
            None => {
                send_error(
                    &frame_tx,
                    "spawn_failed",
                    "shell.exec stdout pipe unavailable".to_string(),
                )
                .await;
                let _ = child.kill().await;
                return Ok(());
            }
        };
        let mut stderr_reader = match child.stderr.take() {
            Some(r) => r,
            None => {
                send_error(
                    &frame_tx,
                    "spawn_failed",
                    "shell.exec stderr pipe unavailable".to_string(),
                )
                .await;
                let _ = child.kill().await;
                return Ok(());
            }
        };

        let mut stdout_buf = [0u8; 65536];
        let mut stderr_buf = [0u8; 65536];
        let mut stdout_open = true;
        let mut stderr_open = true;
        let mut exit_status: Option<std::process::ExitStatus> = None;
        let mut stdout_total_bytes: u64 = 0;
        let mut stderr_total_bytes: u64 = 0;
        let mut stdout_hasher = ring::digest::Context::new(&ring::digest::SHA256);
        let mut stderr_hasher = ring::digest::Context::new(&ring::digest::SHA256);
        let mut stdout_seq: u64 = 0;
        let mut stderr_seq: u64 = 0;
        let mut reconnect_sent = false;

        let wall_clock = async {
            match wall_clock_timeout_ms {
                Some(ms) => tokio::time::sleep(Duration::from_millis(ms)).await,
                None => std::future::pending::<()>().await,
            }
        };
        tokio::pin!(wall_clock);
        let mut idle_deadline = tokio::time::Instant::now()
            + Duration::from_millis(idle_timeout_ms);
        let idle_sleep = tokio::time::sleep_until(idle_deadline);
        tokio::pin!(idle_sleep);
        let reconnect_hint =
            tokio::time::sleep(Duration::from_millis(RELAY_RECONNECT_HINT_MS));
        tokio::pin!(reconnect_hint);
        let mut heartbeat =
            tokio::time::interval(Duration::from_millis(HEARTBEAT_INTERVAL_MS));
        heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let _ = heartbeat.tick().await;

        loop {
            tokio::select! {
                _ = &mut wall_clock => {
                    let _ = child.kill().await;
                    let _ = child.wait().await;
                    send_error(
                        &frame_tx,
                        "wall_clock_timeout",
                        format!(
                            "shell.exec wall-clock timeout after {timeout_ms} ms (policy '{}')",
                            policy.policy_id
                        ),
                    ).await;
                    return Ok(());
                }
                _ = &mut idle_sleep => {
                    let _ = child.kill().await;
                    let _ = child.wait().await;
                    send_error(
                        &frame_tx,
                        "idle_timeout",
                        format!(
                            "shell.exec idle timeout after {idle_timeout_ms} ms of no output (policy '{}')",
                            policy.policy_id
                        ),
                    ).await;
                    return Ok(());
                }
                _ = &mut reconnect_hint, if !reconnect_sent => {
                    reconnect_sent = true;
                    if send_frame(&frame_tx, StreamFrame::Reconnect {
                        reason: "relay-wall-clock".to_string(),
                        stdout_offset: stdout_total_bytes,
                        stderr_offset: stderr_total_bytes,
                    }).await.is_err() {
                        let _ = child.kill().await;
                        return Ok(());
                    }
                }
                _ = heartbeat.tick() => {
                    if send_frame(&frame_tx, StreamFrame::Heartbeat {
                        ts: chrono::Utc::now().timestamp_millis() as u64,
                        stdout_offset: Some(stdout_total_bytes),
                        stderr_offset: Some(stderr_total_bytes),
                    }).await.is_err() {
                        let _ = child.kill().await;
                        return Ok(());
                    }
                }
                wait_result = child.wait(), if exit_status.is_none() => {
                    match wait_result {
                        Ok(status) => {
                            exit_status = Some(status);
                            if !stdout_open && !stderr_open { break; }
                        }
                        Err(e) => {
                            send_error(&frame_tx, "wait_failed", format!("wait shell.exec failed: {e}")).await;
                            return Ok(());
                        }
                    }
                }
                read = stdout_reader.read(&mut stdout_buf), if stdout_open => {
                    match read {
                        Ok(0) => {
                            stdout_open = false;
                            if exit_status.is_some() && !stderr_open { break; }
                        }
                        Ok(n) => {
                            idle_deadline = tokio::time::Instant::now()
                                + Duration::from_millis(idle_timeout_ms);
                            idle_sleep.as_mut().reset(idle_deadline);
                            let offset = stdout_total_bytes;
                            stdout_total_bytes += n as u64;
                            stdout_hasher.update(&stdout_buf[..n]);
                            let data_b64 = base64::engine::general_purpose::STANDARD
                                .encode(&stdout_buf[..n]);
                            let seq = stdout_seq;
                            stdout_seq += 1;
                            // back-pressure: this await blocks on slow consumer.
                            if send_frame(&frame_tx, StreamFrame::Stdout {
                                seq, offset, data_b64,
                            }).await.is_err() {
                                let _ = child.kill().await;
                                return Ok(());
                            }
                        }
                        Err(e) => {
                            send_error(&frame_tx, "read_failed",
                                format!("read shell.exec stdout failed: {e}")).await;
                            let _ = child.kill().await;
                            return Ok(());
                        }
                    }
                }
                read = stderr_reader.read(&mut stderr_buf), if stderr_open => {
                    match read {
                        Ok(0) => {
                            stderr_open = false;
                            if exit_status.is_some() && !stdout_open { break; }
                        }
                        Ok(n) => {
                            idle_deadline = tokio::time::Instant::now()
                                + Duration::from_millis(idle_timeout_ms);
                            idle_sleep.as_mut().reset(idle_deadline);
                            let offset = stderr_total_bytes;
                            stderr_total_bytes += n as u64;
                            stderr_hasher.update(&stderr_buf[..n]);
                            let data_b64 = base64::engine::general_purpose::STANDARD
                                .encode(&stderr_buf[..n]);
                            let seq = stderr_seq;
                            stderr_seq += 1;
                            if send_frame(&frame_tx, StreamFrame::Stderr {
                                seq, offset, data_b64,
                            }).await.is_err() {
                                let _ = child.kill().await;
                                return Ok(());
                            }
                        }
                        Err(e) => {
                            send_error(&frame_tx, "read_failed",
                                format!("read shell.exec stderr failed: {e}")).await;
                            let _ = child.kill().await;
                            return Ok(());
                        }
                    }
                }
            }
        }

        let status = match exit_status {
            Some(s) => s,
            None => match child.wait().await {
                Ok(s) => s,
                Err(e) => {
                    send_error(&frame_tx, "wait_failed",
                        format!("wait shell.exec failed: {e}")).await;
                    return Ok(());
                }
            },
        };

        let stdout_sha256 = {
            let d = stdout_hasher.finish();
            let mut out = String::with_capacity(64);
            for b in d.as_ref() { out.push_str(&format!("{b:02x}")); }
            out
        };
        let stderr_sha256 = {
            let d = stderr_hasher.finish();
            let mut out = String::with_capacity(64);
            for b in d.as_ref() { out.push_str(&format!("{b:02x}")); }
            out
        };

        let _ = send_frame(&frame_tx, StreamFrame::Done {
            exit_code: status.code().unwrap_or(-1),
            total_stdout: stdout_total_bytes,
            total_stderr: stderr_total_bytes,
            stdout_sha256,
            stderr_sha256,
            duration_ms: start.elapsed().as_millis() as u64,
            stdout_object: None,
            stderr_object: None,
        }).await;

        Ok(())
    }

'''
s = s.replace(anchor, new_method + anchor)

if s == orig:
    print("NO CHANGES", file=sys.stderr); sys.exit(1)

p.write_text(s)
print("PATCHED OK (PR#3b-executor)")
