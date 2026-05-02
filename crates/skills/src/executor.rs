use crate::model::{Entrypoint, ShellKind, SkillRecord};
use serde::{Deserialize, Serialize};
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
                    invocation,
                )
                .await
            }
            Entrypoint::Python { script, python } => {
                self.run_process(
                    record.path.as_path(),
                    python.as_deref().unwrap_or("python3"),
                    script,
                    invocation,
                )
                .await
            }
            Entrypoint::Node { script } => {
                self.run_process(record.path.as_path(), "node", script, invocation)
                    .await
            }
        }
    }

    async fn run_process(
        &self,
        cwd: &Path,
        command: &str,
        script: &Path,
        invocation: SkillInvocation,
    ) -> Result<SkillTestReport, String> {
        let started = Instant::now();
        let mut child = Command::new(command)
            .arg(script)
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env_clear()
            .env("PATH", std::env::var("PATH").unwrap_or_default())
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{SkillManifest, SkillScope};

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
}
