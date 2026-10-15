use super::*;

impl TrafficDbStore {
    pub fn clear(&self) -> rusqlite::Result<()> {
        self.clear_with_active_ids(&[])
    }

    pub fn clear_with_active_ids(&self, active_connection_ids: &[String]) -> rusqlite::Result<()> {
        let conn = self.write_conn.lock();
        self.preserve_sequence_high_water(&conn)?;

        let active_ids_set: std::collections::HashSet<&str> =
            active_connection_ids.iter().map(|s| s.as_str()).collect();

        let pending_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM traffic_records WHERE status = 0",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);

        tracing::info!(
            pending = pending_count,
            active_connections = active_connection_ids.len(),
            "[TRAFFIC_DB] Clearing traffic records, preserving active"
        );

        let transaction = conn.unchecked_transaction()?;
        if active_connection_ids.is_empty() {
            transaction.execute("DELETE FROM traffic_records", [])?;
        } else {
            let placeholders: String = active_connection_ids
                .iter()
                .map(|_| "?")
                .collect::<Vec<_>>()
                .join(",");

            let sql = format!(
                "DELETE FROM traffic_records WHERE id NOT IN ({})",
                placeholders
            );

            transaction.execute(
                &sql,
                rusqlite::params_from_iter(active_connection_ids.iter()),
            )?;
        }
        transaction.commit()?;
        if active_connection_ids.is_empty() {
            self.record_count.store(0, Ordering::Relaxed);
            *self.traffic_statistics.write() = TrafficStatistics::default();
        } else {
            let statistics = TrafficStatistics::load(&conn);
            self.record_count
                .store(statistics.total_requests(), Ordering::Relaxed);
            *self.traffic_statistics.write() = statistics;
        }

        #[cfg(test)]
        if let Some(hook) = self.clear_before_cache.read().as_ref() {
            hook();
        }
        let mut cache = self.recent_cache.write();
        let preserved_ids: Vec<String> = cache
            .iter()
            .filter(|(id, summary)| {
                active_ids_set.contains(id.as_str())
                    || (summary.is_websocket() && summary.ss.as_ref().is_some_and(|s| s.is_open))
            })
            .map(|(k, _)| k.clone())
            .collect();

        let mut new_cache = LruCache::new(
            std::num::NonZeroUsize::new(cache.cap().get())
                .unwrap_or(std::num::NonZeroUsize::new(1000).unwrap()),
        );
        for id in preserved_ids {
            if let Some(record) = cache.pop(&id) {
                new_cache.put(id, record);
            }
        }
        *cache = new_cache;

        // Keep the DB -> cache lock order throughout clear. Reacquiring the
        // database lock while holding cache can deadlock with another writer.
        drop(cache);
        #[cfg(test)]
        if let Some(hook) = self.clear_before_compact.read().as_ref() {
            hook();
        }
        Self::compact_with_conn(&conn, active_connection_ids.is_empty());

        tracing::info!("[TRAFFIC_DB] Traffic records cleared (active preserved)");
        Ok(())
    }

    pub(super) fn compact_with_conn(conn: &Connection, full_vacuum: bool) {
        if let Err(e) = conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)") {
            tracing::warn!(error = %e, "[TRAFFIC_DB] WAL checkpoint failed during compact");
        }
        if full_vacuum {
            if let Err(e) = conn.execute_batch("VACUUM") {
                tracing::warn!(error = %e, "[TRAFFIC_DB] VACUUM failed");
            } else {
                tracing::info!("[TRAFFIC_DB] VACUUM completed");
            }
        }
    }

    pub fn compact_db(&self, full_vacuum: bool) {
        let conn = self.write_conn.lock();
        Self::compact_with_conn(&conn, full_vacuum);
    }

    pub fn delete_by_ids(&self, ids: &[String]) -> rusqlite::Result<usize> {
        if ids.is_empty() {
            return Ok(0);
        }

        let conn = self.write_conn.lock();
        let deleted = self.delete_by_ids_with_conn(&conn, ids)?;
        self.decrease_record_count(deleted);
        tracing::info!(
            count = deleted,
            "[TRAFFIC_DB] Deleted traffic records by ids"
        );
        Ok(deleted)
    }

    pub(super) fn cleanup_trigger_threshold(max: usize) -> usize {
        let overflow =
            (max * CLEANUP_TRIGGER_OVERFLOW_PERCENT / 100).min(CLEANUP_TRIGGER_OVERFLOW_MAX);
        max + overflow
    }

    pub(super) fn cleanup_target_count(max: usize) -> usize {
        (max * CLEANUP_TARGET_PERCENT / 100).max(1)
    }

    pub(super) const SIZE_CLEANUP_MAX_DELETE_PERCENT: usize = 20;

    pub(super) fn maybe_cleanup(&self) {
        let max = self.max_records.load(Ordering::Relaxed);
        let count = self.record_count.load(Ordering::Relaxed);

        let trigger = Self::cleanup_trigger_threshold(max);
        let needs_record_cleanup = count > trigger;

        let max_db_size_bytes = self.max_db_size_bytes.load(Ordering::Relaxed);
        let needs_size_cleanup = max_db_size_bytes > 0 && {
            let db_size = fs::metadata(&self.db_path).map(|m| m.len()).unwrap_or(0);
            db_size > max_db_size_bytes
        };

        if !needs_record_cleanup && !needs_size_cleanup {
            return;
        }

        let conn = self.write_conn.lock();

        let mut did_record_cleanup = false;

        if needs_record_cleanup {
            let current_count = self.record_count.load(Ordering::Relaxed);
            if current_count > trigger {
                let target = Self::cleanup_target_count(max);
                let delete_count = current_count.saturating_sub(target);
                let deleted = self.delete_oldest_by_limit(&conn, delete_count);
                if deleted > 0 {
                    self.decrease_record_count(deleted);
                    did_record_cleanup = true;
                    tracing::info!(
                        deleted = deleted,
                        max = max,
                        trigger = trigger,
                        target = target,
                        current = current_count,
                        "[TRAFFIC_DB] Cleaned up old records (trigger: {}%, target: {}%)",
                        CLEANUP_TRIGGER_OVERFLOW_PERCENT,
                        CLEANUP_TARGET_PERCENT,
                    );
                    // Count-based rolling retention is a hot write path. A
                    // TRUNCATE checkpoint here holds the write lock while all
                    // readers release the WAL and turns a burst into repeated
                    // multi-second stalls. SQLite's normal auto-checkpoint is
                    // sufficient because this cleanup is about record count,
                    // not an immediate on-disk byte limit.
                }
            }
        }

        if needs_size_cleanup && !did_record_cleanup {
            let db_size = fs::metadata(&self.db_path).map(|m| m.len()).unwrap_or(0);
            if db_size > max_db_size_bytes {
                let current_count = self.record_count.load(Ordering::Relaxed);
                if current_count == 0 {
                    return;
                }
                let target_size = max_db_size_bytes.saturating_sub(max_db_size_bytes / 4);
                let avg_bytes_per_record = (db_size / current_count as u64).max(1);
                let bytes_to_remove = db_size.saturating_sub(target_size);
                let estimated_remove = bytes_to_remove.div_ceil(avg_bytes_per_record) as usize;
                let max_allowed =
                    (current_count * Self::SIZE_CLEANUP_MAX_DELETE_PERCENT / 100).max(1);
                let to_remove = estimated_remove.min(max_allowed);
                let deleted = self.delete_oldest_by_limit(&conn, to_remove);
                if deleted > 0 {
                    self.decrease_record_count(deleted);
                    tracing::info!(
                        deleted = deleted,
                        estimated = estimated_remove,
                        capped_at = max_allowed,
                        db_size = db_size,
                        max_db_size_bytes = max_db_size_bytes,
                        target_size = target_size,
                        "[TRAFFIC_DB] Cleaned up records due to DB size limit"
                    );
                    Self::compact_with_conn(&conn, false);
                }
            }
        }
    }

    pub fn cleanup_expired_records(&self) -> usize {
        let retention_hours = self.retention_hours.load(Ordering::Relaxed);
        let retention_ms = retention_hours * 60 * 60 * 1000;
        let now = chrono::Utc::now().timestamp_millis() as u64;
        let cutoff = now.saturating_sub(retention_ms);

        let conn = self.write_conn.lock();
        let deleted = self.delete_expired_by_cutoff(&conn, cutoff);
        if deleted > 0 {
            self.decrease_record_count(deleted);
            tracing::info!(
                deleted = deleted,
                retention_hours = retention_hours,
                "[TRAFFIC_DB] Cleaned up expired records"
            );
        }
        deleted
    }

    pub fn count(&self) -> usize {
        self.record_count.load(Ordering::Relaxed)
    }

    pub fn stats(&self) -> TrafficDbStats {
        let count = self.count();
        let db_size = fs::metadata(&self.db_path).map(|m| m.len()).unwrap_or(0);

        let conn = self.read_pool.acquire();
        let oldest: Option<u64> = conn
            .query_row("SELECT MIN(timestamp) FROM traffic_records", [], |row| {
                row.get::<_, Option<i64>>(0)
            })
            .ok()
            .flatten()
            .map(|v| v as u64);

        let newest: Option<u64> = conn
            .query_row("SELECT MAX(timestamp) FROM traffic_records", [], |row| {
                row.get::<_, Option<i64>>(0)
            })
            .ok()
            .flatten()
            .map(|v| v as u64);

        TrafficDbStats {
            record_count: count,
            db_size,
            db_path: self.db_path.display().to_string(),
            max_records: self.max_records.load(Ordering::Relaxed),
            retention_hours: self.retention_hours.load(Ordering::Relaxed),
            current_sequence: self.current_sequence.load(Ordering::Relaxed),
            oldest_timestamp: oldest,
            newest_timestamp: newest,
        }
    }

    pub fn current_sequence(&self) -> u64 {
        self.current_sequence.load(Ordering::Relaxed)
    }

    pub fn oldest_sequence(&self) -> Option<u64> {
        let conn = self.read_pool.acquire();
        conn.query_row("SELECT MIN(sequence) FROM traffic_records", [], |row| {
            row.get::<_, Option<i64>>(0)
        })
        .ok()
        .flatten()
        .map(|sequence| sequence as u64)
    }

    pub fn subscribe(&self) -> broadcast::Receiver<TrafficStoreEvent> {
        self.tx.subscribe()
    }

    pub fn find_latest_client_path_by_app(&self, app_name: &str) -> Option<String> {
        let conn = self.read_pool.acquire();
        conn.query_row(
            "SELECT client_path FROM traffic_records WHERE client_app = ?1 AND client_path IS NOT NULL ORDER BY sequence DESC LIMIT 1",
            [app_name],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()
        .ok()
        .flatten()
        .flatten()
    }

    pub fn set_max_records(&self, max: usize) {
        let normalized = max.clamp(MIN_TRAFFIC_MAX_RECORDS, MAX_TRAFFIC_MAX_RECORDS);
        let old = self.max_records.swap(normalized, Ordering::SeqCst);
        if old != normalized {
            tracing::info!(
                old = old,
                new = normalized,
                "[TRAFFIC_DB] Max records updated"
            );
            self.maybe_cleanup();
        }
    }

    pub fn set_max_db_size_bytes(&self, max: u64) {
        let old = self.max_db_size_bytes.swap(max, Ordering::SeqCst);
        if old != max {
            tracing::info!(
                old = old,
                new = max,
                "[TRAFFIC_DB] Max db size bytes updated"
            );
            self.maybe_cleanup();
        }
    }

    pub fn max_db_size_bytes(&self) -> u64 {
        self.max_db_size_bytes.load(Ordering::Relaxed)
    }

    pub fn set_retention_hours(&self, hours: u64) {
        let old = self.retention_hours.swap(hours, Ordering::SeqCst);
        if old != hours {
            tracing::info!(
                old = old,
                new = hours,
                "[TRAFFIC_DB] Retention hours updated"
            );
        }
    }

    pub(super) fn notify_cleanup(&self, ids: &[String]) {
        if ids.is_empty() {
            return;
        }
        if let Some(notifier) = self.cleanup_notifier.read().as_ref() {
            notifier(ids);
        }
    }

    pub(super) fn remove_from_cache(&self, ids: &[String]) {
        let ids_set: std::collections::HashSet<&str> = ids.iter().map(|s| s.as_str()).collect();
        let mut cache = self.recent_cache.write();
        for id in &ids_set {
            cache.pop(&id.to_string());
        }
    }

    pub(super) fn delete_by_ids_with_conn(
        &self,
        conn: &Connection,
        ids: &[String],
    ) -> rusqlite::Result<usize> {
        if ids.is_empty() {
            return Ok(0);
        }
        self.preserve_sequence_high_water(conn)?;
        // A failure in a later chunk must leave every row, statistic and cache
        // entry intact, so callers can retain all associated side-store data.
        let transaction = conn.unchecked_transaction()?;
        let mut deleted = 0usize;
        let mut removed_dimensions = Vec::new();
        for chunk in ids.chunks(500) {
            let mut seen = std::collections::HashSet::with_capacity(chunk.len());
            let unique_ids: Vec<&String> =
                chunk.iter().filter(|id| seen.insert(id.as_str())).collect();
            let dimensions: Vec<TrafficStatisticsDimensions> = unique_ids
                .iter()
                .filter_map(|id| TrafficStatisticsDimensions::load_by_id(&transaction, id))
                .collect();
            let placeholders: String = unique_ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
            let sql = format!("DELETE FROM traffic_records WHERE id IN ({})", placeholders);
            let count = transaction.execute(&sql, rusqlite::params_from_iter(unique_ids))?;
            deleted += count;
            if count > 0 {
                removed_dimensions.extend(dimensions);
            }
        }
        transaction.commit()?;
        let mut statistics = self.traffic_statistics.write();
        for dimensions in &removed_dimensions {
            statistics.remove(dimensions);
        }
        drop(statistics);
        self.remove_from_cache(ids);
        Ok(deleted)
    }

    pub(super) fn delete_oldest_by_limit(&self, conn: &Connection, limit: usize) -> usize {
        if limit == 0 {
            return 0;
        }
        let mut remaining = limit;
        let mut deleted = 0usize;
        while remaining > 0 {
            let batch = remaining.min(500);
            let mut ids = Vec::new();
            let mut stmt = match conn
                .prepare("SELECT id FROM traffic_records ORDER BY sequence ASC LIMIT ?")
            {
                Ok(s) => s,
                Err(_) => break,
            };
            if let Ok(iter) = stmt.query_map([batch as i64], |row| row.get(0)) {
                for id in iter.flatten() {
                    ids.push(id);
                }
            }
            if ids.is_empty() {
                break;
            }
            let count = self
                .delete_by_ids_with_conn(conn, &ids)
                .unwrap_or_else(|error| {
                    tracing::error!(%error, "[TRAFFIC_DB] Failed to delete oldest records");
                    0
                });
            if count == 0 {
                break;
            }
            deleted += count;
            self.notify_cleanup(&ids);
            if ids.len() >= remaining {
                break;
            }
            remaining = remaining.saturating_sub(ids.len());
        }
        deleted
    }

    pub(super) fn delete_expired_by_cutoff(&self, conn: &Connection, cutoff: u64) -> usize {
        let mut deleted = 0usize;
        loop {
            let mut ids = Vec::new();
            let mut stmt = match conn.prepare(
                "SELECT id FROM traffic_records WHERE timestamp < ? ORDER BY sequence ASC LIMIT ?",
            ) {
                Ok(s) => s,
                Err(_) => break,
            };
            if let Ok(iter) = stmt.query_map([cutoff as i64, 500i64], |row| row.get(0)) {
                for id in iter.flatten() {
                    ids.push(id);
                }
            }
            if ids.is_empty() {
                break;
            }
            let count = self
                .delete_by_ids_with_conn(conn, &ids)
                .unwrap_or_else(|error| {
                    tracing::error!(%error, "[TRAFFIC_DB] Failed to delete expired records");
                    0
                });
            if count == 0 {
                break;
            }
            deleted += count;
            self.notify_cleanup(&ids);
        }
        deleted
    }

    pub fn all_ids_set(&self) -> std::collections::HashSet<String> {
        let conn = self.read_pool.acquire();
        let mut stmt = match conn.prepare("SELECT id FROM traffic_records") {
            Ok(s) => s,
            Err(_) => return std::collections::HashSet::new(),
        };
        let iter = match stmt.query_map([], |row| row.get::<_, String>(0)) {
            Ok(i) => i,
            Err(_) => return std::collections::HashSet::new(),
        };
        iter.flatten().collect()
    }

    pub fn oldest_ids(&self, limit: usize, offset: usize) -> Vec<String> {
        if limit == 0 {
            return Vec::new();
        }
        let conn = self.read_pool.acquire();
        let mut stmt = match conn
            .prepare("SELECT id FROM traffic_records ORDER BY sequence ASC LIMIT ? OFFSET ?")
        {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        let iter = match stmt.query_map([limit as i64, offset as i64], |row| row.get(0)) {
            Ok(i) => i,
            Err(_) => return Vec::new(),
        };
        iter.flatten().collect()
    }

    pub fn checkpoint(&self) {
        let conn = self.write_conn.lock();
        if let Err(e) = conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)") {
            tracing::warn!(error = %e, "[TRAFFIC_DB] WAL checkpoint failed");
        }
    }

    #[cfg(test)]
    pub fn reset_debug_query_counters(&self) {
        self.query_calls.store(0, Ordering::Relaxed);
        self.get_by_ids_calls.store(0, Ordering::Relaxed);
    }

    #[cfg(test)]
    pub fn debug_query_counters(&self) -> (usize, usize) {
        (
            self.query_calls.load(Ordering::Relaxed),
            self.get_by_ids_calls.load(Ordering::Relaxed),
        )
    }
}
