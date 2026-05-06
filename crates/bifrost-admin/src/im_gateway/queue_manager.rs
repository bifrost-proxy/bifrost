//! Session queue manager for IM guide mode and queue mode.
//!
//! Two messaging modes when a session is busy:
//! - **Guide mode** (default): inject message mid-turn, consumed after current tool call batch
//! - **Queue mode** (`/q <msg>`): FIFO queue, processed after current turn completes

use dashmap::DashMap;
use serde::Serialize;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

/// A queued message with a sequence number.
#[derive(Debug, Clone, Serialize)]
pub struct QueueItem {
    pub seq: u64,
    pub message: String,
}

/// Per-session queue state.
struct SessionQueue {
    /// Auto-incrementing sequence counter.
    next_seq: u64,
    /// FIFO queue of pending messages.
    items: VecDeque<QueueItem>,
}

impl Default for SessionQueue {
    fn default() -> Self {
        Self {
            next_seq: 1,
            items: VecDeque::new(),
        }
    }
}

/// Manages guide-mode injection channels and queue-mode FIFO queues per session.
pub struct SessionQueueManager {
    /// Guide message slots: only the latest message is kept (overwrite semantics).
    /// The turn loop polls this after each tool call batch.
    guide_slots: DashMap<String, Arc<Mutex<Option<String>>>>,

    /// Queue-mode FIFO: processed sequentially after each turn completes.
    queues: DashMap<String, SessionQueue>,
}

/// Maximum number of queued messages per session.
const MAX_QUEUE_SIZE: usize = 10;

impl SessionQueueManager {
    pub fn new() -> Self {
        Self {
            guide_slots: DashMap::new(),
            queues: DashMap::new(),
        }
    }

    // ── Guide mode ───────────────────────────────────────────────────────

    /// Get or create the guide channel for a session.
    /// The returned `Arc<Mutex<Option<String>>>` is shared between the event loop
    /// (writer) and the turn loop (reader).
    pub fn get_or_create_guide_channel(&self, session_key: &str) -> Arc<Mutex<Option<String>>> {
        self.guide_slots
            .entry(session_key.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(None)))
            .clone()
    }

    /// Inject a guide message for a busy session (overwrite semantics).
    /// Returns the previous message if one was pending (got replaced).
    pub fn inject_guide(&self, session_key: &str, msg: String) -> Option<String> {
        let channel = self.get_or_create_guide_channel(session_key);
        let mut guard = channel.lock().unwrap();
        let previous = guard.take();
        *guard = Some(msg);
        previous
    }

    // ── Queue mode ───────────────────────────────────────────────────────

    /// Push a message into the queue. Returns the current queue snapshot.
    /// Returns `Err` if the queue is full.
    pub fn push_queue(
        &self,
        session_key: &str,
        msg: String,
    ) -> Result<Vec<QueueItem>, &'static str> {
        let mut entry = self.queues.entry(session_key.to_string()).or_default();
        let queue = entry.value_mut();

        if queue.items.len() >= MAX_QUEUE_SIZE {
            return Err("排队已满（最多 10 条），请等待当前消息处理完成");
        }

        let seq = queue.next_seq;
        queue.next_seq += 1;
        queue.items.push_back(QueueItem { seq, message: msg });

        Ok(queue.items.iter().cloned().collect())
    }

    /// Remove a queued message by sequence number.
    /// Returns `true` if found and removed.
    pub fn remove_queue(&self, session_key: &str, seq: u64) -> bool {
        if let Some(mut entry) = self.queues.get_mut(session_key) {
            let queue = entry.value_mut();
            let before = queue.items.len();
            queue.items.retain(|item| item.seq != seq);
            return queue.items.len() < before;
        }
        false
    }

    /// Pop the next queued message (FIFO).
    pub fn pop_queue(&self, session_key: &str) -> Option<String> {
        if let Some(mut entry) = self.queues.get_mut(session_key) {
            return entry.value_mut().items.pop_front().map(|item| item.message);
        }
        None
    }

    /// Get the current queue status for a session.
    pub fn queue_status(&self, session_key: &str) -> Vec<QueueItem> {
        self.queues
            .get(session_key)
            .map(|entry| entry.value().items.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Clear all state (guide + queue) for a session.
    pub fn clear_session(&self, session_key: &str) {
        self.guide_slots.remove(session_key);
        self.queues.remove(session_key);
    }
}

impl Default for SessionQueueManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_guide_inject_overwrite() {
        let mgr = SessionQueueManager::new();
        let prev = mgr.inject_guide("s1", "msg1".into());
        assert!(prev.is_none());

        let prev = mgr.inject_guide("s1", "msg2".into());
        assert_eq!(prev.as_deref(), Some("msg1"));

        // Consumer reads the latest
        let ch = mgr.get_or_create_guide_channel("s1");
        let taken = ch.lock().unwrap().take();
        assert_eq!(taken.as_deref(), Some("msg2"));
    }

    #[test]
    fn test_queue_push_pop() {
        let mgr = SessionQueueManager::new();
        let items = mgr.push_queue("s1", "a".into()).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].seq, 1);

        let items = mgr.push_queue("s1", "b".into()).unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[1].seq, 2);

        assert_eq!(mgr.pop_queue("s1").as_deref(), Some("a"));
        assert_eq!(mgr.pop_queue("s1").as_deref(), Some("b"));
        assert!(mgr.pop_queue("s1").is_none());
    }

    #[test]
    fn test_queue_remove() {
        let mgr = SessionQueueManager::new();
        mgr.push_queue("s1", "a".into()).unwrap();
        mgr.push_queue("s1", "b".into()).unwrap();
        mgr.push_queue("s1", "c".into()).unwrap();

        assert!(mgr.remove_queue("s1", 2));
        let status = mgr.queue_status("s1");
        assert_eq!(status.len(), 2);
        assert_eq!(status[0].message, "a");
        assert_eq!(status[1].message, "c");
    }

    #[test]
    fn test_queue_max_size() {
        let mgr = SessionQueueManager::new();
        for i in 0..MAX_QUEUE_SIZE {
            mgr.push_queue("s1", format!("msg{i}")).unwrap();
        }
        assert!(mgr.push_queue("s1", "overflow".into()).is_err());
    }

    #[test]
    fn test_clear_session() {
        let mgr = SessionQueueManager::new();
        mgr.inject_guide("s1", "guide".into());
        mgr.push_queue("s1", "queued".into()).unwrap();
        mgr.clear_session("s1");

        let ch = mgr.get_or_create_guide_channel("s1");
        assert!(ch.lock().unwrap().is_none());
        assert!(mgr.queue_status("s1").is_empty());
    }

    /// Test the guide channel shared between producer (IM event loop) and
    /// consumer (turn loop). This simulates the real flow:
    /// 1. Event loop injects a guide message
    /// 2. Turn loop polls and consumes it
    /// 3. Channel is empty after consumption
    #[test]
    fn test_guide_channel_producer_consumer_flow() {
        let mgr = Arc::new(SessionQueueManager::new());

        // Simulate the turn loop getting its channel reference
        let channel = mgr.get_or_create_guide_channel("session_1");

        // Nothing initially
        assert!(channel.lock().unwrap().is_none());

        // Simulate event loop injecting a guide message
        mgr.inject_guide("session_1", "请继续分析日志".into());

        // Turn loop polls: should see the message
        let msg = channel.lock().unwrap().take();
        assert_eq!(msg.as_deref(), Some("请继续分析日志"));

        // After take(), channel is empty again
        assert!(channel.lock().unwrap().is_none());
    }

    /// Test that guide overwrite + queue coexist for the same session.
    /// This simulates: user sends guide, then /q, then another guide.
    #[test]
    fn test_guide_and_queue_coexistence() {
        let mgr = SessionQueueManager::new();

        // Guide inject
        mgr.inject_guide("s1", "guide1".into());

        // Queue push
        mgr.push_queue("s1", "queued1".into()).unwrap();
        mgr.push_queue("s1", "queued2".into()).unwrap();

        // Guide overwrite
        mgr.inject_guide("s1", "guide2".into());

        // Verify guide has latest value
        let ch = mgr.get_or_create_guide_channel("s1");
        assert_eq!(ch.lock().unwrap().take().as_deref(), Some("guide2"));

        // Verify queue is independent
        assert_eq!(mgr.pop_queue("s1").as_deref(), Some("queued1"));
        assert_eq!(mgr.pop_queue("s1").as_deref(), Some("queued2"));
        assert!(mgr.pop_queue("s1").is_none());
    }

    /// Test queue_status returns correct snapshot without modifying state.
    #[test]
    fn test_queue_status_is_readonly() {
        let mgr = SessionQueueManager::new();
        mgr.push_queue("s1", "a".into()).unwrap();
        mgr.push_queue("s1", "b".into()).unwrap();

        // Call status multiple times — should be consistent
        let status1 = mgr.queue_status("s1");
        let status2 = mgr.queue_status("s1");
        assert_eq!(status1.len(), 2);
        assert_eq!(status2.len(), 2);
        assert_eq!(status1[0].message, "a");
        assert_eq!(status1[1].message, "b");

        // Pop should still return both
        assert_eq!(mgr.pop_queue("s1").as_deref(), Some("a"));
        assert_eq!(mgr.pop_queue("s1").as_deref(), Some("b"));
    }

    /// Test session isolation: operations on one session don't affect another.
    #[test]
    fn test_session_isolation() {
        let mgr = SessionQueueManager::new();

        mgr.inject_guide("s1", "guide-s1".into());
        mgr.inject_guide("s2", "guide-s2".into());
        mgr.push_queue("s1", "q-s1".into()).unwrap();
        mgr.push_queue("s2", "q-s2-a".into()).unwrap();
        mgr.push_queue("s2", "q-s2-b".into()).unwrap();

        // Clear s1 only
        mgr.clear_session("s1");

        // s1 is empty
        let ch1 = mgr.get_or_create_guide_channel("s1");
        assert!(ch1.lock().unwrap().is_none());
        assert!(mgr.queue_status("s1").is_empty());

        // s2 is untouched
        let ch2 = mgr.get_or_create_guide_channel("s2");
        assert_eq!(ch2.lock().unwrap().take().as_deref(), Some("guide-s2"));
        assert_eq!(mgr.queue_status("s2").len(), 2);
    }

    /// Test remove with non-existent seq returns false.
    #[test]
    fn test_remove_nonexistent_seq() {
        let mgr = SessionQueueManager::new();
        mgr.push_queue("s1", "a".into()).unwrap();

        assert!(!mgr.remove_queue("s1", 999));
        assert!(!mgr.remove_queue("nonexistent_session", 1));
    }

    /// Test fix for guide message loss at turn end:
    /// When a guide message is injected just as the agent turn is finishing
    /// (model returns finish_reason=stop with no tool calls), the guide_channel
    /// was never consumed because the consumption checkpoint only runs after
    /// tool calls. This test verifies the drain-before-clear pattern.
    #[test]
    fn test_guide_drain_before_clear_prevents_loss() {
        let mgr = SessionQueueManager::new();

        // Simulate: turn loop gets channel reference at start
        let channel = mgr.get_or_create_guide_channel("s1");

        // Simulate: user sends a guide message just as turn is finishing
        mgr.inject_guide("s1", "请帮我分析这个问题".into());

        // ── The fix: drain guide_channel BEFORE checking queue/clear ──
        // This is what `run_agent_chat_with_interleave` now does after turn completes.
        let unconsumed = channel.lock().unwrap().take();
        assert_eq!(unconsumed.as_deref(), Some("请帮我分析这个问题"));

        // If we had called clear_session without draining first, the message
        // would have been lost (the old bug).
        // After draining, clear_session is safe:
        assert!(channel.lock().unwrap().is_none());
        mgr.clear_session("s1");
    }

    /// Test that the drain-then-queue priority works correctly:
    /// Guide messages take priority over queued messages.
    #[test]
    fn test_guide_priority_over_queue_at_turn_end() {
        let mgr = SessionQueueManager::new();
        let channel = mgr.get_or_create_guide_channel("s1");

        // Both guide and queue are pending at turn end
        mgr.inject_guide("s1", "guide_msg".into());
        mgr.push_queue("s1", "queued_msg".into()).unwrap();

        // Fix logic: first drain guide, then check queue
        let guide = channel.lock().unwrap().take();
        assert_eq!(guide.as_deref(), Some("guide_msg"));

        // Queue remains for the next iteration
        assert_eq!(mgr.pop_queue("s1").as_deref(), Some("queued_msg"));
        assert!(mgr.pop_queue("s1").is_none());
    }

    /// Test concurrent access from multiple threads.
    #[test]
    fn test_concurrent_access() {
        let mgr = Arc::new(SessionQueueManager::new());
        let channel = mgr.get_or_create_guide_channel("s1");

        // Spawn writer thread (simulates event loop)
        let mgr_writer = mgr.clone();
        let writer = std::thread::spawn(move || {
            for i in 0..100 {
                mgr_writer.inject_guide("s1", format!("guide_{i}"));
                mgr_writer.push_queue("s1", format!("queue_{i}")).ok();
            }
        });

        // Spawn reader thread (simulates turn loop)
        let channel_reader = channel.clone();
        let reader = std::thread::spawn(move || {
            let mut guide_count = 0;
            for _ in 0..200 {
                if channel_reader.lock().unwrap().take().is_some() {
                    guide_count += 1;
                }
                std::thread::sleep(std::time::Duration::from_micros(50));
            }
            guide_count
        });

        writer.join().unwrap();
        let guide_reads = reader.join().unwrap();

        // At least some guides should have been read
        assert!(guide_reads > 0, "expected some guide reads, got 0");

        // Queue should have some items (up to MAX_QUEUE_SIZE)
        let remaining = mgr.queue_status("s1");
        assert!(remaining.len() <= MAX_QUEUE_SIZE);
    }
}
