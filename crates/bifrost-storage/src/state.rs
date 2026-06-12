use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::PathBuf;

use bifrost_core::{BifrostError, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RuntimeState {
    pub enabled_groups: HashSet<String>,
    pub disabled_rules: HashSet<String>,
    pub userpass_last_connected_at: HashMap<String, u64>,
}

impl RuntimeState {
    pub fn new() -> Self {
        Self::default()
    }
}

#[derive(Clone)]
pub struct StateManager {
    state: RuntimeState,
    state_file: PathBuf,
}

impl StateManager {
    pub fn new() -> Result<Self> {
        let state_file = crate::data_dir().join("state.json");
        Self::with_file(state_file)
    }

    pub fn with_file(state_file: PathBuf) -> Result<Self> {
        if let Some(parent) = state_file.parent() {
            fs::create_dir_all(parent)?;
        }

        let state = if state_file.exists() {
            let content = fs::read_to_string(&state_file)?;
            serde_json::from_str(&content).unwrap_or_default()
        } else {
            RuntimeState::default()
        };

        Ok(Self { state, state_file })
    }

    pub fn state(&self) -> &RuntimeState {
        &self.state
    }

    pub fn load(&mut self) -> Result<()> {
        if self.state_file.exists() {
            let content = fs::read_to_string(&self.state_file)?;
            self.state = serde_json::from_str(&content)
                .map_err(|e| BifrostError::Parse(format!("Failed to parse state: {}", e)))?;
        }
        Ok(())
    }

    pub fn save(&self) -> Result<()> {
        let content = serde_json::to_string_pretty(&self.state)
            .map_err(|e| BifrostError::Config(format!("Failed to serialize state: {}", e)))?;
        fs::write(&self.state_file, content)?;
        Ok(())
    }

    pub fn enable_group(&mut self, name: &str) {
        self.state.enabled_groups.insert(name.to_string());
    }

    pub fn disable_group(&mut self, name: &str) {
        self.state.enabled_groups.remove(name);
    }

    pub fn is_group_enabled(&self, name: &str) -> bool {
        self.state.enabled_groups.contains(name)
    }

    pub fn enabled_groups(&self) -> Vec<String> {
        let mut groups: Vec<_> = self.state.enabled_groups.iter().cloned().collect();
        groups.sort();
        groups
    }

    pub fn enable_rule(&mut self, name: &str) {
        self.state.disabled_rules.remove(name);
    }

    pub fn disable_rule(&mut self, name: &str) {
        self.state.disabled_rules.insert(name.to_string());
    }

    pub fn is_rule_enabled(&self, name: &str) -> bool {
        !self.state.disabled_rules.contains(name)
    }

    pub fn disabled_rules(&self) -> Vec<String> {
        let mut rules: Vec<_> = self.state.disabled_rules.iter().cloned().collect();
        rules.sort();
        rules
    }

    pub fn reset(&mut self) {
        self.state = RuntimeState::default();
    }

    pub fn userpass_last_connected_at(&self) -> &HashMap<String, u64> {
        &self.state.userpass_last_connected_at
    }

    pub fn set_userpass_last_connected_at(&mut self, username: &str, timestamp: u64) {
        self.state
            .userpass_last_connected_at
            .insert(username.to_string(), timestamp);
    }

    pub fn replace_userpass_last_connected_at(&mut self, timestamps: HashMap<String, u64>) {
        self.state.userpass_last_connected_at = timestamps;
    }

    pub fn remove_userpass_last_connected_at(&mut self, username: &str) {
        self.state.userpass_last_connected_at.remove(username);
    }
}

impl Default for StateManager {
    fn default() -> Self {
        Self::new().expect("Failed to create default StateManager")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use tempfile::TempDir;

    fn setup() -> (TempDir, StateManager) {
        let temp_dir = TempDir::new().unwrap();
        let state_file = temp_dir.path().join("state.json");
        let manager = StateManager::with_file(state_file).unwrap();
        (temp_dir, manager)
    }

    #[test]
    fn test_enable_disable_group() {
        let (_temp_dir, mut manager) = setup();

        manager.enable_group("group1");
        assert!(manager.is_group_enabled("group1"));

        manager.disable_group("group1");
        assert!(!manager.is_group_enabled("group1"));
    }

    #[test]
    fn test_enable_disable_rule() {
        let (_temp_dir, mut manager) = setup();

        assert!(manager.is_rule_enabled("rule1"));

        manager.disable_rule("rule1");
        assert!(!manager.is_rule_enabled("rule1"));

        manager.enable_rule("rule1");
        assert!(manager.is_rule_enabled("rule1"));
    }

    #[test]
    fn test_persistence() {
        let temp_dir = TempDir::new().unwrap();
        let state_file = temp_dir.path().join("state.json");

        {
            let mut manager = StateManager::with_file(state_file.clone()).unwrap();
            manager.enable_group("group1");
            manager.disable_rule("rule1");
            manager.save().unwrap();
        }

        {
            let manager = StateManager::with_file(state_file).unwrap();
            assert!(manager.is_group_enabled("group1"));
            assert!(!manager.is_rule_enabled("rule1"));
        }
    }

    #[test]
    fn test_enabled_groups() {
        let (_temp_dir, mut manager) = setup();
        manager.enable_group("group2");
        manager.enable_group("group1");
        manager.enable_group("group3");

        let groups = manager.enabled_groups();
        assert_eq!(groups, vec!["group1", "group2", "group3"]);
    }

    #[test]
    fn test_disabled_rules() {
        let (_temp_dir, mut manager) = setup();
        manager.disable_rule("rule2");
        manager.disable_rule("rule1");
        manager.disable_rule("rule3");

        let rules = manager.disabled_rules();
        assert_eq!(rules, vec!["rule1", "rule2", "rule3"]);
    }

    #[test]
    fn test_reset() {
        let (_temp_dir, mut manager) = setup();
        manager.enable_group("group1");
        manager.disable_rule("rule1");

        manager.reset();

        assert!(manager.enabled_groups().is_empty());
        assert!(manager.disabled_rules().is_empty());
    }

    #[test]
    fn test_load_and_save() {
        let temp_dir = TempDir::new().unwrap();
        let state_file = temp_dir.path().join("state.json");

        let mut manager = StateManager::with_file(state_file.clone()).unwrap();
        manager.enable_group("group1");
        manager.save().unwrap();

        manager.enable_group("group2");
        assert!(manager.is_group_enabled("group2"));

        manager.load().unwrap();
        assert!(!manager.is_group_enabled("group2"));
        assert!(manager.is_group_enabled("group1"));
    }

    #[test]
    fn test_multiple_groups() {
        let (_temp_dir, mut manager) = setup();

        manager.enable_group("api");
        manager.enable_group("frontend");
        manager.enable_group("backend");

        assert!(manager.is_group_enabled("api"));
        assert!(manager.is_group_enabled("frontend"));
        assert!(manager.is_group_enabled("backend"));
        assert!(!manager.is_group_enabled("other"));
    }

    #[test]
    fn test_multiple_rules() {
        let (_temp_dir, mut manager) = setup();

        manager.disable_rule("rule_a");
        manager.disable_rule("rule_b");

        assert!(!manager.is_rule_enabled("rule_a"));
        assert!(!manager.is_rule_enabled("rule_b"));
        assert!(manager.is_rule_enabled("rule_c"));
    }

    #[test]
    fn test_state_access() {
        let (_temp_dir, mut manager) = setup();
        manager.enable_group("test_group");
        manager.disable_rule("test_rule");

        let state = manager.state();
        assert!(state.enabled_groups.contains("test_group"));
        assert!(state.disabled_rules.contains("test_rule"));
    }

    #[test]
    fn test_userpass_last_connected_at_crud() {
        let (_temp_dir, mut manager) = setup();

        assert!(manager.userpass_last_connected_at().is_empty());

        manager.set_userpass_last_connected_at("alice", 10);
        manager.set_userpass_last_connected_at("bob", 20);

        let timestamps = manager.userpass_last_connected_at();
        assert_eq!(timestamps.get("alice"), Some(&10));
        assert_eq!(timestamps.get("bob"), Some(&20));

        let mut replacement = HashMap::new();
        replacement.insert("carol".to_string(), 30);
        manager.replace_userpass_last_connected_at(replacement.clone());

        let after_replace = manager.userpass_last_connected_at();
        assert_eq!(after_replace, &replacement);

        manager.remove_userpass_last_connected_at("carol");
        assert!(manager.userpass_last_connected_at().is_empty());
    }
}
