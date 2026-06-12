use std::collections::{HashMap, HashSet};
use std::net::IpAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::RwLock;
use std::time::{SystemTime, UNIX_EPOCH};

use ipnet::IpNet;
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;
use tracing::{debug, info, warn};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AccessMode {
    AllowAll,
    LocalOnly,
    Whitelist,
    #[default]
    Interactive,
}

impl Serialize for AccessMode {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for AccessMode {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

impl std::str::FromStr for AccessMode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "allow_all" | "allowall" | "all" => Ok(AccessMode::AllowAll),
            "local_only" | "localonly" | "local" => Ok(AccessMode::LocalOnly),
            "whitelist" | "wl" => Ok(AccessMode::Whitelist),
            "interactive" | "prompt" => Ok(AccessMode::Interactive),
            _ => Err(format!("Invalid access mode: {}", s)),
        }
    }
}

impl std::fmt::Display for AccessMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AccessMode::AllowAll => write!(f, "allow_all"),
            AccessMode::LocalOnly => write!(f, "local_only"),
            AccessMode::Whitelist => write!(f, "whitelist"),
            AccessMode::Interactive => write!(f, "interactive"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccessDecision {
    Allow,
    Deny,
    Prompt(IpAddr),
}

#[derive(Debug, Clone, Serialize)]
pub struct PendingAuth {
    pub ip: String,
    pub first_seen: u64,
    pub attempt_count: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct PendingAuthEvent {
    pub event_type: String,
    pub pending_auth: PendingAuth,
    pub total_pending: usize,
}

#[derive(Debug, Clone)]
pub struct AccessControlConfig {
    pub mode: AccessMode,
    pub whitelist: Vec<String>,
    pub allow_lan: bool,
    pub userpass: Option<UserPassAuthConfig>,
}

impl Default for AccessControlConfig {
    fn default() -> Self {
        Self {
            mode: AccessMode::Interactive,
            whitelist: Vec::new(),
            allow_lan: false,
            userpass: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UserPassAccountConfig {
    pub username: String,
    pub password: Option<String>,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct UserPassAuthConfig {
    pub enabled: bool,
    pub accounts: Vec<UserPassAccountConfig>,
    #[serde(default)]
    pub loopback_requires_auth: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct UserPassAccountStatus {
    pub username: String,
    pub enabled: bool,
    pub has_password: bool,
    pub last_connected_at: Option<u64>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq, Default)]
pub struct UserPassAuthStatus {
    pub enabled: bool,
    pub accounts: Vec<UserPassAccountStatus>,
    pub loopback_requires_auth: bool,
}

pub struct ClientAccessControl {
    mode: AccessMode,
    whitelist: HashSet<IpNet>,
    allow_lan: bool,
    userpass: RwLock<Option<UserPassAuthConfig>>,
    userpass_last_connected_at: RwLock<HashMap<String, u64>>,
    temporary_whitelist: RwLock<HashSet<IpAddr>>,
    session_denied: RwLock<HashSet<IpAddr>>,
    pending_authorization: RwLock<Vec<(IpAddr, u64, u32)>>,
    event_sender: broadcast::Sender<PendingAuthEvent>,
    generation: AtomicU64,
    local_subnets: RwLock<Vec<IpNet>>,
}

impl ClientAccessControl {
    pub fn new(config: AccessControlConfig) -> Self {
        let mut whitelist = HashSet::new();

        for entry in &config.whitelist {
            match Self::parse_ip_or_cidr(entry) {
                Ok(ip_net) => {
                    whitelist.insert(ip_net);
                }
                Err(e) => {
                    warn!("Invalid whitelist entry '{}': {}", entry, e);
                }
            }
        }

        let (event_sender, _) = broadcast::channel(64);

        Self {
            mode: config.mode,
            whitelist,
            allow_lan: config.allow_lan,
            userpass: RwLock::new(config.userpass),
            userpass_last_connected_at: RwLock::new(HashMap::new()),
            temporary_whitelist: RwLock::new(HashSet::new()),
            session_denied: RwLock::new(HashSet::new()),
            pending_authorization: RwLock::new(Vec::new()),
            event_sender,
            generation: AtomicU64::new(1),
            local_subnets: RwLock::new(Vec::new()),
        }
    }

    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    fn increment_generation(&self) {
        self.generation.fetch_add(1, Ordering::Release);
    }

    pub fn with_mode(mode: AccessMode) -> Self {
        let (event_sender, _) = broadcast::channel(64);

        Self {
            mode,
            whitelist: HashSet::new(),
            allow_lan: false,
            userpass: RwLock::new(None),
            userpass_last_connected_at: RwLock::new(HashMap::new()),
            temporary_whitelist: RwLock::new(HashSet::new()),
            session_denied: RwLock::new(HashSet::new()),
            pending_authorization: RwLock::new(Vec::new()),
            event_sender,
            generation: AtomicU64::new(1),
            local_subnets: RwLock::new(Vec::new()),
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<PendingAuthEvent> {
        self.event_sender.subscribe()
    }

    fn broadcast_event(&self, event_type: &str, pending_auth: PendingAuth, total_pending: usize) {
        let event = PendingAuthEvent {
            event_type: event_type.to_string(),
            pending_auth,
            total_pending,
        };
        let _ = self.event_sender.send(event);
    }

    fn parse_ip_or_cidr(s: &str) -> Result<IpNet, String> {
        if s.contains('/') {
            s.parse::<IpNet>()
                .map_err(|e| format!("Invalid CIDR notation: {}", e))
        } else {
            let ip: IpAddr = s
                .parse()
                .map_err(|e| format!("Invalid IP address: {}", e))?;
            let ip_net = match ip {
                IpAddr::V4(v4) => format!("{}/32", v4)
                    .parse()
                    .map_err(|e| format!("Failed to create IpNet: {}", e))?,
                IpAddr::V6(v6) => format!("{}/128", v6)
                    .parse()
                    .map_err(|e| format!("Failed to create IpNet: {}", e))?,
            };
            Ok(ip_net)
        }
    }

    pub fn check_access(&self, peer_addr: &IpAddr) -> AccessDecision {
        if self.is_loopback(peer_addr) {
            debug!("Allowing loopback address: {}", peer_addr);
            return AccessDecision::Allow;
        }

        match self.mode {
            AccessMode::AllowAll => {
                debug!("AllowAll mode: accepting {}", peer_addr);
                AccessDecision::Allow
            }
            AccessMode::LocalOnly => {
                info!("LocalOnly mode: rejecting non-local address {}", peer_addr);
                AccessDecision::Deny
            }
            AccessMode::Whitelist => self.check_whitelist(peer_addr),
            AccessMode::Interactive => self.check_interactive(peer_addr),
        }
    }

    fn check_whitelist(&self, peer_addr: &IpAddr) -> AccessDecision {
        if self.is_in_temporary_whitelist(peer_addr) {
            debug!("Allowing temporarily whitelisted address: {}", peer_addr);
            return AccessDecision::Allow;
        }

        if self.is_in_whitelist(peer_addr) {
            debug!("Allowing whitelisted address: {}", peer_addr);
            return AccessDecision::Allow;
        }

        if self.allow_lan && self.is_private_network(peer_addr) {
            debug!("Allowing LAN address: {}", peer_addr);
            return AccessDecision::Allow;
        }

        info!(
            "Whitelist mode: rejecting non-whitelisted address {}",
            peer_addr
        );
        AccessDecision::Deny
    }

    fn check_interactive(&self, peer_addr: &IpAddr) -> AccessDecision {
        if self.is_session_denied(peer_addr) {
            debug!("Denying previously rejected address: {}", peer_addr);
            return AccessDecision::Deny;
        }

        if self.is_in_temporary_whitelist(peer_addr) {
            debug!("Allowing temporarily whitelisted address: {}", peer_addr);
            return AccessDecision::Allow;
        }

        if self.is_in_whitelist(peer_addr) {
            debug!("Allowing whitelisted address: {}", peer_addr);
            return AccessDecision::Allow;
        }

        if self.allow_lan && self.is_private_network(peer_addr) {
            debug!("Allowing LAN address: {}", peer_addr);
            return AccessDecision::Allow;
        }

        AccessDecision::Prompt(*peer_addr)
    }

    pub fn is_loopback(&self, ip: &IpAddr) -> bool {
        ip.is_loopback()
    }

    pub fn is_private_network(&self, ip: &IpAddr) -> bool {
        if self.is_in_local_subnet(ip) {
            return true;
        }
        Self::is_private_range(ip)
    }

    fn is_in_local_subnet(&self, ip: &IpAddr) -> bool {
        let subnets = self.local_subnets.read().unwrap();
        subnets.iter().any(|subnet| subnet.contains(ip))
    }

    pub fn is_private_range(ip: &IpAddr) -> bool {
        match ip {
            IpAddr::V4(v4) => {
                let octets = v4.octets();
                octets[0] == 10
                    || (octets[0] == 172 && (16..=31).contains(&octets[1]))
                    || (octets[0] == 192 && octets[1] == 168)
                    || (octets[0] == 169 && octets[1] == 254)
                    || (octets[0] == 100 && (64..=127).contains(&octets[1]))
            }
            IpAddr::V6(v6) => {
                let segments = v6.segments();
                (segments[0] & 0xfe00) == 0xfc00 || (segments[0] == 0xfe80)
            }
        }
    }

    pub fn set_local_subnets(&self, subnets: Vec<IpNet>) {
        info!(
            "Updating local subnets: {:?}",
            subnets.iter().map(|s| s.to_string()).collect::<Vec<_>>()
        );
        *self.local_subnets.write().unwrap() = subnets;
    }

    pub fn local_subnets(&self) -> Vec<IpNet> {
        self.local_subnets.read().unwrap().clone()
    }

    pub fn is_in_whitelist(&self, ip: &IpAddr) -> bool {
        for net in &self.whitelist {
            if net.contains(ip) {
                return true;
            }
        }
        false
    }

    pub fn is_in_temporary_whitelist(&self, ip: &IpAddr) -> bool {
        let temp = self.temporary_whitelist.read().unwrap();
        temp.contains(ip)
    }

    pub fn is_session_denied(&self, ip: &IpAddr) -> bool {
        let denied = self.session_denied.read().unwrap();
        denied.contains(ip)
    }

    pub fn add_to_whitelist(&mut self, ip_or_cidr: &str) -> Result<(), String> {
        let ip_net = Self::parse_ip_or_cidr(ip_or_cidr)?;
        self.whitelist.insert(ip_net);
        self.increment_generation();
        info!("Added {} to whitelist", ip_or_cidr);
        Ok(())
    }

    pub fn remove_from_whitelist(&mut self, ip_or_cidr: &str) -> Result<bool, String> {
        let ip_net = Self::parse_ip_or_cidr(ip_or_cidr)?;
        let removed = self.whitelist.remove(&ip_net);
        if removed {
            self.increment_generation();
            info!("Removed {} from whitelist", ip_or_cidr);
        }
        Ok(removed)
    }

    pub fn add_temporary(&self, ip: IpAddr) {
        let mut temp = self.temporary_whitelist.write().unwrap();
        temp.insert(ip);
        self.increment_generation();
        info!("Added {} to temporary whitelist", ip);
    }

    pub fn remove_temporary(&self, ip: &IpAddr) -> bool {
        let mut temp = self.temporary_whitelist.write().unwrap();
        let removed = temp.remove(ip);
        if removed {
            self.increment_generation();
        }
        removed
    }

    pub fn deny_session(&self, ip: IpAddr) {
        let mut denied = self.session_denied.write().unwrap();
        denied.insert(ip);
        self.increment_generation();
        info!("Denied {} for this session", ip);
    }

    pub fn clear_session_denied(&self) {
        let mut denied = self.session_denied.write().unwrap();
        denied.clear();
        self.increment_generation();
    }

    pub fn session_denied_entries(&self) -> Vec<IpAddr> {
        let denied = self.session_denied.read().unwrap();
        denied.iter().cloned().collect()
    }

    pub fn remove_session_denied(&self, ip: &IpAddr) -> bool {
        let mut denied = self.session_denied.write().unwrap();
        let removed = denied.remove(ip);
        if removed {
            self.increment_generation();
            info!("Removed {} from session denied list", ip);
        }
        removed
    }

    pub fn whitelist_entries(&self) -> Vec<String> {
        self.whitelist.iter().map(|n| n.to_string()).collect()
    }

    pub fn temporary_whitelist_entries(&self) -> Vec<IpAddr> {
        let temp = self.temporary_whitelist.read().unwrap();
        temp.iter().cloned().collect()
    }

    pub fn mode(&self) -> AccessMode {
        self.mode
    }

    pub fn set_mode(&mut self, mode: AccessMode) {
        self.mode = mode;
        self.increment_generation();
        info!("Access mode changed to: {}", mode);
    }

    pub fn allow_lan(&self) -> bool {
        self.allow_lan
    }

    pub fn set_allow_lan(&mut self, allow: bool) {
        self.allow_lan = allow;
        self.increment_generation();
        info!("Allow LAN set to: {}", allow);
    }

    pub fn userpass_config(&self) -> Option<UserPassAuthConfig> {
        self.userpass.read().unwrap().clone()
    }

    pub fn set_userpass_config(&self, config: Option<UserPassAuthConfig>) {
        *self.userpass.write().unwrap() = config;
        self.retain_userpass_runtime_state();
        self.increment_generation();
    }

    pub fn has_userpass_auth(&self) -> bool {
        self.userpass
            .read()
            .unwrap()
            .as_ref()
            .is_some_and(|config| config.enabled)
    }

    pub fn should_defer_userpass(&self, decision: &AccessDecision) -> bool {
        if !self.has_userpass_auth() {
            return false;
        }
        if matches!(decision, AccessDecision::Allow) {
            return self.loopback_requires_auth();
        }
        true
    }

    pub fn loopback_requires_auth(&self) -> bool {
        self.userpass
            .read()
            .unwrap()
            .as_ref()
            .is_some_and(|config| config.enabled && config.loopback_requires_auth)
    }

    pub fn verify_userpass(&self, username: &str, password: &str) -> Option<String> {
        let userpass = self.userpass.read().unwrap();
        let config = userpass.as_ref()?;
        if !config.enabled {
            return None;
        }

        config.accounts.iter().find_map(|account| {
            if !account.enabled {
                return None;
            }

            match &account.password {
                Some(expected_password)
                    if account.username == username && expected_password == password =>
                {
                    Some(account.username.clone())
                }
                _ => None,
            }
        })
    }

    pub fn record_userpass_success(&self, username: &str, timestamp: u64) {
        self.userpass_last_connected_at
            .write()
            .unwrap()
            .insert(username.to_string(), timestamp);
    }

    pub fn set_userpass_last_connected_at(&self, last_connected_at: HashMap<String, u64>) {
        *self.userpass_last_connected_at.write().unwrap() = last_connected_at;
        self.retain_userpass_runtime_state();
    }

    pub fn userpass_last_connected_at(&self) -> HashMap<String, u64> {
        self.userpass_last_connected_at.read().unwrap().clone()
    }

    pub fn userpass_status(&self) -> UserPassAuthStatus {
        let userpass = self.userpass.read().unwrap();
        let Some(config) = userpass.as_ref() else {
            return UserPassAuthStatus::default();
        };
        let last_connected_at = self.userpass_last_connected_at.read().unwrap();

        UserPassAuthStatus {
            enabled: config.enabled,
            accounts: config
                .accounts
                .iter()
                .map(|account| UserPassAccountStatus {
                    username: account.username.clone(),
                    enabled: account.enabled,
                    has_password: account.password.is_some(),
                    last_connected_at: last_connected_at.get(&account.username).copied(),
                })
                .collect(),
            loopback_requires_auth: config.loopback_requires_auth,
        }
    }

    pub fn add_pending_authorization(&self, ip: IpAddr) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let mut pending = self.pending_authorization.write().unwrap();

        let (first_seen, attempt_count, is_new) =
            if let Some(entry) = pending.iter_mut().find(|(addr, _, _)| *addr == ip) {
                entry.2 += 1;
                (entry.1, entry.2, false)
            } else {
                pending.push((ip, now, 1));
                info!("Added {} to pending authorization", ip);
                (now, 1, true)
            };

        let total_pending = pending.len();
        drop(pending);

        if is_new {
            let pending_auth = PendingAuth {
                ip: ip.to_string(),
                first_seen,
                attempt_count,
            };
            self.broadcast_event("new", pending_auth, total_pending);
        }
    }

    pub fn get_pending_authorizations(&self) -> Vec<PendingAuth> {
        let pending = self.pending_authorization.read().unwrap();
        pending
            .iter()
            .map(|(ip, first_seen, count)| PendingAuth {
                ip: ip.to_string(),
                first_seen: *first_seen,
                attempt_count: *count,
            })
            .collect()
    }

    pub fn pending_authorization_count(&self) -> usize {
        let pending = self.pending_authorization.read().unwrap();
        pending.len()
    }

    pub fn approve_pending(&self, ip: &IpAddr) -> bool {
        let mut pending = self.pending_authorization.write().unwrap();
        let removed_entry = pending.iter().find(|(addr, _, _)| addr == ip).cloned();

        if let Some((_, first_seen, attempt_count)) = removed_entry {
            pending.retain(|(addr, _, _)| addr != ip);
            let total_pending = pending.len();
            drop(pending);

            self.add_temporary(*ip);
            info!("Approved pending authorization for {}", ip);

            let pending_auth = PendingAuth {
                ip: ip.to_string(),
                first_seen,
                attempt_count,
            };
            self.broadcast_event("approved", pending_auth, total_pending);
            true
        } else {
            false
        }
    }

    pub fn reject_pending(&self, ip: &IpAddr) -> bool {
        let mut pending = self.pending_authorization.write().unwrap();
        let removed_entry = pending.iter().find(|(addr, _, _)| addr == ip).cloned();

        if let Some((_, first_seen, attempt_count)) = removed_entry {
            pending.retain(|(addr, _, _)| addr != ip);
            let total_pending = pending.len();
            drop(pending);

            self.deny_session(*ip);
            info!("Rejected pending authorization for {}", ip);

            let pending_auth = PendingAuth {
                ip: ip.to_string(),
                first_seen,
                attempt_count,
            };
            self.broadcast_event("rejected", pending_auth, total_pending);
            true
        } else {
            false
        }
    }

    pub fn clear_pending_authorizations(&self) {
        let mut pending = self.pending_authorization.write().unwrap();
        pending.clear();
        info!("Cleared all pending authorizations");
    }

    fn retain_userpass_runtime_state(&self) {
        let usernames: HashSet<String> = self
            .userpass
            .read()
            .unwrap()
            .as_ref()
            .map(|config| {
                config
                    .accounts
                    .iter()
                    .map(|account| account.username.clone())
                    .collect()
            })
            .unwrap_or_default();
        self.userpass_last_connected_at
            .write()
            .unwrap()
            .retain(|username, _| usernames.contains(username));
    }
}

impl Default for ClientAccessControl {
    fn default() -> Self {
        Self::new(AccessControlConfig::default())
    }
}

const PROXY_AUTH_MAX_FAILURES: u32 = 10;
const PROXY_AUTH_BAN_DURATION_SECS: u64 = 300;
const PROXY_AUTH_CLEANUP_INTERVAL_SECS: u64 = 60;

#[derive(Debug)]
struct ProxyAuthIpState {
    failures: u32,
    last_failure: u64,
    banned_until: Option<u64>,
}

pub struct ProxyAuthRateLimiter {
    state: RwLock<HashMap<IpAddr, ProxyAuthIpState>>,
    last_cleanup: RwLock<u64>,
}

impl ProxyAuthRateLimiter {
    pub fn new() -> Self {
        Self {
            state: RwLock::new(HashMap::new()),
            last_cleanup: RwLock::new(0),
        }
    }

    fn now_secs() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
    }

    pub fn is_banned(&self, ip: &IpAddr) -> bool {
        let now = Self::now_secs();
        self.maybe_cleanup(now);
        let state = self.state.read().unwrap();
        if let Some(entry) = state.get(ip) {
            if let Some(banned_until) = entry.banned_until {
                return now < banned_until;
            }
        }
        false
    }

    pub fn record_failure(&self, ip: IpAddr) -> u32 {
        let now = Self::now_secs();
        self.maybe_cleanup(now);
        let mut state = self.state.write().unwrap();
        let entry = state.entry(ip).or_insert(ProxyAuthIpState {
            failures: 0,
            last_failure: now,
            banned_until: None,
        });
        entry.failures += 1;
        entry.last_failure = now;

        if entry.failures >= PROXY_AUTH_MAX_FAILURES {
            entry.banned_until = Some(now + PROXY_AUTH_BAN_DURATION_SECS);
            warn!(
                ip = %ip,
                failures = entry.failures,
                ban_seconds = PROXY_AUTH_BAN_DURATION_SECS,
                "Proxy auth: IP temporarily banned due to too many failed attempts"
            );
        }
        entry.failures
    }

    pub fn record_success(&self, ip: &IpAddr) {
        let mut state = self.state.write().unwrap();
        state.remove(ip);
    }

    fn maybe_cleanup(&self, now: u64) {
        let last = *self.last_cleanup.read().unwrap();
        if now.saturating_sub(last) < PROXY_AUTH_CLEANUP_INTERVAL_SECS {
            return;
        }
        if let Ok(mut last_cleanup) = self.last_cleanup.try_write() {
            if now.saturating_sub(*last_cleanup) < PROXY_AUTH_CLEANUP_INTERVAL_SECS {
                return;
            }
            *last_cleanup = now;
            drop(last_cleanup);

            let mut state = self.state.write().unwrap();
            state.retain(|_, entry| {
                if let Some(banned_until) = entry.banned_until {
                    if now >= banned_until {
                        return false;
                    }
                }
                now.saturating_sub(entry.last_failure) < PROXY_AUTH_BAN_DURATION_SECS
            });
        }
    }

    pub fn failure_count(&self, ip: &IpAddr) -> u32 {
        let state = self.state.read().unwrap();
        state.get(ip).map(|e| e.failures).unwrap_or(0)
    }
}

impl Default for ProxyAuthRateLimiter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr};

    #[test]
    fn test_access_mode_from_str() {
        assert_eq!(
            "allow_all".parse::<AccessMode>().unwrap(),
            AccessMode::AllowAll
        );
        assert_eq!(
            "local_only".parse::<AccessMode>().unwrap(),
            AccessMode::LocalOnly
        );
        assert_eq!(
            "whitelist".parse::<AccessMode>().unwrap(),
            AccessMode::Whitelist
        );
        assert_eq!(
            "interactive".parse::<AccessMode>().unwrap(),
            AccessMode::Interactive
        );
        assert!("invalid".parse::<AccessMode>().is_err());
    }

    #[test]
    fn test_loopback_detection() {
        let ac = ClientAccessControl::default();

        assert!(ac.is_loopback(&IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1))));
        assert!(ac.is_loopback(&IpAddr::V6(Ipv6Addr::LOCALHOST)));
        assert!(!ac.is_loopback(&IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1))));
    }

    #[test]
    fn test_private_network_detection() {
        let ac = ClientAccessControl::default();

        assert!(ac.is_private_network(&IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1))));
        assert!(ac.is_private_network(&IpAddr::V4(Ipv4Addr::new(172, 16, 0, 1))));
        assert!(ac.is_private_network(&IpAddr::V4(Ipv4Addr::new(172, 31, 255, 255))));
        assert!(ac.is_private_network(&IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1))));
        assert!(ac.is_private_network(&IpAddr::V4(Ipv4Addr::new(169, 254, 1, 1))));

        assert!(ac.is_private_network(&IpAddr::V4(Ipv4Addr::new(100, 64, 0, 1))));
        assert!(ac.is_private_network(&IpAddr::V4(Ipv4Addr::new(100, 86, 178, 33))));
        assert!(ac.is_private_network(&IpAddr::V4(Ipv4Addr::new(100, 127, 255, 255))));

        assert!(!ac.is_private_network(&IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8))));
        assert!(!ac.is_private_network(&IpAddr::V4(Ipv4Addr::new(172, 32, 0, 1))));
        assert!(!ac.is_private_network(&IpAddr::V4(Ipv4Addr::new(100, 63, 255, 255))));
        assert!(!ac.is_private_network(&IpAddr::V4(Ipv4Addr::new(100, 128, 0, 1))));
    }

    #[test]
    fn test_cgn_address_allowed_with_allow_lan() {
        let config = AccessControlConfig {
            mode: AccessMode::Interactive,
            whitelist: vec![],
            allow_lan: true,
            userpass: None,
        };
        let ac = ClientAccessControl::new(config);

        let cgn_ip = IpAddr::V4(Ipv4Addr::new(100, 86, 178, 33));
        assert_eq!(ac.check_access(&cgn_ip), AccessDecision::Allow);
    }

    #[test]
    fn test_cgn_address_prompts_without_allow_lan() {
        let config = AccessControlConfig {
            mode: AccessMode::Interactive,
            whitelist: vec![],
            allow_lan: false,
            userpass: None,
        };
        let ac = ClientAccessControl::new(config);

        let cgn_ip = IpAddr::V4(Ipv4Addr::new(100, 86, 178, 33));
        assert_eq!(ac.check_access(&cgn_ip), AccessDecision::Prompt(cgn_ip));
    }

    #[test]
    fn test_local_subnet_detection_allows_same_subnet() {
        let config = AccessControlConfig {
            mode: AccessMode::Interactive,
            whitelist: vec![],
            allow_lan: true,
            userpass: None,
        };
        let ac = ClientAccessControl::new(config);

        let subnet: IpNet = "203.0.113.0/24".parse().unwrap();
        ac.set_local_subnets(vec![subnet]);

        let same_subnet_ip: IpAddr = "203.0.113.50".parse().unwrap();
        assert!(ac.is_private_network(&same_subnet_ip));
        assert_eq!(ac.check_access(&same_subnet_ip), AccessDecision::Allow);

        let different_subnet_ip: IpAddr = "203.0.114.50".parse().unwrap();
        assert!(!ac.is_private_network(&different_subnet_ip));
        assert_eq!(
            ac.check_access(&different_subnet_ip),
            AccessDecision::Prompt(different_subnet_ip)
        );
    }

    #[test]
    fn test_local_subnet_detection_any_public_ip_in_same_subnet() {
        let config = AccessControlConfig {
            mode: AccessMode::Interactive,
            whitelist: vec![],
            allow_lan: true,
            userpass: None,
        };
        let ac = ClientAccessControl::new(config);

        let subnet: IpNet = "100.86.0.0/16".parse().unwrap();
        ac.set_local_subnets(vec![subnet]);

        let same_subnet_ip: IpAddr = "100.86.178.33".parse().unwrap();
        assert!(ac.is_private_network(&same_subnet_ip));
        assert_eq!(ac.check_access(&same_subnet_ip), AccessDecision::Allow);

        let different_subnet_ip: IpAddr = "100.87.0.1".parse().unwrap();
        assert!(!ac.is_in_local_subnet(&different_subnet_ip));
    }

    #[test]
    fn test_subnet_hot_update_changes_access_decision() {
        let config = AccessControlConfig {
            mode: AccessMode::Interactive,
            whitelist: vec![],
            allow_lan: true,
            userpass: None,
        };
        let ac = ClientAccessControl::new(config);

        let target_ip: IpAddr = "172.20.10.5".parse().unwrap();
        assert_eq!(ac.check_access(&target_ip), AccessDecision::Allow);

        let target_ip: IpAddr = "203.0.113.50".parse().unwrap();
        assert_eq!(
            ac.check_access(&target_ip),
            AccessDecision::Prompt(target_ip)
        );

        let subnet: IpNet = "203.0.113.0/24".parse().unwrap();
        ac.set_local_subnets(vec![subnet]);
        assert_eq!(ac.check_access(&target_ip), AccessDecision::Allow);

        ac.set_local_subnets(vec![]);
        assert_eq!(
            ac.check_access(&target_ip),
            AccessDecision::Prompt(target_ip)
        );

        let subnet_a: IpNet = "10.0.0.0/8".parse().unwrap();
        let subnet_b: IpNet = "203.0.113.0/24".parse().unwrap();
        ac.set_local_subnets(vec![subnet_a, subnet_b]);
        assert_eq!(ac.check_access(&target_ip), AccessDecision::Allow);
        let ip_in_a: IpAddr = "10.1.2.3".parse().unwrap();
        assert_eq!(ac.check_access(&ip_in_a), AccessDecision::Allow);
    }

    #[test]
    fn test_local_only_mode() {
        let ac = ClientAccessControl::with_mode(AccessMode::LocalOnly);

        let localhost = IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1));
        assert_eq!(ac.check_access(&localhost), AccessDecision::Allow);

        let external = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 100));
        assert_eq!(ac.check_access(&external), AccessDecision::Deny);
    }

    #[test]
    fn test_allow_all_mode() {
        let ac = ClientAccessControl::with_mode(AccessMode::AllowAll);

        let external = IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8));
        assert_eq!(ac.check_access(&external), AccessDecision::Allow);
    }

    #[test]
    fn test_whitelist_mode() {
        let config = AccessControlConfig {
            mode: AccessMode::Whitelist,
            whitelist: vec!["192.168.1.0/24".to_string()],
            allow_lan: false,
            userpass: None,
        };
        let ac = ClientAccessControl::new(config);

        let allowed = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 100));
        assert_eq!(ac.check_access(&allowed), AccessDecision::Allow);

        let denied = IpAddr::V4(Ipv4Addr::new(192, 168, 2, 100));
        assert_eq!(ac.check_access(&denied), AccessDecision::Deny);
    }

    #[test]
    fn test_whitelist_with_allow_lan() {
        let config = AccessControlConfig {
            mode: AccessMode::Whitelist,
            whitelist: vec![],
            allow_lan: true,
            userpass: None,
        };
        let ac = ClientAccessControl::new(config);

        let lan_ip = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 100));
        assert_eq!(ac.check_access(&lan_ip), AccessDecision::Allow);

        let external = IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8));
        assert_eq!(ac.check_access(&external), AccessDecision::Deny);
    }

    #[test]
    fn test_interactive_mode() {
        let ac = ClientAccessControl::with_mode(AccessMode::Interactive);

        let external = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 100));
        assert_eq!(ac.check_access(&external), AccessDecision::Prompt(external));
    }

    #[test]
    fn test_temporary_whitelist() {
        let ac = ClientAccessControl::with_mode(AccessMode::Whitelist);

        let ip = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 100));
        assert_eq!(ac.check_access(&ip), AccessDecision::Deny);

        ac.add_temporary(ip);
        assert_eq!(ac.check_access(&ip), AccessDecision::Allow);

        ac.remove_temporary(&ip);
        assert_eq!(ac.check_access(&ip), AccessDecision::Deny);
    }

    #[test]
    fn test_session_denied() {
        let ac = ClientAccessControl::with_mode(AccessMode::Interactive);

        let ip = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 100));

        assert_eq!(ac.check_access(&ip), AccessDecision::Prompt(ip));

        ac.deny_session(ip);
        assert_eq!(ac.check_access(&ip), AccessDecision::Deny);

        ac.clear_session_denied();
        assert_eq!(ac.check_access(&ip), AccessDecision::Prompt(ip));
    }

    #[test]
    fn test_add_remove_whitelist() {
        let mut ac = ClientAccessControl::with_mode(AccessMode::Whitelist);

        ac.add_to_whitelist("10.0.0.1").unwrap();
        let ip = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));
        assert!(ac.is_in_whitelist(&ip));

        ac.remove_from_whitelist("10.0.0.1").unwrap();
        assert!(!ac.is_in_whitelist(&ip));
    }

    #[test]
    fn test_cidr_whitelist() {
        let mut ac = ClientAccessControl::with_mode(AccessMode::Whitelist);

        ac.add_to_whitelist("10.0.0.0/8").unwrap();

        assert!(ac.is_in_whitelist(&IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1))));
        assert!(ac.is_in_whitelist(&IpAddr::V4(Ipv4Addr::new(10, 255, 255, 255))));
        assert!(!ac.is_in_whitelist(&IpAddr::V4(Ipv4Addr::new(11, 0, 0, 1))));
    }

    #[test]
    fn test_whitelist_entries() {
        let config = AccessControlConfig {
            mode: AccessMode::Whitelist,
            whitelist: vec!["192.168.1.0/24".to_string(), "10.0.0.1".to_string()],
            allow_lan: false,
            userpass: None,
        };
        let ac = ClientAccessControl::new(config);

        let entries = ac.whitelist_entries();
        assert_eq!(entries.len(), 2);
    }

    #[test]
    fn test_mode_display() {
        assert_eq!(format!("{}", AccessMode::AllowAll), "allow_all");
        assert_eq!(format!("{}", AccessMode::LocalOnly), "local_only");
        assert_eq!(format!("{}", AccessMode::Whitelist), "whitelist");
        assert_eq!(format!("{}", AccessMode::Interactive), "interactive");
    }

    #[test]
    fn test_should_defer_userpass_allow_decision_no_defer_default() {
        let config = AccessControlConfig {
            mode: AccessMode::LocalOnly,
            whitelist: vec![],
            allow_lan: false,
            userpass: Some(UserPassAuthConfig {
                enabled: true,
                accounts: vec![UserPassAccountConfig {
                    username: "user1".to_string(),
                    password: Some("pass1".to_string()),
                    enabled: true,
                }],
                loopback_requires_auth: false,
            }),
        };
        let ac = ClientAccessControl::new(config);

        assert!(ac.has_userpass_auth());
        assert!(!ac.should_defer_userpass(&AccessDecision::Allow));
    }

    #[test]
    fn test_should_defer_userpass_allow_defers_when_loopback_requires_auth() {
        let config = AccessControlConfig {
            mode: AccessMode::LocalOnly,
            whitelist: vec![],
            allow_lan: false,
            userpass: Some(UserPassAuthConfig {
                enabled: true,
                accounts: vec![UserPassAccountConfig {
                    username: "user1".to_string(),
                    password: Some("pass1".to_string()),
                    enabled: true,
                }],
                loopback_requires_auth: true,
            }),
        };
        let ac = ClientAccessControl::new(config);

        assert!(ac.has_userpass_auth());
        assert!(ac.should_defer_userpass(&AccessDecision::Allow));
    }

    #[test]
    fn test_should_defer_userpass_deny_decision_defers_when_enabled() {
        let config = AccessControlConfig {
            mode: AccessMode::LocalOnly,
            whitelist: vec![],
            allow_lan: false,
            userpass: Some(UserPassAuthConfig {
                enabled: true,
                accounts: vec![UserPassAccountConfig {
                    username: "user1".to_string(),
                    password: Some("pass1".to_string()),
                    enabled: true,
                }],
                loopback_requires_auth: false,
            }),
        };
        let ac = ClientAccessControl::new(config);

        assert!(ac.should_defer_userpass(&AccessDecision::Deny));
    }

    #[test]
    fn test_should_defer_userpass_deny_no_defer_without_userpass() {
        let ac = ClientAccessControl::with_mode(AccessMode::LocalOnly);

        assert!(!ac.has_userpass_auth());
        assert!(!ac.should_defer_userpass(&AccessDecision::Deny));
    }

    #[test]
    fn test_loopback_allowed_with_userpass_enabled_default() {
        let config = AccessControlConfig {
            mode: AccessMode::LocalOnly,
            whitelist: vec![],
            allow_lan: false,
            userpass: Some(UserPassAuthConfig {
                enabled: true,
                accounts: vec![UserPassAccountConfig {
                    username: "user1".to_string(),
                    password: Some("pass1".to_string()),
                    enabled: true,
                }],
                loopback_requires_auth: false,
            }),
        };
        let ac = ClientAccessControl::new(config);

        let localhost = IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1));
        let decision = ac.check_access(&localhost);
        assert_eq!(decision, AccessDecision::Allow);
        assert!(!ac.should_defer_userpass(&decision));

        let external = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 100));
        let decision = ac.check_access(&external);
        assert_eq!(decision, AccessDecision::Deny);
        assert!(ac.should_defer_userpass(&decision));
    }

    #[test]
    fn test_loopback_requires_auth_when_switch_enabled() {
        let config = AccessControlConfig {
            mode: AccessMode::LocalOnly,
            whitelist: vec![],
            allow_lan: false,
            userpass: Some(UserPassAuthConfig {
                enabled: true,
                accounts: vec![UserPassAccountConfig {
                    username: "user1".to_string(),
                    password: Some("pass1".to_string()),
                    enabled: true,
                }],
                loopback_requires_auth: true,
            }),
        };
        let ac = ClientAccessControl::new(config);

        let localhost = IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1));
        let decision = ac.check_access(&localhost);
        assert_eq!(decision, AccessDecision::Allow);
        assert!(ac.should_defer_userpass(&decision));

        let external = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 100));
        let decision = ac.check_access(&external);
        assert_eq!(decision, AccessDecision::Deny);
        assert!(ac.should_defer_userpass(&decision));
    }

    #[test]
    fn test_prompt_decision_defers_with_userpass() {
        let config = AccessControlConfig {
            mode: AccessMode::Interactive,
            whitelist: vec![],
            allow_lan: false,
            userpass: Some(UserPassAuthConfig {
                enabled: true,
                accounts: vec![UserPassAccountConfig {
                    username: "user1".to_string(),
                    password: Some("pass1".to_string()),
                    enabled: true,
                }],
                loopback_requires_auth: false,
            }),
        };
        let ac = ClientAccessControl::new(config);

        let external = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 100));
        let decision = ac.check_access(&external);
        assert!(matches!(decision, AccessDecision::Prompt(_)));
        assert!(ac.should_defer_userpass(&decision));
    }

    #[test]
    fn test_loopback_requires_auth_getter() {
        let config_off = AccessControlConfig {
            mode: AccessMode::LocalOnly,
            whitelist: vec![],
            allow_lan: false,
            userpass: Some(UserPassAuthConfig {
                enabled: true,
                accounts: vec![],
                loopback_requires_auth: false,
            }),
        };
        let ac = ClientAccessControl::new(config_off);
        assert!(!ac.loopback_requires_auth());

        let config_on = AccessControlConfig {
            mode: AccessMode::LocalOnly,
            whitelist: vec![],
            allow_lan: false,
            userpass: Some(UserPassAuthConfig {
                enabled: true,
                accounts: vec![],
                loopback_requires_auth: true,
            }),
        };
        let ac = ClientAccessControl::new(config_on);
        assert!(ac.loopback_requires_auth());

        let config_disabled = AccessControlConfig {
            mode: AccessMode::LocalOnly,
            whitelist: vec![],
            allow_lan: false,
            userpass: Some(UserPassAuthConfig {
                enabled: false,
                accounts: vec![],
                loopback_requires_auth: true,
            }),
        };
        let ac = ClientAccessControl::new(config_disabled);
        assert!(!ac.loopback_requires_auth());
    }

    #[test]
    fn test_verify_userpass_correct_credentials() {
        let config = AccessControlConfig {
            mode: AccessMode::LocalOnly,
            whitelist: vec![],
            allow_lan: false,
            userpass: Some(UserPassAuthConfig {
                enabled: true,
                accounts: vec![UserPassAccountConfig {
                    username: "user1".to_string(),
                    password: Some("pass123".to_string()),
                    enabled: true,
                }],
                loopback_requires_auth: false,
            }),
        };
        let ac = ClientAccessControl::new(config);
        assert_eq!(
            ac.verify_userpass("user1", "pass123"),
            Some("user1".to_string())
        );
    }

    #[test]
    fn test_verify_userpass_wrong_password() {
        let config = AccessControlConfig {
            mode: AccessMode::LocalOnly,
            whitelist: vec![],
            allow_lan: false,
            userpass: Some(UserPassAuthConfig {
                enabled: true,
                accounts: vec![UserPassAccountConfig {
                    username: "user1".to_string(),
                    password: Some("pass123".to_string()),
                    enabled: true,
                }],
                loopback_requires_auth: false,
            }),
        };
        let ac = ClientAccessControl::new(config);
        assert_eq!(ac.verify_userpass("user1", "wrong"), None);
    }

    #[test]
    fn test_verify_userpass_wrong_username() {
        let config = AccessControlConfig {
            mode: AccessMode::LocalOnly,
            whitelist: vec![],
            allow_lan: false,
            userpass: Some(UserPassAuthConfig {
                enabled: true,
                accounts: vec![UserPassAccountConfig {
                    username: "user1".to_string(),
                    password: Some("pass123".to_string()),
                    enabled: true,
                }],
                loopback_requires_auth: false,
            }),
        };
        let ac = ClientAccessControl::new(config);
        assert_eq!(ac.verify_userpass("wronguser", "pass123"), None);
    }

    #[test]
    fn test_verify_userpass_disabled_account() {
        let config = AccessControlConfig {
            mode: AccessMode::LocalOnly,
            whitelist: vec![],
            allow_lan: false,
            userpass: Some(UserPassAuthConfig {
                enabled: true,
                accounts: vec![UserPassAccountConfig {
                    username: "user1".to_string(),
                    password: Some("pass123".to_string()),
                    enabled: false,
                }],
                loopback_requires_auth: false,
            }),
        };
        let ac = ClientAccessControl::new(config);
        assert_eq!(ac.verify_userpass("user1", "pass123"), None);
    }

    #[test]
    fn test_rate_limiter_no_ban_initially() {
        let limiter = ProxyAuthRateLimiter::new();
        let ip: IpAddr = "192.168.1.100".parse().unwrap();
        assert!(!limiter.is_banned(&ip));
        assert_eq!(limiter.failure_count(&ip), 0);
    }

    #[test]
    fn test_rate_limiter_records_failures() {
        let limiter = ProxyAuthRateLimiter::new();
        let ip: IpAddr = "192.168.1.100".parse().unwrap();

        for i in 1..=5 {
            assert_eq!(limiter.record_failure(ip), i);
        }
        assert_eq!(limiter.failure_count(&ip), 5);
        assert!(!limiter.is_banned(&ip));
    }

    #[test]
    fn test_rate_limiter_bans_after_max_failures() {
        let limiter = ProxyAuthRateLimiter::new();
        let ip: IpAddr = "192.168.1.100".parse().unwrap();

        for _ in 0..PROXY_AUTH_MAX_FAILURES {
            limiter.record_failure(ip);
        }
        assert!(limiter.is_banned(&ip));
    }

    #[test]
    fn test_rate_limiter_success_resets() {
        let limiter = ProxyAuthRateLimiter::new();
        let ip: IpAddr = "192.168.1.100".parse().unwrap();

        for _ in 0..5 {
            limiter.record_failure(ip);
        }
        assert_eq!(limiter.failure_count(&ip), 5);

        limiter.record_success(&ip);
        assert_eq!(limiter.failure_count(&ip), 0);
        assert!(!limiter.is_banned(&ip));
    }

    #[test]
    fn test_rate_limiter_independent_ips() {
        let limiter = ProxyAuthRateLimiter::new();
        let ip1: IpAddr = "192.168.1.100".parse().unwrap();
        let ip2: IpAddr = "192.168.1.200".parse().unwrap();

        for _ in 0..PROXY_AUTH_MAX_FAILURES {
            limiter.record_failure(ip1);
        }
        assert!(limiter.is_banned(&ip1));
        assert!(!limiter.is_banned(&ip2));
    }

    #[test]
    fn access_mode_serde_roundtrip() {
        for mode in [
            AccessMode::AllowAll,
            AccessMode::LocalOnly,
            AccessMode::Whitelist,
            AccessMode::Interactive,
        ] {
            let json = serde_json::to_string(&mode).unwrap();
            let back: AccessMode = serde_json::from_str(&json).unwrap();
            assert_eq!(mode, back);
        }
        // Deserialize from alternate spellings + invalid.
        assert_eq!(
            serde_json::from_str::<AccessMode>("\"all\"").unwrap(),
            AccessMode::AllowAll
        );
        assert_eq!(
            serde_json::from_str::<AccessMode>("\"wl\"").unwrap(),
            AccessMode::Whitelist
        );
        assert_eq!(
            serde_json::from_str::<AccessMode>("\"prompt\"").unwrap(),
            AccessMode::Interactive
        );
        assert!(serde_json::from_str::<AccessMode>("\"bogus\"").is_err());
        // Default impl.
        assert_eq!(AccessMode::default(), AccessMode::Interactive);
    }

    #[test]
    fn ipv6_private_and_public_ranges() {
        // Unique local fc00::/7.
        let ula: IpAddr = "fc00::1".parse().unwrap();
        assert!(ClientAccessControl::is_private_range(&ula));
        // Link-local fe80::.
        let ll: IpAddr = "fe80::1".parse().unwrap();
        assert!(ClientAccessControl::is_private_range(&ll));
        // Public IPv6.
        let pub6: IpAddr = "2001:4860:4860::8888".parse().unwrap();
        assert!(!ClientAccessControl::is_private_range(&pub6));
    }

    #[test]
    fn parse_ip_or_cidr_errors_on_invalid() {
        let mut ac = ClientAccessControl::with_mode(AccessMode::Whitelist);
        assert!(ac.add_to_whitelist("not-an-ip").is_err());
        assert!(ac.add_to_whitelist("999.999.999.999/8").is_err());
        // IPv6 single address converts to /128 and is accepted.
        ac.add_to_whitelist("::1").unwrap();
        assert!(ac.is_in_whitelist(&"::1".parse::<IpAddr>().unwrap()));
    }

    #[test]
    fn new_skips_invalid_whitelist_entries() {
        let config = AccessControlConfig {
            mode: AccessMode::Whitelist,
            whitelist: vec!["10.0.0.0/8".to_string(), "garbage".to_string()],
            allow_lan: false,
            userpass: None,
        };
        let ac = ClientAccessControl::new(config);
        // Only the valid entry is retained.
        assert_eq!(ac.whitelist_entries().len(), 1);
    }

    #[test]
    fn generation_increments_on_mutations() {
        let mut ac = ClientAccessControl::with_mode(AccessMode::Whitelist);
        let g0 = ac.generation();
        ac.add_to_whitelist("10.0.0.1").unwrap();
        let g1 = ac.generation();
        assert!(g1 > g0);
        ac.set_mode(AccessMode::AllowAll);
        assert!(ac.generation() > g1);
        let g2 = ac.generation();
        ac.set_allow_lan(true);
        assert!(ac.generation() > g2);
        assert!(ac.allow_lan());
        assert_eq!(ac.mode(), AccessMode::AllowAll);
    }

    #[test]
    fn remove_from_whitelist_returns_false_when_absent() {
        let mut ac = ClientAccessControl::with_mode(AccessMode::Whitelist);
        assert!(!ac.remove_from_whitelist("10.0.0.1").unwrap());
        assert!(ac.remove_from_whitelist("bad-ip").is_err());
    }

    #[test]
    fn temporary_and_session_entries_accessors() {
        let ac = ClientAccessControl::with_mode(AccessMode::Interactive);
        let ip: IpAddr = "192.0.2.5".parse().unwrap();

        ac.add_temporary(ip);
        assert!(ac.temporary_whitelist_entries().contains(&ip));
        assert!(ac.is_in_temporary_whitelist(&ip));
        assert!(!ac.remove_temporary(&"192.0.2.99".parse().unwrap()));
        assert!(ac.remove_temporary(&ip));

        ac.deny_session(ip);
        assert!(ac.is_session_denied(&ip));
        assert!(ac.session_denied_entries().contains(&ip));
        assert!(!ac.remove_session_denied(&"192.0.2.99".parse().unwrap()));
        assert!(ac.remove_session_denied(&ip));
        assert!(!ac.is_session_denied(&ip));
    }

    #[test]
    fn pending_authorization_lifecycle() {
        let ac = ClientAccessControl::with_mode(AccessMode::Interactive);
        let mut rx = ac.subscribe();
        let ip: IpAddr = "203.0.113.7".parse().unwrap();

        ac.add_pending_authorization(ip);
        assert_eq!(ac.pending_authorization_count(), 1);
        // Second call for the same IP increments attempt_count, not a new entry.
        ac.add_pending_authorization(ip);
        assert_eq!(ac.pending_authorization_count(), 1);
        let pendings = ac.get_pending_authorizations();
        assert_eq!(pendings.len(), 1);
        assert_eq!(pendings[0].attempt_count, 2);
        assert_eq!(pendings[0].ip, ip.to_string());

        // The "new" event was broadcast once.
        let evt = rx.try_recv().expect("new event");
        assert_eq!(evt.event_type, "new");
        assert_eq!(evt.total_pending, 1);

        // Approve -> moved to temporary whitelist + "approved" event.
        assert!(ac.approve_pending(&ip));
        assert_eq!(ac.pending_authorization_count(), 0);
        assert!(ac.is_in_temporary_whitelist(&ip));
        let evt = rx.try_recv().expect("approved event");
        assert_eq!(evt.event_type, "approved");

        // Approving a non-pending IP returns false.
        assert!(!ac.approve_pending(&"203.0.113.8".parse().unwrap()));
    }

    #[test]
    fn reject_pending_denies_session() {
        let ac = ClientAccessControl::with_mode(AccessMode::Interactive);
        let ip: IpAddr = "203.0.113.9".parse().unwrap();
        ac.add_pending_authorization(ip);
        assert!(ac.reject_pending(&ip));
        assert!(ac.is_session_denied(&ip));
        assert_eq!(ac.pending_authorization_count(), 0);
        assert!(!ac.reject_pending(&ip));
    }

    #[test]
    fn clear_pending_authorizations_empties() {
        let ac = ClientAccessControl::with_mode(AccessMode::Interactive);
        ac.add_pending_authorization("203.0.113.1".parse().unwrap());
        ac.add_pending_authorization("203.0.113.2".parse().unwrap());
        assert_eq!(ac.pending_authorization_count(), 2);
        ac.clear_pending_authorizations();
        assert_eq!(ac.pending_authorization_count(), 0);
    }

    #[test]
    fn userpass_config_get_set_and_status() {
        let ac = ClientAccessControl::with_mode(AccessMode::LocalOnly);
        assert!(ac.userpass_config().is_none());
        // Default status when no config.
        assert_eq!(ac.userpass_status(), UserPassAuthStatus::default());

        let cfg = UserPassAuthConfig {
            enabled: true,
            accounts: vec![
                UserPassAccountConfig {
                    username: "alice".to_string(),
                    password: Some("pw".to_string()),
                    enabled: true,
                },
                UserPassAccountConfig {
                    username: "bob".to_string(),
                    password: None,
                    enabled: false,
                },
            ],
            loopback_requires_auth: true,
        };
        ac.set_userpass_config(Some(cfg));
        assert!(ac.userpass_config().is_some());

        // Record a connection and verify status reflects it + has_password.
        ac.record_userpass_success("alice", 1234);
        let status = ac.userpass_status();
        assert!(status.enabled);
        assert!(status.loopback_requires_auth);
        assert_eq!(status.accounts.len(), 2);
        let alice = status
            .accounts
            .iter()
            .find(|a| a.username == "alice")
            .unwrap();
        assert!(alice.has_password);
        assert_eq!(alice.last_connected_at, Some(1234));
        let bob = status
            .accounts
            .iter()
            .find(|a| a.username == "bob")
            .unwrap();
        assert!(!bob.has_password);
        assert!(!bob.enabled);
    }

    #[test]
    fn set_userpass_last_connected_at_retains_only_known_users() {
        let ac = ClientAccessControl::with_mode(AccessMode::LocalOnly);
        ac.set_userpass_config(Some(UserPassAuthConfig {
            enabled: true,
            accounts: vec![UserPassAccountConfig {
                username: "alice".to_string(),
                password: Some("pw".to_string()),
                enabled: true,
            }],
            loopback_requires_auth: false,
        }));

        let mut map = HashMap::new();
        map.insert("alice".to_string(), 100u64);
        map.insert("ghost".to_string(), 200u64); // not a configured account
        ac.set_userpass_last_connected_at(map);

        let retained = ac.userpass_last_connected_at();
        assert_eq!(retained.get("alice"), Some(&100));
        assert!(!retained.contains_key("ghost"));
    }

    #[test]
    fn verify_userpass_none_when_disabled_config() {
        let ac = ClientAccessControl::with_mode(AccessMode::LocalOnly);
        ac.set_userpass_config(Some(UserPassAuthConfig {
            enabled: false,
            accounts: vec![UserPassAccountConfig {
                username: "alice".to_string(),
                password: Some("pw".to_string()),
                enabled: true,
            }],
            loopback_requires_auth: false,
        }));
        assert_eq!(ac.verify_userpass("alice", "pw"), None);
        // No userpass at all -> None.
        let ac2 = ClientAccessControl::with_mode(AccessMode::LocalOnly);
        assert_eq!(ac2.verify_userpass("x", "y"), None);
    }

    #[test]
    fn rate_limiter_default_and_failure_count_unknown() {
        let limiter = ProxyAuthRateLimiter::default();
        let ip: IpAddr = "198.51.100.7".parse().unwrap();
        assert_eq!(limiter.failure_count(&ip), 0);
        assert!(!limiter.is_banned(&ip));
    }
}
