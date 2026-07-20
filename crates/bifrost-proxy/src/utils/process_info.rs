use std::cmp::{Ordering as CmpOrdering, Reverse};
use std::collections::{BinaryHeap, HashMap};
use std::future::Future;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock, RwLock};
use std::time::{Duration, Instant};

use tokio::sync::{Notify, Semaphore};
use tokio::task::JoinHandle;
use tracing::{debug, trace, warn};

use bifrost_core::ProcessResolverDiagnostics;

const PROCESS_RESOLUTION_WAIT_TIMEOUT: Duration = Duration::from_secs(2);
const CONNECTION_CACHE_CAPACITY: usize = 16_384;
const PID_CACHE_CAPACITY: usize = 2_048;
const CACHE_SHARD_COUNT: usize = 32;

#[derive(Debug, Clone)]
pub struct ClientProcess {
    pub pid: u32,
    pub name: String,
    pub path: Option<String>,
}

impl ClientProcess {
    pub fn display_name(&self) -> String {
        self.name.clone()
    }
}

/// Successful process attribution owned by one accepted TCP connection.
///
/// A positive result is immutable for the connection lifetime, while a miss is
/// deliberately retryable because the OS socket table can lag accept().
#[derive(Default)]
pub(crate) struct ConnectionProcessState {
    process: OnceLock<Arc<ClientProcess>>,
    resolution_in_flight: AtomicBool,
    resolution_finished: Notify,
}

struct ConnectionResolutionGuard<'a>(&'a ConnectionProcessState);

impl Drop for ConnectionResolutionGuard<'_> {
    fn drop(&mut self) {
        self.0.finish_resolution();
    }
}

impl ConnectionProcessState {
    pub(crate) fn cached(&self) -> Option<Arc<ClientProcess>> {
        self.process.get().cloned()
    }

    pub(crate) fn store(&self, process: Arc<ClientProcess>) -> Arc<ClientProcess> {
        let _ = self.process.set(process);
        Arc::clone(
            self.process
                .get()
                .expect("connection process was initialized"),
        )
    }

    pub(crate) async fn resolve<F, Fut>(&self, resolver: F) -> Option<Arc<ClientProcess>>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Option<Arc<ClientProcess>>>,
    {
        let mut resolver = Some(resolver);
        loop {
            if let Some(process) = self.cached() {
                return Some(process);
            }
            let finished = self.resolution_finished.notified();
            if self.try_start_resolution() {
                let _guard = ConnectionResolutionGuard(self);
                let resolver = resolver
                    .take()
                    .expect("connection resolver can only become leader once");
                let process = resolver().await?;
                return Some(self.store(process));
            }
            finished.await;
        }
    }

    pub(crate) fn try_start_background_resolution(&self) -> bool {
        self.try_start_resolution()
    }

    pub(crate) fn finish_background_resolution(&self) {
        self.finish_resolution();
    }

    fn try_start_resolution(&self) -> bool {
        self.resolution_in_flight
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    fn finish_resolution(&self) {
        self.resolution_in_flight.store(false, Ordering::Release);
        self.resolution_finished.notify_waiters();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct ConnKey {
    client_addr: SocketAddr,
    proxy_addr: Option<SocketAddr>,
}

impl ConnKey {
    fn from_peer_addr(peer_addr: &SocketAddr) -> Self {
        Self {
            client_addr: *peer_addr,
            proxy_addr: None,
        }
    }

    fn from_connection(peer_addr: &SocketAddr, local_addr: &SocketAddr) -> Self {
        Self {
            client_addr: *peer_addr,
            proxy_addr: Some(*local_addr),
        }
    }
}

#[derive(Debug)]
struct CacheEntry<V> {
    value: V,
    expires_at: Instant,
    generation: u64,
}

#[derive(Debug)]
struct CacheMarker<K> {
    key: K,
    expires_at: Instant,
    generation: u64,
}

impl<K> PartialEq for CacheMarker<K> {
    fn eq(&self, other: &Self) -> bool {
        self.expires_at == other.expires_at && self.generation == other.generation
    }
}

impl<K> Eq for CacheMarker<K> {}

impl<K> PartialOrd for CacheMarker<K> {
    fn partial_cmp(&self, other: &Self) -> Option<CmpOrdering> {
        Some(self.cmp(other))
    }
}

impl<K> Ord for CacheMarker<K> {
    fn cmp(&self, other: &Self) -> CmpOrdering {
        self.expires_at
            .cmp(&other.expires_at)
            .then_with(|| self.generation.cmp(&other.generation))
    }
}

struct CacheShard<K, V> {
    entries: HashMap<K, CacheEntry<V>>,
    expiry: BinaryHeap<Reverse<CacheMarker<K>>>,
    next_generation: u64,
    capacity: usize,
}

impl<K, V> CacheShard<K, V>
where
    K: Clone + Eq + Hash,
{
    fn purge_expired(&mut self, now: Instant) -> usize {
        let mut removed = 0;
        while self
            .expiry
            .peek()
            .is_some_and(|marker| marker.0.expires_at <= now)
        {
            let Reverse(marker) = self.expiry.pop().expect("peeked expiry marker");
            if self.entries.get(&marker.key).is_some_and(|entry| {
                entry.generation == marker.generation && entry.expires_at <= now
            }) {
                self.entries.remove(&marker.key);
                removed += 1;
            }
        }
        removed
    }

    fn evict_over_capacity(&mut self) -> usize {
        let mut evicted = 0;
        while self.entries.len() > self.capacity {
            let Some(Reverse(marker)) = self.expiry.pop() else {
                break;
            };
            if self
                .entries
                .get(&marker.key)
                .is_some_and(|entry| entry.generation == marker.generation)
            {
                self.entries.remove(&marker.key);
                evicted += 1;
            }
        }
        evicted
    }
}

/// A bounded, sharded TTL cache whose insert path is O(log shard capacity).
///
/// Expiry and hard-cap eviction use a min-heap, so short-lived negative entries
/// are discarded before longer-lived positive entries. Stale markers are
/// ignored by generation, so replacing a key never requires a full table scan.
/// Reads only acquire the selected shard's shared lock.
struct BoundedTtlCache<K, V> {
    shards: Vec<RwLock<CacheShard<K, V>>>,
    entry_count: AtomicUsize,
    evictions_total: AtomicU64,
    expired_total: AtomicU64,
}

impl<K, V> BoundedTtlCache<K, V>
where
    K: Clone + Eq + Hash,
    V: Clone,
{
    fn new(capacity: usize, shard_count: usize) -> Self {
        assert!(capacity > 0, "cache capacity must be non-zero");
        assert!(shard_count > 0, "cache shard count must be non-zero");
        let base_capacity = capacity / shard_count;
        let remainder = capacity % shard_count;
        let shards = (0..shard_count)
            .map(|index| {
                RwLock::new(CacheShard {
                    entries: HashMap::new(),
                    expiry: BinaryHeap::new(),
                    next_generation: 0,
                    capacity: base_capacity + usize::from(index < remainder),
                })
            })
            .collect();
        Self {
            shards,
            entry_count: AtomicUsize::new(0),
            evictions_total: AtomicU64::new(0),
            expired_total: AtomicU64::new(0),
        }
    }

    fn shard_index(&self, key: &K) -> usize {
        let mut hasher = DefaultHasher::new();
        key.hash(&mut hasher);
        (hasher.finish() as usize) % self.shards.len()
    }

    fn get(&self, key: &K, now: Instant) -> Option<V> {
        let shard = self.shards[self.shard_index(key)]
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        shard
            .entries
            .get(key)
            .filter(|entry| entry.expires_at > now)
            .map(|entry| entry.value.clone())
    }

    fn insert(&self, key: K, value: V, expires_at: Instant) {
        let shard_index = self.shard_index(&key);
        let mut shard = self.shards[shard_index]
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let expired = shard.purge_expired(Instant::now());
        if expired > 0 {
            self.entry_count.fetch_sub(expired, Ordering::Relaxed);
            self.expired_total
                .fetch_add(expired as u64, Ordering::Relaxed);
        }

        shard.next_generation = shard.next_generation.wrapping_add(1);
        let generation = shard.next_generation;
        let marker = CacheMarker {
            key: key.clone(),
            expires_at,
            generation,
        };
        let replaced = shard
            .entries
            .insert(
                key,
                CacheEntry {
                    value,
                    expires_at,
                    generation,
                },
            )
            .is_some();
        shard.expiry.push(Reverse(marker));
        if !replaced {
            self.entry_count.fetch_add(1, Ordering::Relaxed);
        }

        let evicted = shard.evict_over_capacity();
        if evicted > 0 {
            self.entry_count.fetch_sub(evicted, Ordering::Relaxed);
            self.evictions_total
                .fetch_add(evicted as u64, Ordering::Relaxed);
        }
    }

    fn cleanup_expired(&self, now: Instant) -> usize {
        let mut removed = 0;
        for shard in &self.shards {
            removed += shard
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .purge_expired(now);
        }
        if removed > 0 {
            self.entry_count.fetch_sub(removed, Ordering::Relaxed);
            self.expired_total
                .fetch_add(removed as u64, Ordering::Relaxed);
        }
        removed
    }

    fn len(&self) -> usize {
        self.entry_count.load(Ordering::Relaxed)
    }

    fn evictions_total(&self) -> u64 {
        self.evictions_total.load(Ordering::Relaxed)
    }

    fn expired_total(&self) -> u64 {
        self.expired_total.load(Ordering::Relaxed)
    }
}

struct SocketSnapshot {
    connections_to_pids: HashMap<ConnKey, u32>,
    refreshed_at: Instant,
    expires_at: Instant,
}

struct SocketPidMapScan {
    connections_to_pids: HashMap<ConnKey, u32>,
    scanned_pids: usize,
    scanned_fds: usize,
    failed: bool,
}

#[derive(Default)]
struct SnapshotRefreshCoordinator {
    generation: u64,
    refreshing: bool,
}

type SocketPidMapScanner = dyn Fn() -> SocketPidMapScan + Send + Sync;

pub struct ProcessResolver {
    cache: BoundedTtlCache<ConnKey, Option<Arc<ClientProcess>>>,
    pid_cache: BoundedTtlCache<u32, Arc<ClientProcess>>,
    socket_snapshot: RwLock<Option<SocketSnapshot>>,
    snapshot_refresh: Mutex<SnapshotRefreshCoordinator>,
    snapshot_published: Condvar,
    socket_pid_scanner: Arc<SocketPidMapScanner>,
    cache_ttl: Duration,
    pid_cache_ttl: Duration,
    negative_cache_ttl: Duration,
    socket_snapshot_ttl: Duration,
    socket_snapshot_miss_refresh_interval: Duration,
    diagnostics: Arc<ProcessResolverDiagnostics>,
}

impl Default for ProcessResolver {
    fn default() -> Self {
        Self::new()
    }
}

impl ProcessResolver {
    pub fn new() -> Self {
        Self::with_socket_pid_scanner(lookup_socket_pid_map)
    }

    fn with_socket_pid_scanner(
        scanner: impl Fn() -> SocketPidMapScan + Send + Sync + 'static,
    ) -> Self {
        Self {
            cache: BoundedTtlCache::new(CONNECTION_CACHE_CAPACITY, CACHE_SHARD_COUNT),
            pid_cache: BoundedTtlCache::new(PID_CACHE_CAPACITY, CACHE_SHARD_COUNT),
            socket_snapshot: RwLock::new(None),
            snapshot_refresh: Mutex::new(SnapshotRefreshCoordinator::default()),
            snapshot_published: Condvar::new(),
            socket_pid_scanner: Arc::new(scanner),
            cache_ttl: Duration::from_secs(30),
            pid_cache_ttl: Duration::from_secs(2),
            negative_cache_ttl: Duration::from_millis(500),
            socket_snapshot_ttl: Duration::from_millis(250),
            socket_snapshot_miss_refresh_interval: Duration::from_millis(50),
            diagnostics: Arc::new(ProcessResolverDiagnostics::default()),
        }
    }

    pub fn diagnostics(&self) -> Arc<ProcessResolverDiagnostics> {
        Arc::clone(&self.diagnostics)
    }

    pub fn resolve(&self, peer_addr: &SocketAddr) -> Option<ClientProcess> {
        self.resolve_by_key_shared(ConnKey::from_peer_addr(peer_addr))
            .map(|process| process.as_ref().clone())
    }

    pub fn resolve_for_connection(
        &self,
        peer_addr: &SocketAddr,
        local_addr: &SocketAddr,
    ) -> Option<ClientProcess> {
        self.resolve_by_key_shared(ConnKey::from_connection(peer_addr, local_addr))
            .map(|process| process.as_ref().clone())
    }

    pub fn resolve_cached(&self, peer_addr: &SocketAddr) -> Option<ClientProcess> {
        self.get_from_cache(&ConnKey::from_peer_addr(peer_addr))
            .flatten()
            .map(|process| process.as_ref().clone())
    }

    pub fn resolve_cached_for_connection(
        &self,
        peer_addr: &SocketAddr,
        local_addr: &SocketAddr,
    ) -> Option<ClientProcess> {
        self.get_from_cache(&ConnKey::from_connection(peer_addr, local_addr))
            .flatten()
            .map(|process| process.as_ref().clone())
    }

    pub fn resolve_with_retry(
        &self,
        peer_addr: &SocketAddr,
        max_retries: u32,
        delay_ms: u64,
    ) -> Option<ClientProcess> {
        self.resolve_with_retry_by_key_shared(
            ConnKey::from_peer_addr(peer_addr),
            max_retries,
            delay_ms,
        )
        .map(|process| process.as_ref().clone())
    }

    pub fn resolve_for_connection_with_retry(
        &self,
        peer_addr: &SocketAddr,
        local_addr: &SocketAddr,
        max_retries: u32,
        delay_ms: u64,
    ) -> Option<ClientProcess> {
        self.resolve_with_retry_by_key_shared(
            ConnKey::from_connection(peer_addr, local_addr),
            max_retries,
            delay_ms,
        )
        .map(|process| process.as_ref().clone())
    }

    pub async fn resolve_async(
        &self,
        peer_addr: SocketAddr,
        max_retries: u32,
        delay_ms: u64,
    ) -> Option<ClientProcess> {
        if let Some(cached) = self.get_from_cache(&ConnKey::from_peer_addr(&peer_addr)) {
            return cached.map(|process| process.as_ref().clone());
        }

        if !peer_addr.ip().is_loopback() {
            return None;
        }

        self.resolve_with_retry(&peer_addr, max_retries, delay_ms)
    }

    pub async fn resolve_async_for_connection(
        &self,
        peer_addr: SocketAddr,
        local_addr: SocketAddr,
        max_retries: u32,
        delay_ms: u64,
    ) -> Option<ClientProcess> {
        let key = ConnKey::from_connection(&peer_addr, &local_addr);
        if let Some(cached) = self.get_from_cache(&key) {
            return cached.map(|process| process.as_ref().clone());
        }

        if !peer_addr.ip().is_loopback() {
            return None;
        }

        self.resolve_for_connection_with_retry(&peer_addr, &local_addr, max_retries, delay_ms)
    }

    fn resolve_by_key_shared(&self, key: ConnKey) -> Option<Arc<ClientProcess>> {
        if let Some(cached) = self.get_from_cache(&key) {
            return cached;
        }

        let process = self.lookup_process_shared(&key);
        self.update_cache_shared(key, process.clone());
        process
    }

    fn resolve_with_retry_by_key_shared(
        &self,
        key: ConnKey,
        max_retries: u32,
        delay_ms: u64,
    ) -> Option<Arc<ClientProcess>> {
        for attempt in 0..=max_retries {
            if attempt > 0 {
                std::thread::sleep(Duration::from_millis(delay_ms));
            }

            let process = self.lookup_process_shared(&key);
            if process.is_some() {
                debug!(?key, attempt, "Resolved client process");
                self.update_cache_shared(key, process.clone());
                return process;
            }

            debug!(
                ?key,
                attempt, max_retries, "Client process lookup attempt missed"
            );
        }

        debug!(
            ?key,
            max_retries, delay_ms, "Failed to resolve client process after retries"
        );
        self.update_cache_shared(key, None);
        None
    }

    fn get_from_cache(&self, key: &ConnKey) -> Option<Option<Arc<ClientProcess>>> {
        if let Some(cached) = self.cache.get(key, Instant::now()) {
            trace!(?key, "Process info cache hit");
            self.diagnostics.record_cache_hit(cached.is_some());
            return Some(cached);
        }
        None
    }

    fn get_from_cache_for_connection_owned(
        &self,
        key: &ConnKey,
    ) -> Option<Option<Arc<ClientProcess>>> {
        match self.cache.get(key, Instant::now()) {
            Some(None) => {
                self.diagnostics.record_cache_hit(false);
                Some(None)
            }
            Some(Some(_)) => {
                trace!(
                    ?key,
                    "Ignoring cross-connection positive cache for connection-owned attribution"
                );
                None
            }
            None => None,
        }
    }

    fn update_cache(&self, key: ConnKey, process: Option<ClientProcess>) {
        self.update_cache_shared(key, process.map(Arc::new));
    }

    fn update_cache_shared(&self, key: ConnKey, process: Option<Arc<ClientProcess>>) {
        let ttl = if process.is_some() {
            self.cache_ttl
        } else {
            self.negative_cache_ttl
        };
        self.cache.insert(key, process, Instant::now() + ttl);
        self.record_cache_diagnostics();
    }

    fn lookup_process_shared(&self, key: &ConnKey) -> Option<Arc<ClientProcess>> {
        let process = self
            .lookup_pid(key)
            .and_then(|pid| self.lookup_cached_process_by_pid(pid));
        self.diagnostics.record_lookup_result(process.is_some());
        process
    }

    fn lookup_pid(&self, key: &ConnKey) -> Option<u32> {
        let now = Instant::now();
        if let Some(result) = self.lookup_fresh_snapshot(key, now) {
            return result;
        }

        let mut coordinator = match self.snapshot_refresh.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        if coordinator.refreshing {
            let observed_generation = coordinator.generation;
            while coordinator.refreshing && coordinator.generation == observed_generation {
                coordinator = self
                    .snapshot_published
                    .wait(coordinator)
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
            }
            drop(coordinator);
            return self.lookup_published_generation(key);
        }

        if let Some(result) = self.lookup_fresh_snapshot(key, Instant::now()) {
            return result;
        }

        coordinator.refreshing = true;
        drop(coordinator);

        self.diagnostics.record_snapshot_miss();
        let scan_started_at = Instant::now();
        let scan = (self.socket_pid_scanner)();
        let scan_duration_us = scan_started_at.elapsed().as_micros().min(u64::MAX as u128) as u64;
        self.diagnostics.record_snapshot_refresh(
            scan_duration_us,
            scan.scanned_pids,
            scan.scanned_fds,
            scan.failed,
        );
        let pid = scan.connections_to_pids.get(key).copied();
        let published_at = Instant::now();

        if let Ok(mut snapshot_guard) = self.socket_snapshot.write() {
            *snapshot_guard = Some(SocketSnapshot {
                connections_to_pids: scan.connections_to_pids,
                refreshed_at: published_at,
                expires_at: published_at + self.socket_snapshot_ttl,
            });
        }

        let mut coordinator = self
            .snapshot_refresh
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        coordinator.generation = coordinator.generation.wrapping_add(1);
        coordinator.refreshing = false;
        self.snapshot_published.notify_all();

        pid
    }

    fn lookup_fresh_snapshot(&self, key: &ConnKey, now: Instant) -> Option<Option<u32>> {
        let snapshot_guard = self.socket_snapshot.read().ok()?;
        let snapshot = snapshot_guard.as_ref()?;
        if snapshot.expires_at <= now {
            return None;
        }
        if let Some(pid) = snapshot.connections_to_pids.get(key).copied() {
            self.diagnostics.record_snapshot_hit();
            return Some(Some(pid));
        }
        if now.duration_since(snapshot.refreshed_at) < self.socket_snapshot_miss_refresh_interval {
            self.diagnostics.record_snapshot_miss();
            return Some(None);
        }
        None
    }

    fn lookup_published_generation(&self, key: &ConnKey) -> Option<u32> {
        let result = self.socket_snapshot.read().ok().and_then(|snapshot| {
            snapshot
                .as_ref()
                .and_then(|snapshot| snapshot.connections_to_pids.get(key).copied())
        });
        if result.is_some() {
            self.diagnostics.record_snapshot_hit();
        } else {
            self.diagnostics.record_snapshot_miss();
        }
        result
    }

    fn lookup_cached_process_by_pid(&self, pid: u32) -> Option<Arc<ClientProcess>> {
        let now = Instant::now();

        if let Some(process) = self.pid_cache.get(&pid, now) {
            trace!(pid, "Process info pid cache hit");
            return Some(process);
        }

        let (name, path) = get_process_info(pid);
        let process = Arc::new(ClientProcess { pid, name, path });
        self.pid_cache
            .insert(pid, Arc::clone(&process), now + self.pid_cache_ttl);
        self.record_cache_diagnostics();

        Some(process)
    }

    pub fn cleanup_expired(&self) {
        let now = Instant::now();
        let removed = self.cache.cleanup_expired(now);
        let removed_pids = self.pid_cache.cleanup_expired(now);
        if removed > 0 || removed_pids > 0 {
            debug!(
                removed,
                removed_pids,
                remaining = self.cache.len(),
                remaining_pids = self.pid_cache.len(),
                "Cleaned up expired process cache entries"
            );
        }
        self.record_cache_diagnostics();

        if let Ok(mut snapshot) = self.socket_snapshot.write() {
            if snapshot
                .as_ref()
                .is_some_and(|cached| cached.expires_at <= Instant::now())
            {
                *snapshot = None;
            }
        }
    }

    fn record_cache_diagnostics(&self) {
        self.diagnostics.record_cache_state(
            self.cache.len(),
            self.cache.evictions_total(),
            self.cache.expired_total(),
            self.pid_cache.len(),
            self.pid_cache.evictions_total(),
            self.pid_cache.expired_total(),
        );
    }
}

#[cfg(target_os = "macos")]
fn get_process_info(pid: u32) -> (String, Option<String>) {
    let name = get_process_name_macos(pid).unwrap_or_else(|| format!("PID:{}", pid));
    let path = get_process_path_macos(pid);
    (name, path)
}

#[cfg(target_os = "macos")]
fn get_process_name_macos(pid: u32) -> Option<String> {
    use std::ffi::CStr;

    let mut buffer = [0u8; 1024];
    let len = unsafe {
        libc::proc_name(
            pid as i32,
            buffer.as_mut_ptr() as *mut libc::c_void,
            buffer.len() as u32,
        )
    };

    if len > 0 {
        CStr::from_bytes_until_nul(&buffer[..])
            .ok()
            .map(|s| s.to_string_lossy().into_owned())
    } else {
        None
    }
}

#[cfg(target_os = "macos")]
fn get_process_path_macos(pid: u32) -> Option<String> {
    use std::ffi::CStr;

    let mut buffer = [0u8; 4096];
    let len = unsafe {
        libc::proc_pidpath(
            pid as i32,
            buffer.as_mut_ptr() as *mut libc::c_void,
            buffer.len() as u32,
        )
    };

    if len > 0 {
        CStr::from_bytes_until_nul(&buffer[..])
            .ok()
            .map(|s| s.to_string_lossy().into_owned())
    } else {
        None
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use super::{ConnKey, SocketPidMapScan};
    use std::collections::HashMap;
    use std::mem::size_of;
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
    use tracing::{debug, trace};

    const PROC_ALL_PIDS: u32 = 1;
    const PROC_PIDLISTFDS: i32 = 1;
    const PROC_PIDFDSOCKETINFO: i32 = 3;
    const PROX_FDTYPE_SOCKET: u32 = 2;
    const SOCKINFO_TCP: i32 = 2;
    const INI_IPV4: u8 = 0x1;
    const INI_IPV6: u8 = 0x2;
    const TCP_STATES_OF_INTEREST: [i32; 7] = [2, 3, 4, 5, 6, 8, 9];
    const SOCKET_FDINFO_SIZE: usize = 792;
    const SOCKET_FDINFO_PSI_OFFSET: usize = 24;
    #[cfg(test)]
    const SOCKET_INFO_PROTOCOL_OFFSET: usize = SOCKET_FDINFO_PSI_OFFSET + 156;
    #[cfg(test)]
    const SOCKET_INFO_FAMILY_OFFSET: usize = SOCKET_FDINFO_PSI_OFFSET + 160;
    const SOCKET_INFO_KIND_OFFSET: usize = SOCKET_FDINFO_PSI_OFFSET + 232;
    const SOCKET_INFO_PROTO_OFFSET: usize = SOCKET_FDINFO_PSI_OFFSET + 240;
    const TCP_SOCKINFO_STATE_OFFSET: usize = SOCKET_INFO_PROTO_OFFSET + 80;
    const IN_SOCKINFO_FPORT_OFFSET: usize = SOCKET_INFO_PROTO_OFFSET;
    const IN_SOCKINFO_LPORT_OFFSET: usize = SOCKET_INFO_PROTO_OFFSET + 4;
    const IN_SOCKINFO_VFLAG_OFFSET: usize = SOCKET_INFO_PROTO_OFFSET + 24;
    const IN_SOCKINFO_FADDR_OFFSET: usize = SOCKET_INFO_PROTO_OFFSET + 32;
    const IN_SOCKINFO_LADDR_OFFSET: usize = SOCKET_INFO_PROTO_OFFSET + 48;

    unsafe extern "C" {
        fn proc_listpids(
            kind: u32,
            typeinfo: u32,
            buffer: *mut libc::c_void,
            buffersize: i32,
        ) -> i32;
        fn proc_pidinfo(
            pid: i32,
            flavor: i32,
            arg: u64,
            buffer: *mut libc::c_void,
            buffersize: i32,
        ) -> i32;
        fn proc_pidfdinfo(
            pid: i32,
            fd: i32,
            flavor: i32,
            buffer: *mut libc::c_void,
            buffersize: i32,
        ) -> i32;
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct ProcFdInfo {
        proc_fd: i32,
        proc_fdtype: u32,
    }

    struct ParsedTcpSocket {
        kind: i32,
        state: i32,
        vflag: u8,
        local_port_raw: i32,
        remote_port_raw: i32,
        local_addr_raw: [u8; 16],
        remote_addr_raw: [u8; 16],
    }

    pub(super) fn lookup_socket_pid_map_macos() -> SocketPidMapScan {
        let Some(pids) = list_all_pids() else {
            return SocketPidMapScan {
                connections_to_pids: HashMap::new(),
                scanned_pids: 0,
                scanned_fds: 0,
                failed: true,
            };
        };
        let mut connections_to_pids = HashMap::new();
        let mut scanned_fds = 0usize;
        for pid in &pids {
            scanned_fds += collect_process_tcp_sockets(*pid, &mut connections_to_pids);
        }
        debug!(
            pid_count = pids.len(),
            socket_count = connections_to_pids.len(),
            "Refreshed macOS client socket pid snapshot"
        );
        SocketPidMapScan {
            connections_to_pids,
            scanned_pids: pids.len(),
            scanned_fds,
            failed: false,
        }
    }

    fn list_all_pids() -> Option<Vec<u32>> {
        let estimated_bytes = unsafe { proc_listpids(PROC_ALL_PIDS, 0, std::ptr::null_mut(), 0) };
        if estimated_bytes <= 0 {
            return None;
        }

        let mut buffer =
            vec![0i32; (estimated_bytes as usize / size_of::<i32>()).saturating_add(32)];
        let bytes_filled = unsafe {
            proc_listpids(
                PROC_ALL_PIDS,
                0,
                buffer.as_mut_ptr().cast(),
                (buffer.len() * size_of::<i32>()) as i32,
            )
        };
        if bytes_filled <= 0 {
            return None;
        }

        buffer.truncate(bytes_filled as usize / size_of::<i32>());
        Some(
            buffer
                .into_iter()
                .filter(|pid| *pid > 0)
                .map(|pid| pid as u32)
                .collect(),
        )
    }

    fn collect_process_tcp_sockets(
        pid: u32,
        connections_to_pids: &mut HashMap<ConnKey, u32>,
    ) -> usize {
        let mut capacity = 64usize;
        loop {
            let mut fds = vec![
                ProcFdInfo {
                    proc_fd: 0,
                    proc_fdtype: 0
                };
                capacity
            ];
            let buffer_size = (capacity * size_of::<ProcFdInfo>()) as i32;
            let bytes_filled = unsafe {
                proc_pidinfo(
                    pid as i32,
                    PROC_PIDLISTFDS,
                    0,
                    fds.as_mut_ptr().cast(),
                    buffer_size,
                )
            };

            if bytes_filled <= 0 {
                return 0;
            }

            if bytes_filled as usize == buffer_size as usize && capacity < 4096 {
                capacity *= 2;
                continue;
            }

            fds.truncate(bytes_filled as usize / size_of::<ProcFdInfo>());
            let scanned_fds = fds.len();
            for fd in fds {
                if fd.proc_fdtype != PROX_FDTYPE_SOCKET {
                    continue;
                }

                if let Some(key) = socket_fd_key(pid, fd.proc_fd) {
                    connections_to_pids
                        .entry(ConnKey::from_peer_addr(&key.client_addr))
                        .or_insert(pid);
                    connections_to_pids.entry(key).or_insert(pid);
                }
            }

            return scanned_fds;
        }
    }

    fn socket_fd_key(pid: u32, fd: i32) -> Option<ConnKey> {
        let mut socket_fdinfo = [0u8; SOCKET_FDINFO_SIZE];
        let bytes_filled = unsafe {
            proc_pidfdinfo(
                pid as i32,
                fd,
                PROC_PIDFDSOCKETINFO,
                socket_fdinfo.as_mut_ptr().cast(),
                SOCKET_FDINFO_SIZE as i32,
            )
        };
        if bytes_filled != SOCKET_FDINFO_SIZE as i32 {
            trace!(
                pid,
                fd,
                bytes_filled,
                expected = SOCKET_FDINFO_SIZE as i32,
                error = %std::io::Error::last_os_error(),
                "proc_pidfdinfo(PROC_PIDFDSOCKETINFO) did not return socket info"
            );
            return None;
        }

        let tcp = parse_tcp_socket(&socket_fdinfo)?;
        if tcp.kind != SOCKINFO_TCP || !TCP_STATES_OF_INTEREST.contains(&tcp.state) {
            return None;
        }

        connection_key_from_raw(&tcp)
    }

    #[cfg(test)]
    pub(super) fn describe_process_tcp_sockets(pid: u32) -> Vec<String> {
        let mut capacity = 64usize;
        loop {
            let mut fds = vec![
                ProcFdInfo {
                    proc_fd: 0,
                    proc_fdtype: 0
                };
                capacity
            ];
            let buffer_size = (capacity * size_of::<ProcFdInfo>()) as i32;
            let bytes_filled = unsafe {
                proc_pidinfo(
                    pid as i32,
                    PROC_PIDLISTFDS,
                    0,
                    fds.as_mut_ptr().cast(),
                    buffer_size,
                )
            };

            if bytes_filled <= 0 {
                return vec![format!(
                    "pid={pid}: proc_pidinfo(PROC_PIDLISTFDS) returned {bytes_filled}"
                )];
            }

            if bytes_filled as usize == buffer_size as usize && capacity < 4096 {
                capacity *= 2;
                continue;
            }

            fds.truncate(bytes_filled as usize / size_of::<ProcFdInfo>());
            let mut out = Vec::new();
            for fd in fds {
                if fd.proc_fdtype != PROX_FDTYPE_SOCKET {
                    continue;
                }

                let mut socket_fdinfo = [0u8; SOCKET_FDINFO_SIZE];
                let bytes_filled = unsafe {
                    proc_pidfdinfo(
                        pid as i32,
                        fd.proc_fd,
                        PROC_PIDFDSOCKETINFO,
                        socket_fdinfo.as_mut_ptr().cast(),
                        SOCKET_FDINFO_SIZE as i32,
                    )
                };
                if bytes_filled != SOCKET_FDINFO_SIZE as i32 {
                    out.push(format!(
                        "fd={} kind=? proc_pidfdinfo_bytes={} errno={}",
                        fd.proc_fd,
                        bytes_filled,
                        std::io::Error::last_os_error()
                    ));
                    continue;
                }

                let Some(info) = parse_tcp_socket(&socket_fdinfo) else {
                    out.push(format!("fd={} parse_failed", fd.proc_fd));
                    continue;
                };
                if info.kind != SOCKINFO_TCP {
                    out.push(format!("fd={} kind={} non_tcp", fd.proc_fd, info.kind));
                    continue;
                }

                let local_ip = extract_ip_bytes(info.vflag, &info.local_addr_raw);
                let local_port = decode_port(info.local_port_raw);
                let remote_ip = extract_ip_bytes(info.vflag, &info.remote_addr_raw);
                let remote_port = decode_port(info.remote_port_raw);
                let family =
                    read_i32(&socket_fdinfo, SOCKET_INFO_FAMILY_OFFSET).unwrap_or_default();
                let protocol =
                    read_i32(&socket_fdinfo, SOCKET_INFO_PROTOCOL_OFFSET).unwrap_or_default();
                out.push(format!(
                    "fd={} state={} local={:?}:{:?} remote={:?}:{:?} vflag={} family={} protocol={}",
                    fd.proc_fd,
                    info.state,
                    local_ip,
                    local_port,
                    remote_ip,
                    remote_port,
                    info.vflag,
                    family,
                    protocol
                ));
            }

            return out;
        }
    }

    fn parse_tcp_socket(buffer: &[u8; SOCKET_FDINFO_SIZE]) -> Option<ParsedTcpSocket> {
        Some(ParsedTcpSocket {
            kind: read_i32(buffer, SOCKET_INFO_KIND_OFFSET)?,
            state: read_i32(buffer, TCP_SOCKINFO_STATE_OFFSET)?,
            vflag: *buffer.get(IN_SOCKINFO_VFLAG_OFFSET)?,
            local_port_raw: read_i32(buffer, IN_SOCKINFO_LPORT_OFFSET)?,
            remote_port_raw: read_i32(buffer, IN_SOCKINFO_FPORT_OFFSET)?,
            local_addr_raw: read_fixed_16(buffer, IN_SOCKINFO_LADDR_OFFSET)?,
            remote_addr_raw: read_fixed_16(buffer, IN_SOCKINFO_FADDR_OFFSET)?,
        })
    }

    fn read_i32(buffer: &[u8], offset: usize) -> Option<i32> {
        let bytes: [u8; 4] = buffer.get(offset..offset + 4)?.try_into().ok()?;
        Some(i32::from_ne_bytes(bytes))
    }

    fn read_fixed_16(buffer: &[u8], offset: usize) -> Option<[u8; 16]> {
        buffer.get(offset..offset + 16)?.try_into().ok()
    }

    fn connection_key_from_raw(socket: &ParsedTcpSocket) -> Option<ConnKey> {
        let client_ip = extract_ip_bytes(socket.vflag, &socket.local_addr_raw)?;
        let client_port = decode_port(socket.local_port_raw)?;
        let proxy_ip = extract_ip_bytes(socket.vflag, &socket.remote_addr_raw)?;
        let proxy_port = decode_port(socket.remote_port_raw)?;

        Some(ConnKey {
            client_addr: std::net::SocketAddr::new(client_ip, client_port),
            proxy_addr: Some(std::net::SocketAddr::new(proxy_ip, proxy_port)),
        })
    }

    fn decode_port(raw_port: i32) -> Option<u16> {
        let raw_port = u16::try_from(raw_port).ok()?;
        Some(u16::from_be(raw_port))
    }

    fn extract_ip_bytes(vflag: u8, raw_addr: &[u8; 16]) -> Option<IpAddr> {
        match vflag {
            INI_IPV4 => Some(IpAddr::V4(Ipv4Addr::new(
                raw_addr[12],
                raw_addr[13],
                raw_addr[14],
                raw_addr[15],
            ))),
            INI_IPV6 => Some(IpAddr::V6(Ipv6Addr::from(*raw_addr))),
            _ => None,
        }
    }
}

#[cfg(target_os = "macos")]
fn lookup_socket_pid_map() -> SocketPidMapScan {
    macos::lookup_socket_pid_map_macos()
}

#[cfg(target_os = "windows")]
fn get_process_info(pid: u32) -> (String, Option<String>) {
    let path = get_process_path_windows(pid);
    let name = path
        .as_ref()
        .and_then(|path| {
            std::path::Path::new(path)
                .file_stem()
                .and_then(|stem| stem.to_str())
                .map(|stem| stem.to_string())
        })
        .unwrap_or_else(|| format!("PID:{}", pid));
    (name, path)
}

#[cfg(target_os = "windows")]
fn get_process_path_windows(pid: u32) -> Option<String> {
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt;
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_FORMAT,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };

    unsafe {
        let handle: HANDLE = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;

        let mut buffer = vec![0u16; 1024];
        let mut size = buffer.len() as u32;

        let result = QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_FORMAT(0),
            windows::core::PWSTR(buffer.as_mut_ptr()),
            &mut size,
        );

        let _ = CloseHandle(handle);

        if result.is_ok() {
            let path = OsString::from_wide(&buffer[..size as usize]);
            path.into_string().ok()
        } else {
            None
        }
    }
}

#[cfg(target_os = "linux")]
fn get_process_info(pid: u32) -> (String, Option<String>) {
    let path = get_process_path_linux(pid);
    let name = get_process_name_linux(pid).unwrap_or_else(|| {
        path.as_ref()
            .and_then(|path| {
                std::path::Path::new(path)
                    .file_name()
                    .and_then(|name| name.to_str())
                    .map(|name| name.to_string())
            })
            .unwrap_or_else(|| format!("PID:{}", pid))
    });
    (name, path)
}

#[cfg(target_os = "linux")]
fn get_process_name_linux(pid: u32) -> Option<String> {
    std::fs::read_to_string(format!("/proc/{pid}/comm"))
        .ok()
        .map(|content| content.trim().to_string())
}

#[cfg(target_os = "linux")]
fn get_process_path_linux(pid: u32) -> Option<String> {
    std::fs::read_link(format!("/proc/{pid}/exe"))
        .ok()
        .and_then(|path| path.to_str().map(|path| path.to_string()))
}

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
fn get_process_info(_pid: u32) -> (String, Option<String>) {
    ("Unknown".to_string(), None)
}

lazy_static::lazy_static! {
    pub static ref PROCESS_RESOLVER: ProcessResolver = ProcessResolver::new();
}

static BACKGROUND_PROCESS_RESOLUTION_CONCURRENCY: std::sync::LazyLock<usize> =
    std::sync::LazyLock::new(|| {
        std::env::var("BIFROST_BACKGROUND_PROCESS_RESOLUTION_CONCURRENCY")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .filter(|value| *value > 0)
            .unwrap_or(8)
    });

static BACKGROUND_PROCESS_RESOLUTION_SEMAPHORE: std::sync::LazyLock<Semaphore> =
    std::sync::LazyLock::new(|| Semaphore::new(*BACKGROUND_PROCESS_RESOLUTION_CONCURRENCY));

static PROCESS_RESOLUTION_CONCURRENCY: std::sync::LazyLock<usize> =
    std::sync::LazyLock::new(|| {
        std::env::var("BIFROST_PROCESS_RESOLUTION_CONCURRENCY")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .filter(|value| *value > 0)
            .unwrap_or(4)
    });

static PROCESS_RESOLUTION_SEMAPHORE: std::sync::LazyLock<Arc<Semaphore>> =
    std::sync::LazyLock::new(|| Arc::new(Semaphore::new(*PROCESS_RESOLUTION_CONCURRENCY)));

static APP_POLICY_PROCESS_RESOLUTION_RETRIES: std::sync::LazyLock<u32> =
    std::sync::LazyLock::new(|| {
        std::env::var("BIFROST_APP_POLICY_PROCESS_RESOLUTION_RETRIES")
            .ok()
            .and_then(|value| value.parse::<u32>().ok())
            .filter(|value| *value > 0)
            .unwrap_or(20)
    });

static APP_POLICY_PROCESS_RESOLUTION_DELAY_MS: std::sync::LazyLock<u64> =
    std::sync::LazyLock::new(|| {
        std::env::var("BIFROST_APP_POLICY_PROCESS_RESOLUTION_DELAY_MS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .filter(|value| *value > 0)
            .unwrap_or(50)
    });

#[cfg(not(target_os = "macos"))]
fn lookup_socket_pid_map() -> SocketPidMapScan {
    use netstat2::{
        get_sockets_info, AddressFamilyFlags, ProtocolFlags, ProtocolSocketInfo, TcpState,
    };

    let af_flags = AddressFamilyFlags::IPV4 | AddressFamilyFlags::IPV6;
    let proto_flags = ProtocolFlags::TCP;

    let sockets = match get_sockets_info(af_flags, proto_flags) {
        Ok(sockets) => sockets,
        Err(error) => return failed_socket_pid_map_scan(&error),
    };

    let scanned_fds = sockets.len();
    let mut connections_to_pids = HashMap::new();
    let mut scanned_pids = std::collections::HashSet::new();
    for socket in sockets {
        if let ProtocolSocketInfo::Tcp(tcp) = socket.protocol_socket_info {
            if matches!(
                tcp.state,
                TcpState::Established
                    | TcpState::SynSent
                    | TcpState::SynReceived
                    | TcpState::FinWait1
                    | TcpState::FinWait2
                    | TcpState::CloseWait
                    | TcpState::LastAck
            ) {
                if let Some(&pid) = socket.associated_pids.first() {
                    scanned_pids.insert(pid);
                    let key = ConnKey {
                        client_addr: SocketAddr::new(tcp.local_addr, tcp.local_port),
                        proxy_addr: Some(SocketAddr::new(tcp.remote_addr, tcp.remote_port)),
                    };
                    connections_to_pids
                        .entry(ConnKey::from_peer_addr(&key.client_addr))
                        .or_insert(pid);
                    connections_to_pids.entry(key).or_insert(pid);
                }
            }
        }
    }

    debug!(
        socket_count = connections_to_pids.len(),
        "Refreshed client socket pid snapshot"
    );
    SocketPidMapScan {
        connections_to_pids,
        scanned_pids: scanned_pids.len(),
        scanned_fds,
        failed: false,
    }
}

#[cfg(not(target_os = "macos"))]
fn failed_socket_pid_map_scan(error: &dyn std::fmt::Display) -> SocketPidMapScan {
    warn!(error = %error, "Failed to get socket info");
    SocketPidMapScan {
        connections_to_pids: HashMap::new(),
        scanned_pids: 0,
        scanned_fds: 0,
        failed: true,
    }
}

pub fn resolve_client_process(peer_addr: &SocketAddr) -> Option<ClientProcess> {
    PROCESS_RESOLVER.resolve(peer_addr)
}

pub fn resolve_client_process_for_connection(
    peer_addr: &SocketAddr,
    local_addr: &SocketAddr,
) -> Option<ClientProcess> {
    PROCESS_RESOLVER.resolve_for_connection(peer_addr, local_addr)
}

pub fn resolve_client_process_cached(peer_addr: &SocketAddr) -> Option<ClientProcess> {
    PROCESS_RESOLVER.resolve_cached(peer_addr)
}

pub fn resolve_client_process_cached_for_connection(
    peer_addr: &SocketAddr,
    local_addr: &SocketAddr,
) -> Option<ClientProcess> {
    PROCESS_RESOLVER.resolve_cached_for_connection(peer_addr, local_addr)
}

pub fn resolve_client_process_with_retry(
    peer_addr: &SocketAddr,
    max_retries: u32,
    delay_ms: u64,
) -> Option<ClientProcess> {
    PROCESS_RESOLVER.resolve_with_retry(peer_addr, max_retries, delay_ms)
}

pub fn resolve_client_process_for_connection_with_retry(
    peer_addr: &SocketAddr,
    local_addr: &SocketAddr,
    max_retries: u32,
    delay_ms: u64,
) -> Option<ClientProcess> {
    PROCESS_RESOLVER.resolve_for_connection_with_retry(peer_addr, local_addr, max_retries, delay_ms)
}

pub async fn resolve_client_process_async(peer_addr: &SocketAddr) -> Option<ClientProcess> {
    resolve_client_process_async_with_retry(peer_addr, 6, 10).await
}

pub async fn resolve_client_process_async_for_connection(
    peer_addr: &SocketAddr,
    local_addr: &SocketAddr,
) -> Option<ClientProcess> {
    resolve_client_process_async_for_connection_with_retry(peer_addr, local_addr, 6, 10).await
}

pub(crate) async fn resolve_client_process_async_for_connection_shared(
    peer_addr: &SocketAddr,
    local_addr: &SocketAddr,
) -> Option<Arc<ClientProcess>> {
    resolve_client_process_async_for_connection_with_retry_shared(peer_addr, local_addr, 6, 10)
        .await
}

pub async fn resolve_client_process_async_with_retry(
    peer_addr: &SocketAddr,
    max_retries: u32,
    delay_ms: u64,
) -> Option<ClientProcess> {
    let key = ConnKey::from_peer_addr(peer_addr);
    if let Some(cached) = PROCESS_RESOLVER.get_from_cache(&key) {
        return cached.map(|process| process.as_ref().clone());
    }

    if !peer_addr.ip().is_loopback() {
        return None;
    }

    let peer_addr = *peer_addr;
    let result = resolve_with_limited_blocking_task(key, move || {
        PROCESS_RESOLVER.resolve_with_retry(&peer_addr, max_retries, delay_ms)
    })
    .await;
    match result {
        Ok(process) => process,
        Err(err) => {
            warn!(peer_addr = %peer_addr, error = %err, "Async process resolution task failed");
            None
        }
    }
}

pub async fn resolve_client_process_async_for_connection_with_retry(
    peer_addr: &SocketAddr,
    local_addr: &SocketAddr,
    max_retries: u32,
    delay_ms: u64,
) -> Option<ClientProcess> {
    resolve_client_process_async_for_connection_with_retry_impl(
        peer_addr,
        local_addr,
        max_retries,
        delay_ms,
        true,
    )
    .await
    .map(|process| process.as_ref().clone())
}

pub(crate) async fn resolve_client_process_async_for_connection_with_retry_shared(
    peer_addr: &SocketAddr,
    local_addr: &SocketAddr,
    max_retries: u32,
    delay_ms: u64,
) -> Option<Arc<ClientProcess>> {
    resolve_client_process_async_for_connection_with_retry_impl(
        peer_addr,
        local_addr,
        max_retries,
        delay_ms,
        false,
    )
    .await
}

async fn resolve_client_process_async_for_connection_with_retry_impl(
    peer_addr: &SocketAddr,
    local_addr: &SocketAddr,
    max_retries: u32,
    delay_ms: u64,
    reuse_positive_connection_cache: bool,
) -> Option<Arc<ClientProcess>> {
    let key = ConnKey::from_connection(peer_addr, local_addr);
    if reuse_positive_connection_cache {
        if let Some(cached) = PROCESS_RESOLVER.get_from_cache(&key) {
            if let Some(ref process) = cached {
                debug!(
                    peer_addr = %peer_addr,
                    local_addr = %local_addr,
                    client_app = %process.name,
                    client_pid = process.pid,
                    "Client process resolution cache hit for connection"
                );
            }
            return cached;
        }
    } else {
        if let Some(cached) = PROCESS_RESOLVER.get_from_cache_for_connection_owned(&key) {
            return cached;
        }
    }

    if !peer_addr.ip().is_loopback() {
        return None;
    }

    debug!(
        peer_addr = %peer_addr,
        local_addr = %local_addr,
        max_retries,
        delay_ms,
        "Starting async client process resolution for connection"
    );

    let peer_addr = *peer_addr;
    let local_addr = *local_addr;
    let result = resolve_with_limited_blocking_task(key, move || {
        PROCESS_RESOLVER.resolve_with_retry_by_key_shared(key, max_retries, delay_ms)
    })
    .await;
    match result {
        Ok(Some(process)) => {
            debug!(
                peer_addr = %peer_addr,
                local_addr = %local_addr,
                client_app = %process.name,
                client_pid = process.pid,
                "Async client process resolution completed"
            );
            Some(process)
        }
        Ok(None) => {
            debug!(
                peer_addr = %peer_addr,
                local_addr = %local_addr,
                max_retries,
                delay_ms,
                "Async client process resolution completed without a match"
            );
            None
        }
        Err(err) => {
            warn!(
                peer_addr = %peer_addr,
                local_addr = %local_addr,
                error = %err,
                "Async process resolution task failed"
            );
            None
        }
    }
}

pub fn spawn_async_process_resolver<F>(
    peer_addr: SocketAddr,
    local_addr: SocketAddr,
    record_id: String,
    callback: F,
) where
    F: FnOnce(String, ClientProcess) + Send + 'static,
{
    spawn_async_process_resolver_with_finish(peer_addr, local_addr, record_id, callback, || {});
}

pub fn spawn_async_process_resolver_with_finish<F, G>(
    peer_addr: SocketAddr,
    local_addr: SocketAddr,
    record_id: String,
    callback: F,
    finish: G,
) where
    F: FnOnce(String, ClientProcess) + Send + 'static,
    G: FnOnce() + Send + 'static,
{
    tokio::spawn(async move {
        struct FinishGuard<G: FnOnce()>(Option<G>);
        impl<G: FnOnce()> Drop for FinishGuard<G> {
            fn drop(&mut self) {
                if let Some(finish) = self.0.take() {
                    finish();
                }
            }
        }

        let _finish_guard = FinishGuard(Some(finish));
        debug!(
            record_id,
            peer_addr = %peer_addr,
            local_addr = %local_addr,
            "Scheduling background client process backfill"
        );
        let permit = match BACKGROUND_PROCESS_RESOLUTION_SEMAPHORE.try_acquire() {
            Ok(permit) => permit,
            Err(_) => {
                debug!(
                    record_id,
                    peer_addr = %peer_addr,
                    local_addr = %local_addr,
                    limit = *BACKGROUND_PROCESS_RESOLUTION_CONCURRENCY,
                    "Background client process backfill skipped because concurrency limit is saturated"
                );
                return;
            }
        };

        #[cfg(not(target_os = "macos"))]
        tokio::time::sleep(Duration::from_millis(25)).await;

        let key = ConnKey::from_connection(&peer_addr, &local_addr);
        let result = resolve_with_limited_blocking_task(key, move || {
            PROCESS_RESOLVER.resolve_for_connection_with_retry(&peer_addr, &local_addr, 20, 50)
        })
        .await;
        drop(permit);

        match result {
            Ok(Some(process)) => {
                debug!(
                    record_id,
                    peer_addr = %peer_addr,
                    local_addr = %local_addr,
                    client_app = %process.name,
                    client_pid = process.pid,
                    "Background client process backfill resolved client"
                );
                callback(record_id, process);
            }
            Ok(None) => {
                debug!(
                    record_id,
                    peer_addr = %peer_addr,
                    local_addr = %local_addr,
                    "Background client process backfill finished without a match"
                );
            }
            Err(err) => {
                warn!(
                    record_id,
                    peer_addr = %peer_addr,
                    local_addr = %local_addr,
                    error = %err,
                    "Background client process backfill task failed"
                );
            }
        }
    });
}

async fn resolve_with_limited_blocking_task<F, T>(
    key: ConnKey,
    resolver: F,
) -> Result<Option<T>, tokio::task::JoinError>
where
    F: FnOnce() -> Option<T> + Send + 'static,
    T: Send + 'static,
{
    resolve_with_limited_blocking_task_for_semaphore(
        key,
        Arc::clone(&PROCESS_RESOLUTION_SEMAPHORE),
        *PROCESS_RESOLUTION_CONCURRENCY,
        PROCESS_RESOLUTION_WAIT_TIMEOUT,
        resolver,
    )
    .await
}

async fn resolve_with_limited_blocking_task_for_semaphore<F, T>(
    key: ConnKey,
    semaphore: Arc<Semaphore>,
    concurrency_limit: usize,
    timeout_duration: Duration,
    resolver: F,
) -> Result<Option<T>, tokio::task::JoinError>
where
    F: FnOnce() -> Option<T> + Send + 'static,
    T: Send + 'static,
{
    let started_at = Instant::now();
    let permit = match tokio::time::timeout(timeout_duration, semaphore.acquire_owned()).await {
        Ok(Ok(permit)) => permit,
        Ok(Err(_)) => {
            debug!(
                ?key,
                concurrency_limit,
                "Client process resolution skipped because concurrency limiter is closed"
            );
            return Ok(None);
        }
        Err(_) => {
            debug!(
                ?key,
                concurrency_limit,
                timeout_ms = timeout_duration.as_millis(),
                "Client process resolution skipped after waiting for concurrency capacity"
            );
            return Ok(None);
        }
    };

    let elapsed = started_at.elapsed();
    let remaining_timeout = timeout_duration.saturating_sub(elapsed);
    if remaining_timeout.is_zero() {
        debug!(
            ?key,
            concurrency_limit,
            timeout_ms = timeout_duration.as_millis(),
            "Client process resolution skipped because concurrency wait exhausted the timeout budget"
        );
        return Ok(None);
    }

    let task = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        resolver()
    });
    wait_for_process_resolution_with_timeout(task, key, remaining_timeout).await
}

async fn wait_for_process_resolution_with_timeout<T>(
    task: JoinHandle<Option<T>>,
    key: ConnKey,
    timeout_duration: Duration,
) -> Result<Option<T>, tokio::task::JoinError>
where
    T: Send + 'static,
{
    match tokio::time::timeout(timeout_duration, task).await {
        Ok(result) => result,
        Err(_) => {
            warn!(
                ?key,
                timeout_ms = timeout_duration.as_millis(),
                "Client process resolution timed out; continuing without app info"
            );
            PROCESS_RESOLVER.update_cache(key, None);
            Ok(None)
        }
    }
}

pub fn app_policy_process_resolution_retry_config() -> (u32, u64) {
    (
        *APP_POLICY_PROCESS_RESOLUTION_RETRIES,
        *APP_POLICY_PROCESS_RESOLUTION_DELAY_MS,
    )
}

pub fn format_client_info(peer_addr: &SocketAddr, process: Option<&ClientProcess>) -> String {
    match process {
        Some(process) => process.display_name(),
        None => peer_addr.ip().to_string(),
    }
}

#[cfg(test)]
mod tests;
