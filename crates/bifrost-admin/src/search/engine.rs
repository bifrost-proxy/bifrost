use regex::Regex;
use tracing::debug;

use super::types::{
    FilterCondition, MatchLocation, SearchFilters, SearchRequest, SearchResponse, SearchResultItem,
    SearchScope,
};
use crate::body_store::{BodyRef, SharedBodyStore};
use crate::connection_monitor::SharedConnectionMonitor;
use crate::frame_store::SharedFrameStore;
use crate::traffic_db::{
    BodyIndexRow, QueryParams, SharedTrafficDbStore, TrafficSearchFields, TrafficSummaryCompact,
};

const MAX_PREVIEW_CONTEXT: usize = 50;
const DEFAULT_BATCH_SIZE: usize = 50;
const SEARCH_BATCH_SIZE: usize = 200;
const MAX_SEARCH_ITERATIONS: usize = 50;
const MAX_TOTAL_SEARCHED: usize = 10000;
const BODY_INDEX_BLOCK_SIZE: usize = 64 * 1024;
const BODY_INDEX_BITSET_BITS: usize = 32 * 1024;
const BODY_INDEX_BITSET_BYTES: usize = BODY_INDEX_BITSET_BITS / 8;

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
        self.search_stream(request, |_| {}, |_| {})
    }

    pub fn search_stream<F, P>(
        &self,
        request: &SearchRequest,
        mut on_result: F,
        mut on_progress: P,
    ) -> SearchResponse
    where
        F: FnMut(&SearchResultItem),
        P: FnMut(&SearchProgress),
    {
        let search_id = generate_search_id();
        let batch_size = request.limit.unwrap_or(DEFAULT_BATCH_SIZE);
        let keyword_lower = request.keyword.to_lowercase();
        let keyword_bytes_len = keyword_lower.len();

        // Only enable body index when case-folding won't cause false negatives for non-ASCII.
        // - ASCII: safe
        // - Non-ASCII: only safe when lowercase transform is identity (e.g. Chinese)
        let can_use_body_index =
            request.keyword.is_ascii() || request.keyword.to_lowercase() == request.keyword;

        // search scope / filters 计算一次，避免循环里反复判断
        let scope = &request.scope;
        let need_url = scope.should_search_url()
            || request
                .filters
                .conditions
                .iter()
                .any(|c| c.field.as_str() == "url");
        let need_request_headers = scope.should_search_request_headers();
        let need_response_headers = scope.should_search_response_headers();
        let need_request_body_ref = scope.should_search_request_body();
        // SSE events 在 proxy 侧以 `sse_raw` response body 的形式落盘，并不作为 frames 持久化。
        // 因此 sse_events 搜索需要拿到 response_body_ref。
        let need_response_body_ref =
            scope.should_search_response_body() || scope.should_search_sse_events();

        debug!(
            keyword = %request.keyword,
            scope = ?request.scope,
            cursor = ?request.cursor,
            limit = batch_size,
            "[SEARCH] Starting iterative search"
        );

        let mut results = Vec::new();
        let mut total_searched = 0;
        let mut current_cursor = request.cursor;
        let mut iterations = 0;
        let mut db_has_more = true;

        while results.len() < batch_size
            && iterations < MAX_SEARCH_ITERATIONS
            && total_searched < MAX_TOTAL_SEARCHED
            && db_has_more
        {
            iterations += 1;

            let query_params = self.build_query_params_with_cursor(request, current_cursor);
            // 搜索会迭代多次，避免每次 query 都做 COUNT(*)
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

            // 先收集候选 id，批量拉取搜索字段，避免 N+1。
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

            let req_body_index_map = if self.traffic_db.is_body_index_enabled()
                && can_use_body_index
                && need_request_body_ref
                && keyword_bytes_len >= 3
            {
                self.traffic_db.get_body_indexes_by_ids(&candidate_ids, 0)
            } else {
                std::collections::HashMap::new()
            };

            let res_body_index_map = if self.traffic_db.is_body_index_enabled()
                && can_use_body_index
                && need_response_body_ref
                && keyword_bytes_len >= 3
            {
                self.traffic_db.get_body_indexes_by_ids(&candidate_ids, 1)
            } else {
                std::collections::HashMap::new()
            };

            for compact in &query_result.records {
                total_searched += 1;
                current_cursor = Some(compact.seq);

                if !self.matches_filter_compact(compact, &request.filters) {
                    if total_searched >= MAX_TOTAL_SEARCHED {
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
                    if total_searched >= MAX_TOTAL_SEARCHED {
                        break;
                    }
                    continue;
                }

                if let Some(result) = self.search_compact(
                    scope,
                    &keyword_lower,
                    compact,
                    fields,
                    req_body_index_map.get(&compact.id),
                    res_body_index_map.get(&compact.id),
                ) {
                    results.push(result);
                    if let Some(last) = results.last() {
                        on_result(last);
                    }
                    if results.len() >= batch_size {
                        break;
                    }
                }

                if total_searched >= MAX_TOTAL_SEARCHED {
                    break;
                }
            }

            db_has_more = query_result.has_more;
            on_progress(&SearchProgress {
                iterations,
                total_searched,
                total_matched: results.len(),
                cursor: current_cursor,
                has_more_hint: db_has_more && total_searched < MAX_TOTAL_SEARCHED,
            });
        }

        let has_more = db_has_more && total_searched < MAX_TOTAL_SEARCHED;
        let total_matched = results.len();

        debug!(
            iterations = iterations,
            total_searched = total_searched,
            matched = total_matched,
            has_more = has_more,
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
        request_body_index: Option<&BodyIndexRow>,
        response_body_index: Option<&BodyIndexRow>,
    ) -> Option<SearchResultItem> {
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
                if let Some(m) = self.search_body_indexed(
                    &compact.id,
                    0,
                    body_ref,
                    keyword,
                    "request_body",
                    request_body_index,
                ) {
                    return Some(SearchResultItem {
                        record: compact.clone(),
                        matches: vec![m],
                    });
                }
            }
        }

        if scope.should_search_response_body() {
            if let Some(body_ref) = fields.and_then(|f| f.response_body_ref.as_ref()) {
                if let Some(m) = self.search_body_indexed(
                    &compact.id,
                    1,
                    body_ref,
                    keyword,
                    "response_body",
                    response_body_index,
                ) {
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
            // SSE messages 默认以 raw body (sse_raw) 形式存储在 response_body_ref。
            // 这里直接在 body 上做搜索，字段名仍用 sse_event，保持前端/UI 的一致性。
            if let Some(body_ref) = fields.and_then(|f| f.response_body_ref.as_ref()) {
                if let Some(m) = self.search_body_indexed(
                    &compact.id,
                    1,
                    body_ref,
                    keyword,
                    "sse_event",
                    response_body_index,
                ) {
                    return Some(SearchResultItem {
                        record: compact.clone(),
                        matches: vec![m],
                    });
                }
            } else if let Some(frame_matches) =
                self.search_frames(&compact.id, keyword, "sse_event")
            {
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
                "host" => {
                    if condition.operator == "contains" || condition.operator == "equals" {
                        params.host_contains = Some(condition.value.clone());
                    }
                }
                "path" => {
                    if condition.operator == "contains" || condition.operator == "equals" {
                        params.path_contains = Some(condition.value.clone());
                    }
                }
                "url" => {
                    if condition.operator == "contains" || condition.operator == "equals" {
                        params.url_contains = Some(condition.value.clone());
                    }
                }
                "method" => {
                    if condition.operator == "equals" {
                        params.method = Some(condition.value.clone());
                    }
                }
                "client_app" => {
                    params.client_app = Some(condition.value.clone());
                }
                "client_ip" => {
                    params.client_ip = Some(condition.value.clone());
                }
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

    fn search_body_indexed(
        &self,
        traffic_id: &str,
        kind: i32,
        body_ref: &BodyRef,
        keyword_lower: &str,
        field: &str,
        body_index: Option<&BodyIndexRow>,
    ) -> Option<MatchLocation> {
        match body_ref {
            BodyRef::Inline { data } => self.search_text(data, keyword_lower, field),
            BodyRef::File { .. } | BodyRef::FileRange { .. } => {
                let Some(idx) = body_index else {
                    // No index available, fallback to existing load path.
                    return self.search_body(body_ref, keyword_lower, field);
                };

                if idx.kind != kind || idx.id != traffic_id {
                    return self.search_body(body_ref, keyword_lower, field);
                }

                let kw_bytes = keyword_lower.as_bytes();
                if kw_bytes.len() < 3 {
                    // k=2 not supported by design
                    return self.search_body(body_ref, keyword_lower, field);
                }

                if idx.block_count == 0 {
                    // Index row is unusable; never allow false negatives.
                    return self.search_body(body_ref, keyword_lower, field);
                }
                if idx.bitsets.len() != idx.block_count.saturating_mul(BODY_INDEX_BITSET_BYTES) {
                    return self.search_body(body_ref, keyword_lower, field);
                }

                let gram_idxs = build_keyword_trigram_indexes(kw_bytes);
                if gram_idxs.is_empty() {
                    return self.search_body(body_ref, keyword_lower, field);
                }

                let window_len = compute_window_blocks(kw_bytes.len());

                for start_block in 0..idx.block_count {
                    let end_block = (start_block + window_len).min(idx.block_count);
                    if !window_may_match(&idx.bitsets, start_block, end_block, &gram_idxs) {
                        continue;
                    }

                    let rel_start = start_block * BODY_INDEX_BLOCK_SIZE;
                    if rel_start >= idx.size {
                        break;
                    }
                    let rel_available = idx.size - rel_start;
                    let need = (end_block - start_block) * BODY_INDEX_BLOCK_SIZE + kw_bytes.len();
                    let read_len = rel_available.min(need);

                    let Some(bytes) =
                        read_file_range(&idx.path, idx.offset + rel_start as u64, read_len)
                    else {
                        // IO failure: fall back to the existing body loading path.
                        return self.search_body(body_ref, keyword_lower, field);
                    };
                    let text = String::from_utf8_lossy(&bytes).to_string();
                    if let Some(m) =
                        search_text_with_base_offset(&text, keyword_lower, field, rel_start)
                    {
                        return Some(m);
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
                    "HTTP" => {
                        if protocol_upper == "HTTP"
                            || protocol_upper == "HTTP/1.0"
                            || protocol_upper == "HTTP/1.1"
                        {
                            matched = true;
                            break;
                        }
                    }
                    "HTTPS" => {
                        if protocol_upper == "HTTPS" || protocol_upper == "HTTP/2" {
                            matched = true;
                            break;
                        }
                    }
                    "H2" => {
                        if protocol_upper.contains("HTTP/2") {
                            matched = true;
                            break;
                        }
                    }
                    "WS" => {
                        if is_websocket && protocol_upper == "WS" {
                            matched = true;
                            break;
                        }
                    }
                    "WSS" => {
                        if is_websocket && protocol_upper == "WSS" {
                            matched = true;
                            break;
                        }
                    }
                    "H3" => {
                        if is_h3 || protocol_upper == "H3" {
                            matched = true;
                            break;
                        }
                    }
                    "SSE" => {
                        if is_sse {
                            matched = true;
                            break;
                        }
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
                    "error" => {
                        if status == 0 || status >= 500 {
                            matched = true;
                            break;
                        }
                    }
                    "1xx" => {
                        if (100..200).contains(&status) {
                            matched = true;
                            break;
                        }
                    }
                    "2xx" => {
                        if (200..300).contains(&status) {
                            matched = true;
                            break;
                        }
                    }
                    "3xx" => {
                        if (300..400).contains(&status) {
                            matched = true;
                            break;
                        }
                    }
                    "4xx" => {
                        if (400..500).contains(&status) {
                            matched = true;
                            break;
                        }
                    }
                    "5xx" => {
                        if (500..600).contains(&status) {
                            matched = true;
                            break;
                        }
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

#[inline]
fn compute_window_blocks(keyword_len_bytes: usize) -> usize {
    // keyword may span multiple blocks; choose a conservative window size.
    // For most cases (keyword <= 64KB), window_len = 2.
    let blocks = keyword_len_bytes.div_ceil(BODY_INDEX_BLOCK_SIZE);
    (blocks + 1).max(2)
}

fn build_keyword_trigram_indexes(keyword_bytes: &[u8]) -> Vec<usize> {
    if keyword_bytes.len() < 3 {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(keyword_bytes.len().saturating_sub(2));
    let mask = (BODY_INDEX_BITSET_BITS - 1) as u32;
    for i in 0..(keyword_bytes.len() - 2) {
        let b0 = fold_ascii_lower(keyword_bytes[i]);
        let b1 = fold_ascii_lower(keyword_bytes[i + 1]);
        let b2 = fold_ascii_lower(keyword_bytes[i + 2]);
        let idx = (hash_trigram_u32(b0, b1, b2) & mask) as usize;
        out.push(idx);
    }
    out
}

#[inline]
fn window_may_match(
    bitsets: &[u8],
    start_block: usize,
    end_block: usize,
    trigram_idxs: &[usize],
) -> bool {
    // Important: we index trigrams *within* each block only. Trigrams that cross block boundaries
    // will not be present in any single block's bitset.
    //
    // To avoid false negatives, allow up to 2 missing trigrams per boundary in the window.
    let window_blocks = end_block.saturating_sub(start_block).max(1);
    let max_missing = window_blocks.saturating_sub(1) * 2;
    let mut missing = 0usize;

    for &idx in trigram_idxs {
        let byte = idx >> 3;
        let bit = 1u8 << (idx & 7);
        let mut ok = false;
        for b in start_block..end_block {
            let base = b * BODY_INDEX_BITSET_BYTES;
            if (bitsets[base + byte] & bit) != 0 {
                ok = true;
                break;
            }
        }
        if !ok {
            missing += 1;
            if missing > max_missing {
                return false;
            }
        }
    }
    true
}

fn read_file_range(path: &str, start: u64, len: usize) -> Option<Vec<u8>> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(path).ok()?;
    f.seek(SeekFrom::Start(start)).ok()?;
    let mut buf = vec![0u8; len];
    let n = f.read(&mut buf).ok()?;
    if n == 0 {
        return None;
    }
    buf.truncate(n);
    Some(buf)
}

fn search_text_with_base_offset(
    text: &str,
    keyword_lower: &str,
    field: &str,
    base_offset: usize,
) -> Option<MatchLocation> {
    let text_lower = text.to_lowercase();
    if let Some(pos) = text_lower.find(keyword_lower) {
        let start = find_char_boundary(text, pos.saturating_sub(MAX_PREVIEW_CONTEXT), false);
        let end = find_char_boundary(
            text,
            (pos + keyword_lower.len() + MAX_PREVIEW_CONTEXT).min(text.len()),
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
            offset: base_offset.saturating_add(pos),
        })
    } else {
        None
    }
}

#[inline]
fn fold_ascii_lower(b: u8) -> u8 {
    if b.is_ascii_uppercase() {
        b + 32
    } else {
        b
    }
}

#[inline]
fn hash_trigram_u32(b0: u8, b1: u8, b2: u8) -> u32 {
    let mut x = (b0 as u32) | ((b1 as u32) << 8) | ((b2 as u32) << 16);
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb_352d);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846c_a68b);
    x ^ (x >> 16)
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
    use super::*;
    use crate::traffic_db::TrafficDbStore;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::Arc;

    fn create_test_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "bifrost_search_engine_test_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = fs::create_dir_all(&dir);
        dir
    }

    #[inline]
    fn index_block_bytes_v1(block: &[u8], bitset: &mut [u8]) {
        if block.len() < 3 {
            return;
        }
        let mask = (BODY_INDEX_BITSET_BITS - 1) as u32;
        for i in 0..(block.len() - 2) {
            let b0 = fold_ascii_lower(block[i]);
            let b1 = fold_ascii_lower(block[i + 1]);
            let b2 = fold_ascii_lower(block[i + 2]);
            let idx = (hash_trigram_u32(b0, b1, b2) & mask) as usize;
            let byte = idx >> 3;
            let bit = 1u8 << (idx & 7);
            bitset[byte] |= bit;
        }
    }

    #[test]
    fn test_window_may_match_allows_boundary_missing_trigrams() {
        // Build a body where the keyword crosses the block boundary.
        // Without boundary-missing allowance, this would be a false negative.
        let dir = create_test_dir();
        let traffic_db = Arc::new(TrafficDbStore::new(dir.clone(), 10, 0, None).unwrap());
        let engine = SearchEngine::new(traffic_db, None);

        let keyword = "abcde";
        let insert_at = BODY_INDEX_BLOCK_SIZE - 2;

        let mut body = vec![b'x'; BODY_INDEX_BLOCK_SIZE * 2];
        body[insert_at..insert_at + keyword.len()].copy_from_slice(keyword.as_bytes());

        let body_path = dir.join("body.bin");
        fs::write(&body_path, &body).unwrap();

        let mut bitsets = vec![0u8; BODY_INDEX_BITSET_BYTES * 2];
        index_block_bytes_v1(
            &body[..BODY_INDEX_BLOCK_SIZE],
            &mut bitsets[..BODY_INDEX_BITSET_BYTES],
        );
        index_block_bytes_v1(
            &body[BODY_INDEX_BLOCK_SIZE..],
            &mut bitsets[BODY_INDEX_BITSET_BYTES..],
        );

        let idx = BodyIndexRow {
            id: "t1".to_string(),
            kind: 0,
            path: body_path.display().to_string(),
            offset: 0,
            size: body.len(),
            block_count: 2,
            bitsets,
        };

        let body_ref = BodyRef::File {
            path: idx.path.clone(),
            size: idx.size,
        };

        let m = engine.search_body_indexed("t1", 0, &body_ref, keyword, "request_body", Some(&idx));

        assert!(m.is_some());
        assert!(m.unwrap().offset >= insert_at);
    }

    #[test]
    fn test_window_may_match_rejects_when_too_many_missing_trigrams() {
        // Empty bitsets should reject keywords with >2 trigrams in a 2-block window.
        // keyword len=6 => 4 trigrams, max_missing=2 (one boundary).
        let trigram_idxs = build_keyword_trigram_indexes(b"abcdef");
        let bitsets = vec![0u8; BODY_INDEX_BITSET_BYTES * 2];
        assert!(!window_may_match(&bitsets, 0, 2, &trigram_idxs));
    }
}
