#!/usr/bin/env python3
"""Add PR #4c-1 tests for &str wrappers + insert_with_id."""
import pathlib, re
p = pathlib.Path("crates/bifrost-admin/src/remote_invoke/session_ring.rs")
s = p.read_text()
# Find last } of tests mod.
anchor = "mod tests {"
idx = s.index(anchor)
open_brace = s.index("{", idx)
depth = 0; j = open_brace
while j < len(s):
    c = s[j]
    if c == "{": depth += 1
    elif c == "}":
        depth -= 1
        if depth == 0: break
    j += 1
close = j
new_tests = r'''
    #[test]
    fn str_wrappers_roundtrip_stdout() {
        // Valid str-form UUID registered, tee'd, drained.
        let id = Uuid::new_v4();
        let sid = id.to_string();
        assert!(register_session_str(&sid));
        // Duplicate register is idempotent.
        assert!(!register_session_str(&sid));
        tee_stdout_str(&sid, b"hello ");
        tee_stdout_str(&sid, b"world");
        finalize_session_str(&sid, SessionStatus::Done { exit_code: 0 });
        let (bytes, head, status) = resume_stdout(&id, 0).expect("resume");
        assert_eq!(bytes, b"hello world");
        assert_eq!(head, 11);
        assert!(matches!(status, SessionStatus::Done { exit_code: 0 }));
    }

    #[test]
    fn str_wrappers_noop_on_invalid_or_unknown_id() {
        // Invalid format: silent no-op, no panic.
        tee_stdout_str("not-a-uuid", b"ignored");
        tee_stderr_str("not-a-uuid", b"ignored");
        finalize_session_str("not-a-uuid", SessionStatus::Abandoned);
        assert!(!register_session_str("not-a-uuid"));

        // Valid UUID format but unknown: silent no-op for tee, finalize.
        let ghost = Uuid::new_v4().to_string();
        tee_stdout_str(&ghost, b"ignored");
        tee_stderr_str(&ghost, b"ignored");
        // Resume on unknown id yields UnknownCallId.
        let parsed = Uuid::parse_str(&ghost).unwrap();
        assert_eq!(resume_stdout(&parsed, 0), Err(ResumeError::UnknownCallId));
    }

    #[test]
    fn insert_with_id_keeps_existing_entry() {
        let id = Uuid::new_v4();
        global_registry().insert_with_id(id, 1024);
        tee_stdout(&id, b"first");
        // Second insert is a no-op: bytes must persist.
        global_registry().insert_with_id(id, 1024);
        let (bytes, _, _) = resume_stdout(&id, 0).unwrap();
        assert_eq!(bytes, b"first");
    }
'''
s = s[:close] + new_tests + s[close:]
p.write_text(s)
print("OK")
