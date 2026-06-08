use std::net::SocketAddr;

use hyper::Request;

use crate::{
    CERT_PUBLIC_PATH_PREFIX, MOBILE_PUBLIC_PATH_PREFIX, PROXY_PUBLIC_PATH_PREFIX,
    TRUST_PROBE_PUBLIC_PATH_PREFIX, TRUST_PROBE_SHORT_PUBLIC_PATH,
};

#[derive(Debug, Clone)]
pub struct AdminSecurityConfig {
    pub listen_port: u16,
    pub allowed_hosts: Vec<String>,
}

impl AdminSecurityConfig {
    pub fn new(listen_port: u16) -> Self {
        let allowed_hosts = vec![
            format!("127.0.0.1:{}", listen_port),
            format!("localhost:{}", listen_port),
            "127.0.0.1".to_string(),
            "localhost".to_string(),
        ];
        Self {
            listen_port,
            allowed_hosts,
        }
    }
}

pub fn is_cert_public_request<T>(req: &Request<T>) -> bool {
    let path = req.uri().path();
    if !path.starts_with(CERT_PUBLIC_PATH_PREFIX)
        && !path.starts_with(MOBILE_PUBLIC_PATH_PREFIX)
        && !path.starts_with(PROXY_PUBLIC_PATH_PREFIX)
        && !path.starts_with(TRUST_PROBE_PUBLIC_PATH_PREFIX)
        && path != TRUST_PROBE_SHORT_PUBLIC_PATH
    {
        return false;
    }

    true
}

pub fn is_valid_admin_request<T>(
    req: &Request<T>,
    peer_addr: SocketAddr,
    config: &AdminSecurityConfig,
    allow_remote: bool,
) -> bool {
    if !allow_remote && !peer_addr.ip().is_loopback() {
        tracing::debug!(
            "Admin request rejected: peer address {} is not loopback",
            peer_addr
        );
        return false;
    }

    if req.uri().scheme().is_some() {
        tracing::debug!(
            "Admin request rejected: URI contains scheme (proxy request): {}",
            req.uri()
        );
        return false;
    }

    let path = req.uri().path();
    if !path.starts_with(crate::ADMIN_PATH_PREFIX) {
        tracing::debug!(
            "Admin request rejected: path {} does not start with {}",
            path,
            crate::ADMIN_PATH_PREFIX
        );
        return false;
    }

    if let Some(host) = req.headers().get(hyper::header::HOST) {
        if let Ok(host_str) = host.to_str() {
            if !allow_remote && !config.allowed_hosts.iter().any(|h| host_str == h) {
                tracing::debug!(
                    "Admin request rejected: host {} not in allowed list {:?}",
                    host_str,
                    config.allowed_hosts
                );
                return false;
            }
        } else {
            tracing::debug!("Admin request rejected: invalid host header encoding");
            return false;
        }
    } else {
        tracing::debug!("Admin request rejected: missing host header");
        return false;
    }

    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use hyper::Request;
    use std::net::{IpAddr, Ipv4Addr};

    fn create_request(uri: &str, host: Option<&str>) -> Request<()> {
        let mut builder = Request::builder().uri(uri);
        if let Some(h) = host {
            builder = builder.header("Host", h);
        }
        builder.body(()).unwrap()
    }

    #[test]
    fn test_valid_admin_request() {
        let config = AdminSecurityConfig::new(9900);
        let peer_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 12345);
        let req = create_request("/_bifrost/api/rules", Some("127.0.0.1:9900"));

        assert!(is_valid_admin_request(&req, peer_addr, &config, false));
    }

    #[test]
    fn test_reject_non_loopback() {
        let config = AdminSecurityConfig::new(9900);
        let peer_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1)), 12345);
        let req = create_request("/_bifrost/api/rules", Some("127.0.0.1:9900"));

        assert!(!is_valid_admin_request(&req, peer_addr, &config, false));
        assert!(is_valid_admin_request(&req, peer_addr, &config, true));
    }

    #[test]
    fn test_reject_proxy_request() {
        let config = AdminSecurityConfig::new(9900);
        let peer_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 12345);
        let req = create_request(
            "http://127.0.0.1:9900/_bifrost/api/rules",
            Some("127.0.0.1:9900"),
        );

        assert!(!is_valid_admin_request(&req, peer_addr, &config, false));
        assert!(!is_valid_admin_request(&req, peer_addr, &config, true));
    }

    #[test]
    fn test_accept_public_cert_request_with_absolute_uri() {
        let req = create_request(
            "http://127.0.0.1:9900/_bifrost/public/cert",
            Some("127.0.0.1:9900"),
        );

        assert!(is_cert_public_request(&req));
    }

    #[test]
    fn test_accept_public_mobile_profile_request() {
        let req = create_request(
            "http://127.0.0.1:9900/_bifrost/public/mobile/ios-profile.mobileconfig",
            Some("127.0.0.1:9900"),
        );

        assert!(is_cert_public_request(&req));
    }

    #[test]
    fn test_accept_public_proxy_qrcode_request() {
        let req = create_request(
            "http://127.0.0.1:9900/_bifrost/public/proxy/qrcode?ip=192.168.1.2",
            Some("127.0.0.1:9900"),
        );

        assert!(is_cert_public_request(&req));
    }

    #[test]
    fn test_accept_public_trust_probe_request() {
        let req = create_request(
            "http://127.0.0.1:9900/_bifrost/public/trust-probe/00000000-0000-0000-0000-000000000000?t=token",
            Some("127.0.0.1:9900"),
        );

        assert!(is_cert_public_request(&req));
    }

    #[test]
    fn test_accept_short_public_trust_probe_request() {
        let req = create_request("http://127.0.0.1:9900/_bifrost/tp", Some("127.0.0.1:9900"));

        assert!(is_cert_public_request(&req));
    }

    #[test]
    fn test_reject_wrong_host() {
        let config = AdminSecurityConfig::new(9900);
        let peer_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 12345);
        let req = create_request("/_bifrost/api/rules", Some("evil.com:9900"));

        assert!(!is_valid_admin_request(&req, peer_addr, &config, false));
        assert!(is_valid_admin_request(&req, peer_addr, &config, true));
    }

    #[test]
    fn test_reject_missing_host() {
        let config = AdminSecurityConfig::new(9900);
        let peer_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 12345);
        let req = create_request("/_bifrost/api/rules", None);

        assert!(!is_valid_admin_request(&req, peer_addr, &config, false));
        assert!(!is_valid_admin_request(&req, peer_addr, &config, true));
    }

    #[test]
    fn test_accept_localhost_host() {
        let config = AdminSecurityConfig::new(9900);
        let peer_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 12345);
        let req = create_request("/_bifrost/api/rules", Some("localhost:9900"));

        assert!(is_valid_admin_request(&req, peer_addr, &config, false));
    }

    #[test]
    fn test_reject_wrong_path() {
        let config = AdminSecurityConfig::new(9900);
        let peer_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 12345);
        let req = create_request("/api/rules", Some("127.0.0.1:9900"));

        assert!(!is_valid_admin_request(&req, peer_addr, &config, false));
        assert!(!is_valid_admin_request(&req, peer_addr, &config, true));
    }
}
