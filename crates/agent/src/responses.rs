//! Responses API streaming client support.

use crate::config::EffectiveModelConfig;
use crate::history;
use crate::types::{ChatMessage, ModelResponse, TokenUsage, ToolCallMessage, ToolDefinition};
use bifrost_core::text::truncate_bytes_with_suffix;
use futures::StreamExt;
use serde_json::{json, Value};
use tracing::{info, warn};

pub async fn stream_model_response(
    http: &reqwest::Client,
    effective: &EffectiveModelConfig,
    messages: &[ChatMessage],
    tools: &[ToolDefinition],
    output_schema: Option<&Value>,
) -> Result<ModelResponse, String> {
    let url = responses_url(&effective.base_url);
    let (messages, sanitize_report) = history::sanitize_chat_history(messages);
    let (instructions, input_messages) = split_responses_instructions(&messages);

    if sanitize_report.dropped_anything() {
        warn!(
            dropped_orphan_tool_messages = sanitize_report.dropped_orphan_tool_messages,
            dropped_incomplete_tool_call_messages =
                sanitize_report.dropped_incomplete_tool_call_messages,
            sanitized_message_count = messages.len(),
            "sanitized malformed chat history at Responses request boundary"
        );
    }

    let mut body = json!({
        "model": effective.model,
        "input": responses_input(input_messages),
        "max_output_tokens": effective.max_completion_tokens,
        "stream": true,
    });
    if let Some(instructions) = instructions.filter(|text| !text.trim().is_empty()) {
        body["instructions"] = json!(instructions);
    }

    if !tools.is_empty() {
        body["tools"] = responses_tools(tools)?;
    }

    if let Some(schema) = output_schema {
        body["text"] = json!({
            "format": {
                "type": "json_schema",
                "name": "structured_output",
                "strict": true,
                "schema": schema
            }
        });
    }

    if effective.reasoning_effort.is_some() || effective.reasoning_summary.is_some() {
        let mut reasoning = serde_json::Map::new();
        if let Some(effort) = &effective.reasoning_effort {
            reasoning.insert("effort".to_string(), json!(effort));
        }
        if let Some(summary) = &effective.reasoning_summary {
            reasoning.insert("summary".to_string(), json!(summary));
        }
        body["reasoning"] = Value::Object(reasoning);
    }

    info!(
        url = %url,
        model = %effective.model,
        message_count = messages.len(),
        tool_count = tools.len(),
        "sending Responses streaming request"
    );

    let mut request = http
        .post(&url)
        .header("Content-Type", "application/json")
        .header("Accept", "text/event-stream")
        .timeout(std::time::Duration::from_secs(
            effective.request_timeout_secs,
        ));

    if effective.use_azure_auth && !effective.api_key.is_empty() {
        request = request.header("api-key", &effective.api_key);
    } else if !effective.use_azure_auth && !effective.api_key.is_empty() {
        request = request.header("Authorization", format!("Bearer {}", effective.api_key));
    }
    for (key, value) in &effective.extra_headers {
        request = request.header(key.as_str(), value.as_str());
    }

    let response = request
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Responses HTTP request failed: {e}"))?;
    let status = response.status();
    if !status.is_success() {
        let error_body = response.text().await.unwrap_or_default();
        return Err(format!(
            "Responses API error (status {}): {}",
            status,
            truncate_bytes_with_suffix(&error_body, 500, "...")
        ));
    }

    let mut accumulator = ResponsesAccumulator::default();
    let mut parser = SseParser::default();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let bytes = chunk.map_err(|e| format!("failed to read Responses stream: {e}"))?;
        let text = std::str::from_utf8(&bytes)
            .map_err(|e| format!("Responses stream contained non-UTF8 data: {e}"))?;
        for event in parser.push(text)? {
            accumulator.apply_event(&event)?;
        }
    }
    for event in parser.finish()? {
        accumulator.apply_event(&event)?;
    }

    accumulator.finish()
}

pub fn responses_url(base_url: &str) -> String {
    let trimmed = base_url.trim_end_matches('/');
    if trimmed.ends_with("/chat/completions") {
        let prefix = trimmed.trim_end_matches("/chat/completions");
        format!("{prefix}/responses")
    } else if trimmed.ends_with("/responses") {
        trimmed.to_string()
    } else {
        format!("{trimmed}/responses")
    }
}

fn responses_input(messages: &[ChatMessage]) -> Value {
    let mut input = Vec::new();
    for message in messages {
        if message.role == "tool" {
            input.push(json!({
                "type": "function_call_output",
                "call_id": message.tool_call_id,
                "output": message.content.clone().unwrap_or_default(),
            }));
            continue;
        }

        if let Some(content) = message
            .content
            .as_ref()
            .filter(|content| !content.is_empty())
        {
            input.push(json!({
                "role": message.role,
                "content": content,
            }));
        }

        if let Some(tool_calls) = &message.tool_calls {
            for tool_call in tool_calls {
                input.push(json!({
                    "type": "function_call",
                    "call_id": tool_call.id,
                    "name": tool_call.name(),
                    "arguments": tool_call.arguments(),
                }));
            }
        }
    }
    Value::Array(input)
}

fn split_responses_instructions(messages: &[ChatMessage]) -> (Option<&str>, &[ChatMessage]) {
    match messages.first() {
        Some(message) if message.role == "system" => (message.content.as_deref(), &messages[1..]),
        _ => (None, messages),
    }
}

fn responses_tools(tools: &[ToolDefinition]) -> Result<Value, String> {
    let mut converted = Vec::with_capacity(tools.len());
    for tool in tools {
        let function = tool
            .function
            .as_ref()
            .ok_or_else(|| format!("Responses only supports function tools: {}", tool.name()))?;
        converted.push(json!({
            "type": "function",
            "name": function.name,
            "description": function.description,
            "parameters": function.parameters,
            "strict": false,
        }));
    }
    Ok(Value::Array(converted))
}

#[derive(Debug, Default)]
struct SseParser {
    buffer: String,
}

impl SseParser {
    fn push(&mut self, chunk: &str) -> Result<Vec<Value>, String> {
        self.buffer.push_str(chunk);
        let mut values = Vec::new();
        while let Some(index) = self.buffer.find("\n\n") {
            let frame = self.buffer[..index].to_string();
            self.buffer.drain(..index + 2);
            if let Some(value) = parse_sse_frame(&frame)? {
                values.push(value);
            }
        }
        Ok(values)
    }

    fn finish(&mut self) -> Result<Vec<Value>, String> {
        if self.buffer.trim().is_empty() {
            return Ok(Vec::new());
        }
        let frame = std::mem::take(&mut self.buffer);
        Ok(parse_sse_frame(&frame)?.into_iter().collect())
    }
}

fn parse_sse_frame(frame: &str) -> Result<Option<Value>, String> {
    let mut data = String::new();
    for line in frame.lines() {
        let line = line.trim_end_matches('\r');
        if let Some(rest) = line.strip_prefix("data:") {
            let rest = rest.trim_start();
            if rest == "[DONE]" {
                return Ok(None);
            }
            if !data.is_empty() {
                data.push('\n');
            }
            data.push_str(rest);
        }
    }
    if data.trim().is_empty() {
        return Ok(None);
    }
    serde_json::from_str(&data).map(Some).map_err(|e| {
        format!(
            "failed to parse Responses SSE frame as JSON: {e}; frame={}",
            truncate_bytes_with_suffix(&data, 300, "...")
        )
    })
}

#[derive(Debug, Default)]
struct ResponsesAccumulator {
    content: String,
    reasoning_content: String,
    tool_calls: Vec<ToolCallMessage>,
    finish_reason: Option<String>,
    usage: Option<TokenUsage>,
}

impl ResponsesAccumulator {
    fn apply_event(&mut self, event: &Value) -> Result<(), String> {
        match event
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default()
        {
            "response.output_text.delta" => {
                if let Some(delta) = event.get("delta").and_then(Value::as_str) {
                    self.content.push_str(delta);
                }
            }
            "response.reasoning_summary_text.delta" | "response.reasoning_text.delta" => {
                if let Some(delta) = event.get("delta").and_then(Value::as_str) {
                    self.reasoning_content.push_str(delta);
                }
            }
            "response.output_item.done" => {
                if let Some(item) = event.get("item") {
                    self.apply_output_item(item)?;
                }
            }
            "response.completed" => {
                if let Some(response) = event.get("response") {
                    self.apply_response(response)?;
                    self.finish_reason.get_or_insert_with(|| "stop".to_string());
                }
            }
            "response.failed" | "response.incomplete" => {
                self.finish_reason = Some(
                    event
                        .get("type")
                        .and_then(Value::as_str)
                        .unwrap_or("incomplete")
                        .to_string(),
                );
            }
            _ => {}
        }
        Ok(())
    }

    fn apply_response(&mut self, response: &Value) -> Result<(), String> {
        if let Some(output) = response.get("output").and_then(Value::as_array) {
            for item in output {
                if item.get("type").and_then(Value::as_str) == Some("message")
                    && !self.content.is_empty()
                {
                    continue;
                }
                self.apply_output_item(item)?;
            }
        }
        if let Some(text) = response
            .get("output_text")
            .and_then(Value::as_str)
            .filter(|text| self.content.is_empty() && !text.is_empty())
        {
            self.content.push_str(text);
        }
        if let Some(status) = response.get("status").and_then(Value::as_str) {
            self.finish_reason = Some(if status == "completed" {
                "stop".to_string()
            } else {
                status.to_string()
            });
        }
        if let Some(usage) = response.get("usage") {
            self.usage = Some(TokenUsage {
                prompt_tokens: usage
                    .get("input_tokens")
                    .or_else(|| usage.get("prompt_tokens"))
                    .and_then(Value::as_u64)
                    .unwrap_or(0),
                completion_tokens: usage
                    .get("output_tokens")
                    .or_else(|| usage.get("completion_tokens"))
                    .and_then(Value::as_u64)
                    .unwrap_or(0),
                total_tokens: usage
                    .get("total_tokens")
                    .and_then(Value::as_u64)
                    .unwrap_or(0),
            });
        }
        Ok(())
    }

    fn apply_output_item(&mut self, item: &Value) -> Result<(), String> {
        match item.get("type").and_then(Value::as_str).unwrap_or_default() {
            "message" => {
                if let Some(content) = item.get("content").and_then(Value::as_array) {
                    for part in content {
                        if let Some(text) = part
                            .get("text")
                            .or_else(|| part.get("output_text"))
                            .and_then(Value::as_str)
                        {
                            self.content.push_str(text);
                        }
                    }
                }
            }
            "function_call" => {
                let name = item
                    .get("name")
                    .and_then(Value::as_str)
                    .ok_or("Responses function_call missing name")?;
                let arguments = item
                    .get("arguments")
                    .and_then(Value::as_str)
                    .unwrap_or("{}")
                    .to_string();
                let id = item
                    .get("call_id")
                    .or_else(|| item.get("id"))
                    .and_then(Value::as_str)
                    .unwrap_or(name)
                    .to_string();
                if !self.tool_calls.iter().any(|call| call.id == id) {
                    self.tool_calls.push(ToolCallMessage::function_call(
                        id,
                        name.to_string(),
                        arguments,
                    ));
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn finish(self) -> Result<ModelResponse, String> {
        Ok(ModelResponse {
            content: if self.content.is_empty() {
                None
            } else {
                Some(self.content)
            },
            reasoning_content: if self.reasoning_content.is_empty() {
                None
            } else {
                Some(self.reasoning_content)
            },
            tool_calls: self.tool_calls,
            finish_reason: self.finish_reason.unwrap_or_else(|| "stop".to_string()),
            usage: self.usage,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ToolDefinition;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};
    use std::thread;

    #[test]
    fn responses_url_converts_chat_completions_endpoint() {
        assert_eq!(
            responses_url("https://api.openai.com/v1/chat/completions"),
            "https://api.openai.com/v1/responses"
        );
        assert_eq!(
            responses_url("https://api.openai.com/v1/responses/"),
            "https://api.openai.com/v1/responses"
        );
        assert_eq!(
            responses_url("https://example.com/proxy"),
            "https://example.com/proxy/responses"
        );
    }

    #[test]
    fn parses_responses_stream_text_and_function_call() {
        let mut parser = SseParser::default();
        let mut acc = ResponsesAccumulator::default();
        for event in parser
            .push(
                "event: response.output_text.delta\n\
                 data: {\"type\":\"response.output_text.delta\",\"delta\":\"hello\"}\n\n\
                 event: response.output_item.done\n\
                 data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"function_call\",\"call_id\":\"call_1\",\"name\":\"exec_command\",\"arguments\":\"{\\\"cmd\\\":\\\"pwd\\\"}\"}}\n\n\
                 data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"output\":[{\"type\":\"message\",\"content\":[{\"type\":\"output_text\",\"text\":\"hello\"}]}],\"usage\":{\"input_tokens\":2,\"output_tokens\":3,\"total_tokens\":5}}}\n\n",
            )
            .unwrap()
        {
            acc.apply_event(&event).unwrap();
        }
        let response = acc.finish().unwrap();
        assert_eq!(response.content.as_deref(), Some("hello"));
        assert_eq!(response.tool_calls.len(), 1);
        assert_eq!(response.tool_calls[0].name(), "exec_command");
        assert_eq!(response.usage.unwrap().total_tokens, 5);
    }

    #[test]
    fn responses_input_uses_native_function_call_items() {
        let messages = vec![
            ChatMessage::system("base"),
            ChatMessage::user("run pwd"),
            ChatMessage::assistant_with_tool_calls(vec![ToolCallMessage::function_call(
                "call_1".to_string(),
                "exec_command".to_string(),
                "{\"cmd\":\"pwd\"}".to_string(),
            )]),
            ChatMessage::tool_result("call_1", "/tmp"),
        ];
        let (instructions, input_messages) = split_responses_instructions(&messages);
        let input = responses_input(input_messages);
        let items = input.as_array().expect("array input");

        assert_eq!(instructions, Some("base"));
        assert_eq!(items[0]["role"], "user");
        assert_eq!(items[1]["type"], "function_call");
        assert_eq!(items[1]["call_id"], "call_1");
        assert_eq!(items[1]["name"], "exec_command");
        assert!(items[1].get("tool_calls").is_none());
        assert_eq!(items[2]["type"], "function_call_output");
        assert_eq!(items[2]["call_id"], "call_1");
    }

    #[tokio::test]
    async fn streaming_client_sends_responses_shape() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock");
        let url = format!(
            "http://{}/v1/chat/completions",
            listener.local_addr().unwrap()
        );
        let captured = Arc::new(Mutex::new(String::new()));
        let captured_thread = captured.clone();
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut request = Vec::new();
            let mut buf = [0u8; 4096];
            loop {
                let read = stream.read(&mut buf).expect("read");
                if read == 0 {
                    break;
                }
                request.extend_from_slice(&buf[..read]);
                if request.windows(4).any(|window| window == b"\r\n\r\n") {
                    let text = String::from_utf8_lossy(&request).to_string();
                    let content_len = text
                        .lines()
                        .find_map(|line| {
                            line.strip_prefix("Content-Length:")
                                .and_then(|v| v.trim().parse::<usize>().ok())
                        })
                        .unwrap_or(0);
                    let header_len = text.find("\r\n\r\n").unwrap() + 4;
                    if request.len() >= header_len + content_len {
                        break;
                    }
                }
            }
            *captured_thread.lock().unwrap() = String::from_utf8_lossy(&request).to_string();
            let body = "data: {\"type\":\"response.output_text.delta\",\"delta\":\"ok\"}\n\n\
                        data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\"}}\n\n";
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(response.as_bytes()).expect("write");
        });

        let effective = EffectiveModelConfig {
            model: "gpt-test".to_string(),
            base_url: url,
            api_key: "test-key".to_string(),
            max_completion_tokens: 100,
            reasoning_effort: Some("medium".to_string()),
            reasoning_summary: Some("auto".to_string()),
            request_timeout_secs: 5,
            extra_headers: Default::default(),
            use_azure_auth: false,
            stream_max_retries: 0,
            wire_api: crate::config::ModelWireApi::Responses,
        };
        let response = stream_model_response(
            &reqwest::Client::new(),
            &effective,
            &[
                ChatMessage::system("base instructions"),
                ChatMessage::user("hello"),
            ],
            &[ToolDefinition::function(
                "exec_command".to_string(),
                "Run command".to_string(),
                Some(json!({"type":"object"})),
            )],
            None,
        )
        .await
        .unwrap();

        assert_eq!(response.content.as_deref(), Some("ok"));
        let request = captured.lock().unwrap().clone();
        assert!(request.starts_with("POST /v1/responses "));
        assert!(request.contains("\"input\""));
        assert!(request.contains("\"instructions\":\"base instructions\""));
        assert!(request.contains("\"stream\":true"));
        assert!(request.contains("\"reasoning\""));
        assert!(!request.contains("\"messages\""));
        assert!(!request.contains("\"reasoning_effort\""));
    }
}
