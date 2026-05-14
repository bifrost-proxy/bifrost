use std::path::{Path, PathBuf};
use std::time::Instant;

use futures_util::StreamExt;
use reqwest::header::{CONTENT_LENGTH, RANGE};
use reqwest::{Client, StatusCode};
use serde::Serialize;
use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc;

#[derive(Debug, Clone)]
pub struct DownloadRequest {
    pub url: String,
    pub dest: PathBuf,
    pub label: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct DownloadProgress {
    pub label: String,
    pub url: String,
    pub dest: String,
    pub downloaded_bytes: u64,
    pub total_bytes: Option<u64>,
    pub percent: Option<u8>,
    pub bytes_per_second: Option<u64>,
    pub eta_seconds: Option<u64>,
    pub elapsed_ms: u64,
    pub resumed: bool,
    pub complete: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct DownloadOutcome {
    pub label: String,
    pub dest: String,
    pub bytes: u64,
    pub resumed: bool,
}

pub async fn download_with_resume(
    client: &Client,
    request: DownloadRequest,
    progress_tx: Option<mpsc::UnboundedSender<DownloadProgress>>,
) -> Result<DownloadOutcome, String> {
    if request.dest.is_file() {
        let bytes = file_size(&request.dest).await?;
        emit_progress(
            &progress_tx,
            progress_snapshot(&request, bytes, Some(bytes), 0, false, true, Instant::now()),
        );
        return Ok(DownloadOutcome {
            label: request.label,
            dest: request.dest.display().to_string(),
            bytes,
            resumed: false,
        });
    }

    if let Some(parent) = request.dest.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|error| format!("create download dir {}: {error}", parent.display()))?;
    }

    let part_path = partial_path(&request.dest);
    let existing = file_size(&part_path).await.unwrap_or(0);
    let started = Instant::now();
    let mut resumed = existing > 0;
    let mut downloaded = existing;
    let mut response = open_download(client, &request.url, existing).await?;

    if existing > 0 && response.status() != StatusCode::PARTIAL_CONTENT {
        resumed = false;
        downloaded = 0;
        let _ = tokio::fs::remove_file(&part_path).await;
        response = open_download(client, &request.url, 0).await?;
    }

    if !response.status().is_success() {
        return Err(format!(
            "download {} returned {}",
            request.url,
            response.status()
        ));
    }

    let total = response_total_bytes(response.content_length(), downloaded, resumed);
    emit_progress(
        &progress_tx,
        progress_snapshot(
            &request, downloaded, total, downloaded, resumed, false, started,
        ),
    );

    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .append(resumed)
        .write(true)
        .truncate(!resumed)
        .open(&part_path)
        .await
        .map_err(|error| format!("open partial download {}: {error}", part_path.display()))?;

    let mut stream = response.bytes_stream();
    let mut last_emitted = downloaded;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| format!("download {}: {error}", request.url))?;
        file.write_all(&chunk)
            .await
            .map_err(|error| format!("write partial download {}: {error}", part_path.display()))?;
        downloaded += chunk.len() as u64;
        if downloaded.saturating_sub(last_emitted) >= 512 * 1024 {
            emit_progress(
                &progress_tx,
                progress_snapshot(
                    &request, downloaded, total, existing, resumed, false, started,
                ),
            );
            last_emitted = downloaded;
        }
    }
    file.flush()
        .await
        .map_err(|error| format!("flush partial download {}: {error}", part_path.display()))?;

    if let Some(total) = total {
        if downloaded < total {
            return Err(format!(
                "downloaded {downloaded}/{total} bytes for {}",
                request.label
            ));
        }
    }

    tokio::fs::rename(&part_path, &request.dest)
        .await
        .map_err(|error| {
            format!(
                "finalize download {} -> {}: {error}",
                part_path.display(),
                request.dest.display()
            )
        })?;

    emit_progress(
        &progress_tx,
        progress_snapshot(
            &request, downloaded, total, existing, resumed, true, started,
        ),
    );

    Ok(DownloadOutcome {
        label: request.label,
        dest: request.dest.display().to_string(),
        bytes: downloaded,
        resumed,
    })
}

async fn open_download(
    client: &Client,
    url: &str,
    resume_from: u64,
) -> Result<reqwest::Response, String> {
    let mut request = client.get(url);
    if resume_from > 0 {
        request = request.header(RANGE, build_range_header(resume_from));
    }
    request
        .send()
        .await
        .map_err(|error| format!("download {url}: {error}"))
}

fn emit_progress(
    progress_tx: &Option<mpsc::UnboundedSender<DownloadProgress>>,
    progress: DownloadProgress,
) {
    if let Some(tx) = progress_tx {
        let _ = tx.send(progress);
    }
}

fn progress_snapshot(
    request: &DownloadRequest,
    downloaded_bytes: u64,
    total_bytes: Option<u64>,
    starting_bytes: u64,
    resumed: bool,
    complete: bool,
    started: Instant,
) -> DownloadProgress {
    let elapsed_ms = started.elapsed().as_millis() as u64;
    let downloaded_this_run = downloaded_bytes.saturating_sub(starting_bytes);
    let bytes_per_second = if elapsed_ms > 0 && downloaded_this_run > 0 {
        Some((downloaded_this_run * 1000) / elapsed_ms.max(1))
    } else {
        None
    };
    let eta_seconds = match (total_bytes, bytes_per_second) {
        (Some(total), Some(rate)) if rate > 0 && downloaded_bytes < total => {
            Some((total - downloaded_bytes).div_ceil(rate))
        }
        _ => None,
    };

    DownloadProgress {
        label: request.label.clone(),
        url: request.url.clone(),
        dest: request.dest.display().to_string(),
        downloaded_bytes,
        total_bytes,
        percent: download_percent(downloaded_bytes, total_bytes),
        bytes_per_second,
        eta_seconds,
        elapsed_ms,
        resumed,
        complete,
    }
}

async fn file_size(path: &Path) -> Result<u64, String> {
    match tokio::fs::metadata(path).await {
        Ok(metadata) => Ok(metadata.len()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(0),
        Err(error) => Err(format!("stat {}: {error}", path.display())),
    }
}

fn partial_path(dest: &Path) -> PathBuf {
    let mut name = dest
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("download")
        .to_string();
    name.push_str(".part");
    dest.with_file_name(name)
}

fn build_range_header(start: u64) -> String {
    format!("bytes={start}-")
}

fn response_total_bytes(
    content_length: Option<u64>,
    downloaded: u64,
    resumed: bool,
) -> Option<u64> {
    content_length.map(|length| {
        if resumed {
            downloaded.saturating_add(length)
        } else {
            length
        }
    })
}

fn download_percent(downloaded: u64, total: Option<u64>) -> Option<u8> {
    let total = total?;
    if total == 0 {
        return Some(100);
    }
    Some(((downloaded.saturating_mul(100)) / total).min(100) as u8)
}

pub fn content_length_from_headers(headers: &reqwest::header::HeaderMap) -> Option<u64> {
    headers
        .get(CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
}

#[cfg(test)]
mod tests {
    use super::{build_range_header, download_percent, response_total_bytes};

    #[test]
    fn range_header_starts_at_existing_partial_size() {
        assert_eq!(build_range_header(4096), "bytes=4096-");
    }

    #[test]
    fn resumed_total_adds_existing_bytes_to_response_length() {
        assert_eq!(response_total_bytes(Some(600), 400, true), Some(1000));
        assert_eq!(response_total_bytes(Some(1000), 0, false), Some(1000));
    }

    #[test]
    fn download_percent_is_bounded_and_unknown_when_total_missing() {
        assert_eq!(download_percent(25, Some(100)), Some(25));
        assert_eq!(download_percent(150, Some(100)), Some(100));
        assert_eq!(download_percent(25, None), None);
    }
}
