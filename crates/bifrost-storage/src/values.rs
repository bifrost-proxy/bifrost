use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::time::SystemTime;

use bifrost_core::{BifrostError, Result, ValueStore};
use chrono::{DateTime, Utc};

#[derive(Clone)]
pub struct ValuesStorage {
    base_dir: PathBuf,
    cache: HashMap<String, String>,
}

#[derive(Debug, Clone)]
pub struct ValueEntry {
    pub name: String,
    pub value: String,
    pub created_at: String,
    pub updated_at: String,
}

fn metadata_times(path: &PathBuf) -> (SystemTime, SystemTime) {
    let metadata = fs::metadata(path).ok();
    let updated_at = metadata
        .as_ref()
        .and_then(|item| item.modified().ok())
        .unwrap_or(SystemTime::UNIX_EPOCH);
    let created_at = metadata
        .as_ref()
        .and_then(|item| item.created().ok())
        .unwrap_or(updated_at);
    (created_at, updated_at)
}

impl ValuesStorage {
    pub fn new() -> Result<Self> {
        let base_dir = crate::data_dir().join("values");
        Self::with_dir(base_dir)
    }

    pub fn with_dir(dir: PathBuf) -> Result<Self> {
        fs::create_dir_all(&dir)?;
        // Stored values may contain secrets; restrict the directory to the
        // owner on Unix. Best-effort: log a warning on failure.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Err(e) = fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)) {
                tracing::warn!("failed to set 0700 permissions on {}: {e}", dir.display());
            }
        }
        let mut storage = Self {
            base_dir: dir,
            cache: HashMap::new(),
        };
        storage.load_cache()?;
        Ok(storage)
    }

    fn load_cache(&mut self) -> Result<()> {
        self.cache.clear();
        if !self.base_dir.exists() {
            return Ok(());
        }
        for entry in fs::read_dir(&self.base_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) == Some("txt") {
                if let Some(key) = path.file_stem().and_then(|s| s.to_str()) {
                    if let Ok(value) = fs::read_to_string(&path) {
                        self.cache.insert(key.to_string(), value);
                    }
                }
            }
        }
        Ok(())
    }

    fn value_path(&self, key: &str) -> PathBuf {
        let safe_key = key.replace(['/', '\\', ':'], "_");
        self.base_dir.join(format!("{}.txt", safe_key))
    }

    pub fn get_value(&self, key: &str) -> Option<String> {
        self.cache.get(key).cloned()
    }

    pub fn get_entry(&self, key: &str) -> Option<ValueEntry> {
        let value = self.cache.get(key)?.clone();
        let path = self.value_path(key);
        let (created_at, updated_at) = metadata_times(&path);
        Some(ValueEntry {
            name: key.to_string(),
            value,
            created_at: DateTime::<Utc>::from(created_at).to_rfc3339(),
            updated_at: DateTime::<Utc>::from(updated_at).to_rfc3339(),
        })
    }

    pub fn set_value(&mut self, key: &str, value: &str) -> Result<()> {
        let path = self.value_path(key);
        fs::write(&path, value)?;
        self.cache.insert(key.to_string(), value.to_string());
        Ok(())
    }

    pub fn remove_value(&mut self, key: &str) -> Result<()> {
        let path = self.value_path(key);
        if !path.exists() {
            return Err(BifrostError::NotFound(format!("Value '{}' not found", key)));
        }
        fs::remove_file(&path)?;
        self.cache.remove(key);
        Ok(())
    }

    pub fn list_keys(&self) -> Result<Vec<String>> {
        let mut keys: Vec<String> = self.cache.keys().cloned().collect();
        keys.sort();
        Ok(keys)
    }

    pub fn clear(&mut self) -> Result<()> {
        for entry in fs::read_dir(&self.base_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) == Some("txt") {
                fs::remove_file(&path)?;
            }
        }
        self.cache.clear();
        Ok(())
    }

    pub fn exists(&self, key: &str) -> bool {
        self.cache.contains_key(key)
    }

    pub fn refresh(&mut self) -> Result<()> {
        self.load_cache()
    }

    pub fn list_entries(&self) -> Result<Vec<ValueEntry>> {
        let mut entries: Vec<(SystemTime, ValueEntry)> = self
            .cache
            .keys()
            .filter_map(|key| {
                let entry = self.get_entry(key)?;
                let path = self.value_path(key);
                let (created_at, _) = metadata_times(&path);
                Some((created_at, entry))
            })
            .collect();
        entries.sort_by(|(left_time, left_entry), (right_time, right_entry)| {
            right_time
                .cmp(left_time)
                .then_with(|| left_entry.name.cmp(&right_entry.name))
        });
        Ok(entries.into_iter().map(|(_, entry)| entry).collect())
    }

    pub fn create(&mut self, name: &str, value: &str) -> Result<()> {
        if self.exists(name) {
            return Err(BifrostError::AlreadyExists(format!(
                "Value '{}' already exists",
                name
            )));
        }
        self.set_value(name, value)
    }

    pub fn update(&mut self, name: &str, value: &str) -> Result<()> {
        if !self.exists(name) {
            return Err(BifrostError::NotFound(format!(
                "Value '{}' not found",
                name
            )));
        }
        self.set_value(name, value)
    }

    pub fn base_dir(&self) -> &PathBuf {
        &self.base_dir
    }

    pub fn load_from_file(&mut self, path: &std::path::Path) -> Result<usize> {
        let content = fs::read_to_string(path)?;
        let mut count = 0;

        let extension = path.extension().and_then(|s| s.to_str()).unwrap_or("");

        match extension {
            "json" => {
                let map: HashMap<String, String> = serde_json::from_str(&content)
                    .map_err(|e| BifrostError::Parse(format!("Invalid JSON: {}", e)))?;
                for (k, v) in map {
                    self.set_value(&k, &v)?;
                    count += 1;
                }
            }
            "kv" | "env" => {
                for line in content.lines() {
                    let line = line.trim();
                    if line.is_empty() || line.starts_with('#') {
                        continue;
                    }
                    if let Some(eq_pos) = line.find('=') {
                        let key = line[..eq_pos].trim();
                        let value = line[eq_pos + 1..].trim();
                        if !key.is_empty() {
                            self.set_value(key, value)?;
                            count += 1;
                        }
                    }
                }
            }
            _ => {
                if let Some(name) = path.file_stem().and_then(|s| s.to_str()) {
                    self.set_value(name, content.trim())?;
                    count = 1;
                }
            }
        }

        Ok(count)
    }
}

impl ValueStore for ValuesStorage {
    fn get(&self, key: &str) -> Option<String> {
        self.cache.get(key).cloned()
    }

    fn set(&mut self, key: &str, value: String) {
        let path = self.value_path(key);
        if let Err(e) = fs::write(&path, &value) {
            tracing::warn!(
                "failed to persist value '{}' to {}: {e}",
                key,
                path.display()
            );
            return;
        }
        self.cache.insert(key.to_string(), value);
    }

    fn remove(&mut self, key: &str) -> Option<String> {
        let path = self.value_path(key);
        if path.exists() {
            let _ = fs::remove_file(&path);
        }
        self.cache.remove(key)
    }

    fn list(&self) -> Vec<(String, String)> {
        self.cache
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }

    fn contains(&self, key: &str) -> bool {
        self.cache.contains_key(key)
    }

    fn len(&self) -> usize {
        self.cache.len()
    }

    fn as_hashmap(&self) -> HashMap<String, String> {
        self.cache.clone()
    }
}

impl Default for ValuesStorage {
    fn default() -> Self {
        Self::new().expect("Failed to create default ValuesStorage")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn setup() -> (TempDir, ValuesStorage) {
        let temp_dir = TempDir::new().unwrap();
        let storage = ValuesStorage::with_dir(temp_dir.path().to_path_buf()).unwrap();
        (temp_dir, storage)
    }

    #[test]
    fn test_set_and_get() {
        let (_temp_dir, mut storage) = setup();
        storage.set_value("test_key", "test_value").unwrap();

        let value = storage.get_value("test_key");
        assert_eq!(value, Some("test_value".to_string()));
    }

    #[test]
    fn test_get_not_found() {
        let (_temp_dir, storage) = setup();
        let value = storage.get_value("nonexistent");
        assert_eq!(value, None);
    }

    #[test]
    fn test_list() {
        let (_temp_dir, mut storage) = setup();
        storage.set_value("key1", "value1").unwrap();
        storage.set_value("key2", "value2").unwrap();
        storage.set_value("key3", "value3").unwrap();

        let keys = storage.list_keys().unwrap();
        assert_eq!(keys, vec!["key1", "key2", "key3"]);
    }

    #[test]
    fn test_list_empty() {
        let (_temp_dir, storage) = setup();
        let keys = storage.list_keys().unwrap();
        assert!(keys.is_empty());
    }

    #[test]
    fn test_remove() {
        let (_temp_dir, mut storage) = setup();
        storage.set_value("test_key", "test_value").unwrap();
        assert!(storage.exists("test_key"));

        storage.remove_value("test_key").unwrap();
        assert!(!storage.exists("test_key"));
        assert_eq!(storage.get_value("test_key"), None);
    }

    #[test]
    fn test_remove_not_found() {
        let (_temp_dir, mut storage) = setup();
        let result = storage.remove_value("nonexistent");
        assert!(result.is_err());
    }

    #[test]
    fn test_clear() {
        let (_temp_dir, mut storage) = setup();
        storage.set_value("key1", "value1").unwrap();
        storage.set_value("key2", "value2").unwrap();

        storage.clear().unwrap();

        let keys = storage.list_keys().unwrap();
        assert!(keys.is_empty());
    }

    #[test]
    fn test_exists() {
        let (_temp_dir, mut storage) = setup();
        assert!(!storage.exists("test_key"));

        storage.set_value("test_key", "test_value").unwrap();
        assert!(storage.exists("test_key"));
    }

    #[test]
    fn test_overwrite() {
        let (_temp_dir, mut storage) = setup();
        storage.set_value("test_key", "value1").unwrap();
        storage.set_value("test_key", "value2").unwrap();

        let value = storage.get_value("test_key");
        assert_eq!(value, Some("value2".to_string()));
    }

    #[test]
    fn test_update_existing_value() {
        let (_temp_dir, mut storage) = setup();
        storage.create("test_key", "value1").unwrap();

        storage.update("test_key", "value2").unwrap();

        let value = storage.get_value("test_key");
        assert_eq!(value, Some("value2".to_string()));
    }

    #[test]
    fn test_update_missing_value() {
        let (_temp_dir, mut storage) = setup();

        let result = storage.update("missing", "value");

        assert!(matches!(result, Err(BifrostError::NotFound(_))));
    }

    #[test]
    fn test_cache_persistence() {
        let temp_dir = TempDir::new().unwrap();
        let dir = temp_dir.path().to_path_buf();

        {
            let mut storage = ValuesStorage::with_dir(dir.clone()).unwrap();
            storage.set_value("key1", "value1").unwrap();
            storage.set_value("key2", "value2").unwrap();
        }

        {
            let storage = ValuesStorage::with_dir(dir).unwrap();
            assert_eq!(storage.get_value("key1"), Some("value1".to_string()));
            assert_eq!(storage.get_value("key2"), Some("value2".to_string()));
        }
    }

    #[test]
    fn test_refresh() {
        let temp_dir = TempDir::new().unwrap();
        let dir = temp_dir.path().to_path_buf();
        let mut storage = ValuesStorage::with_dir(dir.clone()).unwrap();
        storage.set_value("key1", "value1").unwrap();

        fs::write(dir.join("key2.txt"), "value2").unwrap();

        storage.refresh().unwrap();
        assert_eq!(storage.get_value("key2"), Some("value2".to_string()));
    }

    #[test]
    fn test_list_entries_sorted_by_updated_at_desc() {
        let (_temp_dir, mut storage) = setup();
        storage.set_value("older", "value1").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        storage.set_value("newer", "value2").unwrap();

        let entries = storage.list_entries().unwrap();
        let names: Vec<_> = entries.into_iter().map(|entry| entry.name).collect();
        assert_eq!(names, vec!["newer", "older"]);
    }

    #[test]
    fn test_special_characters_in_key() {
        let (_temp_dir, mut storage) = setup();
        storage.set_value("path/to/key", "value").unwrap();

        assert!(storage.exists("path/to/key"));
        assert_eq!(storage.get_value("path/to/key"), Some("value".to_string()));
    }

    #[test]
    fn test_get_entry_present_and_absent() {
        let (_temp_dir, mut storage) = setup();
        storage.set_value("k", "v").unwrap();

        let entry = storage.get_entry("k").expect("entry should exist");
        assert_eq!(entry.name, "k");
        assert_eq!(entry.value, "v");
        // created_at / updated_at are rfc3339 strings containing a date separator.
        assert!(entry.created_at.contains('T'));
        assert!(entry.updated_at.contains('T'));

        assert!(storage.get_entry("missing").is_none());
    }

    #[test]
    fn test_base_dir_accessor() {
        let (temp_dir, storage) = setup();
        assert_eq!(storage.base_dir(), &temp_dir.path().to_path_buf());
    }

    #[test]
    fn test_create_already_exists() {
        let (_temp_dir, mut storage) = setup();
        storage.create("dup", "v1").unwrap();
        let result = storage.create("dup", "v2");
        assert!(matches!(result, Err(BifrostError::AlreadyExists(_))));
    }

    #[test]
    fn test_load_from_file_json() {
        let (temp_dir, mut storage) = setup();
        let file = temp_dir.path().join("import.json");
        fs::write(&file, r#"{"a":"1","b":"2"}"#).unwrap();

        let count = storage.load_from_file(&file).unwrap();
        assert_eq!(count, 2);
        assert_eq!(storage.get_value("a"), Some("1".to_string()));
        assert_eq!(storage.get_value("b"), Some("2".to_string()));
    }

    #[test]
    fn test_load_from_file_json_invalid() {
        let (temp_dir, mut storage) = setup();
        let file = temp_dir.path().join("bad.json");
        fs::write(&file, "{not valid json}").unwrap();

        let result = storage.load_from_file(&file);
        assert!(matches!(result, Err(BifrostError::Parse(_))));
    }

    #[test]
    fn test_load_from_file_kv_and_env() {
        let (temp_dir, mut storage) = setup();
        let file = temp_dir.path().join("import.kv");
        fs::write(
            &file,
            "# a comment\n\nKEY1 = val1\nKEY2=val2\nnoequals\n=novalue\n",
        )
        .unwrap();

        let count = storage.load_from_file(&file).unwrap();
        assert_eq!(count, 2);
        assert_eq!(storage.get_value("KEY1"), Some("val1".to_string()));
        assert_eq!(storage.get_value("KEY2"), Some("val2".to_string()));
        assert_eq!(storage.get_value("noequals"), None);

        let env_file = temp_dir.path().join("import.env");
        fs::write(&env_file, "FOO=bar\n").unwrap();
        let env_count = storage.load_from_file(&env_file).unwrap();
        assert_eq!(env_count, 1);
        assert_eq!(storage.get_value("FOO"), Some("bar".to_string()));
    }

    #[test]
    fn test_load_from_file_plain_text_fallback() {
        let (temp_dir, mut storage) = setup();
        let file = temp_dir.path().join("token.txt");
        fs::write(&file, "  secret-token  \n").unwrap();

        let count = storage.load_from_file(&file).unwrap();
        assert_eq!(count, 1);
        // Plain-text branch uses file_stem as key and trims the content.
        assert_eq!(storage.get_value("token"), Some("secret-token".to_string()));
    }

    #[test]
    fn test_value_store_trait_impl() {
        let (_temp_dir, mut storage) = setup();

        // set + get + contains + len
        ValueStore::set(&mut storage, "k1", "v1".to_string());
        ValueStore::set(&mut storage, "k2", "v2".to_string());
        assert_eq!(ValueStore::get(&storage, "k1"), Some("v1".to_string()));
        assert!(ValueStore::contains(&storage, "k2"));
        assert!(!ValueStore::contains(&storage, "absent"));
        assert_eq!(ValueStore::len(&storage), 2);

        // list + as_hashmap
        let mut listed = ValueStore::list(&storage);
        listed.sort();
        assert_eq!(
            listed,
            vec![
                ("k1".to_string(), "v1".to_string()),
                ("k2".to_string(), "v2".to_string())
            ]
        );
        let map = ValueStore::as_hashmap(&storage);
        assert_eq!(map.get("k1"), Some(&"v1".to_string()));

        // remove returns previous value, then None
        assert_eq!(
            ValueStore::remove(&mut storage, "k1"),
            Some("v1".to_string())
        );
        assert_eq!(ValueStore::remove(&mut storage, "k1"), None);
        assert_eq!(ValueStore::len(&storage), 1);
    }
}
