use std::{
    collections::HashMap,
    hash::Hash,
    sync::{
        atomic::{AtomicU64, AtomicUsize, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

use tokio::{
    io::{AsyncRead, AsyncWrite},
    net::TcpStream,
    sync::{Mutex, RwLock, Semaphore},
    time::timeout,
};

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct ConnectionKey {
    pub host: String,
    pub port: u16,
    pub is_tls: bool,
}

impl ConnectionKey {
    pub fn new(host: impl Into<String>, port: u16, is_tls: bool) -> Self {
        Self {
            host: host.into(),
            port,
            is_tls,
        }
    }

    pub fn http(host: impl Into<String>, port: u16) -> Self {
        Self::new(host, port, false)
    }

    pub fn https(host: impl Into<String>, port: u16) -> Self {
        Self::new(host, port, true)
    }

    pub fn address(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }
}

pub struct PooledConnection<S: Send + 'static> {
    stream: Option<S>,
    key: ConnectionKey,
    created_at: Instant,
    last_used: Instant,
    pool: Arc<ConnectionPoolInner<S>>,
}

impl<S: Send + 'static> PooledConnection<S> {
    pub fn stream(&mut self) -> &mut S {
        self.stream.as_mut().expect("connection already taken")
    }

    pub fn into_stream(mut self) -> S {
        self.stream.take().expect("connection already taken")
    }

    pub fn key(&self) -> &ConnectionKey {
        &self.key
    }

    pub fn age(&self) -> Duration {
        self.created_at.elapsed()
    }

    pub fn idle_time(&self) -> Duration {
        self.last_used.elapsed()
    }

    pub fn discard(mut self) {
        self.stream.take();
    }
}

impl<S: Send + 'static> Drop for PooledConnection<S> {
    fn drop(&mut self) {
        if let Some(stream) = self.stream.take() {
            let key = self.key.clone();
            let created_at = self.created_at;
            let pool = self.pool.clone();

            tokio::spawn(async move {
                pool.return_connection(key, stream, created_at).await;
            });
        }
    }
}

impl<S: Send + 'static> std::ops::Deref for PooledConnection<S> {
    type Target = S;

    fn deref(&self) -> &Self::Target {
        self.stream.as_ref().expect("connection already taken")
    }
}

impl<S: Send + 'static> std::ops::DerefMut for PooledConnection<S> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.stream.as_mut().expect("connection already taken")
    }
}

struct IdleConnection<S> {
    stream: S,
    created_at: Instant,
    last_used: Instant,
}

struct PooledConnections<S> {
    connections: Vec<IdleConnection<S>>,
    #[allow(dead_code)]
    pending: usize,
}

impl<S> Default for PooledConnections<S> {
    fn default() -> Self {
        Self {
            connections: Vec::new(),
            pending: 0,
        }
    }
}

#[derive(Debug, Clone)]
pub struct PoolConfig {
    pub max_idle_per_host: usize,
    pub max_total_connections: usize,
    pub idle_timeout: Duration,
    pub max_age: Duration,
    pub connect_timeout: Duration,
}

impl Default for PoolConfig {
    fn default() -> Self {
        Self {
            max_idle_per_host: 10,
            max_total_connections: 100,
            idle_timeout: Duration::from_secs(90),
            max_age: Duration::from_secs(300),
            connect_timeout: Duration::from_secs(30),
        }
    }
}

impl PoolConfig {
    pub fn high_performance() -> Self {
        Self {
            max_idle_per_host: 32,
            max_total_connections: 512,
            idle_timeout: Duration::from_secs(120),
            max_age: Duration::from_secs(600),
            connect_timeout: Duration::from_secs(10),
        }
    }

    pub fn low_memory() -> Self {
        Self {
            max_idle_per_host: 2,
            max_total_connections: 20,
            idle_timeout: Duration::from_secs(30),
            max_age: Duration::from_secs(120),
            connect_timeout: Duration::from_secs(30),
        }
    }
}

pub struct PoolStats {
    pub total_connections: AtomicUsize,
    pub idle_connections: AtomicUsize,
    pub connections_created: AtomicU64,
    pub connections_reused: AtomicU64,
    pub connections_closed: AtomicU64,
    pub connection_errors: AtomicU64,
}

impl Default for PoolStats {
    fn default() -> Self {
        Self {
            total_connections: AtomicUsize::new(0),
            idle_connections: AtomicUsize::new(0),
            connections_created: AtomicU64::new(0),
            connections_reused: AtomicU64::new(0),
            connections_closed: AtomicU64::new(0),
            connection_errors: AtomicU64::new(0),
        }
    }
}

impl PoolStats {
    pub fn snapshot(&self) -> PoolStatsSnapshot {
        PoolStatsSnapshot {
            total_connections: self.total_connections.load(Ordering::Relaxed),
            idle_connections: self.idle_connections.load(Ordering::Relaxed),
            connections_created: self.connections_created.load(Ordering::Relaxed),
            connections_reused: self.connections_reused.load(Ordering::Relaxed),
            connections_closed: self.connections_closed.load(Ordering::Relaxed),
            connection_errors: self.connection_errors.load(Ordering::Relaxed),
        }
    }
}

#[derive(Debug, Clone)]
pub struct PoolStatsSnapshot {
    pub total_connections: usize,
    pub idle_connections: usize,
    pub connections_created: u64,
    pub connections_reused: u64,
    pub connections_closed: u64,
    pub connection_errors: u64,
}

impl PoolStatsSnapshot {
    pub fn reuse_rate(&self) -> f64 {
        let total = self.connections_created + self.connections_reused;
        if total == 0 {
            0.0
        } else {
            self.connections_reused as f64 / total as f64
        }
    }
}

struct ConnectionPoolInner<S> {
    pools: RwLock<HashMap<ConnectionKey, Mutex<PooledConnections<S>>>>,
    config: PoolConfig,
    stats: PoolStats,
    semaphore: Semaphore,
}

impl<S: Send + 'static> ConnectionPoolInner<S> {
    fn new(config: PoolConfig) -> Self {
        let max_connections = config.max_total_connections;
        Self {
            pools: RwLock::new(HashMap::new()),
            config,
            stats: PoolStats::default(),
            semaphore: Semaphore::new(max_connections),
        }
    }

    async fn get_idle(&self, key: &ConnectionKey) -> Option<IdleConnection<S>> {
        let pools = self.pools.read().await;
        if let Some(pool_mutex) = pools.get(key) {
            let mut pool = pool_mutex.lock().await;
            while let Some(conn) = pool.connections.pop() {
                if conn.last_used.elapsed() < self.config.idle_timeout
                    && conn.created_at.elapsed() < self.config.max_age
                {
                    self.stats.idle_connections.fetch_sub(1, Ordering::Relaxed);
                    self.stats
                        .connections_reused
                        .fetch_add(1, Ordering::Relaxed);
                    return Some(conn);
                }
                self.stats.idle_connections.fetch_sub(1, Ordering::Relaxed);
                self.stats
                    .connections_closed
                    .fetch_add(1, Ordering::Relaxed);
            }
        }
        None
    }

    async fn return_connection(&self, key: ConnectionKey, stream: S, created_at: Instant) {
        if created_at.elapsed() >= self.config.max_age {
            self.stats
                .connections_closed
                .fetch_add(1, Ordering::Relaxed);
            self.stats.total_connections.fetch_sub(1, Ordering::Relaxed);
            return;
        }

        let pools = self.pools.read().await;
        if let Some(pool_mutex) = pools.get(&key) {
            let mut pool = pool_mutex.lock().await;
            if pool.connections.len() < self.config.max_idle_per_host {
                pool.connections.push(IdleConnection {
                    stream,
                    created_at,
                    last_used: Instant::now(),
                });
                self.stats.idle_connections.fetch_add(1, Ordering::Relaxed);
                return;
            }
        }
        drop(pools);

        {
            let mut pools = self.pools.write().await;
            let pool_mutex = pools.entry(key).or_default();
            let mut pool = pool_mutex.lock().await;
            if pool.connections.len() < self.config.max_idle_per_host {
                pool.connections.push(IdleConnection {
                    stream,
                    created_at,
                    last_used: Instant::now(),
                });
                self.stats.idle_connections.fetch_add(1, Ordering::Relaxed);
                return;
            }
        }

        self.stats
            .connections_closed
            .fetch_add(1, Ordering::Relaxed);
        self.stats.total_connections.fetch_sub(1, Ordering::Relaxed);
    }

    async fn cleanup_expired(&self) {
        let pools = self.pools.read().await;
        for pool_mutex in pools.values() {
            let mut pool = pool_mutex.lock().await;
            let before_len = pool.connections.len();
            pool.connections.retain(|conn| {
                conn.last_used.elapsed() < self.config.idle_timeout
                    && conn.created_at.elapsed() < self.config.max_age
            });
            let removed = before_len - pool.connections.len();
            if removed > 0 {
                self.stats
                    .idle_connections
                    .fetch_sub(removed, Ordering::Relaxed);
                self.stats
                    .connections_closed
                    .fetch_add(removed as u64, Ordering::Relaxed);
                self.stats
                    .total_connections
                    .fetch_sub(removed, Ordering::Relaxed);
            }
        }
    }

    async fn close_by_host(&self, host: &str) -> usize {
        let mut pools = self.pools.write().await;
        let host_lower = host.to_lowercase();

        let keys_to_remove: Vec<ConnectionKey> = pools
            .keys()
            .filter(|k| {
                let key_host = k.host.to_lowercase();
                key_host == host_lower || key_host.ends_with(&format!(".{}", host_lower))
            })
            .cloned()
            .collect();

        let mut total_removed = 0;
        for key in keys_to_remove {
            if let Some(pool_mutex) = pools.remove(&key) {
                let pool = pool_mutex.lock().await;
                let removed = pool.connections.len();
                total_removed += removed;
                self.stats
                    .idle_connections
                    .fetch_sub(removed, Ordering::Relaxed);
                self.stats
                    .connections_closed
                    .fetch_add(removed as u64, Ordering::Relaxed);
                self.stats
                    .total_connections
                    .fetch_sub(removed, Ordering::Relaxed);
            }
        }
        total_removed
    }

    async fn close_by_pattern(&self, pattern: &str) -> usize {
        let mut pools = self.pools.write().await;
        let pattern_lower = pattern.to_lowercase();

        let is_wildcard = pattern_lower.starts_with("*.");
        let base_pattern = if is_wildcard {
            pattern_lower.strip_prefix("*.").unwrap_or(&pattern_lower)
        } else {
            &pattern_lower
        };

        let keys_to_remove: Vec<ConnectionKey> = pools
            .keys()
            .filter(|k| {
                let key_host = k.host.to_lowercase();
                if is_wildcard {
                    let suffix = format!(".{}", base_pattern);
                    key_host.ends_with(&suffix) || key_host == base_pattern
                } else {
                    key_host == pattern_lower || key_host.ends_with(&format!(".{}", pattern_lower))
                }
            })
            .cloned()
            .collect();

        let mut total_removed = 0;
        for key in keys_to_remove {
            if let Some(pool_mutex) = pools.remove(&key) {
                let pool = pool_mutex.lock().await;
                let removed = pool.connections.len();
                total_removed += removed;
                self.stats
                    .idle_connections
                    .fetch_sub(removed, Ordering::Relaxed);
                self.stats
                    .connections_closed
                    .fetch_add(removed as u64, Ordering::Relaxed);
                self.stats
                    .total_connections
                    .fetch_sub(removed, Ordering::Relaxed);
            }
        }
        total_removed
    }
}

pub struct ConnectionPool<S> {
    inner: Arc<ConnectionPoolInner<S>>,
}

impl<S: Send + 'static> Clone for ConnectionPool<S> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

impl ConnectionPool<TcpStream> {
    pub fn new(config: PoolConfig) -> Self {
        Self {
            inner: Arc::new(ConnectionPoolInner::new(config)),
        }
    }

    pub async fn get(
        &self,
        key: ConnectionKey,
    ) -> Result<PooledConnection<TcpStream>, std::io::Error> {
        if let Some(idle) = self.inner.get_idle(&key).await {
            return Ok(PooledConnection {
                stream: Some(idle.stream),
                key,
                created_at: idle.created_at,
                last_used: Instant::now(),
                pool: self.inner.clone(),
            });
        }

        let _permit = self
            .inner
            .semaphore
            .acquire()
            .await
            .map_err(|_| std::io::Error::other("pool semaphore closed"))?;

        let stream = timeout(
            self.inner.config.connect_timeout,
            TcpStream::connect(key.address()),
        )
        .await
        .map_err(|_| {
            self.inner
                .stats
                .connection_errors
                .fetch_add(1, Ordering::Relaxed);
            std::io::Error::new(std::io::ErrorKind::TimedOut, "connection timeout")
        })?
        .inspect_err(|_| {
            self.inner
                .stats
                .connection_errors
                .fetch_add(1, Ordering::Relaxed);
        })?;

        stream.set_nodelay(true)?;

        self.inner
            .stats
            .total_connections
            .fetch_add(1, Ordering::Relaxed);
        self.inner
            .stats
            .connections_created
            .fetch_add(1, Ordering::Relaxed);

        Ok(PooledConnection {
            stream: Some(stream),
            key,
            created_at: Instant::now(),
            last_used: Instant::now(),
            pool: self.inner.clone(),
        })
    }

    pub fn stats(&self) -> PoolStatsSnapshot {
        self.inner.stats.snapshot()
    }

    pub async fn cleanup(&self) {
        self.inner.cleanup_expired().await;
    }

    pub async fn close_by_host(&self, host: &str) -> usize {
        self.inner.close_by_host(host).await
    }

    pub async fn close_by_pattern(&self, pattern: &str) -> usize {
        self.inner.close_by_pattern(pattern).await
    }

    pub fn start_cleanup_task(self) -> tokio::task::JoinHandle<()> {
        let pool = self.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(30));
            loop {
                interval.tick().await;
                pool.cleanup().await;
            }
        })
    }
}

impl<S: AsyncRead + AsyncWrite + Send + Unpin + 'static> ConnectionPool<S> {
    pub fn with_stream_type(config: PoolConfig) -> Self {
        Self {
            inner: Arc::new(ConnectionPoolInner::new(config)),
        }
    }

    pub fn wrap(&self, key: ConnectionKey, stream: S) -> PooledConnection<S> {
        self.inner
            .stats
            .total_connections
            .fetch_add(1, Ordering::Relaxed);
        self.inner
            .stats
            .connections_created
            .fetch_add(1, Ordering::Relaxed);

        PooledConnection {
            stream: Some(stream),
            key,
            created_at: Instant::now(),
            last_used: Instant::now(),
            pool: self.inner.clone(),
        }
    }

    pub async fn get_or_create<F, Fut>(
        &self,
        key: ConnectionKey,
        create_fn: F,
    ) -> Result<PooledConnection<S>, std::io::Error>
    where
        F: FnOnce(&ConnectionKey) -> Fut,
        Fut: std::future::Future<Output = Result<S, std::io::Error>>,
    {
        if let Some(idle) = self.inner.get_idle(&key).await {
            return Ok(PooledConnection {
                stream: Some(idle.stream),
                key,
                created_at: idle.created_at,
                last_used: Instant::now(),
                pool: self.inner.clone(),
            });
        }

        let _permit = self
            .inner
            .semaphore
            .acquire()
            .await
            .map_err(|_| std::io::Error::other("pool semaphore closed"))?;

        let stream = timeout(self.inner.config.connect_timeout, create_fn(&key))
            .await
            .map_err(|_| {
                self.inner
                    .stats
                    .connection_errors
                    .fetch_add(1, Ordering::Relaxed);
                std::io::Error::new(std::io::ErrorKind::TimedOut, "connection timeout")
            })?
            .inspect_err(|_| {
                self.inner
                    .stats
                    .connection_errors
                    .fetch_add(1, Ordering::Relaxed);
            })?;

        self.inner
            .stats
            .total_connections
            .fetch_add(1, Ordering::Relaxed);
        self.inner
            .stats
            .connections_created
            .fetch_add(1, Ordering::Relaxed);

        Ok(PooledConnection {
            stream: Some(stream),
            key,
            created_at: Instant::now(),
            last_used: Instant::now(),
            pool: self.inner.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::pin::Pin;
    use std::task::{Context, Poll};
    use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

    struct DummyStream;

    impl AsyncRead for DummyStream {
        fn poll_read(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            _buf: &mut ReadBuf<'_>,
        ) -> Poll<Result<(), std::io::Error>> {
            Poll::Ready(Ok(()))
        }
    }

    impl AsyncWrite for DummyStream {
        fn poll_write(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            buf: &[u8],
        ) -> Poll<Result<usize, std::io::Error>> {
            Poll::Ready(Ok(buf.len()))
        }

        fn poll_flush(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
        ) -> Poll<Result<(), std::io::Error>> {
            Poll::Ready(Ok(()))
        }

        fn poll_shutdown(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
        ) -> Poll<Result<(), std::io::Error>> {
            Poll::Ready(Ok(()))
        }
    }

    #[test]
    fn test_connection_key() {
        let key1 = ConnectionKey::http("example.com", 80);
        let key2 = ConnectionKey::https("example.com", 443);

        assert!(!key1.is_tls);
        assert!(key2.is_tls);
        assert_eq!(key1.address(), "example.com:80");
        assert_eq!(key2.address(), "example.com:443");
    }

    #[test]
    fn test_pool_config_presets() {
        let default_config = PoolConfig::default();
        let hp_config = PoolConfig::high_performance();
        let lm_config = PoolConfig::low_memory();

        assert!(hp_config.max_total_connections > default_config.max_total_connections);
        assert!(lm_config.max_total_connections < default_config.max_total_connections);
    }

    #[test]
    fn test_stats_snapshot_and_reuse_rate_zero() {
        let stats = PoolStats::default();
        stats.connections_created.store(0, Ordering::Relaxed);
        stats.connections_reused.store(0, Ordering::Relaxed);

        let snapshot = stats.snapshot();
        assert_eq!(snapshot.connections_created, 0);
        assert_eq!(snapshot.connections_reused, 0);
        assert_eq!(snapshot.reuse_rate(), 0.0);
    }

    #[tokio::test]
    async fn cleanup_expired_removes_stale_idle_connections() {
        let config = PoolConfig {
            max_idle_per_host: 10,
            max_total_connections: 100,
            idle_timeout: Duration::from_secs(1),
            max_age: Duration::from_secs(1),
            connect_timeout: Duration::from_secs(30),
        };
        let pool: ConnectionPool<DummyStream> = ConnectionPool::with_stream_type(config);
        let key = ConnectionKey::http("example.com", 80);

        {
            let mut pools = pool.inner.pools.write().await;
            let mutex = pools.entry(key.clone()).or_default();
            let mut per_host = mutex.lock().await;
            per_host.connections.push(IdleConnection {
                stream: DummyStream,
                created_at: Instant::now() - Duration::from_secs(60),
                last_used: Instant::now() - Duration::from_secs(60),
            });
        }
        pool.inner
            .stats
            .idle_connections
            .store(1, Ordering::Relaxed);
        pool.inner
            .stats
            .total_connections
            .store(1, Ordering::Relaxed);

        pool.inner.cleanup_expired().await;
        let snapshot = pool.inner.stats.snapshot();
        assert_eq!(snapshot.idle_connections, 0);
        assert_eq!(snapshot.total_connections, 0);
        assert_eq!(snapshot.connections_closed, 1);
    }

    #[tokio::test]
    async fn close_by_host_and_pattern_remove_expected_hosts() {
        let config = PoolConfig::default();
        let pool: ConnectionPool<DummyStream> = ConnectionPool::with_stream_type(config);

        let key_base = ConnectionKey::http("example.com", 80);
        let key_sub = ConnectionKey::http("api.example.com", 80);
        let key_other = ConnectionKey::http("other.com", 80);

        {
            let mut pools = pool.inner.pools.write().await;
            for key in [&key_base, &key_sub, &key_other] {
                let mutex = pools.entry(key.clone()).or_default();
                let mut per_host = mutex.lock().await;
                per_host.connections.push(IdleConnection {
                    stream: DummyStream,
                    created_at: Instant::now(),
                    last_used: Instant::now(),
                });
            }
        }
        pool.inner
            .stats
            .idle_connections
            .store(3, Ordering::Relaxed);
        pool.inner
            .stats
            .total_connections
            .store(3, Ordering::Relaxed);

        let removed_by_host = pool.inner.close_by_host("example.com").await;
        assert_eq!(removed_by_host, 2);
        let snapshot = pool.inner.stats.snapshot();
        assert_eq!(snapshot.idle_connections, 1);
        assert_eq!(snapshot.total_connections, 1);
        assert_eq!(snapshot.connections_closed, 2);

        // Add some more connections and test wildcard pattern eviction.
        {
            let mut pools = pool.inner.pools.write().await;
            let wildcard_key = ConnectionKey::http("static.example.com", 80);
            let mutex = pools.entry(wildcard_key.clone()).or_default();
            let mut per_host = mutex.lock().await;
            per_host.connections.push(IdleConnection {
                stream: DummyStream,
                created_at: Instant::now(),
                last_used: Instant::now(),
            });
            pool.inner
                .stats
                .idle_connections
                .fetch_add(1, Ordering::Relaxed);
            pool.inner
                .stats
                .total_connections
                .fetch_add(1, Ordering::Relaxed);
        }

        let removed_by_pattern = pool.inner.close_by_pattern("*.example.com").await;
        assert_eq!(removed_by_pattern, 1);
        let snapshot = pool.inner.stats.snapshot();
        assert_eq!(snapshot.idle_connections, 1);
        assert_eq!(snapshot.total_connections, 1);
        assert_eq!(snapshot.connections_closed, 3);
    }
}
