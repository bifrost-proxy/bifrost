use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{json, Value};
use sha1::{Digest, Sha1};
use tokio::time::{sleep, timeout};
use tracing::{info, warn};

use super::native::native_binary;
use super::{
    iso_now, seed_chatgpt_cookies, stop_requested, stopped_error, truncate_for_log,
    wait_page_on_domain, AuthState, BrowserSession, CdpClient, RuntimeConfig,
};

pub(in crate::im_gateway::chatgpt_web) async fn download_behavior_artifacts_for_conversation(
    config: &RuntimeConfig,
    auth: &AuthState,
    conversation_id: &str,
    artifacts: &[Value],
    stop_marker_path: &Path,
) -> Result<Vec<Value>, String> {
    let candidates = downloadable_artifact_candidates(artifacts);
    if candidates.is_empty() {
        return Ok(Vec::new());
    }

    let target_dir = config.attachments_dir.join(conversation_id);
    tokio::fs::create_dir_all(&target_dir)
        .await
        .map_err(|error| {
            format!(
                "create ChatGPT artifact attachment dir {} failed: {error}",
                target_dir.display()
            )
        })?;
    let download_dir = target_dir.join(format!("behavior-downloads-{}", now_millis()));
    tokio::fs::create_dir_all(&download_dir)
        .await
        .map_err(|error| {
            format!(
                "create ChatGPT behavior download dir {} failed: {error}",
                download_dir.display()
            )
        })?;

    let headless = config.browser.execution_mode != "headed";
    let browser = BrowserSession::get_or_launch(config, headless).await?;
    let mut target_id = None;
    let mut downloaded = Vec::new();
    let mut failures = Vec::new();
    let result = async {
        let page = browser.create_new_page().await?;
        target_id = Some(page.id.clone());
        let cdp = CdpClient::connect_with_retry(&page.web_socket_debugger_url, 3).await?;
        cdp.enable_domains().await?;
        cdp.send(
            "Page.setDownloadBehavior",
            json!({
                "behavior": "allow",
                "downloadPath": download_dir.to_string_lossy(),
            }),
        )
        .await?;
        seed_chatgpt_cookies(&cdp, auth).await?;

        let conversation_url = format!(
            "{}/c/{}",
            config.chatgpt.base_url.trim_end_matches('/'),
            conversation_id
        );
        cdp.navigate_and_wait(&conversation_url, Duration::from_secs(30))
            .await?;
        wait_page_on_domain(&cdp, &config.chatgpt.base_url, Duration::from_secs(30)).await?;
        wait_for_behavior_buttons(&cdp).await?;

        for candidate in candidates {
            if stop_requested(stop_marker_path).await {
                return Err(stopped_error());
            }
            let result = if candidate.kind == "image" {
                click_image_artifact_and_download(auth, &cdp, &target_dir, &candidate).await
            } else {
                click_archive_artifact_and_wait_for_download(
                    &cdp,
                    &download_dir,
                    &target_dir,
                    &candidate,
                )
                .await
            };
            match result {
                Ok(Some(item)) => downloaded.push(item),
                Ok(None) => {
                    info!(
                        label = %candidate.label,
                        kind = %candidate.kind,
                        "chatgpt_web artifact: behavior button did not start a download"
                    );
                    failures.push(json!({
                        "kind": candidate.kind,
                        "label": candidate.label,
                        "error": "button did not expose a downloadable artifact",
                    }));
                }
                Err(error) => {
                    warn!(
                        label = %candidate.label,
                        kind = %candidate.kind,
                        error = %error,
                        "chatgpt_web artifact: behavior button download failed"
                    );
                    failures.push(json!({
                        "kind": candidate.kind,
                        "label": candidate.label,
                        "error": error,
                    }));
                }
            }
        }
        Ok(())
    }
    .await;

    if let Some(id) = target_id {
        if let Err(error) = browser.close_target(&id).await {
            warn!(
                target_id = %id,
                error = %error,
                "chatgpt_web artifact: failed to close temporary download tab"
            );
        }
    }
    let _ = tokio::fs::remove_dir(&download_dir).await;
    result?;
    if !failures.is_empty() {
        downloaded.push(json!({
            "kind": "download_failures",
            "label": "ChatGPT Web artifact download failures",
            "failures": failures,
            "source": "chatgpt_behavior_button_download_failures",
        }));
    }
    Ok(downloaded)
}

#[derive(Clone, Debug)]
struct ArtifactCandidate {
    kind: String,
    label: String,
}

fn downloadable_artifact_candidates(artifacts: &[Value]) -> Vec<ArtifactCandidate> {
    let mut candidates = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for artifact in artifacts {
        let label = artifact
            .get("label")
            .or_else(|| artifact.get("buttonText"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let Some(label) = label else {
            continue;
        };
        let kind = artifact
            .get("kind")
            .and_then(Value::as_str)
            .unwrap_or("artifact")
            .to_string();
        let is_archive = kind == "archive"
            || label.to_ascii_lowercase().contains("zip")
            || label.contains("压缩包");
        let is_image = kind == "image";
        if !is_archive && !is_image {
            continue;
        }
        let kind = if is_archive { "archive" } else { "image" }.to_string();
        if !seen.insert(format!("{kind}\n{label}")) {
            continue;
        }
        let candidate = ArtifactCandidate {
            kind: kind.clone(),
            label: label.to_string(),
        };
        candidates.push(candidate);
    }
    candidates
}

async fn wait_for_behavior_buttons(cdp: &CdpClient) -> Result<(), String> {
    let expression = r#"(async () => {
      const deadline = Date.now() + 60000;
      while (Date.now() < deadline) {
        if (document.querySelector('button.behavior-btn, button.entity-underline, .behavior-btn')) {
          return true;
        }
        await new Promise(resolve => setTimeout(resolve, 500));
      }
      return false;
    })()"#;
    let value = cdp
        .send(
            "Runtime.evaluate",
            json!({
                "expression": expression,
                "awaitPromise": true,
                "returnByValue": true,
            }),
        )
        .await?;
    if value
        .get("result")
        .and_then(|result| result.get("value"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        Ok(())
    } else {
        Err("behavior artifact buttons did not appear".to_string())
    }
}

async fn click_archive_artifact_and_wait_for_download(
    cdp: &CdpClient,
    download_dir: &Path,
    target_dir: &Path,
    candidate: &ArtifactCandidate,
) -> Result<Option<Value>, String> {
    let mut events = cdp.subscribe();
    let click_target = locate_behavior_button(cdp, &candidate.label).await?;
    let Some((x, y, actual_label)) = click_target else {
        return Ok(None);
    };
    cdp_click(cdp, x, y).await?;

    let mut download = PendingDownload::default();
    let wait_result = timeout(Duration::from_secs(45), async {
        loop {
            let event = events
                .recv()
                .await
                .map_err(|error| format!("download event channel closed: {error}"))?;
            match event.method.as_str() {
                "Page.downloadWillBegin" | "Browser.downloadWillBegin" => {
                    download.guid = event
                        .params
                        .get("guid")
                        .and_then(Value::as_str)
                        .map(ToString::to_string);
                    download.url = event
                        .params
                        .get("url")
                        .and_then(Value::as_str)
                        .map(ToString::to_string);
                    download.suggested_filename = event
                        .params
                        .get("suggestedFilename")
                        .and_then(Value::as_str)
                        .map(ToString::to_string);
                }
                "Page.downloadProgress" | "Browser.downloadProgress" => {
                    if event.params.get("state").and_then(Value::as_str) == Some("completed") {
                        break Ok(());
                    }
                    if event.params.get("state").and_then(Value::as_str) == Some("canceled") {
                        break Err("download was canceled".to_string());
                    }
                }
                _ => {}
            }
        }
    })
    .await;
    match wait_result {
        Ok(Ok(())) => {}
        Ok(Err(error)) => return Err(error),
        Err(_) => return Ok(None),
    }

    let suggested = download
        .suggested_filename
        .as_deref()
        .unwrap_or(&actual_label);
    let source_path = wait_for_downloaded_file(download_dir, suggested).await?;
    let file_name = unique_download_file_name(target_dir, suggested).await;
    let target_path = target_dir.join(&file_name);
    tokio::fs::rename(&source_path, &target_path)
        .await
        .map_err(|error| {
            format!(
                "move ChatGPT behavior download {} to {} failed: {error}",
                source_path.display(),
                target_path.display()
            )
        })?;
    let bytes = tokio::fs::metadata(&target_path)
        .await
        .map_err(|error| {
            format!(
                "stat downloaded artifact {} failed: {error}",
                target_path.display()
            )
        })?
        .len();
    let metadata_path = target_path.with_extension(format!(
        "{}.json",
        target_path
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or("download")
    ));
    let metadata = json!({
        "source": "chatgpt_web",
        "sourceKind": "behavior_button",
        "conversationId": target_dir.file_name().and_then(|value| value.to_str()).unwrap_or_default(),
        "label": actual_label.clone(),
        "kind": candidate.kind.clone(),
        "downloadGuid": download.guid.clone(),
        "sourceUrl": download.url.clone(),
        "bytes": bytes,
        "cachedAt": iso_now(),
    });
    tokio::fs::write(
        &metadata_path,
        serde_json::to_vec_pretty(&metadata)
            .map_err(|error| format!("serialize behavior artifact metadata failed: {error}"))?,
    )
    .await
    .map_err(|error| {
        format!(
            "write behavior artifact metadata {} failed: {error}",
            metadata_path.display()
        )
    })?;

    info!(
        label = %actual_label,
        path = %target_path.display(),
        bytes,
        "chatgpt_web artifact: cached behavior button download"
    );
    Ok(Some(json!({
        "kind": candidate.kind.clone(),
        "label": actual_label,
        "fileName": file_name,
        "path": target_path.to_string_lossy(),
        "downloadGuid": download.guid,
        "sourceUrl": download.url,
        "bytes": bytes,
        "source": "chatgpt_behavior_button_download",
    })))
}

async fn click_image_artifact_and_download(
    auth: &AuthState,
    cdp: &CdpClient,
    target_dir: &Path,
    candidate: &ArtifactCandidate,
) -> Result<Option<Value>, String> {
    let click_target = locate_behavior_button(cdp, &candidate.label).await?;
    let Some((x, y, actual_label)) = click_target else {
        return Ok(None);
    };
    cdp_click(cdp, x, y).await?;
    let image_url = match wait_for_dialog_image_url(cdp, &actual_label).await {
        Ok(Some(url)) => url,
        Ok(None) => {
            let _ = dismiss_open_dialog(cdp).await;
            return Ok(None);
        }
        Err(error) => {
            let _ = dismiss_open_dialog(cdp).await;
            return Err(error);
        }
    };
    let _ = dismiss_open_dialog(cdp).await;
    let downloaded = native_binary(auth, &image_url).await?;
    let content_type = downloaded
        .content_type
        .as_deref()
        .unwrap_or("application/octet-stream");
    if !content_type.starts_with("image/") {
        let _ = dismiss_open_dialog(cdp).await;
        return Err(format!(
            "behavior image download expected image content type, got {content_type}"
        ));
    }
    if downloaded.bytes.is_empty() {
        let _ = dismiss_open_dialog(cdp).await;
        return Err("behavior image download returned empty response".to_string());
    }

    let file_name = unique_image_file_name(
        target_dir,
        &image_url,
        &actual_label,
        content_type,
        &downloaded.bytes,
    )
    .await;
    let target_path = target_dir.join(&file_name);
    if tokio::fs::metadata(&target_path).await.is_err() {
        tokio::fs::write(&target_path, &downloaded.bytes)
            .await
            .map_err(|error| {
                format!(
                    "write ChatGPT behavior image {} failed: {error}",
                    target_path.display()
                )
            })?;
    }
    let bytes = downloaded.bytes.len();
    let ext = image_extension_from_content_type(content_type);
    let metadata_path = target_path.with_extension(format!("{ext}.json"));
    let metadata = json!({
        "source": "chatgpt_web",
        "sourceKind": "behavior_button_image",
        "conversationId": target_dir.file_name().and_then(|value| value.to_str()).unwrap_or_default(),
        "label": actual_label.clone(),
        "kind": candidate.kind.clone(),
        "sourceUrl": image_url.clone(),
        "contentType": content_type,
        "bytes": bytes,
        "cachedAt": iso_now(),
    });
    tokio::fs::write(
        &metadata_path,
        serde_json::to_vec_pretty(&metadata)
            .map_err(|error| format!("serialize behavior image metadata failed: {error}"))?,
    )
    .await
    .map_err(|error| {
        format!(
            "write behavior image metadata {} failed: {error}",
            metadata_path.display()
        )
    })?;
    info!(
        label = %actual_label,
        path = %target_path.display(),
        bytes,
        "chatgpt_web artifact: cached behavior button image"
    );
    Ok(Some(json!({
        "kind": "image",
        "label": actual_label,
        "fileName": file_name,
        "path": target_path.to_string_lossy(),
        "sourceUrl": image_url,
        "contentType": content_type,
        "bytes": bytes,
        "source": "chatgpt_behavior_button_image_download",
    })))
}

async fn wait_for_dialog_image_url(cdp: &CdpClient, label: &str) -> Result<Option<String>, String> {
    let label_json = serde_json::to_string(label)
        .map_err(|error| format!("serialize image label failed: {error}"))?;
    let expression = format!(
        r#"(async () => {{
          const wanted = {label_json};
          const normalize = value => (value || '').replace(/\s+/g, ' ').trim();
          const pickUrl = value => {{
            if (!value) return null;
            const parts = String(value).split(',').map(item => item.trim().split(/\s+/)[0]);
            for (const part of parts) {{
              if (!part || !part.includes('/backend-api/estuary/content')) continue;
              try {{ return new URL(part, location.origin).href; }} catch (_) {{}}
            }}
            return null;
          }};
          const deadline = Date.now() + 30000;
          while (Date.now() < deadline) {{
            const dialogs = Array.from(document.querySelectorAll('[role="dialog"], [aria-modal="true"]'));
            const roots = dialogs.length ? dialogs : [document];
            for (const root of roots) {{
              const imgs = Array.from(root.querySelectorAll('img, source'));
              const exact = imgs.find(img => normalize(img.alt || img.getAttribute('aria-label') || img.title).includes(wanted));
              const ordered = exact ? [exact, ...imgs.filter(img => img !== exact)] : imgs;
              for (const img of ordered) {{
                const url = pickUrl(img.currentSrc || img.src || img.getAttribute('src') || img.getAttribute('srcset'));
                if (url) return url;
              }}
            }}
            await new Promise(resolve => setTimeout(resolve, 500));
          }}
          return null;
        }})()"#
    );
    let value = cdp
        .send(
            "Runtime.evaluate",
            json!({
                "expression": expression,
                "awaitPromise": true,
                "returnByValue": true,
            }),
        )
        .await?;
    Ok(value
        .get("result")
        .and_then(|result| result.get("value"))
        .and_then(Value::as_str)
        .map(ToString::to_string))
}

async fn dismiss_open_dialog(cdp: &CdpClient) -> Result<(), String> {
    cdp.send(
        "Input.dispatchKeyEvent",
        json!({"type":"keyDown","key":"Escape","code":"Escape","windowsVirtualKeyCode":27,"nativeVirtualKeyCode":27}),
    )
    .await?;
    cdp.send(
        "Input.dispatchKeyEvent",
        json!({"type":"keyUp","key":"Escape","code":"Escape","windowsVirtualKeyCode":27,"nativeVirtualKeyCode":27}),
    )
    .await?;
    sleep(Duration::from_millis(250)).await;
    Ok(())
}

#[derive(Default)]
struct PendingDownload {
    guid: Option<String>,
    suggested_filename: Option<String>,
    url: Option<String>,
}

async fn locate_behavior_button(
    cdp: &CdpClient,
    label: &str,
) -> Result<Option<(f64, f64, String)>, String> {
    let label_json =
        serde_json::to_string(label).map_err(|error| format!("serialize label failed: {error}"))?;
    let expression = format!(
        r#"(() => {{
          const wanted = {label_json};
          const normalize = value => (value || '').replace(/\s+/g, ' ').trim();
          const buttons = Array.from(document.querySelectorAll('button.behavior-btn, button.entity-underline, .behavior-btn'));
          const button = buttons.find(item => normalize(item.getAttribute('title') || item.innerText || item.textContent) === wanted)
            || buttons.find(item => normalize(item.getAttribute('title') || item.innerText || item.textContent).includes(wanted));
          if (!button) return {{ ok: false }};
          button.scrollIntoView({{ block: 'center', inline: 'center' }});
          const rect = button.getBoundingClientRect();
          return {{
            ok: rect.width > 0 && rect.height > 0,
            label: normalize(button.getAttribute('title') || button.innerText || button.textContent),
            x: rect.left + rect.width / 2,
            y: rect.top + rect.height / 2
          }};
        }})()"#
    );
    let value = cdp
        .send(
            "Runtime.evaluate",
            json!({"expression": expression, "returnByValue": true}),
        )
        .await?;
    let result = value
        .get("result")
        .and_then(|result| result.get("value"))
        .cloned()
        .unwrap_or(Value::Null);
    if !result.get("ok").and_then(Value::as_bool).unwrap_or(false) {
        return Ok(None);
    }
    let x = result.get("x").and_then(Value::as_f64).unwrap_or(1.0);
    let y = result.get("y").and_then(Value::as_f64).unwrap_or(1.0);
    let actual_label = result
        .get("label")
        .and_then(Value::as_str)
        .unwrap_or(label)
        .to_string();
    Ok(Some((x, y, actual_label)))
}

async fn cdp_click(cdp: &CdpClient, x: f64, y: f64) -> Result<(), String> {
    cdp.send(
        "Input.dispatchMouseEvent",
        json!({"type":"mouseMoved","x":x,"y":y,"button":"none","pointerType":"mouse"}),
    )
    .await?;
    cdp.send(
        "Input.dispatchMouseEvent",
        json!({"type":"mousePressed","x":x,"y":y,"button":"left","clickCount":1,"pointerType":"mouse"}),
    )
    .await?;
    cdp.send(
        "Input.dispatchMouseEvent",
        json!({"type":"mouseReleased","x":x,"y":y,"button":"left","clickCount":1,"pointerType":"mouse"}),
    )
    .await?;
    Ok(())
}

async fn wait_for_downloaded_file(download_dir: &Path, suggested: &str) -> Result<PathBuf, String> {
    let expected = sanitize_behavior_download_filename(suggested);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(mut entries) = tokio::fs::read_dir(download_dir).await {
            while let Some(entry) = entries
                .next_entry()
                .await
                .map_err(|error| format!("read download dir failed: {error}"))?
            {
                let path = entry.path();
                let file_name = path
                    .file_name()
                    .and_then(|value| value.to_str())
                    .unwrap_or_default();
                if file_name.ends_with(".crdownload") {
                    continue;
                }
                if file_name == expected || !file_name.is_empty() {
                    let size = tokio::fs::metadata(&path)
                        .await
                        .map_err(|error| format!("stat download candidate failed: {error}"))?
                        .len();
                    if size > 0 {
                        return Ok(path);
                    }
                }
            }
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(format!(
                "download completed but file {} was not found in {}",
                expected,
                download_dir.display()
            ));
        }
        sleep(Duration::from_millis(200)).await;
    }
}

async fn unique_download_file_name(target_dir: &Path, suggested: &str) -> String {
    let sanitized = sanitize_behavior_download_filename(suggested);
    let path = Path::new(&sanitized);
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .unwrap_or("chatgpt-web-artifact");
    let extension = path.extension().and_then(|value| value.to_str());
    for suffix in 0..1000 {
        let candidate = if suffix == 0 {
            sanitized.clone()
        } else if let Some(extension) = extension {
            format!("{stem}-{suffix}.{extension}")
        } else {
            format!("{stem}-{suffix}")
        };
        if tokio::fs::metadata(target_dir.join(&candidate))
            .await
            .is_err()
        {
            return candidate;
        }
    }
    format!("{}-{}", stem, now_millis())
}

async fn unique_image_file_name(
    target_dir: &Path,
    image_url: &str,
    label: &str,
    content_type: &str,
    bytes: &[u8],
) -> String {
    let ext = image_extension_from_content_type(content_type);
    let mut hasher = Sha1::new();
    hasher.update(image_url.as_bytes());
    hasher.update(bytes);
    let digest = format!("{:x}", hasher.finalize());
    let stem = image_file_stem_from_url(image_url)
        .unwrap_or_else(|| sanitize_behavior_download_filename(label))
        .trim_end_matches(&format!(".{ext}"))
        .trim()
        .to_string();
    let stem = if stem.is_empty() {
        "chatgpt-web-image".to_string()
    } else {
        sanitize_behavior_download_filename(&stem)
    };
    let base = format!("{stem}-{digest}");
    for suffix in 0..1000 {
        let candidate = if suffix == 0 {
            format!("{base}.{ext}")
        } else {
            format!("{base}-{suffix}.{ext}")
        };
        if tokio::fs::metadata(target_dir.join(&candidate))
            .await
            .is_err()
        {
            return candidate;
        }
    }
    format!("{}-{}.{}", base, now_millis(), ext)
}

fn image_file_stem_from_url(image_url: &str) -> Option<String> {
    let marker = "fn=";
    let query = image_url.split('?').nth(1)?;
    for pair in query.split('&') {
        if let Some(value) = pair.strip_prefix(marker) {
            let decoded = urlencoding::decode(value).ok()?;
            let sanitized = sanitize_behavior_download_filename(&decoded);
            let stem = Path::new(&sanitized)
                .file_stem()
                .and_then(|value| value.to_str())
                .map(str::trim)
                .filter(|value| !value.is_empty())?;
            return Some(stem.to_string());
        }
    }
    None
}

fn image_extension_from_content_type(content_type: &str) -> &'static str {
    let mime = content_type
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    match mime.as_str() {
        "image/jpeg" => "jpg",
        "image/png" => "png",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "image/bmp" => "bmp",
        _ => "img",
    }
}

pub(super) fn sanitize_behavior_download_filename(name: &str) -> String {
    let value = name
        .trim()
        .chars()
        .map(|ch| match ch {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '-',
            ch if ch.is_control() => '-',
            ch => ch,
        })
        .collect::<String>();
    let value = value.trim_matches(['.', ' ']).trim();
    if value.is_empty() {
        "chatgpt-web-artifact".to_string()
    } else {
        truncate_for_log(value, 180)
    }
}

fn now_millis() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default()
}
