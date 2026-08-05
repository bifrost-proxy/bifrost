use std::collections::HashMap;

use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use crate::traffic::TrafficRecord;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HostMetricsAggregate {
    pub host: String,
    pub requests: u64,
    pub bytes_sent: u64,
    pub bytes_received: u64,
    pub http_requests: u64,
    pub https_requests: u64,
    pub tunnel_requests: u64,
    pub ws_requests: u64,
    pub wss_requests: u64,
    pub h3_requests: u64,
    pub socks5_requests: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AppMetricsAggregate {
    pub app_name: String,
    pub requests: u64,
    pub bytes_sent: u64,
    pub bytes_received: u64,
    pub http_requests: u64,
    pub https_requests: u64,
    pub tunnel_requests: u64,
    pub ws_requests: u64,
    pub wss_requests: u64,
    pub h3_requests: u64,
    pub socks5_requests: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct TrafficMetricsBucket {
    requests: u64,
    dimension_requests: u64,
    bytes_sent: u64,
    bytes_received: u64,
    http_requests: u64,
    https_requests: u64,
    tunnel_requests: u64,
    ws_requests: u64,
    wss_requests: u64,
    h3_requests: u64,
    socks5_requests: u64,
}

impl TrafficMetricsBucket {
    fn add(&mut self, dimensions: &TrafficStatisticsDimensions, include_dimension: bool) {
        self.requests = self.requests.saturating_add(1);
        if include_dimension {
            self.dimension_requests = self.dimension_requests.saturating_add(1);
        }
        self.bytes_sent = self.bytes_sent.saturating_add(dimensions.bytes_sent);
        self.bytes_received = self
            .bytes_received
            .saturating_add(dimensions.bytes_received);
        if let Some(counter) = self.protocol_counter_mut(&dimensions.protocol) {
            *counter = counter.saturating_add(1);
        }
    }

    fn subtract(&mut self, dimensions: &TrafficStatisticsDimensions, include_dimension: bool) {
        self.requests = self.requests.saturating_sub(1);
        if include_dimension {
            self.dimension_requests = self.dimension_requests.saturating_sub(1);
        }
        self.bytes_sent = self.bytes_sent.saturating_sub(dimensions.bytes_sent);
        self.bytes_received = self
            .bytes_received
            .saturating_sub(dimensions.bytes_received);
        if let Some(counter) = self.protocol_counter_mut(&dimensions.protocol) {
            *counter = counter.saturating_sub(1);
        }
    }

    fn protocol_counter_mut(&mut self, protocol: &str) -> Option<&mut u64> {
        match protocol {
            "http" => Some(&mut self.http_requests),
            "https" => Some(&mut self.https_requests),
            "tunnel" => Some(&mut self.tunnel_requests),
            "ws" => Some(&mut self.ws_requests),
            "wss" => Some(&mut self.wss_requests),
            "h3" => Some(&mut self.h3_requests),
            "socks5" => Some(&mut self.socks5_requests),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrafficStatisticsSnapshot {
    pub total_requests: u64,
    pub server_sequence: u64,
    pub client_ips: HashMap<String, u64>,
    pub proxy_ports: HashMap<String, u64>,
    pub applications: HashMap<String, u64>,
    pub account_names: HashMap<String, u64>,
    pub domains: HashMap<String, u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct TrafficStatisticsDimensions {
    client_ip: Option<String>,
    proxy_port: Option<String>,
    application: Option<String>,
    account_name: Option<String>,
    domain: Option<String>,
    bytes_sent: u64,
    bytes_received: u64,
    protocol: String,
}

impl TrafficStatisticsDimensions {
    pub(crate) fn from_record(record: &TrafficRecord) -> Self {
        Self {
            client_ip: non_empty(record.client_ip.clone()),
            proxy_port: (record.listener_port > 0).then(|| record.listener_port.to_string()),
            application: record.client_app.clone().and_then(non_empty),
            account_name: record.account_name.clone().and_then(non_empty),
            domain: non_empty(record.host.clone()),
            bytes_sent: trusted_upload_bytes(record) as u64,
            bytes_received: trusted_download_bytes(record) as u64,
            protocol: record.protocol.to_ascii_lowercase(),
        }
    }

    pub(crate) fn load_by_id(conn: &Connection, id: &str) -> Option<Self> {
        conn.query_row(
            "SELECT client_ip, listener_port, client_app, account_name, host, \
                    upload_bytes, download_bytes, protocol \
             FROM traffic_records WHERE id = ?1",
            [id],
            |row| {
                let client_ip: String = row.get(0)?;
                let listener_port = row.get::<_, i64>(1)? as u16;
                let application: Option<String> = row.get(2)?;
                let account_name: Option<String> = row.get(3)?;
                let domain: String = row.get(4)?;
                let bytes_sent = row.get::<_, i64>(5)?.max(0) as u64;
                let bytes_received = row.get::<_, i64>(6)?.max(0) as u64;
                let protocol: String = row.get(7)?;
                Ok(Self {
                    client_ip: non_empty(client_ip),
                    proxy_port: (listener_port > 0).then(|| listener_port.to_string()),
                    application: application.and_then(non_empty),
                    account_name: account_name.and_then(non_empty),
                    domain: non_empty(domain),
                    bytes_sent,
                    bytes_received,
                    protocol: protocol.to_ascii_lowercase(),
                })
            },
        )
        .ok()
    }

    pub(crate) fn from_persisted_update(
        record: &TrafficRecord,
        previous: &TrafficStatisticsDimensions,
    ) -> Self {
        let mut next = Self::from_record(record);
        // persist_update does not rewrite these immutable columns. Keep the
        // in-memory aggregate aligned with what SQLite actually stores even
        // if a caller mutates them in its update closure.
        next.client_ip = previous.client_ip.clone();
        next.domain = previous.domain.clone();
        next.protocol = previous.protocol.clone();
        next
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct TrafficStatistics {
    total_requests: u64,
    client_ips: HashMap<String, u64>,
    proxy_ports: HashMap<String, u64>,
    account_names: HashMap<String, u64>,
    application_metrics: HashMap<String, TrafficMetricsBucket>,
    host_metrics: HashMap<String, TrafficMetricsBucket>,
}

impl TrafficStatistics {
    pub(crate) fn load(conn: &Connection) -> Self {
        let mut statistics = Self::default();
        let mut statement = match conn.prepare(
            "SELECT client_ip, listener_port, client_app, account_name, host, \
                    upload_bytes, download_bytes, protocol FROM traffic_records",
        ) {
            Ok(statement) => statement,
            Err(error) => {
                tracing::warn!(
                    error = %error,
                    "[TRAFFIC_DB] Failed to initialize in-memory traffic statistics"
                );
                return statistics;
            }
        };

        let Ok(rows) = statement.query_map([], |row| {
            let client_ip: String = row.get(0)?;
            let listener_port = row.get::<_, i64>(1)? as u16;
            let application: Option<String> = row.get(2)?;
            let account_name: Option<String> = row.get(3)?;
            let domain: String = row.get(4)?;
            let bytes_sent = row.get::<_, i64>(5)?.max(0) as u64;
            let bytes_received = row.get::<_, i64>(6)?.max(0) as u64;
            let protocol: String = row.get(7)?;
            Ok(TrafficStatisticsDimensions {
                client_ip: non_empty(client_ip),
                proxy_port: (listener_port > 0).then(|| listener_port.to_string()),
                application: application.and_then(non_empty),
                account_name: account_name.and_then(non_empty),
                domain: non_empty(domain),
                bytes_sent,
                bytes_received,
                protocol: protocol.to_ascii_lowercase(),
            })
        }) else {
            return statistics;
        };

        for dimensions in rows.flatten() {
            statistics.insert(&dimensions);
        }
        statistics
    }

    pub(crate) fn insert(&mut self, dimensions: &TrafficStatisticsDimensions) {
        self.total_requests = self.total_requests.saturating_add(1);
        Self::increment(&mut self.client_ips, dimensions.client_ip.as_deref());
        Self::increment(&mut self.proxy_ports, dimensions.proxy_port.as_deref());
        Self::increment(&mut self.account_names, dimensions.account_name.as_deref());
        Self::add_metrics_bucket(
            &mut self.application_metrics,
            dimensions.application.as_deref(),
            dimensions,
        );
        Self::add_metrics_bucket(
            &mut self.host_metrics,
            dimensions.domain.as_deref(),
            dimensions,
        );
    }

    pub(crate) fn remove(&mut self, dimensions: &TrafficStatisticsDimensions) {
        self.total_requests = self.total_requests.saturating_sub(1);
        Self::decrement(&mut self.client_ips, dimensions.client_ip.as_deref());
        Self::decrement(&mut self.proxy_ports, dimensions.proxy_port.as_deref());
        Self::decrement(&mut self.account_names, dimensions.account_name.as_deref());
        Self::subtract_metrics_bucket(
            &mut self.application_metrics,
            dimensions.application.as_deref(),
            dimensions,
        );
        Self::subtract_metrics_bucket(
            &mut self.host_metrics,
            dimensions.domain.as_deref(),
            dimensions,
        );
    }

    pub(crate) fn replace(
        &mut self,
        previous: &TrafficStatisticsDimensions,
        next: &TrafficStatisticsDimensions,
    ) {
        Self::decrement(&mut self.client_ips, previous.client_ip.as_deref());
        Self::decrement(&mut self.proxy_ports, previous.proxy_port.as_deref());
        Self::decrement(&mut self.account_names, previous.account_name.as_deref());
        Self::subtract_metrics_bucket(
            &mut self.application_metrics,
            previous.application.as_deref(),
            previous,
        );
        Self::subtract_metrics_bucket(&mut self.host_metrics, previous.domain.as_deref(), previous);

        Self::increment(&mut self.client_ips, next.client_ip.as_deref());
        Self::increment(&mut self.proxy_ports, next.proxy_port.as_deref());
        Self::increment(&mut self.account_names, next.account_name.as_deref());
        Self::add_metrics_bucket(
            &mut self.application_metrics,
            next.application.as_deref(),
            next,
        );
        Self::add_metrics_bucket(&mut self.host_metrics, next.domain.as_deref(), next);
    }

    pub(crate) fn snapshot(&self, server_sequence: u64) -> TrafficStatisticsSnapshot {
        TrafficStatisticsSnapshot {
            total_requests: self.total_requests,
            server_sequence,
            client_ips: self.client_ips.clone(),
            proxy_ports: self.proxy_ports.clone(),
            applications: self
                .application_metrics
                .iter()
                .filter(|(_, metrics)| metrics.dimension_requests > 0)
                .map(|(name, metrics)| (name.clone(), metrics.dimension_requests))
                .collect(),
            account_names: self.account_names.clone(),
            domains: self
                .host_metrics
                .iter()
                .filter(|(_, metrics)| metrics.dimension_requests > 0)
                .map(|(host, metrics)| (host.clone(), metrics.dimension_requests))
                .collect(),
        }
    }

    pub(crate) fn app_metrics(&self) -> Vec<AppMetricsAggregate> {
        self.application_metrics
            .iter()
            .map(|(app_name, metrics)| AppMetricsAggregate {
                app_name: app_name.clone(),
                requests: metrics.requests,
                bytes_sent: metrics.bytes_sent,
                bytes_received: metrics.bytes_received,
                http_requests: metrics.http_requests,
                https_requests: metrics.https_requests,
                tunnel_requests: metrics.tunnel_requests,
                ws_requests: metrics.ws_requests,
                wss_requests: metrics.wss_requests,
                h3_requests: metrics.h3_requests,
                socks5_requests: metrics.socks5_requests,
            })
            .collect()
    }

    pub(crate) fn host_metrics(&self) -> Vec<HostMetricsAggregate> {
        self.host_metrics
            .iter()
            .map(|(host, metrics)| HostMetricsAggregate {
                host: host.clone(),
                requests: metrics.requests,
                bytes_sent: metrics.bytes_sent,
                bytes_received: metrics.bytes_received,
                http_requests: metrics.http_requests,
                https_requests: metrics.https_requests,
                tunnel_requests: metrics.tunnel_requests,
                ws_requests: metrics.ws_requests,
                wss_requests: metrics.wss_requests,
                h3_requests: metrics.h3_requests,
                socks5_requests: metrics.socks5_requests,
            })
            .collect()
    }

    pub(crate) fn total_requests(&self) -> usize {
        self.total_requests as usize
    }

    fn increment(counts: &mut HashMap<String, u64>, value: Option<&str>) {
        if let Some(value) = value {
            if let Some(count) = counts.get_mut(value) {
                *count = count.saturating_add(1);
            } else {
                counts.insert(value.to_string(), 1);
            }
        }
    }

    fn decrement(counts: &mut HashMap<String, u64>, value: Option<&str>) {
        let Some(value) = value else {
            return;
        };
        let Some(count) = counts.get_mut(value) else {
            return;
        };
        if *count <= 1 {
            counts.remove(value);
        } else {
            *count -= 1;
        }
    }

    fn add_metrics_bucket(
        buckets: &mut HashMap<String, TrafficMetricsBucket>,
        value: Option<&str>,
        dimensions: &TrafficStatisticsDimensions,
    ) {
        let include_dimension = value.is_some();
        buckets
            .entry(value.unwrap_or("Unknown").to_string())
            .or_default()
            .add(dimensions, include_dimension);
    }

    fn subtract_metrics_bucket(
        buckets: &mut HashMap<String, TrafficMetricsBucket>,
        value: Option<&str>,
        dimensions: &TrafficStatisticsDimensions,
    ) {
        let include_dimension = value.is_some();
        let value = value.unwrap_or("Unknown");
        let Some(bucket) = buckets.get_mut(value) else {
            return;
        };
        bucket.subtract(dimensions, include_dimension);
        if bucket.requests == 0 {
            buckets.remove(value);
        }
    }
}

fn trusted_upload_bytes(record: &TrafficRecord) -> usize {
    if record.upload_bytes > 0 {
        return record.upload_bytes;
    }
    if let Some(status) = record.socket_status.as_ref() {
        if status.send_bytes > 0 {
            return status.send_bytes as usize;
        }
    }
    record.request_size
}

fn trusted_download_bytes(record: &TrafficRecord) -> usize {
    if record.download_bytes > 0 {
        return record.download_bytes;
    }
    if let Some(status) = record.socket_status.as_ref() {
        if status.receive_bytes > 0 {
            return status.receive_bytes as usize;
        }
    }
    record.response_size
}

fn non_empty(value: String) -> Option<String> {
    (!value.is_empty()).then_some(value)
}

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    use super::{TrafficStatistics, TrafficStatisticsDimensions};

    fn dimensions(
        ip: &str,
        port: &str,
        app: Option<&str>,
        account: Option<&str>,
        domain: &str,
    ) -> TrafficStatisticsDimensions {
        TrafficStatisticsDimensions {
            client_ip: (!ip.is_empty()).then(|| ip.to_string()),
            proxy_port: (!port.is_empty()).then(|| port.to_string()),
            application: app.map(str::to_string),
            account_name: account.map(str::to_string),
            domain: (!domain.is_empty()).then(|| domain.to_string()),
            bytes_sent: 10,
            bytes_received: 20,
            protocol: "https".to_string(),
        }
    }

    #[test]
    fn statistics_track_insert_replace_and_remove_without_zero_buckets() {
        let first = dimensions("127.0.0.1", "9900", Some("Pending App"), None, "one.test");
        let resolved = dimensions("127.0.0.1", "9900", Some("Codex"), Some("eden"), "one.test");
        let second = dimensions("10.0.0.2", "8800", Some("Codex"), None, "two.test");
        let mut statistics = TrafficStatistics::default();

        statistics.insert(&first);
        statistics.insert(&second);
        statistics.replace(&first, &resolved);
        let snapshot = statistics.snapshot(42);

        assert_eq!(snapshot.total_requests, 2);
        assert_eq!(snapshot.server_sequence, 42);
        assert_eq!(snapshot.applications.get("Codex"), Some(&2));
        assert!(!snapshot.applications.contains_key("Pending App"));
        assert_eq!(snapshot.account_names.get("eden"), Some(&1));
        assert_eq!(snapshot.domains.get("one.test"), Some(&1));
        let codex = statistics
            .app_metrics()
            .into_iter()
            .find(|metrics| metrics.app_name == "Codex")
            .expect("Codex metrics bucket");
        assert_eq!(codex.requests, 2);
        assert_eq!(codex.bytes_sent, 20);
        assert_eq!(codex.bytes_received, 40);
        assert_eq!(codex.https_requests, 2);

        statistics.remove(&resolved);
        let snapshot = statistics.snapshot(43);
        assert_eq!(snapshot.total_requests, 1);
        assert_eq!(snapshot.applications.get("Codex"), Some(&1));
        assert!(!snapshot.client_ips.contains_key("127.0.0.1"));
        assert!(!snapshot.account_names.contains_key("eden"));
        let one = statistics
            .host_metrics()
            .into_iter()
            .find(|metrics| metrics.host == "one.test");
        assert!(one.is_none());
    }

    #[test]
    fn statistics_ignore_empty_dimensions_and_saturate_unknown_removals() {
        let empty = dimensions("", "", None, None, "");
        let unknown = dimensions(
            "10.0.0.1",
            "9900",
            Some("unknown"),
            Some("nobody"),
            "none.test",
        );
        let mut statistics = TrafficStatistics::default();

        statistics.remove(&unknown);
        statistics.remove(&empty);
        statistics.insert(&empty);
        let snapshot = statistics.snapshot(1);

        assert_eq!(snapshot.total_requests, 1);
        assert!(snapshot.client_ips.is_empty());
        assert!(snapshot.proxy_ports.is_empty());
        assert!(snapshot.applications.is_empty());
        assert!(snapshot.account_names.is_empty());
        assert!(snapshot.domains.is_empty());
        let unknown_app = statistics
            .app_metrics()
            .into_iter()
            .find(|metrics| metrics.app_name == "Unknown")
            .expect("missing application is grouped as Unknown for metrics");
        assert_eq!(unknown_app.requests, 1);
    }

    #[test]
    fn statistics_load_returns_empty_when_schema_is_unavailable() {
        let connection = Connection::open_in_memory().expect("open sqlite");

        let snapshot = TrafficStatistics::load(&connection).snapshot(0);

        assert_eq!(snapshot.total_requests, 0);
        assert!(snapshot.client_ips.is_empty());
    }
}
