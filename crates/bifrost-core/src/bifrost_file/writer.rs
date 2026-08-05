use super::types::*;

pub struct BifrostFileWriter;

impl BifrostFileWriter {
    fn write_header(file_type: BifrostFileType) -> String {
        format!("{:02} {}", BIFROST_FILE_VERSION, file_type)
    }

    pub fn write_rules(meta: &RuleFileMeta, rules_content: &str) -> String {
        let rule_count = rules_content
            .lines()
            .filter(|l| {
                let trimmed = l.trim();
                !trimmed.is_empty() && !trimmed.starts_with('#')
            })
            .count();

        let mut output = Self::write_header(BifrostFileType::Rules);
        output.push_str("\n\n");

        output.push_str("[meta]\n");
        output.push_str(&format!("name = \"{}\"\n", escape_toml_string(&meta.name)));
        output.push_str(&format!("enabled = {}\n", meta.enabled));
        output.push_str(&format!("sort_order = {}\n", meta.sort_order));
        output.push_str(&format!("version = \"{}\"\n", meta.version));
        output.push_str(&format!("created_at = \"{}\"\n", meta.created_at));
        output.push_str(&format!("updated_at = \"{}\"\n", meta.updated_at));
        if let Some(ref desc) = meta.description {
            output.push_str(&format!("description = \"{}\"\n", escape_toml_string(desc)));
        }
        if let Some(ref group) = meta.group {
            output.push_str(&format!("group = \"{}\"\n", escape_toml_string(group)));
        }
        output.push_str("\n[meta.sync]\n");
        output.push_str(&format!(
            "rule_id = \"{}\"\n",
            escape_toml_string(&meta.sync.rule_id)
        ));
        output.push_str(&format!(
            "status = \"{}\"\n",
            sync_status_to_str(meta.sync.status)
        ));
        if let Some(ref last_synced_at) = meta.sync.last_synced_at {
            output.push_str(&format!(
                "last_synced_at = \"{}\"\n",
                escape_toml_string(last_synced_at)
            ));
        }
        if let Some(ref last_synced_content_hash) = meta.sync.last_synced_content_hash {
            output.push_str(&format!(
                "last_synced_content_hash = \"{}\"\n",
                escape_toml_string(last_synced_content_hash)
            ));
        }
        if let Some(ref remote_id) = meta.sync.remote_id {
            output.push_str(&format!(
                "remote_id = \"{}\"\n",
                escape_toml_string(remote_id)
            ));
        }
        if let Some(ref remote_user_id) = meta.sync.remote_user_id {
            output.push_str(&format!(
                "remote_user_id = \"{}\"\n",
                escape_toml_string(remote_user_id)
            ));
        }
        if let Some(ref remote_created_at) = meta.sync.remote_created_at {
            output.push_str(&format!(
                "remote_created_at = \"{}\"\n",
                escape_toml_string(remote_created_at)
            ));
        }
        if let Some(ref remote_updated_at) = meta.sync.remote_updated_at {
            output.push_str(&format!(
                "remote_updated_at = \"{}\"\n",
                escape_toml_string(remote_updated_at)
            ));
        }

        output.push_str("\n[options]\n");
        output.push_str(&format!("rule_count = {}\n", rule_count));

        output.push_str("\n---\n");
        output.push_str(rules_content);

        output
    }

    pub fn write_network(
        name: &str,
        description: Option<&str>,
        records: &[NetworkRecord],
    ) -> Result<String, serde_json::Error> {
        let mut output = Self::write_header(BifrostFileType::Network);
        output.push_str("\n\n");

        let now = chrono::Utc::now().to_rfc3339();
        output.push_str("[meta]\n");
        output.push_str(&format!("name = \"{}\"\n", escape_toml_string(name)));
        output.push_str("version = \"1.0.0\"\n");
        output.push_str(&format!("created_at = \"{}\"\n", now));
        if let Some(desc) = description {
            output.push_str(&format!("description = \"{}\"\n", escape_toml_string(desc)));
        }

        output.push_str("\n[options]\n");
        output.push_str(&format!("count = {}\n", records.len()));
        output.push_str("include_body = true\n");
        output.push_str("include_response = true\n");

        output.push_str("\n---\n");
        output.push_str(&serde_json::to_string_pretty(records)?);

        Ok(output)
    }

    pub fn write_script(
        name: &str,
        description: Option<&str>,
        scripts: &[ScriptItem],
    ) -> Result<String, serde_json::Error> {
        let mut output = Self::write_header(BifrostFileType::Script);
        output.push_str("\n\n");

        let now = chrono::Utc::now().to_rfc3339();
        output.push_str("[meta]\n");
        output.push_str(&format!("name = \"{}\"\n", escape_toml_string(name)));
        output.push_str("version = \"1.0.0\"\n");
        output.push_str(&format!("created_at = \"{}\"\n", now));
        if let Some(desc) = description {
            output.push_str(&format!("description = \"{}\"\n", escape_toml_string(desc)));
        }

        output.push_str("\n[options]\n");
        output.push_str(&format!("count = {}\n", scripts.len()));

        output.push_str("\n---\n");
        output.push_str(&serde_json::to_string_pretty(scripts)?);

        Ok(output)
    }

    pub fn write_values(
        name: &str,
        description: Option<&str>,
        values: &ValuesContent,
    ) -> Result<String, serde_json::Error> {
        let mut output = Self::write_header(BifrostFileType::Values);
        output.push_str("\n\n");

        let now = chrono::Utc::now().to_rfc3339();
        output.push_str("[meta]\n");
        output.push_str(&format!("name = \"{}\"\n", escape_toml_string(name)));
        output.push_str("version = \"1.0.0\"\n");
        output.push_str(&format!("created_at = \"{}\"\n", now));
        if let Some(desc) = description {
            output.push_str(&format!("description = \"{}\"\n", escape_toml_string(desc)));
        }

        output.push_str("\n[options]\n");
        output.push_str(&format!("count = {}\n", values.len()));

        output.push_str("\n---\n");
        output.push_str(&serde_json::to_string_pretty(values)?);

        Ok(output)
    }

    pub fn write_template(
        name: &str,
        description: Option<&str>,
        template: &TemplateContent,
    ) -> Result<String, serde_json::Error> {
        let mut output = Self::write_header(BifrostFileType::Template);
        output.push_str("\n\n");

        let now = chrono::Utc::now().to_rfc3339();
        output.push_str("[meta]\n");
        output.push_str(&format!("name = \"{}\"\n", escape_toml_string(name)));
        output.push_str("version = \"1.0.0\"\n");
        output.push_str(&format!("created_at = \"{}\"\n", now));
        if let Some(desc) = description {
            output.push_str(&format!("description = \"{}\"\n", escape_toml_string(desc)));
        }

        output.push_str("\n[options]\n");
        output.push_str(&format!("request_count = {}\n", template.requests.len()));
        output.push_str(&format!("group_count = {}\n", template.groups.len()));

        output.push_str("\n---\n");
        output.push_str(&serde_json::to_string_pretty(template)?);

        Ok(output)
    }
}

fn sync_status_to_str(status: RuleSyncStatus) -> &'static str {
    match status {
        RuleSyncStatus::LocalOnly => "local_only",
        RuleSyncStatus::Synced => "synced",
        RuleSyncStatus::Modified => "modified",
    }
}

fn escape_toml_string(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_write_rules() {
        let meta = RuleFileMeta::new("test-rules".to_string());
        let content = "example.com proxy://localhost:3000";

        let output = BifrostFileWriter::write_rules(&meta, content);

        assert!(output.starts_with("01 rules"));
        assert!(output.contains("name = \"test-rules\""));
        assert!(output.contains("enabled = true"));
        assert!(output.contains("---"));
        assert!(output.contains("example.com proxy://localhost:3000"));
    }

    #[test]
    fn test_write_values() {
        let mut values = ValuesContent::new();
        values.insert("key1".to_string(), "value1".to_string());
        values.insert("key2".to_string(), "value2".to_string());

        let output = BifrostFileWriter::write_values("test-values", None, &values).unwrap();

        assert!(output.starts_with("01 values"));
        assert!(output.contains("name = \"test-values\""));
        assert!(output.contains("count = 2"));
        assert!(output.contains("---"));
    }

    #[test]
    fn test_write_network_preserves_routing_diagnostics() {
        let records = vec![NetworkRecord {
            id: "REQ-test".to_string(),
            method: "GET".to_string(),
            url: "https://lf6-cdn2-tos.bytegoofy.com/index.html".to_string(),
            status: 502,
            host: Some("lf6-cdn2-tos.bytegoofy.com".to_string()),
            path: Some("/index.html".to_string()),
            protocol: Some("https".to_string()),
            actual_url: Some("http://10.37.102.138:8081/index.html".to_string()),
            actual_host: Some("10.37.102.138".to_string()),
            listener_port: Some(9900),
            has_rule_hit: Some(true),
            error_message: Some("Request Failed".to_string()),
            client_app: Some("Google Chrome".to_string()),
            client_path: Some("/Applications/Google Chrome.app".to_string()),
            request_headers: None,
            response_headers: None,
            original_response_headers: None,
            request_body: None,
            response_body: None,
            duration_ms: 78,
            timestamp: 1779283635053,
            matched_rules: Some(vec![MatchedRuleExport {
                pattern: "lf6-cdn2-tos.bytegoofy.com/index.html".to_string(),
                protocol: "Host".to_string(),
                value: "10.37.102.138:8081".to_string(),
            }]),
            active_rules: None,
        }];

        let output = BifrostFileWriter::write_network("network-export-test", None, &records)
            .expect("network export should serialize");

        assert!(output.contains("\"actual_url\": \"http://10.37.102.138:8081/index.html\""));
        assert!(output.contains("\"actual_host\": \"10.37.102.138\""));
        assert!(output.contains("\"has_rule_hit\": true"));
        assert!(output.contains("\"error_message\": \"Request Failed\""));
        assert!(output.contains("\"listener_port\": 9900"));
    }

    #[test]
    fn test_write_network_preserves_both_response_header_snapshots() {
        let record_json = r#"{
            "id":"REQ-headers",
            "method":"GET",
            "url":"https://example.test/",
            "status":200,
            "response_headers":[["content-type","application/json"]],
            "original_response_headers":[["content-type","application/json"],["connection","keep-alive"]],
            "duration_ms":1,
            "timestamp":1
        }"#;
        let record: NetworkRecord = serde_json::from_str(record_json).unwrap();

        let output = BifrostFileWriter::write_network("headers", None, &[record]).unwrap();

        assert!(output.contains("\"response_headers\""));
        assert!(output.contains("\"original_response_headers\""));
        assert!(output.contains("\"connection\""));
    }

    #[test]
    fn test_legacy_network_record_omits_original_response_headers() {
        let record_json = r#"{
            "id":"REQ-legacy",
            "method":"GET",
            "url":"https://example.test/",
            "status":200,
            "response_headers":[["connection","keep-alive"]],
            "duration_ms":1,
            "timestamp":1
        }"#;

        let record: NetworkRecord = serde_json::from_str(record_json).unwrap();

        assert!(record.original_response_headers.is_none());
        assert_eq!(
            record.response_headers,
            Some(vec![("connection".to_string(), "keep-alive".to_string())])
        );
    }

    #[test]
    fn test_escape_toml_string() {
        assert_eq!(escape_toml_string("hello"), "hello");
        assert_eq!(escape_toml_string("hello\"world"), "hello\\\"world");
        assert_eq!(escape_toml_string("hello\nworld"), "hello\\nworld");
    }

    #[test]
    fn test_escape_toml_string_all_specials() {
        assert_eq!(escape_toml_string("a\\b"), "a\\\\b");
        assert_eq!(escape_toml_string("a\rb"), "a\\rb");
        assert_eq!(escape_toml_string("a\tb"), "a\\tb");
    }

    #[test]
    fn test_sync_status_to_str_all_variants() {
        assert_eq!(sync_status_to_str(RuleSyncStatus::LocalOnly), "local_only");
        assert_eq!(sync_status_to_str(RuleSyncStatus::Synced), "synced");
        assert_eq!(sync_status_to_str(RuleSyncStatus::Modified), "modified");
    }

    #[test]
    fn test_write_rules_with_all_optional_fields() {
        let mut meta = RuleFileMeta::new("full".to_string());
        meta.description = Some("a desc".to_string());
        meta.group = Some("grp".to_string());
        meta.sync.status = RuleSyncStatus::Synced;
        meta.sync.rule_id = "rid-1".to_string();
        meta.sync.last_synced_at = Some("2026-01-01T00:00:00Z".to_string());
        meta.sync.last_synced_content_hash = Some("hash123".to_string());
        meta.sync.remote_id = Some("remote-1".to_string());
        meta.sync.remote_user_id = Some("user-1".to_string());
        meta.sync.remote_created_at = Some("2026-01-02T00:00:00Z".to_string());
        meta.sync.remote_updated_at = Some("2026-01-03T00:00:00Z".to_string());

        // include a comment line and a blank line to exercise rule_count filtering
        let content = "# comment\n\nexample.com proxy://localhost:3000\nfoo.com block";
        let output = BifrostFileWriter::write_rules(&meta, content);

        assert!(output.contains("description = \"a desc\""));
        assert!(output.contains("group = \"grp\""));
        assert!(output.contains("status = \"synced\""));
        assert!(output.contains("rule_id = \"rid-1\""));
        assert!(output.contains("last_synced_at = \"2026-01-01T00:00:00Z\""));
        assert!(output.contains("last_synced_content_hash = \"hash123\""));
        assert!(output.contains("remote_id = \"remote-1\""));
        assert!(output.contains("remote_user_id = \"user-1\""));
        assert!(output.contains("remote_created_at = \"2026-01-02T00:00:00Z\""));
        assert!(output.contains("remote_updated_at = \"2026-01-03T00:00:00Z\""));
        // 2 non-comment non-empty lines
        assert!(output.contains("rule_count = 2"));
    }

    #[test]
    fn test_write_script() {
        let scripts = vec![ScriptItem {
            name: "s1".to_string(),
            script_type: "request".to_string(),
            description: Some("d".to_string()),
            content: "console.log(1)".to_string(),
        }];
        let output = BifrostFileWriter::write_script("scr", Some("a script"), &scripts).unwrap();
        assert!(output.starts_with("01 script"));
        assert!(output.contains("name = \"scr\""));
        assert!(output.contains("description = \"a script\""));
        assert!(output.contains("count = 1"));
        assert!(output.contains("\"name\": \"s1\""));
    }

    #[test]
    fn test_write_template() {
        let template = TemplateContent {
            groups: vec![ReplayGroupExport {
                id: "g1".to_string(),
                name: "group".to_string(),
                parent_id: None,
                sort_order: 0,
                created_at: 1,
                updated_at: 2,
            }],
            requests: vec![ReplayRequestExport {
                id: "r1".to_string(),
                group_id: Some("g1".to_string()),
                name: Some("req".to_string()),
                request_type: "http".to_string(),
                method: "GET".to_string(),
                url: "https://example.com".to_string(),
                headers: vec![],
                body: None,
                is_saved: true,
                sort_order: 0,
                created_at: 1,
                updated_at: 2,
            }],
        };
        let output =
            BifrostFileWriter::write_template("tpl", Some("a template"), &template).unwrap();
        assert!(output.starts_with("01 template"));
        assert!(output.contains("request_count = 1"));
        assert!(output.contains("group_count = 1"));
        assert!(output.contains("description = \"a template\""));
    }
}
