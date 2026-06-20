use dashmap::DashMap;
use http_body_util::BodyExt;
use hyper::{body::Incoming, Method, Request, Response, StatusCode};
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use url::Url;
use uuid::Uuid;

use super::{error_response, full_body, json_response, json_response_with_status, BoxBody};

const MAX_TAIL_LINES: usize = 20;
const DEFAULT_VIDEO_CHUNK_BYTES: u64 = 1024 * 1024;
const BEST_VIDEO_FORMAT: &str = "bv*[height>=4320]+ba/bv*[height>=2160]+ba/bv*+ba/b";

static VIDEO_DOWNLOADS: Lazy<DashMap<String, VideoDownloadTask>> = Lazy::new(DashMap::new);

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum VideoDownloadStatus {
    Queued,
    Running,
    Completed,
    Failed,
}

#[derive(Debug, Clone, Serialize)]
pub struct VideoDownloadTask {
    id: String,
    url: String,
    download_dir: String,
    status: VideoDownloadStatus,
    progress_percent: Option<f32>,
    downloaded: Option<String>,
    total: Option<String>,
    speed: Option<String>,
    eta: Option<String>,
    file_path: Option<String>,
    message: Option<String>,
    error: Option<String>,
    created_at_ms: u64,
    updated_at_ms: u64,
}

#[derive(Debug, Serialize)]
struct VideoDefaultsResponse {
    download_dir: String,
}

#[derive(Debug, Deserialize)]
struct CreateVideoDownloadRequest {
    url: String,
    #[serde(default)]
    download_dir: Option<String>,
}

#[derive(Debug, Serialize)]
struct VideoDownloadListResponse {
    downloads: Vec<VideoDownloadTask>,
}

#[derive(Debug, Serialize)]
struct VideoFileActionResponse {
    ok: bool,
}

pub async fn handle_videos(req: Request<Incoming>, path: &str) -> Response<BoxBody> {
    let method = req.method().clone();
    match (method, path) {
        (Method::GET, "/api/videos/defaults") => match default_download_dir() {
            Ok(download_dir) => json_response(&VideoDefaultsResponse {
                download_dir: download_dir.display().to_string(),
            }),
            Err(error) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &error),
        },
        (Method::GET, "/api/videos/downloads") => json_response(&VideoDownloadListResponse {
            downloads: list_downloads(),
        }),
        (Method::POST, "/api/videos/downloads") => create_download(req).await,
        (Method::GET, _) if path.starts_with("/api/videos/downloads/") => {
            let Some((id, action)) = parse_download_action_path(path) else {
                return error_response(StatusCode::NOT_FOUND, "Not Found");
            };
            match action {
                None => match VIDEO_DOWNLOADS.get(&id) {
                    Some(task) => json_response(task.value()),
                    None => error_response(StatusCode::NOT_FOUND, "Video download not found"),
                },
                Some("file") => {
                    let range_header = req
                        .headers()
                        .get("range")
                        .and_then(|value| value.to_str().ok());
                    video_file_response(&id, range_header)
                }
                _ => error_response(StatusCode::NOT_FOUND, "Not Found"),
            }
        }
        (Method::POST, _) if path.starts_with("/api/videos/downloads/") => {
            let Some((id, Some(action))) = parse_download_action_path(path) else {
                return error_response(StatusCode::NOT_FOUND, "Not Found");
            };
            match action {
                "open" => open_video_file(&id, false),
                "reveal" => open_video_file(&id, true),
                "retry" => retry_download(&id),
                _ => error_response(StatusCode::NOT_FOUND, "Not Found"),
            }
        }
        _ => error_response(StatusCode::NOT_FOUND, "Not Found"),
    }
}

async fn create_download(req: Request<Incoming>) -> Response<BoxBody> {
    let body = match req.collect().await {
        Ok(collected) => collected.to_bytes(),
        Err(error) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                &format!("Failed to read body: {error}"),
            )
        }
    };

    let request: CreateVideoDownloadRequest = match serde_json::from_slice(&body) {
        Ok(request) => request,
        Err(error) => {
            return error_response(StatusCode::BAD_REQUEST, &format!("Invalid JSON: {error}"))
        }
    };

    if let Err(error) = validate_youtube_url(&request.url) {
        return error_response(StatusCode::BAD_REQUEST, &error);
    }

    let download_dir = match resolve_download_dir(request.download_dir.as_deref()) {
        Ok(download_dir) => download_dir,
        Err(error) => return error_response(StatusCode::BAD_REQUEST, &error),
    };

    let id = Uuid::new_v4().to_string();
    let now = now_ms();
    let task = VideoDownloadTask {
        id: id.clone(),
        url: request.url.clone(),
        download_dir: download_dir.display().to_string(),
        status: VideoDownloadStatus::Queued,
        progress_percent: None,
        downloaded: None,
        total: None,
        speed: None,
        eta: None,
        file_path: None,
        message: Some("queued".to_string()),
        error: None,
        created_at_ms: now,
        updated_at_ms: now,
    };
    VIDEO_DOWNLOADS.insert(id.clone(), task.clone());

    tokio::spawn(run_download_task(id, request.url, download_dir));

    json_response_with_status(StatusCode::ACCEPTED, &task)
}

fn list_downloads() -> Vec<VideoDownloadTask> {
    let mut downloads = VIDEO_DOWNLOADS
        .iter()
        .map(|entry| entry.value().clone())
        .collect::<Vec<_>>();
    downloads.sort_by_key(|task| std::cmp::Reverse(task.created_at_ms));
    downloads
}

fn parse_download_action_path(path: &str) -> Option<(String, Option<&str>)> {
    let rest = path.strip_prefix("/api/videos/downloads/")?;
    if rest.is_empty() {
        return None;
    }
    let mut parts = rest.split('/');
    let id = parts.next()?.to_string();
    let action = parts.next();
    if parts.next().is_some() {
        return None;
    }
    Some((id, action))
}

async fn run_download_task(id: String, url: String, download_dir: PathBuf) {
    update_task(&id, |task| {
        task.status = VideoDownloadStatus::Running;
        task.message = Some("starting yt-dlp".to_string());
    });

    if let Err(error) = tokio::fs::create_dir_all(&download_dir).await {
        fail_task(&id, format!("Failed to create download directory: {error}"));
        return;
    }

    if let Err(error) = ensure_command_available("yt-dlp").await {
        fail_task(&id, error);
        return;
    }

    let mut command = Command::new("yt-dlp");
    command
        .arg("--no-playlist")
        .arg("--continue")
        .arg("--newline")
        .arg("--progress")
        .arg("--retries")
        .arg("10")
        .arg("--fragment-retries")
        .arg("10")
        .arg("--concurrent-fragments")
        .arg("8")
        .arg("--merge-output-format")
        .arg("mp4")
        .arg("--paths")
        .arg(&download_dir)
        .arg("--output")
        .arg("%(title).180B [%(id)s].%(ext)s")
        .arg("--print")
        .arg("after_move:filepath")
        .arg("-f")
        .arg(BEST_VIDEO_FORMAT)
        .arg(&url)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            fail_task(&id, format!("Failed to spawn yt-dlp: {error}"));
            return;
        }
    };

    let Some(stdout) = child.stdout.take() else {
        fail_task(&id, "Failed to capture yt-dlp stdout".to_string());
        return;
    };
    let Some(stderr) = child.stderr.take() else {
        fail_task(&id, "Failed to capture yt-dlp stderr".to_string());
        return;
    };

    let stderr_tail = std::sync::Arc::new(tokio::sync::Mutex::new(VecDeque::<String>::new()));
    let stdout_task = tokio::spawn(read_ytdlp_stdout(id.clone(), download_dir.clone(), stdout));
    let stderr_task = tokio::spawn(read_ytdlp_stderr(id.clone(), stderr, stderr_tail.clone()));

    let status = match child.wait().await {
        Ok(status) => status,
        Err(error) => {
            fail_task(&id, format!("Failed while waiting for yt-dlp: {error}"));
            return;
        }
    };
    let _ = stdout_task.await;
    let _ = stderr_task.await;

    if status.success() {
        if let Some(file_path) = infer_completed_file_path(&url, &download_dir) {
            update_task(&id, |task| {
                task.file_path = Some(file_path.display().to_string());
            });
        }
        let file_size = completed_file_size_label(&id).await;
        update_task(&id, |task| {
            task.status = VideoDownloadStatus::Completed;
            task.progress_percent = Some(100.0);
            if let Some(file_size) = file_size.clone() {
                task.downloaded = Some(file_size.clone());
                task.total = Some(file_size);
            }
            task.speed = None;
            task.eta = None;
            task.message = Some("completed".to_string());
        });
    } else {
        let tail = stderr_tail
            .lock()
            .await
            .iter()
            .cloned()
            .collect::<Vec<_>>()
            .join("\n");
        fail_task(&id, format!("yt-dlp exited with {status}: {tail}"));
    }
}

fn retry_download(id: &str) -> Response<BoxBody> {
    let (url, download_dir, task) = match prepare_retry_download(id) {
        Ok(value) => value,
        Err((status, error)) => return error_response(status, &error),
    };
    tokio::spawn(run_download_task(id.to_string(), url, download_dir));
    json_response_with_status(StatusCode::ACCEPTED, &task)
}

fn prepare_retry_download(
    id: &str,
) -> Result<(String, PathBuf, VideoDownloadTask), (StatusCode, String)> {
    let mut task = VIDEO_DOWNLOADS.get_mut(id).ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            "Video download not found".to_string(),
        )
    })?;
    if !matches!(&task.status, VideoDownloadStatus::Failed) {
        return Err((
            StatusCode::CONFLICT,
            "Only failed video downloads can be retried".to_string(),
        ));
    }
    let download_dir = PathBuf::from(&task.download_dir);
    if !download_dir.is_absolute() {
        return Err((
            StatusCode::BAD_REQUEST,
            "Download directory must be an absolute path".to_string(),
        ));
    }
    task.status = VideoDownloadStatus::Queued;
    task.progress_percent = None;
    task.downloaded = None;
    task.total = None;
    task.speed = None;
    task.eta = None;
    task.file_path = None;
    task.message = Some("queued for retry".to_string());
    task.error = None;
    task.updated_at_ms = now_ms();

    Ok((task.url.clone(), download_dir, task.clone()))
}

fn video_file_response(id: &str, range_header: Option<&str>) -> Response<BoxBody> {
    let path = match completed_file_path(id) {
        Ok(path) => path,
        Err((status, error)) => return error_response(status, &error),
    };
    let metadata = match std::fs::metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("read video metadata {}: {error}", path.display()),
            )
        }
    };
    let total_len = metadata.len();
    let range = match parse_video_byte_range(range_header, total_len) {
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
        None => (
            0,
            total_len
                .saturating_sub(1)
                .min(DEFAULT_VIDEO_CHUNK_BYTES - 1),
            StatusCode::PARTIAL_CONTENT,
        ),
    };
    let bytes = if total_len == 0 {
        Ok(Vec::new())
    } else {
        read_file_range(&path, start, end)
            .map_err(|error| format!("read video {} bytes {start}-{end}: {error}", path.display()))
    };
    let bytes = match bytes {
        Ok(bytes) => bytes,
        Err(error) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, &error),
    };

    let mut builder = Response::builder()
        .status(status)
        .header("Content-Type", video_content_type(&path))
        .header("Accept-Ranges", "bytes")
        .header("Content-Length", bytes.len().to_string())
        .header("Content-Disposition", content_disposition_inline(&path));
    if status == StatusCode::PARTIAL_CONTENT {
        builder = builder.header("Content-Range", format!("bytes {start}-{end}/{total_len}"));
    }
    builder.body(full_body(bytes)).unwrap()
}

fn open_video_file(id: &str, reveal: bool) -> Response<BoxBody> {
    let path = match completed_file_path(id) {
        Ok(path) => path,
        Err((status, error)) => return error_response(status, &error),
    };
    match open_path(&path, reveal) {
        Ok(()) => json_response(&VideoFileActionResponse { ok: true }),
        Err(error) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &error),
    }
}

fn completed_file_path(id: &str) -> Result<PathBuf, (StatusCode, String)> {
    let Some(task) = VIDEO_DOWNLOADS.get(id) else {
        return Err((
            StatusCode::NOT_FOUND,
            "Video download not found".to_string(),
        ));
    };
    if !matches!(&task.status, VideoDownloadStatus::Completed) {
        return Err((
            StatusCode::CONFLICT,
            "Video download is not completed yet".to_string(),
        ));
    }
    let Some(file_path) = &task.file_path else {
        return Err((
            StatusCode::NOT_FOUND,
            "Completed video file path is not available".to_string(),
        ));
    };
    let path = PathBuf::from(file_path);
    if !path.is_file() {
        return Err((
            StatusCode::NOT_FOUND,
            format!("Video file does not exist: {}", path.display()),
        ));
    }
    Ok(path)
}

fn parse_video_byte_range(
    range_header: Option<&str>,
    total_len: u64,
) -> Result<Option<(u64, u64)>, ()> {
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
    let mut bytes = Vec::with_capacity((end - start + 1).min(DEFAULT_VIDEO_CHUNK_BYTES) as usize);
    reader.read_to_end(&mut bytes)?;
    Ok(bytes)
}

fn video_content_type(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.to_ascii_lowercase())
        .as_deref()
    {
        Some("mp4") | Some("m4v") => "video/mp4",
        Some("webm") => "video/webm",
        Some("mov") => "video/quicktime",
        Some("mkv") => "video/x-matroska",
        _ => "application/octet-stream",
    }
}

fn content_disposition_inline(path: &Path) -> String {
    let filename = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("video");
    let escaped = filename.replace('\\', "\\\\").replace('"', "\\\"");
    format!("inline; filename=\"{escaped}\"")
}

fn open_path(path: &Path, reveal: bool) -> Result<(), String> {
    let mut command = open_path_command(path, reveal)?;
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("open {}: {error}", path.display()))?;
    Ok(())
}

fn open_path_command(path: &Path, reveal: bool) -> Result<std::process::Command, String> {
    #[cfg(target_os = "macos")]
    {
        let mut command = std::process::Command::new("open");
        if reveal {
            command.arg("-R");
        }
        command.arg(path);
        return Ok(command);
    }
    #[cfg(target_os = "windows")]
    {
        let mut command = std::process::Command::new("explorer");
        if reveal {
            command.arg(format!("/select,{}", path.display()));
        } else {
            command.arg(path);
        }
        return Ok(command);
    }
    #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
    {
        let target = if reveal {
            path.parent().unwrap_or(path)
        } else {
            path
        };
        let mut command = std::process::Command::new("xdg-open");
        command.arg(target);
        return Ok(command);
    }
    #[allow(unreachable_code)]
    Err("opening files is not supported on this platform".to_string())
}

async fn read_ytdlp_stdout(id: String, download_dir: PathBuf, stdout: tokio::process::ChildStdout) {
    let mut lines = BufReader::new(stdout).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if let Some(progress) = parse_ytdlp_progress(&line) {
            update_task(&id, |task| {
                task.progress_percent = Some(progress.percent);
                task.downloaded = progress.downloaded.clone();
                task.total = progress.total.clone();
                task.speed = progress.speed.clone();
                task.eta = progress.eta.clone();
                task.message = Some(line.clone());
            });
        } else if let Some(file_path) = extract_output_path(&line, &download_dir) {
            update_task(&id, |task| {
                task.file_path = Some(file_path.display().to_string());
                task.message = Some("finalizing file".to_string());
            });
        } else if !line.trim().is_empty() {
            update_task(&id, |task| {
                task.message = Some(line.clone());
            });
        }
    }
}

async fn read_ytdlp_stderr(
    id: String,
    stderr: tokio::process::ChildStderr,
    stderr_tail: std::sync::Arc<tokio::sync::Mutex<VecDeque<String>>>,
) {
    let mut lines = BufReader::new(stderr).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        {
            let mut tail = stderr_tail.lock().await;
            push_tail(&mut tail, line.clone());
        }
        if !line.trim().is_empty() {
            update_task(&id, |task| {
                task.message = Some(line.clone());
            });
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
struct ParsedProgress {
    percent: f32,
    downloaded: Option<String>,
    total: Option<String>,
    speed: Option<String>,
    eta: Option<String>,
}

fn parse_ytdlp_progress(line: &str) -> Option<ParsedProgress> {
    let rest = line.strip_prefix("[download]")?.trim();
    let parts = rest.split_whitespace().collect::<Vec<_>>();
    let percent = parts.first()?.trim_end_matches('%').parse::<f32>().ok()?;
    let total = parts
        .windows(2)
        .find(|window| window[0] == "of")
        .map(|window| window[1].to_string());
    let downloaded = if parts.len() >= 3 && parts[1] == "of" {
        None
    } else {
        parts
            .windows(3)
            .find(|window| window[1] == "of")
            .map(|window| window[0].to_string())
    };
    let speed = parts
        .windows(2)
        .find(|window| window[0] == "at")
        .map(|window| window[1].to_string());
    let eta = parts
        .windows(2)
        .find(|window| window[0] == "ETA")
        .map(|window| window[1].to_string());

    Some(ParsedProgress {
        percent,
        downloaded,
        total,
        speed,
        eta,
    })
}

fn extract_output_path(line: &str, download_dir: &Path) -> Option<PathBuf> {
    let trimmed = line.trim();
    let download_dir_str = download_dir.to_str()?;
    if trimmed.starts_with(download_dir_str) {
        return Some(PathBuf::from(trimmed));
    }

    const MERGE_PREFIX: &str = "[Merger] Merging formats into ";
    trimmed
        .strip_prefix(MERGE_PREFIX)
        .map(|rest| PathBuf::from(rest.trim_matches('"')))
}

fn infer_completed_file_path(url: &str, download_dir: &Path) -> Option<PathBuf> {
    let video_id = youtube_video_id(url)?;
    let mut candidates = std::fs::read_dir(download_dir)
        .ok()?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            let name = path.file_name()?.to_str()?.to_string();
            if !path.is_file() || !name.contains(&format!("[{video_id}]")) || name.contains(".part")
            {
                return None;
            }
            let modified = entry
                .metadata()
                .and_then(|metadata| metadata.modified())
                .ok();
            Some((modified, path))
        })
        .collect::<Vec<_>>();
    candidates.sort_by_key(|(modified, _)| std::cmp::Reverse(*modified));
    candidates.into_iter().map(|(_, path)| path).next()
}

fn youtube_video_id(raw: &str) -> Option<String> {
    let parsed = Url::parse(raw).ok()?;
    let host = parsed.host_str()?.to_ascii_lowercase();
    if host == "youtu.be" {
        return parsed
            .path_segments()
            .and_then(|mut segments| segments.next())
            .filter(|id| !id.is_empty())
            .map(ToString::to_string);
    }
    parsed
        .query_pairs()
        .find(|(key, _)| key == "v")
        .map(|(_, value)| value.into_owned())
        .filter(|id| !id.is_empty())
}

fn update_task(id: &str, update: impl FnOnce(&mut VideoDownloadTask)) {
    if let Some(mut task) = VIDEO_DOWNLOADS.get_mut(id) {
        update(&mut task);
        task.updated_at_ms = now_ms();
    }
}

fn fail_task(id: &str, error: String) {
    update_task(id, |task| {
        task.status = VideoDownloadStatus::Failed;
        task.error = Some(error.clone());
        task.message = Some(error);
    });
}

async fn ensure_command_available(name: &str) -> Result<(), String> {
    let status = Command::new(name)
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .map_err(|_| missing_command_message(name))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("Required command `{name}` returned {status}"))
    }
}

fn missing_command_message(name: &str) -> String {
    #[cfg(target_os = "macos")]
    let install_hint = "Install it with Homebrew.";
    #[cfg(target_os = "windows")]
    let install_hint = "Install it with winget or from the official release.";
    #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
    let install_hint = "Install it with your system package manager.";

    format!("Missing required command `{name}`. {install_hint}")
}

async fn completed_file_size_label(id: &str) -> Option<String> {
    let path = VIDEO_DOWNLOADS
        .get(id)
        .and_then(|task| task.file_path.as_ref().map(PathBuf::from))?;
    let metadata = tokio::fs::metadata(path).await.ok()?;
    Some(format_bytes(metadata.len()))
}

fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.2}{}", UNITS[unit])
    }
}

fn validate_youtube_url(raw: &str) -> Result<(), String> {
    let parsed = Url::parse(raw).map_err(|error| format!("Invalid URL: {error}"))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err("Only http and https URLs are supported".to_string());
    }
    let host = parsed.host_str().unwrap_or_default().to_ascii_lowercase();
    if host == "youtu.be" || host.ends_with(".youtube.com") || host == "youtube.com" {
        Ok(())
    } else {
        Err("Only YouTube URLs are supported".to_string())
    }
}

fn resolve_download_dir(input: Option<&str>) -> Result<PathBuf, String> {
    let trimmed = input.unwrap_or("").trim();
    if trimmed.is_empty() {
        return default_download_dir();
    }
    let expanded = expand_home(trimmed)?;
    if !expanded.is_absolute() {
        return Err("Download directory must be an absolute path".to_string());
    }
    Ok(expanded)
}

fn default_download_dir() -> Result<PathBuf, String> {
    let Some(base) =
        dirs::download_dir().or_else(|| dirs::home_dir().map(|home| home.join("Downloads")))
    else {
        return Err("Could not determine the system Downloads directory".to_string());
    };
    Ok(base.join("YouTube"))
}

fn expand_home(path: &str) -> Result<PathBuf, String> {
    if path == "~" {
        return dirs::home_dir().ok_or_else(|| "Could not determine home directory".to_string());
    }
    if let Some(rest) = path.strip_prefix("~/") {
        let Some(home) = dirs::home_dir() else {
            return Err("Could not determine home directory".to_string());
        };
        return Ok(home.join(rest));
    }
    Ok(PathBuf::from(path))
}

fn push_tail(tail: &mut VecDeque<String>, line: String) {
    if tail.len() >= MAX_TAIL_LINES {
        tail.pop_front();
    }
    tail.push_back(line);
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn parses_ytdlp_progress() {
        let progress =
            parse_ytdlp_progress("[download]  18.4% of    2.31GiB at    8.45MiB/s ETA 03:48")
                .expect("progress");
        assert_eq!(progress.percent, 18.4);
        assert_eq!(progress.total.as_deref(), Some("2.31GiB"));
        assert_eq!(progress.speed.as_deref(), Some("8.45MiB/s"));
        assert_eq!(progress.eta.as_deref(), Some("03:48"));
    }

    #[test]
    fn validates_youtube_only() {
        assert!(validate_youtube_url("https://www.youtube.com/watch?v=abc").is_ok());
        assert!(validate_youtube_url("https://youtu.be/abc").is_ok());
        assert!(validate_youtube_url("https://example.com/watch?v=abc").is_err());
    }

    #[test]
    fn extracts_youtube_video_ids() {
        assert_eq!(
            youtube_video_id("https://www.youtube.com/watch?v=H_BCdTjO1zw").as_deref(),
            Some("H_BCdTjO1zw")
        );
        assert_eq!(
            youtube_video_id("https://youtu.be/3i5_v_sUZ04").as_deref(),
            Some("3i5_v_sUZ04")
        );
    }

    #[test]
    fn infer_completed_file_path_skips_partials() {
        let temp = TempDir::new().unwrap();
        let partial = temp.path().join("Title [abc].f399.mp4.part");
        let complete = temp.path().join("Title [abc].mp4");
        std::fs::write(&partial, b"partial").unwrap();
        std::fs::write(&complete, b"complete").unwrap();

        assert_eq!(
            infer_completed_file_path("https://www.youtube.com/watch?v=abc", temp.path()),
            Some(complete)
        );
    }

    #[test]
    fn expands_home_download_dir() {
        let absolute_dir = std::env::temp_dir().join("videos");
        let resolved =
            resolve_download_dir(absolute_dir.to_str()).expect("absolute temp path should work");
        assert_eq!(resolved, absolute_dir);
        assert!(resolve_download_dir(Some("relative/videos")).is_err());
    }

    #[test]
    fn formats_completed_file_size() {
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(2_495_946_174), "2.32GiB");
    }

    #[test]
    fn missing_command_message_uses_platform_hint() {
        let message = missing_command_message("yt-dlp");
        assert!(message.contains("yt-dlp"));

        #[cfg(target_os = "macos")]
        assert!(message.contains("Homebrew"));

        #[cfg(target_os = "windows")]
        assert!(message.contains("winget"));

        #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
        assert!(message.contains("package manager"));
    }

    #[test]
    fn open_path_command_targets_platform_file_manager() {
        #[cfg(target_os = "windows")]
        let path = Path::new("C:\\tmp\\video.mp4");
        #[cfg(not(target_os = "windows"))]
        let path = Path::new("/tmp/video.mp4");

        let command = open_path_command(path, false).expect("open command");
        let program = command.get_program().to_string_lossy();
        let args = command
            .get_args()
            .map(|arg| arg.to_string_lossy().to_string())
            .collect::<Vec<_>>();

        #[cfg(target_os = "macos")]
        {
            assert_eq!(program, "open");
            assert_eq!(args, vec!["/tmp/video.mp4"]);
        }

        #[cfg(target_os = "windows")]
        {
            assert_eq!(program, "explorer");
            assert_eq!(args, vec!["C:\\tmp\\video.mp4"]);
        }

        #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
        {
            assert_eq!(program, "xdg-open");
            assert_eq!(args, vec!["/tmp/video.mp4"]);
        }
    }

    #[test]
    fn reveal_path_command_targets_platform_file_manager() {
        #[cfg(target_os = "windows")]
        let path = Path::new("C:\\tmp\\video.mp4");
        #[cfg(not(target_os = "windows"))]
        let path = Path::new("/tmp/video.mp4");

        let command = open_path_command(path, true).expect("reveal command");
        let program = command.get_program().to_string_lossy();
        let args = command
            .get_args()
            .map(|arg| arg.to_string_lossy().to_string())
            .collect::<Vec<_>>();

        #[cfg(target_os = "macos")]
        {
            assert_eq!(program, "open");
            assert_eq!(args, vec!["-R", "/tmp/video.mp4"]);
        }

        #[cfg(target_os = "windows")]
        {
            assert_eq!(program, "explorer");
            assert_eq!(args, vec!["/select,C:\\tmp\\video.mp4"]);
        }

        #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
        {
            assert_eq!(program, "xdg-open");
            assert_eq!(args, vec!["/tmp"]);
        }
    }

    #[tokio::test]
    async fn video_file_response_serves_byte_ranges() {
        let temp = TempDir::new().unwrap();
        let video = temp.path().join("clip.mp4");
        std::fs::write(&video, b"0123456789").unwrap();
        let id = Uuid::new_v4().to_string();
        let now = now_ms();
        VIDEO_DOWNLOADS.insert(
            id.clone(),
            VideoDownloadTask {
                id: id.clone(),
                url: "https://youtu.be/abc".to_string(),
                download_dir: temp.path().display().to_string(),
                status: VideoDownloadStatus::Completed,
                progress_percent: Some(100.0),
                downloaded: Some("10 B".to_string()),
                total: Some("10 B".to_string()),
                speed: None,
                eta: None,
                file_path: Some(video.display().to_string()),
                message: Some("completed".to_string()),
                error: None,
                created_at_ms: now,
                updated_at_ms: now,
            },
        );

        let response = video_file_response(&id, Some("bytes=2-5"));
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
            Some("video/mp4")
        );
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], b"2345");

        VIDEO_DOWNLOADS.remove(&id);
    }

    #[tokio::test]
    async fn video_file_response_defaults_to_initial_partial_chunk() {
        let temp = TempDir::new().unwrap();
        let video = temp.path().join("clip.webm");
        std::fs::write(&video, b"0123456789").unwrap();
        let id = Uuid::new_v4().to_string();
        let now = now_ms();
        VIDEO_DOWNLOADS.insert(
            id.clone(),
            VideoDownloadTask {
                id: id.clone(),
                url: "https://youtu.be/abc".to_string(),
                download_dir: temp.path().display().to_string(),
                status: VideoDownloadStatus::Completed,
                progress_percent: Some(100.0),
                downloaded: Some("10 B".to_string()),
                total: Some("10 B".to_string()),
                speed: None,
                eta: None,
                file_path: Some(video.display().to_string()),
                message: Some("completed".to_string()),
                error: None,
                created_at_ms: now,
                updated_at_ms: now,
            },
        );

        let response = video_file_response(&id, None);
        assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(
            response
                .headers()
                .get("Content-Range")
                .and_then(|value| value.to_str().ok()),
            Some("bytes 0-9/10")
        );
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], b"0123456789");

        VIDEO_DOWNLOADS.remove(&id);
    }

    #[test]
    fn completed_file_path_rejects_unfinished_tasks() {
        let id = Uuid::new_v4().to_string();
        let now = now_ms();
        VIDEO_DOWNLOADS.insert(
            id.clone(),
            VideoDownloadTask {
                id: id.clone(),
                url: "https://youtu.be/abc".to_string(),
                download_dir: std::env::temp_dir().display().to_string(),
                status: VideoDownloadStatus::Running,
                progress_percent: Some(10.0),
                downloaded: None,
                total: None,
                speed: None,
                eta: None,
                file_path: None,
                message: Some("running".to_string()),
                error: None,
                created_at_ms: now,
                updated_at_ms: now,
            },
        );

        let error = completed_file_path(&id).unwrap_err();
        assert_eq!(error.0, StatusCode::CONFLICT);

        VIDEO_DOWNLOADS.remove(&id);
    }

    #[test]
    fn prepare_retry_download_resets_failed_task_for_resume() {
        let id = Uuid::new_v4().to_string();
        let now = now_ms();
        VIDEO_DOWNLOADS.insert(
            id.clone(),
            VideoDownloadTask {
                id: id.clone(),
                url: "https://youtu.be/abc".to_string(),
                download_dir: std::env::temp_dir().display().to_string(),
                status: VideoDownloadStatus::Failed,
                progress_percent: Some(42.0),
                downloaded: Some("739.02MiB".to_string()),
                total: Some("2.31GiB".to_string()),
                speed: Some("2.82MiB/s".to_string()),
                eta: Some("02:31".to_string()),
                file_path: None,
                message: Some("network error".to_string()),
                error: Some("network error".to_string()),
                created_at_ms: now,
                updated_at_ms: now,
            },
        );

        let (url, dir, task) = prepare_retry_download(&id).expect("retryable");
        assert_eq!(url, "https://youtu.be/abc");
        assert_eq!(dir, std::env::temp_dir());
        assert!(matches!(task.status, VideoDownloadStatus::Queued));
        assert_eq!(task.message.as_deref(), Some("queued for retry"));
        assert!(task.error.is_none());
        assert!(task.progress_percent.is_none());

        VIDEO_DOWNLOADS.remove(&id);
    }

    #[test]
    fn prepare_retry_download_rejects_running_task() {
        let id = Uuid::new_v4().to_string();
        let now = now_ms();
        VIDEO_DOWNLOADS.insert(
            id.clone(),
            VideoDownloadTask {
                id: id.clone(),
                url: "https://youtu.be/abc".to_string(),
                download_dir: std::env::temp_dir().display().to_string(),
                status: VideoDownloadStatus::Running,
                progress_percent: Some(10.0),
                downloaded: None,
                total: None,
                speed: None,
                eta: None,
                file_path: None,
                message: Some("running".to_string()),
                error: None,
                created_at_ms: now,
                updated_at_ms: now,
            },
        );

        let error = prepare_retry_download(&id).unwrap_err();
        assert_eq!(error.0, StatusCode::CONFLICT);

        VIDEO_DOWNLOADS.remove(&id);
    }

    #[test]
    fn parse_download_action_path_handles_detail_and_actions() {
        assert_eq!(
            parse_download_action_path("/api/videos/downloads/task-1"),
            Some(("task-1".to_string(), None))
        );
        assert_eq!(
            parse_download_action_path("/api/videos/downloads/task-1/file"),
            Some(("task-1".to_string(), Some("file")))
        );
        assert!(parse_download_action_path("/api/videos/downloads/task-1/file/extra").is_none());
    }
}
