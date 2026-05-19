use std::collections::{HashMap, HashSet};
use std::net::IpAddr;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use parking_lot::RwLock;
use serde::Serialize;
use tokio::sync::broadcast;
use tracing::{debug, info, warn};

const EVENT_CHANNEL_CAPACITY: usize = 64;
const MIN_SAMPLE_FOR_INFERENCE: u32 = 3;
const HIGH_UNTRUST_RATIO: f32 = 0.7;
const MIN_DEFINITE_FOR_NOT_TRUSTED: u32 = 2;

/// How often (in seconds) the same (client, domain, reason) combo is allowed to log.
const LOG_DEDUP_INTERVAL_SECS: u64 = 60;
/// Maximum entries in the log dedup map before we purge stale entries.
const LOG_DEDUP_MAX_ENTRIES: usize = 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TlsAcceptFailureReason {
    ClientDoesNotTrustCa,
    ProbablyClientDoesNotTrustCa,
    DecryptionError,
    CertificateExpired,
    ProtocolIncompatible,
    ConnectionReset,
    Unknown,
}

impl std::fmt::Display for TlsAcceptFailureReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ClientDoesNotTrustCa => write!(f, "client_does_not_trust_ca"),
            Self::ProbablyClientDoesNotTrustCa => {
                write!(f, "probably_client_does_not_trust_ca")
            }
            Self::DecryptionError => write!(f, "decryption_error"),
            Self::CertificateExpired => write!(f, "certificate_expired"),
            Self::ProtocolIncompatible => write!(f, "protocol_incompatible"),
            Self::ConnectionReset => write!(f, "connection_reset"),
            Self::Unknown => write!(f, "unknown"),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ClientTrustStatus {
    Trusted,
    NotTrusted { reason: String },
    PossiblyNotTrusted { reason: String },
    LikelyUntrusted { confidence: f32, sample_count: u32 },
    Unknown,
}

#[derive(Debug, Clone)]
struct DomainFailureDetail {
    definite_count: u32,
    probable_count: u32,
    last_seen: u64,
}

#[derive(Debug, Clone)]
pub struct ClientTrustRecord {
    pub first_seen: u64,
    pub last_seen: u64,
    pub last_success_at: Option<u64>,
    pub last_failure_at: Option<u64>,
    pub handshake_success: u32,
    pub handshake_fail_definite_untrust: u32,
    pub handshake_fail_probable_untrust: u32,
    pub handshake_fail_other: u32,
    pub last_failure_reason: Option<TlsAcceptFailureReason>,
    pub last_failure_domain: Option<String>,
    failed_domains: HashMap<String, DomainFailureDetail>,
    success_domains: HashSet<String>,
}

impl ClientTrustRecord {
    fn new(now: u64) -> Self {
        Self {
            first_seen: now,
            last_seen: now,
            last_success_at: None,
            last_failure_at: None,
            handshake_success: 0,
            handshake_fail_definite_untrust: 0,
            handshake_fail_probable_untrust: 0,
            handshake_fail_other: 0,
            last_failure_reason: None,
            last_failure_domain: None,
            failed_domains: HashMap::new(),
            success_domains: HashSet::new(),
        }
    }

    fn total_untrust(&self) -> u32 {
        self.handshake_fail_definite_untrust + self.handshake_fail_probable_untrust
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct DomainFailureSummary {
    pub domain: String,
    pub definite_count: u32,
    pub probable_count: u32,
    pub has_success: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ClientTrustSummary {
    pub identifier: String,
    pub identifier_type: String,
    pub trust_status: ClientTrustStatus,
    pub handshake_success: u32,
    pub handshake_fail_untrust: u32,
    pub handshake_fail_definite_untrust: u32,
    pub handshake_fail_probable_untrust: u32,
    pub handshake_fail_other: u32,
    pub first_seen: u64,
    pub last_seen: u64,
    pub last_failure_domain: Option<String>,
    pub last_failure_reason: Option<String>,
    pub failed_domains: Vec<DomainFailureSummary>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ClientTrustEvent {
    pub identifier: String,
    pub identifier_type: String,
    pub old_status: String,
    pub new_status: String,
    pub reason: Option<String>,
    pub domain: Option<String>,
    pub timestamp: u64,
}

/// Tracks per-key log dedup state.
#[derive(Debug)]
struct LogDedupEntry {
    last_logged: Instant,
    suppressed: u64,
}

pub struct ClientTlsTrustTracker {
    by_ip: RwLock<HashMap<IpAddr, ClientTrustRecord>>,
    by_app: RwLock<HashMap<String, ClientTrustRecord>>,
    event_sender: broadcast::Sender<ClientTrustEvent>,
    /// Key: "client_identifier|domain|reason_category"
    log_dedup: RwLock<HashMap<String, LogDedupEntry>>,
}

impl Default for ClientTlsTrustTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl ClientTlsTrustTracker {
    pub fn new() -> Self {
        let (event_sender, _) = broadcast::channel(EVENT_CHANNEL_CAPACITY);
        Self {
            by_ip: RwLock::new(HashMap::new()),
            by_app: RwLock::new(HashMap::new()),
            event_sender,
            log_dedup: RwLock::new(HashMap::new()),
        }
    }

    pub fn record_handshake_success(
        &self,
        client_ip: &str,
        client_app: Option<&str>,
        domain: &str,
    ) {
        let now = current_timestamp();
        let ctx = Some(("handshake_success".to_string(), domain.to_string()));

        if let Ok(ip) = client_ip.parse::<IpAddr>() {
            let mut map = self.by_ip.write();
            let record = map.entry(ip).or_insert_with(|| ClientTrustRecord::new(now));
            let old_status = evaluate_trust(record);
            record.last_seen = now;
            record.last_success_at = Some(now);
            record.handshake_success += 1;
            record.success_domains.insert(domain.to_string());
            let new_status = evaluate_trust(record);
            self.maybe_emit_event(client_ip, "ip", &old_status, &new_status, ctx.clone());
        }

        if let Some(app) = client_app {
            if !app.is_empty() {
                let mut map = self.by_app.write();
                let record = map
                    .entry(app.to_string())
                    .or_insert_with(|| ClientTrustRecord::new(now));
                let old_status = evaluate_trust(record);
                record.last_seen = now;
                record.last_success_at = Some(now);
                record.handshake_success += 1;
                record.success_domains.insert(domain.to_string());
                let new_status = evaluate_trust(record);
                self.maybe_emit_event(app, "app", &old_status, &new_status, ctx.clone());
            }
        }
    }

    pub fn record_handshake_failure(
        &self,
        client_ip: &str,
        client_app: Option<&str>,
        domain: &str,
        reason: &TlsAcceptFailureReason,
    ) {
        let now = current_timestamp();
        let is_definite_untrust = matches!(reason, TlsAcceptFailureReason::ClientDoesNotTrustCa);
        let is_probable_untrust =
            matches!(reason, TlsAcceptFailureReason::ProbablyClientDoesNotTrustCa);

        if let Ok(ip) = client_ip.parse::<IpAddr>() {
            let mut map = self.by_ip.write();
            let record = map.entry(ip).or_insert_with(|| ClientTrustRecord::new(now));
            let old_status = evaluate_trust(record);
            record.last_seen = now;
            record.last_failure_at = Some(now);
            record.last_failure_reason = Some(reason.clone());
            record.last_failure_domain = Some(domain.to_string());
            apply_failure_to_record(
                record,
                domain,
                is_definite_untrust,
                is_probable_untrust,
                now,
            );
            let new_status = evaluate_trust(record);
            self.maybe_emit_event(
                client_ip,
                "ip",
                &old_status,
                &new_status,
                Some((reason.to_string(), domain.to_string())),
            );
        }

        if let Some(app) = client_app {
            if !app.is_empty() {
                let mut map = self.by_app.write();
                let record = map
                    .entry(app.to_string())
                    .or_insert_with(|| ClientTrustRecord::new(now));
                let old_status = evaluate_trust(record);
                record.last_seen = now;
                record.last_failure_at = Some(now);
                record.last_failure_reason = Some(reason.clone());
                record.last_failure_domain = Some(domain.to_string());
                apply_failure_to_record(
                    record,
                    domain,
                    is_definite_untrust,
                    is_probable_untrust,
                    now,
                );
                let new_status = evaluate_trust(record);
                self.maybe_emit_event(
                    app,
                    "app",
                    &old_status,
                    &new_status,
                    Some((reason.to_string(), domain.to_string())),
                );
            }
        }

        // Rate-limited logging to avoid spam
        let dedup_key = format!("{}|{}|{}", client_app.unwrap_or(client_ip), domain, reason);

        if is_definite_untrust {
            if let Some(suppressed) = self.check_log_dedup(&dedup_key) {
                if suppressed > 0 {
                    warn!(
                        client_ip = %client_ip,
                        client_app = ?client_app,
                        domain = %domain,
                        reason = %reason,
                        suppressed_count = suppressed,
                        "TLS trust: client sent UnknownCA alert — definitively does not trust Bifrost CA (repeated)"
                    );
                } else {
                    warn!(
                        client_ip = %client_ip,
                        client_app = ?client_app,
                        domain = %domain,
                        reason = %reason,
                        "TLS trust: client sent UnknownCA alert — definitively does not trust Bifrost CA"
                    );
                }
            }
        } else if is_probable_untrust {
            if let Some(suppressed) = self.check_log_dedup(&dedup_key) {
                if suppressed > 0 {
                    info!(
                        client_ip = %client_ip,
                        client_app = ?client_app,
                        domain = %domain,
                        reason = %reason,
                        suppressed_count = suppressed,
                        "TLS trust: ambiguous certificate rejection (BadCertificate/CertificateUnknown) — may or may not be CA trust issue (repeated)"
                    );
                } else {
                    info!(
                        client_ip = %client_ip,
                        client_app = ?client_app,
                        domain = %domain,
                        reason = %reason,
                        "TLS trust: ambiguous certificate rejection (BadCertificate/CertificateUnknown) — may or may not be CA trust issue"
                    );
                }
            }
        } else {
            debug!(
                client_ip = %client_ip,
                client_app = ?client_app,
                domain = %domain,
                reason = %reason,
                "TLS handshake failure (not trust-related)"
            );
        }
    }

    pub fn get_trust_status_by_ip(&self, ip: &IpAddr) -> ClientTrustStatus {
        let map = self.by_ip.read();
        match map.get(ip) {
            Some(record) => evaluate_trust(record),
            None => ClientTrustStatus::Unknown,
        }
    }

    pub fn get_trust_status_by_app(&self, app: &str) -> ClientTrustStatus {
        let map = self.by_app.read();
        match map.get(app) {
            Some(record) => evaluate_trust(record),
            None => ClientTrustStatus::Unknown,
        }
    }

    pub fn get_all_statuses(&self) -> Vec<ClientTrustSummary> {
        let mut results = Vec::new();

        {
            let map = self.by_ip.read();
            for (ip, record) in map.iter() {
                results.push(build_summary(ip.to_string(), "ip".to_string(), record));
            }
        }

        {
            let map = self.by_app.read();
            for (app, record) in map.iter() {
                results.push(build_summary(app.clone(), "app".to_string(), record));
            }
        }

        results.sort_by_key(|a| std::cmp::Reverse(a.last_seen));
        results
    }

    pub fn get_untrusted_count(&self) -> usize {
        let mut count = 0;

        {
            let map = self.by_ip.read();
            for record in map.values() {
                if matches!(
                    evaluate_trust(record),
                    ClientTrustStatus::NotTrusted { .. }
                        | ClientTrustStatus::LikelyUntrusted { .. }
                ) {
                    count += 1;
                }
            }
        }

        {
            let map = self.by_app.read();
            for record in map.values() {
                if matches!(
                    evaluate_trust(record),
                    ClientTrustStatus::NotTrusted { .. }
                        | ClientTrustStatus::LikelyUntrusted { .. }
                ) {
                    count += 1;
                }
            }
        }

        count
    }

    pub fn subscribe(&self) -> broadcast::Receiver<ClientTrustEvent> {
        self.event_sender.subscribe()
    }

    pub fn clear(&self) {
        self.by_ip.write().clear();
        self.by_app.write().clear();
        self.log_dedup.write().clear();
        info!("Client trust tracker cleared");
    }

    /// Returns `Some(suppressed_count)` if the log should be emitted now,
    /// or `None` if it should be suppressed (within dedup window).
    fn check_log_dedup(&self, key: &str) -> Option<u64> {
        let now = Instant::now();
        let mut map = self.log_dedup.write();
        let interval = std::time::Duration::from_secs(LOG_DEDUP_INTERVAL_SECS);

        // Purge stale entries if map grows too large
        if map.len() > LOG_DEDUP_MAX_ENTRIES {
            map.retain(|_, entry| now.duration_since(entry.last_logged) < interval);
        }

        match map.get_mut(key) {
            Some(entry) => {
                if now.duration_since(entry.last_logged) >= interval {
                    let suppressed = entry.suppressed;
                    entry.last_logged = now;
                    entry.suppressed = 0;
                    Some(suppressed)
                } else {
                    entry.suppressed += 1;
                    None
                }
            }
            None => {
                map.insert(
                    key.to_string(),
                    LogDedupEntry {
                        last_logged: now,
                        suppressed: 0,
                    },
                );
                Some(0)
            }
        }
    }

    fn maybe_emit_event(
        &self,
        identifier: &str,
        identifier_type: &str,
        old_status: &ClientTrustStatus,
        new_status: &ClientTrustStatus,
        event_ctx: Option<(String, String)>,
    ) {
        let old_tag = status_tag(old_status);
        let new_tag = status_tag(new_status);
        if old_tag != new_tag {
            let (reason, domain) = event_ctx.unzip();
            let now = current_timestamp();
            let event = ClientTrustEvent {
                identifier: identifier.to_string(),
                identifier_type: identifier_type.to_string(),
                old_status: old_tag.to_string(),
                new_status: new_tag.to_string(),
                reason,
                domain,
                timestamp: now,
            };
            let _ = self.event_sender.send(event);
        }
    }
}

fn apply_failure_to_record(
    record: &mut ClientTrustRecord,
    domain: &str,
    is_definite: bool,
    is_probable: bool,
    now: u64,
) {
    if is_definite {
        record.handshake_fail_definite_untrust += 1;
        let entry =
            record
                .failed_domains
                .entry(domain.to_string())
                .or_insert(DomainFailureDetail {
                    definite_count: 0,
                    probable_count: 0,
                    last_seen: now,
                });
        entry.definite_count += 1;
        entry.last_seen = now;
    } else if is_probable {
        record.handshake_fail_probable_untrust += 1;
        let entry =
            record
                .failed_domains
                .entry(domain.to_string())
                .or_insert(DomainFailureDetail {
                    definite_count: 0,
                    probable_count: 0,
                    last_seen: now,
                });
        entry.probable_count += 1;
        entry.last_seen = now;
    } else {
        record.handshake_fail_other += 1;
    }
}

fn build_summary(
    identifier: String,
    identifier_type: String,
    record: &ClientTrustRecord,
) -> ClientTrustSummary {
    let failed_domains: Vec<DomainFailureSummary> = record
        .failed_domains
        .iter()
        .map(|(domain, detail)| DomainFailureSummary {
            domain: domain.clone(),
            definite_count: detail.definite_count,
            probable_count: detail.probable_count,
            has_success: record.success_domains.contains(domain),
        })
        .collect();

    ClientTrustSummary {
        identifier,
        identifier_type,
        trust_status: evaluate_trust(record),
        handshake_success: record.handshake_success,
        handshake_fail_untrust: record.total_untrust(),
        handshake_fail_definite_untrust: record.handshake_fail_definite_untrust,
        handshake_fail_probable_untrust: record.handshake_fail_probable_untrust,
        handshake_fail_other: record.handshake_fail_other,
        first_seen: record.first_seen,
        last_seen: record.last_seen,
        last_failure_domain: record.last_failure_domain.clone(),
        last_failure_reason: record.last_failure_reason.as_ref().map(|r| r.to_string()),
        failed_domains,
    }
}

pub fn classify_tls_accept_error(error: &std::io::Error) -> TlsAcceptFailureReason {
    let msg = error.to_string();
    let lower = msg.to_ascii_lowercase();

    if lower.contains("unknownca") || lower.contains("unknown_ca") || lower.contains("unknown ca") {
        return TlsAcceptFailureReason::ClientDoesNotTrustCa;
    }

    if lower.contains("badcertificate")
        || lower.contains("bad_certificate")
        || lower.contains("bad certificate")
    {
        return TlsAcceptFailureReason::ProbablyClientDoesNotTrustCa;
    }
    if lower.contains("certificateunknown")
        || lower.contains("certificate_unknown")
        || lower.contains("certificate unknown")
    {
        return TlsAcceptFailureReason::ProbablyClientDoesNotTrustCa;
    }

    if lower.contains("decrypt") {
        return TlsAcceptFailureReason::DecryptionError;
    }

    if lower.contains("certificateexpired") || lower.contains("certificate expired") {
        return TlsAcceptFailureReason::CertificateExpired;
    }

    if lower.contains("handshakefailure") || lower.contains("protocolversion") {
        return TlsAcceptFailureReason::ProtocolIncompatible;
    }

    if lower.contains("connection reset")
        || lower.contains("broken pipe")
        || lower.contains("unexpected eof")
    {
        return TlsAcceptFailureReason::ConnectionReset;
    }

    TlsAcceptFailureReason::Unknown
}

fn evaluate_trust(record: &ClientTrustRecord) -> ClientTrustStatus {
    let total = record.handshake_success
        + record.handshake_fail_definite_untrust
        + record.handshake_fail_probable_untrust
        + record.handshake_fail_other;

    if total == 0 {
        return ClientTrustStatus::Unknown;
    }

    let definite = record.handshake_fail_definite_untrust;
    let total_untrust = record.total_untrust();

    if definite >= MIN_DEFINITE_FOR_NOT_TRUSTED && record.handshake_success == 0 {
        return ClientTrustStatus::NotTrusted {
            reason: record
                .last_failure_reason
                .as_ref()
                .map(|r| r.to_string())
                .unwrap_or_else(|| "unknown".to_string()),
        };
    }

    if definite >= 1 && record.handshake_success == 0 {
        return ClientTrustStatus::PossiblyNotTrusted {
            reason: record
                .last_failure_reason
                .as_ref()
                .map(|r| r.to_string())
                .unwrap_or_else(|| "unknown".to_string()),
        };
    }

    if total_untrust > 0 && record.handshake_success > 0 {
        if let (Some(last_success), Some(last_failure)) =
            (record.last_success_at, record.last_failure_at)
        {
            if last_success > last_failure {
                return ClientTrustStatus::Trusted;
            }
        }
    }

    if total_untrust == 0 && record.handshake_success > 0 {
        return ClientTrustStatus::Trusted;
    }

    if total >= MIN_SAMPLE_FOR_INFERENCE {
        let fail_ratio = total_untrust as f32 / total as f32;
        if fail_ratio > HIGH_UNTRUST_RATIO {
            return ClientTrustStatus::LikelyUntrusted {
                confidence: fail_ratio,
                sample_count: total,
            };
        }
    }

    ClientTrustStatus::Unknown
}

fn status_tag(status: &ClientTrustStatus) -> &'static str {
    match status {
        ClientTrustStatus::Trusted => "trusted",
        ClientTrustStatus::NotTrusted { .. } => "not_trusted",
        ClientTrustStatus::PossiblyNotTrusted { .. } => "possibly_not_trusted",
        ClientTrustStatus::LikelyUntrusted { .. } => "likely_untrusted",
        ClientTrustStatus::Unknown => "unknown",
    }
}

fn current_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_classify_unknown_ca_is_definite() {
        let err = std::io::Error::other("peer sent fatal alert: UnknownCA");
        assert_eq!(
            classify_tls_accept_error(&err),
            TlsAcceptFailureReason::ClientDoesNotTrustCa
        );
    }

    #[test]
    fn test_classify_bad_certificate_is_probable() {
        let err = std::io::Error::other("peer sent fatal alert: BadCertificate");
        assert_eq!(
            classify_tls_accept_error(&err),
            TlsAcceptFailureReason::ProbablyClientDoesNotTrustCa
        );
    }

    #[test]
    fn test_classify_certificate_unknown_is_probable() {
        let err = std::io::Error::other("peer sent fatal alert: CertificateUnknown");
        assert_eq!(
            classify_tls_accept_error(&err),
            TlsAcceptFailureReason::ProbablyClientDoesNotTrustCa
        );
    }

    #[test]
    fn test_classify_decrypt_is_not_untrust() {
        let err = std::io::Error::other("decrypt error");
        assert_eq!(
            classify_tls_accept_error(&err),
            TlsAcceptFailureReason::DecryptionError
        );
    }

    #[test]
    fn test_classify_connection_reset() {
        let err = std::io::Error::other("connection reset by peer");
        assert_eq!(
            classify_tls_accept_error(&err),
            TlsAcceptFailureReason::ConnectionReset
        );
    }

    #[test]
    fn test_classify_unexpected_eof() {
        let err = std::io::Error::other("unexpected eof");
        assert_eq!(
            classify_tls_accept_error(&err),
            TlsAcceptFailureReason::ConnectionReset
        );
    }

    #[test]
    fn test_classify_unknown() {
        let err = std::io::Error::other("some random error");
        assert_eq!(
            classify_tls_accept_error(&err),
            TlsAcceptFailureReason::Unknown
        );
    }

    #[test]
    fn test_classify_handshake_failure() {
        let err = std::io::Error::other("HandshakeFailure");
        assert_eq!(
            classify_tls_accept_error(&err),
            TlsAcceptFailureReason::ProtocolIncompatible
        );
    }

    #[test]
    fn test_classify_certificate_expired() {
        let err = std::io::Error::other("CertificateExpired");
        assert_eq!(
            classify_tls_accept_error(&err),
            TlsAcceptFailureReason::CertificateExpired
        );
    }

    #[test]
    fn test_evaluate_trust_no_data() {
        let record = ClientTrustRecord::new(100);
        assert!(matches!(
            evaluate_trust(&record),
            ClientTrustStatus::Unknown
        ));
    }

    #[test]
    fn test_evaluate_trust_all_success() {
        let mut record = ClientTrustRecord::new(100);
        record.handshake_success = 5;
        assert!(matches!(
            evaluate_trust(&record),
            ClientTrustStatus::Trusted
        ));
    }

    #[test]
    fn test_evaluate_single_definite_failure_is_possibly_not_trusted() {
        let mut record = ClientTrustRecord::new(100);
        record.handshake_fail_definite_untrust = 1;
        record.last_failure_reason = Some(TlsAcceptFailureReason::ClientDoesNotTrustCa);
        assert!(matches!(
            evaluate_trust(&record),
            ClientTrustStatus::PossiblyNotTrusted { .. }
        ));
    }

    #[test]
    fn test_evaluate_multiple_definite_failures_is_not_trusted() {
        let mut record = ClientTrustRecord::new(100);
        record.handshake_fail_definite_untrust = 2;
        record.last_failure_reason = Some(TlsAcceptFailureReason::ClientDoesNotTrustCa);
        assert!(matches!(
            evaluate_trust(&record),
            ClientTrustStatus::NotTrusted { .. }
        ));
    }

    #[test]
    fn test_evaluate_probable_only_is_not_immediately_untrusted() {
        let mut record = ClientTrustRecord::new(100);
        record.handshake_fail_probable_untrust = 2;
        record.last_failure_reason = Some(TlsAcceptFailureReason::ProbablyClientDoesNotTrustCa);
        assert!(matches!(
            evaluate_trust(&record),
            ClientTrustStatus::Unknown
        ));
    }

    #[test]
    fn test_evaluate_probable_enough_samples_becomes_likely_untrusted() {
        let mut record = ClientTrustRecord::new(100);
        record.handshake_fail_probable_untrust = 3;
        record.last_failure_reason = Some(TlsAcceptFailureReason::ProbablyClientDoesNotTrustCa);
        let status = evaluate_trust(&record);
        assert!(matches!(status, ClientTrustStatus::LikelyUntrusted { .. }));
    }

    #[test]
    fn test_evaluate_decrypt_error_alone_stays_unknown() {
        let mut record = ClientTrustRecord::new(100);
        record.handshake_fail_other = 5;
        record.last_failure_reason = Some(TlsAcceptFailureReason::DecryptionError);
        assert!(matches!(
            evaluate_trust(&record),
            ClientTrustStatus::Unknown
        ));
    }

    #[test]
    fn test_evaluate_trust_recovered() {
        let mut record = ClientTrustRecord::new(100);
        record.handshake_fail_definite_untrust = 3;
        record.handshake_success = 2;
        record.last_failure_at = Some(100);
        record.last_success_at = Some(200);
        assert!(matches!(
            evaluate_trust(&record),
            ClientTrustStatus::Trusted
        ));
    }

    #[test]
    fn test_evaluate_mixed_below_ratio_threshold() {
        let mut record = ClientTrustRecord::new(100);
        record.handshake_fail_probable_untrust = 1;
        record.handshake_success = 2;
        record.last_failure_at = Some(200);
        record.last_success_at = Some(100);
        assert!(matches!(
            evaluate_trust(&record),
            ClientTrustStatus::Unknown
        ));
    }

    #[test]
    fn test_tracker_single_definite_failure() {
        let tracker = ClientTlsTrustTracker::new();
        let mut rx = tracker.subscribe();

        tracker.record_handshake_failure(
            "192.168.1.100",
            Some("Firefox"),
            "example.com",
            &TlsAcceptFailureReason::ClientDoesNotTrustCa,
        );

        let ip: IpAddr = "192.168.1.100".parse().unwrap();
        assert!(matches!(
            tracker.get_trust_status_by_ip(&ip),
            ClientTrustStatus::PossiblyNotTrusted { .. }
        ));

        let event = rx.try_recv().unwrap();
        assert_eq!(event.new_status, "possibly_not_trusted");
    }

    #[test]
    fn test_tracker_double_definite_failure_promotes_to_not_trusted() {
        let tracker = ClientTlsTrustTracker::new();

        tracker.record_handshake_failure(
            "192.168.1.100",
            Some("Firefox"),
            "example.com",
            &TlsAcceptFailureReason::ClientDoesNotTrustCa,
        );
        tracker.record_handshake_failure(
            "192.168.1.100",
            Some("Firefox"),
            "github.com",
            &TlsAcceptFailureReason::ClientDoesNotTrustCa,
        );

        let ip: IpAddr = "192.168.1.100".parse().unwrap();
        assert!(matches!(
            tracker.get_trust_status_by_ip(&ip),
            ClientTrustStatus::NotTrusted { .. }
        ));
        assert!(matches!(
            tracker.get_trust_status_by_app("Firefox"),
            ClientTrustStatus::NotTrusted { .. }
        ));

        let statuses = tracker.get_all_statuses();
        assert_eq!(statuses.len(), 2);
        assert_eq!(tracker.get_untrusted_count(), 2);
    }

    #[test]
    fn test_tracker_decrypt_error_does_not_trigger_untrust() {
        let tracker = ClientTlsTrustTracker::new();

        tracker.record_handshake_failure(
            "10.0.0.1",
            None,
            "example.com",
            &TlsAcceptFailureReason::DecryptionError,
        );

        let ip: IpAddr = "10.0.0.1".parse().unwrap();
        assert!(matches!(
            tracker.get_trust_status_by_ip(&ip),
            ClientTrustStatus::Unknown
        ));
        assert_eq!(tracker.get_untrusted_count(), 0);
    }

    #[test]
    fn test_tracker_probable_failures_need_samples_for_likely_untrusted() {
        let tracker = ClientTlsTrustTracker::new();

        tracker.record_handshake_failure(
            "10.0.0.1",
            None,
            "a.com",
            &TlsAcceptFailureReason::ProbablyClientDoesNotTrustCa,
        );
        tracker.record_handshake_failure(
            "10.0.0.1",
            None,
            "b.com",
            &TlsAcceptFailureReason::ProbablyClientDoesNotTrustCa,
        );

        let ip: IpAddr = "10.0.0.1".parse().unwrap();
        assert!(matches!(
            tracker.get_trust_status_by_ip(&ip),
            ClientTrustStatus::Unknown
        ));

        tracker.record_handshake_failure(
            "10.0.0.1",
            None,
            "c.com",
            &TlsAcceptFailureReason::ProbablyClientDoesNotTrustCa,
        );
        assert!(matches!(
            tracker.get_trust_status_by_ip(&ip),
            ClientTrustStatus::LikelyUntrusted { .. }
        ));
    }

    #[test]
    fn test_tracker_event_emission_on_status_change() {
        let tracker = ClientTlsTrustTracker::new();
        let mut rx = tracker.subscribe();

        tracker.record_handshake_failure(
            "10.0.0.1",
            None,
            "example.com",
            &TlsAcceptFailureReason::ClientDoesNotTrustCa,
        );

        let event = rx.try_recv();
        assert!(event.is_ok());
        let event = event.unwrap();
        assert_eq!(event.identifier, "10.0.0.1");
        assert_eq!(event.old_status, "unknown");
        assert_eq!(event.new_status, "possibly_not_trusted");
    }

    #[test]
    fn test_tracker_event_on_promotion_to_not_trusted() {
        let tracker = ClientTlsTrustTracker::new();
        let mut rx = tracker.subscribe();

        tracker.record_handshake_failure(
            "10.0.0.1",
            None,
            "a.com",
            &TlsAcceptFailureReason::ClientDoesNotTrustCa,
        );
        let _ = rx.try_recv();

        tracker.record_handshake_failure(
            "10.0.0.1",
            None,
            "b.com",
            &TlsAcceptFailureReason::ClientDoesNotTrustCa,
        );
        let event = rx.try_recv().unwrap();
        assert_eq!(event.old_status, "possibly_not_trusted");
        assert_eq!(event.new_status, "not_trusted");
    }

    #[test]
    fn test_tracker_clear() {
        let tracker = ClientTlsTrustTracker::new();

        tracker.record_handshake_success("10.0.0.1", Some("Chrome"), "example.com");
        assert_eq!(tracker.get_all_statuses().len(), 2);

        tracker.clear();
        assert_eq!(tracker.get_all_statuses().len(), 0);
    }

    #[test]
    fn test_domain_failure_tracking_in_summary() {
        let tracker = ClientTlsTrustTracker::new();

        tracker.record_handshake_failure(
            "10.0.0.1",
            None,
            "pinned.example.com",
            &TlsAcceptFailureReason::ClientDoesNotTrustCa,
        );
        tracker.record_handshake_failure(
            "10.0.0.1",
            None,
            "pinned.example.com",
            &TlsAcceptFailureReason::ClientDoesNotTrustCa,
        );
        tracker.record_handshake_success("10.0.0.1", None, "other.example.com");

        let statuses = tracker.get_all_statuses();
        let ip_summary = statuses
            .iter()
            .find(|s| s.identifier == "10.0.0.1")
            .unwrap();

        assert_eq!(ip_summary.failed_domains.len(), 1);
        assert_eq!(ip_summary.failed_domains[0].domain, "pinned.example.com");
        assert_eq!(ip_summary.failed_domains[0].definite_count, 2);
        assert!(!ip_summary.failed_domains[0].has_success);
    }

    #[test]
    fn test_domain_with_both_success_and_failure() {
        let tracker = ClientTlsTrustTracker::new();

        tracker.record_handshake_failure(
            "10.0.0.1",
            None,
            "flaky.com",
            &TlsAcceptFailureReason::ProbablyClientDoesNotTrustCa,
        );
        tracker.record_handshake_success("10.0.0.1", None, "flaky.com");

        let statuses = tracker.get_all_statuses();
        let ip_summary = statuses
            .iter()
            .find(|s| s.identifier == "10.0.0.1")
            .unwrap();

        assert_eq!(ip_summary.failed_domains.len(), 1);
        assert!(ip_summary.failed_domains[0].has_success);
    }

    #[test]
    fn test_summary_backward_compat_fields() {
        let tracker = ClientTlsTrustTracker::new();

        tracker.record_handshake_failure(
            "10.0.0.1",
            None,
            "a.com",
            &TlsAcceptFailureReason::ClientDoesNotTrustCa,
        );
        tracker.record_handshake_failure(
            "10.0.0.1",
            None,
            "b.com",
            &TlsAcceptFailureReason::ProbablyClientDoesNotTrustCa,
        );

        let statuses = tracker.get_all_statuses();
        let s = statuses
            .iter()
            .find(|s| s.identifier == "10.0.0.1")
            .unwrap();

        assert_eq!(s.handshake_fail_definite_untrust, 1);
        assert_eq!(s.handshake_fail_probable_untrust, 1);
        assert_eq!(s.handshake_fail_untrust, 2);
    }
}
