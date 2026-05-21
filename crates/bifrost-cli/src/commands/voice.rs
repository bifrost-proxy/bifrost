use std::fs;
use std::io::Read;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::thread;

use base64::Engine as _;
use bifrost_admin::voice::{
    apply_voice_vocabulary, discover_voice_sources, load_voice_vocabulary,
    run_stateful_worker_stdio, save_voice_vocabulary, StatefulVoiceConfig, VoiceVocabulary,
    VoiceVocabularyTerm,
};
use bifrost_core::{BifrostError, Result};
use futures::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::protocol::Message;

use crate::cli::{AiVoiceCommands, AiVoiceVocabularyCommands};

pub(super) fn handle_voice_command(
    action: AiVoiceCommands,
    admin_host: &str,
    admin_port: u16,
) -> Result<()> {
    match action {
        AiVoiceCommands::Sources { json } => print_sources(json),
        AiVoiceCommands::Listen {
            source,
            app,
            input_file,
            duration,
            chunk_ms,
            model,
            provider,
            language,
            format,
            allow_stateful_large_model,
            dry_run,
            text,
        } => listen_voice(VoiceListenArgs {
            source: &source,
            app: app.as_deref(),
            input_file: input_file.as_deref(),
            duration,
            chunk_ms,
            model: &model,
            provider: &provider,
            language: &language,
            format: &format,
            allow_stateful_large_model,
            dry_run,
            text: &text,
            admin_host,
            admin_port,
        }),
        AiVoiceCommands::Worker {
            model,
            model_dir,
            language,
            chunk_size_sec,
            initial_text,
        } => run_stateful_worker_stdio(StatefulVoiceConfig {
            model,
            model_dir,
            language,
            chunk_size_sec,
            initial_text,
        })
        .map_err(BifrostError::Config),
        AiVoiceCommands::Vocabulary { action } => handle_vocabulary_command(action),
    }
}

fn print_sources(json: bool) -> Result<()> {
    let sources = discover_voice_sources();
    if json {
        let body = serde_json::json!({
            "platform": std::env::consts::OS,
            "sources": sources,
        });
        println!("{}", serde_json::to_string_pretty(&body).unwrap());
        return Ok(());
    }

    println!("Voice input sources (local-only):");
    for source in sources {
        let status = serde_json::to_string(&source.status)
            .unwrap_or_else(|_| "\"unknown\"".to_string())
            .trim_matches('"')
            .to_string();
        match source.reason {
            Some(reason) => println!(
                "  {} [{}] - {} ({})",
                source.id, source.kind, status, reason
            ),
            None => println!("  {} [{}] - {}", source.id, source.kind, status),
        }
    }
    Ok(())
}

#[derive(Clone, Copy)]
struct VoiceListenArgs<'a> {
    source: &'a str,
    app: Option<&'a str>,
    input_file: Option<&'a Path>,
    duration: u64,
    chunk_ms: u64,
    model: &'a str,
    provider: &'a str,
    language: &'a str,
    format: &'a str,
    allow_stateful_large_model: bool,
    dry_run: bool,
    text: &'a str,
    admin_host: &'a str,
    admin_port: u16,
}

fn listen_voice(args: VoiceListenArgs<'_>) -> Result<()> {
    if args.dry_run {
        return emit_dry_run_events(args.source, args.app, args.format, args.text);
    }

    match args.source {
        "mic" | "file" => listen_streaming_source(args),
        "system" => Err(BifrostError::Config(
            "system audio capture is not enabled in Voice Input Runtime V1; run `bifrost ai voice sources --json` for capability status".to_string(),
        )),
        "app" => Err(BifrostError::Config(format!(
            "application audio capture is not enabled in Voice Input Runtime V1{}; run `bifrost ai voice sources --json` for capability status",
            args.app.map(|value| format!(" for {value}")).unwrap_or_default()
        ))),
        other => Err(BifrostError::Config(format!(
            "unsupported voice source: {other}"
        ))),
    }
}

fn emit_dry_run_events(source: &str, app: Option<&str>, format: &str, text: &str) -> Result<()> {
    let vocabulary = load_voice_vocabulary().unwrap_or_default();
    let refined = apply_voice_vocabulary(text, &vocabulary);
    if format == "text" {
        println!("{refined}");
        return Ok(());
    }
    let source_id = if let Some(app) = app {
        format!("{source}:{app}")
    } else {
        source.to_string()
    };
    let events = [
        serde_json::json!({
            "type": "source_ready",
            "source": source_id,
            "local_only": true,
        }),
        serde_json::json!({
            "type": "asr_partial",
            "text": text,
            "raw_text": text,
            "window_start_ms": 0,
            "window_end_ms": 1000,
        }),
        serde_json::json!({
            "type": "asr_final_utterance",
            "text": refined,
            "raw_text": text,
            "window_start_ms": 0,
            "window_end_ms": 1000,
        }),
        serde_json::json!({
            "type": "done",
        }),
    ];
    for event in events {
        println!("{}", serde_json::to_string(&event).unwrap());
    }
    Ok(())
}

const STREAM_SAMPLE_RATE: u32 = 16_000;
const STREAM_CHANNELS: u16 = 1;
const STREAM_BYTES_PER_SAMPLE: usize = 2;
const MIN_STREAM_CHUNK_MS: u64 = 500;
const MAX_STREAM_CHUNK_MS: u64 = 4_000;

fn listen_streaming_source(args: VoiceListenArgs<'_>) -> Result<()> {
    if args.duration == 0 || args.duration > 600 {
        return Err(BifrostError::Config(
            "--duration must be between 1 and 600 seconds".to_string(),
        ));
    }
    let chunk_ms = args
        .chunk_ms
        .clamp(MIN_STREAM_CHUNK_MS, MAX_STREAM_CHUNK_MS);
    let runtime = tokio::runtime::Runtime::new().map_err(|error| {
        BifrostError::Io(std::io::Error::other(format!(
            "create voice streaming runtime: {error}"
        )))
    })?;
    runtime.block_on(stream_source_to_voice_service(args, chunk_ms))
}

async fn stream_source_to_voice_service(args: VoiceListenArgs<'_>, chunk_ms: u64) -> Result<()> {
    let url = voice_ws_url(args, chunk_ms);
    let (ws, _) = tokio_tungstenite::connect_async(&url)
        .await
        .map_err(|error| {
            BifrostError::Config(format!(
                "connect Voice Input Runtime at {url}: {error}. Start Bifrost first with `bifrost start --no-system-proxy`."
            ))
        })?;
    let (mut sink, mut stream) = ws.split();
    sink.send(Message::Text(
        serde_json::json!({
            "type": "start",
            "source": args.source,
            "sample_rate": STREAM_SAMPLE_RATE,
            "channels": STREAM_CHANNELS,
            "format": "pcm_s16le",
        })
        .to_string()
        .into(),
    ))
    .await
    .map_err(|error| BifrostError::Config(format!("send voice start frame: {error}")))?;

    let mut ready = false;
    while let Some(message) = stream.next().await {
        let message =
            message.map_err(|error| BifrostError::Config(format!("read voice event: {error}")))?;
        if let Message::Text(text) = message {
            let should_stop = print_voice_service_event(args.format, &text)?;
            let value: serde_json::Value = serde_json::from_str(&text)
                .map_err(|error| BifrostError::Config(format!("parse voice event: {error}")))?;
            if value.get("type").and_then(|value| value.as_str()) == Some("source_ready") {
                ready = true;
                break;
            }
            if should_stop {
                break;
            }
        }
    }
    if !ready {
        return Err(BifrostError::Config(
            "Voice Input Runtime closed before source became ready".to_string(),
        ));
    }

    let mut child = spawn_stream_source(args.source, args.input_file, args.duration)?;
    let stdout = child.stdout.take().ok_or_else(|| {
        BifrostError::Config("voice capture process did not expose stdout".to_string())
    })?;

    let (tx, mut rx) = tokio::sync::mpsc::channel::<Vec<u8>>(16);
    let reader = thread::spawn(move || read_pcm_chunks(stdout, tx));
    let sender = tokio::spawn(async move {
        let mut sequence = 0u64;
        while let Some(chunk) = rx.recv().await {
            sequence += 1;
            let duration_ms = pcm_duration_ms(chunk.len());
            let payload = serde_json::json!({
                "type": "audio",
                "sequence": sequence,
                "duration_ms": duration_ms,
                "data": base64::engine::general_purpose::STANDARD.encode(chunk),
            });
            sink.send(Message::Text(payload.to_string().into()))
                .await
                .map_err(|error| format!("send voice audio frame: {error}"))?;
        }
        sink.send(Message::Text(
            serde_json::json!({"type": "finish"}).to_string().into(),
        ))
        .await
        .map_err(|error| format!("send voice finish frame: {error}"))
    });

    while let Some(message) = stream.next().await {
        let message =
            message.map_err(|error| BifrostError::Config(format!("read voice event: {error}")))?;
        match message {
            Message::Text(text) => {
                let should_stop = print_voice_service_event(args.format, &text)?;
                if should_stop {
                    break;
                }
            }
            Message::Close(_) => break,
            Message::Ping(_) | Message::Pong(_) | Message::Binary(_) | Message::Frame(_) => {}
        }
    }

    sender
        .await
        .map_err(|error| BifrostError::Config(format!("voice sender task failed: {error}")))?
        .map_err(BifrostError::Config)?;
    reader
        .join()
        .map_err(|_| BifrostError::Config("voice PCM reader thread panicked".to_string()))??;
    wait_for_capture_exit(child)?;
    Ok(())
}

fn voice_ws_url(args: VoiceListenArgs<'_>, chunk_ms: u64) -> String {
    let chunk_size_sec = format!("{:.3}", chunk_ms as f32 / 1000.0);
    let mut url = format!(
        "ws://{}:{}/_bifrost/api/voice/listen-ws?source={}&stateful_chunk_sec={}&model={}&provider={}&language={}",
        args.admin_host,
        args.admin_port,
        urlencoding::encode(args.source),
        chunk_size_sec,
        urlencoding::encode(args.model),
        urlencoding::encode(args.provider),
        urlencoding::encode(args.language)
    );
    if args.allow_stateful_large_model {
        url.push_str("&allow_stateful_17b=1");
    }
    url
}

fn read_pcm_chunks<R: Read>(mut stdout: R, tx: tokio::sync::mpsc::Sender<Vec<u8>>) -> Result<()> {
    let mut buffer = [0u8; 8192];
    loop {
        let n = stdout.read(&mut buffer).map_err(BifrostError::Io)?;
        if n == 0 {
            break;
        }
        if tx.blocking_send(buffer[..n].to_vec()).is_err() {
            break;
        }
    }
    Ok(())
}

fn pcm_duration_ms(bytes: usize) -> u64 {
    let bytes_per_ms =
        STREAM_SAMPLE_RATE as usize * STREAM_CHANNELS as usize * STREAM_BYTES_PER_SAMPLE / 1000;
    (bytes / bytes_per_ms) as u64
}

fn print_voice_service_event(format: &str, text: &str) -> Result<bool> {
    if format == "jsonl" {
        println!("{text}");
    } else {
        let value: serde_json::Value = serde_json::from_str(text)
            .map_err(|error| BifrostError::Config(format!("parse voice event: {error}")))?;
        if let Some(delta) = value.get("delta").and_then(|value| value.as_str()) {
            if !delta.trim().is_empty() {
                println!("{delta}");
            }
        }
    }
    let value: serde_json::Value = serde_json::from_str(text)
        .map_err(|error| BifrostError::Config(format!("parse voice event: {error}")))?;
    if value.get("type").and_then(|value| value.as_str()) == Some("error") {
        let message = value
            .get("message")
            .and_then(|value| value.as_str())
            .unwrap_or("voice streaming service returned an error");
        let detail = value.get("detail").and_then(|value| value.as_str());
        return Err(BifrostError::Config(match detail {
            Some(detail) => format!("{message}: {detail}"),
            None => message.to_string(),
        }));
    }
    Ok(value.get("type").and_then(|value| value.as_str()) == Some("done"))
}

fn spawn_stream_source(source: &str, input_file: Option<&Path>, duration: u64) -> Result<Child> {
    match source {
        "mic" => spawn_macos_default_mic_stream(duration),
        "file" => {
            let input = input_file.ok_or_else(|| {
                BifrostError::Config("--input-file is required when --source file".to_string())
            })?;
            if !input.is_file() {
                return Err(BifrostError::Config(format!(
                    "input audio file does not exist: {}",
                    input.display()
                )));
            }
            spawn_file_stream(input, duration)
        }
        other => Err(BifrostError::Config(format!(
            "unsupported streaming voice source: {other}"
        ))),
    }
}

fn spawn_file_stream(input: &Path, duration: u64) -> Result<Child> {
    Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-re"])
        .arg("-i")
        .arg(input)
        .arg("-t")
        .arg(duration.to_string())
        .args([
            "-ar",
            &STREAM_SAMPLE_RATE.to_string(),
            "-ac",
            &STREAM_CHANNELS.to_string(),
            "-f",
            "s16le",
            "pipe:1",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| {
            BifrostError::Io(std::io::Error::other(format!(
                "failed to start ffmpeg realtime file stream: {error}"
            )))
        })
}

#[cfg(target_os = "macos")]
fn spawn_macos_default_mic_stream(duration: u64) -> Result<Child> {
    Command::new("ffmpeg")
        .arg("-hide_banner")
        .arg("-loglevel")
        .arg("error")
        .arg("-f")
        .arg("avfoundation")
        .arg("-i")
        .arg(":0")
        .arg("-t")
        .arg(duration.to_string())
        .arg("-ar")
        .arg(STREAM_SAMPLE_RATE.to_string())
        .arg("-ac")
        .arg(STREAM_CHANNELS.to_string())
        .arg("-f")
        .arg("s16le")
        .arg("pipe:1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| {
            BifrostError::Io(std::io::Error::other(format!(
                "failed to run ffmpeg for microphone capture. Install it with `brew install ffmpeg`: {error}"
            )))
        })
}

#[cfg(not(target_os = "macos"))]
fn spawn_macos_default_mic_stream(_duration: u64) -> Result<Child> {
    Err(BifrostError::Config(
        "native microphone capture is currently implemented only for macOS; use --source file for automated stream tests".to_string(),
    ))
}

fn wait_for_capture_exit(child: Child) -> Result<()> {
    let output = child.wait_with_output().map_err(BifrostError::Io)?;
    if output.status.success() {
        return Ok(());
    }
    Err(BifrostError::Config(format!(
        "voice capture process failed with status {}: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    )))
}

fn handle_vocabulary_command(action: AiVoiceVocabularyCommands) -> Result<()> {
    match action {
        AiVoiceVocabularyCommands::List { json } => {
            let vocabulary = load_voice_vocabulary().map_err(BifrostError::Config)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&vocabulary).unwrap());
            } else if vocabulary.terms.is_empty() {
                println!("Voice vocabulary is empty.");
            } else {
                for term in vocabulary.terms {
                    println!("{} = {}", term.canonical, term.aliases.join(", "));
                }
            }
            Ok(())
        }
        AiVoiceVocabularyCommands::Import { file } => {
            let terms = parse_terms_file(&file)?;
            let vocabulary = VoiceVocabulary { version: 1, terms };
            save_voice_vocabulary(&vocabulary).map_err(BifrostError::Config)?;
            println!(
                "Imported {} voice vocabulary terms.",
                vocabulary.terms.len()
            );
            Ok(())
        }
    }
}

fn parse_terms_file(path: &Path) -> Result<Vec<VoiceVocabularyTerm>> {
    let text = fs::read_to_string(path).map_err(BifrostError::Io)?;
    let mut terms = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((canonical, aliases)) = line.split_once('=') else {
            return Err(BifrostError::Config(format!(
                "invalid vocabulary line {}: expected canonical=alias1,alias2",
                index + 1
            )));
        };
        let canonical = canonical.trim();
        if canonical.is_empty() {
            return Err(BifrostError::Config(format!(
                "invalid vocabulary line {}: canonical term is empty",
                index + 1
            )));
        }
        let aliases: Vec<String> = aliases
            .split(',')
            .map(str::trim)
            .filter(|alias| !alias.is_empty())
            .map(ToString::to_string)
            .collect();
        if aliases.is_empty() {
            return Err(BifrostError::Config(format!(
                "invalid vocabulary line {}: at least one alias is required",
                index + 1
            )));
        }
        terms.push(VoiceVocabularyTerm {
            canonical: canonical.to_string(),
            aliases,
            category: None,
        });
    }
    Ok(terms)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_terms_file_rejects_missing_aliases() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("terms.txt");
        fs::write(&path, "Bifrost=\n").unwrap();
        let error = parse_terms_file(&path).unwrap_err().to_string();
        assert!(error.contains("at least one alias"));
    }

    #[test]
    fn parse_terms_file_accepts_comments_and_aliases() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("terms.txt");
        fs::write(&path, "# project terms\nBifrost=宽增,白 Frost\n").unwrap();
        let terms = parse_terms_file(&path).unwrap();
        assert_eq!(terms.len(), 1);
        assert_eq!(terms[0].canonical, "Bifrost");
        assert_eq!(terms[0].aliases, vec!["宽增", "白 Frost"]);
    }

    #[test]
    fn voice_ws_url_includes_runtime_chunk_options() {
        let args = VoiceListenArgs {
            source: "file",
            app: None,
            input_file: None,
            duration: 7,
            chunk_ms: 2_000,
            model: "Qwen3-ASR-0.6B",
            provider: "qwen3_stateful_streaming",
            language: "chinese",
            format: "jsonl",
            allow_stateful_large_model: false,
            dry_run: false,
            text: "",
            admin_host: "127.0.0.1",
            admin_port: 18887,
        };
        let url = voice_ws_url(args, 2_000);
        assert!(url.contains("/_bifrost/api/voice/listen-ws"));
        assert!(url.contains("source=file"));
        assert!(url.contains("stateful_chunk_sec=2.000"));
        assert!(url.contains("model=Qwen3-ASR-0.6B"));
        assert!(url.contains("provider=qwen3_stateful_streaming"));
        assert!(url.contains("language=chinese"));
    }

    #[test]
    fn voice_ws_url_can_opt_into_large_stateful_model() {
        let mut args = VoiceListenArgs {
            source: "file",
            app: None,
            input_file: None,
            duration: 7,
            chunk_ms: 1_000,
            model: "Qwen3-ASR-0.6B",
            provider: "qwen3_stateful_streaming",
            language: "chinese",
            format: "jsonl",
            allow_stateful_large_model: false,
            dry_run: false,
            text: "",
            admin_host: "127.0.0.1",
            admin_port: 18887,
        };
        assert!(!voice_ws_url(args, 1_000).contains("allow_stateful_17b=1"));

        args.allow_stateful_large_model = true;
        let url = voice_ws_url(args, 1_000);
        assert!(url.contains("allow_stateful_17b=1"));
    }
}
