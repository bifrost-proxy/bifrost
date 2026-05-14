use std::fs::{self, OpenOptions};
use std::io;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use bifrost_admin::asr_runtime::{
    clear_service_state, fixed_asr_home, install_dir, model_dir, now_ms, probe_health_blocking,
    read_service_state, stop_pid, write_service_state, AsrServiceState, DEFAULT_ASR_HOST,
};
use bifrost_admin::resource_download::{download_with_resume, DownloadProgress, DownloadRequest};
use bifrost_core::{BifrostError, Result};
use tokio::sync::mpsc;

use crate::cli::{AiAsrCommands, AiCommands};

const SERVICE_START_TIMEOUT: Duration = Duration::from_secs(180);
const ASR_RELEASE_REPO: &str = "second-state/qwen3_asr_rs";
const ASR_SAMPLE_BASE_URL: &str =
    "https://raw.githubusercontent.com/second-state/qwen3_asr_rs/main/test_audio";

pub fn handle_ai_command(action: AiCommands) -> Result<()> {
    match action {
        AiCommands::Asr { action } => handle_asr_command(action),
    }
}

fn handle_asr_command(action: AiAsrCommands) -> Result<()> {
    ensure_supported_platform()?;
    match action {
        AiAsrCommands::Start { model, language } => {
            let state = start_service(&model, &language)?;
            println!(
                "Qwen3-ASR service started: http://{}:{}",
                state.host, state.port
            );
            Ok(())
        }
        AiAsrCommands::Stop => {
            stop_service()?;
            println!("Qwen3-ASR service stopped.");
            Ok(())
        }
        AiAsrCommands::Status { json } => {
            print_status(json)?;
            Ok(())
        }
        AiAsrCommands::StreamFile {
            audio,
            model,
            language,
            format: _,
        } => stream_file(&audio, &model, &language),
    }
}

fn ensure_supported_platform() -> Result<()> {
    if std::env::consts::OS == "macos" && std::env::consts::ARCH == "aarch64" {
        Ok(())
    } else {
        Err(BifrostError::Config(format!(
            "Qwen3-ASR local runtime is only supported on Apple Silicon macOS; current platform is {}-{}",
            std::env::consts::OS,
            std::env::consts::ARCH
        )))
    }
}

fn print_status(json: bool) -> Result<()> {
    let state = read_service_state(&bifrost_storage::data_dir());
    let ready = state
        .as_ref()
        .map(|state| probe_health_blocking(&state.host, state.port, Duration::from_secs(2)).is_ok())
        .unwrap_or(false);
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "ready": ready,
                "service": state,
            }))
            .map_err(|error| BifrostError::Config(error.to_string()))?
        );
    } else if let Some(state) = state {
        println!("ready: {ready}");
        println!("server: http://{}:{}", state.host, state.port);
        println!("model: {}", state.model);
        println!("language: {}", state.language);
        println!("managed_by: {}", state.managed_by);
    } else {
        println!("ready: false");
        println!("server: not running");
    }
    Ok(())
}

fn stream_file(audio: &Path, model: &str, language: &str) -> Result<()> {
    if !audio.is_file() {
        return Err(BifrostError::Config(format!(
            "audio file does not exist: {}",
            audio.display()
        )));
    }

    let (_state, started_here) = match healthy_state(model, language) {
        Some(state) => (state, false),
        None => (start_service(model, language)?, true),
    };

    let home = fixed_asr_home();
    let install = install_dir(&home);
    let output = Command::new(install.join("asr"))
        .arg(model_dir(&home, model))
        .arg(audio)
        .arg(language)
        .output()
        .map_err(|error| {
            BifrostError::Io(std::io::Error::other(format!("run ASR CLI: {error}")))
        })?;

    if started_here {
        let _ = stop_service();
    }

    if !output.status.success() {
        Err(BifrostError::Config(format!(
            "ASR CLI exited with {}; stderr: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        )))?
    }

    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    println!(
        "{}",
        serde_json::json!({
            "type": "partial",
            "index": 0,
            "text": text,
            "delta": text,
            "committed": ""
        })
    );
    println!(
        "{}",
        serde_json::json!({
            "type": "final",
            "index": 0,
            "text": text,
            "delta": text,
            "committed": text
        })
    );
    Ok(())
}

fn start_service(model: &str, language: &str) -> Result<AsrServiceState> {
    if let Some(state) = healthy_state(model, language) {
        return Ok(state);
    }

    let home = fixed_asr_home();
    let install = install_dir(&home);
    let model_path = model_dir(&home, model);
    prepare_cli_assets(&home, model)?;
    ensure_ffmpeg_for_cli()?;

    if !install.join("asr-server").is_file() || !model_path.join("tokenizer.json").is_file() {
        return Err(BifrostError::Config(format!(
            "Qwen3-ASR assets are still missing under {} after self-check.",
            install.display()
        )));
    }

    let port = allocate_port()?;
    let log_path = bifrost_storage::data_dir().join("asr/asr-server.log");
    if let Some(parent) = log_path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| {
            BifrostError::Io(std::io::Error::other(format!(
                "create ASR log dir: {error}"
            )))
        })?;
    }
    let stdout = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .map_err(|error| {
            BifrostError::Io(std::io::Error::other(format!("open ASR log: {error}")))
        })?;
    let stderr = stdout.try_clone().map_err(|error| {
        BifrostError::Io(std::io::Error::other(format!("clone ASR log: {error}")))
    })?;

    let child = Command::new(install.join("asr-server"))
        .arg("--model-dir")
        .arg(model_path)
        .arg("--host")
        .arg(DEFAULT_ASR_HOST)
        .arg("--port")
        .arg(port.to_string())
        .arg("--language")
        .arg(language)
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr))
        .spawn()
        .map_err(|error| {
            BifrostError::Io(std::io::Error::other(format!("start ASR server: {error}")))
        })?;

    let state = AsrServiceState {
        host: DEFAULT_ASR_HOST.to_string(),
        port,
        model: model.to_string(),
        language: language.to_string(),
        home,
        pid: Some(child.id()),
        managed_by: "cli".to_string(),
        started_at_ms: now_ms(),
    };
    write_service_state(&bifrost_storage::data_dir(), &state).map_err(BifrostError::Config)?;

    let deadline = Instant::now() + SERVICE_START_TIMEOUT;
    while Instant::now() < deadline {
        if probe_health_blocking(DEFAULT_ASR_HOST, port, Duration::from_secs(2)).is_ok() {
            return Ok(state);
        }
        thread::sleep(Duration::from_secs(1));
    }

    let _ = stop_service();
    Err(BifrostError::Config(format!(
        "Timed out waiting for Qwen3-ASR service to become healthy. Log: {}",
        log_path.display()
    )))
}

fn stop_service() -> Result<()> {
    if let Some(state) = read_service_state(&bifrost_storage::data_dir()) {
        if let Some(pid) = state.pid {
            let _ = stop_pid(pid);
        }
    }
    clear_service_state(&bifrost_storage::data_dir()).map_err(BifrostError::Config)
}

fn healthy_state(model: &str, language: &str) -> Option<AsrServiceState> {
    let state = read_service_state(&bifrost_storage::data_dir())?;
    if state.model != model || state.language != language {
        return None;
    }
    probe_health_blocking(&state.host, state.port, Duration::from_secs(2))
        .is_ok()
        .then_some(state)
}

fn allocate_port() -> Result<u16> {
    TcpListener::bind((DEFAULT_ASR_HOST, 0))
        .and_then(|listener| listener.local_addr())
        .map(|addr| addr.port())
        .map_err(|error| {
            BifrostError::Io(std::io::Error::other(format!("allocate ASR port: {error}")))
        })
}

fn prepare_cli_assets(home: &Path, model: &str) -> Result<()> {
    if cli_assets_installed(home, model) {
        return Ok(());
    }
    eprintln!("Qwen3-ASR self-check is repairing missing runtime or model assets.");
    let runtime = tokio::runtime::Runtime::new().map_err(|error| {
        BifrostError::Io(io::Error::other(format!(
            "create ASR download runtime: {error}"
        )))
    })?;
    runtime.block_on(download_cli_assets(home.to_path_buf(), model.to_string()))?;
    install_cli_release(home)?;
    prepare_cli_model(home, model)?;
    Ok(())
}

async fn download_cli_assets(home: PathBuf, model: String) -> Result<()> {
    let client = reqwest::Client::builder()
        .build()
        .map_err(|error| BifrostError::Config(format!("build ASR downloader client: {error}")))?;
    let requests = cli_download_requests(&home, &model)?;
    let (progress_tx, mut progress_rx) = mpsc::unbounded_channel::<DownloadProgress>();
    let progress_task = tokio::spawn(async move {
        while let Some(progress) = progress_rx.recv().await {
            if progress.complete {
                eprintln!("downloaded {}", progress.label);
            } else if let Some(percent) = progress.percent {
                eprintln!("downloading {}: {}%", progress.label, percent);
            } else {
                eprintln!(
                    "downloading {}: {} bytes",
                    progress.label, progress.downloaded_bytes
                );
            }
        }
    });
    for request in requests {
        download_with_resume(&client, request, Some(progress_tx.clone()))
            .await
            .map_err(BifrostError::Config)?;
    }
    drop(progress_tx);
    let _ = progress_task.await;
    Ok(())
}

fn cli_download_requests(home: &Path, model: &str) -> Result<Vec<DownloadRequest>> {
    let mut requests = Vec::new();
    let install = install_dir(home);
    if !install.join("asr").is_file() || !install.join("asr-server").is_file() {
        let asset = detect_asr_release_asset()?;
        requests.push(DownloadRequest {
            url: format!(
                "https://github.com/{ASR_RELEASE_REPO}/releases/latest/download/{asset}.zip"
            ),
            dest: home.join(format!("{asset}.zip")),
            label: format!("{asset}.zip"),
        });
    }
    for file in required_model_files(model) {
        let dest = model_dir(home, model).join(file);
        if !dest.is_file() {
            requests.push(DownloadRequest {
                url: format!("https://huggingface.co/Qwen/{model}/resolve/main/{file}"),
                dest,
                label: format!("{model}/{file}"),
            });
        }
    }
    for sample in [
        "sample1.wav",
        "sample1.txt",
        "sample2.wav",
        "sample2.txt",
        "sample3.wav",
        "sample3.txt",
    ] {
        let dest = install.join(sample);
        if !dest.is_file() {
            requests.push(DownloadRequest {
                url: format!("{ASR_SAMPLE_BASE_URL}/{sample}"),
                dest,
                label: sample.to_string(),
            });
        }
    }
    Ok(requests)
}

fn install_cli_release(home: &Path) -> Result<()> {
    let install = install_dir(home);
    if install.join("asr").is_file() && install.join("asr-server").is_file() {
        return Ok(());
    }
    let asset = detect_asr_release_asset()?;
    let zip_path = home.join(format!("{asset}.zip"));
    let extracted = home.join(asset);
    extract_zip_to_dir(&zip_path, home)?;
    fs::create_dir_all(&install).map_err(|error| {
        BifrostError::Io(io::Error::other(format!("create ASR install dir: {error}")))
    })?;
    copy_dir_contents(&extracted, &install)?;
    let _ = fs::remove_dir_all(&extracted);
    let _ = fs::remove_file(&zip_path);
    mark_cli_binaries_executable(&install)?;
    Ok(())
}

fn prepare_cli_model(home: &Path, model: &str) -> Result<()> {
    let model_path = model_dir(home, model);
    fs::create_dir_all(&model_path).map_err(|error| {
        BifrostError::Io(io::Error::other(format!("create ASR model dir: {error}")))
    })?;
    for file in required_model_files(model) {
        let path = model_path.join(file);
        if !path.is_file() {
            return Err(BifrostError::Config(format!(
                "missing ASR model file after download: {}",
                path.display()
            )));
        }
    }
    let tokenizer_src = install_dir(home)
        .join("tokenizers")
        .join(format!("tokenizer-{}.json", tokenizer_size(model)?));
    fs::copy(&tokenizer_src, model_path.join("tokenizer.json")).map_err(|error| {
        BifrostError::Io(io::Error::other(format!(
            "copy ASR tokenizer {}: {error}",
            tokenizer_src.display()
        )))
    })?;
    Ok(())
}

fn ensure_ffmpeg_for_cli() -> Result<()> {
    if command_succeeds("ffmpeg", &["-version"]) {
        return Ok(());
    }
    if !command_succeeds("brew", &["--version"]) {
        return Err(BifrostError::Config(
            "ffmpeg is required for ASR audio preprocessing, and Homebrew was not found to install it automatically. Install Homebrew and run `brew install ffmpeg`, then retry the same ASR command."
                .to_string(),
        ));
    }
    eprintln!("Qwen3-ASR self-check is installing ffmpeg with Homebrew.");
    let output = Command::new("brew")
        .arg("install")
        .arg("ffmpeg")
        .output()
        .map_err(|error| BifrostError::Io(io::Error::other(format!("run brew: {error}"))))?;
    if output.status.success() && command_succeeds("ffmpeg", &["-version"]) {
        Ok(())
    } else {
        Err(BifrostError::Config(format!(
            "Homebrew ffmpeg installation failed with {}. Install it manually with `brew install ffmpeg`, then retry the same ASR command. {}",
            output.status,
            summarize_command_output(&output.stdout, &output.stderr)
        )))
    }
}

fn cli_assets_installed(home: &Path, model: &str) -> bool {
    install_dir(home).join("asr").is_file()
        && install_dir(home).join("asr-server").is_file()
        && model_dir(home, model).join("tokenizer.json").is_file()
        && required_model_files(model)
            .iter()
            .all(|file| model_dir(home, model).join(file).is_file())
}

fn detect_asr_release_asset() -> Result<&'static str> {
    ensure_supported_platform()?;
    Ok("asr-macos-aarch64")
}

fn required_model_files(model: &str) -> &'static [&'static str] {
    match model {
        "Qwen3-ASR-0.6B" => &["config.json", "model.safetensors"],
        "Qwen3-ASR-1.7B" => &[
            "config.json",
            "model.safetensors.index.json",
            "model-00001-of-00002.safetensors",
            "model-00002-of-00002.safetensors",
        ],
        _ => &["config.json"],
    }
}

fn tokenizer_size(model: &str) -> Result<&'static str> {
    match model {
        "Qwen3-ASR-0.6B" => Ok("0.6B"),
        "Qwen3-ASR-1.7B" => Ok("1.7B"),
        _ => Err(BifrostError::Config(format!(
            "unsupported ASR model: {model}"
        ))),
    }
}

fn extract_zip_to_dir(zip_path: &Path, dest: &Path) -> Result<()> {
    let zip_path = zip_path.to_path_buf();
    let dest = dest.to_path_buf();
    let file = fs::File::open(&zip_path).map_err(|error| {
        BifrostError::Io(io::Error::other(format!(
            "open ASR release zip {}: {error}",
            zip_path.display()
        )))
    })?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|error| BifrostError::Config(format!("read ASR release zip: {error}")))?;
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|error| BifrostError::Config(format!("read zip entry: {error}")))?;
        let Some(enclosed) = entry.enclosed_name().map(|path| path.to_path_buf()) else {
            continue;
        };
        let output = dest.join(enclosed);
        if entry.is_dir() {
            fs::create_dir_all(&output).map_err(|error| {
                BifrostError::Io(io::Error::other(format!(
                    "create ASR unzip dir {}: {error}",
                    output.display()
                )))
            })?;
            continue;
        }
        if let Some(parent) = output.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                BifrostError::Io(io::Error::other(format!(
                    "create ASR unzip parent {}: {error}",
                    parent.display()
                )))
            })?;
        }
        let mut out = fs::File::create(&output).map_err(|error| {
            BifrostError::Io(io::Error::other(format!(
                "create ASR unzip file {}: {error}",
                output.display()
            )))
        })?;
        io::copy(&mut entry, &mut out).map_err(|error| {
            BifrostError::Io(io::Error::other(format!(
                "extract ASR unzip file {}: {error}",
                output.display()
            )))
        })?;
    }
    Ok(())
}

fn copy_dir_contents(from: &Path, to: &Path) -> Result<()> {
    for entry in fs::read_dir(from).map_err(|error| {
        BifrostError::Io(io::Error::other(format!(
            "read ASR release dir {}: {error}",
            from.display()
        )))
    })? {
        let entry = entry.map_err(|error| {
            BifrostError::Io(io::Error::other(format!("read ASR release entry: {error}")))
        })?;
        let source = entry.path();
        let dest = to.join(entry.file_name());
        if source.is_dir() {
            fs::create_dir_all(&dest).map_err(|error| {
                BifrostError::Io(io::Error::other(format!(
                    "create ASR install dir {}: {error}",
                    dest.display()
                )))
            })?;
            copy_dir_contents(&source, &dest)?;
        } else {
            fs::copy(&source, &dest).map_err(|error| {
                BifrostError::Io(io::Error::other(format!(
                    "copy ASR release {} -> {}: {error}",
                    source.display(),
                    dest.display()
                )))
            })?;
        }
    }
    Ok(())
}

fn mark_cli_binaries_executable(install: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for name in ["asr", "asr-server"] {
            let path = install.join(name);
            let mut permissions = fs::metadata(&path)
                .map_err(|error| {
                    BifrostError::Io(io::Error::other(format!(
                        "stat ASR binary {}: {error}",
                        path.display()
                    )))
                })?
                .permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&path, permissions).map_err(|error| {
                BifrostError::Io(io::Error::other(format!(
                    "chmod ASR binary {}: {error}",
                    path.display()
                )))
            })?;
        }
    }
    Ok(())
}

fn command_succeeds(command: &str, args: &[&str]) -> bool {
    Command::new(command)
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn summarize_command_output(stdout: &[u8], stderr: &[u8]) -> String {
    let stdout = String::from_utf8_lossy(stdout);
    let stderr = String::from_utf8_lossy(stderr);
    let combined = format!("{}{}", stdout.trim(), stderr.trim());
    let trimmed = combined.trim();
    if trimmed.is_empty() {
        return "No command output was captured.".to_string();
    }
    let max_chars = 1200;
    if trimmed.chars().count() <= max_chars {
        return trimmed.to_string();
    }
    let tail = trimmed
        .chars()
        .rev()
        .take(max_chars)
        .collect::<String>()
        .chars()
        .rev()
        .collect::<String>();
    format!("...{tail}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::TempDir;

    struct EnvGuard {
        previous: PathBuf,
    }

    impl EnvGuard {
        fn set_data_dir(path: &Path) -> Self {
            let previous = bifrost_storage::data_dir();
            bifrost_storage::set_data_dir(path.to_path_buf());
            Self { previous }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            bifrost_storage::set_data_dir(self.previous.clone());
        }
    }

    #[test]
    fn status_reads_persisted_asr_service_state() {
        let temp = TempDir::new().unwrap();
        let _guard = EnvGuard::set_data_dir(temp.path());
        let state = AsrServiceState {
            host: "127.0.0.1".to_string(),
            port: 18080,
            model: "Qwen3-ASR-1.7B".to_string(),
            language: "chinese".to_string(),
            home: fixed_asr_home(),
            pid: Some(42),
            managed_by: "test".to_string(),
            started_at_ms: 1,
        };
        write_service_state(temp.path(), &state).unwrap();
        let loaded = read_service_state(temp.path()).unwrap();
        assert_eq!(loaded.port, 18080);
        assert_eq!(loaded.managed_by, "test");
    }
}
