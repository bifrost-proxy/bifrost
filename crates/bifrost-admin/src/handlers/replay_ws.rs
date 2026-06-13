use std::collections::HashSet;
use std::io;
use std::io::Read;
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine;
use bytes::{Buf, BufMut, Bytes, BytesMut};
use flate2::read::DeflateDecoder;
use sha1::{Digest, Sha1};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

const WEBSOCKET_GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

// 防御性限制：避免异常/恶意上游通过超大 frame 触发内存压力。
// 该限制只作用于 replay 的帧解析与转发捕获，不影响正常 upstream WebSocket 客户端实现。
const MAX_FRAME_PAYLOAD_LEN: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Opcode {
    Continuation = 0x0,
    Text = 0x1,
    Binary = 0x2,
    Close = 0x8,
    Ping = 0x9,
    Pong = 0xA,
}

impl Opcode {
    fn from_u8(value: u8) -> Option<Self> {
        match value {
            0x0 => Some(Opcode::Continuation),
            0x1 => Some(Opcode::Text),
            0x2 => Some(Opcode::Binary),
            0x8 => Some(Opcode::Close),
            0x9 => Some(Opcode::Ping),
            0xA => Some(Opcode::Pong),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub(super) struct WebSocketFrame {
    pub(super) fin: bool,
    pub(super) rsv1: bool,
    pub(super) rsv2: bool,
    pub(super) rsv3: bool,
    pub(super) opcode: Opcode,
    pub(super) mask: Option<[u8; 4]>,
    pub(super) payload: Bytes,
}

impl WebSocketFrame {
    pub(super) fn encode(&self) -> Bytes {
        let payload_len = self.payload.len();
        let mut buf = BytesMut::with_capacity(14 + payload_len);

        let mut first_byte = self.opcode as u8;
        if self.fin {
            first_byte |= 0x80;
        }
        if self.rsv1 {
            first_byte |= 0x40;
        }
        if self.rsv2 {
            first_byte |= 0x20;
        }
        if self.rsv3 {
            first_byte |= 0x10;
        }
        buf.put_u8(first_byte);

        let mask_bit = if self.mask.is_some() { 0x80 } else { 0 };
        if payload_len < 126 {
            buf.put_u8(mask_bit | payload_len as u8);
        } else if payload_len < 65536 {
            buf.put_u8(mask_bit | 126);
            buf.put_u16(payload_len as u16);
        } else {
            buf.put_u8(mask_bit | 127);
            buf.put_u64(payload_len as u64);
        }

        if let Some(mask) = self.mask {
            buf.put_slice(&mask);
            let mut masked_payload = self.payload.to_vec();
            for (i, byte) in masked_payload.iter_mut().enumerate() {
                *byte ^= mask[i % 4];
            }
            buf.extend_from_slice(&masked_payload);
        } else {
            buf.extend_from_slice(&self.payload);
        }

        buf.freeze()
    }

    pub(super) fn parse(data: &[u8]) -> Option<(Self, usize)> {
        if data.len() < 2 {
            return None;
        }

        let first_byte = data[0];
        let second_byte = data[1];

        let fin = (first_byte & 0x80) != 0;
        let rsv1 = (first_byte & 0x40) != 0;
        let rsv2 = (first_byte & 0x20) != 0;
        let rsv3 = (first_byte & 0x10) != 0;
        let opcode = Opcode::from_u8(first_byte & 0x0F)?;
        let masked = (second_byte & 0x80) != 0;
        let payload_len_indicator = second_byte & 0x7F;

        let mut offset = 2;
        let payload_len: usize;

        match payload_len_indicator.cmp(&126) {
            std::cmp::Ordering::Less => {
                payload_len = payload_len_indicator as usize;
            }
            std::cmp::Ordering::Equal => {
                if data.len() < offset + 2 {
                    return None;
                }
                payload_len = u16::from_be_bytes([data[offset], data[offset + 1]]) as usize;
                offset += 2;
            }
            std::cmp::Ordering::Greater => {
                if data.len() < offset + 8 {
                    return None;
                }
                let mut len_bytes = [0u8; 8];
                len_bytes.copy_from_slice(&data[offset..offset + 8]);
                payload_len = u64::from_be_bytes(len_bytes) as usize;
                offset += 8;
            }
        }

        let mask = if masked {
            if data.len() < offset + 4 {
                return None;
            }
            let mut m = [0u8; 4];
            m.copy_from_slice(&data[offset..offset + 4]);
            offset += 4;
            Some(m)
        } else {
            None
        };

        if data.len() < offset + payload_len {
            return None;
        }

        let mut payload = data[offset..offset + payload_len].to_vec();
        if let Some(m) = mask {
            for (i, byte) in payload.iter_mut().enumerate() {
                *byte ^= m[i % 4];
            }
        }

        let frame = WebSocketFrame {
            fin,
            rsv1,
            rsv2,
            rsv3,
            opcode,
            mask,
            payload: Bytes::from(payload),
        };

        Some((frame, offset + payload_len))
    }

    pub(super) fn close_code(&self) -> Option<u16> {
        if self.opcode == Opcode::Close && self.payload.len() >= 2 {
            Some(u16::from_be_bytes([self.payload[0], self.payload[1]]))
        } else {
            None
        }
    }

    pub(super) fn close_reason(&self) -> Option<&str> {
        if self.opcode == Opcode::Close && self.payload.len() > 2 {
            std::str::from_utf8(&self.payload[2..]).ok()
        } else {
            None
        }
    }
}

pub(super) fn decompress_permessage_deflate_payload(payload: &Bytes) -> Bytes {
    if payload.is_empty() {
        return payload.clone();
    }

    let mut data = payload.to_vec();
    // permessage-deflate 需要追加 tail bytes 才能解码完整消息（RFC 7692）
    data.extend_from_slice(&[0x00, 0x00, 0xff, 0xff]);

    let mut decoder = DeflateDecoder::new(&data[..]);
    let mut decompressed = Vec::new();
    match decoder.read_to_end(&mut decompressed) {
        Ok(_) => Bytes::from(decompressed),
        Err(_) => payload.clone(),
    }
}

fn validate_frame_header(buf: &[u8]) -> io::Result<()> {
    if buf.len() < 2 {
        return Ok(());
    }

    let first_byte = buf[0];
    let second_byte = buf[1];

    let fin = (first_byte & 0x80) != 0;
    let opcode = Opcode::from_u8(first_byte & 0x0F)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "Invalid WebSocket opcode"))?;

    let masked = (second_byte & 0x80) != 0;
    let payload_len_indicator = second_byte & 0x7F;

    let (payload_len, ext_len_bytes) = match payload_len_indicator {
        0..=125 => (payload_len_indicator as usize, 0usize),
        126 => {
            if buf.len() < 4 {
                return Ok(());
            }
            let len = u16::from_be_bytes([buf[2], buf[3]]) as usize;
            (len, 2)
        }
        127 => {
            if buf.len() < 10 {
                return Ok(());
            }
            let mut len_bytes = [0u8; 8];
            len_bytes.copy_from_slice(&buf[2..10]);
            let len_u64 = u64::from_be_bytes(len_bytes);
            // RFC 6455: the most significant bit MUST be 0
            if (len_u64 & (1u64 << 63)) != 0 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Invalid WebSocket payload length (MSB must be 0)",
                ));
            }
            (len_u64 as usize, 8)
        }
        _ => unreachable!(),
    };

    if payload_len > MAX_FRAME_PAYLOAD_LEN {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("WebSocket frame payload too large: {}", payload_len),
        ));
    }

    // Control frame 约束：必须 fin=1 且 payload<=125（RFC 6455）
    if matches!(opcode, Opcode::Close | Opcode::Ping | Opcode::Pong) {
        if !fin {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Invalid control frame: FIN must be set",
            ));
        }
        if payload_len > 125 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Invalid control frame: payload must be <= 125 bytes",
            ));
        }
    }

    // 如果已知 header 长度且 buffer 已超过阈值，但 frame 仍未完整，下一轮会继续读取。
    // 这里不强制要求 header 已完整（mask bytes 可能未到），只做上限/规范提前校验。
    let _header_len = 2 + ext_len_bytes + if masked { 4 } else { 0 };
    Ok(())
}

pub(super) struct WebSocketReader<R> {
    inner: R,
    buffer: BytesMut,
}

impl<R> WebSocketReader<R> {
    pub(super) fn new(inner: R) -> Self {
        Self {
            inner,
            buffer: BytesMut::with_capacity(8192),
        }
    }

    pub(super) fn with_initial_buffer(inner: R, buffer: BytesMut) -> Self {
        Self { inner, buffer }
    }
}

impl<R: AsyncRead + Unpin> WebSocketReader<R> {
    pub(super) async fn next_frame(&mut self) -> io::Result<Option<WebSocketFrame>> {
        loop {
            if let Some((frame, consumed)) = WebSocketFrame::parse(&self.buffer) {
                self.buffer.advance(consumed);
                return Ok(Some(frame));
            }

            // 在继续读取前先对 header 做防御性校验，避免 buffer 被异常大 frame 撑爆。
            validate_frame_header(&self.buffer)?;

            let mut chunk = [0u8; 8192];
            let n = self.inner.read(&mut chunk).await?;
            if n == 0 {
                return Ok(None);
            }
            self.buffer.extend_from_slice(&chunk[..n]);
        }
    }
}

pub(super) struct WebSocketWriter<W> {
    inner: W,
    is_client: bool,
}

impl<W> WebSocketWriter<W> {
    pub(super) fn new(inner: W, is_client: bool) -> Self {
        Self { inner, is_client }
    }
}

impl<W: AsyncWrite + Unpin> WebSocketWriter<W> {
    pub(super) async fn write_frame(&mut self, mut frame: WebSocketFrame) -> io::Result<()> {
        if self.is_client && frame.mask.is_none() {
            frame.mask = Some(generate_mask());
        }
        let encoded = frame.encode();
        self.inner.write_all(&encoded).await?;
        self.inner.flush().await?;
        Ok(())
    }
}

fn generate_mask() -> [u8; 4] {
    let seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u32;
    seed.to_be_bytes()
}

pub(super) fn compute_accept_key(key: &str) -> String {
    let mut hasher = Sha1::new();
    hasher.update(key.as_bytes());
    hasher.update(WEBSOCKET_GUID.as_bytes());
    base64::engine::general_purpose::STANDARD.encode(hasher.finalize())
}

pub(super) fn generate_sec_websocket_key() -> String {
    let seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let bytes = seed.to_le_bytes();
    base64::engine::general_purpose::STANDARD.encode(&bytes[..16])
}

#[derive(Debug, Clone)]
pub(super) struct HttpResponse {
    pub(super) status_code: u16,
    pub(super) status_text: String,
    pub(super) headers: Vec<(String, String)>,
}

impl HttpResponse {
    pub(super) fn header(&self, key: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .map(|(_, v)| v.as_str())
    }

    pub(super) fn parse(data: &[u8]) -> Option<(Self, usize)> {
        let header_end = find_header_end(data)?;
        let header_str = std::str::from_utf8(&data[..header_end]).ok()?;

        let mut lines = header_str.lines();
        let first_line = lines.next()?;
        let parts: Vec<&str> = first_line.splitn(3, ' ').collect();
        if parts.len() < 2 {
            return None;
        }

        let status_code: u16 = parts[1].parse().ok()?;
        let status_text = parts.get(2).unwrap_or(&"").to_string();

        let mut headers = Vec::new();
        for line in lines {
            if line.is_empty() {
                break;
            }
            if let Some(colon_pos) = line.find(':') {
                let key = line[..colon_pos].trim().to_string();
                let value = line[colon_pos + 1..].trim().to_string();
                headers.push((key, value));
            }
        }

        let header_total = header_end + 4;
        Some((
            HttpResponse {
                status_code,
                status_text,
                headers,
            },
            header_total,
        ))
    }
}

fn find_header_end(data: &[u8]) -> Option<usize> {
    (0..data.len().saturating_sub(3)).find(|&i| &data[i..i + 4] == b"\r\n\r\n")
}

pub(super) async fn read_http1_response_with_leftover<R: AsyncRead + Unpin>(
    reader: &mut R,
) -> Result<(HttpResponse, BytesMut), String> {
    let mut buf = BytesMut::with_capacity(8192);
    let mut chunk = [0u8; 4096];
    let max = 64 * 1024;

    loop {
        if buf.len() > max {
            return Err("HTTP response headers too large".to_string());
        }

        if let Some((resp, consumed)) = HttpResponse::parse(&buf) {
            let leftover = buf.split_off(consumed);
            return Ok((resp, leftover));
        }

        let n = reader
            .read(&mut chunk)
            .await
            .map_err(|e| format!("Failed to read handshake response: {}", e))?;
        if n == 0 {
            return Err("Upstream closed connection during handshake".to_string());
        }
        buf.extend_from_slice(&chunk[..n]);
    }
}

pub(super) fn header_values(resp: &HttpResponse, name: &str) -> Vec<String> {
    resp.headers
        .iter()
        .filter(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.clone())
        .collect()
}

pub(super) fn negotiate_protocol(
    client_offer: Option<&str>,
    upstream_selected: Option<&str>,
) -> Option<String> {
    let upstream_selected = upstream_selected?.trim();
    if upstream_selected.is_empty() {
        return None;
    }
    let offered = client_offer?
        .split(',')
        .map(|p| p.trim())
        .filter(|p| !p.is_empty())
        .collect::<HashSet<_>>();
    if offered.contains(upstream_selected) {
        Some(upstream_selected.to_string())
    } else {
        None
    }
}

pub(super) fn negotiate_extensions(
    client_offer: Option<&str>,
    upstream_values: &[String],
) -> Option<String> {
    let client_offer = client_offer?;
    let offered = client_offer
        .split(',')
        .map(|ext| ext.trim())
        .filter(|ext| !ext.is_empty())
        .map(|ext| {
            ext.split(';')
                .next()
                .unwrap_or(ext)
                .trim()
                .to_ascii_lowercase()
        })
        .collect::<HashSet<_>>();

    if offered.is_empty() {
        return None;
    }

    let mut accepted_segments = Vec::new();
    for v in upstream_values {
        for seg in v.split(',') {
            let seg = seg.trim();
            if seg.is_empty() {
                continue;
            }
            let name = seg
                .split(';')
                .next()
                .unwrap_or(seg)
                .trim()
                .to_ascii_lowercase();
            if offered.contains(&name) {
                accepted_segments.push(seg.to_string());
            }
        }
    }

    if accepted_segments.is_empty() {
        None
    } else {
        Some(accepted_segments.join(", "))
    }
}

pub(super) fn parse_permessage_deflate(extensions: &str) -> bool {
    extensions
        .split(',')
        .any(|ext| ext.trim().starts_with("permessage-deflate"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;
    use bytes::Bytes;
    use flate2::write::DeflateEncoder;
    use flate2::Compression;
    use std::io::Write;

    #[test]
    fn websocket_frame_encode_and_parse_roundtrip_unmasked() {
        let frame = WebSocketFrame {
            fin: true,
            rsv1: false,
            rsv2: false,
            rsv3: false,
            opcode: Opcode::Text,
            mask: None,
            payload: Bytes::from_static(b"hello"),
        };

        let encoded = frame.encode();
        let (decoded, consumed) = WebSocketFrame::parse(&encoded).expect("frame should parse");

        assert_eq!(consumed, encoded.len());
        assert!(decoded.fin);
        assert_eq!(decoded.opcode, Opcode::Text);
        assert!(decoded.mask.is_none());
        assert_eq!(decoded.payload, Bytes::from_static(b"hello"));
    }

    #[test]
    fn websocket_frame_encode_and_parse_roundtrip_masked() {
        let frame = WebSocketFrame {
            fin: true,
            rsv1: false,
            rsv2: false,
            rsv3: false,
            opcode: Opcode::Binary,
            mask: Some([1, 2, 3, 4]),
            payload: Bytes::from_static(b"abc"),
        };

        let encoded = frame.encode();
        let (decoded, consumed) = WebSocketFrame::parse(&encoded).expect("frame should parse");

        assert_eq!(consumed, encoded.len());
        assert_eq!(decoded.opcode, Opcode::Binary);
        assert_eq!(decoded.payload, Bytes::from_static(b"abc"));
        assert_eq!(decoded.mask, Some([1, 2, 3, 4]));
    }

    #[test]
    fn websocket_frame_close_code_and_reason_helpers() {
        let mut payload = Vec::new();
        payload.extend_from_slice(&1000u16.to_be_bytes());
        payload.extend_from_slice(b"normal");

        let frame = WebSocketFrame {
            fin: true,
            rsv1: false,
            rsv2: false,
            rsv3: false,
            opcode: Opcode::Close,
            mask: None,
            payload: Bytes::from(payload),
        };

        assert_eq!(frame.close_code(), Some(1000));
        assert_eq!(frame.close_reason(), Some("normal"));
    }

    #[test]
    fn validate_frame_header_rejects_invalid_control_frame() {
        // Control frame (opcode=Close) with FIN=0 is invalid
        let buf = [Opcode::Close as u8, 0];
        let err = validate_frame_header(&buf).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn validate_frame_header_rejects_too_large_payload() {
        // 127 indicates 64-bit extended length. Use a value larger than MAX_FRAME_PAYLOAD_LEN.
        let mut buf = Vec::new();
        buf.push(0x81); // FIN + Text opcode
        buf.push(127); // extended 64-bit length, no mask bit
        let big_len = (MAX_FRAME_PAYLOAD_LEN as u64) + 1;
        buf.extend_from_slice(&big_len.to_be_bytes());

        let err = validate_frame_header(&buf).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn decompress_permessage_deflate_roundtrip() {
        let original = b"hello permessage-deflate";
        let mut encoder = DeflateEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(original).unwrap();
        let mut compressed = encoder.finish().unwrap();

        // Some encoders append the RFC 7692 tail; strip it so that the helper must add it back.
        if compressed.ends_with(&[0x00, 0x00, 0xff, 0xff]) {
            compressed.truncate(compressed.len() - 4);
        }

        let payload = Bytes::from(compressed);
        let decompressed = decompress_permessage_deflate_payload(&payload);
        assert_eq!(decompressed.as_ref(), original);
    }

    #[test]
    fn compute_accept_key_matches_rfc_example() {
        let key = "dGhlIHNhbXBsZSBub25jZQ==";
        let accept = compute_accept_key(key);
        assert_eq!(accept, "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=");
    }

    #[test]
    fn generate_sec_websocket_key_produces_base64_16_bytes() {
        let key = generate_sec_websocket_key();
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(key.as_bytes())
            .unwrap();
        assert_eq!(decoded.len(), 16);
    }

    #[test]
    fn http_response_parse_and_header_helpers_work() {
        let raw = b"HTTP/1.1 101 Switching Protocols\r\n\
                    Sec-WebSocket-Accept: abc\r\n\
                    X-Test: one\r\n\
                    x-test: two\r\n\
                    \r\n";
        let (resp, consumed) = HttpResponse::parse(raw).expect("parse");
        assert_eq!(consumed, raw.len());
        assert_eq!(resp.status_code, 101);
        assert_eq!(resp.status_text, "Switching Protocols");
        assert_eq!(resp.header("Sec-WebSocket-Accept"), Some("abc"));

        let values = header_values(&resp, "x-test");
        assert_eq!(values, vec!["one".to_string(), "two".to_string()]);
    }

    #[test]
    fn negotiate_protocol_selects_common_value() {
        assert_eq!(
            negotiate_protocol(Some("chat, superproto"), Some("chat")),
            Some("chat".to_string())
        );
        assert_eq!(
            negotiate_protocol(Some("chat, superproto"), Some("other")),
            None
        );
        assert_eq!(negotiate_protocol(None, Some("chat")), None);
        assert_eq!(negotiate_protocol(Some("chat"), None), None);
    }

    #[test]
    fn negotiate_extensions_filters_by_client_offer_and_upstream() {
        let client_offer = Some("permessage-deflate; client_max_window_bits, x-custom");
        let upstream_values = vec![
            "permessage-deflate; server_max_window_bits=15".to_string(),
            "x-other".to_string(),
        ];

        let negotiated = negotiate_extensions(client_offer, &upstream_values);
        assert_eq!(
            negotiated.as_deref(),
            Some("permessage-deflate; server_max_window_bits=15")
        );

        assert!(negotiate_extensions(Some("x-unknown"), &upstream_values).is_none());
    }

    #[test]
    fn parse_permessage_deflate_detects_extension() {
        assert!(parse_permessage_deflate(
            "permessage-deflate; client_max_window_bits"
        ));
        assert!(parse_permessage_deflate("x-custom, permessage-deflate"));
        assert!(!parse_permessage_deflate("x-custom, y-other"));
    }
}
