use std::collections::HashMap;

use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use crate::traffic::TrafficRecord;

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
}

impl TrafficStatisticsDimensions {
    pub(crate) fn from_record(record: &TrafficRecord) -> Self {
        Self {
            client_ip: non_empty(record.client_ip.clone()),
            proxy_port: (record.listener_port > 0).then(|| record.listener_port.to_string()),
            application: record.client_app.clone().and_then(non_empty),
            account_name: record.account_name.clone().and_then(non_empty),
            domain: non_empty(record.host.clone()),
        }
    }

    pub(crate) fn load_by_id(conn: &Connection, id: &str) -> Option<Self> {
        conn.query_row(
            "SELECT client_ip, listener_port, client_app, account_name, host \
             FROM traffic_records WHERE id = ?1",
            [id],
            |row| {
                let client_ip: String = row.get(0)?;
                let listener_port = row.get::<_, i64>(1)? as u16;
                let application: Option<String> = row.get(2)?;
                let account_name: Option<String> = row.get(3)?;
                let domain: String = row.get(4)?;
                Ok(Self {
                    client_ip: non_empty(client_ip),
                    proxy_port: (listener_port > 0).then(|| listener_port.to_string()),
                    application: application.and_then(non_empty),
                    account_name: account_name.and_then(non_empty),
                    domain: non_empty(domain),
                })
            },
        )
        .ok()
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct TrafficStatistics {
    total_requests: u64,
    client_ips: HashMap<String, u64>,
    proxy_ports: HashMap<String, u64>,
    applications: HashMap<String, u64>,
    account_names: HashMap<String, u64>,
    domains: HashMap<String, u64>,
}

impl TrafficStatistics {
    pub(crate) fn load(conn: &Connection) -> Self {
        let mut statistics = Self::default();
        let mut statement = match conn.prepare(
            "SELECT client_ip, listener_port, client_app, account_name, host FROM traffic_records",
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

        let rows = match statement.query_map([], |row| {
            let client_ip: String = row.get(0)?;
            let listener_port = row.get::<_, i64>(1)? as u16;
            let application: Option<String> = row.get(2)?;
            let account_name: Option<String> = row.get(3)?;
            let domain: String = row.get(4)?;
            Ok(TrafficStatisticsDimensions {
                client_ip: non_empty(client_ip),
                proxy_port: (listener_port > 0).then(|| listener_port.to_string()),
                application: application.and_then(non_empty),
                account_name: account_name.and_then(non_empty),
                domain: non_empty(domain),
            })
        }) {
            Ok(rows) => rows,
            Err(error) => {
                tracing::warn!(
                    error = %error,
                    "[TRAFFIC_DB] Failed to read initial traffic statistics rows"
                );
                return statistics;
            }
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
        Self::increment(&mut self.applications, dimensions.application.as_deref());
        Self::increment(&mut self.account_names, dimensions.account_name.as_deref());
        Self::increment(&mut self.domains, dimensions.domain.as_deref());
    }

    pub(crate) fn remove(&mut self, dimensions: &TrafficStatisticsDimensions) {
        self.total_requests = self.total_requests.saturating_sub(1);
        Self::decrement(&mut self.client_ips, dimensions.client_ip.as_deref());
        Self::decrement(&mut self.proxy_ports, dimensions.proxy_port.as_deref());
        Self::decrement(&mut self.applications, dimensions.application.as_deref());
        Self::decrement(&mut self.account_names, dimensions.account_name.as_deref());
        Self::decrement(&mut self.domains, dimensions.domain.as_deref());
    }

    pub(crate) fn replace(
        &mut self,
        previous: &TrafficStatisticsDimensions,
        next: &TrafficStatisticsDimensions,
    ) {
        Self::decrement(&mut self.client_ips, previous.client_ip.as_deref());
        Self::decrement(&mut self.proxy_ports, previous.proxy_port.as_deref());
        Self::decrement(&mut self.applications, previous.application.as_deref());
        Self::decrement(&mut self.account_names, previous.account_name.as_deref());
        Self::decrement(&mut self.domains, previous.domain.as_deref());

        Self::increment(&mut self.client_ips, next.client_ip.as_deref());
        Self::increment(&mut self.proxy_ports, next.proxy_port.as_deref());
        Self::increment(&mut self.applications, next.application.as_deref());
        Self::increment(&mut self.account_names, next.account_name.as_deref());
        Self::increment(&mut self.domains, next.domain.as_deref());
    }

    pub(crate) fn snapshot(&self, server_sequence: u64) -> TrafficStatisticsSnapshot {
        TrafficStatisticsSnapshot {
            total_requests: self.total_requests,
            server_sequence,
            client_ips: self.client_ips.clone(),
            proxy_ports: self.proxy_ports.clone(),
            applications: self.applications.clone(),
            account_names: self.account_names.clone(),
            domains: self.domains.clone(),
        }
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
}

fn non_empty(value: String) -> Option<String> {
    (!value.is_empty()).then_some(value)
}

#[cfg(test)]
mod tests {
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

        statistics.remove(&resolved);
        let snapshot = statistics.snapshot(43);
        assert_eq!(snapshot.total_requests, 1);
        assert_eq!(snapshot.applications.get("Codex"), Some(&1));
        assert!(!snapshot.client_ips.contains_key("127.0.0.1"));
        assert!(!snapshot.account_names.contains_key("eden"));
    }

    #[test]
    fn statistics_ignore_empty_dimensions_and_saturate_unknown_removals() {
        let empty = dimensions("", "", None, None, "");
        let mut statistics = TrafficStatistics::default();

        statistics.remove(&empty);
        statistics.insert(&empty);
        let snapshot = statistics.snapshot(1);

        assert_eq!(snapshot.total_requests, 1);
        assert!(snapshot.client_ips.is_empty());
        assert!(snapshot.proxy_ports.is_empty());
        assert!(snapshot.applications.is_empty());
        assert!(snapshot.account_names.is_empty());
        assert!(snapshot.domains.is_empty());
    }
}
