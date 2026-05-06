use std::path::PathBuf;

use serde::{Deserialize, Deserializer};

use crate::cli::{GroupCommands, GroupRuleCommands};

fn nullable_string<'de, D>(deserializer: D) -> std::result::Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(Option::<String>::deserialize(deserializer)?.unwrap_or_default())
}

const DEFAULT_PORT: u16 = 9900;

fn get_effective_port() -> u16 {
    crate::process::read_runtime_port().unwrap_or(DEFAULT_PORT)
}

fn base_url_for_port(port: u16) -> String {
    format!("http://127.0.0.1:{}/_bifrost/api", port)
}

fn agent() -> ureq::Agent {
    bifrost_core::direct_ureq_agent_builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
}

fn api_error(url: &str, err: ureq::Error) -> bifrost_core::BifrostError {
    bifrost_core::BifrostError::Config(format!(
        "Failed to connect to Bifrost admin API at {url}\n\
         Is the proxy server running?\n\n\
         Hint: Start the proxy with: bifrost start\n\n\
         Error: {err}"
    ))
}

#[derive(Debug, Deserialize)]
struct RemoteResponse<T> {
    #[allow(dead_code)]
    code: i32,
    #[allow(dead_code)]
    message: String,
    data: T,
}

#[derive(Debug, Deserialize)]
struct RemoteListPayload<T> {
    list: Option<Vec<T>>,
    total: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct RemoteGroup {
    #[serde(default, deserialize_with = "nullable_string")]
    id: String,
    #[serde(default, deserialize_with = "nullable_string")]
    name: String,
    #[serde(default, alias = "description", deserialize_with = "nullable_string")]
    what: String,
    visibility: Option<serde_json::Value>,
    #[serde(default, deserialize_with = "nullable_string")]
    create_time: String,
}

#[derive(Debug, Deserialize)]
struct GroupRulesResponse {
    group_id: String,
    group_name: String,
    writable: bool,
    rules: Vec<GroupRuleInfo>,
}

#[derive(Debug, Deserialize)]
struct GroupRuleInfo {
    name: String,
    enabled: bool,
    #[allow(dead_code)]
    sort_order: i32,
    rule_count: usize,
    #[allow(dead_code)]
    created_at: String,
    updated_at: String,
}

#[derive(Debug, Deserialize)]
struct GroupRuleDetail {
    name: String,
    content: String,
    enabled: bool,
    #[allow(dead_code)]
    sort_order: i32,
    created_at: String,
    updated_at: String,
    sync: GroupRuleSyncInfo,
}

#[derive(Debug, Deserialize)]
struct GroupRuleSyncInfo {
    status: String,
    remote_id: Option<String>,
    #[allow(dead_code)]
    remote_updated_at: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SuccessResponse {
    #[allow(dead_code)]
    success: Option<bool>,
    message: Option<String>,
}

pub fn handle_group_command(action: GroupCommands) -> bifrost_core::Result<()> {
    let port = get_effective_port();
    handle_group_command_with_port(action, port)
}

fn handle_group_command_with_port(action: GroupCommands, port: u16) -> bifrost_core::Result<()> {
    match action {
        GroupCommands::List {
            keyword,
            limit,
            offset,
        } => handle_group_list(port, keyword, limit, offset),
        GroupCommands::Show { group_id } => handle_group_show(port, &group_id),
        GroupCommands::Rule { action } => handle_group_rule_command_with_port(action, port),
    }
}

fn handle_group_list(
    port: u16,
    keyword: Option<String>,
    limit: usize,
    offset: usize,
) -> bifrost_core::Result<()> {
    let mut url = format!(
        "{}/group?offset={}&limit={}",
        base_url_for_port(port),
        offset,
        limit
    );
    if let Some(ref kw) = keyword {
        url.push_str(&format!("&keyword={}", urlencoding::encode(kw)));
    }

    let resp = agent().get(&url).call().map_err(|e| api_error(&url, e))?;
    let body: RemoteResponse<RemoteListPayload<RemoteGroup>> = resp.into_json().map_err(|e| {
        bifrost_core::BifrostError::Config(format!("Failed to parse response: {e}"))
    })?;

    let groups = body.data.list.unwrap_or_default();
    let total = body.data.total.unwrap_or(0);

    if groups.is_empty() {
        println!("No groups found.");
        return Ok(());
    }

    println!("Groups ({}/{}):", groups.len(), total);
    for g in &groups {
        let vis = match &g.visibility {
            Some(serde_json::Value::Number(n)) if n.as_i64() == Some(1) => "public",
            Some(serde_json::Value::String(s)) if s == "public" => "public",
            _ => "private",
        };
        let desc = if g.what.is_empty() {
            String::new()
        } else {
            format!(" - {}", g.what)
        };
        println!("  {} {} [{}]{}", g.id, g.name, vis, desc);
    }

    let current_page = offset / limit + 1;
    let total_pages = if total == 0 {
        1
    } else {
        (total as usize).div_ceil(limit)
    };
    println!();
    println!(
        "Page {}/{} (total: {}, limit: {}, offset: {})",
        current_page, total_pages, total, limit, offset
    );
    if offset + groups.len() < total as usize {
        let next_offset = offset + limit;
        let mut hint = format!(
            "View next page: bifrost group list --offset {} --limit {}",
            next_offset, limit
        );
        if let Some(ref kw) = keyword {
            hint.push_str(&format!(" --keyword {}", kw));
        }
        println!("{}", hint);
    }

    Ok(())
}

fn handle_group_show(port: u16, group_id: &str) -> bifrost_core::Result<()> {
    let url = format!(
        "{}/group/{}",
        base_url_for_port(port),
        urlencoding::encode(group_id)
    );
    let resp = agent().get(&url).call().map_err(|e| api_error(&url, e))?;
    let body: RemoteResponse<RemoteGroup> = resp.into_json().map_err(|e| {
        bifrost_core::BifrostError::Config(format!("Failed to parse response: {e}"))
    })?;

    let g = body.data;
    let vis = match &g.visibility {
        Some(serde_json::Value::Number(n)) if n.as_i64() == Some(1) => "public",
        Some(serde_json::Value::String(s)) if s == "public" => "public",
        _ => "private",
    };
    println!("Group: {}", g.name);
    println!("  ID: {}", g.id);
    println!("  Visibility: {}", vis);
    if !g.what.is_empty() {
        println!("  Description: {}", g.what);
    }
    println!("  Created: {}", g.create_time);

    Ok(())
}

fn handle_group_rule_command_with_port(
    action: GroupRuleCommands,
    port: u16,
) -> bifrost_core::Result<()> {
    match action {
        GroupRuleCommands::List { group_id } => handle_group_rule_list(port, &group_id),
        GroupRuleCommands::Show { group_id, name } => {
            handle_group_rule_show(port, &group_id, &name)
        }
        GroupRuleCommands::Add {
            group_id,
            name,
            content,
            file,
        } => handle_group_rule_add(port, &group_id, &name, content, file),
        GroupRuleCommands::Update {
            group_id,
            name,
            content,
            file,
        } => handle_group_rule_update(port, &group_id, &name, content, file),
        GroupRuleCommands::Delete { group_id, name } => {
            handle_group_rule_delete(port, &group_id, &name)
        }
        GroupRuleCommands::Enable { group_id, name } => {
            handle_group_rule_toggle(port, &group_id, &name, true)
        }
        GroupRuleCommands::Disable { group_id, name } => {
            handle_group_rule_toggle(port, &group_id, &name, false)
        }
    }
}

fn handle_group_rule_list(port: u16, group_id: &str) -> bifrost_core::Result<()> {
    let url = format!(
        "{}/group-rules/{}",
        base_url_for_port(port),
        urlencoding::encode(group_id)
    );
    let resp = agent().get(&url).call().map_err(|e| api_error(&url, e))?;
    let body: GroupRulesResponse = resp.into_json().map_err(|e| {
        bifrost_core::BifrostError::Config(format!("Failed to parse response: {e}"))
    })?;

    println!("Group: {} ({})", body.group_name, body.group_id);
    println!("Writable: {}", if body.writable { "yes" } else { "no" });
    println!();

    if body.rules.is_empty() {
        println!("No rules found.");
        return Ok(());
    }

    println!("Rules ({}):", body.rules.len());
    for r in &body.rules {
        let status = if r.enabled { "enabled" } else { "disabled" };
        println!(
            "  {} [{}] ({} rules, updated: {})",
            r.name, status, r.rule_count, r.updated_at
        );
    }

    Ok(())
}

fn handle_group_rule_show(port: u16, group_id: &str, rule_name: &str) -> bifrost_core::Result<()> {
    let url = format!(
        "{}/group-rules/{}/{}",
        base_url_for_port(port),
        urlencoding::encode(group_id),
        urlencoding::encode(rule_name)
    );
    let resp = agent().get(&url).call().map_err(|e| api_error(&url, e))?;
    let detail: GroupRuleDetail = resp.into_json().map_err(|e| {
        bifrost_core::BifrostError::Config(format!("Failed to parse response: {e}"))
    })?;

    println!("Rule: {}", detail.name);
    println!(
        "Status: {}",
        if detail.enabled {
            "enabled"
        } else {
            "disabled"
        }
    );
    println!("Sync: {}", detail.sync.status);
    if let Some(ref rid) = detail.sync.remote_id {
        println!("Remote ID: {}", rid);
    }
    println!("Created: {}", detail.created_at);
    println!("Updated: {}", detail.updated_at);
    println!("Content:");
    println!("{}", detail.content);

    Ok(())
}

fn load_rule_content(
    content: Option<String>,
    file: Option<PathBuf>,
) -> bifrost_core::Result<String> {
    if let Some(c) = content {
        Ok(c)
    } else if let Some(path) = file {
        Ok(std::fs::read_to_string(&path)?)
    } else {
        Err(bifrost_core::BifrostError::Config(
            "Either --content or --file must be provided".to_string(),
        ))
    }
}

fn handle_group_rule_add(
    port: u16,
    group_id: &str,
    name: &str,
    content: Option<String>,
    file: Option<PathBuf>,
) -> bifrost_core::Result<()> {
    let rule_content =
        content.or_else(|| file.as_ref().and_then(|p| std::fs::read_to_string(p).ok()));

    let url = format!(
        "{}/group-rules/{}",
        base_url_for_port(port),
        urlencoding::encode(group_id)
    );
    let body = serde_json::json!({
        "name": name,
        "content": rule_content.unwrap_or_default(),
    });

    let resp = agent()
        .post(&url)
        .send_json(&body)
        .map_err(|e| api_error(&url, e))?;
    let detail: GroupRuleDetail = resp.into_json().map_err(|e| {
        bifrost_core::BifrostError::Config(format!("Failed to parse response: {e}"))
    })?;

    println!("Rule '{}' added to group successfully.", detail.name);
    Ok(())
}

fn handle_group_rule_update(
    port: u16,
    group_id: &str,
    name: &str,
    content: Option<String>,
    file: Option<PathBuf>,
) -> bifrost_core::Result<()> {
    let rule_content = load_rule_content(content, file)?;

    let url = format!(
        "{}/group-rules/{}/{}",
        base_url_for_port(port),
        urlencoding::encode(group_id),
        urlencoding::encode(name)
    );
    let body = serde_json::json!({
        "content": rule_content,
    });

    let resp = agent()
        .put(&url)
        .send_json(&body)
        .map_err(|e| api_error(&url, e))?;
    let detail: GroupRuleDetail = resp.into_json().map_err(|e| {
        bifrost_core::BifrostError::Config(format!("Failed to parse response: {e}"))
    })?;

    println!("Rule '{}' updated successfully.", detail.name);
    Ok(())
}

fn handle_group_rule_delete(port: u16, group_id: &str, name: &str) -> bifrost_core::Result<()> {
    let url = format!(
        "{}/group-rules/{}/{}",
        base_url_for_port(port),
        urlencoding::encode(group_id),
        urlencoding::encode(name)
    );

    let resp = agent()
        .delete(&url)
        .call()
        .map_err(|e| api_error(&url, e))?;
    let body: SuccessResponse = resp.into_json().map_err(|e| {
        bifrost_core::BifrostError::Config(format!("Failed to parse response: {e}"))
    })?;

    println!(
        "{}",
        body.message
            .unwrap_or_else(|| format!("Rule '{name}' deleted successfully."))
    );
    Ok(())
}

fn handle_group_rule_toggle(
    port: u16,
    group_id: &str,
    name: &str,
    enabled: bool,
) -> bifrost_core::Result<()> {
    let action = if enabled { "enable" } else { "disable" };
    let url = format!(
        "{}/group-rules/{}/{}/{}",
        base_url_for_port(port),
        urlencoding::encode(group_id),
        urlencoding::encode(name),
        action
    );

    let resp = agent()
        .put(&url)
        .send_bytes(&[])
        .map_err(|e| api_error(&url, e))?;
    let body: SuccessResponse = resp.into_json().map_err(|e| {
        bifrost_core::BifrostError::Config(format!("Failed to parse response: {e}"))
    })?;

    println!(
        "{}",
        body.message
            .unwrap_or_else(|| format!("Rule '{name}' {}d successfully.", action))
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::{GroupCommands, GroupRuleCommands};
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::{mpsc, Mutex, MutexGuard, OnceLock};

    static MOCK_SERVER_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

    struct MockServer {
        port: u16,
        shutdown_tx: mpsc::Sender<()>,
        handle: Option<std::thread::JoinHandle<Vec<String>>>,
        _guard: MutexGuard<'static, ()>,
    }

    impl MockServer {
        fn start(responses: Vec<(u16, &str)>) -> Self {
            let guard = MOCK_SERVER_LOCK
                .get_or_init(|| Mutex::new(()))
                .lock()
                .expect("group command mock server lock poisoned");
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let port = listener.local_addr().unwrap().port();
            let (shutdown_tx, shutdown_rx) = mpsc::channel::<()>();

            let owned_responses: Vec<(u16, String)> = responses
                .into_iter()
                .map(|(code, body)| (code, body.to_string()))
                .collect();

            let handle = std::thread::spawn(move || {
                listener
                    .set_nonblocking(true)
                    .expect("set_nonblocking failed");

                let mut request_log: Vec<String> = Vec::new();
                let mut resp_idx = 0;

                loop {
                    if shutdown_rx.try_recv().is_ok() {
                        break;
                    }

                    match listener.accept() {
                        Ok((mut stream, _)) => {
                            stream
                                .set_nonblocking(false)
                                .expect("set stream blocking failed");
                            stream
                                .set_read_timeout(Some(std::time::Duration::from_secs(2)))
                                .ok();

                            let mut buf = [0u8; 8192];
                            let n = stream.read(&mut buf).unwrap_or(0);
                            let request = String::from_utf8_lossy(&buf[..n]).to_string();

                            let first_line = request.lines().next().unwrap_or("").to_string();
                            request_log.push(first_line);

                            let (status_code, body) = if resp_idx < owned_responses.len() {
                                let r = &owned_responses[resp_idx];
                                resp_idx += 1;
                                (r.0, r.1.as_str())
                            } else {
                                (200, r#"{"success":true}"#)
                            };

                            let status_text = match status_code {
                                200 => "OK",
                                404 => "Not Found",
                                500 => "Internal Server Error",
                                _ => "OK",
                            };

                            let response = format!(
                                "HTTP/1.1 {} {}\r\n\
                                 Content-Type: application/json\r\n\
                                 Content-Length: {}\r\n\
                                 Connection: close\r\n\
                                 \r\n\
                                 {}",
                                status_code,
                                status_text,
                                body.len(),
                                body
                            );
                            let _ = stream.write_all(response.as_bytes());
                            let _ = stream.flush();
                        }
                        Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(std::time::Duration::from_millis(10));
                        }
                        Err(_) => break,
                    }
                }

                request_log
            });

            MockServer {
                port,
                shutdown_tx,
                handle: Some(handle),
                _guard: guard,
            }
        }

        fn stop(mut self) -> Vec<String> {
            let _ = self.shutdown_tx.send(());
            if let Some(h) = self.handle.take() {
                let _ = agent()
                    .get(&format!("http://127.0.0.1:{}/shutdown", self.port))
                    .call();
                std::thread::sleep(std::time::Duration::from_millis(50));
                h.join().unwrap_or_default()
            } else {
                vec![]
            }
        }
    }

    impl Drop for MockServer {
        fn drop(&mut self) {
            let _ = self.shutdown_tx.send(());
        }
    }

    fn mock_port_for_admin(port: u16) -> u16 {
        port
    }

    #[test]
    fn test_group_list_success() {
        let json = r#"{
            "code": 200,
            "message": "ok",
            "data": {
                "list": [
                    {"id":"g1","name":"Team Alpha","what":"Dev team","visibility":1,"create_time":"2024-01-01T00:00:00Z"},
                    {"id":"g2","name":"Team Beta","what":"","visibility":0,"create_time":"2024-02-01T00:00:00Z"}
                ],
                "total": 2
            }
        }"#;

        let server = MockServer::start(vec![(200, json)]);
        let port = mock_port_for_admin(server.port);

        let result = handle_group_command_with_port(
            GroupCommands::List {
                keyword: None,
                limit: 50,
                offset: 0,
            },
            port,
        );
        assert!(result.is_ok());

        let logs = server.stop();
        assert!(logs[0].contains("GET"));
        assert!(logs[0].contains("/group?offset=0&limit=50"));
    }

    #[test]
    fn test_group_list_with_keyword() {
        let json = r#"{
            "code": 200,
            "message": "ok",
            "data": {
                "list": [{"id":"g1","name":"Team Alpha","what":"","visibility":1,"create_time":"2024-01-01T00:00:00Z"}],
                "total": 1
            }
        }"#;

        let server = MockServer::start(vec![(200, json)]);
        let port = mock_port_for_admin(server.port);

        let result = handle_group_command_with_port(
            GroupCommands::List {
                keyword: Some("Alpha".to_string()),
                limit: 10,
                offset: 0,
            },
            port,
        );
        assert!(result.is_ok());

        let logs = server.stop();
        assert!(logs[0].contains("keyword=Alpha"));
        assert!(logs[0].contains("limit=10"));
    }

    #[test]
    fn test_group_list_empty() {
        let json = r#"{"code":200,"message":"ok","data":{"list":[],"total":0}}"#;

        let server = MockServer::start(vec![(200, json)]);
        let port = mock_port_for_admin(server.port);

        let result = handle_group_command_with_port(
            GroupCommands::List {
                keyword: None,
                limit: 50,
                offset: 0,
            },
            port,
        );
        assert!(result.is_ok());
        server.stop();
    }

    #[test]
    fn test_group_list_pagination_first_page() {
        let json = r#"{
            "code": 200,
            "message": "ok",
            "data": {
                "list": [
                    {"id":"g1","name":"Team A","what":"","visibility":1,"create_time":"2024-01-01T00:00:00Z"},
                    {"id":"g2","name":"Team B","what":"","visibility":0,"create_time":"2024-02-01T00:00:00Z"}
                ],
                "total": 5
            }
        }"#;

        let server = MockServer::start(vec![(200, json)]);
        let port = mock_port_for_admin(server.port);

        let result = handle_group_command_with_port(
            GroupCommands::List {
                keyword: None,
                limit: 2,
                offset: 0,
            },
            port,
        );
        assert!(result.is_ok());

        let logs = server.stop();
        assert!(logs[0].contains("offset=0"));
        assert!(logs[0].contains("limit=2"));
    }

    #[test]
    fn test_group_list_pagination_middle_page() {
        let json = r#"{
            "code": 200,
            "message": "ok",
            "data": {
                "list": [
                    {"id":"g3","name":"Team C","what":"","visibility":1,"create_time":"2024-03-01T00:00:00Z"},
                    {"id":"g4","name":"Team D","what":"","visibility":0,"create_time":"2024-04-01T00:00:00Z"}
                ],
                "total": 5
            }
        }"#;

        let server = MockServer::start(vec![(200, json)]);
        let port = mock_port_for_admin(server.port);

        let result = handle_group_command_with_port(
            GroupCommands::List {
                keyword: None,
                limit: 2,
                offset: 2,
            },
            port,
        );
        assert!(result.is_ok());

        let logs = server.stop();
        assert!(logs[0].contains("offset=2"));
        assert!(logs[0].contains("limit=2"));
    }

    #[test]
    fn test_group_list_pagination_last_page() {
        let json = r#"{
            "code": 200,
            "message": "ok",
            "data": {
                "list": [
                    {"id":"g5","name":"Team E","what":"","visibility":1,"create_time":"2024-05-01T00:00:00Z"}
                ],
                "total": 5
            }
        }"#;

        let server = MockServer::start(vec![(200, json)]);
        let port = mock_port_for_admin(server.port);

        let result = handle_group_command_with_port(
            GroupCommands::List {
                keyword: None,
                limit: 2,
                offset: 4,
            },
            port,
        );
        assert!(result.is_ok());

        let logs = server.stop();
        assert!(logs[0].contains("offset=4"));
        assert!(logs[0].contains("limit=2"));
    }

    #[test]
    fn test_group_list_pagination_with_keyword() {
        let json = r#"{
            "code": 200,
            "message": "ok",
            "data": {
                "list": [
                    {"id":"g3","name":"Alpha 3","what":"","visibility":1,"create_time":"2024-03-01T00:00:00Z"}
                ],
                "total": 3
            }
        }"#;

        let server = MockServer::start(vec![(200, json)]);
        let port = mock_port_for_admin(server.port);

        let result = handle_group_command_with_port(
            GroupCommands::List {
                keyword: Some("Alpha".to_string()),
                limit: 1,
                offset: 2,
            },
            port,
        );
        assert!(result.is_ok());

        let logs = server.stop();
        assert!(logs[0].contains("offset=2"));
        assert!(logs[0].contains("limit=1"));
        assert!(logs[0].contains("keyword=Alpha"));
    }

    #[test]
    fn test_group_list_pagination_beyond_total() {
        let json = r#"{
            "code": 200,
            "message": "ok",
            "data": {
                "list": [],
                "total": 3
            }
        }"#;

        let server = MockServer::start(vec![(200, json)]);
        let port = mock_port_for_admin(server.port);

        let result = handle_group_command_with_port(
            GroupCommands::List {
                keyword: None,
                limit: 2,
                offset: 10,
            },
            port,
        );
        assert!(result.is_ok());
        server.stop();
    }

    #[test]
    fn test_group_show_success() {
        let json = r#"{
            "code": 200,
            "message": "ok",
            "data": {"id":"g1","name":"Team Alpha","what":"A dev team","visibility":1,"create_time":"2024-01-01T00:00:00Z"}
        }"#;

        let server = MockServer::start(vec![(200, json)]);
        let port = mock_port_for_admin(server.port);

        let result = handle_group_command_with_port(
            GroupCommands::Show {
                group_id: "g1".to_string(),
            },
            port,
        );
        assert!(result.is_ok());

        let logs = server.stop();
        assert!(logs[0].contains("GET"));
        assert!(logs[0].contains("/group/g1"));
    }

    #[test]
    fn test_group_rule_list_success() {
        let json = r#"{
            "group_id": "g1",
            "group_name": "Team Alpha",
            "writable": true,
            "rules": [
                {"name":"rule1","enabled":true,"sort_order":0,"rule_count":5,"created_at":"2024-01-01T00:00:00Z","updated_at":"2024-06-01T00:00:00Z"},
                {"name":"rule2","enabled":false,"sort_order":1,"rule_count":3,"created_at":"2024-02-01T00:00:00Z","updated_at":"2024-06-02T00:00:00Z"}
            ]
        }"#;

        let server = MockServer::start(vec![(200, json)]);
        let port = mock_port_for_admin(server.port);

        let result = handle_group_command_with_port(
            GroupCommands::Rule {
                action: GroupRuleCommands::List {
                    group_id: "g1".to_string(),
                },
            },
            port,
        );
        assert!(result.is_ok());

        let logs = server.stop();
        assert!(logs[0].contains("GET"));
        assert!(logs[0].contains("/group-rules/g1"));
    }

    #[test]
    fn test_group_rule_list_empty() {
        let json = r#"{
            "group_id": "g1",
            "group_name": "Team Alpha",
            "writable": false,
            "rules": []
        }"#;

        let server = MockServer::start(vec![(200, json)]);
        let port = mock_port_for_admin(server.port);

        let result = handle_group_command_with_port(
            GroupCommands::Rule {
                action: GroupRuleCommands::List {
                    group_id: "g1".to_string(),
                },
            },
            port,
        );
        assert!(result.is_ok());
        server.stop();
    }

    #[test]
    fn test_group_rule_show_success() {
        let json = r#"{
            "name": "my-rule",
            "content": "example.com host://127.0.0.1:3000",
            "enabled": true,
            "sort_order": 0,
            "created_at": "2024-01-01T00:00:00Z",
            "updated_at": "2024-06-01T00:00:00Z",
            "sync": {
                "status": "synced",
                "remote_id": "env-123",
                "remote_updated_at": "2024-06-01T00:00:00Z"
            }
        }"#;

        let server = MockServer::start(vec![(200, json)]);
        let port = mock_port_for_admin(server.port);

        let result = handle_group_command_with_port(
            GroupCommands::Rule {
                action: GroupRuleCommands::Show {
                    group_id: "g1".to_string(),
                    name: "my-rule".to_string(),
                },
            },
            port,
        );
        assert!(result.is_ok());

        let logs = server.stop();
        assert!(logs[0].contains("GET"));
        assert!(logs[0].contains("/group-rules/g1/my-rule"));
    }

    #[test]
    fn test_group_rule_show_disabled_rule() {
        let json = r#"{
            "name": "disabled-rule",
            "content": "",
            "enabled": false,
            "sort_order": 0,
            "created_at": "2024-01-01T00:00:00Z",
            "updated_at": "2024-06-01T00:00:00Z",
            "sync": {
                "status": "local_only",
                "remote_id": null,
                "remote_updated_at": null
            }
        }"#;

        let server = MockServer::start(vec![(200, json)]);
        let port = mock_port_for_admin(server.port);

        let result = handle_group_command_with_port(
            GroupCommands::Rule {
                action: GroupRuleCommands::Show {
                    group_id: "g1".to_string(),
                    name: "disabled-rule".to_string(),
                },
            },
            port,
        );
        assert!(result.is_ok());
        server.stop();
    }

    #[test]
    fn test_group_rule_add_with_content() {
        let json = r#"{
            "name": "new-rule",
            "content": "*.example.com host://localhost:8080",
            "enabled": true,
            "sort_order": 0,
            "created_at": "2024-06-01T00:00:00Z",
            "updated_at": "2024-06-01T00:00:00Z",
            "sync": {"status":"synced","remote_id":"env-456","remote_updated_at":"2024-06-01T00:00:00Z"}
        }"#;

        let server = MockServer::start(vec![(200, json)]);
        let port = mock_port_for_admin(server.port);

        let result = handle_group_command_with_port(
            GroupCommands::Rule {
                action: GroupRuleCommands::Add {
                    group_id: "g1".to_string(),
                    name: "new-rule".to_string(),
                    content: Some("*.example.com host://localhost:8080".to_string()),
                    file: None,
                },
            },
            port,
        );
        assert!(result.is_ok());

        let logs = server.stop();
        assert!(logs[0].contains("POST"));
        assert!(logs[0].contains("/group-rules/g1"));
    }

    #[test]
    fn test_group_rule_add_with_file() {
        let json = r#"{
            "name": "file-rule",
            "content": "file-content-here",
            "enabled": true,
            "sort_order": 0,
            "created_at": "2024-06-01T00:00:00Z",
            "updated_at": "2024-06-01T00:00:00Z",
            "sync": {"status":"synced","remote_id":"env-789","remote_updated_at":"2024-06-01T00:00:00Z"}
        }"#;

        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), "file-content-here").unwrap();

        let server = MockServer::start(vec![(200, json)]);
        let port = mock_port_for_admin(server.port);

        let result = handle_group_command_with_port(
            GroupCommands::Rule {
                action: GroupRuleCommands::Add {
                    group_id: "g1".to_string(),
                    name: "file-rule".to_string(),
                    content: None,
                    file: Some(tmp.path().to_path_buf()),
                },
            },
            port,
        );
        assert!(result.is_ok());

        let logs = server.stop();
        assert!(logs[0].contains("POST"));
    }

    #[test]
    fn test_group_rule_add_empty_content() {
        let json = r#"{
            "name": "empty-rule",
            "content": "",
            "enabled": true,
            "sort_order": 0,
            "created_at": "2024-06-01T00:00:00Z",
            "updated_at": "2024-06-01T00:00:00Z",
            "sync": {"status":"local_only","remote_id":null,"remote_updated_at":null}
        }"#;

        let server = MockServer::start(vec![(200, json)]);
        let port = mock_port_for_admin(server.port);

        let result = handle_group_command_with_port(
            GroupCommands::Rule {
                action: GroupRuleCommands::Add {
                    group_id: "g1".to_string(),
                    name: "empty-rule".to_string(),
                    content: None,
                    file: None,
                },
            },
            port,
        );
        assert!(result.is_ok());
        server.stop();
    }

    #[test]
    fn test_group_rule_update_with_content() {
        let json = r#"{
            "name": "my-rule",
            "content": "updated content",
            "enabled": true,
            "sort_order": 0,
            "created_at": "2024-01-01T00:00:00Z",
            "updated_at": "2024-06-15T00:00:00Z",
            "sync": {"status":"synced","remote_id":"env-123","remote_updated_at":"2024-06-15T00:00:00Z"}
        }"#;

        let server = MockServer::start(vec![(200, json)]);
        let port = mock_port_for_admin(server.port);

        let result = handle_group_command_with_port(
            GroupCommands::Rule {
                action: GroupRuleCommands::Update {
                    group_id: "g1".to_string(),
                    name: "my-rule".to_string(),
                    content: Some("updated content".to_string()),
                    file: None,
                },
            },
            port,
        );
        assert!(result.is_ok());

        let logs = server.stop();
        assert!(logs[0].contains("PUT"));
        assert!(logs[0].contains("/group-rules/g1/my-rule"));
    }

    #[test]
    fn test_group_rule_update_with_file() {
        let json = r#"{
            "name": "my-rule",
            "content": "content from file",
            "enabled": true,
            "sort_order": 0,
            "created_at": "2024-01-01T00:00:00Z",
            "updated_at": "2024-06-15T00:00:00Z",
            "sync": {"status":"synced","remote_id":"env-123","remote_updated_at":"2024-06-15T00:00:00Z"}
        }"#;

        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), "content from file").unwrap();

        let server = MockServer::start(vec![(200, json)]);
        let port = mock_port_for_admin(server.port);

        let result = handle_group_command_with_port(
            GroupCommands::Rule {
                action: GroupRuleCommands::Update {
                    group_id: "g1".to_string(),
                    name: "my-rule".to_string(),
                    content: None,
                    file: Some(tmp.path().to_path_buf()),
                },
            },
            port,
        );
        assert!(result.is_ok());

        let logs = server.stop();
        assert!(logs[0].contains("PUT"));
    }

    #[test]
    fn test_group_rule_update_requires_content() {
        let result = handle_group_command_with_port(
            GroupCommands::Rule {
                action: GroupRuleCommands::Update {
                    group_id: "g1".to_string(),
                    name: "my-rule".to_string(),
                    content: None,
                    file: None,
                },
            },
            12345,
        );
        assert!(result.is_err());
        let err_msg = format!("{}", result.unwrap_err());
        assert!(err_msg.contains("--content"));
    }

    #[test]
    fn test_group_rule_delete_success() {
        let json = r#"{"success":true,"message":"Rule 'my-rule' deleted successfully."}"#;

        let server = MockServer::start(vec![(200, json)]);
        let port = mock_port_for_admin(server.port);

        let result = handle_group_command_with_port(
            GroupCommands::Rule {
                action: GroupRuleCommands::Delete {
                    group_id: "g1".to_string(),
                    name: "my-rule".to_string(),
                },
            },
            port,
        );
        assert!(result.is_ok());

        let logs = server.stop();
        assert!(logs[0].contains("DELETE"));
        assert!(logs[0].contains("/group-rules/g1/my-rule"));
    }

    #[test]
    fn test_group_rule_enable_success() {
        let json = r#"{"success":true,"message":"Rule 'my-rule' enabled"}"#;

        let server = MockServer::start(vec![(200, json)]);
        let port = mock_port_for_admin(server.port);

        let result = handle_group_command_with_port(
            GroupCommands::Rule {
                action: GroupRuleCommands::Enable {
                    group_id: "g1".to_string(),
                    name: "my-rule".to_string(),
                },
            },
            port,
        );
        assert!(result.is_ok());

        let logs = server.stop();
        assert!(logs[0].contains("PUT"));
        assert!(logs[0].contains("/group-rules/g1/my-rule/enable"));
    }

    #[test]
    fn test_group_rule_disable_success() {
        let json = r#"{"success":true,"message":"Rule 'my-rule' disabled"}"#;

        let server = MockServer::start(vec![(200, json)]);
        let port = mock_port_for_admin(server.port);

        let result = handle_group_command_with_port(
            GroupCommands::Rule {
                action: GroupRuleCommands::Disable {
                    group_id: "g1".to_string(),
                    name: "my-rule".to_string(),
                },
            },
            port,
        );
        assert!(result.is_ok());

        let logs = server.stop();
        assert!(logs[0].contains("PUT"));
        assert!(logs[0].contains("/group-rules/g1/my-rule/disable"));
    }

    #[test]
    fn test_group_list_server_not_running() {
        let result = handle_group_command_with_port(
            GroupCommands::List {
                keyword: None,
                limit: 50,
                offset: 0,
            },
            19999,
        );
        assert!(result.is_err());
        let err_msg = format!("{}", result.unwrap_err());
        assert!(err_msg.contains("Failed to connect"));
    }

    #[test]
    fn test_group_show_private_group() {
        let json = r#"{
            "code": 200,
            "message": "ok",
            "data": {"id":"g3","name":"Private Group","what":"Secret","visibility":0,"create_time":"2024-03-01T00:00:00Z"}
        }"#;

        let server = MockServer::start(vec![(200, json)]);
        let port = mock_port_for_admin(server.port);

        let result = handle_group_command_with_port(
            GroupCommands::Show {
                group_id: "g3".to_string(),
            },
            port,
        );
        assert!(result.is_ok());
        server.stop();
    }

    #[test]
    fn test_group_rule_toggle_default_message() {
        let json = r#"{"success":true}"#;

        let server = MockServer::start(vec![(200, json)]);
        let port = mock_port_for_admin(server.port);

        let result = handle_group_command_with_port(
            GroupCommands::Rule {
                action: GroupRuleCommands::Enable {
                    group_id: "g1".to_string(),
                    name: "test-rule".to_string(),
                },
            },
            port,
        );
        assert!(result.is_ok());
        server.stop();
    }

    #[test]
    fn test_group_rule_delete_default_message() {
        let json = r#"{"success":true}"#;

        let server = MockServer::start(vec![(200, json)]);
        let port = mock_port_for_admin(server.port);

        let result = handle_group_command_with_port(
            GroupCommands::Rule {
                action: GroupRuleCommands::Delete {
                    group_id: "g1".to_string(),
                    name: "test-rule".to_string(),
                },
            },
            port,
        );
        assert!(result.is_ok());
        server.stop();
    }

    #[test]
    fn test_group_list_null_list() {
        let json = r#"{"code":200,"message":"ok","data":{"list":null,"total":null}}"#;

        let server = MockServer::start(vec![(200, json)]);
        let port = mock_port_for_admin(server.port);

        let result = handle_group_command_with_port(
            GroupCommands::List {
                keyword: None,
                limit: 50,
                offset: 0,
            },
            port,
        );
        assert!(result.is_ok());
        server.stop();
    }

    #[test]
    fn test_group_list_null_fields() {
        let json = r#"{
            "code": 200,
            "message": "ok",
            "data": {
                "list": [
                    {"id":"g1","name":"Team","what":null,"visibility":null,"create_time":null},
                    {"id":"g2","name":"Team2","description":null,"visibility":"public","create_time":"2024-01-01T00:00:00Z"}
                ],
                "total": 2
            }
        }"#;

        let server = MockServer::start(vec![(200, json)]);
        let port = mock_port_for_admin(server.port);

        let result = handle_group_command_with_port(
            GroupCommands::List {
                keyword: None,
                limit: 50,
                offset: 0,
            },
            port,
        );
        assert!(result.is_ok());
        server.stop();
    }

    #[test]
    fn test_load_rule_content_from_string() {
        let result = load_rule_content(Some("hello".to_string()), None);
        assert_eq!(result.unwrap(), "hello");
    }

    #[test]
    fn test_load_rule_content_from_file() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), "file content").unwrap();

        let result = load_rule_content(None, Some(tmp.path().to_path_buf()));
        assert_eq!(result.unwrap(), "file content");
    }

    #[test]
    fn test_load_rule_content_neither() {
        let result = load_rule_content(None, None);
        assert!(result.is_err());
    }

    #[test]
    fn test_load_rule_content_prefers_string_over_file() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), "file content").unwrap();

        let result = load_rule_content(
            Some("string content".to_string()),
            Some(tmp.path().to_path_buf()),
        );
        assert_eq!(result.unwrap(), "string content");
    }

    #[test]
    fn test_group_rule_url_encoding() {
        let json = r#"{
            "name": "rule with spaces",
            "content": "",
            "enabled": true,
            "sort_order": 0,
            "created_at": "2024-01-01T00:00:00Z",
            "updated_at": "2024-06-01T00:00:00Z",
            "sync": {"status":"local_only","remote_id":null,"remote_updated_at":null}
        }"#;

        let server = MockServer::start(vec![(200, json)]);
        let port = mock_port_for_admin(server.port);

        let result = handle_group_command_with_port(
            GroupCommands::Rule {
                action: GroupRuleCommands::Show {
                    group_id: "group/with/slash".to_string(),
                    name: "rule with spaces".to_string(),
                },
            },
            port,
        );
        assert!(result.is_ok());

        let logs = server.stop();
        assert!(logs[0].contains("group%2Fwith%2Fslash"));
        assert!(logs[0].contains("rule%20with%20spaces"));
    }
}
