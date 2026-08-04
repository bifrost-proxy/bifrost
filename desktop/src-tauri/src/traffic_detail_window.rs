use super::HOST_WINDOW_LABEL;
use tauri::{webview::WebviewWindowBuilder, AppHandle, Manager, WebviewUrl};

const TRAFFIC_DETAIL_WINDOW_LABEL: &str = "traffic-detail";
const TRAFFIC_DETAIL_CLOSED_EVENT: &str = "desktop://traffic-detail-closed";
const MAX_TRAFFIC_RECORD_ID_LENGTH: usize = 512;
const MAX_TRAFFIC_POPUP_ID_LENGTH: usize = 128;

fn percent_encode_query_value(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(*byte as char);
        } else {
            encoded.push('%');
            encoded.push_str(&format!("{byte:02X}"));
        }
    }
    encoded
}

pub(super) fn traffic_detail_app_path(record_id: &str, popup_id: &str) -> Result<String, String> {
    let record_id = record_id.trim();
    let popup_id = popup_id.trim();

    if record_id.is_empty() {
        return Err("traffic record id must not be empty".to_string());
    }
    if popup_id.is_empty() {
        return Err("traffic detail popup id must not be empty".to_string());
    }
    if record_id.chars().count() > MAX_TRAFFIC_RECORD_ID_LENGTH {
        return Err(format!(
            "traffic record id exceeds {MAX_TRAFFIC_RECORD_ID_LENGTH} characters"
        ));
    }
    if popup_id.chars().count() > MAX_TRAFFIC_POPUP_ID_LENGTH {
        return Err(format!(
            "traffic detail popup id exceeds {MAX_TRAFFIC_POPUP_ID_LENGTH} characters"
        ));
    }

    let encoded_record_id = percent_encode_query_value(record_id);
    let encoded_popup_id = percent_encode_query_value(popup_id);
    Ok(format!(
        "index.html#/traffic/detail?detached=1&popupId={encoded_popup_id}&id={encoded_record_id}"
    ))
}

#[tauri::command]
pub(super) async fn open_traffic_detail_window(
    app: AppHandle,
    record_id: String,
    popup_id: String,
) -> Result<(), String> {
    open_traffic_detail_window_impl(app, record_id, popup_id).await
}

pub(super) async fn open_traffic_detail_window_impl<R: tauri::Runtime>(
    app: AppHandle<R>,
    record_id: String,
    popup_id: String,
) -> Result<(), String> {
    let app_path = traffic_detail_app_path(&record_id, &popup_id)?;

    if let Some(window) = app.get_webview_window(TRAFFIC_DETAIL_WINDOW_LABEL) {
        let fragment = app_path
            .strip_prefix("index.html#")
            .unwrap_or(app_path.as_str());
        let mut url = window
            .url()
            .map_err(|error| format!("failed to read traffic detail window URL: {error}"))?;
        url.set_fragment(Some(fragment));
        window
            .navigate(url)
            .map_err(|error| format!("failed to navigate traffic detail window: {error}"))?;
        window
            .show()
            .map_err(|error| format!("failed to show traffic detail window: {error}"))?;
        window
            .set_focus()
            .map_err(|error| format!("failed to focus traffic detail window: {error}"))?;
        return Ok(());
    }

    let window = WebviewWindowBuilder::new(
        &app,
        TRAFFIC_DETAIL_WINDOW_LABEL,
        WebviewUrl::App(app_path.into()),
    )
    .title("Bifrost Traffic Detail")
    .inner_size(1440.0, 900.0)
    .min_inner_size(900.0, 640.0)
    .resizable(true)
    .build()
    .map_err(|error| format!("failed to create traffic detail window: {error}"))?;

    let app_for_close = app.clone();
    window.on_window_event(move |event| {
        if matches!(event, tauri::WindowEvent::Destroyed) {
            if let Some(webview) = app_for_close.get_webview(HOST_WINDOW_LABEL) {
                let script = format!(
                    r#"window.dispatchEvent(new CustomEvent({:?}))"#,
                    TRAFFIC_DETAIL_CLOSED_EVENT
                );
                let _ = webview.eval(&script);
            }
        }
    });
    window
        .show()
        .map_err(|error| format!("failed to show traffic detail window: {error}"))?;
    window
        .set_focus()
        .map_err(|error| format!("failed to focus traffic detail window: {error}"))?;
    Ok(())
}

#[tauri::command]
pub(super) async fn close_traffic_detail_window(app: AppHandle) -> Result<(), String> {
    close_traffic_detail_window_impl(app).await
}

pub(super) async fn close_traffic_detail_window_impl<R: tauri::Runtime>(
    app: AppHandle<R>,
) -> Result<(), String> {
    if let Some(window) = app.get_webview_window(TRAFFIC_DETAIL_WINDOW_LABEL) {
        window
            .close()
            .map_err(|error| format!("failed to close traffic detail window: {error}"))?;
    }
    Ok(())
}
