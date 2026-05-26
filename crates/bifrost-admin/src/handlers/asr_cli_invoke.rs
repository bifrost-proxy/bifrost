//! Fork the native `asr` CLI binary to transcribe a single WAV file.
//!
//! This module is shared between the WebUI scheduled-task pipeline (`asr_jobs.rs`)
//! and the CLI `stream-file` command (`bifrost-cli/src/commands/asr.rs`).
//!
//! ## Why fork-per-chunk (Plan B)?
//!
//! The `asr-server` HTTP daemon (Plan A) accumulates Metal/MLX GPU state that
//! degrades inference speed after the first chunk: RTF rises from ~0.28 to ~0.59
//! over a batch of files. Forking a fresh `asr` process per chunk costs ~1.5 s
//! of model-load overhead but keeps RTF stable at ~0.25–0.28, yielding 2× faster
//! overall throughput for batch workloads.

use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

#[cfg(unix)]
use std::os::unix::process::CommandExt;

use crate::handlers::asr_streaming::WholeFileTranscription;

pub const ASR_MEMORY_LIMIT_ERROR_MARKER: &str = "asr cli exceeded memory footprint limit";
pub const ASR_ABORTED_ERROR_MARKER: &str = "asr cli aborted by task control";
pub const ASR_TIMEOUT_ERROR_MARKER: &str = "asr cli exceeded wall-clock timeout";

const ASR_MAX_FOOTPRINT_MB_ENV: &str = "BIFROST_ASR_MAX_FOOTPRINT_MB";
const ASR_UNSAFE_DISABLE_FOOTPRINT_GUARD_ENV: &str = "BIFROST_ASR_UNSAFE_DISABLE_FOOTPRINT_GUARD";
const ASR_PHYSICAL_SAMPLE_INTERVAL_SECS_ENV: &str = "BIFROST_ASR_PHYSICAL_SAMPLE_INTERVAL_SECS";
const ASR_CHUNK_TIMEOUT_SECS_ENV: &str = "BIFROST_ASR_CHUNK_TIMEOUT_SECS";
const ASR_MIN_CHUNK_TIMEOUT_SECS_ENV: &str = "BIFROST_ASR_MIN_CHUNK_TIMEOUT_SECS";
const ASR_TIMEOUT_MULTIPLIER_ENV: &str = "BIFROST_ASR_TIMEOUT_MULTIPLIER";
const ASR_DEFAULT_MODEL_LIMIT_MB: u64 = 12 * 1024;
const ASR_MODEL_LIMIT_06B_MB: u64 = 8 * 1024;
const ASR_MODEL_LIMIT_17B_MB: u64 = 18 * 1024;
const ASR_DEFAULT_CHUNK_TIMEOUT_SECS: u64 = 120;
const ASR_DEFAULT_MIN_CHUNK_TIMEOUT_SECS: u64 = 45;
const ASR_DEFAULT_TIMEOUT_MULTIPLIER: u64 = 3;
const ASR_MAX_TIMEOUT_MULTIPLIER: u64 = 12;
const ASR_HOST_MEMORY_SAFETY_PERCENT: u64 = 90;
const ASR_MIN_HOST_SAFETY_MB: u64 = 4 * 1024;
const ASR_FOOTPRINT_POLL_INTERVAL: Duration = Duration::from_millis(500);
const ASR_DEFAULT_PHYSICAL_SAMPLE_INTERVAL_SECS: u64 = 5;
const ASR_MIN_PHYSICAL_SAMPLE_INTERVAL_SECS: u64 = 2;
const ASR_MAX_PHYSICAL_SAMPLE_INTERVAL_SECS: u64 = 60;
const ASR_MAX_FOOTPRINT_SAMPLE_FAILURES: u32 = 3;

/// Fork the native `asr` CLI to transcribe a single WAV file (or chunk).
///
/// The CLI is expected to print `<asr_text>...</asr_text>` to stdout.
/// Returns a [`WholeFileTranscription`] with `text` populated and `segments`
/// empty (the native CLI only outputs plain text, no per-word timestamps).
///
/// # Blocking
///
/// This function blocks the calling thread (spawns a child process and waits
/// for it to finish). Callers should wrap it in `tokio::task::spawn_blocking`.
pub fn run_asr_cli(
    asr_bin: &Path,
    model_path: &Path,
    audio: &Path,
    language: &str,
) -> Result<WholeFileTranscription, String> {
    run_asr_cli_inner(
        asr_bin,
        model_path,
        audio,
        language,
        default_footprint_limit_bytes(model_path),
        None,
        None,
    )
}

/// Fork the native `asr` CLI with memory guard, wall-clock timeout, and optional abort check.
///
/// Apple Silicon Metal allocations can show up in `vmmap` physical footprint
/// while `ps` RSS remains modest. Some Qwen3-ASR-1.7B chunks can climb past
/// 20 GiB before failing; this guard kills that child early so callers can
/// bisect the chunk instead of repeating the same high-memory failure.
/// `abort_check` is used by ASR directory tasks for force-pause. It is polled
/// in the same loop as the footprint guard so a running Metal inference can be
/// killed promptly instead of waiting for the current chunk boundary.
pub fn run_asr_cli_with_footprint_guard_timeout_and_abort(
    asr_bin: &Path,
    model_path: &Path,
    audio: &Path,
    language: &str,
    max_runtime: Option<Duration>,
    abort_check: Option<Box<dyn Fn() -> bool + Send>>,
) -> Result<WholeFileTranscription, String> {
    run_asr_cli_inner(
        asr_bin,
        model_path,
        audio,
        language,
        default_footprint_limit_bytes(model_path),
        abort_check,
        max_runtime,
    )
}

fn run_asr_cli_inner(
    asr_bin: &Path,
    model_path: &Path,
    audio: &Path,
    language: &str,
    footprint_limit_bytes: Option<u64>,
    abort_check: Option<Box<dyn Fn() -> bool + Send>>,
    max_runtime: Option<Duration>,
) -> Result<WholeFileTranscription, String> {
    tracing::debug!(
        asr_bin = %asr_bin.display(),
        model = %model_path.display(),
        audio = %audio.display(),
        language,
        "forking asr CLI for inference"
    );
    let started = Instant::now();

    let mut command = Command::new(asr_bin);
    command
        .arg(model_path)
        .arg(audio)
        .arg(language)
        .env("RUST_LOG", "error")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    configure_process_group(&mut command);
    let mut child = command.spawn().map_err(|e| format!("spawn asr cli: {e}"))?;
    let child_pid = child.id();
    tracing::info!(
        pid = child_pid,
        model = %model_path.display(),
        audio = %audio.display(),
        "ASR CLI child started"
    );

    if footprint_limit_bytes.is_some() || abort_check.is_some() || max_runtime.is_some() {
        let pid = child.id();
        let mut peak = 0u64;
        let mut footprint_sample_failures = 0u32;
        let physical_sample_interval = physical_footprint_sample_interval();
        // Give short 30s chunks a chance to finish before the first heavy
        // `vmmap -summary` sample. Immediate sampling is observably expensive
        // on macOS and can double the latency of normal chunks.
        let mut last_physical_sample = Some(Instant::now());
        loop {
            match child.try_wait() {
                Ok(Some(_status)) => break,
                Ok(None) => {}
                Err(error) => return Err(format!("wait asr cli: {error}")),
            }

            if let Some(max_runtime) = max_runtime {
                if started.elapsed() >= max_runtime {
                    terminate_child_process_group(pid, &mut child);
                    let output = child
                        .wait_with_output()
                        .map_err(|e| format!("wait timed out asr cli: {e}"))?;
                    return Err(format!(
                        "{ASR_TIMEOUT_ERROR_MARKER}: pid={pid} timeout_secs={} peak_mb={} stderr={}",
                        max_runtime.as_secs(),
                        peak / 1024 / 1024,
                        String::from_utf8_lossy(&output.stderr).trim()
                    ));
                }
            }

            if abort_check.as_ref().is_some_and(|check| check()) {
                terminate_child_process_group(pid, &mut child);
                let output = child
                    .wait_with_output()
                    .map_err(|e| format!("wait aborted asr cli: {e}"))?;
                return Err(format!(
                    "{ASR_ABORTED_ERROR_MARKER}: pid={pid} peak_mb={} stderr={}",
                    peak / 1024 / 1024,
                    String::from_utf8_lossy(&output.stderr).trim()
                ));
            }

            if let Some(limit) = footprint_limit_bytes {
                let now = Instant::now();
                let should_sample_physical = last_physical_sample
                    .map(|last| now.duration_since(last) >= physical_sample_interval)
                    .unwrap_or(true);
                if !should_sample_physical {
                    thread::sleep(ASR_FOOTPRINT_POLL_INTERVAL);
                    continue;
                }
                last_physical_sample = Some(now);
                match read_process_footprint_bytes(pid) {
                    Ok((footprint, reliable)) => {
                        if reliable {
                            footprint_sample_failures = 0;
                        } else {
                            footprint_sample_failures += 1;
                            tracing::warn!(
                                pid,
                                sample_failures = footprint_sample_failures,
                                rss_mb = footprint / 1024 / 1024,
                                "ASR physical footprint sampling fell back to RSS"
                            );
                        }
                        peak = peak.max(footprint);
                        if footprint > limit {
                            terminate_child_process_group(pid, &mut child);
                            let output = child
                                .wait_with_output()
                                .map_err(|e| format!("wait killed asr cli: {e}"))?;
                            return Err(format!(
                                "{ASR_MEMORY_LIMIT_ERROR_MARKER}: pid={pid} footprint_mb={} limit_mb={} peak_mb={} stderr={}",
                                footprint / 1024 / 1024,
                                limit / 1024 / 1024,
                                peak / 1024 / 1024,
                                String::from_utf8_lossy(&output.stderr).trim()
                            ));
                        }
                        if !reliable
                            && footprint_sample_failures >= ASR_MAX_FOOTPRINT_SAMPLE_FAILURES
                        {
                            terminate_child_process_group(pid, &mut child);
                            let output = child.wait_with_output().map_err(|e| {
                                format!("wait killed asr cli after fallback sampling: {e}")
                            })?;
                            return Err(format!(
                                "{ASR_MEMORY_LIMIT_ERROR_MARKER}: pid={pid} physical footprint unavailable {footprint_sample_failures} times; limit_mb={} peak_mb={} stderr={}",
                                limit / 1024 / 1024,
                                peak / 1024 / 1024,
                                String::from_utf8_lossy(&output.stderr).trim()
                            ));
                        }
                    }
                    Err(error) => {
                        footprint_sample_failures += 1;
                        tracing::warn!(
                            pid,
                            sample_failures = footprint_sample_failures,
                            %error,
                            "ASR footprint sampling failed"
                        );
                        if footprint_sample_failures >= ASR_MAX_FOOTPRINT_SAMPLE_FAILURES {
                            terminate_child_process_group(pid, &mut child);
                            let output = child.wait_with_output().map_err(|e| {
                                format!("wait killed asr cli after sampling failure: {e}")
                            })?;
                            return Err(format!(
                                "{ASR_MEMORY_LIMIT_ERROR_MARKER}: pid={pid} footprint sampling failed {footprint_sample_failures} times; limit_mb={} peak_mb={} stderr={}",
                                limit / 1024 / 1024,
                                peak / 1024 / 1024,
                                String::from_utf8_lossy(&output.stderr).trim()
                            ));
                        }
                    }
                }
            }

            thread::sleep(ASR_FOOTPRINT_POLL_INTERVAL);
        }
    }

    let output = child
        .wait_with_output()
        .map_err(|e| format!("wait asr cli: {e}"))?;

    if !output.status.success() {
        return Err(format!(
            "asr cli exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let text = parse_asr_cli_text(&stdout);
    tracing::info!(
        pid = child_pid,
        elapsed_ms = started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
        text_chars = text.chars().count(),
        "ASR CLI child completed"
    );
    Ok(WholeFileTranscription {
        text,
        segments: Vec::new(),
    })
}

pub(crate) fn default_footprint_limit_bytes(model_path: &Path) -> Option<u64> {
    if std::env::consts::OS != "macos" {
        return None;
    }
    if env_flag_enabled(ASR_UNSAFE_DISABLE_FOOTPRINT_GUARD_ENV) {
        tracing::warn!(
            env = ASR_UNSAFE_DISABLE_FOOTPRINT_GUARD_ENV,
            "ASR footprint guard disabled by unsafe override"
        );
        return None;
    }
    let safe_limit_mb = default_footprint_limit_mb(model_path, host_memory_bytes());
    let configured_limit_mb = std::env::var(ASR_MAX_FOOTPRINT_MB_ENV)
        .ok()
        .and_then(|value| value.parse::<u64>().ok());
    let limit_mb = effective_footprint_limit_mb(safe_limit_mb, configured_limit_mb);
    (limit_mb > 0).then_some(limit_mb.saturating_mul(1024 * 1024))
}

pub(crate) fn asr_chunk_timeout(chunk_duration_secs: u64) -> Duration {
    if let Ok(value) = std::env::var(ASR_CHUNK_TIMEOUT_SECS_ENV) {
        if let Ok(secs) = value.parse::<u64>() {
            if secs > 0 {
                return Duration::from_secs(secs);
            }
        }
    }
    let duration_secs = chunk_duration_secs.max(1);
    let min_timeout_secs = std::env::var(ASR_MIN_CHUNK_TIMEOUT_SECS_ENV)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(ASR_DEFAULT_MIN_CHUNK_TIMEOUT_SECS)
        .min(ASR_DEFAULT_CHUNK_TIMEOUT_SECS);
    let timeout_multiplier = std::env::var(ASR_TIMEOUT_MULTIPLIER_ENV)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(ASR_DEFAULT_TIMEOUT_MULTIPLIER)
        .min(ASR_MAX_TIMEOUT_MULTIPLIER);
    Duration::from_secs(
        duration_secs
            .saturating_mul(timeout_multiplier)
            .clamp(min_timeout_secs, ASR_DEFAULT_CHUNK_TIMEOUT_SECS),
    )
}

fn env_flag_enabled(key: &str) -> bool {
    std::env::var(key)
        .map(|value| matches!(value.as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(false)
}

fn host_memory_bytes() -> Option<u64> {
    let output = Command::new("sysctl")
        .arg("-n")
        .arg("hw.memsize")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse::<u64>()
        .ok()
}

fn default_footprint_limit_mb(model_path: &Path, total_memory_bytes: Option<u64>) -> u64 {
    let model_limit_mb = model_expected_footprint_limit_mb(model_path);
    let Some(total_memory_bytes) = total_memory_bytes else {
        return model_limit_mb;
    };
    let host_safety_mb =
        ((total_memory_bytes / 1024 / 1024) * ASR_HOST_MEMORY_SAFETY_PERCENT) / 100;
    model_limit_mb.min(host_safety_mb.max(ASR_MIN_HOST_SAFETY_MB))
}

fn effective_footprint_limit_mb(safe_limit_mb: u64, configured_limit_mb: Option<u64>) -> u64 {
    configured_limit_mb
        .filter(|value| *value > 0)
        .map(|value| value.min(safe_limit_mb))
        .unwrap_or(safe_limit_mb)
}

fn model_expected_footprint_limit_mb(model_path: &Path) -> u64 {
    let model_name = model_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if model_name.contains("0.6b") || model_name.contains("06b") {
        ASR_MODEL_LIMIT_06B_MB
    } else if model_name.contains("1.7b") || model_name.contains("17b") {
        ASR_MODEL_LIMIT_17B_MB
    } else {
        ASR_DEFAULT_MODEL_LIMIT_MB
    }
}

pub(crate) fn read_process_footprint_bytes(pid: u32) -> Result<(u64, bool), String> {
    match read_physical_footprint_bytes(pid) {
        Ok(footprint) => Ok((footprint, true)),
        Err(vmmap_error) => read_rss_bytes(pid)
            .map(|rss| (rss, false))
            .map_err(|rss_error| {
                format!("vmmap failed ({vmmap_error}); rss fallback failed ({rss_error})")
            }),
    }
}

pub(crate) fn physical_footprint_sample_interval() -> Duration {
    let secs = std::env::var(ASR_PHYSICAL_SAMPLE_INTERVAL_SECS_ENV)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .map(|value| {
            value.clamp(
                ASR_MIN_PHYSICAL_SAMPLE_INTERVAL_SECS,
                ASR_MAX_PHYSICAL_SAMPLE_INTERVAL_SECS,
            )
        })
        .unwrap_or(ASR_DEFAULT_PHYSICAL_SAMPLE_INTERVAL_SECS);
    Duration::from_secs(secs)
}

fn read_physical_footprint_bytes(pid: u32) -> Result<u64, String> {
    let output = Command::new("vmmap")
        .arg("-summary")
        .arg(pid.to_string())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .map_err(|error| format!("run vmmap: {error}"))?;
    if !output.status.success() {
        return Err(format!("vmmap exited with {}", output.status));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout
        .lines()
        .find_map(|line| {
            line.trim()
                .strip_prefix("Physical footprint:")
                .and_then(|size| parse_vmmap_size_bytes(size.trim()))
        })
        .ok_or_else(|| "vmmap output missing Physical footprint".to_string())
}

fn read_rss_bytes(pid: u32) -> Result<u64, String> {
    let output = Command::new("ps")
        .arg("-o")
        .arg("rss=")
        .arg("-p")
        .arg(pid.to_string())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .map_err(|error| format!("run ps: {error}"))?;
    if !output.status.success() {
        return Err(format!("ps exited with {}", output.status));
    }
    let rss_kb = String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse::<u64>()
        .map_err(|error| format!("parse ps rss: {error}"))?;
    Ok(rss_kb.saturating_mul(1024))
}

fn parse_vmmap_size_bytes(size: &str) -> Option<u64> {
    let token = size.split_whitespace().next()?;
    let (number, multiplier) = match token.chars().last()? {
        'K' => (&token[..token.len() - 1], 1024f64),
        'M' => (&token[..token.len() - 1], 1024f64 * 1024f64),
        'G' => (&token[..token.len() - 1], 1024f64 * 1024f64 * 1024f64),
        _ => (token, 1f64),
    };
    number
        .parse::<f64>()
        .ok()
        .map(|value| (value * multiplier) as u64)
}

#[cfg(unix)]
fn configure_process_group(command: &mut Command) {
    command.process_group(0);
}

#[cfg(not(unix))]
fn configure_process_group(_command: &mut Command) {}

#[cfg(unix)]
fn terminate_child_process_group(pid: u32, child: &mut std::process::Child) {
    use nix::sys::signal::{killpg, Signal};
    use nix::unistd::Pid;

    let _ = killpg(Pid::from_raw(pid as i32), Signal::SIGKILL);
    let _ = child.kill();
}

#[cfg(not(unix))]
fn terminate_child_process_group(_pid: u32, child: &mut std::process::Child) {
    let _ = child.kill();
}

/// Extract the transcription text from the `asr` CLI stdout.
///
/// The CLI prints `Text: <asr_text>...</asr_text>` (closing tag may be absent
/// on the last line). We strip the wrapping markers and trim. If the format is
/// unexpected we fall back to the raw last non-empty line so we never silently
/// lose content.
pub fn parse_asr_cli_text(stdout: &str) -> String {
    if let Some(start) = stdout.find("<asr_text>") {
        let after_open = &stdout[start + "<asr_text>".len()..];
        let body = match after_open.find("</asr_text>") {
            Some(end) => &after_open[..end],
            None => after_open,
        };
        return body.trim().to_string();
    }
    // Fallback: take the line starting with "Text: ", or the last non-empty line.
    for line in stdout.lines().rev() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("Text:") {
            return rest.trim().to_string();
        }
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_asr_tag() {
        assert_eq!(
            parse_asr_cli_text("some noise\nText: <asr_text>hello world</asr_text>\n"),
            "hello world"
        );
    }

    #[test]
    fn parse_asr_tag_no_close() {
        assert_eq!(
            parse_asr_cli_text("Text: <asr_text>partial output"),
            "partial output"
        );
    }

    #[test]
    fn parse_fallback_text_prefix() {
        assert_eq!(
            parse_asr_cli_text("some noise\nText: fallback text\n"),
            "fallback text"
        );
    }

    #[test]
    fn parse_fallback_last_line() {
        assert_eq!(parse_asr_cli_text("some noise\nfinal line\n"), "final line");
    }

    #[test]
    fn parse_empty() {
        assert_eq!(parse_asr_cli_text(""), "");
    }

    #[test]
    fn parse_vmmap_size_units() {
        assert_eq!(parse_vmmap_size_bytes("512K"), Some(512 * 1024));
        assert_eq!(parse_vmmap_size_bytes("12.5M"), Some(13_107_200));
        assert_eq!(parse_vmmap_size_bytes("14.0G"), Some(15_032_385_536));
        assert_eq!(parse_vmmap_size_bytes("not-a-size"), None);
    }

    #[test]
    fn default_footprint_limit_is_model_informed_and_host_safe() {
        let model_06b = Path::new("/tmp/Qwen3-ASR-0.6B");
        let model_17b = Path::new("/tmp/Qwen3-ASR-1.7B");
        let unknown = Path::new("/tmp/custom-asr");

        assert_eq!(
            default_footprint_limit_mb(model_06b, Some(64 * 1024 * 1024 * 1024)),
            8 * 1024
        );
        assert_eq!(
            default_footprint_limit_mb(model_17b, Some(64 * 1024 * 1024 * 1024)),
            18 * 1024
        );
        assert_eq!(
            default_footprint_limit_mb(model_17b, Some(16 * 1024 * 1024 * 1024)),
            14_745
        );
        assert_eq!(default_footprint_limit_mb(unknown, None), 12 * 1024);
    }

    #[test]
    fn configured_footprint_limit_cannot_exceed_safe_limit() {
        assert_eq!(
            effective_footprint_limit_mb(12 * 1024, Some(20 * 1024)),
            12 * 1024
        );
        assert_eq!(
            effective_footprint_limit_mb(12 * 1024, Some(8 * 1024)),
            8 * 1024
        );
        assert_eq!(effective_footprint_limit_mb(12 * 1024, Some(0)), 12 * 1024);
        assert_eq!(effective_footprint_limit_mb(12 * 1024, None), 12 * 1024);
    }

    #[test]
    fn physical_footprint_sample_interval_is_bounded() {
        std::env::remove_var(ASR_PHYSICAL_SAMPLE_INTERVAL_SECS_ENV);
        assert_eq!(
            physical_footprint_sample_interval(),
            Duration::from_secs(ASR_DEFAULT_PHYSICAL_SAMPLE_INTERVAL_SECS)
        );

        std::env::set_var(ASR_PHYSICAL_SAMPLE_INTERVAL_SECS_ENV, "1");
        assert_eq!(
            physical_footprint_sample_interval(),
            Duration::from_secs(ASR_MIN_PHYSICAL_SAMPLE_INTERVAL_SECS)
        );

        std::env::set_var(ASR_PHYSICAL_SAMPLE_INTERVAL_SECS_ENV, "120");
        assert_eq!(
            physical_footprint_sample_interval(),
            Duration::from_secs(ASR_MAX_PHYSICAL_SAMPLE_INTERVAL_SECS)
        );

        std::env::set_var(ASR_PHYSICAL_SAMPLE_INTERVAL_SECS_ENV, "7");
        assert_eq!(physical_footprint_sample_interval(), Duration::from_secs(7));
        std::env::remove_var(ASR_PHYSICAL_SAMPLE_INTERVAL_SECS_ENV);
    }

    #[test]
    fn abort_check_kills_running_cli_child() {
        let temp = tempfile::tempdir().unwrap();
        let script = temp.path().join("fake-asr.sh");
        std::fs::write(&script, "#!/bin/sh\nsleep 10\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = std::fs::metadata(&script).unwrap().permissions();
            permissions.set_mode(0o755);
            std::fs::set_permissions(&script, permissions).unwrap();
        }

        let result = run_asr_cli_with_footprint_guard_timeout_and_abort(
            &script,
            Path::new("/tmp/model"),
            Path::new("/tmp/audio.wav"),
            "chinese",
            None,
            Some(Box::new(|| true)),
        );

        let error = result.unwrap_err();
        assert!(error.contains(ASR_ABORTED_ERROR_MARKER), "{error}");
    }
}
