use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct DownloadTuning {
    pub(super) connect_timeout_secs: u64,
    pub(super) download_timeout_secs: u64,
    pub(super) mirror_probe_timeout_secs: u64,
    pub(super) download_tries: usize,
}

impl Default for DownloadTuning {
    fn default() -> Self {
        Self {
            connect_timeout_secs: DOWNLOAD_CONNECT_TIMEOUT_SECS,
            download_timeout_secs: DOWNLOAD_TIMEOUT_SECS,
            mirror_probe_timeout_secs: MIRROR_PROBE_TIMEOUT_SECS,
            download_tries: DOWNLOAD_TRIES,
        }
    }
}

impl DownloadTuning {
    pub(super) fn from_env() -> Self {
        Self {
            connect_timeout_secs: positive_env_u64(
                "BIFROST_DOWNLOAD_CONNECT_TIMEOUT",
                DOWNLOAD_CONNECT_TIMEOUT_SECS,
            ),
            download_timeout_secs: positive_env_u64(
                "BIFROST_DOWNLOAD_TIMEOUT",
                DOWNLOAD_TIMEOUT_SECS,
            ),
            mirror_probe_timeout_secs: positive_env_u64(
                "BIFROST_MIRROR_PROBE_TIMEOUT",
                MIRROR_PROBE_TIMEOUT_SECS,
            ),
            download_tries: positive_env_usize("BIFROST_DOWNLOAD_TRIES", DOWNLOAD_TRIES),
        }
    }
}

pub(super) fn parse_positive_u64(value: Option<&str>, default: u64) -> u64 {
    value
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default)
}

pub(super) fn parse_positive_usize(value: Option<&str>, default: usize) -> usize {
    value
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default)
}

fn positive_env_u64(name: &str, default: u64) -> u64 {
    parse_positive_u64(env::var(name).ok().as_deref(), default)
}

fn positive_env_usize(name: &str, default: usize) -> usize {
    parse_positive_usize(env::var(name).ok().as_deref(), default)
}
