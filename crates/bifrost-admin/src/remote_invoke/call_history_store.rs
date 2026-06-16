use std::collections::{BTreeMap, HashMap};
use std::fs::{self, OpenOptions};
use std::hash::{Hash, Hasher};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use bifrost_command::CanonicalQueryCommand;
use bifrost_core::{BifrostError, Result};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::types::CallInfo;
#[cfg(test)]
use super::types::CallStatus;

const CALL_HISTORY_STORE_FILE: &str = "remote_invoke_call_history.json";
const CALL_HISTORY_STORE_DIR: &str = "remote_invoke_call_history";
const CALL_HISTORY_STORE_VERSION: u32 = 2;
const CALL_HISTORY_COMMAND_TEXT_LIMIT: usize = 120;
const CALL_HISTORY_MAX_STORE_FILE_BYTES: u64 = 256 * 1024 * 1024;
const CALL_HISTORY_MAX_LINE_BYTES: usize = 2 * 1024 * 1024;
const CALL_HISTORY_HARD_MAX_RECORDS: usize = 1_000;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PersistedCallHistoryEntry {
    call_id: String,
    relay_url: String,
    client_instance_id: String,
    call: CallInfo,
    updated_at: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct CallHistoryClientMeta {
    version: u32,
    line_count: usize,
}

#[derive(Debug, Clone, Default)]
struct ReadClientEntriesResult {
    entries: Vec<PersistedCallHistoryEntry>,
    needs_compaction: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct CallHistoryPage {
    pub(crate) calls: Vec<CallInfo>,
    pub(crate) next_cursor: Option<u64>,
}

pub(crate) struct CallHistoryStore {
    file_path: PathBuf,
    dir_path: PathBuf,
    lock: Mutex<()>,
}

impl CallHistoryStore {
    pub(crate) fn new(data_dir: &std::path::Path) -> Self {
        let admin_dir = data_dir.join("admin");
        let file_path = admin_dir.join(CALL_HISTORY_STORE_FILE);
        if file_path.exists() {
            if let Err(error) = fs::remove_file(&file_path) {
                tracing::warn!(
                    error = %error,
                    path = %file_path.display(),
                    "failed to remove legacy remote invoke call history store"
                );
            } else {
                tracing::warn!(
                    path = %file_path.display(),
                    "removed legacy remote invoke call history store"
                );
            }
        }
        Self {
            file_path,
            dir_path: admin_dir.join(CALL_HISTORY_STORE_DIR),
            lock: Mutex::new(()),
        }
    }

    #[cfg(test)]
    pub(crate) fn load_for_client(
        &self,
        relay_url: &str,
        client_instance_id: &str,
        max_records: usize,
        retention_days: u32,
    ) -> Result<std::collections::VecDeque<CallInfo>> {
        let _guard = self.lock.lock();
        self.remove_legacy_store_file()?;
        let max_records = effective_max_records(max_records);
        let mut read = self.read_current_entries_for_client(relay_url, client_instance_id)?;
        let before = read.entries.len();
        let sanitized =
            prune_and_sanitize_client_entries(&mut read.entries, max_records, retention_days);
        if read.needs_compaction || sanitized || before != read.entries.len() {
            self.compact_client_entries(relay_url, client_instance_id, &read.entries)?;
        }
        let mut calls = read
            .entries
            .into_iter()
            .map(|entry| entry.call)
            .collect::<Vec<_>>();
        calls.sort_by_key(|call| call.started_at);
        Ok(calls.into())
    }

    pub(crate) fn load_page_for_client(
        &self,
        relay_url: &str,
        client_instance_id: &str,
        max_records: usize,
        retention_days: u32,
        limit: usize,
        before: Option<u64>,
    ) -> Result<CallHistoryPage> {
        let _guard = self.lock.lock();
        self.remove_legacy_store_file()?;
        let max_records = effective_max_records(max_records);
        let limit = limit.clamp(1, max_records.max(1));
        let mut read = self.read_current_entries_for_client(relay_url, client_instance_id)?;
        let before_len = read.entries.len();
        let sanitized =
            prune_and_sanitize_client_entries(&mut read.entries, max_records, retention_days);
        if read.needs_compaction || sanitized || before_len != read.entries.len() {
            self.compact_client_entries(relay_url, client_instance_id, &read.entries)?;
        }

        let mut calls = read
            .entries
            .into_iter()
            .map(|entry| entry.call)
            .filter(|call| before.is_none_or(|cursor| call.started_at < cursor))
            .collect::<Vec<_>>();
        calls.sort_by_key(|call| std::cmp::Reverse(call.started_at));

        let has_more = calls.len() > limit;
        if has_more {
            calls.truncate(limit);
        }
        let next_cursor = if has_more {
            calls.last().map(|call| call.started_at)
        } else {
            None
        };
        Ok(CallHistoryPage { calls, next_cursor })
    }

    pub(crate) fn load_call_for_client(
        &self,
        relay_url: &str,
        client_instance_id: &str,
        call_id: &str,
        max_records: usize,
        retention_days: u32,
    ) -> Result<Option<CallInfo>> {
        let _guard = self.lock.lock();
        self.remove_legacy_store_file()?;
        let max_records = effective_max_records(max_records);
        let mut read = self.read_current_entries_for_client(relay_url, client_instance_id)?;
        let before_len = read.entries.len();
        let sanitized =
            prune_and_sanitize_client_entries(&mut read.entries, max_records, retention_days);
        if read.needs_compaction || sanitized || before_len != read.entries.len() {
            self.compact_client_entries(relay_url, client_instance_id, &read.entries)?;
        }
        Ok(read
            .entries
            .into_iter()
            .find(|entry| entry.call_id == call_id)
            .map(|entry| entry.call))
    }

    pub(crate) fn upsert(
        &self,
        relay_url: &str,
        client_instance_id: &str,
        call: &CallInfo,
        max_records: usize,
        retention_days: u32,
    ) -> Result<()> {
        let _guard = self.lock.lock();
        self.remove_legacy_store_file()?;
        let max_records = effective_max_records(max_records);
        let mut call = call.clone();
        sanitize_call_for_history(&mut call);
        let entry = PersistedCallHistoryEntry {
            call_id: call.call_id.clone(),
            relay_url: relay_url.to_string(),
            client_instance_id: client_instance_id.to_string(),
            call,
            updated_at: now_millis(),
        };
        let line_count = self.append_client_entry(&entry)?;
        if line_count > max_records {
            let mut read = self.read_current_entries_for_client(relay_url, client_instance_id)?;
            prune_and_sanitize_client_entries(&mut read.entries, max_records, retention_days);
            self.compact_client_entries(relay_url, client_instance_id, &read.entries)?;
        }
        Ok(())
    }

    pub(crate) fn clear_for_client(
        &self,
        relay_url: &str,
        client_instance_id: &str,
    ) -> Result<usize> {
        let _guard = self.lock.lock();
        self.remove_legacy_store_file()?;
        let removed = self
            .read_current_entries_for_client(relay_url, client_instance_id)?
            .entries
            .len();
        self.remove_client_log(relay_url, client_instance_id)?;
        Ok(removed)
    }

    fn read_current_entries_for_client(
        &self,
        relay_url: &str,
        client_instance_id: &str,
    ) -> Result<ReadClientEntriesResult> {
        let mut latest_by_call: HashMap<String, PersistedCallHistoryEntry> = HashMap::new();
        let mut needs_compaction = false;

        let path = self.client_log_path(relay_url, client_instance_id);
        if !path.exists() {
            return Ok(ReadClientEntriesResult {
                entries: latest_by_call.into_values().collect(),
                needs_compaction,
            });
        }
        if let Ok(meta) = fs::metadata(&path) {
            if meta.len() > CALL_HISTORY_MAX_STORE_FILE_BYTES {
                self.remove_client_log(relay_url, client_instance_id)?;
                tracing::warn!(
                    path = %path.display(),
                    bytes = meta.len(),
                    "removed oversized remote invoke call history log"
                );
                return Ok(ReadClientEntriesResult {
                    entries: latest_by_call.into_values().collect(),
                    needs_compaction: true,
                });
            }
        }

        let file = fs::File::open(&path).map_err(|e| {
            BifrostError::Io(std::io::Error::other(format!(
                "open {}: {e}",
                path.display()
            )))
        })?;
        for line in BufReader::new(file).lines() {
            let line = match line {
                Ok(line) => line,
                Err(error) => {
                    tracing::warn!(
                        error = %error,
                        path = %path.display(),
                        "skip unreadable remote invoke call history line"
                    );
                    needs_compaction = true;
                    continue;
                }
            };
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if line.len() > CALL_HISTORY_MAX_LINE_BYTES {
                tracing::warn!(
                    path = %path.display(),
                    bytes = line.len(),
                    "skip oversized remote invoke call history line"
                );
                needs_compaction = true;
                continue;
            }
            match serde_json::from_str::<PersistedCallHistoryEntry>(line) {
                Ok(entry)
                    if entry.relay_url == relay_url
                        && entry.client_instance_id == client_instance_id =>
                {
                    upsert_latest_entry(&mut latest_by_call, entry);
                }
                Ok(_) => {
                    needs_compaction = true;
                }
                Err(error) => {
                    tracing::warn!(
                        error = %error,
                        path = %path.display(),
                        "skip malformed remote invoke call history line"
                    );
                    needs_compaction = true;
                }
            }
        }

        Ok(ReadClientEntriesResult {
            entries: latest_by_call.into_values().collect(),
            needs_compaction,
        })
    }

    fn append_client_entry(&self, entry: &PersistedCallHistoryEntry) -> Result<usize> {
        fs::create_dir_all(&self.dir_path).map_err(|e| {
            BifrostError::Io(std::io::Error::other(format!(
                "mkdir {}: {e}",
                self.dir_path.display()
            )))
        })?;
        let path = self.client_log_path(&entry.relay_url, &entry.client_instance_id);
        let mut content = serde_json::to_vec(entry)
            .map_err(|e| BifrostError::Config(format!("serialize call history entry: {e}")))?;
        content.push(b'\n');
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|e| {
                BifrostError::Io(std::io::Error::other(format!(
                    "open {}: {e}",
                    path.display()
                )))
            })?;
        file.write_all(&content).map_err(|e| {
            BifrostError::Io(std::io::Error::other(format!(
                "write {}: {e}",
                path.display()
            )))
        })?;
        let mut meta = self.read_client_meta(&entry.relay_url, &entry.client_instance_id)?;
        meta.version = CALL_HISTORY_STORE_VERSION;
        meta.line_count = meta.line_count.saturating_add(1);
        self.write_client_meta(&entry.relay_url, &entry.client_instance_id, &meta)?;
        Ok(meta.line_count)
    }

    fn compact_client_entries(
        &self,
        relay_url: &str,
        client_instance_id: &str,
        entries: &[PersistedCallHistoryEntry],
    ) -> Result<()> {
        if entries.is_empty() {
            self.remove_client_log(relay_url, client_instance_id)?;
            return Ok(());
        }
        fs::create_dir_all(&self.dir_path).map_err(|e| {
            BifrostError::Io(std::io::Error::other(format!(
                "mkdir {}: {e}",
                self.dir_path.display()
            )))
        })?;
        let path = self.client_log_path(relay_url, client_instance_id);
        let tmp_path = path.with_extension("jsonl.tmp");
        let mut file = fs::File::create(&tmp_path).map_err(|e| {
            BifrostError::Io(std::io::Error::other(format!(
                "create {}: {e}",
                tmp_path.display()
            )))
        })?;
        let mut written = 0usize;
        for entry in entries {
            if entry.relay_url == relay_url && entry.client_instance_id == client_instance_id {
                let line = serde_json::to_vec(entry).map_err(|e| {
                    BifrostError::Config(format!("serialize call history entry: {e}"))
                })?;
                file.write_all(&line).map_err(|e| {
                    BifrostError::Io(std::io::Error::other(format!(
                        "write {}: {e}",
                        tmp_path.display()
                    )))
                })?;
                file.write_all(b"\n").map_err(|e| {
                    BifrostError::Io(std::io::Error::other(format!(
                        "write {}: {e}",
                        tmp_path.display()
                    )))
                })?;
                written += 1;
            }
        }
        file.flush().map_err(BifrostError::Io)?;
        fs::rename(&tmp_path, &path).map_err(|e| {
            BifrostError::Io(std::io::Error::other(format!(
                "rename {} to {}: {e}",
                tmp_path.display(),
                path.display()
            )))
        })?;
        self.write_client_meta(
            relay_url,
            client_instance_id,
            &CallHistoryClientMeta {
                version: CALL_HISTORY_STORE_VERSION,
                line_count: written,
            },
        )?;
        Ok(())
    }

    fn remove_client_log(&self, relay_url: &str, client_instance_id: &str) -> Result<()> {
        let paths = [
            self.client_log_path(relay_url, client_instance_id),
            self.client_meta_path(relay_url, client_instance_id),
        ];
        for path in paths {
            if path.exists() {
                fs::remove_file(&path).map_err(|e| {
                    BifrostError::Io(std::io::Error::other(format!(
                        "remove {}: {e}",
                        path.display()
                    )))
                })?;
            }
        }
        Ok(())
    }

    fn remove_legacy_store_file(&self) -> Result<()> {
        if self.file_path.exists() {
            fs::remove_file(&self.file_path).map_err(|e| {
                BifrostError::Io(std::io::Error::other(format!(
                    "remove {}: {e}",
                    self.file_path.display()
                )))
            })?;
            tracing::warn!(
                path = %self.file_path.display(),
                "removed legacy remote invoke call history store"
            );
        }
        Ok(())
    }

    fn read_client_meta(
        &self,
        relay_url: &str,
        client_instance_id: &str,
    ) -> Result<CallHistoryClientMeta> {
        let path = self.client_meta_path(relay_url, client_instance_id);
        if !path.exists() {
            return Ok(CallHistoryClientMeta {
                version: CALL_HISTORY_STORE_VERSION,
                line_count: 0,
            });
        }
        let content = fs::read_to_string(&path).map_err(|e| {
            BifrostError::Io(std::io::Error::other(format!(
                "read {}: {e}",
                path.display()
            )))
        })?;
        match serde_json::from_str::<CallHistoryClientMeta>(&content) {
            Ok(meta) if meta.version == CALL_HISTORY_STORE_VERSION => Ok(meta),
            Ok(_) | Err(_) => {
                let _ = fs::remove_file(&path);
                Ok(CallHistoryClientMeta {
                    version: CALL_HISTORY_STORE_VERSION,
                    line_count: 0,
                })
            }
        }
    }

    fn write_client_meta(
        &self,
        relay_url: &str,
        client_instance_id: &str,
        meta: &CallHistoryClientMeta,
    ) -> Result<()> {
        fs::create_dir_all(&self.dir_path).map_err(|e| {
            BifrostError::Io(std::io::Error::other(format!(
                "mkdir {}: {e}",
                self.dir_path.display()
            )))
        })?;
        let path = self.client_meta_path(relay_url, client_instance_id);
        let content = serde_json::to_string(meta)
            .map_err(|e| BifrostError::Config(format!("serialize call history metadata: {e}")))?;
        fs::write(&path, content).map_err(|e| {
            BifrostError::Io(std::io::Error::other(format!(
                "write {}: {e}",
                path.display()
            )))
        })
    }

    pub(crate) fn client_log_path(&self, relay_url: &str, client_instance_id: &str) -> PathBuf {
        self.dir_path.join(format!(
            "{}.jsonl",
            history_client_key(relay_url, client_instance_id)
        ))
    }

    fn client_meta_path(&self, relay_url: &str, client_instance_id: &str) -> PathBuf {
        self.dir_path.join(format!(
            "{}.meta.json",
            history_client_key(relay_url, client_instance_id)
        ))
    }
}

fn history_client_key(relay_url: &str, client_instance_id: &str) -> String {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    relay_url.hash(&mut hasher);
    client_instance_id.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

fn upsert_latest_entry(
    latest_by_call: &mut HashMap<String, PersistedCallHistoryEntry>,
    entry: PersistedCallHistoryEntry,
) {
    latest_by_call
        .entry(entry.call_id.clone())
        .and_modify(|existing| {
            if entry.updated_at >= existing.updated_at {
                *existing = entry.clone();
            }
        })
        .or_insert(entry);
}

fn prune_and_sanitize_client_entries(
    entries: &mut Vec<PersistedCallHistoryEntry>,
    max_records: usize,
    retention_days: u32,
) -> bool {
    let before = entries.len();
    let mut changed = false;
    for entry in entries.iter_mut() {
        changed |= sanitize_call_for_history(&mut entry.call);
    }
    let cutoff = now_millis().saturating_sub(retention_days as u64 * 24 * 60 * 60 * 1000);
    entries.retain(|entry| entry.call.started_at >= cutoff);
    if entries.len() > max_records {
        entries.sort_by_key(|entry| entry.call.started_at);
        let remove_count = entries.len() - max_records;
        entries.drain(0..remove_count);
    }
    changed || before != entries.len()
}

fn effective_max_records(configured: usize) -> usize {
    configured.clamp(1, CALL_HISTORY_HARD_MAX_RECORDS)
}

fn truncate_history_string(value: &mut String) -> bool {
    let char_count = value.chars().count();
    if char_count <= CALL_HISTORY_COMMAND_TEXT_LIMIT {
        return false;
    }
    let suffix = format!("…({char_count})");
    let text_budget = CALL_HISTORY_COMMAND_TEXT_LIMIT.saturating_sub(suffix.len());
    *value = format!(
        "{}{}",
        value.chars().take(text_budget).collect::<String>(),
        suffix
    );
    true
}

fn truncate_history_option(value: &mut Option<String>) -> bool {
    value.as_mut().map(truncate_history_string).unwrap_or(false)
}

fn sanitize_history_json_string(value: &mut String) -> bool {
    let Ok(mut json_value) = serde_json::from_str::<Value>(value) else {
        return truncate_history_string(value);
    };
    if !sanitize_history_json_value(&mut json_value) {
        return false;
    }
    match serde_json::to_string(&json_value) {
        Ok(sanitized) => {
            *value = sanitized;
            true
        }
        Err(_) => truncate_history_string(value),
    }
}

fn sanitize_history_json_option(value: &mut Option<String>) -> bool {
    value
        .as_mut()
        .map(sanitize_history_json_string)
        .unwrap_or(false)
}

fn sanitize_history_json_value(value: &mut Value) -> bool {
    match value {
        Value::String(text) => truncate_history_string(text),
        Value::Array(items) => {
            let mut changed = false;
            for item in items {
                changed |= sanitize_history_json_value(item);
            }
            changed
        }
        Value::Object(map) => {
            let mut changed = false;
            for item in map.values_mut() {
                changed |= sanitize_history_json_value(item);
            }
            changed
        }
        _ => false,
    }
}

fn truncate_history_vec(value: &mut Option<Vec<String>>) -> bool {
    let mut changed = false;
    if let Some(items) = value {
        for item in items {
            changed |= truncate_history_string(item);
        }
    }
    changed
}

fn truncate_history_env(value: &mut Option<BTreeMap<String, String>>) -> bool {
    let Some(env) = value else {
        return false;
    };
    let mut changed = false;
    let mut truncated = BTreeMap::new();
    for (mut key, mut item_value) in std::mem::take(env) {
        changed |= truncate_history_string(&mut key);
        changed |= truncate_history_string(&mut item_value);
        truncated.insert(key, item_value);
    }
    *env = truncated;
    changed
}

fn sanitize_query_for_history(query: &mut Option<CanonicalQueryCommand>) -> bool {
    let Some(query) = query else {
        return false;
    };
    let mut changed = false;
    match query {
        CanonicalQueryCommand::Search(args) => {
            changed |= truncate_history_string(&mut args.keyword);
            for value in &mut args.filters.protocols {
                changed |= truncate_history_string(value);
            }
            for value in &mut args.filters.status_ranges {
                changed |= truncate_history_string(value);
            }
            for value in &mut args.filters.content_types {
                changed |= truncate_history_string(value);
            }
            for value in &mut args.filters.client_ips {
                changed |= truncate_history_string(value);
            }
            for value in &mut args.filters.client_apps {
                changed |= truncate_history_string(value);
            }
            for value in &mut args.filters.domains {
                changed |= truncate_history_string(value);
            }
            for condition in &mut args.filters.conditions {
                changed |= truncate_history_string(&mut condition.field);
                changed |= truncate_history_string(&mut condition.operator);
                changed |= truncate_history_string(&mut condition.value);
            }
        }
        CanonicalQueryCommand::TrafficList(args) => {
            changed |= truncate_history_option(&mut args.method);
            changed |= truncate_history_option(&mut args.protocol);
            changed |= truncate_history_option(&mut args.host);
            changed |= truncate_history_option(&mut args.url);
            changed |= truncate_history_option(&mut args.path);
            changed |= truncate_history_option(&mut args.content_type);
            changed |= truncate_history_option(&mut args.client_ip);
            changed |= truncate_history_option(&mut args.client_app);
        }
        CanonicalQueryCommand::TrafficGet(args) => {
            changed |= truncate_history_string(&mut args.id);
        }
        CanonicalQueryCommand::TrafficClear(args) => {
            if let Some(ids) = &mut args.ids {
                for id in ids {
                    changed |= truncate_history_string(id);
                }
            }
        }
    }
    changed
}

pub(super) fn sanitize_call_for_history(call: &mut CallInfo) -> bool {
    let mut changed = false;
    changed |= truncate_history_string(&mut call.command_summary.command_preview);
    changed |= sanitize_history_json_option(&mut call.command_summary.masked_args_json);
    changed |= truncate_history_string(&mut call.command.command);
    changed |= sanitize_history_json_option(&mut call.command.args_json);
    changed |= sanitize_query_for_history(&mut call.command.query);
    changed |= truncate_history_option(&mut call.command.policy_id);
    changed |= truncate_history_vec(&mut call.command.argv);
    changed |= truncate_history_option(&mut call.command.shell);
    changed |= truncate_history_option(&mut call.command.command_text);
    changed |= truncate_history_option(&mut call.command.cwd);
    changed |= truncate_history_env(&mut call.command.env);
    changed |= truncate_history_option(&mut call.policy_id);
    changed
}

#[cfg(test)]
pub(super) fn finalize_non_terminal_restored_calls(
    history: &mut std::collections::VecDeque<CallInfo>,
    now_millis: u64,
) -> usize {
    let mut finalized = 0;
    for call in history {
        if is_terminal_call_status(call.status) {
            continue;
        }
        call.status = CallStatus::Failed;
        call.exit_code.get_or_insert(-1);
        call.ended_at.get_or_insert(now_millis);
        call.duration_ms
            .get_or_insert(now_millis.saturating_sub(call.started_at));
        finalized += 1;
    }
    finalized
}

#[cfg(test)]
fn is_terminal_call_status(status: CallStatus) -> bool {
    matches!(
        status,
        CallStatus::Completed | CallStatus::Failed | CallStatus::Cancelled | CallStatus::Timeout
    )
}

pub(super) fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use bifrost_command::{CanonicalQueryCommand, SearchArgs};
    use std::collections::VecDeque;
    use tempfile::TempDir;

    use super::super::types::{
        AuthMethod, CommandKind, CommandSummary, FileAccessScope, RemoteCommand,
    };

    fn make_call_info(call_id: &str) -> CallInfo {
        CallInfo {
            call_id: call_id.to_string(),
            grant_id: "grant-1".to_string(),
            pairing_id: None,
            client_instance_id: "test-instance".to_string(),
            caller_fingerprint: "test-fp".to_string(),
            auth_method: AuthMethod::PairCode,
            command_kind: CommandKind::QueryReadonly,
            status: CallStatus::Streaming,
            command_summary: CommandSummary {
                command_preview: "status".to_string(),
                ..Default::default()
            },
            command: RemoteCommand {
                kind: CommandKind::QueryReadonly,
                command: "status".to_string(),
                args_json: None,
                query: None,
                policy_id: None,
                exec_mode: None,
                argv: None,
                shell: None,
                command_text: None,
                cwd: None,
                env: None,
                stdin_mode: None,
                timeout_ms: None,
                login: false,
                pty: None,
                output_mode: None,
                caller_fingerprint: None,
                ssh_fingerprint: None,
                file_access: FileAccessScope::default(),
                grant_id: None,
            },
            source_ip: None,
            caller_display_name: Some("TestCaller".to_string()),
            payload_digest: None,
            stdout_digest: None,
            stderr_digest: None,
            exit_code: None,
            started_at: 1000,
            ended_at: None,
            duration_ms: None,
            bytes_in: None,
            bytes_out: None,
            ssh_key_id: None,
            ssh_key_fingerprint: None,
            policy_id: None,
            exec_mode: None,
            output_mode: None,
            pty_enabled: None,
        }
    }

    #[test]
    fn test_call_history_store_prunes_by_retention_and_max_records() {
        let temp = TempDir::new().unwrap();
        let store = CallHistoryStore::new(temp.path());
        let now = now_millis();
        let old = now - 8 * 24 * 60 * 60 * 1000;
        let mut old_call = make_call_info("old");
        old_call.started_at = old;
        let mut mid_call = make_call_info("mid");
        mid_call.started_at = now;
        let mut new_call = make_call_info("new");
        new_call.started_at = now + 1;

        store
            .upsert("https://relay", "client", &old_call, 100, 30)
            .unwrap();
        store
            .upsert("https://relay", "client", &mid_call, 2, 7)
            .unwrap();
        store
            .upsert("https://relay", "client", &new_call, 2, 7)
            .unwrap();

        let calls = store
            .load_for_client("https://relay", "client", 1, 7)
            .unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].call_id, "new");
    }

    #[test]
    fn test_call_history_store_clear_for_client_removes_only_current_client() {
        let temp = TempDir::new().unwrap();
        let store = CallHistoryStore::new(temp.path());
        let now = now_millis();
        let mut call_a = make_call_info("a");
        call_a.started_at = now;
        let mut call_b = make_call_info("b");
        call_b.started_at = now;

        store
            .upsert("https://relay", "client-a", &call_a, 100, 7)
            .unwrap();
        store
            .upsert("https://relay", "client-b", &call_b, 100, 7)
            .unwrap();

        assert_eq!(
            store.clear_for_client("https://relay", "client-a").unwrap(),
            1
        );
        assert!(store
            .load_for_client("https://relay", "client-a", 100, 7)
            .unwrap()
            .is_empty());
        assert_eq!(
            store
                .load_for_client("https://relay", "client-b", 100, 7)
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn test_call_history_store_truncates_command_fields_before_persisting() {
        let temp = TempDir::new().unwrap();
        let store = CallHistoryStore::new(temp.path());
        let original_len = CALL_HISTORY_COMMAND_TEXT_LIMIT + 50;
        let long_value = "x".repeat(original_len);
        let expected_suffix = format!("…({original_len})");
        let mut call = make_call_info("long");
        call.started_at = now_millis();
        call.command_summary.command_preview = long_value.clone();
        call.command_summary.masked_args_json =
            Some(serde_json::json!({ "keyword": long_value.clone() }).to_string());
        call.command.command = long_value.clone();
        call.command.args_json =
            Some(serde_json::json!({ "keyword": long_value.clone() }).to_string());
        call.command.argv = Some(vec![long_value.clone()]);
        call.command.shell = Some(long_value.clone());
        call.command.command_text = Some(long_value.clone());
        call.command.cwd = Some(long_value.clone());
        call.command.env = Some(BTreeMap::from([(long_value.clone(), long_value.clone())]));
        call.command.query = Some(CanonicalQueryCommand::Search(SearchArgs {
            keyword: long_value.clone(),
            ..SearchArgs::default()
        }));

        store
            .upsert("https://relay", "client", &call, 100, 7)
            .unwrap();

        let calls = store
            .load_for_client("https://relay", "client", 100, 7)
            .unwrap();
        let persisted = &calls[0];

        // 截断后总长度不超过限制
        assert!(
            persisted.command.command.chars().count() <= CALL_HISTORY_COMMAND_TEXT_LIMIT,
            "command too long: {}",
            persisted.command.command.chars().count()
        );
        // 截断后包含原始长度后缀
        assert!(
            persisted.command.command.ends_with(&expected_suffix),
            "command missing suffix: {}",
            persisted.command.command
        );

        let masked_args_json: Value =
            serde_json::from_str(persisted.command_summary.masked_args_json.as_ref().unwrap())
                .unwrap();
        let masked_keyword = masked_args_json["keyword"].as_str().unwrap();
        assert!(masked_keyword.chars().count() <= CALL_HISTORY_COMMAND_TEXT_LIMIT);
        assert!(masked_keyword.ends_with(&expected_suffix));

        let args_json: Value =
            serde_json::from_str(persisted.command.args_json.as_ref().unwrap()).unwrap();
        let args_keyword = args_json["keyword"].as_str().unwrap();
        assert!(args_keyword.chars().count() <= CALL_HISTORY_COMMAND_TEXT_LIMIT);
        assert!(args_keyword.ends_with(&expected_suffix));

        match persisted.command.query.as_ref().unwrap() {
            CanonicalQueryCommand::Search(args) => {
                assert!(args.keyword.chars().count() <= CALL_HISTORY_COMMAND_TEXT_LIMIT);
                assert!(args.keyword.ends_with(&expected_suffix));
            }
            other => panic!("unexpected query command: {other:?}"),
        }

        // 原始超长字符串不应出现在存储文件中
        let raw_store =
            std::fs::read_to_string(store.client_log_path("https://relay", "client")).unwrap();
        assert!(!raw_store.contains(&long_value));
    }

    #[test]
    fn test_call_history_store_removes_legacy_json_without_migrating() {
        let temp = TempDir::new().unwrap();
        let store = CallHistoryStore::new(temp.path());
        let legacy_path = temp.path().join("admin").join(CALL_HISTORY_STORE_FILE);
        std::fs::create_dir_all(legacy_path.parent().unwrap()).unwrap();
        std::fs::write(
            &legacy_path,
            serde_json::json!({
                "version": CALL_HISTORY_STORE_VERSION,
                "entries": [{
                    "call_id": "legacy",
                    "relay_url": "https://relay",
                    "client_instance_id": "client",
                    "call": make_call_info("legacy"),
                    "updated_at": now_millis()
                }]
            })
            .to_string(),
        )
        .unwrap();

        let page = store
            .load_page_for_client("https://relay", "client", 100, 7, 100, None)
            .unwrap();

        assert!(page.calls.is_empty());
        assert!(!legacy_path.exists());
    }

    #[test]
    fn test_call_history_store_compacts_bad_jsonl_lines_and_caps_records() {
        let temp = TempDir::new().unwrap();
        let store = CallHistoryStore::new(temp.path());
        let now = now_millis();
        for i in 0..4 {
            let mut call = make_call_info(&format!("call-{i}"));
            call.started_at = now + i;
            store
                .upsert("https://relay", "client", &call, 3, 7)
                .unwrap();
        }
        let path = store.client_log_path("https://relay", "client");
        {
            use std::io::Write;
            let mut file = std::fs::OpenOptions::new()
                .append(true)
                .open(&path)
                .unwrap();
            writeln!(file, "{{bad json").unwrap();
        }

        let page = store
            .load_page_for_client("https://relay", "client", 3, 7, 10, None)
            .unwrap();

        assert_eq!(page.calls.len(), 3);
        assert_eq!(page.calls[0].call_id, "call-3");
        assert_eq!(page.calls[2].call_id, "call-1");
        let compacted = std::fs::read_to_string(&path).unwrap();
        assert!(!compacted.contains("{bad json"));
        assert_eq!(compacted.lines().count(), 3);
    }

    #[test]
    fn test_call_history_store_hard_caps_configured_max_records_at_1000() {
        let temp = TempDir::new().unwrap();
        let store = CallHistoryStore::new(temp.path());
        let now = now_millis();
        for i in 0..1001 {
            let mut call = make_call_info(&format!("call-{i}"));
            call.started_at = now + i;
            store
                .upsert("https://relay", "client", &call, 5_000, 7)
                .unwrap();
        }

        let page = store
            .load_page_for_client("https://relay", "client", 5_000, 7, 2_000, None)
            .unwrap();

        assert_eq!(page.calls.len(), CALL_HISTORY_HARD_MAX_RECORDS);
        assert_eq!(page.calls[0].call_id, "call-1000");
        assert_eq!(page.calls[999].call_id, "call-1");
        let compacted =
            std::fs::read_to_string(store.client_log_path("https://relay", "client")).unwrap();
        assert_eq!(compacted.lines().count(), CALL_HISTORY_HARD_MAX_RECORDS);
    }

    #[test]
    fn test_finalize_non_terminal_restored_calls_marks_streaming_failed() {
        let mut history: VecDeque<CallInfo> = VecDeque::new();
        history.push_back(make_call_info("streaming-call"));
        history.push_back({
            let mut completed = make_call_info("completed-call");
            completed.status = CallStatus::Completed;
            completed.exit_code = Some(0);
            completed
        });

        let finalized = finalize_non_terminal_restored_calls(&mut history, 1500);

        assert_eq!(finalized, 1);
        assert_eq!(history[0].status, CallStatus::Failed);
        assert_eq!(history[0].exit_code, Some(-1));
        assert_eq!(history[0].ended_at, Some(1500));
        assert_eq!(history[0].duration_ms, Some(500));
        assert_eq!(history[1].status, CallStatus::Completed);
    }
}
