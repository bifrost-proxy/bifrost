use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use bifrost_core::{BifrostError, Result};
use bytes::BytesMut;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::{TcpStream, UdpSocket};
use tokio::sync::RwLock;
use tracing::debug;

use crate::protocol::{ProtocolDetector, QuicPacketDetector, TransportProtocol};

const PEEK_BUFFER_SIZE: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetectedProtocol {
    Http,
    Socks5,
    Socks4,
    Tls,
    Unknown,
}

impl From<TransportProtocol> for DetectedProtocol {
    fn from(p: TransportProtocol) -> Self {
        match p {
            TransportProtocol::Http1 | TransportProtocol::Http2 => DetectedProtocol::Http,
            TransportProtocol::Tls => DetectedProtocol::Tls,
            TransportProtocol::Socks5 => DetectedProtocol::Socks5,
            TransportProtocol::Socks4 => DetectedProtocol::Socks4,
            TransportProtocol::WebSocket
            | TransportProtocol::Sse
            | TransportProtocol::Grpc
            | TransportProtocol::Raw => DetectedProtocol::Http,
        }
    }
}

pub struct PeekableStream {
    stream: TcpStream,
    peeked_data: BytesMut,
    peeked_pos: usize,
}

impl PeekableStream {
    pub fn new(stream: TcpStream) -> Self {
        Self {
            stream,
            peeked_data: BytesMut::new(),
            peeked_pos: 0,
        }
    }

    pub async fn detect_protocol(&mut self) -> Result<DetectedProtocol> {
        let mut buf = [0u8; PEEK_BUFFER_SIZE];

        let n = self
            .stream
            .peek(&mut buf)
            .await
            .map_err(|e| BifrostError::Network(format!("Failed to peek stream: {}", e)))?;

        if n == 0 {
            return Ok(DetectedProtocol::Unknown);
        }

        self.peeked_data.extend_from_slice(&buf[..n]);

        match ProtocolDetector::detect_protocol_type(&buf[..n]) {
            Some(p) => Ok(DetectedProtocol::from(p)),
            None => Ok(DetectedProtocol::Unknown),
        }
    }

    pub fn into_inner(self) -> TcpStream {
        self.stream
    }

    pub fn peer_addr(&self) -> io::Result<SocketAddr> {
        self.stream.peer_addr()
    }
}

impl AsyncRead for PeekableStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if self.peeked_pos < self.peeked_data.len() {
            let remaining = &self.peeked_data[self.peeked_pos..];
            let to_copy = remaining.len().min(buf.remaining());
            buf.put_slice(&remaining[..to_copy]);
            self.peeked_pos += to_copy;
            return Poll::Ready(Ok(()));
        }

        Pin::new(&mut self.stream).poll_read(cx, buf)
    }
}

impl AsyncWrite for PeekableStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.stream).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_shutdown(cx)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UdpPacketType {
    Quic,
    Socks5Relay,
    Unknown,
}

pub struct UdpPacketDetector;

impl UdpPacketDetector {
    pub fn detect(
        data: &[u8],
        registered_clients: &[SocketAddr],
        source: &SocketAddr,
    ) -> UdpPacketType {
        if data.len() < 4 {
            return UdpPacketType::Unknown;
        }

        if registered_clients.contains(source) && Self::is_socks5_udp_packet(data) {
            return UdpPacketType::Socks5Relay;
        }

        if Self::is_quic_packet(data) {
            return UdpPacketType::Quic;
        }

        UdpPacketType::Unknown
    }

    fn is_quic_packet(data: &[u8]) -> bool {
        QuicPacketDetector::is_quic_packet(data)
    }

    fn is_socks5_udp_packet(data: &[u8]) -> bool {
        if data.len() < 10 {
            return false;
        }

        if data[0] != 0 || data[1] != 0 {
            return false;
        }

        let atyp = data[3];
        match atyp {
            0x01 => data.len() >= 10,
            0x03 => {
                if data.len() < 5 {
                    return false;
                }
                let domain_len = data[4] as usize;
                data.len() >= 5 + domain_len + 2
            }
            0x04 => data.len() >= 22,
            _ => false,
        }
    }
}

pub struct UnifiedUdpSocket {
    socket: Arc<UdpSocket>,
    registered_socks5_clients: Arc<RwLock<Vec<SocketAddr>>>,
}

impl UnifiedUdpSocket {
    pub fn new(socket: UdpSocket) -> Self {
        Self {
            socket: Arc::new(socket),
            registered_socks5_clients: Arc::new(RwLock::new(Vec::new())),
        }
    }

    pub async fn register_socks5_client(&self, addr: SocketAddr) {
        let mut clients = self.registered_socks5_clients.write().await;
        if !clients.contains(&addr) {
            clients.push(addr);
            debug!("Registered SOCKS5 UDP client: {}", addr);
        }
    }

    pub async fn unregister_socks5_client(&self, addr: &SocketAddr) {
        let mut clients = self.registered_socks5_clients.write().await;
        clients.retain(|a| a != addr);
        debug!("Unregistered SOCKS5 UDP client: {}", addr);
    }

    pub async fn recv_from_with_type(
        &self,
        buf: &mut [u8],
    ) -> io::Result<(usize, SocketAddr, UdpPacketType)> {
        let (len, addr) = self.socket.recv_from(buf).await?;
        let clients = self.registered_socks5_clients.read().await;
        let packet_type = UdpPacketDetector::detect(&buf[..len], &clients, &addr);
        Ok((len, addr, packet_type))
    }

    pub async fn send_to(&self, buf: &[u8], target: SocketAddr) -> io::Result<usize> {
        self.socket.send_to(buf, target).await
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.socket.local_addr()
    }

    pub fn inner(&self) -> &Arc<UdpSocket> {
        &self.socket
    }

    pub fn into_inner(self) -> Arc<UdpSocket> {
        self.socket
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[test]
    fn test_quic_long_header_detection() {
        let quic_long_header = [0xC0, 0x00, 0x00, 0x01];
        assert!(UdpPacketDetector::is_quic_packet(&quic_long_header));

        let quic_initial = [0xC3, 0x00, 0x00, 0x01, 0x08];
        assert!(UdpPacketDetector::is_quic_packet(&quic_initial));
    }

    #[test]
    fn test_quic_short_header_detection() {
        let quic_short_header = [
            0x40, 0x00, 0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b,
            0x0c, 0x0d, 0x0e, 0x0f, 0x10, 0x11,
        ];
        assert!(UdpPacketDetector::is_quic_packet(&quic_short_header));
    }

    #[test]
    fn test_socks5_udp_detection() {
        let socks5_ipv4 = [0x00, 0x00, 0x00, 0x01, 8, 8, 8, 8, 0x00, 0x35, 0x12, 0x34];
        assert!(UdpPacketDetector::is_socks5_udp_packet(&socks5_ipv4));

        let socks5_domain = [
            0x00, 0x00, 0x00, 0x03, 0x06, b'g', b'o', b'o', b'g', b'l', b'e', 0x01, 0xBB, 0x00,
        ];
        assert!(UdpPacketDetector::is_socks5_udp_packet(&socks5_domain));

        let invalid = [0x00, 0x01, 0x00, 0x01];
        assert!(!UdpPacketDetector::is_socks5_udp_packet(&invalid));
    }

    #[test]
    fn test_packet_type_detection() {
        let source = "127.0.0.1:12345".parse().unwrap();
        let registered = vec![source];

        let socks5_packet = [0x00, 0x00, 0x00, 0x01, 8, 8, 8, 8, 0x00, 0x35, 0x00];
        assert_eq!(
            UdpPacketDetector::detect(&socks5_packet, &registered, &source),
            UdpPacketType::Socks5Relay
        );

        let quic_packet = [0xC0, 0x00, 0x00, 0x01];
        assert_eq!(
            UdpPacketDetector::detect(&quic_packet, &registered, &source),
            UdpPacketType::Quic
        );

        let unknown_source: SocketAddr = "127.0.0.1:54321".parse().unwrap();
        assert_eq!(
            UdpPacketDetector::detect(&socks5_packet, &registered, &unknown_source),
            UdpPacketType::Unknown
        );
    }

    #[test]
    fn transport_protocol_conversion_covers_all_variants() {
        assert_eq!(
            DetectedProtocol::from(TransportProtocol::Http1),
            DetectedProtocol::Http
        );
        assert_eq!(
            DetectedProtocol::from(TransportProtocol::Http2),
            DetectedProtocol::Http
        );
        assert_eq!(
            DetectedProtocol::from(TransportProtocol::Tls),
            DetectedProtocol::Tls
        );
        assert_eq!(
            DetectedProtocol::from(TransportProtocol::Socks5),
            DetectedProtocol::Socks5
        );
        assert_eq!(
            DetectedProtocol::from(TransportProtocol::Socks4),
            DetectedProtocol::Socks4
        );
        for protocol in [
            TransportProtocol::WebSocket,
            TransportProtocol::Sse,
            TransportProtocol::Grpc,
            TransportProtocol::Raw,
        ] {
            assert_eq!(DetectedProtocol::from(protocol), DetectedProtocol::Http);
        }
    }

    #[test]
    fn socks5_udp_detection_rejects_boundaries_and_accepts_ipv6() {
        assert!(!UdpPacketDetector::is_socks5_udp_packet(&[0; 9]));
        assert!(!UdpPacketDetector::is_socks5_udp_packet(&[
            1, 0, 0, 1, 0, 0, 0, 0, 0, 0
        ]));
        assert!(!UdpPacketDetector::is_socks5_udp_packet(&[
            0, 0, 0, 3, 10, b'a', 0, 0, 0, 0
        ]));
        assert!(!UdpPacketDetector::is_socks5_udp_packet(&[
            0, 0, 0, 9, 0, 0, 0, 0, 0, 0
        ]));

        let mut ipv6 = vec![0, 0, 0, 4];
        ipv6.extend_from_slice(&[0; 16]);
        ipv6.extend_from_slice(&[0, 80]);
        assert!(UdpPacketDetector::is_socks5_udp_packet(&ipv6));
    }

    #[tokio::test]
    async fn peekable_stream_detects_and_replays_peeked_bytes() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let client = tokio::spawn(async move {
            let mut stream = TcpStream::connect(addr).await.unwrap();
            stream.write_all(b"GET / HTTP/1.1\r\n").await.unwrap();
            let mut reply = [0; 2];
            stream.read_exact(&mut reply).await.unwrap();
            reply
        });

        let (stream, peer) = listener.accept().await.unwrap();
        let mut peekable = PeekableStream::new(stream);
        assert_eq!(peekable.peer_addr().unwrap(), peer);
        assert_eq!(
            peekable.detect_protocol().await.unwrap(),
            DetectedProtocol::Http
        );
        let mut request = [0; 16];
        peekable.read_exact(&mut request).await.unwrap();
        assert_eq!(&request, b"GET / HTTP/1.1\r\n");
        peekable.write_all(b"OK").await.unwrap();
        peekable.flush().await.unwrap();
        assert_eq!(client.await.unwrap(), *b"OK");
        let _inner = peekable.into_inner();
    }

    #[tokio::test]
    async fn peekable_stream_reports_unknown_on_clean_eof() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let client = tokio::spawn(async move { TcpStream::connect(addr).await.unwrap() });
        let (stream, _) = listener.accept().await.unwrap();
        drop(client.await.unwrap());
        let mut peekable = PeekableStream::new(stream);
        assert_eq!(
            peekable.detect_protocol().await.unwrap(),
            DetectedProtocol::Unknown
        );
        peekable.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn unified_udp_socket_registers_classifies_sends_and_unwraps() {
        let receiver = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let receiver_addr = receiver.local_addr().unwrap();
        let unified = UnifiedUdpSocket::new(receiver);
        assert_eq!(unified.local_addr().unwrap(), receiver_addr);
        assert_eq!(unified.inner().local_addr().unwrap(), receiver_addr);

        let sender = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let sender_addr = sender.local_addr().unwrap();
        unified.register_socks5_client(sender_addr).await;
        unified.register_socks5_client(sender_addr).await;
        let packet = [0, 0, 0, 1, 127, 0, 0, 1, 0, 80, b'x'];
        sender.send_to(&packet, receiver_addr).await.unwrap();

        let mut buf = [0; 64];
        let (n, source, kind) = unified.recv_from_with_type(&mut buf).await.unwrap();
        assert_eq!(source, sender_addr);
        assert_eq!(kind, UdpPacketType::Socks5Relay);
        assert_eq!(&buf[..n], &packet);

        unified.unregister_socks5_client(&sender_addr).await;
        sender.send_to(&packet, receiver_addr).await.unwrap();
        let (_, _, kind) = unified.recv_from_with_type(&mut buf).await.unwrap();
        assert_eq!(kind, UdpPacketType::Unknown);

        unified.send_to(b"pong", sender_addr).await.unwrap();
        let (n, _) = sender.recv_from(&mut buf).await.unwrap();
        assert_eq!(&buf[..n], b"pong");
        assert_eq!(unified.into_inner().local_addr().unwrap(), receiver_addr);
    }
}
