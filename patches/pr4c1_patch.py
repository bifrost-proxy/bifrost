#!/usr/bin/env python3
"""PR #4c-1: add &str-keyed variants of session_ring helpers so worker.rs
(which already holds String UUIDs from the relay) can call them without
round-trip parsing. The existing Uuid-keyed API is untouched.
"""
import pathlib, sys

p = pathlib.Path("crates/bifrost-admin/src/remote_invoke/session_ring.rs")
s = p.read_text()

# Insert a new section of &str-keyed helpers right after resume_stderr's fn end.
anchor = "pub fn resume_stderr("
i = s.index(anchor)
brace_open = s.index("{", i)
depth = 0; j = brace_open
while j < len(s):
    c = s[j]
    if c == "{": depth += 1
    elif c == "}":
        depth -= 1
        if depth == 0: break
    j += 1
end = j + 1

new_helpers = r'''

// ---------- PR #4c-1: &str-keyed convenience wrappers ----------
//
// worker.rs carries call_id as a String (the relay's canonical form). These
// wrappers parse on demand and delegate to the Uuid-keyed helpers, so
// malformed or non-UUID ids are treated as a silent no-op (same contract as
// the Uuid helpers for unknown ids). This keeps zero risk of panicking the
// hot path on unexpected input from the relay.

/// Parse `call_id` and mirror a stdout chunk if valid. Silent no-op on
/// parse failure or unknown call_id.
pub fn tee_stdout_str(call_id: &str, bytes: &[u8]) {
    if let Ok(id) = Uuid::parse_str(call_id) {
        tee_stdout(&id, bytes);
    }
}

/// Parse `call_id` and mirror a stderr chunk if valid. Silent no-op on
/// parse failure or unknown call_id.
pub fn tee_stderr_str(call_id: &str, bytes: &[u8]) {
    if let Ok(id) = Uuid::parse_str(call_id) {
        tee_stderr(&id, bytes);
    }
}

/// Parse `call_id` and finalize the session if valid. Silent no-op on
/// parse failure or unknown call_id.
pub fn finalize_session_str(call_id: &str, status: SessionStatus) {
    if let Ok(id) = Uuid::parse_str(call_id) {
        finalize_session(&id, status);
    }
}

/// Parse `call_id` and register a new session. Returns the canonical
/// string form of the newly assigned Uuid so callers that want to hand
/// the id back to the relay can do so without re-formatting. When
/// `preferred` is Some and parses as a Uuid, it is used as-is and the
/// session is created with that id; otherwise a fresh Uuid is minted.
pub fn register_session_str(capacity: usize, preferred: Option<&str>) -> String {
    if let Some(raw) = preferred {
        if let Ok(id) = Uuid::parse_str(raw) {
            // Insert at this specific id so the relay-assigned call_id
            // stays the primary handle. If an entry already exists (rare;
            // only on replay) we leave it alone.
            let reg = global_registry();
            if reg.get(&id).is_none() {
                reg.insert_with_id(id, capacity);
            }
            return id.to_string();
        }
    }
    register_session(capacity).to_string()
}
'''
s = s[:end] + new_helpers + s[end:]

# Also add an insert_with_id method on SessionRegistry. Find its impl block.
reg_impl_anchor = "impl SessionRegistry {"
if reg_impl_anchor not in s:
    print("ERROR: SessionRegistry impl not found", file=sys.stderr); sys.exit(1)
i = s.index(reg_impl_anchor)
# Find matching } of the impl block.
brace_open = s.index("{", i)
depth = 0; j = brace_open
while j < len(s):
    c = s[j]
    if c == "{": depth += 1
    elif c == "}":
        depth -= 1
        if depth == 0: break
    j += 1
impl_end = j

new_method = r'''
    /// PR #4c-1: insert a session with a caller-specified id (used when
    /// the relay already assigned a call_id and we want the session key
    /// to match). No-op if an entry with this id already exists.
    pub fn insert_with_id(&self, id: Uuid, capacity: usize) {
        let mut sessions = self.sessions.lock().expect("sessions poisoned");
        sessions.entry(id).or_insert_with(|| {
            std::sync::Arc::new(std::sync::Mutex::new(SessionState {
                stdout: SessionRing::new(capacity),
                stderr: SessionRing::new(capacity),
                status: SessionStatus::Running,
            }))
        });
    }
'''
s = s[:impl_end] + new_method + s[impl_end:]

p.write_text(s)
print(f"OK: updated {p}, new length {len(s)} bytes")
