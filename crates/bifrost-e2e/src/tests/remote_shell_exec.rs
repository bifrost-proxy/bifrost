use std::fs;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use std::time::{SystemTime, UNIX_EPOCH};

use bifrost_admin::remote_invoke::types::{CommandKind, RemoteCommand, ShellExecMode};
use bifrost_admin::remote_invoke::RemoteInvokeExecutor;
use bifrost_storage::{RemoteShellPolicy, RemoteShellSet, RemoteShellStore};

use crate::runner::TestCase;

fn platform_echo_executable() -> String {
    if cfg!(target_os = "windows") {
        let windir = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".to_string());
        format!(r"{}\System32\cmd.exe", windir)
    } else {
        "/bin/echo".to_string()
    }
}

fn platform_echo_argv() -> Vec<String> {
    if cfg!(target_os = "windows") {
        let exe = platform_echo_executable();
        vec![
            exe,
            "/C".to_string(),
            "echo".to_string(),
            "hello-e2e".to_string(),
        ]
    } else {
        vec!["/bin/echo".to_string(), "hello-e2e".to_string()]
    }
}

fn platform_denied_executable() -> String {
    if cfg!(target_os = "windows") {
        r"C:\Windows\System32\where.exe".to_string()
    } else {
        "/bin/date".to_string()
    }
}

fn platform_cwd() -> String {
    std::env::temp_dir().to_string_lossy().to_string()
}

fn platform_expected_stdout() -> &'static str {
    if cfg!(target_os = "windows") {
        "hello-e2e\r\n"
    } else {
        "hello-e2e\n"
    }
}

fn streaming_shell_program() -> &'static str {
    if cfg!(target_os = "windows") {
        "cmd"
    } else {
        "/bin/sh"
    }
}

fn streaming_shell_command() -> &'static str {
    if cfg!(target_os = "windows") {
        "<nul set /p =stream-one & powershell -NoLogo -NoProfile -Command \"Start-Sleep -Milliseconds 350\" & <nul set /p =stream-two"
    } else {
        "printf stream-one; sleep 0.35; printf stream-two"
    }
}

pub fn get_all_tests() -> Vec<TestCase> {
    vec![
        TestCase::standalone(
            "remote_shell_exec_policy_guard",
            "shell.exec runs only when local remote shell policy allows it",
            "remote_shell_exec",
            || async {
                let base = std::env::temp_dir().join(format!(
                    "bifrost-remote-shell-e2e-{}",
                    SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .map_err(|e| e.to_string())?
                        .as_nanos()
                ));
                fs::create_dir_all(&base).map_err(|e| e.to_string())?;
                bifrost_storage::set_data_dir(base.clone());

                let echo_exe = platform_echo_executable();
                let cwd = platform_cwd();

                let store = RemoteShellStore::new().map_err(|e| e.to_string())?;
                store
                    .save(&RemoteShellSet {
                        schema_version: 1,
                        version: 1,
                        policies: vec![RemoteShellPolicy {
                            id: "echo-argv".to_string(),
                            name: "echo-argv".to_string(),
                            description: None,
                            enabled: true,
                            profile_id: None,
                            metadata: serde_json::json!({
                                "exec_mode": "argv_exec",
                                "allowed_executables": [echo_exe],
                                "cwd_allowlist": [cwd],
                                "env_allowlist": ["LANG"],
                                "max_timeout_ms": 5000
                            }),
                        }],
                        profiles: vec![],
                    })
                    .map_err(|e| e.to_string())?;

                let executor = RemoteInvokeExecutor::new("127.0.0.1", 18080);
                let command = RemoteCommand {
                    kind: CommandKind::ShellExec,
                    policy_id: Some("echo-argv".to_string()),
                    exec_mode: Some(ShellExecMode::ArgvExec),
                    argv: Some(platform_echo_argv()),
                    cwd: Some(cwd.clone()),
                    ..Default::default()
                };

                let response = executor
                    .execute(&command)
                    .await
                    .map_err(|e| e.to_string())?;
                if response.exit_code != 0 {
                    return Err(format!(
                        "expected exit code 0, got {} (stderr: {:?})",
                        response.exit_code, response.stderr
                    ));
                }
                let expected_stdout = platform_expected_stdout();
                if response.stdout.as_deref() != Some(expected_stdout) {
                    return Err(format!(
                        "unexpected stdout: {:?}, expected {:?}",
                        response.stdout, expected_stdout
                    ));
                }

                let denied_exe = platform_denied_executable();
                let denied = RemoteCommand {
                    kind: CommandKind::ShellExec,
                    policy_id: Some("echo-argv".to_string()),
                    exec_mode: Some(ShellExecMode::ArgvExec),
                    argv: Some(vec![denied_exe]),
                    cwd: Some(cwd),
                    ..Default::default()
                };
                match executor.execute(&denied).await {
                    Ok(response) if response.exit_code == 0 => {
                        return Err("expected denied command to fail".to_string());
                    }
                    Ok(_) => {}
                    Err(error) => {
                        if !error
                            .to_string()
                            .contains("is not allowed by policy 'echo-argv'")
                        {
                            return Err(format!("unexpected denied error: {}", error));
                        }
                    }
                }

                let _ = fs::remove_dir_all(&base);
                Ok(())
            },
        ),
        TestCase::standalone(
            "remote_shell_exec_streams_stdout",
            "shell.exec streams stdout chunks before the process exits",
            "remote_shell_exec",
            || async {
                let base = std::env::temp_dir().join(format!(
                    "bifrost-remote-shell-stream-e2e-{}",
                    SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .map_err(|e| e.to_string())?
                        .as_nanos()
                ));
                fs::create_dir_all(&base).map_err(|e| e.to_string())?;
                bifrost_storage::set_data_dir(base.clone());

                let store = RemoteShellStore::new().map_err(|e| e.to_string())?;
                store
                    .save(&RemoteShellSet {
                        schema_version: 1,
                        version: 1,
                        policies: vec![RemoteShellPolicy {
                            id: "stream-shell".to_string(),
                            name: "stream-shell".to_string(),
                            description: None,
                            enabled: true,
                            profile_id: None,
                            metadata: serde_json::json!({
                                "exec_mode": "shell_text",
                                "allowed_shell_patterns": ["^(?s:.*)$"],
                                "shell": streaming_shell_program(),
                                "max_timeout_ms": 5000
                            }),
                        }],
                        profiles: vec![],
                    })
                    .map_err(|e| e.to_string())?;

                let executor = RemoteInvokeExecutor::new("127.0.0.1", 18080);
                let command = RemoteCommand {
                    kind: CommandKind::ShellExec,
                    policy_id: Some("stream-shell".to_string()),
                    exec_mode: Some(ShellExecMode::ShellText),
                    command_text: Some(streaming_shell_command().to_string()),
                    ..Default::default()
                };

                let chunks: Arc<Mutex<Vec<(String, u128)>>> = Arc::new(Mutex::new(Vec::new()));
                let sink = Arc::clone(&chunks);
                let started = Instant::now();
                let response = executor
                    .execute_with_stdout_sink(&command, |chunk| {
                        let sink = Arc::clone(&sink);
                        let elapsed_ms = started.elapsed().as_millis();
                        async move {
                            sink.lock()
                                .expect("stream chunks lock")
                                .push((chunk, elapsed_ms));
                            Ok(())
                        }
                    })
                    .await
                    .map_err(|e| e.to_string())?;

                if response.exit_code != 0 {
                    return Err(format!(
                        "expected exit code 0, got {} (stderr: {:?})",
                        response.exit_code, response.stderr
                    ));
                }
                if response.stdout.as_deref() != Some("stream-onestream-two") {
                    return Err(format!(
                        "unexpected stdout: {:?}, expected \"stream-onestream-two\"",
                        response.stdout
                    ));
                }

                let chunks = chunks.lock().map_err(|e| e.to_string())?;
                if chunks.len() < 2 {
                    return Err(format!(
                        "expected at least 2 streamed chunks, got {:?}",
                        *chunks
                    ));
                }
                if chunks[0].0 != "stream-one" || chunks[1].0 != "stream-two" {
                    return Err(format!("unexpected streamed chunks: {:?}", *chunks));
                }
                if chunks[0].1 >= 250 {
                    return Err(format!(
                        "first streamed chunk arrived too late: {}ms",
                        chunks[0].1
                    ));
                }
                if chunks[1].1 < 250 {
                    return Err(format!(
                        "second streamed chunk arrived too early: {}ms",
                        chunks[1].1
                    ));
                }

                let _ = fs::remove_dir_all(&base);
                Ok(())
            },
        ),
    ]
}
