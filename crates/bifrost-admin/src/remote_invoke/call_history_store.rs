use std::collections::{BTreeMap, VecDeque};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use bifrost_command::CanonicalQueryCommand;
use bifrost_core::{BifrostError, Result};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::types::{CallInfo, CallStatus, CommandSummary, RemoteCommand};

const CALL_HISTORY_STORE_FILE: &str = "remote_invoke_call_history.json";
const CALL_HISTORY_STORE_VERSION: u32 = 2;
const CALL_HISTORY_COMMAND_TEXT_LIMIT: usize = 120;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CallHistoryStoreFile {
    version: u32,
    entries: Vec<PersistedCallHistoryEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PersistedCallHistoryEntry {
    call_id: String,
    relay_url: String,
    client_instance_id: String,
    call: CallInfo,
    updated_at: u64,
}

pub(crate) struct CallHistoryStore {
    file_path: PathBuf,
    lock: Mutex<()>,
}

impl CallHistoryStore {
    pub(crate) fn new(data_dir: &std::path::Path) -> Self {
        Self {
            file_path: data_dir.join("admin").join(CALL_HISTORY_STORE_FILE),
            lock: Mutex::new(()),
        }
    }

    pub(crate) fn load_for_client(
        &self,
        relay_url: &str,
        client_instance_id: &str,
        max_records: usize,
        retention_days: u32,
    ) -> Result<VecDeque<CallInfo>> {
        let _guard = self.lock.lock();
        let mut file = self.read_store_file()?;
        let before = file.entries.len();
        prune_call_history_entries(
            &mut file.entries,
            relay_url,
            client_instance_id,
            max_records,
            retention_days,
        );
        let mut sanitized = false;
        for entry in &mut file.entries {
            if entry.relay_url == relay_url
                && entry.client_instance_id == client_instance_id
                && sanitize_call_for_history(&mut entry.call)
            {
                sanitized = true;
            }
        }
        if before != file.entries.len() || sanitized {
            self.write_store_file(&file)?;
        }
        let mut calls = file
            .entries
            .into_iter()
            .filter(|entry| {
                entry.relay_url == relay_url && entry.client_instance_id == client_instance_id
            })
            .map(|entry| entry.call)
            .collect::<Vec<_>>();
        calls.sort_by_key(|call| call.started_at);
        Ok(calls.into())
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
        let mut file = self.read_store_file()?;
        file.entries.retain(|entry| {
            !(entry.relay_url == relay_url
                && entry.client_instance_id == client_instance_id
                && entry.call_id == call.call_id)
        });
        let mut call = call.clone();
        sanitize_call_for_history(&mut call);
        file.entries.push(PersistedCallHistoryEntry {
            call_id: call.call_id.clone(),
            relay_url: relay_url.to_string(),
            client_instance_id: client_instance_id.to_string(),
            call,
            updated_at: now_millis(),
        });
        prune_call_history_entries(
            &mut file.entries,
            relay_url,
            client_instance_id,
            max_records,
            retention_days,
        );
        self.write_store_file(&file)
    }

    pub(crate) fn clear_for_client(
        &self,
        relay_url: &str,
        client_instance_id: &str,
    ) -> Result<usize> {
        let _guard = self.lock.lock();
        let mut file = self.read_store_file()?;
        let before = file.entries.len();
        file.entries.retain(|entry| {
            !(entry.relay_url == relay_url && entry.client_instance_id == client_instance_id)
        });
        let removed = before - file.entries.len();
        if removed > 0 {
            self.write_store_file(&file)?;
        }
        Ok(removed)
    }

    fn read_store_file(&self) -> Result<CallHistoryStoreFile> {
        if !self.file_path.exists() {
            return Ok(CallHistoryStoreFile {
                version: CALL_HISTORY_STORE_VERSION,
                entries: Vec::new(),
            });
        }
        let content = std::fs::read_to_string(&self.file_path).map_err(|e| {
            BifrostError::Io(std::io::Error::other(format!(
                "read {}: {e}",
                self.file_path.display()
            )))
        })?;
        match serde_json::from_str::<CallHistoryStoreFile>(&content) {
            Ok(file) if file.version == CALL_HISTORY_STORE_VERSION => Ok(file),
            Ok(file) => {
                self.reset_store_file()?;
                Err(BifrostError::Config(format!(
                    "reset incompatible call history store version {}",
                    file.version
                )))
            }
            Err(e) => {
                self.reset_store_file()?;
                Err(BifrostError::Config(format!(
                    "reset unreadable call history store {}: {e}",
                    self.file_path.display()
                )))
            }
        }
    }

    fn write_store_file(&self, file: &CallHistoryStoreFile) -> Result<()> {
        if let Some(parent) = self.file_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                BifrostError::Io(std::io::Error::other(format!(
                    "mkdir {}: {e}",
                    parent.display()
                )))
            })?;
        }
        let content = serde_json::to_string_pretty(file)
            .map_err(|e| BifrostError::Config(format!("serialize call history store: {e}")))?;
        std::fs::write(&self.file_path, content).map_err(|e| {
            BifrostError::Io(std::io::Error::other(format!(
                "write {}: {e}",
                self.file_path.display()
            )))
        })?;
        Ok(())
    }

    fn reset_store_file(&self) -> Result<()> {
        if self.file_path.exists() {
            std::fs::remove_file(&self.file_path).map_err(|e| {
                BifrostError::Io(std::io::Error::other(format!(
                    "remove {}: {e}",
                    self.file_path.display()
                )))
            })?;
        }
        Ok(())
    }
}

fn prune_call_history_entries(
    entries: &mut Vec<PersistedCallHistoryEntry>,
    relay_url: &str,
    client_instance_id: &str,
    max_records: usize,
    retention_days: u32,
) {
    let cutoff = now_millis().saturating_sub(retention_days as u64 * 24 * 60 * 60 * 1000);
    entries.retain(|entry| {
        entry.relay_url != relay_url
            || entry.client_instance_id != client_instance_id
            || entry.call.started_at >= cutoff
    });
    let mut indexes = entries
        .iter()
        .enumerate()
        .filter(|(_, entry)| {
            entry.relay_url == relay_url && entry.client_instance_id == client_instance_id
        })
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    if indexes.len() <= max_records {
        return;
    }
    indexes.sort_by_key(|index| entries[*index].call.started_at);
    let remove_count = indexes.len() - max_records;
    let mut remove_indexes = indexes.into_iter().take(remove_count).collect::<Vec<_>>();
    remove_indexes.sort_unstable_by(|a, b| b.cmp(a));
    for index in remove_indexes {
        entries.remove(index);
    }
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

pub(super) fn finalize_non_terminal_restored_calls(
    history: &mut VecDeque<CallInfo>,
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
    use tempfile::TempDir;

    use super::super::types::{AuthMethod, CommandKind};

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
                pty: None,
                output_mode: None,
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
            std::fs::read_to_string(temp.path().join("admin").join(CALL_HISTORY_STORE_FILE))
                .unwrap();
        assert!(!raw_store.contains(&long_value));
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
