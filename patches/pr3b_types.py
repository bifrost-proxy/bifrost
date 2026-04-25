#!/usr/bin/env python3
"""PR #3b part 1: extend StreamFrame with offset + Reconnect variant."""
import pathlib, sys

p = pathlib.Path("crates/bifrost-admin/src/remote_invoke/types.rs")
s = p.read_text()

# Add offset to Stdout/Stderr variants, and a Reconnect variant.
old_block = '''pub enum StreamFrame {
    Stdout {
        seq: u64,
        data_b64: String,
    },
    Stderr {
        seq: u64,
        data_b64: String,
    },
    Heartbeat {
        ts: u64,
    },'''

new_block = '''pub enum StreamFrame {
    Stdout {
        seq: u64,
        /// PR#3b: absolute byte offset of the first byte in `data_b64` within
        /// the total stdout stream. Enables receivers to detect gaps, dedup
        /// on resume, and reassemble in order even across reconnects.
        #[serde(default)]
        offset: u64,
        data_b64: String,
    },
    Stderr {
        seq: u64,
        #[serde(default)]
        offset: u64,
        data_b64: String,
    },
    Heartbeat {
        ts: u64,
        /// PR#3b: last stdout/stderr offsets the executor has emitted so a
        /// disconnected receiver can tell whether it is behind.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        stdout_offset: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        stderr_offset: Option<u64>,
    },
    /// PR#3b: advisory signal from executor to receiver: "the current relay
    /// connection is approaching its wall-clock limit; please reconnect soon
    /// with Resume(call_id, from_offset)". Receivers that ignore this will
    /// still get correct data, but may hit a hard relay-side disconnect.
    Reconnect {
        reason: String,
        stdout_offset: u64,
        stderr_offset: u64,
    },'''

assert old_block in s, "StreamFrame head anchor missed"
s = s.replace(old_block, new_block)

# Update the existing roundtrip test to carry offset (still valid since default=0).
# No test change required — `#[serde(default)]` means old JSON still parses.

p.write_text(s)
print("PATCHED types.rs OK")
