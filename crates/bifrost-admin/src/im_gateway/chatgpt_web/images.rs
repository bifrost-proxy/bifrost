use std::collections::BTreeSet;
use std::path::Path;
use std::time::Duration;

use serde_json::{json, Value};
use sha1::{Digest, Sha1};
use tokio::time::{sleep, timeout};
use tracing::{info, warn};

use super::native::{native_binary, native_json};
use super::{
    evaluate_value, iso_now, seed_chatgpt_cookies, stop_requested, stopped_error, truncate_for_log,
    AuthState, BrowserSession, CdpClient, RuntimeConfig,
};

pub(in crate::im_gateway::chatgpt_web) async fn download_generated_images_for_conversation(
    config: &RuntimeConfig,
    auth: &AuthState,
    conversation_id: &str,
    generated_images: &[Value],
    requested_image_count_hint: Option<usize>,
    stop_marker_path: &Path,
) -> Result<Vec<Value>, String> {
    let expected_count = requested_image_count_hint.map_or(generated_images.len(), |hint| {
        generated_images.len().max(hint)
    });
    let mut urls = collect_generated_image_download_urls(config, auth, generated_images).await?;
    if urls.len() < expected_count {
        for url in collect_generated_image_urls_from_browser(
            config,
            auth,
            conversation_id,
            expected_count,
            stop_marker_path,
        )
        .await?
        {
            push_unique_url(&mut urls, &url);
        }
    }
    let mut seen = BTreeSet::new();
    urls.retain(|url| seen.insert(url.clone()));
    if urls.len() > expected_count {
        urls = urls.split_off(urls.len() - expected_count);
    }
    if urls.is_empty() {
        return Err(format!(
            "image_download_failed: expected {expected_count} ChatGPT generated image URLs, found 0"
        ));
    }
    if urls.len() < expected_count {
        // DOM may render the same image multiple times (thumbnail + expanded).
        // Proceed with whatever unique URLs we have rather than failing entirely.
        info!(
            expected = expected_count,
            found = urls.len(),
            "chatgpt_web image: fewer unique URLs than expected (DOM duplicates), proceeding with available"
        );
    }

    let target_dir = config.attachments_dir.join(conversation_id);
    tokio::fs::create_dir_all(&target_dir)
        .await
        .map_err(|error| {
            format!(
                "create ChatGPT image attachment dir {} failed: {error}",
                target_dir.display()
            )
        })?;

    let mut images = Vec::new();
    for (index, url) in urls.iter().enumerate() {
        if stop_requested(stop_marker_path).await {
            return Err(stopped_error());
        }
        // Retry image download with exponential backoff.
        // Image CDN can return transient 429/5xx, especially during
        // high-concurrency scenarios.
        let downloaded = {
            const MAX_IMAGE_DL_ATTEMPTS: u32 = 3;
            let mut last_err = String::new();
            let mut ok = None;
            for attempt in 1..=MAX_IMAGE_DL_ATTEMPTS {
                match native_binary(auth, url).await {
                    Ok(d) => {
                        ok = Some(d);
                        break;
                    }
                    Err(e) => {
                        warn!(
                            attempt,
                            max = MAX_IMAGE_DL_ATTEMPTS,
                            url = truncate_for_log(url, 120),
                            error = %e,
                            "chatgpt_web image: download failed, will retry"
                        );
                        last_err = e;
                        if attempt < MAX_IMAGE_DL_ATTEMPTS {
                            let backoff = Duration::from_secs(2u64.pow(attempt));
                            sleep(backoff).await;
                        }
                    }
                }
            }
            match ok {
                Some(d) => d,
                None => {
                    // Fallback: fetch via CDP (browser has auth cookies and cache)
                    info!(
                        url = truncate_for_log(url, 120),
                        "chatgpt_web image: native download failed, trying CDP fetch fallback"
                    );
                    match fetch_image_via_cdp(config, conversation_id, url).await {
                        Ok(d) => d,
                        Err(cdp_err) => {
                            warn!(
                                error = %cdp_err,
                                "chatgpt_web image: CDP fetch fallback also failed"
                            );
                            return Err(format!(
                                "image_download_failed: all attempts failed for {}: native={last_err}, cdp={cdp_err}",
                                truncate_for_log(url, 120)
                            ));
                        }
                    }
                }
            }
        };
        if downloaded.bytes.is_empty() {
            return Err(format!(
                "image_download_failed: empty image response for {url}"
            ));
        }
        let content_type = downloaded
            .content_type
            .as_deref()
            .unwrap_or("application/octet-stream");
        if !content_type.starts_with("image/") {
            return Err(format!(
                "image_download_failed: expected image content type for {url}, got {content_type}"
            ));
        }
        let mut hasher = Sha1::new();
        hasher.update(url.as_bytes());
        hasher.update(&downloaded.bytes);
        let digest = format!("{:x}", hasher.finalize());
        let ext = image_extension_from_content_type(content_type);
        let file_name = format!("chatgpt-web-image-{}-{}.{}", index + 1, digest, ext);
        let path = target_dir.join(file_name);
        if tokio::fs::metadata(&path).await.is_err() {
            tokio::fs::write(&path, &downloaded.bytes)
                .await
                .map_err(|error| {
                    format!(
                        "write ChatGPT image attachment {} failed: {error}",
                        path.display()
                    )
                })?;
        }
        let metadata_path = path.with_extension(format!("{ext}.json"));
        let metadata = json!({
            "source": "chatgpt_web",
            "conversationId": conversation_id,
            "sourceUrl": url,
            "contentType": content_type,
            "bytes": downloaded.bytes.len(),
            "cachedAt": iso_now(),
        });
        tokio::fs::write(
            &metadata_path,
            serde_json::to_vec_pretty(&metadata)
                .map_err(|error| format!("serialize ChatGPT image metadata failed: {error}"))?,
        )
        .await
        .map_err(|error| {
            format!(
                "write ChatGPT image metadata {} failed: {error}",
                metadata_path.display()
            )
        })?;
        info!(
            path = %path.display(),
            bytes = downloaded.bytes.len(),
            content_type,
            "chatgpt_web image: cached generated original image"
        );
        images.push(json!({
            "path": path.to_string_lossy(),
            "sourceUrl": url,
            "contentType": content_type,
            "bytes": downloaded.bytes.len(),
        }));
    }
    Ok(images)
}

async fn collect_generated_image_download_urls(
    config: &RuntimeConfig,
    auth: &AuthState,
    generated_images: &[Value],
) -> Result<Vec<String>, String> {
    let mut urls = Vec::new();
    for image in generated_images {
        if let Some(url) = image
            .get("sourceUrl")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            push_unique_url(&mut urls, url);
            continue;
        }
        let Some(file_id) = image
            .get("fileId")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| value.starts_with("file_"))
        else {
            continue;
        };
        let path = format!(
            "/backend-api/files/{}/download",
            urlencoding::encode(file_id)
        );
        let response = match native_json(config, auth, &path).await {
            Ok(response) => response,
            Err(error) => {
                warn!(
                    file_id,
                    error = %error,
                    "chatgpt_web image: ChatGPT file download URL lookup failed; will try remaining image sources"
                );
                continue;
            }
        };
        let Some(download_url) = response
            .get("download_url")
            .or_else(|| response.get("downloadUrl"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| value.starts_with("http://") || value.starts_with("https://"))
        else {
            warn!(
                file_id,
                response = %truncate_for_log(&response.to_string(), 300),
                "chatgpt_web image: ChatGPT file download response missing download_url"
            );
            continue;
        };
        push_unique_url(&mut urls, download_url);
    }
    Ok(urls)
}

async fn collect_generated_image_urls_from_browser(
    config: &RuntimeConfig,
    auth: &AuthState,
    conversation_id: &str,
    expected_count: usize,
    stop_marker_path: &Path,
) -> Result<Vec<String>, String> {
    let url = format!("{}/c/{}", config.chatgpt.base_url, conversation_id);
    let headless = config.browser.execution_mode != "headed";
    let browser = BrowserSession::get_or_launch(config, headless).await?;
    let mut _target_id = None;
    let mut client = None;
    let result = async {
        let page = browser.open_or_find_page(&url).await?;
        _target_id = Some(page.id.clone());
        let cdp = CdpClient::connect_with_retry(&page.web_socket_debugger_url, 3).await?;
        cdp.enable_domains().await?;
        seed_chatgpt_cookies(&cdp, auth).await?;
        if !headless {
            cdp.send("Page.bringToFront", json!({})).await?;
        }

        let mut fallback_dom_urls = Vec::<String>::new();
        // If the page is already on the correct conversation, extract images
        // from the DOM directly — avoids Page.navigate which triggers 429.
        let already_on_page = page.url.contains(&format!("/c/{conversation_id}"));
        if already_on_page {
            info!(
                page_url = %page.url,
                conversation_id,
                "chatgpt_web image: page already on conversation, trying DOM extraction"
            );
            let dom_urls = collect_dom_estuary_image_urls(&cdp).await.unwrap_or_default();
            if !dom_urls.is_empty() {
                info!(
                    count = dom_urls.len(),
                    "chatgpt_web image: found images via DOM extraction"
                );
                for image_url in &dom_urls {
                    push_unique_url(&mut fallback_dom_urls, image_url);
                }
                if dom_urls.len() >= expected_count {
                    client = Some(cdp);
                    return Ok(dom_urls);
                }
                info!(
                    found = dom_urls.len(),
                    expected = expected_count,
                    "chatgpt_web image: DOM extraction below expected count, waiting for lazy images"
                );
            }
            // DOM extraction found nothing — wait a bit and retry (images may
            // still be loading/rendering).
            info!("chatgpt_web image: DOM extraction empty, waiting for render then retrying");
            sleep(Duration::from_secs(3)).await;
            let dom_urls = collect_dom_estuary_image_urls(&cdp).await.unwrap_or_default();
            if !dom_urls.is_empty() {
                info!(
                    count = dom_urls.len(),
                    "chatgpt_web image: found images via DOM extraction (retry)"
                );
                for image_url in &dom_urls {
                    push_unique_url(&mut fallback_dom_urls, image_url);
                }
                if dom_urls.len() >= expected_count {
                    client = Some(cdp);
                    return Ok(dom_urls);
                }
                info!(
                    found = dom_urls.len(),
                    expected = expected_count,
                    "chatgpt_web image: retry DOM extraction still below expected count, falling back to network observation"
                );
            }
            warn!("chatgpt_web image: DOM extraction did not reach expected count, falling back to network observation");
        }

        // Navigate and observe network requests for image URLs.
        let mut events = cdp.subscribe();
        cdp.send("Page.navigate", json!({"url": url})).await?;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(45);
        let mut urls = fallback_dom_urls;
        while tokio::time::Instant::now() < deadline {
            if stop_requested(stop_marker_path).await {
                return Err(stopped_error());
            }
            match timeout(Duration::from_millis(500), events.recv()).await {
                Ok(Ok(event)) if event.method == "Network.requestWillBeSent" => {
                    if let Some(image_url) = event
                        .params
                        .get("request")
                        .and_then(|request| request.get("url"))
                        .and_then(Value::as_str)
                        .filter(|value| value.contains("/backend-api/estuary/content"))
                    {
                        push_unique_url(&mut urls, image_url);
                    }
                }
                Ok(Ok(_)) | Ok(Err(_)) | Err(_) => {}
            }
            if let Ok(dom_urls) = collect_dom_estuary_image_urls(&cdp).await {
                for image_url in dom_urls {
                    push_unique_url(&mut urls, &image_url);
                }
            }
            if urls.len() >= expected_count {
                break;
            }
        }
        if urls.is_empty() {
            warn!(
                conversation_id,
                expected_count,
                "chatgpt_web image: no estuary image URLs found from rendered conversation"
            );
        }
        client = Some(cdp);
        Ok(urls)
    }
    .await;
    // Do NOT navigate away — stay on the current page to avoid 429 rate-limits.
    if let Some(cdp) = client {
        cdp.close();
    }
    result
}

async fn collect_dom_estuary_image_urls(cdp: &CdpClient) -> Result<Vec<String>, String> {
    let expression = r#"
      (() => {
        const urls = [];
        const pushUrl = (value) => {
          if (!value) return;
          const candidates = String(value).split(',').map((item) => item.trim().split(/\s+/)[0]);
          for (const candidate of candidates) {
            if (!candidate || !candidate.includes('/backend-api/estuary/content')) continue;
            try { urls.push(new URL(candidate, location.origin).href); } catch (_) {}
          }
        };
        for (const el of document.querySelectorAll('img, source')) {
          pushUrl(el.currentSrc);
          pushUrl(el.src);
          pushUrl(el.getAttribute('src'));
          pushUrl(el.getAttribute('srcset'));
        }
        for (const el of document.querySelectorAll('[style*="estuary/content"]')) {
          pushUrl(el.getAttribute('style'));
        }
        for (const scroller of document.querySelectorAll('main, [data-testid], div')) {
          if (scroller.scrollHeight > scroller.clientHeight) {
            scroller.scrollTop = scroller.scrollHeight;
          }
        }
        return Array.from(new Set(urls));
      })()
    "#;
    let value = evaluate_value(cdp, expression).await?;
    Ok(value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(ToString::to_string)
        .collect())
}

fn push_unique_url(urls: &mut Vec<String>, url: &str) {
    if !urls.iter().any(|item| item == url) {
        urls.push(url.to_string());
    }
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

/// Fallback: fetch an image via the browser's JavaScript fetch API.
/// The browser already has authentication cookies so this works even when
/// the native HTTP client fails (e.g., missing cookies, 403, CORS).
async fn fetch_image_via_cdp(
    config: &RuntimeConfig,
    conversation_id: &str,
    url: &str,
) -> Result<super::native::NativeBinary, String> {
    use base64::Engine;

    let tab = super::browser::get_conversation_tab(&config.profile_dir, conversation_id)
        .ok_or_else(|| "no active browser tab for conversation".to_string())?;
    if tab.cdp.is_closed() {
        return Err("CDP connection closed".to_string());
    }

    // Use JavaScript fetch in the browser context — it carries all auth cookies automatically.
    let js = format!(
        r#"(async () => {{
            try {{
                const resp = await fetch({url_json});
                if (!resp.ok) return JSON.stringify({{ error: 'HTTP ' + resp.status }});
                const contentType = resp.headers.get('content-type') || 'image/png';
                const blob = await resp.blob();
                const reader = new FileReader();
                const base64 = await new Promise((resolve, reject) => {{
                    reader.onloadend = () => resolve(reader.result);
                    reader.onerror = reject;
                    reader.readAsDataURL(blob);
                }});
                // base64 is "data:<mime>;base64,<data>"
                const data = base64.split(',')[1] || '';
                return JSON.stringify({{ ok: true, contentType, data, bytes: blob.size }});
            }} catch (e) {{
                return JSON.stringify({{ error: e.message || String(e) }});
            }}
        }})()"#,
        url_json = serde_json::to_string(url).unwrap_or_default()
    );

    let result = tokio::time::timeout(Duration::from_secs(20), async {
        evaluate_value(&tab.cdp, &js).await
    })
    .await
    .map_err(|_| "CDP fetch timed out after 20s".to_string())?
    .map_err(|e| format!("CDP evaluate failed: {e}"))?;

    let data: Value = if let Some(s) = result.as_str() {
        serde_json::from_str(s).map_err(|e| format!("parse CDP fetch result: {e}"))?
    } else {
        result
    };

    if let Some(error) = data.get("error").and_then(Value::as_str) {
        return Err(format!("browser fetch failed: {error}"));
    }

    let base64_data = data
        .get("data")
        .and_then(Value::as_str)
        .ok_or("no data in CDP fetch response")?;
    let content_type = data
        .get("contentType")
        .and_then(Value::as_str)
        .map(ToString::to_string);

    let bytes = base64::engine::general_purpose::STANDARD
        .decode(base64_data)
        .map_err(|e| format!("base64 decode failed: {e}"))?;

    info!(
        url = truncate_for_log(url, 120),
        bytes = bytes.len(),
        "chatgpt_web image: successfully fetched via CDP browser fetch"
    );

    Ok(super::native::NativeBinary {
        bytes: bytes::Bytes::from(bytes),
        content_type,
    })
}
