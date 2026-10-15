use super::*;

impl TrafficDbStore {
    pub fn query(&self, params: &QueryParams) -> QueryResult {
        #[cfg(test)]
        self.query_calls.fetch_add(1, Ordering::Relaxed);
        self.query_internal(params, QueryTotalMode::Estimated)
    }

    pub fn query_with_exact_total(&self, params: &QueryParams) -> QueryResult {
        #[cfg(test)]
        self.query_calls.fetch_add(1, Ordering::Relaxed);
        self.query_internal(params, QueryTotalMode::Exact)
    }

    pub fn query_latest_window(&self, limit: usize) -> QueryResult {
        let params = QueryParams {
            limit: Some(limit),
            direction: Direction::Backward,
            ..Default::default()
        };
        let mut result = self.query(&params);
        Self::normalize_latest_window_result(&mut result);
        result
    }

    /// 用于搜索等高频迭代场景的查询：不会计算 total（COUNT(*)），避免重复全表扫描。
    pub fn query_for_search(&self, params: &QueryParams) -> QueryResult {
        self.query_internal(params, QueryTotalMode::None)
    }

    pub(super) fn query_internal(
        &self,
        params: &QueryParams,
        total_mode: QueryTotalMode,
    ) -> QueryResult {
        let conn = self.read_pool.acquire();
        let limit = params.limit.unwrap_or(100);
        let mut fetch_params = params.clone();
        fetch_params.limit = Some(limit.saturating_add(1));
        let (sql, values) = fetch_params.build_select_sql();
        let param_refs: Vec<&dyn rusqlite::ToSql> =
            values.iter().map(|v| v as &dyn rusqlite::ToSql).collect();

        let mut stmt = match conn.prepare(&sql) {
            Ok(s) => s,
            Err(e) => {
                tracing::error!(error = %e, "[TRAFFIC_DB] Failed to prepare query");
                return QueryResult {
                    records: vec![],
                    next_cursor: None,
                    prev_cursor: None,
                    has_more: false,
                    total: self.record_count.load(Ordering::Relaxed),
                    server_sequence: self.current_sequence.load(Ordering::Relaxed),
                };
            }
        };

        let mut records: Vec<TrafficSummaryCompact> = stmt
            .query_map(param_refs.as_slice(), |row| {
                let socket_status = build_socket_status_summary(
                    row.get::<_, bool>(21)?,
                    row.get::<_, i64>(22)? as u64,
                    row.get::<_, i64>(23)? as u64,
                    row.get::<_, i64>(24)? as u64,
                    row.get::<_, i64>(25)? as u64,
                    row.get::<_, i64>(26)? as usize,
                );
                let rc = row.get::<_, i64>(27)? as usize;
                let rp = row
                    .get::<_, Option<String>>(28)?
                    .and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok())
                    .unwrap_or_default();

                Ok(TrafficSummaryCompact {
                    seq: row.get::<_, i64>(0)? as u64,
                    id: row.get(1)?,
                    ts: row.get::<_, i64>(2)? as u64,
                    h: row.get(3)?,
                    m: row.get(4)?,
                    s: row.get::<_, i32>(5)? as u16,
                    proto: row.get(6)?,
                    p: row.get(8)?,
                    ct: row.get(9)?,
                    req_ct: row.get(29)?,
                    req_sz: row.get::<_, i64>(10)? as usize,
                    res_sz: row.get::<_, i64>(11)? as usize,
                    up: row.get::<_, i64>(12)? as usize,
                    down: row.get::<_, i64>(13)? as usize,
                    dur: row.get::<_, i64>(14)? as u64,
                    lp: row.get::<_, i64>(15)? as u16,
                    cip: row.get(16)?,
                    capp: row.get(17)?,
                    cpid: row.get::<_, Option<i32>>(18)?.map(|v| v as u32),
                    acct: row.get(30)?,
                    flags: row.get::<_, i32>(19)? as u32,
                    fc: row.get::<_, i64>(20)? as usize,
                    ss: socket_status,
                    st: format_timestamp_ms(row.get::<_, i64>(2)? as u64),
                    et: {
                        let ts = row.get::<_, i64>(2)? as u64;
                        let dur = row.get::<_, i64>(14)? as u64;
                        if dur > 0 {
                            Some(format_timestamp_ms(ts + dur))
                        } else {
                            None
                        }
                    },
                    rc,
                    rp,
                })
            })
            .map(|r| r.filter_map(|r| r.ok()).collect())
            .unwrap_or_default();

        let has_more = records.len() > limit;
        records.truncate(limit);

        let (next_cursor, prev_cursor) = if records.is_empty() {
            (None, None)
        } else {
            match params.direction {
                Direction::Forward => (
                    records.last().map(|r| r.seq),
                    records.first().map(|r| r.seq),
                ),
                Direction::Backward => (
                    records.last().map(|r| r.seq),
                    records.first().map(|r| r.seq),
                ),
            }
        };

        let total = match total_mode {
            QueryTotalMode::None => 0,
            QueryTotalMode::Estimated => self.record_count.load(Ordering::Relaxed),
            QueryTotalMode::Exact => {
                if params.has_filters() {
                    self.count_with_conn(&conn, params)
                } else {
                    self.record_count.load(Ordering::Relaxed)
                }
            }
        };

        QueryResult {
            records,
            next_cursor,
            prev_cursor,
            has_more,
            total,
            server_sequence: self.current_sequence.load(Ordering::Relaxed),
        }
    }

    pub(super) fn normalize_latest_window_result(result: &mut QueryResult) {
        if result.records.is_empty() {
            result.next_cursor = None;
            result.prev_cursor = None;
            return;
        }

        result.records.reverse();
        result.prev_cursor = result.records.first().map(|r| r.seq);
        result.next_cursor = result.records.last().map(|r| r.seq);
    }

    /// 批量拉取搜索所需的轻量字段，避免 search 中的 N+1 `get_by_id`。
    pub fn get_search_fields_by_ids(
        &self,
        ids: &[&str],
        need_url: bool,
        need_request_headers: bool,
        need_response_headers: bool,
        need_request_body_ref: bool,
        need_response_body_ref: bool,
    ) -> std::collections::HashMap<String, TrafficSearchFields> {
        use std::collections::HashMap;

        if ids.is_empty() {
            return HashMap::new();
        }

        // 至少要取 id。
        let mut columns: Vec<&str> = vec!["tr.id"];
        if need_url {
            columns.push("tr.url");
        }
        if need_request_headers {
            columns.push("td.request_headers_blob");
        }
        if need_response_headers {
            columns.push("td.original_response_headers_blob");
        }
        if need_request_body_ref {
            columns.push("td.request_body_ref_blob");
        }
        if need_response_body_ref {
            columns.push("td.response_body_ref_blob");
            columns.push("td.derived_response_body_ref_blob");
        }
        if need_request_body_ref || need_response_body_ref {
            columns.push("td.body_metadata_blob");
        }

        // 全部不需要也就不查。
        if columns.len() == 1 {
            return ids
                .iter()
                .map(|id| {
                    (
                        (*id).to_string(),
                        TrafficSearchFields {
                            id: (*id).to_string(),
                            url: None,
                            request_headers: None,
                            response_headers: None,
                            request_body_ref: None,
                            response_body_ref: None,
                            derived_response_body_ref: None,
                            request_body_content_encoding: None,
                            response_body_content_encoding: None,
                        },
                    )
                })
                .collect();
        }

        let placeholders: Vec<&str> = ids.iter().map(|_| "?").collect();
        let sql = format!(
            "SELECT {} FROM traffic_records tr \
             LEFT JOIN traffic_record_details td ON td.id = tr.id \
             WHERE tr.id IN ({})",
            columns.join(","),
            placeholders.join(",")
        );

        let conn = self.read_pool.acquire();
        let mut stmt = match conn.prepare(&sql) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(error = %e, "[TRAFFIC_DB] Failed to prepare get_search_fields_by_ids");
                return HashMap::new();
            }
        };

        let params: Vec<&dyn rusqlite::ToSql> =
            ids.iter().map(|id| id as &dyn rusqlite::ToSql).collect();

        let mut out: HashMap<String, TrafficSearchFields> = HashMap::new();
        let iter = match stmt.query_map(params.as_slice(), |row| {
            let mut idx = 0usize;
            let id: String = row.get(idx)?;
            idx += 1;

            let url: Option<String> = if need_url {
                let v: String = row.get(idx)?;
                idx += 1;
                Some(v)
            } else {
                None
            };

            let request_headers: Option<Vec<(String, String)>> = if need_request_headers {
                let blob: Option<Vec<u8>> = row.get(idx)?;
                idx += 1;
                blob.and_then(|b| decode_detail_blob(&b).ok())
            } else {
                None
            };

            let response_headers: Option<Vec<(String, String)>> = if need_response_headers {
                let blob: Option<Vec<u8>> = row.get(idx)?;
                idx += 1;
                blob.and_then(|b| decode_detail_blob(&b).ok())
            } else {
                None
            };

            let request_body_ref: Option<BodyRef> = if need_request_body_ref {
                let blob: Option<Vec<u8>> = row.get(idx)?;
                idx += 1;
                blob.and_then(|b| decode_detail_blob(&b).ok())
            } else {
                None
            };

            let response_body_ref: Option<BodyRef> = if need_response_body_ref {
                let blob: Option<Vec<u8>> = row.get(idx)?;
                idx += 1;
                blob.and_then(|b| decode_detail_blob(&b).ok())
            } else {
                None
            };

            let derived_response_body_ref: Option<BodyRef> = if need_response_body_ref {
                let blob: Option<Vec<u8>> = row.get(idx)?;
                idx += 1;
                blob.and_then(|b| decode_detail_blob(&b).ok())
            } else {
                None
            };

            let body_metadata: Option<crate::TrafficBodyMetadata> =
                if need_request_body_ref || need_response_body_ref {
                    let blob: Option<Vec<u8>> = row.get(idx)?;
                    blob.and_then(|b| decode_detail_blob(&b).ok())
                } else {
                    None
                };
            let request_body_content_encoding = body_metadata
                .as_ref()
                .filter(|metadata| metadata.version == crate::TRAFFIC_BODY_METADATA_VERSION)
                .and_then(|metadata| metadata.request.as_ref())
                .and_then(|metadata| metadata.content_encoding.clone());
            let response_body_content_encoding = body_metadata
                .as_ref()
                .filter(|metadata| metadata.version == crate::TRAFFIC_BODY_METADATA_VERSION)
                .and_then(|metadata| metadata.response.as_ref())
                .and_then(|metadata| metadata.content_encoding.clone());

            Ok(TrafficSearchFields {
                id: id.clone(),
                url,
                request_headers,
                response_headers,
                request_body_ref,
                response_body_ref,
                derived_response_body_ref,
                request_body_content_encoding,
                response_body_content_encoding,
            })
        }) {
            Ok(i) => i,
            Err(e) => {
                tracing::warn!(error = %e, "[TRAFFIC_DB] get_search_fields_by_ids query failed");
                return HashMap::new();
            }
        };

        for row in iter.flatten() {
            out.insert(row.id.clone(), row);
        }

        out
    }

    pub(super) fn count_with_conn(&self, conn: &Connection, params: &QueryParams) -> usize {
        let (sql, values) = params.build_count_sql();
        let param_refs: Vec<&dyn rusqlite::ToSql> =
            values.iter().map(|v| v as &dyn rusqlite::ToSql).collect();

        conn.query_row(&sql, param_refs.as_slice(), |row| row.get::<_, i64>(0))
            .map(|v| v as usize)
            .unwrap_or(0)
    }

    pub fn get_by_id(&self, id: &str) -> Option<TrafficRecord> {
        // 详情强制走 DB（避免把完整 record 常驻内存）
        self.get_by_id_from_db(id)
    }

    pub fn get_id_by_devtools_client_req_id(&self, client_req_id: &str) -> Option<String> {
        let client_req_id = client_req_id.trim();
        if client_req_id.is_empty() {
            return None;
        }

        let conn = self.read_pool.acquire();
        conn.query_row(
            "SELECT id FROM traffic_records \
             WHERE devtools_client_req_id = ?1 AND (flags & 32) = 0 \
             ORDER BY sequence ASC LIMIT 1",
            [client_req_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .ok()
        .flatten()
    }

    pub(super) fn get_by_id_from_db(&self, id: &str) -> Option<TrafficRecord> {
        let conn = self.read_pool.acquire();
        let mut record = conn
            .query_row(
                "SELECT sequence, id, timestamp, host, method, status, protocol, url, path, \
                 content_type, request_content_type, request_size, response_size, upload_bytes, download_bytes, duration_ms, \
                 listener_port, client_ip, client_app, client_pid, client_path, account_name, flags, frame_count, \
                 last_frame_id, socket_is_open, socket_send_count, socket_receive_count, \
                 socket_send_bytes, socket_receive_bytes, socket_frame_count, \
                 devtools_client_req_id \
                 FROM traffic_records WHERE id = ?",
                [id],
                Self::row_to_record_core,
            )
            .optional()
            .ok()
            .flatten()?;

        Self::load_record_details(&conn, id, &mut record);
        Some(record)
    }

    pub fn get_by_ids(&self, ids: &[&str]) -> Vec<TrafficSummaryCompact> {
        #[cfg(test)]
        self.get_by_ids_calls.fetch_add(1, Ordering::Relaxed);
        if ids.is_empty() {
            return vec![];
        }

        let placeholders: Vec<&str> = ids.iter().map(|_| "?").collect();
        let sql = format!(
            "SELECT sequence, id, timestamp, host, method, status, protocol, \
             url, path, content_type, request_size, response_size, upload_bytes, download_bytes, duration_ms, \
             listener_port, client_ip, client_app, client_pid, flags, frame_count, \
             socket_is_open, socket_send_count, socket_receive_count, \
             socket_send_bytes, socket_receive_bytes, socket_frame_count, \
             rule_count, rule_protocols, request_content_type, account_name \
             FROM traffic_records WHERE id IN ({}) ORDER BY sequence DESC",
            placeholders.join(",")
        );

        let conn = self.read_pool.acquire();
        let mut stmt = match conn.prepare(&sql) {
            Ok(s) => s,
            Err(_) => return vec![],
        };

        let params: Vec<&dyn rusqlite::ToSql> =
            ids.iter().map(|id| id as &dyn rusqlite::ToSql).collect();

        stmt.query_map(params.as_slice(), |row| {
            let socket_status = build_socket_status_summary(
                row.get::<_, bool>(21)?,
                row.get::<_, i64>(22)? as u64,
                row.get::<_, i64>(23)? as u64,
                row.get::<_, i64>(24)? as u64,
                row.get::<_, i64>(25)? as u64,
                row.get::<_, i64>(26)? as usize,
            );
            let rc = row.get::<_, i64>(27)? as usize;
            let rp = row
                .get::<_, Option<String>>(28)?
                .and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok())
                .unwrap_or_default();

            Ok(TrafficSummaryCompact {
                seq: row.get::<_, i64>(0)? as u64,
                id: row.get(1)?,
                ts: row.get::<_, i64>(2)? as u64,
                h: row.get(3)?,
                m: row.get(4)?,
                s: row.get::<_, i32>(5)? as u16,
                proto: row.get(6)?,
                p: row.get(8)?,
                ct: row.get(9)?,
                req_ct: row.get(29)?,
                req_sz: row.get::<_, i64>(10)? as usize,
                res_sz: row.get::<_, i64>(11)? as usize,
                up: row.get::<_, i64>(12)? as usize,
                down: row.get::<_, i64>(13)? as usize,
                dur: row.get::<_, i64>(14)? as u64,
                lp: row.get::<_, i64>(15)? as u16,
                cip: row.get(16)?,
                capp: row.get(17)?,
                cpid: row.get::<_, Option<i32>>(18)?.map(|v| v as u32),
                acct: row.get(30)?,
                flags: row.get::<_, i32>(19)? as u32,
                fc: row.get::<_, i64>(20)? as usize,
                ss: socket_status,
                st: format_timestamp_ms(row.get::<_, i64>(2)? as u64),
                et: {
                    let ts = row.get::<_, i64>(2)? as u64;
                    let dur = row.get::<_, i64>(14)? as u64;
                    if dur > 0 {
                        Some(format_timestamp_ms(ts + dur))
                    } else {
                        None
                    }
                },
                rc,
                rp,
            })
        })
        .map(|r| r.filter_map(|r| r.ok()).collect())
        .unwrap_or_default()
    }

    pub(super) fn row_to_record_core(row: &rusqlite::Row) -> rusqlite::Result<TrafficRecord> {
        let flags: i32 = row.get("flags")?;

        Ok(TrafficRecord {
            sequence: row.get::<_, i64>("sequence")? as u64,
            id: row.get("id")?,
            timestamp: row.get::<_, i64>("timestamp")? as u64,
            host: row.get("host")?,
            method: row.get("method")?,
            status: row.get::<_, i32>("status")? as u16,
            protocol: row.get("protocol")?,
            url: row.get("url")?,
            path: row.get("path")?,
            content_type: row.get("content_type")?,
            request_content_type: row.get("request_content_type")?,
            request_size: row.get::<_, i64>("request_size")? as usize,
            response_size: row.get::<_, i64>("response_size")? as usize,
            upload_bytes: row.get::<_, i64>("upload_bytes")? as usize,
            download_bytes: row.get::<_, i64>("download_bytes")? as usize,
            duration_ms: row.get::<_, i64>("duration_ms")? as u64,
            listener_port: row.get::<_, i64>("listener_port")? as u16,
            client_ip: row.get("client_ip")?,
            client_app: row.get("client_app")?,
            client_pid: row.get::<_, Option<i32>>("client_pid")?.map(|v| v as u32),
            client_path: row.get("client_path")?,
            account_name: row.get("account_name")?,
            is_tunnel: flags & 1 != 0,
            is_websocket: flags & 2 != 0,
            is_sse: flags & 4 != 0,
            is_h3: flags & 8 != 0,
            has_rule_hit: flags & 16 != 0,
            is_replay: flags & 32 != 0,
            frame_count: row.get::<_, i64>("frame_count")? as usize,
            last_frame_id: row.get::<_, i64>("last_frame_id")? as u64,
            timing: None,
            request_headers: None,
            original_response_headers: None,
            matched_rules: None,
            socket_status: build_socket_status_summary(
                row.get::<_, bool>("socket_is_open")?,
                row.get::<_, i64>("socket_send_count")? as u64,
                row.get::<_, i64>("socket_receive_count")? as u64,
                row.get::<_, i64>("socket_send_bytes")? as u64,
                row.get::<_, i64>("socket_receive_bytes")? as u64,
                row.get::<_, i64>("socket_frame_count")? as usize,
            ),
            request_body_ref: None,
            response_body_ref: None,
            derived_response_body_ref: None,
            body_metadata: None,
            raw_request_body_ref: None,
            raw_response_body_ref: None,
            actual_url: None,
            actual_host: None,
            original_request_headers: None,
            response_headers: None,
            error_message: None,
            req_script_results: None,
            res_script_results: None,
            decode_req_script_results: None,
            decode_res_script_results: None,
            devtools_client_req_id: row.get("devtools_client_req_id")?,
        })
    }

    pub(super) fn load_record_details(conn: &Connection, id: &str, record: &mut TrafficRecord) {
        let _ = conn
            .query_row(
                "SELECT timing_blob, request_headers_blob, original_response_headers_blob, \
                 matched_rules_blob, request_body_ref_blob, response_body_ref_blob, \
                 derived_response_body_ref_blob, raw_request_body_ref_blob, raw_response_body_ref_blob, body_metadata_blob, actual_url, actual_host, \
                 original_request_headers_blob, response_headers_blob, \
                 socket_status_blob, req_script_results_blob, res_script_results_blob, \
                 decode_req_script_results_blob, decode_res_script_results_blob, error_message \
                 FROM traffic_record_details WHERE id = ?",
                [id],
                |row| {
                    let timing_blob: Option<Vec<u8>> = row.get(0)?;
                    let req_headers_blob: Option<Vec<u8>> = row.get(1)?;
                    let res_headers_blob: Option<Vec<u8>> = row.get(2)?;
                    let rules_blob: Option<Vec<u8>> = row.get(3)?;
                    let req_body_blob: Option<Vec<u8>> = row.get(4)?;
                    let res_body_blob: Option<Vec<u8>> = row.get(5)?;
                    let derived_res_body_blob: Option<Vec<u8>> = row.get(6)?;
                    let raw_req_body_blob: Option<Vec<u8>> = row.get(7)?;
                    let raw_res_body_blob: Option<Vec<u8>> = row.get(8)?;
                    let body_metadata_blob: Option<Vec<u8>> = row.get(9)?;
                    let orig_req_headers_blob: Option<Vec<u8>> = row.get(12)?;
                    let res_headers_modified_blob: Option<Vec<u8>> = row.get(13)?;
                    let socket_status_blob: Option<Vec<u8>> = row.get(14)?;
                    let req_script_results_blob: Option<Vec<u8>> = row.get(15)?;
                    let res_script_results_blob: Option<Vec<u8>> = row.get(16)?;
                    let decode_req_results_blob: Option<Vec<u8>> = row.get(17)?;
                    let decode_res_results_blob: Option<Vec<u8>> = row.get(18)?;

                    record.timing = timing_blob.and_then(|b| decode_detail_blob(&b).ok());
                    record.request_headers =
                        req_headers_blob.and_then(|b| decode_detail_blob(&b).ok());
                    record.original_response_headers =
                        res_headers_blob.and_then(|b| decode_detail_blob(&b).ok());
                    record.matched_rules = rules_blob.and_then(|b| decode_detail_blob(&b).ok());
                    record.request_body_ref =
                        req_body_blob.and_then(|b| decode_detail_blob(&b).ok());
                    record.response_body_ref =
                        res_body_blob.and_then(|b| decode_detail_blob(&b).ok());
                    record.derived_response_body_ref =
                        derived_res_body_blob.and_then(|b| decode_detail_blob(&b).ok());
                    record.raw_request_body_ref =
                        raw_req_body_blob.and_then(|b| decode_detail_blob(&b).ok());
                    record.raw_response_body_ref =
                        raw_res_body_blob.and_then(|b| decode_detail_blob(&b).ok());
                    record.body_metadata =
                        body_metadata_blob.and_then(|b| decode_detail_blob(&b).ok());
                    record.actual_url = row.get(10)?;
                    record.actual_host = row.get(11)?;
                    record.original_request_headers =
                        orig_req_headers_blob.and_then(|b| decode_detail_blob(&b).ok());
                    record.response_headers =
                        res_headers_modified_blob.and_then(|b| decode_detail_blob(&b).ok());
                    record.socket_status = socket_status_blob
                        .and_then(|b| decode_detail_blob(&b).ok())
                        .or_else(|| record.socket_status.clone());
                    record.req_script_results =
                        req_script_results_blob.and_then(|b| serde_json::from_slice(&b).ok());
                    record.res_script_results =
                        res_script_results_blob.and_then(|b| serde_json::from_slice(&b).ok());
                    record.decode_req_script_results =
                        decode_req_results_blob.and_then(|b| serde_json::from_slice(&b).ok());
                    record.decode_res_script_results =
                        decode_res_results_blob.and_then(|b| serde_json::from_slice(&b).ok());
                    record.error_message = row.get(19)?;
                    Ok(())
                },
            )
            .optional();
    }
}
