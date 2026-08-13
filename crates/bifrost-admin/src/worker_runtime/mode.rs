use super::WorkerKind;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionMode {
    Legacy,
    Worker,
}

impl ExecutionMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Legacy => "legacy",
            Self::Worker => "worker",
        }
    }
}

pub const fn execution_mode_env(kind: WorkerKind) -> &'static str {
    match kind {
        WorkerKind::ExternalCli => "BIFROST_EXTERNAL_CLI_EXECUTION_MODE",
        WorkerKind::Browser => "BIFROST_BROWSER_EXECUTION_MODE",
        WorkerKind::Asr => "BIFROST_ASR_EXECUTION_MODE",
        WorkerKind::ImGateway => "BIFROST_IM_GATEWAY_EXECUTION_MODE",
        WorkerKind::RemoteInvoke => "BIFROST_REMOTE_INVOKE_EXECUTION_MODE",
        WorkerKind::RemoteExecution => "BIFROST_REMOTE_EXECUTION_MODE",
    }
}

pub fn execution_mode(kind: WorkerKind) -> ExecutionMode {
    let env_name = execution_mode_env(kind);
    let Some(raw) = std::env::var_os(env_name) else {
        return ExecutionMode::Worker;
    };
    let raw = raw.to_string_lossy();
    match parse_execution_mode(&raw) {
        Some(mode) => mode,
        None => {
            tracing::warn!(
                worker_kind = kind.as_str(),
                env_name,
                value = %raw,
                "invalid auxiliary execution mode; defaulting to worker isolation"
            );
            ExecutionMode::Worker
        }
    }
}

pub fn worker_execution_enabled(kind: WorkerKind) -> bool {
    execution_mode(kind) == ExecutionMode::Worker
}

fn parse_execution_mode(value: &str) -> Option<ExecutionMode> {
    match value.trim().to_ascii_lowercase().replace('-', "_").as_str() {
        "legacy" | "inline" | "in_process" => Some(ExecutionMode::Legacy),
        "worker" | "isolated" | "out_of_process" => Some(ExecutionMode::Worker),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_supported_execution_mode_aliases() {
        assert_eq!(parse_execution_mode("legacy"), Some(ExecutionMode::Legacy));
        assert_eq!(
            parse_execution_mode("in-process"),
            Some(ExecutionMode::Legacy)
        );
        assert_eq!(parse_execution_mode("worker"), Some(ExecutionMode::Worker));
        assert_eq!(
            parse_execution_mode("out-of-process"),
            Some(ExecutionMode::Worker)
        );
        assert_eq!(parse_execution_mode("unknown"), None);
    }

    #[test]
    fn every_worker_kind_has_a_stable_mode_environment_variable() {
        let cases = [
            (
                WorkerKind::ExternalCli,
                "BIFROST_EXTERNAL_CLI_EXECUTION_MODE",
            ),
            (WorkerKind::Browser, "BIFROST_BROWSER_EXECUTION_MODE"),
            (WorkerKind::Asr, "BIFROST_ASR_EXECUTION_MODE"),
            (WorkerKind::ImGateway, "BIFROST_IM_GATEWAY_EXECUTION_MODE"),
            (
                WorkerKind::RemoteInvoke,
                "BIFROST_REMOTE_INVOKE_EXECUTION_MODE",
            ),
            (WorkerKind::RemoteExecution, "BIFROST_REMOTE_EXECUTION_MODE"),
        ];
        for (kind, expected) in cases {
            assert_eq!(execution_mode_env(kind), expected);
        }
    }
}
