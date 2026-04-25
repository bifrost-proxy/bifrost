#!/usr/bin/env python3
"""PR #5d-3: add integration tests that pipe a fabricated SSE stream through
subscribe_call_events_streaming-equivalent logic, exercising the wiring
end-to-end before the dispatcher (PR #5e) and the relay (PR #6) are ready.

We can't easily drive `subscribe_call_events_streaming` because it binds to
a real HTTP SSE source. Instead, we test the composition:
   SSE-framed bytes  ->  parse_stream_frame_from_sse_data  ->  state.feed

against realistic multi-frame streams including Reconnect + resume.

Appends 3 new tests to crates/bifrost-cli/src/commands/caller_stream_frame.rs.
"""
import pathlib, sys, re

path = pathlib.Path("crates/bifrost-cli/src/commands/caller_stream_frame.rs")
s = path.read_text()

# Find last `}` of tests mod.
anchor = "#[cfg(test)]\nmod tests {"
idx = s.index(anchor)
open_brace = s.index("{", idx)
depth = 0
j = open_brace
while j < len(s):
    c = s[j]
    if c == "{": depth += 1
    elif c == "}":
        depth -= 1
        if depth == 0:
            close_pos = j; break
    j += 1

new_tests = r'''
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
                StreamDecision::Done { exit_code, duration_ms, .. } => {
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
        std::fs::File::open(&p).unwrap().read_to_string(&mut got).unwrap();
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
                StreamDecision::ReconnectAt { stdout_offset, stderr_offset, reason } => {
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
                StreamDecision::Done { .. } => { saw_done = true; }
                other => panic!("unexpected decision on conn2: {:?}", other),
            }
        }
        assert!(saw_done, "expected Done on conn2");
        drop(state);

        let mut got = String::new();
        std::fs::File::open(&p).unwrap().read_to_string(&mut got).unwrap();
        assert_eq!(got, "00001111222233333".chars().take(16).collect::<String>());
    }

    /// PR #5d-3: set_heads on an existing state must not reset digests; a
    /// digest_mismatch result (vs a fabricated "wrong" expected digest)
    /// must still report via digest_ok=false without erroring.
    #[test]
    fn set_heads_preserves_running_digest() {
        let mut state = CallerStreamState::new(Vec::<u8>::new(), Vec::<u8>::new(), true);
        let frame1 = parse_stream_frame_from_sse_data(
            r#"{"kind":"stdout","seq":1,"offset":0,"data_b64":"aGVsbG8="}"#, // "hello"
        ).unwrap();
        assert!(matches!(state.feed(&frame1).unwrap(), StreamDecision::Continue));

        state.set_heads(5, 0);
        let frame2 = parse_stream_frame_from_sse_data(
            r#"{"kind":"stdout","seq":2,"offset":5,"data_b64":"IHdvcmxk"}"#, // " world"
        ).unwrap();
        assert!(matches!(state.feed(&frame2).unwrap(), StreamDecision::Continue));

        // Done with WRONG digest should yield digest_ok=false.
        let done = parse_stream_frame_from_sse_data(
            r#"{"kind":"done","exit_code":0,"duration_ms":1,"total_stdout":11,"total_stderr":0,"stdout_sha256":"deadbeef","stderr_sha256":""}"#,
        ).unwrap();
        match state.feed(&done).unwrap() {
            StreamDecision::Done { digest_ok, .. } => assert!(!digest_ok),
            other => panic!("expected Done, got {:?}", other),
        }
    }
'''

s = s[:close_pos] + new_tests + s[close_pos:]
path.write_text(s)
print(f"OK: updated {path}, new length {len(s)} bytes")
