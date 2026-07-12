use bytes::{BufMut, Bytes, BytesMut};

use super::PerMessageDeflateInflater;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Opcode {
    Continuation = 0x0,
    Text = 0x1,
    Binary = 0x2,
    Close = 0x8,
    Ping = 0x9,
    Pong = 0xA,
}

impl Opcode {
    pub fn from_u8(value: u8) -> Option<Self> {
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

    pub fn is_control(&self) -> bool {
        matches!(self, Opcode::Close | Opcode::Ping | Opcode::Pong)
    }
}

#[derive(Debug, Clone)]
pub struct WebSocketFrame {
    pub fin: bool,
    pub rsv1: bool,
    pub rsv2: bool,
    pub rsv3: bool,
    pub opcode: Opcode,
    pub mask: Option<[u8; 4]>,
    pub payload: Bytes,
}

impl WebSocketFrame {
    pub fn text(data: impl AsRef<str>) -> Self {
        Self {
            fin: true,
            rsv1: false,
            rsv2: false,
            rsv3: false,
            opcode: Opcode::Text,
            mask: None,
            payload: Bytes::copy_from_slice(data.as_ref().as_bytes()),
        }
    }

    pub fn binary(data: impl AsRef<[u8]>) -> Self {
        Self {
            fin: true,
            rsv1: false,
            rsv2: false,
            rsv3: false,
            opcode: Opcode::Binary,
            mask: None,
            payload: Bytes::copy_from_slice(data.as_ref()),
        }
    }

    pub fn ping(data: impl AsRef<[u8]>) -> Self {
        Self {
            fin: true,
            rsv1: false,
            rsv2: false,
            rsv3: false,
            opcode: Opcode::Ping,
            mask: None,
            payload: Bytes::copy_from_slice(data.as_ref()),
        }
    }

    pub fn pong(data: impl AsRef<[u8]>) -> Self {
        Self {
            fin: true,
            rsv1: false,
            rsv2: false,
            rsv3: false,
            opcode: Opcode::Pong,
            mask: None,
            payload: Bytes::copy_from_slice(data.as_ref()),
        }
    }

    pub fn close(code: Option<u16>, reason: &str) -> Self {
        let mut payload = BytesMut::new();
        if let Some(code) = code {
            payload.put_u16(code);
            payload.extend_from_slice(reason.as_bytes());
        }
        Self {
            fin: true,
            rsv1: false,
            rsv2: false,
            rsv3: false,
            opcode: Opcode::Close,
            mask: None,
            payload: payload.freeze(),
        }
    }

    pub fn with_mask(mut self, mask: [u8; 4]) -> Self {
        self.mask = Some(mask);
        self
    }

    pub fn encode(&self) -> Bytes {
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

    pub fn parse(data: &[u8]) -> Option<(Self, usize)> {
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

    pub fn close_code(&self) -> Option<u16> {
        if self.opcode == Opcode::Close && self.payload.len() >= 2 {
            Some(u16::from_be_bytes([self.payload[0], self.payload[1]]))
        } else {
            None
        }
    }

    pub fn close_reason(&self) -> Option<&str> {
        if self.opcode == Opcode::Close && self.payload.len() > 2 {
            std::str::from_utf8(&self.payload[2..]).ok()
        } else {
            None
        }
    }

    pub fn decompress_payload(&self) -> Bytes {
        if !self.rsv1 || self.payload.is_empty() {
            return self.payload.clone();
        }

        let mut inflater = PerMessageDeflateInflater::new();
        match inflater.decompress_message(self.payload.as_ref()) {
            Ok(bytes) => bytes,
            Err(e) => {
                tracing::debug!("[WS] Failed to decompress frame payload: {}", e);
                self.payload.clone()
            }
        }
    }

    pub fn is_compressed(&self) -> bool {
        self.rsv1
    }
}

#[cfg(test)]
mod coverage_90_tests {
    use super::*;

    #[test]
    fn large_payload_uses_64_bit_length_and_rejects_truncation() {
        let frame = WebSocketFrame::binary(vec![b'x'; 70_000]);
        let encoded = frame.encode();
        assert_eq!(encoded[1] & 0x7f, 127);
        assert!(WebSocketFrame::parse(&encoded[..9]).is_none());
        assert!(WebSocketFrame::parse(&encoded[..encoded.len() - 1]).is_none());
        let (parsed, consumed) = WebSocketFrame::parse(&encoded).unwrap();
        assert_eq!(parsed.payload.len(), 70_000);
        assert_eq!(consumed, encoded.len());
    }

    #[test]
    fn invalid_compressed_payload_falls_back_to_wire_bytes() {
        let mut frame = WebSocketFrame::text("invalid-deflate");
        frame.rsv1 = true;
        assert_eq!(frame.decompress_payload(), frame.payload);
    }
}
