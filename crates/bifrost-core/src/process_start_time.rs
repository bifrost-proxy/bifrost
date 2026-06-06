//! Cross-platform process start-time lookup.
//!
//! The lifecycle helper uses this to detect PID reuse: if the recorded
//! start-time of the parent process no longer matches the OS-reported one for
//! the same PID, the original parent has exited and the PID has been recycled.
//! Returning `None` lets callers gracefully degrade to PID-only checks instead
//! of failing closed.

/// Returns the start-time of the given process in milliseconds since the Unix
/// epoch, or `None` when the value cannot be determined (process gone,
/// permission denied, kernel structures unavailable, etc.).
pub fn get_process_start_time_ms(pid: u32) -> Option<u64> {
    platform::get_process_start_time_ms(pid)
}

/// Returns the start-time of the current process. Convenience for call sites
/// that record their own start-time once at boot.
pub fn current_process_start_time_ms() -> Option<u64> {
    get_process_start_time_ms(std::process::id())
}

/// Tolerance window when comparing two start-times for the same PID. Operating
/// systems publish start-time at varying resolutions (jiffies, microseconds,
/// 100 ns ticks); 2 seconds is wider than any of them and still tight enough
/// to catch PID reuse on any realistic time-scale.
pub const START_TIME_MATCH_TOLERANCE_MS: u64 = 2_000;

/// Returns `true` when both values are present and within
/// [`START_TIME_MATCH_TOLERANCE_MS`] of each other; `None` on either side is
/// treated as "unknown" rather than mismatch so callers can fall back to a
/// PID-only check.
pub fn start_times_match(recorded: Option<u64>, observed: Option<u64>) -> StartTimeMatch {
    match (recorded, observed) {
        (Some(a), Some(b)) => {
            let diff = a.max(b) - a.min(b);
            if diff <= START_TIME_MATCH_TOLERANCE_MS {
                StartTimeMatch::Match
            } else {
                StartTimeMatch::Mismatch {
                    recorded: a,
                    observed: b,
                }
            }
        }
        _ => StartTimeMatch::Unknown,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartTimeMatch {
    Match,
    Mismatch { recorded: u64, observed: u64 },
    Unknown,
}

#[cfg(target_os = "linux")]
mod platform {
    use std::fs;
    use std::sync::OnceLock;

    pub fn get_process_start_time_ms(pid: u32) -> Option<u64> {
        let stat = fs::read_to_string(format!("/proc/{}/stat", pid)).ok()?;
        // Field 22 is starttime in clock ticks since boot. Fields 0..1 are
        // pid and (comm); comm may contain spaces and parens, so we split
        // after the trailing ')' to get a stable index.
        let close_paren = stat.rfind(')')?;
        let rest = &stat[close_paren + 1..];
        let fields: Vec<&str> = rest.split_whitespace().collect();
        // After the rfind(')'), field index 0 in `fields` corresponds to
        // field 3 (state) of the proc(5) layout, so starttime (field 22) is
        // at offset 22 - 3 = 19.
        let starttime_jiffies: u64 = fields.get(19)?.parse().ok()?;

        let clk_tck = clock_ticks_per_second()?;
        let btime_secs = read_btime_seconds()?;
        let starttime_secs_since_boot = starttime_jiffies / clk_tck;
        let starttime_jiffies_remainder = starttime_jiffies % clk_tck;
        let extra_ms = starttime_jiffies_remainder.saturating_mul(1_000) / clk_tck;

        Some(
            btime_secs
                .saturating_mul(1_000)
                .saturating_add(starttime_secs_since_boot.saturating_mul(1_000))
                .saturating_add(extra_ms),
        )
    }

    fn clock_ticks_per_second() -> Option<u64> {
        static CACHED: OnceLock<Option<u64>> = OnceLock::new();
        *CACHED.get_or_init(|| {
            let value = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
            if value <= 0 {
                None
            } else {
                Some(value as u64)
            }
        })
    }

    fn read_btime_seconds() -> Option<u64> {
        static CACHED: OnceLock<Option<u64>> = OnceLock::new();
        *CACHED.get_or_init(|| {
            let stat = fs::read_to_string("/proc/stat").ok()?;
            for line in stat.lines() {
                if let Some(value) = line.strip_prefix("btime ") {
                    return value.trim().parse().ok();
                }
            }
            None
        })
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use std::mem;

    pub fn get_process_start_time_ms(pid: u32) -> Option<u64> {
        let mut info: libc::proc_bsdinfo = unsafe { mem::zeroed() };
        let size = mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
        let written = unsafe {
            libc::proc_pidinfo(
                pid as libc::c_int,
                libc::PROC_PIDTBSDINFO,
                0,
                &mut info as *mut _ as *mut libc::c_void,
                size,
            )
        };
        if written != size {
            return None;
        }
        if info.pbi_start_tvsec == 0 {
            return None;
        }
        Some(
            info.pbi_start_tvsec
                .saturating_mul(1_000)
                .saturating_add(info.pbi_start_tvusec / 1_000),
        )
    }
}

#[cfg(windows)]
mod platform {
    use windows_sys::Win32::Foundation::{CloseHandle, FILETIME};
    use windows_sys::Win32::System::Threading::{
        GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    pub fn get_process_start_time_ms(pid: u32) -> Option<u64> {
        unsafe {
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if handle.is_null() {
                return None;
            }

            let mut creation = FILETIME {
                dwLowDateTime: 0,
                dwHighDateTime: 0,
            };
            let mut exit = FILETIME {
                dwLowDateTime: 0,
                dwHighDateTime: 0,
            };
            let mut kernel = FILETIME {
                dwLowDateTime: 0,
                dwHighDateTime: 0,
            };
            let mut user = FILETIME {
                dwLowDateTime: 0,
                dwHighDateTime: 0,
            };
            let ok = GetProcessTimes(handle, &mut creation, &mut exit, &mut kernel, &mut user);
            CloseHandle(handle);
            if ok == 0 {
                return None;
            }

            // FILETIME is 100-ns ticks since 1601-01-01 UTC. Convert to Unix
            // milliseconds.
            let ticks = ((creation.dwHighDateTime as u64) << 32) | (creation.dwLowDateTime as u64);
            // 11644473600 seconds between 1601 and 1970.
            const EPOCH_DIFF_100NS: u64 = 11_644_473_600 * 10_000_000;
            if ticks < EPOCH_DIFF_100NS {
                return None;
            }
            let unix_100ns = ticks - EPOCH_DIFF_100NS;
            Some(unix_100ns / 10_000)
        }
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
mod platform {
    pub fn get_process_start_time_ms(_pid: u32) -> Option<u64> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_process_has_start_time() {
        let value = current_process_start_time_ms();
        assert!(value.is_some(), "expected start-time for current process");
        let value = value.unwrap();
        assert!(value > 0, "start-time must be positive");
    }

    #[test]
    fn start_time_is_stable_across_calls() {
        let pid = std::process::id();
        let first = get_process_start_time_ms(pid).expect("first lookup");
        std::thread::sleep(std::time::Duration::from_millis(20));
        let second = get_process_start_time_ms(pid).expect("second lookup");
        let diff = first.max(second) - first.min(second);
        assert!(
            diff <= START_TIME_MATCH_TOLERANCE_MS,
            "start-time should be stable but differed by {} ms",
            diff
        );
    }

    #[test]
    fn missing_pid_returns_none() {
        // pid 0 is the scheduler/idle and is not enumerable via the per-pid APIs we use.
        assert!(get_process_start_time_ms(0).is_none());
    }

    #[test]
    fn start_times_match_within_tolerance() {
        assert_eq!(
            start_times_match(Some(1_000), Some(1_500)),
            StartTimeMatch::Match
        );
    }

    #[test]
    fn start_times_match_detects_mismatch() {
        match start_times_match(Some(1_000), Some(10_000)) {
            StartTimeMatch::Mismatch { recorded, observed } => {
                assert_eq!(recorded, 1_000);
                assert_eq!(observed, 10_000);
            }
            other => panic!("expected mismatch, got {:?}", other),
        }
    }

    #[test]
    fn start_times_match_unknown_when_either_missing() {
        assert_eq!(start_times_match(None, Some(1)), StartTimeMatch::Unknown);
        assert_eq!(start_times_match(Some(1), None), StartTimeMatch::Unknown);
        assert_eq!(start_times_match(None, None), StartTimeMatch::Unknown);
    }
}
