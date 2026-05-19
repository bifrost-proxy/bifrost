use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use hyper::{Response, StatusCode};

use crate::handlers::{error_response, full_body, BoxBody};

pub(super) fn source_audio_response(path: &Path, range_header: Option<&str>) -> Response<BoxBody> {
    if !path.is_file() {
        return error_response(StatusCode::NOT_FOUND, "ASR task source audio not found");
    }
    let metadata = match std::fs::metadata(path) {
        Ok(value) => value,
        Err(error) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("read source audio metadata {}: {error}", path.display()),
            )
        }
    };
    let total_len = metadata.len();
    let range = match parse_byte_range(range_header, total_len) {
        Ok(range) => range,
        Err(()) => {
            return Response::builder()
                .status(StatusCode::RANGE_NOT_SATISFIABLE)
                .header("Content-Range", format!("bytes */{total_len}"))
                .header("Accept-Ranges", "bytes")
                .body(full_body(""))
                .unwrap()
        }
    };
    let (start, end, status) = match range {
        Some((start, end)) => (start, end, StatusCode::PARTIAL_CONTENT),
        None if total_len == 0 => (0, 0, StatusCode::OK),
        None => (0, total_len - 1, StatusCode::OK),
    };
    let bytes = if total_len == 0 {
        Ok(Vec::new())
    } else {
        read_file_range(path, start, end).map_err(|error| {
            format!(
                "read source audio {} bytes {start}-{end}: {error}",
                path.display()
            )
        })
    };
    let bytes = match bytes {
        Ok(value) => value,
        Err(error) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, &error),
    };
    let mut builder = Response::builder()
        .status(status)
        .header("Content-Type", audio_content_type(path))
        .header("Accept-Ranges", "bytes")
        .header("Content-Length", bytes.len().to_string());
    if status == StatusCode::PARTIAL_CONTENT {
        builder = builder.header("Content-Range", format!("bytes {start}-{end}/{total_len}"));
    }
    builder.body(full_body(bytes)).unwrap()
}

fn audio_content_type(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.to_ascii_lowercase())
        .as_deref()
    {
        Some("aac") => "audio/aac",
        Some("aif") | Some("aiff") => "audio/aiff",
        Some("flac") => "audio/flac",
        Some("m4a") | Some("mp4") => "audio/mp4",
        Some("mp3") => "audio/mpeg",
        Some("ogg") | Some("opus") => "audio/ogg",
        Some("wav") => "audio/wav",
        Some("webm") => "audio/webm",
        _ => "application/octet-stream",
    }
}

fn parse_byte_range(range_header: Option<&str>, total_len: u64) -> Result<Option<(u64, u64)>, ()> {
    let Some(header) = range_header else {
        return Ok(None);
    };
    let Some(range) = header.trim().strip_prefix("bytes=") else {
        return Err(());
    };
    if range.contains(',') || total_len == 0 {
        return Err(());
    }
    let Some((start_raw, end_raw)) = range.split_once('-') else {
        return Err(());
    };
    if start_raw.is_empty() {
        let suffix_len = end_raw.parse::<u64>().map_err(|_| ())?;
        if suffix_len == 0 {
            return Err(());
        }
        let start = total_len.saturating_sub(suffix_len);
        return Ok(Some((start, total_len - 1)));
    }
    let start = start_raw.parse::<u64>().map_err(|_| ())?;
    if start >= total_len {
        return Err(());
    }
    let end = if end_raw.is_empty() {
        total_len - 1
    } else {
        end_raw.parse::<u64>().map_err(|_| ())?.min(total_len - 1)
    };
    if start > end {
        return Err(());
    }
    Ok(Some((start, end)))
}

fn read_file_range(path: &Path, start: u64, end: u64) -> std::io::Result<Vec<u8>> {
    let mut file = std::fs::File::open(path)?;
    file.seek(SeekFrom::Start(start))?;
    let mut reader = file.take(end - start + 1);
    let mut bytes = Vec::with_capacity((end - start + 1).min(1024 * 1024) as usize);
    reader.read_to_end(&mut bytes)?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use http_body_util::BodyExt as _;
    use tempfile::TempDir;

    #[tokio::test]
    async fn source_audio_response_serves_byte_ranges() {
        let temp = TempDir::new().unwrap();
        let audio = temp.path().join("clip.wav");
        std::fs::write(&audio, b"0123456789").unwrap();

        let response = source_audio_response(&audio, Some("bytes=2-5"));
        assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(
            response
                .headers()
                .get("Content-Range")
                .and_then(|value| value.to_str().ok()),
            Some("bytes 2-5/10")
        );
        assert_eq!(
            response
                .headers()
                .get("Content-Type")
                .and_then(|value| value.to_str().ok()),
            Some("audio/wav")
        );
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], b"2345");
    }
}
