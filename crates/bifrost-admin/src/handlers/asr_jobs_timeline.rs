use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use chrono::{
    DateTime, Duration as ChronoDuration, Local, LocalResult, NaiveDate, NaiveTime, TimeZone,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct TimelineSegment {
    pub(super) index: usize,
    pub(super) audio_start_ms: u64,
    pub(super) audio_end_ms: u64,
    pub(super) absolute_start_ms: Option<u64>,
    pub(super) absolute_end_ms: Option<u64>,
    pub(super) text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct TranscriptTimeline {
    pub(super) task_id: String,
    pub(super) task_name: String,
    pub(super) source_path: PathBuf,
    pub(super) source_size: Option<u64>,
    pub(super) source_modified_ms: Option<u64>,
    pub(super) source_created_at_ms: Option<u64>,
    pub(super) source_created_at_source: Option<String>,
    pub(super) media_duration_ms: Option<u64>,
    pub(super) model: String,
    pub(super) language: String,
    pub(super) processed_at_ms: u64,
    pub(super) segments: Vec<TimelineSegment>,
}

#[derive(Debug, Clone)]
pub(super) struct SourceAudioInfo {
    pub(super) source_size: Option<u64>,
    pub(super) source_modified_ms: Option<u64>,
    pub(super) source_created_at_ms: Option<u64>,
    pub(super) source_created_at_source: Option<String>,
    pub(super) media_duration_ms: Option<u64>,
}

pub(super) fn inspect_source_audio(path: &Path) -> SourceAudioInfo {
    let source_size = source_size(path);
    let source_modified_ms = source_modified_ms(path);
    let (ffprobe_start_ms, ffprobe_start_source, media_duration_ms) = ffprobe_audio_info(path);
    let (source_created_at_ms, source_created_at_source) = if let Some(start_ms) = ffprobe_start_ms
    {
        (Some(start_ms), ffprobe_start_source)
    } else if let Some(start_ms) = parse_filename_created_at_ms(path) {
        (Some(start_ms), Some("filename_timestamp".to_string()))
    } else if let Some(start_ms) = filesystem_created_ms(path) {
        (Some(start_ms), Some("filesystem_birthtime".to_string()))
    } else if let Some(start_ms) = source_modified_ms {
        (Some(start_ms), Some("filesystem_modified_time".to_string()))
    } else {
        (None, None)
    };
    SourceAudioInfo {
        source_size,
        source_modified_ms,
        source_created_at_ms,
        source_created_at_source,
        media_duration_ms,
    }
}

fn ffprobe_audio_info(path: &Path) -> (Option<u64>, Option<String>, Option<u64>) {
    let output = std::process::Command::new("ffprobe")
        .arg("-v")
        .arg("error")
        .arg("-show_entries")
        .arg("format=duration:format_tags=date,creation_time")
        .arg("-of")
        .arg("json")
        .arg(path)
        .output();
    let Ok(output) = output else {
        return (None, None, None);
    };
    if !output.status.success() {
        return (None, None, None);
    }
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(&output.stdout) else {
        return (None, None, None);
    };
    let format = value.get("format").and_then(|value| value.as_object());
    let media_duration_ms = format
        .and_then(|format| format.get("duration"))
        .and_then(|value| value.as_str())
        .and_then(|value| value.parse::<f64>().ok())
        .map(|seconds| (seconds * 1000.0).round() as u64);
    let tags = format
        .and_then(|format| format.get("tags"))
        .and_then(|value| value.as_object());
    let date = tags
        .and_then(|tags| tags.get("date"))
        .and_then(|value| value.as_str());
    let creation_time = tags
        .and_then(|tags| tags.get("creation_time"))
        .and_then(|value| value.as_str());
    let created_at_ms = parse_ffprobe_created_at_ms(date, creation_time);
    let source = created_at_ms.map(|_| {
        if date.is_some() && creation_time.is_some() {
            "ffprobe.date_creation_time".to_string()
        } else {
            "ffprobe.creation_time".to_string()
        }
    });
    (created_at_ms, source, media_duration_ms)
}

pub(super) fn render_timeline_text(timeline: &TranscriptTimeline, fallback_text: &str) -> String {
    if timeline.segments.is_empty() {
        return fallback_text.trim().to_string();
    }
    timeline
        .segments
        .iter()
        .map(|segment| {
            let range = match (segment.absolute_start_ms, segment.absolute_end_ms) {
                (Some(start), Some(end)) => {
                    format!(
                        "{} - {}",
                        format_wall_clock_ms(start),
                        format_wall_clock_ms(end)
                    )
                }
                _ => format!(
                    "{} - {}",
                    format_audio_offset_ms(segment.audio_start_ms),
                    format_audio_offset_ms(segment.audio_end_ms)
                ),
            };
            format!("[{range}] {}", segment.text.trim())
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn format_wall_clock_ms(value: u64) -> String {
    Local
        .timestamp_millis_opt(value as i64)
        .earliest()
        .map(|datetime| datetime.format("%Y-%m-%d %H:%M:%S%.3f").to_string())
        .unwrap_or_else(|| value.to_string())
}

fn format_audio_offset_ms(value: u64) -> String {
    let millis = value % 1000;
    let total_seconds = value / 1000;
    let seconds = total_seconds % 60;
    let total_minutes = total_seconds / 60;
    let minutes = total_minutes % 60;
    let hours = total_minutes / 60;
    format!("{hours:02}:{minutes:02}:{seconds:02}.{millis:03}")
}

fn parse_ffprobe_created_at_ms(date: Option<&str>, creation_time: Option<&str>) -> Option<u64> {
    let creation_time = creation_time?;
    if let Ok(datetime) = DateTime::parse_from_rfc3339(creation_time) {
        return Some(datetime.timestamp_millis() as u64);
    }
    let date = date?;
    let date = NaiveDate::parse_from_str(date.trim(), "%Y-%m-%d").ok()?;
    let time = parse_hms(creation_time)?;
    Some(local_datetime_hms(date, time)?.timestamp_millis() as u64)
}

fn parse_filename_created_at_ms(path: &Path) -> Option<u64> {
    let filename = path.file_name()?.to_str()?;
    let parts = filename.split('_').collect::<Vec<_>>();
    for window in parts.windows(2) {
        let date = window[0];
        let time = window[1];
        if date.len() == 8
            && time.len() >= 6
            && date.chars().all(|ch| ch.is_ascii_digit())
            && time[..6].chars().all(|ch| ch.is_ascii_digit())
        {
            let date = NaiveDate::parse_from_str(date, "%Y%m%d").ok()?;
            let time = NaiveTime::parse_from_str(&time[..6], "%H%M%S").ok()?;
            return Some(local_datetime_hms(date, time)?.timestamp_millis() as u64);
        }
    }
    None
}

fn parse_hms(value: &str) -> Option<NaiveTime> {
    let trimmed = value.trim();
    let time = trimmed.split('.').next().unwrap_or(trimmed);
    NaiveTime::parse_from_str(time, "%H:%M:%S").ok()
}

fn local_datetime_hms(date: NaiveDate, time: NaiveTime) -> Option<DateTime<Local>> {
    let naive = date.and_time(time);
    match Local.from_local_datetime(&naive) {
        LocalResult::Single(value) => Some(value),
        LocalResult::Ambiguous(first, second) => Some(first.min(second)),
        LocalResult::None => {
            for offset in 1..=180 {
                let shifted = naive + ChronoDuration::minutes(offset);
                match Local.from_local_datetime(&shifted) {
                    LocalResult::Single(value) => return Some(value),
                    LocalResult::Ambiguous(first, second) => return Some(first.min(second)),
                    LocalResult::None => {}
                }
            }
            None
        }
    }
}

fn filesystem_created_ms(path: &Path) -> Option<u64> {
    std::fs::metadata(path)
        .ok()
        .and_then(|meta| meta.created().ok())
        .and_then(|created| created.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_millis() as u64)
}

pub(super) fn source_size(path: &Path) -> Option<u64> {
    std::fs::metadata(path).ok().map(|meta| meta.len())
}

pub(super) fn source_modified_ms(path: &Path) -> Option<u64> {
    std::fs::metadata(path)
        .ok()
        .and_then(|meta| meta.modified().ok())
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_millis() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Datelike, TimeZone, Timelike};

    #[test]
    fn parses_real_recorder_filename_timestamp() {
        let parsed =
            parse_filename_created_at_ms(Path::new("TX02_MIC001_20260514_114433_orig.wav"))
                .unwrap();
        let datetime = Local
            .timestamp_millis_opt(parsed as i64)
            .earliest()
            .unwrap();
        assert_eq!(datetime.year(), 2026);
        assert_eq!(datetime.month(), 5);
        assert_eq!(datetime.day(), 14);
        assert_eq!(datetime.hour(), 11);
        assert_eq!(datetime.minute(), 44);
        assert_eq!(datetime.second(), 33);
    }

    #[test]
    fn renders_timeline_text_with_absolute_time_ranges() {
        let start = Local
            .with_ymd_and_hms(2026, 5, 14, 11, 44, 33)
            .earliest()
            .unwrap()
            .timestamp_millis() as u64;
        let timeline = TranscriptTimeline {
            task_id: "task1".to_string(),
            task_name: "Task".to_string(),
            source_path: PathBuf::from("/tmp/audio.wav"),
            source_size: Some(1),
            source_modified_ms: None,
            source_created_at_ms: Some(start),
            source_created_at_source: Some("filename_timestamp".to_string()),
            media_duration_ms: Some(2_000),
            model: "Qwen3-ASR-1.7B".to_string(),
            language: "chinese".to_string(),
            processed_at_ms: start,
            segments: vec![TimelineSegment {
                index: 0,
                audio_start_ms: 1_000,
                audio_end_ms: 2_000,
                absolute_start_ms: Some(start + 1_000),
                absolute_end_ms: Some(start + 2_000),
                text: "测试内容".to_string(),
            }],
        };

        let rendered = render_timeline_text(&timeline, "");
        assert!(rendered.contains("2026-05-14 11:44:34.000"));
        assert!(rendered.contains("测试内容"));
    }
}
