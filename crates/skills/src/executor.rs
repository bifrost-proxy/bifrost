use crate::model::{Entrypoint, ShellKind, SkillRecord};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;
use std::process::Stdio;
use std::time::Instant;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::time::{timeout, Duration};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct SkillInvocation {
    pub input: serde_json::Value,
    pub timeout_ms: Option<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct SkillTestReport {
    pub stdout: String,
    pub stderr: String,
    pub tool_calls: Vec<serde_json::Value>,
    pub duration_ms: u64,
    pub exit_code: Option<i32>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ExecutionEvent {
    Log {
        level: String,
        message: String,
    },
    ToolCall {
        id: String,
        name: String,
        arguments: serde_json::Value,
    },
    Output {
        data: serde_json::Value,
    },
    Done,
}

#[derive(Clone, Debug)]
pub struct SkillExecutor {
    default_timeout_ms: u64,
}

impl Default for SkillExecutor {
    fn default() -> Self {
        Self {
            default_timeout_ms: 30_000,
        }
    }
}

impl SkillExecutor {
    pub fn new(default_timeout_ms: u64) -> Self {
        Self { default_timeout_ms }
    }

    pub async fn execute(
        &self,
        record: &SkillRecord,
        invocation: SkillInvocation,
    ) -> Result<SkillTestReport, String> {
        let started = Instant::now();
        match &record.manifest.entrypoint {
            Entrypoint::Inline { instructions_md } => Ok(SkillTestReport {
                stdout: serde_json::json!({
                    "instructions_md": instructions_md,
                    "input": invocation.input,
                })
                .to_string(),
                stderr: String::new(),
                tool_calls: Vec::new(),
                duration_ms: started.elapsed().as_millis() as u64,
                exit_code: Some(0),
            }),
            Entrypoint::Shell { script, shell } => {
                self.run_process(
                    record.path.as_path(),
                    shell_command(shell),
                    script,
                    &record.manifest.env,
                    invocation,
                )
                .await
            }
            Entrypoint::Python { script, python } => {
                self.run_process(
                    record.path.as_path(),
                    python.as_deref().unwrap_or("python3"),
                    script,
                    &record.manifest.env,
                    invocation,
                )
                .await
            }
            Entrypoint::Node { script } => {
                self.run_process(
                    record.path.as_path(),
                    "node",
                    script,
                    &record.manifest.env,
                    invocation,
                )
                .await
            }
        }
    }

    async fn run_process(
        &self,
        cwd: &Path,
        command: &str,
        script: &Path,
        manifest_env: &BTreeMap<String, String>,
        invocation: SkillInvocation,
    ) -> Result<SkillTestReport, String> {
        let started = Instant::now();
        let mut command_builder = Command::new(command);
        command_builder
            .arg(script)
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env_clear();
        apply_skill_env(&mut command_builder, manifest_env);
        let mut child = command_builder
            .spawn()
            .map_err(|error| format!("spawn skill entrypoint: {error}"))?;
        let mut stdin = child.stdin.take().ok_or("missing child stdin")?;
        let payload = serde_json::json!({
            "input": invocation.input,
            "tool_ack_id": null,
        });
        stdin
            .write_all(format!("{payload}\n").as_bytes())
            .await
            .map_err(|error| format!("write skill stdin: {error}"))?;
        drop(stdin);

        let stdout = child.stdout.take().ok_or("missing child stdout")?;
        let stderr = child.stderr.take().ok_or("missing child stderr")?;
        let wait = async {
            let mut stdout_reader = BufReader::new(stdout).lines();
            let mut stdout_text = String::new();
            let mut tool_calls = Vec::new();
            while let Some(line) = stdout_reader
                .next_line()
                .await
                .map_err(|error| format!("read stdout: {error}"))?
            {
                if stdout_text.len() < 4 * 1024 * 1024 {
                    stdout_text.push_str(&line);
                    stdout_text.push('\n');
                }
                if let Ok(ExecutionEvent::ToolCall {
                    id,
                    name,
                    arguments,
                }) = serde_json::from_str::<ExecutionEvent>(&line)
                {
                    tool_calls.push(serde_json::json!({
                        "id": id,
                        "name": name,
                        "arguments": arguments,
                    }));
                }
            }
            let mut stderr_text = String::new();
            let mut stderr_reader = BufReader::new(stderr).lines();
            while let Some(line) = stderr_reader
                .next_line()
                .await
                .map_err(|error| format!("read stderr: {error}"))?
            {
                if stderr_text.len() < 4 * 1024 * 1024 {
                    stderr_text.push_str(&line);
                    stderr_text.push('\n');
                }
            }
            let status = child
                .wait()
                .await
                .map_err(|error| format!("wait skill process: {error}"))?;
            Ok::<_, String>((stdout_text, stderr_text, tool_calls, status.code()))
        };
        let timeout_ms = invocation.timeout_ms.unwrap_or(self.default_timeout_ms);
        let (stdout, stderr, tool_calls, exit_code) =
            timeout(Duration::from_millis(timeout_ms), wait)
                .await
                .map_err(|_| "skill execution timed out".to_string())??;
        Ok(SkillTestReport {
            stdout,
            stderr,
            tool_calls,
            duration_ms: started.elapsed().as_millis() as u64,
            exit_code,
        })
    }
}

fn shell_command(shell: &ShellKind) -> &'static str {
    match shell {
        ShellKind::Bash => "bash",
        ShellKind::Sh => "sh",
        ShellKind::Zsh => "zsh",
        ShellKind::PowerShell => "powershell",
    }
}

const HOST_ENV_WHITELIST: &[&str] = &[
    "PATH",
    "HOME",
    "USER",
    "LOGNAME",
    "LANG",
    "LC_ALL",
    "LC_CTYPE",
    "TMPDIR",
    "TEMP",
    "TMP",
    "TERM",
    "SHELL",
    "SSL_CERT_FILE",
    "SSL_CERT_DIR",
    "CARGO_HOME",
    "RUSTUP_HOME",
];

fn apply_skill_env(command: &mut Command, manifest_env: &BTreeMap<String, String>) {
    for key in HOST_ENV_WHITELIST {
        if let Ok(value) = std::env::var(key) {
            command.env(key, value);
        }
    }
    for (key, value) in manifest_env {
        command.env(key, value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{SkillManifest, SkillScope};
    use std::path::PathBuf;

    // Serializes the exec-heavy tests that write a script file and then spawn
    // it, to avoid ETXTBSY ("Text file busy") races under parallel test runs.
    // Uses a tokio mutex so the guard can be safely held across the test's
    // await points (clippy::await_holding_lock).
    static HARNESS_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    #[tokio::test]
    async fn inline_executor_returns_prompt_payload() {
        let manifest = SkillManifest::minimal_inline("inline", "inline", SkillScope::Repo);
        let record = SkillRecord {
            name: manifest.name.clone(),
            version: manifest.version.clone(),
            description: manifest.description.clone(),
            scope: SkillScope::Repo,
            effective_scope: SkillScope::Repo,
            shadow_scopes: Vec::new(),
            enabled: true,
            path: ".".into(),
            skill_md_path: "SKILL.md".into(),
            checksum: String::new(),
            manifest,
        };
        let report = SkillExecutor::default()
            .execute(
                &record,
                SkillInvocation {
                    input: serde_json::json!({"city":"Paris"}),
                    timeout_ms: None,
                },
            )
            .await
            .unwrap();
        assert_eq!(report.exit_code, Some(0));
        assert!(report.stdout.contains("Paris"));
    }

    #[tokio::test]
    async fn process_executor_keeps_common_host_env() {
        let mut command = Command::new("sh");
        command
            .arg("-c")
            .arg("printf '%s\\n%s\\n' \"$HOME\" \"$PATH\"");
        command.env_clear();
        apply_skill_env(&mut command, &BTreeMap::new());

        let output = command.output().await.unwrap();
        assert!(output.status.success());
        let stdout = String::from_utf8(output.stdout).unwrap();
        let lines: Vec<_> = stdout.lines().collect();
        assert!(lines.first().is_some_and(|value| !value.is_empty()));
        assert!(lines.get(1).is_some_and(|value| !value.is_empty()));
    }

    // --- run_process integration via real shell scripts ---------------------

    fn shell_record(dir: &Path, script_name: &str, env: BTreeMap<String, String>) -> SkillRecord {
        let mut manifest = SkillManifest::minimal_inline("sh-skill", "sh", SkillScope::Repo);
        manifest.entrypoint = Entrypoint::Shell {
            script: PathBuf::from(script_name),
            shell: ShellKind::Sh,
        };
        manifest.env = env;
        SkillRecord {
            name: manifest.name.clone(),
            version: manifest.version.clone(),
            description: manifest.description.clone(),
            scope: SkillScope::Repo,
            effective_scope: SkillScope::Repo,
            shadow_scopes: Vec::new(),
            enabled: true,
            path: dir.to_path_buf(),
            skill_md_path: dir.join("SKILL.md"),
            checksum: String::new(),
            manifest,
        }
    }

    fn write_script(dir: &Path, name: &str, body: &str) {
        use std::io::Write;
        let mut f = std::fs::File::create(dir.join(name)).unwrap();
        f.write_all(body.as_bytes()).unwrap();
        f.sync_all().unwrap();
    }

    #[tokio::test]
    async fn shell_executor_captures_stdout_and_tool_calls() {
        let _guard = HARNESS_LOCK.lock().await;
        let dir = std::env::temp_dir().join(format!("skill-exec-{}", uuid_like()));
        std::fs::create_dir_all(&dir).unwrap();
        // The script reads the JSON payload on stdin, echoes a tool_call event
        // and a plain log line, and exits 0.
        write_script(
            &dir,
            "run.sh",
            "read line\n\
             echo '{\"type\":\"tool_call\",\"id\":\"t1\",\"name\":\"search\",\"arguments\":{\"q\":\"x\"}}'\n\
             echo 'plain line'\n",
        );
        let record = shell_record(&dir, "run.sh", BTreeMap::new());
        let report = SkillExecutor::default()
            .execute(
                &record,
                SkillInvocation {
                    input: serde_json::json!({"k":"v"}),
                    timeout_ms: Some(10_000),
                },
            )
            .await
            .unwrap();

        assert_eq!(report.exit_code, Some(0));
        assert!(report.stdout.contains("plain line"));
        assert_eq!(report.tool_calls.len(), 1);
        assert_eq!(report.tool_calls[0]["name"], "search");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn shell_executor_captures_stderr_and_exit_code() {
        let _guard = HARNESS_LOCK.lock().await;
        let dir = std::env::temp_dir().join(format!("skill-exec-{}", uuid_like()));
        std::fs::create_dir_all(&dir).unwrap();
        write_script(&dir, "fail.sh", "read line\necho 'boom' 1>&2\nexit 3\n");
        let record = shell_record(&dir, "fail.sh", BTreeMap::new());
        let report = SkillExecutor::default()
            .execute(
                &record,
                SkillInvocation {
                    input: serde_json::Value::Null,
                    timeout_ms: Some(10_000),
                },
            )
            .await
            .unwrap();

        assert_eq!(report.exit_code, Some(3));
        assert!(report.stderr.contains("boom"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn shell_executor_passes_manifest_env() {
        let _guard = HARNESS_LOCK.lock().await;
        let dir = std::env::temp_dir().join(format!("skill-exec-{}", uuid_like()));
        std::fs::create_dir_all(&dir).unwrap();
        write_script(&dir, "env.sh", "read line\necho \"$SKILL_GREETING\"\n");
        let mut env = BTreeMap::new();
        env.insert("SKILL_GREETING".to_string(), "hello-skill".to_string());
        let record = shell_record(&dir, "env.sh", env);
        let report = SkillExecutor::new(10_000)
            .execute(
                &record,
                SkillInvocation {
                    input: serde_json::Value::Null,
                    timeout_ms: None,
                },
            )
            .await
            .unwrap();

        assert_eq!(report.exit_code, Some(0));
        assert!(report.stdout.contains("hello-skill"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn shell_executor_times_out_on_slow_script() {
        let _guard = HARNESS_LOCK.lock().await;
        let dir = std::env::temp_dir().join(format!("skill-exec-{}", uuid_like()));
        std::fs::create_dir_all(&dir).unwrap();
        write_script(&dir, "slow.sh", "read line\nsleep 5\n");
        let record = shell_record(&dir, "slow.sh", BTreeMap::new());
        let err = SkillExecutor::default()
            .execute(
                &record,
                SkillInvocation {
                    input: serde_json::Value::Null,
                    timeout_ms: Some(100),
                },
            )
            .await
            .expect_err("should time out");
        assert!(err.contains("timed out"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn process_executor_errors_when_command_missing() {
        let _guard = HARNESS_LOCK.lock().await;
        let dir = std::env::temp_dir().join(format!("skill-exec-{}", uuid_like()));
        std::fs::create_dir_all(&dir).unwrap();
        write_script(&dir, "noop.sh", "read line\n");
        let mut record = shell_record(&dir, "noop.sh", BTreeMap::new());
        // Force a non-existent interpreter via the Node entrypoint pointing at
        // a binary that does not exist on PATH.
        record.manifest.entrypoint = Entrypoint::Python {
            script: PathBuf::from("noop.sh"),
            python: Some("definitely-not-a-real-interpreter-xyz".to_string()),
        };
        let err = SkillExecutor::default()
            .execute(
                &record,
                SkillInvocation {
                    input: serde_json::Value::Null,
                    timeout_ms: Some(5_000),
                },
            )
            .await
            .expect_err("missing interpreter should error");
        assert!(err.contains("spawn skill entrypoint"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn shell_command_maps_all_kinds() {
        assert_eq!(shell_command(&ShellKind::Bash), "bash");
        assert_eq!(shell_command(&ShellKind::Sh), "sh");
        assert_eq!(shell_command(&ShellKind::Zsh), "zsh");
        assert_eq!(shell_command(&ShellKind::PowerShell), "powershell");
    }

    // Lightweight unique suffix without pulling in the uuid crate here.
    fn uuid_like() -> String {
        use std::time::{SystemTime, UNIX_EPOCH};
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        format!("{nanos}-{:?}", std::thread::current().id()).replace(['(', ')', ' '], "")
    }
}
