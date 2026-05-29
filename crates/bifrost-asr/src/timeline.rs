use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use chrono::{
    DateTime, Duration as ChronoDuration, Local, LocalResult, NaiveDate, NaiveTime, TimeZone,
};
use serde::{Deserialize, Serialize};

pub const ASR_TASK_SEGMENT_MAX_MS: u64 = 30_000;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimelineSegment {
    pub index: usize,
    pub audio_start_ms: u64,
    pub audio_end_ms: u64,
    pub absolute_start_ms: Option<u64>,
    pub absolute_end_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speaker: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speaker_display_name: Option<String>,
    #[serde(default)]
    pub overlap: bool,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimelineSpeaker {
    pub id: String,
    pub display_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mapped_profile_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidate_profile_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidate_display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidate_confidence: Option<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptTimeline {
    pub task_id: String,
    pub task_name: String,
    pub source_path: PathBuf,
    pub source_size: Option<u64>,
    pub source_modified_ms: Option<u64>,
    pub source_created_at_ms: Option<u64>,
    pub source_created_at_source: Option<String>,
    pub media_duration_ms: Option<u64>,
    pub model: String,
    pub language: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diarization_profile: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub speakers: Vec<TimelineSpeaker>,
    pub processed_at_ms: u64,
    pub segments: Vec<TimelineSegment>,
}

#[derive(Debug, Clone)]
pub struct SourceAudioInfo {
    pub source_size: Option<u64>,
    pub source_modified_ms: Option<u64>,
    pub source_created_at_ms: Option<u64>,
    pub source_created_at_source: Option<String>,
    pub media_duration_ms: Option<u64>,
}

pub fn inspect_source_audio(path: &Path) -> SourceAudioInfo {
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

pub fn render_timeline_text(timeline: &TranscriptTimeline, fallback_text: &str) -> String {
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
            let speaker = segment
                .speaker_display_name
                .as_ref()
                .or(segment.speaker.as_ref())
                .map(|label| format!(" {label}:"))
                .unwrap_or_default();
            format!("[{range}]{speaker} {}", segment.text.trim())
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

pub fn source_size(path: &Path) -> Option<u64> {
    std::fs::metadata(path).ok().map(|meta| meta.len())
}

pub fn source_modified_ms(path: &Path) -> Option<u64> {
    std::fs::metadata(path)
        .ok()
        .and_then(|meta| meta.modified().ok())
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_millis() as u64)
}

pub fn normalize_timeline_segments(timeline: &mut TranscriptTimeline) {
    if timeline.segments.iter().all(|segment| {
        segment.audio_end_ms.saturating_sub(segment.audio_start_ms) <= ASR_TASK_SEGMENT_MAX_MS
    }) {
        return;
    }

    let mut normalized = Vec::new();
    for segment in timeline.segments.drain(..) {
        normalized.extend(split_timeline_segment(
            segment,
            timeline.source_created_at_ms,
            ASR_TASK_SEGMENT_MAX_MS,
        ));
    }
    for (index, segment) in normalized.iter_mut().enumerate() {
        segment.index = index;
    }
    timeline.segments = normalized;
}

pub fn split_timeline_segment(
    segment: TimelineSegment,
    source_created_at_ms: Option<u64>,
    max_segment_ms: u64,
) -> Vec<TimelineSegment> {
    let duration_ms = segment.audio_end_ms.saturating_sub(segment.audio_start_ms);
    if duration_ms <= max_segment_ms || max_segment_ms == 0 {
        return vec![segment];
    }

    let window_count = duration_ms.div_ceil(max_segment_ms) as usize;
    let text_chars = segment.text.chars().collect::<Vec<_>>();
    let text_len = text_chars.len();
    let absolute_base = segment.absolute_start_ms.or_else(|| {
        source_created_at_ms.map(|source_start| source_start.saturating_add(segment.audio_start_ms))
    });

    (0..window_count)
        .map(|window_index| {
            let audio_start_ms = segment
                .audio_start_ms
                .saturating_add((window_index as u64).saturating_mul(max_segment_ms));
            let audio_end_ms = segment.audio_end_ms.min(
                segment
                    .audio_start_ms
                    .saturating_add(((window_index as u64) + 1).saturating_mul(max_segment_ms)),
            );
            let char_start = text_len.saturating_mul(window_index) / window_count;
            let char_end = text_len.saturating_mul(window_index + 1) / window_count;
            let text = text_chars[char_start..char_end].iter().collect::<String>();
            let absolute_start_ms = absolute_base.map(|base| {
                base.saturating_add(audio_start_ms.saturating_sub(segment.audio_start_ms))
            });
            let absolute_end_ms = absolute_base.map(|base| {
                base.saturating_add(audio_end_ms.saturating_sub(segment.audio_start_ms))
            });

            TimelineSegment {
                index: window_index,
                audio_start_ms,
                audio_end_ms,
                absolute_start_ms,
                absolute_end_ms,
                speaker: segment.speaker.clone(),
                speaker_display_name: segment.speaker_display_name.clone(),
                overlap: segment.overlap,
                text,
            }
        })
        .collect()
}

pub fn generate_daily_summaries(
    task_output_dir: &Path,
    task_name: &str,
) -> Result<Vec<PathBuf>, String> {
    let mut timeline_files = Vec::new();
    collect_timeline_files(task_output_dir, &mut timeline_files);

    struct FlatSegment {
        source_label: String,
        absolute_start_ms: u64,
        absolute_end_ms: u64,
        speaker_label: Option<String>,
        text: String,
    }

    let mut all_segments: Vec<FlatSegment> = Vec::new();
    for timeline_path in &timeline_files {
        let content = match std::fs::read_to_string(timeline_path) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(path = %timeline_path.display(), error = %e, "skipping unreadable timeline");
                continue;
            }
        };
        let timeline: TranscriptTimeline = match serde_json::from_str(&content) {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!(path = %timeline_path.display(), error = %e, "skipping malformed timeline");
                continue;
            }
        };
        let source_label = timeline_path
            .strip_prefix(task_output_dir)
            .ok()
            .and_then(|rel| rel.to_str())
            .map(|s| {
                s.trim_end_matches(".timeline.json")
                    .trim_end_matches('.')
                    .to_string()
            })
            .or_else(|| {
                timeline
                    .source_path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
            })
            .unwrap_or_default();
        for seg in &timeline.segments {
            let (abs_start, abs_end) = match (seg.absolute_start_ms, seg.absolute_end_ms) {
                (Some(s), Some(e)) => (s, e),
                _ => {
                    let base = timeline.processed_at_ms;
                    (base + seg.audio_start_ms, base + seg.audio_end_ms)
                }
            };
            all_segments.push(FlatSegment {
                source_label: source_label.clone(),
                absolute_start_ms: abs_start,
                absolute_end_ms: abs_end,
                speaker_label: seg
                    .speaker_display_name
                    .clone()
                    .or_else(|| seg.speaker.clone()),
                text: seg.text.clone(),
            });
        }
    }

    if all_segments.is_empty() {
        return Ok(Vec::new());
    }

    all_segments.sort_by_key(|s| s.absolute_start_ms);

    let mut by_date: BTreeMap<NaiveDate, Vec<&FlatSegment>> = BTreeMap::new();
    for seg in &all_segments {
        let date = Local
            .timestamp_millis_opt(seg.absolute_start_ms as i64)
            .earliest()
            .map(|dt| dt.date_naive())
            .unwrap_or_else(|| NaiveDate::from_ymd_opt(1970, 1, 1).unwrap());
        by_date.entry(date).or_default().push(seg);
    }

    let daily_dir = task_output_dir.join(".daily");
    std::fs::create_dir_all(&daily_dir).map_err(|e| format!("create daily dir: {e}"))?;

    let mut written = Vec::new();
    for (date, segments) in &by_date {
        let mut md = String::new();
        md.push_str(&format!("# {task_name} — {date}\n\n"));

        let mut file_groups: Vec<(&str, Vec<&FlatSegment>)> = Vec::new();
        for seg in segments {
            if let Some(group) = file_groups
                .last_mut()
                .filter(|(name, _)| *name == seg.source_label.as_str())
            {
                group.1.push(seg);
            } else if let Some(group) = file_groups
                .iter_mut()
                .find(|(name, _)| *name == seg.source_label.as_str())
            {
                group.1.push(seg);
            } else {
                file_groups.push((&seg.source_label, vec![seg]));
            }
        }

        for (i, (filename, segs)) in file_groups.iter().enumerate() {
            if i > 0 {
                md.push('\n');
            }
            md.push_str(&format!("## {filename}\n\n"));
            for seg in segs {
                let start = format_wall_clock_ms(seg.absolute_start_ms);
                let end = format_wall_clock_ms(seg.absolute_end_ms);
                let speaker = seg
                    .speaker_label
                    .as_ref()
                    .map(|label| format!(" **{label}:**"))
                    .unwrap_or_default();
                md.push_str(&format!(
                    "**[{start} -> {end}]**{speaker}  \n{}\n\n",
                    seg.text.trim()
                ));
            }
        }

        let path = daily_dir.join(format!("{date}.md"));
        let needs_write = match std::fs::read(&path) {
            Ok(existing) => existing != md.as_bytes(),
            Err(_) => true,
        };
        if needs_write {
            std::fs::write(&path, md.as_bytes())
                .map_err(|e| format!("write {}: {e}", path.display()))?;
        }
        written.push(path);
    }

    Ok(written)
}

fn collect_timeline_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if path.file_name().and_then(|n| n.to_str()) == Some(".daily") {
                continue;
            }
            collect_timeline_files(&path, out);
        } else if path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.ends_with(".timeline.json"))
        {
            out.push(path);
        }
    }
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
            diarization_profile: None,
            speakers: Vec::new(),
            processed_at_ms: start,
            segments: vec![TimelineSegment {
                index: 0,
                audio_start_ms: 1_000,
                audio_end_ms: 2_000,
                absolute_start_ms: Some(start + 1_000),
                absolute_end_ms: Some(start + 2_000),
                speaker: None,
                speaker_display_name: None,
                overlap: false,
                text: "测试内容".to_string(),
            }],
        };

        let rendered = render_timeline_text(&timeline, "");
        assert!(rendered.contains("2026-05-14 11:44:34.000"));
        assert!(rendered.contains("测试内容"));
    }

    #[test]
    fn normalizes_legacy_oversized_segments() {
        let mut timeline = TranscriptTimeline {
            task_id: "task".to_string(),
            task_name: "Task".to_string(),
            source_path: PathBuf::from("/tmp/audio.wav"),
            source_size: None,
            source_modified_ms: None,
            source_created_at_ms: Some(1_000_000),
            source_created_at_source: None,
            media_duration_ms: Some(65_000),
            model: "Qwen3-ASR-1.7B".to_string(),
            language: "chinese".to_string(),
            diarization_profile: None,
            speakers: Vec::new(),
            processed_at_ms: 1,
            segments: vec![TimelineSegment {
                index: 0,
                audio_start_ms: 0,
                audio_end_ms: 65_000,
                absolute_start_ms: None,
                absolute_end_ms: None,
                speaker: Some("speaker_00".to_string()),
                speaker_display_name: Some("用户A".to_string()),
                overlap: false,
                text: "abcdefghi".to_string(),
            }],
        };

        normalize_timeline_segments(&mut timeline);

        assert_eq!(timeline.segments.len(), 3);
        assert_eq!(timeline.segments[0].audio_start_ms, 0);
        assert_eq!(timeline.segments[0].audio_end_ms, 30_000);
        assert_eq!(timeline.segments[1].audio_start_ms, 30_000);
        assert_eq!(timeline.segments[1].audio_end_ms, 60_000);
        assert_eq!(timeline.segments[2].audio_start_ms, 60_000);
        assert_eq!(timeline.segments[2].audio_end_ms, 65_000);
        assert!(timeline
            .segments
            .iter()
            .all(|segment| segment.speaker.as_deref() == Some("speaker_00")));
        assert_eq!(timeline.segments[2].absolute_end_ms, Some(1_065_000));
    }

    #[test]
    fn generates_daily_summary_grouped_by_date() {
        let temp = tempfile::tempdir().unwrap();
        let task_dir = temp.path().join("task1");
        std::fs::create_dir_all(&task_dir).unwrap();

        let start1 = Local
            .with_ymd_and_hms(2026, 5, 14, 10, 0, 0)
            .earliest()
            .unwrap()
            .timestamp_millis() as u64;
        let start2 = Local
            .with_ymd_and_hms(2026, 5, 14, 14, 30, 0)
            .earliest()
            .unwrap()
            .timestamp_millis() as u64;

        let t1 = TranscriptTimeline {
            task_id: "task1".to_string(),
            task_name: "My Task".to_string(),
            source_path: PathBuf::from("/data/meeting_a.wav"),
            source_size: Some(100),
            source_modified_ms: None,
            source_created_at_ms: Some(start1),
            source_created_at_source: None,
            media_duration_ms: Some(60_000),
            model: "Qwen3-ASR-1.7B".to_string(),
            language: "chinese".to_string(),
            diarization_profile: None,
            speakers: Vec::new(),
            processed_at_ms: start1,
            segments: vec![TimelineSegment {
                index: 0,
                audio_start_ms: 0,
                audio_end_ms: 60_000,
                absolute_start_ms: Some(start1),
                absolute_end_ms: Some(start1 + 60_000),
                speaker: None,
                speaker_display_name: None,
                overlap: false,
                text: "上午会议内容".to_string(),
            }],
        };
        let t2 = TranscriptTimeline {
            task_id: "task1".to_string(),
            task_name: "My Task".to_string(),
            source_path: PathBuf::from("/data/sub/meeting_b.wav"),
            source_size: Some(200),
            source_modified_ms: None,
            source_created_at_ms: Some(start2),
            source_created_at_source: None,
            media_duration_ms: Some(60_000),
            model: "Qwen3-ASR-1.7B".to_string(),
            language: "chinese".to_string(),
            diarization_profile: None,
            speakers: Vec::new(),
            processed_at_ms: start2,
            segments: vec![TimelineSegment {
                index: 0,
                audio_start_ms: 0,
                audio_end_ms: 60_000,
                absolute_start_ms: Some(start2),
                absolute_end_ms: Some(start2 + 60_000),
                speaker: None,
                speaker_display_name: None,
                overlap: false,
                text: "下午讨论内容".to_string(),
            }],
        };

        std::fs::write(
            task_dir.join("meeting_a.timeline.json"),
            serde_json::to_string_pretty(&t1).unwrap(),
        )
        .unwrap();
        std::fs::create_dir_all(task_dir.join("sub")).unwrap();
        std::fs::write(
            task_dir.join("sub/meeting_b.timeline.json"),
            serde_json::to_string_pretty(&t2).unwrap(),
        )
        .unwrap();

        let result = generate_daily_summaries(&task_dir, "My Task").unwrap();
        assert_eq!(result.len(), 1);
        assert!(result[0].ends_with("2026-05-14.md"));

        let content = std::fs::read_to_string(&result[0]).unwrap();
        assert!(content.contains("# My Task — 2026-05-14"));
        assert!(content.contains("## meeting_a"));
        assert!(content.contains("## sub/meeting_b"));
        assert!(content.contains("上午会议内容"));
        assert!(content.contains("下午讨论内容"));

        let pos_morning = content.find("上午会议内容").unwrap();
        let pos_afternoon = content.find("下午讨论内容").unwrap();
        assert!(pos_morning < pos_afternoon);
    }
}
