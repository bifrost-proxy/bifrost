use std::fmt;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use colored::Colorize;

use bifrost_core::BifrostError;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

const SKILL_RAW_URL: &str = "https://raw.githubusercontent.com/bifrost-proxy/bifrost/main/SKILL.md";
const REMOTE_SKILL_RAW_URL: &str =
    "https://raw.githubusercontent.com/bifrost-proxy/bifrost/main/skill_remote.md";

const EMBEDDED_SKILL_MD: &str =
    include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../SKILL.md"));
const EMBEDDED_REMOTE_SKILL_MD: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../skill_remote.md"
));

#[derive(Debug, Clone, Copy)]
struct SkillSource {
    dir_name: &'static str,
    display_name: &'static str,
    raw_url: &'static str,
    embedded_content: &'static str,
}

#[derive(Debug, Clone)]
struct SkillAsset {
    source: SkillSource,
    content: String,
}

const SKILL_SOURCES: &[SkillSource] = &[
    SkillSource {
        dir_name: "bifrost",
        display_name: "SKILL.md",
        raw_url: SKILL_RAW_URL,
        embedded_content: EMBEDDED_SKILL_MD,
    },
    SkillSource {
        dir_name: "bifrost-remote",
        display_name: "skill_remote.md",
        raw_url: REMOTE_SKILL_RAW_URL,
        embedded_content: EMBEDDED_REMOTE_SKILL_MD,
    },
];

#[derive(Debug, Clone, PartialEq)]
pub enum AiTool {
    ClaudeCode,
    Codex,
    Trae,
    Cursor,
    GitHubCopilot,
    Universal,
}

impl AiTool {
    fn all() -> Vec<AiTool> {
        vec![
            AiTool::ClaudeCode,
            AiTool::Codex,
            AiTool::Trae,
            AiTool::Cursor,
            AiTool::GitHubCopilot,
        ]
    }

    fn default_global_dirs(&self) -> Vec<PathBuf> {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("~"));
        match self {
            AiTool::ClaudeCode => {
                vec![home.join(".claude").join("skills").join("bifrost")]
            }
            AiTool::Codex => vec![
                home.join(".codex").join("skills").join("bifrost"),
                home.join(".agents").join("skills").join("bifrost"),
            ],
            AiTool::Trae => vec![
                home.join(".trae").join("skills").join("bifrost"),
                home.join(".trae-cn").join("skills").join("bifrost"),
            ],
            AiTool::Cursor => vec![home.join(".cursor").join("skills").join("bifrost")],
            AiTool::GitHubCopilot => {
                vec![home.join(".copilot").join("skills").join("bifrost")]
            }
            AiTool::Universal => vec![home.join(".agents").join("skills").join("bifrost")],
        }
    }

    fn project_local_dir(&self, base: &Path) -> PathBuf {
        match self {
            AiTool::ClaudeCode => base.join(".claude").join("skills").join("bifrost"),
            AiTool::Codex => base.join(".codex").join("skills").join("bifrost"),
            AiTool::Trae => base.join(".trae").join("skills").join("bifrost"),
            AiTool::Cursor => base.join(".cursor").join("skills").join("bifrost"),
            AiTool::GitHubCopilot => base.join(".github").join("skills").join("bifrost"),
            AiTool::Universal => base.join(".agents").join("skills").join("bifrost"),
        }
    }

    fn project_local_dirs(&self, base: &Path) -> Vec<PathBuf> {
        match self {
            AiTool::Codex => vec![
                self.project_local_dir(base),
                base.join(".agents").join("skills").join("bifrost"),
            ],
            _ => vec![self.project_local_dir(base)],
        }
    }

    fn target_filename(&self) -> &str {
        "SKILL.md"
    }

    fn wrap_content(&self, raw_content: &str) -> String {
        raw_content.to_string()
    }
}

impl fmt::Display for AiTool {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AiTool::ClaudeCode => write!(f, "Claude Code"),
            AiTool::Codex => write!(f, "Codex"),
            AiTool::Trae => write!(f, "Trae"),
            AiTool::Cursor => write!(f, "Cursor"),
            AiTool::GitHubCopilot => write!(f, "GitHub Copilot"),
            AiTool::Universal => write!(f, "Universal Agent Skills"),
        }
    }
}

fn parse_tool(s: &str) -> Result<Vec<AiTool>, BifrostError> {
    match s.to_lowercase().replace(' ', "-").as_str() {
        "all" => Ok(AiTool::all()),
        "claude-code" | "claude" => Ok(vec![AiTool::ClaudeCode]),
        "codex" | "openai-codex" => Ok(vec![AiTool::Codex]),
        "trae" => Ok(vec![AiTool::Trae]),
        "cursor" => Ok(vec![AiTool::Cursor]),
        "github-copilot" | "copilot" => Ok(vec![AiTool::GitHubCopilot]),
        "universal" | "agent-skills" => Ok(vec![AiTool::Universal]),
        _ => Err(BifrostError::Config(format!(
            "Unknown tool: '{}'. Available: claude-code, codex, trae, cursor, github-copilot, universal, all",
            s
        ))),
    }
}

fn format_network_error(err: &ureq::Error) -> String {
    match err {
        ureq::Error::Status(code, resp) => {
            let url = resp.get_url();
            match code {
                404 => format!(
                    "HTTP 404 Not Found: the remote file was not found at {url}. \
                     The URL may have changed or the file may have been removed."
                ),
                403 => format!(
                    "HTTP 403 Forbidden: access denied to {url}. \
                     Check if the repository is public or if a token is required."
                ),
                429 => "HTTP 429 Too Many Requests: rate limited by the server. \
                     Please wait a moment and try again."
                    .to_string(),
                500..=599 => format!(
                    "HTTP {code} Server Error: the remote server returned an error. \
                     This is likely a temporary issue — please retry later."
                ),
                _ => format!("HTTP {code}: unexpected status code from {url}."),
            }
        }
        ureq::Error::Transport(transport) => {
            let kind = transport.kind();
            let detail = transport
                .message()
                .map(|m| m.to_string())
                .unwrap_or_default();
            match kind {
                ureq::ErrorKind::Dns => format!(
                    "DNS resolution failed: could not resolve the hostname. \
                     Check your internet connection and DNS settings. ({detail})"
                ),
                ureq::ErrorKind::ConnectionFailed => format!(
                    "Connection failed: could not connect to the remote server. \
                     The server may be down or a firewall may be blocking the connection. ({detail})"
                ),
                ureq::ErrorKind::Io => {
                    let lower = detail.to_lowercase();
                    if lower.contains("timed out") || lower.contains("timeout") {
                        format!(
                            "Connection timed out: the server did not respond in time. \
                             Check your network or try again later. ({detail})"
                        )
                    } else if lower.contains("connection refused") {
                        format!(
                            "Connection refused: the server actively refused the connection. ({detail})"
                        )
                    } else if lower.contains("reset") {
                        format!(
                            "Connection reset: the connection was unexpectedly closed. ({detail})"
                        )
                    } else {
                        format!("Network I/O error: {detail}")
                    }
                }
                ureq::ErrorKind::TooManyRedirects => "Too many redirects: the server redirected too many times. \
                     The URL may be misconfigured."
                    .to_string(),
                ureq::ErrorKind::BadStatus => format!(
                    "Bad status line: received a malformed HTTP response. ({detail})"
                ),
                ureq::ErrorKind::BadHeader => format!(
                    "Bad header: received a malformed HTTP header. ({detail})"
                ),
                _ => format!("Transport error ({}): {detail}", kind),
            }
        }
    }
}

fn format_io_error(err: &io::Error, path: &Path, operation: &str) -> BifrostError {
    let path_display = path.display();
    match err.kind() {
        io::ErrorKind::PermissionDenied => BifrostError::Io(io::Error::new(
            err.kind(),
            format!(
                "Permission denied: cannot {operation} '{path_display}'. \
                 Try running with sudo or choose a different directory with --dir <path>"
            ),
        )),
        io::ErrorKind::NotFound => BifrostError::Io(io::Error::new(
            err.kind(),
            format!(
                "Path not found: '{path_display}' does not exist and cannot be created. \
                 Verify the path is correct or use --dir <path> to specify a different location."
            ),
        )),
        io::ErrorKind::AlreadyExists => BifrostError::Io(io::Error::new(
            err.kind(),
            format!("Path conflict: '{path_display}' already exists as a different type. ({err})"),
        )),
        _ => {
            let raw = err.raw_os_error();
            let os_hint = raw
                .map(|code| format!(" (OS error {code})"))
                .unwrap_or_default();
            let lower = err.to_string().to_lowercase();
            let hint = if lower.contains("no space") || lower.contains("disk full") {
                " Hint: the disk may be full — free up space and retry."
            } else if lower.contains("name too long") || lower.contains("file name too long") {
                " Hint: the file path is too long — use --dir <path> with a shorter path."
            } else if lower.contains("read-only") {
                " Hint: the filesystem is read-only — choose a writable location with --dir <path>."
            } else {
                ""
            };
            BifrostError::Io(io::Error::new(
                err.kind(),
                format!("Failed to {operation} '{path_display}': {err}{os_hint}.{hint}"),
            ))
        }
    }
}

fn download_skill_source(source: SkillSource) -> Result<String, BifrostError> {
    if std::env::var("BIFROST_INSTALL_SKILL_SOURCE")
        .ok()
        .map(|v| v.to_lowercase())
        .as_deref()
        == Some("embedded")
    {
        println!(
            "{} {}",
            format!("📦 Using embedded {}:", source.display_name).bright_cyan(),
            "(compiled in)".dimmed()
        );
        return Ok(source.embedded_content.to_string());
    }

    println!(
        "{} {}",
        format!("⬇ Downloading latest {} from:", source.display_name).bright_cyan(),
        source.raw_url.dimmed()
    );

    // NOTE: Even with per-socket timeouts, some environments can still hang during TLS/DNS.
    // Guard the whole network attempt with a hard deadline, and fall back to embedded copy.
    let hard_timeout = Duration::from_secs(45);
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let attempt = (|| -> Result<String, String> {
            let agent = ureq::AgentBuilder::new()
                .timeout_connect(Duration::from_secs(10))
                .timeout_read(Duration::from_secs(30))
                .timeout_write(Duration::from_secs(30))
                .build();

            let response = agent
                .get(source.raw_url)
                .call()
                .map_err(|e| format_network_error(&e))?;

            response
                .into_string()
                .map_err(|e| format!("Failed to read response body: {e}"))
        })();
        let _ = tx.send(attempt);
    });

    let body = match rx.recv_timeout(hard_timeout) {
        Ok(Ok(body)) => body,
        Ok(Err(err_msg)) => {
            println!(
                "  {} {}",
                "⚠".bright_yellow(),
                "Failed to download SKILL.md from network; falling back to embedded copy."
                    .bright_yellow()
            );
            println!("    {}", err_msg.dimmed());
            return Ok(source.embedded_content.to_string());
        }
        Err(mpsc::RecvTimeoutError::Timeout) => {
            println!(
                "  {} {}",
                "⚠".bright_yellow(),
                format!(
                    "Download timed out after {}s; falling back to embedded copy.",
                    hard_timeout.as_secs()
                )
                .bright_yellow()
            );
            return Ok(source.embedded_content.to_string());
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            println!(
                "  {} {}",
                "⚠".bright_yellow(),
                "Download worker disconnected; falling back to embedded copy.".bright_yellow()
            );
            return Ok(source.embedded_content.to_string());
        }
    };

    if body.trim().is_empty() {
        return Err(BifrostError::Parse(
            "Downloaded SKILL.md is empty — the remote file may be blank or corrupted. \
             Please verify the source URL and try again."
                .to_string(),
        ));
    }

    let normalized_body = body.replace("\r\n", "\n");
    if !normalized_body.starts_with("---\n") || normalized_body.matches("---").count() < 2 {
        println!(
            "  {} {}",
            "⚠".bright_yellow(),
            "Warning: Downloaded SKILL.md does not contain standard YAML frontmatter (---)."
                .bright_yellow()
        );
        println!(
            "    {}",
            "Major AI coding tools and Agent Skills runtimes (Claude Code, Codex, Trae, Cursor, \
             GitHub Copilot, and standard .agents/skills consumers) require frontmatter with \
             'name' and 'description' fields for skill auto-discovery."
                .dimmed()
        );
    }

    println!(
        "{}",
        format!("✓ Downloaded {} bytes", body.len()).bright_green()
    );

    Ok(body)
}

fn download_skill_bundle() -> Result<Vec<SkillAsset>, BifrostError> {
    SKILL_SOURCES
        .iter()
        .copied()
        .map(|source| download_skill_source(source).map(|content| SkillAsset { source, content }))
        .collect()
}

fn prompt_confirm(message: &str) -> bool {
    print!("{} [y/N]: ", message);
    io::stdout().flush().ok();
    let mut input = String::new();
    if io::stdin().read_line(&mut input).is_err() {
        return false;
    }
    matches!(input.trim().to_lowercase().as_str(), "y" | "yes")
}

fn resolve_target_dirs(tool: &AiTool, custom_dir: &Option<PathBuf>, cwd: bool) -> Vec<PathBuf> {
    if let Some(d) = custom_dir {
        return vec![d.clone()];
    }
    if cwd {
        let base = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        return tool.project_local_dirs(&base);
    }
    tool.default_global_dirs()
}

fn skill_target_dir(primary_target_dir: &Path, source: SkillSource) -> PathBuf {
    if source.dir_name == "bifrost" {
        return primary_target_dir.to_path_buf();
    }

    primary_target_dir
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(source.dir_name)
}

fn install_to_dir(
    tool: &AiTool,
    asset: &SkillAsset,
    primary_target_dir: &Path,
) -> Result<(), BifrostError> {
    let target_dir = skill_target_dir(primary_target_dir, asset.source);
    let target_file = target_dir.join(tool.target_filename());

    println!();
    println!(
        "{} {}",
        format!("📦 Installing {} to {}:", asset.source.dir_name, tool)
            .bright_cyan()
            .bold(),
        target_file.display()
    );

    if target_file.exists() {
        println!(
            "  {} {}",
            "⚠ Overwriting existing file with latest version from remote:".bright_yellow(),
            target_file.display()
        );
    }

    fs::create_dir_all(&target_dir)
        .map_err(|e| format_io_error(&e, &target_dir, "create directory"))?;

    let final_content = tool.wrap_content(&asset.content);

    fs::write(&target_file, &final_content)
        .map_err(|e| format_io_error(&e, &target_file, "write file"))?;

    println!(
        "  {} {} ({})",
        "✓".bright_green().bold(),
        target_file.display(),
        format!("{} bytes", final_content.len()).dimmed()
    );

    Ok(())
}

fn install_to_tool(
    tool: &AiTool,
    assets: &[SkillAsset],
    custom_dir: &Option<PathBuf>,
    cwd: bool,
) -> Result<(), BifrostError> {
    let dirs = resolve_target_dirs(tool, custom_dir, cwd);
    for d in &dirs {
        for asset in assets {
            install_to_dir(tool, asset, d)?;
        }
    }
    Ok(())
}

pub fn handle_install_skill(
    tool: Option<String>,
    dir: Option<PathBuf>,
    cwd: bool,
    yes: bool,
) -> Result<(), BifrostError> {
    if dir.is_some() && cwd {
        return Err(BifrostError::Config(
            "--dir and --cwd are mutually exclusive. Use --dir for a custom path, or --cwd for the current project directory.".to_string(),
        ));
    }

    let tools = match &tool {
        Some(t) => parse_tool(t)?,
        None => AiTool::all(),
    };

    let separator = "─".repeat(64);
    println!();
    println!("{}", separator.bright_cyan());
    println!("{}", "  🔧 Bifrost SKILL.md Installer".bright_cyan().bold());
    println!("{}", separator.bright_cyan());
    println!();

    println!(
        "  {} {}",
        "Source:".dimmed(),
        "GitHub main branch (latest)".bright_white()
    );
    println!(
        "  {} {}",
        "Target tools:".dimmed(),
        tools
            .iter()
            .map(|t| format!("{}", t))
            .collect::<Vec<_>>()
            .join(", ")
            .bright_white()
    );

    let mode_label = if dir.is_some() {
        "custom directory"
    } else if cwd {
        "project-local (current directory)"
    } else {
        "global"
    };
    println!(
        "  {} {}",
        "Install mode:".dimmed(),
        mode_label.bright_white()
    );

    if let Some(ref d) = dir {
        println!(
            "  {} {}",
            "Custom directory:".dimmed(),
            d.display().to_string().bright_white()
        );
    }

    println!();

    println!("  Target paths:");
    for t in &tools {
        let target_dirs = resolve_target_dirs(t, &dir, cwd);
        for target_dir in &target_dirs {
            for source in SKILL_SOURCES {
                let target_file = skill_target_dir(target_dir, *source).join(t.target_filename());
                let exists = if target_file.exists() {
                    " (exists → overwrite)".bright_yellow().to_string()
                } else {
                    " (new)".bright_green().to_string()
                };
                println!(
                    "    {} {} / {} → {}{}",
                    "•".bright_cyan(),
                    t,
                    source.dir_name,
                    target_file.display(),
                    exists
                );
            }
        }
    }

    println!();

    if !yes && !prompt_confirm("Proceed with installation?") {
        println!("{}", "Installation cancelled.".dimmed());
        return Ok(());
    }

    let assets = download_skill_bundle()?;

    let mut success_count = 0;
    let mut errors: Vec<(AiTool, String)> = Vec::new();

    for t in &tools {
        match install_to_tool(t, &assets, &dir, cwd) {
            Ok(()) => success_count += 1,
            Err(e) => {
                println!(
                    "  {} {} — {}",
                    "✗".bright_red().bold(),
                    t,
                    e.to_string().bright_red()
                );
                errors.push((t.clone(), e.to_string()));
            }
        }
    }

    println!();
    println!("{}", separator.bright_cyan());

    if errors.is_empty() {
        println!(
            "{}",
            format!(
                "  ✓ Successfully installed to {} tool{}!",
                success_count,
                if success_count > 1 { "s" } else { "" }
            )
            .bright_green()
            .bold()
        );
    } else {
        println!(
            "{}",
            format!(
                "  ⚠ Installed to {}/{} tools ({} failed)",
                success_count,
                tools.len(),
                errors.len()
            )
            .bright_yellow()
            .bold()
        );
        for (tool, err) in &errors {
            println!("    {} {}: {}", "✗".bright_red(), tool, err);
        }
    }

    println!("{}", separator.bright_cyan());
    println!();

    Ok(())
}
