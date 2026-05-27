use std::fs;
use std::io::{self, IsTerminal, Read, Write};
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
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::terminal;
use futures::{SinkExt, StreamExt};
use serde_json::Value;
use tokio_tungstenite::tungstenite::protocol::Message;

use super::asr::AsrTaskClient;
use super::voice_wake_worker::run_wake_worker;
use crate::cli::{
    AiVoiceCommands, AiVoiceVocabularyCommands, AiVoiceWakeBindingCommands, AiVoiceWakeCommands,
    AiVoiceWakeListenerCommands, AiVoiceWakeProfileCommands,
};

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
        AiVoiceCommands::Wake { action } => {
            let client = AsrTaskClient::new(admin_host, admin_port);
            handle_wake_command(&client, action)
        }
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
        "ws://{}:{}/_bifrost/api/voice/listen-ws?source={}&stateful_chunk_sec={}&model={}&provider={}&language={}&owner_module=cli",
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

fn handle_wake_command(client: &AsrTaskClient, action: AiVoiceWakeCommands) -> Result<()> {
    match action {
        AiVoiceWakeCommands::Status { json } => {
            let value = client.get_json("/voice/wake/status")?;
            if json {
                print_json(&value)
            } else {
                println!(
                    "Voice wake: enabled={}, profiles={}, bindings={}, events={}",
                    value["enabled"].as_bool().unwrap_or(false),
                    value["profile_count"].as_u64().unwrap_or(0),
                    value["binding_count"].as_u64().unwrap_or(0),
                    value["event_count"].as_u64().unwrap_or(0)
                );
                if let Some(path) = value["store_path"].as_str() {
                    println!("Store: {path}");
                }
                Ok(())
            }
        }
        AiVoiceWakeCommands::Profile { action } => handle_wake_profile_command(client, action),
        AiVoiceWakeCommands::Binding { action } => handle_wake_binding_command(client, action),
        AiVoiceWakeCommands::Listener { action } => handle_wake_listener_command(client, action),
        AiVoiceWakeCommands::BindAudio {
            audio,
            voiceprint_profile_id,
            name,
            profile_id,
            binding_id,
            phrase,
            model,
            language,
            key,
            modifiers,
            press_count,
            speaker_threshold,
            cooldown_ms,
            json,
        } => handle_wake_bind_audio_command(
            client,
            WakeBindAudioArgs {
                audio: &audio,
                voiceprint_profile_id: &voiceprint_profile_id,
                name: name.as_deref(),
                profile_id,
                binding_id,
                phrase,
                model: &model,
                language: &language,
                key,
                modifiers,
                press_count,
                speaker_threshold,
                cooldown_ms,
                json,
            },
        ),
        AiVoiceWakeCommands::Trigger {
            phrase,
            profile,
            speaker_confidence,
            execute,
            json,
        } => {
            let body = serde_json::json!({
                "phrase": phrase,
                "profile_id": profile,
                "speaker_confidence": speaker_confidence,
                "dry_run": !execute,
            });
            let value = client.post_json_body("/voice/wake/trigger", &body)?;
            if json {
                print_json(&value)
            } else {
                print_trigger_result(&value);
                Ok(())
            }
        }
        AiVoiceWakeCommands::Worker {
            admin_host,
            admin_port,
            device,
            chunk_ms,
            dry_run,
        } => {
            let worker_client = AsrTaskClient::new(&admin_host, admin_port);
            run_wake_worker(&worker_client, device.as_deref(), chunk_ms, !dry_run)
        }
    }
}

fn handle_wake_listener_command(
    client: &AsrTaskClient,
    action: AiVoiceWakeListenerCommands,
) -> Result<()> {
    match action {
        AiVoiceWakeListenerCommands::Start {
            source,
            device,
            chunk_ms,
            dry_run,
            mock_transcripts,
            mock_interval_ms,
            mock_speaker_profile_id,
            mock_speaker_confidence,
            json,
        } => {
            let body = serde_json::json!({
                "source": source,
                "device": device,
                "chunk_ms": chunk_ms,
                "execute": !dry_run,
                "mock_transcripts": mock_transcripts,
                "mock_interval_ms": mock_interval_ms,
                "mock_speaker_profile_id": mock_speaker_profile_id,
                "mock_speaker_confidence": mock_speaker_confidence,
            });
            let value = client.post_json_body("/voice/wake/listener/start", &body)?;
            if json {
                print_json(&value)
            } else {
                let listener = &value["listener"];
                println!(
                    "Voice wake listener started: source={}, chunk_ms={}, execute={}",
                    listener["source"].as_str().unwrap_or("-"),
                    listener["chunk_ms"].as_u64().unwrap_or(0),
                    !dry_run
                );
                Ok(())
            }
        }
        AiVoiceWakeListenerCommands::Stop { json } => {
            let value = client.post_json("/voice/wake/listener/stop")?;
            if json {
                print_json(&value)
            } else {
                println!("Voice wake listener stopped.");
                Ok(())
            }
        }
    }
}

struct WakeBindAudioArgs<'a> {
    audio: &'a Path,
    voiceprint_profile_id: &'a str,
    name: Option<&'a str>,
    profile_id: Option<String>,
    binding_id: Option<String>,
    phrase: Option<String>,
    model: &'a str,
    language: &'a str,
    key: Option<String>,
    modifiers: Vec<String>,
    press_count: u8,
    speaker_threshold: Option<f32>,
    cooldown_ms: Option<u64>,
    json: bool,
}

fn handle_wake_bind_audio_command(
    client: &AsrTaskClient,
    args: WakeBindAudioArgs<'_>,
) -> Result<()> {
    if !args.audio.is_file() {
        return Err(BifrostError::Config(format!(
            "audio file does not exist: {}",
            args.audio.display()
        )));
    }
    let phrase = match args.phrase {
        Some(phrase) if !phrase.trim().is_empty() => phrase.trim().to_string(),
        Some(_) => {
            return Err(BifrostError::Config(
                "recognized phrase override cannot be empty".to_string(),
            ))
        }
        None => recognize_wake_phrase_from_audio(client, args.audio, args.model, args.language)?,
    };
    if phrase.trim().is_empty() {
        return Err(BifrostError::Config(
            "ASR did not recognize a wake phrase from the audio sample".to_string(),
        ));
    }
    let (key, modifiers) = match args.key {
        Some(key) => (key, args.modifiers),
        None => capture_shortcut_from_terminal()?,
    };
    let profile_body = serde_json::json!({
        "id": args.profile_id,
        "display_name": args.name.unwrap_or(args.voiceprint_profile_id),
        "voiceprint_profile_id": args.voiceprint_profile_id,
        "speaker_threshold": args.speaker_threshold,
    });
    let profile = client.post_json_body("/voice/wake/profiles", &profile_body)?;
    let binding_body = serde_json::json!({
        "id": args.binding_id,
        "phrase": phrase,
        "profile_id": profile["id"].as_str().unwrap_or_default(),
        "speaker_threshold": args.speaker_threshold,
        "cooldown_ms": args.cooldown_ms,
        "action": {
            "type": "key_press",
            "key": key,
            "keycode": null,
            "modifiers": modifiers,
            "press_count": args.press_count,
        },
    });
    let binding = client.post_json_body("/voice/wake/bindings", &binding_body)?;
    let output = serde_json::json!({
        "phrase": binding["phrase"],
        "profile": profile,
        "binding": binding,
    });
    if args.json {
        print_json(&output)
    } else {
        println!(
            "Created voice wake binding {}: '{}' -> {}",
            binding["id"].as_str().unwrap_or("-"),
            binding["phrase"].as_str().unwrap_or("-"),
            format_wake_action(&binding["action"])
        );
        Ok(())
    }
}

pub(super) fn recognize_wake_phrase_from_audio(
    client: &AsrTaskClient,
    audio: &Path,
    model: &str,
    language: &str,
) -> Result<String> {
    let start_path = format!(
        "/asr/service/start?model={}&language={}&owner_module=voice_wake_cli",
        url_encode(model),
        url_encode(language)
    );
    let service = client.post_json(&start_path)?;
    if !service["ready"].as_bool().unwrap_or(false) {
        return Err(BifrostError::Config(
            service["detail"]
                .as_str()
                .or_else(|| service["message"].as_str())
                .unwrap_or("ASR service is not ready")
                .to_string(),
        ));
    }
    let file_name = audio
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("wake-audio");
    let bytes = fs::read(audio).map_err(|error| {
        BifrostError::Io(io::Error::other(format!(
            "read wake audio {}: {error}",
            audio.display()
        )))
    })?;
    let (content_type, body) = build_multipart_upload(file_name, &bytes);
    let url = format!(
        "{}/asr/transcribe-stream?model={}&language={}&owner_module=voice_wake_cli",
        client.base_url,
        url_encode(model),
        url_encode(language)
    );
    let response = client
        .agent
        .post(&url)
        .set("content-type", &content_type)
        .set("accept", "text/event-stream")
        .send_bytes(&body)
        .map_err(|error| voice_wake_api_error("POST", &url, error))?;
    let stream = response.into_string().map_err(|error| {
        BifrostError::Io(io::Error::other(format!(
            "read ASR wake audio response from {url}: {error}"
        )))
    })?;
    extract_wake_phrase_from_sse(&stream).ok_or_else(|| {
        BifrostError::Config("ASR response did not include recognized wake text".to_string())
    })
}

struct RawModeGuard;

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        let _ = terminal::disable_raw_mode();
    }
}

fn capture_shortcut_from_terminal() -> Result<(String, Vec<String>)> {
    if !std::io::stdin().is_terminal() {
        return Err(BifrostError::Config(
            "pass --key/--modifiers in non-interactive terminals".to_string(),
        ));
    }
    eprint!("Press shortcut to bind: ");
    io::stderr().flush().ok();
    terminal::enable_raw_mode().map_err(BifrostError::Io)?;
    let _raw_mode = RawModeGuard;
    let result = loop {
        match event::read().map_err(BifrostError::Io)? {
            Event::Key(key) if key.kind == KeyEventKind::Press => {
                if let Some(parsed) = shortcut_from_key_event(key) {
                    break Ok(parsed);
                }
            }
            _ => {}
        }
    };
    if let Ok((key, modifiers)) = &result {
        eprintln!("{}", format_shortcut(key, modifiers));
    }
    result
}

fn build_multipart_upload(file_name: &str, bytes: &[u8]) -> (String, Vec<u8>) {
    let boundary = format!("bifrost-wake-audio-{}", uuid::Uuid::new_v4().as_simple());
    let mut body = Vec::new();
    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(
        format!(
            "Content-Disposition: form-data; name=\"file\"; filename=\"{}\"\r\n",
            sanitize_multipart_filename(file_name)
        )
        .as_bytes(),
    );
    body.extend_from_slice(b"Content-Type: application/octet-stream\r\n\r\n");
    body.extend_from_slice(bytes);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    (format!("multipart/form-data; boundary={boundary}"), body)
}

fn sanitize_multipart_filename(file_name: &str) -> String {
    file_name
        .chars()
        .map(|ch| match ch {
            '"' | '\r' | '\n' => '_',
            other => other,
        })
        .collect()
}

fn url_encode(value: &str) -> String {
    urlencoding::encode(value).into_owned()
}

fn voice_wake_api_error(method: &str, url: &str, error: ureq::Error) -> BifrostError {
    match error {
        ureq::Error::Status(status, response) => {
            let body = response.into_string().unwrap_or_default();
            BifrostError::Config(format!(
                "{method} {url} failed with HTTP {status}: {}",
                truncate(&body, 500)
            ))
        }
        other => BifrostError::Config(format!(
            "Failed to connect to Bifrost admin API at {url}. Start Bifrost first with `bifrost start --no-system-proxy`. Error: {other}"
        )),
    }
}

fn extract_wake_phrase_from_sse(stream: &str) -> Option<String> {
    let mut latest = None;
    for line in stream.lines() {
        let data = line
            .strip_prefix("data:")
            .map(str::trim)
            .unwrap_or_else(|| line.trim());
        if data.is_empty() || data == "[DONE]" {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(data) else {
            continue;
        };
        if let Some(text) = asr_text_candidate(&value) {
            latest = Some(clean_wake_phrase_candidate(&text));
        }
    }
    latest
}

fn asr_text_candidate(value: &Value) -> Option<String> {
    for key in [
        "text",
        "final_text",
        "transcript",
        "committed_text",
        "delta",
    ] {
        if let Some(text) = value.get(key).and_then(Value::as_str) {
            let text = text.trim();
            if !text.is_empty() {
                return Some(text.to_string());
            }
        }
    }
    if let Some(text) = value
        .get("result")
        .and_then(|result| result.get("text"))
        .and_then(Value::as_str)
    {
        let text = text.trim();
        if !text.is_empty() {
            return Some(text.to_string());
        }
    }
    None
}

fn clean_wake_phrase_candidate(value: &str) -> String {
    value
        .lines()
        .map(|line| {
            let trimmed = line.trim();
            trimmed
                .split_once(':')
                .or_else(|| trimmed.split_once('：'))
                .and_then(|(prefix, rest)| {
                    let prefix = prefix.trim();
                    let rest = rest.trim();
                    let looks_like_speaker = prefix.starts_with("用户")
                        || prefix.to_ascii_lowercase().starts_with("speaker")
                        || prefix.to_ascii_lowercase().starts_with("spk")
                        || (prefix.is_ascii()
                            && prefix.len() <= 64
                            && prefix
                                .chars()
                                .all(|ch| ch.is_ascii_alphanumeric() || ch.is_ascii_whitespace())
                            && !rest.is_empty());
                    looks_like_speaker.then_some(rest)
                })
                .unwrap_or(trimmed)
        })
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
        .trim()
        .to_string()
}

fn format_shortcut(key: &str, modifiers: &[String]) -> String {
    if modifiers.is_empty() {
        key.to_string()
    } else {
        format!("{}+{key}", modifiers.join("+"))
    }
}

fn shortcut_from_key_event(event: KeyEvent) -> Option<(String, Vec<String>)> {
    let key = match event.code {
        KeyCode::Char(' ') => "space".to_string(),
        KeyCode::Char(ch) => ch.to_ascii_lowercase().to_string(),
        KeyCode::Enter => "return".to_string(),
        KeyCode::Tab | KeyCode::BackTab => "tab".to_string(),
        KeyCode::Esc => "escape".to_string(),
        KeyCode::Left => "left".to_string(),
        KeyCode::Right => "right".to_string(),
        KeyCode::Up => "up".to_string(),
        KeyCode::Down => "down".to_string(),
        _ => return None,
    };
    let mut modifiers = Vec::new();
    if event.modifiers.contains(KeyModifiers::SUPER) || event.modifiers.contains(KeyModifiers::META)
    {
        modifiers.push("cmd".to_string());
    }
    if event.modifiers.contains(KeyModifiers::SHIFT) {
        modifiers.push("shift".to_string());
    }
    if event.modifiers.contains(KeyModifiers::CONTROL) {
        modifiers.push("ctrl".to_string());
    }
    if event.modifiers.contains(KeyModifiers::ALT) {
        modifiers.push("option".to_string());
    }
    Some((key, modifiers))
}

fn handle_wake_profile_command(
    client: &AsrTaskClient,
    action: AiVoiceWakeProfileCommands,
) -> Result<()> {
    match action {
        AiVoiceWakeProfileCommands::List { json } => {
            let value = client.get_json("/voice/wake/profiles")?;
            if json {
                print_json(&value)
            } else {
                let profiles = value["profiles"].as_array().cloned().unwrap_or_default();
                if profiles.is_empty() {
                    println!("No voice wake profiles.");
                } else {
                    println!(
                        "{:<40}  {:<24}  {:<24}  THRESHOLD",
                        "ID", "NAME", "VOICEPRINT"
                    );
                    for profile in profiles {
                        println!(
                            "{:<40}  {:<24}  {:<24}  {:.2}",
                            profile["id"].as_str().unwrap_or("-"),
                            truncate(profile["display_name"].as_str().unwrap_or("-"), 24),
                            truncate(profile["voiceprint_profile_id"].as_str().unwrap_or("-"), 24),
                            profile["speaker_threshold"].as_f64().unwrap_or(0.0)
                        );
                    }
                }
                Ok(())
            }
        }
        AiVoiceWakeProfileCommands::Add {
            id,
            name,
            voiceprint_profile_id,
            speaker_threshold,
            json,
        } => {
            let body = serde_json::json!({
                "id": id,
                "display_name": name,
                "voiceprint_profile_id": voiceprint_profile_id,
                "speaker_threshold": speaker_threshold,
            });
            let value = client.post_json_body("/voice/wake/profiles", &body)?;
            if json {
                print_json(&value)
            } else {
                println!(
                    "Created voice wake profile {} ({})",
                    value["id"].as_str().unwrap_or("-"),
                    value["display_name"].as_str().unwrap_or("-")
                );
                Ok(())
            }
        }
    }
}

fn handle_wake_binding_command(
    client: &AsrTaskClient,
    action: AiVoiceWakeBindingCommands,
) -> Result<()> {
    match action {
        AiVoiceWakeBindingCommands::List { json } => {
            let value = client.get_json("/voice/wake/bindings")?;
            if json {
                print_json(&value)
            } else {
                let bindings = value["bindings"].as_array().cloned().unwrap_or_default();
                if bindings.is_empty() {
                    println!("No voice wake bindings.");
                } else {
                    println!("{:<40}  {:<16}  {:<18}  ACTION", "ID", "PROFILE", "PHRASE");
                    for binding in bindings {
                        println!(
                            "{:<40}  {:<16}  {:<18}  {}",
                            binding["id"].as_str().unwrap_or("-"),
                            truncate(binding["profile_id"].as_str().unwrap_or("-"), 16),
                            truncate(binding["phrase"].as_str().unwrap_or("-"), 18),
                            format_wake_action(&binding["action"])
                        );
                    }
                }
                Ok(())
            }
        }
        AiVoiceWakeBindingCommands::Add {
            id,
            phrase,
            profile,
            key,
            keycode,
            modifiers,
            press_count,
            disabled,
            kws_score,
            kws_threshold,
            speaker_threshold,
            cooldown_ms,
            json,
        } => {
            let action = serde_json::json!({
                "type": "key_press",
                "key": if keycode.is_some() { None::<String> } else { Some(key) },
                "keycode": keycode,
                "modifiers": modifiers,
                "press_count": press_count,
            });
            let body = serde_json::json!({
                "id": id,
                "phrase": phrase,
                "profile_id": profile,
                "enabled": !disabled,
                "kws_score": kws_score,
                "kws_threshold": kws_threshold,
                "speaker_threshold": speaker_threshold,
                "cooldown_ms": cooldown_ms,
                "action": action,
            });
            let value = client.post_json_body("/voice/wake/bindings", &body)?;
            if json {
                print_json(&value)
            } else {
                println!(
                    "Created voice wake binding {}: '{}' -> {}",
                    value["id"].as_str().unwrap_or("-"),
                    value["phrase"].as_str().unwrap_or("-"),
                    format_wake_action(&value["action"])
                );
                Ok(())
            }
        }
    }
}

fn print_trigger_result(value: &Value) {
    let binding = &value["binding"];
    let result = &value["action_result"];
    println!(
        "Matched voice wake binding {} for phrase '{}'",
        binding["id"].as_str().unwrap_or("-"),
        binding["phrase"].as_str().unwrap_or("-")
    );
    println!(
        "Action: {} (dry_run={}, executed={})",
        format_wake_action(&binding["action"]),
        result["dry_run"].as_bool().unwrap_or(true),
        result["executed"].as_bool().unwrap_or(false)
    );
    if let Some(message) = result["message"].as_str() {
        println!("Result: {message}");
    }
}

fn format_wake_action(action: &Value) -> String {
    if action["type"].as_str() == Some("key_press") {
        let modifiers = action["modifiers"]
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.as_str())
                    .collect::<Vec<_>>()
                    .join("+")
            })
            .filter(|value| !value.is_empty())
            .map(|value| format!("{value}+"))
            .unwrap_or_default();
        if let Some(keycode) = action["keycode"].as_u64() {
            return format!("key_press {modifiers}keycode:{keycode}");
        }
        return format!(
            "key_press {modifiers}{}",
            action["key"].as_str().unwrap_or("-")
        );
    }
    action["type"].as_str().unwrap_or("unknown").to_string()
}

fn print_json(value: &Value) -> Result<()> {
    println!(
        "{}",
        serde_json::to_string_pretty(value)
            .map_err(|error| BifrostError::Config(error.to_string()))?
    );
    Ok(())
}

fn truncate(value: &str, max_chars: usize) -> String {
    let mut chars = value.chars();
    let truncated = chars.by_ref().take(max_chars).collect::<String>();
    if chars.next().is_some() {
        format!("{truncated}...")
    } else {
        truncated
    }
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
        assert!(url.contains("owner_module=cli"));
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

    #[test]
    fn extract_wake_phrase_from_sse_uses_last_text_candidate() {
        let stream = "event: partial\n\
data: {\"delta\":\"打\"}\n\n\
data: {\"text\":\"打开录音\"}\n\n\
data: [DONE]\n";
        assert_eq!(
            extract_wake_phrase_from_sse(stream).as_deref(),
            Some("打开录音")
        );
    }

    #[test]
    fn extract_wake_phrase_from_sse_strips_speaker_prefix() {
        let stream = "data: {\"text\":\"用户B: 打开录音\"}\n\n";
        assert_eq!(
            extract_wake_phrase_from_sse(stream).as_deref(),
            Some("打开录音")
        );

        let stream = "data: {\"text\":\"Manual Voice Wake Live: hello hello\"}\n\n";
        assert_eq!(
            extract_wake_phrase_from_sse(stream).as_deref(),
            Some("hello hello")
        );
    }

    #[test]
    fn shortcut_from_key_event_maps_key_and_modifiers() {
        let event = KeyEvent::new(
            KeyCode::Char('A'),
            KeyModifiers::SHIFT | KeyModifiers::ALT | KeyModifiers::SUPER,
        );
        let (key, modifiers) = shortcut_from_key_event(event).unwrap();
        assert_eq!(key, "a");
        assert_eq!(
            modifiers,
            vec!["cmd".to_string(), "shift".to_string(), "option".to_string()]
        );
        assert_eq!(format_shortcut(&key, &modifiers), "cmd+shift+option+a");
    }
}
