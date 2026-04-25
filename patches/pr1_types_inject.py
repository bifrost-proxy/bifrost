#!/usr/bin/env python3
"""PR #1 — Protocol-layer extensions. Idempotent, simple-string anchors."""
from __future__ import annotations
import pathlib, sys

TARGET = pathlib.Path("crates/bifrost-admin/src/remote_invoke/types.rs")

FIELD_MARKER = "// BEGIN PR#1 RemoteCommand large-output fields"
BLOCK_MARKER = "// BEGIN PR#1 large-output protocol extensions"

# Anchor used for RemoteCommand field injection.
ANCHOR = "    pub output_mode: Option<OutputMode>,\n"

NEW_FIELDS = (
    "    " + FIELD_MARKER + "\n"
    "    /// Transport strategy for stdout/stderr. When `None`, the executor\n"
    "    /// falls back to the legacy Inline behaviour (bounded by\n"
    "    /// `max_output_bytes`) for backward compatibility with older clients.\n"
    "    #[serde(default, skip_serializing_if = \"Option::is_none\")]\n"
    "    pub output_transport: Option<OutputTransport>,\n"
    "    /// Per-invocation override of the byte cap used by Inline transport.\n"
    "    /// `None` => use policy / profile defaults. `Some(0)` => unbounded\n"
    "    /// (valid only with Streaming / SideChannel transport).\n"
    "    #[serde(default, skip_serializing_if = \"Option::is_none\")]\n"
    "    pub max_output_bytes: Option<u64>,\n"
    "    // END PR#1 RemoteCommand large-output fields\n"
)

NEW_BLOCK = r'''
// BEGIN PR#1 large-output protocol extensions
//
// ADDITIVE types for the `large_output_v1` protocol revision. Older peers
// that do not understand the new variants negotiate down to
// `OutputTransport::Inline` via `ProtocolFeatures` during handshake. No
// existing on-wire form is broken by this block.

pub const PROTOCOL_VERSION: u32 = 2;
pub const PROTOCOL_FEATURE_LARGE_OUTPUT_V1: &str = "large_output_v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputTransport {
    /// Legacy: stdout/stderr returned inline inside `RemoteInvokeResponse`,
    /// bounded by `max_output_bytes`. Preserves pre-v1 behaviour.
    Inline,
    /// v1: executor emits StreamFrame::Stdout/Stderr chunks with monotonic
    /// seq and base64-encoded payload. Final response carries digest + size.
    Streaming,
    /// v1: executor uploads full stdout/stderr to object storage; final
    /// response carries an `ObjectRef`. SSE still emitted for live tailing.
    SideChannel,
}

impl Default for OutputTransport {
    fn default() -> Self { OutputTransport::Inline }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObjectRef {
    pub url: String,
    pub size: u64,
    pub sha256: String,
    pub expires_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StreamFrame {
    Stdout { seq: u64, data_b64: String },
    Stderr { seq: u64, data_b64: String },
    Heartbeat { ts: u64 },
    Ack {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        stdout_seq: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        stderr_seq: Option<u64>,
    },
    Done {
        exit_code: i32,
        total_stdout: u64,
        total_stderr: u64,
        stdout_sha256: String,
        stderr_sha256: String,
        duration_ms: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        stdout_object: Option<ObjectRef>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        stderr_object: Option<ObjectRef>,
    },
    Error { code: String, message: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProtocolFeatures {
    pub version: u32,
    #[serde(default)]
    pub features: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_inline_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_frame_bytes: Option<u64>,
}

impl Default for ProtocolFeatures {
    fn default() -> Self {
        Self {
            version: PROTOCOL_VERSION,
            features: vec![PROTOCOL_FEATURE_LARGE_OUTPUT_V1.to_string()],
            max_inline_bytes: Some(4 * 1024 * 1024),
            max_frame_bytes:  Some(256 * 1024),
        }
    }
}

impl ProtocolFeatures {
    pub fn negotiates_large_output(&self, peer: &ProtocolFeatures) -> bool {
        self.features.iter().any(|f| f == PROTOCOL_FEATURE_LARGE_OUTPUT_V1)
            && peer.features.iter().any(|f| f == PROTOCOL_FEATURE_LARGE_OUTPUT_V1)
    }
    pub fn effective_max_frame_bytes(&self, peer: &ProtocolFeatures) -> u64 {
        let a = self.max_frame_bytes.unwrap_or(256 * 1024);
        let b = peer.max_frame_bytes.unwrap_or(256 * 1024);
        a.min(b)
    }
    pub fn effective_max_inline_bytes(&self, peer: &ProtocolFeatures) -> u64 {
        let a = self.max_inline_bytes.unwrap_or(4 * 1024 * 1024);
        let b = peer.max_inline_bytes.unwrap_or(4 * 1024 * 1024);
        a.min(b)
    }
}

#[cfg(test)]
mod pr1_large_output_protocol_tests {
    use super::*;

    #[test]
    fn output_transport_roundtrip() {
        for t in [OutputTransport::Inline, OutputTransport::Streaming, OutputTransport::SideChannel] {
            let s = serde_json::to_string(&t).unwrap();
            let back: OutputTransport = serde_json::from_str(&s).unwrap();
            assert_eq!(t, back);
        }
    }

    #[test]
    fn stream_frame_roundtrip() {
        let frames = vec![
            StreamFrame::Stdout { seq: 0, data_b64: "YQ==".into() },
            StreamFrame::Stderr { seq: 7, data_b64: "Yg==".into() },
            StreamFrame::Heartbeat { ts: 1_700_000_000_000 },
            StreamFrame::Ack { stdout_seq: Some(5), stderr_seq: None },
            StreamFrame::Done {
                exit_code: 0,
                total_stdout: 123,
                total_stderr: 0,
                stdout_sha256: "deadbeef".into(),
                stderr_sha256: "cafe".into(),
                duration_ms: 12,
                stdout_object: None,
                stderr_object: None,
            },
            StreamFrame::Error { code: "policy.denied".into(), message: "nope".into() },
        ];
        for f in frames {
            let s = serde_json::to_string(&f).unwrap();
            let back: StreamFrame = serde_json::from_str(&s).unwrap();
            assert_eq!(s, serde_json::to_string(&back).unwrap());
        }
    }

    #[test]
    fn remote_command_back_compat_without_new_fields() {
        let old = r#"{"kind":"shell_exec","command":"","command_text":"echo hi"}"#;
        let cmd: RemoteCommand = serde_json::from_str(old).unwrap();
        assert!(cmd.output_transport.is_none());
        assert!(cmd.max_output_bytes.is_none());
    }

    #[test]
    fn features_negotiation() {
        let a = ProtocolFeatures::default();
        let b = ProtocolFeatures::default();
        assert!(a.negotiates_large_output(&b));
        assert_eq!(a.effective_max_frame_bytes(&b), 256 * 1024);
    }
}
// END PR#1 large-output protocol extensions
'''


def main() -> int:
    if not TARGET.exists():
        print(f"ERROR: {TARGET} not found", file=sys.stderr)
        return 1
    src = TARGET.read_text()

    if FIELD_MARKER in src:
        print("[skip] RemoteCommand fields already present")
    else:
        if ANCHOR not in src:
            print("ERROR: anchor not found:", repr(ANCHOR), file=sys.stderr)
            return 2
        src = src.replace(ANCHOR, ANCHOR + NEW_FIELDS, 1)
        print("[ok] injected RemoteCommand fields")

    if BLOCK_MARKER in src:
        print("[skip] protocol-extension block already present")
    else:
        src = src.rstrip() + "\n" + NEW_BLOCK + "\n"
        print("[ok] appended protocol-extension block")

    TARGET.write_text(src)
    print(f"[done] wrote {len(src)} bytes to {TARGET}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
