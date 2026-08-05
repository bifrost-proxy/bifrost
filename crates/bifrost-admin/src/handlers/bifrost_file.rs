use bifrost_core::bifrost_file::{
    ActiveRuleExport, ActiveRuleSource, ActiveRulesExport, BifrostFileParser, BifrostFileType,
    BifrostFileWriter, KeyValueItemExport, MatchedRuleExport, NetworkRecord, ReplayBodyExport,
    ReplayGroupExport, ReplayRequestExport, ScriptItem, TemplateContent, ValuesContent,
};
use bifrost_core::normalize_rule_content;
use bifrost_storage::{RuleFile, RulesStorage};
use http_body_util::BodyExt;
use hyper::{body::Incoming, Method, Request, Response, StatusCode};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

use super::{error_response, full_body, json_response, method_not_allowed, BoxBody};
use crate::state::SharedAdminState;
use crate::traffic::TrafficRecord;

const EMPTY_NETWORK_IMPORT_ERROR: &str = "Network file contains 0 records; nothing to import. Re-export from Network after selecting at least one visible request.";

#[derive(Debug, Serialize)]
pub struct DetectResponse {
    pub file_type: BifrostFileType,
    pub meta: serde_json::Value,
}

#[derive(Debug, Serialize)]
pub struct PreviewResponse {
    pub file_type: BifrostFileType,
    pub meta: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rules: Option<RulesPreview>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub network: Option<NetworkPreview>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub item_count: Option<usize>,
}

#[derive(Debug, Serialize)]
pub struct RulesPreview {
    pub name: String,
    pub enabled: bool,
    pub description: Option<String>,
    pub line_count: usize,
    pub content: String,
}

#[derive(Debug, Serialize)]
pub struct NetworkPreview {
    pub record_count: usize,
    pub hosts: Vec<String>,
    pub records: Vec<NetworkPreviewRecord>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub single_record: Option<NetworkPreviewDetail>,
}

#[derive(Debug, Serialize)]
pub struct NetworkPreviewRecord {
    pub id: String,
    pub method: String,
    pub url: String,
    pub status: u16,
    pub host: String,
    pub path: String,
    pub protocol: String,
    pub client_app: Option<String>,
    pub duration_ms: u64,
    pub request_size: usize,
    pub response_size: usize,
}

#[derive(Debug, Serialize)]
pub struct NetworkPreviewDetail {
    pub record: TrafficRecord,
    pub request_body: Option<String>,
    pub response_body: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ImportResponse {
    pub success: bool,
    pub file_type: BifrostFileType,
    pub data: ImportedData,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

#[derive(Debug, Default, Serialize)]
pub struct ImportedData {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rule_names: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rule_count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub record_count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub script_names: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub script_count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value_names: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value_count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group_count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_count: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub struct ExportRulesRequest {
    pub rule_names: Vec<String>,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ExportNetworkRequest {
    pub record_ids: Vec<String>,
    #[serde(default)]
    pub include_body: Option<bool>,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ExportScriptRequest {
    pub script_names: Vec<String>,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ExportValuesRequest {
    #[serde(default)]
    pub value_names: Option<Vec<String>>,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ExportTemplateRequest {
    #[serde(default)]
    pub group_ids: Option<Vec<String>>,
    #[serde(default)]
    pub request_ids: Option<Vec<String>>,
    #[serde(default)]
    pub description: Option<String>,
}

pub async fn handle_bifrost_file(
    req: Request<Incoming>,
    path: &str,
    state: SharedAdminState,
) -> Response<BoxBody> {
    match (req.method(), path) {
        (&Method::POST, "/detect") => handle_detect(req).await,
        (&Method::POST, "/preview") => handle_preview(req).await,
        (&Method::POST, "/import") => handle_import(req, state).await,
        (&Method::POST, "/export/rules") => handle_export_rules(req, state).await,
        (&Method::POST, "/export/network") => handle_export_network(req, state).await,
        (&Method::POST, "/export/scripts") => handle_export_scripts(req, state).await,
        (&Method::POST, "/export/values") => handle_export_values(req, state).await,
        (&Method::POST, "/export/templates") => handle_export_templates(req, state).await,
        _ => method_not_allowed(),
    }
}

async fn read_body(req: Request<Incoming>) -> Result<String, Response<BoxBody>> {
    let body_bytes = req
        .into_body()
        .collect()
        .await
        .map_err(|e| {
            error_response(
                StatusCode::BAD_REQUEST,
                &format!("Failed to read body: {}", e),
            )
        })?
        .to_bytes();

    String::from_utf8(body_bytes.to_vec())
        .map_err(|e| error_response(StatusCode::BAD_REQUEST, &format!("Invalid UTF-8: {}", e)))
}

async fn read_json<T: for<'de> Deserialize<'de>>(
    req: Request<Incoming>,
) -> Result<T, Response<BoxBody>> {
    let body = read_body(req).await?;
    serde_json::from_str(&body)
        .map_err(|e| error_response(StatusCode::BAD_REQUEST, &format!("Invalid JSON: {}", e)))
}

async fn handle_detect(req: Request<Incoming>) -> Response<BoxBody> {
    let content = match read_body(req).await {
        Ok(c) => c,
        Err(resp) => return resp,
    };

    let file_type = match BifrostFileParser::detect_type(&content) {
        Ok(t) => t,
        Err(e) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                &format!("Failed to detect type: {}", e),
            )
        }
    };

    let meta = match BifrostFileParser::parse_raw(&content) {
        Ok(raw) => toml::from_str::<toml::Value>(&raw.meta_raw)
            .map(toml_to_json)
            .unwrap_or(serde_json::Value::Null),
        Err(_) => serde_json::Value::Null,
    };

    json_response(&DetectResponse { file_type, meta })
}

async fn handle_preview(req: Request<Incoming>) -> Response<BoxBody> {
    let content = match read_body(req).await {
        Ok(c) => c,
        Err(resp) => return resp,
    };

    match build_preview(&content) {
        Ok(preview) => json_response(&preview),
        Err(message) => error_response(StatusCode::BAD_REQUEST, &message),
    }
}

fn build_preview(content: &str) -> Result<PreviewResponse, String> {
    let file_type = BifrostFileParser::detect_type(content)
        .map_err(|e| format!("Failed to detect type: {}", e))?;
    let meta = BifrostFileParser::parse_raw(content)
        .ok()
        .and_then(|raw| toml::from_str::<toml::Value>(&raw.meta_raw).ok())
        .map(toml_to_json)
        .unwrap_or(serde_json::Value::Null);

    match file_type {
        BifrostFileType::Rules => {
            let result = BifrostFileParser::parse_rules_tolerant(content);
            let file = result.data;
            let line_count = file
                .content
                .lines()
                .filter(|line| !line.trim().is_empty())
                .count();
            Ok(PreviewResponse {
                file_type,
                meta,
                rules: Some(RulesPreview {
                    name: file.meta.name,
                    enabled: file.meta.enabled,
                    description: file.meta.description,
                    line_count,
                    content: file.content,
                }),
                network: None,
                item_count: Some(1),
            })
        }
        BifrostFileType::Network => {
            let file = BifrostFileParser::parse_network(content)
                .map_err(|e| format!("Failed to parse network file: {}", e))?;
            Ok(PreviewResponse {
                file_type,
                meta,
                rules: None,
                item_count: Some(file.content.len()),
                network: Some(build_network_preview(&file.content)),
            })
        }
        BifrostFileType::Script => {
            let file = BifrostFileParser::parse_script(content)
                .map_err(|e| format!("Failed to parse script file: {}", e))?;
            Ok(PreviewResponse {
                file_type,
                meta,
                rules: None,
                network: None,
                item_count: Some(file.content.len()),
            })
        }
        BifrostFileType::Values => {
            let file = BifrostFileParser::parse_values(content)
                .map_err(|e| format!("Failed to parse values file: {}", e))?;
            Ok(PreviewResponse {
                file_type,
                meta,
                rules: None,
                network: None,
                item_count: Some(file.content.len()),
            })
        }
        BifrostFileType::Template => {
            let file = BifrostFileParser::parse_template(content)
                .map_err(|e| format!("Failed to parse template file: {}", e))?;
            Ok(PreviewResponse {
                file_type,
                meta,
                rules: None,
                network: None,
                item_count: Some(file.content.groups.len() + file.content.requests.len()),
            })
        }
    }
}

fn build_network_preview(records: &[NetworkRecord]) -> NetworkPreview {
    let mut hosts = BTreeSet::new();
    let preview_records: Vec<NetworkPreviewRecord> = records
        .iter()
        .take(50)
        .map(|record| {
            let traffic_record = network_record_to_traffic_record(record);
            hosts.insert(traffic_record.host.clone());
            NetworkPreviewRecord {
                id: record.id.clone(),
                method: record.method.clone(),
                url: record.url.clone(),
                status: record.status,
                host: traffic_record.host,
                path: traffic_record.path,
                protocol: traffic_record.protocol,
                client_app: record.client_app.clone(),
                duration_ms: record.duration_ms,
                request_size: record.request_body.as_ref().map_or(0, |body| body.len()),
                response_size: record.response_body.as_ref().map_or(0, |body| body.len()),
            }
        })
        .collect();

    for record in records.iter().skip(50) {
        let traffic_record = network_record_to_traffic_record(record);
        hosts.insert(traffic_record.host);
    }

    let single_record = records
        .first()
        .filter(|_| records.len() == 1)
        .map(|record| NetworkPreviewDetail {
            record: network_record_to_traffic_record(record),
            request_body: record.request_body.clone(),
            response_body: record.response_body.clone(),
        });

    NetworkPreview {
        record_count: records.len(),
        hosts: hosts.into_iter().take(20).collect(),
        records: preview_records,
        single_record,
    }
}

async fn handle_import(req: Request<Incoming>, state: SharedAdminState) -> Response<BoxBody> {
    let content = match read_body(req).await {
        Ok(c) => c,
        Err(resp) => return resp,
    };

    let file_type = match BifrostFileParser::detect_type(&content) {
        Ok(t) => t,
        Err(e) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                &format!("Failed to detect type: {}", e),
            )
        }
    };

    match file_type {
        BifrostFileType::Rules => import_rules(&content, &state).await,
        BifrostFileType::Network => import_network(&content, &state).await,
        BifrostFileType::Script => import_scripts(&content, &state).await,
        BifrostFileType::Values => import_values(&content, &state).await,
        BifrostFileType::Template => import_templates(&content, &state).await,
    }
}

async fn import_rules(content: &str, state: &SharedAdminState) -> Response<BoxBody> {
    let config_manager = match &state.config_manager {
        Some(cm) => cm.clone(),
        None => {
            return error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "Config manager not configured",
            )
        }
    };

    let result = BifrostFileParser::parse_rules_tolerant(content);
    let mut warnings: Vec<String> = result
        .warnings
        .iter()
        .map(|w| format!("[{}] {}", w.level, w.message))
        .collect();
    let file = result.data;
    let normalized_content = normalize_rule_content(&file.content);

    if normalized_content != file.content {
        warnings
            .push("Converted legacy ignore:// rules to passthrough:// during import".to_string());
    }

    let rule = bifrost_storage::RuleFile::new(file.meta.name.clone(), normalized_content)
        .with_enabled(file.meta.enabled)
        .with_sort_order(file.meta.sort_order)
        .with_description(file.meta.description);

    if let Err(e) = config_manager.save_rule(&rule).await {
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("Failed to save rule: {}", e),
        );
    }

    json_response(&ImportResponse {
        success: true,
        file_type: BifrostFileType::Rules,
        data: ImportedData {
            rule_names: Some(vec![file.meta.name]),
            rule_count: Some(1),
            ..Default::default()
        },
        warnings,
    })
}

async fn import_network(content: &str, state: &SharedAdminState) -> Response<BoxBody> {
    let traffic_db_store = match &state.traffic_db_store {
        Some(store) => store.clone(),
        None => {
            return error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "Traffic database store not configured",
            )
        }
    };

    let file = match BifrostFileParser::parse_network(content) {
        Ok(f) => f,
        Err(e) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                &format!("Failed to parse network file: {}", e),
            )
        }
    };

    let mut warnings: Vec<String> = Vec::new();
    let mut record_count = 0;

    if let Err(message) = validate_network_import_records(file.content.len()) {
        return error_response(StatusCode::BAD_REQUEST, message);
    }

    for network_record in &file.content {
        let traffic_record = network_record_to_traffic_record(network_record);
        traffic_db_store.record(traffic_record);
        record_count += 1;
    }

    if record_count > 0 {
        warnings.push(format!(
            "Imported {} record(s) with 'OUT-' prefix IDs",
            record_count
        ));
    }

    json_response(&ImportResponse {
        success: true,
        file_type: BifrostFileType::Network,
        data: ImportedData {
            record_count: Some(record_count),
            ..Default::default()
        },
        warnings,
    })
}

fn network_record_to_traffic_record(record: &NetworkRecord) -> TrafficRecord {
    let parsed_url = url::Url::parse(&record.url).ok();
    let fallback_host = parsed_url
        .as_ref()
        .and_then(|u| u.host_str())
        .map(|h| h.to_string())
        .unwrap_or_default();
    let fallback_path = parsed_url
        .as_ref()
        .map(|u| {
            let p = u.path();
            if let Some(q) = u.query() {
                format!("{}?{}", p, q)
            } else {
                p.to_string()
            }
        })
        .unwrap_or_default();
    let fallback_protocol = parsed_url
        .as_ref()
        .map(|u| u.scheme().to_uppercase())
        .unwrap_or_else(|| "HTTP".to_string());

    let matched_rules: Option<Vec<crate::traffic::MatchedRule>> =
        record.matched_rules.as_ref().map(|rules| {
            rules
                .iter()
                .map(|r| crate::traffic::MatchedRule {
                    pattern: r.pattern.clone(),
                    protocol: r.protocol.clone(),
                    value: r.value.clone(),
                    rule_name: None,
                    raw: None,
                    line: None,
                })
                .collect()
        });

    let has_rule_hit = matched_rules.as_ref().is_some_and(|r| !r.is_empty());

    let imported_id = format!("OUT-{}", record.id);

    let request_size = record.request_body.as_ref().map_or(0, |b| b.len());
    let response_size = record.response_body.as_ref().map_or(0, |b| b.len());
    let (original_response_headers, response_headers) =
        if let Some(original) = &record.original_response_headers {
            let delivered = record
                .response_headers
                .clone()
                .unwrap_or_else(|| original.clone());
            let changed = (delivered != *original).then_some(delivered);
            (Some(original.clone()), changed)
        } else {
            // Backward compatibility: network exports created before the two-snapshot
            // format stored the upstream snapshot in `response_headers`.
            (record.response_headers.clone(), None)
        };

    TrafficRecord {
        id: imported_id,
        sequence: 0,
        timestamp: record.timestamp,
        method: record.method.clone(),
        url: record.url.clone(),
        status: record.status,
        content_type: None,
        request_size,
        response_size,
        upload_bytes: request_size,
        download_bytes: response_size,
        duration_ms: record.duration_ms,
        listener_port: record.listener_port.unwrap_or(0),
        timing: None,
        request_headers: record.request_headers.clone(),
        original_response_headers,
        request_body_ref: None,
        response_body_ref: None,
        derived_response_body_ref: None,
        raw_request_body_ref: None,
        raw_response_body_ref: None,
        client_ip: "imported".to_string(),
        client_app: record
            .client_app
            .clone()
            .or_else(|| Some("Bifrost Import".to_string())),
        client_pid: None,
        client_path: record.client_path.clone(),
        account_name: None,
        host: record.host.clone().unwrap_or(fallback_host),
        path: record.path.clone().unwrap_or(fallback_path),
        protocol: record.protocol.clone().unwrap_or(fallback_protocol),
        actual_url: record.actual_url.clone(),
        actual_host: record.actual_host.clone(),
        original_request_headers: None,
        response_headers,
        is_tunnel: false,
        has_rule_hit: record.has_rule_hit.unwrap_or(has_rule_hit),
        matched_rules,
        request_content_type: None,
        is_websocket: false,
        is_sse: false,
        is_h3: false,
        is_replay: false,
        socket_status: None,
        frame_count: 0,
        last_frame_id: 0,
        error_message: record.error_message.clone(),
        req_script_results: None,
        res_script_results: None,
        decode_req_script_results: None,
        decode_res_script_results: None,
        devtools_client_req_id: None,
    }
}

async fn import_scripts(content: &str, state: &SharedAdminState) -> Response<BoxBody> {
    let script_manager = match &state.script_manager {
        Some(sm) => sm.clone(),
        None => {
            return error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "Script manager not configured",
            )
        }
    };

    let file = match BifrostFileParser::parse_script(content) {
        Ok(f) => f,
        Err(e) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                &format!("Failed to parse script file: {}", e),
            )
        }
    };

    let mut imported_names = Vec::new();
    let mut warnings = Vec::new();

    let manager = script_manager.read().await;
    for script in &file.content {
        let script_type = match script.script_type.as_str() {
            "request" => bifrost_script::ScriptType::Request,
            "response" => bifrost_script::ScriptType::Response,
            "decode" => bifrost_script::ScriptType::Decode,
            "parser" => bifrost_script::ScriptType::Parser,
            _ => {
                warnings.push(format!(
                    "Invalid script type for '{}': {}",
                    script.name, script.script_type
                ));
                continue;
            }
        };

        if let Err(e) = manager
            .engine()
            .save_script(script_type, &script.name, &script.content)
            .await
        {
            warnings.push(format!("Failed to save script '{}': {}", script.name, e));
        } else {
            imported_names.push(script.name.clone());
        }
    }

    json_response(&ImportResponse {
        success: true,
        file_type: BifrostFileType::Script,
        data: ImportedData {
            script_names: Some(imported_names.clone()),
            script_count: Some(imported_names.len()),
            ..Default::default()
        },
        warnings,
    })
}

async fn import_values(content: &str, state: &SharedAdminState) -> Response<BoxBody> {
    let values_storage = match &state.values_storage {
        Some(vs) => vs.clone(),
        None => {
            return error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "Values storage not configured",
            )
        }
    };

    let file = match BifrostFileParser::parse_values(content) {
        Ok(f) => f,
        Err(e) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                &format!("Failed to parse values file: {}", e),
            )
        }
    };

    let mut imported_names = Vec::new();
    let mut warnings = Vec::new();

    {
        let mut storage = values_storage.write();
        for (key, value) in &file.content {
            if let Err(e) = storage.set_value(key, value) {
                warnings.push(format!("Failed to set value '{}': {}", key, e));
            } else {
                imported_names.push(key.clone());
            }
        }
    }

    json_response(&ImportResponse {
        success: true,
        file_type: BifrostFileType::Values,
        data: ImportedData {
            value_names: Some(imported_names.clone()),
            value_count: Some(imported_names.len()),
            ..Default::default()
        },
        warnings,
    })
}

async fn import_templates(content: &str, state: &SharedAdminState) -> Response<BoxBody> {
    let replay_db_store = match &state.replay_db_store {
        Some(db) => db.clone(),
        None => return error_response(StatusCode::SERVICE_UNAVAILABLE, "Replay DB not configured"),
    };

    let file = match BifrostFileParser::parse_template(content) {
        Ok(f) => f,
        Err(e) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                &format!("Failed to parse template file: {}", e),
            )
        }
    };

    let mut warnings = Vec::new();
    let mut group_count = 0;
    let mut request_count = 0;

    for group in &file.content.groups {
        let replay_group = crate::replay_db::ReplayGroup {
            id: group.id.clone(),
            name: group.name.clone(),
            parent_id: group.parent_id.clone(),
            sort_order: group.sort_order,
            created_at: group.created_at,
            updated_at: group.updated_at,
        };
        if let Err(e) = replay_db_store.create_group(&replay_group) {
            warnings.push(format!("Failed to save group '{}': {}", group.name, e));
        } else {
            group_count += 1;
        }
    }

    let mut next_seq = replay_db_store.next_imported_sequence();
    for request in &file.content.requests {
        let replay_request = convert_to_replay_request(request, next_seq);
        if let Err(e) = replay_db_store.create_request(&replay_request) {
            warnings.push(format!(
                "Failed to save request '{}': {}",
                replay_request.id, e
            ));
        } else {
            request_count += 1;
            next_seq += 1;
        }
    }

    json_response(&ImportResponse {
        success: true,
        file_type: BifrostFileType::Template,
        data: ImportedData {
            group_count: Some(group_count),
            request_count: Some(request_count),
            ..Default::default()
        },
        warnings,
    })
}

fn convert_to_replay_request(
    export: &ReplayRequestExport,
    seq: usize,
) -> crate::replay_db::ReplayRequest {
    let request_type = match export.request_type.as_str() {
        "sse" => crate::replay_db::RequestType::Sse,
        "websocket" => crate::replay_db::RequestType::WebSocket,
        _ => crate::replay_db::RequestType::Http,
    };

    let headers: Vec<crate::replay_db::KeyValueItem> = export
        .headers
        .iter()
        .map(|h| crate::replay_db::KeyValueItem {
            id: h.id.clone(),
            key: h.key.clone(),
            value: h.value.clone(),
            enabled: h.enabled,
            description: h.description.clone(),
        })
        .collect();

    let body = export.body.as_ref().map(|b| {
        let body_type = match b.body_type.as_str() {
            "form-data" => crate::replay_db::BodyType::FormData,
            "x-www-form-urlencoded" => crate::replay_db::BodyType::XWwwFormUrlencoded,
            "raw" => crate::replay_db::BodyType::Raw,
            "binary" => crate::replay_db::BodyType::Binary,
            _ => crate::replay_db::BodyType::None,
        };

        let raw_type = b.raw_type.as_ref().map(|rt| match rt.as_str() {
            "json" => crate::replay_db::RawType::Json,
            "xml" => crate::replay_db::RawType::Xml,
            "javascript" => crate::replay_db::RawType::Javascript,
            "html" => crate::replay_db::RawType::Html,
            _ => crate::replay_db::RawType::Text,
        });

        let form_data: Vec<crate::replay_db::KeyValueItem> = b
            .form_data
            .iter()
            .map(|f| crate::replay_db::KeyValueItem {
                id: f.id.clone(),
                key: f.key.clone(),
                value: f.value.clone(),
                enabled: f.enabled,
                description: f.description.clone(),
            })
            .collect();

        crate::replay_db::ReplayBody {
            body_type,
            raw_type,
            content: b.content.clone(),
            form_data,
            binary_file: b.binary_file.clone(),
        }
    });

    let imported_id = format!("OUT-{:03}", seq);

    crate::replay_db::ReplayRequest {
        id: imported_id,
        group_id: export.group_id.clone(),
        name: export.name.clone(),
        request_type,
        method: export.method.clone(),
        url: export.url.clone(),
        headers,
        body,
        is_saved: export.is_saved,
        sort_order: export.sort_order,
        source: crate::replay_db::RequestSource::Imported,
        created_at: export.created_at,
        updated_at: export.updated_at,
    }
}

async fn handle_export_rules(req: Request<Incoming>, state: SharedAdminState) -> Response<BoxBody> {
    let config_manager = match &state.config_manager {
        Some(cm) => cm.clone(),
        None => {
            return error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "Config manager not configured",
            )
        }
    };

    let request: ExportRulesRequest = match read_json(req).await {
        Ok(r) => r,
        Err(resp) => return resp,
    };

    let mut all_content = String::new();
    let mut all_names = Vec::new();

    for name in &request.rule_names {
        match config_manager.load_rule(name).await {
            Ok(rule) => {
                if !all_content.is_empty() {
                    all_content.push_str("\n\n");
                }
                all_content.push_str(&format!("# === {} ===\n", name));
                all_content.push_str(&rule.content);
                all_names.push(name.clone());
            }
            Err(e) => {
                tracing::warn!(name = %name, error = %e, "Failed to load rule for export");
            }
        }
    }

    let export_name = if all_names.len() == 1 {
        all_names[0].clone()
    } else {
        format!("rules-export-{}", all_names.len())
    };

    let meta = bifrost_core::bifrost_file::RuleFileMeta {
        name: export_name,
        enabled: true,
        sort_order: 0,
        version: "1.0.0".to_string(),
        created_at: chrono::Utc::now().to_rfc3339(),
        updated_at: chrono::Utc::now().to_rfc3339(),
        description: request.description,
        group: None,
        sync: bifrost_core::bifrost_file::RuleSyncMeta::default(),
    };

    let output = BifrostFileWriter::write_rules(&meta, &all_content);

    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "text/plain; charset=utf-8")
        .body(full_body(output))
        .unwrap()
}

async fn handle_export_network(
    req: Request<Incoming>,
    state: SharedAdminState,
) -> Response<BoxBody> {
    let request: ExportNetworkRequest = match read_json(req).await {
        Ok(r) => r,
        Err(resp) => return resp,
    };

    let include_body = request.include_body.unwrap_or(true);
    let mut records: Vec<NetworkRecord> = Vec::new();
    let mut missing_ids: Vec<String> = Vec::new();

    if request.record_ids.is_empty() {
        return error_response(
            StatusCode::BAD_REQUEST,
            "Select at least one Network record before exporting a .bifrost file",
        );
    }

    for id in &request.record_ids {
        let traffic = if let Some(ref db_store) = state.traffic_db_store {
            db_store.get_by_id(id)
        } else {
            return error_response(StatusCode::SERVICE_UNAVAILABLE, "Traffic DB not configured");
        };

        if let Some(traffic) = traffic {
            records.push(traffic_to_network_record(&traffic, include_body, &state).await);
        } else {
            missing_ids.push(id.clone());
        }
    }

    if let Err(message) =
        validate_network_export_records(&request.record_ids, &missing_ids, records.len())
    {
        return error_response(StatusCode::BAD_REQUEST, &message);
    }

    let export_name = format!("network-export-{}", records.len());

    let output = match BifrostFileWriter::write_network(
        &export_name,
        request.description.as_deref(),
        &records,
    ) {
        Ok(o) => o,
        Err(e) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("Failed to write network file: {}", e),
            )
        }
    };

    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "text/plain; charset=utf-8")
        .body(full_body(output))
        .unwrap()
}

fn validate_network_import_records(record_count: usize) -> Result<(), &'static str> {
    if record_count == 0 {
        Err(EMPTY_NETWORK_IMPORT_ERROR)
    } else {
        Ok(())
    }
}

fn validate_network_export_records(
    requested_ids: &[String],
    missing_ids: &[String],
    exported_count: usize,
) -> Result<(), String> {
    if requested_ids.is_empty() {
        return Err(
            "Select at least one Network record before exporting a .bifrost file".to_string(),
        );
    }

    if !missing_ids.is_empty() {
        return Err(format!(
            "Failed to export network file: {} selected record(s) no longer exist: {}",
            missing_ids.len(),
            missing_ids.join(", ")
        ));
    }

    if exported_count == 0 {
        return Err("Network export produced 0 records; nothing to write".to_string());
    }

    Ok(())
}

async fn traffic_to_network_record(
    traffic: &TrafficRecord,
    include_body: bool,
    state: &SharedAdminState,
) -> NetworkRecord {
    let mut request_body = None;
    let mut response_body = None;

    if include_body {
        if let Some(ref body_store) = state.body_store {
            let store = body_store.read();
            if let Some(ref body_ref) = traffic.request_body_ref {
                request_body = store.load(body_ref);
            }
            if let Some(ref body_ref) = traffic.response_body_ref {
                response_body = store.load(body_ref);
            }
        }
    }

    let matched_rules = traffic.matched_rules.as_ref().map(|rules| {
        rules
            .iter()
            .map(|r| MatchedRuleExport {
                pattern: r.pattern.clone(),
                protocol: r.protocol.clone(),
                value: r.value.clone(),
            })
            .collect()
    });

    NetworkRecord {
        id: traffic.id.clone(),
        method: traffic.method.clone(),
        url: traffic.url.clone(),
        status: traffic.status,
        host: Some(traffic.host.clone()),
        path: Some(traffic.path.clone()),
        protocol: Some(traffic.protocol.clone()),
        actual_url: traffic.actual_url.clone(),
        actual_host: traffic.actual_host.clone(),
        listener_port: Some(traffic.listener_port),
        has_rule_hit: Some(traffic.has_rule_hit),
        error_message: traffic.error_message.clone(),
        client_app: traffic.client_app.clone(),
        client_path: traffic.client_path.clone(),
        request_headers: traffic.request_headers.clone(),
        response_headers: traffic
            .response_headers
            .clone()
            .or_else(|| traffic.original_response_headers.clone()),
        original_response_headers: traffic.original_response_headers.clone(),
        request_body,
        response_body,
        duration_ms: traffic.duration_ms,
        timestamp: traffic.timestamp,
        matched_rules,
        active_rules: Some(build_active_rules_export(traffic.listener_port, state).await),
    }
}

async fn build_active_rules_export(
    listener_port: u16,
    state: &SharedAdminState,
) -> ActiveRulesExport {
    let admin_port = state.port();
    if listener_port != 0 && listener_port != admin_port {
        return build_custom_port_active_rules_export(listener_port, state).await;
    }
    build_default_port_active_rules_export(state)
}

async fn build_custom_port_active_rules_export(
    listener_port: u16,
    state: &SharedAdminState,
) -> ActiveRulesExport {
    let admin_port = state.port();
    let Some(manager) = state.temporary_port_manager() else {
        return unavailable_active_rules_export(
            ActiveRuleSource::CustomPort,
            admin_port,
            listener_port,
            "Temporary port manager is not configured",
        );
    };

    match manager.active_summary(listener_port).await {
        Ok(summary) => {
            let rules = summary
                .rules
                .into_iter()
                .map(|rule| ActiveRuleExport {
                    name: rule.name,
                    rule_count: rule.rule_count,
                    group_id: rule.group_id,
                    group_name: rule.group_name,
                    content: rule.content,
                })
                .collect();
            ActiveRulesExport {
                source: ActiveRuleSource::CustomPort,
                admin_port,
                listener_port: summary.port,
                total: summary.total,
                rules,
                merged_content: summary.merged_content,
                unavailable_reason: None,
            }
        }
        Err(error) => unavailable_active_rules_export(
            ActiveRuleSource::CustomPort,
            admin_port,
            listener_port,
            error.message,
        ),
    }
}

fn build_default_port_active_rules_export(state: &SharedAdminState) -> ActiveRulesExport {
    let admin_port = state.port();
    match collect_default_active_rules(state) {
        Ok((rules, merged_content)) => ActiveRulesExport {
            source: ActiveRuleSource::DefaultPort,
            admin_port,
            listener_port: admin_port,
            total: rules.len(),
            rules,
            merged_content,
            unavailable_reason: None,
        },
        Err(error) => unavailable_active_rules_export(
            ActiveRuleSource::DefaultPort,
            admin_port,
            admin_port,
            error,
        ),
    }
}

fn unavailable_active_rules_export(
    source: ActiveRuleSource,
    admin_port: u16,
    listener_port: u16,
    reason: impl Into<String>,
) -> ActiveRulesExport {
    ActiveRulesExport {
        source,
        admin_port,
        listener_port,
        total: 0,
        rules: Vec::new(),
        merged_content: String::new(),
        unavailable_reason: Some(reason.into()),
    }
}

fn collect_default_active_rules(
    state: &SharedAdminState,
) -> Result<(Vec<ActiveRuleExport>, String), String> {
    let mut rules = Vec::new();
    let mut content_parts = Vec::new();
    let base_dir = state.rules_storage.base_dir();

    if !base_dir.exists() {
        return Ok((rules, String::new()));
    }

    if let Err(error) = state.rules_storage.ensure_default_rule() {
        tracing::warn!(
            error = %error,
            "failed to initialize Default rule for active rules export"
        );
    }

    collect_enabled_rule_files(
        &state.rules_storage,
        None,
        None,
        &mut rules,
        &mut content_parts,
    )
    .map_err(|e| format!("Failed to load default enabled rules: {e}"))?;

    let entries =
        std::fs::read_dir(base_dir).map_err(|e| format!("Failed to read rule directories: {e}"))?;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let dir_name = entry.file_name().to_string_lossy().to_string();
        let group_id = {
            let cache = state.group_name_cache();
            cache.reverse_lookup(&dir_name)
        };
        let group_storage = RulesStorage::with_dir(path)
            .map_err(|e| format!("Failed to open group rule directory '{dir_name}': {e}"))?;
        collect_enabled_rule_files(
            &group_storage,
            group_id.as_deref(),
            Some(&dir_name),
            &mut rules,
            &mut content_parts,
        )
        .map_err(|e| format!("Failed to load enabled rules from '{dir_name}': {e}"))?;
    }

    Ok((rules, content_parts.join("\n")))
}

fn collect_enabled_rule_files(
    storage: &RulesStorage,
    group_id: Option<&str>,
    group_name: Option<&str>,
    rules: &mut Vec<ActiveRuleExport>,
    content_parts: &mut Vec<String>,
) -> Result<(), String> {
    let enabled = storage.load_enabled().map_err(|e| e.to_string())?;
    for rule in enabled {
        content_parts.push(rule.content.clone());
        rules.push(active_rule_from_file(rule, group_id, group_name));
    }
    Ok(())
}

fn active_rule_from_file(
    rule: RuleFile,
    group_id: Option<&str>,
    group_name: Option<&str>,
) -> ActiveRuleExport {
    ActiveRuleExport {
        name: rule.name,
        rule_count: count_rules(&rule.content),
        group_id: group_id.map(str::to_string),
        group_name: group_name.map(str::to_string),
        content: Some(rule.content),
    }
}

fn count_rules(content: &str) -> usize {
    content
        .lines()
        .filter(|line| {
            let trimmed = line.trim();
            !trimmed.is_empty() && !trimmed.starts_with('#')
        })
        .count()
}

async fn handle_export_scripts(
    req: Request<Incoming>,
    state: SharedAdminState,
) -> Response<BoxBody> {
    let script_manager = match &state.script_manager {
        Some(sm) => sm.clone(),
        None => {
            return error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "Script manager not configured",
            )
        }
    };

    let request: ExportScriptRequest = match read_json(req).await {
        Ok(r) => r,
        Err(resp) => return resp,
    };

    let mut scripts: Vec<ScriptItem> = Vec::new();

    let manager = script_manager.read().await;
    let request_scripts = manager
        .engine()
        .list_scripts(bifrost_script::ScriptType::Request)
        .await
        .unwrap_or_default();
    let response_scripts = manager
        .engine()
        .list_scripts(bifrost_script::ScriptType::Response)
        .await
        .unwrap_or_default();
    let decode_scripts = manager
        .engine()
        .list_scripts(bifrost_script::ScriptType::Decode)
        .await
        .unwrap_or_default();
    let parser_scripts = manager
        .engine()
        .list_scripts(bifrost_script::ScriptType::Parser)
        .await
        .unwrap_or_default();
    let all_scripts: Vec<_> = request_scripts
        .into_iter()
        .chain(response_scripts)
        .chain(decode_scripts)
        .chain(parser_scripts)
        .collect();

    for name in &request.script_names {
        let parts: Vec<&str> = name.splitn(2, '/').collect();
        if parts.len() != 2 {
            continue;
        }

        let script_type = match parts[0] {
            "request" => bifrost_script::ScriptType::Request,
            "response" => bifrost_script::ScriptType::Response,
            "decode" => bifrost_script::ScriptType::Decode,
            "parser" => bifrost_script::ScriptType::Parser,
            _ => continue,
        };
        let script_name = parts[1];

        if let Some(info) = all_scripts
            .iter()
            .find(|s| s.name == script_name && s.script_type == script_type)
        {
            if let Ok(content) = manager.engine().load_script(script_type, script_name).await {
                scripts.push(ScriptItem {
                    name: script_name.to_string(),
                    script_type: match script_type {
                        bifrost_script::ScriptType::Request => "request".to_string(),
                        bifrost_script::ScriptType::Response => "response".to_string(),
                        bifrost_script::ScriptType::Decode => "decode".to_string(),
                        bifrost_script::ScriptType::Parser => "parser".to_string(),
                    },
                    description: info.description.clone(),
                    content,
                });
            }
        }
    }

    let export_name = format!("scripts-export-{}", scripts.len());

    let output = match BifrostFileWriter::write_script(
        &export_name,
        request.description.as_deref(),
        &scripts,
    ) {
        Ok(o) => o,
        Err(e) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("Failed to write script file: {}", e),
            )
        }
    };

    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "text/plain; charset=utf-8")
        .body(full_body(output))
        .unwrap()
}

async fn handle_export_values(
    req: Request<Incoming>,
    state: SharedAdminState,
) -> Response<BoxBody> {
    let values_storage = match &state.values_storage {
        Some(vs) => vs.clone(),
        None => {
            return error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "Values storage not configured",
            )
        }
    };

    let request: ExportValuesRequest = match read_json(req).await {
        Ok(r) => r,
        Err(resp) => return resp,
    };

    let all_values: Vec<(String, String)> = {
        let storage = values_storage.read();
        storage
            .list_entries()
            .unwrap_or_default()
            .into_iter()
            .map(|e| (e.name, e.value))
            .collect()
    };
    let mut values: ValuesContent = ValuesContent::new();

    match &request.value_names {
        Some(names) => {
            for (key, value) in all_values {
                if names.contains(&key) {
                    values.insert(key, value);
                }
            }
        }
        None => {
            for (key, value) in all_values {
                values.insert(key, value);
            }
        }
    }

    let export_name = format!("values-export-{}", values.len());

    let output = match BifrostFileWriter::write_values(
        &export_name,
        request.description.as_deref(),
        &values,
    ) {
        Ok(o) => o,
        Err(e) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("Failed to write values file: {}", e),
            )
        }
    };

    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "text/plain; charset=utf-8")
        .body(full_body(output))
        .unwrap()
}

async fn handle_export_templates(
    req: Request<Incoming>,
    state: SharedAdminState,
) -> Response<BoxBody> {
    let replay_db_store = match &state.replay_db_store {
        Some(db) => db.clone(),
        None => return error_response(StatusCode::SERVICE_UNAVAILABLE, "Replay DB not configured"),
    };

    let request: ExportTemplateRequest = match read_json(req).await {
        Ok(r) => r,
        Err(resp) => return resp,
    };

    let all_groups = replay_db_store.list_groups();
    let all_request_summaries = replay_db_store.list_requests(None, None, None, None);

    let mut groups: Vec<ReplayGroupExport> = Vec::new();
    let mut requests: Vec<ReplayRequestExport> = Vec::new();

    let group_ids = request.group_ids.as_ref();
    let request_ids = request.request_ids.as_ref();

    if let Some(ids) = request_ids {
        for summary in &all_request_summaries {
            if ids.contains(&summary.id) {
                if let Some(req) = replay_db_store.get_request(&summary.id) {
                    requests.push(convert_from_replay_request(&req));
                }
            }
        }

        let mut needed_group_ids: Vec<String> =
            requests.iter().filter_map(|r| r.group_id.clone()).collect();
        needed_group_ids.sort();
        needed_group_ids.dedup();

        for group in &all_groups {
            if needed_group_ids.contains(&group.id) {
                groups.push(ReplayGroupExport {
                    id: group.id.clone(),
                    name: group.name.clone(),
                    parent_id: group.parent_id.clone(),
                    sort_order: group.sort_order,
                    created_at: group.created_at,
                    updated_at: group.updated_at,
                });
            }
        }
    } else if let Some(ids) = group_ids {
        for group in &all_groups {
            if ids.contains(&group.id) {
                groups.push(ReplayGroupExport {
                    id: group.id.clone(),
                    name: group.name.clone(),
                    parent_id: group.parent_id.clone(),
                    sort_order: group.sort_order,
                    created_at: group.created_at,
                    updated_at: group.updated_at,
                });
            }
        }

        for summary in &all_request_summaries {
            if let Some(ref gid) = summary.group_id {
                if ids.contains(gid) {
                    if let Some(req) = replay_db_store.get_request(&summary.id) {
                        requests.push(convert_from_replay_request(&req));
                    }
                }
            }
        }
    } else {
        for group in &all_groups {
            groups.push(ReplayGroupExport {
                id: group.id.clone(),
                name: group.name.clone(),
                parent_id: group.parent_id.clone(),
                sort_order: group.sort_order,
                created_at: group.created_at,
                updated_at: group.updated_at,
            });
        }
        for summary in &all_request_summaries {
            if let Some(req) = replay_db_store.get_request(&summary.id) {
                requests.push(convert_from_replay_request(&req));
            }
        }
    }

    let template = TemplateContent { groups, requests };
    let export_name = format!("templates-export-{}", template.requests.len());

    let output = match BifrostFileWriter::write_template(
        &export_name,
        request.description.as_deref(),
        &template,
    ) {
        Ok(o) => o,
        Err(e) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("Failed to write template file: {}", e),
            )
        }
    };

    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "text/plain; charset=utf-8")
        .body(full_body(output))
        .unwrap()
}

fn convert_from_replay_request(req: &crate::replay_db::ReplayRequest) -> ReplayRequestExport {
    let headers: Vec<KeyValueItemExport> = req
        .headers
        .iter()
        .map(|h| KeyValueItemExport {
            id: h.id.clone(),
            key: h.key.clone(),
            value: h.value.clone(),
            enabled: h.enabled,
            description: h.description.clone(),
        })
        .collect();

    let body = req.body.as_ref().map(|b| {
        let body_type = match b.body_type {
            crate::replay_db::BodyType::FormData => "form-data".to_string(),
            crate::replay_db::BodyType::XWwwFormUrlencoded => "x-www-form-urlencoded".to_string(),
            crate::replay_db::BodyType::Raw => "raw".to_string(),
            crate::replay_db::BodyType::Binary => "binary".to_string(),
            crate::replay_db::BodyType::None => "none".to_string(),
        };

        let raw_type = b.raw_type.as_ref().map(|rt| match rt {
            crate::replay_db::RawType::Json => "json".to_string(),
            crate::replay_db::RawType::Xml => "xml".to_string(),
            crate::replay_db::RawType::Javascript => "javascript".to_string(),
            crate::replay_db::RawType::Html => "html".to_string(),
            crate::replay_db::RawType::Text => "text".to_string(),
        });

        let form_data: Vec<KeyValueItemExport> = b
            .form_data
            .iter()
            .map(|f| KeyValueItemExport {
                id: f.id.clone(),
                key: f.key.clone(),
                value: f.value.clone(),
                enabled: f.enabled,
                description: f.description.clone(),
            })
            .collect();

        ReplayBodyExport {
            body_type,
            raw_type,
            content: b.content.clone(),
            form_data,
            binary_file: b.binary_file.clone(),
        }
    });

    let request_type = match req.request_type {
        crate::replay_db::RequestType::Sse => "sse".to_string(),
        crate::replay_db::RequestType::WebSocket => "websocket".to_string(),
        crate::replay_db::RequestType::Http => "http".to_string(),
    };

    ReplayRequestExport {
        id: req.id.clone(),
        group_id: req.group_id.clone(),
        name: req.name.clone(),
        request_type,
        method: req.method.clone(),
        url: req.url.clone(),
        headers,
        body,
        is_saved: req.is_saved,
        sort_order: req.sort_order,
        created_at: req.created_at,
        updated_at: req.updated_at,
    }
}

fn toml_to_json(toml_val: toml::Value) -> serde_json::Value {
    match toml_val {
        toml::Value::String(s) => serde_json::Value::String(s),
        toml::Value::Integer(i) => serde_json::Value::Number(i.into()),
        toml::Value::Float(f) => serde_json::Number::from_f64(f)
            .map_or(serde_json::Value::Null, serde_json::Value::Number),
        toml::Value::Boolean(b) => serde_json::Value::Bool(b),
        toml::Value::Array(arr) => {
            serde_json::Value::Array(arr.into_iter().map(toml_to_json).collect())
        }
        toml::Value::Table(table) => {
            let map: serde_json::Map<String, serde_json::Value> = table
                .into_iter()
                .map(|(k, v)| (k, toml_to_json(v)))
                .collect();
            serde_json::Value::Object(map)
        }
        toml::Value::Datetime(dt) => serde_json::Value::String(dt.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::sync::Arc;

    #[test]
    fn network_record_import_preserves_routing_diagnostics() {
        let record = NetworkRecord {
            id: "REQ-exported".to_string(),
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
        };

        let traffic = network_record_to_traffic_record(&record);

        assert_eq!(traffic.id, "OUT-REQ-exported");
        assert_eq!(traffic.host, "lf6-cdn2-tos.bytegoofy.com");
        assert_eq!(traffic.path, "/index.html");
        assert_eq!(traffic.protocol, "https");
        assert_eq!(
            traffic.actual_url.as_deref(),
            Some("http://10.37.102.138:8081/index.html")
        );
        assert_eq!(traffic.actual_host.as_deref(), Some("10.37.102.138"));
        assert_eq!(traffic.listener_port, 9900);
        assert!(traffic.has_rule_hit);
        assert_eq!(traffic.error_message.as_deref(), Some("Request Failed"));
        assert_eq!(traffic.client_app.as_deref(), Some("Google Chrome"));
        assert_eq!(
            traffic
                .matched_rules
                .as_ref()
                .and_then(|rules| rules.first())
                .map(|r| { (r.pattern.as_str(), r.protocol.as_str(), r.value.as_str(),) }),
            Some((
                "lf6-cdn2-tos.bytegoofy.com/index.html",
                "Host",
                "10.37.102.138:8081"
            ))
        );
    }

    #[test]
    fn network_import_rejects_empty_package() {
        let err = validate_network_import_records(0).unwrap_err();
        assert_eq!(err, EMPTY_NETWORK_IMPORT_ERROR);
    }

    #[test]
    fn preview_rules_includes_rule_details() {
        let content = r#"01 rules

[meta]
name = "preview-rules"
enabled = false
description = "preview before import"

---
example.com proxy://127.0.0.1:8080
"#;

        let preview = build_preview(content).expect("rules preview");

        assert_eq!(preview.file_type, BifrostFileType::Rules);
        assert_eq!(preview.item_count, Some(1));
        let rules = preview.rules.expect("rules preview payload");
        assert_eq!(rules.name, "preview-rules");
        assert!(!rules.enabled);
        assert_eq!(rules.description.as_deref(), Some("preview before import"));
        assert_eq!(rules.line_count, 1);
        assert_eq!(rules.content.trim(), "example.com proxy://127.0.0.1:8080");
    }

    #[test]
    fn preview_single_network_record_includes_detail_payload() {
        let records = vec![NetworkRecord {
            id: "REQ-preview".to_string(),
            method: "POST".to_string(),
            url: "https://api.example.test/v1/items?limit=1".to_string(),
            status: 201,
            host: Some("api.example.test".to_string()),
            path: Some("/v1/items?limit=1".to_string()),
            protocol: Some("https".to_string()),
            actual_url: None,
            actual_host: None,
            listener_port: Some(9900),
            has_rule_hit: Some(false),
            error_message: None,
            client_app: Some("Preview Client".to_string()),
            client_path: None,
            request_headers: Some(vec![(
                "content-type".to_string(),
                "application/json".to_string(),
            )]),
            response_headers: Some(vec![(
                "content-type".to_string(),
                "application/json".to_string(),
            )]),
            original_response_headers: None,
            request_body: Some(r#"{"name":"preview"}"#.to_string()),
            response_body: Some(r#"{"ok":true}"#.to_string()),
            duration_ms: 42,
            timestamp: 1779283635053,
            matched_rules: None,
            active_rules: None,
        }];
        let content = BifrostFileWriter::write_network("preview-network", None, &records)
            .expect("network package");

        let preview = build_preview(&content).expect("network preview");

        assert_eq!(preview.file_type, BifrostFileType::Network);
        assert_eq!(preview.item_count, Some(1));
        let network = preview.network.expect("network preview payload");
        assert_eq!(network.record_count, 1);
        assert_eq!(network.hosts, vec!["api.example.test".to_string()]);
        assert_eq!(network.records.len(), 1);
        assert_eq!(network.records[0].method, "POST");
        let detail = network.single_record.expect("single record detail");
        assert_eq!(detail.record.id, "OUT-REQ-preview");
        assert_eq!(detail.record.host, "api.example.test");
        assert_eq!(
            detail.record.original_response_headers,
            Some(vec![(
                "content-type".to_string(),
                "application/json".to_string(),
            )])
        );
        assert!(detail.record.response_headers.is_none());
        assert_eq!(
            detail.request_body.as_deref(),
            Some(r#"{"name":"preview"}"#)
        );
        assert_eq!(detail.response_body.as_deref(), Some(r#"{"ok":true}"#));
    }

    #[test]
    fn network_export_rejects_empty_selection() {
        let err = validate_network_export_records(&[], &[], 0).unwrap_err();
        assert!(err.contains("Select at least one Network record"));
    }

    #[test]
    fn network_export_rejects_missing_selected_records() {
        let requested = vec!["A".to_string(), "B".to_string()];
        let missing = vec!["B".to_string()];

        let err = validate_network_export_records(&requested, &missing, 1).unwrap_err();

        assert!(err.contains("1 selected record(s) no longer exist"));
        assert!(err.contains("B"));
    }

    #[test]
    fn network_export_allows_resolved_records() {
        let requested = vec!["A".to_string()];

        validate_network_export_records(&requested, &[], 1).unwrap();
    }

    #[test]
    fn network_import_preserves_original_and_delivered_response_headers() {
        let original = vec![
            ("content-type".to_string(), "application/json".to_string()),
            ("connection".to_string(), "keep-alive".to_string()),
        ];
        let delivered = vec![("content-type".to_string(), "application/json".to_string())];
        let record = NetworkRecord {
            id: "REQ-header-snapshots".to_string(),
            method: "GET".to_string(),
            url: "https://example.test/headers".to_string(),
            status: 200,
            host: None,
            path: None,
            protocol: None,
            actual_url: None,
            actual_host: None,
            listener_port: None,
            has_rule_hit: Some(false),
            error_message: None,
            client_app: None,
            client_path: None,
            request_headers: None,
            response_headers: Some(delivered.clone()),
            original_response_headers: Some(original.clone()),
            request_body: None,
            response_body: None,
            duration_ms: 1,
            timestamp: 1,
            matched_rules: None,
            active_rules: None,
        };

        let traffic = network_record_to_traffic_record(&record);

        assert_eq!(traffic.original_response_headers, Some(original));
        assert_eq!(traffic.response_headers, Some(delivered));
        assert!(!traffic.has_rule_hit);
    }

    #[tokio::test]
    async fn network_export_writes_original_and_delivered_response_headers() {
        let dir = tempfile::tempdir().unwrap();
        let storage = RulesStorage::with_dir(dir.path().to_path_buf()).unwrap();
        let state = Arc::new(crate::state::AdminState::new_for_test(9900, storage));
        let original = vec![
            ("content-type".to_string(), "application/json".to_string()),
            ("connection".to_string(), "keep-alive".to_string()),
        ];
        let delivered = vec![("content-type".to_string(), "application/json".to_string())];
        let mut traffic = TrafficRecord::new(
            "REQ-header-export".to_string(),
            "GET".to_string(),
            "https://example.test/headers".to_string(),
        );
        traffic.original_response_headers = Some(original.clone());
        traffic.response_headers = Some(delivered.clone());

        let record = traffic_to_network_record(&traffic, false, &state).await;

        assert_eq!(record.original_response_headers, Some(original));
        assert_eq!(record.response_headers, Some(delivered));
    }

    #[tokio::test]
    async fn network_export_attaches_default_port_active_rules() {
        let dir = tempfile::tempdir().unwrap();
        let storage = RulesStorage::with_dir(dir.path().to_path_buf()).unwrap();
        storage
            .save(&RuleFile::new(
                "default-rule",
                "default.example.test status://209",
            ))
            .unwrap();
        storage
            .save(
                &RuleFile::new("disabled-rule", "disabled.example.test status://500")
                    .with_enabled(false),
            )
            .unwrap();
        let state = Arc::new(crate::state::AdminState::new_for_test(9900, storage));
        let mut traffic = TrafficRecord::new(
            "REQ-1".to_string(),
            "GET".to_string(),
            "http://default.example.test/".to_string(),
        );
        traffic.listener_port = 9900;

        let record = traffic_to_network_record(&traffic, false, &state).await;
        let active_rules = record.active_rules.expect("active rules snapshot");

        assert_eq!(record.listener_port, Some(9900));
        assert_eq!(active_rules.source, ActiveRuleSource::DefaultPort);
        assert_eq!(active_rules.listener_port, 9900);
        assert_eq!(active_rules.total, 1);
        assert_eq!(active_rules.rules[0].name, "default-rule");
        assert_eq!(
            active_rules.rules[0].content.as_deref(),
            Some("default.example.test status://209")
        );
        assert!(active_rules
            .merged_content
            .contains("default.example.test status://209"));
        assert!(!active_rules
            .merged_content
            .contains("disabled.example.test"));
    }

    #[tokio::test]
    async fn network_export_reports_empty_default_rules_when_rule_dir_missing() {
        let dir = tempfile::tempdir().unwrap();
        let storage = RulesStorage::with_dir(dir.path().join("missing-rules")).unwrap();
        let state = Arc::new(crate::state::AdminState::new_for_test(9900, storage));
        let mut traffic = TrafficRecord::new(
            "REQ-empty".to_string(),
            "GET".to_string(),
            "http://empty.example.test/".to_string(),
        );
        traffic.listener_port = 9900;

        let record = traffic_to_network_record(&traffic, false, &state).await;
        let active_rules = record.active_rules.expect("active rules snapshot");

        assert_eq!(active_rules.source, ActiveRuleSource::DefaultPort);
        assert_eq!(active_rules.total, 0);
        assert!(active_rules.rules.is_empty());
        assert!(active_rules.merged_content.is_empty());
        assert!(active_rules.unavailable_reason.is_none());
    }

    #[tokio::test]
    async fn network_export_uses_custom_port_active_rules_for_request_port() {
        let dir = tempfile::tempdir().unwrap();
        let storage = RulesStorage::with_dir(dir.path().to_path_buf()).unwrap();
        storage
            .save(&RuleFile::new(
                "default-rule",
                "default.example.test status://209",
            ))
            .unwrap();
        let state = Arc::new(crate::state::AdminState::new_for_test(9900, storage));
        state.set_temporary_port_manager(Arc::new(FakeTemporaryPortManager));
        let mut traffic = TrafficRecord::new(
            "REQ-2".to_string(),
            "GET".to_string(),
            "http://custom.example.test/".to_string(),
        );
        traffic.listener_port = 19090;

        let record = traffic_to_network_record(&traffic, false, &state).await;
        let active_rules = record.active_rules.expect("active rules snapshot");

        assert_eq!(record.listener_port, Some(19090));
        assert_eq!(active_rules.source, ActiveRuleSource::CustomPort);
        assert_eq!(active_rules.admin_port, 9900);
        assert_eq!(active_rules.listener_port, 19090);
        assert_eq!(active_rules.total, 1);
        assert_eq!(active_rules.rules[0].name, "custom-port-rule");
        assert_eq!(
            active_rules.rules[0].content.as_deref(),
            Some("custom.example.test status://210")
        );
        assert!(active_rules
            .merged_content
            .contains("custom.example.test status://210"));
        assert!(!active_rules.merged_content.contains("default.example.test"));
    }

    struct FakeTemporaryPortManager;

    #[async_trait]
    impl crate::temp_ports::TemporaryPortManager for FakeTemporaryPortManager {
        async fn bind(
            &self,
            _req: crate::temp_ports::TemporaryPortBindRequest,
        ) -> Result<crate::temp_ports::TemporaryPortBinding, crate::temp_ports::TemporaryPortError>
        {
            unreachable!("bind is not used by this test")
        }

        async fn update(
            &self,
            _port: u16,
            _req: crate::temp_ports::TemporaryPortUpdateRequest,
        ) -> Result<crate::temp_ports::TemporaryPortBinding, crate::temp_ports::TemporaryPortError>
        {
            unreachable!("update is not used by this test")
        }

        async fn destroy(
            &self,
            _port: u16,
        ) -> Result<crate::temp_ports::TemporaryPortBinding, crate::temp_ports::TemporaryPortError>
        {
            unreachable!("destroy is not used by this test")
        }

        async fn list(&self) -> Vec<crate::temp_ports::TemporaryPortBinding> {
            Vec::new()
        }

        async fn show(
            &self,
            _port: u16,
        ) -> Result<crate::temp_ports::TemporaryPortBinding, crate::temp_ports::TemporaryPortError>
        {
            unreachable!("show is not used by this test")
        }

        async fn active_summary(
            &self,
            port: u16,
        ) -> Result<
            crate::temp_ports::TemporaryPortActiveSummary,
            crate::temp_ports::TemporaryPortError,
        > {
            Ok(crate::temp_ports::TemporaryPortActiveSummary {
                port,
                total: 1,
                rules: vec![crate::temp_ports::TemporaryPortRuleItem {
                    name: "custom-port-rule".to_string(),
                    rule_count: 1,
                    group_id: None,
                    group_name: None,
                    content: Some("custom.example.test status://210".to_string()),
                }],
                merged_content: "custom.example.test status://210".to_string(),
            })
        }
    }

    #[test]
    fn count_rules_ignores_blank_and_comment_lines() {
        let content = "# comment\n\n rule-one  \n   # another comment\nrule-two\n   \n";
        assert_eq!(count_rules(content), 2);
    }

    #[test]
    fn toml_to_json_converts_common_value_types() {
        use toml::Value;

        let mut table = toml::map::Map::new();
        table.insert("s".to_string(), Value::String("v".to_string()));
        table.insert("i".to_string(), Value::Integer(1));
        table.insert("b".to_string(), Value::Boolean(true));
        table.insert(
            "arr".to_string(),
            Value::Array(vec![Value::Integer(2), Value::Integer(3)]),
        );
        let outer = Value::Table(table);

        let json = toml_to_json(outer);
        assert_eq!(json["s"], "v");
        assert_eq!(json["i"], 1);
        assert_eq!(json["b"], true);
        assert_eq!(json["arr"][0], 2);
        assert_eq!(json["arr"][1], 3);
    }

    #[test]
    fn convert_to_replay_request_maps_request_and_body_types() {
        use crate::replay_db::{BodyType, RawType, RequestSource, RequestType};

        let headers = vec![KeyValueItemExport {
            id: "h1".to_string(),
            key: "X-Test".to_string(),
            value: "v".to_string(),
            enabled: true,
            description: Some("header".to_string()),
        }];

        let body_export = ReplayBodyExport {
            body_type: "form-data".to_string(),
            raw_type: Some("json".to_string()),
            content: Some("body".to_string()),
            form_data: vec![KeyValueItemExport {
                id: "f1".to_string(),
                key: "k".to_string(),
                value: "v".to_string(),
                enabled: true,
                description: None,
            }],
            binary_file: Some("file.bin".to_string()),
        };

        let export = ReplayRequestExport {
            id: "rid".to_string(),
            group_id: Some("gid".to_string()),
            name: Some("name".to_string()),
            request_type: "sse".to_string(),
            method: "GET".to_string(),
            url: "http://example.test/".to_string(),
            headers: headers.clone(),
            body: Some(body_export),
            is_saved: true,
            sort_order: 42,
            created_at: 100,
            updated_at: 200,
        };

        let replay = convert_to_replay_request(&export, 7);

        assert_eq!(replay.id, "OUT-007");
        assert_eq!(replay.group_id.as_deref(), Some("gid"));
        assert_eq!(replay.name.as_deref(), Some("name"));
        assert_eq!(replay.request_type, RequestType::Sse);
        assert_eq!(replay.method, "GET");
        assert_eq!(replay.url, "http://example.test/");
        assert!(replay.is_saved);
        assert_eq!(replay.sort_order, 42);
        assert_eq!(replay.source, RequestSource::Imported);
        assert_eq!(replay.created_at, 100);
        assert_eq!(replay.updated_at, 200);
        assert_eq!(replay.headers.len(), 1);
        assert_eq!(replay.headers[0].key, "X-Test");
        assert_eq!(replay.headers[0].value, "v");
        assert_eq!(replay.headers[0].description.as_deref(), Some("header"));

        let body = replay.body.expect("body");
        assert_eq!(body.body_type, BodyType::FormData);
        assert!(matches!(body.raw_type, Some(RawType::Json)));
        assert_eq!(body.content.as_deref(), Some("body"));
        assert_eq!(body.form_data.len(), 1);
        assert_eq!(body.form_data[0].key, "k");
        assert_eq!(body.binary_file.as_deref(), Some("file.bin"));
    }

    #[test]
    fn convert_from_replay_request_maps_request_and_body_types() {
        use crate::replay_db::{
            BodyType, KeyValueItem, RawType, ReplayBody, ReplayRequest, RequestSource, RequestType,
        };

        let headers = vec![KeyValueItem {
            id: "h1".to_string(),
            key: "X-Test".to_string(),
            value: "v".to_string(),
            enabled: true,
            description: Some("header".to_string()),
        }];

        let body = ReplayBody {
            body_type: BodyType::Raw,
            raw_type: Some(RawType::Xml),
            content: Some("body".to_string()),
            form_data: vec![KeyValueItem {
                id: "f1".to_string(),
                key: "k".to_string(),
                value: "v".to_string(),
                enabled: true,
                description: None,
            }],
            binary_file: Some("file.bin".to_string()),
        };

        let request = ReplayRequest {
            id: "rid".to_string(),
            group_id: Some("gid".to_string()),
            name: Some("name".to_string()),
            request_type: RequestType::WebSocket,
            method: "POST".to_string(),
            url: "ws://example.test/".to_string(),
            headers,
            body: Some(body),
            is_saved: true,
            sort_order: 11,
            source: RequestSource::Imported,
            created_at: 123,
            updated_at: 456,
        };

        let export = convert_from_replay_request(&request);

        assert_eq!(export.id, "rid");
        assert_eq!(export.group_id.as_deref(), Some("gid"));
        assert_eq!(export.name.as_deref(), Some("name"));
        assert_eq!(export.request_type, "websocket");
        assert_eq!(export.method, "POST");
        assert_eq!(export.url, "ws://example.test/");
        assert!(export.is_saved);
        assert_eq!(export.sort_order, 11);
        assert_eq!(export.created_at, 123);
        assert_eq!(export.updated_at, 456);
        assert_eq!(export.headers.len(), 1);
        assert_eq!(export.headers[0].key, "X-Test");
        assert_eq!(export.headers[0].value, "v");
        assert_eq!(export.headers[0].description.as_deref(), Some("header"));

        let body_export = export.body.expect("body");
        assert_eq!(body_export.body_type, "raw");
        assert_eq!(body_export.raw_type.as_deref(), Some("xml"));
        assert_eq!(body_export.content.as_deref(), Some("body"));
        assert_eq!(body_export.form_data.len(), 1);
        assert_eq!(body_export.form_data[0].key, "k");
        assert_eq!(body_export.form_data[0].value, "v");
        assert_eq!(body_export.binary_file.as_deref(), Some("file.bin"));
    }

    #[test]
    fn unavailable_active_rules_export_is_empty_with_reason() {
        let export = unavailable_active_rules_export(
            ActiveRuleSource::CustomPort,
            9900,
            19090,
            "missing manager",
        );

        assert_eq!(export.source, ActiveRuleSource::CustomPort);
        assert_eq!(export.admin_port, 9900);
        assert_eq!(export.listener_port, 19090);
        assert_eq!(export.total, 0);
        assert!(export.rules.is_empty());
        assert!(export.merged_content.is_empty());
        assert_eq!(
            export.unavailable_reason.as_deref(),
            Some("missing manager")
        );
    }

    #[test]
    fn network_record_import_falls_back_to_url_when_host_and_path_missing() {
        let record = NetworkRecord {
            id: "REQ-fallback".to_string(),
            method: "POST".to_string(),
            url: "https://example.test/api?x=1".to_string(),
            status: 201,
            host: None,
            path: None,
            protocol: None,
            actual_url: None,
            actual_host: None,
            listener_port: None,
            has_rule_hit: None,
            error_message: None,
            client_app: None,
            client_path: None,
            request_headers: None,
            response_headers: None,
            original_response_headers: None,
            request_body: None,
            response_body: None,
            duration_ms: 0,
            timestamp: 0,
            matched_rules: None,
            active_rules: None,
        };

        let traffic = network_record_to_traffic_record(&record);

        assert_eq!(traffic.host, "example.test");
        assert_eq!(traffic.path, "/api?x=1");
        assert_eq!(traffic.protocol, "HTTPS");
    }

    #[test]
    fn network_record_import_uses_default_protocol_for_invalid_url() {
        let record = NetworkRecord {
            id: "REQ-invalid-url".to_string(),
            method: "GET".to_string(),
            url: "not a valid url".to_string(),
            status: 0,
            host: None,
            path: None,
            protocol: None,
            actual_url: None,
            actual_host: None,
            listener_port: None,
            has_rule_hit: None,
            error_message: None,
            client_app: None,
            client_path: None,
            request_headers: None,
            response_headers: None,
            original_response_headers: None,
            request_body: None,
            response_body: None,
            duration_ms: 0,
            timestamp: 0,
            matched_rules: None,
            active_rules: None,
        };

        let traffic = network_record_to_traffic_record(&record);

        assert_eq!(traffic.host, "");
        assert_eq!(traffic.path, "");
        assert_eq!(traffic.protocol, "HTTP");
    }
}
