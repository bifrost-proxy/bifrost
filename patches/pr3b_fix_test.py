#!/usr/bin/env python3
"""Fix existing stream_frame_roundtrip test to populate new required fields."""
import pathlib
p = pathlib.Path("crates/bifrost-admin/src/remote_invoke/types.rs")
s = p.read_text()

old = '''            StreamFrame::Stdout {
                seq: 0,
                data_b64: "YQ==".into(),
            },
            StreamFrame::Stderr {
                seq: 7,
                data_b64: "Yg==".into(),
            },
            StreamFrame::Heartbeat {
                ts: 1_700_000_000_000,
            },'''
new = '''            StreamFrame::Stdout {
                seq: 0,
                offset: 0,
                data_b64: "YQ==".into(),
            },
            StreamFrame::Stderr {
                seq: 7,
                offset: 0,
                data_b64: "Yg==".into(),
            },
            StreamFrame::Heartbeat {
                ts: 1_700_000_000_000,
                stdout_offset: Some(65536),
                stderr_offset: Some(0),
            },
            StreamFrame::Reconnect {
                reason: "relay-wall-clock".into(),
                stdout_offset: 65536,
                stderr_offset: 0,
            },'''

assert old in s
s = s.replace(old, new)
p.write_text(s)
print("OK")
