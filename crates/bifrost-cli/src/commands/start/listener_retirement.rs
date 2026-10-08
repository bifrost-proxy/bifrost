//! Retain both endpoints during an interrupted system-proxy port transfer.
use std::future::Future;
use std::time::Duration;
use tokio::task::JoinHandle;

type ListenerTask = JoinHandle<bifrost_core::Result<()>>;
struct ListenerGuard(ListenerTask);
impl Drop for ListenerGuard {
    fn drop(&mut self) {
        self.0.abort();
    }
}

#[derive(Default)]
pub(super) struct RetiredProxyListeners(Vec<JoinHandle<()>>);
impl RetiredProxyListeners {
    pub(super) fn pending(&mut self) -> bool {
        self.0.retain(|task| !task.is_finished());
        !self.0.is_empty()
    }
    pub(super) fn retire(&mut self, listener: ListenerTask, host: String, port: u16) {
        // Construct before spawning, so cancellation before the first poll also
        // aborts the old listener rather than detaching its JoinHandle.
        let listener = ListenerGuard(listener);
        self.0.push(tokio::spawn(retire_when_unreferenced(
            listener,
            move || {
                let host = host.clone();
                async move {
                    tokio::task::spawn_blocking(move || {
                        if !bifrost_core::SystemProxyManager::is_supported() {
                            return true;
                        }
                        match bifrost_core::SystemProxyManager::get_current() {
                            Ok(current) if !current.target_matches(&host, port) => {
                                bifrost_core::SystemProxyManager::any_service_proxy_matches(
                                    &host, port,
                                )
                                .is_ok_and(|matches| !matches)
                            }
                            _ => false,
                        }
                    })
                    .await
                    .unwrap_or(false)
                }
            },
            Duration::from_secs(1),
            Duration::from_millis(250),
        )));
    }
}
impl Drop for RetiredProxyListeners {
    fn drop(&mut self) {
        for task in &self.0 {
            task.abort();
        }
    }
}

async fn retire_when_unreferenced<F, Fut>(
    _listener: ListenerGuard,
    mut no_references: F,
    interval: Duration,
    drain_grace: Duration,
) where
    F: FnMut() -> Fut,
    Fut: Future<Output = bool>,
{
    loop {
        if no_references().await {
            tokio::time::sleep(drain_grace).await;
            return;
        }
        tokio::time::sleep(interval).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    };

    struct Alive(Arc<AtomicBool>);
    impl Drop for Alive {
        fn drop(&mut self) {
            self.0.store(false, Ordering::Release);
        }
    }
    fn live_listener() -> (ListenerGuard, Arc<AtomicBool>) {
        let alive = Arc::new(AtomicBool::new(true));
        let guard = Alive(alive.clone());
        let task = tokio::spawn(async move {
            let _guard = guard;
            std::future::pending::<bifrost_core::Result<()>>().await
        });
        (ListenerGuard(task), alive)
    }
    async fn wait_until_stopped(alive: &AtomicBool) {
        tokio::time::timeout(Duration::from_secs(1), async {
            while alive.load(Ordering::Acquire) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn referenced_or_unknown_old_endpoint_stays_alive_until_verified_clear() {
        let (listener, alive) = live_listener();
        let status = Arc::new(AtomicUsize::new(1)); // referenced
        let polls = Arc::new(AtomicUsize::new(0));
        let observed = status.clone();
        let poll_count = polls.clone();
        let retirement = tokio::spawn(retire_when_unreferenced(
            listener,
            move || {
                poll_count.fetch_add(1, Ordering::AcqRel);
                std::future::ready(observed.load(Ordering::Acquire) == 0)
            },
            Duration::from_millis(1),
            Duration::ZERO,
        ));
        while polls.load(Ordering::Acquire) < 2 {
            tokio::task::yield_now().await;
        }
        assert!(alive.load(Ordering::Acquire));
        status.store(2, Ordering::Release); // query error/unknown is not clear
        let previous = polls.load(Ordering::Acquire);
        while polls.load(Ordering::Acquire) == previous {
            tokio::task::yield_now().await;
        }
        assert!(alive.load(Ordering::Acquire));
        status.store(0, Ordering::Release);
        tokio::time::timeout(Duration::from_secs(1), retirement)
            .await
            .unwrap()
            .unwrap();
        wait_until_stopped(&alive).await;
    }

    #[tokio::test]
    async fn shutdown_aborts_retired_listener_even_before_retirement_future_is_polled() {
        let (listener, alive) = live_listener();
        let retirement = tokio::spawn(retire_when_unreferenced(
            listener,
            || std::future::ready(false),
            Duration::from_secs(1),
            Duration::ZERO,
        ));
        retirement.abort();
        let _ = retirement.await;
        wait_until_stopped(&alive).await;
    }

    #[tokio::test]
    async fn retirement_registry_drop_aborts_an_unpolled_listener() {
        let alive = Arc::new(AtomicBool::new(true));
        let guard = Alive(alive.clone());
        let listener = tokio::spawn(async move {
            let _guard = guard;
            std::future::pending::<bifrost_core::Result<()>>().await
        });
        let mut registry = RetiredProxyListeners::default();
        assert!(!registry.pending());
        registry.retire(listener, "127.0.0.1".into(), 18745);
        assert!(registry.pending());
        // The current-thread test has not yielded, so no OS probe can run.
        drop(registry);
        wait_until_stopped(&alive).await;
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    #[tokio::test]
    async fn unsupported_os_retires_listener_after_drain_and_clears_pending() {
        let alive = Arc::new(AtomicBool::new(true));
        let guard = Alive(alive.clone());
        let listener = tokio::spawn(async move {
            let _guard = guard;
            std::future::pending::<bifrost_core::Result<()>>().await
        });
        let mut registry = RetiredProxyListeners::default();
        registry.retire(listener, "127.0.0.1".into(), 18745);
        assert!(registry.pending());
        wait_until_stopped(&alive).await;
        assert!(!registry.pending());
    }
}
