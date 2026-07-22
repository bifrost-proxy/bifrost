use super::*;
use std::collections::HashSet;
use std::time::Duration;

use sha1::{Digest, Sha1};
use url::Url;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(super) struct AgentReplyImageCacheKey {
    provider_id: String,
    path: PathBuf,
    len: u64,
    modified_ms: u128,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct AgentReplyLocalImage {
    pub(super) alt: String,
    pub(super) path: PathBuf,
}

/// Convert agent Markdown before it is placed into a Feishu card.
///
/// Local image references such as `![chart](./chart.png)` or `![chart](/tmp/chart.png)`
/// are uploaded once per provider/file fingerprint and rewritten to Feishu's
/// `![chart](image_key)` form. The later Markdown converter preserves image_key
/// references so Feishu cards can render them as inline images.
pub(super) async fn render_agent_markdown_for_feishu(
    feishu: &crate::im_gateway::feishu::FeishuProvider,
    provider: &ImProviderConfig,
    markdown: &str,
    base_dir: Option<&Path>,
) -> String {
    if !markdown.contains("![") {
        return markdown.to_string();
    }

    let mut output = String::with_capacity(markdown.len());
    let mut inside_code_block = false;
    let mut code_fence: Option<String> = None;

    for line in markdown.split('\n') {
        let trimmed = line.trim_start();
        if inside_code_block {
            output.push_str(line);
            output.push('\n');
            if let Some(ref fence) = code_fence {
                if trimmed.starts_with(fence.as_str()) && trimmed[fence.len()..].trim().is_empty() {
                    inside_code_block = false;
                    code_fence = None;
                }
            }
            continue;
        }

        if let Some(fence) = detect_markdown_code_fence(trimmed) {
            inside_code_block = true;
            code_fence = Some(fence);
            output.push_str(line);
            output.push('\n');
            continue;
        }

        output.push_str(
            &rewrite_agent_markdown_images_in_line(feishu, provider, line, base_dir).await,
        );
        output.push('\n');
    }

    if output.ends_with('\n') && !markdown.ends_with('\n') {
        output.pop();
    }
    output
}

pub(super) fn detect_markdown_code_fence(trimmed: &str) -> Option<String> {
    let fence_char = trimmed.chars().next()?;
    if fence_char != '`' && fence_char != '~' {
        return None;
    }
    let fence_len = trimmed.chars().take_while(|&c| c == fence_char).count();
    if fence_len < 3 {
        return None;
    }
    Some(trimmed[..fence_len].to_string())
}

pub(super) async fn rewrite_agent_markdown_images_in_line(
    feishu: &crate::im_gateway::feishu::FeishuProvider,
    provider: &ImProviderConfig,
    line: &str,
    base_dir: Option<&Path>,
) -> String {
    if !line.contains("![") {
        return line.to_string();
    }

    let mut result = String::with_capacity(line.len());
    let mut pos = 0;
    while pos < line.len() {
        if line.as_bytes()[pos] == b'!' && pos + 1 < line.len() && line.as_bytes()[pos + 1] == b'['
        {
            if let Some((alt, url, end)) =
                crate::im_gateway::markdown_converter::parse_image_syntax(line, pos + 2)
            {
                if is_local_markdown_image_candidate(&url) {
                    if let Some(image_path) = resolve_agent_reply_image_path(&url, base_dir) {
                        match upload_agent_reply_image_cached(feishu, provider, &image_path).await {
                            Ok(image_key) => {
                                result.push_str(&format!("![{}]({})", alt, image_key));
                                pos = end;
                                continue;
                            }
                            Err(error) => {
                                warn!(
                                    provider_id = %provider.id,
                                    path = %image_path.display(),
                                    error = %error,
                                    "failed to upload local image referenced by agent markdown"
                                );
                            }
                        }
                    } else {
                        warn!(
                            provider_id = %provider.id,
                            image_url = %url,
                            "failed to resolve local image referenced by agent markdown"
                        );
                    }
                    result.push_str(&local_image_fallback_markdown(&alt, &url));
                    pos = end;
                    continue;
                }
            }
        }

        let ch = line[pos..].chars().next().unwrap();
        result.push(ch);
        pos += ch.len_utf8();
    }
    result
}

pub(super) fn local_image_fallback_markdown(alt: &str, _url: &str) -> String {
    let label = if alt.trim().is_empty() {
        "图片".to_string()
    } else {
        alt.trim().to_string()
    };
    format!("[{} 未能上传]", label)
}

pub(super) fn collect_agent_reply_local_images(
    markdown: &str,
    base_dir: Option<&Path>,
) -> Vec<AgentReplyLocalImage> {
    if !markdown.contains("![") {
        return Vec::new();
    }

    let mut images = Vec::new();
    let mut seen = HashSet::new();
    let mut inside_code_block = false;
    let mut code_fence: Option<String> = None;

    for line in markdown.split('\n') {
        let trimmed = line.trim_start();
        if inside_code_block {
            if let Some(ref fence) = code_fence {
                if trimmed.starts_with(fence.as_str()) && trimmed[fence.len()..].trim().is_empty() {
                    inside_code_block = false;
                    code_fence = None;
                }
            }
            continue;
        }

        if let Some(fence) = detect_markdown_code_fence(trimmed) {
            inside_code_block = true;
            code_fence = Some(fence);
            continue;
        }

        let mut pos = 0;
        while pos < line.len() {
            if line.as_bytes()[pos] == b'!'
                && pos + 1 < line.len()
                && line.as_bytes()[pos + 1] == b'['
            {
                if let Some((alt, url, end)) =
                    crate::im_gateway::markdown_converter::parse_image_syntax(line, pos + 2)
                {
                    if let Some(image_path) = resolve_agent_reply_image_path(&url, base_dir) {
                        let dedupe_key = image_path
                            .canonicalize()
                            .unwrap_or_else(|_| image_path.clone());
                        if seen.insert(dedupe_key) {
                            images.push(AgentReplyLocalImage {
                                alt,
                                path: image_path,
                            });
                        }
                    }
                    pos = end;
                    continue;
                }
            }

            let ch = line[pos..].chars().next().unwrap();
            pos += ch.len_utf8();
        }
    }

    images
}

#[cfg(test)]
pub(super) fn strip_agent_reply_local_images(markdown: &str, base_dir: Option<&Path>) -> String {
    if !markdown.contains("![") {
        return markdown.to_string();
    }

    let mut output = String::with_capacity(markdown.len());
    let mut inside_code_block = false;
    let mut code_fence: Option<String> = None;

    for line in markdown.split('\n') {
        let trimmed = line.trim_start();
        if inside_code_block {
            output.push_str(line);
            output.push('\n');
            if let Some(ref fence) = code_fence {
                if trimmed.starts_with(fence.as_str()) && trimmed[fence.len()..].trim().is_empty() {
                    inside_code_block = false;
                    code_fence = None;
                }
            }
            continue;
        }

        if let Some(fence) = detect_markdown_code_fence(trimmed) {
            inside_code_block = true;
            code_fence = Some(fence);
            output.push_str(line);
            output.push('\n');
            continue;
        }

        output.push_str(&strip_agent_reply_local_images_in_line(line, base_dir));
        output.push('\n');
    }

    if output.ends_with('\n') && !markdown.ends_with('\n') {
        output.pop();
    }
    let stripped = output.trim();
    if stripped.is_empty() {
        "已生成图片，正在发送原图。".to_string()
    } else {
        stripped.to_string()
    }
}

#[cfg(test)]
pub(super) fn prepare_agent_reply_text_and_images(
    markdown: &str,
    base_dir: Option<&Path>,
) -> (String, Vec<AgentReplyLocalImage>) {
    let images = collect_agent_reply_local_images(markdown, base_dir);
    let text = if images.is_empty() {
        markdown.to_string()
    } else {
        strip_agent_reply_local_images(markdown, base_dir)
    };
    (text, images)
}

pub(super) async fn prepare_agent_reply_text_and_images_with_downloads(
    markdown: &str,
    base_dir: Option<&Path>,
) -> (
    String,
    Vec<AgentReplyLocalImage>,
    Vec<AgentReplyLocalAttachment>,
) {
    let mut images = collect_agent_reply_local_images(markdown, base_dir);
    let mut attachments = Vec::new();
    let mut remote_urls_to_strip = HashSet::new();
    let mut linked_image_urls_to_strip = HashSet::new();
    collect_agent_reply_local_attachment_links(markdown, base_dir, &mut images, &mut attachments);
    for remote_image in collect_agent_reply_remote_image_links(markdown) {
        match download_agent_reply_remote_image(&remote_image).await {
            Ok(local_image) => {
                remote_urls_to_strip.insert(remote_image.url);
                images.push(local_image);
            }
            Err(error) => {
                warn!(
                    image_url = %remote_image.url,
                    error = %error,
                    "failed to download remote image referenced by agent markdown"
                );
            }
        }
    }
    for remote_attachment in collect_agent_reply_remote_attachment_links(markdown) {
        match download_agent_reply_remote_attachment(&remote_attachment).await {
            Ok(downloaded) => {
                if is_image_mime_or_path(downloaded.mime_type.as_deref(), &downloaded.path) {
                    linked_image_urls_to_strip.insert(remote_attachment.url);
                    images.push(AgentReplyLocalImage {
                        alt: remote_attachment.label,
                        path: downloaded.path,
                    });
                } else {
                    attachments.push(AgentReplyLocalAttachment {
                        label: remote_attachment.label,
                        path: downloaded.path,
                        mime_type: downloaded.mime_type,
                    });
                }
            }
            Err(error) => {
                warn!(
                    attachment_url = %remote_attachment.url,
                    error = %error,
                    "failed to download remote attachment referenced by agent markdown"
                );
            }
        }
    }

    let text = if images.is_empty() {
        markdown.to_string()
    } else {
        strip_agent_reply_resolved_images(
            markdown,
            base_dir,
            &remote_urls_to_strip,
            &linked_image_urls_to_strip,
        )
    };
    (text, images, attachments)
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(super) struct AgentReplyRemoteImage {
    pub(super) alt: String,
    pub(super) url: String,
}

pub(super) fn collect_agent_reply_remote_image_links(markdown: &str) -> Vec<AgentReplyRemoteImage> {
    if !markdown.contains("![") {
        return Vec::new();
    }

    let mut images = Vec::new();
    let mut seen = HashSet::new();
    let mut inside_code_block = false;
    let mut code_fence: Option<String> = None;

    for line in markdown.split('\n') {
        let trimmed = line.trim_start();
        if inside_code_block {
            if let Some(ref fence) = code_fence {
                if trimmed.starts_with(fence.as_str()) && trimmed[fence.len()..].trim().is_empty() {
                    inside_code_block = false;
                    code_fence = None;
                }
            }
            continue;
        }

        if let Some(fence) = detect_markdown_code_fence(trimmed) {
            inside_code_block = true;
            code_fence = Some(fence);
            continue;
        }

        let mut pos = 0;
        while pos < line.len() {
            if line.as_bytes()[pos] == b'!'
                && pos + 1 < line.len()
                && line.as_bytes()[pos + 1] == b'['
            {
                if let Some((alt, url, end)) =
                    crate::im_gateway::markdown_converter::parse_image_syntax(line, pos + 2)
                {
                    let destination = markdown_image_destination(&url).to_string();
                    if is_remote_markdown_image_attachment_candidate(&destination)
                        && seen.insert(destination.clone())
                    {
                        images.push(AgentReplyRemoteImage {
                            alt,
                            url: destination,
                        });
                    }
                    pos = end;
                    continue;
                }
            }

            let ch = line[pos..].chars().next().unwrap();
            pos += ch.len_utf8();
        }
    }

    images
}

pub(super) fn is_remote_markdown_image_attachment_candidate(raw_url: &str) -> bool {
    let destination = markdown_image_destination(raw_url);
    let Ok(parsed) = Url::parse(destination) else {
        return false;
    };
    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        return false;
    }
    let host = parsed.host_str().unwrap_or_default().to_ascii_lowercase();
    let path = parsed.path().to_ascii_lowercase();
    if host == "www.google.com" && path == "/s2/favicons" {
        return false;
    }
    if host.ends_with("chatgpt.com") && path.contains("/backend-api/estuary/content") {
        return true;
    }
    if host.ends_with("files.oaiusercontent.com")
        || host.ends_with("oaidalleapiprodscus.blob.core.windows.net")
        || host.ends_with("cdn.openai.com")
    {
        return true;
    }
    if image_extension_from_path(&path).is_some() {
        return true;
    }
    parsed.query_pairs().any(|(key, value)| {
        let key = key.to_ascii_lowercase();
        (key == "filename" || key == "file" || key == "name")
            && image_extension_from_path(&value.to_ascii_lowercase()).is_some()
    })
}

async fn download_agent_reply_remote_image(
    image: &AgentReplyRemoteImage,
) -> bifrost_core::Result<AgentReplyLocalImage> {
    let http = bifrost_core::outbound_reqwest_client().map_err(|error| {
        bifrost_core::BifrostError::Network(format!(
            "build agent reply image downloader failed: {error}"
        ))
    })?;
    let response = http
        .get(&image.url)
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .map_err(|error| {
            bifrost_core::BifrostError::Network(format!(
                "download agent reply image failed: {}",
                bifrost_core::format_reqwest_error(&error)
            ))
        })?;
    if !response.status().is_success() {
        return Err(bifrost_core::BifrostError::Network(format!(
            "download agent reply image returned HTTP {}",
            response.status()
        )));
    }
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.split(';').next().unwrap_or(value).trim().to_string());
    let bytes = response.bytes().await.map_err(|error| {
        bifrost_core::BifrostError::Network(format!("read agent reply image body failed: {error}"))
    })?;
    if bytes.is_empty() {
        return Err(bifrost_core::BifrostError::Network(
            "download agent reply image returned empty body".to_string(),
        ));
    }
    if bytes.len() as u64 > MAX_AGENT_REPLY_IMAGE_BYTES {
        return Err(bifrost_core::BifrostError::Config(format!(
            "downloaded agent reply image exceeds {MAX_AGENT_REPLY_IMAGE_BYTES} bytes"
        )));
    }
    if let Some(ref content_type) = content_type {
        if !content_type.starts_with("image/") {
            return Err(bifrost_core::BifrostError::Network(format!(
                "download agent reply attachment is not an image: {content_type}"
            )));
        }
    }

    let mut hasher = Sha1::new();
    hasher.update(image.url.as_bytes());
    hasher.update(&bytes);
    let digest = format!("{:x}", hasher.finalize());
    let ext = content_type
        .as_deref()
        .and_then(image_extension_from_content_type)
        .or_else(|| {
            Url::parse(&image.url)
                .ok()
                .and_then(|url| image_extension_from_path(url.path()))
        })
        .unwrap_or("bin");
    let dir = bifrost_storage::data_dir()
        .join("agent")
        .join("im_gateway")
        .join("attachments")
        .join("agent_reply_markdown");
    tokio::fs::create_dir_all(&dir).await.map_err(|error| {
        bifrost_core::BifrostError::Io(std::io::Error::new(
            error.kind(),
            format!("create agent reply markdown attachment dir failed: {error}"),
        ))
    })?;
    let path = dir.join(format!("agent-reply-image-{digest}.{ext}"));
    tokio::fs::write(&path, &bytes).await.map_err(|error| {
        bifrost_core::BifrostError::Io(std::io::Error::new(
            error.kind(),
            format!(
                "write agent reply markdown attachment '{}' failed: {error}",
                path.display()
            ),
        ))
    })?;
    Ok(AgentReplyLocalImage {
        alt: image.alt.clone(),
        path,
    })
}

fn strip_agent_reply_resolved_images(
    markdown: &str,
    base_dir: Option<&Path>,
    remote_urls: &HashSet<String>,
    linked_image_urls: &HashSet<String>,
) -> String {
    if !markdown.contains('[') {
        return markdown.to_string();
    }

    let mut output = String::with_capacity(markdown.len());
    let mut inside_code_block = false;
    let mut code_fence: Option<String> = None;

    for line in markdown.split('\n') {
        let trimmed = line.trim_start();
        if inside_code_block {
            output.push_str(line);
            output.push('\n');
            if let Some(ref fence) = code_fence {
                if trimmed.starts_with(fence.as_str()) && trimmed[fence.len()..].trim().is_empty() {
                    inside_code_block = false;
                    code_fence = None;
                }
            }
            continue;
        }

        if let Some(fence) = detect_markdown_code_fence(trimmed) {
            inside_code_block = true;
            code_fence = Some(fence);
            output.push_str(line);
            output.push('\n');
            continue;
        }

        output.push_str(&strip_agent_reply_resolved_images_in_line(
            line,
            base_dir,
            remote_urls,
            linked_image_urls,
        ));
        output.push('\n');
    }

    if output.ends_with('\n') && !markdown.ends_with('\n') {
        output.pop();
    }
    let stripped = output.trim();
    if stripped.is_empty() {
        "已生成图片，正在发送原图。".to_string()
    } else {
        stripped.to_string()
    }
}

fn strip_agent_reply_resolved_images_in_line(
    line: &str,
    base_dir: Option<&Path>,
    remote_urls: &HashSet<String>,
    linked_image_urls: &HashSet<String>,
) -> String {
    if !line.contains('[') {
        return line.to_string();
    }

    let mut result = String::with_capacity(line.len());
    let mut pos = 0;
    while pos < line.len() {
        if line.as_bytes()[pos] == b'!' && pos + 1 < line.len() && line.as_bytes()[pos + 1] == b'['
        {
            if let Some((_alt, url, end)) =
                crate::im_gateway::markdown_converter::parse_image_syntax(line, pos + 2)
            {
                let destination = markdown_image_destination(&url);
                if resolve_agent_reply_image_path(destination, base_dir).is_some()
                    || remote_urls.contains(destination)
                {
                    pos = end;
                    continue;
                }
            }
        }
        if line.as_bytes()[pos] == b'['
            && (pos == 0 || line.as_bytes()[pos.saturating_sub(1)] != b'!')
        {
            if let Some((_label, url, end)) =
                crate::im_gateway::markdown_converter::parse_image_syntax(line, pos + 1)
            {
                let destination = markdown_image_destination(&url);
                if linked_image_urls.contains(destination) {
                    pos = end;
                    continue;
                }
            }
        }

        let ch = line[pos..].chars().next().unwrap();
        result.push(ch);
        pos += ch.len_utf8();
    }
    result
}

pub(super) fn image_extension_from_content_type(content_type: &str) -> Option<&'static str> {
    match content_type.to_ascii_lowercase().as_str() {
        "image/png" => Some("png"),
        "image/jpeg" | "image/jpg" => Some("jpg"),
        "image/gif" => Some("gif"),
        "image/webp" => Some("webp"),
        "image/bmp" => Some("bmp"),
        _ => None,
    }
}

pub(super) fn image_extension_from_path(path: &str) -> Option<&'static str> {
    let path = path.split('?').next().unwrap_or(path);
    let ext = std::path::Path::new(path)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    match ext.as_str() {
        "png" => Some("png"),
        "jpg" | "jpeg" => Some("jpg"),
        "gif" => Some("gif"),
        "webp" => Some("webp"),
        "bmp" => Some("bmp"),
        _ => None,
    }
}

#[cfg(test)]
fn strip_agent_reply_local_images_in_line(line: &str, base_dir: Option<&Path>) -> String {
    if !line.contains("![") {
        return line.to_string();
    }

    let mut result = String::with_capacity(line.len());
    let mut pos = 0;
    while pos < line.len() {
        if line.as_bytes()[pos] == b'!' && pos + 1 < line.len() && line.as_bytes()[pos + 1] == b'['
        {
            if let Some((_alt, url, end)) =
                crate::im_gateway::markdown_converter::parse_image_syntax(line, pos + 2)
            {
                if resolve_agent_reply_image_path(&url, base_dir).is_some() {
                    pos = end;
                    continue;
                }
            }
        }

        let ch = line[pos..].chars().next().unwrap();
        result.push(ch);
        pos += ch.len_utf8();
    }
    result
}

pub(super) async fn upload_agent_reply_image_cached(
    feishu: &crate::im_gateway::feishu::FeishuProvider,
    provider: &ImProviderConfig,
    image_path: &Path,
) -> bifrost_core::Result<String> {
    let metadata = tokio::fs::metadata(image_path).await.map_err(|error| {
        bifrost_core::BifrostError::Io(std::io::Error::new(
            error.kind(),
            format!(
                "failed to stat agent reply image '{}': {}",
                image_path.display(),
                error
            ),
        ))
    })?;
    if !metadata.is_file() {
        return Err(bifrost_core::BifrostError::Config(format!(
            "agent reply image is not a file: {}",
            image_path.display()
        )));
    }
    if metadata.len() > MAX_AGENT_REPLY_IMAGE_BYTES {
        return Err(bifrost_core::BifrostError::Config(format!(
            "agent reply image exceeds {} bytes: {}",
            MAX_AGENT_REPLY_IMAGE_BYTES,
            image_path.display()
        )));
    }

    let cache_key = AgentReplyImageCacheKey {
        provider_id: provider.id.clone(),
        path: image_path
            .canonicalize()
            .unwrap_or_else(|_| image_path.to_path_buf()),
        len: metadata.len(),
        modified_ms: metadata
            .modified()
            .ok()
            .and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|duration| duration.as_millis())
            .unwrap_or_default(),
    };

    if let Some(image_key) = agent_reply_image_cache()
        .lock()
        .ok()
        .and_then(|cache| cache.get(&cache_key).cloned())
    {
        return Ok(image_key);
    }

    let bytes = tokio::fs::read(image_path).await.map_err(|error| {
        bifrost_core::BifrostError::Io(std::io::Error::new(
            error.kind(),
            format!(
                "failed to read agent reply image '{}': {}",
                image_path.display(),
                error
            ),
        ))
    })?;
    let file_name = image_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("agent-reply-image.png");
    let uploaded = feishu
        .upload_image(
            provider,
            "message",
            file_name,
            bytes,
            mime_type_for_image_path(image_path),
        )
        .await?;
    let image_key = uploaded.image_key;

    if let Ok(mut cache) = agent_reply_image_cache().lock() {
        cache.insert(cache_key, image_key.clone());
    }
    Ok(image_key)
}

pub(super) async fn upload_agent_reply_image_for_im(
    client: &ImProviderClient,
    provider: &ImProviderConfig,
    image_path: &Path,
) -> bifrost_core::Result<String> {
    let metadata = tokio::fs::metadata(image_path).await.map_err(|error| {
        bifrost_core::BifrostError::Io(std::io::Error::new(
            error.kind(),
            format!(
                "failed to stat agent reply image '{}': {}",
                image_path.display(),
                error
            ),
        ))
    })?;
    if !metadata.is_file() {
        return Err(bifrost_core::BifrostError::Config(format!(
            "agent reply image is not a file: {}",
            image_path.display()
        )));
    }
    if metadata.len() > MAX_AGENT_REPLY_IMAGE_BYTES {
        return Err(bifrost_core::BifrostError::Config(format!(
            "agent reply image exceeds {} bytes: {}",
            MAX_AGENT_REPLY_IMAGE_BYTES,
            image_path.display()
        )));
    }

    let cache_key = AgentReplyImageCacheKey {
        provider_id: provider.id.clone(),
        path: image_path
            .canonicalize()
            .unwrap_or_else(|_| image_path.to_path_buf()),
        len: metadata.len(),
        modified_ms: metadata
            .modified()
            .ok()
            .and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|duration| duration.as_millis())
            .unwrap_or_default(),
    };

    if let Some(image_key) = agent_reply_image_cache()
        .lock()
        .ok()
        .and_then(|cache| cache.get(&cache_key).cloned())
    {
        return Ok(image_key);
    }

    let bytes = tokio::fs::read(image_path).await.map_err(|error| {
        bifrost_core::BifrostError::Io(std::io::Error::new(
            error.kind(),
            format!(
                "failed to read agent reply image '{}': {}",
                image_path.display(),
                error
            ),
        ))
    })?;
    let file_name = image_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("agent-reply-image.png");
    let uploaded = client
        .upload_image(
            provider,
            "message",
            file_name,
            bytes,
            mime_type_for_image_path(image_path),
        )
        .await?;
    let image_key = uploaded.image_key;

    if let Ok(mut cache) = agent_reply_image_cache().lock() {
        cache.insert(cache_key, image_key.clone());
    }
    Ok(image_key)
}

pub(super) fn agent_reply_image_cache() -> &'static Mutex<HashMap<AgentReplyImageCacheKey, String>>
{
    AGENT_REPLY_IMAGE_UPLOAD_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(super) fn provider_agent_work_dir(provider: &ImProviderConfig) -> Option<PathBuf> {
    provider
        .agent_config
        .as_ref()
        .and_then(|config| config.work_dir.as_deref())
        .map(str::trim)
        .filter(|work_dir| !work_dir.is_empty())
        .map(PathBuf::from)
}

pub(super) fn resolve_agent_reply_image_path(
    raw_url: &str,
    base_dir: Option<&Path>,
) -> Option<PathBuf> {
    let destination = markdown_image_destination(raw_url);
    if !is_local_markdown_image_candidate(destination) {
        return None;
    }

    if let Some(path) = destination.strip_prefix("file://") {
        return Some(PathBuf::from(path));
    }

    let path = PathBuf::from(destination);
    if path.is_absolute() {
        Some(path)
    } else {
        base_dir.map(|base_dir| base_dir.join(path))
    }
}

pub(super) fn is_local_markdown_image_candidate(raw_url: &str) -> bool {
    let destination = markdown_image_destination(raw_url);
    !destination.is_empty()
        && !destination.starts_with("http://")
        && !destination.starts_with("https://")
        && !looks_like_feishu_image_key(destination)
}

pub(super) fn markdown_image_destination(raw_url: &str) -> &str {
    let trimmed = raw_url.trim();
    let trimmed = trimmed
        .strip_prefix('<')
        .and_then(|value| value.strip_suffix('>'))
        .unwrap_or(trimmed);
    if let Some((path, _title)) = trimmed.split_once(" \"") {
        path.trim()
    } else if let Some((path, _title)) = trimmed.split_once(" '") {
        path.trim()
    } else {
        trimmed
    }
}

pub(super) fn looks_like_feishu_image_key(value: &str) -> bool {
    value.starts_with("img_")
}

pub(super) fn mime_type_for_image_path(path: &Path) -> Option<&'static str> {
    match path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        "bmp" => Some("image/bmp"),
        _ => None,
    }
}

/// Send an agent reply text via Feishu card and log the outbound message.
///
/// Extracted helper to share between the main turn loop and session-free command fast path.
pub(super) async fn send_agent_reply(
    client: &ImProviderClient,
    provider: &ImProviderConfig,
    event: &ImEvent,
    reply_text: &str,
    message_log_store: &Arc<ImMessageLogStore>,
) {
    send_agent_reply_with_title(client, provider, event, reply_text, message_log_store, None).await;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct AgentReplyTargetRef {
    pub(super) receive_id_type: String,
    pub(super) receive_id: String,
}

pub(super) fn agent_reply_target_ref(
    provider: &ImProviderConfig,
    event: &ImEvent,
) -> Option<AgentReplyTargetRef> {
    match provider.provider_type {
        crate::im_gateway::types::ImProviderType::Weixin
        | crate::im_gateway::types::ImProviderType::WeChat
        | crate::im_gateway::types::ImProviderType::Webhook => first_non_empty([
            event.source.chat_id.as_deref(),
            event.source.user_id.as_deref(),
            provider.owner_open_id.as_deref(),
        ])
        .map(|receive_id| AgentReplyTargetRef {
            receive_id_type: "open_id".to_string(),
            receive_id,
        }),
        crate::im_gateway::types::ImProviderType::Feishu => {
            if let Some(chat_id) = first_non_empty([event.source.chat_id.as_deref()]) {
                return Some(AgentReplyTargetRef {
                    receive_id_type: "chat_id".to_string(),
                    receive_id: chat_id,
                });
            }
            first_non_empty([
                event.source.user_id.as_deref(),
                provider.owner_open_id.as_deref(),
            ])
            .map(|receive_id| AgentReplyTargetRef {
                receive_id_type: "open_id".to_string(),
                receive_id,
            })
        }
    }
}

fn first_non_empty<const N: usize>(values: [Option<&str>; N]) -> Option<String> {
    values
        .into_iter()
        .flatten()
        .map(str::trim)
        .find(|value| !value.is_empty())
        .map(str::to_string)
}

pub(super) fn build_agent_reply_target(
    provider: &ImProviderConfig,
    event: &ImEvent,
    id: &str,
    display_name: &str,
    default_msg_type: &str,
) -> Option<crate::im_gateway::types::ImTarget> {
    let target_ref = agent_reply_target_ref(provider, event)?;
    Some(crate::im_gateway::types::ImTarget {
        id: id.to_string(),
        provider_id: provider.id.clone(),
        display_name: display_name.to_string(),
        enabled: true,
        receive_id_type: target_ref.receive_id_type,
        receive_id: target_ref.receive_id,
        default_msg_type: default_msg_type.to_string(),
        created_at: 0,
        updated_at: 0,
    })
}

/// Send an agent reply card. The Feishu provider strips the root title before
/// delivery, while other providers keep their existing card semantics.
pub(super) async fn send_agent_reply_with_title(
    client: &ImProviderClient,
    provider: &ImProviderConfig,
    event: &ImEvent,
    reply_text: &str,
    message_log_store: &Arc<ImMessageLogStore>,
    title: Option<&str>,
) {
    let image_base_dir = provider_agent_work_dir(provider);
    let (reply_text_for_card, reply_images, reply_attachments) =
        prepare_agent_reply_text_and_images_with_downloads(reply_text, image_base_dir.as_deref())
            .await;

    let Some(reply_target) = build_agent_reply_target(
        provider,
        event,
        "__agent_reply__",
        "Agent Reply",
        "interactive",
    ) else {
        error!("no reply target to send agent reply");
        return;
    };

    let rendered_text = if let Some(feishu) = client.feishu() {
        render_agent_markdown_for_feishu(
            &feishu,
            provider,
            &reply_text_for_card,
            image_base_dir.as_deref(),
        )
        .await
    } else {
        reply_text_for_card.clone()
    };
    let converted_text =
        crate::im_gateway::markdown_converter::convert_to_feishu_markdown(&rendered_text);
    let card_title = title.unwrap_or("Bifrost AI");
    let card = serde_json::json!({
        "schema": "2.0",
        "config": {
            "width_mode": "fill",
            "update_multi": true
        },
        "header": {
            "template": "blue",
            "title": {
                "tag": "plain_text",
                "content": card_title
            }
        },
        "body": {
            "elements": [
                {
                    "tag": "markdown",
                    "content": converted_text,
                    "element_id": "agent_reply"
                }
            ]
        }
    });

    let send_result = client
        .send_reply_card(
            provider,
            &reply_target,
            event.source.message_id.as_deref(),
            card,
            crate::im_gateway::types::SendOptions::default(),
        )
        .await;

    let (status, message_id, error_msg) = match &send_result {
        Ok(r) => (MessageStatus::Success, r.message_id.clone(), None),
        Err(e) => (MessageStatus::Failed, None, Some(e.to_string())),
    };
    let log = ImMessageLog {
        id: uuid_short(),
        provider_id: provider.id.clone(),
        direction: MessageDirection::Outbound,
        status,
        timestamp: now_ms(),
        target_id: Some(reply_target.receive_id.clone()),
        target_name: Some(reply_target.display_name.clone()),
        message_id,
        msg_type: Some("interactive".to_string()),
        content_preview: Some(truncate_str(&reply_text_for_card, 200)),
        trigger: Some("agent".to_string()),
        error: error_msg,
        sender_open_id: None,
        event_id: Some(event.event_id.clone()),
        reaction_added: None,
    };
    if let Err(e) = message_log_store.add(log) {
        error!(error = %e, "failed to store agent outbound message log");
    }

    match send_result {
        Ok(_) => debug!("agent reply sent successfully"),
        Err(e) => error!(error = %e, "failed to send agent reply"),
    }

    send_agent_reply_assets(
        client,
        provider,
        event,
        &reply_target,
        &reply_images,
        &reply_attachments,
        message_log_store,
    )
    .await;
}

pub(super) async fn send_agent_reply_images(
    client: &ImProviderClient,
    provider: &ImProviderConfig,
    event: &ImEvent,
    reply_target: &crate::im_gateway::types::ImTarget,
    images: &[AgentReplyLocalImage],
    message_log_store: &Arc<ImMessageLogStore>,
) {
    let image_target = crate::im_gateway::types::ImTarget {
        default_msg_type: "image".to_string(),
        ..reply_target.clone()
    };
    for image in images {
        let file_name = image
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("agent-reply-image")
            .to_string();
        let send_uuid = uuid_short();
        let send_result = match upload_agent_reply_image_for_im(client, provider, &image.path).await
        {
            Ok(image_key) => {
                client
                    .send_image(provider, &image_target, &image_key, Some(&send_uuid))
                    .await
            }
            Err(error) => Err(error),
        };

        let (status, message_id, error_msg) = match &send_result {
            Ok(r) => (MessageStatus::Success, r.message_id.clone(), None),
            Err(e) => (MessageStatus::Failed, None, Some(e.to_string())),
        };
        let label = if image.alt.trim().is_empty() {
            file_name.clone()
        } else {
            format!("{} ({})", image.alt.trim(), file_name)
        };
        let log = ImMessageLog {
            id: uuid_short(),
            provider_id: provider.id.clone(),
            direction: MessageDirection::Outbound,
            status,
            timestamp: now_ms(),
            target_id: Some(image_target.receive_id.clone()),
            target_name: Some(image_target.display_name.clone()),
            message_id,
            msg_type: Some("image".to_string()),
            content_preview: Some(format!("[image:{label}]")),
            trigger: Some("agent".to_string()),
            error: error_msg,
            sender_open_id: None,
            event_id: Some(event.event_id.clone()),
            reaction_added: None,
        };
        if let Err(e) = message_log_store.add(log) {
            error!(error = %e, "failed to store agent outbound image message log");
        }

        match send_result {
            Ok(_) => debug!(path = %image.path.display(), "agent reply image sent successfully"),
            Err(e) => {
                error!(path = %image.path.display(), error = %e, "failed to send agent reply image")
            }
        }
    }
}

/// Send an agent reply with plan progress and tool calls panel (for goal continuation).
/// This sends a card similar to the final response rendering but can be called mid-continuation.
#[allow(dead_code)]
#[allow(clippy::too_many_arguments)]
pub(super) async fn send_agent_reply_with_plan(
    client: &ImProviderClient,
    provider: &ImProviderConfig,
    event: &ImEvent,
    reply_text: &str,
    plan_steps: Option<&[PlanStep]>,
    tool_calls_log: &[ToolCallLog],
    title: Option<&str>,
    message_log_store: &Arc<ImMessageLogStore>,
) {
    let image_base_dir = provider_agent_work_dir(provider);
    let (reply_text_for_card, reply_images, reply_attachments) =
        prepare_agent_reply_text_and_images_with_downloads(reply_text, image_base_dir.as_deref())
            .await;
    let Some(reply_target) = build_agent_reply_target(
        provider,
        event,
        "__agent_reply__",
        "Agent Reply",
        "interactive",
    ) else {
        error!("no reply target to send agent reply");
        return;
    };

    let rendered_text = if let Some(feishu) = client.feishu() {
        render_agent_markdown_for_feishu(
            &feishu,
            provider,
            &reply_text_for_card,
            image_base_dir.as_deref(),
        )
        .await
    } else {
        reply_text_for_card.clone()
    };
    let converted_text =
        crate::im_gateway::markdown_converter::convert_to_feishu_markdown(&rendered_text);
    let mut elements = vec![serde_json::json!({
        "tag": "markdown",
        "content": converted_text,
        "element_id": "agent_reply"
    })];

    // Add plan progress panel if present
    if let Some(steps) = plan_steps {
        let completed = steps
            .iter()
            .filter(|s| matches!(s.status, bifrost_agent::PlanStepStatus::Completed))
            .count();
        let total = steps.len();
        let mut plan_md = String::new();
        for s in steps {
            plan_md.push_str(&format!("{} {}\n", s.status.emoji(), s.step));
        }
        elements.push(serde_json::json!({
            "tag": "collapsible_panel",
            "expanded": true,
            "background_color": "grey",
            "header": {
                "title": {
                    "tag": "plain_text",
                    "content": format!("📋 任务计划（{}/{}）", completed, total)
                }
            },
            "vertical_spacing": "2px",
            "padding": "4px 8px 4px 8px",
            "elements": [{
                "tag": "markdown",
                "content": plan_md
            }]
        }));
    }

    // Add tool calls panel if present
    if !tool_calls_log.is_empty() {
        let mut tool_md = String::new();
        for log in tool_calls_log {
            let icon = if log.success { "✅" } else { "❌" };
            tool_md.push_str(&format!("{} `{}`\n", icon, log.tool_name));
            let result_preview = truncate_str(&log.result, 500);
            tool_md.push_str(&format!("```\n{}\n```\n", result_preview));
        }
        elements.push(serde_json::json!({
            "tag": "collapsible_panel",
            "expanded": false,
            "background_color": "grey",
            "header": {
                "title": {
                    "tag": "plain_text",
                    "content": format!("🔧 工具调用记录（{}次）", tool_calls_log.len())
                }
            },
            "vertical_spacing": "2px",
            "padding": "4px 8px 4px 8px",
            "elements": [{
                "tag": "markdown",
                "content": tool_md
            }]
        }));
    }

    let card_title = title.unwrap_or("Bifrost AI");
    let card = serde_json::json!({
        "schema": "2.0",
        "config": {
            "width_mode": "fill",
            "update_multi": true
        },
        "header": {
            "template": "blue",
            "title": {
                "tag": "plain_text",
                "content": card_title
            }
        },
        "body": {
            "elements": elements
        }
    });

    let send_result = client
        .send_reply_card(
            provider,
            &reply_target,
            event.source.message_id.as_deref(),
            card,
            crate::im_gateway::types::SendOptions::default(),
        )
        .await;

    let (status, message_id, error_msg) = match &send_result {
        Ok(r) => (MessageStatus::Success, r.message_id.clone(), None),
        Err(e) => (MessageStatus::Failed, None, Some(e.to_string())),
    };
    let log = ImMessageLog {
        id: uuid_short(),
        provider_id: provider.id.clone(),
        direction: MessageDirection::Outbound,
        status,
        timestamp: now_ms(),
        target_id: Some(reply_target.receive_id.clone()),
        target_name: Some(reply_target.display_name.clone()),
        message_id,
        msg_type: Some("interactive".to_string()),
        content_preview: Some(truncate_str(&reply_text_for_card, 200)),
        trigger: Some("agent_continuation".to_string()),
        error: error_msg,
        sender_open_id: None,
        event_id: Some(event.event_id.clone()),
        reaction_added: None,
    };
    if let Err(e) = message_log_store.add(log) {
        error!(error = %e, "failed to store agent outbound message log");
    }

    match send_result {
        Ok(_) => debug!("agent reply with plan sent successfully"),
        Err(e) => error!(error = %e, "failed to send agent reply with plan"),
    }

    send_agent_reply_assets(
        client,
        provider,
        event,
        &reply_target,
        &reply_images,
        &reply_attachments,
        message_log_store,
    )
    .await;
}

/// Best-effort helper to send an error notification card to the provider owner.
#[allow(dead_code)]
pub(super) async fn send_error_card_to_owner(
    feishu: &Arc<crate::im_gateway::feishu::FeishuProvider>,
    provider: &ImProviderConfig,
    error_message: &str,
) {
    let target_open_id = match provider.owner_open_id.as_deref() {
        Some(id) if !id.is_empty() => id,
        _ => return,
    };

    let target = crate::im_gateway::types::ImTarget {
        id: "__error_notify__".to_string(),
        provider_id: provider.id.clone(),
        display_name: "Error Notify".to_string(),
        enabled: true,
        receive_id_type: "open_id".to_string(),
        receive_id: target_open_id.to_string(),
        default_msg_type: "interactive".to_string(),
        created_at: 0,
        updated_at: 0,
    };

    let converted_error =
        crate::im_gateway::markdown_converter::convert_to_feishu_markdown(error_message);
    let card = serde_json::json!({
        "schema": "2.0",
        "config": { "width_mode": "fill" },
        "header": {
            "template": "red",
            "title": { "tag": "plain_text", "content": "Agent Runner Error" }
        },
        "body": {
            "elements": [{
                "tag": "markdown",
                "content": converted_error
            }]
        }
    });

    if let Err(e) = feishu
        .send_card(
            provider,
            &target,
            card,
            crate::im_gateway::types::SendOptions::default(),
        )
        .await
    {
        warn!(error = %e, "failed to send error card to owner");
    }
}

pub(super) async fn handle_provider_policy(
    req: Request<Incoming>,
    service: &ImGatewayService,
    id: &str,
) -> Response<BoxBody> {
    match *req.method() {
        Method::GET => {
            let Some(_provider) = service.provider_store.get(id) else {
                return error_response(StatusCode::NOT_FOUND, "Provider not found");
            };
            // Return policy placeholder — policy store will be integrated in future
            json_response(&serde_json::json!({
                "provider_id": id,
                "permissions": [],
                "script_policy_binding": null,
            }))
        }
        Method::PATCH => {
            let _patch: serde_json::Value = match read_body_json(req).await {
                Ok(v) => v,
                Err(resp) => return resp,
            };
            let Some(_provider) = service.provider_store.get(id) else {
                return error_response(StatusCode::NOT_FOUND, "Provider not found");
            };
            // Policy update placeholder
            json_response(&serde_json::json!({"success": true}))
        }
        _ => method_not_allowed(),
    }
}

pub(super) async fn handle_provider_policy_bind_shell(
    req: Request<Incoming>,
    service: &ImGatewayService,
    id: &str,
) -> Response<BoxBody> {
    if req.method() != Method::POST {
        return method_not_allowed();
    }
    let _body: serde_json::Value = match read_body_json(req).await {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    let Some(_provider) = service.provider_store.get(id) else {
        return error_response(StatusCode::NOT_FOUND, "Provider not found");
    };
    // Bind-shell placeholder
    json_response(&serde_json::json!({"success": true}))
}
