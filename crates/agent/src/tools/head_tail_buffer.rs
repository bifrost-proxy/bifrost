//! A capped buffer that preserves a stable prefix ("head") and suffix ("tail"),
//! dropping the middle once it exceeds the configured maximum.
//!
//! This is used for shell command output streaming to prevent memory exhaustion
//! from commands that produce extremely large outputs. The buffer retains the
//! beginning and end of the output, which are typically the most useful parts
//! for debugging and understanding command behavior.
//!
//! Ported from Codex's `unified_exec/head_tail_buffer.rs`.

use std::collections::VecDeque;

/// Default maximum bytes for shell output buffer (1 MiB, matching Codex).
pub const DEFAULT_MAX_BYTES: usize = 1024 * 1024;

/// A capped buffer that preserves a stable prefix ("head") and suffix ("tail"),
/// dropping the middle once it exceeds the configured maximum. The buffer is
/// symmetric meaning 50% of the capacity is allocated to the head and 50% is
/// allocated to the tail.
#[derive(Debug)]
pub struct HeadTailBuffer {
    max_bytes: usize,
    head_budget: usize,
    tail_budget: usize,
    head: VecDeque<Vec<u8>>,
    tail: VecDeque<Vec<u8>>,
    head_bytes: usize,
    tail_bytes: usize,
    omitted_bytes: usize,
}

impl Default for HeadTailBuffer {
    fn default() -> Self {
        Self::new(DEFAULT_MAX_BYTES)
    }
}

impl HeadTailBuffer {
    /// Create a new buffer that retains at most `max_bytes` of output.
    ///
    /// The retained output is split across a prefix ("head") and suffix ("tail")
    /// budget, dropping bytes from the middle once the limit is exceeded.
    pub fn new(max_bytes: usize) -> Self {
        let head_budget = max_bytes / 2;
        let tail_budget = max_bytes.saturating_sub(head_budget);
        Self {
            max_bytes,
            head_budget,
            tail_budget,
            head: VecDeque::new(),
            tail: VecDeque::new(),
            head_bytes: 0,
            tail_bytes: 0,
            omitted_bytes: 0,
        }
    }

    /// Total bytes currently retained by the buffer (head + tail).
    pub fn retained_bytes(&self) -> usize {
        self.head_bytes.saturating_add(self.tail_bytes)
    }

    /// Total bytes that were dropped from the middle due to the size cap.
    pub fn omitted_bytes(&self) -> usize {
        self.omitted_bytes
    }

    /// Total bytes that were pushed to the buffer (retained + omitted).
    pub fn total_bytes(&self) -> usize {
        self.retained_bytes().saturating_add(self.omitted_bytes)
    }

    /// Append a chunk of bytes to the buffer.
    ///
    /// Bytes are first added to the head until the head budget is full; any
    /// remaining bytes are added to the tail, with older tail bytes being
    /// dropped to preserve the tail budget.
    pub fn push_chunk(&mut self, chunk: Vec<u8>) {
        if self.max_bytes == 0 {
            self.omitted_bytes = self.omitted_bytes.saturating_add(chunk.len());
            return;
        }

        // Fill the head budget first, then keep a capped tail.
        if self.head_bytes < self.head_budget {
            let remaining_head = self.head_budget.saturating_sub(self.head_bytes);
            if chunk.len() <= remaining_head {
                self.head_bytes = self.head_bytes.saturating_add(chunk.len());
                self.head.push_back(chunk);
                return;
            }

            // Split the chunk: part goes to head, remainder goes to tail.
            let (head_part, tail_part) = chunk.split_at(remaining_head);
            if !head_part.is_empty() {
                self.head_bytes = self.head_bytes.saturating_add(head_part.len());
                self.head.push_back(head_part.to_vec());
            }
            self.push_to_tail(tail_part.to_vec());
            return;
        }

        self.push_to_tail(chunk);
    }

    /// Return the retained output as a single byte vector.
    ///
    /// The output is formed by concatenating head chunks, then tail chunks.
    /// Omitted bytes are not represented in the returned value.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.retained_bytes());
        for chunk in self.head.iter() {
            out.extend_from_slice(chunk);
        }
        for chunk in self.tail.iter() {
            out.extend_from_slice(chunk);
        }
        out
    }

    /// Return the retained output as a String, with lossy UTF-8 conversion.
    ///
    /// Also includes a header indicating total lines and omitted bytes if any.
    pub fn to_formatted_string(&self) -> String {
        let bytes = self.to_bytes();
        let text = String::from_utf8_lossy(&bytes).to_string();
        let total_lines = text.lines().count();

        if self.omitted_bytes > 0 {
            format!(
                "Total output lines: {}\n\n{}\n\n... ({} bytes omitted from middle) ...",
                total_lines, text, self.omitted_bytes
            )
        } else {
            text
        }
    }

    /// Drain all retained chunks from the buffer and reset its state.
    ///
    /// The drained chunks are returned in head-then-tail order. Omitted bytes
    /// are discarded along with the retained content.
    pub fn drain_chunks(&mut self) -> Vec<Vec<u8>> {
        let mut out: Vec<Vec<u8>> = self.head.drain(..).collect();
        out.extend(self.tail.drain(..));
        self.head_bytes = 0;
        self.tail_bytes = 0;
        self.omitted_bytes = 0;
        out
    }

    fn push_to_tail(&mut self, chunk: Vec<u8>) {
        if self.tail_budget == 0 {
            self.omitted_bytes = self.omitted_bytes.saturating_add(chunk.len());
            return;
        }

        if chunk.len() >= self.tail_budget {
            // This single chunk is larger than the whole tail budget. Keep only the last
            // tail_budget bytes and drop everything else.
            let start = chunk.len().saturating_sub(self.tail_budget);
            let kept = chunk[start..].to_vec();
            let dropped = chunk.len().saturating_sub(kept.len());
            self.omitted_bytes = self
                .omitted_bytes
                .saturating_add(self.tail_bytes)
                .saturating_add(dropped);
            self.tail.clear();
            self.tail_bytes = kept.len();
            self.tail.push_back(kept);
            return;
        }

        self.tail_bytes = self.tail_bytes.saturating_add(chunk.len());
        self.tail.push_back(chunk);
        self.trim_tail_to_budget();
    }

    fn trim_tail_to_budget(&mut self) {
        let mut excess = self.tail_bytes.saturating_sub(self.tail_budget);
        while excess > 0 {
            match self.tail.front_mut() {
                Some(front) if excess >= front.len() => {
                    excess -= front.len();
                    self.tail_bytes = self.tail_bytes.saturating_sub(front.len());
                    self.omitted_bytes = self.omitted_bytes.saturating_add(front.len());
                    self.tail.pop_front();
                }
                Some(front) => {
                    front.drain(..excess);
                    self.tail_bytes = self.tail_bytes.saturating_sub(excess);
                    self.omitted_bytes = self.omitted_bytes.saturating_add(excess);
                    break;
                }
                None => break,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_buffer() {
        let buf = HeadTailBuffer::new(100);
        assert_eq!(buf.retained_bytes(), 0);
        assert_eq!(buf.omitted_bytes(), 0);
        assert!(buf.to_bytes().is_empty());
    }

    #[test]
    fn test_small_output_fits_entirely() {
        let mut buf = HeadTailBuffer::new(100);
        buf.push_chunk(b"hello".to_vec());
        buf.push_chunk(b" world".to_vec());

        assert_eq!(buf.retained_bytes(), 11);
        assert_eq!(buf.omitted_bytes(), 0);
        assert_eq!(&buf.to_bytes(), b"hello world");
    }

    #[test]
    fn test_large_output_drops_middle() {
        let mut buf = HeadTailBuffer::new(100);

        // Push 200 bytes total
        buf.push_chunk(b"head".to_vec()); // 4 bytes -> head
        buf.push_chunk(vec![b'X'; 192]); // 192 bytes -> middle dropped, tail kept
        buf.push_chunk(b"tail".to_vec()); // 4 bytes -> tail

        // Should have retained ~50 bytes head + ~50 bytes tail
        assert!(buf.retained_bytes() <= 100);
        assert!(buf.omitted_bytes() > 0);

        let result = buf.to_bytes();
        // Head should start with "head"
        assert!(result.starts_with(b"head"));
        // Tail should end with "tail"
        assert!(result.ends_with(b"tail"));
    }

    #[test]
    fn test_zero_max_bytes_drops_everything() {
        let mut buf = HeadTailBuffer::new(0);
        buf.push_chunk(b"hello".to_vec());

        assert_eq!(buf.retained_bytes(), 0);
        assert_eq!(buf.omitted_bytes(), 5);
    }

    #[test]
    fn test_single_chunk_larger_than_tail_budget() {
        let mut buf = HeadTailBuffer::new(100);
        // Fill head first
        buf.push_chunk(vec![b'H'; 50]);

        // Now push a chunk larger than tail budget (50)
        buf.push_chunk(vec![b'T'; 100]);

        // Tail should only have last 50 bytes of the large chunk
        assert!(buf.tail_bytes <= 50);
    }

    #[test]
    fn test_formatted_string_includes_omitted_notice() {
        let mut buf = HeadTailBuffer::new(20);
        buf.push_chunk(b"head".to_vec());
        buf.push_chunk(vec![b'X'; 100]);
        buf.push_chunk(b"tail".to_vec());

        let s = buf.to_formatted_string();
        assert!(s.contains("bytes omitted"));
        assert!(s.contains("head"));
        assert!(s.contains("tail"));
    }

    #[test]
    fn test_formatted_string_no_notice_when_no_omission() {
        let mut buf = HeadTailBuffer::new(100);
        buf.push_chunk(b"hello world".to_vec());

        let s = buf.to_formatted_string();
        assert!(!s.contains("bytes omitted"));
        assert!(s.contains("hello world"));
    }

    #[test]
    fn test_drain_resets_state() {
        let mut buf = HeadTailBuffer::new(100);
        buf.push_chunk(b"hello".to_vec());

        let chunks = buf.drain_chunks();
        assert_eq!(chunks.len(), 1);

        assert_eq!(buf.retained_bytes(), 0);
        assert_eq!(buf.omitted_bytes(), 0);
    }
}
