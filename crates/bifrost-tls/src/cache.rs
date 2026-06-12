use lru::LruCache;
use parking_lot::Mutex;
use rustls::sign::CertifiedKey;
use rustls::ServerConfig;
use std::num::NonZeroUsize;
use std::sync::Arc;

const DEFAULT_CACHE_SIZE: usize = 1000;

#[derive(Debug)]
pub struct CertCache {
    cache: Mutex<LruCache<String, Arc<CertifiedKey>>>,
}

impl CertCache {
    pub fn new() -> Self {
        Self::with_capacity(DEFAULT_CACHE_SIZE)
    }

    pub fn with_capacity(capacity: usize) -> Self {
        let cap =
            NonZeroUsize::new(capacity).unwrap_or(NonZeroUsize::new(DEFAULT_CACHE_SIZE).unwrap());
        Self {
            cache: Mutex::new(LruCache::new(cap)),
        }
    }

    pub fn get(&self, domain: &str) -> Option<Arc<CertifiedKey>> {
        self.cache.lock().get(domain).cloned()
    }

    pub fn insert(&self, domain: &str, cert: Arc<CertifiedKey>) {
        self.cache.lock().put(domain.to_string(), cert);
    }

    pub fn remove(&self, domain: &str) -> Option<Arc<CertifiedKey>> {
        self.cache.lock().pop(domain)
    }

    pub fn clear(&self) {
        self.cache.lock().clear();
    }

    pub fn len(&self) -> usize {
        self.cache.lock().len()
    }

    pub fn is_empty(&self) -> bool {
        self.cache.lock().is_empty()
    }

    pub fn capacity(&self) -> usize {
        self.cache.lock().cap().get()
    }
}

impl Default for CertCache {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct ServerConfigCacheKey {
    domain: String,
    alpn_protocols: Vec<Vec<u8>>,
}

impl ServerConfigCacheKey {
    fn new(domain: &str, alpn_protocols: &[Vec<u8>]) -> Self {
        Self {
            domain: domain.to_string(),
            alpn_protocols: alpn_protocols.to_vec(),
        }
    }
}

pub struct ServerConfigCache {
    cache: Mutex<LruCache<ServerConfigCacheKey, Arc<ServerConfig>>>,
}

impl std::fmt::Debug for ServerConfigCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ServerConfigCache")
            .field("len", &self.len())
            .field("capacity", &self.capacity())
            .finish()
    }
}

impl ServerConfigCache {
    pub fn new() -> Self {
        Self::with_capacity(DEFAULT_CACHE_SIZE)
    }

    pub fn with_capacity(capacity: usize) -> Self {
        let cap =
            NonZeroUsize::new(capacity).unwrap_or(NonZeroUsize::new(DEFAULT_CACHE_SIZE).unwrap());
        Self {
            cache: Mutex::new(LruCache::new(cap)),
        }
    }

    pub fn get(&self, domain: &str, alpn_protocols: &[Vec<u8>]) -> Option<Arc<ServerConfig>> {
        self.cache
            .lock()
            .get(&ServerConfigCacheKey::new(domain, alpn_protocols))
            .cloned()
    }

    pub fn insert(&self, domain: &str, alpn_protocols: &[Vec<u8>], config: Arc<ServerConfig>) {
        self.cache
            .lock()
            .put(ServerConfigCacheKey::new(domain, alpn_protocols), config);
    }

    pub fn clear(&self) {
        self.cache.lock().clear();
    }

    pub fn len(&self) -> usize {
        self.cache.lock().len()
    }

    pub fn is_empty(&self) -> bool {
        self.cache.lock().is_empty()
    }

    pub fn capacity(&self) -> usize {
        self.cache.lock().cap().get()
    }
}

impl Default for ServerConfigCache {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ca::generate_root_ca;
    use crate::config::TlsConfig as BifrostTlsConfig;
    use crate::dynamic::DynamicCertGenerator;
    use crate::init_crypto_provider;

    fn create_test_cert(domain: &str) -> Arc<CertifiedKey> {
        let ca = Arc::new(generate_root_ca().expect("Failed to generate CA"));
        let generator = DynamicCertGenerator::new(ca);
        Arc::new(
            generator
                .generate_for_domain(domain)
                .expect("Failed to generate cert"),
        )
    }

    fn create_test_server_config(domain: &str) -> Arc<ServerConfig> {
        init_crypto_provider();
        let cert = create_test_cert(domain);
        BifrostTlsConfig::build_server_config(cert.as_ref()).expect("Failed to build server config")
    }

    #[test]
    fn test_cache_new() {
        let cache = CertCache::new();
        assert_eq!(cache.capacity(), DEFAULT_CACHE_SIZE);
        assert!(cache.is_empty());
    }

    #[test]
    fn test_cache_with_capacity() {
        let cache = CertCache::with_capacity(500);
        assert_eq!(cache.capacity(), 500);
    }

    #[test]
    fn test_cache_insert_and_get() {
        let cache = CertCache::new();
        let cert = create_test_cert("example.com");

        cache.insert("example.com", cert.clone());
        let retrieved = cache.get("example.com");

        assert!(retrieved.is_some());
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn test_cache_miss() {
        let cache = CertCache::new();
        let result = cache.get("nonexistent.com");
        assert!(result.is_none());
    }

    #[test]
    fn test_cache_remove() {
        let cache = CertCache::new();
        let cert = create_test_cert("example.com");

        cache.insert("example.com", cert);
        assert_eq!(cache.len(), 1);

        let removed = cache.remove("example.com");
        assert!(removed.is_some());
        assert!(cache.is_empty());
    }

    #[test]
    fn test_cache_clear() {
        let cache = CertCache::new();
        cache.insert("example1.com", create_test_cert("example1.com"));
        cache.insert("example2.com", create_test_cert("example2.com"));

        assert_eq!(cache.len(), 2);
        cache.clear();
        assert!(cache.is_empty());
    }

    #[test]
    fn test_cache_lru_eviction() {
        let cache = CertCache::with_capacity(2);

        cache.insert("example1.com", create_test_cert("example1.com"));
        cache.insert("example2.com", create_test_cert("example2.com"));
        cache.insert("example3.com", create_test_cert("example3.com"));

        assert_eq!(cache.len(), 2);
        assert!(cache.get("example1.com").is_none());
        assert!(cache.get("example2.com").is_some());
        assert!(cache.get("example3.com").is_some());
    }

    #[test]
    fn test_server_config_cache_insert_and_get() {
        let cache = ServerConfigCache::new();
        let alpn = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
        let config = create_test_server_config("example.com");

        cache.insert("example.com", &alpn, config.clone());
        let retrieved = cache.get("example.com", &alpn);

        assert!(retrieved.is_some());
        assert!(Arc::ptr_eq(
            &config,
            &retrieved.expect("cached config should exist")
        ));
    }

    #[test]
    fn test_server_config_cache_is_alpn_specific() {
        let cache = ServerConfigCache::new();
        let h2 = vec![b"h2".to_vec()];
        let http1 = vec![b"http/1.1".to_vec()];
        let config = create_test_server_config("example.com");

        cache.insert("example.com", &h2, config);

        assert!(cache.get("example.com", &h2).is_some());
        assert!(cache.get("example.com", &http1).is_none());
    }

    #[test]
    fn test_cert_cache_default() {
        // Exercises `impl Default for CertCache`.
        let cache = CertCache::default();
        assert!(cache.is_empty());
        assert_eq!(cache.capacity(), DEFAULT_CACHE_SIZE);
    }

    #[test]
    fn test_cert_cache_with_capacity_zero_falls_back_to_default() {
        // capacity(0) must fall back to DEFAULT_CACHE_SIZE (NonZeroUsize guard).
        let cache = CertCache::with_capacity(0);
        assert_eq!(cache.capacity(), DEFAULT_CACHE_SIZE);
    }

    #[test]
    fn test_server_config_cache_default_and_capacity() {
        // Exercises `impl Default for ServerConfigCache`, `with_capacity`,
        // `capacity`, `is_empty`, `len`, and `clear`.
        let cache = ServerConfigCache::default();
        assert_eq!(cache.capacity(), DEFAULT_CACHE_SIZE);
        assert!(cache.is_empty());
        assert_eq!(cache.len(), 0);

        let alpn = vec![b"h2".to_vec()];
        cache.insert(
            "example.com",
            &alpn,
            create_test_server_config("example.com"),
        );
        assert!(!cache.is_empty());
        assert_eq!(cache.len(), 1);

        cache.clear();
        assert!(cache.is_empty());
    }

    #[test]
    fn test_server_config_cache_with_capacity_zero_falls_back_to_default() {
        let cache = ServerConfigCache::with_capacity(0);
        assert_eq!(cache.capacity(), DEFAULT_CACHE_SIZE);
    }

    #[test]
    fn test_server_config_cache_debug_impl() {
        // Exercises the custom `impl Debug for ServerConfigCache`.
        let cache = ServerConfigCache::with_capacity(7);
        let rendered = format!("{cache:?}");
        assert!(rendered.contains("ServerConfigCache"));
        assert!(rendered.contains("len"));
        assert!(rendered.contains("capacity"));
    }
}
