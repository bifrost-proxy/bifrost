use super::*;
use std::collections::HashSet;
use std::time::Duration;

use sha1::{Digest, Sha1};
use url::Url;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct AgentReplyLocalAttachment {
    pub(super) label: String,
    pub(super) path: PathBuf,
    pub(super) mime_type: Option<String>,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(super) struct AgentReplyRemoteAttachment {
    pub(super) label: String,
    pub(super) url: String,
}

#[derive(Clone, Debug)]
pub(super) struct AgentReplyDownloadedAttachment {
    pub(super) path: PathBuf,
    pub(super) mime_type: Option<String>,
}

pub(super) fn collect_agent_reply_local_attachment_links(
    markdown: &str,
    base_dir: Option<&Path>,
    images: &mut Vec<AgentReplyLocalImage>,
    attachments: &mut Vec<AgentReplyLocalAttachment>,
) {
    if !markdown.contains('[') {
        return;
    }

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
            if line.as_bytes()[pos] == b'['
                && (pos == 0 || line.as_bytes()[pos.saturating_sub(1)] != b'!')
            {
                if let Some((label, url, end)) =
                    crate::im_gateway::markdown_converter::parse_image_syntax(line, pos + 1)
                {
                    let destination = markdown_image_destination(&url);
                    if is_local_markdown_image_candidate(destination) {
                        if let Some(path) = resolve_agent_reply_image_path(destination, base_dir) {
                            let dedupe_key = path.canonicalize().unwrap_or_else(|_| path.clone());
                            if seen.insert(dedupe_key) {
                                if is_image_mime_or_path(None, &path) {
                                    images.push(AgentReplyLocalImage { alt: label, path });
                                } else if is_explicit_attachment_label_or_path(&label, destination)
                                {
                                    attachments.push(AgentReplyLocalAttachment {
                                        label,
                                        path,
                                        mime_type: None,
                                    });
                                }
                            }
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
}

pub(super) fn collect_agent_reply_remote_attachment_links(
    markdown: &str,
) -> Vec<AgentReplyRemoteAttachment> {
    if !markdown.contains('[') {
        return Vec::new();
    }

    let mut attachments = Vec::new();
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
            if line.as_bytes()[pos] == b'['
                && (pos == 0 || line.as_bytes()[pos.saturating_sub(1)] != b'!')
            {
                if let Some((label, url, end)) =
                    crate::im_gateway::markdown_converter::parse_image_syntax(line, pos + 1)
                {
                    let destination = markdown_image_destination(&url).to_string();
                    if is_remote_markdown_attachment_candidate(&label, &destination)
                        && seen.insert(destination.clone())
                    {
                        attachments.push(AgentReplyRemoteAttachment {
                            label,
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

    attachments
}

fn is_remote_markdown_attachment_candidate(label: &str, raw_url: &str) -> bool {
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
    if is_explicit_attachment_label_or_path(label, &path) {
        return true;
    }
    parsed.query_pairs().any(|(key, value)| {
        let key = key.to_ascii_lowercase();
        (key == "filename" || key == "file" || key == "name")
            && attachment_extension_from_path(&value.to_ascii_lowercase()).is_some()
    })
}

fn is_explicit_attachment_label_or_path(label: &str, path: &str) -> bool {
    let label = label.to_ascii_lowercase();
    if label.contains("附件")
        || label.contains("下载")
        || label.contains("download")
        || label.contains("attachment")
        || label.contains("file")
    {
        return true;
    }
    image_extension_from_path(path).is_some() || attachment_extension_from_path(path).is_some()
}

pub(super) async fn download_agent_reply_remote_attachment(
    attachment: &AgentReplyRemoteAttachment,
) -> bifrost_core::Result<AgentReplyDownloadedAttachment> {
    let http = bifrost_core::outbound_reqwest_client_builder()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|error| {
            bifrost_core::BifrostError::Network(format!(
                "build agent reply attachment downloader failed: {error}"
            ))
        })?;
    let response = http.get(&attachment.url).send().await.map_err(|error| {
        bifrost_core::BifrostError::Network(format!(
            "download agent reply attachment failed: {}",
            bifrost_core::format_reqwest_error(&error)
        ))
    })?;
    if !response.status().is_success() {
        return Err(bifrost_core::BifrostError::Network(format!(
            "download agent reply attachment returned HTTP {}",
            response.status()
        )));
    }
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.split(';').next().unwrap_or(value).trim().to_string());
    let bytes = response.bytes().await.map_err(|error| {
        bifrost_core::BifrostError::Network(format!(
            "read agent reply attachment body failed: {error}"
        ))
    })?;
    if bytes.is_empty() {
        return Err(bifrost_core::BifrostError::Network(
            "download agent reply attachment returned empty body".to_string(),
        ));
    }
    if bytes.len() as u64 > MAX_AGENT_REPLY_ATTACHMENT_BYTES {
        return Err(bifrost_core::BifrostError::Config(format!(
            "downloaded agent reply attachment exceeds {MAX_AGENT_REPLY_ATTACHMENT_BYTES} bytes"
        )));
    }

    let mut hasher = Sha1::new();
    hasher.update(attachment.url.as_bytes());
    hasher.update(&bytes);
    let digest = format!("{:x}", hasher.finalize());
    let ext = content_type
        .as_deref()
        .and_then(extension_from_content_type)
        .or_else(|| {
            Url::parse(&attachment.url)
                .ok()
                .and_then(|url| extension_from_attachment_url(&url))
        })
        .unwrap_or("bin");
    let dir = bifrost_storage::data_dir()
        .join("agent")
        .join("im_gateway")
        .join("attachments")
        .join("agent_reply_files");
    tokio::fs::create_dir_all(&dir).await.map_err(|error| {
        bifrost_core::BifrostError::Io(std::io::Error::new(
            error.kind(),
            format!("create agent reply file attachment dir failed: {error}"),
        ))
    })?;
    let path = dir.join(format!("agent-reply-attachment-{digest}.{ext}"));
    tokio::fs::write(&path, &bytes).await.map_err(|error| {
        bifrost_core::BifrostError::Io(std::io::Error::new(
            error.kind(),
            format!(
                "write agent reply file attachment '{}' failed: {error}",
                path.display()
            ),
        ))
    })?;
    Ok(AgentReplyDownloadedAttachment {
        path,
        mime_type: content_type,
    })
}

fn extension_from_content_type(content_type: &str) -> Option<&'static str> {
    image_extension_from_content_type(content_type).or_else(|| {
        match content_type.to_ascii_lowercase().as_str() {
            "application/pdf" => Some("pdf"),
            "text/plain" => Some("txt"),
            "text/markdown" => Some("md"),
            "text/csv" => Some("csv"),
            "application/json" => Some("json"),
            "application/zip" => Some("zip"),
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document" => {
                Some("docx")
            }
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet" => Some("xlsx"),
            "application/vnd.openxmlformats-officedocument.presentationml.presentation" => {
                Some("pptx")
            }
            _ => None,
        }
    })
}

fn attachment_extension_from_path(path: &str) -> Option<&'static str> {
    let path = path.split('?').next().unwrap_or(path);
    let ext = std::path::Path::new(path)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    match ext.as_str() {
        "pdf" => Some("pdf"),
        "txt" => Some("txt"),
        "md" => Some("md"),
        "csv" => Some("csv"),
        "json" => Some("json"),
        "zip" => Some("zip"),
        "doc" => Some("doc"),
        "docx" => Some("docx"),
        "xls" => Some("xls"),
        "xlsx" => Some("xlsx"),
        "ppt" => Some("ppt"),
        "pptx" => Some("pptx"),
        _ => None,
    }
}

fn extension_from_attachment_url(url: &Url) -> Option<&'static str> {
    image_extension_from_path(url.path()).or_else(|| {
        attachment_extension_from_path(url.path()).or_else(|| {
            url.query_pairs().find_map(|(key, value)| {
                let key = key.to_ascii_lowercase();
                if key == "filename" || key == "file" || key == "name" {
                    let value = value.to_ascii_lowercase();
                    image_extension_from_path(&value)
                        .or_else(|| attachment_extension_from_path(&value))
                } else {
                    None
                }
            })
        })
    })
}

pub(super) fn is_image_mime_or_path(mime_type: Option<&str>, path: &Path) -> bool {
    mime_type
        .map(|value| value.to_ascii_lowercase().starts_with("image/"))
        .unwrap_or(false)
        || image_extension_from_path(path.to_string_lossy().as_ref()).is_some()
}

async fn upload_agent_reply_file_for_im(
    client: &ImProviderClient,
    provider: &ImProviderConfig,
    attachment: &AgentReplyLocalAttachment,
) -> bifrost_core::Result<String> {
    let metadata = tokio::fs::metadata(&attachment.path)
        .await
        .map_err(|error| {
            bifrost_core::BifrostError::Io(std::io::Error::new(
                error.kind(),
                format!(
                    "failed to stat agent reply attachment '{}': {}",
                    attachment.path.display(),
                    error
                ),
            ))
        })?;
    if !metadata.is_file() {
        return Err(bifrost_core::BifrostError::Config(format!(
            "agent reply attachment is not a file: {}",
            attachment.path.display()
        )));
    }
    if metadata.len() > MAX_AGENT_REPLY_ATTACHMENT_BYTES {
        return Err(bifrost_core::BifrostError::Config(format!(
            "agent reply attachment exceeds {} bytes: {}",
            MAX_AGENT_REPLY_ATTACHMENT_BYTES,
            attachment.path.display()
        )));
    }

    let bytes = tokio::fs::read(&attachment.path).await.map_err(|error| {
        bifrost_core::BifrostError::Io(std::io::Error::new(
            error.kind(),
            format!(
                "failed to read agent reply attachment '{}': {}",
                attachment.path.display(),
                error
            ),
        ))
    })?;
    let file_name = attachment
        .path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("agent-reply-attachment");
    client
        .upload_file(provider, file_name, bytes, attachment.mime_type.as_deref())
        .await
}

pub(super) async fn send_agent_reply_attachments(
    client: &ImProviderClient,
    provider: &ImProviderConfig,
    event: &ImEvent,
    reply_target: &crate::im_gateway::types::ImTarget,
    attachments: &[AgentReplyLocalAttachment],
    message_log_store: &Arc<ImMessageLogStore>,
) {
    let file_target = crate::im_gateway::types::ImTarget {
        default_msg_type: "file".to_string(),
        ..reply_target.clone()
    };
    for attachment in attachments {
        let file_name = attachment
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("agent-reply-attachment")
            .to_string();
        let send_uuid = uuid_short();
        let send_result = match upload_agent_reply_file_for_im(client, provider, attachment).await {
            Ok(file_key) => {
                client
                    .send_file(provider, &file_target, &file_key, Some(&send_uuid))
                    .await
            }
            Err(error) => Err(error),
        };

        let (status, message_id, error_msg) = match &send_result {
            Ok(r) => (MessageStatus::Success, r.message_id.clone(), None),
            Err(e) => (MessageStatus::Failed, None, Some(e.to_string())),
        };
        let label = if attachment.label.trim().is_empty() {
            file_name.clone()
        } else {
            format!("{} ({})", attachment.label.trim(), file_name)
        };
        let log = ImMessageLog {
            id: uuid_short(),
            provider_id: provider.id.clone(),
            direction: MessageDirection::Outbound,
            status,
            timestamp: now_ms(),
            target_id: Some(file_target.receive_id.clone()),
            target_name: Some(file_target.display_name.clone()),
            message_id,
            msg_type: Some("file".to_string()),
            content_preview: Some(format!(
                "[file:{label}] local={}",
                attachment.path.display()
            )),
            trigger: Some("agent".to_string()),
            error: error_msg,
            sender_open_id: None,
            event_id: Some(event.event_id.clone()),
            reaction_added: None,
        };
        if let Err(e) = message_log_store.add(log) {
            error!(error = %e, "failed to store agent outbound file message log");
        }

        match send_result {
            Ok(_) => debug!(
                path = %attachment.path.display(),
                "agent reply attachment sent successfully"
            ),
            Err(e) => warn!(
                path = %attachment.path.display(),
                error = %e,
                "failed to send agent reply attachment; local file is retained"
            ),
        }
    }
}

pub(super) async fn send_agent_reply_assets(
    client: &ImProviderClient,
    provider: &ImProviderConfig,
    event: &ImEvent,
    reply_target: &crate::im_gateway::types::ImTarget,
    images: &[AgentReplyLocalImage],
    attachments: &[AgentReplyLocalAttachment],
    message_log_store: &Arc<ImMessageLogStore>,
) {
    if !images.is_empty() {
        send_agent_reply_images(
            client,
            provider,
            event,
            reply_target,
            images,
            message_log_store,
        )
        .await;
    }
    if !attachments.is_empty() {
        send_agent_reply_attachments(
            client,
            provider,
            event,
            reply_target,
            attachments,
            message_log_store,
        )
        .await;
    }
}
