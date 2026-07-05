mod capture;
mod upgrade;

pub use capture::websocket_bidirectional_generic_with_capture;
pub use upgrade::handle_websocket_upgrade;

use crate::protocol::Opcode;
use bifrost_admin::{AdminState, FrameType};
use bifrost_core::{BifrostError, Result};

fn persist_socket_summary(state: &AdminState, record_id: &str) {
    let status = state.connection_monitor.get_connection_status(record_id);
    let last_frame_id = state
        .connection_monitor
        .get_last_frame_id(record_id)
        .unwrap_or(0);
    let frame_count = status.as_ref().map(|s| s.frame_count).unwrap_or(0);
    let status = status.map(|mut s| {
        s.is_open = false;
        s
    });
    let response_size = status
        .as_ref()
        .map(|s| s.send_bytes + s.receive_bytes)
        .unwrap_or(0) as usize;
    state.update_traffic_by_id(record_id, move |record| {
        if let Some(ref s) = status {
            record.upload_bytes = s.send_bytes as usize;
            record.download_bytes = s.receive_bytes as usize;
        }
        record.response_size = response_size;
        record.frame_count = frame_count;
        record.last_frame_id = last_frame_id;
        if let Some(ref s) = status {
            record.socket_status = Some(s.clone());
        }
    });
}

#[cfg(test)]
fn extract_sec_websocket_accept(response: &str) -> Option<String> {
    for line in response.lines() {
        if line.to_lowercase().starts_with("sec-websocket-accept:") {
            return Some(
                line.split(':')
                    .skip(1)
                    .collect::<String>()
                    .trim()
                    .to_string(),
            );
        }
    }
    None
}

fn parse_host_port(host: &str) -> Result<(String, u16)> {
    let host_without_path = host.split('/').next().unwrap_or(host);

    let parts: Vec<&str> = host_without_path.split(':').collect();
    match parts.len() {
        1 => Ok((parts[0].to_string(), 80)),
        2 => {
            let port = parts[1]
                .parse()
                .map_err(|_| BifrostError::Parse(format!("Invalid port: {}", parts[1])))?;
            Ok((parts[0].to_string(), port))
        }
        _ => Err(BifrostError::Parse(format!("Invalid host: {}", host))),
    }
}

fn opcode_to_frame_type(opcode: Opcode) -> FrameType {
    match opcode {
        Opcode::Continuation => FrameType::Continuation,
        Opcode::Text => FrameType::Text,
        Opcode::Binary => FrameType::Binary,
        Opcode::Close => FrameType::Close,
        Opcode::Ping => FrameType::Ping,
        Opcode::Pong => FrameType::Pong,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_host_port_with_port() {
        let (host, port) = parse_host_port("example.com:8080").unwrap();
        assert_eq!(host, "example.com");
        assert_eq!(port, 8080);
    }

    #[test]
    fn test_parse_host_port_default() {
        let (host, port) = parse_host_port("example.com").unwrap();
        assert_eq!(host, "example.com");
        assert_eq!(port, 80);
    }

    #[test]
    fn test_parse_host_port_invalid() {
        let result = parse_host_port("example.com:invalid");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_host_port_with_path() {
        let (host, port) = parse_host_port("127.0.0.1:3020/ws").unwrap();
        assert_eq!(host, "127.0.0.1");
        assert_eq!(port, 3020);
    }

    #[test]
    fn test_parse_host_port_with_path_no_port() {
        let (host, port) = parse_host_port("example.com/ws/path").unwrap();
        assert_eq!(host, "example.com");
        assert_eq!(port, 80);
    }

    #[test]
    fn test_websocket_opcode_from_u8() {
        use crate::protocol::{Opcode, WebSocketFrame};
        assert_eq!(Opcode::from_u8(0x0), Some(Opcode::Continuation));
        assert_eq!(Opcode::from_u8(0x1), Some(Opcode::Text));
        assert_eq!(Opcode::from_u8(0x2), Some(Opcode::Binary));
        assert_eq!(Opcode::from_u8(0x8), Some(Opcode::Close));
        assert_eq!(Opcode::from_u8(0x9), Some(Opcode::Ping));
        assert_eq!(Opcode::from_u8(0xA), Some(Opcode::Pong));
        assert_eq!(Opcode::from_u8(0xF), None);
        let _ = WebSocketFrame::text("");
    }

    #[test]
    fn test_websocket_frame_text() {
        use crate::protocol::{Opcode, WebSocketFrame};
        let frame = WebSocketFrame::text("hello");
        assert!(frame.fin);
        assert_eq!(frame.opcode, Opcode::Text);
        assert!(frame.mask.is_none());
        assert_eq!(frame.payload, bytes::Bytes::from("hello"));
    }

    #[test]
    fn test_websocket_frame_binary() {
        use crate::protocol::{Opcode, WebSocketFrame};
        let data = vec![0x01, 0x02, 0x03];
        let frame = WebSocketFrame::binary(data.clone());
        assert!(frame.fin);
        assert_eq!(frame.opcode, Opcode::Binary);
        assert_eq!(frame.payload, bytes::Bytes::from(data));
    }

    #[test]
    fn test_websocket_frame_close() {
        use crate::protocol::{Opcode, WebSocketFrame};
        let frame = WebSocketFrame::close(None, "");
        assert!(frame.fin);
        assert_eq!(frame.opcode, Opcode::Close);
    }

    #[test]
    fn test_websocket_frame_ping_pong() {
        use crate::protocol::{Opcode, WebSocketFrame};
        let ping = WebSocketFrame::ping("ping");
        assert_eq!(ping.opcode, Opcode::Ping);

        let pong = WebSocketFrame::pong("pong");
        assert_eq!(pong.opcode, Opcode::Pong);
    }

    #[test]
    fn test_extract_sec_websocket_accept() {
        let response = "HTTP/1.1 101 Switching Protocols\r\n\
                        Upgrade: websocket\r\n\
                        Connection: Upgrade\r\n\
                        Sec-WebSocket-Accept: s3pPLMBiTxaQ9kYGzzhZRbK+xOo=\r\n\r\n";
        let accept = extract_sec_websocket_accept(response);
        assert_eq!(accept, Some("s3pPLMBiTxaQ9kYGzzhZRbK+xOo=".to_string()));
    }

    #[test]
    fn test_extract_sec_websocket_accept_missing() {
        let response = "HTTP/1.1 101 Switching Protocols\r\n\
                        Upgrade: websocket\r\n\
                        Connection: Upgrade\r\n\r\n";
        let accept = extract_sec_websocket_accept(response);
        assert!(accept.is_none());
    }
}
