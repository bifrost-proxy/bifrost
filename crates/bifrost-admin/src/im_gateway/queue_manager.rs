//! Session queue manager for IM guide mode and queue mode.
//!
//! Two messaging modes when a session is busy:
//! - **Queue mode** (default for ordinary IM messages): FIFO queue, processed after current turn
//!   completes
//! - **Guide mode** (`/g <msg>` in IM or Guide selection in WebUI): mid-turn guidance for
//!   runtimes that support it

use dashmap::DashMap;
use serde::Serialize;
use std::collections::VecDeque;
use std::sync::Arc;

use bifrost_agent::session::{GuideChannel, GuideMessageChannel};

/// A queued message with a sequence number.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct QueueItem {
    pub seq: u64,
    pub message: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<bifrost_agent::ChatImageInput>,
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
    /// Guide message slots: all pending guide messages are kept and merged by
    /// the turn loop at the next guide checkpoint.
    guide_slots: DashMap<String, GuideChannel>,

    /// Guide messages that have been handed to an isolated worker process but
    /// are still part of the current active turn from the main process view.
    handed_off_guides: DashMap<String, VecDeque<String>>,

    /// Queue-mode FIFO: processed sequentially after each turn completes.
    queues: DashMap<String, SessionQueue>,
}

/// Maximum number of queued messages per session.
const MAX_QUEUE_SIZE: usize = 10;

impl SessionQueueManager {
    pub fn new() -> Self {
        Self {
            guide_slots: DashMap::new(),
            handed_off_guides: DashMap::new(),
            queues: DashMap::new(),
        }
    }

    // ── Guide mode ───────────────────────────────────────────────────────

    /// Get or create the guide channel for a session.
    /// The returned channel is shared between the event loop
    /// (writer) and the turn loop (reader).
    pub fn get_or_create_guide_channel(&self, session_key: &str) -> GuideChannel {
        self.guide_slots
            .entry(session_key.to_string())
            .or_insert_with(|| Arc::new(GuideMessageChannel::new()))
            .clone()
    }

    /// Inject a guide message for a busy session.
    /// Returns the number of guide messages now waiting to enter the loop.
    pub fn inject_guide(&self, session_key: &str, msg: String) -> usize {
        let channel = self.get_or_create_guide_channel(session_key);
        channel.push_back(msg)
    }

    /// Get pending guide messages without modifying state.
    pub fn guide_status(&self, session_key: &str) -> Vec<String> {
        let mut guides = self
            .guide_slots
            .get(session_key)
            .map(|entry| entry.snapshot())
            .unwrap_or_default();
        if let Some(entry) = self.handed_off_guides.get(session_key) {
            for guide in entry.iter() {
                if !guides.iter().any(|existing| existing == guide) {
                    guides.push(guide.clone());
                }
            }
        }
        guides
    }

    /// Record guide messages handed to an isolated worker so status endpoints
    /// can keep reporting them until the active turn finishes.
    pub fn mark_guides_handed_to_worker(&self, session_key: &str, messages: &[String]) {
        let mut entry = self
            .handed_off_guides
            .entry(session_key.to_string())
            .or_default();
        let handed_off = entry.value_mut();
        for message in messages {
            if !message.trim().is_empty() && !handed_off.iter().any(|existing| existing == message)
            {
                handed_off.push_back(message.clone());
            }
        }
    }

    /// Reconcile the guides handed to a worker against the ones it reported as
    /// actually consumed. Returns the handed-off guides that were NOT consumed
    /// (the lost-race case: a guide reached the worker over the IPC pipe after
    /// the turn-end checkpoint already ran, so it was silently dropped). The
    /// handed-off record for this session is cleared regardless, since the turn
    /// has ended. Each consumed entry is matched at most once so duplicate guide
    /// texts are handled correctly.
    pub fn reconcile_handed_off_guides(
        &self,
        session_key: &str,
        consumed: &[String],
    ) -> Vec<String> {
        let Some((_, handed_off)) = self.handed_off_guides.remove(session_key) else {
            return Vec::new();
        };
        let mut remaining_consumed: Vec<&String> =
            consumed.iter().filter(|m| !m.trim().is_empty()).collect();
        let mut unconsumed = Vec::new();
        for guide in handed_off {
            if guide.trim().is_empty() {
                continue;
            }
            if let Some(pos) = remaining_consumed.iter().position(|c| **c == guide) {
                remaining_consumed.remove(pos);
            } else {
                unconsumed.push(guide);
            }
        }
        unconsumed
    }

    // ── Queue mode ───────────────────────────────────────────────────────

    /// Push a message into the queue. Returns the current queue snapshot.
    /// Returns `Err` if the queue is full.
    pub fn push_queue(
        &self,
        session_key: &str,
        msg: String,
    ) -> Result<Vec<QueueItem>, &'static str> {
        self.push_queue_with_images(session_key, msg, Vec::new())
    }

    /// Push a message and its image attachments into the queue.
    /// Returns the current queue snapshot. Returns `Err` if the queue is full.
    pub fn push_queue_with_images(
        &self,
        session_key: &str,
        msg: String,
        images: Vec<bifrost_agent::ChatImageInput>,
    ) -> Result<Vec<QueueItem>, &'static str> {
        let mut entry = self.queues.entry(session_key.to_string()).or_default();
        let queue = entry.value_mut();

        if queue.items.len() >= MAX_QUEUE_SIZE {
            return Err("排队已满（最多 10 条），请等待当前消息处理完成");
        }

        let seq = queue.next_seq;
        queue.next_seq += 1;
        queue.items.push_back(QueueItem {
            seq,
            message: msg,
            images,
        });

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
        self.pop_queue_item(session_key).map(|item| item.message)
    }

    /// Pop the next queued message with attachments (FIFO).
    pub fn pop_queue_item(&self, session_key: &str) -> Option<QueueItem> {
        if let Some(mut entry) = self.queues.get_mut(session_key) {
            return entry.value_mut().items.pop_front();
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
        self.handed_off_guides.remove(session_key);
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
    fn test_guide_inject_appends() {
        let mgr = SessionQueueManager::new();
        let count = mgr.inject_guide("s1", "msg1".into());
        assert_eq!(count, 1);

        let count = mgr.inject_guide("s1", "msg2".into());
        assert_eq!(count, 2);

        // Consumer reads all pending guides in insertion order.
        let ch = mgr.get_or_create_guide_channel("s1");
        let taken: Vec<String> = ch.lock().unwrap().drain(..).collect();
        assert_eq!(taken, vec!["msg1".to_string(), "msg2".to_string()]);
    }

    #[test]
    fn test_guide_status_is_readonly() {
        let mgr = SessionQueueManager::new();
        mgr.inject_guide("s1", "msg1".into());
        mgr.inject_guide("s1", "msg2".into());

        let status1 = mgr.guide_status("s1");
        let status2 = mgr.guide_status("s1");
        assert_eq!(status1, vec!["msg1".to_string(), "msg2".to_string()]);
        assert_eq!(status2, status1);

        let ch = mgr.get_or_create_guide_channel("s1");
        let taken: Vec<String> = ch.lock().unwrap().drain(..).collect();
        assert_eq!(taken, vec!["msg1".to_string(), "msg2".to_string()]);
    }

    #[test]
    fn test_guide_status_includes_worker_handoff_snapshot() {
        let mgr = SessionQueueManager::new();
        mgr.inject_guide("s1", "guide before handoff".into());
        let ch = mgr.get_or_create_guide_channel("s1");
        let handed_off: Vec<String> = ch.lock().unwrap().drain(..).collect();

        mgr.mark_guides_handed_to_worker("s1", &handed_off);

        assert_eq!(
            mgr.guide_status("s1"),
            vec!["guide before handoff".to_string()]
        );
        mgr.clear_session("s1");
        assert!(mgr.guide_status("s1").is_empty());
    }

    #[test]
    fn test_reconcile_handed_off_guides_returns_unconsumed() {
        let mgr = SessionQueueManager::new();
        // Two guides handed to the worker.
        mgr.mark_guides_handed_to_worker("s1", &["consumed".into(), "lost".into()]);
        // Worker reports it only consumed the first one (the second lost the
        // IPC race against the turn-end checkpoint).
        let unconsumed = mgr.reconcile_handed_off_guides("s1", &["consumed".into()]);
        assert_eq!(unconsumed, vec!["lost".to_string()]);
        // Handoff record is cleared after reconciliation regardless.
        assert!(mgr.guide_status("s1").is_empty());
    }

    #[test]
    fn test_reconcile_handed_off_guides_all_consumed() {
        let mgr = SessionQueueManager::new();
        mgr.mark_guides_handed_to_worker("s1", &["a".into(), "b".into()]);
        let unconsumed = mgr.reconcile_handed_off_guides("s1", &["a".into(), "b".into()]);
        assert!(unconsumed.is_empty());
        assert!(mgr.guide_status("s1").is_empty());
    }

    #[test]
    fn test_reconcile_handed_off_guides_none_handed() {
        let mgr = SessionQueueManager::new();
        let unconsumed = mgr.reconcile_handed_off_guides("s1", &["x".into()]);
        assert!(unconsumed.is_empty());
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
    fn test_queue_preserves_image_attachments() {
        let mgr = SessionQueueManager::new();
        mgr.push_queue_with_images(
            "s1",
            "look at this".into(),
            vec![bifrost_agent::ChatImageInput {
                mime_type: "image/png".to_string(),
                data: "aGVsbG8=".to_string(),
            }],
        )
        .unwrap();

        let item = mgr.pop_queue_item("s1").expect("queued item");
        assert_eq!(item.message, "look at this");
        assert_eq!(item.images.len(), 1);
        assert_eq!(item.images[0].mime_type, "image/png");
        assert_eq!(item.images[0].data, "aGVsbG8=");
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
        assert!(ch.lock().unwrap().is_empty());
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
        assert!(channel.lock().unwrap().is_empty());

        // Simulate event loop injecting a guide message
        mgr.inject_guide("session_1", "请继续分析日志".into());

        // Turn loop polls: should see the message
        let msg: Vec<String> = channel.lock().unwrap().drain(..).collect();
        assert_eq!(msg, vec!["请继续分析日志".to_string()]);

        // After drain(), channel is empty again
        assert!(channel.lock().unwrap().is_empty());
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

        // Guide append
        mgr.inject_guide("s1", "guide2".into());

        // Verify guides keep insertion order
        let ch = mgr.get_or_create_guide_channel("s1");
        let guides: Vec<String> = ch.lock().unwrap().drain(..).collect();
        assert_eq!(guides, vec!["guide1".to_string(), "guide2".to_string()]);

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
        assert!(ch1.lock().unwrap().is_empty());
        assert!(mgr.queue_status("s1").is_empty());

        // s2 is untouched
        let ch2 = mgr.get_or_create_guide_channel("s2");
        let guides: Vec<String> = ch2.lock().unwrap().drain(..).collect();
        assert_eq!(guides, vec!["guide-s2".to_string()]);
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
        // The external runner event loop drains this queue after each turn completes.
        let unconsumed: Vec<String> = channel.lock().unwrap().drain(..).collect();
        assert_eq!(unconsumed, vec!["请帮我分析这个问题".to_string()]);

        // If we had called clear_session without draining first, the message
        // would have been lost (the old bug).
        // After draining, clear_session is safe:
        assert!(channel.lock().unwrap().is_empty());
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
        let guide: Vec<String> = channel.lock().unwrap().drain(..).collect();
        assert_eq!(guide, vec!["guide_msg".to_string()]);

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
                guide_count += channel_reader.lock().unwrap().drain(..).count();
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
