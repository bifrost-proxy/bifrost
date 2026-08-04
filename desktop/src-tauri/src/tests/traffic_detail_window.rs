use crate::{
    close_traffic_detail_window_impl, dispatch_traffic_detail_closed,
    open_traffic_detail_window_impl, traffic_detail_app_path,
};
use tauri::Manager;

#[test]
fn traffic_detail_path_encodes_query_values() {
    assert_eq!(
        traffic_detail_app_path(" REQ /?#%中文 ", " popup&=1 ").unwrap(),
        "index.html#/traffic/detail?detached=1&popupId=popup%26%3D1&id=REQ%20%2F%3F%23%25%E4%B8%AD%E6%96%87"
    );
}

#[test]
fn traffic_detail_path_rejects_empty_values() {
    assert_eq!(
        traffic_detail_app_path("   ", "popup-1").unwrap_err(),
        "traffic record id must not be empty"
    );
    assert_eq!(
        traffic_detail_app_path("REQ-1", "  ").unwrap_err(),
        "traffic detail popup id must not be empty"
    );
}

#[test]
fn traffic_detail_path_rejects_overlong_values() {
    assert_eq!(
        traffic_detail_app_path(&"r".repeat(513), "popup-1").unwrap_err(),
        "traffic record id exceeds 512 characters"
    );
    assert_eq!(
        traffic_detail_app_path("REQ-1", &"p".repeat(129)).unwrap_err(),
        "traffic detail popup id exceeds 128 characters"
    );
}

#[test]
fn traffic_detail_close_event_targets_main_webview_not_host_shell() {
    let app = tauri::test::mock_builder()
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build mock desktop app");

    tauri::webview::WebviewWindowBuilder::new(
        &app,
        "host",
        tauri::WebviewUrl::App("index.html".into()),
    )
    .build()
    .expect("build host-labeled mock webview");
    assert_eq!(
        dispatch_traffic_detail_closed(app.handle()).unwrap_err(),
        "main traffic webview is unavailable"
    );

    tauri::webview::WebviewWindowBuilder::new(
        &app,
        "main",
        tauri::WebviewUrl::App("index.html".into()),
    )
    .build()
    .expect("build main mock webview");
    dispatch_traffic_detail_closed(app.handle())
        .expect("dispatch close event to the main traffic webview");
}

#[test]
fn native_traffic_detail_window_is_created_reused_and_close_is_idempotent() {
    let app = tauri::test::mock_builder()
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build mock desktop app");

    tauri::async_runtime::block_on(open_traffic_detail_window_impl(
        app.handle().clone(),
        "REQ-1".to_string(),
        "popup-1".to_string(),
    ))
    .expect("open native detail window");
    assert!(app.get_webview_window("traffic-detail").is_some());
    assert_eq!(app.webview_windows().len(), 1);

    tauri::async_runtime::block_on(open_traffic_detail_window_impl(
        app.handle().clone(),
        "REQ-2".to_string(),
        "popup-2".to_string(),
    ))
    .expect("reuse native detail window");
    assert_eq!(app.webview_windows().len(), 1);
    assert_eq!(
        app.get_webview_window("traffic-detail")
            .expect("reused traffic detail window")
            .url()
            .expect("read reused traffic detail URL")
            .fragment(),
        Some("/traffic/detail?detached=1&popupId=popup-2&id=REQ-2")
    );

    tauri::async_runtime::block_on(close_traffic_detail_window_impl(app.handle().clone()))
        .expect("close native detail window");
    tauri::async_runtime::block_on(close_traffic_detail_window_impl(app.handle().clone()))
        .expect("repeated close remains safe in mock runtime");
}
