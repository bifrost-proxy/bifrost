use crate::timeline::{TimelineSegment, TranscriptTimeline};

pub fn render_srt(timeline: &TranscriptTimeline) -> String {
    let mut output = String::new();
    for (index, segment) in subtitle_segments(timeline).enumerate() {
        if !output.is_empty() {
            output.push('\n');
        }
        output.push_str(&(index + 1).to_string());
        output.push('\n');
        output.push_str(&format!(
            "{} --> {}\n",
            format_srt_time(segment.audio_start_ms),
            format_srt_time(segment.audio_end_ms)
        ));
        output.push_str(&subtitle_text(segment));
        output.push('\n');
    }
    output
}

pub fn render_vtt(timeline: &TranscriptTimeline) -> String {
    let mut output = String::from("WEBVTT\n\n");
    for segment in subtitle_segments(timeline) {
        output.push_str(&format!(
            "{} --> {}\n",
            format_vtt_time(segment.audio_start_ms),
            format_vtt_time(segment.audio_end_ms)
        ));
        output.push_str(&subtitle_text(segment));
        output.push_str("\n\n");
    }
    output
}

fn subtitle_segments(timeline: &TranscriptTimeline) -> impl Iterator<Item = &TimelineSegment> {
    timeline
        .segments
        .iter()
        .filter(|segment| segment.audio_end_ms > segment.audio_start_ms)
        .filter(|segment| !segment.text.trim().is_empty())
}

fn subtitle_text(segment: &TimelineSegment) -> String {
    let text = segment.text.trim();
    match segment
        .speaker_display_name
        .as_deref()
        .or(segment.speaker.as_deref())
    {
        Some(speaker) if !speaker.trim().is_empty() => format!("{}: {}", speaker.trim(), text),
        _ => text.to_string(),
    }
}

fn format_srt_time(ms: u64) -> String {
    let (hours, minutes, seconds, millis) = split_time(ms);
    format!("{hours:02}:{minutes:02}:{seconds:02},{millis:03}")
}

fn format_vtt_time(ms: u64) -> String {
    let (hours, minutes, seconds, millis) = split_time(ms);
    format!("{hours:02}:{minutes:02}:{seconds:02}.{millis:03}")
}

fn split_time(ms: u64) -> (u64, u64, u64, u64) {
    let hours = ms / 3_600_000;
    let minutes = (ms % 3_600_000) / 60_000;
    let seconds = (ms % 60_000) / 1_000;
    let millis = ms % 1_000;
    (hours, minutes, seconds, millis)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crate::timeline::{TimelineSegment, TranscriptTimeline};

    use super::*;

    fn timeline() -> TranscriptTimeline {
        TranscriptTimeline {
            task_id: "task".to_string(),
            task_name: "Task".to_string(),
            source_path: PathBuf::from("meeting.wav"),
            source_size: None,
            source_modified_ms: None,
            source_created_at_ms: None,
            source_created_at_source: None,
            media_duration_ms: Some(5_860),
            model: "Qwen3-ASR-1.7B".to_string(),
            language: "chinese".to_string(),
            diarization_profile: Some("sherpa-onnx-balanced".to_string()),
            speakers: Vec::new(),
            processed_at_ms: 0,
            segments: vec![TimelineSegment {
                index: 0,
                audio_start_ms: 1_200,
                audio_end_ms: 5_860,
                absolute_start_ms: None,
                absolute_end_ms: None,
                speaker: Some("speaker_00".to_string()),
                speaker_display_name: Some("用户A".to_string()),
                overlap: false,
                text: "你好，我想咨询一下订单。".to_string(),
            }],
        }
    }

    #[test]
    fn subtitle_writer_formats_srt_timecode() {
        let output = render_srt(&timeline());

        assert!(output.contains("00:00:01,200 --> 00:00:05,860"));
        assert!(output.contains("用户A: 你好，我想咨询一下订单。"));
    }

    #[test]
    fn subtitle_writer_formats_vtt_timecode() {
        let output = render_vtt(&timeline());

        assert!(output.starts_with("WEBVTT\n\n"));
        assert!(output.contains("00:00:01.200 --> 00:00:05.860"));
        assert!(output.contains("用户A: 你好，我想咨询一下订单。"));
    }
}
