use std::collections::HashMap;

use bytes::{BufMut, Bytes, BytesMut};

pub const H2_PREFACE: &[u8] = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n";
pub const H2_PREFACE_LEN: usize = 24;

pub const FRAME_HEADER_SIZE: usize = 9;

pub const SETTINGS_HEADER_TABLE_SIZE: u16 = 0x1;
pub const SETTINGS_ENABLE_PUSH: u16 = 0x2;
pub const SETTINGS_MAX_CONCURRENT_STREAMS: u16 = 0x3;
pub const SETTINGS_INITIAL_WINDOW_SIZE: u16 = 0x4;
pub const SETTINGS_MAX_FRAME_SIZE: u16 = 0x5;
pub const SETTINGS_MAX_HEADER_LIST_SIZE: u16 = 0x6;

pub const DEFAULT_INITIAL_WINDOW_SIZE: u32 = 65535;
pub const DEFAULT_MAX_FRAME_SIZE: u32 = 16384;
pub const MAX_FRAME_SIZE_LIMIT: u32 = 16777215;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum FrameType {
    Data = 0x0,
    Headers = 0x1,
    Priority = 0x2,
    RstStream = 0x3,
    Settings = 0x4,
    PushPromise = 0x5,
    Ping = 0x6,
    GoAway = 0x7,
    WindowUpdate = 0x8,
    Continuation = 0x9,
    Unknown(u8),
}

impl From<u8> for FrameType {
    fn from(value: u8) -> Self {
        match value {
            0x0 => FrameType::Data,
            0x1 => FrameType::Headers,
            0x2 => FrameType::Priority,
            0x3 => FrameType::RstStream,
            0x4 => FrameType::Settings,
            0x5 => FrameType::PushPromise,
            0x6 => FrameType::Ping,
            0x7 => FrameType::GoAway,
            0x8 => FrameType::WindowUpdate,
            0x9 => FrameType::Continuation,
            _ => FrameType::Unknown(value),
        }
    }
}

impl From<FrameType> for u8 {
    fn from(value: FrameType) -> Self {
        match value {
            FrameType::Data => 0x0,
            FrameType::Headers => 0x1,
            FrameType::Priority => 0x2,
            FrameType::RstStream => 0x3,
            FrameType::Settings => 0x4,
            FrameType::PushPromise => 0x5,
            FrameType::Ping => 0x6,
            FrameType::GoAway => 0x7,
            FrameType::WindowUpdate => 0x8,
            FrameType::Continuation => 0x9,
            FrameType::Unknown(v) => v,
        }
    }
}

pub mod flags {
    pub const END_STREAM: u8 = 0x1;
    pub const END_HEADERS: u8 = 0x4;
    pub const PADDED: u8 = 0x8;
    pub const PRIORITY: u8 = 0x20;
    pub const ACK: u8 = 0x1;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum ErrorCode {
    NoError = 0x0,
    ProtocolError = 0x1,
    InternalError = 0x2,
    FlowControlError = 0x3,
    SettingsTimeout = 0x4,
    StreamClosed = 0x5,
    FrameSizeError = 0x6,
    RefusedStream = 0x7,
    Cancel = 0x8,
    CompressionError = 0x9,
    ConnectError = 0xa,
    EnhanceYourCalm = 0xb,
    InadequateSecurity = 0xc,
    Http11Required = 0xd,
}

impl From<u32> for ErrorCode {
    fn from(value: u32) -> Self {
        match value {
            0x0 => ErrorCode::NoError,
            0x1 => ErrorCode::ProtocolError,
            0x2 => ErrorCode::InternalError,
            0x3 => ErrorCode::FlowControlError,
            0x4 => ErrorCode::SettingsTimeout,
            0x5 => ErrorCode::StreamClosed,
            0x6 => ErrorCode::FrameSizeError,
            0x7 => ErrorCode::RefusedStream,
            0x8 => ErrorCode::Cancel,
            0x9 => ErrorCode::CompressionError,
            0xa => ErrorCode::ConnectError,
            0xb => ErrorCode::EnhanceYourCalm,
            0xc => ErrorCode::InadequateSecurity,
            0xd => ErrorCode::Http11Required,
            _ => ErrorCode::ProtocolError,
        }
    }
}

#[derive(Debug, Clone)]
pub struct FrameHeader {
    pub length: u32,
    pub frame_type: FrameType,
    pub flags: u8,
    pub stream_id: u32,
}

impl FrameHeader {
    pub fn new(frame_type: FrameType, flags: u8, stream_id: u32, length: u32) -> Self {
        Self {
            length,
            frame_type,
            flags,
            stream_id,
        }
    }

    pub fn encode(&self) -> Bytes {
        let mut buf = BytesMut::with_capacity(FRAME_HEADER_SIZE);
        buf.put_u8((self.length >> 16) as u8);
        buf.put_u8((self.length >> 8) as u8);
        buf.put_u8(self.length as u8);
        buf.put_u8(self.frame_type.into());
        buf.put_u8(self.flags);
        buf.put_u32(self.stream_id & 0x7FFFFFFF);
        buf.freeze()
    }

    pub fn parse(data: &[u8]) -> Option<Self> {
        if data.len() < FRAME_HEADER_SIZE {
            return None;
        }

        let length = ((data[0] as u32) << 16) | ((data[1] as u32) << 8) | (data[2] as u32);
        let frame_type = FrameType::from(data[3]);
        let flags = data[4];
        let stream_id = u32::from_be_bytes([data[5], data[6], data[7], data[8]]) & 0x7FFFFFFF;

        Some(Self {
            length,
            frame_type,
            flags,
            stream_id,
        })
    }

    pub fn has_flag(&self, flag: u8) -> bool {
        self.flags & flag != 0
    }
}

#[derive(Debug, Clone)]
pub struct Frame {
    pub header: FrameHeader,
    pub payload: Bytes,
}

impl Frame {
    pub fn new(header: FrameHeader, payload: Bytes) -> Self {
        Self { header, payload }
    }

    pub fn encode(&self) -> Bytes {
        let header = self.header.encode();
        let mut buf = BytesMut::with_capacity(header.len() + self.payload.len());
        buf.extend_from_slice(&header);
        buf.extend_from_slice(&self.payload);
        buf.freeze()
    }

    pub fn parse(data: &[u8]) -> Option<(Self, usize)> {
        let header = FrameHeader::parse(data)?;
        let total_len = FRAME_HEADER_SIZE + header.length as usize;

        if data.len() < total_len {
            return None;
        }

        let payload = Bytes::copy_from_slice(&data[FRAME_HEADER_SIZE..total_len]);
        Some((Self::new(header, payload), total_len))
    }
}

pub fn build_settings_frame(settings: &[(u16, u32)], ack: bool) -> Frame {
    let flags = if ack { flags::ACK } else { 0 };

    if ack || settings.is_empty() {
        return Frame::new(
            FrameHeader::new(FrameType::Settings, flags, 0, 0),
            Bytes::new(),
        );
    }

    let mut payload = BytesMut::with_capacity(settings.len() * 6);
    for (id, value) in settings {
        payload.put_u16(*id);
        payload.put_u32(*value);
    }

    Frame::new(
        FrameHeader::new(FrameType::Settings, 0, 0, payload.len() as u32),
        payload.freeze(),
    )
}

pub fn build_window_update_frame(stream_id: u32, increment: u32) -> Frame {
    let mut payload = BytesMut::with_capacity(4);
    payload.put_u32(increment & 0x7FFFFFFF);

    Frame::new(
        FrameHeader::new(FrameType::WindowUpdate, 0, stream_id, 4),
        payload.freeze(),
    )
}

pub fn build_ping_frame(data: &[u8; 8], ack: bool) -> Frame {
    let flags = if ack { flags::ACK } else { 0 };

    Frame::new(
        FrameHeader::new(FrameType::Ping, flags, 0, 8),
        Bytes::copy_from_slice(data),
    )
}

pub fn build_goaway_frame(last_stream_id: u32, error_code: ErrorCode, debug_data: &[u8]) -> Frame {
    let mut payload = BytesMut::with_capacity(8 + debug_data.len());
    payload.put_u32(last_stream_id & 0x7FFFFFFF);
    payload.put_u32(error_code as u32);
    payload.extend_from_slice(debug_data);

    Frame::new(
        FrameHeader::new(FrameType::GoAway, 0, 0, payload.len() as u32),
        payload.freeze(),
    )
}

pub fn build_rst_stream_frame(stream_id: u32, error_code: ErrorCode) -> Frame {
    let mut payload = BytesMut::with_capacity(4);
    payload.put_u32(error_code as u32);

    Frame::new(
        FrameHeader::new(FrameType::RstStream, 0, stream_id, 4),
        payload.freeze(),
    )
}

pub fn build_data_frame(stream_id: u32, data: &[u8], end_stream: bool) -> Frame {
    let flags = if end_stream { flags::END_STREAM } else { 0 };

    Frame::new(
        FrameHeader::new(FrameType::Data, flags, stream_id, data.len() as u32),
        Bytes::copy_from_slice(data),
    )
}

pub fn parse_settings_payload(payload: &[u8]) -> HashMap<u16, u32> {
    let mut settings = HashMap::new();
    let mut offset = 0;

    while offset + 6 <= payload.len() {
        let id = u16::from_be_bytes([payload[offset], payload[offset + 1]]);
        let value = u32::from_be_bytes([
            payload[offset + 2],
            payload[offset + 3],
            payload[offset + 4],
            payload[offset + 5],
        ]);
        settings.insert(id, value);
        offset += 6;
    }

    settings
}

pub fn is_h2_preface(data: &[u8]) -> bool {
    data.starts_with(H2_PREFACE)
}

pub fn default_settings() -> Vec<(u16, u32)> {
    vec![
        (SETTINGS_MAX_CONCURRENT_STREAMS, 100),
        (SETTINGS_INITIAL_WINDOW_SIZE, DEFAULT_INITIAL_WINDOW_SIZE),
        (SETTINGS_MAX_FRAME_SIZE, DEFAULT_MAX_FRAME_SIZE),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_frame_type_conversion() {
        assert_eq!(FrameType::from(0x0), FrameType::Data);
        assert_eq!(FrameType::from(0x1), FrameType::Headers);
        assert_eq!(FrameType::from(0x4), FrameType::Settings);
        assert!(matches!(FrameType::from(0xFF), FrameType::Unknown(0xFF)));

        assert_eq!(u8::from(FrameType::Data), 0x0);
        assert_eq!(u8::from(FrameType::Headers), 0x1);
        assert_eq!(u8::from(FrameType::Settings), 0x4);
    }

    #[test]
    fn coverage_90_all_frame_and_error_code_variants_roundtrip() {
        for value in 0_u8..=10 {
            let frame_type = FrameType::from(value);
            let encoded = u8::from(frame_type);
            assert_eq!(encoded, value);
        }
        for (value, expected) in [
            (0, ErrorCode::NoError),
            (1, ErrorCode::ProtocolError),
            (2, ErrorCode::InternalError),
            (3, ErrorCode::FlowControlError),
            (4, ErrorCode::SettingsTimeout),
            (5, ErrorCode::StreamClosed),
            (6, ErrorCode::FrameSizeError),
            (7, ErrorCode::RefusedStream),
            (8, ErrorCode::Cancel),
            (9, ErrorCode::CompressionError),
            (10, ErrorCode::ConnectError),
            (11, ErrorCode::EnhanceYourCalm),
            (12, ErrorCode::InadequateSecurity),
            (13, ErrorCode::Http11Required),
            (14, ErrorCode::ProtocolError),
        ] {
            assert_eq!(ErrorCode::from(value), expected);
        }
        assert!(FrameHeader::parse(&[0; FRAME_HEADER_SIZE - 1]).is_none());
        let header = FrameHeader::new(FrameType::Data, 0, 1, 4).encode();
        assert!(Frame::parse(&header).is_none());
    }

    #[test]
    fn test_frame_header_encode_decode() {
        let header = FrameHeader::new(FrameType::Settings, flags::ACK, 0, 0);
        let encoded = header.encode();
        let decoded = FrameHeader::parse(&encoded).unwrap();

        assert_eq!(decoded.length, 0);
        assert_eq!(decoded.frame_type, FrameType::Settings);
        assert_eq!(decoded.flags, flags::ACK);
        assert_eq!(decoded.stream_id, 0);
    }

    #[test]
    fn test_frame_header_with_payload() {
        let header = FrameHeader::new(FrameType::Data, flags::END_STREAM, 1, 100);
        let encoded = header.encode();
        let decoded = FrameHeader::parse(&encoded).unwrap();

        assert_eq!(decoded.length, 100);
        assert_eq!(decoded.frame_type, FrameType::Data);
        assert!(decoded.has_flag(flags::END_STREAM));
        assert_eq!(decoded.stream_id, 1);
    }

    #[test]
    fn test_frame_encode_decode() {
        let header = FrameHeader::new(FrameType::Data, 0, 1, 5);
        let payload = Bytes::from("hello");
        let frame = Frame::new(header, payload);

        let encoded = frame.encode();
        let (decoded, consumed) = Frame::parse(&encoded).unwrap();

        assert_eq!(decoded.header.frame_type, FrameType::Data);
        assert_eq!(decoded.header.stream_id, 1);
        assert_eq!(decoded.payload.as_ref(), b"hello");
        assert_eq!(consumed, encoded.len());
    }

    #[test]
    fn test_build_settings_frame() {
        let settings = vec![
            (SETTINGS_MAX_CONCURRENT_STREAMS, 100),
            (SETTINGS_INITIAL_WINDOW_SIZE, 65535),
        ];
        let frame = build_settings_frame(&settings, false);

        assert_eq!(frame.header.frame_type, FrameType::Settings);
        assert_eq!(frame.header.stream_id, 0);
        assert_eq!(frame.payload.len(), 12);
    }

    #[test]
    fn test_build_settings_ack() {
        let frame = build_settings_frame(&[], true);

        assert_eq!(frame.header.frame_type, FrameType::Settings);
        assert!(frame.header.has_flag(flags::ACK));
        assert_eq!(frame.payload.len(), 0);
    }

    #[test]
    fn test_build_window_update_frame() {
        let frame = build_window_update_frame(1, 1024);

        assert_eq!(frame.header.frame_type, FrameType::WindowUpdate);
        assert_eq!(frame.header.stream_id, 1);
        assert_eq!(frame.header.length, 4);
    }

    #[test]
    fn test_build_ping_frame() {
        let data = [1, 2, 3, 4, 5, 6, 7, 8];
        let frame = build_ping_frame(&data, false);

        assert_eq!(frame.header.frame_type, FrameType::Ping);
        assert!(!frame.header.has_flag(flags::ACK));
        assert_eq!(frame.payload.as_ref(), &data);
    }

    #[test]
    fn test_build_ping_ack() {
        let data = [1, 2, 3, 4, 5, 6, 7, 8];
        let frame = build_ping_frame(&data, true);

        assert!(frame.header.has_flag(flags::ACK));
    }

    #[test]
    fn test_build_goaway_frame() {
        let frame = build_goaway_frame(0, ErrorCode::NoError, b"goodbye");

        assert_eq!(frame.header.frame_type, FrameType::GoAway);
        assert_eq!(frame.header.stream_id, 0);
    }

    #[test]
    fn test_build_rst_stream_frame() {
        let frame = build_rst_stream_frame(1, ErrorCode::Cancel);

        assert_eq!(frame.header.frame_type, FrameType::RstStream);
        assert_eq!(frame.header.stream_id, 1);
        assert_eq!(frame.header.length, 4);
    }

    #[test]
    fn test_build_data_frame() {
        let frame = build_data_frame(1, b"hello", true);

        assert_eq!(frame.header.frame_type, FrameType::Data);
        assert_eq!(frame.header.stream_id, 1);
        assert!(frame.header.has_flag(flags::END_STREAM));
        assert_eq!(frame.payload.as_ref(), b"hello");
    }

    #[test]
    fn test_parse_settings_payload() {
        let mut payload = BytesMut::new();
        payload.put_u16(SETTINGS_MAX_CONCURRENT_STREAMS);
        payload.put_u32(100);
        payload.put_u16(SETTINGS_INITIAL_WINDOW_SIZE);
        payload.put_u32(65535);

        let settings = parse_settings_payload(&payload);

        assert_eq!(settings.get(&SETTINGS_MAX_CONCURRENT_STREAMS), Some(&100));
        assert_eq!(settings.get(&SETTINGS_INITIAL_WINDOW_SIZE), Some(&65535));
    }

    #[test]
    fn test_is_h2_preface() {
        assert!(is_h2_preface(H2_PREFACE));
        assert!(is_h2_preface(b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\nextra data"));
        assert!(!is_h2_preface(b"GET / HTTP/1.1"));
    }

    #[test]
    fn test_error_code_conversion() {
        assert_eq!(ErrorCode::from(0x0), ErrorCode::NoError);
        assert_eq!(ErrorCode::from(0x8), ErrorCode::Cancel);
        assert_eq!(ErrorCode::from(0xFFFF), ErrorCode::ProtocolError);
    }

    #[test]
    fn test_default_settings() {
        let settings = default_settings();
        assert!(!settings.is_empty());
        assert!(settings
            .iter()
            .any(|(id, _)| *id == SETTINGS_MAX_CONCURRENT_STREAMS));
    }
}
