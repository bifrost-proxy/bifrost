use serde::{Deserialize, Serialize};

use crate::traffic::{MatchedRule, SocketStatus, TrafficRecord};

#[allow(non_snake_case)]
pub mod TrafficFlags {
    pub const IS_TUNNEL: u32 = 1 << 0;
    pub const IS_WEBSOCKET: u32 = 1 << 1;
    pub const IS_SSE: u32 = 1 << 2;
    pub const IS_H3: u32 = 1 << 3;
    pub const HAS_RULE_HIT: u32 = 1 << 4;
    pub const IS_REPLAY: u32 = 1 << 5;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrafficDbStats {
    pub record_count: usize,
    pub db_size: u64,
    pub db_path: String,
    pub max_records: usize,
    pub retention_hours: u64,
    pub current_sequence: u64,
    pub oldest_timestamp: Option<u64>,
    pub newest_timestamp: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrafficSummaryCompact {
    pub id: String,
    pub seq: u64,
    pub ts: u64,
    pub m: String,
    pub h: String,
    pub p: String,
    pub s: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ct: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub req_ct: Option<String>,
    pub req_sz: usize,
    pub res_sz: usize,
    #[serde(default)]
    pub up: usize,
    #[serde(default)]
    pub down: usize,
    pub dur: u64,
    #[serde(default)]
    pub lp: u16,
    pub proto: String,
    pub cip: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capp: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpid: Option<u32>,
    pub flags: u32,
    pub fc: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ss: Option<SocketStatus>,
    pub st: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub et: Option<String>,
    pub rc: usize,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub rp: Vec<String>,
}

impl TrafficSummaryCompact {
    pub fn from_record(record: &TrafficRecord) -> Self {
        let mut flags = 0u32;
        if record.is_tunnel {
            flags |= TrafficFlags::IS_TUNNEL;
        }
        if record.is_websocket {
            flags |= TrafficFlags::IS_WEBSOCKET;
        }
        if record.is_sse {
            flags |= TrafficFlags::IS_SSE;
        }
        if record.is_h3 {
            flags |= TrafficFlags::IS_H3;
        }
        if record.has_rule_hit {
            flags |= TrafficFlags::HAS_RULE_HIT;
        }
        if record.is_replay {
            flags |= TrafficFlags::IS_REPLAY;
        }

        let start_time = format_timestamp_ms(record.timestamp);
        let end_time = if record.duration_ms > 0 {
            Some(format_timestamp_ms(record.timestamp + record.duration_ms))
        } else {
            None
        };

        let (rc, rp) = summarize_matched_rules(record.matched_rules.as_deref());

        Self {
            id: record.id.clone(),
            seq: record.sequence,
            ts: record.timestamp,
            m: record.method.clone(),
            h: record.host.clone(),
            p: record.path.clone(),
            s: record.status,
            ct: record.content_type.clone(),
            req_ct: record.request_content_type.clone(),
            req_sz: record.request_size,
            res_sz: record.response_size,
            up: record.upload_bytes,
            down: record.download_bytes,
            dur: record.duration_ms,
            lp: record.listener_port,
            proto: record.protocol.clone(),
            cip: record.client_ip.clone(),
            capp: record.client_app.clone(),
            cpid: record.client_pid,
            flags,
            fc: record.frame_count,
            ss: record.socket_status.clone(),
            st: start_time,
            et: end_time,
            rc,
            rp,
        }
    }

    pub fn is_tunnel(&self) -> bool {
        self.flags & TrafficFlags::IS_TUNNEL != 0
    }

    pub fn is_websocket(&self) -> bool {
        self.flags & TrafficFlags::IS_WEBSOCKET != 0
    }

    pub fn is_sse(&self) -> bool {
        self.flags & TrafficFlags::IS_SSE != 0
    }

    pub fn is_h3(&self) -> bool {
        self.flags & TrafficFlags::IS_H3 != 0
    }

    pub fn has_rule_hit(&self) -> bool {
        self.flags & TrafficFlags::HAS_RULE_HIT != 0
    }

    pub fn is_replay(&self) -> bool {
        self.flags & TrafficFlags::IS_REPLAY != 0
    }
}

pub fn build_socket_status_summary(
    is_open: bool,
    send_count: u64,
    receive_count: u64,
    send_bytes: u64,
    receive_bytes: u64,
    frame_count: usize,
) -> Option<SocketStatus> {
    if !is_open
        && send_count == 0
        && receive_count == 0
        && send_bytes == 0
        && receive_bytes == 0
        && frame_count == 0
    {
        return None;
    }

    Some(SocketStatus {
        is_open,
        send_count,
        receive_count,
        send_bytes,
        receive_bytes,
        frame_count,
        close_code: None,
        close_reason: None,
    })
}

pub fn summarize_matched_rules(rules: Option<&[MatchedRule]>) -> (usize, Vec<String>) {
    match rules {
        Some(rules) => {
            let count = rules.len();
            let protocols: Vec<String> = rules
                .iter()
                .map(|r| r.protocol.clone())
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect();
            (count, protocols)
        }
        None => (0, vec![]),
    }
}

fn format_timestamp_ms(timestamp_ms: u64) -> String {
    use chrono::{Local, TimeZone};
    let secs = (timestamp_ms / 1000) as i64;
    let nanos = ((timestamp_ms % 1000) * 1_000_000) as u32;
    Local
        .timestamp_opt(secs, nanos)
        .single()
        .map(|dt| dt.format("%Y-%m-%d %H:%M:%S%.3f").to_string())
        .unwrap_or_else(|| "-".to_string())
}

pub fn encode_flags(record: &TrafficRecord) -> u32 {
    let mut flags = 0u32;
    if record.is_tunnel {
        flags |= TrafficFlags::IS_TUNNEL;
    }
    if record.is_websocket {
        flags |= TrafficFlags::IS_WEBSOCKET;
    }
    if record.is_sse {
        flags |= TrafficFlags::IS_SSE;
    }
    if record.is_h3 {
        flags |= TrafficFlags::IS_H3;
    }
    if record.has_rule_hit {
        flags |= TrafficFlags::HAS_RULE_HIT;
    }
    if record.is_replay {
        flags |= TrafficFlags::IS_REPLAY;
    }
    flags
}
