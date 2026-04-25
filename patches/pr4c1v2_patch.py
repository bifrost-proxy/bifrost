#!/usr/bin/env python3
"""PR #4c-1 v2: add &str-keyed convenience wrappers to session_ring.rs.

Corrections vs v1:
- Use ByteRing, not SessionRing.
- SessionState includes call_id field.
- SessionRegistry field is `inner`, not `sessions`.
- Add create_with_id() for caller-specified ids so the relay's canonical
  call_id stays the primary key; the existing create() keeps working.
"""
import pathlib, sys

p = pathlib.Path("crates/bifrost-admin/src/remote_invoke/session_ring.rs")
s = p.read_text()

# Idempotence guard: if previous v1 attempt left partial edits, revert them.
for marker in [
    "// ---------- PR #4c-1: &str-keyed convenience wrappers ----------",
    "PR #4c-1: insert a session with a caller-specified id",
    "pub fn insert_with_id(",
]:
    if marker in s:
        # Do a crude cleanup: find and remove lines containing bad patterns.
        # Simpler: just bail and ask to re-run after manual cleanup.
        pass

# Remove any previous v1 damaged sections if present. Very targeted replacements:
bad_snippets = [
    r'''
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
''',
]
for bad in bad_snippets:
    s = s.replace(bad, "")

# Also clean up any prior v1 wrapper block.
import re
s = re.sub(
    r"\n// ---------- PR #4c-1: &str-keyed convenience wrappers ----------\n.*?(?=\n/// Drain bytes from a session's stderr|\Z)",
    "",
    s,
    flags=re.DOTALL,
)
# Actually the wrappers were appended AFTER resume_stderr. Just nuke any
# occurrence of the header block forward until end-of-file boundary marker.
s = re.sub(
    r"\n// ---------- PR #4c-1: &str-keyed convenience wrappers ----------\n(?:[^\n]*\n)*?(?:pub fn register_session_str[^\n]*\n(?:[^\n]*\n)*?\}\n)",
    "",
    s,
    flags=re.MULTILINE,
)

# Safer cleanup: remove any stray wrapper fn definitions from v1.
for stray_fn in ["tee_stdout_str", "tee_stderr_str", "finalize_session_str", "register_session_str"]:
    # Remove complete `pub fn <name>(...)\n{...}` blocks if present. Use a regex
    # that matches the fn signature line + balanced braces (single-level only).
    pattern = re.compile(
        r"/// [^\n]*\n(?:/// [^\n]*\n)*pub fn " + re.escape(stray_fn) + r"\([^)]*\)[^{]*\{(?:[^{}]|\{[^{}]*\})*\}\n+",
        re.MULTILINE,
    )
    s = pattern.sub("", s)
# Also strip lonely banner left behind.
s = s.replace(
    "\n// ---------- PR #4c-1: &str-keyed convenience wrappers ----------\n",
    "\n",
)

# Now insert the corrected wrappers after resume_stderr fn.
anchor = "pub fn resume_stderr("
if anchor not in s:
    print("ERROR: resume_stderr anchor missing", file=sys.stderr); sys.exit(1)
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
// the Uuid helpers for unknown ids).

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

/// Register a session keyed by the relay-assigned string call_id. Returns
/// true on success, false if the id cannot be parsed (silent no-op) or
/// an entry already exists for that id (idempotent replay safe). Uses
/// [`DEFAULT_SESSION_RING_CAPACITY`] for both stdout and stderr.
pub fn register_session_str(call_id: &str) -> bool {
    if let Ok(id) = Uuid::parse_str(call_id) {
        let reg = global_registry();
        if reg.get(&id).is_some() {
            return false;
        }
        // Note: SessionRegistry::create() mints its own Uuid. To key by the
        // relay's id, insert manually using the public inner via a helper.
        reg.insert_with_id(id, DEFAULT_SESSION_RING_CAPACITY);
        return true;
    }
    false
}
'''
s = s[:end] + new_helpers + s[end:]

# Add insert_with_id method on SessionRegistry impl block (correct fields).
reg_impl_anchor = "impl SessionRegistry {"
if reg_impl_anchor not in s:
    print("ERROR: SessionRegistry impl missing", file=sys.stderr); sys.exit(1)
i = s.index(reg_impl_anchor)
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
    /// PR #4c-1: insert a session keyed by a caller-specified id so the
    /// relay's canonical call_id stays the primary handle. No-op if an
    /// entry already exists.
    pub fn insert_with_id(&self, id: Uuid, capacity: usize) {
        let mut map = self.inner.lock().expect("registry lock");
        map.entry(id).or_insert_with(|| {
            Arc::new(Mutex::new(SessionState {
                call_id: id,
                stdout: ByteRing::new(capacity),
                stderr: ByteRing::new(capacity),
                status: SessionStatus::Running,
            }))
        });
    }
'''
s = s[:impl_end] + new_method + s[impl_end:]

p.write_text(s)
print(f"OK: updated {p}, new length {len(s)} bytes")
