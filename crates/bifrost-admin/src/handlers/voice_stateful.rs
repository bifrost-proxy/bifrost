#[cfg(target_os = "macos")]
use std::path::Path;
use std::path::PathBuf;
#[cfg(any(target_os = "macos", all(test, unix)))]
use std::process::Stdio;
use std::time::Duration;

use base64::Engine as _;
use serde::{Deserialize, Serialize};
#[cfg(any(target_os = "macos", all(test, unix)))]
use tokio::io::AsyncBufReadExt;
use tokio::io::{AsyncWriteExt, BufReader, Lines};
#[cfg(any(target_os = "macos", all(test, unix)))]
use tokio::process::Command;
use tokio::process::{Child, ChildStdin, ChildStdout};
use tokio::time::Instant;
use tracing::warn;

pub(crate) const STATEFUL_PROVIDER_ID: &str = "qwen3_stateful_streaming";
#[cfg(target_os = "macos")]
const DEFAULT_WORKER_STARTUP_TIMEOUT_MS: u64 = 60_000;
#[cfg(target_os = "macos")]
const DEFAULT_WORKER_REQUEST_TIMEOUT_MS: u64 = 30_000;
const WORKER_EXIT_TIMEOUT_MS: u64 = 2_000;

#[derive(Debug, Clone)]
pub struct StatefulVoiceConfig {
    pub model: String,
    pub model_dir: PathBuf,
    pub language: String,
    pub chunk_size_sec: f32,
    pub initial_text: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct StatefulVoiceResult {
    pub(crate) text: String,
    pub(crate) language: String,
    pub(crate) inference_ms: u128,
}

pub(crate) enum StatefulVoiceSession {
    Process(Box<ProcessStatefulVoiceSession>),
    Fake(FakeStatefulVoiceSession),
}

impl StatefulVoiceSession {
    pub(crate) fn fake(text: String, language: String) -> Self {
        Self::Fake(FakeStatefulVoiceSession {
            text,
            language,
            feed_count: 0,
        })
    }

    pub(crate) async fn feed_pcm16(
        &mut self,
        pcm: &[u8],
    ) -> Result<Option<StatefulVoiceResult>, String> {
        match self {
            Self::Process(session) => session.feed_pcm16(pcm).await,
            Self::Fake(session) => session.feed_pcm16(pcm).await,
        }
    }

    /// Reset streaming state without killing the worker.
    pub(crate) async fn reset(&mut self) -> Result<StatefulVoiceResult, String> {
        match self {
            Self::Process(session) => session.reset().await,
            Self::Fake(session) => session.finish().await,
        }
    }

    /// Kill the worker process and wait for it to fully exit, releasing memory.
    pub(crate) async fn shutdown(self) {
        match self {
            Self::Process(session) => session.shutdown().await,
            Self::Fake(_) => {}
        }
    }
}

pub(crate) async fn start_stateful_voice_session(
    config: StatefulVoiceConfig,
) -> Result<StatefulVoiceSession, String> {
    platform::start_process_stateful_voice_session(config)
        .await
        .map(Box::new)
        .map(StatefulVoiceSession::Process)
}

pub(crate) struct FakeStatefulVoiceSession {
    text: String,
    language: String,
    feed_count: u64,
}

impl FakeStatefulVoiceSession {
    async fn feed_pcm16(&mut self, pcm: &[u8]) -> Result<Option<StatefulVoiceResult>, String> {
        if pcm.is_empty() || pcm.iter().all(|byte| *byte == 0) {
            return Ok(None);
        }
        self.feed_count = self.feed_count.saturating_add(1);
        Ok(Some(StatefulVoiceResult {
            text: self.text.clone(),
            language: self.language.clone(),
            inference_ms: 0,
        }))
    }

    async fn finish(&mut self) -> Result<StatefulVoiceResult, String> {
        Ok(StatefulVoiceResult {
            text: self.text.clone(),
            language: self.language.clone(),
            inference_ms: 0,
        })
    }
}

pub(crate) struct ProcessStatefulVoiceSession {
    child: Child,
    stdin: ChildStdin,
    stdout: Lines<BufReader<ChildStdout>>,
    request_timeout: Duration,
}

impl ProcessStatefulVoiceSession {
    async fn feed_pcm16(&mut self, pcm: &[u8]) -> Result<Option<StatefulVoiceResult>, String> {
        if pcm.is_empty() {
            return Ok(None);
        }
        let request = WorkerRequest::Audio {
            data: base64::engine::general_purpose::STANDARD.encode(pcm),
        };
        self.write_worker_request(&request).await?;
        match self
            .read_worker_output_with_timeout("audio response", self.request_timeout)
            .await?
        {
            WorkerOutput::Partial {
                text,
                language,
                inference_ms,
            } => Ok(Some(StatefulVoiceResult {
                text,
                language,
                inference_ms: u128::from(inference_ms),
            })),
            WorkerOutput::Empty => Ok(None),
            WorkerOutput::Error { message } => Err(message),
            other => Err(format!(
                "unexpected stateful ASR worker audio response: {other:?}"
            )),
        }
    }

    /// Reset streaming state without killing the worker process.
    /// Used for silence commits to finalize the current utterance while keeping
    /// the worker alive for the next utterance.
    async fn reset(&mut self) -> Result<StatefulVoiceResult, String> {
        self.write_worker_request(&WorkerRequest::Reset).await?;
        match self
            .read_worker_output_with_timeout("reset response", self.request_timeout)
            .await?
        {
            WorkerOutput::Final {
                text,
                language,
                inference_ms,
            } => Ok(StatefulVoiceResult {
                text,
                language,
                inference_ms: u128::from(inference_ms),
            }),
            WorkerOutput::Error { message } => Err(message),
            other => Err(format!(
                "unexpected stateful ASR worker reset response: {other:?}"
            )),
        }
    }

    async fn write_worker_request(&mut self, request: &WorkerRequest) -> Result<(), String> {
        let line = serde_json::to_string(request)
            .map_err(|error| format!("serialize stateful ASR worker request: {error}"))?;
        self.write_worker_bytes(line.as_bytes(), "request").await?;
        self.write_worker_bytes(b"\n", "request newline").await?;
        match tokio::time::timeout(self.request_timeout, self.stdin.flush()).await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(error)) => Err(format!("flush stateful ASR worker request: {error}")),
            Err(_) => Err(self.kill_timeout_error("flush request", self.request_timeout)),
        }
    }

    async fn write_worker_bytes(&mut self, bytes: &[u8], operation: &str) -> Result<(), String> {
        match tokio::time::timeout(self.request_timeout, self.stdin.write_all(bytes)).await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(error)) => Err(format!("write stateful ASR worker {operation}: {error}")),
            Err(_) => Err(self.kill_timeout_error(operation, self.request_timeout)),
        }
    }

    async fn read_worker_output_with_timeout(
        &mut self,
        operation: &str,
        timeout: Duration,
    ) -> Result<WorkerOutput, String> {
        let deadline = Instant::now() + timeout;
        loop {
            let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
                return Err(self.kill_timeout_error(operation, timeout));
            };
            if remaining.is_zero() {
                return Err(self.kill_timeout_error(operation, timeout));
            }

            let line = match tokio::time::timeout(remaining, self.stdout.next_line()).await {
                Ok(Ok(Some(line))) => line,
                Ok(Ok(None)) => {
                    return Err("stateful ASR worker exited without a response".to_string());
                }
                Ok(Err(error)) => {
                    return Err(format!("read stateful ASR worker response: {error}"));
                }
                Err(_) => {
                    return Err(self.kill_timeout_error(operation, timeout));
                }
            };
            if line.trim().is_empty() {
                continue;
            }
            if !line.trim_start().starts_with('{') {
                warn!(
                    line = %line,
                    "ignored non-JSON stateful ASR worker stdout line"
                );
                continue;
            }
            return serde_json::from_str(&line).map_err(|error| {
                format!("parse stateful ASR worker response: {error}; line={line}")
            });
        }
    }

    fn kill_timeout_error(&mut self, operation: &str, timeout: Duration) -> String {
        let _ = self.child.start_kill();
        format!(
            "stateful ASR worker {operation} timed out after {}ms; worker unloaded",
            timeout.as_millis()
        )
    }
}

impl ProcessStatefulVoiceSession {
    /// Gracefully kill the worker process and wait for it to fully exit,
    /// ensuring its ~2GB memory is released before a new worker is spawned.
    pub(crate) async fn shutdown(mut self) {
        let _ = self.child.start_kill();
        let _ = tokio::time::timeout(
            Duration::from_millis(WORKER_EXIT_TIMEOUT_MS),
            self.child.wait(),
        )
        .await;
    }
}

impl Drop for ProcessStatefulVoiceSession {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
    }
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum WorkerRequest {
    Audio { data: String },
    Reset,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum WorkerOutput {
    Ready,
    Empty,
    Partial {
        text: String,
        language: String,
        inference_ms: u64,
    },
    Final {
        text: String,
        language: String,
        inference_ms: u64,
    },
    Error {
        message: String,
    },
}

#[cfg(target_os = "macos")]
fn worker_startup_timeout() -> Duration {
    worker_timeout_from_env(
        "BIFROST_VOICE_WORKER_STARTUP_TIMEOUT_MS",
        DEFAULT_WORKER_STARTUP_TIMEOUT_MS,
    )
}

#[cfg(target_os = "macos")]
fn worker_request_timeout() -> Duration {
    worker_timeout_from_env(
        "BIFROST_VOICE_WORKER_REQUEST_TIMEOUT_MS",
        DEFAULT_WORKER_REQUEST_TIMEOUT_MS,
    )
}

#[cfg(target_os = "macos")]
fn worker_timeout_from_env(key: &str, default_ms: u64) -> Duration {
    let millis = std::env::var(key)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default_ms);
    Duration::from_millis(millis)
}

#[cfg(target_os = "macos")]
fn validate_model_assets(model_dir: &Path) -> Result<(), String> {
    for file in ["config.json", "tokenizer.json"] {
        let path = model_dir.join(file);
        if !path.is_file() {
            return Err(format!("missing stateful ASR asset {}", path.display()));
        }
    }
    if !model_dir.join("model.safetensors").is_file()
        && !model_dir.join("model.safetensors.index.json").is_file()
    {
        return Err(format!(
            "missing stateful ASR safetensors under {}",
            model_dir.display()
        ));
    }
    Ok(())
}

#[cfg(target_os = "macos")]
mod platform {
    use std::io::{self, BufRead, Write};
    use std::time::Instant;

    use super::*;
    use qwen3_asr::{AsrInference, StreamingOptions};
    use tracing::warn;

    pub(super) async fn start_process_stateful_voice_session(
        config: StatefulVoiceConfig,
    ) -> Result<ProcessStatefulVoiceSession, String> {
        validate_model_assets(&config.model_dir)?;
        let exe = std::env::current_exe()
            .map_err(|error| format!("resolve Bifrost executable for voice worker: {error}"))?;
        let mut command = Command::new(exe);
        command
            .args(["ai", "voice", "worker"])
            .arg("--model")
            .arg(&config.model)
            .arg("--model-dir")
            .arg(&config.model_dir)
            .arg("--language")
            .arg(&config.language)
            .arg("--chunk-size-sec")
            .arg(format!("{:.3}", config.chunk_size_sec.clamp(0.5, 4.0)))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        if let Some(initial_text) = &config.initial_text {
            command.arg("--initial-text").arg(initial_text);
        }
        let mut child = command
            .spawn()
            .map_err(|error| format!("spawn stateful ASR worker process: {error}"))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| "stateful ASR worker stdin is unavailable".to_string())?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "stateful ASR worker stdout is unavailable".to_string())?;
        let mut session = ProcessStatefulVoiceSession {
            child,
            stdin,
            stdout: BufReader::new(stdout).lines(),
            request_timeout: worker_request_timeout(),
        };
        match session
            .read_worker_output_with_timeout("startup response", worker_startup_timeout())
            .await?
        {
            WorkerOutput::Ready => Ok(session),
            WorkerOutput::Error { message } => Err(message),
            other => Err(format!(
                "unexpected stateful ASR worker startup response: {other:?}"
            )),
        }
    }

    #[derive(Debug, Deserialize)]
    #[serde(tag = "type", rename_all = "snake_case")]
    enum WorkerInput {
        Audio { data: String },
        Reset,
        Finish,
    }

    /// Maximum audio duration (in samples) before automatically resetting the streaming
    /// state. Longer sessions cause decoder slowdown as the KV cache and audio_accum grow
    /// without bound. After this threshold, we finalize the current segment and reinit
    /// streaming with the last transcript as `initial_text` for context continuity.
    const MAX_SEGMENT_SAMPLES: usize = 16_000 * 30; // 30 seconds

    pub fn run_stateful_worker_stdio(config: StatefulVoiceConfig) -> Result<(), String> {
        validate_model_assets(&config.model_dir)?;
        let device = qwen3_asr::best_device();
        let engine =
            AsrInference::load(&config.model_dir, device).map_err(|error| error.to_string())?;
        warm_stateful_engine(&engine);
        let chunk_size_sec = config.chunk_size_sec.clamp(0.5, 4.0);
        let language = config.language;
        let mut options = StreamingOptions::default()
            .with_chunk_size_sec(chunk_size_sec)
            .with_language(&language);
        if let Some(initial_text) = config.initial_text {
            options = options.with_initial_text(initial_text);
        }
        let mut state = engine.init_streaming(options);
        let mut accumulated_samples: usize = 0;
        let mut last_text = String::new();
        write_worker_output(&WorkerOutput::Ready)?;
        let stdin = io::stdin();
        for line in stdin.lock().lines() {
            let line = line.map_err(|error| format!("read stateful ASR worker stdin: {error}"))?;
            if line.trim().is_empty() {
                continue;
            }
            let input: WorkerInput = match serde_json::from_str(&line) {
                Ok(input) => input,
                Err(error) => {
                    write_worker_output(&WorkerOutput::Error {
                        message: format!("invalid stateful ASR worker request: {error}"),
                    })?;
                    continue;
                }
            };
            match input {
                WorkerInput::Audio { data } => {
                    let pcm = match base64::engine::general_purpose::STANDARD.decode(data) {
                        Ok(pcm) => pcm,
                        Err(error) => {
                            write_worker_output(&WorkerOutput::Error {
                                message: format!("invalid stateful ASR worker audio: {error}"),
                            })?;
                            continue;
                        }
                    };
                    let samples = pcm16le_to_f32(&pcm)?;
                    accumulated_samples += samples.len();
                    let inference_started = Instant::now();
                    let result = engine
                        .feed_audio(&mut state, &samples)
                        .map_err(|error| error.to_string())?;
                    match result {
                        Some(result) => {
                            last_text.clone_from(&result.text);
                            write_worker_output(&WorkerOutput::Partial {
                                text: result.text,
                                language: result.language,
                                inference_ms: elapsed_millis_u64(inference_started),
                            })?;
                        }
                        None => write_worker_output(&WorkerOutput::Empty)?,
                    }

                    // Auto-reset state to prevent decoder slowdown on long sessions
                    if accumulated_samples >= MAX_SEGMENT_SAMPLES {
                        // Finalize current segment silently (we already emitted partials)
                        let _ = engine.finish_streaming(&mut state);
                        // Reinit with last transcript as context for continuity
                        let mut new_options = StreamingOptions::default()
                            .with_chunk_size_sec(chunk_size_sec)
                            .with_language(&language);
                        if !last_text.is_empty() {
                            new_options = new_options.with_initial_text(&last_text);
                        }
                        state = engine.init_streaming(new_options);
                        accumulated_samples = 0;
                    }
                }
                WorkerInput::Reset => {
                    // Reset streaming state without exiting — used for silence commits
                    let inference_started = Instant::now();
                    let result = engine
                        .finish_streaming(&mut state)
                        .map_err(|error| error.to_string())?;
                    write_worker_output(&WorkerOutput::Final {
                        text: result.text.clone(),
                        language: result.language.clone(),
                        inference_ms: elapsed_millis_u64(inference_started),
                    })?;
                    // Reinit state for next utterance
                    let mut new_options = StreamingOptions::default()
                        .with_chunk_size_sec(chunk_size_sec)
                        .with_language(&language);
                    if !result.text.is_empty() {
                        last_text = result.text;
                    }
                    if !last_text.is_empty() {
                        new_options = new_options.with_initial_text(&last_text);
                    }
                    state = engine.init_streaming(new_options);
                    accumulated_samples = 0;
                }
                WorkerInput::Finish => {
                    let inference_started = Instant::now();
                    let result = engine
                        .finish_streaming(&mut state)
                        .map_err(|error| error.to_string())?;
                    write_worker_output(&WorkerOutput::Final {
                        text: result.text,
                        language: result.language,
                        inference_ms: elapsed_millis_u64(inference_started),
                    })?;
                    break;
                }
            }
        }
        Ok(())
    }

    fn write_worker_output(output: &WorkerOutput) -> Result<(), String> {
        let text = serde_json::to_string(output)
            .map_err(|error| format!("serialize stateful ASR worker output: {error}"))?;
        let mut stdout = io::stdout().lock();
        writeln!(stdout, "{text}").map_err(|error| format!("write worker stdout: {error}"))?;
        stdout
            .flush()
            .map_err(|error| format!("flush worker stdout: {error}"))
    }

    fn elapsed_millis_u64(started: Instant) -> u64 {
        u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    fn warm_stateful_engine(engine: &AsrInference) {
        let options = StreamingOptions::default()
            .with_chunk_size_sec(0.5)
            .with_language("english");
        let mut state = engine.init_streaming(options);
        let silence = vec![0.0f32; 8_000];
        if let Err(error) = engine.feed_audio(&mut state, &silence) {
            warn!("stateful ASR warm-up feed failed: {}", error);
        }
    }

    fn pcm16le_to_f32(pcm: &[u8]) -> Result<Vec<f32>, String> {
        if !pcm.len().is_multiple_of(2) {
            return Err("stateful ASR PCM frame must contain whole i16 samples".to_string());
        }
        Ok(pcm
            .chunks_exact(2)
            .map(|chunk| i16::from_le_bytes([chunk[0], chunk[1]]) as f32 / i16::MAX as f32)
            .collect())
    }
}

#[cfg(not(target_os = "macos"))]
mod platform {
    use super::*;

    pub(super) async fn start_process_stateful_voice_session(
        config: StatefulVoiceConfig,
    ) -> Result<ProcessStatefulVoiceSession, String> {
        let StatefulVoiceConfig {
            model,
            model_dir,
            language,
            chunk_size_sec,
            initial_text,
        } = config;
        let _ = (model, model_dir, language, chunk_size_sec, initial_text);
        Err("qwen3 stateful streaming is currently enabled only on macOS Metal builds".to_string())
    }

    pub fn run_stateful_worker_stdio(_config: StatefulVoiceConfig) -> Result<(), String> {
        Err(
            "qwen3 stateful streaming worker is currently enabled only on macOS Metal builds"
                .to_string(),
        )
    }
}

pub use platform::run_stateful_worker_stdio;

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    async fn hung_process_session(script: &str) -> ProcessStatefulVoiceSession {
        let mut child = Command::new("sh")
            .arg("-c")
            .arg(script)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .expect("spawn hung voice worker test process");
        let stdin = child.stdin.take().expect("test worker stdin");
        let stdout = child.stdout.take().expect("test worker stdout");
        ProcessStatefulVoiceSession {
            child,
            stdin,
            stdout: BufReader::new(stdout).lines(),
            request_timeout: Duration::from_millis(50),
        }
    }

    #[tokio::test]
    async fn hung_worker_feed_times_out_and_kills_child() {
        let mut session = hung_process_session("while IFS= read -r _line; do sleep 60; done").await;
        let err = session.feed_pcm16(&[1, 0, 2, 0]).await.unwrap_err();
        assert!(err.contains("audio response timed out"), "{err}");
        assert!(err.contains("worker unloaded"), "{err}");
        tokio::time::timeout(Duration::from_secs(2), session.child.wait())
            .await
            .expect("hung worker should be killed after feed timeout")
            .expect("wait on killed worker");
    }

    #[tokio::test]
    async fn hung_worker_reset_times_out_and_kills_child() {
        let mut session = hung_process_session("while IFS= read -r _line; do sleep 60; done").await;
        let err = session.reset().await.unwrap_err();
        assert!(err.contains("reset response timed out"), "{err}");
        assert!(err.contains("worker unloaded"), "{err}");
        tokio::time::timeout(Duration::from_secs(2), session.child.wait())
            .await
            .expect("hung worker should be killed after reset timeout")
            .expect("wait on killed worker");
    }

    #[tokio::test]
    async fn hung_worker_startup_read_times_out_and_kills_child() {
        let mut session = hung_process_session("sleep 60").await;
        let err = session
            .read_worker_output_with_timeout("startup response", Duration::from_millis(50))
            .await
            .unwrap_err();
        assert!(err.contains("startup response timed out"), "{err}");
        assert!(err.contains("worker unloaded"), "{err}");
        tokio::time::timeout(Duration::from_secs(2), session.child.wait())
            .await
            .expect("hung worker should be killed after startup timeout")
            .expect("wait on killed worker");
    }

    #[tokio::test]
    async fn worker_stdout_log_lines_are_ignored_before_json_response() {
        let mut session = hung_process_session(
            "printf '\\033[2m2026-05-21T16:26:10Z\\033[0m \\033[32m INFO\\033[0m qwen3_asr: Using Metal device\\n'; printf '{\"type\":\"ready\"}\\n'; sleep 60",
        )
        .await;
        let output = session
            .read_worker_output_with_timeout("startup response", Duration::from_secs(1))
            .await
            .expect("worker log line should not corrupt JSON protocol");

        assert!(matches!(output, WorkerOutput::Ready));
    }
}
