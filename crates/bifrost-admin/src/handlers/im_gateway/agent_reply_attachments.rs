use super::*;
use std::collections::HashSet;
use std::time::Duration;

use futures_util::StreamExt;
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
                            if is_image_mime_or_path(None, &path) {
                                if seen.insert(dedupe_key) {
                                    images.push(AgentReplyLocalImage { alt: label, path });
                                }
                            } else if is_explicit_attachment_label_or_path(
                                &label,
                                &path.to_string_lossy(),
                            )
                                && seen.insert(dedupe_key)
                            {
                                attachments.push(AgentReplyLocalAttachment {
                                    label,
                                    mime_type: mime_guess::from_path(&path)
                                        .first_raw()
                                        .map(str::to_string),
                                    path,
                                });
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
    if attachment_url_points_to_denied_path(&parsed) {
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

fn attachment_url_points_to_denied_path(url: &Url) -> bool {
    is_denied_agent_reply_attachment_path(url.path())
        || url.query_pairs().any(|(key, value)| {
            let key = key.to_ascii_lowercase();
            (key == "filename" || key == "file" || key == "name")
                && is_denied_agent_reply_attachment_path(value.as_ref())
        })
}

pub(super) fn is_explicit_attachment_label_or_path(label: &str, path: &str) -> bool {
    // Source code is intentionally never auto-sent from a terminal reply,
    // even when the link label says "file". Configuration files have a
    // dedicated allowlist below; keeping code on a denylist prevents a broad
    // attachment label from accidentally publishing implementation details.
    if is_denied_agent_reply_attachment_path(path) {
        return false;
    }
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
    download_remote_attachment_with_limit(attachment, MAX_AGENT_REPLY_ATTACHMENT_BYTES).await
}

pub(super) async fn download_remote_attachment_with_limit(
    attachment: &AgentReplyRemoteAttachment,
    max_bytes: u64,
) -> bifrost_core::Result<AgentReplyDownloadedAttachment> {
    let http = bifrost_core::outbound_reqwest_client().map_err(|error| {
        bifrost_core::BifrostError::Network(format!(
            "build agent reply attachment downloader failed: {error}"
        ))
    })?;
    let response = http
        .get(&attachment.url)
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .map_err(|error| {
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
    if response
        .content_length()
        .is_some_and(|length| length > max_bytes)
    {
        return Err(remote_attachment_size_error(max_bytes));
    }
    let mut bytes = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| {
            bifrost_core::BifrostError::Network(format!(
                "read agent reply attachment body failed: {error}"
            ))
        })?;
        let buffered = bytes.len() as u64;
        if buffered.saturating_add(chunk.len() as u64) > max_bytes {
            return Err(remote_attachment_size_error(max_bytes));
        }
        bytes.extend_from_slice(&chunk);
    }
    if bytes.is_empty() {
        return Err(bifrost_core::BifrostError::Network(
            "download agent reply attachment returned empty body".to_string(),
        ));
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

fn remote_attachment_size_error(max_bytes: u64) -> bifrost_core::BifrostError {
    let max_mib = max_bytes / 1024 / 1024;
    bifrost_core::BifrostError::Config(format!("远程附件超过 IM 通道上传文件 {max_mib} MiB 上限"))
}

pub(super) fn extension_from_content_type(content_type: &str) -> Option<&'static str> {
    image_extension_from_content_type(content_type).or_else(|| {
        match content_type.to_ascii_lowercase().as_str() {
            "application/pdf" => Some("pdf"),
            "text/plain" => Some("txt"),
            "text/markdown" => Some("md"),
            "text/csv" => Some("csv"),
            "application/json" => Some("json"),
            "application/yaml" | "application/x-yaml" | "text/yaml" | "text/x-yaml" => Some("yaml"),
            "application/toml" | "text/toml" | "application/x-toml" => Some("toml"),
            "application/xml" | "text/xml" => Some("xml"),
            "application/zip" => Some("zip"),
            "application/x-tar" => Some("tar"),
            "application/gzip" | "application/x-gzip" => Some("gz"),
            "application/x-bzip2" => Some("bz2"),
            "application/x-xz" => Some("xz"),
            "application/zstd" | "application/x-zstd" => Some("zst"),
            "application/x-7z-compressed" => Some("7z"),
            "application/vnd.rar" | "application/x-rar-compressed" => Some("rar"),
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document" => {
                Some("docx")
            }
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet" => Some("xlsx"),
            "application/vnd.openxmlformats-officedocument.presentationml.presentation" => {
                Some("pptx")
            }
            "video/mp4" => Some("mp4"),
            "video/webm" => Some("webm"),
            "audio/mpeg" => Some("mp3"),
            "audio/mp4" | "audio/x-m4a" => Some("m4a"),
            "audio/wav" | "audio/x-wav" => Some("wav"),
            "audio/ogg" => Some("ogg"),
            _ => None,
        }
    })
}

pub(super) fn attachment_extension_from_path(path: &str) -> Option<&'static str> {
    let path = path.split('?').next().unwrap_or(path).to_ascii_lowercase();
    for (suffix, extension) in [
        (".tar.gz", "tar.gz"),
        (".tar.bz2", "tar.bz2"),
        (".tar.xz", "tar.xz"),
        (".tar.zst", "tar.zst"),
    ] {
        if path.ends_with(suffix) {
            return Some(extension);
        }
    }
    let ext = std::path::Path::new(&path)
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
        "jsonc" => Some("jsonc"),
        "json5" => Some("json5"),
        "yaml" => Some("yaml"),
        "yml" => Some("yml"),
        "toml" => Some("toml"),
        "ini" => Some("ini"),
        "cfg" => Some("cfg"),
        "conf" => Some("conf"),
        "config" => Some("config"),
        "cnf" => Some("cnf"),
        "properties" => Some("properties"),
        "xml" => Some("xml"),
        "hcl" => Some("hcl"),
        "tfvars" => Some("tfvars"),
        "plist" => Some("plist"),
        "xcconfig" => Some("xcconfig"),
        "zip" => Some("zip"),
        "tar" => Some("tar"),
        "tgz" => Some("tgz"),
        "tbz" => Some("tbz"),
        "tbz2" => Some("tbz2"),
        "txz" => Some("txz"),
        "tzst" => Some("tzst"),
        "gz" => Some("gz"),
        "bz2" => Some("bz2"),
        "xz" => Some("xz"),
        "zst" => Some("zst"),
        "7z" => Some("7z"),
        "rar" => Some("rar"),
        "doc" => Some("doc"),
        "docx" => Some("docx"),
        "xls" => Some("xls"),
        "xlsx" => Some("xlsx"),
        "ppt" => Some("ppt"),
        "pptx" => Some("pptx"),
        "patch" => Some("patch"),
        "diff" => Some("diff"),
        "mp4" => Some("mp4"),
        "webm" => Some("webm"),
        "mov" => Some("mov"),
        "m4v" => Some("m4v"),
        "mp3" => Some("mp3"),
        "m4a" => Some("m4a"),
        "wav" => Some("wav"),
        "ogg" => Some("ogg"),
        "opus" => Some("opus"),
        "flac" => Some("flac"),
        "aac" => Some("aac"),
        _ => None,
    }
}

pub(super) fn is_source_code_path(path: &str) -> bool {
    let decoded = urlencoding::decode(path).ok();
    let path = decoded.as_deref().unwrap_or(path);
    let path = path.split('?').next().unwrap_or(path).to_ascii_lowercase();
    let path = std::path::Path::new(&path);
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    if matches!(
        file_name,
        "dockerfile"
            | "containerfile"
            | "makefile"
            | "gnumakefile"
            | "rakefile"
            | "gemfile"
            | "vagrantfile"
            | "jenkinsfile"
            | "cmakelists.txt"
    ) {
        return true;
    }
    let ext = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    matches!(
        ext.as_str(),
        "rs" | "py"
            | "pyw"
            | "js"
            | "jsx"
            | "mjs"
            | "cjs"
            | "ts"
            | "tsx"
            | "java"
            | "kt"
            | "kts"
            | "go"
            | "c"
            | "h"
            | "cc"
            | "cpp"
            | "cxx"
            | "hh"
            | "hpp"
            | "hxx"
            | "cs"
            | "fs"
            | "fsx"
            | "vb"
            | "swift"
            | "m"
            | "mm"
            | "rb"
            | "php"
            | "scala"
            | "sc"
            | "lua"
            | "r"
            | "dart"
            | "ex"
            | "exs"
            | "erl"
            | "hrl"
            | "clj"
            | "cljs"
            | "cljc"
            | "groovy"
            | "gradle"
            | "sol"
            | "move"
            | "zig"
            | "nim"
            | "nix"
            | "hs"
            | "lhs"
            | "ml"
            | "mli"
            | "v"
            | "odin"
            | "asm"
            | "s"
            | "cmake"
            | "mk"
            | "vue"
            | "svelte"
            | "astro"
            | "sh"
            | "bash"
            | "zsh"
            | "fish"
            | "ps1"
            | "bat"
            | "cmd"
            | "sql"
            | "graphql"
            | "gql"
            | "proto"
            | "html"
            | "htm"
            | "css"
            | "scss"
            | "sass"
            | "less"
    )
}

pub(super) fn is_sensitive_config_path(path: &str) -> bool {
    let decoded = urlencoding::decode(path).ok();
    let path = decoded.as_deref().unwrap_or(path);
    let path = path.split('?').next().unwrap_or(path).to_ascii_lowercase();
    let file_name = std::path::Path::new(&path)
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    file_name == ".env"
        || file_name.starts_with(".env.")
        || matches!(
            file_name,
            "credentials" | "secrets" | "id_rsa" | "id_ed25519"
        )
}

fn is_denied_agent_reply_attachment_path(path: &str) -> bool {
    is_source_code_path(path) || is_sensitive_config_path(path)
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
    if metadata.len() == 0 {
        let path = attachment.path.display();
        return Err(bifrost_core::BifrostError::Config(format!(
            "IM 通道不允许上传空文件：{path}"
        )));
    }
    if metadata.len() > MAX_AGENT_REPLY_ATTACHMENT_BYTES {
        return Err(bifrost_core::BifrostError::Config(format!(
            "文件超过 IM 通道上传文件 {} MiB 上限：{}",
            MAX_AGENT_REPLY_ATTACHMENT_BYTES / 1024 / 1024,
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
    initial_failure_notices: &[String],
    message_log_store: &Arc<ImMessageLogStore>,
) {
    let file_target = crate::im_gateway::types::ImTarget {
        default_msg_type: "file".to_string(),
        ..reply_target.clone()
    };
    let mut failure_notices = initial_failure_notices.to_vec();
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
            content: Some(format!("[file:{label}]")),
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
            Err(e) => {
                let path = attachment.path.display();
                warn!("failed to send agent reply attachment {path}; local file is retained: {e}");
                failure_notices.push(format!(
                    "文件「{label}」未发送成功：{e}；任务结论已正常发布。"
                ));
            }
        }
    }
    if !failure_notices.is_empty() {
        send_agent_reply_attachment_notice(
            client,
            provider,
            event,
            reply_target,
            &failure_notices,
            message_log_store,
        )
        .await;
    }
}

async fn send_agent_reply_attachment_notice(
    client: &ImProviderClient,
    provider: &ImProviderConfig,
    event: &ImEvent,
    reply_target: &crate::im_gateway::types::ImTarget,
    notices: &[String],
    message_log_store: &Arc<ImMessageLogStore>,
) {
    let text = format!(
        "附件发送提示（不影响任务结论）：\n- {}",
        notices.join("\n- ")
    );
    let card = crate::im_gateway::feishu::build_default_text_card(&text);
    let send_result = client
        .send_reply_card(
            provider,
            reply_target,
            event.source.message_id.as_deref(),
            card,
            crate::im_gateway::types::SendOptions::default(),
        )
        .await;
    let (status, message_id, error_msg) = match &send_result {
        Ok(result) => (MessageStatus::Success, result.message_id.clone(), None),
        Err(error) => (MessageStatus::Failed, None, Some(error.to_string())),
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
        content_preview: Some(truncate_str(&text, 200)),
        content: Some(text),
        trigger: Some("agent_attachment_notice".to_string()),
        error: error_msg,
        sender_open_id: None,
        event_id: Some(event.event_id.clone()),
        reaction_added: None,
    };
    if let Err(error) = message_log_store.add(log) {
        warn!(error = %error, "failed to store agent attachment notice log");
    }
    match send_result {
        Ok(_) => debug!("agent attachment failure notice sent successfully"),
        Err(error) => warn!(
            error = %error,
            "failed to send agent attachment failure notice; terminal task remains successful"
        ),
    }
}

// Keeping the shared delivery context explicit makes both the standard and
// planned reply paths use the same best-effort attachment semantics.
#[allow(clippy::too_many_arguments)]
pub(super) async fn send_agent_reply_assets(
    client: &ImProviderClient,
    provider: &ImProviderConfig,
    event: &ImEvent,
    reply_target: &crate::im_gateway::types::ImTarget,
    images: &[AgentReplyLocalImage],
    attachments: &[AgentReplyLocalAttachment],
    attachment_notices: &[String],
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
    if !attachments.is_empty() || !attachment_notices.is_empty() {
        send_agent_reply_attachments(
            client,
            provider,
            event,
            reply_target,
            attachments,
            attachment_notices,
            message_log_store,
        )
        .await;
    }
}
