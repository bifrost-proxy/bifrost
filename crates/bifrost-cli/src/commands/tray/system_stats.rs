use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant},
};

use bifrost_storage::TraySystemStatsItems;
use sysinfo::{Disks, Networks, System};

const CPU_MEMORY_REFRESH_INTERVAL: Duration = Duration::from_secs(3);
const DISK_REFRESH_INTERVAL: Duration = Duration::from_secs(30);
const NETWORK_MIN_SAMPLE_INTERVAL: Duration = Duration::from_millis(900);
const NETWORK_LIST_REFRESH_INTERVAL: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, PartialEq)]
pub struct SystemStatsSnapshot {
    pub cpu_percent: f32,
    pub memory_used_bytes: u64,
    pub memory_total_bytes: u64,
    pub disk_used_percent: Option<f32>,
    pub network_down_bytes_per_sec: Option<u64>,
    pub network_up_bytes_per_sec: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemStatsMenuLines {
    pub system: String,
    pub network: String,
    pub menu_bar: String,
}

impl SystemStatsMenuLines {
    pub fn collecting(items: &TraySystemStatsItems) -> Self {
        Self {
            system: collecting_system_line(items),
            network: collecting_network_line(items),
            menu_bar: collecting_menu_bar_line(items),
        }
    }
}

pub struct SystemStatsSampler {
    system: System,
    networks: Networks,
    disks: Disks,
    disk_mount_point: Option<PathBuf>,
    cpu_percent: f32,
    memory_used_bytes: u64,
    memory_total_bytes: u64,
    disk_used_percent: Option<f32>,
    last_cpu_memory_at: Option<Instant>,
    last_disk_at: Option<Instant>,
    last_network_list_refresh_at: Option<Instant>,
    last_network_interfaces: BTreeMap<String, NetworkInterfaceSample>,
    active_network_interface: Option<String>,
    preferred_network_interface: Option<String>,
    smoothed_network_rates: Option<NetworkRates>,
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
    pub fn new(data_dir: &Path) -> Self {
        let mut system = System::new();
        system.refresh_memory();
        system.refresh_cpu_all();

        let mut networks = Networks::new_with_refreshed_list();
        networks.refresh();

        let disks = Disks::new_with_refreshed_list();
        let disk_mount_point = select_disk_mount_point(disks.list(), Some(data_dir));
        let disk_used_percent = disk_mount_point
            .as_deref()
            .and_then(|mount_point| disk_usage_percent(disks.list(), mount_point));

        Self {
            cpu_percent: system.global_cpu_usage().clamp(0.0, 100.0),
            memory_used_bytes: system.used_memory(),
            memory_total_bytes: system.total_memory(),
            system,
            networks,
            disks,
            disk_mount_point,
            disk_used_percent,
            last_cpu_memory_at: None,
            last_disk_at: None,
            last_network_list_refresh_at: None,
            last_network_interfaces: BTreeMap::new(),
            active_network_interface: None,
            preferred_network_interface: detect_default_network_interface(),
            smoothed_network_rates: None,
        }
    }

    pub fn sample(&mut self, now: Instant, items: &TraySystemStatsItems) -> SystemStatsSnapshot {
        let needs_cpu_or_memory = items.cpu || items.memory;
        if needs_cpu_or_memory
            && self
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

        if items.disk
            && self
                .last_disk_at
                .map(|last| now.saturating_duration_since(last) >= DISK_REFRESH_INTERVAL)
                .unwrap_or(true)
        {
            self.disks.refresh();
            self.disk_used_percent = self
                .disk_mount_point
                .as_deref()
                .and_then(|mount_point| disk_usage_percent(self.disks.list(), mount_point));
            self.last_disk_at = Some(now);
        }

        let rates = if items.upload || items.download {
            if self
                .last_network_list_refresh_at
                .map(|last| now.saturating_duration_since(last) >= NETWORK_LIST_REFRESH_INTERVAL)
                .unwrap_or(true)
            {
                self.networks.refresh_list();
                self.last_network_list_refresh_at = Some(now);
                self.update_preferred_network_interface();
            }
            self.networks.refresh();
            self.sample_network_rates(now)
        } else {
            self.reset_network_baseline();
            None
        };

        SystemStatsSnapshot {
            cpu_percent: self.cpu_percent,
            memory_used_bytes: self.memory_used_bytes,
            memory_total_bytes: self.memory_total_bytes,
            disk_used_percent: self.disk_used_percent,
            network_down_bytes_per_sec: rates.map(|rates| rates.down_bytes_per_sec),
            network_up_bytes_per_sec: rates.map(|rates| rates.up_bytes_per_sec),
        }
    }

    pub fn reset_network_baseline(&mut self) {
        self.last_network_interfaces.clear();
        self.active_network_interface = None;
        self.smoothed_network_rates = None;
    }

    fn update_preferred_network_interface(&mut self) {
        self.set_preferred_network_interface(detect_default_network_interface());
    }

    fn set_preferred_network_interface(&mut self, preferred: Option<String>) {
        if preferred != self.preferred_network_interface {
            self.preferred_network_interface = preferred;
            self.reset_network_baseline();
        }
    }

    fn sample_network_rates(&mut self, now: Instant) -> Option<NetworkRates> {
        let current_interfaces = collect_network_interfaces(&self.networks);
        let mut candidates = Vec::new();

        for (name, current_totals) in &current_interfaces {
            let Some(last) = self.last_network_interfaces.get(name) else {
                continue;
            };
            let Some(rates) = network_rate_from_totals(*current_totals, last.totals, now, last.at)
            else {
                continue;
            };
            candidates.push((name.clone(), rates));
        }

        self.last_network_interfaces = current_interfaces
            .into_iter()
            .map(|(name, totals)| (name, NetworkInterfaceSample { at: now, totals }))
            .collect();

        let selected = select_network_rates(
            &mut self.active_network_interface,
            self.preferred_network_interface.as_deref(),
            &candidates,
        )?;
        let smoothed = smooth_network_rates(self.smoothed_network_rates, selected);
        self.smoothed_network_rates = Some(smoothed);
        Some(smoothed)
    }
}

fn select_network_rates(
    active_interface: &mut Option<String>,
    preferred_interface: Option<&str>,
    candidates: &[(String, NetworkRates)],
) -> Option<NetworkRates> {
    if candidates.is_empty() {
        *active_interface = None;
        return None;
    }

    if let Some(preferred_interface) = preferred_interface {
        if let Some(selected) = candidates
            .iter()
            .find(|(name, _)| name == preferred_interface)
        {
            *active_interface = Some(selected.0.clone());
            return Some(selected.1);
        }
    }

    let best = candidates.iter().max_by_key(|(_, rates)| {
        rates
            .down_bytes_per_sec
            .saturating_add(rates.up_bytes_per_sec)
    })?;

    let selected = match active_interface.as_deref() {
        Some(active_name) => {
            let active = candidates.iter().find(|(name, _)| name == active_name);
            let should_switch = active
                .map(|(_, active_rates)| {
                    network_rate_weight(&best.1)
                        > network_rate_weight(active_rates).saturating_mul(2)
                })
                .unwrap_or(true);
            if should_switch {
                best
            } else {
                active.unwrap_or(best)
            }
        }
        None => best,
    };

    *active_interface = Some(selected.0.clone());
    Some(selected.1)
}

fn detect_default_network_interface() -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        detect_default_network_interface_from_route(&["-n", "get", "default"]).or_else(|| {
            detect_default_network_interface_from_route(&["-n", "get", "-inet6", "default"])
        })
    }

    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

#[cfg(target_os = "macos")]
fn detect_default_network_interface_from_route(args: &[&str]) -> Option<String> {
    let output = Command::new("route").args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    parse_default_network_interface(&String::from_utf8_lossy(&output.stdout))
}

#[cfg(any(test, target_os = "macos"))]
fn parse_default_network_interface(output: &str) -> Option<String> {
    for line in output.lines() {
        let line = line.trim();
        if let Some(interface) = line.strip_prefix("interface:") {
            let interface = interface.trim();
            if !interface.is_empty() && !is_ignored_network_interface(interface) {
                return Some(interface.to_string());
            }
        }
    }
    None
}

fn network_rate_weight(rates: &NetworkRates) -> u64 {
    rates
        .down_bytes_per_sec
        .saturating_add(rates.up_bytes_per_sec)
        .max(1)
}

fn smooth_network_rates(previous: Option<NetworkRates>, current: NetworkRates) -> NetworkRates {
    const CURRENT_WEIGHT: u64 = 3;
    const PREVIOUS_WEIGHT: u64 = 2;

    let Some(previous) = previous else {
        return current;
    };

    NetworkRates {
        down_bytes_per_sec: weighted_average(
            previous.down_bytes_per_sec,
            current.down_bytes_per_sec,
            PREVIOUS_WEIGHT,
            CURRENT_WEIGHT,
        ),
        up_bytes_per_sec: weighted_average(
            previous.up_bytes_per_sec,
            current.up_bytes_per_sec,
            PREVIOUS_WEIGHT,
            CURRENT_WEIGHT,
        ),
    }
}

fn weighted_average(previous: u64, current: u64, previous_weight: u64, current_weight: u64) -> u64 {
    let total_weight = previous_weight.saturating_add(current_weight).max(1);
    previous
        .saturating_mul(previous_weight)
        .saturating_add(current.saturating_mul(current_weight))
        / total_weight
}

fn collect_network_interfaces(networks: &Networks) -> BTreeMap<String, NetworkTotals> {
    let mut interfaces = BTreeMap::new();
    for (name, network) in networks {
        if is_ignored_network_interface(name) {
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

fn select_disk_mount_point(disks: &[sysinfo::Disk], current_dir: Option<&Path>) -> Option<PathBuf> {
    if let Some(path) = current_dir {
        if let Some(disk) = disks
            .iter()
            .filter(|disk| path.starts_with(disk.mount_point()))
            .max_by_key(|disk| disk.mount_point().components().count())
        {
            return Some(disk.mount_point().to_path_buf());
        }
    }

    disks
        .iter()
        .find(|disk| disk.mount_point() == Path::new("/"))
        .or_else(|| disks.first())
        .map(|disk| disk.mount_point().to_path_buf())
}

fn disk_usage_percent(disks: &[sysinfo::Disk], mount_point: &Path) -> Option<f32> {
    let disk = disks
        .iter()
        .find(|disk| disk.mount_point() == mount_point)?;
    let total = disk.total_space();
    if total == 0 {
        return None;
    }
    let used = total.saturating_sub(disk.available_space());
    Some((used as f32 / total as f32 * 100.0).clamp(0.0, 100.0))
}

fn network_rate_from_totals(
    current: NetworkTotals,
    last: NetworkTotals,
    now: Instant,
    last_at: Instant,
) -> Option<NetworkRates> {
    let elapsed = now.saturating_duration_since(last_at);
    if elapsed < NETWORK_MIN_SAMPLE_INTERVAL {
        return None;
    }
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

pub fn menu_lines(
    snapshot: &SystemStatsSnapshot,
    items: &TraySystemStatsItems,
) -> SystemStatsMenuLines {
    let mut system_parts = Vec::new();
    if items.cpu {
        system_parts.push(format!("CPU {}", format_percent(snapshot.cpu_percent)));
    }
    if items.memory {
        system_parts.push(format!(
            "Memory {} / {}",
            format_bytes(snapshot.memory_used_bytes),
            format_bytes(snapshot.memory_total_bytes)
        ));
    }
    if items.disk {
        system_parts.push(format!(
            "Disk {}",
            snapshot
                .disk_used_percent
                .map(format_percent)
                .unwrap_or_else(|| "collecting...".to_string())
        ));
    }

    let mut network_parts = Vec::new();
    if items.upload {
        network_parts.push(format!(
            "Up {}",
            snapshot
                .network_up_bytes_per_sec
                .map(format_bytes_rate)
                .unwrap_or_else(|| "collecting...".to_string())
        ));
    }
    if items.download {
        network_parts.push(format!(
            "Down {}",
            snapshot
                .network_down_bytes_per_sec
                .map(format_bytes_rate)
                .unwrap_or_else(|| "collecting...".to_string())
        ));
    }

    let mut menu_bar_parts = Vec::new();
    if items.cpu {
        menu_bar_parts.push(format!(
            "C{}",
            format_menu_bar_percent(snapshot.cpu_percent)
        ));
    }
    if items.memory {
        menu_bar_parts.push(format!(
            "M{}",
            format_menu_bar_memory_percent(snapshot.memory_used_bytes, snapshot.memory_total_bytes)
        ));
    }
    if items.disk {
        menu_bar_parts.push(format!(
            "D{}",
            snapshot
                .disk_used_percent
                .map(format_menu_bar_percent)
                .unwrap_or_else(|| "--%".to_string())
        ));
    }
    if items.upload {
        menu_bar_parts.push(format!(
            "↑{}",
            snapshot
                .network_up_bytes_per_sec
                .map(format_menu_bar_bytes_rate)
                .unwrap_or_else(|| "---/s".to_string())
        ));
    }
    if items.download {
        menu_bar_parts.push(format!(
            "↓{}",
            snapshot
                .network_down_bytes_per_sec
                .map(format_menu_bar_bytes_rate)
                .unwrap_or_else(|| "---/s".to_string())
        ));
    }

    SystemStatsMenuLines {
        system: line_with_prefix("System", &system_parts),
        network: line_with_prefix("Network", &network_parts),
        menu_bar: menu_bar_parts.join(" | "),
    }
}

fn collecting_system_line(items: &TraySystemStatsItems) -> String {
    let mut parts = Vec::new();
    if items.cpu {
        parts.push("CPU collecting...");
    }
    if items.memory {
        parts.push("Memory collecting...");
    }
    if items.disk {
        parts.push("Disk collecting...");
    }
    line_with_prefix("System", &parts)
}

fn collecting_network_line(items: &TraySystemStatsItems) -> String {
    let mut parts = Vec::new();
    if items.upload {
        parts.push("Up collecting...");
    }
    if items.download {
        parts.push("Down collecting...");
    }
    line_with_prefix("Network", &parts)
}

fn collecting_menu_bar_line(items: &TraySystemStatsItems) -> String {
    let mut parts = Vec::new();
    if items.cpu {
        parts.push("C--%");
    }
    if items.memory {
        parts.push("M--%");
    }
    if items.disk {
        parts.push("D--%");
    }
    if items.upload {
        parts.push("↑---/s");
    }
    if items.download {
        parts.push("↓---/s");
    }
    parts.join(" | ")
}

fn line_with_prefix<T: AsRef<str>>(prefix: &str, parts: &[T]) -> String {
    if parts.is_empty() {
        return format!("{prefix}: disabled");
    }
    format!(
        "{prefix}: {}",
        parts
            .iter()
            .map(AsRef::as_ref)
            .collect::<Vec<_>>()
            .join(" | ")
    )
}

pub fn format_bytes_rate(bytes_per_sec: u64) -> String {
    format!("{}/s", format_bytes(bytes_per_sec))
}

fn format_menu_bar_bytes_rate(bytes_per_sec: u64) -> String {
    format!("{}/s", format_menu_bar_bytes_compact(bytes_per_sec))
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

fn format_menu_bar_bytes_compact(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "K", "M", "G", "T"];
    let mut value = bytes as f64;
    let mut unit = 0_usize;
    while value >= 999.5 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    let rounded = value.round().clamp(0.0, 999.0) as u64;
    format!("{rounded:03}{}", UNITS[unit])
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

fn format_menu_bar_percent(value: f32) -> String {
    let rounded = value.round().clamp(0.0, 100.0) as u8;
    if rounded >= 100 {
        "100%".to_string()
    } else {
        format!("{rounded:02}%")
    }
}

fn format_menu_bar_memory_percent(used_bytes: u64, total_bytes: u64) -> String {
    if total_bytes == 0 {
        return "--%".to_string();
    }
    format_menu_bar_percent((used_bytes as f32 / total_bytes as f32 * 100.0).clamp(0.0, 100.0))
}

fn bytes_per_sec(bytes: u64, seconds: f64) -> u64 {
    if seconds <= 0.0 {
        return 0;
    }
    (bytes as f64 / seconds).round() as u64
}

fn is_ignored_network_interface(name: &str) -> bool {
    let normalized = name.to_ascii_lowercase();
    normalized == "lo"
        || normalized == "lo0"
        || normalized.starts_with("loopback")
        || normalized.contains("loopback pseudo-interface")
        || normalized.starts_with("awdl")
        || normalized.starts_with("llw")
        || normalized.starts_with("utun")
        || normalized.starts_with("ipsec")
        || normalized.starts_with("bridge")
        || normalized.starts_with("vmenet")
        || normalized.starts_with("vmnet")
        || normalized.starts_with("docker")
        || normalized.starts_with("br-")
        || normalized.starts_with("tap")
        || normalized.starts_with("tun")
        || normalized.starts_with("gif")
        || normalized.starts_with("stf")
        || normalized.starts_with("anpi")
        || normalized.contains("virtual")
        || normalized.contains("vmware")
        || normalized.contains("virtualbox")
        || normalized.contains("parallels")
        || normalized.contains("tailscale")
        || normalized.contains("zerotier")
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
        let lines = menu_lines(
            &SystemStatsSnapshot {
                cpu_percent: 23.4,
                memory_used_bytes: 18 * 1024 * 1024 * 1024,
                memory_total_bytes: 32 * 1024 * 1024 * 1024,
                disk_used_percent: Some(58.7),
                network_down_bytes_per_sec: Some(512 * 1024),
                network_up_bytes_per_sec: Some(1536 * 1024),
            },
            &TraySystemStatsItems::default(),
        );

        assert_eq!(
            lines.system,
            "System: CPU 23% | Memory 18.0 GB / 32.0 GB | Disk 59%"
        );
        assert_eq!(lines.network, "Network: Up 1.5 MB/s | Down 512 KB/s");
        assert_eq!(lines.menu_bar, "C23% | M56% | D59% | ↑002M/s | ↓512K/s");
    }

    #[test]
    fn menu_lines_keep_network_collecting_until_two_counter_samples_exist() {
        let lines = menu_lines(
            &SystemStatsSnapshot {
                cpu_percent: 23.4,
                memory_used_bytes: 18 * 1024 * 1024 * 1024,
                memory_total_bytes: 32 * 1024 * 1024 * 1024,
                disk_used_percent: Some(58.7),
                network_down_bytes_per_sec: None,
                network_up_bytes_per_sec: None,
            },
            &TraySystemStatsItems::default(),
        );

        assert_eq!(
            lines.network,
            "Network: Up collecting... | Down collecting..."
        );
        assert_eq!(lines.menu_bar, "C23% | M56% | D59% | ↑---/s | ↓---/s");
    }

    #[test]
    fn menu_lines_uses_fixed_width_menu_bar_fields() {
        let lines = menu_lines(
            &SystemStatsSnapshot {
                cpu_percent: 5.4,
                memory_used_bytes: 3 * 1024 * 1024 * 1024,
                memory_total_bytes: 32 * 1024 * 1024 * 1024,
                disk_used_percent: Some(8.6),
                network_down_bytes_per_sec: Some(26 * 1024),
                network_up_bytes_per_sec: Some(8 * 1024),
            },
            &TraySystemStatsItems::default(),
        );

        assert_eq!(lines.menu_bar, "C05% | M09% | D09% | ↑008K/s | ↓026K/s");
    }

    #[test]
    fn menu_lines_handles_unknown_total_memory_in_menu_bar() {
        let lines = menu_lines(
            &SystemStatsSnapshot {
                cpu_percent: 5.4,
                memory_used_bytes: 18 * 1024 * 1024 * 1024,
                memory_total_bytes: 0,
                disk_used_percent: None,
                network_down_bytes_per_sec: Some(512 * 1024),
                network_up_bytes_per_sec: Some(1536 * 1024),
            },
            &TraySystemStatsItems::default(),
        );

        assert_eq!(lines.menu_bar, "C05% | M--% | D--% | ↑002M/s | ↓512K/s");
    }

    #[test]
    fn menu_lines_respects_individual_enabled_items() {
        let items = TraySystemStatsItems {
            cpu: true,
            memory: false,
            disk: false,
            upload: false,
            download: true,
        };
        let lines = menu_lines(
            &SystemStatsSnapshot {
                cpu_percent: 5.4,
                memory_used_bytes: 3 * 1024 * 1024 * 1024,
                memory_total_bytes: 32 * 1024 * 1024 * 1024,
                disk_used_percent: Some(8.6),
                network_down_bytes_per_sec: Some(26 * 1024),
                network_up_bytes_per_sec: Some(8 * 1024),
            },
            &items,
        );

        assert_eq!(lines.system, "System: CPU 5.4%");
        assert_eq!(lines.network, "Network: Down 26.0 KB/s");
        assert_eq!(lines.menu_bar, "C05% | ↓026K/s");
    }

    #[test]
    fn collecting_lines_respect_individual_enabled_items() {
        let items = TraySystemStatsItems {
            cpu: false,
            memory: true,
            disk: false,
            upload: true,
            download: false,
        };

        let lines = SystemStatsMenuLines::collecting(&items);

        assert_eq!(lines.system, "System: Memory collecting...");
        assert_eq!(lines.network, "Network: Up collecting...");
        assert_eq!(lines.menu_bar, "M--% | ↑---/s");
    }

    #[test]
    fn sample_resets_network_baseline_when_network_items_are_disabled() {
        let temp = tempfile::tempdir().unwrap();
        let mut sampler = SystemStatsSampler::new(temp.path());
        sampler.last_network_interfaces.insert(
            "en0".to_string(),
            NetworkInterfaceSample {
                at: Instant::now(),
                totals: NetworkTotals {
                    received: 10,
                    transmitted: 20,
                },
            },
        );
        sampler.active_network_interface = Some("en0".to_string());
        sampler.smoothed_network_rates = Some(NetworkRates {
            down_bytes_per_sec: 100,
            up_bytes_per_sec: 200,
        });

        let items = TraySystemStatsItems {
            cpu: true,
            memory: false,
            disk: false,
            upload: false,
            download: false,
        };
        let snapshot = sampler.sample(Instant::now(), &items);

        assert!(snapshot.network_down_bytes_per_sec.is_none());
        assert!(snapshot.network_up_bytes_per_sec.is_none());
        assert!(sampler.last_network_interfaces.is_empty());
        assert!(sampler.active_network_interface.is_none());
        assert!(sampler.smoothed_network_rates.is_none());
    }

    #[test]
    fn sample_skips_disabled_metric_refreshes_and_throttles_disk_refresh() {
        let temp = tempfile::tempdir().unwrap();
        let mut sampler = SystemStatsSampler::new(temp.path());
        let now = Instant::now();
        sampler.last_cpu_memory_at = Some(now);
        sampler.last_disk_at = Some(now);

        let network_only = TraySystemStatsItems {
            cpu: false,
            memory: false,
            disk: false,
            upload: true,
            download: true,
        };
        let _ = sampler.sample(now + CPU_MEMORY_REFRESH_INTERVAL * 2, &network_only);

        assert_eq!(sampler.last_cpu_memory_at, Some(now));
        assert_eq!(sampler.last_disk_at, Some(now));

        let disk_enabled = TraySystemStatsItems {
            cpu: false,
            memory: false,
            disk: true,
            upload: false,
            download: false,
        };
        let _ = sampler.sample(now + CPU_MEMORY_REFRESH_INTERVAL * 2, &disk_enabled);
        assert_eq!(sampler.last_disk_at, Some(now));

        let after_disk_window = now + DISK_REFRESH_INTERVAL + Duration::from_secs(1);
        let _ = sampler.sample(after_disk_window, &disk_enabled);
        assert_eq!(sampler.last_disk_at, Some(after_disk_window));
    }

    #[test]
    fn network_rate_uses_cumulative_counters_and_actual_elapsed_time() {
        let last_at = Instant::now();
        let now = last_at + Duration::from_secs(2);

        let rates = network_rate_from_totals(
            NetworkTotals {
                received: 7168,
                transmitted: 5120,
            },
            NetworkTotals {
                received: 1024,
                transmitted: 1024,
            },
            now,
            last_at,
        )
        .expect("valid rate");

        assert_eq!(rates.down_bytes_per_sec, 3072);
        assert_eq!(rates.up_bytes_per_sec, 2048);
    }

    #[test]
    fn network_rate_skips_short_sampling_windows_to_avoid_spikes() {
        let last_at = Instant::now();
        let now = last_at + Duration::from_millis(250);

        assert_eq!(
            network_rate_from_totals(
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
            ),
            None
        );
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
    fn network_rate_selection_uses_active_interface_not_sum() {
        let mut active = Some("en0".to_string());
        let rates = select_network_rates(
            &mut active,
            None,
            &[
                (
                    "en0".to_string(),
                    NetworkRates {
                        down_bytes_per_sec: 100 * 1024,
                        up_bytes_per_sec: 10 * 1024,
                    },
                ),
                (
                    "bridge100".to_string(),
                    NetworkRates {
                        down_bytes_per_sec: 300 * 1024,
                        up_bytes_per_sec: 20 * 1024,
                    },
                ),
            ],
        )
        .expect("selected rates");

        assert_eq!(active, Some("bridge100".to_string()));
        assert_eq!(rates.down_bytes_per_sec, 300 * 1024);
        assert_eq!(rates.up_bytes_per_sec, 20 * 1024);
    }

    #[test]
    fn network_rate_selection_has_hysteresis_to_avoid_flapping() {
        let mut active = Some("en0".to_string());
        let rates = select_network_rates(
            &mut active,
            None,
            &[
                (
                    "en0".to_string(),
                    NetworkRates {
                        down_bytes_per_sec: 100 * 1024,
                        up_bytes_per_sec: 10 * 1024,
                    },
                ),
                (
                    "en1".to_string(),
                    NetworkRates {
                        down_bytes_per_sec: 130 * 1024,
                        up_bytes_per_sec: 10 * 1024,
                    },
                ),
            ],
        )
        .expect("selected rates");

        assert_eq!(active, Some("en0".to_string()));
        assert_eq!(rates.down_bytes_per_sec, 100 * 1024);
    }

    #[test]
    fn network_rate_selection_prefers_default_route_interface() {
        let mut active = Some("en0".to_string());
        let rates = select_network_rates(
            &mut active,
            Some("en1"),
            &[
                (
                    "en0".to_string(),
                    NetworkRates {
                        down_bytes_per_sec: 500 * 1024,
                        up_bytes_per_sec: 100 * 1024,
                    },
                ),
                (
                    "en1".to_string(),
                    NetworkRates {
                        down_bytes_per_sec: 100 * 1024,
                        up_bytes_per_sec: 20 * 1024,
                    },
                ),
            ],
        )
        .expect("selected rates");

        assert_eq!(active, Some("en1".to_string()));
        assert_eq!(rates.down_bytes_per_sec, 100 * 1024);
    }

    #[test]
    fn default_route_parser_ignores_virtual_interfaces() {
        assert_eq!(
            parse_default_network_interface(
                "route to: default\n  interface: bridge100\n  gateway: 10.211.55.1\n"
            ),
            None
        );
        assert_eq!(
            parse_default_network_interface(
                "route to: default\n  gateway: 192.168.8.1\n  interface: en1\n"
            ),
            Some("en1".to_string())
        );
    }

    #[test]
    fn preferred_interface_change_resets_network_baseline() {
        let mut sampler = SystemStatsSampler::new(Path::new("/tmp"));
        sampler.preferred_network_interface = Some("en0".to_string());
        sampler.last_network_interfaces.insert(
            "en0".to_string(),
            NetworkInterfaceSample {
                at: Instant::now(),
                totals: NetworkTotals {
                    received: 1,
                    transmitted: 2,
                },
            },
        );
        sampler.active_network_interface = Some("en0".to_string());
        sampler.smoothed_network_rates = Some(NetworkRates {
            down_bytes_per_sec: 100,
            up_bytes_per_sec: 50,
        });

        sampler.set_preferred_network_interface(Some("en1".to_string()));

        assert!(sampler.last_network_interfaces.is_empty());
        assert!(sampler.active_network_interface.is_none());
        assert!(sampler.smoothed_network_rates.is_none());
    }

    #[test]
    fn network_rate_smoothing_dampens_single_sample_jumps() {
        let smoothed = smooth_network_rates(
            Some(NetworkRates {
                down_bytes_per_sec: 100,
                up_bytes_per_sec: 50,
            }),
            NetworkRates {
                down_bytes_per_sec: 1100,
                up_bytes_per_sec: 550,
            },
        );

        assert_eq!(smoothed.down_bytes_per_sec, 700);
        assert_eq!(smoothed.up_bytes_per_sec, 350);
    }

    #[test]
    fn ignored_interface_detection_covers_loopback_virtual_and_physical_names() {
        assert!(is_ignored_network_interface("lo0"));
        assert!(is_ignored_network_interface("Loopback Pseudo-Interface 1"));
        assert!(is_ignored_network_interface("lo"));
        assert!(is_ignored_network_interface("utun3"));
        assert!(is_ignored_network_interface("bridge100"));
        assert!(is_ignored_network_interface("vmenet0"));
        assert!(is_ignored_network_interface("Parallels Host-Only #1"));
        assert!(!is_ignored_network_interface("en0"));
        assert!(!is_ignored_network_interface("Ethernet"));
    }
}
