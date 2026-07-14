use std::sync::{Arc, LazyLock, Mutex, RwLock};
use std::time::{Duration, Instant};

const NATIVE_CERT_CACHE_TTL: Duration = Duration::from_secs(10 * 60);

#[derive(Debug, Clone)]
struct NativeCertLoad {
    certificates_der: Arc<[Vec<u8>]>,
    error_count: usize,
}

#[derive(Debug, Clone)]
struct CachedNativeCerts {
    certificates_der: Arc<[Vec<u8>]>,
    expires_at: Instant,
    generation: u64,
}

type NativeCertLoader = dyn Fn() -> NativeCertLoad + Send + Sync;

struct NativeCertCache {
    state: RwLock<Option<CachedNativeCerts>>,
    refresh_lock: Mutex<()>,
    ttl: Duration,
    loader: Arc<NativeCertLoader>,
    next_generation: std::sync::atomic::AtomicU64,
}

impl NativeCertCache {
    fn system(ttl: Duration) -> Self {
        Self::with_loader(ttl, || {
            let result = rustls_native_certs::load_native_certs();
            NativeCertLoad {
                certificates_der: result
                    .certs
                    .into_iter()
                    .map(|cert| cert.as_ref().to_vec())
                    .collect::<Vec<_>>()
                    .into(),
                error_count: result.errors.len(),
            }
        })
    }

    fn with_loader(
        ttl: Duration,
        loader: impl Fn() -> NativeCertLoad + Send + Sync + 'static,
    ) -> Self {
        Self {
            state: RwLock::new(None),
            refresh_lock: Mutex::new(()),
            ttl,
            loader: Arc::new(loader),
            next_generation: std::sync::atomic::AtomicU64::new(1),
        }
    }

    fn get(&self) -> Arc<[Vec<u8>]> {
        self.get_with_generation().0
    }

    fn get_with_generation(&self) -> (Arc<[Vec<u8>]>, u64) {
        let now = Instant::now();
        if let Some(cached) = self.fresh_snapshot(now) {
            return cached;
        }

        let _refresh_guard = self
            .refresh_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let now = Instant::now();
        if let Some(cached) = self.fresh_snapshot(now) {
            return cached;
        }

        let previous = self
            .state
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
            .map(|cached| Arc::clone(&cached.certificates_der));
        let loaded = (self.loader)();

        if loaded.certificates_der.is_empty() && loaded.error_count > 0 {
            tracing::warn!(
                error_count = loaded.error_count,
                "failed to refresh native certificate trust store"
            );
            if let Some(previous) = previous {
                return self.publish(Arc::clone(&previous), now);
            }
        } else if loaded.error_count > 0 {
            tracing::warn!(
                cert_count = loaded.certificates_der.len(),
                error_count = loaded.error_count,
                "native certificate trust store loaded with partial errors"
            );
        }

        let certificates_der = loaded.certificates_der;
        self.publish(certificates_der, now)
    }

    fn fresh_snapshot(&self, now: Instant) -> Option<(Arc<[Vec<u8>]>, u64)> {
        self.state
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
            .filter(|cached| cached.expires_at > now)
            .map(|cached| (Arc::clone(&cached.certificates_der), cached.generation))
    }

    fn publish(&self, certificates_der: Arc<[Vec<u8>]>, now: Instant) -> (Arc<[Vec<u8>]>, u64) {
        let generation = self
            .next_generation
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let mut state = self
            .state
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *state = Some(CachedNativeCerts {
            certificates_der,
            expires_at: now + self.ttl,
            generation,
        });
        let certificates_der = Arc::clone(
            &state
                .as_ref()
                .expect("snapshot was published")
                .certificates_der,
        );
        (certificates_der, generation)
    }

    fn invalidate(&self) {
        let mut state = self
            .state
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(cached) = state.as_mut() {
            cached.expires_at = Instant::now();
        }
    }
}

static NATIVE_CERT_CACHE: LazyLock<NativeCertCache> =
    LazyLock::new(|| NativeCertCache::system(NATIVE_CERT_CACHE_TTL));

pub fn native_certificates_der() -> Arc<[Vec<u8>]> {
    NATIVE_CERT_CACHE.get()
}

pub(crate) fn native_certificates_der_with_generation() -> (Arc<[Vec<u8>]>, u64) {
    NATIVE_CERT_CACHE.get_with_generation()
}

pub fn invalidate_native_certificate_cache() {
    NATIVE_CERT_CACHE.invalidate();
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::thread;

    use super::*;

    fn load(certificates: &[&[u8]], error_count: usize) -> NativeCertLoad {
        NativeCertLoad {
            certificates_der: certificates
                .iter()
                .map(|cert| cert.to_vec())
                .collect::<Vec<_>>()
                .into(),
            error_count,
        }
    }

    #[test]
    fn concurrent_callers_share_one_load() {
        let loads = Arc::new(AtomicUsize::new(0));
        let loads_for_loader = Arc::clone(&loads);
        let cache = Arc::new(NativeCertCache::with_loader(
            Duration::from_secs(60),
            move || {
                loads_for_loader.fetch_add(1, Ordering::SeqCst);
                thread::sleep(Duration::from_millis(20));
                load(&[b"cert-a"], 0)
            },
        ));

        let threads = (0..12)
            .map(|_| {
                let cache = Arc::clone(&cache);
                thread::spawn(move || cache.get())
            })
            .collect::<Vec<_>>();
        for handle in threads {
            assert_eq!(&*handle.join().unwrap(), &[b"cert-a".to_vec()]);
        }
        assert_eq!(loads.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn invalidation_forces_one_reload() {
        let loads = Arc::new(AtomicUsize::new(0));
        let loads_for_loader = Arc::clone(&loads);
        let cache = NativeCertCache::with_loader(Duration::from_secs(60), move || {
            let generation = loads_for_loader.fetch_add(1, Ordering::SeqCst) + 1;
            load(&[format!("cert-{generation}").as_bytes()], 0)
        });

        let (first, first_generation) = cache.get_with_generation();
        let (cached, cached_generation) = cache.get_with_generation();
        assert_eq!(&*first, &[b"cert-1".to_vec()]);
        assert_eq!(&*cached, &[b"cert-1".to_vec()]);
        assert_eq!(cached_generation, first_generation);
        cache.invalidate();
        let (refreshed, refreshed_generation) = cache.get_with_generation();
        assert_eq!(&*refreshed, &[b"cert-2".to_vec()]);
        assert!(refreshed_generation > first_generation);
        assert_eq!(loads.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn expired_refresh_failure_keeps_last_good_snapshot() {
        let loads = Arc::new(AtomicUsize::new(0));
        let loads_for_loader = Arc::clone(&loads);
        let cache = NativeCertCache::with_loader(Duration::from_millis(1), move || {
            if loads_for_loader.fetch_add(1, Ordering::SeqCst) == 0 {
                load(&[b"last-good"], 0)
            } else {
                load(&[], 1)
            }
        });

        assert_eq!(&*cache.get(), &[b"last-good".to_vec()]);
        thread::sleep(Duration::from_millis(5));
        assert_eq!(&*cache.get(), &[b"last-good".to_vec()]);
        assert_eq!(loads.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn first_failure_is_cached_to_avoid_repeated_system_loads() {
        let loads = Arc::new(AtomicUsize::new(0));
        let loads_for_loader = Arc::clone(&loads);
        let cache = NativeCertCache::with_loader(Duration::from_secs(60), move || {
            loads_for_loader.fetch_add(1, Ordering::SeqCst);
            load(&[], 1)
        });

        assert!(cache.get().is_empty());
        assert!(cache.get().is_empty());
        assert_eq!(loads.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn partial_success_is_cached() {
        let cache =
            NativeCertCache::with_loader(Duration::from_secs(60), || load(&[b"usable-cert"], 1));

        assert_eq!(&*cache.get(), &[b"usable-cert".to_vec()]);
        assert_eq!(&*cache.get(), &[b"usable-cert".to_vec()]);
    }

    #[test]
    fn public_cache_can_be_loaded_and_invalidated() {
        let before = native_certificates_der();
        invalidate_native_certificate_cache();
        let after = native_certificates_der();

        assert!(!before.is_empty());
        assert!(!after.is_empty());
    }
}
