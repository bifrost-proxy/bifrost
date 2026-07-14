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
}

type NativeCertLoader = dyn Fn() -> NativeCertLoad + Send + Sync;

struct NativeCertCache {
    state: RwLock<Option<CachedNativeCerts>>,
    refresh_lock: Mutex<()>,
    ttl: Duration,
    loader: Arc<NativeCertLoader>,
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
        }
    }

    fn get(&self) -> Arc<[Vec<u8>]> {
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
                has_stale_snapshot = previous.is_some(),
                "failed to refresh native certificate trust store"
            );
            if let Some(previous) = previous {
                self.publish(Arc::clone(&previous), now);
                return previous;
            }
        } else if loaded.error_count > 0 {
            tracing::warn!(
                cert_count = loaded.certificates_der.len(),
                error_count = loaded.error_count,
                "native certificate trust store loaded with partial errors"
            );
        } else {
            tracing::debug!(
                cert_count = loaded.certificates_der.len(),
                "loaded native certificate trust store"
            );
        }

        let certificates_der = loaded.certificates_der;
        self.publish(Arc::clone(&certificates_der), now);
        certificates_der
    }

    fn fresh_snapshot(&self, now: Instant) -> Option<Arc<[Vec<u8>]>> {
        self.state
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
            .filter(|cached| cached.expires_at > now)
            .map(|cached| Arc::clone(&cached.certificates_der))
    }

    fn publish(&self, certificates_der: Arc<[Vec<u8>]>, now: Instant) {
        let mut state = self
            .state
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *state = Some(CachedNativeCerts {
            certificates_der,
            expires_at: now + self.ttl,
        });
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

        assert_eq!(&*cache.get(), &[b"cert-1".to_vec()]);
        assert_eq!(&*cache.get(), &[b"cert-1".to_vec()]);
        cache.invalidate();
        assert_eq!(&*cache.get(), &[b"cert-2".to_vec()]);
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
}
