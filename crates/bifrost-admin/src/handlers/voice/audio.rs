use super::{
    parse_query_u64, VOICE_AUDIO_FORMAT, VOICE_CHANNELS, VOICE_MAX_UTTERANCE_MS, VOICE_SAMPLE_RATE,
    VOICE_SILENCE_COMMIT_MS, VOICE_SILENCE_RMS_THRESHOLD, VOICE_WORKER_IDLE_UNLOAD_MS,
    VOICE_WS_IDLE_TIMEOUT_MS,
};
use crate::handlers::asr_streaming::{append_transcript_delta, dedupe_increment};

const VOICE_REALTIME_AUDIO_BUFFER_RETAIN_MS: u64 = 120_000;

#[derive(Debug, Clone, Copy)]
pub(super) struct VoiceRuntimeTuning {
    pub(super) silence_commit_ms: u64,
    pub(super) worker_idle_unload_ms: u64,
    pub(super) max_utterance_ms: u64,
    pub(super) ws_idle_timeout_ms: u64,
}

impl VoiceRuntimeTuning {
    pub(super) fn from_query(query: &str) -> Self {
        Self {
            silence_commit_ms: parse_query_u64(query, "silence_commit_ms")
                .unwrap_or(VOICE_SILENCE_COMMIT_MS),
            worker_idle_unload_ms: parse_query_u64(query, "worker_idle_unload_ms")
                .unwrap_or(VOICE_WORKER_IDLE_UNLOAD_MS),
            max_utterance_ms: parse_query_u64(query, "max_utterance_ms")
                .unwrap_or(VOICE_MAX_UTTERANCE_MS),
            ws_idle_timeout_ms: parse_query_u64(query, "ws_idle_timeout_ms")
                .unwrap_or(VOICE_WS_IDLE_TIMEOUT_MS),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct VoiceAudioConfig {
    pub(super) sample_rate: u32,
    pub(super) channels: u16,
}

impl Default for VoiceAudioConfig {
    fn default() -> Self {
        Self {
            sample_rate: VOICE_SAMPLE_RATE,
            channels: VOICE_CHANNELS,
        }
    }
}

impl VoiceAudioConfig {
    pub(super) fn from_start(
        sample_rate: Option<u32>,
        channels: Option<u16>,
        format: Option<&str>,
    ) -> Result<Self, String> {
        let sample_rate = sample_rate.unwrap_or(VOICE_SAMPLE_RATE);
        let channels = channels.unwrap_or(VOICE_CHANNELS);
        let format = format.unwrap_or(VOICE_AUDIO_FORMAT);
        if sample_rate != VOICE_SAMPLE_RATE {
            return Err(format!(
                "Voice WebSocket requires {VOICE_SAMPLE_RATE}Hz PCM; client sent {sample_rate}Hz"
            ));
        }
        if channels != VOICE_CHANNELS {
            return Err(format!(
                "Voice WebSocket requires mono PCM; client sent {channels} channels"
            ));
        }
        if format != VOICE_AUDIO_FORMAT {
            return Err(format!(
                "Voice WebSocket requires {VOICE_AUDIO_FORMAT}; client sent {format}"
            ));
        }
        Ok(Self {
            sample_rate,
            channels,
        })
    }
}

#[derive(Debug, Clone)]
pub(super) struct VoiceTranscriptState {
    pub(super) committed: String,
    pub(super) partial: String,
    utterance_started_at_ms: Option<u128>,
    pub(super) silence_ms: u64,
    tuning: VoiceRuntimeTuning,
}

impl VoiceTranscriptState {
    pub(super) fn new(tuning: VoiceRuntimeTuning) -> Self {
        Self {
            committed: String::new(),
            partial: String::new(),
            utterance_started_at_ms: None,
            silence_ms: 0,
            tuning,
        }
    }

    pub(super) fn mark_audio_activity(
        &mut self,
        captured_at_ms: u128,
        chunk_ms: u64,
        is_speech: bool,
    ) {
        if is_speech {
            self.silence_ms = 0;
            if self.utterance_started_at_ms.is_none() {
                self.utterance_started_at_ms =
                    Some(captured_at_ms.saturating_sub(u128::from(chunk_ms.max(1))));
            }
        } else {
            self.silence_ms = self.silence_ms.saturating_add(chunk_ms.max(1));
        }
    }

    pub(super) fn should_commit_for_silence(&self) -> bool {
        !self.partial.trim().is_empty() && self.silence_ms >= self.tuning.silence_commit_ms
    }

    pub(super) fn should_unload_idle_worker(&self) -> bool {
        self.partial.trim().is_empty() && self.silence_ms >= self.tuning.worker_idle_unload_ms
    }

    pub(super) fn should_commit_for_max_duration(&self, captured_at_ms: u128) -> bool {
        self.utterance_started_at_ms
            .map(|started| {
                captured_at_ms.saturating_sub(started) >= u128::from(self.tuning.max_utterance_ms)
            })
            .unwrap_or(false)
    }

    pub(super) fn has_active_utterance(&self) -> bool {
        self.utterance_started_at_ms.is_some() || !self.partial.trim().is_empty()
    }

    pub(super) fn utterance_started_at_ms(&self) -> Option<u128> {
        self.utterance_started_at_ms
    }

    pub(super) fn commit_partial(&mut self) -> (String, String) {
        let partial = self.partial.clone();
        let delta = dedupe_increment(&self.committed, &self.partial);
        if !delta.trim().is_empty() {
            append_transcript_delta(&mut self.committed, &delta);
        }
        self.partial.clear();
        self.utterance_started_at_ms = None;
        self.silence_ms = 0;
        (delta, partial)
    }
}

#[derive(Debug, Clone)]
struct VoiceAudioTimelineChunk {
    start_ms: u64,
    end_ms: u64,
    bytes: Vec<u8>,
}

#[derive(Debug, Default, Clone)]
pub(super) struct VoiceRealtimeAudioBuffer {
    chunks: Vec<VoiceAudioTimelineChunk>,
}

impl VoiceRealtimeAudioBuffer {
    pub(super) fn clear(&mut self) {
        self.chunks.clear();
    }

    pub(super) fn push_chunk(
        &mut self,
        bytes: &[u8],
        captured_at_ms: u128,
        chunk_ms: u64,
        config: VoiceAudioConfig,
    ) {
        let end_ms = u64::try_from(captured_at_ms).unwrap_or(u64::MAX);
        let start_ms = end_ms.saturating_sub(chunk_ms.max(1));
        self.chunks.push(VoiceAudioTimelineChunk {
            start_ms,
            end_ms,
            bytes: bytes.to_vec(),
        });
        let retain_after_ms = end_ms.saturating_sub(VOICE_REALTIME_AUDIO_BUFFER_RETAIN_MS);
        self.chunks.retain(|chunk| chunk.end_ms >= retain_after_ms);
        if config.channels != 1 || config.sample_rate == 0 {
            self.chunks.clear();
        }
    }

    pub(super) fn slice_pcm16(
        &self,
        start_ms: u64,
        end_ms: u64,
        config: VoiceAudioConfig,
    ) -> Vec<u8> {
        if start_ms >= end_ms || config.sample_rate == 0 || config.channels != 1 {
            return Vec::new();
        }
        let mut output = Vec::new();
        for chunk in &self.chunks {
            if chunk.end_ms <= start_ms || chunk.start_ms >= end_ms {
                continue;
            }
            let keep_start_ms = start_ms.max(chunk.start_ms);
            let keep_end_ms = end_ms.min(chunk.end_ms);
            if keep_start_ms >= keep_end_ms {
                continue;
            }
            let byte_start = ms_offset_to_pcm16_byte_offset(
                keep_start_ms.saturating_sub(chunk.start_ms),
                config.sample_rate,
            )
            .min(chunk.bytes.len());
            let byte_end = ms_offset_to_pcm16_byte_offset(
                keep_end_ms.saturating_sub(chunk.start_ms),
                config.sample_rate,
            )
            .min(chunk.bytes.len());
            if byte_end > byte_start {
                output.extend_from_slice(&chunk.bytes[byte_start..byte_end]);
            }
        }
        output
    }
}

fn ms_offset_to_pcm16_byte_offset(offset_ms: u64, sample_rate: u32) -> usize {
    let samples = offset_ms.saturating_mul(u64::from(sample_rate)) / 1_000;
    usize::try_from(samples.saturating_mul(2)).unwrap_or(usize::MAX) & !1
}

pub(super) fn validate_voice_audio_chunk(
    bytes: &[u8],
    config: VoiceAudioConfig,
) -> Result<(), String> {
    if config.sample_rate != VOICE_SAMPLE_RATE || config.channels != VOICE_CHANNELS {
        return Err(format!(
            "Voice audio must be {VOICE_SAMPLE_RATE}Hz mono PCM16 before model inference"
        ));
    }
    if !bytes.len().is_multiple_of(2) {
        return Err("Voice PCM16 audio chunk must contain whole i16 samples".to_string());
    }
    Ok(())
}

pub(super) fn voice_audio_chunk_duration_ms(bytes: &[u8], config: VoiceAudioConfig) -> u64 {
    let sample_count = bytes.len() as u64 / 2 / u64::from(config.channels.max(1));
    ((sample_count.saturating_mul(1_000)) / u64::from(config.sample_rate.max(1))).max(1)
}

pub(super) fn voice_pcm16_rms(bytes: &[u8]) -> Result<f32, String> {
    if !bytes.len().is_multiple_of(2) {
        return Err("Voice PCM16 audio chunk must contain whole i16 samples".to_string());
    }
    if bytes.is_empty() {
        return Ok(0.0);
    }
    let mut sum = 0.0f64;
    let mut count = 0usize;
    for chunk in bytes.as_chunks::<2>().0 {
        let sample = i16::from_le_bytes(*chunk) as f64 / i16::MAX as f64;
        sum += sample * sample;
        count += 1;
    }
    Ok((sum / count.max(1) as f64).sqrt() as f32)
}

pub(super) fn is_voice_speech_chunk(bytes: &[u8]) -> Result<bool, String> {
    Ok(voice_pcm16_rms(bytes)? > VOICE_SILENCE_RMS_THRESHOLD)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pcm16(samples: &[i16]) -> Vec<u8> {
        samples
            .iter()
            .flat_map(|sample| sample.to_le_bytes())
            .collect()
    }

    #[test]
    fn realtime_audio_buffer_slices_window_across_chunks() {
        let config = VoiceAudioConfig::default();
        let first = pcm16(&vec![1; 16_000]);
        let second = pcm16(&vec![2; 16_000]);
        let mut buffer = VoiceRealtimeAudioBuffer::default();
        buffer.push_chunk(&first, 1_000, 1_000, config);
        buffer.push_chunk(&second, 2_000, 1_000, config);

        let sliced = buffer.slice_pcm16(500, 1_500, config);
        assert_eq!(sliced.len(), 16_000 * 2);
        assert_eq!(i16::from_le_bytes([sliced[0], sliced[1]]), 1);
        let midpoint = 8_000 * 2;
        assert_eq!(
            i16::from_le_bytes([sliced[midpoint], sliced[midpoint + 1]]),
            2
        );
    }

    #[test]
    fn speech_activity_marks_utterance_at_chunk_start() {
        let mut state = VoiceTranscriptState::new(VoiceRuntimeTuning::from_query(""));
        state.mark_audio_activity(1_500, 500, true);
        assert_eq!(state.utterance_started_at_ms(), Some(1_000));
    }
}
