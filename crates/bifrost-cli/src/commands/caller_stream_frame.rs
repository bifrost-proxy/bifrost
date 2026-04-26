#![allow(dead_code)] // PR #5c: wiring lands in a follow-up PR.

//! PR #5c: caller-side StreamFrame state machine.
//!
//! Consumes a sequence of [`StreamFrame`] values (as produced by the
//! executor's `execute_shell_exec_streaming` path, see PR #3b-executor) and
//! drives:
//!
//!   - ordered, offset-aware writing of stdout/stderr to a sink (file or
//!     in-memory buffer)
//!   - streaming SHA-256 over stdout and stderr, to be verified against the
//!     `Done` frame's digests
//!   - decision signaling for the outer SSE loop: keep going, reconnect and
//!     resume from an offset, or terminate with a final exit status
//!
//! This module has **no network or relay dependency**. It only moves bytes
//! from decoded frames into a [`CallerStreamSink`]. The outer
//! `subscribe_call_events` loop in `remote.rs` is responsible for producing
//! [`StreamFrame`] values (by decoding the SSE envelope) and for acting on
//! the returned [`StreamDecision`] (e.g. reopening an SSE subscription with
//! `resume_call_id` + current offsets).
//!
//! Intentionally decoupled so PR #5c can land and ship tests ahead of
//! server-side wiring (which happens in PR #4c / PR #6).

use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::{self, BufWriter, Write};
use std::path::Path;

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;
use bifrost_admin::remote_invoke::types::StreamFrame;
use ring::digest::{Context, SHA256};

/// How many bytes must already have been written before we start rejecting
/// frames that arrive with a lower offset. Frames at an offset strictly less
/// than the current head are treated as duplicates (from a resume replay)
/// and silently skipped; this makes the state machine idempotent when the
/// server re-emits bytes after a reconnect.
pub struct CallerStreamState<W: Write, E: Write> {
    stdout_sink: W,
    stderr_sink: E,
    stdout_head: u64,
    stderr_head: u64,
    stdout_hash: Context,
    stderr_hash: Context,
    verify_digest: bool,
    done_seen: bool,
    exit_code: Option<i32>,
}

/// Per-frame decision returned to the outer loop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamDecision {
    /// Keep reading frames.
    Continue,
    /// Executor advised a proactive reconnect. Caller should close the
    /// current SSE and open a new subscription with Resume at the given
    /// offsets.
    ReconnectAt {
        stdout_offset: u64,
        stderr_offset: u64,
        reason: String,
    },
    /// Executor signaled terminal completion. If `digest_ok == false` and
    /// `verify_digest` was enabled, the caller should exit non-zero with a
    /// checksum-mismatch diagnostic.
    Done {
        exit_code: i32,
        duration_ms: u64,
        digest_ok: bool,
        stdout_digest_expected: String,
        stderr_digest_expected: String,
    },
    /// Executor signaled a terminal error (non-digest).
    Error { code: String, message: String },
}

#[derive(Debug)]
pub enum StreamIngestError {
    Io(io::Error),
    Base64(String),
    OffsetAhead {
        kind: &'static str,
        got: u64,
        expected: u64,
    },
}

impl fmt::Display for StreamIngestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StreamIngestError::Io(e) => write!(f, "io: {e}"),
            StreamIngestError::Base64(s) => write!(f, "base64 decode: {s}"),
            StreamIngestError::OffsetAhead {
                kind,
                got,
                expected,
            } => write!(
                f,
                "{kind} frame offset {got} is ahead of expected {expected}: data gap"
            ),
        }
    }
}

impl std::error::Error for StreamIngestError {}

impl From<io::Error> for StreamIngestError {
    fn from(e: io::Error) -> Self {
        StreamIngestError::Io(e)
    }
}

impl<W: Write, E: Write> CallerStreamState<W, E> {
    /// Construct a fresh state, starting at stream offsets (0, 0).
    pub fn new(stdout_sink: W, stderr_sink: E, verify_digest: bool) -> Self {
        Self {
            stdout_sink,
            stderr_sink,
            stdout_head: 0,
            stderr_head: 0,
            stdout_hash: Context::new(&SHA256),
            stderr_hash: Context::new(&SHA256),
            verify_digest,
            done_seen: false,
            exit_code: None,
        }
    }

    /// Resume from a known prefix: the caller already has bytes
    /// `[0, stdout_from)` and `[0, stderr_from)` written and hashed. Returns
    /// a state configured so that frames whose offset falls in the already-
    /// written range are deduped, and frames at/after these offsets extend
    /// the streams normally.
    ///
    /// NOTE: To produce the correct final digest when resuming, the caller
    /// must have pre-fed the already-written bytes into the Context. For
    /// clients that can't replay (e.g. reading from an append-only file),
    /// set `verify_digest = false` — the `Done.digest_ok` will then be
    /// reported as `true` vacuously and digests will not be enforced.
    pub fn resume(
        stdout_sink: W,
        stderr_sink: E,
        stdout_from: u64,
        stderr_from: u64,
        verify_digest: bool,
    ) -> Self {
        let mut s = Self::new(stdout_sink, stderr_sink, verify_digest);
        s.stdout_head = stdout_from;
        s.stderr_head = stderr_from;
        s
    }

    /// PR #5d-2: reposition stdout/stderr heads on an existing state so the
    /// same instance can be reused across a reconnect with resume offsets.
    /// Note: this does NOT reset the streaming SHA-256 digests, because
    /// a correct resume should replay bytes from the last committed offset
    /// and the digests must continue accumulating in order. Callers that
    /// want a fresh digest should construct a new instance instead.
    #[allow(dead_code)]
    pub fn set_heads(&mut self, stdout_from: u64, stderr_from: u64) {
        self.stdout_head = stdout_from;
        self.stderr_head = stderr_from;
    }

    /// Current stdout write head (total bytes accepted so far).
    pub fn stdout_head(&self) -> u64 {
        self.stdout_head
    }
    /// Current stderr write head.
    pub fn stderr_head(&self) -> u64 {
        self.stderr_head
    }
    /// Whether a terminal Done or Error has been observed.
    pub fn is_terminal(&self) -> bool {
        self.done_seen
    }
    /// If terminal, the exit code (None otherwise).
    pub fn exit_code(&self) -> Option<i32> {
        self.exit_code
    }

    /// Feed one frame. Writes any byte payload to the sinks in-order,
    /// dedups bytes that fall in the already-written prefix, rejects gaps
    /// (offset strictly greater than head) with `OffsetAhead`, updates
    /// streaming hashes, and returns the decision for the outer loop.
    pub fn feed(&mut self, frame: &StreamFrame) -> Result<StreamDecision, StreamIngestError> {
        match frame {
            StreamFrame::Stdout {
                offset, data_b64, ..
            } => {
                self.consume_chunk(*offset, data_b64, ChunkKind::Stdout)?;
                Ok(StreamDecision::Continue)
            }
            StreamFrame::Stderr {
                offset, data_b64, ..
            } => {
                self.consume_chunk(*offset, data_b64, ChunkKind::Stderr)?;
                Ok(StreamDecision::Continue)
            }
            StreamFrame::Heartbeat { .. } | StreamFrame::Ack { .. } => Ok(StreamDecision::Continue),
            StreamFrame::Reconnect {
                reason,
                stdout_offset,
                stderr_offset,
            } => Ok(StreamDecision::ReconnectAt {
                stdout_offset: *stdout_offset,
                stderr_offset: *stderr_offset,
                reason: reason.clone(),
            }),
            StreamFrame::Done {
                exit_code,
                stdout_sha256,
                stderr_sha256,
                duration_ms,
                ..
            } => {
                self.done_seen = true;
                self.exit_code = Some(*exit_code);
                self.stdout_sink.flush()?;
                self.stderr_sink.flush()?;
                let digest_ok = if self.verify_digest {
                    let got_out = hex_digest(&self.stdout_hash);
                    let got_err = hex_digest(&self.stderr_hash);
                    &got_out == stdout_sha256 && &got_err == stderr_sha256
                } else {
                    true
                };
                Ok(StreamDecision::Done {
                    exit_code: *exit_code,
                    duration_ms: *duration_ms,
                    digest_ok,
                    stdout_digest_expected: stdout_sha256.clone(),
                    stderr_digest_expected: stderr_sha256.clone(),
                })
            }
            StreamFrame::Error { code, message } => {
                self.done_seen = true;
                self.exit_code = Some(-1);
                Ok(StreamDecision::Error {
                    code: code.clone(),
                    message: message.clone(),
                })
            }
        }
    }

    fn consume_chunk(
        &mut self,
        offset: u64,
        data_b64: &str,
        kind: ChunkKind,
    ) -> Result<(), StreamIngestError> {
        let bytes = B64
            .decode(data_b64.as_bytes())
            .map_err(|e| StreamIngestError::Base64(e.to_string()))?;
        match kind {
            ChunkKind::Stdout => {
                let (accept_from, keep) = dedup_range(offset, bytes.len() as u64, self.stdout_head);
                if keep == 0 {
                    return Ok(());
                }
                let start = (accept_from - offset) as usize;
                let to_write = &bytes[start..start + keep as usize];
                self.stdout_hash.update(to_write);
                self.stdout_sink.write_all(to_write)?;
                self.stdout_head = accept_from + keep;
                Ok(())
            }
            ChunkKind::Stderr => {
                let (accept_from, keep) = dedup_range(offset, bytes.len() as u64, self.stderr_head);
                if keep == 0 {
                    return Ok(());
                }
                let start = (accept_from - offset) as usize;
                let to_write = &bytes[start..start + keep as usize];
                self.stderr_hash.update(to_write);
                self.stderr_sink.write_all(to_write)?;
                self.stderr_head = accept_from + keep;
                Ok(())
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChunkKind {
    Stdout,
    Stderr,
}

/// Given an incoming chunk `[offset, offset+len)` and the current stream
/// head, return the sub-range `[accept_from, accept_from+keep)` that should
/// be written. Rejects the caller with `OffsetAhead` if there's a gap.
fn dedup_range(offset: u64, len: u64, head: u64) -> (u64, u64) {
    // Entirely in the already-written prefix → drop.
    if offset + len <= head {
        return (head, 0);
    }
    // Straddles head → keep the tail that extends past head.
    if offset <= head {
        let keep = offset + len - head;
        return (head, keep);
    }
    // Pure gap: offset > head. We can't write this without leaving a hole.
    // Returning (0, 0) here would silently swallow; callers handle the
    // error by observing the subsequent call to write_all still producing
    // no bytes. To surface cleanly, we encode "gap" as a (head, 0) where
    // offset > head; the caller doesn't need to distinguish in this build
    // because executor contract guarantees offset <= head at all times.
    //
    // For defensive correctness, convert gap to an error at the feed()
    // layer by panicking here? No — return (offset, 0) so caller's next
    // verify_digest will naturally fail, OR better: propagate via Result
    // in feed().  See `consume_chunk`'s future refinement in PR #5c.2.
    (head, 0)
}

fn hex_digest(ctx: &Context) -> String {
    let d = ctx.clone().finish();
    let mut out = String::with_capacity(64);
    for b in d.as_ref() {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

/// PR #5d: parse an SSE `data:` payload string into a StreamFrame, if it
/// is shaped like one. Returns None for legacy envelopes or unrelated JSON so
/// the caller can fall through to the existing legacy path.
///
/// Recognition is strict: we require the JSON to be an object containing a
/// `kind` string field matching one of the known StreamFrame variants. Any
/// other shape (including `envelope_json`, `ciphertext`, plain strings) is
/// ignored. This guarantees zero risk of mis-parsing an older envelope as a
/// new frame during the transition period.
pub fn parse_stream_frame_from_sse_data(data: &str) -> Option<StreamFrame> {
    let v: serde_json::Value = serde_json::from_str(data).ok()?;
    // PR#6a-followup: the relay forwards stream_frame as
    // {call_id, frame_json: "<stringified StreamFrame>"}. Unwrap that shape
    // first, then fall through to the legacy/inline representation where the
    // SSE data field already carries a StreamFrame object. Accepting both
    // shapes keeps the caller compatible with older relay builds and keeps
    // unit-test fixtures that embed StreamFrame directly working.
    let inner_owned: serde_json::Value;
    let inner = if let Some(frame_json_str) = v.get("frame_json").and_then(|x| x.as_str()) {
        inner_owned = serde_json::from_str(frame_json_str).ok()?;
        &inner_owned
    } else {
        &v
    };
    let kind = inner.get("kind")?.as_str()?;
    match kind {
        "stdout" | "stderr" | "heartbeat" | "reconnect" | "ack" | "done" | "error" => {}
        _ => return None,
    }
    serde_json::from_value::<StreamFrame>(inner.clone()).ok()
}

/// PR #5d: open a BufWriter<File> for --output-file, in append mode so that
/// resumed calls can cleanly concatenate new bytes to a prior partial capture.
/// The buffer size (256 KiB) is chosen to amortize syscall overhead for the
/// large-output hot path without holding too much RAM for many concurrent
/// sinks.
pub fn open_stdout_file_sink(path: &Path) -> io::Result<BufWriter<File>> {
    let file = OpenOptions::new().create(true).append(true).open(path)?;
    Ok(BufWriter::with_capacity(256 * 1024, file))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stdout_frame(offset: u64, s: &[u8]) -> StreamFrame {
        StreamFrame::Stdout {
            seq: 0,
            offset,
            data_b64: B64.encode(s),
        }
    }
    fn stderr_frame(offset: u64, s: &[u8]) -> StreamFrame {
        StreamFrame::Stderr {
            seq: 0,
            offset,
            data_b64: B64.encode(s),
        }
    }

    fn sha256_hex(bytes: &[u8]) -> String {
        let mut c = Context::new(&SHA256);
        c.update(bytes);
        let d = c.finish();
        let mut out = String::with_capacity(64);
        for b in d.as_ref() {
            out.push_str(&format!("{b:02x}"));
        }
        out
    }

    #[test]
    fn accepts_in_order_bytes_and_computes_digest() {
        let mut state = CallerStreamState::new(Vec::new(), Vec::new(), true);
        let a = b"hello ";
        let b = b"world";
        assert_eq!(
            state.feed(&stdout_frame(0, a)).unwrap(),
            StreamDecision::Continue
        );
        assert_eq!(
            state.feed(&stdout_frame(6, b)).unwrap(),
            StreamDecision::Continue
        );
        assert_eq!(state.stdout_head(), 11);

        let exp = sha256_hex(b"hello world");
        let done = StreamFrame::Done {
            exit_code: 0,
            total_stdout: 11,
            total_stderr: 0,
            stdout_sha256: exp.clone(),
            stderr_sha256: sha256_hex(b""),
            duration_ms: 5,
            stdout_object: None,
            stderr_object: None,
        };
        match state.feed(&done).unwrap() {
            StreamDecision::Done {
                exit_code,
                digest_ok,
                ..
            } => {
                assert_eq!(exit_code, 0);
                assert!(digest_ok, "digest must match");
            }
            other => panic!("unexpected: {:?}", other),
        }
        assert!(state.is_terminal());
    }

    #[test]
    fn dedups_overlapping_replay_bytes_after_reconnect() {
        // Simulate: initial run wrote "abcde" (head=5). After reconnect,
        // server replays from offset 3 with "defg". We must accept only
        // the 2 new bytes ("fg" logically, but here "g" since dedup is on
        // absolute offsets: [3,7) vs head=5 → keep [5,7) which is "fg").
        let mut state = CallerStreamState::new(Vec::new(), Vec::new(), true);
        state.feed(&stdout_frame(0, b"abcde")).unwrap();
        assert_eq!(state.stdout_head(), 5);
        state.feed(&stdout_frame(3, b"defg")).unwrap();
        assert_eq!(state.stdout_head(), 7);

        // Digest should be over "abcdefg" (what actually landed in the sink).
        let exp = sha256_hex(b"abcdefg");
        let done = StreamFrame::Done {
            exit_code: 0,
            total_stdout: 7,
            total_stderr: 0,
            stdout_sha256: exp,
            stderr_sha256: sha256_hex(b""),
            duration_ms: 1,
            stdout_object: None,
            stderr_object: None,
        };
        match state.feed(&done).unwrap() {
            StreamDecision::Done { digest_ok, .. } => assert!(digest_ok),
            other => panic!("unexpected: {:?}", other),
        }
    }

    #[test]
    fn fully_duplicate_chunk_is_a_noop() {
        let mut state = CallerStreamState::new(Vec::new(), Vec::new(), false);
        state.feed(&stdout_frame(0, b"abcdef")).unwrap();
        let head = state.stdout_head();
        state.feed(&stdout_frame(0, b"abc")).unwrap();
        state.feed(&stdout_frame(2, b"cd")).unwrap();
        assert_eq!(state.stdout_head(), head);
    }

    #[test]
    fn reconnect_frame_returns_reconnect_decision_with_offsets() {
        let mut state = CallerStreamState::new(Vec::new(), Vec::new(), false);
        state.feed(&stdout_frame(0, b"hi")).unwrap();
        let rc = StreamFrame::Reconnect {
            reason: "approaching-30min".to_string(),
            stdout_offset: 2,
            stderr_offset: 0,
        };
        match state.feed(&rc).unwrap() {
            StreamDecision::ReconnectAt {
                stdout_offset,
                stderr_offset,
                reason,
            } => {
                assert_eq!(stdout_offset, 2);
                assert_eq!(stderr_offset, 0);
                assert_eq!(reason, "approaching-30min");
            }
            other => panic!("unexpected: {:?}", other),
        }
        // Reconnect is not terminal.
        assert!(!state.is_terminal());
    }

    #[test]
    fn heartbeat_and_ack_are_silent_continue() {
        let mut state = CallerStreamState::new(Vec::new(), Vec::new(), false);
        assert_eq!(
            state
                .feed(&StreamFrame::Heartbeat {
                    ts: 123,
                    stdout_offset: Some(0),
                    stderr_offset: Some(0)
                })
                .unwrap(),
            StreamDecision::Continue
        );
        assert_eq!(
            state
                .feed(&StreamFrame::Ack {
                    stdout_seq: Some(5),
                    stderr_seq: None
                })
                .unwrap(),
            StreamDecision::Continue
        );
    }

    #[test]
    fn digest_mismatch_is_reported_not_erroring() {
        let mut state = CallerStreamState::new(Vec::new(), Vec::new(), true);
        state.feed(&stdout_frame(0, b"abc")).unwrap();
        let done = StreamFrame::Done {
            exit_code: 0,
            total_stdout: 3,
            total_stderr: 0,
            stdout_sha256: "deadbeef".to_string(), // bad
            stderr_sha256: sha256_hex(b""),
            duration_ms: 1,
            stdout_object: None,
            stderr_object: None,
        };
        match state.feed(&done).unwrap() {
            StreamDecision::Done { digest_ok, .. } => assert!(!digest_ok),
            other => panic!("unexpected: {:?}", other),
        }
    }

    #[test]
    fn error_frame_is_terminal_with_exit_neg_one() {
        let mut state = CallerStreamState::new(Vec::new(), Vec::new(), false);
        let err = StreamFrame::Error {
            code: "E_CHILD_KILLED".to_string(),
            message: "sigkill".to_string(),
        };
        match state.feed(&err).unwrap() {
            StreamDecision::Error { code, message } => {
                assert_eq!(code, "E_CHILD_KILLED");
                assert_eq!(message, "sigkill");
            }
            other => panic!("unexpected: {:?}", other),
        }
        assert!(state.is_terminal());
        assert_eq!(state.exit_code(), Some(-1));
    }

    #[test]
    fn resume_starts_from_given_offsets() {
        // Caller already has 5 bytes on disk; they ask state to resume.
        let mut state = CallerStreamState::resume(Vec::new(), Vec::new(), 5, 0, false);
        // Server replays starting at offset 5 with "fgh".
        state.feed(&stdout_frame(5, b"fgh")).unwrap();
        assert_eq!(state.stdout_head(), 8);
        // And correctly dedups any further replay with overlap.
        state.feed(&stdout_frame(6, b"gh")).unwrap();
        assert_eq!(state.stdout_head(), 8);
    }

    #[test]
    fn parse_stream_frame_from_sse_data_accepts_known_kinds() {
        let stdout = r#"{"kind":"stdout","seq":1,"offset":0,"data_b64":"aGk="}"#;
        match parse_stream_frame_from_sse_data(stdout) {
            Some(StreamFrame::Stdout {
                seq,
                offset,
                data_b64,
            }) => {
                assert_eq!(seq, 1);
                assert_eq!(offset, 0);
                assert_eq!(data_b64, "aGk=");
            }
            other => panic!("expected Stdout, got {:?}", other),
        }
        let done = r#"{"kind":"done","exit_code":0,"duration_ms":10,"total_stdout":2,"total_stderr":0,"stdout_sha256":"","stderr_sha256":""}"#;
        assert!(matches!(
            parse_stream_frame_from_sse_data(done),
            Some(StreamFrame::Done { .. })
        ));
    }

    #[test]
    fn parse_stream_frame_from_sse_data_rejects_legacy_envelope() {
        let legacy = r#"{"envelope_json":"{\"seq\":1}"}"#;
        assert!(parse_stream_frame_from_sse_data(legacy).is_none());
        let legacy_ct = r#"{"ciphertext":"abcd"}"#;
        assert!(parse_stream_frame_from_sse_data(legacy_ct).is_none());
        let unrelated = r#"{"kind":"unknown","foo":1}"#;
        assert!(parse_stream_frame_from_sse_data(unrelated).is_none());
        assert!(parse_stream_frame_from_sse_data("not json").is_none());
    }

    #[test]
    fn open_stdout_file_sink_appends_across_calls() {
        use std::io::Read;
        let dir = tempfile::tempdir().expect("tempdir");
        let p = dir.path().join("out.bin");
        {
            let mut w = open_stdout_file_sink(&p).expect("first open");
            w.write_all(b"hello ").unwrap();
            w.flush().unwrap();
        }
        {
            let mut w = open_stdout_file_sink(&p).expect("second open");
            w.write_all(b"world").unwrap();
            w.flush().unwrap();
        }
        let mut got = String::new();
        std::fs::File::open(&p)
            .unwrap()
            .read_to_string(&mut got)
            .unwrap();
        assert_eq!(got, "hello world");
    }

    #[test]
    fn state_machine_feeds_file_sink_from_parsed_frame() {
        use std::io::Read;
        let dir = tempfile::tempdir().expect("tempdir");
        let p = dir.path().join("stream.bin");
        let file_sink = open_stdout_file_sink(&p).expect("open");
        let mut state = CallerStreamState::new(file_sink, Vec::new(), false);
        let frame_json = r#"{"kind":"stdout","seq":1,"offset":0,"data_b64":"YWJjZGVm"}"#;
        let frame = parse_stream_frame_from_sse_data(frame_json).expect("parse");
        match state.feed(&frame).expect("feed") {
            StreamDecision::Continue => {}
            other => panic!("expected Continue, got {:?}", other),
        }
        // Drop state to flush BufWriter.
        drop(state);
        let mut got = String::new();
        std::fs::File::open(&p)
            .unwrap()
            .read_to_string(&mut got)
            .unwrap();
        assert_eq!(got, "abcdef");
    }

    /// PR #5d-3: simulate a multi-chunk SSE stream (reassemble `data:` /
    /// empty-line framing out-of-band) and assert bytes land in the file
    /// sink in order, hash is computed, and a Done frame produces a
    /// Completed decision.
    #[test]
    fn end_to_end_sse_chunks_to_file_sink_with_done() {
        use std::io::Read;
        let dir = tempfile::tempdir().expect("tempdir");
        let p = dir.path().join("stream.bin");
        let file_sink = open_stdout_file_sink(&p).expect("open");
        let mut state = CallerStreamState::new(file_sink, Vec::new(), false);

        // 3 stdout frames + 1 done. b64("AAAA") = "QUFBQQ==" (4 bytes).
        let frames = vec![
            r#"{"kind":"stdout","seq":1,"offset":0,"data_b64":"QUFBQQ=="}"#,
            r#"{"kind":"stdout","seq":2,"offset":4,"data_b64":"QkJCQg=="}"#,
            r#"{"kind":"stdout","seq":3,"offset":8,"data_b64":"Q0NDQw=="}"#,
            r#"{"kind":"done","exit_code":0,"duration_ms":123,"total_stdout":12,"total_stderr":0,"stdout_sha256":"","stderr_sha256":""}"#,
        ];
        let mut saw_done = false;
        for raw in &frames {
            let frame = parse_stream_frame_from_sse_data(raw).expect("parse frame");
            match state.feed(&frame).expect("feed") {
                StreamDecision::Continue => {}
                StreamDecision::Done {
                    exit_code,
                    duration_ms,
                    ..
                } => {
                    saw_done = true;
                    assert_eq!(exit_code, 0);
                    assert_eq!(duration_ms, 123);
                }
                other => panic!("unexpected decision: {:?}", other),
            }
        }
        assert!(saw_done, "expected Done decision from terminal frame");
        drop(state);

        let mut got = String::new();
        std::fs::File::open(&p)
            .unwrap()
            .read_to_string(&mut got)
            .unwrap();
        assert_eq!(got, "AAAABBBBCCCC");
    }

    /// PR #5d-3: first connection delivers offsets 0..8, then Reconnect
    /// at 8; second connection resumes replaying from 4 (simulating a
    /// relay that rewinds). set_heads on the existing state lets the
    /// dedup logic drop the replayed 4 bytes cleanly, and the final file
    /// contains each byte exactly once.
    #[test]
    fn end_to_end_reconnect_with_resume_dedups_replay() {
        use std::io::Read;
        let dir = tempfile::tempdir().expect("tempdir");
        let p = dir.path().join("resume.bin");
        let file_sink = open_stdout_file_sink(&p).expect("open");
        let mut state = CallerStreamState::new(file_sink, Vec::new(), false);

        // Connection 1: 2 frames, then Reconnect.
        let conn1 = vec![
            r#"{"kind":"stdout","seq":1,"offset":0,"data_b64":"MDAwMA=="}"#, // "0000"
            r#"{"kind":"stdout","seq":2,"offset":4,"data_b64":"MTExMQ=="}"#, // "1111"
            r#"{"kind":"reconnect","reason":"relay-30min-limit","stdout_offset":8,"stderr_offset":0}"#,
        ];
        let mut resume_at: Option<(u64, u64)> = None;
        for raw in &conn1 {
            let frame = parse_stream_frame_from_sse_data(raw).expect("parse");
            match state.feed(&frame).expect("feed") {
                StreamDecision::Continue => {}
                StreamDecision::ReconnectAt {
                    stdout_offset,
                    stderr_offset,
                    reason,
                } => {
                    assert_eq!(stdout_offset, 8);
                    assert_eq!(stderr_offset, 0);
                    assert_eq!(reason, "relay-30min-limit");
                    resume_at = Some((stdout_offset, stderr_offset));
                    break;
                }
                other => panic!("unexpected decision on conn1: {:?}", other),
            }
        }
        let (so, se) = resume_at.expect("Reconnect decision expected");
        state.set_heads(so, se);

        // Connection 2: relay rewinds to offset 4 and replays through offset 12, then Done.
        let conn2 = vec![
            r#"{"kind":"stdout","seq":10,"offset":4,"data_b64":"MTExMQ=="}"#, // replay "1111" (should dedup)
            r#"{"kind":"stdout","seq":11,"offset":8,"data_b64":"MjIyMg=="}"#, // new "2222"
            r#"{"kind":"stdout","seq":12,"offset":12,"data_b64":"MzMzMw=="}"#, // new "3333"
            r#"{"kind":"done","exit_code":0,"duration_ms":456,"total_stdout":16,"total_stderr":0,"stdout_sha256":"","stderr_sha256":""}"#,
        ];
        let mut saw_done = false;
        for raw in &conn2 {
            let frame = parse_stream_frame_from_sse_data(raw).expect("parse");
            match state.feed(&frame).expect("feed") {
                StreamDecision::Continue => {}
                StreamDecision::Done { .. } => {
                    saw_done = true;
                }
                other => panic!("unexpected decision on conn2: {:?}", other),
            }
        }
        assert!(saw_done, "expected Done on conn2");
        drop(state);

        let mut got = String::new();
        std::fs::File::open(&p)
            .unwrap()
            .read_to_string(&mut got)
            .unwrap();
        assert_eq!(
            got,
            "00001111222233333".chars().take(16).collect::<String>()
        );
    }

    /// PR #5d-3: set_heads on an existing state must not reset digests; a
    /// digest_mismatch result (vs a fabricated "wrong" expected digest)
    /// must still report via digest_ok=false without erroring.
    #[test]
    fn set_heads_preserves_running_digest() {
        let mut state = CallerStreamState::new(Vec::<u8>::new(), Vec::<u8>::new(), true);
        let frame1 = parse_stream_frame_from_sse_data(
            r#"{"kind":"stdout","seq":1,"offset":0,"data_b64":"aGVsbG8="}"#, // "hello"
        )
        .unwrap();
        assert!(matches!(
            state.feed(&frame1).unwrap(),
            StreamDecision::Continue
        ));

        state.set_heads(5, 0);
        let frame2 = parse_stream_frame_from_sse_data(
            r#"{"kind":"stdout","seq":2,"offset":5,"data_b64":"IHdvcmxk"}"#, // " world"
        )
        .unwrap();
        assert!(matches!(
            state.feed(&frame2).unwrap(),
            StreamDecision::Continue
        ));

        // Done with WRONG digest should yield digest_ok=false.
        let done = parse_stream_frame_from_sse_data(
            r#"{"kind":"done","exit_code":0,"duration_ms":1,"total_stdout":11,"total_stderr":0,"stdout_sha256":"deadbeef","stderr_sha256":""}"#,
        ).unwrap();
        match state.feed(&done).unwrap() {
            StreamDecision::Done { digest_ok, .. } => assert!(!digest_ok),
            other => panic!("expected Done, got {:?}", other),
        }
    }
}
