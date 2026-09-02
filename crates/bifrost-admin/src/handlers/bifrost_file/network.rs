use super::*;

pub(super) fn build_network_preview(records: &[NetworkRecord]) -> NetworkPreview {
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
                request_size: body_size(
                    record.request_body.as_deref(),
                    record.request_body_base64.as_deref(),
                ),
                response_size: body_size(
                    record.response_body.as_deref(),
                    record.response_body_base64.as_deref(),
                ),
            }
        })
        .collect();

    for record in records.iter().skip(50) {
        let traffic_record = network_record_to_traffic_record(record);
        hosts.insert(traffic_record.host);
    }

    let mut warnings = Vec::new();
    if records.len() > 1 {
        for record in records {
            let request_warning = record.request_body.as_deref().and_then(|text| {
                legacy_lossy_body_warning(
                    text,
                    record.request_body_base64.as_deref(),
                    &record.request_headers,
                    "request",
                )
            });
            let response_warning = record.response_body.as_deref().and_then(|text| {
                legacy_lossy_body_warning(
                    text,
                    record.response_body_base64.as_deref(),
                    effective_response_headers(record),
                    "response",
                )
            });
            warnings.extend(
                request_warning
                    .into_iter()
                    .chain(response_warning)
                    .map(|warning| format!("Record {}: {warning}", record.id)),
            );
        }
    }
    let single_record = records
        .first()
        .filter(|_| records.len() == 1)
        .map(|record| {
            let request_body = preview_body(
                record.request_body.as_deref(),
                record.request_body_base64.as_deref(),
                &record.request_headers,
                "request",
            );
            let response_body = preview_body(
                record.response_body.as_deref(),
                record.response_body_base64.as_deref(),
                effective_response_headers(record),
                "response",
            );
            warnings.extend(request_body.warning);
            warnings.extend(response_body.warning);
            NetworkPreviewDetail {
                record: network_record_to_traffic_record(record),
                request_body: request_body.text,
                response_body: response_body.text,
            }
        });

    NetworkPreview {
        record_count: records.len(),
        hosts: hosts.into_iter().take(20).collect(),
        records: preview_records,
        single_record,
        warnings,
    }
}

pub(super) fn effective_response_headers(record: &NetworkRecord) -> &Option<Vec<(String, String)>> {
    if record.response_headers.is_some() {
        &record.response_headers
    } else {
        &record.original_response_headers
    }
}

pub(super) async fn handle_import(
    req: Request<Incoming>,
    state: SharedAdminState,
) -> Response<BoxBody> {
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

pub(super) async fn import_network(content: &str, state: &SharedAdminState) -> Response<BoxBody> {
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
    if let Err(message) = validate_network_body_base64(&file.content) {
        return error_response(StatusCode::BAD_REQUEST, &message);
    }

    for network_record in &file.content {
        let mut traffic_record = network_record_to_traffic_record(network_record);
        if let Some(body_store) = state.body_store.as_ref() {
            if let Err(message) =
                persist_imported_bodies(network_record, &mut traffic_record, body_store)
            {
                return error_response(StatusCode::BAD_REQUEST, &message);
            }
        } else if network_record.request_body.is_some()
            || network_record.request_body_base64.is_some()
            || network_record.response_body.is_some()
            || network_record.response_body_base64.is_some()
        {
            warnings.push(format!(
                "Record {} bodies could not be persisted because the body store is unavailable",
                network_record.id
            ));
        }
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

struct ImportedBodyBytes {
    primary: Option<Vec<u8>>,
    raw: Option<Vec<u8>>,
    primary_content_encoding: Option<String>,
}

fn imported_body_bytes(
    text: Option<&str>,
    body_base64: Option<&str>,
    headers: &Option<Vec<(String, String)>>,
) -> Result<ImportedBodyBytes, String> {
    let raw = body_base64
        .map(|encoded| {
            STANDARD
                .decode(encoded)
                .map_err(|error| format!("invalid base64 body data: {error}"))
        })
        .transpose()?;
    let content_encoding = content_encoding_value(headers);
    let (primary, primary_content_encoding) = if let Some(text) = text {
        (Some(text.as_bytes().to_vec()), None)
    } else if let Some(bytes) = raw.as_ref() {
        match content_encoding.as_deref() {
            Some(encoding) if content_encoding_is_supported(encoding) => {
                match decompress_with_limit(bytes, encoding, DEFAULT_MAX_DECOMPRESSED_BODY_BYTES) {
                    Ok(decoded) => (Some(decoded), None),
                    Err(_) => (Some(bytes.clone()), Some(encoding.to_string())),
                }
            }
            _ => (Some(bytes.clone()), None),
        }
    } else {
        (None, None)
    };
    Ok(ImportedBodyBytes {
        primary,
        raw,
        primary_content_encoding,
    })
}

fn store_imported_body(
    store: &crate::BodyStore,
    record_id: &str,
    kind: &str,
    bytes: Option<&[u8]>,
    content_encoding: Option<&str>,
) -> Result<Option<crate::BodyRef>, String> {
    let Some(bytes) = bytes.filter(|bytes| !bytes.is_empty()) else {
        return Ok(None);
    };
    let body_ref = store
        .store(record_id, kind, bytes)
        .ok_or_else(|| format!("{kind} body persistence is unavailable"))?;
    if !body_ref.is_file() {
        return Err(format!("{kind} body could not be persisted losslessly"));
    }
    Ok(Some(body_ref.with_content_encoding(content_encoding)))
}

pub(super) fn persist_imported_bodies(
    network_record: &NetworkRecord,
    traffic_record: &mut TrafficRecord,
    body_store: &crate::SharedBodyStore,
) -> Result<(), String> {
    let request = imported_body_bytes(
        network_record.request_body.as_deref(),
        network_record.request_body_base64.as_deref(),
        &network_record.request_headers,
    )
    .map_err(|error| format!("Record {} request body: {error}", network_record.id))?;
    let response = imported_body_bytes(
        network_record.response_body.as_deref(),
        network_record.response_body_base64.as_deref(),
        effective_response_headers(network_record),
    )
    .map_err(|error| format!("Record {} response body: {error}", network_record.id))?;
    let store = body_store.read();
    let persistence_result = (|| {
        traffic_record.request_body_ref = store_imported_body(
            &store,
            &traffic_record.id,
            "req",
            request.primary.as_deref(),
            request.primary_content_encoding.as_deref(),
        )?;
        traffic_record.response_body_ref = store_imported_body(
            &store,
            &traffic_record.id,
            "res",
            response.primary.as_deref(),
            response.primary_content_encoding.as_deref(),
        )?;
        traffic_record.raw_request_body_ref = store_imported_body(
            &store,
            &traffic_record.id,
            "req_raw",
            request.raw.as_deref(),
            None,
        )?;
        traffic_record.raw_response_body_ref = store_imported_body(
            &store,
            &traffic_record.id,
            "res_raw",
            response.raw.as_deref(),
            None,
        )?;
        Ok(())
    })();
    if persistence_result.is_err() {
        for body_ref in [
            traffic_record.request_body_ref.take(),
            traffic_record.response_body_ref.take(),
            traffic_record.raw_request_body_ref.take(),
            traffic_record.raw_response_body_ref.take(),
        ]
        .into_iter()
        .flatten()
        {
            store.remove(&body_ref);
        }
    }
    persistence_result.map_err(|error: String| format!("Record {}: {error}", network_record.id))
}

pub(super) fn validate_network_body_base64(records: &[NetworkRecord]) -> Result<(), String> {
    for record in records {
        for (label, encoded) in [
            ("request_body_base64", record.request_body_base64.as_deref()),
            (
                "response_body_base64",
                record.response_body_base64.as_deref(),
            ),
        ] {
            if let Some(encoded) = encoded {
                STANDARD.decode(encoded).map_err(|error| {
                    format!("Record {} has invalid {label}: {error}", record.id)
                })?;
            }
        }
    }
    Ok(())
}

pub(super) fn network_record_to_traffic_record(record: &NetworkRecord) -> TrafficRecord {
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

    let request_size = body_size(
        record.request_body.as_deref(),
        record.request_body_base64.as_deref(),
    );
    let response_size = body_size(
        record.response_body.as_deref(),
        record.response_body_base64.as_deref(),
    );
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
        content_type: header_value(effective_response_headers(record), "content-type"),
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
        request_content_type: header_value(&record.request_headers, "content-type"),
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
