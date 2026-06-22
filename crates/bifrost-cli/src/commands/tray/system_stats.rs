use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

#[cfg(all(unix, not(target_os = "macos")))]
use std::{ffi::CString, os::unix::ffi::OsStrExt};
#[cfg(target_os = "macos")]
use std::{
    ffi::{CStr, CString},
    os::unix::ffi::OsStrExt,
};

use bifrost_storage::TraySystemStatsItems;
use sysinfo::{Disks, MemoryRefreshKind, RefreshKind, System};

use super::menu::SystemStatsMenuLines;

const CPU_MEMORY_REFRESH_INTERVAL: Duration = Duration::from_secs(2);
const DISK_REFRESH_INTERVAL: Duration = Duration::from_secs(30);
const NETWORK_MIN_SAMPLE_INTERVAL: Duration = Duration::from_millis(900);
const NETWORK_LIST_REFRESH_INTERVAL: Duration = Duration::from_secs(60);
const DISK_IO_REFRESH_INTERVAL: Duration = Duration::from_secs(5);
const DISK_IO_QUERY_TIMEOUT: Duration = Duration::from_secs(2);
const DEFAULT_ROUTE_QUERY_TIMEOUT: Duration = Duration::from_millis(750);
const DEFAULT_ROUTE_POLL_INTERVAL: Duration = Duration::from_millis(10);

#[derive(Debug, Clone, PartialEq, Default)]
pub struct SystemStatsSnapshot {
    pub cpu_percent: f32,
    pub cpu_logical_cores: Option<usize>,
    pub load_one: f32,
    pub load_five: f32,
    pub load_fifteen: f32,
    pub memory_pressure_percent: Option<f32>,
    pub memory_used_bytes: u64,
    pub memory_compressed_bytes: u64,
    pub memory_cached_bytes: u64,
    pub memory_total_bytes: u64,
    pub swap_used_bytes: Option<u64>,
    pub swap_total_bytes: Option<u64>,
    pub disk_used_percent: Option<f32>,
    pub disk_free_bytes: Option<u64>,
    pub disk_total_bytes: Option<u64>,
    pub disk_mount_point: Option<String>,
    pub disk_total_bytes_per_sec: Option<u64>,
    pub disk_read_bytes_per_sec: Option<u64>,
    pub disk_write_bytes_per_sec: Option<u64>,
    pub network_down_bytes_per_sec: Option<u64>,
    pub network_up_bytes_per_sec: Option<u64>,
    pub network_interface: Option<String>,
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
    last_cpu_ticks: Option<MacCpuTicks>,
    disk_mount_point: Option<PathBuf>,
    system: System,
    cpu_percent: f32,
    cpu_logical_cores: Option<usize>,
    load_one: f32,
    load_five: f32,
    load_fifteen: f32,
    memory_pressure_percent: Option<f32>,
    memory_used_bytes: u64,
    memory_compressed_bytes: u64,
    memory_cached_bytes: u64,
    memory_total_bytes: u64,
    swap_used_bytes: Option<u64>,
    swap_total_bytes: Option<u64>,
    disk_used_percent: Option<f32>,
    disk_free_bytes: Option<u64>,
    disk_total_bytes: Option<u64>,
    disk_total_bytes_per_sec: Option<u64>,
    disk_read_bytes_per_sec: Option<u64>,
    disk_write_bytes_per_sec: Option<u64>,
    last_cpu_at: Option<Instant>,
    last_memory_at: Option<Instant>,
    last_disk_at: Option<Instant>,
    last_disk_io_at: Option<Instant>,
    last_network_list_refresh_at: Option<Instant>,
    last_network_interfaces: BTreeMap<String, NetworkInterfaceSample>,
    active_network_interface: Option<String>,
    preferred_network_interface: Option<String>,
    default_network_interface_resolver: DefaultNetworkInterfaceResolver,
    disk_io_resolver: DiskIoResolver,
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
struct DiskIoRates {
    read_bytes_per_sec: u64,
    write_bytes_per_sec: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DiskIoCounters {
    read_bytes: u64,
    write_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct NetworkInterfaceSample {
    at: Instant,
    totals: NetworkTotals,
}

struct DefaultNetworkInterfaceResolver {
    tx: mpsc::Sender<Option<String>>,
    rx: mpsc::Receiver<Option<String>>,
    pending: bool,
}

struct DiskIoResolver {
    tx: mpsc::Sender<Option<DiskIoRates>>,
    rx: mpsc::Receiver<Option<DiskIoRates>>,
    pending: bool,
}

impl DiskIoResolver {
    fn new() -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            tx,
            rx,
            pending: false,
        }
    }

    fn request_refresh(&mut self) {
        if self.pending {
            return;
        }

        let tx = self.tx.clone();
        match thread::Builder::new()
            .name("bifrost-tray-disk-io".to_string())
            .stack_size(128 * 1024)
            .spawn(move || {
                let _ = tx.send(sample_disk_io_rates());
            }) {
            Ok(_) => {
                self.pending = true;
            }
            Err(error) => {
                tracing::debug!(error = %error, "failed to spawn disk io sampling worker");
            }
        }
    }

    fn drain_latest(&mut self) -> Option<Option<DiskIoRates>> {
        let mut latest = None;
        loop {
            match self.rx.try_recv() {
                Ok(value) => {
                    self.pending = false;
                    latest = Some(value);
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.pending = false;
                    break;
                }
            }
        }
        latest
    }
}

impl DefaultNetworkInterfaceResolver {
    fn new() -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            tx,
            rx,
            pending: false,
        }
    }

    fn request_refresh(&mut self) {
        if self.pending {
            return;
        }

        let tx = self.tx.clone();
        match thread::Builder::new()
            .name("bifrost-tray-route-detect".to_string())
            .stack_size(128 * 1024)
            .spawn(move || {
                let _ = tx.send(detect_default_network_interface());
            }) {
            Ok(_) => {
                self.pending = true;
            }
            Err(error) => {
                tracing::debug!(
                    error = %error,
                    "failed to spawn default route detection worker"
                );
            }
        }
    }

    fn drain_latest(&mut self) -> Option<Option<String>> {
        let mut latest = None;
        loop {
            match self.rx.try_recv() {
                Ok(value) => {
                    self.pending = false;
                    latest = Some(value);
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.pending = false;
                    break;
                }
            }
        }
        latest
    }
}

#[cfg(target_os = "macos")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MacCpuTicks {
    user: u64,
    system: u64,
    idle: u64,
    nice: u64,
}

#[cfg(target_os = "macos")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MacMemoryPageCounts {
    free: u64,
    speculative: u64,
    wired: u64,
    purgeable: u64,
    internal: u64,
    compressor: u64,
    file_backed: u64,
}

#[cfg(target_os = "macos")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MacMemorySample {
    used_bytes: u64,
    compressed_bytes: u64,
    cached_bytes: u64,
}

impl SystemStatsSampler {
    pub fn new(data_dir: &Path) -> Self {
        let last_cpu_ticks = mac_cpu_ticks();
        let mut system = System::new_with_specifics(
            RefreshKind::new().with_memory(MemoryRefreshKind::new().with_swap()),
        );
        system.refresh_memory_specifics(MemoryRefreshKind::new().with_swap());
        let load = System::load_average();
        let memory_total_bytes = mac_memory_total_bytes().unwrap_or(0);
        let memory_pressure_percent = mac_memory_pressure_percent();
        let memory_sample = mac_memory_sample(memory_total_bytes).unwrap_or(MacMemorySample {
            used_bytes: 0,
            compressed_bytes: 0,
            cached_bytes: 0,
        });
        let swap_total_bytes = Some(system.total_swap());
        let swap_used_bytes = Some(system.used_swap());

        let disks = Disks::new_with_refreshed_list();
        let disk_mount_point = select_disk_mount_point(disks.list(), Some(data_dir));
        let disk_used_percent = disk_mount_point.as_deref().and_then(disk_usage_percent);
        let (disk_total_bytes, disk_free_bytes) = disk_mount_point
            .as_deref()
            .map(|mount| disk_space(disks.list(), mount))
            .unwrap_or((None, None));

        Self {
            cpu_percent: 0.0,
            cpu_logical_cores: std::thread::available_parallelism().ok().map(usize::from),
            load_one: load.one as f32,
            load_five: load.five as f32,
            load_fifteen: load.fifteen as f32,
            memory_pressure_percent,
            memory_used_bytes: memory_sample.used_bytes,
            memory_compressed_bytes: memory_sample.compressed_bytes,
            memory_cached_bytes: memory_sample.cached_bytes,
            memory_total_bytes,
            swap_used_bytes,
            swap_total_bytes,
            last_cpu_ticks,
            disk_mount_point,
            system,
            disk_used_percent,
            disk_free_bytes,
            disk_total_bytes,
            disk_total_bytes_per_sec: None,
            disk_read_bytes_per_sec: None,
            disk_write_bytes_per_sec: None,
            last_cpu_at: None,
            last_memory_at: None,
            last_disk_at: None,
            last_disk_io_at: None,
            last_network_list_refresh_at: None,
            last_network_interfaces: BTreeMap::new(),
            active_network_interface: None,
            preferred_network_interface: None,
            default_network_interface_resolver: DefaultNetworkInterfaceResolver::new(),
            disk_io_resolver: DiskIoResolver::new(),
            smoothed_network_rates: None,
        }
    }

    #[cfg(test)]
    pub fn sample(&mut self, now: Instant, items: &TraySystemStatsItems) -> SystemStatsSnapshot {
        self.sample_with_disk_io(now, items, true)
    }

    pub fn sample_with_disk_io(
        &mut self,
        now: Instant,
        items: &TraySystemStatsItems,
        sample_disk_io: bool,
    ) -> SystemStatsSnapshot {
        if items.cpu
            && self
                .last_cpu_at
                .map(|last| now.saturating_duration_since(last) >= CPU_MEMORY_REFRESH_INTERVAL)
                .unwrap_or(true)
        {
            if let Some(current_ticks) = mac_cpu_ticks() {
                if let Some(cpu_percent) = mac_cpu_percent(self.last_cpu_ticks, current_ticks) {
                    self.cpu_percent = cpu_percent;
                }
                self.last_cpu_ticks = Some(current_ticks);
            }
            let load = System::load_average();
            self.load_one = load.one as f32;
            self.load_five = load.five as f32;
            self.load_fifteen = load.fifteen as f32;
            self.last_cpu_at = Some(now);
        }

        if items.memory
            && self
                .last_memory_at
                .map(|last| now.saturating_duration_since(last) >= CPU_MEMORY_REFRESH_INTERVAL)
                .unwrap_or(true)
        {
            self.memory_total_bytes = mac_memory_total_bytes().unwrap_or(self.memory_total_bytes);
            self.memory_pressure_percent = mac_memory_pressure_percent();
            if let Some(memory_sample) = mac_memory_sample(self.memory_total_bytes) {
                self.memory_used_bytes = memory_sample.used_bytes;
                self.memory_compressed_bytes = memory_sample.compressed_bytes;
                self.memory_cached_bytes = memory_sample.cached_bytes;
            }
            self.system
                .refresh_memory_specifics(MemoryRefreshKind::new().with_swap());
            self.swap_total_bytes = Some(self.system.total_swap());
            self.swap_used_bytes = Some(self.system.used_swap());
            self.last_memory_at = Some(now);
        }

        if items.disk
            && self
                .last_disk_at
                .map(|last| now.saturating_duration_since(last) >= DISK_REFRESH_INTERVAL)
                .unwrap_or(true)
        {
            let disks = Disks::new_with_refreshed_list();
            self.disk_used_percent = self
                .disk_mount_point
                .as_deref()
                .and_then(disk_usage_percent);
            if let Some(mount) = self.disk_mount_point.as_deref() {
                let (total, free) = disk_space(disks.list(), mount);
                self.disk_total_bytes = total;
                self.disk_free_bytes = free;
            }
            self.last_disk_at = Some(now);
        }

        if items.disk && sample_disk_io {
            if let Some(rates) = self.disk_io_resolver.drain_latest() {
                self.disk_read_bytes_per_sec = rates.map(|value| value.read_bytes_per_sec);
                self.disk_write_bytes_per_sec = rates.map(|value| value.write_bytes_per_sec);
                self.disk_total_bytes_per_sec = rates.map(|value| {
                    value
                        .read_bytes_per_sec
                        .saturating_add(value.write_bytes_per_sec)
                });
            }
            if self
                .last_disk_io_at
                .map(|last| now.saturating_duration_since(last) >= DISK_IO_REFRESH_INTERVAL)
                .unwrap_or(true)
            {
                self.last_disk_io_at = Some(now);
                self.disk_io_resolver.request_refresh();
            }
        } else if !items.disk {
            self.disk_total_bytes_per_sec = None;
            self.disk_read_bytes_per_sec = None;
            self.disk_write_bytes_per_sec = None;
        }

        let rates = if items.upload || items.download {
            self.apply_resolved_default_network_interface();
            if self
                .last_network_list_refresh_at
                .map(|last| now.saturating_duration_since(last) >= NETWORK_LIST_REFRESH_INTERVAL)
                .unwrap_or(true)
            {
                self.last_network_list_refresh_at = Some(now);
                self.default_network_interface_resolver.request_refresh();
            }
            self.sample_network_rates(now)
        } else {
            self.reset_network_baseline();
            None
        };

        SystemStatsSnapshot {
            cpu_percent: self.cpu_percent,
            cpu_logical_cores: self.cpu_logical_cores,
            memory_pressure_percent: self.memory_pressure_percent,
            memory_used_bytes: self.memory_used_bytes,
            memory_compressed_bytes: self.memory_compressed_bytes,
            memory_cached_bytes: self.memory_cached_bytes,
            memory_total_bytes: self.memory_total_bytes,
            disk_used_percent: self.disk_used_percent,
            disk_free_bytes: self.disk_free_bytes,
            disk_total_bytes: self.disk_total_bytes,
            disk_mount_point: self
                .disk_mount_point
                .as_ref()
                .map(|path| path.to_string_lossy().to_string()),
            disk_total_bytes_per_sec: self.disk_total_bytes_per_sec,
            disk_read_bytes_per_sec: self.disk_read_bytes_per_sec,
            disk_write_bytes_per_sec: self.disk_write_bytes_per_sec,
            network_down_bytes_per_sec: rates.map(|rates| rates.down_bytes_per_sec),
            network_up_bytes_per_sec: rates.map(|rates| rates.up_bytes_per_sec),
            network_interface: self.active_network_interface.clone(),
            load_one: self.load_one,
            load_five: self.load_five,
            load_fifteen: self.load_fifteen,
            swap_used_bytes: self.swap_used_bytes,
            swap_total_bytes: self.swap_total_bytes,
        }
    }

    pub fn reset_network_baseline(&mut self) {
        self.last_network_interfaces.clear();
        self.active_network_interface = None;
        self.smoothed_network_rates = None;
    }

    fn apply_resolved_default_network_interface(&mut self) {
        if let Some(preferred) = self.default_network_interface_resolver.drain_latest() {
            self.set_preferred_network_interface(preferred);
        }
    }

    fn set_preferred_network_interface(&mut self, preferred: Option<String>) {
        if preferred != self.preferred_network_interface {
            self.preferred_network_interface = preferred;
            self.reset_network_baseline();
        }
    }

    fn sample_network_rates(&mut self, now: Instant) -> Option<NetworkRates> {
        let current_interfaces = collect_network_interfaces();
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
    detect_default_network_interface_from_route(&["-n", "get", "default"]).or_else(|| {
        detect_default_network_interface_from_route(&["-n", "get", "-inet6", "default"])
    })
}

fn detect_default_network_interface_from_route(args: &[&str]) -> Option<String> {
    let output = command_output_with_timeout("route", args, DEFAULT_ROUTE_QUERY_TIMEOUT)?;
    if !output.status.success() {
        return None;
    }
    parse_default_network_interface(&String::from_utf8_lossy(&output.stdout))
}

fn command_output_with_timeout(
    program: &str,
    args: &[&str],
    timeout: Duration,
) -> Option<std::process::Output> {
    let mut child = Command::new(program)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let started = Instant::now();

    loop {
        match child.try_wait() {
            Ok(Some(_)) => return child.wait_with_output().ok(),
            Ok(None) => {
                if started.elapsed() >= timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    tracing::debug!(
                        program,
                        args = ?args,
                        timeout_ms = timeout.as_millis(),
                        "default route command timed out"
                    );
                    return None;
                }
                thread::sleep(DEFAULT_ROUTE_POLL_INTERVAL);
            }
            Err(error) => {
                tracing::debug!(
                    program,
                    args = ?args,
                    error = %error,
                    "failed to poll default route command"
                );
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}

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

#[allow(deprecated)]
fn mac_cpu_ticks() -> Option<MacCpuTicks> {
    let mut info = std::mem::MaybeUninit::<libc::host_cpu_load_info_data_t>::uninit();
    let mut count = libc::HOST_CPU_LOAD_INFO_COUNT;
    let result = unsafe {
        libc::host_statistics(
            libc::mach_host_self(),
            libc::HOST_CPU_LOAD_INFO,
            info.as_mut_ptr() as libc::host_info_t,
            &mut count,
        )
    };
    if result != libc::KERN_SUCCESS {
        return None;
    }

    let info = unsafe { info.assume_init() };
    Some(MacCpuTicks {
        user: info.cpu_ticks[libc::CPU_STATE_USER as usize] as u64,
        system: info.cpu_ticks[libc::CPU_STATE_SYSTEM as usize] as u64,
        idle: info.cpu_ticks[libc::CPU_STATE_IDLE as usize] as u64,
        nice: info.cpu_ticks[libc::CPU_STATE_NICE as usize] as u64,
    })
}

fn mac_cpu_percent(previous: Option<MacCpuTicks>, current: MacCpuTicks) -> Option<f32> {
    let previous = previous?;
    let user = current.user.checked_sub(previous.user)?;
    let system = current.system.checked_sub(previous.system)?;
    let idle = current.idle.checked_sub(previous.idle)?;
    let nice = current.nice.checked_sub(previous.nice)?;
    let total = user
        .saturating_add(system)
        .saturating_add(idle)
        .saturating_add(nice);
    if total == 0 {
        return None;
    }

    let used = total.saturating_sub(idle);
    Some((used as f32 / total as f32 * 100.0).clamp(0.0, 100.0))
}

fn mac_memory_total_bytes() -> Option<u64> {
    let mut mib = [libc::CTL_HW, libc::HW_MEMSIZE];
    let mut value = 0_u64;
    let mut len = std::mem::size_of::<u64>();
    let result = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            mib.len() as libc::c_uint,
            &mut value as *mut u64 as *mut libc::c_void,
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    };
    if result == 0 && value > 0 {
        Some(value)
    } else {
        None
    }
}

#[cfg(target_os = "macos")]
fn mac_memory_pressure_percent() -> Option<f32> {
    let name = CString::new("kern.memorystatus_level").ok()?;
    let mut free_level: libc::c_int = 0;
    let mut len = std::mem::size_of::<libc::c_int>();
    let result = unsafe {
        libc::sysctlbyname(
            name.as_ptr(),
            &mut free_level as *mut _ as *mut libc::c_void,
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    };
    if result != 0 || len != std::mem::size_of::<libc::c_int>() {
        return None;
    }

    Some(mac_memory_pressure_percent_from_free_level(free_level))
}

#[cfg(target_os = "macos")]
fn mac_memory_pressure_percent_from_free_level(free_level: libc::c_int) -> f32 {
    100.0 - (free_level.clamp(0, 100) as f32)
}

#[allow(deprecated)]
fn mac_memory_sample(total_bytes: u64) -> Option<MacMemorySample> {
    if total_bytes == 0 {
        return None;
    }

    let mut info = std::mem::MaybeUninit::<libc::vm_statistics64_data_t>::uninit();
    let mut count = libc::HOST_VM_INFO64_COUNT;
    let result = unsafe {
        libc::host_statistics64(
            libc::mach_host_self(),
            libc::HOST_VM_INFO64,
            info.as_mut_ptr() as libc::host_info64_t,
            &mut count,
        )
    };
    if result != libc::KERN_SUCCESS {
        return None;
    }

    let info = unsafe { info.assume_init() };
    let page_size = unsafe { libc::vm_page_size as u64 };
    if page_size == 0 {
        return None;
    }

    let total_pages = total_bytes / page_size;
    if total_pages == 0 {
        return None;
    }

    let page_counts = MacMemoryPageCounts {
        free: info.free_count as u64,
        speculative: info.speculative_count as u64,
        wired: info.wire_count as u64,
        purgeable: info.purgeable_count as u64,
        internal: info.internal_page_count as u64,
        compressor: info.compressor_page_count as u64,
        file_backed: info.external_page_count as u64,
    };
    let used_pages = mac_memory_used_pages(page_counts, total_pages);
    Some(MacMemorySample {
        used_bytes: pages_to_bytes(used_pages, page_size, total_bytes),
        compressed_bytes: pages_to_bytes(page_counts.compressor, page_size, total_bytes),
        cached_bytes: pages_to_bytes(
            mac_memory_cached_pages(page_counts, total_pages),
            page_size,
            total_bytes,
        ),
    })
}

#[cfg(target_os = "macos")]
fn mac_memory_used_pages(counts: MacMemoryPageCounts, total_pages: u64) -> u64 {
    let activity_monitor_style_pages = counts
        .internal
        .saturating_sub(counts.purgeable)
        .saturating_add(counts.wired)
        .saturating_add(counts.compressor);
    if activity_monitor_style_pages > 0 {
        return activity_monitor_style_pages.min(total_pages);
    }

    total_pages.saturating_sub(counts.free.saturating_add(counts.speculative))
}

#[cfg(target_os = "macos")]
fn mac_memory_cached_pages(counts: MacMemoryPageCounts, total_pages: u64) -> u64 {
    counts
        .file_backed
        .saturating_add(counts.speculative)
        .saturating_add(counts.purgeable)
        .min(total_pages)
}

fn pages_to_bytes(pages: u64, page_size: u64, total_bytes: u64) -> u64 {
    pages.saturating_mul(page_size).min(total_bytes)
}

fn collect_network_interfaces() -> BTreeMap<String, NetworkTotals> {
    let mut interfaces = BTreeMap::new();
    let mut addrs: *mut libc::ifaddrs = std::ptr::null_mut();
    if unsafe { libc::getifaddrs(&mut addrs) } != 0 {
        return interfaces;
    }

    let mut cursor = addrs;
    while !cursor.is_null() {
        let ifaddr = unsafe { &*cursor };
        if !ifaddr.ifa_name.is_null()
            && !ifaddr.ifa_addr.is_null()
            && !ifaddr.ifa_data.is_null()
            && unsafe { (*ifaddr.ifa_addr).sa_family as i32 } == libc::AF_LINK
        {
            let name = unsafe { CStr::from_ptr(ifaddr.ifa_name) }.to_string_lossy();
            if !is_ignored_network_interface(&name) {
                let data = unsafe { &*(ifaddr.ifa_data as *const libc::if_data) };
                interfaces.insert(
                    name.into_owned(),
                    NetworkTotals {
                        received: data.ifi_ibytes as u64,
                        transmitted: data.ifi_obytes as u64,
                    },
                );
            }
        }
        cursor = ifaddr.ifa_next;
    }

    unsafe { libc::freeifaddrs(addrs) };
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

fn disk_usage_percent(mount_point: &Path) -> Option<f32> {
    let c_path = CString::new(mount_point.as_os_str().as_bytes()).ok()?;
    let mut stat = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    let result = unsafe { libc::statvfs(c_path.as_ptr(), stat.as_mut_ptr()) };
    if result != 0 {
        return None;
    }

    let stat = unsafe { stat.assume_init() };
    let block_size = stat.f_frsize.max(stat.f_bsize);
    if block_size == 0 || stat.f_blocks == 0 {
        return None;
    }

    let total = (stat.f_blocks as u128).saturating_mul(block_size as u128);
    if total == 0 {
        return None;
    }
    let available = (stat.f_bavail as u128).saturating_mul(block_size as u128);
    let used = total.saturating_sub(available);
    Some((used as f32 / total as f32 * 100.0).clamp(0.0, 100.0))
}

fn disk_space(disks: &[sysinfo::Disk], mount_point: &Path) -> (Option<u64>, Option<u64>) {
    disks
        .iter()
        .find(|disk| disk.mount_point() == mount_point)
        .map(|disk| (Some(disk.total_space()), Some(disk.available_space())))
        .unwrap_or((None, None))
}

fn sample_disk_io_rates() -> Option<DiskIoRates> {
    let first = disk_io_counters()?;
    thread::sleep(Duration::from_secs(1));
    let second = disk_io_counters()?;
    Some(DiskIoRates {
        read_bytes_per_sec: second.read_bytes.saturating_sub(first.read_bytes),
        write_bytes_per_sec: second.write_bytes.saturating_sub(first.write_bytes),
    })
}

fn disk_io_counters() -> Option<DiskIoCounters> {
    let output = command_output_with_timeout(
        "ioreg",
        &["-rc", "IOBlockStorageDriver", "-k", "Statistics", "-l"],
        DISK_IO_QUERY_TIMEOUT,
    )?;
    if !output.status.success() {
        return None;
    }
    parse_ioreg_disk_io_counters(&String::from_utf8_lossy(&output.stdout))
}

fn parse_ioreg_disk_io_counters(output: &str) -> Option<DiskIoCounters> {
    let mut read_bytes = 0_u64;
    let mut write_bytes = 0_u64;
    let mut found = false;

    for statistics in output
        .match_indices("\"Statistics\"")
        .map(|(index, _)| &output[index..])
    {
        let Some(end) = statistics.find('}') else {
            continue;
        };
        let block = &statistics[..end];
        let read = parse_ioreg_stat_value(block, "Bytes (Read)").unwrap_or(0);
        let write = parse_ioreg_stat_value(block, "Bytes (Write)").unwrap_or(0);
        if read == 0 && write == 0 {
            continue;
        }
        read_bytes = read_bytes.saturating_add(read);
        write_bytes = write_bytes.saturating_add(write);
        found = true;
    }

    found.then_some(DiskIoCounters {
        read_bytes,
        write_bytes,
    })
}

fn parse_ioreg_stat_value(block: &str, name: &str) -> Option<u64> {
    let key = format!("\"{name}\"=");
    let start = block.find(&key)? + key.len();
    let digits = block[start..]
        .chars()
        .take_while(|ch| ch.is_ascii_digit())
        .collect::<String>();
    digits.parse().ok()
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
        let mut memory_details = vec![format!(
            "Memory {} / {}",
            format_bytes(snapshot.memory_used_bytes),
            format_bytes(snapshot.memory_total_bytes)
        )];
        if let Some(pressure_percent) = snapshot.memory_pressure_percent {
            memory_details.push(format!("Pressure {}", format_percent(pressure_percent)));
        }
        if snapshot.memory_compressed_bytes > 0 {
            memory_details.push(format!(
                "Compressed {}",
                format_bytes(snapshot.memory_compressed_bytes)
            ));
        }
        if snapshot.memory_cached_bytes > 0 {
            memory_details.push(format!(
                "Cached {}",
                format_bytes(snapshot.memory_cached_bytes)
            ));
        }
        system_parts.push(memory_details.join(" | "));
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
            format_menu_bar_memory_percent(snapshot.memory_used_bytes, snapshot.memory_total_bytes,)
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
    if let Some(network) = menu_bar_network_part(
        items,
        snapshot.network_up_bytes_per_sec,
        snapshot.network_down_bytes_per_sec,
    ) {
        menu_bar_parts.push(network);
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
    if let Some(network) = collecting_menu_bar_network_part(items) {
        parts.push(network);
    }
    parts.join(" | ")
}

fn menu_bar_network_part(
    items: &TraySystemStatsItems,
    upload_bytes_per_sec: Option<u64>,
    download_bytes_per_sec: Option<u64>,
) -> Option<String> {
    let mut parts = Vec::with_capacity(2);
    if items.upload {
        parts.push(format!(
            "↑{}",
            upload_bytes_per_sec
                .map(format_menu_bar_bytes_rate)
                .unwrap_or_else(|| "---".to_string())
        ));
    }
    if items.download {
        parts.push(format!(
            "↓{}",
            download_bytes_per_sec
                .map(format_menu_bar_bytes_rate)
                .unwrap_or_else(|| "---".to_string())
        ));
    }
    (!parts.is_empty()).then(|| parts.join(" "))
}

fn collecting_menu_bar_network_part(items: &TraySystemStatsItems) -> Option<&'static str> {
    match (items.upload, items.download) {
        (true, true) => Some("↑--- ↓---"),
        (true, false) => Some("↑---"),
        (false, true) => Some("↓---"),
        (false, false) => None,
    }
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

fn format_menu_bar_percent(value: f32) -> String {
    let rounded = quantize_menu_bar_percent(value);
    if rounded >= 100 {
        "100%".to_string()
    } else {
        format!("{rounded}%")
    }
}

fn format_menu_bar_memory_percent(used_bytes: u64, total_bytes: u64) -> String {
    if total_bytes == 0 {
        return "--%".to_string();
    }
    let rounded = (used_bytes as f32 / total_bytes as f32 * 100.0)
        .round()
        .clamp(0.0, 100.0) as u8;
    if rounded >= 100 {
        "100%".to_string()
    } else {
        format!("{rounded}%")
    }
}

fn bytes_per_sec(bytes: u64, seconds: f64) -> u64 {
    if seconds <= 0.0 {
        return 0;
    }
    (bytes as f64 / seconds).round() as u64
}

fn quantize_menu_bar_percent(value: f32) -> u8 {
    let rounded = value.round().clamp(0.0, 100.0) as u8;
    if rounded >= 100 {
        100
    } else {
        rounded / 5 * 5
    }
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
    fn parse_ioreg_disk_io_counters_sums_non_empty_devices() {
        let output = r#"
+-o IOBlockStorageDriver  <class IOBlockStorageDriver>
    {
      "Statistics" = {"Bytes (Read)"=0,"Bytes (Write)"=0}
    }
+-o IOBlockStorageDriver  <class IOBlockStorageDriver>
    {
      "Statistics" = {"Operations (Write)"=44,"Bytes (Read)"=9193232998400,"Bytes (Write)"=9291843264512,"Operations (Read)"=64}
    }
"#;

        assert_eq!(
            parse_ioreg_disk_io_counters(output),
            Some(DiskIoCounters {
                read_bytes: 9_193_232_998_400,
                write_bytes: 9_291_843_264_512,
            })
        );
    }

    #[test]
    fn menu_lines_show_two_rows_with_system_and_network_totals() {
        let lines = menu_lines(
            &SystemStatsSnapshot {
                cpu_percent: 23.4,
                memory_pressure_percent: None,
                memory_used_bytes: 18 * 1024 * 1024 * 1024,
                memory_compressed_bytes: 0,
                memory_cached_bytes: 0,
                memory_total_bytes: 32 * 1024 * 1024 * 1024,
                disk_used_percent: Some(58.7),
                network_down_bytes_per_sec: Some(512 * 1024),
                network_up_bytes_per_sec: Some(1536 * 1024),
                ..SystemStatsSnapshot::default()
            },
            &TraySystemStatsItems::default(),
        );

        assert_eq!(
            lines.system,
            "System: CPU 23% | Memory 18.0 GB / 32.0 GB | Disk 59%"
        );
        assert_eq!(lines.network, "Network: Up 1.5 MB/s | Down 512 KB/s");
        assert_eq!(lines.menu_bar, "C20% | M56% | D55% | ↑1.5 M/s ↓512 K/s");
    }

    #[test]
    fn menu_lines_keep_network_collecting_until_two_counter_samples_exist() {
        let lines = menu_lines(
            &SystemStatsSnapshot {
                cpu_percent: 23.4,
                memory_pressure_percent: None,
                memory_used_bytes: 18 * 1024 * 1024 * 1024,
                memory_compressed_bytes: 0,
                memory_cached_bytes: 0,
                memory_total_bytes: 32 * 1024 * 1024 * 1024,
                disk_used_percent: Some(58.7),
                network_down_bytes_per_sec: None,
                network_up_bytes_per_sec: None,
                ..SystemStatsSnapshot::default()
            },
            &TraySystemStatsItems::default(),
        );

        assert_eq!(
            lines.network,
            "Network: Up collecting... | Down collecting..."
        );
        assert_eq!(lines.menu_bar, "C20% | M56% | D55% | ↑--- ↓---");
    }

    #[test]
    fn menu_lines_uses_fixed_width_menu_bar_fields() {
        let lines = menu_lines(
            &SystemStatsSnapshot {
                cpu_percent: 5.4,
                memory_pressure_percent: None,
                memory_used_bytes: 3 * 1024 * 1024 * 1024,
                memory_compressed_bytes: 0,
                memory_cached_bytes: 0,
                memory_total_bytes: 32 * 1024 * 1024 * 1024,
                disk_used_percent: Some(8.6),
                network_down_bytes_per_sec: Some(26 * 1024),
                network_up_bytes_per_sec: Some(8 * 1024),
                ..SystemStatsSnapshot::default()
            },
            &TraySystemStatsItems::default(),
        );

        assert_eq!(lines.menu_bar, "C5% | M9% | D5% | ↑8.0 K/s ↓26.0 K/s");
    }

    #[test]
    fn menu_lines_shows_memory_load_and_pressure_on_macos() {
        let lines = menu_lines(
            &SystemStatsSnapshot {
                cpu_percent: 5.4,
                memory_pressure_percent: Some(22.1),
                memory_used_bytes: 28 * 1024 * 1024 * 1024,
                memory_compressed_bytes: 6 * 1024 * 1024 * 1024,
                memory_cached_bytes: 14 * 1024 * 1024 * 1024,
                memory_total_bytes: 64 * 1024 * 1024 * 1024,
                disk_used_percent: Some(30.0),
                network_down_bytes_per_sec: Some(26 * 1024),
                network_up_bytes_per_sec: Some(8 * 1024),
                ..SystemStatsSnapshot::default()
            },
            &TraySystemStatsItems::default(),
        );

        assert_eq!(
            lines.system,
            "System: CPU 5.4% | Memory 28.0 GB / 64.0 GB | Pressure 22% | Compressed 6.0 GB | Cached 14.0 GB | Disk 30%"
        );
        assert_eq!(lines.menu_bar, "C5% | M44% | D30% | ↑8.0 K/s ↓26.0 K/s");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn mac_memory_pressure_percent_from_free_level_clamps_to_pressure() {
        assert_eq!(mac_memory_pressure_percent_from_free_level(78), 22.0);
        assert_eq!(mac_memory_pressure_percent_from_free_level(120), 0.0);
        assert_eq!(mac_memory_pressure_percent_from_free_level(-10), 100.0);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn mac_memory_used_pages_includes_compressor_and_excludes_cache() {
        let counts = MacMemoryPageCounts {
            free: 600,
            speculative: 100,
            wired: 300,
            purgeable: 200,
            internal: 1_500,
            compressor: 250,
            file_backed: 1_000,
        };

        assert_eq!(mac_memory_used_pages(counts, 4_000), 1_850);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn mac_memory_used_pages_subtracts_purgeable_memory() {
        let counts = MacMemoryPageCounts {
            free: 600,
            speculative: 100,
            wired: 300,
            purgeable: 700,
            internal: 1_500,
            compressor: 250,
            file_backed: 1_000,
        };

        assert_eq!(mac_memory_used_pages(counts, 4_000), 1_350);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn mac_memory_cached_pages_includes_file_backed_speculative_and_purgeable() {
        let counts = MacMemoryPageCounts {
            free: 600,
            speculative: 100,
            wired: 300,
            purgeable: 700,
            internal: 1_500,
            compressor: 250,
            file_backed: 1_000,
        };

        assert_eq!(mac_memory_cached_pages(counts, 4_000), 1_800);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn mac_memory_used_pages_falls_back_to_non_free_pages_when_pressure_counts_are_empty() {
        let counts = MacMemoryPageCounts {
            free: 600,
            speculative: 100,
            wired: 0,
            purgeable: 0,
            internal: 0,
            compressor: 0,
            file_backed: 0,
        };

        assert_eq!(mac_memory_used_pages(counts, 4_000), 3_300);
    }

    #[test]
    fn menu_lines_handles_unknown_total_memory_in_menu_bar() {
        let lines = menu_lines(
            &SystemStatsSnapshot {
                cpu_percent: 5.4,
                memory_pressure_percent: None,
                memory_used_bytes: 18 * 1024 * 1024 * 1024,
                memory_compressed_bytes: 0,
                memory_cached_bytes: 0,
                memory_total_bytes: 0,
                disk_used_percent: None,
                network_down_bytes_per_sec: Some(512 * 1024),
                network_up_bytes_per_sec: Some(1536 * 1024),
                ..SystemStatsSnapshot::default()
            },
            &TraySystemStatsItems::default(),
        );

        assert_eq!(lines.menu_bar, "C5% | M--% | D--% | ↑1.5 M/s ↓512 K/s");
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
                memory_pressure_percent: None,
                memory_used_bytes: 3 * 1024 * 1024 * 1024,
                memory_compressed_bytes: 0,
                memory_cached_bytes: 0,
                memory_total_bytes: 32 * 1024 * 1024 * 1024,
                disk_used_percent: Some(8.6),
                network_down_bytes_per_sec: Some(26 * 1024),
                network_up_bytes_per_sec: Some(8 * 1024),
                ..SystemStatsSnapshot::default()
            },
            &items,
        );

        assert_eq!(lines.system, "System: CPU 5.4%");
        assert_eq!(lines.network, "Network: Down 26.0 KB/s");
        assert_eq!(lines.menu_bar, "C5% | ↓26.0 K/s");
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
        assert_eq!(lines.menu_bar, "M--% | ↑---");
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
        sampler.last_cpu_at = Some(now);
        sampler.last_memory_at = Some(now);
        sampler.last_disk_at = Some(now);

        let network_only = TraySystemStatsItems {
            cpu: false,
            memory: false,
            disk: false,
            upload: true,
            download: true,
        };
        let _ = sampler.sample(now + CPU_MEMORY_REFRESH_INTERVAL * 2, &network_only);

        assert_eq!(sampler.last_cpu_at, Some(now));
        assert_eq!(sampler.last_memory_at, Some(now));
        assert_eq!(sampler.last_disk_at, Some(now));

        let cpu_only = TraySystemStatsItems {
            cpu: true,
            memory: false,
            disk: false,
            upload: false,
            download: false,
        };
        let after_cpu_window = now + CPU_MEMORY_REFRESH_INTERVAL + Duration::from_millis(1);
        let _ = sampler.sample(after_cpu_window, &cpu_only);
        assert_eq!(sampler.last_cpu_at, Some(after_cpu_window));
        assert_eq!(sampler.last_memory_at, Some(now));

        let memory_only = TraySystemStatsItems {
            cpu: false,
            memory: true,
            disk: false,
            upload: false,
            download: false,
        };
        let after_memory_window =
            after_cpu_window + CPU_MEMORY_REFRESH_INTERVAL + Duration::from_millis(1);
        let _ = sampler.sample(after_memory_window, &memory_only);
        assert_eq!(sampler.last_cpu_at, Some(after_cpu_window));
        assert_eq!(sampler.last_memory_at, Some(after_memory_window));

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
    fn sample_can_skip_disk_io_refresh_for_closed_dashboard() {
        let temp = tempfile::tempdir().unwrap();
        let mut sampler = SystemStatsSampler::new(temp.path());
        let items = TraySystemStatsItems {
            cpu: false,
            memory: false,
            disk: true,
            upload: false,
            download: false,
        };
        let now = Instant::now();

        let _ = sampler.sample_with_disk_io(now, &items, false);
        assert_eq!(sampler.last_disk_io_at, None);

        let when_open = now + Duration::from_secs(1);
        let _ = sampler.sample_with_disk_io(when_open, &items, true);
        assert_eq!(sampler.last_disk_io_at, Some(when_open));
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn disk_usage_percent_reports_valid_percent_for_existing_path() {
        let temp = tempfile::tempdir().unwrap();
        let percent = disk_usage_percent(temp.path()).expect("disk usage for temp dir");

        assert!((0.0..=100.0).contains(&percent));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn disk_usage_percent_is_unsupported_on_windows() {
        let temp = tempfile::tempdir().unwrap();

        assert!(disk_usage_percent(temp.path()).is_none());
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
    fn preferred_interface_resolver_result_is_applied_without_waiting_for_next_refresh() {
        let mut sampler = SystemStatsSampler::new(Path::new("/tmp"));
        sampler
            .default_network_interface_resolver
            .tx
            .send(Some("en9".to_string()))
            .unwrap();

        sampler.apply_resolved_default_network_interface();

        assert_eq!(sampler.preferred_network_interface, Some("en9".to_string()));
    }

    #[cfg(unix)]
    #[test]
    fn command_output_with_timeout_returns_when_child_hangs() {
        let started = Instant::now();
        let output = command_output_with_timeout(
            "sh",
            &["-c", "sleep 2; echo late"],
            Duration::from_millis(50),
        );

        assert!(output.is_none());
        assert!(started.elapsed() < Duration::from_secs(1));
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
