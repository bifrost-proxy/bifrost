use std::collections::HashSet;

use bifrost_command::{
    CanonicalQueryCommand, FilterCondition as CommandFilterCondition,
    SearchArgs as CommandSearchArgs, TrafficClearArgs, TrafficGetArgs, TrafficListArgs,
    TrafficListDirection,
};
use bifrost_core::{BifrostError, Result};
use serde_json::{json, Value};

use crate::body_store::BodyRef;
use crate::handlers::network_body::{
    decode_content_encoded_body, DEFAULT_MAX_DECOMPRESSED_BODY_BYTES,
};
use crate::push::SharedPushManager;
use crate::search::{
    FilterCondition, SearchEngine, SearchFilters, SearchInclude, SearchProgress, SearchRequest,
    SearchResponse, SearchScope,
};
use crate::state::SharedAdminState;
use crate::traffic_db::{Direction, QueryParams, QueryResult, TextMatchMode};
use crate::TrafficRecord;

async fn join_clear_task<T>(
    task: tokio::task::JoinHandle<std::result::Result<T, String>>,
    label: &str,
) -> Result<T> {
    task.await
        .map_err(|e| BifrostError::Config(format!("{label} clear task join failed: {e}")))?
        .map_err(BifrostError::Config)
}

async fn configured_decompress_output_bytes(state: &SharedAdminState) -> usize {
    match state.config_manager.as_ref() {
        Some(config_manager) => {
            config_manager
                .config()
                .await
                .sandbox
                .limits
                .max_decompress_output_bytes
        }
        None => DEFAULT_MAX_DECOMPRESSED_BODY_BYTES,
    }
}

#[derive(Clone)]
pub struct AdminQueryService {
    state: SharedAdminState,
    push_manager: Option<SharedPushManager>,
}

impl AdminQueryService {
    pub fn new(state: SharedAdminState) -> Self {
        Self {
            state,
            push_manager: None,
        }
    }

    pub fn with_push_manager(
        state: SharedAdminState,
        push_manager: Option<SharedPushManager>,
    ) -> Self {
        Self {
            state,
            push_manager,
        }
    }

    pub async fn execute(&self, command: &CanonicalQueryCommand) -> Result<Value> {
        match command {
            CanonicalQueryCommand::Search(args) => {
                let response = self.search(args).await?;
                serde_json::to_value(response)
                    .map_err(|e| BifrostError::Config(format!("serialize search response: {e}")))
            }
            CanonicalQueryCommand::TrafficList(args) => {
                let response = self.list_traffic(args).await?;
                serde_json::to_value(response).map_err(|e| {
                    BifrostError::Config(format!("serialize traffic list response: {e}"))
                })
            }
            CanonicalQueryCommand::TrafficGet(args) => self.get_traffic_json(args).await,
            CanonicalQueryCommand::TrafficClear(args) => {
                let response = self.clear_traffic(args).await?;
                Ok(json!({ "success": true, "message": response }))
            }
        }
    }

    pub async fn search(&self, args: &CommandSearchArgs) -> Result<SearchResponse> {
        let request = search_request_from_command(args);
        let traffic_db =
            self.state.traffic_db_store.clone().ok_or_else(|| {
                BifrostError::Config("Traffic database not available".to_string())
            })?;
        let body_store = self.state.body_store.clone();
        let frame_store = self.state.frame_store.clone();
        let connection_monitor = Some(self.state.connection_monitor.clone());
        let max_decompress_output_bytes = configured_decompress_output_bytes(&self.state).await;
        let state = self.state.clone();

        tokio::task::spawn_blocking(move || {
            let engine = SearchEngine::with_frame_support(
                traffic_db,
                body_store,
                frame_store,
                connection_monitor,
            )
            .with_decompression_limit(max_decompress_output_bytes);
            let mut response = engine.search(&request);
            for result in &mut response.results {
                let mut record = result.record.clone();
                state.reconcile_socket_summary(&mut record);
                result.record = record;
            }
            response
        })
        .await
        .map_err(|e| BifrostError::Config(format!("search task join failed: {e}")))
    }

    pub async fn search_stream<F, P>(
        &self,
        args: &CommandSearchArgs,
        on_result: F,
        on_progress: P,
    ) -> Result<SearchResponse>
    where
        F: FnMut(&crate::search::SearchResultItem) + Send + 'static,
        P: FnMut(&SearchProgress) + Send + 'static,
    {
        let request = search_request_from_command(args);
        let traffic_db =
            self.state.traffic_db_store.clone().ok_or_else(|| {
                BifrostError::Config("Traffic database not available".to_string())
            })?;
        let body_store = self.state.body_store.clone();
        let frame_store = self.state.frame_store.clone();
        let connection_monitor = Some(self.state.connection_monitor.clone());
        let max_decompress_output_bytes = configured_decompress_output_bytes(&self.state).await;
        let state = self.state.clone();

        tokio::task::spawn_blocking(move || {
            let engine = SearchEngine::with_frame_support(
                traffic_db,
                body_store,
                frame_store,
                connection_monitor,
            )
            .with_decompression_limit(max_decompress_output_bytes);
            let mut response = engine.search_stream(&request, on_result, on_progress);
            for result in &mut response.results {
                let mut record = result.record.clone();
                state.reconcile_socket_summary(&mut record);
                result.record = record;
            }
            response
        })
        .await
        .map_err(|e| BifrostError::Config(format!("search stream task join failed: {e}")))
    }

    pub async fn list_traffic(&self, args: &TrafficListArgs) -> Result<QueryResult> {
        let params = traffic_list_params_from_command(args);
        self.query_traffic_params(params).await
    }

    pub async fn query_traffic_params(&self, params: QueryParams) -> Result<QueryResult> {
        let needs_exact_total = params.has_filters();
        let db_store =
            self.state.traffic_db_store.clone().ok_or_else(|| {
                BifrostError::Config("Traffic database not available".to_string())
            })?;
        let state = self.state.clone();

        let mut result = tokio::task::spawn_blocking(move || {
            if needs_exact_total {
                db_store.query_with_exact_total(&params)
            } else {
                db_store.query(&params)
            }
        })
        .await
        .map_err(|e| BifrostError::Config(format!("traffic list task join failed: {e}")))?;

        for record in &mut result.records {
            state.reconcile_socket_summary(record);
        }
        Ok(result)
    }

    pub async fn get_traffic_record(&self, id: &str) -> Result<TrafficRecord> {
        let lookup_id = self.resolve_traffic_lookup_id(id).await?;
        let db_store =
            self.state.traffic_db_store.clone().ok_or_else(|| {
                BifrostError::Config("Traffic database not available".to_string())
            })?;
        let id = lookup_id.clone();
        let state = self.state.clone();
        let record = tokio::task::spawn_blocking(move || db_store.get_by_id(&lookup_id))
            .await
            .map_err(|e| BifrostError::Config(format!("traffic detail task join failed: {e}")))?;

        let mut record = record
            .ok_or_else(|| BifrostError::NotFound(format!("Traffic record '{}' not found", id)))?;
        state.reconcile_traffic_record(&mut record);
        Ok(record)
    }

    async fn resolve_traffic_lookup_id(&self, id: &str) -> Result<String> {
        if !id.chars().all(|c| c.is_ascii_digit()) {
            return Ok(id.to_string());
        }

        if let Some(exact) = self
            .find_id_by_exact_sequence(id.parse().map_err(|_| {
                BifrostError::Parse(format!("Invalid traffic sequence suffix: {}", id))
            })?)
            .await?
        {
            return Ok(exact);
        }

        let suffix_val: u64 = id
            .parse()
            .map_err(|_| BifrostError::Parse(format!("Invalid traffic sequence suffix: {}", id)))?;
        let server_seq = self.fetch_server_sequence().await?;
        let Some(modulus) = 10u64.checked_pow(id.len() as u32) else {
            return Err(BifrostError::Parse(format!(
                "Traffic ID suffix too long: {}",
                id
            )));
        };
        let suffix = suffix_val % modulus;

        let mut candidates = Vec::new();
        let mut candidate = suffix;
        while candidate <= server_seq {
            candidates.push(candidate);
            candidate += modulus;
        }
        candidates.reverse();

        for candidate_seq in candidates {
            if let Some(found) = self.find_id_by_exact_sequence(candidate_seq).await? {
                return Ok(found);
            }
        }

        Err(BifrostError::NotFound(format!(
            "Traffic record '{}' not found",
            id
        )))
    }

    async fn fetch_server_sequence(&self) -> Result<u64> {
        let result = self
            .query_traffic_params(QueryParams {
                limit: Some(1),
                ..QueryParams::default()
            })
            .await?;
        Ok(result.server_sequence)
    }

    async fn find_id_by_exact_sequence(&self, seq: u64) -> Result<Option<String>> {
        let result = self
            .query_traffic_params(QueryParams {
                cursor: Some(seq.saturating_add(1)),
                limit: Some(1),
                direction: Direction::Backward,
                ..QueryParams::default()
            })
            .await?;
        Ok(result
            .records
            .into_iter()
            .find(|record| record.seq == seq)
            .map(|record| record.id))
    }

    pub async fn get_traffic_json(&self, args: &TrafficGetArgs) -> Result<Value> {
        let record = self.get_traffic_record(&args.id).await?;
        let mut detail = serde_json::to_value(&record)
            .map_err(|e| BifrostError::Config(format!("serialize traffic detail: {e}")))?;

        if args.request_body {
            detail["request_body"] = self
                .load_body_json(
                    record
                        .request_body_ref
                        .as_ref()
                        .or(record.raw_request_body_ref.as_ref()),
                )
                .await?;
        }

        if args.response_body {
            detail["response_body"] = self
                .load_body_json(
                    record
                        .response_body_ref
                        .as_ref()
                        .or(record.raw_response_body_ref.as_ref()),
                )
                .await?;
        }

        Ok(detail)
    }

    pub async fn clear_traffic(&self, args: &TrafficClearArgs) -> Result<String> {
        match &args.ids {
            Some(ids) if !ids.is_empty() => self.clear_traffic_by_ids(ids.clone()).await,
            _ => self.clear_all_traffic().await,
        }
    }

    async fn load_body_json(&self, body_ref: Option<&BodyRef>) -> Result<Value> {
        let Some(body_ref) = body_ref.cloned() else {
            return Ok(json!(null));
        };

        match body_ref {
            BodyRef::Inline { data } => Ok(json!({ "success": true, "data": data })),
            BodyRef::File { .. } | BodyRef::FileRange { .. } => {
                let body_store =
                    self.state.body_store.clone().ok_or_else(|| {
                        BifrostError::Config("Body store not configured".to_string())
                    })?;
                let loaded = tokio::task::spawn_blocking(move || {
                    let store = body_store.read();
                    store.load_bytes(&body_ref).map(|bytes| {
                        let decoded = decode_content_encoded_body(
                            bytes,
                            body_ref.content_encoding().as_deref(),
                        );
                        String::from_utf8_lossy(&decoded).to_string()
                    })
                })
                .await
                .map_err(|e| BifrostError::Config(format!("body load task join failed: {e}")))?;

                Ok(json!({
                    "success": loaded.is_some(),
                    "data": loaded
                }))
            }
        }
    }

    async fn clear_traffic_by_ids(&self, ids: Vec<String>) -> Result<String> {
        let active_connection_ids = self.state.connection_monitor.active_connection_ids();
        let active_set: HashSet<&String> = active_connection_ids.iter().collect();
        let ids_to_delete: Vec<String> = ids
            .into_iter()
            .filter(|id| !active_set.contains(id))
            .collect();

        if ids_to_delete.is_empty() {
            return Ok("No traffic records to clear (all are active connections)".to_string());
        }

        if let Some(ref db_store) = self.state.traffic_db_store {
            let db_store_clone = db_store.clone();
            let ids_for_db = ids_to_delete.clone();
            let delete_task = tokio::task::spawn_blocking(move || {
                db_store_clone.delete_by_ids(&ids_for_db);
                Ok(())
            });
            join_clear_task(delete_task, "traffic db delete-by-ids").await?;
        }

        if let Some(ref body_store) = self.state.body_store {
            let body_store_clone = body_store.clone();
            let ids_for_body = ids_to_delete.clone();
            let delete_task = tokio::task::spawn_blocking(move || {
                body_store_clone
                    .write()
                    .delete_by_ids(&ids_for_body)
                    .map(|_| ())
                    .map_err(|e| e.to_string())
            });
            let _ = join_clear_task(delete_task, "body store delete-by-ids").await;
        }

        if let Some(ref frame_store) = self.state.frame_store {
            let frame_store_clone = frame_store.clone();
            let ids_for_frame = ids_to_delete.clone();
            let delete_task = tokio::task::spawn_blocking(move || {
                frame_store_clone
                    .delete_by_ids(&ids_for_frame)
                    .map(|_| ())
                    .map_err(|e| e.to_string())
            });
            let _ = join_clear_task(delete_task, "frame store delete-by-ids").await;
        }

        if let Some(ref ws_payload_store) = self.state.ws_payload_store {
            let ws_payload_store_clone = ws_payload_store.clone();
            let ids_for_payload = ids_to_delete.clone();
            let delete_task = tokio::task::spawn_blocking(move || {
                ws_payload_store_clone
                    .delete_by_ids(&ids_for_payload)
                    .map(|_| ())
                    .map_err(|e| e.to_string())
            });
            let _ = join_clear_task(delete_task, "ws payload store delete-by-ids").await;
        }

        if let Some(pm) = &self.push_manager {
            pm.invalidate_overview_cache();
            pm.broadcast_traffic_deleted(ids_to_delete.clone());
        }

        Ok(format!(
            "{} traffic records cleared successfully",
            ids_to_delete.len()
        ))
    }

    async fn clear_all_traffic(&self) -> Result<String> {
        self.state.connection_monitor.clear();

        if let Some(ref db_store) = self.state.traffic_db_store {
            let db_store_clone = db_store.clone();
            let clear_task = tokio::task::spawn_blocking(move || {
                db_store_clone.clear();
                Ok(())
            });
            join_clear_task(clear_task, "traffic db clear-all").await?;
        }

        if let Some(ref body_store) = self.state.body_store {
            let body_store_clone = body_store.clone();
            let clear_task = tokio::task::spawn_blocking(move || {
                body_store_clone
                    .write()
                    .clear()
                    .map(|_| ())
                    .map_err(|e| e.to_string())
            });
            let _ = join_clear_task(clear_task, "body store clear-all").await;
        }

        if let Some(ref frame_store) = self.state.frame_store {
            let frame_store_clone = frame_store.clone();
            let clear_task = tokio::task::spawn_blocking(move || {
                frame_store_clone
                    .clear()
                    .map(|_| ())
                    .map_err(|e| e.to_string())
            });
            let _ = join_clear_task(clear_task, "frame store clear-all").await;
        }

        if let Some(ref ws_payload_store) = self.state.ws_payload_store {
            let ws_payload_store_clone = ws_payload_store.clone();
            let clear_task = tokio::task::spawn_blocking(move || {
                ws_payload_store_clone
                    .clear()
                    .map(|_| ())
                    .map_err(|e| e.to_string())
            });
            let _ = join_clear_task(clear_task, "ws payload store clear-all").await;
        }

        if let Some(pm) = &self.push_manager {
            pm.invalidate_overview_cache();
            pm.notify_traffic_statistics_changed();
        }

        Ok("All traffic data cleared successfully".to_string())
    }
}

pub fn search_request_from_command(args: &CommandSearchArgs) -> SearchRequest {
    SearchRequest {
        keyword: args.keyword.clone(),
        scope: SearchScope {
            request_body: args.scope.request_body,
            response_body: args.scope.response_body,
            request_headers: args.scope.request_headers,
            response_headers: args.scope.response_headers,
            url: args.scope.url,
            websocket_messages: args.scope.websocket_messages,
            sse_events: args.scope.sse_events,
            all: args.scope.all,
        },
        filters: SearchFilters {
            protocols: args.filters.protocols.clone(),
            status_ranges: args.filters.status_ranges.clone(),
            content_types: args.filters.content_types.clone(),
            has_rule_hit: args.filters.has_rule_hit,
            conditions: args
                .filters
                .conditions
                .iter()
                .map(filter_condition_from_command)
                .collect(),
            client_ips: args.filters.client_ips.clone(),
            client_apps: args.filters.client_apps.clone(),
            account_names: args.filters.account_names.clone(),
            domains: args.filters.domains.clone(),
        },
        cursor: args.cursor,
        limit: args.limit,
        max_scan: args.max_scan,
        max_results: args.max_results,
        record_ids: args.record_ids.clone(),
        time_range: args.time_range.as_ref().map(|tr| crate::search::TimeRange {
            since_ms: tr.since_ms,
            until_ms: tr.until_ms,
        }),
        include: SearchInclude {
            request_body: args.include.request_body,
            response_body: args.include.response_body,
            request_headers: args.include.request_headers,
            response_headers: args.include.response_headers,
            max_body_bytes: args.include.max_body_bytes,
        },
    }
}

pub fn traffic_list_params_from_command(args: &TrafficListArgs) -> QueryParams {
    QueryParams {
        cursor: args.cursor,
        limit: Some(args.limit.unwrap_or(50)),
        direction: match args.direction {
            TrafficListDirection::Backward => Direction::Backward,
            TrafficListDirection::Forward => Direction::Forward,
        },
        method: args.method.clone(),
        status: args.status,
        status_min: args.status_min,
        status_max: args.status_max,
        protocol: args.protocol.clone(),
        host_contains: args.host.clone(),
        url_contains: args.url.clone(),
        path_contains: args.path.clone(),
        content_type: args.content_type.clone(),
        client_ip: args.client_ip.clone(),
        client_ip_match: TextMatchMode::Contains,
        client_app: args.client_app.clone(),
        client_app_match: TextMatchMode::Contains,
        listener_port: args.listener_port,
        has_rule_hit: args.has_rule_hit,
        is_websocket: args.is_websocket,
        is_sse: args.is_sse,
        is_tunnel: args.is_tunnel,
        ..Default::default()
    }
}

fn filter_condition_from_command(condition: &CommandFilterCondition) -> FilterCondition {
    FilterCondition {
        field: condition.field.clone(),
        operator: condition.operator.clone(),
        value: condition.value.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bifrost_command::{
        SearchFilters as CommandSearchFilters, SearchScope as CommandSearchScope,
    };
    use std::io::Write;
    use tokio::time::{timeout, Duration};

    use crate::push::{start_push_tasks, ClientSubscription, PushMessage};
    use crate::test_support::TestAdminState;

    #[test]
    fn test_search_request_from_command_preserves_filters() {
        let args = CommandSearchArgs {
            keyword: "hello".to_string(),
            scope: CommandSearchScope {
                all: false,
                request_headers: true,
                ..CommandSearchScope::default()
            },
            filters: CommandSearchFilters {
                status_ranges: vec!["2xx".to_string()],
                conditions: vec![CommandFilterCondition {
                    field: "method".to_string(),
                    operator: "equals".to_string(),
                    value: "GET".to_string(),
                }],
                ..CommandSearchFilters::default()
            },
            limit: Some(20),
            max_scan: Some(1000),
            max_results: Some(50),
            cursor: Some(42),
            time_range: None,
            ..Default::default()
        };

        let request = search_request_from_command(&args);
        assert_eq!(request.keyword, "hello");
        assert!(!request.scope.all);
        assert!(request.scope.request_headers);
        assert_eq!(request.filters.status_ranges, vec!["2xx"]);
        assert_eq!(request.filters.conditions.len(), 1);
        assert_eq!(request.limit, Some(20));
        assert_eq!(request.cursor, Some(42));
    }

    #[test]
    fn test_traffic_list_params_from_command_maps_direction() {
        let params = traffic_list_params_from_command(&TrafficListArgs {
            limit: Some(10),
            direction: TrafficListDirection::Forward,
            host: Some("example.com".to_string()),
            listener_port: Some(50831),
            ..TrafficListArgs::default()
        });

        assert_eq!(params.limit, Some(10));
        assert_eq!(params.direction, Direction::Forward);
        assert_eq!(params.host_contains.as_deref(), Some("example.com"));
        assert_eq!(params.listener_port, Some(50831));
    }

    #[tokio::test]
    async fn traffic_get_decodes_content_encoded_file_body() {
        let harness = TestAdminState::builder().build();
        let plaintext = br#"{"marker":"traffic-get-plaintext"}"#;
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(plaintext).expect("compress body");
        let compressed = encoder.finish().expect("finish gzip body");
        let body_ref = harness
            .body_store
            .read()
            .store("query-service-encoded", "res", &compressed)
            .expect("store compressed body")
            .with_content_encoding(Some("gzip"));
        let mut record = TrafficRecord::new(
            "query-service-encoded".to_string(),
            "GET".to_string(),
            "http://example.test/query-service-encoded".to_string(),
        );
        record.status = 200;
        record.response_body_ref = Some(body_ref);
        harness.traffic_db.record(record);
        let service = AdminQueryService::new(harness.state());

        let detail = service
            .get_traffic_json(&TrafficGetArgs {
                id: "query-service-encoded".to_string(),
                response_body: true,
                ..TrafficGetArgs::default()
            })
            .await
            .expect("get traffic detail");

        assert_eq!(
            detail["response_body"]["data"],
            String::from_utf8_lossy(plaintext).as_ref()
        );
    }

    #[tokio::test]
    async fn traffic_get_reports_missing_file_body_without_panicking() {
        let harness = TestAdminState::builder().build();
        let service = AdminQueryService::new(harness.state());
        let body_ref = BodyRef::File {
            path: harness
                .data_dir()
                .join("missing-traffic-body")
                .to_string_lossy()
                .to_string(),
            size: 128,
        };

        let body = service
            .load_body_json(Some(&body_ref))
            .await
            .expect("missing body is represented in the response");

        assert_eq!(body["success"], false);
        assert!(body["data"].is_null());
    }

    #[tokio::test]
    async fn clear_all_notifies_authoritative_traffic_statistics() {
        let harness = TestAdminState::builder().build();
        let mut record = TrafficRecord::new(
            "query-service-clear".to_string(),
            "GET".to_string(),
            "http://example.test/query-service-clear".to_string(),
        );
        record.status = 200;
        harness.traffic_db.record(record);
        let manager = harness.push_manager();
        let (_client, mut receiver) = manager.register_client(
            "query-service-clear".to_string(),
            ClientSubscription {
                need_traffic: true,
                ..Default::default()
            },
        );
        let handles = start_push_tasks(manager.clone());
        let service = AdminQueryService::with_push_manager(harness.state(), Some(manager));

        let result = service.clear_all_traffic().await.expect("clear traffic");

        assert_eq!(result, "All traffic data cleared successfully");
        assert_eq!(harness.traffic_db.count(), 0);
        let statistics = timeout(Duration::from_secs(2), async {
            loop {
                if let Some(PushMessage::TrafficStatistics(statistics)) = receiver.recv().await {
                    break statistics;
                }
            }
        })
        .await
        .expect("expected clear-all statistics push");
        assert_eq!(statistics.total_requests, 0);

        for handle in handles {
            handle.abort();
        }
    }
}
