/// Use ffmpeg to extract a fixed-duration chunk from the source WAV.
async fn ffmpeg_split_chunk(
    source: &Path,
    output: &Path,
    offset_secs: u64,
    duration_secs: u64,
    pause_check: Option<&(dyn Fn() -> bool + Send + Sync)>,
) -> Result<(), String> {
    let mut command = Command::new("ffmpeg");
    command
        .arg("-nostdin")
        .arg("-hide_banner")
        .arg("-loglevel")
        .arg("error")
        .arg("-y")
        .arg("-ss")
        .arg(offset_secs.to_string())
        .arg("-t")
        .arg(duration_secs.to_string())
        .arg("-i")
        .arg(source)
        .arg("-c")
        .arg("copy")
        .arg(output);
    let result = run_abortable_command(
        command,
        "ffmpeg chunk split",
        pause_check,
        ffmpeg_chunk_split_timeout(duration_secs),
    )
    .await?;
    if result.status.success() {
        Ok(())
    } else {
        Err(format!(
            "ffmpeg chunk split failed: {}",
            String::from_utf8_lossy(&result.stderr).trim()
        ))
    }
}

async fn ffmpeg_cut_wav_ms(
    source: &Path,
    output: &Path,
    start_ms: u64,
    end_ms: u64,
    pause_check: Option<&(dyn Fn() -> bool + Send + Sync)>,
) -> Result<(), String> {
    let duration_ms = end_ms.saturating_sub(start_ms);
    if duration_ms == 0 {
        return Err("ffmpeg segment cut: empty duration".to_string());
    }
    let mut command = Command::new("ffmpeg");
    command
        .arg("-nostdin")
        .arg("-hide_banner")
        .arg("-loglevel")
        .arg("error")
        .arg("-y")
        .arg("-ss")
        .arg(format!("{:.3}", start_ms as f64 / 1000.0))
        .arg("-t")
        .arg(format!("{:.3}", duration_ms as f64 / 1000.0))
        .arg("-i")
        .arg(source)
        .arg("-ar")
        .arg("16000")
        .arg("-ac")
        .arg("1")
        .arg(output);
    let result = run_abortable_command(
        command,
        "ffmpeg speaker segment cut",
        pause_check,
        ffmpeg_chunk_split_timeout(duration_ms.div_ceil(1000).max(1)),
    )
    .await?;
    if result.status.success() {
        Ok(())
    } else {
        Err(format!(
            "ffmpeg speaker segment cut failed: {}",
            String::from_utf8_lossy(&result.stderr).trim()
        ))
    }
}

async fn ffmpeg_cut_pcm16le_ms(
    source: &Path,
    output: &Path,
    start_ms: u64,
    end_ms: u64,
) -> Result<(), String> {
    let duration_ms = end_ms.saturating_sub(start_ms);
    if duration_ms == 0 {
        return Err("ffmpeg voiceprint cut: empty duration".to_string());
    }
    let mut command = Command::new("ffmpeg");
    command
        .arg("-nostdin")
        .arg("-hide_banner")
        .arg("-loglevel")
        .arg("error")
        .arg("-y")
        .arg("-ss")
        .arg(format!("{:.3}", start_ms as f64 / 1000.0))
        .arg("-t")
        .arg(format!("{:.3}", duration_ms as f64 / 1000.0))
        .arg("-i")
        .arg(source)
        .arg("-ar")
        .arg(VOICEPRINT_SAMPLE_RATE.to_string())
        .arg("-ac")
        .arg("1")
        .arg("-acodec")
        .arg("pcm_s16le")
        .arg("-f")
        .arg("s16le")
        .arg(output);
    let result = run_abortable_command(
        command,
        "ffmpeg voiceprint segment cut",
        None,
        ffmpeg_chunk_split_timeout(duration_ms.div_ceil(1000).max(1)),
    )
    .await?;
    if result.status.success() {
        Ok(())
    } else {
        Err(format!(
            "ffmpeg voiceprint segment cut failed: {}",
            String::from_utf8_lossy(&result.stderr).trim()
        ))
    }
}

async fn run_abortable_command(
    mut command: Command,
    label: &str,
    pause_check: Option<&(dyn Fn() -> bool + Send + Sync)>,
    timeout: Duration,
) -> Result<std::process::Output, String> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    command.kill_on_drop(true);
    configure_tokio_process_group(&mut command);
    let mut child = command
        .spawn()
        .map_err(|error| format!("{label}: failed to spawn process: {error}"))?;
    let pid = child.id();
    let started = Instant::now();

    loop {
        if let Some(_status) = child
            .try_wait()
            .map_err(|error| format!("{label}: failed to poll process: {error}"))?
        {
            return child
                .wait_with_output()
                .await
                .map_err(|error| format!("{label}: failed to collect output: {error}"));
        }

        if pause_check.is_some_and(|check| check()) {
            terminate_tokio_child_process_group(pid, &mut child);
            let _ = child.wait().await;
            return Err(ASR_TASK_PAUSED_MESSAGE.to_string());
        }

        if started.elapsed() > timeout {
            terminate_tokio_child_process_group(pid, &mut child);
            let _ = child.wait().await;
            return Err(format!("{label}: timed out after {}s", timeout.as_secs()));
        }

        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

#[cfg(unix)]
fn configure_tokio_process_group(command: &mut Command) {
    command.process_group(0);
}

#[cfg(not(unix))]
fn configure_tokio_process_group(_command: &mut Command) {}

#[cfg(unix)]
fn terminate_tokio_child_process_group(pid: Option<u32>, child: &mut tokio::process::Child) {
    if let Some(pid) = pid {
        let _ = killpg(NixPid::from_raw(pid as i32), Signal::SIGKILL);
    }
    let _ = child.start_kill();
}

#[cfg(not(unix))]
fn terminate_tokio_child_process_group(_pid: Option<u32>, child: &mut tokio::process::Child) {
    let _ = child.start_kill();
}

fn ffmpeg_normalize_timeout(media_duration_ms: Option<u64>) -> Duration {
    let media_secs = media_duration_ms
        .map(|millis| millis.div_ceil(1000))
        .unwrap_or(FFMPEG_NORMALIZE_MIN_TIMEOUT_SECS);
    Duration::from_secs(media_secs.saturating_mul(2).clamp(
        FFMPEG_NORMALIZE_MIN_TIMEOUT_SECS,
        FFMPEG_NORMALIZE_MAX_TIMEOUT_SECS,
    ))
}

fn ffmpeg_chunk_split_timeout(duration_secs: u64) -> Duration {
    Duration::from_secs(
        duration_secs
            .saturating_mul(2)
            .clamp(FFMPEG_CHUNK_SPLIT_TIMEOUT_SECS, 5 * 60),
    )
}

/// Minimum chunk duration (seconds) to attempt transcription. Chunks shorter
/// than this are skipped — they don't carry enough audio for the encoder and
/// are the most common trigger for MLX reshape-to-empty-tensor crashes.
const MIN_CHUNK_SECS: u64 = 1;

/// RMS energy threshold (16-bit PCM scale, 0–32768). Chunks with energy below
/// this value are considered silence and skipped. 30 ≈ -60 dBFS, below any
/// audible speech or typical ambient noise. Only true digital silence or
/// near-zero signals are filtered; low-volume speech passes through safely.
const SILENCE_RMS_THRESHOLD: f64 = 30.0;

/// Compute RMS energy of a 16-bit PCM WAV file (16 kHz mono expected).
/// Returns `None` if the file cannot be read or is not a valid RIFF/WAVE.
fn compute_wav_rms_energy(wav_path: &Path) -> Option<f64> {
    use std::io::{Read, Seek, SeekFrom};

    let mut file = std::fs::File::open(wav_path).ok()?;
    let mut riff_header = [0_u8; 12];
    file.read_exact(&mut riff_header).ok()?;
    // Validate RIFF/WAVE header (12 bytes minimum).
    if &riff_header[0..4] != b"RIFF" || &riff_header[8..12] != b"WAVE" {
        return None;
    }

    // Stream the data chunk instead of loading an entire long recording into
    // memory merely to decide whether inference should start.
    loop {
        let mut chunk_header = [0_u8; 8];
        file.read_exact(&mut chunk_header).ok()?;
        let chunk_size = u32::from_le_bytes([
            chunk_header[4],
            chunk_header[5],
            chunk_header[6],
            chunk_header[7],
        ]) as u64;
        if &chunk_header[0..4] == b"data" {
            let mut remaining = chunk_size;
            let mut sample_count = 0_u64;
            let mut sum_sq = 0.0_f64;
            let mut buffer = [0_u8; 64 * 1024];
            while remaining >= 2 {
                let wanted = (remaining.min(buffer.len() as u64) as usize) & !1;
                file.read_exact(&mut buffer[..wanted]).ok()?;
                for sample in buffer[..wanted].chunks_exact(2) {
                    let value = i16::from_le_bytes([sample[0], sample[1]]) as f64;
                    sum_sq += value * value;
                }
                sample_count += (wanted / 2) as u64;
                remaining -= wanted as u64;
            }
            return if sample_count == 0 {
                Some(0.0)
            } else {
                Some((sum_sq / sample_count as f64).sqrt())
            };
        }
        // WAV payloads are word-aligned (one pad byte for odd-sized chunks).
        file.seek(SeekFrom::Current((chunk_size + (chunk_size & 1)) as i64))
            .ok()?;
    }
}

/// Check if input is already 16kHz mono 16-bit PCM WAV.
/// Returns true if no transcoding is needed.
async fn is_already_normalized_wav(input: &Path) -> bool {
    if wav_header_is_normalized(input) {
        return true;
    }

    let result = Command::new("ffprobe")
        .arg("-v")
        .arg("error")
        .arg("-select_streams")
        .arg("a:0")
        .arg("-show_entries")
        .arg("stream=codec_name,sample_rate,channels")
        .arg("-of")
        .arg("csv=p=0")
        .arg(input)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .await;
    let Ok(output) = result else {
        return false;
    };
    if !output.status.success() {
        return false;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let trimmed = stdout.trim();
    // Expected format: "pcm_s16le,16000,1"
    trimmed == "pcm_s16le,16000,1"
}

fn wav_header_is_normalized(input: &Path) -> bool {
    let data = match std::fs::read(input) {
        Ok(data) => data,
        Err(_) => return false,
    };
    if data.len() < 12 || &data[0..4] != b"RIFF" || &data[8..12] != b"WAVE" {
        return false;
    }

    let mut pos = 12usize;
    while pos + 8 <= data.len() {
        let chunk_id = &data[pos..pos + 4];
        let chunk_size =
            u32::from_le_bytes([data[pos + 4], data[pos + 5], data[pos + 6], data[pos + 7]])
                as usize;
        let chunk_start = pos + 8;
        let chunk_end = chunk_start.saturating_add(chunk_size);
        if chunk_end > data.len() {
            return false;
        }

        if chunk_id == b"fmt " {
            if chunk_size < 16 {
                return false;
            }
            let audio_format = u16::from_le_bytes([data[chunk_start], data[chunk_start + 1]]);
            let channels = u16::from_le_bytes([data[chunk_start + 2], data[chunk_start + 3]]);
            let sample_rate = u32::from_le_bytes([
                data[chunk_start + 4],
                data[chunk_start + 5],
                data[chunk_start + 6],
                data[chunk_start + 7],
            ]);
            let bits_per_sample =
                u16::from_le_bytes([data[chunk_start + 14], data[chunk_start + 15]]);
            return audio_format == 1
                && channels == 1
                && sample_rate == 16_000
                && bits_per_sample == 16;
        }

        pos += 8 + chunk_size + (chunk_size & 1);
    }
    false
}

/// Create a temp dir, normalize `input` into it, and return `(wav_path, temp_dir_path)`.
/// The temp dir is `keep()`-ed so the caller controls its lifetime.
async fn normalize_to_temp(
    input: &Path,
    pause_check: Option<&(dyn Fn() -> bool + Send + Sync)>,
    media_duration_ms: Option<u64>,
) -> Result<(PathBuf, PathBuf), String> {
    let temp = tempfile::tempdir().map_err(|e| format!("create temp dir: {e}"))?;
    let wav = temp.path().join("input.wav");
    normalize_audio_file(input, &wav, pause_check, media_duration_ms).await?;
    Ok((wav, temp.keep()))
}

async fn normalize_audio_file(
    input: &Path,
    output: &Path,
    pause_check: Option<&(dyn Fn() -> bool + Send + Sync)>,
    media_duration_ms: Option<u64>,
) -> Result<(), String> {
    // Fast path: if the input is already 16kHz mono 16-bit PCM WAV, create a
    // hard link (or fallback to copy) instead of running ffmpeg transcoding.
    // This saves ~1-3s per file and avoids duplicating ~250MB for large WAVs.
    if is_already_normalized_wav(input).await {
        tracing::debug!(
            input = %input.display(),
            "skipping ffmpeg normalize: input is already 16kHz mono PCM WAV"
        );
        // Try hard link first (instant, no disk space), fall back to copy.
        if std::fs::hard_link(input, output).is_err() {
            std::fs::copy(input, output).map_err(|error| {
                format!(
                    "copy already-normalized WAV {} -> {}: {error}",
                    input.display(),
                    output.display()
                )
            })?;
        }
        return Ok(());
    }

    let mut command = Command::new("ffmpeg");
    command
        .arg("-nostdin")
        .arg("-hide_banner")
        .arg("-loglevel")
        .arg("error")
        .arg("-y")
        .arg("-i")
        .arg(input)
        .arg("-ar")
        .arg("16000")
        .arg("-ac")
        .arg("1")
        .arg(output);
    let result = run_abortable_command(
        command,
        "ffmpeg normalize",
        pause_check,
        ffmpeg_normalize_timeout(media_duration_ms),
    )
    .await?;
    if result.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&result.stderr).trim().to_string())
    }
}
