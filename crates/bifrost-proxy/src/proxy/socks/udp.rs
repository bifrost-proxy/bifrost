use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use bifrost_admin::{AdminState, TrafficRecord, TrafficType};
use bifrost_core::{AccessDecision, BifrostError, ClientAccessControl, Result};
use tokio::net::UdpSocket;
use tokio::sync::{mpsc, RwLock};
use tracing::{debug, error, info};

use crate::dns::DnsResolver;
use crate::protocol::QuicPacketDetector;
use crate::server::{ProxyConfig, RulesResolver};

use super::tcp::{AddressType, SocksAddress};

const UDP_BUFFER_SIZE: usize = 65535;
const SESSION_TIMEOUT: Duration = Duration::from_secs(300);
const CLEANUP_INTERVAL: Duration = Duration::from_secs(60);

#[derive(Debug, Clone)]
pub struct UdpSession {
    pub client_addr: SocketAddr,
    pub relay_socket: Arc<UdpSocket>,
    pub last_activity: Instant,
    pub bytes_sent: Arc<AtomicU64>,
    pub bytes_received: Arc<AtomicU64>,
    pub packet_count: Arc<AtomicU64>,
    pub req_id: String,
}

static UDP_SESSION_COUNTER: AtomicU64 = AtomicU64::new(1);

fn generate_udp_session_id() -> String {
    let id = UDP_SESSION_COUNTER.fetch_add(1, Ordering::SeqCst);
    format!("UDP-{:06}", id)
}

pub struct UdpRelay {
    bind_addr: SocketAddr,
    sessions: Arc<RwLock<HashMap<SocketAddr, UdpSession>>>,
    shutdown_tx: Option<mpsc::Sender<()>>,
    rules: Option<Arc<dyn RulesResolver>>,
    proxy_config: Option<ProxyConfig>,
    admin_state: Option<Arc<AdminState>>,
    dns_resolver: Option<Arc<DnsResolver>>,
    access_control: Option<Arc<RwLock<ClientAccessControl>>>,
}

impl UdpRelay {
    pub fn new(bind_addr: SocketAddr) -> Self {
        Self {
            bind_addr,
            sessions: Arc::new(RwLock::new(HashMap::new())),
            shutdown_tx: None,
            rules: None,
            proxy_config: None,
            admin_state: None,
            dns_resolver: None,
            access_control: None,
        }
    }

    pub fn with_access_control(mut self, access_control: Arc<RwLock<ClientAccessControl>>) -> Self {
        self.access_control = Some(access_control);
        self
    }

    pub fn with_rules(mut self, rules: Arc<dyn RulesResolver>) -> Self {
        self.rules = Some(rules);
        self
    }

    pub fn with_proxy_config(mut self, proxy_config: ProxyConfig) -> Self {
        self.proxy_config = Some(proxy_config);
        self
    }

    #[allow(dead_code)]
    pub fn with_admin_state(mut self, admin_state: Option<Arc<AdminState>>) -> Self {
        self.admin_state = admin_state;
        self
    }

    #[allow(dead_code)]
    pub fn with_dns_resolver(mut self, dns_resolver: Arc<DnsResolver>) -> Self {
        self.dns_resolver = Some(dns_resolver);
        self
    }

    pub async fn start(&mut self) -> Result<SocketAddr> {
        let socket = UdpSocket::bind(self.bind_addr).await.map_err(|e| {
            BifrostError::Network(format!(
                "Failed to bind UDP relay on {}: {}",
                self.bind_addr, e
            ))
        })?;

        let local_addr = socket.local_addr().map_err(|e| {
            BifrostError::Network(format!("Failed to get UDP relay local address: {}", e))
        })?;

        info!("SOCKS5 UDP relay listening on {}", local_addr);
        let listener_port = local_addr.port();

        let socket = Arc::new(socket);
        let sessions = Arc::clone(&self.sessions);

        let (shutdown_tx, mut shutdown_rx) = mpsc::channel::<()>(1);
        self.shutdown_tx = Some(shutdown_tx);

        let relay_socket = Arc::clone(&socket);
        let relay_sessions = Arc::clone(&sessions);
        let rules = self.rules.clone();
        let dns_resolver = self.dns_resolver.clone();
        let admin_state = self.admin_state.clone();
        let access_control = self.access_control.clone();
        let verbose = self
            .proxy_config
            .as_ref()
            .map(|c| c.verbose_logging)
            .unwrap_or(false);

        tokio::spawn(async move {
            let mut buf = vec![0u8; UDP_BUFFER_SIZE];

            loop {
                tokio::select! {
                    result = relay_socket.recv_from(&mut buf) => {
                        match result {
                            Ok((len, src_addr)) => {
                                if let Err(e) = Self::handle_packet(
                                    &relay_socket,
                                    &relay_sessions,
                                    &buf[..len],
                                    src_addr,
                                    &rules,
                                    &dns_resolver,
                                    &admin_state,
                                    &access_control,
                                    verbose,
                                    listener_port,
                                ).await {
                                    debug!("UDP relay packet error from {}: {}", src_addr, e);
                                }
                            }
                            Err(e) => {
                                error!("UDP relay recv error: {}", e);
                            }
                        }
                    }
                    _ = shutdown_rx.recv() => {
                        info!("UDP relay shutting down");
                        break;
                    }
                }
            }
        });

        let cleanup_sessions = Arc::clone(&sessions);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(CLEANUP_INTERVAL).await;
                Self::cleanup_sessions(&cleanup_sessions).await;
            }
        });

        Ok(local_addr)
    }

    #[allow(clippy::too_many_arguments)]
    async fn handle_packet(
        relay_socket: &Arc<UdpSocket>,
        sessions: &Arc<RwLock<HashMap<SocketAddr, UdpSession>>>,
        data: &[u8],
        src_addr: SocketAddr,
        rules: &Option<Arc<dyn RulesResolver>>,
        dns_resolver: &Option<Arc<DnsResolver>>,
        admin_state: &Option<Arc<AdminState>>,
        access_control: &Option<Arc<RwLock<ClientAccessControl>>>,
        verbose: bool,
        listener_port: u16,
    ) -> Result<()> {
        if let Some(ref ac) = access_control {
            let decision = {
                let access_control = ac.read().await;
                access_control.check_access(&src_addr.ip())
            };

            match decision {
                AccessDecision::Allow => {}
                AccessDecision::Deny => {
                    debug!(
                        "SOCKS5 UDP: Access denied for client {} (not in whitelist)",
                        src_addr.ip()
                    );
                    return Ok(());
                }
                AccessDecision::Prompt(ip) => {
                    {
                        let ac = access_control.as_ref().unwrap().read().await;
                        ac.add_pending_authorization(ip);
                    }
                    debug!(
                        "SOCKS5 UDP: Access pending approval for client {}",
                        src_addr.ip()
                    );
                    return Ok(());
                }
            }
        }

        if data.len() < 10 {
            return Err(BifrostError::Parse("UDP packet too short".to_string()));
        }

        let rsv = u16::from_be_bytes([data[0], data[1]]);
        let frag = data[2];
        let atyp = data[3];

        if rsv != 0 {
            return Err(BifrostError::Parse("Invalid RSV field".to_string()));
        }

        if frag != 0 {
            debug!("UDP fragmentation not supported, dropping packet");
            return Ok(());
        }

        let (dest_addr, dest_port, payload_offset) = Self::parse_address(atyp, &data[4..])?;

        let payload = &data[4 + payload_offset..];

        let is_quic = Self::is_quic_packet(payload);

        debug!(
            "UDP relay: {} -> {:?}:{} ({} bytes, quic={})",
            src_addr,
            dest_addr,
            dest_port,
            payload.len(),
            is_quic
        );

        let (final_host, final_port, dns_servers) =
            Self::apply_rules(&dest_addr, dest_port, rules, is_quic, verbose);

        let target_addr = match &final_host {
            SocksAddress::IPv4(ip) => SocketAddr::new((*ip).into(), final_port),
            SocksAddress::IPv6(ip) => SocketAddr::new((*ip).into(), final_port),
            SocksAddress::DomainName(domain) => {
                if let Some(resolver) = dns_resolver {
                    if !dns_servers.is_empty() {
                        if verbose {
                            info!(
                                "UDP relay: [DNS] resolving {} with custom servers: {:?}",
                                domain, dns_servers
                            );
                        }
                        match resolver.resolve(domain, &dns_servers).await {
                            Ok(Some(ip)) => {
                                if verbose {
                                    info!("UDP relay: [DNS] resolved {} -> {}", domain, ip);
                                }
                                SocketAddr::new(ip, final_port)
                            }
                            Ok(None) | Err(_) => {
                                tokio::net::lookup_host(format!("{}:{}", domain, final_port))
                                    .await
                                    .map_err(|e| {
                                        BifrostError::Network(format!("DNS lookup failed: {}", e))
                                    })?
                                    .next()
                                    .ok_or_else(|| {
                                        BifrostError::Network("No address resolved".to_string())
                                    })?
                            }
                        }
                    } else {
                        match resolver.resolve(domain, &[]).await {
                            Ok(Some(ip)) => SocketAddr::new(ip, final_port),
                            Ok(None) | Err(_) => {
                                tokio::net::lookup_host(format!("{}:{}", domain, final_port))
                                    .await
                                    .map_err(|e| {
                                        BifrostError::Network(format!("DNS lookup failed: {}", e))
                                    })?
                                    .next()
                                    .ok_or_else(|| {
                                        BifrostError::Network("No address resolved".to_string())
                                    })?
                            }
                        }
                    }
                } else {
                    tokio::net::lookup_host(format!("{}:{}", domain, final_port))
                        .await
                        .map_err(|e| BifrostError::Network(format!("DNS lookup failed: {}", e)))?
                        .next()
                        .ok_or_else(|| BifrostError::Network("No address resolved".to_string()))?
                }
            }
        };

        let session = {
            let sessions_read = sessions.read().await;
            sessions_read.get(&src_addr).cloned()
        };

        let payload_len = payload.len() as u64;

        let (relay_socket_for_target, session_bytes_sent, _session_bytes_received, req_id) =
            if let Some(mut session) = session {
                session.last_activity = Instant::now();
                session.packet_count.fetch_add(1, Ordering::Relaxed);
                let bytes_sent = Arc::clone(&session.bytes_sent);
                let bytes_received = Arc::clone(&session.bytes_received);
                let req_id = session.req_id.clone();
                {
                    let mut sessions_write = sessions.write().await;
                    sessions_write.insert(src_addr, session.clone());
                }
                (session.relay_socket, bytes_sent, bytes_received, req_id)
            } else {
                let new_socket = UdpSocket::bind("0.0.0.0:0").await.map_err(|e| {
                    BifrostError::Network(format!("Failed to create relay socket: {}", e))
                })?;

                let new_socket = Arc::new(new_socket);
                let bytes_sent = Arc::new(AtomicU64::new(0));
                let bytes_received = Arc::new(AtomicU64::new(0));
                let packet_count = Arc::new(AtomicU64::new(1));
                let req_id = generate_udp_session_id();

                let session = UdpSession {
                    client_addr: src_addr,
                    relay_socket: Arc::clone(&new_socket),
                    last_activity: Instant::now(),
                    bytes_sent: Arc::clone(&bytes_sent),
                    bytes_received: Arc::clone(&bytes_received),
                    packet_count: Arc::clone(&packet_count),
                    req_id: req_id.clone(),
                };

                {
                    let mut sessions_write = sessions.write().await;
                    sessions_write.insert(src_addr, session);
                }

                if let Some(ref state) = admin_state {
                    state
                        .metrics_collector
                        .increment_connections_by_type(TrafficType::Socks5);

                    let host_str = match &dest_addr {
                        SocksAddress::IPv4(ip) => ip.to_string(),
                        SocksAddress::IPv6(ip) => ip.to_string(),
                        SocksAddress::DomainName(domain) => domain.clone(),
                    };

                    let mut record = TrafficRecord::new(
                        req_id.clone(),
                        "UDP".to_string(),
                        format!("udp://{}:{}", host_str, dest_port),
                    );
                    record.status = 200;
                    record.protocol = "socks5-udp".to_string();
                    record.host = host_str.clone();
                    record.is_tunnel = true;
                    record.client_ip = src_addr.ip().to_string();
                    record.listener_port = listener_port;

                    if let Some(ref rules) = rules {
                        let scheme = if is_quic || dest_port == 443 {
                            "https"
                        } else {
                            "http"
                        };
                        let url = format!("{}://{}:{}/", scheme, host_str, dest_port);
                        let resolved_rules = rules.resolve(&url, "GET");
                        record.has_rule_hit = !resolved_rules.rules.is_empty()
                            || resolved_rules.host.is_some()
                            || resolved_rules.proxy.is_some();
                        record.matched_rules = crate::utils::build_matched_rules(&resolved_rules);
                    }

                    state.record_traffic(record);

                    info!(
                        "[{}] SOCKS5 UDP session created for {} -> {}:{}",
                        req_id,
                        src_addr,
                        target_addr.ip(),
                        target_addr.port()
                    );
                }

                let response_socket = Arc::clone(&new_socket);
                let main_relay = Arc::clone(relay_socket);
                let client = src_addr;
                let session_bytes_received_clone = Arc::clone(&bytes_received);
                let admin_state_clone = admin_state.clone();
                let req_id_clone = req_id.clone();

                tokio::spawn(async move {
                    let mut buf = vec![0u8; UDP_BUFFER_SIZE];
                    loop {
                        match tokio::time::timeout(
                            SESSION_TIMEOUT,
                            response_socket.recv_from(&mut buf),
                        )
                        .await
                        {
                            Ok(Ok((len, remote_addr))) => {
                                session_bytes_received_clone
                                    .fetch_add(len as u64, Ordering::Relaxed);

                                let response = Self::build_udp_response(&remote_addr, &buf[..len]);

                                if let Err(e) = main_relay.send_to(&response, client).await {
                                    debug!("Failed to send UDP response to client: {}", e);
                                    break;
                                }

                                if let Some(ref state) = admin_state_clone {
                                    state.metrics_collector.add_bytes_received_by_type(
                                        TrafficType::Socks5,
                                        len as u64,
                                    );
                                }
                            }
                            Ok(Err(e)) => {
                                debug!("UDP session recv error: {}", e);
                                break;
                            }
                            Err(_) => {
                                debug!("UDP session timeout for client {}", client);
                                break;
                            }
                        }
                    }

                    if let Some(ref state) = admin_state_clone {
                        state
                            .metrics_collector
                            .decrement_connections_by_type(TrafficType::Socks5);

                        let total_received = session_bytes_received_clone.load(Ordering::Relaxed);
                        state.update_traffic_by_id(&req_id_clone, move |record| {
                            record.response_size = total_received as usize;
                            record.download_bytes = total_received as usize;
                        });

                        debug!(
                            "[{}] SOCKS5 UDP session closed, received={} bytes",
                            req_id_clone, total_received
                        );
                    }
                });

                (new_socket, bytes_sent, bytes_received, req_id)
            };

        session_bytes_sent.fetch_add(payload_len, Ordering::Relaxed);

        if let Some(ref state) = admin_state {
            state
                .metrics_collector
                .add_bytes_sent_by_type(TrafficType::Socks5, payload_len);
            state
                .metrics_collector
                .increment_requests_by_type(TrafficType::Socks5);

            let total_sent = session_bytes_sent.load(Ordering::Relaxed);
            state.update_traffic_by_id(&req_id, move |record| {
                record.request_size = total_sent as usize;
                record.upload_bytes = total_sent as usize;
            });
        }

        relay_socket_for_target
            .send_to(payload, target_addr)
            .await
            .map_err(|e| BifrostError::Network(format!("Failed to send UDP packet: {}", e)))?;

        Ok(())
    }

    fn parse_address(atyp: u8, data: &[u8]) -> Result<(SocksAddress, u16, usize)> {
        SocksAddress::parse_from_bytes(atyp, data)
    }

    fn is_quic_packet(data: &[u8]) -> bool {
        QuicPacketDetector::is_quic_packet(data)
    }

    fn apply_rules(
        dest_addr: &SocksAddress,
        dest_port: u16,
        rules: &Option<Arc<dyn RulesResolver>>,
        is_quic: bool,
        verbose: bool,
    ) -> (SocksAddress, u16, Vec<String>) {
        let Some(rules) = rules else {
            return (dest_addr.clone(), dest_port, vec![]);
        };

        let host_str = match dest_addr {
            SocksAddress::IPv4(ip) => ip.to_string(),
            SocksAddress::IPv6(ip) => ip.to_string(),
            SocksAddress::DomainName(domain) => domain.clone(),
        };

        let scheme = if is_quic || dest_port == 443 {
            "https"
        } else {
            "http"
        };
        let url = format!("{}://{}:{}/", scheme, host_str, dest_port);

        let resolved = rules.resolve(&url, "GET");

        let dns_servers = resolved.dns_servers.clone();

        if let Some(ref host_rule) = resolved.host {
            let parts: Vec<&str> = host_rule.split(':').collect();
            let new_host = parts[0].to_string();
            let new_port = if parts.len() > 1 {
                parts[1].parse().unwrap_or(dest_port)
            } else {
                dest_port
            };

            if verbose {
                info!(
                    "UDP relay: host rule applied - {}:{} -> {}:{}",
                    host_str, dest_port, new_host, new_port
                );
            }

            if let Ok(ipv4) = new_host.parse::<std::net::Ipv4Addr>() {
                return (SocksAddress::IPv4(ipv4), new_port, dns_servers);
            }
            if let Ok(ipv6) = new_host.parse::<std::net::Ipv6Addr>() {
                return (SocksAddress::IPv6(ipv6), new_port, dns_servers);
            }
            return (SocksAddress::DomainName(new_host), new_port, dns_servers);
        }

        (dest_addr.clone(), dest_port, dns_servers)
    }

    fn build_udp_response(remote_addr: &SocketAddr, payload: &[u8]) -> Vec<u8> {
        let mut response = vec![0u8, 0u8, 0u8];

        match remote_addr {
            SocketAddr::V4(addr) => {
                response.push(AddressType::IPv4 as u8);
                response.extend_from_slice(&addr.ip().octets());
                response.extend_from_slice(&addr.port().to_be_bytes());
            }
            SocketAddr::V6(addr) => {
                response.push(AddressType::IPv6 as u8);
                response.extend_from_slice(&addr.ip().octets());
                response.extend_from_slice(&addr.port().to_be_bytes());
            }
        }

        response.extend_from_slice(payload);
        response
    }

    async fn cleanup_sessions(sessions: &Arc<RwLock<HashMap<SocketAddr, UdpSession>>>) {
        let now = Instant::now();
        let mut sessions_write = sessions.write().await;
        let before = sessions_write.len();

        sessions_write
            .retain(|_, session| now.duration_since(session.last_activity) < SESSION_TIMEOUT);

        let after = sessions_write.len();
        if before != after {
            debug!("UDP relay: cleaned up {} expired sessions", before - after);
        }
    }

    #[allow(dead_code)]
    pub async fn shutdown(&mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(()).await;
        }
    }
}

impl Drop for UdpRelay {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.try_send(());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::ResolvedRules;
    use tokio::time::timeout;

    #[derive(Clone)]
    struct StaticRules(ResolvedRules);

    impl RulesResolver for StaticRules {
        fn resolve_with_context(
            &self,
            _url: &str,
            _method: &str,
            _req_headers: &HashMap<String, String>,
            _req_cookies: &HashMap<String, String>,
        ) -> ResolvedRules {
            self.0.clone()
        }
    }

    #[test]
    fn test_parse_ipv4_address() {
        let data = [192, 168, 1, 1, 0x1F, 0x90, 0x00];
        let (addr, port, offset) = UdpRelay::parse_address(0x01, &data).unwrap();
        assert!(matches!(addr, SocksAddress::IPv4(_)));
        assert_eq!(port, 8080);
        assert_eq!(offset, 6);
    }

    #[test]
    fn test_parse_domain_address() {
        let mut data = vec![11u8];
        data.extend_from_slice(b"example.com");
        data.extend_from_slice(&[0x01, 0xBB]);

        let (addr, port, offset) = UdpRelay::parse_address(0x03, &data).unwrap();
        assert!(matches!(addr, SocksAddress::DomainName(ref d) if d == "example.com"));
        assert_eq!(port, 443);
        assert_eq!(offset, 14);
    }

    #[test]
    fn test_parse_ipv6_address() {
        let mut data = vec![0u8; 16];
        data.extend_from_slice(&[0x1F, 0x90]); // 8080
        let (addr, port, offset) = UdpRelay::parse_address(0x04, &data).unwrap();
        assert!(matches!(addr, SocksAddress::IPv6(_)));
        assert_eq!(port, 8080);
        assert_eq!(offset, 18);
    }

    #[test]
    fn test_parse_address_error_paths() {
        let err = UdpRelay::parse_address(0x01, &[192, 168, 1])
            .expect_err("expected IPv4 too short error");
        assert!(matches!(err, BifrostError::Parse(msg) if msg.contains("IPv4 address too short")));

        let err =
            UdpRelay::parse_address(0x03, &[]).expect_err("expected domain length missing error");
        assert!(
            matches!(err, BifrostError::Parse(msg) if msg.contains("Domain name length missing"))
        );

        let err = UdpRelay::parse_address(0x03, &[5, b'a', b'b'])
            .expect_err("expected domain too short error");
        assert!(matches!(err, BifrostError::Parse(msg) if msg.contains("Domain name too short")));

        let err =
            UdpRelay::parse_address(0x04, &[0; 10]).expect_err("expected IPv6 too short error");
        assert!(matches!(err, BifrostError::Parse(msg) if msg.contains("IPv6 address too short")));

        let err = UdpRelay::parse_address(0xFF, &[0; 4]).expect_err("expected invalid type error");
        assert!(matches!(err, BifrostError::Parse(msg) if msg.contains("Invalid address type")));
    }

    #[test]
    fn test_build_udp_response_ipv4() {
        let addr: SocketAddr = "192.168.1.1:8080".parse().unwrap();
        let payload = b"test";
        let response = UdpRelay::build_udp_response(&addr, payload);

        assert_eq!(response[0], 0);
        assert_eq!(response[1], 0);
        assert_eq!(response[2], 0);
        assert_eq!(response[3], 0x01);
        assert_eq!(&response[4..8], &[192, 168, 1, 1]);
        assert_eq!(&response[8..10], &[0x1F, 0x90]);
        assert_eq!(&response[10..], b"test");
    }

    #[test]
    fn test_build_udp_response_ipv6() {
        let addr: SocketAddr = "[2001:db8::1]:8080".parse().unwrap();
        let payload = b"quic";
        let response = UdpRelay::build_udp_response(&addr, payload);

        assert_eq!(response[0], 0);
        assert_eq!(response[1], 0);
        assert_eq!(response[2], 0);
        assert_eq!(response[3], AddressType::IPv6 as u8);
        let expected_ip = match addr.ip() {
            std::net::IpAddr::V6(ip) => ip.octets(),
            _ => panic!("expected IPv6 address"),
        };
        assert_eq!(&response[4..20], &expected_ip);
        assert_eq!(&response[20..22], &addr.port().to_be_bytes());
        assert_eq!(&response[22..], b"quic");
    }

    #[tokio::test]
    async fn udp_relay_forwards_roundtrip_reuses_session_and_shuts_down() {
        let echo = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let echo_addr = echo.local_addr().unwrap();
        let echo_task = tokio::spawn(async move {
            let mut buf = [0_u8; 64];
            for _ in 0..2 {
                let (n, peer) = echo.recv_from(&mut buf).await.unwrap();
                echo.send_to(&buf[..n], peer).await.unwrap();
            }
        });

        let mut relay = UdpRelay::new("127.0.0.1:0".parse().unwrap())
            .with_proxy_config(ProxyConfig {
                verbose_logging: true,
                ..Default::default()
            })
            .with_admin_state(None)
            .with_dns_resolver(Arc::new(DnsResolver::new(false)));
        let relay_addr = relay.start().await.unwrap();
        let client = UdpSocket::bind("127.0.0.1:0").await.unwrap();

        for payload in [b"one".as_slice(), b"two".as_slice()] {
            let mut packet = vec![0, 0, 0, AddressType::IPv4 as u8];
            packet.extend_from_slice(&[127, 0, 0, 1]);
            packet.extend_from_slice(&echo_addr.port().to_be_bytes());
            packet.extend_from_slice(payload);
            client.send_to(&packet, relay_addr).await.unwrap();
            let mut response = [0_u8; 64];
            let (n, source) = timeout(Duration::from_secs(2), client.recv_from(&mut response))
                .await
                .unwrap()
                .unwrap();
            assert_eq!(source, relay_addr);
            assert_eq!(&response[10..n], payload);
        }

        let sessions = relay.sessions.read().await;
        let session = sessions.values().next().unwrap();
        assert_eq!(session.packet_count.load(Ordering::Relaxed), 2);
        assert_eq!(session.bytes_sent.load(Ordering::Relaxed), 6);
        drop(sessions);
        relay.shutdown().await;
        echo_task.await.unwrap();
    }

    #[tokio::test]
    async fn udp_relay_handles_invalid_packets_and_cleans_expired_sessions() {
        let relay_socket = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
        let sessions = Arc::new(RwLock::new(HashMap::new()));
        let source: SocketAddr = "127.0.0.1:30001".parse().unwrap();

        let error = UdpRelay::handle_packet(
            &relay_socket,
            &sessions,
            &[0, 0, 0],
            source,
            &None,
            &None,
            &None,
            &None,
            false,
            0,
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("too short"));

        let mut bad_rsv = vec![0, 1, 0, AddressType::IPv4 as u8];
        bad_rsv.extend_from_slice(&[127, 0, 0, 1, 0, 80]);
        assert!(UdpRelay::handle_packet(
            &relay_socket,
            &sessions,
            &bad_rsv,
            source,
            &None,
            &None,
            &None,
            &None,
            false,
            0,
        )
        .await
        .is_err());
        let mut fragmented = bad_rsv;
        fragmented[0] = 0;
        fragmented[1] = 0;
        fragmented[2] = 1;
        UdpRelay::handle_packet(
            &relay_socket,
            &sessions,
            &fragmented,
            source,
            &None,
            &None,
            &None,
            &None,
            false,
            0,
        )
        .await
        .unwrap();

        let session_socket = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
        sessions.write().await.insert(
            source,
            UdpSession {
                client_addr: source,
                relay_socket: session_socket,
                last_activity: Instant::now() - SESSION_TIMEOUT - Duration::from_secs(1),
                bytes_sent: Arc::new(AtomicU64::new(0)),
                bytes_received: Arc::new(AtomicU64::new(0)),
                packet_count: Arc::new(AtomicU64::new(0)),
                req_id: generate_udp_session_id(),
            },
        );
        UdpRelay::cleanup_sessions(&sessions).await;
        assert!(sessions.read().await.is_empty());
    }

    #[tokio::test]
    async fn coverage_90_domain_roundtrip_records_admin_traffic_and_applies_rules() {
        let echo = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let echo_addr = echo.local_addr().unwrap();
        let echo_task = tokio::spawn(async move {
            let mut buf = [0_u8; 64];
            for _ in 0..3 {
                let (n, peer) = echo.recv_from(&mut buf).await.unwrap();
                echo.send_to(&buf[..n], peer).await.unwrap();
            }
        });

        let main_relay = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
        let client = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let source = client.local_addr().unwrap();
        let sessions = Arc::new(RwLock::new(HashMap::new()));
        let harness = bifrost_admin::test_support::TestAdminState::builder()
            .port(19510)
            .build();
        let rules: Option<Arc<dyn RulesResolver>> = Some(Arc::new(StaticRules(ResolvedRules {
            host: Some(format!("127.0.0.1:{}", echo_addr.port())),
            dns_servers: vec!["1.1.1.1".to_string()],
            ..Default::default()
        })));

        let domain = b"localhost";
        let mut packet = vec![0, 0, 0, AddressType::DomainName as u8, domain.len() as u8];
        packet.extend_from_slice(domain);
        packet.extend_from_slice(&443_u16.to_be_bytes());
        packet.extend_from_slice(b"domain-payload");
        UdpRelay::handle_packet(
            &main_relay,
            &sessions,
            &packet,
            source,
            &rules,
            &Some(Arc::new(DnsResolver::new(false))),
            &Some(harness.state()),
            &None,
            true,
            19510,
        )
        .await
        .unwrap();

        let mut response = [0_u8; 128];
        let (n, _) = timeout(Duration::from_secs(2), client.recv_from(&mut response))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(&response[10..n], b"domain-payload");
        let session = sessions.read().await.values().next().unwrap().clone();
        assert_eq!(session.bytes_sent.load(Ordering::Relaxed), 14);
        let record = harness
            .traffic_db
            .get_by_id(&session.req_id)
            .expect("UDP traffic record");
        assert_eq!(record.protocol, "socks5-udp");
        assert_eq!(record.listener_port, 19510);

        for resolver in [Some(Arc::new(DnsResolver::new(false))), None] {
            let direct_domain = b"127.0.0.1";
            let mut direct = vec![
                0,
                0,
                0,
                AddressType::DomainName as u8,
                direct_domain.len() as u8,
            ];
            direct.extend_from_slice(direct_domain);
            direct.extend_from_slice(&echo_addr.port().to_be_bytes());
            direct.extend_from_slice(b"domain-payload");
            UdpRelay::handle_packet(
                &main_relay,
                &sessions,
                &direct,
                source,
                &None,
                &resolver,
                &None,
                &None,
                true,
                19510,
            )
            .await
            .unwrap();
            let (n, _) = timeout(Duration::from_secs(2), client.recv_from(&mut response))
                .await
                .unwrap()
                .unwrap();
            assert_eq!(&response[10..n], b"domain-payload");
        }
        echo_task.await.unwrap();

        let domain_rule: Option<Arc<dyn RulesResolver>> =
            Some(Arc::new(StaticRules(ResolvedRules {
                host: Some("replacement.test:8443".to_string()),
                ..Default::default()
            })));
        let (host, port, _) = UdpRelay::apply_rules(
            &SocksAddress::DomainName("source.test".to_string()),
            80,
            &domain_rule,
            false,
            true,
        );
        assert!(matches!(host, SocksAddress::DomainName(ref value) if value == "replacement.test"));
        assert_eq!(port, 8443);
    }
}
