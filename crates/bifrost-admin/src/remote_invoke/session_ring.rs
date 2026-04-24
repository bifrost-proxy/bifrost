//! PR #4: SessionRing — per-call_id byte ring buffer enabling resume across
//! relay disconnects.
//!
//! This module is standalone and additive. It does NOT depend on worker.rs
//! state, so it can be merged safely while another developer edits worker.rs
//! for call_history_store work. Wiring into the relay request handler is
//! deferred to PR #4b once call_history_store has landed.
//!
//! Design:
//!   - A ring holds the most-recent `capacity` bytes of stdout/stderr for a
//!     given call_id.
//!   - Writers (executor loop) append bytes; once capacity is exceeded the
//!     oldest bytes are dropped.
//!   - Readers (resume handler) call `read_from(offset)` to obtain bytes in
//!     range `[offset, head)` as long as `offset >= tail`; otherwise they get
//!     `ResumeError::DataEvicted`.
//!   - Acks advance a `last_ack_offset` that is NOT used for eviction; the
//!     ring evicts purely by capacity. `last_ack_offset` is exposed so the
//!     caller / future PR can grow/shrink capacity heuristically.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use uuid::Uuid;

/// Default ring capacity per stream (stdout or stderr) per call_id: 64 MiB.
pub const DEFAULT_SESSION_RING_CAPACITY: usize = 64 * 1024 * 1024;

/// Errors from resume reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResumeError {
    /// Requested offset is older than anything we still retain.
    DataEvicted {
        requested_offset: u64,
        earliest_available_offset: u64,
    },
    /// Requested offset is beyond what's been written (should not normally
    /// happen; receiver got ahead of sender somehow).
    NotYetProduced {
        requested_offset: u64,
        current_head_offset: u64,
    },
    /// Call id is unknown.
    UnknownCallId,
}

/// Fixed-capacity byte ring tracking absolute byte offsets. Not Sync on its
/// own; wrap in Arc<Mutex<...>> for concurrent access (see [`SessionRegistry`]).
#[derive(Debug)]
pub struct ByteRing {
    capacity: usize,
    /// Absolute offset of the first byte currently in the buffer.
    tail: u64,
    /// Absolute offset one past the last byte currently in the buffer.
    head: u64,
    /// Backing storage; acts as a circular buffer when `head - tail == capacity`.
    buf: Vec<u8>,
    /// Ring write cursor (index in `buf`).
    write_idx: usize,
    /// Last acked absolute offset (monotonic non-decreasing).
    last_ack_offset: u64,
}

impl ByteRing {
    pub fn new(capacity: usize) -> Self {
        assert!(capacity > 0, "capacity must be > 0");
        Self {
            capacity,
            tail: 0,
            head: 0,
            buf: vec![0u8; capacity],
            write_idx: 0,
            last_ack_offset: 0,
        }
    }

    /// Absolute offset of the first byte still retained.
    pub fn tail(&self) -> u64 {
        self.tail
    }
    /// Absolute offset just past the last byte retained.
    pub fn head(&self) -> u64 {
        self.head
    }
    /// Current retained byte count.
    pub fn len(&self) -> usize {
        (self.head - self.tail) as usize
    }
    pub fn is_empty(&self) -> bool {
        self.head == self.tail
    }
    pub fn capacity(&self) -> usize {
        self.capacity
    }
    pub fn last_ack_offset(&self) -> u64 {
        self.last_ack_offset
    }

    /// Append bytes to the ring. If appending pushes us over capacity the
    /// oldest bytes are dropped (tail advances).
    pub fn append(&mut self, bytes: &[u8]) {
        for chunk in bytes.chunks(self.capacity) {
            // Write into the ring buffer, wrapping.
            let n = chunk.len();
            let end = self.write_idx + n;
            if end <= self.capacity {
                self.buf[self.write_idx..end].copy_from_slice(chunk);
            } else {
                let first = self.capacity - self.write_idx;
                self.buf[self.write_idx..].copy_from_slice(&chunk[..first]);
                self.buf[..n - first].copy_from_slice(&chunk[first..]);
            }
            self.write_idx = (self.write_idx + n) % self.capacity;
            self.head += n as u64;
            // Advance tail if over capacity.
            if self.head - self.tail > self.capacity as u64 {
                self.tail = self.head - self.capacity as u64;
            }
        }
    }

    /// Try to read bytes starting at `from_offset` into `out`. Returns the
    /// number of bytes written into `out`, or `ResumeError` if the offset is
    /// outside the retained range. Reads at most `out.len()` bytes.
    pub fn read_from(&self, from_offset: u64, out: &mut [u8]) -> Result<usize, ResumeError> {
        if from_offset > self.head {
            return Err(ResumeError::NotYetProduced {
                requested_offset: from_offset,
                current_head_offset: self.head,
            });
        }
        if from_offset < self.tail {
            return Err(ResumeError::DataEvicted {
                requested_offset: from_offset,
                earliest_available_offset: self.tail,
            });
        }
        let available = (self.head - from_offset) as usize;
        let n = available.min(out.len());
        if n == 0 {
            return Ok(0);
        }
        // Map absolute offset to ring index.
        let relative = (from_offset - self.tail) as usize;
        // head index in the ring buffer:
        let tail_idx = (self.write_idx + self.capacity - self.len()) % self.capacity;
        let start = (tail_idx + relative) % self.capacity;
        let end = start + n;
        if end <= self.capacity {
            out[..n].copy_from_slice(&self.buf[start..end]);
        } else {
            let first = self.capacity - start;
            out[..first].copy_from_slice(&self.buf[start..]);
            out[first..n].copy_from_slice(&self.buf[..n - first]);
        }
        Ok(n)
    }

    /// Record that the receiver has acked through `offset`. Ignores
    /// non-monotonic acks.
    pub fn ack(&mut self, offset: u64) {
        if offset > self.last_ack_offset {
            self.last_ack_offset = offset;
        }
    }
}

/// One call's state.
#[derive(Debug)]
pub struct SessionState {
    pub call_id: Uuid,
    pub stdout: ByteRing,
    pub stderr: ByteRing,
    pub status: SessionStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionStatus {
    Running,
    Done { exit_code: i32 },
    Failed { code: String },
    Abandoned,
}

/// Registry of live sessions keyed by call_id.
#[derive(Debug, Default, Clone)]
pub struct SessionRegistry {
    inner: Arc<Mutex<HashMap<Uuid, Arc<Mutex<SessionState>>>>>,
}

impl SessionRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn create(&self, capacity: usize) -> (Uuid, Arc<Mutex<SessionState>>) {
        let call_id = Uuid::new_v4();
        let state = Arc::new(Mutex::new(SessionState {
            call_id,
            stdout: ByteRing::new(capacity),
            stderr: ByteRing::new(capacity),
            status: SessionStatus::Running,
        }));
        self.inner
            .lock()
            .expect("registry lock")
            .insert(call_id, Arc::clone(&state));
        (call_id, state)
    }

    pub fn get(&self, call_id: &Uuid) -> Option<Arc<Mutex<SessionState>>> {
        self.inner
            .lock()
            .expect("registry lock")
            .get(call_id)
            .cloned()
    }

    pub fn remove(&self, call_id: &Uuid) -> Option<Arc<Mutex<SessionState>>> {
        self.inner.lock().expect("registry lock").remove(call_id)
    }

    pub fn len(&self) -> usize {
        self.inner.lock().expect("registry lock").len()
    }

    pub fn is_empty(&self) -> bool {
        self.inner.lock().expect("registry lock").is_empty()
    }
}

// PR #4b: process-global singleton registry + tee helpers.
//
// The worker / executor pipeline today is synchronous wrt frame emission
// (it calls `relay_client.post_call_frame(...).await` per chunk). To add
// resume support without touching worker.rs (which is under parallel
// modification for call_history_store), we expose a lock-free-ish global
// registry. Future PR #4c will add a *single* `tee_stdout(call_id, bytes)`
// call into the worker's existing `on_stdout` closure; because tee is a
// no-op when no session has been registered for that call_id, this stays
// 100% backwards compatible.

use std::sync::OnceLock;

static GLOBAL_REGISTRY: OnceLock<SessionRegistry> = OnceLock::new();

/// Access the process-wide session registry (lazy init).
pub fn global_registry() -> &'static SessionRegistry {
    GLOBAL_REGISTRY.get_or_init(SessionRegistry::new)
}

/// Register a new session in the global registry and return its call_id.
/// Convenience wrapper so callers don't need to touch the registry type
/// directly.
pub fn register_session(capacity: usize) -> Uuid {
    let (id, _state) = global_registry().create(capacity);
    id
}

/// Mirror a stdout chunk into the session's ring. Silent no-op if no
/// session is registered for this call_id (keeps legacy callers unchanged).
pub fn tee_stdout(call_id: &Uuid, bytes: &[u8]) {
    if let Some(state_arc) = global_registry().get(call_id) {
        let mut state = state_arc.lock().expect("session state poisoned");
        state.stdout.append(bytes);
    }
}

/// Mirror a stderr chunk into the session's ring. Silent no-op if absent.
pub fn tee_stderr(call_id: &Uuid, bytes: &[u8]) {
    if let Some(state_arc) = global_registry().get(call_id) {
        let mut state = state_arc.lock().expect("session state poisoned");
        state.stderr.append(bytes);
    }
}

/// Finalize a session by setting its terminal status. Subsequent reads
/// still succeed for any retained bytes so the CLI can drain after Done.
pub fn finalize_session(call_id: &Uuid, status: SessionStatus) {
    if let Some(state_arc) = global_registry().get(call_id) {
        let mut state = state_arc.lock().expect("session state poisoned");
        state.status = status;
    }
}

/// Drain bytes from a session's stdout ring starting at `from_offset`.
/// Returns `(bytes, head_offset, status)`. If the call is unknown or the
/// offset was evicted, returns the corresponding ResumeError.
pub fn resume_stdout(
    call_id: &Uuid,
    from_offset: u64,
) -> Result<(Vec<u8>, u64, SessionStatus), ResumeError> {
    let state_arc = global_registry()
        .get(call_id)
        .ok_or(ResumeError::UnknownCallId)?;
    let state = state_arc.lock().expect("session state poisoned");
    let head = state.stdout.head();
    // Pre-size buffer to exactly the available range; read_from fills it.
    let available = head.saturating_sub(from_offset) as usize;
    let mut out = vec![0u8; available];
    let n = state.stdout.read_from(from_offset, &mut out)?;
    out.truncate(n);
    Ok((out, head, state.status.clone()))
}

/// Drain bytes from a session's stderr ring starting at `from_offset`.
pub fn resume_stderr(
    call_id: &Uuid,
    from_offset: u64,
) -> Result<(Vec<u8>, u64, SessionStatus), ResumeError> {
    let state_arc = global_registry()
        .get(call_id)
        .ok_or(ResumeError::UnknownCallId)?;
    let state = state_arc.lock().expect("session state poisoned");
    let head = state.stderr.head();
    let available = head.saturating_sub(from_offset) as usize;
    let mut out = vec![0u8; available];
    let n = state.stderr.read_from(from_offset, &mut out)?;
    out.truncate(n);
    Ok((out, head, state.status.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_ring_basic_append_and_read() {
        let mut r = ByteRing::new(16);
        r.append(b"hello");
        assert_eq!(r.head(), 5);
        assert_eq!(r.tail(), 0);
        let mut out = [0u8; 5];
        assert_eq!(r.read_from(0, &mut out).unwrap(), 5);
        assert_eq!(&out, b"hello");
        // Partial read starting mid-stream.
        let mut out = [0u8; 3];
        assert_eq!(r.read_from(2, &mut out).unwrap(), 3);
        assert_eq!(&out, b"llo");
    }

    #[test]
    fn byte_ring_wraps_when_exceeding_capacity() {
        let mut r = ByteRing::new(8);
        r.append(b"abcdefgh"); // head=8, tail=0
        r.append(b"ijkl"); // head=12, tail=4 (oldest 4 bytes evicted)
        assert_eq!(r.head(), 12);
        assert_eq!(r.tail(), 4);
        let mut out = [0u8; 8];
        let n = r.read_from(4, &mut out).unwrap();
        assert_eq!(n, 8);
        assert_eq!(&out, b"efghijkl");
    }

    #[test]
    fn byte_ring_errors_on_evicted_and_future_offsets() {
        let mut r = ByteRing::new(4);
        r.append(b"12345"); // head=5, tail=1
        let mut out = [0u8; 4];
        match r.read_from(0, &mut out).unwrap_err() {
            ResumeError::DataEvicted {
                requested_offset: 0,
                earliest_available_offset: 1,
            } => {}
            e => panic!("{e:?}"),
        }
        match r.read_from(10, &mut out).unwrap_err() {
            ResumeError::NotYetProduced {
                requested_offset: 10,
                current_head_offset: 5,
            } => {}
            e => panic!("{e:?}"),
        }
    }

    #[test]
    fn byte_ring_read_at_head_returns_zero() {
        let mut r = ByteRing::new(8);
        r.append(b"abc");
        let mut out = [0u8; 4];
        assert_eq!(r.read_from(3, &mut out).unwrap(), 0);
    }

    #[test]
    fn byte_ring_large_chunk_append_slides_tail_correctly() {
        let mut r = ByteRing::new(4);
        r.append(b"0123456789"); // only last 4 bytes retained
        assert_eq!(r.head(), 10);
        assert_eq!(r.tail(), 6);
        let mut out = [0u8; 4];
        assert_eq!(r.read_from(6, &mut out).unwrap(), 4);
        assert_eq!(&out, b"6789");
    }

    #[test]
    fn byte_ring_ack_is_monotonic() {
        let mut r = ByteRing::new(16);
        r.append(b"abcdefgh");
        r.ack(5);
        assert_eq!(r.last_ack_offset(), 5);
        r.ack(3); // older, ignored
        assert_eq!(r.last_ack_offset(), 5);
        r.ack(7);
        assert_eq!(r.last_ack_offset(), 7);
    }

    #[test]
    fn session_registry_create_get_remove() {
        let reg = SessionRegistry::new();
        assert!(reg.is_empty());
        let (id, state) = reg.create(DEFAULT_SESSION_RING_CAPACITY);
        assert_eq!(reg.len(), 1);
        assert!(reg.get(&id).is_some());
        state.lock().unwrap().stdout.append(b"xyz");
        let got = reg.get(&id).unwrap();
        assert_eq!(got.lock().unwrap().stdout.head(), 3);
        reg.remove(&id).unwrap();
        assert!(reg.is_empty());
        assert!(reg.get(&id).is_none());
    }

    #[test]
    fn session_registry_isolates_calls() {
        let reg = SessionRegistry::new();
        let (a, sa) = reg.create(128);
        let (b, sb) = reg.create(128);
        assert_ne!(a, b);
        sa.lock().unwrap().stdout.append(b"aa");
        sb.lock().unwrap().stdout.append(b"bbbb");
        assert_eq!(reg.get(&a).unwrap().lock().unwrap().stdout.head(), 2);
        assert_eq!(reg.get(&b).unwrap().lock().unwrap().stdout.head(), 4);
    }

    // ---- PR #4b: global singleton + tee/resume helpers ----

    #[test]
    fn global_tee_and_resume_stdout_roundtrip() {
        let call_id = register_session(1024);
        tee_stdout(&call_id, b"hello ");
        tee_stdout(&call_id, b"world");
        let (bytes, head, status) = resume_stdout(&call_id, 0).unwrap();
        assert_eq!(bytes, b"hello world");
        assert_eq!(head, 11);
        assert_eq!(status, SessionStatus::Running);

        // Partial resume from offset 6
        let (bytes2, head2, _) = resume_stdout(&call_id, 6).unwrap();
        assert_eq!(bytes2, b"world");
        assert_eq!(head2, 11);

        // Cleanup so the global registry doesn't leak between tests.
        global_registry().remove(&call_id);
    }

    #[test]
    fn global_tee_stderr_is_isolated_from_stdout() {
        let call_id = register_session(1024);
        tee_stdout(&call_id, b"OUT");
        tee_stderr(&call_id, b"ERR!!");
        let (so, _, _) = resume_stdout(&call_id, 0).unwrap();
        let (se, _, _) = resume_stderr(&call_id, 0).unwrap();
        assert_eq!(so, b"OUT");
        assert_eq!(se, b"ERR!!");
        global_registry().remove(&call_id);
    }

    #[test]
    fn finalize_session_transitions_status() {
        let call_id = register_session(128);
        tee_stdout(&call_id, b"done");
        finalize_session(&call_id, SessionStatus::Done { exit_code: 0 });
        let (bytes, _, status) = resume_stdout(&call_id, 0).unwrap();
        assert_eq!(bytes, b"done");
        assert_eq!(status, SessionStatus::Done { exit_code: 0 });
        global_registry().remove(&call_id);
    }

    #[test]
    fn tee_on_unknown_call_id_is_silent_noop() {
        // A random uuid not registered; must not panic.
        let ghost = Uuid::new_v4();
        tee_stdout(&ghost, b"ignored");
        tee_stderr(&ghost, b"ignored");
        assert!(matches!(
            resume_stdout(&ghost, 0).unwrap_err(),
            ResumeError::UnknownCallId
        ));
    }

    #[test]
    fn resume_after_eviction_reports_data_evicted() {
        let call_id = register_session(8);
        tee_stdout(&call_id, b"0123456789ABCDEF"); // 16 bytes into 8-cap ring
        let err = resume_stdout(&call_id, 0).unwrap_err();
        match err {
            ResumeError::DataEvicted {
                requested_offset,
                earliest_available_offset,
            } => {
                assert_eq!(requested_offset, 0);
                assert_eq!(earliest_available_offset, 8);
            }
            other => panic!("unexpected err: {:?}", other),
        }
        // Fresh read from the retained tail works.
        let (bytes, head, _) = resume_stdout(&call_id, 8).unwrap();
        assert_eq!(bytes, b"89ABCDEF");
        assert_eq!(head, 16);
        global_registry().remove(&call_id);
    }
}
