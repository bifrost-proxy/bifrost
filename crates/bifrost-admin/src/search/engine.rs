use std::time::{Duration, Instant};

use regex::Regex;
use tracing::{debug, warn};

use super::types::{
    FilterCondition, MatchLocation, SearchFilters, SearchRequest, SearchResponse, SearchResultItem,
    SearchScope,
};
use crate::body_store::{BodyRef, SharedBodyStore};
use crate::connection_monitor::SharedConnectionMonitor;
use crate::frame_store::SharedFrameStore;
use crate::traffic_db::{
    QueryParams, SharedTrafficDbStore, TextMatchMode, TrafficSearchFields, TrafficSummaryCompact,
};

const MAX_PREVIEW_CONTEXT: usize = 50;
const DEFAULT_BATCH_SIZE: usize = 50;
const SEARCH_BATCH_SIZE: usize = 1000;
const DEFAULT_MAX_SCAN: usize = 100_000;
const DEFAULT_STREAM_MAX_RESULTS: usize = 100;
const SEARCH_TIMEOUT: Duration = Duration::from_secs(300);

pub struct SearchEngine {
    traffic_db: SharedTrafficDbStore,
    body_store: Option<SharedBodyStore>,
    frame_store: Option<SharedFrameStore>,
    connection_monitor: Option<SharedConnectionMonitor>,
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
        }
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
        let need_url = (has_keyword && scope.should_search_url())
            || request
                .filters
                .conditions
                .iter()
                .any(|c| c.field.as_str() == "url");
        let need_request_headers = has_keyword && scope.should_search_request_headers();
        let need_response_headers = has_keyword && scope.should_search_response_headers();
        let need_request_body_ref = has_keyword && scope.should_search_request_body();
        let need_response_body_ref = has_keyword && scope.should_search_response_body();

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

        while results.len() < max_results && total_searched < max_total_searched && db_has_more {
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
                total_searched += 1;
                current_cursor = Some(compact.seq);

                if !self.matches_filter_compact(compact, &request.filters) {
                    if total_searched >= max_total_searched {
                        break;
                    }
                    continue;
                }

                let fields = fields_map.get(&compact.id);

                if !request.filters.conditions.is_empty()
                    && !self.matches_conditions_compact(
                        compact,
                        fields,
                        &request.filters.conditions,
                    )
                {
                    if total_searched >= max_total_searched {
                        break;
                    }
                    continue;
                }

                if let Some(result) = self.search_compact(scope, &keyword_lower, compact, fields) {
                    results.push(result);
                    if let Some(last) = results.last() {
                        on_result(last);
                    }
                    if !streaming && results.len() >= max_results {
                        break;
                    }
                }

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

        let has_more = timed_out || (db_has_more && total_searched < max_total_searched);
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
            search_id,
        }
    }

    fn search_compact(
        &self,
        scope: &SearchScope,
        keyword: &str,
        compact: &TrafficSummaryCompact,
        fields: Option<&TrafficSearchFields>,
    ) -> Option<SearchResultItem> {
        if keyword.trim().is_empty() {
            return Some(SearchResultItem {
                record: compact.clone(),
                matches: Vec::new(),
            });
        }

        // 搜索目标是尽快返回结果：一条 record 只要命中一次就足够展示。
        // 因此这里按“便宜 -> 昂贵”的顺序，并在首次命中后立即返回。

        if scope.should_search_url() {
            let url_text = fields
                .and_then(|f| f.url.as_deref())
                .map(|s| s.to_string())
                .unwrap_or_else(|| build_compact_url(compact));
            if let Some(m) = self.search_text(&url_text, keyword, "url") {
                return Some(SearchResultItem {
                    record: compact.clone(),
                    matches: vec![m],
                });
            }
        }

        if scope.should_search_request_headers() {
            if let Some(headers) = fields.and_then(|f| f.request_headers.as_ref()) {
                for (k, v) in headers {
                    let header_text = format!("{}: {}", k, v);
                    if let Some(m) = self.search_text(&header_text, keyword, "request_header") {
                        return Some(SearchResultItem {
                            record: compact.clone(),
                            matches: vec![m],
                        });
                    }
                }
            }
        }

        if scope.should_search_response_headers() {
            if let Some(headers) = fields.and_then(|f| f.response_headers.as_ref()) {
                for (k, v) in headers {
                    let header_text = format!("{}: {}", k, v);
                    if let Some(m) = self.search_text(&header_text, keyword, "response_header") {
                        return Some(SearchResultItem {
                            record: compact.clone(),
                            matches: vec![m],
                        });
                    }
                }
            }
        }

        if scope.should_search_request_body() {
            if let Some(body_ref) = fields.and_then(|f| f.request_body_ref.as_ref()) {
                if let Some(m) = self.search_body(body_ref, keyword, "request_body") {
                    return Some(SearchResultItem {
                        record: compact.clone(),
                        matches: vec![m],
                    });
                }
            }
        }

        if scope.should_search_response_body() {
            if let Some(body_ref) = fields.and_then(|f| {
                f.derived_response_body_ref
                    .as_ref()
                    .or(f.response_body_ref.as_ref())
            }) {
                if let Some(m) = self.search_body(body_ref, keyword, "response_body") {
                    return Some(SearchResultItem {
                        record: compact.clone(),
                        matches: vec![m],
                    });
                }
            }
        }

        // WS/SSE frame 搜索最贵，且只在对应记录上启用。
        let is_websocket = (compact.flags & crate::traffic_db::TrafficFlags::IS_WEBSOCKET) != 0;
        let is_sse = (compact.flags & crate::traffic_db::TrafficFlags::IS_SSE) != 0;

        if is_websocket && scope.should_search_websocket_messages() {
            if let Some(frame_matches) =
                self.search_frames(&compact.id, keyword, "websocket_message")
            {
                if let Some(first) = frame_matches.into_iter().next() {
                    return Some(SearchResultItem {
                        record: compact.clone(),
                        matches: vec![first],
                    });
                }
            }
        }

        if is_sse && scope.should_search_sse_events() {
            if let Some(frame_matches) = self.search_frames(&compact.id, keyword, "sse_event") {
                if let Some(first) = frame_matches.into_iter().next() {
                    return Some(SearchResultItem {
                        record: compact.clone(),
                        matches: vec![first],
                    });
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
            ..Default::default()
        };

        let filters = &request.filters;

        if let Some(rule_hit) = filters.has_rule_hit {
            params.has_rule_hit = Some(rule_hit);
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
                "content_type" => {
                    params.content_type = Some(condition.value.clone());
                }
                _ => {}
            }
        }

        params
    }

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
        let text_lower = text.to_lowercase();
        if let Some(pos) = text_lower.find(keyword) {
            let start = find_char_boundary(text, pos.saturating_sub(MAX_PREVIEW_CONTEXT), false);
            let end = find_char_boundary(
                text,
                (pos + keyword.len() + MAX_PREVIEW_CONTEXT).min(text.len()),
                true,
            );

            let preview = if start > 0 || end < text.len() {
                let prefix = if start > 0 { "..." } else { "" };
                let suffix = if end < text.len() { "..." } else { "" };
                format!("{}{}{}", prefix, &text[start..end], suffix)
            } else {
                text[start..end].to_string()
            };

            Some(MatchLocation {
                field: field.to_string(),
                preview,
                offset: pos,
            })
        } else {
            None
        }
    }

    fn search_body(&self, body_ref: &BodyRef, keyword: &str, field: &str) -> Option<MatchLocation> {
        match body_ref {
            BodyRef::Inline { data } => self.search_text(data, keyword, field),
            BodyRef::File { .. } | BodyRef::FileRange { .. } => {
                if let Some(ref body_store) = self.body_store {
                    let store = body_store.read();
                    if let Some(content) = store.load(body_ref) {
                        return self.search_text(&content, keyword, field);
                    }
                }
                None
            }
        }
    }

    fn search_frames(
        &self,
        connection_id: &str,
        keyword: &str,
        field: &str,
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
                        if let Some(m) = self.search_body(body_ref, keyword, field) {
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
                            if let Some(m) = self.search_body(body_ref, keyword, field) {
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

        if !filters.domains.is_empty() {
            let host = &compact.h;
            if !filters.domains.iter().any(|d| host.contains(d)) {
                return false;
            }
        }

        true
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use tempfile::TempDir;

    use super::SearchEngine;
    use crate::body_store::BodyRef;
    use crate::search::{SearchFilters, SearchRequest, SearchScope};
    use crate::traffic::TrafficRecord;
    use crate::traffic_db::TrafficDbStore;

    #[test]
    fn response_body_search_prefers_derived_sse_body() {
        let dir = TempDir::new().expect("temp dir");
        let db = Arc::new(
            TrafficDbStore::new(dir.path().join("traffic"), 1024, 64 * 1024 * 1024, Some(24))
                .expect("traffic db"),
        );

        let mut record = TrafficRecord::new(
            "REQ-search-derived".to_string(),
            "GET".to_string(),
            "https://example.com/v1/chat/completions".to_string(),
        );
        record.set_sse();
        record.response_body_ref = Some(BodyRef::Inline {
            data: concat!(
                "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"hello \"}}]}\n\n",
                "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"world\"}}]}\n\n",
                "data: [DONE]\n\n"
            )
            .to_string(),
        });
        record.derived_response_body_ref = Some(BodyRef::Inline {
            data: serde_json::json!({
                "object": "chat.completion",
                "choices": [{
                    "index": 0,
                    "message": {
                        "role": "assistant",
                        "content": "hello world"
                    },
                    "finish_reason": "stop"
                }]
            })
            .to_string(),
        });
        db.record(record);

        let engine = SearchEngine::new(db, None);
        let response = engine.search(&SearchRequest {
            keyword: "hello world".to_string(),
            scope: SearchScope {
                all: false,
                response_body: true,
                ..Default::default()
            },
            filters: SearchFilters::default(),
            cursor: None,
            limit: Some(20),
            max_scan: None,
            max_results: None,
        });

        assert_eq!(response.total_matched, 1);
        assert_eq!(response.results[0].matches[0].field, "response_body");
        assert!(response.results[0].matches[0]
            .preview
            .contains("hello world"));
    }

    #[test]
    fn response_body_search_finds_decoded_bp_body() {
        let dir = TempDir::new().expect("temp dir");
        let db = Arc::new(
            TrafficDbStore::new(dir.path().join("traffic"), 1024, 64 * 1024 * 1024, Some(24))
                .expect("traffic db"),
        );

        let mut record = TrafficRecord::new(
            "REQ-search-bp".to_string(),
            "POST".to_string(),
            "https://example.com/bp".to_string(),
        );
        record.raw_response_body_ref = Some(BodyRef::Inline {
            data: "\u{0000}\u{0001}binary".to_string(),
        });
        record.response_body_ref = Some(BodyRef::Inline {
            data: serde_json::json!({
                "decoded": true,
                "marker": "bp-search-unique-needle"
            })
            .to_string(),
        });
        db.record(record);

        let engine = SearchEngine::new(db, None);
        let response = engine.search(&SearchRequest {
            keyword: "bp-search-unique-needle".to_string(),
            scope: SearchScope {
                all: false,
                response_body: true,
                ..Default::default()
            },
            filters: SearchFilters::default(),
            cursor: None,
            limit: Some(20),
            max_scan: None,
            max_results: None,
        });

        assert_eq!(response.total_matched, 1);
        assert_eq!(response.results[0].record.id, "REQ-search-bp");
        assert_eq!(response.results[0].matches[0].field, "response_body");
    }
}
