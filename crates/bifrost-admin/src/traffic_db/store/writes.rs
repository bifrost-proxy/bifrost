use super::*;

impl TrafficDbStore {
    pub(super) fn serialize_detail_fields(record: &TrafficRecord) -> SerializedDetailFields {
        let timing_blob = record
            .timing
            .as_ref()
            .and_then(|t| encode_detail_blob(t).ok());
        let req_headers_blob = record
            .request_headers
            .as_ref()
            .and_then(|h| encode_detail_blob(h).ok());
        let res_headers_blob = record
            .original_response_headers
            .as_ref()
            .and_then(|h| encode_detail_blob(h).ok());
        let rules_blob = record
            .matched_rules
            .as_ref()
            .and_then(|r| encode_detail_blob(r).ok());
        let socket_status_blob = record
            .socket_status
            .as_ref()
            .and_then(|s| encode_detail_blob(s).ok());
        let req_body_blob = record
            .request_body_ref
            .as_ref()
            .and_then(|b| encode_detail_blob(b).ok());
        let res_body_blob = record
            .response_body_ref
            .as_ref()
            .and_then(|b| encode_detail_blob(b).ok());
        let derived_res_body_blob = record
            .derived_response_body_ref
            .as_ref()
            .and_then(|b| encode_detail_blob(b).ok());
        let raw_req_body_blob = record
            .raw_request_body_ref
            .as_ref()
            .and_then(|b| encode_detail_blob(b).ok());
        let raw_res_body_blob = record
            .raw_response_body_ref
            .as_ref()
            .and_then(|b| encode_detail_blob(b).ok());
        let body_metadata_blob = record
            .body_metadata
            .as_ref()
            .and_then(|metadata| encode_detail_blob(metadata).ok());
        let orig_req_headers_blob = record
            .original_request_headers
            .as_ref()
            .and_then(|h| encode_detail_blob(h).ok());
        let res_headers_modified_blob = record
            .response_headers
            .as_ref()
            .and_then(|h| encode_detail_blob(h).ok());
        let req_script_results_blob = record
            .req_script_results
            .as_ref()
            .and_then(|r| serde_json::to_vec(r).ok());
        let res_script_results_blob = record
            .res_script_results
            .as_ref()
            .and_then(|r| serde_json::to_vec(r).ok());
        let decode_req_results_blob = record
            .decode_req_script_results
            .as_ref()
            .and_then(|r| serde_json::to_vec(r).ok());
        let decode_res_results_blob = record
            .decode_res_script_results
            .as_ref()
            .and_then(|r| serde_json::to_vec(r).ok());

        SerializedDetailFields {
            timing_blob,
            req_headers_blob,
            res_headers_blob,
            rules_blob,
            socket_status_blob,
            req_body_blob,
            res_body_blob,
            derived_res_body_blob,
            raw_req_body_blob,
            raw_res_body_blob,
            body_metadata_blob,
            orig_req_headers_blob,
            res_headers_modified_blob,
            req_script_results_blob,
            res_script_results_blob,
            decode_req_results_blob,
            decode_res_results_blob,
        }
    }

    pub(super) fn insert_record_tx(
        tx: &rusqlite::Transaction<'_>,
        seq: u64,
        record: &TrafficRecord,
    ) -> rusqlite::Result<Option<TrafficStatisticsDimensions>> {
        let upload_bytes = trusted_upload_bytes(record);
        let download_bytes = trusted_download_bytes(record);
        let flags = encode_flags(record);
        let socket_is_open = record.socket_status.as_ref().is_some_and(|s| s.is_open);
        let socket_send_count = record.socket_status.as_ref().map_or(0, |s| s.send_count);
        let socket_receive_count = record.socket_status.as_ref().map_or(0, |s| s.receive_count);
        let socket_send_bytes = record.socket_status.as_ref().map_or(0, |s| s.send_bytes);
        let socket_receive_bytes = record.socket_status.as_ref().map_or(0, |s| s.receive_bytes);
        let socket_frame_count = record
            .socket_status
            .as_ref()
            .map_or(record.frame_count, |s| s.frame_count);
        let (rule_count, rule_protocols) = summarize_matched_rules(record.matched_rules.as_deref());
        let rule_protocols_json = serde_json::to_string(&rule_protocols).ok();
        let SerializedDetailFields {
            timing_blob,
            req_headers_blob,
            res_headers_blob,
            rules_blob,
            socket_status_blob,
            req_body_blob,
            res_body_blob,
            derived_res_body_blob,
            raw_req_body_blob,
            raw_res_body_blob,
            body_metadata_blob,
            orig_req_headers_blob,
            res_headers_modified_blob,
            req_script_results_blob,
            res_script_results_blob,
            decode_req_results_blob,
            decode_res_results_blob,
        } = Self::serialize_detail_fields(record);

        macro_rules! insert_main_record {
            ($sql:expr) => {
                tx.execute(
                    $sql,
                    params![
                        seq as i64,
                        &record.id,
                        record.timestamp as i64,
                        &record.host,
                        &record.method,
                        record.status as i32,
                        &record.protocol,
                        &record.url,
                        &record.path,
                        &record.content_type,
                        &record.request_content_type,
                        record.request_size as i64,
                        record.response_size as i64,
                        upload_bytes as i64,
                        download_bytes as i64,
                        record.duration_ms as i64,
                        record.listener_port as i64,
                        &record.client_ip,
                        &record.client_app,
                        record.client_pid.map(|p| p as i32),
                        &record.client_path,
                        &record.account_name,
                        flags as i32,
                        record.frame_count as i64,
                        record.last_frame_id as i64,
                        socket_is_open,
                        socket_send_count as i64,
                        socket_receive_count as i64,
                        socket_send_bytes as i64,
                        socket_receive_bytes as i64,
                        socket_frame_count as i64,
                        rule_count as i64,
                        &rule_protocols_json,
                        &record.devtools_client_req_id,
                    ],
                )
            };
        }
        let previous_dimensions = match insert_main_record!(get_insert_sql()) {
            Ok(_) => None,
            Err(error)
                if matches!(
                    &error,
                    rusqlite::Error::SqliteFailure(code, _)
                        if code.code == rusqlite::ErrorCode::ConstraintViolation
                ) =>
            {
                let Some(previous) = TrafficStatisticsDimensions::load_by_id(tx, &record.id) else {
                    return Err(error);
                };
                insert_main_record!(get_replace_sql())?;
                Some(previous)
            }
            Err(error) => return Err(error),
        };
        tx.execute(
            get_insert_detail_sql(),
            params![
                &record.id,
                timing_blob,
                req_headers_blob,
                res_headers_blob,
                rules_blob,
                req_body_blob,
                res_body_blob,
                derived_res_body_blob,
                raw_req_body_blob,
                raw_res_body_blob,
                body_metadata_blob,
                &record.actual_url,
                &record.actual_host,
                orig_req_headers_blob,
                res_headers_modified_blob,
                socket_status_blob,
                req_script_results_blob,
                res_script_results_blob,
                decode_req_results_blob,
                decode_res_results_blob,
                &record.error_message,
            ],
        )?;
        Ok(previous_dimensions)
    }

    pub fn record(&self, mut record: TrafficRecord) {
        let seq = self.current_sequence.fetch_add(1, Ordering::SeqCst);
        record.sequence = seq;

        {
            let mut conn = self.write_conn.lock();
            let result = (|| -> rusqlite::Result<Option<TrafficStatisticsDimensions>> {
                let tx = conn.transaction()?;
                let previous = Self::insert_record_tx(&tx, seq, &record)?;
                tx.commit()?;
                Ok(previous)
            })();

            match result {
                Err(e) => {
                    tracing::error!(error = %e, id = %record.id, "[TRAFFIC_DB] Failed to insert record");
                }
                Ok(previous) => {
                    let next = TrafficStatisticsDimensions::from_record(&record);
                    if let Some(previous) = previous {
                        self.traffic_statistics.write().replace(&previous, &next);
                    } else {
                        self.increase_record_count(1);
                        self.traffic_statistics.write().insert(&next);
                    }
                    if Self::should_keep_in_cache(&record) {
                        self.recent_cache.write().put(
                            record.id.clone(),
                            TrafficSummaryCompact::from_record(&record),
                        );
                    }
                    if self.tx.receiver_count() > 0 {
                        let _ = self.tx.send(TrafficStoreEvent::Inserted(record.clone()));
                    }
                }
            }
        }

        let old = self.write_count.fetch_add(1, Ordering::Relaxed);
        if old / CLEANUP_CHECK_INTERVAL != (old + 1) / CLEANUP_CHECK_INTERVAL {
            self.maybe_cleanup();
        }
    }

    pub fn record_batch(&self, records: Vec<TrafficRecord>) {
        let batch_size = records.len();
        if let Err(error) = self.try_record_batch(records) {
            tracing::error!(%error, batch_size, "[TRAFFIC_DB] Failed to insert record batch");
        }
    }

    pub fn try_record_batch(&self, records: Vec<TrafficRecord>) -> rusqlite::Result<()> {
        if records.is_empty() {
            return Ok(());
        }

        let mut records_with_seq = Vec::with_capacity(records.len());
        for mut record in records {
            let seq = self.current_sequence.fetch_add(1, Ordering::SeqCst);
            record.sequence = seq;
            records_with_seq.push((seq, record));
        }

        let batch_len = records_with_seq.len();

        let mut conn = self.write_conn.lock();
        let previous_dimensions = {
            let tx = conn.transaction()?;
            let mut previous = Vec::with_capacity(records_with_seq.len());
            for (seq, record) in &records_with_seq {
                previous.push(Self::insert_record_tx(&tx, *seq, record)?);
            }
            tx.commit()?;
            previous
        };

        let mut inserted_count = 0usize;
        let mut statistics = self.traffic_statistics.write();
        for ((_, record), previous) in records_with_seq.iter().zip(previous_dimensions) {
            let next = TrafficStatisticsDimensions::from_record(record);
            if let Some(previous) = previous {
                statistics.replace(&previous, &next);
            } else {
                statistics.insert(&next);
                inserted_count += 1;
            }
        }
        drop(statistics);
        self.increase_record_count(inserted_count);
        drop(conn);

        let mut cache = self.recent_cache.write();
        for (_, record) in &records_with_seq {
            if Self::should_keep_in_cache(record) {
                cache.put(
                    record.id.clone(),
                    TrafficSummaryCompact::from_record(record),
                );
            }
        }
        drop(cache);

        if self.tx.receiver_count() > 0 {
            for (_, record) in &records_with_seq {
                let _ = self.tx.send(TrafficStoreEvent::Inserted(record.clone()));
            }
        }

        let old = self
            .write_count
            .fetch_add(batch_len as u64, Ordering::Relaxed);
        let new = old + batch_len as u64;
        if old / CLEANUP_CHECK_INTERVAL != new / CLEANUP_CHECK_INTERVAL {
            self.maybe_cleanup();
        }
        Ok(())
    }

    pub fn update_by_id<F>(&self, id: &str, updater: F) -> bool
    where
        F: FnOnce(&mut TrafficRecord),
    {
        // 注意：recent_cache 只保存 summary，不保存完整 TrafficRecord。
        // update 必须以 DB 为准，否则会出现“用精简结构覆盖写回导致字段丢失”的风险。
        if let Some(mut record) = self.get_by_id_from_db(id) {
            updater(&mut record);
            if !self.persist_update(&record) {
                return false;
            }
            {
                let mut cache = self.recent_cache.write();
                if Self::should_keep_in_cache(&record) {
                    cache.put(
                        record.id.clone(),
                        TrafficSummaryCompact::from_record(&record),
                    );
                } else {
                    cache.pop(&record.id);
                }
            }
            if self.tx.receiver_count() > 0 {
                let _ = self.tx.send(TrafficStoreEvent::Updated(record));
            }
            return true;
        }

        false
    }

    pub(super) fn should_keep_in_cache(record: &TrafficRecord) -> bool {
        if record.status == 0 {
            return true;
        }
        if record.is_websocket || record.is_sse || record.is_tunnel {
            return true;
        }
        record.socket_status.as_ref().is_some_and(|s| s.is_open)
    }

    pub(super) fn persist_update(&self, record: &TrafficRecord) -> bool {
        let mut conn = self.write_conn.lock();
        let upload_bytes = trusted_upload_bytes(record);
        let download_bytes = trusted_download_bytes(record);
        let flags = encode_flags(record);
        let socket_is_open = record.socket_status.as_ref().is_some_and(|s| s.is_open);
        let socket_send_count = record.socket_status.as_ref().map_or(0, |s| s.send_count);
        let socket_receive_count = record.socket_status.as_ref().map_or(0, |s| s.receive_count);
        let socket_send_bytes = record.socket_status.as_ref().map_or(0, |s| s.send_bytes);
        let socket_receive_bytes = record.socket_status.as_ref().map_or(0, |s| s.receive_bytes);
        let socket_frame_count = record
            .socket_status
            .as_ref()
            .map_or(record.frame_count, |s| s.frame_count);
        let (rule_count, rule_protocols) = summarize_matched_rules(record.matched_rules.as_deref());
        let rule_protocols_json = serde_json::to_string(&rule_protocols).ok();
        let SerializedDetailFields {
            timing_blob,
            req_headers_blob,
            res_headers_blob,
            rules_blob,
            socket_status_blob,
            req_body_blob,
            res_body_blob,
            derived_res_body_blob,
            raw_req_body_blob,
            raw_res_body_blob,
            body_metadata_blob,
            orig_req_headers_blob,
            res_headers_modified_blob,
            req_script_results_blob,
            res_script_results_blob,
            decode_req_results_blob,
            decode_res_results_blob,
        } = Self::serialize_detail_fields(record);

        let result = (|| -> rusqlite::Result<Option<TrafficStatisticsDimensions>> {
            let tx = conn.transaction()?;
            let previous_dimensions = TrafficStatisticsDimensions::load_by_id(&tx, &record.id);
            let updated = tx.execute(
                get_update_sql(),
                params![
                    record.status as i32,
                    &record.content_type,
                    &record.request_content_type,
                    record.request_size as i64,
                    record.response_size as i64,
                    upload_bytes as i64,
                    download_bytes as i64,
                    record.duration_ms as i64,
                    record.listener_port as i64,
                    &record.client_app,
                    record.client_pid.map(|p| p as i32),
                    &record.client_path,
                    &record.account_name,
                    flags as i32,
                    record.frame_count as i64,
                    record.last_frame_id as i64,
                    socket_is_open,
                    socket_send_count as i64,
                    socket_receive_count as i64,
                    socket_send_bytes as i64,
                    socket_receive_bytes as i64,
                    socket_frame_count as i64,
                    rule_count as i64,
                    rule_protocols_json,
                    &record.devtools_client_req_id,
                    &record.id,
                ],
            )?;
            if updated == 0 || previous_dimensions.is_none() {
                return Ok(None);
            }
            tx.execute(
                get_update_detail_sql(),
                params![
                    &record.id,
                    timing_blob,
                    req_headers_blob,
                    res_headers_blob,
                    rules_blob,
                    req_body_blob,
                    res_body_blob,
                    derived_res_body_blob,
                    raw_req_body_blob,
                    raw_res_body_blob,
                    body_metadata_blob,
                    &record.actual_url,
                    &record.actual_host,
                    orig_req_headers_blob,
                    res_headers_modified_blob,
                    socket_status_blob,
                    req_script_results_blob,
                    res_script_results_blob,
                    decode_req_results_blob,
                    decode_res_results_blob,
                    &record.error_message,
                ],
            )?;
            tx.commit()?;
            Ok(previous_dimensions)
        })();

        match result {
            Err(e) => {
                tracing::error!(error = %e, id = %record.id, "[TRAFFIC_DB] Failed to update record");
                false
            }
            Ok(None) => false,
            Ok(Some(previous_dimensions)) => {
                let next_dimensions = TrafficStatisticsDimensions::from_persisted_update(
                    record,
                    &previous_dimensions,
                );
                self.traffic_statistics
                    .write()
                    .replace(&previous_dimensions, &next_dimensions);
                true
            }
        }
    }
}
