use std::io::{self, BufRead};

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufRead, AsyncBufReadExt};

pub const WORKER_PROTOCOL_VERSION: u32 = 1;
pub const WORKER_MAX_FRAME_BYTES: usize = 1024 * 1024;
pub const WORKER_HEARTBEAT_INTERVAL_SECS: u64 = 10;
pub const WORKER_MAX_ID_BYTES: usize = 128;
pub const WORKER_MAX_OPERATION_BYTES: usize = 128;
pub const WORKER_MAX_EVENT_BYTES: usize = 128;
pub const WORKER_MAX_CAPABILITIES: usize = 64;
pub const WORKER_MAX_CAPABILITY_BYTES: usize = 128;
pub const WORKER_MAX_ERROR_BYTES: usize = 16 * 1024;

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
    let frame: WorkerFrame = serde_json::from_str(line)
        .map_err(|error| format!("parse worker frame failed: {error}"))?;
    validate_worker_frame(&frame)?;
    Ok(frame)
}

pub fn parse_parent_frame(line: &str) -> Result<ParentFrame, String> {
    if line.len() > WORKER_MAX_FRAME_BYTES {
        return Err(format!(
            "parent frame exceeds hard limit: {} bytes",
            line.len()
        ));
    }
    let frame: ParentFrame = serde_json::from_str(line)
        .map_err(|error| format!("parse parent frame failed: {error}"))?;
    validate_parent_frame(&frame)?;
    Ok(frame)
}

fn validate_worker_frame(frame: &WorkerFrame) -> Result<(), String> {
    match frame {
        WorkerFrame::Hello { hello } => {
            validate_required_metadata(
                "worker instance id",
                &hello.worker_instance_id,
                WORKER_MAX_ID_BYTES,
            )?;
            validate_metadata("build version", &hello.build_version, WORKER_MAX_ID_BYTES)?;
            validate_required_metadata("startup token", &hello.startup_token, WORKER_MAX_ID_BYTES)?;
            if hello.capabilities.len() > WORKER_MAX_CAPABILITIES {
                return Err(format!(
                    "worker capabilities exceed hard limit: {} > {}",
                    hello.capabilities.len(),
                    WORKER_MAX_CAPABILITIES
                ));
            }
            for capability in &hello.capabilities {
                validate_required_metadata(
                    "worker capability",
                    capability,
                    WORKER_MAX_CAPABILITY_BYTES,
                )?;
            }
        }
        WorkerFrame::Ready { worker_instance_id }
        | WorkerFrame::Goodbye {
            worker_instance_id, ..
        } => validate_required_metadata(
            "worker instance id",
            worker_instance_id,
            WORKER_MAX_ID_BYTES,
        )?,
        WorkerFrame::Heartbeat { heartbeat } => validate_required_metadata(
            "worker instance id",
            &heartbeat.worker_instance_id,
            WORKER_MAX_ID_BYTES,
        )?,
        WorkerFrame::Response { response } => {
            validate_required_metadata("request id", &response.request_id, WORKER_MAX_ID_BYTES)?;
            if let Some(error) = response.error.as_deref() {
                validate_metadata("worker error", error, WORKER_MAX_ERROR_BYTES)?;
            }
        }
        WorkerFrame::Event { event } => {
            validate_optional_id("request id", event.request_id.as_deref())?;
            validate_optional_id("job id", event.job_id.as_deref())?;
            validate_required_metadata("event", &event.event, WORKER_MAX_EVENT_BYTES)?;
        }
        WorkerFrame::ConfigApplied { request_id, .. } => {
            validate_required_metadata("request id", request_id, WORKER_MAX_ID_BYTES)?;
        }
    }
    if let WorkerFrame::Goodbye {
        reason: Some(reason),
        ..
    } = frame
    {
        validate_metadata("goodbye reason", reason, WORKER_MAX_ERROR_BYTES)?;
    }
    Ok(())
}

fn validate_parent_frame(frame: &ParentFrame) -> Result<(), String> {
    match frame {
        ParentFrame::Request { request } => {
            validate_required_metadata("request id", &request.request_id, WORKER_MAX_ID_BYTES)?;
            validate_optional_id("job id", request.job_id.as_deref())?;
            validate_required_metadata(
                "operation",
                &request.operation,
                WORKER_MAX_OPERATION_BYTES,
            )?;
        }
        ParentFrame::Cancel { request_id, job_id } => {
            validate_required_metadata("request id", request_id, WORKER_MAX_ID_BYTES)?;
            validate_optional_id("job id", job_id.as_deref())?;
        }
        ParentFrame::ConfigApply { request_id, .. }
        | ParentFrame::Ping { request_id }
        | ParentFrame::Shutdown { request_id } => {
            validate_required_metadata("request id", request_id, WORKER_MAX_ID_BYTES)?;
        }
    }
    Ok(())
}

fn validate_optional_id(label: &str, value: Option<&str>) -> Result<(), String> {
    if let Some(value) = value {
        validate_required_metadata(label, value, WORKER_MAX_ID_BYTES)?;
    }
    Ok(())
}

fn validate_required_metadata(label: &str, value: &str, max_bytes: usize) -> Result<(), String> {
    if value.trim().is_empty() {
        return Err(format!("{label} must not be empty"));
    }
    validate_metadata(label, value, max_bytes)
}

fn validate_metadata(label: &str, value: &str, max_bytes: usize) -> Result<(), String> {
    if value.len() > max_bytes {
        return Err(format!(
            "{label} exceeds hard limit: {} > {} bytes",
            value.len(),
            max_bytes
        ));
    }
    if value.chars().any(char::is_control) {
        return Err(format!("{label} contains control characters"));
    }
    Ok(())
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

    #[test]
    fn oversized_request_metadata_is_rejected() {
        let frame = ParentFrame::Request {
            request: WorkerRequest {
                request_id: "request-1".to_string(),
                job_id: None,
                deadline_unix_ms: None,
                operation: "x".repeat(WORKER_MAX_OPERATION_BYTES + 1),
                payload: serde_json::Value::Null,
            },
        };
        let line = serde_json::to_string(&frame).unwrap();
        let error = parse_parent_frame(&line).unwrap_err();
        assert!(error.contains("operation exceeds hard limit"));
    }

    #[test]
    fn control_characters_in_event_metadata_are_rejected() {
        let frame = WorkerFrame::Event {
            event: WorkerEvent {
                request_id: None,
                job_id: None,
                event: "progress\nspoof".to_string(),
                payload: serde_json::Value::Null,
            },
        };
        let line = serde_json::to_string(&frame).unwrap();
        let error = parse_worker_frame(&line).unwrap_err();
        assert!(error.contains("control characters"));
    }

    #[test]
    fn excessive_capability_count_is_rejected() {
        let frame = WorkerFrame::Hello {
            hello: WorkerHello {
                protocol_version: WORKER_PROTOCOL_VERSION,
                worker_kind: WorkerKind::Browser,
                worker_instance_id: "instance-1".to_string(),
                pid: 1,
                build_version: "test".to_string(),
                startup_token: "token".to_string(),
                capabilities: vec!["capability".to_string(); WORKER_MAX_CAPABILITIES + 1],
            },
        };
        let line = serde_json::to_string(&frame).unwrap();
        let error = parse_worker_frame(&line).unwrap_err();
        assert!(error.contains("capabilities exceed hard limit"));
    }
}
