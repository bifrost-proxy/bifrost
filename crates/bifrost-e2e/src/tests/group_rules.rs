use std::net::TcpListener;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bifrost_storage::{RuleFile, RulesStorage};

use crate::assertions::{assert_json_field, assert_status};
use crate::{ProxyInstance, TestCase};

pub fn get_all_tests() -> Vec<TestCase> {
    vec![
        TestCase::standalone(
            "group_rules_sync_unavailable_returns_503",
            "Group rules API returns 503 when sync manager is not available",
            "group_rules",
            || async move {
                let port = pick_unused_port()?;
                let (_proxy, _admin_state) =
                    ProxyInstance::start_with_admin(port, vec![], false, true)
                        .await
                        .map_err(|e| format!("Failed to start proxy: {}", e))?;

                let client = build_client()?;

                let resp = client
                    .get(format!(
                        "http://127.0.0.1:{}/_bifrost/api/group-rules/test-group-id",
                        port
                    ))
                    .send()
                    .await
                    .map_err(|e| format!("Request failed: {}", e))?;

                assert_status(&resp, 503)?;
                let body: serde_json::Value = resp
                    .json()
                    .await
                    .map_err(|e| format!("Failed to parse JSON: {}", e))?;
                let error = body.get("error").and_then(|v| v.as_str()).unwrap_or("");
                if !error.contains("Sync manager not available") {
                    return Err(format!(
                        "Expected 'Sync manager not available' error, got: {}",
                        error
                    ));
                }
                Ok(())
            },
        ),
        TestCase::standalone(
            "group_rules_missing_group_id_returns_400",
            "Group rules API returns 400 when group_id is missing from path",
            "group_rules",
            || async move {
                let port = pick_unused_port()?;
                let (_proxy, _admin_state) =
                    ProxyInstance::start_with_admin_sync(port, vec![], false, true)
                        .await
                        .map_err(|e| format!("Failed to start proxy: {}", e))?;

                let client = build_client()?;

                let resp = client
                    .get(format!(
                        "http://127.0.0.1:{}/_bifrost/api/group-rules/",
                        port
                    ))
                    .send()
                    .await
                    .map_err(|e| format!("Request failed: {}", e))?;

                let status = resp.status().as_u16();
                if status != 400 && status != 503 {
                    return Err(format!("Expected 400 or 503, got {}", status));
                }
                Ok(())
            },
        ),
        TestCase::standalone(
            "group_proxy_sync_unavailable_returns_503",
            "Group proxy API returns 503 when sync manager is not available",
            "group_rules",
            || async move {
                let port = pick_unused_port()?;
                let (_proxy, _admin_state) =
                    ProxyInstance::start_with_admin(port, vec![], false, true)
                        .await
                        .map_err(|e| format!("Failed to start proxy: {}", e))?;

                let client = build_client()?;

                let resp = client
                    .get(format!(
                        "http://127.0.0.1:{}/_bifrost/api/group?offset=0&limit=10",
                        port
                    ))
                    .send()
                    .await
                    .map_err(|e| format!("Request failed: {}", e))?;

                assert_status(&resp, 503)?;
                let body: serde_json::Value = resp
                    .json()
                    .await
                    .map_err(|e| format!("Failed to parse JSON: {}", e))?;
                let error = body.get("error").and_then(|v| v.as_str()).unwrap_or("");
                if !error.contains("Sync manager not available") {
                    return Err(format!(
                        "Expected 'Sync manager not available' error, got: {}",
                        error
                    ));
                }
                Ok(())
            },
        ),
        TestCase::standalone(
            "group_rules_get_rule_without_list_returns_400",
            "Getting a group rule without first listing returns 400 (group not loaded yet)",
            "group_rules",
            || async move {
                let port = pick_unused_port()?;
                let (_proxy, _admin_state) =
                    ProxyInstance::start_with_admin_sync(port, vec![], false, true)
                        .await
                        .map_err(|e| format!("Failed to start proxy: {}", e))?;

                let client = build_client()?;

                let resp = client
                    .get(format!(
                        "http://127.0.0.1:{}/_bifrost/api/group-rules/some-group/my-rule",
                        port
                    ))
                    .send()
                    .await
                    .map_err(|e| format!("Request failed: {}", e))?;

                assert_status(&resp, 400)?;
                let body: serde_json::Value = resp
                    .json()
                    .await
                    .map_err(|e| format!("Failed to parse JSON: {}", e))?;
                let error = body.get("error").and_then(|v| v.as_str()).unwrap_or("");
                if !error.contains("not loaded yet") {
                    return Err(format!("Expected 'not loaded yet' error, got: {}", error));
                }
                Ok(())
            },
        ),
        TestCase::standalone(
            "group_rules_enable_without_list_returns_400",
            "Enabling a group rule without first listing returns 400 (group not loaded yet)",
            "group_rules",
            || async move {
                let port = pick_unused_port()?;
                let (_proxy, _admin_state) =
                    ProxyInstance::start_with_admin_sync(port, vec![], false, true)
                        .await
                        .map_err(|e| format!("Failed to start proxy: {}", e))?;

                let client = build_client()?;

                let resp = client
                    .put(format!(
                        "http://127.0.0.1:{}/_bifrost/api/group-rules/some-group/my-rule/enable",
                        port
                    ))
                    .send()
                    .await
                    .map_err(|e| format!("Request failed: {}", e))?;

                assert_status(&resp, 400)?;
                let body: serde_json::Value = resp
                    .json()
                    .await
                    .map_err(|e| format!("Failed to parse JSON: {}", e))?;
                let error = body.get("error").and_then(|v| v.as_str()).unwrap_or("");
                if !error.contains("not loaded yet") {
                    return Err(format!("Expected 'not loaded yet' error, got: {}", error));
                }
                Ok(())
            },
        ),
        TestCase::standalone(
            "group_rules_enable_method_not_allowed",
            "Enable/disable endpoint only accepts PUT, other methods return 405",
            "group_rules",
            || async move {
                let port = pick_unused_port()?;
                let (_proxy, _admin_state) =
                    ProxyInstance::start_with_admin_sync(port, vec![], false, true)
                        .await
                        .map_err(|e| format!("Failed to start proxy: {}", e))?;

                let client = build_client()?;

                let resp = client
                    .get(format!(
                        "http://127.0.0.1:{}/_bifrost/api/group-rules/some-group/my-rule/enable",
                        port
                    ))
                    .send()
                    .await
                    .map_err(|e| format!("Request failed: {}", e))?;

                assert_status(&resp, 405)?;

                let resp2 = client
                    .post(format!(
                        "http://127.0.0.1:{}/_bifrost/api/group-rules/some-group/my-rule/disable",
                        port
                    ))
                    .send()
                    .await
                    .map_err(|e| format!("Request failed: {}", e))?;

                assert_status(&resp2, 405)?;

                Ok(())
            },
        ),
        TestCase::standalone(
            "group_rules_enable_disable_local_storage",
            "Enable and disable group rules via admin API with pre-populated local storage",
            "group_rules",
            || async move {
                let port = pick_unused_port()?;
                let (_proxy, admin_state) =
                    ProxyInstance::start_with_admin_sync(port, vec![], false, true)
                        .await
                        .map_err(|e| format!("Failed to start proxy: {}", e))?;

                let group_name = "test-enable-group";
                let group_id = "grp-enable-001";
                {
                    let mut cache = admin_state.group_name_cache();
                    cache.insert(group_id.to_string(), group_name.to_string());
                }

                let group_storage = setup_group_storage(&admin_state, group_name)?;

                let mut rule =
                    RuleFile::new("enable-test-rule", "example.com host://127.0.0.1:3000");
                rule.enabled = false;
                rule.group = Some(group_name.to_string());
                group_storage
                    .save(&rule)
                    .map_err(|e| format!("Failed to save rule: {}", e))?;

                let client = build_client()?;

                let resp = client
                    .put(format!(
                        "http://127.0.0.1:{}/_bifrost/api/group-rules/{}/enable-test-rule/enable",
                        port, group_id
                    ))
                    .send()
                    .await
                    .map_err(|e| format!("Enable request failed: {}", e))?;

                assert_status(&resp, 200)?;
                let body: serde_json::Value = resp
                    .json()
                    .await
                    .map_err(|e| format!("Failed to parse JSON: {}", e))?;
                let message = body.get("message").and_then(|v| v.as_str()).unwrap_or("");
                if !message.contains("enabled") {
                    return Err(format!("Expected 'enabled' in message, got: {}", message));
                }

                let loaded = group_storage
                    .load("enable-test-rule")
                    .map_err(|e| format!("Failed to load rule: {}", e))?;
                if !loaded.enabled {
                    return Err("Rule should be enabled after enable API call".to_string());
                }

                let resp2 = client
                    .put(format!(
                        "http://127.0.0.1:{}/_bifrost/api/group-rules/{}/enable-test-rule/disable",
                        port, group_id
                    ))
                    .send()
                    .await
                    .map_err(|e| format!("Disable request failed: {}", e))?;

                assert_status(&resp2, 200)?;
                let body2: serde_json::Value = resp2
                    .json()
                    .await
                    .map_err(|e| format!("Failed to parse JSON: {}", e))?;
                let message2 = body2.get("message").and_then(|v| v.as_str()).unwrap_or("");
                if !message2.contains("disabled") {
                    return Err(format!("Expected 'disabled' in message, got: {}", message2));
                }

                let loaded2 = group_storage
                    .load("enable-test-rule")
                    .map_err(|e| format!("Failed to load rule after disable: {}", e))?;
                if loaded2.enabled {
                    return Err("Rule should be disabled after disable API call".to_string());
                }

                cleanup_group_storage(&admin_state, group_name);
                Ok(())
            },
        ),
        TestCase::standalone(
            "group_rules_enable_nonexistent_rule_returns_error",
            "Enabling a nonexistent rule in a loaded group returns 500",
            "group_rules",
            || async move {
                let port = pick_unused_port()?;
                let (_proxy, admin_state) =
                    ProxyInstance::start_with_admin_sync(port, vec![], false, true)
                        .await
                        .map_err(|e| format!("Failed to start proxy: {}", e))?;

                let group_name = "test-enable-nonexist-group";
                let group_id = "grp-enable-nonexist-001";
                {
                    let mut cache = admin_state.group_name_cache();
                    cache.insert(group_id.to_string(), group_name.to_string());
                }
                let _group_storage = setup_group_storage(&admin_state, group_name)?;

                let client = build_client()?;

                let resp = client
                    .put(format!(
                        "http://127.0.0.1:{}/_bifrost/api/group-rules/{}/nonexistent-rule/enable",
                        port, group_id
                    ))
                    .send()
                    .await
                    .map_err(|e| format!("Request failed: {}", e))?;

                assert_status(&resp, 500)?;

                cleanup_group_storage(&admin_state, group_name);
                Ok(())
            },
        ),
        TestCase::standalone(
            "group_rules_get_rule_from_local_storage",
            "Get a specific group rule detail from pre-populated local storage",
            "group_rules",
            || async move {
                let port = pick_unused_port()?;
                let (_proxy, admin_state) =
                    ProxyInstance::start_with_admin_sync(port, vec![], false, true)
                        .await
                        .map_err(|e| format!("Failed to start proxy: {}", e))?;

                let group_name = "test-get-rule-group";
                let group_id = "grp-get-001";
                {
                    let mut cache = admin_state.group_name_cache();
                    cache.insert(group_id.to_string(), group_name.to_string());
                }

                let group_storage = setup_group_storage(&admin_state, group_name)?;

                let mut rule = RuleFile::new(
                    "detail-rule",
                    "api.example.com host://127.0.0.1:8080\ntest.example.com host://127.0.0.1:9090",
                );
                rule.enabled = true;
                rule.group = Some(group_name.to_string());
                rule.mark_synced(
                    "env-123",
                    "user-456",
                    "2024-01-01T00:00:00Z",
                    "2024-06-01T00:00:00Z",
                );
                group_storage
                    .save(&rule)
                    .map_err(|e| format!("Failed to save rule: {}", e))?;

                let client = build_client()?;

                let resp = client
                    .get(format!(
                        "http://127.0.0.1:{}/_bifrost/api/group-rules/{}/detail-rule",
                        port, group_id
                    ))
                    .send()
                    .await
                    .map_err(|e| format!("Request failed: {}", e))?;

                assert_status(&resp, 200)?;
                let json: serde_json::Value = resp
                    .json()
                    .await
                    .map_err(|e| format!("Failed to parse JSON: {}", e))?;

                assert_json_field(&json, "name", "detail-rule")?;
                assert_json_field(&json, "enabled", "true")?;

                let content = json
                    .get("content")
                    .and_then(|v| v.as_str())
                    .ok_or("Missing 'content' field")?;
                if !content.contains("api.example.com") {
                    return Err(format!(
                        "Content should contain 'api.example.com', got: {}",
                        content
                    ));
                }

                let sync = json.get("sync").ok_or("Missing 'sync' field")?;
                assert_json_field(sync, "status", "synced")?;
                assert_json_field(sync, "remote_id", "env-123")?;

                cleanup_group_storage(&admin_state, group_name);
                Ok(())
            },
        ),
        TestCase::standalone(
            "group_rules_get_nonexistent_rule_returns_404",
            "Getting a nonexistent group rule returns 404",
            "group_rules",
            || async move {
                let port = pick_unused_port()?;
                let (_proxy, admin_state) =
                    ProxyInstance::start_with_admin_sync(port, vec![], false, true)
                        .await
                        .map_err(|e| format!("Failed to start proxy: {}", e))?;

                let group_name = "test-get-notfound-group";
                let group_id = "grp-notfound-001";
                {
                    let mut cache = admin_state.group_name_cache();
                    cache.insert(group_id.to_string(), group_name.to_string());
                }
                let _group_storage = setup_group_storage(&admin_state, group_name)?;

                let client = build_client()?;

                let resp = client
                    .get(format!(
                        "http://127.0.0.1:{}/_bifrost/api/group-rules/{}/nonexistent",
                        port, group_id
                    ))
                    .send()
                    .await
                    .map_err(|e| format!("Request failed: {}", e))?;

                assert_status(&resp, 404)?;

                cleanup_group_storage(&admin_state, group_name);
                Ok(())
            },
        ),
        TestCase::standalone(
            "group_rules_url_encoded_rule_name",
            "Group rules API handles URL-encoded rule names correctly",
            "group_rules",
            || async move {
                let port = pick_unused_port()?;
                let (_proxy, admin_state) =
                    ProxyInstance::start_with_admin_sync(port, vec![], false, true)
                        .await
                        .map_err(|e| format!("Failed to start proxy: {}", e))?;

                let group_name = "test-url-encode-group";
                let group_id = "grp-urlencode-001";
                {
                    let mut cache = admin_state.group_name_cache();
                    cache.insert(group_id.to_string(), group_name.to_string());
                }

                let group_storage = setup_group_storage(&admin_state, group_name)?;

                let rule_name = "rule with spaces";
                let mut rule = RuleFile::new(rule_name, "example.com host://127.0.0.1:3000");
                rule.enabled = false;
                rule.group = Some(group_name.to_string());
                group_storage
                    .save(&rule)
                    .map_err(|e| format!("Failed to save rule: {}", e))?;

                let client = build_client()?;

                let encoded_name = urlencoding::encode(rule_name);
                let resp = client
                    .get(format!(
                        "http://127.0.0.1:{}/_bifrost/api/group-rules/{}/{}",
                        port, group_id, encoded_name
                    ))
                    .send()
                    .await
                    .map_err(|e| format!("Request failed: {}", e))?;

                assert_status(&resp, 200)?;
                let json: serde_json::Value = resp
                    .json()
                    .await
                    .map_err(|e| format!("Failed to parse JSON: {}", e))?;

                assert_json_field(&json, "name", rule_name)?;

                let resp2 = client
                    .put(format!(
                        "http://127.0.0.1:{}/_bifrost/api/group-rules/{}/{}/enable",
                        port, group_id, encoded_name
                    ))
                    .send()
                    .await
                    .map_err(|e| format!("Enable request failed: {}", e))?;

                assert_status(&resp2, 200)?;

                let loaded = group_storage
                    .load(rule_name)
                    .map_err(|e| format!("Failed to load rule: {}", e))?;
                if !loaded.enabled {
                    return Err("Rule with spaces should be enabled".to_string());
                }

                cleanup_group_storage(&admin_state, group_name);
                Ok(())
            },
        ),
        TestCase::standalone(
            "group_rules_active_summary_includes_group_rules",
            "Active summary endpoint includes enabled group rules from subdirectories",
            "group_rules",
            || async move {
                let port = pick_unused_port()?;
                let (_proxy, admin_state) =
                    ProxyInstance::start_with_admin_sync(port, vec![], false, true)
                        .await
                        .map_err(|e| format!("Failed to start proxy: {}", e))?;
                save_test_sync_token(&admin_state).await?;

                let group_name = "test-active-summary-group";
                let group_id = "grp-active-001";
                {
                    let mut cache = admin_state.group_name_cache();
                    cache.insert(group_id.to_string(), group_name.to_string());
                }

                let group_storage = setup_group_storage(&admin_state, group_name)?;

                let mut enabled_rule =
                    RuleFile::new("active-rule-1", "active.example.com host://127.0.0.1:3000");
                enabled_rule.enabled = true;
                enabled_rule.group = Some(group_name.to_string());
                group_storage
                    .save(&enabled_rule)
                    .map_err(|e| format!("Failed to save enabled rule: {}", e))?;

                let mut disabled_rule = RuleFile::new(
                    "inactive-rule-1",
                    "inactive.example.com host://127.0.0.1:4000",
                );
                disabled_rule.enabled = false;
                disabled_rule.group = Some(group_name.to_string());
                group_storage
                    .save(&disabled_rule)
                    .map_err(|e| format!("Failed to save disabled rule: {}", e))?;

                let client = build_client()?;

                let resp = client
                    .get(format!(
                        "http://127.0.0.1:{}/_bifrost/api/rules/active-summary",
                        port
                    ))
                    .send()
                    .await
                    .map_err(|e| format!("Request failed: {}", e))?;

                assert_status(&resp, 200)?;
                let json: serde_json::Value = resp
                    .json()
                    .await
                    .map_err(|e| format!("Failed to parse JSON: {}", e))?;

                let rules = json
                    .get("rules")
                    .and_then(|v| v.as_array())
                    .ok_or("Missing 'rules' array")?;

                let has_active = rules
                    .iter()
                    .any(|r| r.get("name").and_then(|n| n.as_str()) == Some("active-rule-1"));
                if !has_active {
                    return Err(format!(
                        "Expected 'active-rule-1' in active summary, got rules: {:?}",
                        rules
                            .iter()
                            .filter_map(|r| r.get("name").and_then(|n| n.as_str()))
                            .collect::<Vec<_>>()
                    ));
                }

                let has_inactive = rules
                    .iter()
                    .any(|r| r.get("name").and_then(|n| n.as_str()) == Some("inactive-rule-1"));
                if has_inactive {
                    return Err(
                        "Disabled rule 'inactive-rule-1' should NOT appear in active summary"
                            .to_string(),
                    );
                }

                let active_rule = rules
                    .iter()
                    .find(|r| r.get("name").and_then(|n| n.as_str()) == Some("active-rule-1"))
                    .ok_or("active-rule-1 not found in rules")?;

                let group_name_val = active_rule
                    .get("group_name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if group_name_val != group_name {
                    return Err(format!(
                        "Expected group_name '{}', got '{}'",
                        group_name, group_name_val
                    ));
                }

                cleanup_group_storage(&admin_state, group_name);
                Ok(())
            },
        ),
        TestCase::standalone(
            "group_rules_active_summary_after_toggle",
            "Active summary reflects rule state changes after enable/disable toggle",
            "group_rules",
            || async move {
                let port = pick_unused_port()?;
                let (_proxy, admin_state) =
                    ProxyInstance::start_with_admin_sync(port, vec![], false, true)
                        .await
                        .map_err(|e| format!("Failed to start proxy: {}", e))?;
                save_test_sync_token(&admin_state).await?;

                let group_name = "test-toggle-summary-group";
                let group_id = "grp-toggle-001";
                {
                    let mut cache = admin_state.group_name_cache();
                    cache.insert(group_id.to_string(), group_name.to_string());
                }

                let group_storage = setup_group_storage(&admin_state, group_name)?;

                let mut rule =
                    RuleFile::new("toggle-rule", "toggle.example.com host://127.0.0.1:5000");
                rule.enabled = false;
                rule.group = Some(group_name.to_string());
                group_storage
                    .save(&rule)
                    .map_err(|e| format!("Failed to save rule: {}", e))?;

                let client = build_client()?;

                let resp1 = client
                    .get(format!(
                        "http://127.0.0.1:{}/_bifrost/api/rules/active-summary",
                        port
                    ))
                    .send()
                    .await
                    .map_err(|e| format!("Request failed: {}", e))?;
                let json1: serde_json::Value = resp1
                    .json()
                    .await
                    .map_err(|e| format!("Failed to parse JSON: {}", e))?;
                let rules1 = json1
                    .get("rules")
                    .and_then(|v| v.as_array())
                    .ok_or("Missing 'rules' array")?;
                let has_toggle_before = rules1
                    .iter()
                    .any(|r| r.get("name").and_then(|n| n.as_str()) == Some("toggle-rule"));
                if has_toggle_before {
                    return Err("Disabled rule should not appear in active summary".to_string());
                }

                let enable_resp = client
                    .put(format!(
                        "http://127.0.0.1:{}/_bifrost/api/group-rules/{}/toggle-rule/enable",
                        port, group_id
                    ))
                    .send()
                    .await
                    .map_err(|e| format!("Enable request failed: {}", e))?;
                assert_status(&enable_resp, 200)?;

                let resp2 = client
                    .get(format!(
                        "http://127.0.0.1:{}/_bifrost/api/rules/active-summary",
                        port
                    ))
                    .send()
                    .await
                    .map_err(|e| format!("Request failed: {}", e))?;
                let json2: serde_json::Value = resp2
                    .json()
                    .await
                    .map_err(|e| format!("Failed to parse JSON: {}", e))?;
                let rules2 = json2
                    .get("rules")
                    .and_then(|v| v.as_array())
                    .ok_or("Missing 'rules' array")?;
                let has_toggle_after = rules2
                    .iter()
                    .any(|r| r.get("name").and_then(|n| n.as_str()) == Some("toggle-rule"));
                if !has_toggle_after {
                    return Err(
                        "Enabled rule should appear in active summary after toggle".to_string()
                    );
                }

                let disable_resp = client
                    .put(format!(
                        "http://127.0.0.1:{}/_bifrost/api/group-rules/{}/toggle-rule/disable",
                        port, group_id
                    ))
                    .send()
                    .await
                    .map_err(|e| format!("Disable request failed: {}", e))?;
                assert_status(&disable_resp, 200)?;

                let resp3 = client
                    .get(format!(
                        "http://127.0.0.1:{}/_bifrost/api/rules/active-summary",
                        port
                    ))
                    .send()
                    .await
                    .map_err(|e| format!("Request failed: {}", e))?;
                let json3: serde_json::Value = resp3
                    .json()
                    .await
                    .map_err(|e| format!("Failed to parse JSON: {}", e))?;
                let rules3 = json3
                    .get("rules")
                    .and_then(|v| v.as_array())
                    .ok_or("Missing 'rules' array")?;
                let has_toggle_final = rules3
                    .iter()
                    .any(|r| r.get("name").and_then(|n| n.as_str()) == Some("toggle-rule"));
                if has_toggle_final {
                    return Err(
                        "Disabled rule should not appear in active summary after disable"
                            .to_string(),
                    );
                }

                cleanup_group_storage(&admin_state, group_name);
                Ok(())
            },
        ),
        TestCase::standalone(
            "group_rules_active_summary_skips_group_rules_without_login",
            "Active summary does not consume enabled group rules without an active sync session",
            "group_rules",
            || async move {
                let port = pick_unused_port()?;
                let (_proxy, admin_state) =
                    ProxyInstance::start_with_admin_sync(port, vec![], false, true)
                        .await
                        .map_err(|e| format!("Failed to start proxy: {}", e))?;

                let group_name = "uncached-active-summary-group";
                let group_storage = setup_group_storage(&admin_state, group_name)?;

                let mut rule = RuleFile::new(
                    "uncached-group-rule",
                    "uncached-active.example.com host://127.0.0.1:5100",
                );
                rule.enabled = true;
                rule.group = Some(group_name.to_string());
                group_storage
                    .save(&rule)
                    .map_err(|e| format!("Failed to save uncached group rule: {}", e))?;

                let client = build_client()?;
                let resp = client
                    .get(format!(
                        "http://127.0.0.1:{}/_bifrost/api/rules/active-summary",
                        port
                    ))
                    .send()
                    .await
                    .map_err(|e| format!("Request failed: {}", e))?;
                assert_status(&resp, 200)?;
                let json: serde_json::Value = resp
                    .json()
                    .await
                    .map_err(|e| format!("Failed to parse JSON: {}", e))?;
                let rules = json
                    .get("rules")
                    .and_then(|v| v.as_array())
                    .ok_or("Missing 'rules' array")?;
                if rules
                    .iter()
                    .any(|r| r.get("name").and_then(|n| n.as_str()) == Some("uncached-group-rule"))
                {
                    return Err(format!(
                        "Group rule should not appear in active summary without login, got {:?}",
                        rules
                    ));
                }

                cleanup_group_storage(&admin_state, group_name);
                Ok(())
            },
        ),
        TestCase::standalone(
            "group_rules_enable_refreshes_badge_cache",
            "Enabling a group rule refreshes proxy badge active-rule data",
            "group_rules",
            || async move {
                let port = pick_unused_port()?;
                let (_proxy, admin_state) =
                    ProxyInstance::start_with_admin_sync(port, vec![], false, true)
                        .await
                        .map_err(|e| format!("Failed to start proxy: {}", e))?;
                save_test_sync_token(&admin_state).await?;

                let group_name = "badge-cache-group";
                let group_id = "grp-badge-cache-001";
                {
                    let mut cache = admin_state.group_name_cache();
                    cache.insert(group_id.to_string(), group_name.to_string());
                }

                let group_storage = setup_group_storage(&admin_state, group_name)?;
                let mut rule =
                    RuleFile::new("badge-cache-rule", "badge-cache.example.com status://200");
                rule.enabled = false;
                rule.group = Some(group_name.to_string());
                group_storage
                    .save(&rule)
                    .map_err(|e| format!("Failed to save badge cache rule: {}", e))?;

                admin_state.refresh_badge_rules_cache();
                let before: serde_json::Value =
                    serde_json::from_str(&admin_state.badge_rules_json())
                        .map_err(|e| format!("Failed to parse initial badge JSON: {}", e))?;
                if before
                    .get("rules")
                    .and_then(|v| v.as_array())
                    .is_some_and(|rules| {
                        rules.iter().any(|r| {
                            r.get("name").and_then(|n| n.as_str()) == Some("badge-cache-rule")
                        })
                    })
                {
                    return Err("Disabled group rule should not be in badge cache".to_string());
                }

                let client = build_client()?;
                let enable_resp = client
                    .put(format!(
                        "http://127.0.0.1:{}/_bifrost/api/group-rules/{}/badge-cache-rule/enable",
                        port, group_id
                    ))
                    .send()
                    .await
                    .map_err(|e| format!("Enable request failed: {}", e))?;
                assert_status(&enable_resp, 200)?;

                let after: serde_json::Value =
                    serde_json::from_str(&admin_state.badge_rules_json())
                        .map_err(|e| format!("Failed to parse updated badge JSON: {}", e))?;
                let rules = after
                    .get("rules")
                    .and_then(|v| v.as_array())
                    .ok_or("Missing badge rules array")?;
                let badge_rule = rules
                    .iter()
                    .find(|r| r.get("name").and_then(|n| n.as_str()) == Some("badge-cache-rule"))
                    .ok_or_else(|| {
                        format!(
                            "Expected badge-cache-rule in badge cache after enable, got {:?}",
                            rules
                        )
                    })?;
                if badge_rule.get("group_id").and_then(|v| v.as_str()) != Some(group_name) {
                    return Err(format!(
                        "Expected badge group_id navigation label '{}', got {:?}",
                        group_name,
                        badge_rule.get("group_id")
                    ));
                }
                if badge_rule.get("group_name").and_then(|v| v.as_str()) != Some(group_id) {
                    return Err(format!(
                        "Expected badge group_name navigation id '{}', got {:?}",
                        group_id,
                        badge_rule.get("group_name")
                    ));
                }

                cleanup_group_storage(&admin_state, group_name);
                Ok(())
            },
        ),
        TestCase::standalone(
            "group_rules_active_summary_survives_remote_cache_resolution_failure",
            "Active summary keeps local group rules when remote cache resolution cannot complete",
            "group_rules",
            || async move {
                let port = pick_unused_port()?;
                let (_proxy, admin_state) =
                    ProxyInstance::start_with_admin_sync(port, vec![], false, true)
                        .await
                        .map_err(|e| format!("Failed to start proxy: {}", e))?;
                save_test_sync_token(&admin_state).await?;

                let group_name = "remote-cache-failure-group";
                let group_storage = setup_group_storage(&admin_state, group_name)?;

                let mut rule = RuleFile::new(
                    "remote-cache-failure-rule",
                    "remote-cache-failure.example.com status://218",
                );
                rule.enabled = true;
                rule.group = Some(group_name.to_string());
                group_storage
                    .save(&rule)
                    .map_err(|e| format!("Failed to save remote-cache-failure rule: {}", e))?;

                let client = build_client()?;
                let first = fetch_active_summary(&client, port).await?;
                assert_active_rule_present(
                    &first,
                    "remote-cache-failure-rule",
                    Some(group_name),
                    Some(group_name),
                    Some("status://218"),
                )?;

                let deadline = Instant::now() + Duration::from_secs(2);
                while admin_state.is_group_cache_resolved() && Instant::now() < deadline {
                    tokio::time::sleep(Duration::from_millis(25)).await;
                }
                if admin_state.is_group_cache_resolved() {
                    return Err(
                        "group cache resolution flag stayed set after remote resolution failure"
                            .to_string(),
                    );
                }

                let second = fetch_active_summary(&client, port).await?;
                assert_active_rule_present(
                    &second,
                    "remote-cache-failure-rule",
                    Some(group_name),
                    Some(group_name),
                    Some("status://218"),
                )?;
                if !admin_state
                    .rules_storage
                    .base_dir()
                    .join(group_name)
                    .is_dir()
                {
                    return Err("active-summary must not delete local group rule directory".into());
                }

                cleanup_group_storage(&admin_state, group_name);
                Ok(())
            },
        ),
        TestCase::standalone(
            "group_rules_rapid_toggle_keeps_active_summary_and_badge_consistent",
            "Rapid group rule toggles converge active-summary and badge cache to the same state",
            "group_rules",
            || async move {
                let port = pick_unused_port()?;
                let (_proxy, admin_state) =
                    ProxyInstance::start_with_admin_sync(port, vec![], false, true)
                        .await
                        .map_err(|e| format!("Failed to start proxy: {}", e))?;
                save_test_sync_token(&admin_state).await?;

                let group_name = "rapid-toggle-stability-group";
                let group_id = "grp-rapid-toggle-stability";
                {
                    let mut cache = admin_state.group_name_cache();
                    cache.insert(group_id.to_string(), group_name.to_string());
                }

                let group_storage = setup_group_storage(&admin_state, group_name)?;
                let mut rule = RuleFile::new(
                    "rapid-toggle-stability-rule",
                    "rapid-toggle-stability.example.com status://219",
                );
                rule.enabled = false;
                rule.group = Some(group_name.to_string());
                group_storage
                    .save(&rule)
                    .map_err(|e| format!("Failed to save rapid toggle rule: {}", e))?;

                let client = build_client()?;
                for i in 0..3 {
                    let enable_resp = client
                        .put(format!(
                            "http://127.0.0.1:{}/_bifrost/api/group-rules/{}/rapid-toggle-stability-rule/enable",
                            port, group_id
                        ))
                        .send()
                        .await
                        .map_err(|e| format!("Enable request {i} failed: {e}"))?;
                    assert_status(&enable_resp, 200)?;
                    wait_for_rule_state(
                        &client,
                        &admin_state,
                        port,
                        "rapid-toggle-stability-rule",
                        true,
                        "status://219",
                    )
                    .await?;

                    let disable_resp = client
                        .put(format!(
                            "http://127.0.0.1:{}/_bifrost/api/group-rules/{}/rapid-toggle-stability-rule/disable",
                            port, group_id
                        ))
                        .send()
                        .await
                        .map_err(|e| format!("Disable request {i} failed: {e}"))?;
                    assert_status(&disable_resp, 200)?;
                    wait_for_rule_state(
                        &client,
                        &admin_state,
                        port,
                        "rapid-toggle-stability-rule",
                        false,
                        "status://219",
                    )
                    .await?;
                }

                cleanup_group_storage(&admin_state, group_name);
                Ok(())
            },
        ),
        TestCase::standalone(
            "group_rules_deeplink_accepts_group_name",
            "Group rules API accepts the local group name used by badge and Rules deep links",
            "group_rules",
            || async move {
                let port = pick_unused_port()?;
                let (_proxy, admin_state) =
                    ProxyInstance::start_with_admin_sync(port, vec![], false, true)
                        .await
                        .map_err(|e| format!("Failed to start proxy: {}", e))?;
                save_test_sync_token(&admin_state).await?;

                let group_name = "deeplink-name-group";
                let group_id = "grp-deeplink-name";
                {
                    let mut cache = admin_state.group_name_cache();
                    cache.insert(group_id.to_string(), group_name.to_string());
                }

                let group_storage = setup_group_storage(&admin_state, group_name)?;
                let mut rule = RuleFile::new(
                    "deeplink-name-rule",
                    "deeplink-name.example.com status://220",
                );
                rule.enabled = false;
                rule.group = Some(group_name.to_string());
                group_storage
                    .save(&rule)
                    .map_err(|e| format!("Failed to save deeplink rule: {}", e))?;

                let client = build_client()?;
                let list_resp = client
                    .get(format!(
                        "http://127.0.0.1:{}/_bifrost/api/group-rules/{}",
                        port, group_name
                    ))
                    .send()
                    .await
                    .map_err(|e| format!("List by group name failed: {}", e))?;
                assert_status(&list_resp, 200)?;
                let list_json: serde_json::Value = list_resp
                    .json()
                    .await
                    .map_err(|e| format!("Failed to parse list JSON: {}", e))?;
                assert_json_field(&list_json, "group_id", group_id)?;
                assert_json_field(&list_json, "group_name", group_name)?;
                let rules = list_json
                    .get("rules")
                    .and_then(|v| v.as_array())
                    .ok_or("Missing group rules array")?;
                if !rules
                    .iter()
                    .any(|r| r.get("name").and_then(|n| n.as_str()) == Some("deeplink-name-rule"))
                {
                    return Err(format!(
                        "Expected deeplink-name-rule in list, got {rules:?}"
                    ));
                }

                let detail_resp = client
                    .get(format!(
                        "http://127.0.0.1:{}/_bifrost/api/group-rules/{}/deeplink-name-rule",
                        port, group_name
                    ))
                    .send()
                    .await
                    .map_err(|e| format!("Detail by group name failed: {}", e))?;
                assert_status(&detail_resp, 200)?;
                let detail_json: serde_json::Value = detail_resp
                    .json()
                    .await
                    .map_err(|e| format!("Failed to parse detail JSON: {}", e))?;
                assert_json_field(&detail_json, "name", "deeplink-name-rule")?;

                let enable_resp = client
                    .put(format!(
                        "http://127.0.0.1:{}/_bifrost/api/group-rules/{}/deeplink-name-rule/enable",
                        port, group_name
                    ))
                    .send()
                    .await
                    .map_err(|e| format!("Enable by group name failed: {}", e))?;
                assert_status(&enable_resp, 200)?;
                let summary = fetch_active_summary(&client, port).await?;
                assert_active_rule_present(
                    &summary,
                    "deeplink-name-rule",
                    Some(group_id),
                    Some(group_name),
                    Some("status://220"),
                )?;

                cleanup_group_storage(&admin_state, group_name);
                Ok(())
            },
        ),
        TestCase::standalone(
            "group_rules_logout_keeps_group_id_navigation_cache",
            "Logout keeps persisted group id/name mapping so Dynamic Island and injected Badge links keep using the real group id after re-login",
            "group_rules",
            || async move {
                let port = pick_unused_port()?;
                let (_proxy, admin_state) =
                    ProxyInstance::start_with_admin_sync(port, vec![], false, true)
                        .await
                        .map_err(|e| format!("Failed to start proxy: {}", e))?;
                save_test_sync_token(&admin_state).await?;

                let group_name = "logout-relogin-group";
                let group_id = "grp-logout-relogin";
                {
                    let mut cache = admin_state.group_name_cache();
                    cache.insert(group_id.to_string(), group_name.to_string());
                }
                admin_state.persist_group_name_cache();

                let group_storage = setup_group_storage(&admin_state, group_name)?;
                let mut rule = RuleFile::new(
                    "logout-relogin-rule",
                    "logout-relogin.example.com status://221",
                );
                rule.enabled = true;
                rule.group = Some(group_name.to_string());
                group_storage
                    .save(&rule)
                    .map_err(|e| format!("Failed to save logout relogin rule: {}", e))?;
                admin_state.refresh_badge_rules_cache();

                let client = build_client()?;
                let before = fetch_active_summary(&client, port).await?;
                assert_active_rule_present(
                    &before,
                    "logout-relogin-rule",
                    Some(group_id),
                    Some(group_name),
                    Some("status://221"),
                )?;
                assert_badge_navigation_mapping(
                    &admin_state.badge_rules_json(),
                    "logout-relogin-rule",
                    group_name,
                    group_id,
                )?;

                let logout_resp = client
                    .post(format!("http://127.0.0.1:{}/_bifrost/api/sync/logout", port))
                    .send()
                    .await
                    .map_err(|e| format!("Logout request failed: {}", e))?;
                assert_status(&logout_resp, 200)?;

                let after_logout = fetch_active_summary(&client, port).await?;
                if active_summary_has_rule(&after_logout, "logout-relogin-rule")? {
                    return Err("Logged-out active summary must not consume group rules".to_string());
                }
                let after_logout_badge: serde_json::Value =
                    serde_json::from_str(&admin_state.badge_rules_json())
                        .map_err(|e| format!("Failed to parse logged-out badge JSON: {}", e))?;
                if after_logout_badge
                    .get("rules")
                    .and_then(|v| v.as_array())
                    .is_some_and(|rules| {
                        rules.iter().any(|r| {
                            r.get("name").and_then(|n| n.as_str())
                                == Some("logout-relogin-rule")
                        })
                    })
                {
                    return Err("Logged-out badge cache must not list active group rules".to_string());
                }
                {
                    let cache = admin_state.group_name_cache();
                    if cache.reverse_lookup(group_name) != Some(group_id.to_string()) {
                        return Err("Logout should keep persisted group id/name cache".to_string());
                    }
                }

                save_test_sync_token_via_public_callback(&client, port).await?;

                let after = fetch_active_summary(&client, port).await?;
                assert_active_rule_present(
                    &after,
                    "logout-relogin-rule",
                    Some(group_id),
                    Some(group_name),
                    Some("status://221"),
                )?;
                assert_badge_navigation_mapping(
                    &admin_state.badge_rules_json(),
                    "logout-relogin-rule",
                    group_name,
                    group_id,
                )?;

                cleanup_group_storage(&admin_state, group_name);
                Ok(())
            },
        ),
        TestCase::standalone(
            "group_rules_special_chars_in_group_name",
            "Group rules with special characters in group name are sanitized for directory names",
            "group_rules",
            || async move {
                let port = pick_unused_port()?;
                let (_proxy, admin_state) =
                    ProxyInstance::start_with_admin_sync(port, vec![], false, true)
                        .await
                        .map_err(|e| format!("Failed to start proxy: {}", e))?;

                let group_name = "test/group\\with:special\0chars";
                let sanitized = group_name.replace(['/', '\\', '\0', ':'], "_");
                let group_id = "grp-special-001";
                {
                    let mut cache = admin_state.group_name_cache();
                    cache.insert(group_id.to_string(), sanitized.to_string());
                }

                let group_storage = setup_group_storage(&admin_state, &sanitized)?;

                let mut rule =
                    RuleFile::new("special-rule", "special.example.com host://127.0.0.1:6000");
                rule.enabled = true;
                rule.group = Some(sanitized.clone());
                group_storage
                    .save(&rule)
                    .map_err(|e| format!("Failed to save rule: {}", e))?;

                let client = build_client()?;

                let resp = client
                    .get(format!(
                        "http://127.0.0.1:{}/_bifrost/api/group-rules/{}/special-rule",
                        port, group_id
                    ))
                    .send()
                    .await
                    .map_err(|e| format!("Request failed: {}", e))?;

                assert_status(&resp, 200)?;
                let json: serde_json::Value = resp
                    .json()
                    .await
                    .map_err(|e| format!("Failed to parse JSON: {}", e))?;

                assert_json_field(&json, "name", "special-rule")?;
                assert_json_field(&json, "enabled", "true")?;

                cleanup_group_storage(&admin_state, &sanitized);
                Ok(())
            },
        ),
        TestCase::standalone(
            "group_rules_multiple_groups_isolation",
            "Rules from different groups are isolated in separate storage directories",
            "group_rules",
            || async move {
                let port = pick_unused_port()?;
                let (_proxy, admin_state) =
                    ProxyInstance::start_with_admin_sync(port, vec![], false, true)
                        .await
                        .map_err(|e| format!("Failed to start proxy: {}", e))?;
                save_test_sync_token(&admin_state).await?;

                let group_a_name = "test-isolation-group-a";
                let group_a_id = "grp-iso-a";
                let group_b_name = "test-isolation-group-b";
                let group_b_id = "grp-iso-b";
                {
                    let mut cache = admin_state.group_name_cache();
                    cache.insert(group_a_id.to_string(), group_a_name.to_string());
                    cache.insert(group_b_id.to_string(), group_b_name.to_string());
                }

                let storage_a = setup_group_storage(&admin_state, group_a_name)?;
                let storage_b = setup_group_storage(&admin_state, group_b_name)?;

                let mut rule_a =
                    RuleFile::new("shared-name", "a.example.com host://127.0.0.1:3001");
                rule_a.enabled = true;
                rule_a.group = Some(group_a_name.to_string());
                storage_a
                    .save(&rule_a)
                    .map_err(|e| format!("Failed to save rule A: {}", e))?;

                let mut rule_b =
                    RuleFile::new("shared-name", "b.example.com host://127.0.0.1:3002");
                rule_b.enabled = false;
                rule_b.group = Some(group_b_name.to_string());
                storage_b
                    .save(&rule_b)
                    .map_err(|e| format!("Failed to save rule B: {}", e))?;

                let client = build_client()?;

                let resp_a = client
                    .get(format!(
                        "http://127.0.0.1:{}/_bifrost/api/group-rules/{}/shared-name",
                        port, group_a_id
                    ))
                    .send()
                    .await
                    .map_err(|e| format!("Request A failed: {}", e))?;
                assert_status(&resp_a, 200)?;
                let json_a: serde_json::Value = resp_a
                    .json()
                    .await
                    .map_err(|e| format!("Failed to parse JSON A: {}", e))?;
                assert_json_field(&json_a, "enabled", "true")?;
                let content_a = json_a.get("content").and_then(|v| v.as_str()).unwrap_or("");
                if !content_a.contains("a.example.com") {
                    return Err(format!("Group A rule content wrong: {}", content_a));
                }

                let resp_b = client
                    .get(format!(
                        "http://127.0.0.1:{}/_bifrost/api/group-rules/{}/shared-name",
                        port, group_b_id
                    ))
                    .send()
                    .await
                    .map_err(|e| format!("Request B failed: {}", e))?;
                assert_status(&resp_b, 200)?;
                let json_b: serde_json::Value = resp_b
                    .json()
                    .await
                    .map_err(|e| format!("Failed to parse JSON B: {}", e))?;
                assert_json_field(&json_b, "enabled", "false")?;
                let content_b = json_b.get("content").and_then(|v| v.as_str()).unwrap_or("");
                if !content_b.contains("b.example.com") {
                    return Err(format!("Group B rule content wrong: {}", content_b));
                }

                let summary_resp = client
                    .get(format!(
                        "http://127.0.0.1:{}/_bifrost/api/rules/active-summary",
                        port
                    ))
                    .send()
                    .await
                    .map_err(|e| format!("Summary request failed: {}", e))?;
                assert_status(&summary_resp, 200)?;
                let summary_json: serde_json::Value = summary_resp
                    .json()
                    .await
                    .map_err(|e| format!("Failed to parse summary JSON: {}", e))?;
                let summary_rules = summary_json
                    .get("rules")
                    .and_then(|v| v.as_array())
                    .ok_or("Missing 'rules' array")?;

                let group_a_active: Vec<_> = summary_rules
                    .iter()
                    .filter(|r| r.get("group_name").and_then(|n| n.as_str()) == Some(group_a_name))
                    .collect();
                let group_b_active: Vec<_> = summary_rules
                    .iter()
                    .filter(|r| r.get("group_name").and_then(|n| n.as_str()) == Some(group_b_name))
                    .collect();

                if group_a_active.len() != 1 {
                    return Err(format!(
                        "Expected 1 active rule from group A, got {}",
                        group_a_active.len()
                    ));
                }
                if !group_b_active.is_empty() {
                    return Err(format!(
                        "Expected 0 active rules from group B (disabled), got {}",
                        group_b_active.len()
                    ));
                }

                cleanup_group_storage(&admin_state, group_a_name);
                cleanup_group_storage(&admin_state, group_b_name);
                Ok(())
            },
        ),
        TestCase::standalone(
            "group_rules_delete_nonexistent_returns_error",
            "Deleting a nonexistent group rule returns error (404 or 503 without sync)",
            "group_rules",
            || async move {
                let port = pick_unused_port()?;
                let (_proxy, admin_state) =
                    ProxyInstance::start_with_admin_sync(port, vec![], false, true)
                        .await
                        .map_err(|e| format!("Failed to start proxy: {}", e))?;

                let client = build_client()?;

                let resp = client
                    .delete(format!(
                        "http://127.0.0.1:{}/_bifrost/api/group-rules/some-group/nonexistent-rule",
                        port
                    ))
                    .send()
                    .await
                    .map_err(|e| format!("Delete request failed: {}", e))?;

                let status = resp.status().as_u16();
                if status == 200 {
                    return Err("Expected error status for deleting nonexistent rule".to_string());
                }

                let _ = admin_state;
                Ok(())
            },
        ),
        TestCase::standalone(
            "group_rules_update_nonexistent_returns_error",
            "Updating a nonexistent group rule returns error",
            "group_rules",
            || async move {
                let port = pick_unused_port()?;
                let (_proxy, _admin_state) =
                    ProxyInstance::start_with_admin_sync(port, vec![], false, true)
                        .await
                        .map_err(|e| format!("Failed to start proxy: {}", e))?;

                let client = build_client()?;

                let resp = client
                    .put(format!(
                        "http://127.0.0.1:{}/_bifrost/api/group-rules/some-group/nonexistent-rule",
                        port
                    ))
                    .json(&serde_json::json!({ "content": "new content" }))
                    .send()
                    .await
                    .map_err(|e| format!("Update request failed: {}", e))?;

                let status = resp.status().as_u16();
                if status == 200 {
                    return Err("Expected error status for updating nonexistent rule".to_string());
                }

                Ok(())
            },
        ),
        TestCase::standalone(
            "group_rules_create_without_sync_returns_503",
            "Creating a group rule returns 503 when sync manager is not available",
            "group_rules",
            || async move {
                let port = pick_unused_port()?;
                let (_proxy, _admin_state) =
                    ProxyInstance::start_with_admin(port, vec![], false, true)
                        .await
                        .map_err(|e| format!("Failed to start proxy: {}", e))?;

                let client = build_client()?;

                let resp = client
                    .post(format!(
                        "http://127.0.0.1:{}/_bifrost/api/group-rules/some-group",
                        port
                    ))
                    .json(&serde_json::json!({
                        "name": "new-rule",
                        "content": "example.com host://127.0.0.1:3000"
                    }))
                    .send()
                    .await
                    .map_err(|e| format!("Create request failed: {}", e))?;

                assert_status(&resp, 503)?;
                Ok(())
            },
        ),
        TestCase::standalone(
            "group_proxy_method_not_allowed",
            "Group proxy returns 405 for unsupported HTTP methods",
            "group_rules",
            || async move {
                let port = pick_unused_port()?;
                let (_proxy, _admin_state) =
                    ProxyInstance::start_with_admin_sync(port, vec![], false, true)
                        .await
                        .map_err(|e| format!("Failed to start proxy: {}", e))?;

                let client = build_client()?;

                let resp = client
                    .request(
                        reqwest::Method::OPTIONS,
                        format!("http://127.0.0.1:{}/_bifrost/api/group", port),
                    )
                    .send()
                    .await
                    .map_err(|e| format!("OPTIONS request failed: {}", e))?;

                let status = resp.status().as_u16();
                if status != 204 && status != 405 && status != 503 {
                    return Err(format!(
                        "Expected 204 (CORS preflight), 405 or 503, got {}",
                        status
                    ));
                }

                Ok(())
            },
        ),
        TestCase::standalone(
            "group_rules_multiple_rules_in_one_group",
            "Multiple rules within one group are correctly stored and retrieved",
            "group_rules",
            || async move {
                let port = pick_unused_port()?;
                let (_proxy, admin_state) =
                    ProxyInstance::start_with_admin_sync(port, vec![], false, true)
                        .await
                        .map_err(|e| format!("Failed to start proxy: {}", e))?;
                save_test_sync_token(&admin_state).await?;

                let group_name = "test-multi-rules-group";
                let group_id = "grp-multi-001";
                {
                    let mut cache = admin_state.group_name_cache();
                    cache.insert(group_id.to_string(), group_name.to_string());
                }

                let group_storage = setup_group_storage(&admin_state, group_name)?;

                for i in 0..5 {
                    let name = format!("multi-rule-{}", i);
                    let content = format!("multi{}.example.com host://127.0.0.1:{}", i, 3000 + i);
                    let mut rule = RuleFile::new(&name, &content);
                    rule.enabled = i % 2 == 0;
                    rule.group = Some(group_name.to_string());
                    group_storage
                        .save(&rule)
                        .map_err(|e| format!("Failed to save rule {}: {}", i, e))?;
                }

                let client = build_client()?;

                for i in 0..5 {
                    let name = format!("multi-rule-{}", i);
                    let resp = client
                        .get(format!(
                            "http://127.0.0.1:{}/_bifrost/api/group-rules/{}/{}",
                            port, group_id, name
                        ))
                        .send()
                        .await
                        .map_err(|e| format!("Request for rule {} failed: {}", i, e))?;

                    assert_status(&resp, 200)?;
                    let json: serde_json::Value = resp
                        .json()
                        .await
                        .map_err(|e| format!("Failed to parse JSON for rule {}: {}", i, e))?;

                    assert_json_field(&json, "name", &name)?;
                    let expected_enabled = if i % 2 == 0 { "true" } else { "false" };
                    assert_json_field(&json, "enabled", expected_enabled)?;
                }

                let summary_resp = client
                    .get(format!(
                        "http://127.0.0.1:{}/_bifrost/api/rules/active-summary",
                        port
                    ))
                    .send()
                    .await
                    .map_err(|e| format!("Summary request failed: {}", e))?;
                assert_status(&summary_resp, 200)?;
                let summary_json: serde_json::Value = summary_resp
                    .json()
                    .await
                    .map_err(|e| format!("Failed to parse summary JSON: {}", e))?;
                let summary_rules = summary_json
                    .get("rules")
                    .and_then(|v| v.as_array())
                    .ok_or("Missing 'rules' array")?;

                let group_active: Vec<_> = summary_rules
                    .iter()
                    .filter(|r| r.get("group_name").and_then(|n| n.as_str()) == Some(group_name))
                    .collect();

                if group_active.len() != 3 {
                    return Err(format!(
                        "Expected 3 active rules (indices 0,2,4), got {}",
                        group_active.len()
                    ));
                }

                cleanup_group_storage(&admin_state, group_name);
                Ok(())
            },
        ),
    ]
}

fn pick_unused_port() -> Result<u16, String> {
    TcpListener::bind("127.0.0.1:0")
        .map_err(|e| format!("Failed to bind ephemeral port: {}", e))?
        .local_addr()
        .map(|addr| addr.port())
        .map_err(|e| format!("Failed to read ephemeral port: {}", e))
}

async fn fetch_active_summary(
    client: &reqwest::Client,
    port: u16,
) -> Result<serde_json::Value, String> {
    let resp = client
        .get(format!(
            "http://127.0.0.1:{}/_bifrost/api/rules/active-summary",
            port
        ))
        .send()
        .await
        .map_err(|e| format!("Active summary request failed: {}", e))?;
    assert_status(&resp, 200)?;
    resp.json()
        .await
        .map_err(|e| format!("Failed to parse active summary JSON: {}", e))
}

fn active_summary_has_rule(summary: &serde_json::Value, rule_name: &str) -> Result<bool, String> {
    let rules = summary
        .get("rules")
        .and_then(|v| v.as_array())
        .ok_or("Missing 'rules' array")?;
    Ok(rules
        .iter()
        .any(|r| r.get("name").and_then(|n| n.as_str()) == Some(rule_name)))
}

fn assert_active_rule_present(
    summary: &serde_json::Value,
    rule_name: &str,
    expected_group_id: Option<&str>,
    expected_group_name: Option<&str>,
    expected_merged_content: Option<&str>,
) -> Result<(), String> {
    let rules = summary
        .get("rules")
        .and_then(|v| v.as_array())
        .ok_or("Missing 'rules' array")?;
    let active_rule = rules
        .iter()
        .find(|r| r.get("name").and_then(|n| n.as_str()) == Some(rule_name))
        .ok_or_else(|| format!("Expected {rule_name} in active summary, got {rules:?}"))?;

    if active_rule.get("group_id").and_then(|v| v.as_str()) != expected_group_id {
        return Err(format!(
            "Expected group_id {:?}, got {:?}",
            expected_group_id,
            active_rule.get("group_id")
        ));
    }
    if active_rule.get("group_name").and_then(|v| v.as_str()) != expected_group_name {
        return Err(format!(
            "Expected group_name {:?}, got {:?}",
            expected_group_name,
            active_rule.get("group_name")
        ));
    }
    if let Some(expected) = expected_merged_content {
        let merged = summary
            .get("merged_content")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        if !merged.contains(expected) {
            return Err(format!(
                "Expected active summary merged_content to contain '{expected}', got '{merged}'"
            ));
        }
    }
    Ok(())
}

fn badge_has_rule(
    admin_state: &Arc<bifrost_admin::AdminState>,
    rule_name: &str,
    expected_content: &str,
) -> Result<bool, String> {
    let badge: serde_json::Value = serde_json::from_str(&admin_state.badge_rules_json())
        .map_err(|e| format!("Failed to parse badge JSON: {}", e))?;
    let has_rule = badge
        .get("rules")
        .and_then(|v| v.as_array())
        .is_some_and(|rules| {
            rules
                .iter()
                .any(|r| r.get("name").and_then(|n| n.as_str()) == Some(rule_name))
        });
    let merged = badge
        .get("merged_content")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    Ok(has_rule && merged.contains(expected_content))
}

fn assert_badge_navigation_mapping(
    badge_json: &str,
    rule_name: &str,
    expected_local_group_name: &str,
    expected_remote_group_id: &str,
) -> Result<(), String> {
    let parsed: serde_json::Value =
        serde_json::from_str(badge_json).map_err(|e| format!("Invalid badge JSON: {e}"))?;
    let rules = parsed
        .get("rules")
        .and_then(|v| v.as_array())
        .ok_or("Missing badge rules array")?;
    let active_rule = rules
        .iter()
        .find(|r| r.get("name").and_then(|n| n.as_str()) == Some(rule_name))
        .ok_or_else(|| format!("Expected {rule_name} in badge rules, got {rules:?}"))?;

    if active_rule.get("group_id").and_then(|v| v.as_str()) != Some(expected_local_group_name) {
        return Err(format!(
            "Expected badge group_id/local dir {:?}, got {:?}",
            expected_local_group_name,
            active_rule.get("group_id")
        ));
    }
    if active_rule.get("group_name").and_then(|v| v.as_str()) != Some(expected_remote_group_id) {
        return Err(format!(
            "Expected badge group_name/remote id {:?}, got {:?}",
            expected_remote_group_id,
            active_rule.get("group_name")
        ));
    }

    Ok(())
}

async fn wait_for_rule_state(
    client: &reqwest::Client,
    admin_state: &Arc<bifrost_admin::AdminState>,
    port: u16,
    rule_name: &str,
    expected_enabled: bool,
    expected_content: &str,
) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(2);

    loop {
        let summary = fetch_active_summary(client, port).await?;
        let active_has_rule = active_summary_has_rule(&summary, rule_name)?;
        let active_merged = summary
            .get("merged_content")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .contains(expected_content);
        let badge_has_rule = badge_has_rule(admin_state, rule_name, expected_content)?;

        if active_has_rule == expected_enabled
            && active_merged == expected_enabled
            && badge_has_rule == expected_enabled
        {
            return Ok(());
        }

        if Instant::now() >= deadline {
            return Err(format!(
                "Timed out waiting for {rule_name} enabled={expected_enabled}; last_summary={summary:?}; last_badge={}",
                admin_state.badge_rules_json()
            ));
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

fn build_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .no_proxy()
        .build()
        .map_err(|e| format!("Failed to create client: {}", e))
}

async fn save_test_sync_token(admin_state: &Arc<bifrost_admin::AdminState>) -> Result<(), String> {
    let sync_manager = admin_state
        .sync_manager
        .as_ref()
        .ok_or("Expected sync manager for group rule runtime test")?;
    let sync_base_url = std::env::var("BIFROST_E2E_SYNC_BASE_URL").ok();
    let sync_token = std::env::var("BIFROST_E2E_SYNC_TOKEN").ok();

    if let (Some(base_url), Some(token)) = (sync_base_url, sync_token) {
        let base_url = base_url.trim();
        let token = token.trim();
        if !base_url.is_empty() && !token.is_empty() {
            return sync_manager
                .save_login_session(token.to_string(), base_url.to_string())
                .await
                .map(|_| ())
                .map_err(|e| format!("Failed to save test sync session: {}", e));
        }
    }

    sync_manager
        .save_token("bifrost-e2e-group-rule-token".to_string())
        .await
        .map(|_| ())
        .map_err(|e| format!("Failed to save test sync token: {}", e))
}

async fn save_test_sync_token_via_public_callback(
    client: &reqwest::Client,
    port: u16,
) -> Result<(), String> {
    let response = client
        .get(format!(
            "http://127.0.0.1:{}/_bifrost/public/sync-login?token=bifrost-e2e-group-rule-token",
            port
        ))
        .send()
        .await
        .map_err(|e| format!("Public sync login callback failed: {}", e))?;
    assert_status(&response, 200)
}

fn setup_group_storage(
    admin_state: &Arc<bifrost_admin::AdminState>,
    group_name: &str,
) -> Result<RulesStorage, String> {
    let base = admin_state.rules_storage.base_dir();
    let dir = base.join(group_name);
    RulesStorage::with_dir(dir).map_err(|e| format!("Failed to create group rules dir: {}", e))
}

fn cleanup_group_storage(admin_state: &Arc<bifrost_admin::AdminState>, group_name: &str) {
    let base = admin_state.rules_storage.base_dir();
    let dir = base.join(group_name);
    let _ = std::fs::remove_dir_all(dir);
}
