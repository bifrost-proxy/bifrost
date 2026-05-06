use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

use crate::traffic::TrafficRecord;
use crate::traffic_db::SharedTrafficDbStore;

pub type TrafficUpdater = Arc<dyn Fn(&mut TrafficRecord) + Send + Sync>;

#[derive(Clone)]
pub enum TrafficCommand {
    Record(Box<TrafficRecord>),
    Update { id: String, updater: TrafficUpdater },
}

impl std::fmt::Debug for TrafficCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TrafficCommand::Record(record) => f.debug_tuple("Record").field(&record.id).finish(),
            TrafficCommand::Update { id, .. } => f
                .debug_struct("Update")
                .field("id", id)
                .field("updater", &"<fn>")
                .finish(),
        }
    }
}

pub struct AsyncTrafficWriter {
    tx: mpsc::Sender<TrafficCommand>,
}

impl AsyncTrafficWriter {
    pub fn new(buffer_size: usize) -> (Self, mpsc::Receiver<TrafficCommand>) {
        let (tx, rx) = mpsc::channel(buffer_size);
        (Self { tx }, rx)
    }

    #[inline]
    pub fn record(&self, record: TrafficRecord) {
        if let Err(e) = self.tx.try_send(TrafficCommand::Record(Box::new(record))) {
            match e {
                mpsc::error::TrySendError::Full(_) => {
                    warn!("Traffic channel full, dropping record");
                }
                mpsc::error::TrySendError::Closed(_) => {
                    error!("Traffic channel closed");
                }
            }
        }
    }

    #[inline]
    pub fn update_by_id<F>(&self, id: &str, updater: F)
    where
        F: Fn(&mut TrafficRecord) + Send + Sync + 'static,
    {
        let cmd = TrafficCommand::Update {
            id: id.to_string(),
            updater: Arc::new(updater),
        };
        if let Err(e) = self.tx.try_send(cmd) {
            match e {
                mpsc::error::TrySendError::Full(_) => {
                    warn!("Traffic channel full, dropping update for {}", id);
                }
                mpsc::error::TrySendError::Closed(_) => {
                    error!("Traffic channel closed");
                }
            }
        }
    }
}

impl Clone for AsyncTrafficWriter {
    fn clone(&self) -> Self {
        Self {
            tx: self.tx.clone(),
        }
    }
}

pub type SharedAsyncTrafficWriter = Arc<AsyncTrafficWriter>;

pub fn start_async_traffic_processor(
    mut rx: mpsc::Receiver<TrafficCommand>,
    traffic_db_store: SharedTrafficDbStore,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        info!("Async traffic processor started");

        let mut batch: Vec<Box<TrafficRecord>> = Vec::with_capacity(64);
        let mut updates: Vec<(String, TrafficUpdater)> = Vec::with_capacity(32);

        loop {
            batch.clear();
            updates.clear();

            match rx.recv().await {
                Some(cmd) => {
                    match cmd {
                        TrafficCommand::Record(record) => batch.push(record),
                        TrafficCommand::Update { id, updater } => updates.push((id, updater)),
                    }

                    while batch.len() < 64 && updates.len() < 32 {
                        match rx.try_recv() {
                            Ok(TrafficCommand::Record(record)) => batch.push(record),
                            Ok(TrafficCommand::Update { id, updater }) => {
                                updates.push((id, updater))
                            }
                            Err(mpsc::error::TryRecvError::Empty) => break,
                            Err(mpsc::error::TryRecvError::Disconnected) => {
                                info!("Traffic channel disconnected, processing remaining batch");
                                break;
                            }
                        }
                    }

                    if !batch.is_empty() {
                        let batch_size = batch.len();
                        let records: Vec<TrafficRecord> =
                            batch.drain(..).map(|record| *record).collect();
                        let store = traffic_db_store.clone();
                        if let Err(e) = tokio::task::spawn_blocking(move || {
                            store.record_batch(records);
                        })
                        .await
                        {
                            error!("Traffic record_batch task panicked: {}", e);
                        }
                        debug!("Processed {} traffic records", batch_size);
                    }

                    if !updates.is_empty() {
                        let update_count = updates.len();
                        let has_subscribers = traffic_db_store.has_traffic_event_subscribers();
                        let drained = std::mem::take(&mut updates);
                        let store = traffic_db_store.clone();
                        if let Err(e) = tokio::task::spawn_blocking(move || {
                            if has_subscribers {
                                for (id, updater) in drained {
                                    store.update_by_id(&id, |r| updater(r));
                                }
                            } else {
                                let mut grouped: HashMap<String, Vec<TrafficUpdater>> =
                                    HashMap::new();
                                let mut order: Vec<String> = Vec::new();
                                for (id, updater) in drained {
                                    let bucket = grouped.entry(id.clone()).or_insert_with(|| {
                                        order.push(id.clone());
                                        Vec::new()
                                    });
                                    bucket.push(updater);
                                }

                                for id in order {
                                    if let Some(grouped_updaters) = grouped.remove(&id) {
                                        store.update_by_id(&id, move |record| {
                                            for updater in &grouped_updaters {
                                                updater(record);
                                            }
                                        });
                                    }
                                }
                            }
                        })
                        .await
                        {
                            error!("Traffic update task panicked: {}", e);
                        }
                        debug!("Processed {} traffic updates", update_count);
                    }
                }
                None => {
                    info!("Traffic channel closed, shutting down processor");
                    break;
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::traffic_db::TrafficDbStore;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    fn make_temp_dir(name: &str) -> std::path::PathBuf {
        let pid = std::process::id();
        let ts = chrono::Utc::now().timestamp_millis();
        let dir = std::env::temp_dir().join(format!("bifrost-{}-{}-{}", name, pid, ts));
        let _ = std::fs::create_dir_all(&dir);
        dir
    }

    async fn wait_until<F>(timeout: Duration, mut condition: F) -> bool
    where
        F: FnMut() -> bool,
    {
        let deadline = Instant::now() + timeout;
        loop {
            if condition() {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    #[tokio::test]
    async fn test_async_traffic_writer() {
        let (writer, rx) = AsyncTrafficWriter::new(100);
        let dir = make_temp_dir("async-traffic-writer");
        let traffic_dir = dir.join("traffic");
        let _ = std::fs::create_dir_all(&traffic_dir);
        let db_store = Arc::new(
            TrafficDbStore::new(traffic_dir, 1000, 0, None)
                .expect("failed to create traffic db store"),
        );

        let handle = start_async_traffic_processor(rx, db_store.clone());

        let record = TrafficRecord::new(
            "test-1".to_string(),
            "GET".to_string(),
            "https://example.com".to_string(),
        );
        writer.record(record);

        assert!(
            wait_until(Duration::from_secs(2), || db_store
                .get_by_id("test-1")
                .is_some())
            .await
        );

        drop(writer);
        let _ = tokio::time::timeout(Duration::from_secs(1), handle).await;
    }

    #[tokio::test]
    async fn test_async_traffic_update() {
        let (writer, rx) = AsyncTrafficWriter::new(100);
        let dir = make_temp_dir("async-traffic-update");
        let traffic_dir = dir.join("traffic");
        let _ = std::fs::create_dir_all(&traffic_dir);
        let db_store = Arc::new(
            TrafficDbStore::new(traffic_dir, 1000, 0, None)
                .expect("failed to create traffic db store"),
        );

        let handle = start_async_traffic_processor(rx, db_store.clone());

        let record = TrafficRecord::new(
            "test-update".to_string(),
            "GET".to_string(),
            "https://example.com".to_string(),
        );
        writer.record(record);

        assert!(
            wait_until(Duration::from_secs(2), || db_store
                .get_by_id("test-update")
                .is_some())
            .await
        );

        writer.update_by_id("test-update", |r| {
            r.status = 200;
            r.duration_ms = 100;
        });

        assert!(
            wait_until(Duration::from_secs(2), || {
                db_store
                    .get_by_id("test-update")
                    .map(|record| record.status == 200 && record.duration_ms == 100)
                    .unwrap_or(false)
            })
            .await
        );

        let updated = db_store.get_by_id("test-update").unwrap();
        assert_eq!(updated.status, 200);
        assert_eq!(updated.duration_ms, 100);

        drop(writer);
        let _ = tokio::time::timeout(Duration::from_secs(1), handle).await;
    }

    #[tokio::test]
    async fn test_batch_processing() {
        let (writer, rx) = AsyncTrafficWriter::new(1000);
        let dir = make_temp_dir("async-traffic-batch");
        let traffic_dir = dir.join("traffic");
        let _ = std::fs::create_dir_all(&traffic_dir);
        let db_store = Arc::new(
            TrafficDbStore::new(traffic_dir, 10000, 0, None)
                .expect("failed to create traffic db store"),
        );

        let handle = start_async_traffic_processor(rx, db_store.clone());

        for i in 0..100 {
            let record = TrafficRecord::new(
                format!("batch-{}", i),
                "GET".to_string(),
                format!("https://example.com/{}", i),
            );
            writer.record(record);
        }

        assert!(wait_until(Duration::from_secs(2), || db_store.count() == 100).await);
        assert_eq!(db_store.count(), 100);

        drop(writer);
        let _ = tokio::time::timeout(Duration::from_secs(1), handle).await;
    }
}
