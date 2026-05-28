use std::path::{Path, PathBuf};

use crate::runtime::text_output_dir;

pub fn output_paths_in(
    data_dir: &Path,
    task_id: &str,
    source: &Path,
    audio_dir: &Path,
) -> (PathBuf, PathBuf, PathBuf) {
    // Preserve the relative directory structure from audio_dir so that output
    // mirrors the original folder hierarchy (important for recursive tasks).
    let relative = source
        .strip_prefix(audio_dir)
        .unwrap_or(source.file_name().map(Path::new).unwrap_or(source));
    let stem = relative.with_extension("");
    let dir = text_output_dir(data_dir).join(task_id);
    (
        dir.join(format!("{}.txt", stem.display())),
        dir.join(format!("{}.json", stem.display())),
        dir.join(format!("{}.timeline.json", stem.display())),
    )
}

pub fn subtitle_path_from_timeline(timeline_path: &Path, extension: &str) -> PathBuf {
    let clean_extension = extension.trim_start_matches('.');
    let Some(file_name) = timeline_path.file_name().and_then(|name| name.to_str()) else {
        return timeline_path.with_extension(clean_extension);
    };
    if let Some(stem) = file_name.strip_suffix(".timeline.json") {
        return timeline_path.with_file_name(format!("{stem}.{clean_extension}"));
    }
    timeline_path.with_extension(clean_extension)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn subtitle_path_uses_timeline_stem() {
        let path = PathBuf::from("/tmp/meeting.timeline.json");

        assert_eq!(
            subtitle_path_from_timeline(&path, "srt"),
            PathBuf::from("/tmp/meeting.srt")
        );
        assert_eq!(
            subtitle_path_from_timeline(&path, ".vtt"),
            PathBuf::from("/tmp/meeting.vtt")
        );
    }
}
