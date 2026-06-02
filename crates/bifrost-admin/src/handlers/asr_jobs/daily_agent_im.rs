// ─── Daily Agent IM Delivery ─────────────────────────────────────────────────

const DAILY_AGENT_IM_TEXT_CHUNK_CHARS: usize = 12_000;

/// Discover the admin port for self-calls. Resolution order:
/// 1. BIFROST_ADMIN_PORT env var
/// 2. data_dir/runtime.json written by the active daemon
/// 3. data_dir/admin.port file (legacy/manual override)
/// 4. fallback 9900 (default production port)
fn discover_admin_port() -> u16 {
    if let Ok(port_str) = std::env::var("BIFROST_ADMIN_PORT") {
        if let Ok(port) = port_str.parse::<u16>() {
            return port;
        }
    }

    let runtime_file = bifrost_storage::data_dir().join("runtime.json");
    if let Ok(content) = std::fs::read_to_string(&runtime_file) {
        if let Some(port) = serde_json::from_str::<serde_json::Value>(&content)
            .ok()
            .and_then(|value| value.get("port").and_then(|port| port.as_u64()))
            .and_then(|port| u16::try_from(port).ok())
        {
            return port;
        }
    }

    let port_file = bifrost_storage::data_dir().join("admin.port");
    if let Ok(content) = std::fs::read_to_string(&port_file) {
        if let Ok(port) = content.trim().parse::<u16>() {
            return port;
        }
    }

    9900
}

fn daily_agent_im_send_url(port: u16) -> String {
    format!("http://127.0.0.1:{port}/_bifrost/api/im-gateway/messages/send")
}

fn split_daily_agent_im_content(content: &str) -> Vec<String> {
    if content.chars().count() <= DAILY_AGENT_IM_TEXT_CHUNK_CHARS {
        return vec![content.to_string()];
    }

    let mut chunks = Vec::new();
    let mut current = String::new();
    let mut current_chars = 0usize;
    for ch in content.chars() {
        current.push(ch);
        current_chars += 1;
        if current_chars >= DAILY_AGENT_IM_TEXT_CHUNK_CHARS {
            chunks.push(std::mem::take(&mut current));
            current_chars = 0;
        }
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    chunks
}

fn decorate_daily_agent_im_chunk(chunk: &str, index: usize, total: usize) -> String {
    if total <= 1 {
        return chunk.to_string();
    }
    format!("ASR Daily Agent Report {}/{}\n\n{}", index + 1, total, chunk)
}

/// Send an IM message via the local IM gateway API.
async fn send_daily_agent_im_message(
    task: &AsrDirectoryTask,
    content: &str,
    _run_id: &str,
    report_count: usize,
) -> Result<(), String> {
    let channel = daily_agent_im_channel(task).ok_or("IM channel not configured")?;

    let port = discover_admin_port();
    let url = daily_agent_im_send_url(port);
    let target_id = channel.target_id;

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| format!("create HTTP client: {e}"))?;

    let chunks = split_daily_agent_im_content(content);
    let chunk_count = chunks.len();
    for (index, chunk) in chunks.iter().enumerate() {
        let text = decorate_daily_agent_im_chunk(chunk, index, chunk_count);
        let mut body = serde_json::json!({
            "target_id": target_id,
            "msg_type": "text",
            "text": text,
        });
        if let Some(provider_id) = channel.provider_id {
            body["provider_id"] = serde_json::Value::String(provider_id.to_string());
        }

        let resp = client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("IM send request failed for chunk {}/{}: {e}", index + 1, chunk_count))?;

        let status = resp.status();
        if !status.is_success() {
            let resp_text = resp.text().await.unwrap_or_default();
            return Err(format!(
                "IM send failed for chunk {}/{} (HTTP {}): {}",
                index + 1,
                chunk_count,
                status,
                resp_text
            ));
        }
    }

    let _ = update_daily_agent_im_sent(task);

    tracing::info!(
        task_id = %task.id,
        target_id,
        provider_id = ?channel.provider_id,
        content_len = content.len(),
        chunks = chunk_count,
        report_count,
        "ASR daily agent IM delivery sent"
    );

    Ok(())
}
