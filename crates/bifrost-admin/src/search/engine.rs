use std::collections::HashMap;
use std::time::{Duration, Instant};

use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use regex::Regex;
use serde_json::Value as JsonValue;
use tracing::{debug, warn};

use super::json_path;
use super::types::{
    BodiesPayload, BodyChunk, FilterCondition, HeadersPayload, MatchLocation, SearchFilters,
    SearchInclude, SearchRequest, SearchResponse, SearchResultItem, SearchScope, SearchedRange,
};
use crate::body_store::{BodyRef, SharedBodyStore};
use crate::connection_monitor::SharedConnectionMonitor;
use crate::frame_store::SharedFrameStore;
use crate::handlers::network_body::{
    content_encoding_is_supported, decompress_with_limit_metered,
    DEFAULT_MAX_DECOMPRESSED_BODY_BYTES,
};
use crate::traffic_db::{
    QueryParams, SharedTrafficDbStore, TextMatchMode, TrafficSearchFields, TrafficSummaryCompact,
};

#[derive(Debug, Clone)]
enum BodyCacheEntry {
    Json(JsonValue),
    NonJson,
    Missing,
}

struct BodyReadCache {
    bytes: HashMap<String, Option<Vec<u8>>>,
    json: HashMap<String, BodyCacheEntry>,
    max_decompressed_bytes: usize,
    remaining_decompressed_bytes: usize,
    decompression_budget_exhausted: bool,
    current_record_started_with_fresh_budget: bool,
    current_record_exceeds_decompression_budget: bool,
}

impl BodyReadCache {
    fn new(max_decompressed_bytes: usize) -> Self {
        Self {
            bytes: HashMap::new(),
            json: HashMap::new(),
            max_decompressed_bytes,
            remaining_decompressed_bytes: max_decompressed_bytes,
            decompression_budget_exhausted: false,
            current_record_started_with_fresh_budget: true,
            current_record_exceeds_decompression_budget: false,
        }
    }

    fn remove_record(&mut self, record_id: &str) {
        for side in ["req", "res"] {
            let key = format!("{side}:{record_id}");
            self.bytes.remove(&key);
            self.json.remove(&key);
        }
        let frame_prefix = format!("frame:{record_id}:");
        self.bytes.retain(|key, _| !key.starts_with(&frame_prefix));
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BodySide {
    Request,
    Response,
}

const MAX_PREVIEW_CONTEXT: usize = 50;
const DEFAULT_BATCH_SIZE: usize = 50;
const SEARCH_BATCH_SIZE: usize = 1000;
const DEFAULT_MAX_SCAN: usize = 100_000;
const DEFAULT_STREAM_MAX_RESULTS: usize = 100;
const SEARCH_TIMEOUT: Duration = Duration::from_secs(300);
const DEFAULT_MAX_SEARCH_DECOMPRESSED_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_TARGET_RECORD_IDS: usize = 500;

pub struct SearchEngine {
    traffic_db: SharedTrafficDbStore,
    body_store: Option<SharedBodyStore>,
    frame_store: Option<SharedFrameStore>,
    connection_monitor: Option<SharedConnectionMonitor>,
    max_decompress_output_bytes: usize,
    max_search_decompressed_bytes: usize,
}

#[derive(Debug, Clone)]
pub struct SearchProgress {
    pub iterations: usize,
    pub total_searched: usize,
    pub total_matched: usize,
    pub cursor: Option<u64>,
    pub has_more_hint: bool,
}

impl SearchEngine {
    pub fn new(traffic_db: SharedTrafficDbStore, body_store: Option<SharedBodyStore>) -> Self {
        Self {
            traffic_db,
            body_store,
            frame_store: None,
            connection_monitor: None,
            max_decompress_output_bytes: DEFAULT_MAX_DECOMPRESSED_BODY_BYTES,
            max_search_decompressed_bytes: DEFAULT_MAX_SEARCH_DECOMPRESSED_BYTES,
        }
    }

    pub fn with_frame_support(
        traffic_db: SharedTrafficDbStore,
        body_store: Option<SharedBodyStore>,
        frame_store: Option<SharedFrameStore>,
        connection_monitor: Option<SharedConnectionMonitor>,
    ) -> Self {
        Self {
            traffic_db,
            body_store,
            frame_store,
            connection_monitor,
            max_decompress_output_bytes: DEFAULT_MAX_DECOMPRESSED_BODY_BYTES,
            max_search_decompressed_bytes: DEFAULT_MAX_SEARCH_DECOMPRESSED_BYTES,
        }
    }

    pub fn with_decompression_limit(mut self, max_output_bytes: usize) -> Self {
        self.max_decompress_output_bytes = max_output_bytes.max(1);
        self
    }

    #[cfg(test)]
    fn with_decompression_budget(mut self, max_output_bytes: usize, total_bytes: usize) -> Self {
        self.max_decompress_output_bytes = max_output_bytes.max(1);
        self.max_search_decompressed_bytes = total_bytes;
        self
    }

    pub fn search(&self, request: &SearchRequest) -> SearchResponse {
        self.search_internal(request, false, |_| {}, |_| {})
    }

    pub fn search_stream<F, P>(
        &self,
        request: &SearchRequest,
        on_result: F,
        on_progress: P,
    ) -> SearchResponse
    where
        F: FnMut(&SearchResultItem),
        P: FnMut(&SearchProgress),
    {
        self.search_internal(request, true, on_result, on_progress)
    }

    fn search_internal<F, P>(
        &self,
        request: &SearchRequest,
        streaming: bool,
        mut on_result: F,
        mut on_progress: P,
    ) -> SearchResponse
    where
        F: FnMut(&SearchResultItem),
        P: FnMut(&SearchProgress),
    {
        let search_id = generate_search_id();
        let batch_size = request.limit.unwrap_or(DEFAULT_BATCH_SIZE);
        let max_results = if streaming {
            request.max_results.unwrap_or(DEFAULT_STREAM_MAX_RESULTS)
        } else {
            batch_size
        };
        let keyword_lower = request.keyword.to_lowercase();
        let has_keyword = !keyword_lower.trim().is_empty();
        let started_at = Instant::now();
        let max_total_searched = request.max_scan.unwrap_or(DEFAULT_MAX_SCAN);

        let scope = &request.scope;
        let conds = &request.filters.conditions;
        let cond_needs_req_header = conds.iter().any(|c| c.field.starts_with("req.header."));
        let cond_needs_res_header = conds.iter().any(|c| c.field.starts_with("res.header."));
        let cond_needs_req_body = conds.iter().any(|c| c.field.starts_with("req.body."));
        let cond_needs_res_body = conds.iter().any(|c| c.field.starts_with("res.body."));
        let need_url = (has_keyword && scope.should_search_url())
            || conds.iter().any(|c| c.field.as_str() == "url");
        let need_request_headers =
            (has_keyword && scope.should_search_request_headers()) || cond_needs_req_header;
        let need_response_headers =
            (has_keyword && scope.should_search_response_headers()) || cond_needs_res_header;
        let need_request_body_ref =
            (has_keyword && scope.should_search_request_body()) || cond_needs_req_body;
        let need_response_body_ref =
            (has_keyword && scope.should_search_response_body()) || cond_needs_res_body;

        debug!(
            keyword = %request.keyword,
            scope = ?request.scope,
            cursor = ?request.cursor,
            limit = batch_size,
            max_results = max_results,
            max_scan = max_total_searched,
            streaming = streaming,
            "[SEARCH] Starting iterative search"
        );

        let mut results = Vec::new();
        let mut total_searched = 0;
        let mut current_cursor = request.cursor;
        let mut iterations = 0;
        let mut db_has_more = true;
        let mut timed_out = false;
        let mut decompression_budget_exhausted = false;

        let mut body_cache = BodyReadCache::new(self.max_search_decompressed_bytes);
        let include = &request.include;
        let need_hydrate = include.any();
        let need_include_req_body = include.request_body;
        let need_include_res_body = include.response_body;
        let need_include_req_headers = include.request_headers;
        let need_include_res_headers = include.response_headers;
        // Promote fields fetches when hydration needs them but search/filter does not.
        let need_request_headers = need_request_headers || need_include_req_headers;
        let need_response_headers = need_response_headers || need_include_res_headers;
        let need_request_body_ref = need_request_body_ref || need_include_req_body;
        let need_response_body_ref = need_response_body_ref || need_include_res_body;
        let mut oldest_ts_ms: Option<i64> = None;
        let mut newest_ts_ms: Option<i64> = None;
        let mut scanned_count: usize = 0;

        'search: while results.len() < max_results
            && total_searched < max_total_searched
            && db_has_more
        {
            if started_at.elapsed() >= SEARCH_TIMEOUT {
                warn!(
                    elapsed_ms = started_at.elapsed().as_millis() as u64,
                    iterations,
                    total_searched,
                    matched = results.len(),
                    "[SEARCH] Timeout reached, returning partial results"
                );
                timed_out = true;
                break;
            }

            iterations += 1;

            let query_params = self.build_query_params_with_cursor(request, current_cursor);
            let query_result = self.traffic_db.query_for_search(&query_params);

            if query_result.records.is_empty() {
                db_has_more = false;
                break;
            }

            debug!(
                iteration = iterations,
                candidates = query_result.records.len(),
                current_results = results.len(),
                total_searched = total_searched,
                "[SEARCH] Processing batch"
            );

            let candidate_ids: Vec<&str> = query_result
                .records
                .iter()
                .filter(|c| self.matches_filter_compact(c, &request.filters))
                .map(|c| c.id.as_str())
                .collect();

            let fields_map = if candidate_ids.is_empty() {
                std::collections::HashMap::new()
            } else {
                self.traffic_db.get_search_fields_by_ids(
                    &candidate_ids,
                    need_url,
                    need_request_headers,
                    need_response_headers,
                    need_request_body_ref,
                    need_response_body_ref,
                )
            };

            for compact in &query_result.records {
                body_cache.current_record_started_with_fresh_budget =
                    body_cache.remaining_decompressed_bytes == body_cache.max_decompressed_bytes;
                body_cache.current_record_exceeds_decompression_budget = false;
                let previous_cursor = current_cursor;
                let previous_oldest_ts_ms = oldest_ts_ms;
                let previous_newest_ts_ms = newest_ts_ms;
                total_searched += 1;
                scanned_count += 1;
                current_cursor = Some(compact.seq);

                let ts_signed = compact.ts as i64;
                oldest_ts_ms = Some(match oldest_ts_ms {
                    Some(prev) => prev.min(ts_signed),
                    None => ts_signed,
                });
                newest_ts_ms = Some(match newest_ts_ms {
                    Some(prev) => prev.max(ts_signed),
                    None => ts_signed,
                });

                if !self.matches_filter_compact(compact, &request.filters) {
                    if total_searched >= max_total_searched {
                        break;
                    }
                    continue;
                }

                let fields = fields_map.get(&compact.id);

                let conditions_match = request.filters.conditions.is_empty()
                    || self.matches_conditions_compact_with_cache(
                        compact,
                        fields,
                        &request.filters.conditions,
                        &mut body_cache,
                    );
                if body_cache.decompression_budget_exhausted {
                    decompression_budget_exhausted = true;
                    if !body_cache.current_record_exceeds_decompression_budget {
                        total_searched = total_searched.saturating_sub(1);
                        scanned_count = scanned_count.saturating_sub(1);
                        current_cursor = previous_cursor;
                        oldest_ts_ms = previous_oldest_ts_ms;
                        newest_ts_ms = previous_newest_ts_ms;
                    }
                    break 'search;
                }
                if !conditions_match {
                    body_cache.remove_record(&compact.id);
                    if total_searched >= max_total_searched {
                        break;
                    }
                    continue;
                }

                let result =
                    self.search_compact(scope, &keyword_lower, compact, fields, &mut body_cache);
                if body_cache.decompression_budget_exhausted {
                    decompression_budget_exhausted = true;
                    if !body_cache.current_record_exceeds_decompression_budget {
                        total_searched = total_searched.saturating_sub(1);
                        scanned_count = scanned_count.saturating_sub(1);
                        current_cursor = previous_cursor;
                        oldest_ts_ms = previous_oldest_ts_ms;
                        newest_ts_ms = previous_newest_ts_ms;
                    }
                    break 'search;
                }
                if let Some(mut result) = result {
                    if need_hydrate {
                        self.hydrate_result_item(
                            &mut result,
                            compact,
                            fields,
                            include,
                            &mut body_cache,
                        );
                        if body_cache.decompression_budget_exhausted {
                            decompression_budget_exhausted = true;
                            if !body_cache.current_record_exceeds_decompression_budget {
                                total_searched = total_searched.saturating_sub(1);
                                scanned_count = scanned_count.saturating_sub(1);
                                current_cursor = previous_cursor;
                                oldest_ts_ms = previous_oldest_ts_ms;
                                newest_ts_ms = previous_newest_ts_ms;
                            }
                            break 'search;
                        }
                    }
                    results.push(result);
                    if let Some(last) = results.last() {
                        on_result(last);
                    }
                    if !streaming && results.len() >= max_results {
                        body_cache.remove_record(&compact.id);
                        break;
                    }
                }

                body_cache.remove_record(&compact.id);

                if total_searched >= max_total_searched {
                    break;
                }
            }

            db_has_more = query_result.has_more;
            on_progress(&SearchProgress {
                iterations,
                total_searched,
                total_matched: results.len(),
                cursor: current_cursor,
                has_more_hint: db_has_more && total_searched < max_total_searched,
            });
        }

        let has_more = timed_out
            || decompression_budget_exhausted
            || (db_has_more && total_searched < max_total_searched);
        let total_matched = results.len();

        debug!(
            iterations = iterations,
            total_searched = total_searched,
            matched = total_matched,
            has_more = has_more,
            timed_out = timed_out,
            elapsed_ms = started_at.elapsed().as_millis() as u64,
            "[SEARCH] Iterative search completed"
        );

        SearchResponse {
            results,
            total_searched,
            total_matched,
            next_cursor: current_cursor,
            has_more,
            partial_reason: decompression_budget_exhausted
                .then(|| "decompression_budget_exhausted".to_string()),
            search_id,
            searched_range: SearchedRange {
                oldest_ts_ms,
                newest_ts_ms,
                scanned_count,
            },
        }
    }

    fn search_compact(
        &self,
        scope: &SearchScope,
        keyword: &str,
        compact: &TrafficSummaryCompact,
        fields: Option<&TrafficSearchFields>,
        body_cache: &mut BodyReadCache,
    ) -> Option<SearchResultItem> {
        if keyword.trim().is_empty() {
            return Some(result_item(compact.clone(), Vec::new()));
        }

        // 搜索目标是尽快返回结果：一条 record 只要命中一次就足够展示。
        // 因此这里按“便宜 -> 昂贵”的顺序，并在首次命中后立即返回。

        if scope.should_search_url() {
            let url_text = fields
                .and_then(|f| f.url.as_deref())
                .map(|s| s.to_string())
                .unwrap_or_else(|| build_compact_url(compact));
            if let Some(m) = self.search_text(&url_text, keyword, "url") {
                return Some(result_item(compact.clone(), vec![m]));
            }
        }

        if scope.should_search_request_headers() {
            if let Some(headers) = fields.and_then(|f| f.request_headers.as_ref()) {
                for (k, v) in headers {
                    let header_text = format!("{}: {}", k, v);
                    if let Some(m) = self.search_text(&header_text, keyword, "request_header") {
                        return Some(result_item(compact.clone(), vec![m]));
                    }
                }
            }
        }

        if scope.should_search_response_headers() {
            if let Some(headers) = fields.and_then(|f| f.response_headers.as_ref()) {
                for (k, v) in headers {
                    let header_text = format!("{}: {}", k, v);
                    if let Some(m) = self.search_text(&header_text, keyword, "response_header") {
                        return Some(result_item(compact.clone(), vec![m]));
                    }
                }
            }
        }

        if scope.should_search_request_body() {
            if let Some(body_ref) = fields.and_then(|f| f.request_body_ref.as_ref()) {
                if let Some(m) = self.search_body(
                    body_ref,
                    keyword,
                    "request_body",
                    &format!("req:{}", compact.id),
                    body_cache,
                ) {
                    return Some(result_item(compact.clone(), vec![m]));
                }
            }
        }

        if scope.should_search_response_body() {
            if let Some(body_ref) = fields.and_then(|f| {
                f.derived_response_body_ref
                    .as_ref()
                    .or(f.response_body_ref.as_ref())
            }) {
                if let Some(m) = self.search_body(
                    body_ref,
                    keyword,
                    "response_body",
                    &format!("res:{}", compact.id),
                    body_cache,
                ) {
                    return Some(result_item(compact.clone(), vec![m]));
                }
            }
        }

        // WS/SSE frame 搜索最贵，且只在对应记录上启用。
        let is_websocket = (compact.flags & crate::traffic_db::TrafficFlags::IS_WEBSOCKET) != 0;
        let is_sse = (compact.flags & crate::traffic_db::TrafficFlags::IS_SSE) != 0;

        if is_websocket && scope.should_search_websocket_messages() {
            if let Some(frame_matches) =
                self.search_frames(&compact.id, keyword, "websocket_message", body_cache)
            {
                if let Some(first) = frame_matches.into_iter().next() {
                    return Some(result_item(compact.clone(), vec![first]));
                }
            }
        }

        if is_sse && scope.should_search_sse_events() {
            if let Some(frame_matches) =
                self.search_frames(&compact.id, keyword, "sse_event", body_cache)
            {
                if let Some(first) = frame_matches.into_iter().next() {
                    return Some(result_item(compact.clone(), vec![first]));
                }
            }
        }

        None
    }

    fn build_query_params_with_cursor(
        &self,
        request: &SearchRequest,
        cursor: Option<u64>,
    ) -> QueryParams {
        let mut params = QueryParams {
            cursor,
            limit: Some(SEARCH_BATCH_SIZE),
            direction: crate::traffic_db::Direction::Backward,
            record_ids: (!request.record_ids.is_empty()).then(|| request.record_ids.clone()),
            ..Default::default()
        };

        let filters = &request.filters;

        if let Some(rule_hit) = filters.has_rule_hit {
            params.has_rule_hit = Some(rule_hit);
        }

        if let Some(time_range) = request.time_range.as_ref() {
            params.since_ms = time_range.since_ms;
            params.until_ms = time_range.until_ms;
        }

        for protocol in &filters.protocols {
            match protocol.to_uppercase().as_str() {
                "WS" | "WSS" => params.is_websocket = Some(true),
                "H3" => params.is_h3 = Some(true),
                _ => {}
            }
        }

        for condition in &filters.conditions {
            match condition.field.as_str() {
                "host" if condition.operator == "contains" || condition.operator == "equals" => {
                    params.host_contains = Some(condition.value.clone());
                }
                "path" if condition.operator == "contains" || condition.operator == "equals" => {
                    params.path_contains = Some(condition.value.clone());
                }
                "url" if condition.operator == "contains" || condition.operator == "equals" => {
                    params.url_contains = Some(condition.value.clone());
                }
                "method" if condition.operator == "equals" => {
                    params.method = Some(condition.value.clone());
                }
                "client_app" => match condition.operator.as_str() {
                    "equals" => {
                        params.client_app = Some(condition.value.clone());
                        params.client_app_match = TextMatchMode::Equals;
                    }
                    "is_empty" => params.client_app_empty = Some(true),
                    "is_not_empty" => params.client_app_empty = Some(false),
                    "contains" => {
                        params.client_app = Some(condition.value.clone());
                    }
                    _ => {}
                },
                "client_ip" => match condition.operator.as_str() {
                    "equals" => {
                        params.client_ip = Some(condition.value.clone());
                        params.client_ip_match = TextMatchMode::Equals;
                    }
                    "is_empty" => params.client_ip_empty = Some(true),
                    "is_not_empty" => params.client_ip_empty = Some(false),
                    "contains" => {
                        params.client_ip = Some(condition.value.clone());
                    }
                    _ => {}
                },
                "listener_port" | "port" if condition.operator == "equals" => {
                    params.listener_port = condition.value.parse().ok();
                }
                "content_type" => {
                    params.content_type = Some(condition.value.clone());
                }
                _ => {}
            }
        }

        params
    }

    #[allow(dead_code)]
    fn matches_conditions_compact(
        &self,
        compact: &TrafficSummaryCompact,
        fields: Option<&TrafficSearchFields>,
        conditions: &[FilterCondition],
    ) -> bool {
        for condition in conditions {
            if !self.matches_condition_compact(compact, fields, condition) {
                return false;
            }
        }
        true
    }

    fn matches_conditions_compact_with_cache(
        &self,
        compact: &TrafficSummaryCompact,
        fields: Option<&TrafficSearchFields>,
        conditions: &[FilterCondition],
        body_cache: &mut BodyReadCache,
    ) -> bool {
        for condition in conditions {
            let field = condition.field.as_str();
            let matched = if field == "ts" {
                eval_ts_condition(compact.ts as i64, condition)
            } else if let Some(rest) = field.strip_prefix("req.header.") {
                eval_header_condition(
                    fields.and_then(|f| f.request_headers.as_ref()),
                    rest,
                    condition,
                )
            } else if let Some(rest) = field.strip_prefix("res.header.") {
                eval_header_condition(
                    fields.and_then(|f| f.response_headers.as_ref()),
                    rest,
                    condition,
                )
            } else if let Some(path) = field.strip_prefix("req.body.") {
                let body_ref = fields.and_then(|f| f.request_body_ref.as_ref());
                self.eval_body_json_condition(
                    body_ref,
                    &compact.id,
                    BodySide::Request,
                    path,
                    condition,
                    body_cache,
                )
            } else if let Some(path) = field.strip_prefix("res.body.") {
                let body_ref = fields.and_then(|f| {
                    f.derived_response_body_ref
                        .as_ref()
                        .or(f.response_body_ref.as_ref())
                });
                self.eval_body_json_condition(
                    body_ref,
                    &compact.id,
                    BodySide::Response,
                    path,
                    condition,
                    body_cache,
                )
            } else {
                self.matches_condition_compact(compact, fields, condition)
            };
            if !matched {
                return false;
            }
        }
        true
    }

    fn eval_body_json_condition(
        &self,
        body_ref: Option<&BodyRef>,
        record_id: &str,
        side: BodySide,
        path: &str,
        condition: &FilterCondition,
        body_cache: &mut BodyReadCache,
    ) -> bool {
        let cache_key = format!(
            "{}:{}",
            match side {
                BodySide::Request => "req",
                BodySide::Response => "res",
            },
            record_id
        );
        if !body_cache.json.contains_key(&cache_key) {
            let entry = match body_ref
                .and_then(|body_ref| self.load_body_bytes_cached(&cache_key, body_ref, body_cache))
            {
                Some(bytes) => match serde_json::from_slice::<JsonValue>(bytes) {
                    Ok(v) => BodyCacheEntry::Json(v),
                    Err(_) => BodyCacheEntry::NonJson,
                },
                None => BodyCacheEntry::Missing,
            };
            body_cache.json.insert(cache_key.clone(), entry);
        }
        let entry = body_cache.json.get(&cache_key).expect("just inserted");
        let json = match entry {
            BodyCacheEntry::Json(v) => v,
            BodyCacheEntry::NonJson | BodyCacheEntry::Missing => return false,
        };
        let normalized = if path.starts_with('$') {
            path.to_string()
        } else {
            format!("$.{}", path)
        };
        let nodes = json_path::eval(json, &normalized);
        if nodes.is_empty() {
            return condition.operator == "is_empty" || condition.operator == "not_contains";
        }
        for node in nodes {
            if eval_value_condition(node, condition) {
                return true;
            }
        }
        false
    }

    fn matches_condition_compact(
        &self,
        compact: &TrafficSummaryCompact,
        fields: Option<&TrafficSearchFields>,
        condition: &FilterCondition,
    ) -> bool {
        let url_fallback;
        let field_value: &str = match condition.field.as_str() {
            "url" => {
                if let Some(u) = fields.and_then(|f| f.url.as_deref()) {
                    u
                } else {
                    url_fallback = build_compact_url(compact);
                    &url_fallback
                }
            }
            "host" => compact.h.as_str(),
            "path" => compact.p.as_str(),
            "method" => compact.m.as_str(),
            "content_type" => compact.ct.as_deref().unwrap_or(""),
            "client_app" => compact.capp.as_deref().unwrap_or(""),
            "client_ip" => compact.cip.as_str(),
            "listener_port" | "port" => {
                url_fallback = compact.lp.to_string();
                &url_fallback
            }
            _ => return true,
        };

        let field_lower = field_value.to_lowercase();
        let value_lower = condition.value.to_lowercase();

        match condition.operator.as_str() {
            "contains" => field_lower.contains(&value_lower),
            "equals" => field_lower == value_lower,
            "not_contains" => !field_lower.contains(&value_lower),
            "is_empty" => field_value.trim().is_empty(),
            "is_not_empty" => !field_value.trim().is_empty(),
            "regex" => Regex::new(&condition.value)
                .map(|re| re.is_match(field_value))
                .unwrap_or(false),
            _ => field_lower.contains(&value_lower),
        }
    }

    fn search_text(&self, text: &str, keyword: &str, field: &str) -> Option<MatchLocation> {
        if text.is_ascii() && keyword.is_ascii() {
            return find_ascii_case_insensitive(text.as_bytes(), keyword.as_bytes())
                .map(|pos| build_text_match(text, pos, keyword.len(), field));
        }

        let text_lower = text.to_lowercase();
        text_lower
            .find(keyword)
            .map(|pos| build_text_match(text, pos, keyword.len(), field))
    }

    fn search_body(
        &self,
        body_ref: &BodyRef,
        keyword: &str,
        field: &str,
        cache_key: &str,
        body_cache: &mut BodyReadCache,
    ) -> Option<MatchLocation> {
        let bytes = self.load_body_bytes_cached(cache_key, body_ref, body_cache)?;
        self.search_body_bytes(bytes, keyword, field)
    }

    fn load_decoded_body_bytes(
        &self,
        body_ref: &BodyRef,
        remaining_decompressed_bytes: &mut usize,
        decompression_budget_exhausted: &mut bool,
        current_record_started_with_fresh_budget: bool,
        current_record_exceeds_decompression_budget: &mut bool,
    ) -> Option<Vec<u8>> {
        let bytes = match body_ref {
            BodyRef::Inline { data } => Some(data.as_bytes().to_vec()),
            other => self.body_store.as_ref()?.read().load_bytes(other),
        }?;
        let Some(content_encoding) = body_ref.content_encoding() else {
            return Some(bytes);
        };
        if !content_encoding_is_supported(&content_encoding) {
            return Some(bytes);
        }

        let output_limit = self
            .max_decompress_output_bytes
            .min(*remaining_decompressed_bytes);
        let globally_limited = *remaining_decompressed_bytes < self.max_decompress_output_bytes;
        if output_limit == 0 {
            *decompression_budget_exhausted = true;
            *current_record_exceeds_decompression_budget = current_record_started_with_fresh_budget;
            return None;
        }
        match decompress_with_limit_metered(&bytes, &content_encoding, output_limit) {
            Ok((decoded, consumed_bytes)) => {
                *remaining_decompressed_bytes =
                    remaining_decompressed_bytes.saturating_sub(consumed_bytes);
                Some(decoded)
            }
            Err(_) => {
                // A decoder may already have produced `output_limit` bytes
                // before reporting malformed input or an over-limit body.
                // Charge the whole attempt so repeated bombs cannot bypass
                // the request-wide budget through the raw fallback path.
                *remaining_decompressed_bytes =
                    remaining_decompressed_bytes.saturating_sub(output_limit);
                if globally_limited {
                    *decompression_budget_exhausted = true;
                    *current_record_exceeds_decompression_budget =
                        current_record_started_with_fresh_budget;
                    None
                } else {
                    Some(bytes)
                }
            }
        }
    }

    fn load_body_bytes_cached<'a>(
        &self,
        cache_key: &str,
        body_ref: &BodyRef,
        body_cache: &'a mut BodyReadCache,
    ) -> Option<&'a [u8]> {
        if !body_cache.bytes.contains_key(cache_key) {
            let bytes = self.load_decoded_body_bytes(
                body_ref,
                &mut body_cache.remaining_decompressed_bytes,
                &mut body_cache.decompression_budget_exhausted,
                body_cache.current_record_started_with_fresh_budget,
                &mut body_cache.current_record_exceeds_decompression_budget,
            );
            body_cache.bytes.insert(cache_key.to_string(), bytes);
        }
        body_cache.bytes.get(cache_key)?.as_deref()
    }

    fn search_body_bytes(&self, bytes: &[u8], keyword: &str, field: &str) -> Option<MatchLocation> {
        if bytes.is_ascii() && keyword.is_ascii() {
            return find_ascii_case_insensitive(bytes, keyword.as_bytes())
                .map(|pos| build_bytes_match(bytes, pos, keyword.len(), field));
        }

        let content = String::from_utf8_lossy(bytes);
        self.search_text(&content, keyword, field)
    }

    fn search_frames(
        &self,
        connection_id: &str,
        keyword: &str,
        field: &str,
        body_cache: &mut BodyReadCache,
    ) -> Option<Vec<MatchLocation>> {
        use std::collections::HashSet;

        let mut matches = Vec::new();
        let mut seen_frame_ids: HashSet<u64> = HashSet::new();

        if let Some(ref monitor) = self.connection_monitor {
            if let Some((frames, _)) = monitor.get_frames(connection_id, None, usize::MAX) {
                for frame in frames {
                    if seen_frame_ids.contains(&frame.frame_id) {
                        continue;
                    }
                    seen_frame_ids.insert(frame.frame_id);

                    if let Some(preview) = &frame.payload_preview {
                        if let Some(m) = self.search_text(preview, keyword, field) {
                            matches.push(m);
                            break;
                        }
                    }

                    if let Some(body_ref) = &frame.payload_ref {
                        if let Some(m) = self.search_body(
                            body_ref,
                            keyword,
                            field,
                            &format!("frame:{connection_id}:{}", frame.frame_id),
                            body_cache,
                        ) {
                            matches.push(m);
                            break;
                        }
                    }
                }
            }
        }

        if matches.is_empty() {
            if let Some(ref fs) = self.frame_store {
                if let Ok(frames) = fs.load_all_frames(connection_id) {
                    for frame in frames {
                        if seen_frame_ids.contains(&frame.frame_id) {
                            continue;
                        }
                        seen_frame_ids.insert(frame.frame_id);

                        if let Some(preview) = &frame.payload_preview {
                            if let Some(m) = self.search_text(preview, keyword, field) {
                                matches.push(m);
                                break;
                            }
                        }

                        if let Some(body_ref) = &frame.payload_ref {
                            if let Some(m) = self.search_body(
                                body_ref,
                                keyword,
                                field,
                                &format!("frame:{connection_id}:{}", frame.frame_id),
                                body_cache,
                            ) {
                                matches.push(m);
                                break;
                            }
                        }
                    }
                }
            }
        }

        if matches.is_empty() {
            None
        } else {
            Some(matches)
        }
    }

    fn matches_filter_compact(
        &self,
        compact: &TrafficSummaryCompact,
        filters: &SearchFilters,
    ) -> bool {
        use crate::TrafficFlags;

        if let Some(rule_hit) = filters.has_rule_hit {
            let has_rule = (compact.flags & TrafficFlags::HAS_RULE_HIT) != 0;
            if has_rule != rule_hit {
                return false;
            }
        }

        if !filters.protocols.is_empty() {
            let protocol_upper = compact.proto.to_uppercase();
            let is_websocket = (compact.flags & TrafficFlags::IS_WEBSOCKET) != 0;
            let is_sse = (compact.flags & TrafficFlags::IS_SSE) != 0;
            let is_h3 = (compact.flags & TrafficFlags::IS_H3) != 0;
            let mut matched = false;

            for p in &filters.protocols {
                match p.to_uppercase().as_str() {
                    "HTTP"
                        if protocol_upper == "HTTP"
                            || protocol_upper == "HTTP/1.0"
                            || protocol_upper == "HTTP/1.1" =>
                    {
                        matched = true;
                        break;
                    }
                    "HTTPS" if protocol_upper == "HTTPS" || protocol_upper == "HTTP/2" => {
                        matched = true;
                        break;
                    }
                    "H2" if protocol_upper.contains("HTTP/2") => {
                        matched = true;
                        break;
                    }
                    "WS" if is_websocket && protocol_upper == "WS" => {
                        matched = true;
                        break;
                    }
                    "WSS" if is_websocket && protocol_upper == "WSS" => {
                        matched = true;
                        break;
                    }
                    "H3" if is_h3 || protocol_upper == "H3" => {
                        matched = true;
                        break;
                    }
                    "SSE" if is_sse => {
                        matched = true;
                        break;
                    }
                    _ => {}
                }
            }

            if !matched {
                return false;
            }
        }

        if !filters.status_ranges.is_empty() {
            let status = compact.s;
            let mut matched = false;

            for range in &filters.status_ranges {
                match range.as_str() {
                    "error" if status == 0 || status >= 500 => {
                        matched = true;
                        break;
                    }
                    "1xx" if (100..200).contains(&status) => {
                        matched = true;
                        break;
                    }
                    "2xx" if (200..300).contains(&status) => {
                        matched = true;
                        break;
                    }
                    "3xx" if (300..400).contains(&status) => {
                        matched = true;
                        break;
                    }
                    "4xx" if (400..500).contains(&status) => {
                        matched = true;
                        break;
                    }
                    "5xx" if (500..600).contains(&status) => {
                        matched = true;
                        break;
                    }
                    _ => {}
                }
            }

            if !matched {
                return false;
            }
        }

        if !filters.content_types.is_empty() {
            let res_ct = compact.ct.as_deref().unwrap_or("").to_lowercase();
            let req_ct = compact.req_ct.as_deref().unwrap_or("").to_lowercase();
            let mut matched = false;

            for ct in &filters.content_types {
                let ct_lower = ct.to_lowercase();
                let patterns: Vec<&str> = match ct_lower.as_str() {
                    "json" => vec!["json", "application/json", "text/json"],
                    "form" => vec![
                        "form",
                        "x-www-form-urlencoded",
                        "multipart/form-data",
                        "application/x-www-form-urlencoded",
                    ],
                    "xml" => vec!["xml", "application/xml", "text/xml"],
                    "js" => vec!["javascript", "text/javascript", "application/javascript"],
                    "css" => vec!["css", "text/css"],
                    "font" => vec![
                        "font",
                        "woff",
                        "woff2",
                        "ttf",
                        "otf",
                        "eot",
                        "font/",
                        "application/font",
                    ],
                    "doc" => vec!["html", "text/html", "application/xhtml"],
                    "media" => vec![
                        "image", "video", "audio", "image/", "video/", "audio/", "png", "jpg",
                        "jpeg", "gif", "webp", "svg", "mp4", "webm", "mp3", "wav",
                    ],
                    "sse" => vec!["event-stream", "text/event-stream"],
                    _ => vec![ct_lower.as_str()],
                };

                for pattern in patterns {
                    if res_ct.contains(pattern) || req_ct.contains(pattern) {
                        matched = true;
                        break;
                    }
                }

                if matched {
                    break;
                }
            }

            if !matched {
                return false;
            }
        }

        if !filters.client_ips.is_empty() && !filters.client_ips.contains(&compact.cip) {
            return false;
        }

        if !filters.client_apps.is_empty() {
            match &compact.capp {
                Some(app) if filters.client_apps.contains(app) => {}
                _ => return false,
            }
        }

        if !filters.account_names.is_empty() {
            match &compact.acct {
                Some(account_name) if filters.account_names.contains(account_name) => {}
                _ => return false,
            }
        }

        if !filters.domains.is_empty() {
            let host = &compact.h;
            if !filters.domains.iter().any(|d| host.contains(d)) {
                return false;
            }
        }

        true
    }
}

/// Construct a `SearchResultItem` with `bodies`/`headers` defaulted to None.
/// Hydration (when requested) is layered on top by `SearchEngine::hydrate_result_item`.
fn result_item(record: TrafficSummaryCompact, matches: Vec<MatchLocation>) -> SearchResultItem {
    SearchResultItem {
        record,
        matches,
        bodies: None,
        headers: None,
    }
}

impl SearchEngine {
    /// Attach bodies/headers to a `SearchResultItem` when `SearchInclude` flags request it.
    ///
    /// Body bytes are deduplicated per-record-per-side via `body_cache` so a record
    /// that matched on both bodies (or had a JSONPath condition pre-load it) will not hit the
    /// body store more than once. Truncation honours `include.body_limit()` and sets
    /// `BodyChunk.truncated = true` while still reporting the original `size`.
    fn hydrate_result_item(
        &self,
        item: &mut SearchResultItem,
        compact: &TrafficSummaryCompact,
        fields: Option<&TrafficSearchFields>,
        include: &SearchInclude,
        body_cache: &mut BodyReadCache,
    ) {
        if include.request_headers || include.response_headers {
            let request = if include.request_headers {
                fields
                    .and_then(|f| f.request_headers.clone())
                    .unwrap_or_default()
            } else {
                Vec::new()
            };
            let response = if include.response_headers {
                fields
                    .and_then(|f| f.response_headers.clone())
                    .unwrap_or_default()
            } else {
                Vec::new()
            };
            item.headers = Some(HeadersPayload { request, response });
        }

        if include.request_body || include.response_body {
            let limit = include.body_limit();
            let request_chunk = if include.request_body {
                let body_ref = fields.and_then(|f| f.request_body_ref.as_ref());
                self.load_body_chunk(
                    &compact.id,
                    BodySide::Request,
                    body_ref,
                    compact.req_ct.clone(),
                    limit,
                    body_cache,
                )
            } else {
                None
            };
            let response_chunk = if include.response_body {
                let body_ref = fields.and_then(|f| {
                    f.derived_response_body_ref
                        .as_ref()
                        .or(f.response_body_ref.as_ref())
                });
                self.load_body_chunk(
                    &compact.id,
                    BodySide::Response,
                    body_ref,
                    compact.ct.clone(),
                    limit,
                    body_cache,
                )
            } else {
                None
            };
            if request_chunk.is_some() || response_chunk.is_some() {
                item.bodies = Some(BodiesPayload {
                    request: request_chunk,
                    response: response_chunk,
                });
            }
        }
    }

    fn load_body_chunk(
        &self,
        record_id: &str,
        side: BodySide,
        body_ref: Option<&BodyRef>,
        content_type: Option<String>,
        limit: usize,
        body_cache: &mut BodyReadCache,
    ) -> Option<BodyChunk> {
        let body_ref = body_ref?;
        let cache_key = format!(
            "{}:{}",
            match side {
                BodySide::Request => "req",
                BodySide::Response => "res",
            },
            record_id
        );
        let bytes = self.load_body_bytes_cached(&cache_key, body_ref, body_cache)?;
        let original_size = bytes.len();
        let (slice, truncated) = if original_size > limit {
            (&bytes[..limit], true)
        } else {
            (bytes, false)
        };
        Some(BodyChunk {
            bytes_b64: BASE64.encode(slice),
            size: original_size,
            truncated,
            content_type,
        })
    }
}

fn build_compact_url(compact: &TrafficSummaryCompact) -> String {
    // compact 中 proto/h/p 是 UI 展示和过滤的核心字段。
    // 这里仅用于搜索预览/匹配，避免为了 URL 再回表查整条 record。
    let scheme = compact.proto.trim();
    if scheme.is_empty() {
        format!("http://{}{}", compact.h, compact.p)
    } else {
        format!("{}://{}{}", scheme, compact.h, compact.p)
    }
}

fn generate_search_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis();
    format!("search_{}", timestamp)
}

fn find_char_boundary(s: &str, byte_index: usize, search_forward: bool) -> usize {
    if byte_index >= s.len() {
        return s.len();
    }

    if s.is_char_boundary(byte_index) {
        return byte_index;
    }

    if search_forward {
        for i in byte_index..s.len() {
            if s.is_char_boundary(i) {
                return i;
            }
        }
        s.len()
    } else {
        for i in (0..byte_index).rev() {
            if s.is_char_boundary(i) {
                return i;
            }
        }
        0
    }
}

fn find_ascii_case_insensitive(haystack: &[u8], needle_lower: &[u8]) -> Option<usize> {
    if needle_lower.is_empty() {
        return Some(0);
    }
    if haystack.len() < needle_lower.len() {
        return None;
    }

    let first = needle_lower[0];
    let last_start = haystack.len() - needle_lower.len();
    let mut start = 0usize;
    while start <= last_start {
        if haystack[start].to_ascii_lowercase() != first {
            start += 1;
            continue;
        }

        let mut matched = true;
        for offset in 1..needle_lower.len() {
            if haystack[start + offset].to_ascii_lowercase() != needle_lower[offset] {
                matched = false;
                break;
            }
        }
        if matched {
            return Some(start);
        }
        start += 1;
    }

    None
}

fn build_text_match(text: &str, pos: usize, keyword_len: usize, field: &str) -> MatchLocation {
    let start = find_char_boundary(text, pos.saturating_sub(MAX_PREVIEW_CONTEXT), false);
    let end = find_char_boundary(
        text,
        (pos + keyword_len + MAX_PREVIEW_CONTEXT).min(text.len()),
        true,
    );

    let preview = if start > 0 || end < text.len() {
        let prefix = if start > 0 { "..." } else { "" };
        let suffix = if end < text.len() { "..." } else { "" };
        format!("{}{}{}", prefix, &text[start..end], suffix)
    } else {
        text[start..end].to_string()
    };

    MatchLocation {
        field: field.to_string(),
        preview,
        offset: pos,
    }
}

fn build_bytes_match(bytes: &[u8], pos: usize, keyword_len: usize, field: &str) -> MatchLocation {
    let start = pos.saturating_sub(MAX_PREVIEW_CONTEXT);
    let end = (pos + keyword_len + MAX_PREVIEW_CONTEXT).min(bytes.len());
    let preview_body = String::from_utf8_lossy(&bytes[start..end]);
    let preview = if start > 0 || end < bytes.len() {
        let prefix = if start > 0 { "..." } else { "" };
        let suffix = if end < bytes.len() { "..." } else { "" };
        format!("{}{}{}", prefix, preview_body, suffix)
    } else {
        preview_body.to_string()
    };

    MatchLocation {
        field: field.to_string(),
        preview,
        offset: pos,
    }
}

fn parse_ts_value(raw: &str) -> Option<i64> {
    let trimmed = raw.trim();
    if let Ok(n) = trimmed.parse::<i64>() {
        return Some(n);
    }
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(trimmed) {
        return Some(dt.timestamp_millis());
    }
    None
}

fn eval_ts_condition(ts_ms: i64, condition: &FilterCondition) -> bool {
    let target = match parse_ts_value(&condition.value) {
        Some(v) => v,
        None => return false,
    };
    match condition.operator.as_str() {
        "equals" => ts_ms == target,
        "lt" => ts_ms < target,
        "gt" => ts_ms > target,
        "lte" => ts_ms <= target,
        "gte" => ts_ms >= target,
        _ => false,
    }
}

fn eval_header_condition(
    headers: Option<&Vec<(String, String)>>,
    name: &str,
    condition: &FilterCondition,
) -> bool {
    let name_lower = name.to_lowercase();
    let values: Vec<&str> = match headers {
        Some(h) => h
            .iter()
            .filter(|(k, _)| k.to_lowercase() == name_lower)
            .map(|(_, v)| v.as_str())
            .collect(),
        None => Vec::new(),
    };
    match condition.operator.as_str() {
        "is_empty" => values.is_empty() || values.iter().all(|v| v.trim().is_empty()),
        "is_not_empty" => values.iter().any(|v| !v.trim().is_empty()),
        _ => {
            if values.is_empty() {
                return matches!(condition.operator.as_str(), "not_contains");
            }
            let target_lower = condition.value.to_lowercase();
            for v in &values {
                let v_lower = v.to_lowercase();
                let ok = match condition.operator.as_str() {
                    "contains" => v_lower.contains(&target_lower),
                    "equals" => v_lower == target_lower,
                    "not_contains" => !v_lower.contains(&target_lower),
                    "regex" => Regex::new(&condition.value)
                        .map(|re| re.is_match(v))
                        .unwrap_or(false),
                    _ => v_lower.contains(&target_lower),
                };
                if ok {
                    return true;
                }
            }
            false
        }
    }
}

fn eval_value_condition(value: &JsonValue, condition: &FilterCondition) -> bool {
    let as_text = match value {
        JsonValue::String(s) => s.clone(),
        JsonValue::Number(n) => n.to_string(),
        JsonValue::Bool(b) => b.to_string(),
        JsonValue::Null => String::new(),
        _ => value.to_string(),
    };
    let op = condition.operator.as_str();
    if matches!(op, "lt" | "gt" | "lte" | "gte") {
        let lhs = value
            .as_f64()
            .or_else(|| as_text.trim().parse::<f64>().ok());
        let rhs = condition.value.trim().parse::<f64>().ok();
        if let (Some(l), Some(r)) = (lhs, rhs) {
            return match op {
                "lt" => l < r,
                "gt" => l > r,
                "lte" => l <= r,
                "gte" => l >= r,
                _ => false,
            };
        }
        return false;
    }
    let lhs = as_text.to_lowercase();
    let rhs = condition.value.to_lowercase();
    match op {
        "contains" => lhs.contains(&rhs),
        "equals" => lhs == rhs,
        "not_contains" => !lhs.contains(&rhs),
        "is_empty" => as_text.trim().is_empty() || value.is_null(),
        "is_not_empty" => !as_text.trim().is_empty() && !value.is_null(),
        "regex" => Regex::new(&condition.value)
            .map(|re| re.is_match(&as_text))
            .unwrap_or(false),
        _ => lhs.contains(&rhs),
    }
}

#[cfg(test)]
mod tests;
