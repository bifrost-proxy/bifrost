use bytes::Bytes;
use flate2::{Decompress, FlushDecompress, Status};

#[derive(Debug, Clone, Default)]
pub struct PerMessageDeflateConfig {
    pub client_no_context_takeover: bool,
    pub server_no_context_takeover: bool,
    pub client_max_window_bits: Option<u8>,
    pub server_max_window_bits: Option<u8>,
}

impl PerMessageDeflateConfig {
    pub fn enabled(&self) -> bool {
        true
    }
}

/// Upper bound on the decompressed size of a single permessage-deflate
/// message.
///
/// permessage-deflate lets a peer ship a tiny compressed frame that inflates
/// to gigabytes (a "zip bomb"). Because the proxy decompresses every captured
/// WS frame for recording, an unbounded inflater is a memory-exhaustion DoS.
/// We cap the inflated output per message; once the cap is hit we stop
/// decompressing and return what we have so far (the recorded payload is
/// truncated, but the proxy stays alive). 64 MiB comfortably covers legitimate
/// application messages while bounding worst-case allocation.
pub const MAX_DECOMPRESSED_MESSAGE_BYTES: usize = 64 * 1024 * 1024;

/// 解析 `Sec-WebSocket-Extensions` 中的 `permessage-deflate` 配置。
///
/// 仅关注：
/// - `server_no_context_takeover` / `client_no_context_takeover`
/// - `server_max_window_bits` / `client_max_window_bits`
pub fn parse_permessage_deflate_config(extensions: &str) -> Option<PerMessageDeflateConfig> {
    let mut found: Option<PerMessageDeflateConfig> = None;

    for ext in extensions.split(',') {
        let ext = ext.trim();
        if ext.is_empty() {
            continue;
        }

        // name; param1; param2=xxx
        let mut parts = ext.split(';').map(|s| s.trim()).filter(|s| !s.is_empty());
        let name = parts.next()?;
        if !name.eq_ignore_ascii_case("permessage-deflate") {
            continue;
        }

        let mut cfg = PerMessageDeflateConfig::default();
        for p in parts {
            if p.eq_ignore_ascii_case("client_no_context_takeover") {
                cfg.client_no_context_takeover = true;
                continue;
            }
            if p.eq_ignore_ascii_case("server_no_context_takeover") {
                cfg.server_no_context_takeover = true;
                continue;
            }

            if let Some((k, v)) = p.split_once('=') {
                let k = k.trim();
                let v = v.trim();
                if k.eq_ignore_ascii_case("client_max_window_bits") {
                    if let Ok(bits) = v.parse::<u8>() {
                        cfg.client_max_window_bits = Some(bits);
                    }
                } else if k.eq_ignore_ascii_case("server_max_window_bits") {
                    if let Ok(bits) = v.parse::<u8>() {
                        cfg.server_max_window_bits = Some(bits);
                    }
                }
            }
        }

        found = Some(cfg);
        break;
    }

    found
}

pub fn parse_permessage_deflate(extensions: &str) -> bool {
    parse_permessage_deflate_config(extensions).is_some()
}

/// permessage-deflate 的增量解压器。
///
/// - context takeover：跨消息复用 inflater 状态
/// - no_context_takeover：每条消息前 reset
#[derive(Debug)]
pub struct PerMessageDeflateInflater {
    inner: Decompress,
}

impl PerMessageDeflateInflater {
    pub fn new() -> Self {
        // `false` 表示 raw DEFLATE（无 zlib header），符合 permessage-deflate。
        Self {
            inner: Decompress::new(false),
        }
    }

    pub fn reset(&mut self) {
        self.inner = Decompress::new(false);
    }

    pub fn decompress_message(&mut self, payload: &[u8]) -> Result<Bytes, flate2::DecompressError> {
        if payload.is_empty() {
            return Ok(Bytes::new());
        }

        // permessage-deflate：每条消息以 SYNC_FLUSH 结束，但不会携带 trailer。
        // 追加 0x00 0x00 0xff 0xff 以补齐 flush 边界。
        let mut input = Vec::with_capacity(payload.len() + 4);
        input.extend_from_slice(payload);
        if !payload.ends_with(&[0x00, 0x00, 0xff, 0xff]) {
            input.extend_from_slice(&[0x00, 0x00, 0xff, 0xff]);
        }

        let mut out = Vec::new();
        let mut buf = [0u8; 8192];
        let mut input_pos = 0usize;

        while input_pos < input.len() {
            let before_in = self.inner.total_in();
            let before_out = self.inner.total_out();

            let status =
                self.inner
                    .decompress(&input[input_pos..], &mut buf, FlushDecompress::Sync)?;

            let used_in = (self.inner.total_in() - before_in) as usize;
            let produced_out = (self.inner.total_out() - before_out) as usize;

            if produced_out > 0 {
                out.extend_from_slice(&buf[..produced_out]);
            }
            input_pos = input_pos.saturating_add(used_in);

            // Decompression-bomb guard: stop once the inflated output exceeds
            // the per-message cap. We keep what we have (truncated record)
            // rather than allocating unboundedly.
            if out.len() >= MAX_DECOMPRESSED_MESSAGE_BYTES {
                out.truncate(MAX_DECOMPRESSED_MESSAGE_BYTES);
                tracing::warn!(
                    cap = MAX_DECOMPRESSED_MESSAGE_BYTES,
                    "[WS] permessage-deflate output hit decompression cap; truncating"
                );
                break;
            }

            // 在 SYNC_FLUSH 模式下，通常不会 StreamEnd，但这里允许提前退出。
            if matches!(status, Status::StreamEnd) {
                break;
            }
            if used_in == 0 && produced_out == 0 {
                break;
            }
        }

        Ok(Bytes::from(out))
    }
}

impl Default for PerMessageDeflateInflater {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::write::DeflateEncoder;
    use flate2::Compression;
    use std::io::Write;

    /// Compress `data` as a single permessage-deflate message body (raw
    /// DEFLATE, SYNC_FLUSH trailer stripped by the inflater).
    fn deflate_raw(data: &[u8]) -> Vec<u8> {
        let mut enc = DeflateEncoder::new(Vec::new(), Compression::best());
        enc.write_all(data).unwrap();
        enc.finish().unwrap()
    }

    #[test]
    fn decompresses_roundtrip() {
        let original = b"hello permessage-deflate world".repeat(16);
        let compressed = deflate_raw(&original);
        let mut inflater = PerMessageDeflateInflater::new();
        let out = inflater.decompress_message(&compressed).unwrap();
        assert_eq!(out.as_ref(), original.as_slice());
    }

    #[test]
    fn decompression_bomb_is_capped() {
        // A highly compressible payload that inflates far beyond the cap.
        let huge = vec![0u8; MAX_DECOMPRESSED_MESSAGE_BYTES * 2];
        let compressed = deflate_raw(&huge);
        // The compressed form must be dramatically smaller than the cap,
        // otherwise this isn't a meaningful bomb test.
        assert!(compressed.len() < MAX_DECOMPRESSED_MESSAGE_BYTES / 10);

        let mut inflater = PerMessageDeflateInflater::new();
        let out = inflater.decompress_message(&compressed).unwrap();
        assert!(
            out.len() <= MAX_DECOMPRESSED_MESSAGE_BYTES,
            "output {} exceeded cap {}",
            out.len(),
            MAX_DECOMPRESSED_MESSAGE_BYTES
        );
    }
}
