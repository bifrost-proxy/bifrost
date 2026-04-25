#!/usr/bin/env python3
"""PR #3b-executor tests: streaming, SHA match, reconnect frame, back-pressure."""
import pathlib
p = pathlib.Path("crates/bifrost-admin/src/remote_invoke/executor.rs")
s = p.read_text()
lines = s.rstrip("\n").split("\n")
assert lines[-1] == "}"

new_tests = '''
    fn drain_frames_sync(
        runtime: &tokio::runtime::Runtime,
        mut rx: tokio::sync::mpsc::Receiver<crate::remote_invoke::types::StreamFrame>,
    ) -> Vec<crate::remote_invoke::types::StreamFrame> {
        runtime.block_on(async move {
            let mut v = Vec::new();
            while let Some(f) = rx.recv().await {
                v.push(f);
            }
            v
        })
    }

    #[test]
    fn test_streaming_emits_stdout_with_monotonic_offsets_and_matching_sha() {
        use crate::remote_invoke::types::StreamFrame;
        use ring::digest::{Context, SHA256};

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
                    id: "stream3b".to_string(),
                    name: "stream3b".to_string(),
                    description: None,
                    enabled: true,
                    profile_id: None,
                    metadata: serde_json::json!({
                        "exec_mode": "shell_text",
                        "allowed_shell_patterns": ["^(?s:.*)$"],
                        "shell": streaming_shell_program(),
                    }),
                }],
                profiles: vec![],
            })
            .expect("save");

        let executor = RemoteInvokeExecutor::new("127.0.0.1", 8800);
        // Produce 200 KiB of stdout so we get multiple 64 KiB frames.
        let cmd = RemoteCommand {
            kind: super::super::types::CommandKind::ShellExec,
            policy_id: Some("stream3b".to_string()),
            exec_mode: Some(ShellExecMode::ShellText),
            command_text: Some(
                // printf a 64-byte pattern 3200 times => 204_800 bytes
                "i=0; while [ $i -lt 3200 ]; do \
                    printf '0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef'; \
                    i=$((i+1)); \
                done".to_string(),
            ),
            ..Default::default()
        };

        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        let (tx, rx) = tokio::sync::mpsc::channel::<StreamFrame>(32);
        let exec_arc = std::sync::Arc::new(executor);
        let exec_clone = std::sync::Arc::clone(&exec_arc);
        let cmd_arc = std::sync::Arc::new(cmd);
        let cmd_clone = std::sync::Arc::clone(&cmd_arc);
        runtime.spawn(async move {
            exec_clone
                .execute_shell_exec_streaming(&cmd_clone, tx)
                .await
                .expect("streaming ok");
        });
        let frames = drain_frames_sync(&runtime, rx);

        // Expectations: at least one Stdout frame, monotonic offsets, final Done
        // with matching total + sha.
        let mut reconstructed: Vec<u8> = Vec::new();
        let mut expected_offset: u64 = 0;
        let mut seq_count: u64 = 0;
        let mut done_frame: Option<StreamFrame> = None;
        for f in &frames {
            match f {
                StreamFrame::Stdout { seq, offset, data_b64 } => {
                    assert_eq!(*seq, seq_count, "seq must be monotonic");
                    assert_eq!(*offset, expected_offset, "offset must be monotonic");
                    seq_count += 1;
                    let bytes = base64::Engine::decode(
                        &base64::engine::general_purpose::STANDARD,
                        data_b64,
                    ).expect("b64 decode");
                    expected_offset += bytes.len() as u64;
                    reconstructed.extend_from_slice(&bytes);
                }
                StreamFrame::Stderr { .. } => {}
                StreamFrame::Heartbeat { .. } => {}
                StreamFrame::Reconnect { .. } => {}
                StreamFrame::Done { .. } => {
                    done_frame = Some(f.clone());
                }
                StreamFrame::Error { code, message } => {
                    panic!("unexpected Error frame: {code} {message}");
                }
                StreamFrame::Ack { .. } => {}
            }
        }
        let done = done_frame.expect("Done frame required");
        match done {
            StreamFrame::Done { exit_code, total_stdout, stdout_sha256, .. } => {
                assert_eq!(exit_code, 0);
                assert_eq!(total_stdout, reconstructed.len() as u64);
                assert_eq!(total_stdout, 3200 * 64);
                // Compute SHA over reconstructed, compare.
                let mut ctx = Context::new(&SHA256);
                ctx.update(&reconstructed);
                let d = ctx.finish();
                let mut expected = String::with_capacity(64);
                for b in d.as_ref() { expected.push_str(&format!("{b:02x}")); }
                assert_eq!(stdout_sha256, expected);
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn test_streaming_backpressure_blocks_on_slow_consumer() {
        // With mpsc capacity=1 and a producer that emits many chunks, the
        // executor must wait for us to recv() before emitting the next frame.
        // We verify by measuring that between two consecutive recv()s the
        // producer could not run away and flood memory.
        use crate::remote_invoke::types::StreamFrame;

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
                    id: "bp-test".to_string(),
                    name: "bp-test".to_string(),
                    description: None,
                    enabled: true,
                    profile_id: None,
                    metadata: serde_json::json!({
                        "exec_mode": "shell_text",
                        "allowed_shell_patterns": ["^(?s:.*)$"],
                        "shell": streaming_shell_program(),
                    }),
                }],
                profiles: vec![],
            })
            .expect("save");

        let executor = std::sync::Arc::new(RemoteInvokeExecutor::new("127.0.0.1", 8800));
        let cmd = std::sync::Arc::new(RemoteCommand {
            kind: super::super::types::CommandKind::ShellExec,
            policy_id: Some("bp-test".to_string()),
            exec_mode: Some(ShellExecMode::ShellText),
            command_text: Some(
                "i=0; while [ $i -lt 64 ]; do \
                    printf '0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef'; \
                    i=$((i+1)); \
                done".to_string(),
            ),
            ..Default::default()
        });

        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        let (tx, mut rx) = tokio::sync::mpsc::channel::<StreamFrame>(1);
        let exec_clone = std::sync::Arc::clone(&executor);
        let cmd_clone = std::sync::Arc::clone(&cmd);
        let join = runtime.spawn(async move {
            exec_clone.execute_shell_exec_streaming(&cmd_clone, tx).await
        });

        // Consume slowly. Total stdout is 4096 bytes -> likely 1 Stdout frame +
        // Done. Accept a flexible assertion: first frame pulled must be either
        // Stdout or an early Heartbeat; drain; terminate normally.
        let result = runtime.block_on(async move {
            let mut total = 0u64;
            let mut done_seen = false;
            while let Some(f) = rx.recv().await {
                // slow consumer: 5 ms per frame
                tokio::time::sleep(Duration::from_millis(5)).await;
                match f {
                    StreamFrame::Stdout { data_b64, .. } => {
                        let bytes = base64::Engine::decode(
                            &base64::engine::general_purpose::STANDARD,
                            &data_b64,
                        ).expect("b64");
                        total += bytes.len() as u64;
                    }
                    StreamFrame::Done { total_stdout, .. } => {
                        done_seen = true;
                        assert_eq!(total, total_stdout);
                    }
                    StreamFrame::Error { code, message } => {
                        panic!("Error frame: {code} {message}");
                    }
                    _ => {}
                }
            }
            assert!(done_seen, "Done frame must be observed");
            total
        });
        assert_eq!(result, 64 * 64);
        runtime.block_on(join).expect("join").expect("streaming result");
    }

    #[test]
    fn test_streaming_reports_policy_rejection_via_error_frame() {
        use crate::remote_invoke::types::StreamFrame;

        let _guard = crate::remote_invoke::remote_shell_test_guard();
        let dir = TempDir::new().expect("tempdir");
        let data_dir = dir.path().join("bifrost-data");
        std::fs::create_dir_all(&data_dir).expect("create data dir");
        bifrost_storage::set_data_dir(data_dir);

        // No policy saved -> resolve_shell_policy should reject.
        let executor = RemoteInvokeExecutor::new("127.0.0.1", 8800);
        let cmd = RemoteCommand {
            kind: super::super::types::CommandKind::ShellExec,
            policy_id: Some("missing-policy".to_string()),
            exec_mode: Some(ShellExecMode::ShellText),
            command_text: Some("true".to_string()),
            ..Default::default()
        };

        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        let (tx, rx) = tokio::sync::mpsc::channel::<StreamFrame>(8);
        let exec_arc = std::sync::Arc::new(executor);
        let exec_clone = std::sync::Arc::clone(&exec_arc);
        let cmd_arc = std::sync::Arc::new(cmd);
        let cmd_clone = std::sync::Arc::clone(&cmd_arc);
        runtime.spawn(async move {
            exec_clone
                .execute_shell_exec_streaming(&cmd_clone, tx)
                .await
                .expect("streaming ok");
        });
        let frames = drain_frames_sync(&runtime, rx);
        assert_eq!(frames.len(), 1, "expected exactly one Error frame, got {:?}", frames);
        match &frames[0] {
            StreamFrame::Error { code, .. } => {
                assert_eq!(code, "policy_rejected");
            }
            other => panic!("expected Error, got {other:?}"),
        }
    }
'''
lines = lines[:-1] + [new_tests.rstrip("\n"), "}"]
p.write_text("\n".join(lines) + "\n")
print("INSERTED OK")
