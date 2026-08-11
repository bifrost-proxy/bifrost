use std::fmt;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use colored::Colorize;

use bifrost_core::BifrostError;

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
    Universal,
    ClaudeCode,
}

impl AiTool {
    fn all() -> Vec<AiTool> {
        vec![AiTool::Universal, AiTool::ClaudeCode]
    }

    fn default_global_dirs(&self) -> Vec<PathBuf> {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("~"));
        match self {
            AiTool::Universal => vec![home.join(".agents").join("skills").join("bifrost")],
            AiTool::ClaudeCode => {
                vec![home.join(".claude").join("skills").join("bifrost")]
            }
        }
    }

    fn project_local_dirs(&self, base: &Path) -> Vec<PathBuf> {
        match self {
            AiTool::Universal => vec![base.join(".agents").join("skills").join("bifrost")],
            AiTool::ClaudeCode => vec![base.join(".claude").join("skills").join("bifrost")],
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
            AiTool::Universal => write!(f, "Universal Agent Skills"),
            AiTool::ClaudeCode => write!(f, "Claude Code"),
        }
    }
}

fn parse_tool(s: &str) -> Result<Vec<AiTool>, BifrostError> {
    match s.to_lowercase().replace(' ', "-").as_str() {
        "all" => Ok(AiTool::all()),
        "universal" | "agent-skills" => Ok(vec![AiTool::Universal]),
        "claude-code" | "claude" => Ok(vec![AiTool::ClaudeCode]),
        _ => Err(BifrostError::Config(format!(
            "Unknown tool: '{}'. Available: universal, claude-code, all",
            s
        ))),
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

    // Even with per-socket timeouts, DNS/TLS can hang in some environments.
    // Guard the whole attempt and fall back to the copy compiled into the CLI.
    let hard_timeout = Duration::from_secs(45);
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let attempt = (|| -> Result<String, String> {
            let client = bifrost_core::github_blocking_reqwest_client_builder()
                .connect_timeout(Duration::from_secs(10))
                .timeout(Duration::from_secs(30))
                .build()
                .map_err(|e| format!("Failed to build GitHub HTTP client: {e}"))?;

            let response = client.get(source.raw_url).send().map_err(|e| {
                format!("Network error: {}", bifrost_core::format_reqwest_error(&e))
            })?;

            if !response.status().is_success() {
                return Err(format!(
                    "HTTP error: {} returned {}",
                    source.raw_url,
                    response.status()
                ));
            }

            response
                .text()
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
                "Failed to download skill from network; falling back to embedded copy."
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
            "Downloaded skill is empty — the remote file may be blank or corrupted. \
             Please verify the source URL and try again."
                .to_string(),
        ));
    }

    let normalized_body = body.replace("\r\n", "\n");
    if !normalized_body.starts_with("---\n") || normalized_body.matches("---").count() < 2 {
        println!(
            "  {} {}",
            "⚠".bright_yellow(),
            "Warning: Downloaded skill does not contain standard YAML frontmatter (---)."
                .bright_yellow()
        );
        println!(
            "    {}",
            "Claude Code and standard .agents/skills consumers require frontmatter with \
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
    let cwd_base = || std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    resolve_target_dirs_with_base(tool, custom_dir, cwd, cwd_base)
}

fn resolve_target_dirs_with_base(
    tool: &AiTool,
    custom_dir: &Option<PathBuf>,
    cwd: bool,
    cwd_base: impl FnOnce() -> PathBuf,
) -> Vec<PathBuf> {
    let env_dir = std::env::var_os("BIFROST_INSTALL_SKILL_DIR")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from);
    resolve_target_dirs_with_base_and_env(tool, custom_dir, cwd, cwd_base, env_dir)
}

fn resolve_target_dirs_with_base_and_env(
    tool: &AiTool,
    custom_dir: &Option<PathBuf>,
    cwd: bool,
    cwd_base: impl FnOnce() -> PathBuf,
    env_dir: Option<PathBuf>,
) -> Vec<PathBuf> {
    if let Some(d) = custom_dir {
        return vec![d.clone()];
    }
    if cwd {
        let base = cwd_base();
        return tool.project_local_dirs(&base);
    }
    if let Some(d) = env_dir {
        return vec![d];
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
    for dir in resolve_target_dirs(tool, custom_dir, cwd) {
        for asset in assets {
            install_to_dir(tool, asset, &dir)?;
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
    for tool in &tools {
        for target_dir in resolve_target_dirs(tool, &dir, cwd) {
            for source in SKILL_SOURCES {
                let target_file = skill_target_dir(&target_dir, *source).join(tool.target_filename());
                let exists = if target_file.exists() {
                    " (exists → overwrite)".bright_yellow().to_string()
                } else {
                    " (new)".bright_green().to_string()
                };
                println!(
                    "    {} {} / {} → {}{}",
                    "•".bright_cyan(),
                    tool,
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

    for tool in &tools {
        match install_to_tool(tool, &assets, &dir, cwd) {
            Ok(()) => success_count += 1,
            Err(e) => {
                println!(
                    "  {} {} — {}",
                    "✗".bright_red().bold(),
                    tool,
                    e.to_string().bright_red()
                );
                errors.push((tool.clone(), e.to_string()));
            }
        }
    }

    println!();
    println!("{}", separator.bright_cyan());

    if errors.is_empty() {
        println!(
            "{}",
            format!(
                "  ✓ Successfully installed to {} target{}!",
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
                "  ⚠ Installed to {}/{} targets ({} failed)",
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};
    use tempfile::TempDir;

    #[test]
    fn parse_tool_supports_only_universal_and_claude() {
        let all = parse_tool("all").unwrap();
        assert_eq!(all, vec![AiTool::Universal, AiTool::ClaudeCode]);

        assert_eq!(parse_tool("universal").unwrap(), vec![AiTool::Universal]);
        assert_eq!(parse_tool("agent-skills").unwrap(), vec![AiTool::Universal]);
        assert_eq!(parse_tool("claude-code").unwrap(), vec![AiTool::ClaudeCode]);
        assert_eq!(parse_tool("Claude Code").unwrap(), vec![AiTool::ClaudeCode]);

        for legacy in ["codex", "trae", "cursor", "github-copilot", "copilot"] {
            assert!(parse_tool(legacy).is_err(), "legacy target should be rejected: {legacy}");
        }
    }

    #[test]
    fn default_global_dirs_are_standard_agents_and_claude_only() {
        let universal = AiTool::Universal.default_global_dirs();
        assert_eq!(universal.len(), 1);
        assert!(universal[0].ends_with(Path::new(".agents/skills/bifrost")));

        let claude = AiTool::ClaudeCode.default_global_dirs();
        assert_eq!(claude.len(), 1);
        assert!(claude[0].ends_with(Path::new(".claude/skills/bifrost")));
    }

    #[test]
    fn resolve_target_dirs_prefers_custom_dir() {
        let custom = PathBuf::from("/tmp/custom-dir");
        let dirs = resolve_target_dirs(&AiTool::Universal, &Some(custom.clone()), false);
        assert_eq!(dirs, vec![custom]);
    }

    #[test]
    fn resolve_target_dirs_uses_cwd_project_dirs() {
        let temp_dir = TempDir::new().unwrap();
        let universal = resolve_target_dirs_with_base(&AiTool::Universal, &None, true, || {
            temp_dir.path().into()
        });
        assert_eq!(
            universal,
            vec![temp_dir
                .path()
                .join(".agents")
                .join("skills")
                .join("bifrost")]
        );

        let claude = resolve_target_dirs_with_base(&AiTool::ClaudeCode, &None, true, || {
            temp_dir.path().into()
        });
        assert_eq!(
            claude,
            vec![temp_dir
                .path()
                .join(".claude")
                .join("skills")
                .join("bifrost")]
        );
    }

    #[test]
    fn resolve_target_dirs_uses_env_dir_for_global_installs() {
        let env_dir = PathBuf::from("/tmp/env-skill-dir");
        let dirs = resolve_target_dirs_with_base_and_env(
            &AiTool::Universal,
            &None,
            false,
            || PathBuf::from("/tmp/project"),
            Some(env_dir.clone()),
        );
        assert_eq!(dirs, vec![env_dir]);
    }

    #[test]
    fn resolve_target_dirs_keeps_explicit_targets_ahead_of_env_dir() {
        let custom = PathBuf::from("/tmp/custom-dir");
        let env_dir = Some(PathBuf::from("/tmp/env-skill-dir"));
        let dirs = resolve_target_dirs_with_base_and_env(
            &AiTool::Universal,
            &Some(custom.clone()),
            false,
            || PathBuf::from("/tmp/project"),
            env_dir.clone(),
        );
        assert_eq!(dirs, vec![custom]);

        let project = PathBuf::from("/tmp/project");
        let dirs = resolve_target_dirs_with_base_and_env(
            &AiTool::Universal,
            &None,
            true,
            || project.clone(),
            env_dir,
        );
        assert_eq!(dirs, AiTool::Universal.project_local_dirs(&project));
    }

    #[test]
    fn skill_target_dir_for_primary_and_remote_sources() {
        let base = PathBuf::from("/home/user/.agents/skills/bifrost");
        let primary = skill_target_dir(&base, SKILL_SOURCES[0]);
        assert_eq!(primary, base);

        let expected_remote = base
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(SKILL_SOURCES[1].dir_name);
        let remote = skill_target_dir(&base, SKILL_SOURCES[1]);
        assert_eq!(remote, expected_remote);
    }

    #[test]
    fn install_to_dir_writes_skill_file() {
        let temp_dir = TempDir::new().unwrap();
        let tool = AiTool::Universal;
        let asset = SkillAsset {
            source: SKILL_SOURCES[0],
            content: "test-skill".to_string(),
        };
        let primary_target = temp_dir.path().join(".agents").join("skills").join("bifrost");

        install_to_dir(&tool, &asset, &primary_target).unwrap();

        let target_file = primary_target.join(tool.target_filename());
        let content = std::fs::read_to_string(&target_file).unwrap();
        assert_eq!(content, tool.wrap_content("test-skill"));
    }

    #[test]
    fn download_skill_source_uses_embedded_when_requested() {
        let old = std::env::var("BIFROST_INSTALL_SKILL_SOURCE").ok();
        std::env::set_var("BIFROST_INSTALL_SKILL_SOURCE", "embedded");

        let content = download_skill_source(SKILL_SOURCES[0]).unwrap();
        assert!(!content.is_empty());

        if let Some(old) = old {
            std::env::set_var("BIFROST_INSTALL_SKILL_SOURCE", old);
        } else {
            std::env::remove_var("BIFROST_INSTALL_SKILL_SOURCE");
        }
    }

    #[test]
    fn format_io_error_special_cases() {
        use std::io::{Error, ErrorKind};

        let path = Path::new("/tmp/test");
        let err = Error::new(ErrorKind::PermissionDenied, "denied");
        let msg = format_io_error(&err, path, "write file").to_string();
        assert!(msg.contains("Permission denied"));

        let err = Error::new(ErrorKind::NotFound, "missing");
        let msg = format_io_error(&err, path, "read file").to_string();
        assert!(msg.contains("Path not found"));

        let err = Error::new(ErrorKind::AlreadyExists, "exists");
        let msg = format_io_error(&err, path, "create directory").to_string();
        assert!(msg.contains("already exists"));

        let err = Error::other("disk full");
        let msg = format_io_error(&err, path, "write file").to_string();
        assert!(msg.contains("disk may be full"));
    }
}
