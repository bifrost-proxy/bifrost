use std::io::{self, BufRead};

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufRead, AsyncBufReadExt};

pub const WORKER_PROTOCOL_VERSION: u32 = 1;
pub const WORKER_MAX_FRAME_BYTES: usize = 1024 * 1024;
pub const WORKER_HEARTBEAT_INTERVAL_SECS: u64 = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerKind {
    ExternalCli,
    Browser,
    Asr,
    ImGateway,
    RemoteInvoke,
}

impl WorkerKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ExternalCli => "external_cli",
            Self::Browser => "browser",
            Self::Asr => "asr",
            Self::ImGateway => "im_gateway",
            Self::RemoteInvoke => "remote_invoke",
        }
    }
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerLifecycleState {
    Stopped,
    Starting,
    Ready,
    Busy,
    Degraded,
    Stopping,
    Backoff,
    CircuitOpen,
    Disabled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkerHello {
    pub protocol_version: u32,
    pub worker_kind: WorkerKind,
    pub worker_instance_id: String,
    pub pid: u32,
    pub build_version: String,
    pub startup_token: String,
    #[serde(default)]
    pub capabilities: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkerHeartbeat {
    pub worker_instance_id: String,
    pub timestamp_ms: u64,
    #[serde(default)]
    pub active_jobs: usize,
    #[serde(default)]
    pub queued_jobs: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkerRequest {
    pub request_id: String,
    #[serde(default)]
    pub job_id: Option<String>,
    #[serde(default)]
    pub deadline_unix_ms: Option<u64>,
    pub operation: String,
    #[serde(default)]
    pub payload: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkerResponse {
    pub request_id: String,
    pub ok: bool,
    #[serde(default)]
    pub payload: serde_json::Value,
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkerEvent {
    #[serde(default)]
    pub request_id: Option<String>,
    #[serde(default)]
    pub job_id: Option<String>,
    pub event: String,
    #[serde(default)]
    pub payload: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ParentFrame {
    Request {
        request: WorkerRequest,
    },
    Cancel {
        request_id: String,
        #[serde(default)]
        job_id: Option<String>,
    },
    ConfigApply {
        request_id: String,
        generation: u64,
        payload: serde_json::Value,
    },
    Ping {
        request_id: String,
    },
    Shutdown {
        request_id: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WorkerFrame {
    Hello {
        hello: WorkerHello,
    },
    Ready {
        worker_instance_id: String,
    },
    Heartbeat {
        heartbeat: WorkerHeartbeat,
    },
    Response {
        response: WorkerResponse,
    },
    Event {
        event: WorkerEvent,
    },
    ConfigApplied {
        request_id: String,
        generation: u64,
    },
    Goodbye {
        worker_instance_id: String,
        #[serde(default)]
        reason: Option<String>,
    },
}

pub fn serialize_frame<T: Serialize>(value: &T) -> Result<String, String> {
    let line = serde_json::to_string(value)
        .map_err(|error| format!("serialize worker frame failed: {error}"))?;
    if line.len() > WORKER_MAX_FRAME_BYTES {
        return Err(format!(
            "worker frame exceeds hard limit: {} > {} bytes",
            line.len(),
            WORKER_MAX_FRAME_BYTES
        ));
    }
    Ok(line)
}

pub fn parse_worker_frame(line: &str) -> Result<WorkerFrame, String> {
    if line.len() > WORKER_MAX_FRAME_BYTES {
        return Err(format!(
            "worker frame exceeds hard limit: {} bytes",
            line.len()
        ));
    }
    serde_json::from_str(line).map_err(|error| format!("parse worker frame failed: {error}"))
}

pub fn parse_parent_frame(line: &str) -> Result<ParentFrame, String> {
    if line.len() > WORKER_MAX_FRAME_BYTES {
        return Err(format!(
            "parent frame exceeds hard limit: {} bytes",
            line.len()
        ));
    }
    serde_json::from_str(line).map_err(|error| format!("parse parent frame failed: {error}"))
}

pub(crate) fn read_limited_sync_line<R: BufRead>(
    reader: &mut R,
    max_bytes: usize,
) -> io::Result<Option<String>> {
    let mut line = Vec::with_capacity(max_bytes.min(8 * 1024));
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            return if line.is_empty() {
                Ok(None)
            } else {
                Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "worker IPC frame ended before newline",
                ))
            };
        }
        if let Some(newline) = available.iter().position(|byte| *byte == b'\n') {
            if line.len().saturating_add(newline) > max_bytes {
                return Err(frame_too_large_error(max_bytes));
            }
            line.extend_from_slice(&available[..newline]);
            reader.consume(newline + 1);
            break;
        }
        if line.len().saturating_add(available.len()) > max_bytes {
            return Err(frame_too_large_error(max_bytes));
        }
        line.extend_from_slice(available);
        let consumed = available.len();
        reader.consume(consumed);
    }
    decode_frame_line(line)
}

pub(crate) async fn read_limited_async_line<R: AsyncBufRead + Unpin>(
    reader: &mut R,
    max_bytes: usize,
) -> io::Result<Option<String>> {
    let mut line = Vec::with_capacity(max_bytes.min(8 * 1024));
    loop {
        let available = reader.fill_buf().await?;
        if available.is_empty() {
            return if line.is_empty() {
                Ok(None)
            } else {
                Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "worker IPC frame ended before newline",
                ))
            };
        }
        if let Some(newline) = available.iter().position(|byte| *byte == b'\n') {
            if line.len().saturating_add(newline) > max_bytes {
                return Err(frame_too_large_error(max_bytes));
            }
            line.extend_from_slice(&available[..newline]);
            reader.consume(newline + 1);
            break;
        }
        if line.len().saturating_add(available.len()) > max_bytes {
            return Err(frame_too_large_error(max_bytes));
        }
        line.extend_from_slice(available);
        let consumed = available.len();
        reader.consume(consumed);
    }
    decode_frame_line(line)
}

fn frame_too_large_error(max_bytes: usize) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("worker IPC frame exceeds hard limit of {max_bytes} bytes"),
    )
}

fn decode_frame_line(mut line: Vec<u8>) -> io::Result<Option<String>> {
    if line.last() == Some(&b'\r') {
        line.pop();
    }
    String::from_utf8(line)
        .map(Some)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn worker_frame_round_trip() {
        let frame = WorkerFrame::Heartbeat {
            heartbeat: WorkerHeartbeat {
                worker_instance_id: "instance-1".to_string(),
                timestamp_ms: 42,
                active_jobs: 1,
                queued_jobs: 2,
            },
        };
        let line = serialize_frame(&frame).unwrap();
        match parse_worker_frame(&line).unwrap() {
            WorkerFrame::Heartbeat { heartbeat } => {
                assert_eq!(heartbeat.active_jobs, 1);
                assert_eq!(heartbeat.queued_jobs, 2);
            }
            other => panic!("unexpected frame: {other:?}"),
        }
    }

    #[test]
    fn oversized_frame_is_rejected() {
        let line = "x".repeat(WORKER_MAX_FRAME_BYTES + 1);
        assert!(parse_worker_frame(&line).is_err());
        assert!(parse_parent_frame(&line).is_err());
    }

    #[test]
    fn bounded_reader_rejects_unterminated_frame() {
        let limit = 64;
        let mut reader = std::io::BufReader::new(Cursor::new(vec![b'x'; limit + 1]));
        assert_eq!(
            read_limited_sync_line(&mut reader, limit)
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidData
        );
    }
}
