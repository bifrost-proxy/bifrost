#!/usr/bin/env python3
"""PR #4c-2: inject session_ring register/tee/finalize into worker.rs.

Inject points in worker.rs:
1. Before `executor.execute_with_stdout_sink(&command, |chunk| {...}).await;`,
   call `session_ring::register_session_str(&cid);` to create the ring.
2. Inside the |chunk| closure, before `Self::encrypt_call_frame(...)`, call
   `session_ring::tee_stdout_str(&cid, chunk.as_bytes());`.
3. After the result match, when we have the exit code, call
   `session_ring::finalize_session_str(&cid, SessionStatus::Done { exit_code })`
   for the success path, and `SessionStatus::Failed { code }` for errors.

All three calls are silent no-ops if parsing fails, so this is safe even if
the relay ever sends a non-UUID call_id.
"""
import pathlib, sys, re

p = pathlib.Path("crates/bifrost-admin/src/remote_invoke/worker.rs")
s = p.read_text()

# --- 1. Add `use` for the helpers (local to this block, module-level import). ---
# session_ring is already declared as a sibling mod; use the wrappers.
# Search an existing `use super::session_ring` or `use crate::remote_invoke::session_ring` line.
if "use crate::remote_invoke::session_ring" not in s and "use super::session_ring" not in s:
    # Add at top near other `use` statements.
    # Find "use super::" or "use crate::remote_invoke::" region.
    anchor = "use super::"
    if anchor in s:
        idx = s.index(anchor)
        s = s[:idx] + "use super::session_ring;\n" + s[idx:]
    else:
        print("WARN: no super:: import anchor; adding at top after last `use`", file=sys.stderr)
        # Fallback: add after first blank line below the first `use` block.
        m = re.search(r"^use .+?;\n(?=\n)", s, re.MULTILINE | re.DOTALL)
        if m:
            s = s[:m.end()] + "use super::session_ring;\n" + s[m.end():]

# --- 2. Inject register_session_str before execute_with_stdout_sink. ---
anchor_exec = "let result = executor\n                .execute_with_stdout_sink(&command, |chunk| {"
if anchor_exec not in s:
    print("ERROR: execute_with_stdout_sink anchor not found", file=sys.stderr); sys.exit(1)

rep_exec = """// PR #4c-2: register a session in the global ring so tee/resume can work.
            let _session_registered = session_ring::register_session_str(&cid);

            let result = executor
                .execute_with_stdout_sink(&command, |chunk| {"""
s = s.replace(anchor_exec, rep_exec, 1)

# --- 3. Inject tee_stdout_str inside the closure, before encrypt_call_frame. ---
# Locate the first `async move {` inside the closure (unique by context).
anchor_tee = """                    async move {
                        let envelope = Self::encrypt_call_frame("""
if anchor_tee not in s:
    print("ERROR: tee anchor not found", file=sys.stderr); sys.exit(1)

rep_tee = """                    async move {
                        // PR #4c-2: mirror stdout bytes into the session ring
                        // for resume. Silent no-op if cid is not a UUID.
                        session_ring::tee_stdout_str(&cid, chunk.as_bytes());
                        let envelope = Self::encrypt_call_frame("""
s = s.replace(anchor_tee, rep_tee, 1)

# --- 4. Inject finalize_session_str after the result match, both Ok/Err arms. ---
# Find the "match result {" after the execute_with_stdout_sink. Only one in this scope.
# We identify by scanning forward from the rep_exec insertion.
idx_after = s.index(rep_exec) + len(rep_exec)
match_anchor = "match result {"
m_pos = s.index(match_anchor, idx_after)

# Find "Ok(response) => {" and "Err(error)" arm bodies and inject at their
# first lines.
ok_arm = "Ok(response) => {"
ok_pos = s.index(ok_arm, m_pos)
# Line after "Ok(response) => {" — we add our call right after it on a new line.
ok_inject = (
    "Ok(response) => {\n"
    "                    // PR #4c-2: finalize the session ring for successful exit.\n"
    "                    session_ring::finalize_session_str(\n"
    "                        &cid,\n"
    "                        super::session_ring::SessionStatus::Done {\n"
    "                            exit_code: response.exit_code,\n"
    "                        },\n"
    "                    );"
)
s = s[:ok_pos] + ok_inject + s[ok_pos + len(ok_arm):]

# Err arm. Look for "Err(error) => {" after the ok_pos.
err_arm_start = s.find("Err(error) => {", ok_pos)
if err_arm_start != -1:
    err_inject = (
        "Err(error) => {\n"
        "                    // PR #4c-2: finalize the session ring for failure.\n"
        "                    session_ring::finalize_session_str(\n"
        "                        &cid,\n"
        "                        super::session_ring::SessionStatus::Failed {\n"
        "                            code: error.to_string(),\n"
        "                        },\n"
        "                    );"
    )
    s = s[:err_arm_start] + err_inject + s[err_arm_start + len("Err(error) => {"):]

p.write_text(s)
print(f"OK: updated {p}, new length {len(s)} bytes")
