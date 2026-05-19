fn find_memory_limit_hint<'a>(
    hints: &'a [AsrChunkMemoryHint],
    model: &str,
    offset_secs: u64,
    duration_secs: u64,
) -> Option<&'a AsrChunkMemoryHint> {
    hints.iter().find(|hint| {
        hint.model == model
            && hint.offset_secs == offset_secs
            && hint.duration_secs == duration_secs
            && hint.preferred_chunk_secs >= MIN_BISECT_SECS
            && hint.preferred_chunk_secs < duration_secs
    })
}

fn suggested_memory_hint_chunk_secs(duration_secs: u64) -> u64 {
    if duration_secs <= MIN_BISECT_SECS {
        MIN_BISECT_SECS
    } else {
        (duration_secs / 2)
            .max(MIN_BISECT_SECS)
            .min(duration_secs.saturating_sub(1))
    }
}

fn merge_memory_limit_hints(
    existing: Vec<AsrChunkMemoryHint>,
    learned: Vec<AsrChunkMemoryHint>,
) -> Vec<AsrChunkMemoryHint> {
    let mut merged: Vec<AsrChunkMemoryHint> = Vec::new();
    for hint in existing.into_iter().chain(learned) {
        if let Some(current) = merged.iter_mut().find(|current| {
            current.model == hint.model
                && current.offset_secs == hint.offset_secs
                && current.duration_secs == hint.duration_secs
        }) {
            current.preferred_chunk_secs = current
                .preferred_chunk_secs
                .min(hint.preferred_chunk_secs)
                .max(MIN_BISECT_SECS);
            current.trigger_count = current.trigger_count.max(hint.trigger_count);
            if hint.last_triggered_at_ms >= current.last_triggered_at_ms {
                current.last_triggered_at_ms = hint.last_triggered_at_ms;
                current.last_error = hint.last_error;
            }
        } else {
            merged.push(hint);
        }
    }
    merged.sort_by_key(|hint| (hint.offset_secs, hint.duration_secs, hint.model.clone()));
    merged
}

fn merge_memory_limit_events_into_hints(
    hints: &mut Vec<AsrChunkMemoryHint>,
    model: &str,
    root_offset_secs: u64,
    root_duration_secs: u64,
    events: &[AsrMemoryLimitEvent],
) {
    if events.is_empty() {
        return;
    }
    let preferred = events
        .iter()
        .map(|event| event.suggested_chunk_secs)
        .min()
        .unwrap_or_else(|| suggested_memory_hint_chunk_secs(root_duration_secs))
        .max(MIN_BISECT_SECS)
        .min(root_duration_secs.saturating_sub(1).max(MIN_BISECT_SECS));
    let now = now_ms();
    let last_error = events.last().map(|event| {
        format!(
            "root={}s+{}s event={}s+{}s: {}",
            root_offset_secs,
            root_duration_secs,
            event.offset_secs,
            event.duration_secs,
            event.error
        )
    });

    if let Some(existing) = hints.iter_mut().find(|hint| {
        hint.model == model
            && hint.offset_secs == root_offset_secs
            && hint.duration_secs == root_duration_secs
    }) {
        existing.preferred_chunk_secs = existing.preferred_chunk_secs.min(preferred);
        existing.trigger_count = existing
            .trigger_count
            .saturating_add(events.len().try_into().unwrap_or(u32::MAX));
        existing.last_triggered_at_ms = now;
        existing.last_error = last_error;
    } else {
        hints.push(AsrChunkMemoryHint {
            model: model.to_string(),
            offset_secs: root_offset_secs,
            duration_secs: root_duration_secs,
            preferred_chunk_secs: preferred,
            trigger_count: events.len().try_into().unwrap_or(u32::MAX),
            last_triggered_at_ms: now,
            last_error,
        });
    }
}

#[allow(clippy::too_many_arguments)]
async fn transcribe_chunk_with_memory_hint(
    asr_bin: &Path,
    model_path: &Path,
    language: &str,
    chunk_wav: &Path,
    chunk_offset_secs: u64,
    chunk_duration_secs: u64,
    chunk_label: usize,
    tmp_dir: &Path,
    preferred_chunk_secs: u64,
    force_pause_task_id: Option<&str>,
    memory_events: Option<std::sync::Arc<StdMutex<Vec<AsrMemoryLimitEvent>>>>,
) -> Result<WholeFileTranscription, String> {
    let preferred_chunk_secs = preferred_chunk_secs.max(MIN_BISECT_SECS);
    if preferred_chunk_secs >= chunk_duration_secs {
        return transcribe_single_chunk_with_bisect(
            asr_bin,
            model_path,
            language,
            chunk_wav,
            chunk_offset_secs,
            chunk_duration_secs,
            chunk_label,
            tmp_dir,
            force_pause_task_id,
            memory_events,
        )
        .await;
    }

    let mut merged_text = String::new();
    let mut merged_segments = Vec::new();
    let mut local_offset_secs = 0u64;
    let mut part_index = 0usize;
    while local_offset_secs < chunk_duration_secs {
        if force_pause_task_id.is_some_and(task_force_pause_requested) {
            return Err(ASR_TASK_PAUSED_MESSAGE.to_string());
        }
        let remaining = chunk_duration_secs - local_offset_secs;
        let part_duration_secs = remaining.min(preferred_chunk_secs);
        let part_wav = tmp_dir.join(format!("hint-{chunk_label}-{part_index}.wav"));
        let pause_check = || force_pause_task_id.is_some_and(task_force_pause_requested);
        ffmpeg_split_chunk(
            chunk_wav,
            &part_wav,
            local_offset_secs,
            part_duration_secs,
            Some(&pause_check),
        )
        .await
        .map_err(|error| {
            format!("memory-hint chunk {chunk_label} part {part_index} split failed: {error}")
        })?;

        let part = transcribe_single_chunk_with_bisect(
            asr_bin,
            model_path,
            language,
            &part_wav,
            chunk_offset_secs + local_offset_secs,
            part_duration_secs,
            chunk_label,
            tmp_dir,
            force_pause_task_id,
            memory_events.clone(),
        )
        .await;
        let _ = std::fs::remove_file(&part_wav);

        let part = part?;
        if !part.text.is_empty() {
            if !merged_text.is_empty() {
                merged_text.push('\n');
            }
            merged_text.push_str(&part.text);
        }
        let local_offset_ms = local_offset_secs * 1000;
        merged_segments.extend(
            part.segments
                .into_iter()
                .map(|(start, end, text)| (start + local_offset_ms, end + local_offset_ms, text)),
        );

        local_offset_secs += part_duration_secs;
        part_index += 1;
    }

    Ok(WholeFileTranscription {
        text: merged_text,
        segments: merged_segments,
    })
}

/// Transcribe a single chunk with retry + bisect-on-failure strategy.
///
/// 1. Try up to 3 times with the full chunk audio.
/// 2. If all retries fail, bisect the chunk into two halves and recursively
///    transcribe each half. This handles MLX reshape errors triggered by
///    specific audio segments — a smaller window usually avoids the crash.
/// 3. Recursion stops when the sub-chunk is shorter than `MIN_BISECT_SECS`.
#[allow(clippy::too_many_arguments)]
async fn transcribe_single_chunk_with_bisect(
    asr_bin: &Path,
    model_path: &Path,
    language: &str,
    chunk_wav: &Path,
    chunk_offset_secs: u64,
    chunk_duration_secs: u64,
    chunk_label: usize,
    tmp_dir: &Path,
    force_pause_task_id: Option<&str>,
    memory_events: Option<std::sync::Arc<StdMutex<Vec<AsrMemoryLimitEvent>>>>,
) -> Result<WholeFileTranscription, String> {
    transcribe_chunk_bisect_inner(
        asr_bin,
        model_path,
        language,
        chunk_wav,
        chunk_offset_secs,
        chunk_duration_secs,
        chunk_label,
        tmp_dir,
        0, // recursion depth
        force_pause_task_id,
        memory_events,
    )
    .await
}

/// Inner recursive implementation with depth tracking to produce unique filenames.
#[allow(clippy::too_many_arguments)]
fn transcribe_chunk_bisect_inner<'a>(
    asr_bin: &'a Path,
    model_path: &'a Path,
    language: &'a str,
    chunk_wav: &'a Path,
    chunk_offset_secs: u64,
    chunk_duration_secs: u64,
    chunk_label: usize,
    tmp_dir: &'a Path,
    depth: u32,
    force_pause_task_id: Option<&'a str>,
    memory_events: Option<std::sync::Arc<StdMutex<Vec<AsrMemoryLimitEvent>>>>,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<WholeFileTranscription, String>> + Send + 'a>,
> {
    Box::pin(async move {
        // --- Pre-filter: skip silent or near-empty sub-chunks ---
        // This prevents the MLX encoder from crashing on empty tensors during
        // bisect recursion, where sub-chunks are often silence at boundaries.
        if chunk_duration_secs < MIN_CHUNK_SECS {
            tracing::warn!(
                chunk = chunk_label,
                depth = depth,
                duration_secs = chunk_duration_secs,
                "bisect sub-chunk too short ({MIN_CHUNK_SECS}s min), returning empty"
            );
            return Ok(WholeFileTranscription {
                text: String::new(),
                segments: Vec::new(),
            });
        }
        let sub_rms = compute_wav_rms_energy(chunk_wav);
        let sub_is_silent = match sub_rms {
            Some(rms) => rms < SILENCE_RMS_THRESHOLD,
            None => true, // unreadable WAV → skip
        };
        if sub_is_silent {
            tracing::warn!(
                chunk = chunk_label,
                depth = depth,
                rms = ?sub_rms,
                "bisect sub-chunk is silence, returning empty"
            );
            return Ok(WholeFileTranscription {
                text: String::new(),
                segments: Vec::new(),
            });
        }

        // --- attempt direct transcription with retries ---
        let max_attempts = 3u32;
        let mut last_error = String::new();
        for attempt in 0..max_attempts {
            if attempt > 0 {
                tracing::warn!(
                    chunk = chunk_label,
                    depth = depth,
                    attempt = attempt + 1,
                    "retrying ASR chunk after transient failure"
                );
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            }
            let bin = asr_bin.to_path_buf();
            let model = model_path.to_path_buf();
            let audio = chunk_wav.to_path_buf();
            let lang = language.to_string();
            let force_pause_task_id = force_pause_task_id.map(str::to_string);
            match tokio::task::spawn_blocking(move || {
                let abort_check = force_pause_task_id.map(|task_id| {
                    Box::new(move || task_force_pause_requested(&task_id))
                        as Box<dyn Fn() -> bool + Send>
                });
                run_asr_cli_with_footprint_guard_and_abort(&bin, &model, &audio, &lang, abort_check)
            })
            .await
            {
                Ok(Ok(r)) => return Ok(r),
                Ok(Err(e)) => {
                    if e.contains(ASR_ABORTED_ERROR_MARKER) {
                        return Err(ASR_TASK_PAUSED_MESSAGE.to_string());
                    }
                    let memory_limited = e.contains(ASR_MEMORY_LIMIT_ERROR_MARKER);
                    last_error = e;
                    if memory_limited {
                        if let Some(events) = &memory_events {
                            events.lock().unwrap().push(AsrMemoryLimitEvent {
                                offset_secs: chunk_offset_secs,
                                duration_secs: chunk_duration_secs,
                                suggested_chunk_secs: suggested_memory_hint_chunk_secs(
                                    chunk_duration_secs,
                                ),
                                error: last_error.clone(),
                            });
                        }
                        tracing::warn!(
                            chunk = chunk_label,
                            depth = depth,
                            error = %last_error,
                            "ASR chunk exceeded footprint limit; bisecting without same-size retry"
                        );
                        break;
                    }
                }
                Err(e) => last_error = format!("join: {e}"),
            }
        }

        // --- all retries exhausted — try bisecting ---
        if chunk_duration_secs <= MIN_BISECT_SECS {
            return Err(format!(
                "chunk {chunk_label} (offset {chunk_offset_secs}s, len {chunk_duration_secs}s, depth {depth}) \
             failed after {max_attempts} attempts and is too short to bisect: {last_error}"
            ));
        }

        let half = chunk_duration_secs / 2;
        let second_half = chunk_duration_secs - half;
        tracing::warn!(
            chunk = chunk_label,
            depth = depth,
            offset_secs = chunk_offset_secs,
            total_secs = chunk_duration_secs,
            half_a = half,
            half_b = second_half,
            "bisecting failed chunk into two halves for retry"
        );

        // Use depth in filename to avoid "Output same as Input" when recursing:
        // depth 0: bisect-42-d0-a.wav / bisect-42-d0-b.wav
        // depth 1: bisect-42-d1-a.wav / bisect-42-d1-b.wav  (input was bisect-42-d0-a.wav)
        let sub_a = tmp_dir.join(format!("bisect-{chunk_label}-d{depth}-a.wav"));
        let sub_b = tmp_dir.join(format!("bisect-{chunk_label}-d{depth}-b.wav"));
        let pause_check = || force_pause_task_id.is_some_and(task_force_pause_requested);
        ffmpeg_split_chunk(chunk_wav, &sub_a, 0, half, Some(&pause_check))
            .await
            .map_err(|e| format!("bisect chunk {chunk_label} d{depth} split-a: {e}"))?;
        ffmpeg_split_chunk(chunk_wav, &sub_b, half, second_half, Some(&pause_check))
            .await
            .map_err(|e| format!("bisect chunk {chunk_label} d{depth} split-b: {e}"))?;

        // Recursively transcribe each half.
        // IMPORTANT: try both halves independently. If half-a fails (e.g. MLX
        // crash on a boundary segment) we must still attempt half-b to recover
        // as much audio as possible. Only return Err when BOTH halves fail.
        let result_a = match transcribe_chunk_bisect_inner(
            asr_bin,
            model_path,
            language,
            &sub_a,
            chunk_offset_secs,
            half,
            chunk_label,
            tmp_dir,
            depth + 1,
            force_pause_task_id,
            memory_events.clone(),
        )
        .await
        {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(
                    chunk = chunk_label,
                    depth = depth,
                    half = "a",
                    half_secs = half,
                    %e,
                    "bisect half-a failed, will still attempt half-b"
                );
                WholeFileTranscription {
                    text: String::new(),
                    segments: Vec::new(),
                }
            }
        };

        // GPU cooldown between sub-halves.
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;

        let result_b = match transcribe_chunk_bisect_inner(
            asr_bin,
            model_path,
            language,
            &sub_b,
            chunk_offset_secs + half,
            second_half,
            chunk_label,
            tmp_dir,
            depth + 1,
            force_pause_task_id,
            memory_events.clone(),
        )
        .await
        {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(
                    chunk = chunk_label,
                    depth = depth,
                    half = "b",
                    half_secs = second_half,
                    %e,
                    "bisect half-b failed"
                );
                WholeFileTranscription {
                    text: String::new(),
                    segments: Vec::new(),
                }
            }
        };

        // Clean up sub-chunk files.
        let _ = std::fs::remove_file(&sub_a);
        let _ = std::fs::remove_file(&sub_b);

        // If both halves produced nothing (both crashed or both empty),
        // propagate failure so the caller records this chunk for later retry.
        let a_empty = result_a.text.is_empty() && result_a.segments.is_empty();
        let b_empty = result_b.text.is_empty() && result_b.segments.is_empty();
        if a_empty && b_empty {
            return Err(format!(
                "chunk {chunk_label} (depth {depth}) bisect: both halves produced no output; last_error: {last_error}"
            ));
        }

        // Merge results: concatenate text & segments.
        // IMPORTANT: result_b's segment timestamps are relative to sub_b's own
        // audio (starting from 0). We must shift them by `half` seconds so they
        // align with the parent chunk's timeline. This shift compounds correctly
        // through recursive bisect: each depth adjusts its own half_b offset,
        // and the Phase 2 caller adds the chunk's absolute offset on top.
        let mut merged_text = result_a.text;
        if !result_b.text.is_empty() {
            if !merged_text.is_empty() {
                merged_text.push('\n');
            }
            merged_text.push_str(&result_b.text);
        }
        let half_ms = half * 1000;
        let mut merged_segments = result_a.segments;
        merged_segments.extend(
            result_b
                .segments
                .into_iter()
                .map(|(s, e, t)| (s + half_ms, e + half_ms, t)),
        );

        tracing::info!(
            chunk = chunk_label,
            depth = depth,
            half_a_recovered = !a_empty,
            half_b_recovered = !b_empty,
            "bisect transcription completed for previously failed chunk"
        );

        Ok(WholeFileTranscription {
            text: merged_text,
            segments: merged_segments,
        })
    }) // Box::pin(async move { ... })
}
