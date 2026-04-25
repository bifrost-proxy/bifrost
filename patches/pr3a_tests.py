#!/usr/bin/env python3
"""PR #3a tests: idle_timeout fires when output stagnates; wall_clock still works."""
import pathlib, sys

p = pathlib.Path("crates/bifrost-admin/src/remote_invoke/executor.rs")
s = p.read_text()

# Insert new tests just before the final `}` of `mod tests { ... }` block.
# Anchor: last occurrence of "\n}\n" preceded by nothing — just find the last `}`
# which we know is at line 2926 (matches `mod tests {`).
# We append before the closing brace by splitting on the last `}` line.
lines = s.rstrip("\n").split("\n")
assert lines[-1] == "}", f"last line expected to be `}}`, got {lines[-1]!r}"

new_tests = '''
    #[test]
    fn test_execute_shell_exec_idle_timeout_kills_stuck_child() {
        // PR#3a: child sleeps longer than idle timeout without emitting output
        //        -> must be killed with "idle timeout" error.
        let _guard = crate::remote_invoke::remote_shell_test_guard();
        let dir = TempDir::new().expect("tempdir");
        let data_dir = dir.path().join("bifrost-data");
        std::fs::create_dir_all(&data_dir).expect("create data dir");
        bifrost_storage::set_data_dir(data_dir);

        RemoteShellStore::new()
            .expect("store")
            .save(&RemoteShellSet {
                schema_version: 1,
                version: 1,
                policies: vec![RemoteShellPolicy {
                    id: "idle-test".to_string(),
                    name: "idle-test".to_string(),
                    description: None,
                    enabled: true,
                    profile_id: None,
                    metadata: serde_json::json!({
                        "exec_mode": "shell_text",
                        "allowed_shell_patterns": ["^(?s:.*)$"],
                        "shell": streaming_shell_program(),
                        // No wall-clock timeout; idle should kick in first.
                        "max_idle_ms": 300u64
                    }),
                }],
                profiles: vec![],
            })
            .expect("save");

        let executor = RemoteInvokeExecutor::new("127.0.0.1", 8800);
        let cmd = RemoteCommand {
            kind: super::super::types::CommandKind::ShellExec,
            policy_id: Some("idle-test".to_string()),
            exec_mode: Some(ShellExecMode::ShellText),
            // Sleep ~5s without printing anything. Idle timeout 300ms must kill.
            command_text: Some("sleep 5".to_string()),
            ..Default::default()
        };

        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        let started = Instant::now();
        let err = runtime
            .block_on(executor.execute(&cmd))
            .expect_err("idle timeout must produce an error");
        let elapsed = started.elapsed();
        assert!(
            elapsed < Duration::from_millis(2_000),
            "idle timeout should have killed within a second or two, elapsed={:?}",
            elapsed
        );
        let msg = format!("{err}");
        assert!(
            msg.contains("idle timeout"),
            "expected idle timeout error, got: {msg}"
        );
    }

    #[test]
    fn test_execute_shell_exec_wall_clock_timeout_still_enforced() {
        // PR#3a: when command.timeout_ms is set, wall-clock must still kill
        //        even if output keeps streaming (idle deadline refreshed).
        let _guard = crate::remote_invoke::remote_shell_test_guard();
        let dir = TempDir::new().expect("tempdir");
        let data_dir = dir.path().join("bifrost-data");
        std::fs::create_dir_all(&data_dir).expect("create data dir");
        bifrost_storage::set_data_dir(data_dir);

        RemoteShellStore::new()
            .expect("store")
            .save(&RemoteShellSet {
                schema_version: 1,
                version: 1,
                policies: vec![RemoteShellPolicy {
                    id: "wall-test".to_string(),
                    name: "wall-test".to_string(),
                    description: None,
                    enabled: true,
                    profile_id: None,
                    metadata: serde_json::json!({
                        "exec_mode": "shell_text",
                        "allowed_shell_patterns": ["^(?s:.*)$"],
                        "shell": streaming_shell_program(),
                        // Large idle; small wall-clock should win.
                        "max_idle_ms": 60_000u64
                    }),
                }],
                profiles: vec![],
            })
            .expect("save");

        let executor = RemoteInvokeExecutor::new("127.0.0.1", 8800);
        let cmd = RemoteCommand {
            kind: super::super::types::CommandKind::ShellExec,
            policy_id: Some("wall-test".to_string()),
            exec_mode: Some(ShellExecMode::ShellText),
            // Print a dot every 50ms forever -> never idle; wall-clock must trip.
            command_text: Some(
                "i=0; while [ $i -lt 200 ]; do printf .; sleep 0.05; i=$((i+1)); done".to_string(),
            ),
            timeout_ms: Some(400),
            ..Default::default()
        };

        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        let started = Instant::now();
        let err = runtime
            .block_on(executor.execute(&cmd))
            .expect_err("wall-clock timeout must produce an error");
        let elapsed = started.elapsed();
        assert!(
            elapsed < Duration::from_millis(2_000),
            "wall-clock should have killed within 2s, elapsed={:?}",
            elapsed
        );
        let msg = format!("{err}");
        assert!(
            msg.contains("wall-clock timeout"),
            "expected wall-clock timeout error, got: {msg}"
        );
    }
'''

# Insert before final `}`
lines = lines[:-1] + [new_tests.rstrip("\n"), "}"]
p.write_text("\n".join(lines) + "\n")
print("INSERTED OK")
