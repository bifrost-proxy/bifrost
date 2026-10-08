use std::net::IpAddr;

use bifrost_admin::ADMIN_PATH_PREFIX;
use hyper::Request;

use super::ADMIN_VIRTUAL_HOST;

pub(super) fn is_admin_virtual_host_request<B>(req: &Request<B>) -> bool {
    if req.method() == hyper::Method::CONNECT {
        return false;
    }

    if let Some(uri_host) = req.uri().host() {
        if uri_host.eq_ignore_ascii_case(ADMIN_VIRTUAL_HOST) {
            return true;
        }
    }

    if let Some(host_val) = req.headers().get("host").and_then(|h| h.to_str().ok()) {
        let host_without_port = host_val.split(':').next().unwrap_or(host_val);
        if host_without_port.eq_ignore_ascii_case(ADMIN_VIRTUAL_HOST) {
            return true;
        }
    }

    false
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct AdminRoutingDecision {
    pub(super) is_admin_virtual_host: bool,
    pub(super) is_proxy_request_to_other_server: bool,
    pub(super) routes_to_admin: bool,
}

pub(super) fn admin_routing_decision<B>(
    req: &Request<B>,
    self_port: u16,
    self_host: &str,
) -> AdminRoutingDecision {
    let is_admin_virtual_host = is_admin_virtual_host_request(req);
    let is_proxy_request_to_other_server =
        is_proxy_request_to_other_for_admin_routing(req, self_port, self_host);
    let path = req.uri().path();
    let routes_to_admin = is_devtools_bridge_admin_path(path)
        || (!is_proxy_request_to_other_server
            && (path.starts_with(ADMIN_PATH_PREFIX) || is_admin_virtual_host));

    AdminRoutingDecision {
        is_admin_virtual_host,
        is_proxy_request_to_other_server,
        routes_to_admin,
    }
}

pub(super) fn is_proxy_request_to_other_for_admin_routing<B>(
    req: &Request<B>,
    self_port: u16,
    self_host: &str,
) -> bool {
    if is_admin_virtual_host_request(req) {
        return false;
    }
    is_proxy_request_targeting_other(req, self_port, self_host)
}

pub(super) fn is_proxy_request_targeting_other<B>(
    req: &Request<B>,
    self_port: u16,
    self_host: &str,
) -> bool {
    proxy_request_targets_other(req, self_port, self_host, || {
        local_ip_address::list_afinet_netifas()
            .ok()
            .map(|interfaces| interfaces.into_iter().map(|(_, ip)| ip).collect())
    })
}

fn proxy_request_targets_other<B>(
    req: &Request<B>,
    self_port: u16,
    self_host: &str,
    local_ips: impl FnOnce() -> Option<Vec<IpAddr>>,
) -> bool {
    let uri = req.uri();
    if uri.scheme().is_none() && uri.host().is_none() {
        return false;
    }

    let target_host = match uri.host() {
        Some(h) => h,
        None => return false,
    };
    let target_port = uri.port_u16().unwrap_or(80);

    if target_port != self_port {
        return true;
    }

    let is_loopback_target =
        target_host == "127.0.0.1" || target_host == "localhost" || target_host == "[::1]";
    let self_is_loopback =
        self_host == "127.0.0.1" || self_host == "localhost" || self_host == "[::1]";
    let self_is_wildcard = self_host == "0.0.0.0" || self_host == "[::]";

    if target_host == self_host {
        return false;
    }
    if is_loopback_target && (self_is_loopback || self_is_wildcard) {
        return false;
    }
    if self_is_wildcard {
        // A wildcard bind does not make every same-port destination local.
        // Only classify literal addresses absent from this machine as remote.
        // Keep ambiguous hostnames and interface enumeration failures on the
        // existing admin validation path, rather than weakening its protection.
        let Ok(target_ip) = target_host.trim_matches(['[', ']']).parse::<IpAddr>() else {
            return false;
        };
        let target_ip = target_ip.to_canonical();
        if target_ip.is_loopback() || target_ip.is_unspecified() {
            return false;
        }
        return local_ips()
            .is_some_and(|ips| !ips.into_iter().any(|ip| ip.to_canonical() == target_ip));
    }

    true
}

pub(super) fn is_devtools_bridge_admin_path(path: &str) -> bool {
    path.strip_prefix(ADMIN_PATH_PREFIX)
        .is_some_and(|rest| rest.starts_with("/api/devtools/bridge/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(target: &str, port: u16) -> Request<()> {
        Request::builder()
            .uri(format!("http://{target}:{port}/_bifrost/api/rules"))
            .header("host", format!("{target}:{port}"))
            .body(())
            .unwrap()
    }

    #[test]
    fn wildcard_bind_distinguishes_same_port_remote_and_local_ips() {
        for bind in ["0.0.0.0", "[::]"] {
            for (target, remote) in [
                ("192.0.2.80", true),
                ("[2001:db8::80]", true),
                ("192.0.2.10", false),
                ("[2001:db8::10]", false),
                ("[::ffff:192.0.2.10]", false),
                ("127.0.0.1", false),
                ("127.0.0.2", false),
                ("[::1]", false),
                ("[::ffff:127.0.0.1]", false),
                ("0.0.0.0", false),
                ("[::]", false),
                ("localhost", false),
                ("ambiguous.test", false),
            ] {
                assert_eq!(
                    proxy_request_targets_other(&request(target, 18888), 18888, bind, || {
                        Some(vec![
                            "192.0.2.10".parse().unwrap(),
                            "2001:db8::10".parse().unwrap(),
                        ])
                    }),
                    remote,
                    "bind={bind}, target={target}"
                );
            }
        }
    }

    #[test]
    fn wildcard_interface_failure_keeps_admin_validation() {
        assert!(!proxy_request_targets_other(
            &request("192.0.2.80", 18888),
            18888,
            "0.0.0.0",
            || None
        ));
    }

    #[test]
    fn different_port_and_explicit_bind_do_not_enumerate_interfaces() {
        for (target, port, bind, remote) in [
            ("192.0.2.80", 18889, "0.0.0.0", true),
            ("192.0.2.80", 18888, "192.0.2.10", true),
            ("192.0.2.10", 18888, "192.0.2.10", false),
        ] {
            assert_eq!(
                proxy_request_targets_other(&request(target, port), 18888, bind, || panic!(
                    "interface enumeration must be lazy"
                )),
                remote
            );
        }
    }

    #[test]
    fn relative_admin_and_virtual_host_routes_are_preserved() {
        for bind in ["0.0.0.0", "[::]"] {
            let relative = Request::builder()
                .uri("/_bifrost/api/rules")
                .body(())
                .unwrap();
            assert!(admin_routing_decision(&relative, 18888, bind).routes_to_admin);
            let virtual_host = request("bifrost.local", 18888);
            assert!(admin_routing_decision(&virtual_host, 18888, bind).routes_to_admin);
        }
    }

    #[test]
    fn same_port_remote_ip_skips_admin_but_local_proxy_request_still_fails_security() {
        let req = request("192.0.2.80", 18888);
        let routing = admin_routing_decision(&req, 18888, "0.0.0.0");
        assert!(routing.is_proxy_request_to_other_server);
        assert!(!routing.routes_to_admin);

        let local = request("127.0.0.1", 18888);
        assert!(admin_routing_decision(&local, 18888, "0.0.0.0").routes_to_admin);
        assert!(!bifrost_admin::is_valid_admin_request(
            &local,
            "127.0.0.1:12345".parse().unwrap(),
            &bifrost_admin::AdminSecurityConfig::new(18888),
            true
        ));
    }
}
