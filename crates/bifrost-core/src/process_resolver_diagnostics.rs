use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessResolverDiagnosticsSnapshot {
    pub lookup_requests_total: u64,
    pub positive_cache_hits_total: u64,
    pub negative_cache_hits_total: u64,
    pub snapshot_hits_total: u64,
    pub snapshot_misses_total: u64,
    pub snapshot_refreshes_total: u64,
    pub snapshot_refresh_failures_total: u64,
    pub scan_duration_total_us: u64,
    pub scan_duration_max_us: u64,
    pub scanned_pids_total: u64,
    pub scanned_fds_total: u64,
    pub resolved_total: u64,
    pub unresolved_total: u64,
}

#[derive(Debug, Default)]
pub struct ProcessResolverDiagnostics {
    lookup_requests_total: AtomicU64,
    positive_cache_hits_total: AtomicU64,
    negative_cache_hits_total: AtomicU64,
    snapshot_hits_total: AtomicU64,
    snapshot_misses_total: AtomicU64,
    snapshot_refreshes_total: AtomicU64,
    snapshot_refresh_failures_total: AtomicU64,
    scan_duration_total_us: AtomicU64,
    scan_duration_max_us: AtomicU64,
    scanned_pids_total: AtomicU64,
    scanned_fds_total: AtomicU64,
    resolved_total: AtomicU64,
    unresolved_total: AtomicU64,
}

impl ProcessResolverDiagnostics {
    #[inline]
    pub fn record_lookup_result(&self, resolved: bool) {
        self.lookup_requests_total.fetch_add(1, Ordering::Relaxed);
        if resolved {
            self.resolved_total.fetch_add(1, Ordering::Relaxed);
        } else {
            self.unresolved_total.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[inline]
    pub fn record_cache_hit(&self, resolved: bool) {
        if resolved {
            self.positive_cache_hits_total
                .fetch_add(1, Ordering::Relaxed);
        } else {
            self.negative_cache_hits_total
                .fetch_add(1, Ordering::Relaxed);
        }
    }

    #[inline]
    pub fn record_snapshot_hit(&self) {
        self.snapshot_hits_total.fetch_add(1, Ordering::Relaxed);
    }

    #[inline]
    pub fn record_snapshot_miss(&self) {
        self.snapshot_misses_total.fetch_add(1, Ordering::Relaxed);
    }

    #[inline]
    pub fn record_snapshot_refresh(
        &self,
        duration_us: u64,
        scanned_pids: usize,
        scanned_fds: usize,
        failed: bool,
    ) {
        self.snapshot_refreshes_total
            .fetch_add(1, Ordering::Relaxed);
        if failed {
            self.snapshot_refresh_failures_total
                .fetch_add(1, Ordering::Relaxed);
        }
        self.scan_duration_total_us
            .fetch_add(duration_us, Ordering::Relaxed);
        self.scan_duration_max_us
            .fetch_max(duration_us, Ordering::Relaxed);
        self.scanned_pids_total
            .fetch_add(scanned_pids as u64, Ordering::Relaxed);
        self.scanned_fds_total
            .fetch_add(scanned_fds as u64, Ordering::Relaxed);
    }

    pub fn snapshot(&self) -> ProcessResolverDiagnosticsSnapshot {
        ProcessResolverDiagnosticsSnapshot {
            lookup_requests_total: self.lookup_requests_total.load(Ordering::Relaxed),
            positive_cache_hits_total: self.positive_cache_hits_total.load(Ordering::Relaxed),
            negative_cache_hits_total: self.negative_cache_hits_total.load(Ordering::Relaxed),
            snapshot_hits_total: self.snapshot_hits_total.load(Ordering::Relaxed),
            snapshot_misses_total: self.snapshot_misses_total.load(Ordering::Relaxed),
            snapshot_refreshes_total: self.snapshot_refreshes_total.load(Ordering::Relaxed),
            snapshot_refresh_failures_total: self
                .snapshot_refresh_failures_total
                .load(Ordering::Relaxed),
            scan_duration_total_us: self.scan_duration_total_us.load(Ordering::Relaxed),
            scan_duration_max_us: self.scan_duration_max_us.load(Ordering::Relaxed),
            scanned_pids_total: self.scanned_pids_total.load(Ordering::Relaxed),
            scanned_fds_total: self.scanned_fds_total.load(Ordering::Relaxed),
            resolved_total: self.resolved_total.load(Ordering::Relaxed),
            unresolved_total: self.unresolved_total.load(Ordering::Relaxed),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_accumulates_lookup_cache_and_scan_statistics() {
        let diagnostics = ProcessResolverDiagnostics::default();
        diagnostics.record_lookup_result(true);
        diagnostics.record_lookup_result(false);
        diagnostics.record_cache_hit(true);
        diagnostics.record_cache_hit(false);
        diagnostics.record_snapshot_hit();
        diagnostics.record_snapshot_miss();
        diagnostics.record_snapshot_refresh(12, 3, 20, false);
        diagnostics.record_snapshot_refresh(30, 4, 25, true);

        assert_eq!(
            diagnostics.snapshot(),
            ProcessResolverDiagnosticsSnapshot {
                lookup_requests_total: 2,
                positive_cache_hits_total: 1,
                negative_cache_hits_total: 1,
                snapshot_hits_total: 1,
                snapshot_misses_total: 1,
                snapshot_refreshes_total: 2,
                snapshot_refresh_failures_total: 1,
                scan_duration_total_us: 42,
                scan_duration_max_us: 30,
                scanned_pids_total: 7,
                scanned_fds_total: 45,
                resolved_total: 1,
                unresolved_total: 1,
            }
        );
    }
}
