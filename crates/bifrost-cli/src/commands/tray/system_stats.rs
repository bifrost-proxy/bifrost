use std::time::{Duration, Instant};

use sysinfo::{Networks, System};

#[derive(Debug, Clone, PartialEq)]
pub struct SystemStatsSnapshot {
    pub cpu_percent: f32,
    pub memory_used_bytes: u64,
    pub memory_total_bytes: u64,
    pub network_down_bytes_per_sec: u64,
    pub network_up_bytes_per_sec: u64,
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
    last_sample_at: Option<Instant>,
}

impl SystemStatsSampler {
    pub fn new() -> Self {
        let mut system = System::new();
        system.refresh_memory();
        system.refresh_cpu_all();

        let mut networks = Networks::new_with_refreshed_list();
        networks.refresh();

        Self {
            system,
            networks,
            last_sample_at: None,
        }
    }

    pub fn sample(&mut self, now: Instant) -> SystemStatsSnapshot {
        self.system.refresh_memory();
        self.system.refresh_cpu_usage();
        self.networks.refresh_list();
        self.networks.refresh();

        let elapsed = self
            .last_sample_at
            .map(|last| now.saturating_duration_since(last))
            .filter(|elapsed| elapsed.as_secs_f64() > 0.0)
            .unwrap_or(Duration::from_secs(1));
        self.last_sample_at = Some(now);

        let mut received = 0_u64;
        let mut transmitted = 0_u64;
        for (name, network) in &self.networks {
            if is_loopback_interface(name) {
                continue;
            }
            received = received.saturating_add(network.received());
            transmitted = transmitted.saturating_add(network.transmitted());
        }

        let seconds = elapsed.as_secs_f64();
        SystemStatsSnapshot {
            cpu_percent: self.system.global_cpu_usage().clamp(0.0, 100.0),
            memory_used_bytes: self.system.used_memory(),
            memory_total_bytes: self.system.total_memory(),
            network_down_bytes_per_sec: bytes_per_sec(received, seconds),
            network_up_bytes_per_sec: bytes_per_sec(transmitted, seconds),
        }
    }
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
            format_bytes_rate(snapshot.network_up_bytes_per_sec),
            format_bytes_rate(snapshot.network_down_bytes_per_sec)
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
            network_down_bytes_per_sec: 512 * 1024,
            network_up_bytes_per_sec: 1536 * 1024,
        });

        assert_eq!(lines.system, "System: CPU 23% | Memory 18.0 GB / 32.0 GB");
        assert_eq!(lines.network, "Network: Up 1.5 MB/s | Down 512 KB/s");
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
