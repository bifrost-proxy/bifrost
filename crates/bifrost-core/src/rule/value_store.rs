use std::collections::HashMap;
use std::sync::{Arc, RwLock};

pub trait ValueStore: Send + Sync {
    fn get(&self, key: &str) -> Option<String>;
    fn set(&mut self, key: &str, value: String);
    fn remove(&mut self, key: &str) -> Option<String>;
    fn list(&self) -> Vec<(String, String)>;
    fn contains(&self, key: &str) -> bool {
        self.get(key).is_some()
    }
    fn len(&self) -> usize {
        self.list().len()
    }
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
    fn as_hashmap(&self) -> HashMap<String, String> {
        self.list().into_iter().collect()
    }
}

#[derive(Debug, Clone, Default)]
pub struct MemoryValueStore {
    values: HashMap<String, String>,
}

impl MemoryValueStore {
    pub fn new() -> Self {
        Self {
            values: HashMap::new(),
        }
    }

    pub fn from_hashmap(values: HashMap<String, String>) -> Self {
        Self { values }
    }

    #[allow(clippy::should_implement_trait)]
    pub fn from_iter<I, K, V>(iter: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>,
    {
        Self {
            values: iter
                .into_iter()
                .map(|(k, v)| (k.into(), v.into()))
                .collect(),
        }
    }
}

impl ValueStore for MemoryValueStore {
    fn get(&self, key: &str) -> Option<String> {
        self.values.get(key).cloned()
    }

    fn set(&mut self, key: &str, value: String) {
        self.values.insert(key.to_string(), value);
    }

    fn remove(&mut self, key: &str) -> Option<String> {
        self.values.remove(key)
    }

    fn list(&self) -> Vec<(String, String)> {
        self.values
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }

    fn contains(&self, key: &str) -> bool {
        self.values.contains_key(key)
    }

    fn len(&self) -> usize {
        self.values.len()
    }
}

pub struct CompositeValueStore {
    layers: Vec<Arc<RwLock<dyn ValueStore>>>,
}

impl CompositeValueStore {
    pub fn new() -> Self {
        Self { layers: Vec::new() }
    }

    pub fn with_layer(mut self, store: Arc<RwLock<dyn ValueStore>>) -> Self {
        self.layers.push(store);
        self
    }

    pub fn add_layer(&mut self, store: Arc<RwLock<dyn ValueStore>>) {
        self.layers.push(store);
    }

    pub fn prepend_layer(&mut self, store: Arc<RwLock<dyn ValueStore>>) {
        self.layers.insert(0, store);
    }
}

impl Default for CompositeValueStore {
    fn default() -> Self {
        Self::new()
    }
}

impl ValueStore for CompositeValueStore {
    fn get(&self, key: &str) -> Option<String> {
        for layer in &self.layers {
            if let Ok(guard) = layer.read() {
                if let Some(value) = guard.get(key) {
                    return Some(value);
                }
            }
        }
        None
    }

    fn set(&mut self, key: &str, value: String) {
        if let Some(first) = self.layers.first() {
            if let Ok(mut guard) = first.write() {
                guard.set(key, value);
            }
        }
    }

    fn remove(&mut self, key: &str) -> Option<String> {
        if let Some(first) = self.layers.first() {
            if let Ok(mut guard) = first.write() {
                return guard.remove(key);
            }
        }
        None
    }

    fn list(&self) -> Vec<(String, String)> {
        let mut merged: HashMap<String, String> = HashMap::new();
        for layer in self.layers.iter().rev() {
            if let Ok(guard) = layer.read() {
                for (k, v) in guard.list() {
                    merged.insert(k, v);
                }
            }
        }
        merged.into_iter().collect()
    }
}

pub type SharedValueStore = Arc<RwLock<dyn ValueStore>>;

pub fn create_shared_store<S: ValueStore + 'static>(store: S) -> SharedValueStore {
    Arc::new(RwLock::new(store))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_memory_store_basic() {
        let mut store = MemoryValueStore::new();
        assert!(store.is_empty());

        store.set("key1", "value1".to_string());
        assert_eq!(store.get("key1"), Some("value1".to_string()));
        assert_eq!(store.len(), 1);

        store.set("key2", "value2".to_string());
        assert_eq!(store.len(), 2);

        let removed = store.remove("key1");
        assert_eq!(removed, Some("value1".to_string()));
        assert_eq!(store.len(), 1);
        assert!(!store.contains("key1"));
    }

    #[test]
    fn test_memory_store_from_hashmap() {
        let mut map = HashMap::new();
        map.insert("a".to_string(), "1".to_string());
        map.insert("b".to_string(), "2".to_string());

        let store = MemoryValueStore::from_hashmap(map);
        assert_eq!(store.get("a"), Some("1".to_string()));
        assert_eq!(store.get("b"), Some("2".to_string()));
    }

    #[test]
    fn test_composite_store_priority() {
        let mut high_priority = MemoryValueStore::new();
        high_priority.set("shared", "high".to_string());
        high_priority.set("high_only", "high_value".to_string());

        let mut low_priority = MemoryValueStore::new();
        low_priority.set("shared", "low".to_string());
        low_priority.set("low_only", "low_value".to_string());

        let composite = CompositeValueStore::new()
            .with_layer(create_shared_store(high_priority))
            .with_layer(create_shared_store(low_priority));

        assert_eq!(composite.get("shared"), Some("high".to_string()));
        assert_eq!(composite.get("high_only"), Some("high_value".to_string()));
        assert_eq!(composite.get("low_only"), Some("low_value".to_string()));
        assert_eq!(composite.get("nonexistent"), None);
    }

    #[test]
    fn test_as_hashmap() {
        let mut store = MemoryValueStore::new();
        store.set("key1", "value1".to_string());
        store.set("key2", "value2".to_string());

        let map = store.as_hashmap();
        assert_eq!(map.get("key1"), Some(&"value1".to_string()));
        assert_eq!(map.get("key2"), Some(&"value2".to_string()));
    }

    #[test]
    fn test_memory_store_from_iter() {
        let store = MemoryValueStore::from_iter([("a", "1"), ("b", "2")]);
        assert_eq!(store.get("a"), Some("1".to_string()));
        assert_eq!(store.get("b"), Some("2".to_string()));
        assert_eq!(store.len(), 2);
    }

    #[test]
    fn test_memory_store_default_is_empty() {
        let store = MemoryValueStore::default();
        assert!(store.is_empty());
        assert_eq!(store.len(), 0);
        assert!(store.list().is_empty());
    }

    #[test]
    fn test_composite_default_and_empty() {
        let composite = CompositeValueStore::default();
        assert!(composite.is_empty());
        assert_eq!(composite.len(), 0);
        assert_eq!(composite.get("x"), None);
        assert!(composite.list().is_empty());
        assert!(composite.as_hashmap().is_empty());
    }

    #[test]
    fn test_composite_add_and_prepend_layer() {
        let mut base = MemoryValueStore::new();
        base.set("shared", "base".to_string());
        base.set("base_only", "b".to_string());

        let mut top = MemoryValueStore::new();
        top.set("shared", "top".to_string());

        let mut composite = CompositeValueStore::new();
        composite.add_layer(create_shared_store(base));
        // prepend pushes the higher-priority layer to the front
        composite.prepend_layer(create_shared_store(top));

        assert_eq!(composite.get("shared"), Some("top".to_string()));
        assert_eq!(composite.get("base_only"), Some("b".to_string()));
        assert!(composite.contains("shared"));
        assert!(!composite.contains("missing"));
    }

    #[test]
    fn test_composite_set_remove_and_list() {
        let first = create_shared_store(MemoryValueStore::new());
        let second = create_shared_store(MemoryValueStore::from_iter([("only_second", "s")]));

        let mut composite = CompositeValueStore::new()
            .with_layer(first)
            .with_layer(second);

        // set writes to the first layer
        composite.set("k", "v".to_string());
        assert_eq!(composite.get("k"), Some("v".to_string()));

        // list merges all layers
        let map = composite.as_hashmap();
        assert_eq!(map.get("k"), Some(&"v".to_string()));
        assert_eq!(map.get("only_second"), Some(&"s".to_string()));
        assert_eq!(composite.len(), 2);

        // remove operates on the first layer
        assert_eq!(composite.remove("k"), Some("v".to_string()));
        assert_eq!(composite.get("k"), None);
        // removing a key that only lives in a non-first layer returns None
        assert_eq!(composite.remove("only_second"), None);
    }

    #[test]
    fn test_composite_empty_set_remove_noop() {
        let mut composite = CompositeValueStore::new();
        // no layers: set is a no-op, remove returns None
        composite.set("k", "v".to_string());
        assert_eq!(composite.get("k"), None);
        assert_eq!(composite.remove("k"), None);
    }
}
