#!/usr/bin/env python3
"""PR #5d-2: add subscribe_call_events_streaming to RemoteInvokeCaller.

Strategy:
- Locate the closing `}` of the existing subscribe_call_events method.
- Insert a new method directly after it, inside the same `impl RemoteInvokeCaller` block.
- Add imports at the top for the new types.

The new method mirrors subscribe_call_events's SSE plumbing, but on each
SSE `frame` event it first tries parse_stream_frame_from_sse_data on the
decrypted or raw payload; if that yields a StreamFrame, it routes through
CallerStreamState::feed() and honours StreamDecision. On ReconnectAt it
returns a ReconnectNeeded outcome without touching the `exit` / legacy paths.

Legacy-shape payloads fall through to the same handling as subscribe_call_events
to preserve behavior during transition.
"""

import re
import sys
import pathlib

path = pathlib.Path("crates/bifrost-cli/src/commands/remote.rs")
src = path.read_text()

# --- 1. Add imports ---
# Already has: use crate::commands::caller_stream_frame; implicit via mod.
# We need to `use crate::commands::caller_stream_frame::{...}` for direct names.
anchor_import = "use bifrost_core::{direct_reqwest_client_builder, BifrostError};"
if anchor_import not in src:
    print("ERROR: missing import anchor", file=sys.stderr); sys.exit(1)

new_imports = """use bifrost_core::{direct_reqwest_client_builder, BifrostError};
use crate::commands::caller_stream_frame::{
    parse_stream_frame_from_sse_data, CallerStreamState, StreamDecision, StreamIngestError,
};"""
src = src.replace(anchor_import, new_imports, 1)

# --- 2. Locate end of subscribe_call_events method ---
# Find `async fn subscribe_call_events(` then walk to matching braces end.
m = re.search(r"    async fn subscribe_call_events\(", src)
if not m:
    print("ERROR: subscribe_call_events not found", file=sys.stderr); sys.exit(1)
start = m.start()
# Walk forward to the function body's opening `{`.
i = src.index(") -> bifrost_core::Result<CallResult> {", start)
body_open = src.index("{", i)
depth = 0
j = body_open
while j < len(src):
    c = src[j]
    if c == "{": depth += 1
    elif c == "}":
        depth -= 1
        if depth == 0:
            fn_end = j
            break
    j += 1
else:
    print("ERROR: subscribe_call_events end not found", file=sys.stderr); sys.exit(1)

# Insertion point: right after fn_end (after the `}` + newline).
insertion = fn_end + 1

new_method = r'''

    /// PR #5d-2: streaming variant of subscribe_call_events that routes
    /// recognised StreamFrame-shaped payloads through CallerStreamState
    /// and surfaces a structured outcome so the dispatcher can implement an
    /// outer reconnect-with-offset loop.
    ///
    /// Legacy envelope payloads (envelope_json / ciphertext) are NOT handled
    /// here; callers should fall back to `subscribe_call_events` when the
    /// remote does not yet emit StreamFrames. Detection is strict via
    /// `parse_stream_frame_from_sse_data`.
    ///
    /// The `resume_from` parameter, when Some, pre-positions the state
    /// machine heads so duplicate bytes across a reconnect are deduped.
    async fn subscribe_call_events_streaming<W: std::io::Write, E: std::io::Write>(
        &self,
        call_id: &str,
        relay_token: &str,
        state: &mut CallerStreamState<W, E>,
        resume_from: Option<(u64, u64)>,
        timeout_secs: u64,
    ) -> bifrost_core::Result<StreamingSubscriptionOutcome> {
        if let Some((so, se)) = resume_from {
            state.resume(so, se);
        }

        let url = format!(
            "{}/v4/remote-invoke/calls/{}/events",
            self.base_url, call_id
        );
        let sse_http = direct_reqwest_client_builder()
            .connect_timeout(Duration::from_secs(10))
            .build()
            .map_err(|e| BifrostError::Network(format!("build streaming sse client: {e}")))?;
        let response = sse_http
            .get(&url)
            .header("Authorization", format!("Bearer {relay_token}"))
            .send()
            .await
            .map_err(|e| BifrostError::Network(format!("subscribe streaming events failed: {e}")))?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(BifrostError::Network(format!(
                "streaming events returned {status}: {body}"
            )));
        }

        let mut stream = response.bytes_stream();
        let idle = Duration::from_secs(timeout_secs);
        let mut timeout = Box::pin(tokio::time::sleep(idle));

        let mut event_name = String::new();
        let mut data_buf = String::new();
        let mut partial_line = String::new();

        loop {
            tokio::select! {
                _ = &mut timeout => {
                    warn!("streaming events idle timeout");
                    return Ok(StreamingSubscriptionOutcome::Disconnected {
                        reason: "idle_timeout".to_string(),
                    });
                }
                chunk = stream.next() => {
                    match chunk {
                        Some(Ok(bytes)) => {
                            let text = String::from_utf8_lossy(&bytes);
                            partial_line.push_str(&text);
                            timeout.as_mut().reset(tokio::time::Instant::now() + idle);

                            while let Some(pos) = partial_line.find('\n') {
                                let line = partial_line[..pos].trim_end_matches('\r').to_string();
                                partial_line = partial_line[pos + 1..].to_string();

                                if line.is_empty() {
                                    if !event_name.is_empty() && !data_buf.is_empty() {
                                        if event_name == "frame" {
                                            if let Some(frame) = parse_stream_frame_from_sse_data(&data_buf) {
                                                match state.feed(&frame).map_err(|e| BifrostError::Config(format!("stream ingest error: {e:?}")))? {
                                                    StreamDecision::Continue => {}
                                                    StreamDecision::ReconnectAt { stdout_offset, stderr_offset, reason } => {
                                                        return Ok(StreamingSubscriptionOutcome::ReconnectNeeded {
                                                            stdout_offset,
                                                            stderr_offset,
                                                            reason,
                                                        });
                                                    }
                                                    StreamDecision::Done { exit_code, duration_ms, digest_ok, .. } => {
                                                        return Ok(StreamingSubscriptionOutcome::Completed {
                                                            exit_code,
                                                            duration_ms: Some(duration_ms),
                                                            digest_ok,
                                                        });
                                                    }
                                                    StreamDecision::Error { code, message } => {
                                                        return Ok(StreamingSubscriptionOutcome::ErrorFrame { code, message });
                                                    }
                                                }
                                            } else {
                                                debug!("streaming path saw non-StreamFrame frame payload; ignoring");
                                            }
                                        } else if event_name == "status" {
                                            if let Ok(v) = serde_json::from_str::<Value>(&data_buf) {
                                                if let Some(status) = parse_call_terminal_status(&v) {
                                                    if status == "cancelled" {
                                                        return Ok(StreamingSubscriptionOutcome::Cancelled);
                                                    }
                                                }
                                            }
                                        }
                                    }
                                    event_name.clear();
                                    data_buf.clear();
                                } else if let Some(ev) = line.strip_prefix("event:") {
                                    event_name = ev.trim().to_string();
                                } else if let Some(d) = line.strip_prefix("data:") {
                                    if !data_buf.is_empty() { data_buf.push('\n'); }
                                    data_buf.push_str(d.trim());
                                }
                            }
                        }
                        Some(Err(e)) => {
                            return Ok(StreamingSubscriptionOutcome::Disconnected {
                                reason: format!("sse error: {e}"),
                            });
                        }
                        None => {
                            return Ok(StreamingSubscriptionOutcome::Disconnected {
                                reason: "sse stream closed".to_string(),
                            });
                        }
                    }
                }
            }
        }
    }
'''

src = src[:insertion] + new_method + src[insertion:]

# --- 3. Add StreamingSubscriptionOutcome enum above the impl block ---
# Place it near CallResult definition.
call_result_anchor = "#[derive(Debug, Clone, Default)]\nstruct CallResult {"
if call_result_anchor not in src:
    print("ERROR: CallResult anchor not found", file=sys.stderr); sys.exit(1)

outcome_enum = '''#[derive(Debug)]
#[allow(dead_code)]
pub enum StreamingSubscriptionOutcome {
    /// The call finished with a terminal Done frame.
    Completed {
        exit_code: i32,
        duration_ms: Option<u64>,
        digest_ok: bool,
    },
    /// The executor emitted a Reconnect advisory or we observed a mid-stream
    /// disconnect that can be resumed; caller should reopen a new SSE
    /// subscription with these offsets.
    ReconnectNeeded {
        stdout_offset: u64,
        stderr_offset: u64,
        reason: String,
    },
    /// The SSE stream closed or errored without a terminal frame; caller
    /// should decide whether to resume or surface as a hard failure.
    Disconnected {
        reason: String,
    },
    /// The executor emitted an Error frame; treat as a non-resumable failure.
    ErrorFrame {
        code: String,
        message: String,
    },
    /// The status channel reported the call was cancelled.
    Cancelled,
}

'''

src = src.replace(call_result_anchor, outcome_enum + call_result_anchor, 1)

path.write_text(src)
print(f"OK: updated {path}, new length {len(src)} bytes")
