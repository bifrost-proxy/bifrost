use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use futures_util::StreamExt;
use sha1::{Digest, Sha1};
use tracing::warn;
use url::Url;

use crate::im_gateway::feishu::FeishuProvider;
use crate::im_gateway::types::ImProviderConfig;

const MAX_IMAGE_BYTES: u64 = 10 * 1024 * 1024;
const MAX_IMAGE_CACHE_ENTRIES: usize = 256;
static IMAGE_KEY_CACHE: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();

/// Resolve Markdown image references into Feishu `image_key` references.
///
/// Fenced code blocks are preserved. Existing `img_*` references are already
/// valid Feishu resources and pass through unchanged. Failures are isolated to
/// the affected image so progress-card updates and terminal delivery continue.
pub async fn render_markdown_images(
    feishu: &FeishuProvider,
    provider: &ImProviderConfig,
    markdown: &str,
    base_dir: Option<&Path>,
) -> String {
    if !markdown.contains("![") {
        return markdown.to_string();
    }

    let mut output = String::with_capacity(markdown.len());
    let mut code_fence: Option<String> = None;
    for line in markdown.split('\n') {
        let trimmed = line.trim_start();
        if let Some(fence) = code_fence.as_ref() {
            output.push_str(line);
            output.push('\n');
            if trimmed.starts_with(fence.as_str()) && trimmed[fence.len()..].trim().is_empty() {
                code_fence = None;
            }
            continue;
        }
        if let Some(fence) = detect_code_fence(trimmed) {
            code_fence = Some(fence);
            output.push_str(line);
            output.push('\n');
            continue;
        }
        output.push_str(&render_line(feishu, provider, line, base_dir).await);
        output.push('\n');
    }
    if output.ends_with('\n') && !markdown.ends_with('\n') {
        output.pop();
    }
    output
}

async fn render_line(
    feishu: &FeishuProvider,
    provider: &ImProviderConfig,
    line: &str,
    base_dir: Option<&Path>,
) -> String {
    let mut result = String::with_capacity(line.len());
    let mut pos = 0;
    while pos < line.len() {
        if line.as_bytes()[pos] == b'!' && pos + 1 < line.len() && line.as_bytes()[pos + 1] == b'['
        {
            if let Some((alt, raw_destination, end)) =
                crate::im_gateway::markdown_converter::parse_image_syntax(line, pos + 2)
            {
                let destination = markdown_destination(&raw_destination);
                if destination.starts_with("img_") {
                    result.push_str(&line[pos..end]);
                    pos = end;
                    continue;
                }
                let uploaded =
                    if destination.starts_with("http://") || destination.starts_with("https://") {
                        upload_remote_image(feishu, provider, destination).await
                    } else if let Some(path) = resolve_local_path(destination, base_dir) {
                        upload_local_image(feishu, provider, path).await
                    } else {
                        Err(bifrost_core::BifrostError::Config(
                            "relative image has no runner work directory".to_string(),
                        ))
                    };
                match uploaded {
                    Ok(image_key) => result.push_str(&format!("![{}]({})", alt, image_key)),
                    Err(error) => {
                        warn!(
                            provider_id = %provider.id,
                            image_source = %safe_image_source(destination),
                            error = %error,
                            "failed to resolve Feishu Markdown image"
                        );
                        result.push_str(&failed_image_markdown(&alt));
                    }
                }
                pos = end;
                continue;
            }
        }
        let ch = line[pos..].chars().next().expect("valid UTF-8 boundary");
        result.push(ch);
        pos += ch.len_utf8();
    }
    result
}

async fn upload_local_image(
    feishu: &FeishuProvider,
    provider: &ImProviderConfig,
    path: PathBuf,
) -> bifrost_core::Result<String> {
    let metadata = tokio::fs::metadata(&path).await.map_err(|error| {
        bifrost_core::BifrostError::Io(std::io::Error::new(
            error.kind(),
            format!("failed to stat Feishu Markdown image: {error}"),
        ))
    })?;
    if !metadata.is_file() {
        return Err(bifrost_core::BifrostError::Config(
            "Feishu Markdown image is not a file".to_string(),
        ));
    }
    if metadata.len() > MAX_IMAGE_BYTES {
        return Err(bifrost_core::BifrostError::Config(format!(
            "Feishu Markdown image exceeds {MAX_IMAGE_BYTES} bytes"
        )));
    }
    let canonical = path.canonicalize().unwrap_or_else(|_| path.clone());
    let modified_ms = metadata
        .modified()
        .ok()
        .and_then(|value| value.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|value| value.as_millis())
        .unwrap_or_default();
    let cache_key = format!(
        "{}:local:{}:{}:{}",
        provider.id,
        canonical.display(),
        metadata.len(),
        modified_ms
    );
    if let Some(image_key) = cached_image_key(&cache_key) {
        return Ok(image_key);
    }
    let bytes = tokio::fs::read(&path).await.map_err(|error| {
        bifrost_core::BifrostError::Io(std::io::Error::new(
            error.kind(),
            format!("failed to read Feishu Markdown image: {error}"),
        ))
    })?;
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("agent-image.png");
    let uploaded = feishu
        .upload_image(
            provider,
            "message",
            file_name,
            bytes,
            mime_type_for_path(&path),
        )
        .await?;
    cache_image_key(cache_key, uploaded.image_key.clone());
    Ok(uploaded.image_key)
}

async fn upload_remote_image(
    feishu: &FeishuProvider,
    provider: &ImProviderConfig,
    source: &str,
) -> bifrost_core::Result<String> {
    let parsed = Url::parse(source).map_err(|error| {
        bifrost_core::BifrostError::Config(format!("invalid remote image URL: {error}"))
    })?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(bifrost_core::BifrostError::Config(
            "remote image URL must use HTTP or HTTPS".to_string(),
        ));
    }
    let http = bifrost_core::outbound_reqwest_client().map_err(|error| {
        bifrost_core::BifrostError::Network(format!("build image downloader failed: {error}"))
    })?;
    let response = http
        .get(source)
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .map_err(|error| {
            bifrost_core::BifrostError::Network(format!(
                "download remote image failed: {}",
                bifrost_core::format_reqwest_error(&error)
            ))
        })?;
    if !response.status().is_success() {
        return Err(bifrost_core::BifrostError::Network(format!(
            "download remote image returned HTTP {}",
            response.status()
        )));
    }
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.split(';').next().unwrap_or(value).trim().to_string());
    if content_type
        .as_deref()
        .is_some_and(|value| !value.starts_with("image/"))
    {
        return Err(bifrost_core::BifrostError::Network(
            "remote Markdown image did not return an image Content-Type".to_string(),
        ));
    }
    if response
        .content_length()
        .is_some_and(|length| length > MAX_IMAGE_BYTES)
    {
        return Err(bifrost_core::BifrostError::Config(format!(
            "remote Markdown image exceeds {MAX_IMAGE_BYTES} bytes"
        )));
    }
    let mut bytes = Vec::new();
    let mut body = response.bytes_stream();
    while let Some(chunk) = body.next().await {
        let chunk = chunk.map_err(|error| {
            bifrost_core::BifrostError::Network(format!("read remote image failed: {error}"))
        })?;
        if bytes.len().saturating_add(chunk.len()) > MAX_IMAGE_BYTES as usize {
            return Err(bifrost_core::BifrostError::Config(format!(
                "remote Markdown image exceeds {MAX_IMAGE_BYTES} bytes"
            )));
        }
        bytes.extend_from_slice(&chunk);
    }
    if bytes.is_empty() {
        return Err(bifrost_core::BifrostError::Config(
            "remote Markdown image is empty".to_string(),
        ));
    }
    let mut hasher = Sha1::new();
    hasher.update(source.as_bytes());
    hasher.update(&bytes);
    let digest = format!("{:x}", hasher.finalize());
    let cache_key = format!("{}:remote:{digest}", provider.id);
    if let Some(image_key) = cached_image_key(&cache_key) {
        return Ok(image_key);
    }
    let file_name = parsed
        .path_segments()
        .and_then(|mut parts| parts.next_back())
        .filter(|value| !value.is_empty())
        .unwrap_or("agent-remote-image.png");
    let uploaded = feishu
        .upload_image(
            provider,
            "message",
            file_name,
            bytes,
            content_type.as_deref(),
        )
        .await?;
    cache_image_key(cache_key, uploaded.image_key.clone());
    Ok(uploaded.image_key)
}

fn resolve_local_path(destination: &str, base_dir: Option<&Path>) -> Option<PathBuf> {
    if let Some(path) = destination.strip_prefix("file://") {
        return Some(PathBuf::from(path));
    }
    let path = PathBuf::from(destination);
    if path.is_absolute() {
        Some(path)
    } else {
        base_dir.map(|base| base.join(path))
    }
}

fn detect_code_fence(trimmed: &str) -> Option<String> {
    let marker = trimmed.chars().next()?;
    if marker != '`' && marker != '~' {
        return None;
    }
    let length = trimmed.chars().take_while(|value| *value == marker).count();
    (length >= 3).then(|| trimmed[..length].to_string())
}

fn markdown_destination(raw: &str) -> &str {
    let trimmed = raw.trim();
    let trimmed = trimmed
        .strip_prefix('<')
        .and_then(|value| value.strip_suffix('>'))
        .unwrap_or(trimmed);
    trimmed
        .split_once(" \"")
        .or_else(|| trimmed.split_once(" '"))
        .map(|(path, _)| path.trim())
        .unwrap_or(trimmed)
}

fn mime_type_for_path(path: &Path) -> Option<&'static str> {
    match path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        "bmp" => Some("image/bmp"),
        "ico" => Some("image/x-icon"),
        "tif" | "tiff" => Some("image/tiff"),
        "heic" => Some("image/heic"),
        _ => None,
    }
}

fn failed_image_markdown(alt: &str) -> String {
    let label = if alt.trim().is_empty() {
        "图片"
    } else {
        alt.trim()
    };
    format!("[{label} 未能上传]")
}

fn safe_image_source(source: &str) -> String {
    if source.starts_with("http://") || source.starts_with("https://") {
        Url::parse(source)
            .ok()
            .and_then(|url| url.host_str().map(str::to_string))
            .unwrap_or_else(|| "remote-url".to_string())
    } else {
        Path::new(source)
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("local-image")
            .to_string()
    }
}

fn cached_image_key(key: &str) -> Option<String> {
    IMAGE_KEY_CACHE
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .ok()
        .and_then(|cache| cache.get(key).cloned())
}

fn cache_image_key(key: String, image_key: String) {
    if let Ok(mut cache) = IMAGE_KEY_CACHE
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
    {
        if cache.len() >= MAX_IMAGE_CACHE_ENTRIES && !cache.contains_key(&key) {
            if let Some(eviction_key) = cache.keys().next().cloned() {
                cache.remove(&eviction_key);
            }
        }
        cache.insert(key, image_key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::im_gateway::progress_card::tests::{
        mock_feishu_provider, spawn_mock_feishu_progress_server,
    };

    #[test]
    fn markdown_destination_strips_wrappers_and_titles() {
        assert_eq!(markdown_destination("<./chart.png>"), "./chart.png");
        assert_eq!(markdown_destination("./chart.png \"Chart\""), "./chart.png");
        assert_eq!(markdown_destination("./chart.png 'Chart'"), "./chart.png");
    }

    #[test]
    fn fallback_does_not_expose_image_source() {
        assert_eq!(failed_image_markdown("chart"), "[chart 未能上传]");
        assert_eq!(failed_image_markdown(" "), "[图片 未能上传]");
        assert_eq!(
            safe_image_source("https://example.com/private/chart.png?token=secret"),
            "example.com"
        );
        assert_eq!(safe_image_source("https://"), "remote-url");
        assert_eq!(safe_image_source("/private/path/chart.png"), "chart.png");
        assert_eq!(safe_image_source("/"), "local-image");
    }

    #[test]
    fn local_path_and_mime_helpers_cover_supported_variants() {
        let base = Path::new("/runner");
        assert_eq!(
            resolve_local_path("file:///tmp/a.png", None),
            Some(PathBuf::from("/tmp/a.png"))
        );
        assert_eq!(
            resolve_local_path("/tmp/b.jpg", None),
            Some(PathBuf::from("/tmp/b.jpg"))
        );
        assert_eq!(
            resolve_local_path("c.gif", Some(base)),
            Some(PathBuf::from("/runner/c.gif"))
        );
        assert_eq!(resolve_local_path("c.gif", None), None);

        for (name, expected) in [
            ("a.png", Some("image/png")),
            ("a.JPG", Some("image/jpeg")),
            ("a.jpeg", Some("image/jpeg")),
            ("a.gif", Some("image/gif")),
            ("a.webp", Some("image/webp")),
            ("a.bmp", Some("image/bmp")),
            ("a.ico", Some("image/x-icon")),
            ("a.tif", Some("image/tiff")),
            ("a.tiff", Some("image/tiff")),
            ("a.heic", Some("image/heic")),
            ("a.bin", None),
        ] {
            assert_eq!(mime_type_for_path(Path::new(name)), expected, "{name}");
        }
    }

    #[test]
    fn image_cache_is_bounded_and_updates_existing_keys() {
        let prefix = format!("bounded-cache-{}", std::process::id());
        for index in 0..=MAX_IMAGE_CACHE_ENTRIES {
            cache_image_key(format!("{prefix}-{index}"), format!("img_{index}"));
        }
        cache_image_key(format!("{prefix}-256"), "img_updated".to_string());
        assert_eq!(
            cached_image_key(&format!("{prefix}-256")).as_deref(),
            Some("img_updated")
        );
        let cache = IMAGE_KEY_CACHE.get().unwrap().lock().unwrap();
        assert!(cache.len() <= MAX_IMAGE_CACHE_ENTRIES);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn existing_image_key_and_fenced_example_do_not_upload() {
        let server = spawn_mock_feishu_progress_server().await;
        let provider = mock_feishu_provider(&server.base_url);
        let markdown = "![ready](img_v3_existing)\n```md\n![sample](./missing.png)\n```";

        let rendered =
            render_markdown_images(&FeishuProvider::new(), &provider, markdown, None).await;

        assert_eq!(rendered, markdown);
        assert!(server
            .image_upload_payloads
            .lock()
            .expect("image upload payloads lock")
            .is_empty());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn missing_local_image_degrades_only_the_image() {
        let server = spawn_mock_feishu_progress_server().await;
        let provider = mock_feishu_provider(&server.base_url);
        let temp = tempfile::tempdir().expect("temp image dir");

        let rendered = render_markdown_images(
            &FeishuProvider::new(),
            &provider,
            "before ![chart](missing.png) after",
            Some(temp.path()),
        )
        .await;

        assert_eq!(rendered, "before [chart 未能上传] after");
        assert!(server
            .image_upload_payloads
            .lock()
            .expect("image upload payloads lock")
            .is_empty());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn relative_image_without_base_directory_degrades() {
        let server = spawn_mock_feishu_progress_server().await;
        let provider = mock_feishu_provider(&server.base_url);
        let rendered = render_markdown_images(
            &FeishuProvider::new(),
            &provider,
            "![relative](chart.png)",
            None,
        )
        .await;
        assert_eq!(rendered, "[relative 未能上传]");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn directory_and_oversized_local_images_are_rejected() {
        let server = spawn_mock_feishu_progress_server().await;
        let provider = mock_feishu_provider(&server.base_url);
        let temp = tempfile::tempdir().expect("temp image dir");
        let oversized = temp.path().join("oversized.png");
        std::fs::File::create(&oversized)
            .unwrap()
            .set_len(MAX_IMAGE_BYTES + 1)
            .unwrap();
        let markdown = format!(
            "![directory]({})\n![oversized]({})",
            temp.path().display(),
            oversized.display()
        );
        let rendered =
            render_markdown_images(&FeishuProvider::new(), &provider, &markdown, None).await;
        assert_eq!(rendered, "[directory 未能上传]\n[oversized 未能上传]");
        assert!(server.image_upload_payloads.lock().unwrap().is_empty());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn file_url_and_absolute_local_path_upload_and_cache() {
        let server = spawn_mock_feishu_progress_server().await;
        let mut provider = mock_feishu_provider(&server.base_url);
        provider.id = "feishu-local-variants-test".to_string();
        let temp = tempfile::tempdir().expect("temp image dir");
        let path = temp.path().join("variant.PNG");
        tokio::fs::write(&path, b"variant-image").await.unwrap();
        let markdown = format!(
            "![file](file://{})\n![absolute]({})",
            path.display(),
            path.display()
        );
        let rendered =
            render_markdown_images(&FeishuProvider::new(), &provider, &markdown, None).await;
        assert_eq!(
            rendered,
            "![file](img_v3_progress_inline)\n![absolute](img_v3_progress_inline)"
        );
        assert_eq!(server.image_upload_payloads.lock().unwrap().len(), 1);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn remote_image_is_downloaded_uploaded_and_cached() {
        let server = spawn_mock_feishu_progress_server().await;
        let mut provider = mock_feishu_provider(&server.base_url);
        provider.id = "feishu-remote-image-test".to_string();
        let origin = server
            .base_url
            .strip_suffix("/open-apis")
            .expect("mock origin");
        let markdown = format!("![remote]({origin}/remote.png)");

        let first =
            render_markdown_images(&FeishuProvider::new(), &provider, &markdown, None).await;
        let second =
            render_markdown_images(&FeishuProvider::new(), &provider, &markdown, None).await;

        assert_eq!(first, "![remote](img_v3_progress_inline)");
        assert_eq!(second, first);
        assert_eq!(
            server
                .image_upload_payloads
                .lock()
                .expect("image upload payloads lock")
                .len(),
            1
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn oversized_remote_image_is_rejected_before_body_buffering() {
        let server = spawn_mock_feishu_progress_server().await;
        let mut provider = mock_feishu_provider(&server.base_url);
        provider.id = "feishu-oversized-remote-image-test".to_string();
        let origin = server
            .base_url
            .strip_suffix("/open-apis")
            .expect("mock origin");

        let rendered = render_markdown_images(
            &FeishuProvider::new(),
            &provider,
            &format!("![large]({origin}/too-large.png)"),
            None,
        )
        .await;

        assert_eq!(rendered, "[large 未能上传]");
        assert!(server
            .image_upload_payloads
            .lock()
            .expect("image upload payloads lock")
            .is_empty());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn remote_http_content_type_and_empty_body_errors_degrade() {
        let server = spawn_mock_feishu_progress_server().await;
        let mut provider = mock_feishu_provider(&server.base_url);
        provider.id = "feishu-remote-errors-test".to_string();
        let origin = server.base_url.strip_suffix("/open-apis").unwrap();
        let markdown = format!(
            "![missing]({origin}/missing.png)\n![text]({origin}/not-image.png)\n![empty]({origin}/empty.png)"
        );
        let rendered =
            render_markdown_images(&FeishuProvider::new(), &provider, &markdown, None).await;
        assert_eq!(
            rendered,
            "[missing 未能上传]\n[text 未能上传]\n[empty 未能上传]"
        );
        assert!(server.image_upload_payloads.lock().unwrap().is_empty());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn invalid_and_non_http_remote_urls_are_rejected() {
        let server = spawn_mock_feishu_progress_server().await;
        let provider = mock_feishu_provider(&server.base_url);
        assert!(
            upload_remote_image(&FeishuProvider::new(), &provider, "not a url")
                .await
                .is_err()
        );
        assert!(
            upload_remote_image(&FeishuProvider::new(), &provider, "ftp://example.com/a.png")
                .await
                .is_err()
        );
    }
}
