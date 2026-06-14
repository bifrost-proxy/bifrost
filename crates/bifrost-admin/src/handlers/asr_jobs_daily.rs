use std::path::{Path, PathBuf};

use chrono::NaiveDate;
use serde::Serialize;

use crate::asr_runtime::text_output_dir;
use crate::handlers::asr_jobs_timeline::{
    generate_daily_summaries, source_modified_ms, source_size,
};

#[derive(Debug, Clone, Serialize)]
pub(super) struct AsrDailyDocumentSummary {
    pub(super) date: String,
    pub(super) path: PathBuf,
    pub(super) size: Option<u64>,
    pub(super) modified_ms: Option<u64>,
    pub(super) text_chars: usize,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct AsrDailyDocumentDetail {
    pub(super) task_id: String,
    pub(super) task_name: String,
    #[serde(flatten)]
    pub(super) summary: AsrDailyDocumentSummary,
    pub(super) content: String,
}

pub(super) fn list_daily_documents_for_task(
    data_dir: &Path,
    task_id: &str,
    task_name: &str,
) -> Result<Vec<AsrDailyDocumentSummary>, String> {
    let task_output_dir = text_output_dir(data_dir).join(task_id);
    refresh_daily_documents(&task_output_dir, task_name)?;
    let daily_dir = task_output_dir.join(".daily");
    let Ok(entries) = std::fs::read_dir(&daily_dir) else {
        return Ok(Vec::new());
    };

    let mut documents = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(stem) = path
            .file_stem()
            .and_then(|name| name.to_str())
            .map(str::to_string)
        else {
            continue;
        };
        if parse_daily_document_date(&stem).is_err() {
            continue;
        }
        documents.push(daily_document_summary(&stem, path)?);
    }
    documents.sort_by(|left, right| right.date.cmp(&left.date));
    Ok(documents)
}

pub(super) fn read_daily_document_for_task(
    data_dir: &Path,
    task_id: &str,
    task_name: &str,
    date: &str,
) -> Result<Option<AsrDailyDocumentDetail>, String> {
    parse_daily_document_date(date)?;
    let task_output_dir = text_output_dir(data_dir).join(task_id);
    refresh_daily_documents(&task_output_dir, task_name)?;
    let path = task_output_dir.join(".daily").join(format!("{date}.md"));
    if !path.is_file() {
        return Ok(None);
    }
    let content = std::fs::read_to_string(&path)
        .map_err(|error| format!("read ASR daily document {}: {error}", path.display()))?;
    let summary = daily_document_summary(date, path)?;
    Ok(Some(AsrDailyDocumentDetail {
        task_id: task_id.to_string(),
        task_name: task_name.to_string(),
        summary,
        content,
    }))
}

fn refresh_daily_documents(task_output_dir: &Path, task_name: &str) -> Result<(), String> {
    if !task_output_dir.exists() {
        return Ok(());
    }
    generate_daily_summaries(task_output_dir, task_name).map(|_| ())
}

fn daily_document_summary(date: &str, path: PathBuf) -> Result<AsrDailyDocumentSummary, String> {
    let content = std::fs::read_to_string(&path)
        .map_err(|error| format!("read ASR daily document {}: {error}", path.display()))?;
    Ok(AsrDailyDocumentSummary {
        date: date.to_string(),
        path: path.clone(),
        size: source_size(&path),
        modified_ms: source_modified_ms(&path),
        text_chars: content.chars().count(),
    })
}

fn parse_daily_document_date(date: &str) -> Result<NaiveDate, String> {
    NaiveDate::parse_from_str(date, "%Y-%m-%d")
        .map_err(|_| "ASR daily document date must use YYYY-MM-DD".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::handlers::asr_jobs_timeline::{TimelineSegment, TranscriptTimeline};
    use chrono::{Local, TimeZone};

    #[test]
    fn lists_and_reads_daily_documents_from_timeline_outputs() {
        let temp = tempfile::tempdir().unwrap();
        let task_dir = text_output_dir(temp.path()).join("task-daily");
        std::fs::create_dir_all(&task_dir).unwrap();
        let start = Local
            .with_ymd_and_hms(2026, 5, 14, 11, 44, 33)
            .earliest()
            .unwrap()
            .timestamp_millis() as u64;
        let timeline = TranscriptTimeline {
            task_id: "task-daily".to_string(),
            task_name: "Daily Task".to_string(),
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
                audio_start_ms: 0,
                audio_end_ms: 2_000,
                absolute_start_ms: Some(start),
                absolute_end_ms: Some(start + 2_000),
                speaker: None,
                speaker_display_name: None,
                overlap: false,
                text: "完整整理内容".to_string(),
            }],
        };
        std::fs::write(
            task_dir.join("audio.timeline.json"),
            serde_json::to_string_pretty(&timeline).unwrap(),
        )
        .unwrap();

        let summaries =
            list_daily_documents_for_task(temp.path(), "task-daily", "Daily Task").unwrap();
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].date, "2026-05-14");
        assert!(summaries[0].text_chars > 0);

        let detail =
            read_daily_document_for_task(temp.path(), "task-daily", "Daily Task", "2026-05-14")
                .unwrap()
                .unwrap();
        assert_eq!(detail.task_id, "task-daily");
        assert!(detail.content.contains("# Daily Task"));
        assert!(detail.content.contains("完整整理内容"));
    }

    #[test]
    fn rejects_non_date_daily_document_paths() {
        let temp = tempfile::tempdir().unwrap();
        let result =
            read_daily_document_for_task(temp.path(), "task-daily", "Daily Task", "../secret");
        assert!(result.is_err());
    }
}

#[cfg(test)]
mod daily_extra_tests {
    use super::*;

    #[test]
    fn parse_daily_document_date_enforces_yyyy_mm_dd_format() {
        assert!(parse_daily_document_date("2024-01-02").is_ok());
        assert!(parse_daily_document_date("2024-13-01").is_err());
        assert!(parse_daily_document_date("2024/01/02").is_err());
    }
}
