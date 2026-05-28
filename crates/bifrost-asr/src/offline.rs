use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::artifacts::{output_paths_in, subtitle_path_from_timeline};
use crate::subtitle::{render_srt, render_vtt};
use crate::timeline::{render_timeline_text, TranscriptTimeline};

#[derive(Debug, Clone)]
pub struct OfflineSubtitleArtifactPaths {
    pub text_path: PathBuf,
    pub metadata_path: PathBuf,
    pub timeline_path: PathBuf,
    pub srt_path: PathBuf,
    pub vtt_path: PathBuf,
}

pub struct OfflineSubtitleArtifactRequest<'a> {
    pub data_dir: &'a Path,
    pub task_id: &'a str,
    pub source_path: &'a Path,
    pub audio_dir: &'a Path,
    pub timeline: &'a TranscriptTimeline,
    pub fallback_text: &'a str,
    pub metadata: Value,
}

pub fn write_offline_subtitle_artifacts(
    request: OfflineSubtitleArtifactRequest<'_>,
) -> Result<OfflineSubtitleArtifactPaths, String> {
    let (text_path, metadata_path, timeline_path) = output_paths_in(
        request.data_dir,
        request.task_id,
        request.source_path,
        request.audio_dir,
    );
    if let Some(parent) = text_path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| format!("create text dir: {error}"))?;
    }

    let srt_path = subtitle_path_from_timeline(&timeline_path, "srt");
    let vtt_path = subtitle_path_from_timeline(&timeline_path, "vtt");
    let timeline_text = render_timeline_text(request.timeline, request.fallback_text);
    std::fs::write(&text_path, timeline_text)
        .map_err(|error| format!("write transcript text: {error}"))?;
    std::fs::write(
        &timeline_path,
        serde_json::to_string_pretty(request.timeline).map_err(|error| error.to_string())?,
    )
    .map_err(|error| format!("write transcript timeline: {error}"))?;
    std::fs::write(&srt_path, render_srt(request.timeline))
        .map_err(|error| format!("write transcript srt: {error}"))?;
    std::fs::write(&vtt_path, render_vtt(request.timeline))
        .map_err(|error| format!("write transcript vtt: {error}"))?;

    let mut metadata = request.metadata;
    if let Some(object) = metadata.as_object_mut() {
        object.insert("text_path".to_string(), path_value(&text_path));
        object.insert("metadata_path".to_string(), path_value(&metadata_path));
        object.insert("timeline_path".to_string(), path_value(&timeline_path));
        object.insert("srt_path".to_string(), path_value(&srt_path));
        object.insert("vtt_path".to_string(), path_value(&vtt_path));
    }
    std::fs::write(
        &metadata_path,
        serde_json::to_string_pretty(&metadata).map_err(|error| error.to_string())?,
    )
    .map_err(|error| format!("write transcript metadata: {error}"))?;

    Ok(OfflineSubtitleArtifactPaths {
        text_path,
        metadata_path,
        timeline_path,
        srt_path,
        vtt_path,
    })
}

fn path_value(path: &Path) -> Value {
    Value::String(path.to_string_lossy().to_string())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::timeline::{TimelineSegment, TranscriptTimeline};

    use super::*;

    #[test]
    fn offline_artifact_writer_writes_text_timeline_subtitles_and_metadata() {
        let temp = tempfile::tempdir().unwrap();
        let audio_dir = temp.path().join("audio");
        std::fs::create_dir_all(&audio_dir).unwrap();
        let source_path = audio_dir.join("meeting.wav");
        std::fs::write(&source_path, b"fake").unwrap();
        let timeline = TranscriptTimeline {
            task_id: "task".to_string(),
            task_name: "Task".to_string(),
            source_path: source_path.clone(),
            source_size: None,
            source_modified_ms: None,
            source_created_at_ms: None,
            source_created_at_source: None,
            media_duration_ms: Some(1_500),
            model: "Qwen3-ASR-1.7B".to_string(),
            language: "chinese".to_string(),
            diarization_profile: None,
            speakers: Vec::new(),
            processed_at_ms: 42,
            segments: vec![TimelineSegment {
                index: 0,
                audio_start_ms: 0,
                audio_end_ms: 1_500,
                absolute_start_ms: None,
                absolute_end_ms: None,
                speaker: Some("speaker_00".to_string()),
                speaker_display_name: Some("用户A".to_string()),
                overlap: false,
                text: "你好".to_string(),
            }],
        };

        let paths = write_offline_subtitle_artifacts(OfflineSubtitleArtifactRequest {
            data_dir: temp.path(),
            task_id: "task",
            source_path: &source_path,
            audio_dir: &audio_dir,
            timeline: &timeline,
            fallback_text: "fallback",
            metadata: json!({ "task_id": "task" }),
        })
        .unwrap();

        assert!(paths.text_path.is_file());
        assert!(paths.metadata_path.is_file());
        assert!(paths.timeline_path.is_file());
        assert!(paths.srt_path.is_file());
        assert!(paths.vtt_path.is_file());
        assert!(std::fs::read_to_string(paths.srt_path)
            .unwrap()
            .contains("用户A: 你好"));
        let metadata = std::fs::read_to_string(paths.metadata_path).unwrap();
        assert!(metadata.contains("srt_path"));
        assert!(metadata.contains("vtt_path"));
    }
}
