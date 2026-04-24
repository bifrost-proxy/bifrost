//! PR #5e-prep: pure helpers that construct `StreamFrame` values for the
//! executor side of the large-output pipeline.
//!
//! These helpers are intentionally **not** wired into `worker.rs` yet.
//! They exist so that, once the relay v4 protocol lands (PR #6), the
//! hot-path injection in `worker.rs` becomes a trivial call site change
//! with shape/encoding already validated by unit tests.
//!
//! Invariants enforced by these helpers:
//! - `Stdout`/`Stderr` frames carry the **absolute byte offset** of the
//!   first byte in `data_b64` within the stream. Monotonic `seq` is the
//!   caller's responsibility (bumped per emission).
//! - `data_b64` is standard (padded) base64 of the raw bytes — matches
//!   the decoder in `crates/bifrost-cli/src/commands/caller_stream_frame.rs`.
//! - `Done` carries total byte counts and hex SHA-256 digests so the
//!   caller can verify the streamed content against the authoritative
//!   length/digest once the command finishes.

use base64::Engine;

use super::types::{ObjectRef, StreamFrame};

/// Build a `StreamFrame::Stdout` for the given chunk.
///
/// `offset` MUST equal the absolute byte offset of `bytes[0]` within the
/// full stdout stream for the call. `seq` MUST be strictly monotonic per
/// (call_id, kind) — duplicates will confuse resume dedup.
pub fn build_stdout_frame(seq: u64, offset: u64, bytes: &[u8]) -> StreamFrame {
    let engine = base64::engine::general_purpose::STANDARD;
    StreamFrame::Stdout {
        seq,
        offset,
        data_b64: engine.encode(bytes),
    }
}

/// Mirror of [`build_stdout_frame`] for stderr.
pub fn build_stderr_frame(seq: u64, offset: u64, bytes: &[u8]) -> StreamFrame {
    let engine = base64::engine::general_purpose::STANDARD;
    StreamFrame::Stderr {
        seq,
        offset,
        data_b64: engine.encode(bytes),
    }
}

/// Build a heartbeat frame advertising the executor's latest offsets.
pub fn build_heartbeat_frame(
    ts: u64,
    stdout_offset: Option<u64>,
    stderr_offset: Option<u64>,
) -> StreamFrame {
    StreamFrame::Heartbeat {
        ts,
        stdout_offset,
        stderr_offset,
    }
}

/// Build a terminal `Done` frame.
///
/// `stdout_sha256` / `stderr_sha256` are lowercase hex digests of the
/// full streams. Object refs are optional and describe out-of-band
/// storage (for streams larger than the relay inline limit).
#[allow(clippy::too_many_arguments)]
pub fn build_done_frame(
    exit_code: i32,
    total_stdout: u64,
    total_stderr: u64,
    stdout_sha256: String,
    stderr_sha256: String,
    duration_ms: u64,
    stdout_object: Option<ObjectRef>,
    stderr_object: Option<ObjectRef>,
) -> StreamFrame {
    StreamFrame::Done {
        exit_code,
        total_stdout,
        total_stderr,
        stdout_sha256,
        stderr_sha256,
        duration_ms,
        stdout_object,
        stderr_object,
    }
}

/// Build a terminal `Error` frame.
pub fn build_error_frame(code: impl Into<String>, message: impl Into<String>) -> StreamFrame {
    StreamFrame::Error {
        code: code.into(),
        message: message.into(),
    }
}

/// Build an advisory `Reconnect` frame indicating the receiver should
/// re-subscribe with a `Resume(call_id, stdout_offset, stderr_offset)`
/// hint.
pub fn build_reconnect_frame(
    reason: impl Into<String>,
    stdout_offset: u64,
    stderr_offset: u64,
) -> StreamFrame {
    StreamFrame::Reconnect {
        reason: reason.into(),
        stdout_offset,
        stderr_offset,
    }
}

/// Serialize a `StreamFrame` to its canonical JSON payload string —
/// exactly what the relay should forward as the SSE `data:` field.
///
/// Returns `None` on serialization failure (should be unreachable for
/// any frame produced by the helpers above).
pub fn frame_to_json(frame: &StreamFrame) -> Option<String> {
    serde_json::to_string(frame).ok()
}

/// Minimal monotonic sequence counter. `worker.rs` will hold one per
/// (call_id, kind) once wired. Kept here so tests can exercise the full
/// "emit chunk → bump seq → re-emit" lifecycle without borrowing from a
/// real shell executor.
#[derive(Debug, Default, Clone, Copy)]
pub struct FrameSeq {
    next: u64,
}

impl FrameSeq {
    pub fn new() -> Self {
        Self { next: 0 }
    }
    pub fn take(&mut self) -> u64 {
        let v = self.next;
        self.next = self.next.saturating_add(1);
        v
    }
    pub fn peek(&self) -> u64 {
        self.next
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn kind_of(v: &Value) -> &str {
        v.get("kind").and_then(|k| k.as_str()).unwrap_or("")
    }

    #[test]
    fn stdout_frame_has_correct_shape() {
        let f = build_stdout_frame(7, 1024, b"hello");
        let json = frame_to_json(&f).expect("serialize");
        let v: Value = serde_json::from_str(&json).expect("parse");
        assert_eq!(kind_of(&v), "stdout");
        assert_eq!(v.get("seq").and_then(|x| x.as_u64()), Some(7));
        assert_eq!(v.get("offset").and_then(|x| x.as_u64()), Some(1024));
        // "hello" => aGVsbG8=
        assert_eq!(v.get("data_b64").and_then(|x| x.as_str()), Some("aGVsbG8="));
    }

    #[test]
    fn stderr_frame_has_correct_shape() {
        let f = build_stderr_frame(0, 0, b"");
        let json = frame_to_json(&f).expect("serialize");
        let v: Value = serde_json::from_str(&json).expect("parse");
        assert_eq!(kind_of(&v), "stderr");
        assert_eq!(v.get("seq").and_then(|x| x.as_u64()), Some(0));
        assert_eq!(v.get("offset").and_then(|x| x.as_u64()), Some(0));
        assert_eq!(v.get("data_b64").and_then(|x| x.as_str()), Some(""));
    }

    #[test]
    fn heartbeat_frame_omits_none_offsets() {
        let f = build_heartbeat_frame(1_700_000_000, Some(1024), None);
        let json = frame_to_json(&f).expect("serialize");
        let v: Value = serde_json::from_str(&json).expect("parse");
        assert_eq!(kind_of(&v), "heartbeat");
        assert_eq!(v.get("ts").and_then(|x| x.as_u64()), Some(1_700_000_000));
        assert_eq!(v.get("stdout_offset").and_then(|x| x.as_u64()), Some(1024));
        assert!(v.get("stderr_offset").is_none(), "None must skip serialize");
    }

    #[test]
    fn done_frame_has_correct_shape() {
        let f = build_done_frame(
            0,
            5,
            0,
            "abc123".to_string(),
            "def456".to_string(),
            42,
            None,
            None,
        );
        let json = frame_to_json(&f).expect("serialize");
        let v: Value = serde_json::from_str(&json).expect("parse");
        assert_eq!(kind_of(&v), "done");
        assert_eq!(v.get("exit_code").and_then(|x| x.as_i64()), Some(0));
        assert_eq!(v.get("total_stdout").and_then(|x| x.as_u64()), Some(5));
        assert_eq!(v.get("duration_ms").and_then(|x| x.as_u64()), Some(42));
        assert_eq!(
            v.get("stdout_sha256").and_then(|x| x.as_str()),
            Some("abc123")
        );
        assert!(v.get("stdout_object").is_none());
        assert!(v.get("stderr_object").is_none());
    }

    #[test]
    fn error_frame_has_correct_shape() {
        let f = build_error_frame("exec_failed", "boom");
        let json = frame_to_json(&f).expect("serialize");
        let v: Value = serde_json::from_str(&json).expect("parse");
        assert_eq!(kind_of(&v), "error");
        assert_eq!(v.get("code").and_then(|x| x.as_str()), Some("exec_failed"));
        assert_eq!(v.get("message").and_then(|x| x.as_str()), Some("boom"));
    }

    #[test]
    fn reconnect_frame_has_correct_shape() {
        let f = build_reconnect_frame("relay_wall_clock", 1024, 512);
        let json = frame_to_json(&f).expect("serialize");
        let v: Value = serde_json::from_str(&json).expect("parse");
        assert_eq!(kind_of(&v), "reconnect");
        assert_eq!(
            v.get("reason").and_then(|x| x.as_str()),
            Some("relay_wall_clock")
        );
        assert_eq!(v.get("stdout_offset").and_then(|x| x.as_u64()), Some(1024));
        assert_eq!(v.get("stderr_offset").and_then(|x| x.as_u64()), Some(512));
    }

    #[test]
    fn frame_seq_is_strictly_monotonic() {
        let mut s = FrameSeq::new();
        assert_eq!(s.peek(), 0);
        assert_eq!(s.take(), 0);
        assert_eq!(s.take(), 1);
        assert_eq!(s.take(), 2);
        assert_eq!(s.peek(), 3);
    }

    #[test]
    fn multi_chunk_offsets_accumulate() {
        // Simulate three stdout chunks with monotonic seq and offsets.
        let mut seq = FrameSeq::new();
        let chunks: [&[u8]; 3] = [b"aaa", b"bbbb", b"cc"];
        let mut offset: u64 = 0;
        let mut frames = Vec::new();
        for c in chunks {
            let f = build_stdout_frame(seq.take(), offset, c);
            offset += c.len() as u64;
            frames.push(f);
        }
        // Validate seqs 0,1,2 and offsets 0,3,7.
        let expected_offsets = [0u64, 3, 7];
        for (i, f) in frames.iter().enumerate() {
            match f {
                StreamFrame::Stdout { seq, offset, .. } => {
                    assert_eq!(*seq, i as u64);
                    assert_eq!(*offset, expected_offsets[i]);
                }
                _ => panic!("expected Stdout"),
            }
        }
    }

    #[test]
    fn roundtrip_through_json_preserves_bytes() {
        // Include non-UTF-8 bytes to ensure base64 path is byte-exact.
        let raw: Vec<u8> = (0u8..=255).collect();
        let f = build_stdout_frame(0, 0, &raw);
        let json = frame_to_json(&f).expect("serialize");
        let parsed: StreamFrame = serde_json::from_str(&json).expect("parse");
        let engine = base64::engine::general_purpose::STANDARD;
        match parsed {
            StreamFrame::Stdout { data_b64, .. } => {
                let decoded = engine.decode(&data_b64).expect("b64 decode");
                assert_eq!(decoded, raw);
            }
            _ => panic!("expected Stdout"),
        }
    }
}
