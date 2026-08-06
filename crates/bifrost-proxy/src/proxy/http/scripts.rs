use std::collections::HashMap;
use std::sync::Arc;

use bifrost_admin::AdminState;
use bifrost_script::{
    MatchedRuleInfo, RequestData, ResponseData, ScriptContext, ScriptType, StreamScriptEvent,
    StreamScriptMode, StreamScriptOutput, StreamScriptStep, StreamScriptWorker,
};
use bytes::{Bytes, BytesMut};
use http_body_util::{BodyExt, StreamBody};
use hyper::body::Frame;
use hyper::header::{HeaderName, HeaderValue};
use tokio::sync::mpsc;

use crate::server::BoxBody;
use crate::server::ResolvedRules;
use crate::transform::{compress_body, try_decompress_body_with_limit, ContentInjectionResult};
use crate::utils::logging::RequestContext;

// SSE does not define an event-size limit. Keep a generous guard against an
// unterminated event consuming unbounded memory, without constraining normal
// large tool arguments or generated code. Data is never truncated.
const MAX_STREAM_SSE_EVENT_BYTES: usize = 16 * 1024 * 1024;

pub(in crate::proxy::http) fn build_matched_rules_info(
    resolved_rules: &ResolvedRules,
) -> Vec<MatchedRuleInfo> {
    resolved_rules
        .rules
        .iter()
        .map(|r| MatchedRuleInfo {
            pattern: r.pattern.clone(),
            protocol: r.protocol.to_string(),
            value: r.value.clone(),
        })
        .collect()
}

pub(in crate::proxy::http) fn parse_url_parts(url: &str) -> (String, String, String) {
    if let Ok(parsed) = url::Url::parse(url) {
        let host = parsed.host_str().unwrap_or("").to_string();
        let path = parsed.path().to_string();
        let protocol = parsed.scheme().to_string();
        (host, path, protocol)
    } else {
        ("".to_string(), url.to_string(), "http".to_string())
    }
}

pub(in crate::proxy::http) fn headers_to_hashmap(
    headers: &[(String, String)],
) -> HashMap<String, String> {
    header_pairs_to_hashmap(headers)
}

pub(in crate::proxy::http) fn header_pairs_to_hashmap(
    headers: &[(String, String)],
) -> HashMap<String, String> {
    let mut map = HashMap::with_capacity(headers.len());
    for (key, value) in headers {
        map.insert(key.clone(), value.clone());
    }
    map
}

pub(in crate::proxy::http) fn header_map_to_hashmap(
    headers: &hyper::HeaderMap,
) -> HashMap<String, String> {
    let mut map = HashMap::with_capacity(headers.len());
    for (key, value) in headers {
        map.insert(key.to_string(), value.to_str().unwrap_or("").to_string());
    }
    map
}

fn lower_header_map(headers: &HashMap<String, String>) -> HashMap<String, (String, String)> {
    headers
        .iter()
        .map(|(key, value)| (key.to_ascii_lowercase(), (key.clone(), value.clone())))
        .collect()
}

pub(in crate::proxy::http) fn apply_script_headers_to_header_map(
    original_headers: &hyper::HeaderMap,
    original_script_headers: &HashMap<String, String>,
    updated_script_headers: &HashMap<String, String>,
) -> hyper::HeaderMap {
    let original_script_headers = lower_header_map(original_script_headers);
    let updated_script_headers = lower_header_map(updated_script_headers);
    let mut result = hyper::HeaderMap::new();
    let mut rewritten = std::collections::HashSet::new();

    for (name, value) in original_headers {
        let lower_name = name.as_str().to_ascii_lowercase();
        match updated_script_headers.get(&lower_name) {
            Some((_, updated_value))
                if original_script_headers
                    .get(&lower_name)
                    .map(|(_, original_value)| original_value == updated_value)
                    .unwrap_or(false) =>
            {
                result.append(name.clone(), value.clone());
            }
            Some((updated_name, updated_value)) if rewritten.insert(lower_name) => {
                if let (Ok(name), Ok(value)) = (
                    HeaderName::from_bytes(updated_name.as_bytes()),
                    HeaderValue::from_str(updated_value),
                ) {
                    result.insert(name, value);
                }
            }
            Some(_) => {}
            None => {}
        }
    }

    for (lower_name, (updated_name, updated_value)) in &updated_script_headers {
        if original_script_headers.contains_key(lower_name) || rewritten.contains(lower_name) {
            continue;
        }
        if let (Ok(name), Ok(value)) = (
            HeaderName::from_bytes(updated_name.as_bytes()),
            HeaderValue::from_str(updated_value),
        ) {
            result.insert(name, value);
        }
    }

    result
}

pub(in crate::proxy::http) fn body_to_script_string(
    body: &Bytes,
    content_encoding: Option<&str>,
    max_decompress_output_bytes: usize,
) -> Option<String> {
    if body.is_empty() {
        return None;
    }

    let content_encoding =
        content_encoding.filter(|encoding| !encoding.eq_ignore_ascii_case("identity"));
    let decoded = if let Some(content_encoding) = content_encoding {
        try_decompress_body_with_limit(body.as_ref(), content_encoding, max_decompress_output_bytes)
            .ok()?
    } else {
        body.to_vec()
    };

    String::from_utf8(decoded).ok()
}

pub(in crate::proxy::http) fn script_string_to_body(
    body: &str,
    content_encoding: Option<&str>,
) -> ContentInjectionResult {
    let content_encoding =
        content_encoding.filter(|encoding| !encoding.eq_ignore_ascii_case("identity"));
    if let Some(content_encoding) = content_encoding {
        match compress_body(body.as_bytes(), content_encoding) {
            Ok(compressed) => ContentInjectionResult {
                body: Bytes::from(compressed),
                content_encoding: Some(content_encoding.to_string()),
            },
            Err(_) => ContentInjectionResult {
                body: Bytes::from(body.to_string()),
                content_encoding: None,
            },
        }
    } else {
        ContentInjectionResult {
            body: Bytes::from(body.to_string()),
            content_encoding: None,
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(in crate::proxy::http) async fn execute_request_scripts(
    admin_state: &Option<Arc<AdminState>>,
    script_names: &[String],
    ctx: &RequestContext,
    resolved_rules: &ResolvedRules,
    url: &str,
    method: &mut String,
    headers: &mut HashMap<String, String>,
    body: &mut Option<String>,
    values: &HashMap<String, String>,
) -> Vec<bifrost_script::ScriptExecutionResult> {
    if script_names.is_empty() {
        return vec![];
    }

    let state = match admin_state {
        Some(s) => s,
        None => return vec![],
    };

    let manager = match &state.script_manager {
        Some(m) => m,
        None => return vec![],
    };

    let cfg = if let Some(cm) = state.config_manager.as_ref() {
        Some(cm.config().await)
    } else {
        None
    };
    let matched_rules = build_matched_rules_info(resolved_rules);
    let (host, path, protocol) = parse_url_parts(url);

    let mut request_data = RequestData {
        url: url.to_string(),
        method: method.clone(),
        host,
        path,
        protocol,
        client_ip: ctx.client_ip.clone(),
        client_app: ctx.client_app.clone(),
        headers: headers.clone(),
        body: body.clone(),
    };

    let script_ctx = ScriptContext {
        request_id: ctx.id_str().to_string(),
        script_name: script_names.first().cloned().unwrap_or_default(),
        script_type: ScriptType::Request,
        values: values.clone(),
        matched_rules,
    };

    let mgr = manager.read().await;
    let results = if let Some(ref cfg) = cfg {
        mgr.execute_request_scripts_with_config(script_names, &mut request_data, &script_ctx, cfg)
            .await
    } else {
        mgr.execute_request_scripts(script_names, &mut request_data, &script_ctx)
            .await
    };

    if results.iter().any(|r| r.success) {
        *method = request_data.method;
        *headers = request_data.headers;
        *body = request_data.body;
    }

    results
}

#[allow(clippy::too_many_arguments)]
pub(in crate::proxy::http) async fn execute_response_scripts(
    admin_state: &Option<Arc<AdminState>>,
    script_names: &[String],
    ctx: &RequestContext,
    resolved_rules: &ResolvedRules,
    request_url: &str,
    request_method: &str,
    request_headers: &HashMap<String, String>,
    request_body: Option<String>,
    status: &mut u16,
    status_text: &mut String,
    headers: &mut HashMap<String, String>,
    body: &mut Option<String>,
    values: &HashMap<String, String>,
) -> Vec<bifrost_script::ScriptExecutionResult> {
    if script_names.is_empty() {
        return vec![];
    }

    let state = match admin_state {
        Some(s) => s,
        None => return vec![],
    };

    let manager = match &state.script_manager {
        Some(m) => m,
        None => return vec![],
    };

    let cfg = if let Some(cm) = state.config_manager.as_ref() {
        Some(cm.config().await)
    } else {
        None
    };
    let matched_rules = build_matched_rules_info(resolved_rules);
    let (host, path, protocol) = parse_url_parts(request_url);

    let mut response_data = ResponseData {
        status: *status,
        status_text: status_text.clone(),
        headers: headers.clone(),
        body: body.clone(),
        request: RequestData {
            url: request_url.to_string(),
            method: request_method.to_string(),
            host,
            path,
            protocol,
            client_ip: ctx.client_ip.clone(),
            client_app: ctx.client_app.clone(),
            headers: request_headers.clone(),
            body: request_body,
        },
    };

    let script_ctx = ScriptContext {
        request_id: ctx.id_str().to_string(),
        script_name: script_names.first().cloned().unwrap_or_default(),
        script_type: ScriptType::Response,
        values: values.clone(),
        matched_rules,
    };

    let mgr = manager.read().await;
    let results = if let Some(ref cfg) = cfg {
        mgr.execute_response_scripts_with_config(script_names, &mut response_data, &script_ctx, cfg)
            .await
    } else {
        mgr.execute_response_scripts(script_names, &mut response_data, &script_ctx)
            .await
    };

    if results.iter().any(|r| r.success) {
        *status = response_data.status;
        *status_text = response_data.status_text;
        *headers = response_data.headers;
        *body = response_data.body;
    }

    results
}

#[allow(clippy::too_many_arguments)]
pub(in crate::proxy::http) async fn initialize_response_stream_script(
    admin_state: &Option<Arc<AdminState>>,
    script_names: &[String],
    ctx: &RequestContext,
    resolved_rules: &ResolvedRules,
    request_url: &str,
    request_method: &str,
    request_headers: &HashMap<String, String>,
    status: u16,
    status_text: String,
    response_headers: HashMap<String, String>,
    values: &HashMap<String, String>,
) -> Result<StreamScriptWorker, String> {
    if script_names.len() != 1 {
        return Err("resStreamScript currently requires exactly one script".to_string());
    }
    let state = admin_state
        .as_ref()
        .ok_or_else(|| "script manager is unavailable".to_string())?;
    let manager = state
        .script_manager
        .as_ref()
        .ok_or_else(|| "script manager is unavailable".to_string())?;
    let script_ref = &script_names[0];
    let inline = script_ref
        .strip_prefix('{')
        .and_then(|name| name.strip_suffix('}'))
        .and_then(|name| values.get(name).map(String::as_str));
    let (host, path, protocol) = parse_url_parts(request_url);
    let response = ResponseData {
        status,
        status_text,
        headers: response_headers,
        body: None,
        request: RequestData {
            url: request_url.to_string(),
            method: request_method.to_string(),
            host,
            path,
            protocol,
            client_ip: ctx.client_ip.clone(),
            client_app: ctx.client_app.clone(),
            headers: request_headers.clone(),
            body: None,
        },
    };
    let script_ctx = ScriptContext {
        request_id: ctx.id_str().to_string(),
        script_name: script_ref.clone(),
        script_type: ScriptType::Response,
        values: values.clone(),
        matched_rules: build_matched_rules_info(resolved_rules),
    };
    let manager = manager.read().await;
    if let Some(config_manager) = state.config_manager.as_ref() {
        let config = config_manager.config().await;
        manager
            .engine()
            .create_response_stream_worker_with_config(
                script_ref,
                inline,
                &response,
                &script_ctx,
                &config,
            )
            .await
            .map_err(|error| error.to_string())
    } else {
        manager
            .engine()
            .create_response_stream_worker(script_ref, inline, &response, &script_ctx)
            .await
            .map_err(|error| error.to_string())
    }
}

fn find_sse_event_boundary(buffer: &[u8]) -> Option<(usize, usize)> {
    let mut index = 0;
    let mut previous_line_ending: Option<(usize, usize)> = None;
    while index < buffer.len() {
        let ending_len = match buffer[index] {
            b'\r' if buffer.get(index + 1) == Some(&b'\n') => 2,
            b'\r' | b'\n' => 1,
            _ => {
                index += 1;
                continue;
            }
        };
        if let Some((start, end)) = previous_line_ending {
            if end == index {
                return Some((start, index + ending_len - start));
            }
        }
        previous_line_ending = Some((index, index + ending_len));
        index += ending_len;
    }
    None
}

fn encode_stream_output(output: StreamScriptOutput) -> Bytes {
    match output {
        StreamScriptOutput::Raw(raw) => Bytes::from(raw),
        StreamScriptOutput::Event {
            id,
            event,
            data,
            retry,
        } => crate::protocol::SseEvent {
            id,
            event,
            data,
            retry,
        }
        .encode(),
    }
}

async fn send_stream_step(
    tx: &mpsc::Sender<Result<Frame<Bytes>, hyper::Error>>,
    step: StreamScriptStep,
) -> bool {
    if tx.is_closed() {
        return false;
    }
    for output in step.outputs {
        if tx
            .send(Ok(Frame::data(encode_stream_output(output))))
            .await
            .is_err()
        {
            return false;
        }
    }
    if step.delay_ms > 0 {
        tokio::select! {
            _ = tx.closed() => return false,
            _ = tokio::time::sleep(std::time::Duration::from_millis(step.delay_ms)) => {}
        }
    }
    !step.done && !tx.is_closed()
}

async fn send_stream_script_error(
    tx: &mpsc::Sender<Result<Frame<Bytes>, hyper::Error>>,
    error: impl std::fmt::Display,
) {
    let data = serde_json::json!({
        "type": "error",
        "code": "bifrost_stream_script_error",
        "message": error.to_string(),
    });
    let _ = tx
        .send(Ok(Frame::data(Bytes::from(format!(
            "event: error\ndata: {data}\n\n"
        )))))
        .await;
}

/// Wrap an upstream SSE body and execute response scripts once per complete SSE
/// event. A bounded channel provides downstream backpressure; dropping the
/// returned body drops the receiver and stops the upstream worker.
pub(in crate::proxy::http) fn create_response_stream_script_body(
    mut upstream: Option<BoxBody>,
    mut worker: StreamScriptWorker,
) -> BoxBody {
    let (tx, rx) = mpsc::channel::<Result<Frame<Bytes>, hyper::Error>>(8);

    tokio::spawn(async move {
        if !send_stream_step(&tx, worker.take_initial_step()).await {
            return;
        }
        if worker.mode() == StreamScriptMode::Mock {
            // Mock output is self-contained. Releasing the unused upstream body
            // here cancels the origin stream instead of retaining its socket.
            drop(upstream.take());
            loop {
                let next = tokio::select! {
                    _ = tx.closed() => return,
                    next = worker.next() => next,
                };
                match next {
                    Ok(step) => {
                        if !send_stream_step(&tx, step).await {
                            break;
                        }
                        tokio::task::yield_now().await;
                    }
                    Err(error) => {
                        send_stream_script_error(&tx, error).await;
                        break;
                    }
                }
            }
            if !tx.is_closed() {
                match worker.end().await {
                    Ok(step) => {
                        let _ = send_stream_step(&tx, step).await;
                    }
                    Err(error) => send_stream_script_error(&tx, error).await,
                }
            }
            return;
        }

        let Some(ref mut upstream) = upstream else {
            send_stream_script_error(&tx, "transform stream requires an upstream body").await;
            return;
        };
        let mut buffer = BytesMut::new();
        let mut delimiter_scan_from = 0;
        loop {
            let frame_result = tokio::select! {
                _ = tx.closed() => return,
                frame = upstream.frame() => frame,
            };
            let Some(frame_result) = frame_result else {
                break;
            };
            let frame = match frame_result {
                Ok(frame) => frame,
                Err(error) => {
                    let _ = tx.send(Err(error)).await;
                    return;
                }
            };

            if let Some(data) = frame.data_ref() {
                buffer.extend_from_slice(data);
                while let Some((relative_position, delimiter_len)) =
                    find_sse_event_boundary(&buffer[delimiter_scan_from..])
                {
                    let position = delimiter_scan_from + relative_position;
                    if position > MAX_STREAM_SSE_EVENT_BYTES {
                        send_stream_script_error(
                            &tx,
                            format!(
                                "upstream SSE event exceeds {MAX_STREAM_SSE_EVENT_BYTES} bytes"
                            ),
                        )
                        .await;
                        return;
                    }
                    let event = buffer.split_to(position + delimiter_len).freeze();
                    delimiter_scan_from = 0;
                    let Some(event) =
                        crate::protocol::SseEvent::parse(&String::from_utf8_lossy(&event))
                    else {
                        continue;
                    };
                    match worker
                        .event(StreamScriptEvent {
                            id: event.id,
                            event: event.event,
                            data: event.data,
                            retry: event.retry,
                        })
                        .await
                    {
                        Ok(step) => {
                            if !send_stream_step(&tx, step).await {
                                return;
                            }
                        }
                        Err(error) => {
                            send_stream_script_error(&tx, error).await;
                            return;
                        }
                    }
                }
                // Only the final two line endings can combine with the next
                // network frame to form an SSE delimiter. Retaining a
                // three-byte overlap covers CRLF plus a possible lone CR/LF
                // without rescanning the complete accumulated event for every
                // frame (which would become O(n²) for multi-megabyte events).
                delimiter_scan_from = buffer.len().saturating_sub(3);
                if buffer.len() > MAX_STREAM_SSE_EVENT_BYTES {
                    send_stream_script_error(
                        &tx,
                        format!("upstream SSE event exceeds {MAX_STREAM_SSE_EVENT_BYTES} bytes"),
                    )
                    .await;
                    return;
                }
            } else if frame.is_trailers() && tx.send(Ok(frame)).await.is_err() {
                return;
            }
        }

        if !buffer.is_empty() {
            if let Some(event) =
                crate::protocol::SseEvent::parse(&String::from_utf8_lossy(&buffer.freeze()))
            {
                match worker
                    .event(StreamScriptEvent {
                        id: event.id,
                        event: event.event,
                        data: event.data,
                        retry: event.retry,
                    })
                    .await
                {
                    Ok(step) => {
                        if !send_stream_step(&tx, step).await {
                            return;
                        }
                    }
                    Err(error) => {
                        send_stream_script_error(&tx, error).await;
                        return;
                    }
                }
            }
        }
        match worker.end().await {
            Ok(step) => {
                let _ = send_stream_step(&tx, step).await;
            }
            Err(error) => send_stream_script_error(&tx, error).await,
        }
    });

    let stream = futures_util::stream::unfold(rx, |mut receiver| async move {
        receiver.recv().await.map(|item| (item, receiver))
    });
    BodyExt::boxed(StreamBody::new(stream))
}

#[cfg(test)]
mod tests {
    use super::*;
    use bifrost_script::{ScriptEngine, ScriptEngineConfig};
    use futures_util::stream;
    use std::convert::Infallible;
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    fn test_stream_response() -> ResponseData {
        ResponseData {
            status: 200,
            status_text: "OK".to_string(),
            headers: HashMap::new(),
            body: None,
            request: RequestData::default(),
        }
    }

    fn test_stream_context() -> ScriptContext {
        ScriptContext {
            request_id: "proxy-stream-test".to_string(),
            script_name: "inline-stream".to_string(),
            script_type: ScriptType::Response,
            values: HashMap::new(),
            matched_rules: vec![],
        }
    }

    async fn test_stream_worker(script: &str) -> StreamScriptWorker {
        let engine = ScriptEngine::new(ScriptEngineConfig {
            scripts_dir: PathBuf::from("target/test-stream-scripts"),
            ..Default::default()
        });
        engine
            .create_response_stream_worker(
                "inline-stream",
                Some(script),
                &test_stream_response(),
                &test_stream_context(),
            )
            .await
            .unwrap()
    }

    #[test]
    fn apply_script_headers_preserves_unchanged_multi_value_headers() {
        let mut original = hyper::HeaderMap::new();
        original.append("set-cookie", "a=1; Path=/".parse().unwrap());
        original.append("set-cookie", "b=2; Path=/".parse().unwrap());
        original.insert("content-type", "text/plain".parse().unwrap());

        let original_script_headers = header_map_to_hashmap(&original);
        let mut updated_script_headers = original_script_headers.clone();
        updated_script_headers.insert("x-script".to_string(), "ran".to_string());

        let result = apply_script_headers_to_header_map(
            &original,
            &original_script_headers,
            &updated_script_headers,
        );

        let cookies: Vec<_> = result
            .get_all("set-cookie")
            .iter()
            .map(|value| value.to_str().unwrap().to_string())
            .collect();
        assert_eq!(cookies, vec!["a=1; Path=/", "b=2; Path=/"]);
        assert_eq!(result.get("x-script").unwrap(), "ran");
    }

    #[test]
    fn apply_script_headers_rewrites_touched_multi_value_header_once() {
        let mut original = hyper::HeaderMap::new();
        original.append("set-cookie", "a=1; Path=/".parse().unwrap());
        original.append("set-cookie", "b=2; Path=/".parse().unwrap());

        let original_script_headers = header_map_to_hashmap(&original);
        let mut updated_script_headers = original_script_headers.clone();
        updated_script_headers.insert("set-cookie".to_string(), "c=3; Path=/".to_string());

        let result = apply_script_headers_to_header_map(
            &original,
            &original_script_headers,
            &updated_script_headers,
        );

        let cookies: Vec<_> = result
            .get_all("set-cookie")
            .iter()
            .map(|value| value.to_str().unwrap().to_string())
            .collect();
        assert_eq!(cookies, vec!["c=3; Path=/"]);
    }

    #[test]
    fn sse_boundary_uses_the_earliest_mixed_line_ending() {
        let input = b"data: first\n\ndata: second\r\n\r\n";
        assert_eq!(find_sse_event_boundary(input), Some((11, 2)));
        assert_eq!(find_sse_event_boundary(b"data: cr\r\r"), Some((8, 2)));
        assert_eq!(find_sse_event_boundary(b"data: mixed\r\n\n"), Some((11, 3)));
    }

    #[tokio::test]
    async fn transform_stream_emits_before_never_ending_upstream_completes() {
        let worker = test_stream_worker(
            r#"
                stream.mode = "transform";
                let count = 0;
                stream.onEvent = (event) => ({
                    event: "mapped",
                    data: (++count) + ":" + event.data
                });
            "#,
        )
        .await;
        let (upstream_tx, upstream_rx) = mpsc::channel::<Result<Frame<Bytes>, Infallible>>(4);
        let upstream_stream = stream::unfold(upstream_rx, |mut receiver| async move {
            receiver.recv().await.map(|frame| (frame, receiver))
        });
        let upstream = StreamBody::new(upstream_stream)
            .map_err(|never| match never {})
            .boxed();
        let mut body = create_response_stream_script_body(Some(upstream), worker);

        let started_at = Instant::now();
        upstream_tx
            .send(Ok(Frame::data(Bytes::from_static(b"data: fir"))))
            .await
            .unwrap();
        upstream_tx
            .send(Ok(Frame::data(Bytes::from_static(b"st\n\n"))))
            .await
            .unwrap();

        // Keep upstream_tx alive: the upstream is deliberately never completed.
        let first = tokio::time::timeout(Duration::from_millis(500), body.frame())
            .await
            .expect("first transformed frame must arrive before upstream EOF")
            .expect("stream body must still be open")
            .unwrap();
        let first_at = Instant::now();
        assert_eq!(
            first.into_data().unwrap(),
            Bytes::from_static(b"event: mapped\ndata: 1:first\n\n")
        );
        assert!(first_at.duration_since(started_at) < Duration::from_millis(500));

        upstream_tx
            .send(Ok(Frame::data(Bytes::from_static(b"data: second\n\n"))))
            .await
            .unwrap();
        let second = tokio::time::timeout(Duration::from_millis(500), body.frame())
            .await
            .expect("second frame must not wait for upstream EOF")
            .expect("stream body must still be open")
            .unwrap();
        let second_at = Instant::now();
        assert_eq!(
            second.into_data().unwrap(),
            Bytes::from_static(b"event: mapped\ndata: 2:second\n\n")
        );
        assert!(second_at >= first_at);
        drop(body);
        drop(upstream_tx);
    }

    #[tokio::test]
    async fn dropping_downstream_cancels_idle_upstream_immediately() {
        let worker = test_stream_worker(
            r#"
                stream.mode = "transform";
                stream.onEvent = (event) => event;
            "#,
        )
        .await;
        let (upstream_tx, upstream_rx) = mpsc::channel::<Result<Frame<Bytes>, Infallible>>(1);
        let upstream_stream = stream::unfold(upstream_rx, |mut receiver| async move {
            receiver.recv().await.map(|frame| (frame, receiver))
        });
        let upstream = StreamBody::new(upstream_stream)
            .map_err(|never| match never {})
            .boxed();
        let body = create_response_stream_script_body(Some(upstream), worker);

        drop(body);

        tokio::time::timeout(Duration::from_millis(300), upstream_tx.closed())
            .await
            .expect("idle upstream must be dropped when the downstream disconnects");
    }

    #[tokio::test]
    async fn transform_stream_preserves_multi_megabyte_event_without_truncation() {
        let worker = test_stream_worker(
            r#"
                stream.mode = "transform";
                stream.onEvent = (event) => ({ data: event.data });
            "#,
        )
        .await;
        let large_data = "x".repeat(MAX_STREAM_SSE_EVENT_BYTES - 1024);
        let wire_event = format!("data: {large_data}\n\n");
        let (upstream_tx, upstream_rx) = mpsc::channel::<Result<Frame<Bytes>, Infallible>>(4);
        let upstream_stream = stream::unfold(upstream_rx, |mut receiver| async move {
            receiver.recv().await.map(|frame| (frame, receiver))
        });
        let upstream = StreamBody::new(upstream_stream)
            .map_err(|never| match never {})
            .boxed();
        let mut body = create_response_stream_script_body(Some(upstream), worker);

        // Thousands of small frames guard against accidentally rescanning the
        // full accumulated event for every upstream frame.
        for chunk in wire_event.as_bytes().chunks(4093) {
            upstream_tx
                .send(Ok(Frame::data(Bytes::copy_from_slice(chunk))))
                .await
                .unwrap();
        }
        let frame = tokio::time::timeout(Duration::from_secs(10), body.frame())
            .await
            .expect("large event must be transformed without waiting for EOF")
            .expect("large event frame must exist")
            .unwrap()
            .into_data()
            .unwrap();
        let expected = format!("data: {large_data}\n\n");
        assert_eq!(frame.len(), expected.len());
        assert_eq!(frame.as_ref(), expected.as_bytes());
        drop(body);
        drop(upstream_tx);
    }

    #[tokio::test]
    async fn oversized_event_fails_explicitly_without_partial_output() {
        let worker = test_stream_worker(
            r#"
                stream.mode = "transform";
                stream.onEvent = (event) => event;
            "#,
        )
        .await;
        let (upstream_tx, upstream_rx) = mpsc::channel::<Result<Frame<Bytes>, Infallible>>(2);
        let upstream_stream = stream::unfold(upstream_rx, |mut receiver| async move {
            receiver.recv().await.map(|frame| (frame, receiver))
        });
        let upstream = StreamBody::new(upstream_stream)
            .map_err(|never| match never {})
            .boxed();
        let mut body = create_response_stream_script_body(Some(upstream), worker);
        let oversized = Bytes::from(vec![b'x'; MAX_STREAM_SSE_EVENT_BYTES + 1]);
        upstream_tx.send(Ok(Frame::data(oversized))).await.unwrap();

        let frame = tokio::time::timeout(Duration::from_secs(2), body.frame())
            .await
            .expect("oversized event must produce an explicit error")
            .expect("error frame must be present")
            .unwrap()
            .into_data()
            .unwrap();
        let text = String::from_utf8(frame.to_vec()).unwrap();
        assert!(text.starts_with("event: error\n"));
        assert!(text.contains("bifrost_stream_script_error"));
        assert!(text.contains("upstream SSE event exceeds 16777216 bytes"));
        assert!(!text.contains(&"x".repeat(1024)));
    }

    #[tokio::test]
    async fn mock_stream_emits_each_step_at_script_timing() {
        let worker = test_stream_worker(
            r#"
                stream.mode = "mock";
                let count = 0;
                stream.next = () => {
                    count += 1;
                    return {
                        output: "data: mock-" + count + "\n\n",
                        delayMs: 90,
                        done: count === 2
                    };
                };
            "#,
        )
        .await;
        let mut body = create_response_stream_script_body(None, worker);
        let started_at = Instant::now();

        let first = tokio::time::timeout(Duration::from_millis(300), body.frame())
            .await
            .expect("first mock frame must be emitted immediately")
            .expect("mock body must contain first frame")
            .unwrap();
        let first_at = Instant::now();
        assert_eq!(
            first.into_data().unwrap(),
            Bytes::from_static(b"data: mock-1\n\n")
        );
        assert!(first_at.duration_since(started_at) < Duration::from_millis(80));

        let second = tokio::time::timeout(Duration::from_millis(300), body.frame())
            .await
            .expect("second mock frame must arrive after delay")
            .expect("mock body must contain second frame")
            .unwrap();
        let second_at = Instant::now();
        assert_eq!(
            second.into_data().unwrap(),
            Bytes::from_static(b"data: mock-2\n\n")
        );
        assert!(second_at.duration_since(first_at) >= Duration::from_millis(70));
    }
}
