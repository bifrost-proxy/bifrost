use super::*;

#[test]
fn test_read_pool_concurrent_reads_do_not_block() {
    let dir = create_test_dir();
    let store = Arc::new(TrafficDbStore::new(dir.clone(), 1000, 0, None).unwrap());

    for i in 0..20 {
        let mut r = TrafficRecord::new(
            format!("conc-{}", i),
            "GET".to_string(),
            format!("https://example{}.com/path", i),
        );
        r.status = 200;
        store.record(r);
    }

    let barrier = Arc::new(std::sync::Barrier::new(READ_POOL_SIZE + 1));
    let mut handles = Vec::new();

    for t in 0..READ_POOL_SIZE {
        let s = store.clone();
        let b = barrier.clone();
        handles.push(std::thread::spawn(move || {
            b.wait();
            let start = Instant::now();
            let params = QueryParams {
                limit: Some(10),
                direction: Direction::Backward,
                ..Default::default()
            };
            let result = s.query(&params);
            let elapsed = start.elapsed();
            assert!(
                !result.records.is_empty(),
                "thread {} should get records",
                t
            );
            elapsed
        }));
    }

    barrier.wait();
    let durations: Vec<Duration> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    let max = durations.iter().max().unwrap();
    assert!(
        *max < Duration::from_secs(2),
        "max concurrent read took {:?}, expected < 2s",
        max
    );

    cleanup_test_dir(&dir);
}

#[test]
fn test_read_pool_pragma_query_only() {
    let dir = create_test_dir();
    let store = TrafficDbStore::new(dir.clone(), 100, 0, None).unwrap();

    for _ in 0..READ_POOL_SIZE {
        let conn = store.read_pool.acquire();
        let result = conn.execute(
            "INSERT INTO traffic_records (sequence, id, timestamp, host, method, status, \
             protocol, url, path, content_type, request_content_type, request_size, \
             response_size, duration_ms, client_ip, client_app, flags, frame_count, \
             last_frame_id, socket_is_open, socket_send_count, socket_receive_count, \
             socket_send_bytes, socket_receive_bytes, socket_frame_count, rule_count) \
             VALUES (999, 'x', 0, '', '', 0, '', '', '', '', '', 0, 0, 0, '', '', 0, 0, 0, 0, 0, 0, 0, 0, 0, 0)",
            [],
        );
        assert!(result.is_err(), "read pool connection should be query_only");
    }

    cleanup_test_dir(&dir);
}

#[test]
fn test_read_pool_under_write_pressure() {
    let dir = create_test_dir();
    let store = Arc::new(TrafficDbStore::new(dir.clone(), 5000, 0, None).unwrap());

    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));

    let (first_committed_tx, first_committed_rx) = std::sync::mpsc::sync_channel(0);
    let writer_store = store.clone();
    let writer_stop = stop.clone();
    let writer = std::thread::spawn(move || {
        let mut i = 0u64;
        while !writer_stop.load(Ordering::Relaxed) {
            let mut r = TrafficRecord::new(
                format!("wp-{}", i),
                "POST".to_string(),
                format!("https://pressure{}.com", i),
            );
            r.status = 201;
            writer_store.record(r);
            i += 1;
            if i == 1 {
                assert_eq!(
                    writer_store.count(),
                    1,
                    "first pressure record must be persisted"
                );
                first_committed_tx.send(()).unwrap();
            }
            if i.is_multiple_of(50) {
                std::thread::sleep(Duration::from_millis(1));
            }
        }
        i
    });

    // Readers of an empty database legitimately return no rows. A fixed
    // startup sleep does not establish that the writer has committed on Windows.
    first_committed_rx
        .recv_timeout(Duration::from_secs(10))
        .expect("pressure writer did not commit its first record");

    let results = pressure_readers(&store);
    stop.store(true, Ordering::Relaxed);
    let written = writer.join().unwrap();

    for (i, &count) in results.iter().enumerate() {
        assert!(
            count > 0,
            "reader {} got 0 successful reads under write pressure (written: {})",
            i,
            written
        );
        assert_eq!(
            count, 20,
            "every read must succeed while the writer stays active"
        );
    }

    cleanup_test_dir(&dir);
}

fn pressure_readers(store: &Arc<TrafficDbStore>) -> Vec<u32> {
    let mut read_handles = Vec::new();
    for _ in 0..8 {
        let s = store.clone();
        read_handles.push(std::thread::spawn(move || {
            let mut success = 0u32;
            for _ in 0..20 {
                let params = QueryParams {
                    limit: Some(10),
                    direction: Direction::Backward,
                    ..Default::default()
                };
                let result = s.query(&params);
                if !result.records.is_empty() {
                    success += 1;
                }
            }
            success
        }));
    }

    read_handles
        .into_iter()
        .map(|h| h.join().unwrap())
        .collect()
}

#[test]
fn test_read_pool_pressure_startup_can_reproduce_zero_reads_before_first_commit() {
    let dir = create_test_dir();
    let store = Arc::new(TrafficDbStore::new(dir.clone(), 5000, 0, None).unwrap());
    let (permit_tx, permit_rx) = std::sync::mpsc::sync_channel(0);
    let writer_store = store.clone();
    let writer = std::thread::spawn(move || {
        permit_rx.recv_timeout(Duration::from_secs(10)).unwrap();
        writer_store.record(epoch_test_record("first-delayed-write"));
        writer_store.count()
    });
    // Force the exact ordering allowed by the old 50ms setup: all readers
    // finish before the first commit, then the in-flight writer commits once.
    let early_reads = pressure_readers(&store);
    permit_tx.send(()).unwrap();
    let written = writer.join().unwrap();
    assert_eq!(written, 1);
    assert!(early_reads.iter().all(|count| *count == 0));
    assert!(pressure_readers(&store).iter().all(|count| *count == 20));
    cleanup_test_dir(&dir);
}
