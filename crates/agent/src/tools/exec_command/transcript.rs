use std::collections::VecDeque;

use crate::tools::head_tail_buffer::DEFAULT_MAX_BYTES;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ExecStream {
    Stdout,
    Stderr,
}

#[derive(Clone, Debug)]
pub(super) struct ExecOutputChunk {
    pub chunk_id: u64,
    pub stream: ExecStream,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug)]
pub(super) struct ExecRead {
    pub chunk_id: u64,
    pub output: String,
    pub new_output_bytes: usize,
    pub output_lossy: bool,
    pub lost_chunk_count: u64,
    pub truncated_bytes: u64,
}

#[derive(Debug)]
pub(super) struct ExecTranscript {
    chunks: VecDeque<ExecOutputChunk>,
    next_chunk_id: u64,
    retained_bytes: usize,
    max_bytes: usize,
    lost_chunk_count: u64,
    truncated_bytes: u64,
}

impl Default for ExecTranscript {
    fn default() -> Self {
        Self::new(DEFAULT_MAX_BYTES)
    }
}

impl ExecTranscript {
    pub fn new(max_bytes: usize) -> Self {
        Self {
            chunks: VecDeque::new(),
            next_chunk_id: 1,
            retained_bytes: 0,
            max_bytes,
            lost_chunk_count: 0,
            truncated_bytes: 0,
        }
    }

    pub fn push_chunk(&mut self, stream: ExecStream, bytes: Vec<u8>) -> u64 {
        let chunk_id = self.next_chunk_id;
        self.next_chunk_id = self.next_chunk_id.saturating_add(1);
        self.retained_bytes = self.retained_bytes.saturating_add(bytes.len());
        self.chunks.push_back(ExecOutputChunk {
            chunk_id,
            stream,
            bytes,
        });
        self.trim_to_budget();
        chunk_id
    }

    pub fn latest_chunk_id(&self) -> u64 {
        self.next_chunk_id.saturating_sub(1)
    }

    pub fn has_after(&self, cursor: u64) -> bool {
        self.latest_chunk_id() > cursor
    }

    pub fn read_since(&self, cursor: u64) -> ExecRead {
        let mut output_chunks = Vec::new();
        let mut new_output_bytes = 0_usize;
        let output_lossy = self.lost_chunk_count > 0
            && self.latest_chunk_id() > cursor
            && self
                .chunks
                .front()
                .is_none_or(|first| first.chunk_id > cursor.saturating_add(1));

        for chunk in self.chunks.iter().filter(|chunk| chunk.chunk_id > cursor) {
            new_output_bytes = new_output_bytes.saturating_add(chunk.bytes.len());
            output_chunks.push((chunk.stream, chunk.bytes.as_slice()));
        }

        ExecRead {
            chunk_id: self.latest_chunk_id(),
            output: format_chunks(&output_chunks),
            new_output_bytes,
            output_lossy,
            lost_chunk_count: self.lost_chunk_count,
            truncated_bytes: self.truncated_bytes,
        }
    }

    fn trim_to_budget(&mut self) {
        if self.max_bytes == 0 {
            while let Some(chunk) = self.chunks.pop_front() {
                self.retained_bytes = self.retained_bytes.saturating_sub(chunk.bytes.len());
                self.lost_chunk_count = self.lost_chunk_count.saturating_add(1);
                self.truncated_bytes = self
                    .truncated_bytes
                    .saturating_add(u64::try_from(chunk.bytes.len()).unwrap_or(u64::MAX));
            }
            return;
        }

        while self.retained_bytes > self.max_bytes {
            let Some(chunk) = self.chunks.pop_front() else {
                self.retained_bytes = 0;
                break;
            };
            self.retained_bytes = self.retained_bytes.saturating_sub(chunk.bytes.len());
            self.lost_chunk_count = self.lost_chunk_count.saturating_add(1);
            self.truncated_bytes = self
                .truncated_bytes
                .saturating_add(u64::try_from(chunk.bytes.len()).unwrap_or(u64::MAX));
        }
    }
}

fn format_chunks(chunks: &[(ExecStream, &[u8])]) -> String {
    let mut output = String::new();
    let mut current_stream: Option<ExecStream> = None;

    for (stream, bytes) in chunks {
        if *stream == ExecStream::Stderr && current_stream != Some(ExecStream::Stderr) {
            if !output.is_empty() && !output.ends_with('\n') {
                output.push('\n');
            }
            output.push_str("[stderr]\n");
        }
        current_stream = Some(*stream);
        output.push_str(&String::from_utf8_lossy(bytes));
    }

    output.trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_since_is_non_destructive_for_multiple_consumers() {
        let mut transcript = ExecTranscript::new(1024);
        let first = transcript.push_chunk(ExecStream::Stdout, b"alpha\n".to_vec());
        transcript.push_chunk(ExecStream::Stdout, b"beta\n".to_vec());

        let watcher = transcript.read_since(0);
        let manual = transcript.read_since(0);
        assert_eq!(watcher.output, "alpha\nbeta");
        assert_eq!(manual.output, watcher.output);

        let after_first = transcript.read_since(first);
        assert_eq!(after_first.output, "beta");
        assert_eq!(after_first.new_output_bytes, 5);
    }

    #[test]
    fn retained_budget_marks_lost_chunks() {
        let mut transcript = ExecTranscript::new(4);
        transcript.push_chunk(ExecStream::Stdout, b"abcd".to_vec());
        transcript.push_chunk(ExecStream::Stdout, b"efgh".to_vec());

        let read = transcript.read_since(0);
        assert_eq!(read.output, "efgh");
        assert!(read.output_lossy);
        assert_eq!(read.lost_chunk_count, 1);
        assert_eq!(read.truncated_bytes, 4);
    }

    #[test]
    fn zero_retention_marks_fully_lost_output_as_lossy() {
        let mut transcript = ExecTranscript::new(0);
        transcript.push_chunk(ExecStream::Stdout, b"abcd".to_vec());

        let read = transcript.read_since(0);
        assert_eq!(read.output, "");
        assert!(read.output_lossy);
        assert_eq!(read.lost_chunk_count, 1);
        assert_eq!(read.truncated_bytes, 4);
    }
}
