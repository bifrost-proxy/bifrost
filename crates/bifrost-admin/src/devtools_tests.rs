use super::*;

fn input(url: &str) -> RegisterPageInput {
    RegisterPageInput {
        url: url.to_string(),
        origin: "http://example.test".to_string(),
        traffic_id: "101".to_string(),
        mode: DevtoolsMode::Read,
        matched_rule: Some(MatchedDevtoolsRule {
            pattern: "example.test".to_string(),
            raw: Some("example.test devtools://mode=read".to_string()),
            line: Some(1),
            evaluate_allowlist: Vec::new(),
        }),
    }
}

fn network_input(url: &str, client_req_id: Option<&str>, status: u16) -> NetworkEventInput {
    NetworkEventInput {
        url: url.to_string(),
        method: Some("GET".to_string()),
        status: Some(status),
        resource_type: Some("Fetch".to_string()),
        client_req_id: client_req_id.map(str::to_string),
        query_params: Vec::new(),
        request_headers: Vec::new(),
        response_headers: Vec::new(),
        from_cache: Some(false),
    }
}

#[test]
fn test_proxied_page_registry_records_document_request_with_devtools_rule() {
    let broker = BrowserDebugBroker::new();
    let (page_id, token) =
        broker.register_page_candidate(input("http://example.test/devtools/basic.html"));

    let pages = broker.list_pages(true);
    assert_eq!(pages.len(), 1);
    assert_eq!(pages[0].page_id, page_id);
    assert_eq!(pages[0].state, DebugPageState::Candidate);
    assert_eq!(pages[0].traffic_ids, vec!["101".to_string()]);
    assert!(!token.is_empty());
    assert!(
        broker.list_debuggable_pages(true).is_empty(),
        "unconnected candidates must not be shown as selectable DevTools pages"
    );
}

#[test]
fn test_page_bridge_hello_marks_page_discoverable() {
    let broker = BrowserDebugBroker::new();
    let (page_id, token) =
        broker.register_page_candidate(input("http://example.test/devtools/basic.html"));

    broker
        .bridge_hello(
            &page_id,
            BridgeHelloPayload {
                token,
                scope: None,
                tab_id: Some("tab-a".to_string()),
                title: Some("Fixture".to_string()),
                url: None,
                user_agent: Some("Mobile Safari".to_string()),
                dom_snapshot: Some("<html></html>".to_string()),
                dom_tree: None,
                storage: None,
                console: Vec::new(),
                network: Vec::new(),
            },
        )
        .expect("bridge hello");

    let page = broker.list_pages(true).remove(0);
    assert_eq!(page.title.as_deref(), Some("Fixture"));
    assert_eq!(page.state, DebugPageState::Discoverable);
    assert_eq!(page.user_agent.as_deref(), Some("Mobile Safari"));
}

#[test]
fn test_page_bridge_close_hides_stale_page_from_debuggable_list() {
    let broker = BrowserDebugBroker::new();
    let (page_id, token) =
        broker.register_page_candidate(input("http://example.test/devtools/basic.html"));
    broker
        .bridge_hello(
            &page_id,
            BridgeHelloPayload {
                token: token.clone(),
                scope: None,
                tab_id: Some("tab-a".to_string()),
                title: Some("Fixture".to_string()),
                url: None,
                user_agent: None,
                dom_snapshot: None,
                dom_tree: None,
                storage: None,
                console: Vec::new(),
                network: Vec::new(),
            },
        )
        .expect("bridge hello");

    broker
        .bridge_close(&page_id, BridgeClosePayload { token })
        .expect("bridge close");

    let stale_page = broker.list_pages(true).remove(0);
    assert_eq!(stale_page.state, DebugPageState::Stale);
    assert!(
        broker.list_debuggable_pages(true).is_empty(),
        "stale pages must not remain selectable"
    );
}

#[test]
fn test_page_bridge_reload_migrates_open_session_to_new_page_id() {
    let broker = BrowserDebugBroker::new();
    let (old_page_id, old_token) =
        broker.register_page_candidate(input("http://example.test/devtools/basic.html"));
    broker
        .bridge_hello(
            &old_page_id,
            BridgeHelloPayload {
                token: old_token.clone(),
                scope: None,
                tab_id: Some("tab-a".to_string()),
                title: Some("Before".to_string()),
                url: None,
                user_agent: None,
                dom_snapshot: Some("<html><body>before</body></html>".to_string()),
                dom_tree: None,
                storage: None,
                console: Vec::new(),
                network: Vec::new(),
            },
        )
        .expect("old bridge hello");
    let session = broker.open_session(&old_page_id).expect("session");
    broker
        .bridge_close(&old_page_id, BridgeClosePayload { token: old_token })
        .expect("old bridge close");

    let (new_page_id, new_token) =
        broker.register_page_candidate(input("http://example.test/devtools/basic.html"));
    broker
        .bridge_hello(
            &new_page_id,
            BridgeHelloPayload {
                token: new_token,
                scope: None,
                tab_id: Some("tab-a".to_string()),
                title: Some("After".to_string()),
                url: Some("http://example.test/devtools/basic.html?after=1".to_string()),
                user_agent: None,
                dom_snapshot: Some("<html><body>after</body></html>".to_string()),
                dom_tree: None,
                storage: None,
                console: Vec::new(),
                network: Vec::new(),
            },
        )
        .expect("new bridge hello");

    let snapshot = broker.snapshot(&session.session_id).expect("snapshot");
    assert_eq!(snapshot["page"]["page_id"], new_page_id);
    assert_eq!(snapshot["page"]["title"], "After");
    assert_eq!(broker.list_debuggable_pages(true).len(), 1);
    assert!(broker.get_page(&old_page_id).is_none());
}

#[test]
fn test_page_bridge_same_tab_id_keeps_concurrent_pages_separate() {
    let broker = BrowserDebugBroker::new();
    let (first_page_id, first_token) =
        broker.register_page_candidate(input("http://example.test/assistant"));
    broker
        .bridge_hello(
            &first_page_id,
            BridgeHelloPayload {
                token: first_token.clone(),
                scope: None,
                tab_id: Some("cloned-tab".to_string()),
                title: Some("Assistant A".to_string()),
                url: None,
                user_agent: None,
                dom_snapshot: None,
                dom_tree: None,
                storage: None,
                console: Vec::new(),
                network: Vec::new(),
            },
        )
        .expect("first hello");
    let (tx, _rx) = mpsc::channel(1);
    broker
        .bridge_ws_attach(&first_page_id, &first_token, tx)
        .expect("attach");

    let (second_page_id, second_token) =
        broker.register_page_candidate(input("http://example.test/assistant"));
    broker
        .bridge_hello(
            &second_page_id,
            BridgeHelloPayload {
                token: second_token,
                scope: None,
                tab_id: Some("cloned-tab".to_string()),
                title: Some("Assistant B".to_string()),
                url: None,
                user_agent: None,
                dom_snapshot: None,
                dom_tree: None,
                storage: None,
                console: Vec::new(),
                network: Vec::new(),
            },
        )
        .expect("second hello");

    let pages = broker.list_debuggable_pages(true);
    assert_eq!(pages.len(), 2);
    assert!(pages.iter().any(|page| page.page_id == first_page_id));
    assert!(pages.iter().any(|page| page.page_id == second_page_id));
}

#[test]
fn test_page_bridge_rejects_token_replay_or_mismatch() {
    let broker = BrowserDebugBroker::new();
    let (page_id, _token) =
        broker.register_page_candidate(input("http://example.test/devtools/basic.html"));

    let result = broker.bridge_hello(
        &page_id,
        BridgeHelloPayload {
            token: "wrong".to_string(),
            scope: None,
            tab_id: Some("tab-a".to_string()),
            title: Some("Fixture".to_string()),
            url: None,
            user_agent: None,
            dom_snapshot: None,
            dom_tree: None,
            storage: None,
            console: Vec::new(),
            network: Vec::new(),
        },
    );

    assert!(result.is_err());
    assert_eq!(broker.list_pages(true)[0].state, DebugPageState::Candidate);
}

#[test]
fn test_page_bridge_seq_dedupes_replayed_messages() {
    let broker = BrowserDebugBroker::new();
    let (page_id, _token) =
        broker.register_page_candidate(input("http://example.test/devtools/basic.html"));

    assert!(broker.remember_bridge_seq(&page_id, 42));
    assert!(
        !broker.remember_bridge_seq(&page_id, 42),
        "replayed bridge messages with the same seq must not be processed twice"
    );
    assert!(broker.remember_bridge_seq(&page_id, 43));
}

#[test]
fn test_page_bridge_network_cache_dedupes_snapshot_replay_by_client_req_id() {
    let broker = BrowserDebugBroker::new();
    let (page_id, token) =
        broker.register_page_candidate(input("http://example.test/devtools/basic.html"));
    let event = network_input(
        "http://example.test/devtools/api/ping?case=basic",
        Some("client-req-1"),
        200,
    );

    broker
        .bridge_network(
            &page_id,
            BridgeNetworkPayload {
                token: token.clone(),
                event: event.clone(),
            },
        )
        .expect("live network event");
    broker
        .bridge_hello(
            &page_id,
            BridgeHelloPayload {
                token,
                scope: Some("full".to_string()),
                tab_id: Some("tab-a".to_string()),
                title: Some("Fixture".to_string()),
                url: None,
                user_agent: None,
                dom_snapshot: None,
                dom_tree: None,
                storage: None,
                console: Vec::new(),
                network: vec![event],
            },
        )
        .expect("bridge hello");

    let page = broker.get_page(&page_id).expect("page");
    let ping_events: Vec<_> = page
        .network_events
        .iter()
        .filter(|event| event.url.contains("/devtools/api/ping?case=basic"))
        .collect();
    assert_eq!(
        ping_events.len(),
        1,
        "snapshot refreshes must not duplicate a frontend bridge network event"
    );
    assert_eq!(
        ping_events[0].client_req_id.as_deref(),
        Some("client-req-1")
    );
}

#[test]
fn test_page_bridge_network_cache_prefers_client_req_event_over_performance_fallback() {
    let mut events = Vec::new();
    merge_network_event(
            &mut events,
            network_event_from_input(network_input(
                "http://example.test/devtools/static.js?__bifrost_client_req_id=client-req-2&case=asset#hash",
                None,
                0,
            ))
            .expect("fallback event"),
        );
    merge_network_event(
        &mut events,
        network_event_from_input(network_input(
            "http://example.test/devtools/static.js?case=asset",
            Some("client-req-2"),
            200,
        ))
        .expect("mapped event"),
    );

    assert_eq!(events.len(), 1);
    assert_eq!(events[0].client_req_id.as_deref(), Some("client-req-2"));
    assert_eq!(events[0].status, Some(200));
    assert_eq!(
        events[0].url,
        "http://example.test/devtools/static.js?case=asset"
    );
}

#[test]
fn test_page_bridge_live_queues_are_bounded() {
    assert!(bridge_command_queue_capacity() > 0);
    assert!(session_live_queue_capacity() > 0);
    assert!(
        session_live_queue_capacity() <= 1024,
        "WebUI live queues must remain bounded to protect the admin process from slow consumers"
    );
}

#[tokio::test]
async fn test_runtime_evaluate_allowed_for_default_session_scope() {
    let broker = BrowserDebugBroker::new();
    let (page_id, token) =
        broker.register_page_candidate(input("http://example.test/devtools/basic.html"));
    broker
        .bridge_hello(
            &page_id,
            BridgeHelloPayload {
                token,
                scope: None,
                tab_id: Some("tab-a".to_string()),
                title: Some("Fixture".to_string()),
                url: None,
                user_agent: None,
                dom_snapshot: None,
                dom_tree: None,
                storage: None,
                console: Vec::new(),
                network: Vec::new(),
            },
        )
        .expect("bridge hello");
    let session = broker.open_session(&page_id).expect("session");

    let result = broker
        .command(
            &session.session_id,
            "runtime.evaluate",
            serde_json::json!({"expression": "document.title"}),
        )
        .await;

    assert!(
        result.unwrap_err().contains("timed out"),
        "runtime.evaluate should be queued even for legacy read-mode pages"
    );
}
