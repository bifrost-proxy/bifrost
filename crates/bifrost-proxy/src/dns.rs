use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::str::FromStr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bifrost_core::{BifrostError, Result};
use hickory_resolver::config::{ConnectionConfig, NameServerConfig, ResolverConfig, ResolverOpts};
use hickory_resolver::net::runtime::TokioRuntimeProvider;
use hickory_resolver::TokioResolver;
use tokio::sync::RwLock;
use tracing::{debug, info, instrument, warn};

const DEFAULT_CACHE_TTL_SECS: u64 = 300;
const DEFAULT_CACHE_CAPACITY: usize = 1000;

#[derive(Debug, Clone)]
struct DnsCacheEntry {
    ip: IpAddr,
    expires_at: Instant,
}

impl DnsCacheEntry {
    fn new(ip: IpAddr, ttl: Duration) -> Self {
        Self {
            ip,
            expires_at: Instant::now() + ttl,
        }
    }

    fn is_expired(&self) -> bool {
        Instant::now() >= self.expires_at
    }
}

pub struct DnsCache {
    entries: RwLock<HashMap<String, DnsCacheEntry>>,
    ttl: Duration,
    capacity: usize,
}

impl DnsCache {
    pub fn new(ttl_secs: u64, capacity: usize) -> Self {
        Self {
            entries: RwLock::new(HashMap::new()),
            ttl: Duration::from_secs(ttl_secs),
            capacity,
        }
    }

    fn make_key(host: &str, server: &str) -> String {
        format!("{}@{}", host, server)
    }

    pub async fn get(&self, host: &str, server: &str) -> Option<IpAddr> {
        let key = Self::make_key(host, server);
        let entries = self.entries.read().await;

        if let Some(entry) = entries.get(&key) {
            if !entry.is_expired() {
                debug!(
                    target: "bifrost_proxy::dns",
                    host = %host,
                    server = %server,
                    ip = %entry.ip,
                    "DNS cache hit"
                );
                return Some(entry.ip);
            }
        }
        None
    }

    pub async fn put(&self, host: &str, server: &str, ip: IpAddr) {
        let key = Self::make_key(host, server);
        let entry = DnsCacheEntry::new(ip, self.ttl);

        let mut entries = self.entries.write().await;

        if entries.len() >= self.capacity && !entries.contains_key(&key) {
            self.evict_expired_or_oldest(&mut entries);
        }

        entries.insert(key, entry);

        debug!(
            target: "bifrost_proxy::dns",
            host = %host,
            server = %server,
            ip = %ip,
            ttl_secs = self.ttl.as_secs(),
            "DNS cache entry added"
        );
    }

    fn evict_expired_or_oldest(&self, entries: &mut HashMap<String, DnsCacheEntry>) {
        let expired_keys: Vec<String> = entries
            .iter()
            .filter(|(_, v)| v.is_expired())
            .map(|(k, _)| k.clone())
            .collect();

        if !expired_keys.is_empty() {
            for key in expired_keys {
                entries.remove(&key);
            }
            debug!(
                target: "bifrost_proxy::dns",
                "Evicted expired DNS cache entries"
            );
            return;
        }

        if let Some((oldest_key, _)) = entries
            .iter()
            .min_by_key(|(_, v)| v.expires_at)
            .map(|(k, v)| (k.clone(), v.clone()))
        {
            entries.remove(&oldest_key);
            debug!(
                target: "bifrost_proxy::dns",
                key = %oldest_key,
                "Evicted oldest DNS cache entry"
            );
        }
    }

    pub async fn clear(&self) {
        let mut entries = self.entries.write().await;
        entries.clear();
        debug!(
            target: "bifrost_proxy::dns",
            "DNS cache cleared"
        );
    }

    pub async fn len(&self) -> usize {
        self.entries.read().await.len()
    }

    pub async fn is_empty(&self) -> bool {
        self.entries.read().await.is_empty()
    }

    pub async fn stats(&self) -> DnsCacheStats {
        let entries = self.entries.read().await;
        let total = entries.len();
        let expired = entries.values().filter(|e| e.is_expired()).count();
        DnsCacheStats {
            total_entries: total,
            expired_entries: expired,
            capacity: self.capacity,
            ttl_secs: self.ttl.as_secs(),
        }
    }
}

impl Default for DnsCache {
    fn default() -> Self {
        Self::new(DEFAULT_CACHE_TTL_SECS, DEFAULT_CACHE_CAPACITY)
    }
}

#[derive(Debug, Clone)]
pub struct DnsCacheStats {
    pub total_entries: usize,
    pub expired_entries: usize,
    pub capacity: usize,
    pub ttl_secs: u64,
}

#[derive(Debug, Clone)]
pub enum DnsServerType {
    Standard { addr: SocketAddr },
    DoH { url: String },
    DoT { addr: SocketAddr, hostname: String },
}

impl DnsServerType {
    pub fn parse(server: &str) -> Result<Self> {
        if server.starts_with("https://") {
            Ok(DnsServerType::DoH {
                url: server.to_string(),
            })
        } else if let Some(rest) = server.strip_prefix("tls://") {
            let (host, port) = if let Some(colon_pos) = rest.rfind(':') {
                let port_str = &rest[colon_pos + 1..];
                if let Ok(port) = port_str.parse::<u16>() {
                    (&rest[..colon_pos], port)
                } else {
                    (rest, 853)
                }
            } else {
                (rest, 853)
            };

            let addr = if let Ok(ip) = IpAddr::from_str(host) {
                SocketAddr::new(ip, port)
            } else {
                return Err(BifrostError::Config(format!(
                    "DoT requires IP address, got hostname: {}",
                    host
                )));
            };

            Ok(DnsServerType::DoT {
                addr,
                hostname: host.to_string(),
            })
        } else {
            let (host, port) = if let Some(colon_pos) = server.rfind(':') {
                let port_str = &server[colon_pos + 1..];
                if let Ok(port) = port_str.parse::<u16>() {
                    (&server[..colon_pos], port)
                } else {
                    (server, 53)
                }
            } else {
                (server, 53)
            };

            let ip = IpAddr::from_str(host).map_err(|e| {
                BifrostError::Config(format!("Invalid DNS server IP '{}': {}", host, e))
            })?;

            Ok(DnsServerType::Standard {
                addr: SocketAddr::new(ip, port),
            })
        }
    }
}

pub struct DnsResolver {
    system_resolver: TokioResolver,
    custom_resolvers: RwLock<HashMap<String, Arc<TokioResolver>>>,
    cache: DnsCache,
    timeout: Duration,
    verbose_logging: bool,
}

impl DnsResolver {
    pub fn new(verbose_logging: bool) -> Self {
        let system_resolver =
            Self::build_tokio_resolver(ResolverConfig::default(), ResolverOpts::default())
                .expect("default DNS resolver configuration must be valid");

        Self {
            system_resolver,
            custom_resolvers: RwLock::new(HashMap::new()),
            cache: DnsCache::default(),
            timeout: Duration::from_secs(5),
            verbose_logging,
        }
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn with_cache_config(mut self, ttl_secs: u64, capacity: usize) -> Self {
        self.cache = DnsCache::new(ttl_secs, capacity);
        self
    }

    pub async fn cache_stats(&self) -> DnsCacheStats {
        self.cache.stats().await
    }

    pub async fn clear_cache(&self) {
        self.cache.clear().await;
    }

    #[instrument(skip(self), fields(host = %host, servers = ?dns_servers))]
    pub async fn resolve(&self, host: &str, dns_servers: &[String]) -> Result<Option<IpAddr>> {
        if let Ok(ip) = IpAddr::from_str(host) {
            debug!(
                target: "bifrost_proxy::dns",
                host = %host,
                "Host is already an IP address, skipping DNS resolution"
            );
            return Ok(Some(ip));
        }

        if dns_servers.is_empty() {
            debug!(
                target: "bifrost_proxy::dns",
                host = %host,
                "No custom DNS servers configured, using system resolver"
            );
            return Ok(None);
        }

        let servers_key = dns_servers.join(",");
        if let Some(cached_ip) = self.cache.get(host, &servers_key).await {
            return Ok(Some(cached_ip));
        }

        if self.verbose_logging {
            info!(
                target: "bifrost_proxy::dns",
                host = %host,
                servers = ?dns_servers,
                "Starting DNS resolution with custom servers"
            );
        }

        for (index, server_str) in dns_servers.iter().enumerate() {
            for server in server_str.split(',') {
                let server = server.trim();
                if server.is_empty() {
                    continue;
                }

                debug!(
                    target: "bifrost_proxy::dns",
                    server = %server,
                    attempt = index + 1,
                    total = dns_servers.len(),
                    "Trying DNS server"
                );

                match self.resolve_with_server(host, server).await {
                    Ok(ip) => {
                        info!(
                            target: "bifrost_proxy::dns",
                            host = %host,
                            server = %server,
                            resolved_ip = %ip,
                            "DNS resolution successful"
                        );
                        self.cache.put(host, &servers_key, ip).await;
                        return Ok(Some(ip));
                    }
                    Err(e) => {
                        warn!(
                            target: "bifrost_proxy::dns",
                            host = %host,
                            server = %server,
                            error = %e,
                            "DNS server failed, trying next"
                        );
                    }
                }
            }
        }

        warn!(
            target: "bifrost_proxy::dns",
            host = %host,
            servers = ?dns_servers,
            "All custom DNS servers failed, falling back to system resolver"
        );

        Ok(None)
    }

    async fn resolve_with_server(&self, host: &str, server: &str) -> Result<IpAddr> {
        let server_type = DnsServerType::parse(server)?;

        match server_type {
            DnsServerType::Standard { addr } => self.resolve_standard(host, addr).await,
            DnsServerType::DoH { url } => self.resolve_doh(host, &url).await,
            DnsServerType::DoT { addr, hostname } => self.resolve_dot(host, addr, &hostname).await,
        }
    }

    async fn resolve_standard(&self, host: &str, addr: SocketAddr) -> Result<IpAddr> {
        let resolver = self.get_or_create_resolver(&addr.to_string()).await?;

        let lookup = tokio::time::timeout(self.timeout, resolver.lookup_ip(host))
            .await
            .map_err(|_| {
                BifrostError::Network(format!(
                    "DNS lookup timeout for {} using server {}",
                    host, addr
                ))
            })?
            .map_err(|e| {
                BifrostError::Network(format!(
                    "DNS lookup failed for {} using server {}: {}",
                    host, addr, e
                ))
            })?;

        lookup.iter().next().ok_or_else(|| {
            BifrostError::Network(format!(
                "No IP addresses found for {} using server {}",
                host, addr
            ))
        })
    }

    async fn resolve_doh(&self, host: &str, url: &str) -> Result<IpAddr> {
        debug!(
            target: "bifrost_proxy::dns",
            host = %host,
            url = %url,
            "DNS over HTTPS resolution (not yet implemented, falling back)"
        );

        Err(BifrostError::Network(format!(
            "DNS over HTTPS not yet implemented for {}",
            url
        )))
    }

    async fn resolve_dot(&self, host: &str, addr: SocketAddr, hostname: &str) -> Result<IpAddr> {
        debug!(
            target: "bifrost_proxy::dns",
            host = %host,
            addr = %addr,
            hostname = %hostname,
            "DNS over TLS resolution (not yet implemented, falling back)"
        );

        Err(BifrostError::Network(format!(
            "DNS over TLS not yet implemented for {}",
            addr
        )))
    }

    async fn get_or_create_resolver(&self, server_key: &str) -> Result<Arc<TokioResolver>> {
        {
            let cache = self.custom_resolvers.read().await;
            if let Some(resolver) = cache.get(server_key) {
                return Ok(resolver.clone());
            }
        }

        let addr: SocketAddr = server_key.parse().map_err(|e| {
            BifrostError::Config(format!(
                "Invalid DNS server address '{}': {}",
                server_key, e
            ))
        })?;

        let mut connection = ConnectionConfig::udp();
        connection.port = addr.port();
        let name_server = NameServerConfig::new(addr.ip(), true, vec![connection]);
        let config = ResolverConfig::from_parts(None, vec![], vec![name_server]);

        let mut opts = ResolverOpts::default();
        opts.timeout = self.timeout;
        opts.attempts = 2;

        let resolver = Arc::new(Self::build_tokio_resolver(config, opts)?);

        {
            let mut cache = self.custom_resolvers.write().await;
            cache.insert(server_key.to_string(), resolver.clone());
        }

        debug!(
            target: "bifrost_proxy::dns",
            server = %server_key,
            "Created new DNS resolver for server"
        );

        Ok(resolver)
    }

    fn build_tokio_resolver(config: ResolverConfig, opts: ResolverOpts) -> Result<TokioResolver> {
        let mut builder =
            TokioResolver::builder_with_config(config, TokioRuntimeProvider::default());
        *builder.options_mut() = opts;
        builder
            .build()
            .map_err(|e| BifrostError::Config(format!("Failed to create DNS resolver: {}", e)))
    }

    #[instrument(skip(self), fields(host = %host))]
    pub async fn resolve_with_system(&self, host: &str) -> Result<IpAddr> {
        if let Ok(ip) = IpAddr::from_str(host) {
            return Ok(ip);
        }

        debug!(
            target: "bifrost_proxy::dns",
            host = %host,
            "Resolving with system DNS"
        );

        let lookup = tokio::time::timeout(self.timeout, self.system_resolver.lookup_ip(host))
            .await
            .map_err(|_| BifrostError::Network(format!("System DNS lookup timeout for {}", host)))?
            .map_err(|e| {
                BifrostError::Network(format!("System DNS lookup failed for {}: {}", host, e))
            })?;

        lookup.iter().next().ok_or_else(|| {
            BifrostError::Network(format!(
                "No IP addresses found for {} using system DNS",
                host
            ))
        })
    }
}

impl Default for DnsResolver {
    fn default() -> Self {
        Self::new(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_standard_dns_server() {
        let server = DnsServerType::parse("8.8.8.8").unwrap();
        match server {
            DnsServerType::Standard { addr } => {
                assert_eq!(addr.ip().to_string(), "8.8.8.8");
                assert_eq!(addr.port(), 53);
            }
            _ => panic!("Expected Standard DNS server"),
        }
    }

    #[test]
    fn test_parse_standard_dns_server_with_port() {
        let server = DnsServerType::parse("8.8.8.8:5353").unwrap();
        match server {
            DnsServerType::Standard { addr } => {
                assert_eq!(addr.ip().to_string(), "8.8.8.8");
                assert_eq!(addr.port(), 5353);
            }
            _ => panic!("Expected Standard DNS server"),
        }
    }

    #[test]
    fn test_parse_doh_server() {
        let server = DnsServerType::parse("https://dns.google/dns-query").unwrap();
        match server {
            DnsServerType::DoH { url } => {
                assert_eq!(url, "https://dns.google/dns-query");
            }
            _ => panic!("Expected DoH DNS server"),
        }
    }

    #[test]
    fn test_parse_dot_server() {
        let server = DnsServerType::parse("tls://8.8.8.8").unwrap();
        match server {
            DnsServerType::DoT { addr, .. } => {
                assert_eq!(addr.ip().to_string(), "8.8.8.8");
                assert_eq!(addr.port(), 853);
            }
            _ => panic!("Expected DoT DNS server"),
        }
    }

    #[test]
    fn test_parse_dot_server_with_port() {
        let server = DnsServerType::parse("tls://8.8.8.8:8853").unwrap();
        match server {
            DnsServerType::DoT { addr, .. } => {
                assert_eq!(addr.ip().to_string(), "8.8.8.8");
                assert_eq!(addr.port(), 8853);
            }
            _ => panic!("Expected DoT DNS server"),
        }
    }

    #[test]
    fn test_parse_invalid_dns_server() {
        let result = DnsServerType::parse("invalid-hostname");
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn coverage_90_cache_empty_clear_and_dot_hostname_validation() {
        let cache = DnsCache::new(60, 4);
        assert!(cache.is_empty().await);
        cache
            .put("coverage.test", "system", "127.0.0.1".parse().unwrap())
            .await;
        assert!(!cache.is_empty().await);
        cache.clear().await;
        assert!(cache.is_empty().await);

        let error = DnsServerType::parse("tls://resolver.example:853").unwrap_err();
        assert!(error.to_string().contains("DoT requires IP address"));
        assert!(matches!(
            DnsServerType::parse("tls://1.1.1.1").unwrap(),
            DnsServerType::DoT { .. }
        ));
        assert!(matches!(
            DnsServerType::parse("1.1.1.1").unwrap(),
            DnsServerType::Standard { .. }
        ));
        let resolver = DnsResolver::new(false);
        assert!(resolver.resolve_with_system("localhost").await.is_ok());
        assert!(resolver
            .resolve_with_system("definitely-not-a-real-host.invalid")
            .await
            .is_err());
    }

    #[tokio::test]
    async fn test_resolver_skip_ip_address() {
        let resolver = DnsResolver::new(false);
        let result = resolver.resolve("192.168.1.1", &[]).await.unwrap();
        assert_eq!(result, Some(IpAddr::from_str("192.168.1.1").unwrap()));
    }

    #[tokio::test]
    async fn test_resolver_empty_servers_returns_none() {
        let resolver = DnsResolver::new(false);
        let result = resolver.resolve("example.com", &[]).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_dns_cache_basic() {
        let cache = DnsCache::new(60, 100);
        let ip = IpAddr::from_str("1.2.3.4").unwrap();

        assert!(cache.get("example.com", "8.8.8.8").await.is_none());

        cache.put("example.com", "8.8.8.8", ip).await;

        let cached = cache.get("example.com", "8.8.8.8").await;
        assert_eq!(cached, Some(ip));

        assert!(cache.get("example.com", "1.1.1.1").await.is_none());
        assert!(cache.get("other.com", "8.8.8.8").await.is_none());
    }

    #[tokio::test]
    async fn test_dns_cache_expiry() {
        let cache = DnsCache::new(1, 100);
        let ip = IpAddr::from_str("1.2.3.4").unwrap();

        cache.put("example.com", "8.8.8.8", ip).await;
        assert!(cache.get("example.com", "8.8.8.8").await.is_some());

        tokio::time::sleep(Duration::from_secs(2)).await;

        assert!(cache.get("example.com", "8.8.8.8").await.is_none());
    }

    #[tokio::test]
    async fn test_dns_cache_capacity() {
        let cache = DnsCache::new(60, 3);

        cache
            .put("a.com", "dns", IpAddr::from_str("1.1.1.1").unwrap())
            .await;
        cache
            .put("b.com", "dns", IpAddr::from_str("2.2.2.2").unwrap())
            .await;
        cache
            .put("c.com", "dns", IpAddr::from_str("3.3.3.3").unwrap())
            .await;

        assert_eq!(cache.len().await, 3);

        cache
            .put("d.com", "dns", IpAddr::from_str("4.4.4.4").unwrap())
            .await;

        assert!(cache.len().await <= 3);
    }

    #[tokio::test]
    async fn test_dns_cache_stats() {
        let cache = DnsCache::new(60, 100);

        cache
            .put("a.com", "dns", IpAddr::from_str("1.1.1.1").unwrap())
            .await;
        cache
            .put("b.com", "dns", IpAddr::from_str("2.2.2.2").unwrap())
            .await;

        let stats = cache.stats().await;
        assert_eq!(stats.total_entries, 2);
        assert_eq!(stats.capacity, 100);
        assert_eq!(stats.ttl_secs, 60);
    }

    #[tokio::test]
    async fn test_dns_cache_clear() {
        let cache = DnsCache::new(60, 100);

        cache
            .put("a.com", "dns", IpAddr::from_str("1.1.1.1").unwrap())
            .await;
        cache
            .put("b.com", "dns", IpAddr::from_str("2.2.2.2").unwrap())
            .await;

        assert_eq!(cache.len().await, 2);

        cache.clear().await;

        assert_eq!(cache.len().await, 0);
    }

    #[tokio::test]
    async fn test_resolver_with_cache_config() {
        let resolver = DnsResolver::new(false).with_cache_config(120, 500);

        let stats = resolver.cache_stats().await;
        assert_eq!(stats.ttl_secs, 120);
        assert_eq!(stats.capacity, 500);
    }

    #[tokio::test]
    async fn coverage_90_custom_resolver_failures_fall_back_and_cache_resolvers() {
        let resolver = DnsResolver::new(true).with_timeout(Duration::from_millis(20));
        let servers = vec![
            " , https://127.0.0.1/dns-query".to_string(),
            "tls://127.0.0.1:853".to_string(),
            "127.0.0.1:9".to_string(),
            "invalid-server".to_string(),
        ];
        assert_eq!(
            resolver
                .resolve("coverage.invalid", &servers)
                .await
                .unwrap(),
            None
        );

        let first = resolver
            .get_or_create_resolver("127.0.0.1:9")
            .await
            .unwrap();
        let second = resolver
            .get_or_create_resolver("127.0.0.1:9")
            .await
            .unwrap();
        assert!(Arc::ptr_eq(&first, &second));
        assert!(resolver
            .get_or_create_resolver("not-an-address")
            .await
            .is_err());
    }

    #[tokio::test]
    async fn coverage_90_system_resolution_and_expired_capacity_paths() {
        let resolver = DnsResolver::default().with_timeout(Duration::from_secs(2));
        assert_eq!(
            resolver.resolve_with_system("127.0.0.9").await.unwrap(),
            IpAddr::from_str("127.0.0.9").unwrap()
        );
        assert!(resolver.resolve_with_system("localhost").await.is_ok());

        let cache = DnsCache::new(0, 1);
        cache
            .put("expired", "server", IpAddr::from_str("127.0.0.1").unwrap())
            .await;
        cache
            .put(
                "replacement",
                "server",
                IpAddr::from_str("127.0.0.2").unwrap(),
            )
            .await;
        let stats = cache.stats().await;
        assert_eq!(stats.capacity, 1);
        assert!(stats.expired_entries <= stats.total_entries);
    }
}
