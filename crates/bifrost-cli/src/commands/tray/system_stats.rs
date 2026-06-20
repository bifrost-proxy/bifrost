use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};

use sysinfo::{Networks, System};

const CPU_MEMORY_REFRESH_INTERVAL: Duration = Duration::from_secs(3);

#[derive(Debug, Clone, PartialEq)]
pub struct SystemStatsSnapshot {
    pub cpu_percent: f32,
    pub memory_used_bytes: u64,
    pub memory_total_bytes: u64,
    pub network_down_bytes_per_sec: Option<u64>,
    pub network_up_bytes_per_sec: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemStatsMenuLines {
    pub system: String,
    pub network: String,
}

impl SystemStatsMenuLines {
    pub fn collecting() -> Self {
        Self {
            system: "System: collecting CPU and memory...".to_string(),
            network: "Network: collecting throughput...".to_string(),
        }
    }
}

pub struct SystemStatsSampler {
    system: System,
    networks: Networks,
    cpu_percent: f32,
    memory_used_bytes: u64,
    memory_total_bytes: u64,
    last_cpu_memory_at: Option<Instant>,
    last_network_interfaces: BTreeMap<String, NetworkInterfaceSample>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct NetworkTotals {
    received: u64,
    transmitted: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct NetworkRates {
    down_bytes_per_sec: u64,
    up_bytes_per_sec: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct NetworkInterfaceSample {
    at: Instant,
    totals: NetworkTotals,
}

impl SystemStatsSampler {
    pub fn new() -> Self {
        let mut system = System::new();
        system.refresh_memory();
        system.refresh_cpu_all();

        let mut networks = Networks::new_with_refreshed_list();
        networks.refresh();

        Self {
            cpu_percent: system.global_cpu_usage().clamp(0.0, 100.0),
            memory_used_bytes: system.used_memory(),
            memory_total_bytes: system.total_memory(),
            system,
            networks,
            last_cpu_memory_at: None,
            last_network_interfaces: BTreeMap::new(),
        }
    }

    pub fn sample(&mut self, now: Instant) -> SystemStatsSnapshot {
        if self
            .last_cpu_memory_at
            .map(|last| now.saturating_duration_since(last) >= CPU_MEMORY_REFRESH_INTERVAL)
            .unwrap_or(true)
        {
            self.system.refresh_memory();
            self.system.refresh_cpu_usage();
            self.cpu_percent = self.system.global_cpu_usage().clamp(0.0, 100.0);
            self.memory_used_bytes = self.system.used_memory();
            self.memory_total_bytes = self.system.total_memory();
            self.last_cpu_memory_at = Some(now);
        }

        self.networks.refresh_list();
        self.networks.refresh();
        let rates = self.sample_network_rates(now);

        SystemStatsSnapshot {
            cpu_percent: self.cpu_percent,
            memory_used_bytes: self.memory_used_bytes,
            memory_total_bytes: self.memory_total_bytes,
            network_down_bytes_per_sec: rates.map(|rates| rates.down_bytes_per_sec),
            network_up_bytes_per_sec: rates.map(|rates| rates.up_bytes_per_sec),
        }
    }

    pub fn reset_network_baseline(&mut self) {
        self.last_network_interfaces.clear();
    }

    fn sample_network_rates(&mut self, now: Instant) -> Option<NetworkRates> {
        let current_interfaces = collect_network_interfaces(&self.networks);
        let mut down_bytes_per_sec = 0_u64;
        let mut up_bytes_per_sec = 0_u64;
        let mut saw_rate = false;

        for (name, current_totals) in &current_interfaces {
            let Some(last) = self.last_network_interfaces.get(name) else {
                continue;
            };
            let Some(rates) = network_rate_from_totals(*current_totals, last.totals, now, last.at)
            else {
                continue;
            };
            saw_rate = true;
            down_bytes_per_sec = down_bytes_per_sec.saturating_add(rates.down_bytes_per_sec);
            up_bytes_per_sec = up_bytes_per_sec.saturating_add(rates.up_bytes_per_sec);
        }

        self.last_network_interfaces = current_interfaces
            .into_iter()
            .map(|(name, totals)| (name, NetworkInterfaceSample { at: now, totals }))
            .collect();

        saw_rate.then_some(NetworkRates {
            down_bytes_per_sec,
            up_bytes_per_sec,
        })
    }
}

fn collect_network_interfaces(networks: &Networks) -> BTreeMap<String, NetworkTotals> {
    let mut interfaces = BTreeMap::new();
    for (name, network) in networks {
        if is_loopback_interface(name) {
            continue;
        }
        interfaces.insert(
            name.to_string(),
            NetworkTotals {
                received: network.total_received(),
                transmitted: network.total_transmitted(),
            },
        );
    }
    interfaces
}

fn network_rate_from_totals(
    current: NetworkTotals,
    last: NetworkTotals,
    now: Instant,
    last_at: Instant,
) -> Option<NetworkRates> {
    let elapsed = now.saturating_duration_since(last_at);
    let seconds = elapsed.as_secs_f64();
    if seconds <= 0.0 {
        return None;
    }
    Some(NetworkRates {
        down_bytes_per_sec: bytes_per_sec(current.received.checked_sub(last.received)?, seconds),
        up_bytes_per_sec: bytes_per_sec(
            current.transmitted.checked_sub(last.transmitted)?,
            seconds,
        ),
    })
}

pub fn menu_lines(snapshot: &SystemStatsSnapshot) -> SystemStatsMenuLines {
    SystemStatsMenuLines {
        system: format!(
            "System: CPU {} | Memory {} / {}",
            format_percent(snapshot.cpu_percent),
            format_bytes(snapshot.memory_used_bytes),
            format_bytes(snapshot.memory_total_bytes)
        ),
        network: format!(
            "Network: Up {} | Down {}",
            snapshot
                .network_up_bytes_per_sec
                .map(format_bytes_rate)
                .unwrap_or_else(|| "collecting...".to_string()),
            snapshot
                .network_down_bytes_per_sec
                .map(format_bytes_rate)
                .unwrap_or_else(|| "collecting...".to_string())
        ),
    }
}

pub fn format_bytes_rate(bytes_per_sec: u64) -> String {
    format!("{}/s", format_bytes(bytes_per_sec))
}

pub fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0_usize;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{} {}", bytes, UNITS[unit])
    } else if value >= 100.0 {
        format!("{value:.0} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

fn format_percent(value: f32) -> String {
    if value >= 100.0 {
        "100%".to_string()
    } else if value >= 10.0 {
        format!("{value:.0}%")
    } else {
        format!("{value:.1}%")
    }
}

fn bytes_per_sec(bytes: u64, seconds: f64) -> u64 {
    if seconds <= 0.0 {
        return 0;
    }
    (bytes as f64 / seconds).round() as u64
}

fn is_loopback_interface(name: &str) -> bool {
    let normalized = name.to_ascii_lowercase();
    normalized == "lo"
        || normalized == "lo0"
        || normalized.starts_with("loopback")
        || normalized.contains("loopback pseudo-interface")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_format_uses_compact_units() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(1536), "1.5 KB");
        assert_eq!(format_bytes(100 * 1024 * 1024), "100 MB");
    }

    #[test]
    fn menu_lines_show_two_rows_with_system_and_network_totals() {
        let lines = menu_lines(&SystemStatsSnapshot {
            cpu_percent: 23.4,
            memory_used_bytes: 18 * 1024 * 1024 * 1024,
            memory_total_bytes: 32 * 1024 * 1024 * 1024,
            network_down_bytes_per_sec: Some(512 * 1024),
            network_up_bytes_per_sec: Some(1536 * 1024),
        });

        assert_eq!(lines.system, "System: CPU 23% | Memory 18.0 GB / 32.0 GB");
        assert_eq!(lines.network, "Network: Up 1.5 MB/s | Down 512 KB/s");
    }

    #[test]
    fn menu_lines_keep_network_collecting_until_two_counter_samples_exist() {
        let lines = menu_lines(&SystemStatsSnapshot {
            cpu_percent: 23.4,
            memory_used_bytes: 18 * 1024 * 1024 * 1024,
            memory_total_bytes: 32 * 1024 * 1024 * 1024,
            network_down_bytes_per_sec: None,
            network_up_bytes_per_sec: None,
        });

        assert_eq!(
            lines.network,
            "Network: Up collecting... | Down collecting..."
        );
    }

    #[test]
    fn network_rate_uses_cumulative_counters_and_actual_elapsed_time() {
        let last_at = Instant::now();
        let now = last_at + Duration::from_millis(500);

        let rates = network_rate_from_totals(
            NetworkTotals {
                received: 4096,
                transmitted: 2048,
            },
            NetworkTotals {
                received: 1024,
                transmitted: 1024,
            },
            now,
            last_at,
        )
        .expect("valid rate");

        assert_eq!(rates.down_bytes_per_sec, 6144);
        assert_eq!(rates.up_bytes_per_sec, 2048);
    }

    #[test]
    fn network_rate_skips_counter_reset_or_interface_recreation() {
        let last_at = Instant::now();
        let now = last_at + Duration::from_secs(1);

        assert_eq!(
            network_rate_from_totals(
                NetworkTotals {
                    received: 10,
                    transmitted: 20,
                },
                NetworkTotals {
                    received: 100,
                    transmitted: 20,
                },
                now,
                last_at,
            ),
            None
        );
    }

    #[test]
    fn loopback_detection_covers_macos_windows_and_linux_names() {
        assert!(is_loopback_interface("lo0"));
        assert!(is_loopback_interface("Loopback Pseudo-Interface 1"));
        assert!(is_loopback_interface("lo"));
        assert!(!is_loopback_interface("en0"));
        assert!(!is_loopback_interface("Ethernet"));
    }
}
