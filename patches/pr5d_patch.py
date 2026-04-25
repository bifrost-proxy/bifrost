#!/usr/bin/env python3
"""PR #5d patch: add SSE-payload adapter + file sink factory to caller_stream_frame.rs.

Appends two new pub items before the existing `#[cfg(test)]` block:
  - fn parse_stream_frame_from_sse_data(data: &str) -> Option<StreamFrame>
  - fn open_stdout_file_sink(path: &Path) -> io::Result<BufWriter<File>>

Also adds 4 new unit tests inside the existing tests module.
"""
import base64, sys, pathlib, re

path = pathlib.Path("crates/bifrost-cli/src/commands/caller_stream_frame.rs")
src = path.read_text()

# --- 1. Inject new imports near the top (after existing `use` block) ---
import_anchor = "use std::io::{self, Write};"
if import_anchor not in src:
    print("ERROR: import anchor not found", file=sys.stderr); sys.exit(1)

new_imports = """use std::io::{self, BufWriter, Write};
use std::fs::{File, OpenOptions};
use std::path::Path;"""
src = src.replace("use std::io::{self, Write};", new_imports, 1)

# --- 2. Inject new pub fns right before `#[cfg(test)]` ---
test_anchor = "#[cfg(test)]\nmod tests {"
if test_anchor not in src:
    print("ERROR: test anchor not found", file=sys.stderr); sys.exit(1)

new_fns = '''/// PR #5d: parse an SSE `data:` payload string into a StreamFrame, if it
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
    let kind = v.get("kind")?.as_str()?;
    match kind {
        "stdout" | "stderr" | "heartbeat" | "reconnect" | "ack" | "done" | "error" => {}
        _ => return None,
    }
    serde_json::from_value::<StreamFrame>(v).ok()
}

/// PR #5d: open a BufWriter<File> for --output-file, in append mode so that
/// resumed calls can cleanly concatenate new bytes to a prior partial capture.
/// The buffer size (256 KiB) is chosen to amortize syscall overhead for the
/// large-output hot path without holding too much RAM for many concurrent
/// sinks.
pub fn open_stdout_file_sink(path: &Path) -> io::Result<BufWriter<File>> {
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    Ok(BufWriter::with_capacity(256 * 1024, file))
}

'''

src = src.replace(test_anchor, new_fns + test_anchor, 1)

# --- 3. Add 4 unit tests inside the existing tests module ---
# Find the last `}` of the tests module by locating the test_anchor then matching braces.
idx = src.find(test_anchor)
# Walk from there, find the matching closing brace.
depth = 0
i = idx + len(test_anchor) - 1  # position at the `{` after "mod tests "
# Actually we want the position of the `{`.
open_brace_pos = src.index("{", idx)
j = open_brace_pos
depth = 0
while j < len(src):
    c = src[j]
    if c == "{":
        depth += 1
    elif c == "}":
        depth -= 1
        if depth == 0:
            tests_close_pos = j
            break
    j += 1
else:
    print("ERROR: tests module closing brace not found", file=sys.stderr); sys.exit(1)

new_tests = '''
    #[test]
    fn parse_stream_frame_from_sse_data_accepts_known_kinds() {
        let stdout = r#"{"kind":"stdout","seq":1,"offset":0,"data_b64":"aGk="}"#;
        match parse_stream_frame_from_sse_data(stdout) {
            Some(StreamFrame::Stdout { seq, offset, data_b64 }) => {
                assert_eq!(seq, 1);
                assert_eq!(offset, 0);
                assert_eq!(data_b64, "aGk=");
            }
            other => panic!("expected Stdout, got {:?}", other),
        }
        let done = r#"{"kind":"done","exit_code":0,"duration_ms":10,"total_stdout":2,"total_stderr":0,"stdout_sha256":"","stderr_sha256":""}"#;
        assert!(matches!(parse_stream_frame_from_sse_data(done), Some(StreamFrame::Done { .. })));
    }

    #[test]
    fn parse_stream_frame_from_sse_data_rejects_legacy_envelope() {
        let legacy = r#"{"envelope_json":"{\\"seq\\":1}"}"#;
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
        std::fs::File::open(&p).unwrap().read_to_string(&mut got).unwrap();
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
        std::fs::File::open(&p).unwrap().read_to_string(&mut got).unwrap();
        assert_eq!(got, "abcdef");
    }
'''

# Insert before closing brace of tests mod
src = src[:tests_close_pos] + new_tests + src[tests_close_pos:]

path.write_text(src)
print(f"OK: updated {path}, new length {len(src)} bytes")
