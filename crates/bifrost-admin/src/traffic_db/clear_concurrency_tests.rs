use super::{TrafficDbStore, TrafficStoreEvent};
use crate::traffic::TrafficRecord;
use std::sync::{mpsc, Arc, Barrier};
use std::time::Duration;

fn record(id: &str) -> TrafficRecord {
    let mut record = TrafficRecord::new(
        id.to_owned(),
        "GET".into(),
        format!("http://clear.test/{id}"),
    );
    record.status = 200;
    record
}

#[test]
fn concurrent_clears_keep_db_lock_and_release_cache_before_compaction() {
    const CHILD_ENV: &str = "BIFROST_CLEAR_REGRESSION_CHILD";
    if std::env::var_os(CHILD_ENV).is_some() {
        overlapping_clear_scenario();
        return;
    }
    // A regression deadlocks native threads. Bound the entire process so old
    // code fails deterministically without leaving a hung test suite behind.
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "traffic_db::store::clear_concurrency_tests::concurrent_clears_keep_db_lock_and_release_cache_before_compaction",
            "--nocapture",
        ])
        .env(CHILD_ENV, "1")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            let output = child.wait_with_output().unwrap();
            assert!(
                status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
            break;
        }
        if std::time::Instant::now() >= deadline {
            child.kill().unwrap();
            let output = child.wait_with_output().unwrap();
            panic!(
                "overlapping clear processes deadlocked: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn overlapping_clear_scenario() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(TrafficDbStore::new(dir.path().into(), 1000, 0, None).unwrap());
    store.record(record("before"));
    let (entered_tx, entered_rx) = mpsc::channel();
    let (resume_tx, resume_rx) = mpsc::channel();
    let resume_rx = parking_lot::Mutex::new(resume_rx);
    let weak = Arc::downgrade(&store);
    let first = std::sync::atomic::AtomicBool::new(true);
    *store.clear_before_compact.write() = Some(Arc::new(move || {
        if first.swap(false, std::sync::atomic::Ordering::SeqCst) {
            let store = weak.upgrade().unwrap();
            let released_db = store.write_conn.try_lock().is_some();
            if !released_db {
                assert!(store.recent_cache.try_write().is_some());
            }
            entered_tx.send(released_db).unwrap();
            resume_rx
                .lock()
                .recv_timeout(Duration::from_secs(5))
                .unwrap();
        }
    }));
    let (second_db_tx, second_db_rx) = mpsc::channel();
    let cache_entries = std::sync::atomic::AtomicUsize::new(0);
    *store.clear_before_cache.write() = Some(Arc::new(move || {
        if cache_entries.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 1 {
            second_db_tx.send(()).unwrap();
        }
    }));
    let a = Arc::clone(&store);
    let first_clear = std::thread::spawn(move || a.clear().unwrap());
    let released_db = entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    let b = Arc::clone(&store);
    let (started_tx, started_rx) = mpsc::channel();
    let second_clear = std::thread::spawn(move || {
        started_tx.send(()).unwrap();
        b.clear().unwrap();
    });
    started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    if released_db {
        // Old clear owns cache. Wait until the second clear owns DB before
        // resuming the first at compact_db: the AB-BA cycle is now forced.
        second_db_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    }
    resume_tx.send(()).unwrap();
    first_clear.join().unwrap();
    second_clear.join().unwrap();
    assert!(store.get_by_id("before").is_none());
}

#[test]
fn many_clears_with_writes_preserve_active_and_capture_after_clear() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(TrafficDbStore::new(dir.path().into(), 10_000, 0, None).unwrap());
    store.record(record("active"));
    let barrier = Arc::new(Barrier::new(33));
    let (done_tx, done_rx) = mpsc::channel();
    let mut workers = Vec::new();
    for worker in 0..32 {
        let store = Arc::clone(&store);
        let barrier = Arc::clone(&barrier);
        let done = done_tx.clone();
        workers.push(std::thread::spawn(move || {
            barrier.wait();
            for iteration in 0..4 {
                if worker < 24 {
                    store.clear_with_active_ids(&["active".into()]).unwrap();
                } else {
                    store.record(record(&format!("during-{worker}-{iteration}")));
                }
            }
            done.send(()).unwrap();
        }));
    }
    barrier.wait();
    for _ in 0..32 {
        done_rx
            .recv_timeout(Duration::from_secs(30))
            .expect("concurrent clear/write stalled");
    }
    for worker in workers {
        worker.join().unwrap();
    }
    assert!(store.get_by_id("active").is_some());
    let epoch = store.database_epoch().to_owned();
    let sequence = store.current_sequence();
    store.clear().unwrap();
    assert!(store.get_by_id("active").is_none());
    let mut events = store.subscribe();
    for i in 0..5 {
        store.record(record(&format!("after-{i}")));
        match events.try_recv().unwrap() {
            TrafficStoreEvent::Inserted(record) => {
                assert_eq!(record.id, format!("after-{i}"));
                assert!(record.sequence >= sequence);
            }
            other => panic!("expected inserted event, got {other:?}"),
        }
    }
    assert_eq!(store.database_epoch(), epoch);
    assert_eq!(store.traffic_statistics().total_requests, 5);
    drop(store);
    let reopened = TrafficDbStore::new(dir.path().into(), 10_000, 0, None).unwrap();
    for i in 0..5 {
        assert!(reopened.get_by_id(&format!("after-{i}")).is_some());
    }
    assert_eq!(reopened.traffic_statistics().total_requests, 5);
}
