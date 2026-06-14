use super::*;
use std::collections::BTreeMap;

#[test]
fn chatgpt_web_browser_defaults_to_headed_mode() {
    let config = BrowserConfig::default();
    assert_eq!(config.execution_mode, "headed");
}

#[test]
fn im_long_reply_delivery_chatgpt_web_dom_extraction_does_not_truncate_response_text() {
    let source = include_str!("interaction.rs");
    assert!(
        !source.contains("text.slice(0, 10000)"),
        "ChatGPT Web DOM extraction must persist the full final response text"
    );
    assert!(
        !source.contains("t.slice(0, 10000)"),
        "ChatGPT Web DOM extraction must preserve full natural response batches"
    );
}

#[test]
fn composer_text_injection_uses_paste_only_above_threshold() {
    assert_eq!(
        composer_text_injection_mode(&"a".repeat(120)),
        ComposerTextInjectionMode::InsertText
    );
    assert_eq!(
        composer_text_injection_mode(&"a".repeat(121)),
        ComposerTextInjectionMode::NativeClipboardPaste
    );
}

#[test]
fn composer_text_injection_threshold_counts_characters() {
    assert_eq!(
        composer_text_injection_mode(&"你".repeat(120)),
        ComposerTextInjectionMode::InsertText
    );
    assert_eq!(
        composer_text_injection_mode(&"你".repeat(121)),
        ComposerTextInjectionMode::NativeClipboardPaste
    );
}

#[test]
fn composer_text_injection_large_text_uses_native_clipboard_paste() {
    assert_eq!(
        composer_text_injection_mode(&"daily report\n".repeat(40_000)),
        ComposerTextInjectionMode::NativeClipboardPaste
    );
}

#[test]
fn native_clipboard_paste_uses_platform_modifier() {
    let expected = if cfg!(target_os = "macos") { 4 } else { 2 };
    assert_eq!(paste_modifier(), expected);
}

#[test]
fn native_clipboard_paste_uses_browser_clipboard_api_not_system_clipboard() {
    let source = include_str!("send.rs");
    assert!(source.contains("Browser.grantPermissions"));
    assert!(source.contains("navigator.clipboard.writeText"));
    assert!(!source.contains("SystemClipboardGuard"));
    assert!(!source.contains("pbcopy"));
}

#[test]
fn native_clipboard_paste_scales_send_button_waits_for_large_prompts() {
    assert_eq!(
        send_button_ready_max_wait(&"a".repeat(120)),
        Duration::from_secs(10)
    );

    let large_prompt = "daily report\n".repeat(40_000);
    assert!(send_button_ready_max_wait(&large_prompt) > Duration::from_secs(30));
    assert_eq!(
        send_button_ready_retry_max_wait(&large_prompt),
        Duration::from_secs(60)
    );
}

#[test]
fn failure_diagnostics_conversation_hint_accepts_known_param_names() {
    let mut request = ExternalCliRunRequest {
        images: Vec::new(),
        message: "hello".to_string(),
        operation: "ask".to_string(),
        params: json!({"conversationId": " c1 "}),
        provider_id: None,
        runner_id: None,
        session_key: None,
        runtime: "external_cli".to_string(),
        adapter: ADAPTER_ID.to_string(),
        work_dir: None,
        instructions: None,
        adapter_config: ExternalCliAdapterConfig::default(),
        allow_work_dirs: Vec::new(),
        inject_bifrost_tools: false,
        skill_paths: Vec::new(),
    };
    assert_eq!(
        conversation_id_hint_from_request(&request).as_deref(),
        Some("c1")
    );

    request.params = json!({"conversation_id": "c2"});
    assert_eq!(
        conversation_id_hint_from_request(&request).as_deref(),
        Some("c2")
    );
}

#[tokio::test]
async fn failure_diagnostics_manifest_is_written_to_run_dir() {
    let temp = tempfile::tempdir().expect("tempdir");
    write_run_failure_manifest(
        temp.path(),
        "ask",
        Some("c1"),
        "browser_wrong_page",
        vec![json!({"name": "page_dom", "status": "written"})],
    )
    .await;

    let manifest = tokio::fs::read_to_string(temp.path().join("failure_diagnostics.json"))
        .await
        .expect("read diagnostics");
    let manifest: Value = serde_json::from_str(&manifest).expect("parse diagnostics");
    assert_eq!(manifest["status"], "failed");
    assert_eq!(manifest["operation"], "ask");
    assert_eq!(manifest["conversationId"], "c1");
    assert_eq!(manifest["error"], "browser_wrong_page");
    assert_eq!(manifest["artifacts"][0]["name"], "page_dom");
}

#[tokio::test]
async fn run_adapter_writes_failure_diagnostics_on_authenticated_error() {
    let temp = tempfile::tempdir().expect("tempdir");
    let state_path = temp.path().join("auth_state.json");
    let mut headers = BTreeMap::new();
    headers.insert("authorization".to_string(), "Bearer captured".to_string());
    let state = AuthState {
        captured_at: iso_now(),
        base_url: DEFAULT_BASE_URL.to_string(),
        user_agent: "Mozilla/5.0".to_string(),
        captured_auth_headers: headers,
        captured_auth_identity: AuthorizationIdentity {
            has_bearer_token: true,
            has_profile_email: true,
            profile_email_verified: Some(true),
            has_user_identity: true,
            has_account_identity: true,
            complete: true,
            expires_at: None,
        },
        captured_account_check: Some(BrowserAccountCheckProof {
            captured_at: iso_now(),
            status: 200,
            logged_in: true,
        }),
        cookies: Vec::new(),
    };
    write_auth_state(&state_path, &state)
        .await
        .expect("write auth state");
    let request = ExternalCliRunRequest {
        images: Vec::new(),
        message: "hello".to_string(),
        operation: "bad-op".to_string(),
        params: Value::Null,
        provider_id: None,
        runner_id: None,
        session_key: None,
        runtime: "external_cli".to_string(),
        adapter: ADAPTER_ID.to_string(),
        work_dir: None,
        instructions: None,
        adapter_config: ExternalCliAdapterConfig {
            timeout_secs: Some(1),
            extra: BTreeMap::from([
                (
                    "auth".to_string(),
                    json!({"statePath": state_path.to_string_lossy()}),
                ),
                (
                    "browser".to_string(),
                    json!({
                        "executable": temp.path().join("missing-browser").to_string_lossy(),
                        "openOnAuthRequired": false,
                    }),
                ),
                ("chatgpt".to_string(), json!({"baseUrl": "http://[::1"})),
            ]),
            ..ExternalCliAdapterConfig::default()
        },
        allow_work_dirs: Vec::new(),
        inject_bifrost_tools: false,
        skill_paths: Vec::new(),
    };
    let run_dir = temp.path().join("run");
    tokio::fs::create_dir_all(&run_dir).await.expect("run dir");

    let error = run_adapter(&request, "hello", &run_dir)
        .await
        .expect_err("unsupported operation should fail");

    assert!(error.contains("unsupported chatgpt_web operation"));
    let manifest = tokio::fs::read_to_string(run_dir.join("failure_diagnostics.json"))
        .await
        .expect("read diagnostics");
    let manifest: Value = serde_json::from_str(&manifest).expect("parse diagnostics");
    assert_eq!(manifest["operation"], "bad-op");
    assert_eq!(manifest["artifacts"][0]["name"], "conversation_response");
    assert_eq!(manifest["artifacts"][0]["status"], "skipped");
    assert_eq!(manifest["artifacts"][1]["name"], "page_dom");
    assert_eq!(manifest["artifacts"][1]["status"], "capture_failed");
}

#[test]
fn page_dom_metadata_keeps_full_html_in_separate_artifact() {
    let capture = json!({
        "url": "https://chatgpt.com/",
        "title": "ChatGPT",
        "readyState": "complete",
        "bodyText": "hello",
        "html": "<html><body>hello</body></html>",
        "composer": [{"tag": "DIV"}],
        "landmarks": [{"tag": "MAIN"}]
    });
    let metadata =
        diagnostics::page_dom_metadata(&capture, std::path::Path::new("/tmp/page_dom.html"), 31);
    assert_eq!(metadata["htmlPath"], "/tmp/page_dom.html");
    assert_eq!(metadata["htmlBytes"], 31);
    assert!(metadata.get("html").is_none());
    assert_eq!(metadata["composer"][0]["tag"], "DIV");
}

#[test]
fn generated_image_tool_result_is_final_and_counts_all_images() {
    let conversation = json!({
        "mapping": {
            "user": {
                "message": {
                    "id": "user-1",
                    "author": {"role": "user"},
                    "create_time": 10.0,
                    "status": "finished_successfully",
                    "content": {"parts": ["帮我生成4张可爱的小猫咪"]}
                }
            },
            "tool": {
                "message": {
                    "id": "tool-1",
                    "author": {"role": "tool"},
                    "create_time": 11.0,
                    "status": "finished_successfully",
                    "metadata": {"model_slug": "gpt-5-4-thinking"},
                    "content": {
                        "parts": [
                            {"content_type": "image_asset_pointer", "asset_pointer": "sediment://file_cat_1", "width": 1024, "height": 1024, "size_bytes": 11},
                            {"content_type": "image_asset_pointer", "asset_pointer": "sediment://file_cat_2", "width": 1024, "height": 1024, "size_bytes": 22},
                            {"content_type": "image_asset_pointer", "asset_pointer": "sediment://file_cat_3", "width": 1024, "height": 1024, "size_bytes": 33},
                            {"content_type": "image_asset_pointer", "asset_pointer": "sediment://file_cat_4", "width": 1024, "height": 1024, "size_bytes": 44},
                            "Generated images from the last `image_gen.text2im` call were saved at:\n- /mnt/data/cat_1.png (wxh = 1024 x 1024)\n- /mnt/data/cat_2.png (wxh = 1024 x 1024)\n- /mnt/data/cat_3.png (wxh = 1024 x 1024)\n- /mnt/data/cat_4.png (wxh = 1024 x 1024)"
                        ]
                    }
                }
            }
        }
    });
    let message = interaction::find_generated_images_tool_message(&conversation, Some(10.5))
        .expect("generated image tool result should be final");
    assert_eq!(message["text"], "已生成图片，正在发送原图。");
    assert_eq!(message["generatedImages"].as_array().map(Vec::len), Some(4));
    assert_eq!(
        message["generatedImages"][0]["fileId"].as_str(),
        Some("file_cat_1")
    );
    assert_eq!(message["generatedImages"][3]["sizeBytes"], 44);
}

#[test]
fn generated_image_assets_without_finished_path_list_are_final_when_not_streaming() {
    let conversation = json!({
        "mapping": {
            "tool": {
                "message": {
                    "id": "tool-final-assets",
                    "author": {"role": "tool"},
                    "create_time": 11.0,
                    "status": "finished_successfully",
                    "content": {
                        "parts": [
                            {"content_type": "image_asset_pointer", "asset_pointer": "sediment://file_cat_1", "width": 1024, "height": 1024, "size_bytes": 11},
                            {"content_type": "image_asset_pointer", "asset_pointer": "sediment://file_cat_2", "width": 1024, "height": 1024, "size_bytes": 22}
                        ]
                    }
                }
            }
        }
    });

    let message = interaction::find_generated_images_tool_message(&conversation, Some(10.5))
        .expect("finished image_asset_pointer messages should be final without /mnt/data text");
    assert_eq!(message["text"], "已生成图片，正在发送原图。");
    assert_eq!(message["generatedImages"].as_array().map(Vec::len), Some(2));
    assert_eq!(
        message["generatedImages"][1]["fileId"].as_str(),
        Some("file_cat_2")
    );
}

#[test]
fn generated_image_assets_wait_when_any_message_is_in_progress() {
    let conversation = json!({
        "mapping": {
            "tool": {
                "message": {
                    "id": "tool-partial",
                    "author": {"role": "tool"},
                    "create_time": 11.0,
                    "status": "finished_successfully",
                    "content": {
                        "parts": [
                            {"content_type": "image_asset_pointer", "asset_pointer": "sediment://file_cat_1", "width": 1024, "height": 1024, "size_bytes": 11},
                            {"content_type": "image_asset_pointer", "asset_pointer": "sediment://file_cat_2", "width": 1024, "height": 1024, "size_bytes": 22}
                        ]
                    }
                }
            },
            "assistant-streaming": {
                "message": {
                    "id": "assistant-streaming",
                    "author": {"role": "assistant"},
                    "create_time": 11.5,
                    "status": "in_progress",
                    "end_turn": false,
                    "content": {"parts": [""]}
                }
            }
        }
    });

    assert!(
        interaction::try_waited_final_from_sse(&conversation, Some(10.5)).is_none(),
        "asset-only image messages must not complete while another turn is still in progress"
    );
}

#[test]
fn dom_content_accepts_short_text_replies() {
    assert!(interaction::dom_content_is_usable("OK", 0));
    assert!(interaction::dom_content_is_usable(" 好 ", 0));
    assert!(interaction::dom_content_is_usable("ChatGPT 说：OK", 0));
    assert!(interaction::dom_content_is_usable("", 1));
    assert!(!interaction::dom_content_is_usable("ChatGPT 说：", 0));
    assert!(!interaction::dom_content_is_usable("   ", 0));
}

#[test]
fn dom_artifact_notice_lists_behavior_button_images_and_archive() {
    let artifacts = vec![
        json!({"kind": "image", "label": "大模型竞争格局"}),
        json!({"kind": "image", "label": "模型发布时间线"}),
        json!({"kind": "archive", "label": "5 张图片 ZIP"}),
        json!({"kind": "archive", "label": "5 张图片 ZIP"}),
    ];

    let text = interaction::append_dom_artifact_notice("打包下载：", &artifacts);

    assert!(text.contains("ChatGPT Web 页面还提供了这些可点击下载项"));
    assert!(text.contains("- 图片: 大模型竞争格局"));
    assert!(text.contains("- 图片: 模型发布时间线"));
    assert!(text.contains("- 压缩包: 5 张图片 ZIP"));
    assert_eq!(text.matches("5 张图片 ZIP").count(), 1);
}

#[test]
fn dom_extraction_source_scans_chatgpt_behavior_button_artifacts() {
    let source = include_str!("interaction.rs");
    let adapter_source = include_str!("../chatgpt_web.rs");

    assert!(source.contains("button.behavior-btn"));
    assert!(source.contains("chatgpt_behavior_button"));
    assert!(source.contains("artifactCount"));
    assert!(source.contains("append_dom_artifact_notice"));
    assert!(source.contains("return 'image';"));
    assert!(adapter_source.contains("download_behavior_artifacts_for_conversation"));
    assert!(adapter_source.contains("downloadedArtifacts"));
    assert!(adapter_source.contains("behavior_artifacts.json"));
    assert!(adapter_source.contains("![{label}]({path})"));
    assert!(include_str!("artifacts.rs").contains("chatgpt_behavior_button_image_download"));
    assert!(include_str!("artifacts.rs").contains("wait_for_dialog_image_url"));
}

#[test]
fn chatgpt_web_behavior_artifacts_merge_final_and_summary_without_duplicates() {
    let waited = WaitedFinal {
        final_message: json!({
            "text": "done",
            "artifacts": [
                {"kind": "image", "label": "大模型竞争格局"},
                {"kind": "archive", "label": "5 张图片 ZIP"}
            ]
        }),
        summary: json!({
            "artifacts": [
                {"kind": "archive", "label": "5 张图片 ZIP"},
                {"kind": "image", "label": "模型发布时间线"}
            ]
        }),
        had_429_or_fallback: false,
        all_texts: vec!["done".to_string()],
    };

    let artifacts = chatgpt_behavior_artifacts(&waited);

    assert_eq!(artifacts.len(), 3);
    assert_eq!(artifacts[0]["label"], "大模型竞争格局");
    assert_eq!(artifacts[1]["label"], "5 张图片 ZIP");
    assert_eq!(artifacts[2]["label"], "模型发布时间线");
}

#[test]
fn chatgpt_web_downloaded_artifact_links_force_single_response_content() {
    let mut response = "打包下载：".to_string();

    append_downloaded_behavior_artifact_links(
        &mut response,
        &[
            json!({
                "kind": "image",
                "label": "大模型竞争格局",
                "path": "/tmp/chatgpt/ai_infographic_01_overview.png"
            }),
            json!({
                "kind": "archive",
                "label": "5 张图片 ZIP",
                "path": "/tmp/chatgpt/ai_infographics_5_images.zip"
            }),
            json!({
                "kind": "download_failures",
                "failures": [
                    {"kind": "image", "label": "模型发布时间线", "error": "dialog image URL not found"}
                ]
            }),
        ],
    );
    let responses =
        chatgpt_web_responses_for_delivery(&response, &["打包下载：".to_string()], true);

    assert!(response.contains("![大模型竞争格局](/tmp/chatgpt/ai_infographic_01_overview.png)"));
    assert!(response.contains("已自动下载 ChatGPT Web 附件"));
    assert!(response.contains("[5 张图片 ZIP](/tmp/chatgpt/ai_infographics_5_images.zip)"));
    assert!(response.contains("部分 ChatGPT Web 附件未能自动下载"));
    assert!(response.contains("- 图片: 模型发布时间线 (dialog image URL not found)"));
    assert_eq!(responses, vec![response]);
}

#[test]
fn chatgpt_web_downloaded_artifact_links_reports_failures_without_attachments() {
    let mut response = String::new();
    append_downloaded_behavior_artifact_links(
        &mut response,
        &[json!({
            "kind": "download_failures",
            "failures": [
                {"kind": "image", "label": "大模型竞争格局", "error": "dialog image URL not found"}
            ]
        })],
    );

    assert!(response.starts_with("部分 ChatGPT Web 附件未能自动下载："));
    assert!(response.contains("- 图片: 大模型竞争格局 (dialog image URL not found)"));
}

#[test]
fn chatgpt_web_behavior_download_filename_is_safe() {
    assert_eq!(
        super::artifacts::sanitize_behavior_download_filename("../a:b*c?.zip"),
        "-a-b-c-.zip"
    );
    assert_eq!(
        super::artifacts::sanitize_behavior_download_filename("   "),
        "chatgpt-web-artifact"
    );
}

#[test]
fn dom_output_state_waits_for_generation_controls_to_finish() {
    assert_eq!(
        interaction::dom_output_in_progress_reason(&json!({
            "text": "ChatGPT 说：正在创建图片",
            "imageCount": 0,
            "pendingOutputStatusText": true,
            "composerVisible": false,
            "sendButtonVisible": false
        })),
        Some("pending_output_status_text")
    );
    assert_eq!(
        interaction::dom_output_in_progress_reason(&json!({
            "text": "ChatGPT 说：正在打草稿",
            "imageCount": 0,
            "pendingOutputStatusText": true,
            "composerVisible": true,
            "sendButtonVisible": true
        })),
        Some("pending_output_status_text")
    );
    assert_eq!(
        interaction::dom_output_in_progress_reason(&json!({
            "text": "ChatGPT 说：",
            "imageCount": 0,
            "pendingOutputStatusText": true,
            "composerVisible": true,
            "sendButtonVisible": true
        })),
        Some("pending_output_status_text")
    );
    assert_eq!(
        interaction::dom_output_in_progress_reason(&json!({
            "text": "最后微调一下...",
            "imageCount": 0,
            "pendingOutputStatusText": true,
            "composerVisible": true,
            "sendButtonVisible": true
        })),
        Some("pending_output_status_text")
    );
    assert_eq!(
        interaction::dom_output_in_progress_reason(&json!({
            "text": "OK",
            "imageCount": 0,
            "pendingOutputStatusText": false,
            "composerVisible": true,
            "composerDisabled": false,
            "composerTextLength": 0,
            "sendButtonVisible": true,
            "sendButtonDisabled": true
        })),
        None
    );
    assert_eq!(
        interaction::dom_output_in_progress_reason(&json!({
            "text": "OK",
            "imageCount": 0,
            "pendingOutputStatusText": false,
            "composerVisible": true,
            "composerDisabled": false,
            "composerTextLength": 0,
            "sendButtonVisible": true,
            "sendButtonDisabled": false
        })),
        None
    );
    assert_eq!(
        interaction::dom_output_in_progress_reason(&json!({
            "text": "OK",
            "imageCount": 0,
            "pendingOutputStatusText": false,
            "composerVisible": true,
            "composerDisabled": false,
            "composerTextLength": 0,
            "sendButtonVisible": false
        })),
        None
    );
    assert_eq!(
        interaction::dom_output_in_progress_reason(&json!({
            "text": "OK",
            "imageCount": 0,
            "pendingOutputStatusText": false,
            "composerVisible": true,
            "composerDisabled": false,
            "composerTextLength": 2,
            "sendButtonVisible": false
        })),
        Some("send_button_not_ready")
    );
    assert_eq!(
        interaction::dom_output_in_progress_reason(&json!({
            "text": "partial",
            "imageCount": 0,
            "stopButtonVisible": true,
            "composerVisible": true,
            "sendButtonVisible": false
        })),
        Some("stop_button_visible")
    );
}

#[test]
fn chatgpt_web_delivery_uses_final_response_when_images_are_appended() {
    let response = "已生成图片，正在发送原图。\n\n![ChatGPT 生成图片 1](/tmp/cat.png)";
    let all_texts = vec!["已生成图片，正在发送原图。".to_string()];

    let responses = chatgpt_web_responses_for_delivery(response, &all_texts, true);

    assert_eq!(responses, vec![response.to_string()]);
}

#[test]
fn chatgpt_web_delivery_preserves_multi_text_without_image_side_effects() {
    let response = "thinking\n\nanswer";
    let all_texts = vec!["thinking".to_string(), "answer".to_string()];

    let responses = chatgpt_web_responses_for_delivery(response, &all_texts, false);

    assert_eq!(responses, all_texts);
}

#[test]
fn chatgpt_web_delivery_preserves_natural_process_batches() {
    let response = "plan 1\n\nplan 2\n\nplan 3\n\nfinal";
    let all_texts = vec![
        "plan 1".to_string(),
        "plan 2".to_string(),
        "plan 3".to_string(),
        "final".to_string(),
    ];

    let responses = chatgpt_web_responses_for_delivery(response, &all_texts, false);

    assert_eq!(responses, all_texts);
}

#[test]
fn chatgpt_web_response_events_split_thinking_tools_and_final() {
    let raw = json!({
        "conversationId": "c1",
        "toolCalls": [
            {
                "name": "Search the web",
                "text": "Search the web: 今日 AI 新闻",
                "status": "finished"
            }
        ]
    });
    let responses = vec![
        "先梳理检索范围".to_string(),
        "已筛完一轮".to_string(),
        "最终答案".to_string(),
    ];

    let events = chatgpt_web_response_events("最终答案", &raw, &responses);

    assert_eq!(events.len(), 4);
    assert_eq!(
        events[0].event_type,
        ExternalCliProgressEventType::AssistantDelta
    );
    assert_eq!(events[0].raw["phase"], "thinking");
    assert_eq!(events[0].raw["batchIndex"], 0);
    assert_eq!(
        events[1].event_type,
        ExternalCliProgressEventType::AssistantDelta
    );
    assert_eq!(events[1].raw["phase"], "thinking");
    assert_eq!(events[1].raw["batchIndex"], 1);
    assert_eq!(
        events[2].event_type,
        ExternalCliProgressEventType::ToolFinished
    );
    assert_eq!(events[2].raw["phase"], "tool");
    assert_eq!(events[2].content, "Search the web: 今日 AI 新闻");
    assert_eq!(
        events[3].event_type,
        ExternalCliProgressEventType::AssistantFinal
    );
    assert_eq!(events[3].raw["phase"], "final");
    assert_eq!(events[3].raw["batchIndex"], 2);
    assert_eq!(events[3].content, "最终答案");
}

#[test]
fn requested_image_count_hint_reads_chinese_and_digit_counts() {
    assert_eq!(
        requested_image_count_hint("提供不同角度、不同衣服款式的10张照片"),
        Some(10)
    );
    assert_eq!(requested_image_count_hint("生成十张图片"), Some(10));
    assert_eq!(requested_image_count_hint("generate 4 photos"), Some(4));
    assert_eq!(requested_image_count_hint("只要一张图"), Some(1));
    assert_eq!(requested_image_count_hint("整理成图片"), None);
}

#[test]
fn stale_conversation_wrong_page_retries_as_new_conversation() {
    assert!(should_retry_as_new_conversation(
        "browser_wrong_page: expected_conversation_id=Some(\"old\") actual_conversation_id=None page_kind=new_conversation url=https://chatgpt.com/"
    ));
    assert!(!should_retry_as_new_conversation(
        "browser_wrong_page: expected_conversation_id=Some(\"old\") actual_conversation_id=Some(\"other\") page_kind=conversation url=https://chatgpt.com/c/other"
    ));
    // 429 / fatal error pages should also trigger fallback to new conversation.
    assert!(should_retry_as_new_conversation(
        "browser_wrong_page: expected_conversation_id=Some(\"old\") actual_conversation_id=None page_kind=unknown url=https://chatgpt.com/c/old body_error=Too Many Requests"
    ));
    assert!(should_retry_as_new_conversation(
        "browser_wrong_page: expected_conversation_id=Some(\"old\") actual_conversation_id=None page_kind=unknown url=https://chatgpt.com/c/old body_error=Something went wrong"
    ));
}

#[test]
fn auth_flow_pages_are_not_retryable_conversation_navigation() {
    let login_page = json!({
        "readyState": "complete",
        "url": "https://chatgpt.com/auth/login",
        "title": "开始使用 | ChatGPT",
        "bodyText": "登录即可开始聊天 使用 Google 账号继续 使用 Apple 账号继续",
        "conversationId": null,
        "pageKind": "new_conversation",
        "visibleComposerCount": 0
    });
    assert!(page_state_is_auth_flow_page(&login_page));
    let error = target_page_error(&login_page, Some("6a0cac55"));
    assert!(error.starts_with("auth_required:"));
    assert!(!should_retry_as_new_conversation(&error));
    assert!(target_page_is_terminal_mismatch(
        &login_page,
        Some("6a0cac55")
    ));

    let google_oauth = json!({
        "readyState": "complete",
        "url": "https://accounts.google.com/o/oauth2/v2/auth?client_id=chatgpt",
        "title": "Sign in - Google Accounts",
        "bodyText": "Sign in Continue with Google",
        "conversationId": null,
        "pageKind": "new_conversation",
        "visibleComposerCount": 0
    });
    assert!(page_state_is_auth_flow_page(&google_oauth));
    let error = target_page_error(&google_oauth, Some("6a0cac55"));
    assert!(error.starts_with("auth_required:"));
    assert!(!should_retry_as_new_conversation(&error));
}

#[test]
fn handoff_heartbeat_reports_browser_and_cdp_failures() {
    assert_eq!(
        handoff_heartbeat_error(true, false).as_deref(),
        Some("browser_unavailable: ChatGPT Web browser exited while waiting for handoff")
    );
    assert_eq!(
        handoff_heartbeat_error(false, true).as_deref(),
        Some("browser_unavailable: ChatGPT Web CDP connection closed while waiting for handoff")
    );
    assert!(handoff_heartbeat_error(false, false).is_none());
}

#[test]
fn handoff_page_heartbeat_error_is_non_recoverable() {
    let error = handoff_page_heartbeat_error("CDP command timed out: Runtime.evaluate");
    assert!(error.starts_with("browser_unavailable:"));
    assert!(error.contains("Runtime.evaluate"));
}

#[test]
fn account_check_requires_usable_user_account() {
    let json = json!({
        "accounts": {
            "default": {
                "can_access_with_session": true,
                "account": {
                    "account_id": "acc-1",
                    "account_user_id": "user-1",
                    "is_deactivated": false
                }
            }
        }
    });
    assert!(native::account_check_logged_in(&json));
}

#[test]
fn account_check_rejects_guest_or_deactivated_accounts() {
    let json = json!({
        "accounts": {
            "default": {
                "can_access_with_session": true,
                "account": {
                    "account_id": "acc-1",
                    "account_user_id": "ua-1",
                    "is_deactivated": false
                }
            },
            "disabled": {
                "can_access_with_session": true,
                "account": {
                    "account_id": "acc-2",
                    "account_user_id": "user-2",
                    "is_deactivated": true
                }
            }
        }
    });
    assert!(!native::account_check_logged_in(&json));
}

#[tokio::test]
async fn auth_status_accepts_recent_browser_account_check_when_native_probe_fails() {
    let mut headers = BTreeMap::new();
    headers.insert(
        "authorization".to_string(),
        "Bearer not-a-jwt-but-present-for-this-status-test".to_string(),
    );
    let state = AuthState {
        captured_at: iso_now(),
        base_url: DEFAULT_BASE_URL.to_string(),
        user_agent: "Mozilla/5.0".to_string(),
        captured_auth_headers: headers,
        captured_auth_identity: AuthorizationIdentity {
            has_bearer_token: true,
            has_profile_email: true,
            profile_email_verified: Some(true),
            has_user_identity: true,
            has_account_identity: true,
            complete: true,
            expires_at: None,
        },
        captured_account_check: Some(BrowserAccountCheckProof {
            captured_at: iso_now(),
            status: 200,
            logged_in: true,
        }),
        cookies: vec![BrowserCookie {
            name: "__Secure-next-auth.session-token".to_string(),
            value: "token".to_string(),
            domain: ".chatgpt.com".to_string(),
            path: Some("/".to_string()),
            secure: Some(true),
            http_only: Some(true),
            same_site: None,
            expires: None,
        }],
    };
    let temp = tempfile::tempdir().expect("tempdir");
    let config = RuntimeConfig {
        browser: BrowserConfig::default(),
        chatgpt: ChatGptConfig {
            base_url: "http://[::1".to_string(),
            poll_interval_ms: 1,
            timeout_secs: 1,
            session_consistency: "strong".to_string(),
            rate_limit_retry_secs: 180,
            rate_limit_max_retries: 5,
        },
        profile_dir: temp.path().join("profile"),
        state_path: temp.path().join("auth_state.json"),
        sessions_path: temp.path().join("sessions.json"),
        attachments_dir: temp.path().join("attachments"),
    };

    let status = auth_status_from_state(&config, &state)
        .await
        .expect("auth status");

    assert!(status.logged_in);
    assert!(status.account_check_ok);
    assert!(status.identity_complete);
    assert_eq!(status.account_status, Some(200));
    assert!(status
        .message
        .as_deref()
        .unwrap_or_default()
        .contains("using captured browser accounts/check proof"));
}

#[tokio::test]
async fn auth_status_accepts_browser_account_check_when_native_probe_is_forbidden() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind mock accounts/check");
    let port = listener.local_addr().expect("local addr").port();
    tokio::spawn(async move {
        if let Ok((mut stream, _)) = listener.accept().await {
            let mut buf = [0_u8; 1024];
            let _ = tokio::io::AsyncReadExt::read(&mut stream, &mut buf).await;
            let response = b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n";
            let _ = tokio::io::AsyncWriteExt::write_all(&mut stream, response).await;
        }
    });

    let mut headers = BTreeMap::new();
    headers.insert("authorization".to_string(), "Bearer captured".to_string());
    let state = AuthState {
        captured_at: iso_now(),
        base_url: DEFAULT_BASE_URL.to_string(),
        user_agent: "Mozilla/5.0".to_string(),
        captured_auth_headers: headers,
        captured_auth_identity: AuthorizationIdentity {
            has_bearer_token: true,
            has_profile_email: true,
            profile_email_verified: Some(true),
            has_user_identity: true,
            has_account_identity: true,
            complete: true,
            expires_at: None,
        },
        captured_account_check: Some(BrowserAccountCheckProof {
            captured_at: iso_now(),
            status: 200,
            logged_in: true,
        }),
        cookies: Vec::new(),
    };
    let temp = tempfile::tempdir().expect("tempdir");
    let config = RuntimeConfig {
        browser: BrowserConfig::default(),
        chatgpt: ChatGptConfig {
            base_url: format!("http://127.0.0.1:{port}"),
            poll_interval_ms: 1,
            timeout_secs: 1,
            session_consistency: "strong".to_string(),
            rate_limit_retry_secs: 180,
            rate_limit_max_retries: 5,
        },
        profile_dir: temp.path().join("profile"),
        state_path: temp.path().join("auth_state.json"),
        sessions_path: temp.path().join("sessions.json"),
        attachments_dir: temp.path().join("attachments"),
    };

    let status = auth_status_from_state(&config, &state)
        .await
        .expect("auth status");

    assert!(status.logged_in);
    assert!(status.account_check_ok);
    assert_eq!(status.account_status, Some(403));
    assert!(status
        .message
        .as_deref()
        .unwrap_or_default()
        .contains("using captured browser accounts/check proof"));
}

#[tokio::test]
async fn auth_status_rejects_expired_authorization_identity() {
    let mut headers = BTreeMap::new();
    headers.insert("authorization".to_string(), "Bearer captured".to_string());
    let state = AuthState {
        captured_at: iso_now(),
        base_url: DEFAULT_BASE_URL.to_string(),
        user_agent: "Mozilla/5.0".to_string(),
        captured_auth_headers: headers,
        captured_auth_identity: AuthorizationIdentity {
            has_bearer_token: true,
            has_profile_email: true,
            profile_email_verified: Some(true),
            has_user_identity: true,
            has_account_identity: true,
            complete: true,
            expires_at: Some((chrono::Utc::now() - chrono::Duration::minutes(5)).to_rfc3339()),
        },
        captured_account_check: Some(BrowserAccountCheckProof {
            captured_at: iso_now(),
            status: 200,
            logged_in: true,
        }),
        cookies: Vec::new(),
    };
    let temp = tempfile::tempdir().expect("tempdir");
    let config = RuntimeConfig {
        browser: BrowserConfig::default(),
        chatgpt: ChatGptConfig {
            base_url: "http://[::1".to_string(),
            poll_interval_ms: 1,
            timeout_secs: 1,
            session_consistency: "strong".to_string(),
            rate_limit_retry_secs: 180,
            rate_limit_max_retries: 5,
        },
        profile_dir: temp.path().join("profile"),
        state_path: temp.path().join("auth_state.json"),
        sessions_path: temp.path().join("sessions.json"),
        attachments_dir: temp.path().join("attachments"),
    };

    let status = auth_status_from_state(&config, &state)
        .await
        .expect("auth status");

    assert!(!status.logged_in);
    assert!(!status.identity_complete);
    assert_eq!(status.state, "auth_required");
    assert!(status
        .message
        .as_deref()
        .unwrap_or_default()
        .contains("authorization token expired"));
}

#[tokio::test]
async fn auth_status_rejects_stale_browser_account_check_when_native_forbidden() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind mock accounts/check");
    let port = listener.local_addr().expect("local addr").port();
    tokio::spawn(async move {
        if let Ok((mut stream, _)) = listener.accept().await {
            let mut buf = [0_u8; 1024];
            let _ = tokio::io::AsyncReadExt::read(&mut stream, &mut buf).await;
            let response = b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n";
            let _ = tokio::io::AsyncWriteExt::write_all(&mut stream, response).await;
        }
    });

    let mut headers = BTreeMap::new();
    headers.insert("authorization".to_string(), "Bearer captured".to_string());
    let stale_captured_at = (chrono::Utc::now() - chrono::Duration::minutes(30)).to_rfc3339();
    let state = AuthState {
        captured_at: iso_now(),
        base_url: DEFAULT_BASE_URL.to_string(),
        user_agent: "Mozilla/5.0".to_string(),
        captured_auth_headers: headers,
        captured_auth_identity: AuthorizationIdentity {
            has_bearer_token: true,
            has_profile_email: true,
            profile_email_verified: Some(true),
            has_user_identity: true,
            has_account_identity: true,
            complete: true,
            expires_at: None,
        },
        captured_account_check: Some(BrowserAccountCheckProof {
            captured_at: stale_captured_at,
            status: 200,
            logged_in: true,
        }),
        cookies: Vec::new(),
    };
    let temp = tempfile::tempdir().expect("tempdir");
    let config = RuntimeConfig {
        browser: BrowserConfig::default(),
        chatgpt: ChatGptConfig {
            base_url: format!("http://127.0.0.1:{port}"),
            poll_interval_ms: 1,
            timeout_secs: 1,
            session_consistency: "strong".to_string(),
            rate_limit_retry_secs: 180,
            rate_limit_max_retries: 5,
        },
        profile_dir: temp.path().join("profile"),
        state_path: temp.path().join("auth_state.json"),
        sessions_path: temp.path().join("sessions.json"),
        attachments_dir: temp.path().join("attachments"),
    };

    let status = auth_status_from_state(&config, &state)
        .await
        .expect("auth status");

    assert!(!status.logged_in);
    assert!(!status.account_check_ok);
    assert_eq!(status.account_status, Some(403));
    assert_eq!(status.state, "auth_required");
    assert!(status
        .message
        .as_deref()
        .unwrap_or_default()
        .contains("captured browser accounts/check proof is stale"));
}

#[test]
fn browser_account_check_proof_rejects_far_future_timestamp() {
    let proof = BrowserAccountCheckProof {
        captured_at: (chrono::Utc::now() + chrono::Duration::minutes(30)).to_rfc3339(),
        status: 200,
        logged_in: true,
    };

    assert!(!browser_account_check_proof_is_fresh(&proof));
}

#[test]
fn send_sse_parser_extracts_handoff_identifiers() {
    let text = "data: {\"type\":\"stream_handoff\",\"conversation_id\":\"c1\",\"turn_exchange_id\":\"t1\"}\n\n";
    let parsed = parse_send_sse(text);
    assert_eq!(parsed.conversation_id.as_deref(), Some("c1"));
    assert_eq!(parsed.turn_exchange_id.as_deref(), Some("t1"));
    assert_eq!(parsed.event_types, vec!["stream_handoff"]);
}

#[test]
fn post_capture_is_handoff_submission_evidence() {
    let recovered_without_post = vec![
        "handoff_recovered_after_sse_interrupt".to_string(),
        "browser_ui".to_string(),
    ];
    assert!(!has_handoff_submission_evidence(&recovered_without_post));

    let recovered_with_post = vec![
        "handoff_recovered_after_sse_interrupt".to_string(),
        "browser_post_captured".to_string(),
        "browser_ui".to_string(),
    ];
    assert!(has_handoff_submission_evidence(&recovered_with_post));
}

#[test]
fn extracts_conversation_id_from_chatgpt_url() {
    assert_eq!(
        extract_conversation_id_from_url("https://chatgpt.com/c/abc-123?model=x").as_deref(),
        Some("abc-123")
    );
    assert_eq!(
        extract_conversation_id_from_url("https://chatgpt.com/"),
        None
    );
}

#[test]
fn target_page_state_distinguishes_append_and_new_conversation_modes() {
    let existing = json!({
        "readyState": "complete",
        "conversationId": "c1",
        "pageKind": "conversation",
        "visibleComposerCount": 1
    });
    assert!(target_page_matches(&existing, Some("c1"), true));
    assert!(!target_page_matches(&existing, Some("c2"), true));
    assert!(!target_page_matches(&existing, None, true));
    assert!(target_page_is_terminal_mismatch(&existing, Some("c2")));
    assert!(target_page_is_terminal_mismatch(&existing, None));

    let new_page = json!({
        "readyState": "complete",
        "conversationId": null,
        "pageKind": "new_conversation",
        "visibleComposerCount": 1
    });
    assert!(target_page_matches(&new_page, None, true));
    assert!(!target_page_matches(&new_page, Some("c1"), true));
    assert!(target_page_is_terminal_mismatch(&new_page, Some("c1")));
    assert!(!target_page_is_terminal_mismatch(&new_page, None));
}

#[test]
fn target_page_state_requires_visible_composer_when_requested() {
    let loading = json!({
        "readyState": "complete",
        "conversationId": "c1",
        "pageKind": "conversation",
        "visibleComposerCount": 0
    });
    assert!(target_page_matches(&loading, Some("c1"), false));
    assert!(!target_page_matches(&loading, Some("c1"), true));
}

#[test]
fn detects_submitted_user_message_after_timestamp() {
    let conversation = json!({
        "mapping": {
            "node": {
                "message": {
                    "author": {"role": "user"},
                    "create_time": 10.0,
                    "content": {"parts": ["你是谁"]}
                }
            }
        }
    });
    assert!(conversation_has_user_message_after(
        &conversation,
        "你是谁",
        9.0
    ));
    assert!(!conversation_has_user_message_after(
        &conversation,
        "你是谁",
        11.0
    ));
}

#[test]
fn treats_initial_conversation_read_404_as_transient() {
    assert!(is_transient_conversation_read_error(
        "auth_or_challenge: ChatGPT native HTTP 404 for /backend-api/conversation/c1"
    ));
    assert!(is_transient_conversation_read_error(
        "auth_or_challenge: ChatGPT native HTTP 403 for /backend-api/conversation/c1"
    ));
    assert!(!is_transient_conversation_read_error(
        "auth_or_challenge: ChatGPT native HTTP 401 for /backend-api/conversation/c1"
    ));
}

#[test]
fn summarize_detail_includes_ordered_messages() {
    let conversation = json!({
        "conversation_id": "c1",
        "current_node": "a1",
        "mapping": {
            "assistant": {
                "message": {
                    "id": "a1",
                    "author": {"role": "assistant"},
                    "create_time": 2.0,
                    "status": "finished_successfully",
                    "end_turn": true,
                    "content": {"parts": ["你好"]}
                }
            },
            "user": {
                "message": {
                    "id": "u1",
                    "author": {"role": "user"},
                    "create_time": 1.0,
                    "content": {"parts": ["你好"]}
                }
            }
        }
    });
    let summary = summarize_conversation_detail(&conversation);
    let messages = summary.get("messages").and_then(Value::as_array).unwrap();
    assert_eq!(messages.len(), 2);
    assert_eq!(
        messages[0].get("role").and_then(Value::as_str),
        Some("user")
    );
    assert_eq!(
        messages[1].get("role").and_then(Value::as_str),
        Some("assistant")
    );
}

fn make_run_request_with_params(params: Value, session_key: Option<&str>) -> ExternalCliRunRequest {
    ExternalCliRunRequest {
        message: "hello".to_string(),
        images: Vec::new(),
        operation: "ask".to_string(),
        params,
        provider_id: None,
        runner_id: None,
        session_key: session_key.map(ToString::to_string),
        runtime: "external_cli".to_string(),
        adapter: ADAPTER_ID.to_string(),
        work_dir: None,
        instructions: None,
        adapter_config: ExternalCliAdapterConfig::default(),
        allow_work_dirs: Vec::new(),
        inject_bifrost_tools: false,
        skill_paths: Vec::new(),
    }
}

#[tokio::test]
async fn requested_or_session_conversation_prefers_param_over_session_map() {
    let temp = tempfile::tempdir().expect("tempdir");
    let config = RuntimeConfig {
        browser: BrowserConfig::default(),
        chatgpt: ChatGptConfig::default(),
        profile_dir: temp.path().join("profile"),
        state_path: temp.path().join("auth_state.json"),
        sessions_path: temp.path().join("sessions.json"),
        attachments_dir: temp.path().join("attachments"),
    };

    // Pre-populate session map with a different conversation id
    tokio::fs::write(
        &config.sessions_path,
        json!({ "sk1": "old-conv" }).to_string(),
    )
    .await
    .expect("write sessions");

    let request = make_run_request_with_params(json!({ "conversationId": " c-new " }), Some("sk1"));
    let result = interaction::requested_or_session_conversation(&request, &config)
        .await
        .expect("ok");
    assert_eq!(result.as_deref(), Some("c-new"));
}

#[tokio::test]
async fn requested_or_session_conversation_reads_session_map_when_param_missing() {
    let temp = tempfile::tempdir().expect("tempdir");
    let config = RuntimeConfig {
        browser: BrowserConfig::default(),
        chatgpt: ChatGptConfig::default(),
        profile_dir: temp.path().join("profile"),
        state_path: temp.path().join("auth_state.json"),
        sessions_path: temp.path().join("sessions.json"),
        attachments_dir: temp.path().join("attachments"),
    };

    tokio::fs::write(
        &config.sessions_path,
        json!({ "sk1": "conv-from-session" }).to_string(),
    )
    .await
    .expect("write sessions");

    let request = make_run_request_with_params(Value::Null, Some("sk1"));
    let result = interaction::requested_or_session_conversation(&request, &config)
        .await
        .expect("ok");
    assert_eq!(result.as_deref(), Some("conv-from-session"));

    let request2 = make_run_request_with_params(Value::Null, None);
    let result2 = interaction::requested_or_session_conversation(&request2, &config)
        .await
        .expect("ok");
    assert_eq!(result2, None);
}

#[tokio::test]
async fn save_session_conversation_persists_and_updates_mapping() {
    let temp = tempfile::tempdir().expect("tempdir");
    let config = RuntimeConfig {
        browser: BrowserConfig::default(),
        chatgpt: ChatGptConfig::default(),
        profile_dir: temp.path().join("profile"),
        state_path: temp.path().join("auth_state.json"),
        sessions_path: temp.path().join("sessions.json"),
        attachments_dir: temp.path().join("attachments"),
    };

    interaction::save_session_conversation(&config, "sk1", "c1")
        .await
        .expect("save sk1");
    interaction::save_session_conversation(&config, "sk2", "c2")
        .await
        .expect("save sk2");
    interaction::save_session_conversation(&config, "sk1", "c1-new")
        .await
        .expect("update sk1");

    let content = tokio::fs::read_to_string(&config.sessions_path)
        .await
        .expect("read map");
    let map: serde_json::Value = serde_json::from_str(&content).expect("parse map");
    assert_eq!(map.get("sk1").and_then(Value::as_str), Some("c1-new"));
    assert_eq!(map.get("sk2").and_then(Value::as_str), Some("c2"));
}

#[test]
fn try_waited_final_from_sse_builds_waited_final_for_finished_assistant() {
    let sse_detail = json!({
        "mapping": {
            "user": {
                "message": {
                    "id": "u1",
                    "author": {"role": "user"},
                    "create_time": 0.0,
                    "content": {"parts": ["hi"]}
                }
            },
            "a1": {
                "message": {
                    "id": "a1",
                    "author": {"role": "assistant"},
                    "create_time": 1.0,
                    "status": "finished_successfully",
                    "end_turn": true,
                    "content": {"parts": ["thinking"]}
                }
            },
            "a2": {
                "message": {
                    "id": "a2",
                    "author": {"role": "assistant"},
                    "create_time": 2.0,
                    "status": "finished_successfully",
                    "end_turn": true,
                    "content": {"parts": ["final answer"]}
                }
            }
        }
    });

    let waited = interaction::try_waited_final_from_sse(&sse_detail, Some(0.0))
        .expect("waited final from sse");
    assert_eq!(
        waited.final_message.get("text").and_then(Value::as_str),
        Some("thinking\n\nfinal answer")
    );
    assert_eq!(
        waited.all_texts,
        vec!["thinking".to_string(), "final answer".to_string()]
    );
    assert!(!waited.had_429_or_fallback);
}

#[test]
fn browser_fetch_headers_strips_cookie_and_user_agent() {
    let mut headers = BTreeMap::new();
    headers.insert("authorization".to_string(), "Bearer captured".to_string());
    headers.insert("cookie".to_string(), "session=1".to_string());
    headers.insert("user-agent".to_string(), "UA".to_string());
    let state = AuthState {
        captured_at: iso_now(),
        base_url: DEFAULT_BASE_URL.to_string(),
        user_agent: "UA".to_string(),
        captured_auth_headers: headers,
        captured_auth_identity: AuthorizationIdentity::default(),
        captured_account_check: None,
        cookies: Vec::new(),
    };

    let browser_headers = browser_fetch_headers(&state);
    assert_eq!(
        browser_headers.get("authorization").map(String::as_str),
        Some("Bearer captured")
    );
    assert!(!browser_headers.contains_key("cookie"));
    assert!(!browser_headers.contains_key("user-agent"));
}
