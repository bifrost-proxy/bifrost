pub mod asr;
pub mod im_broker;
pub mod im_gateway;
mod jobs;
mod mode;
mod process;
mod protocol;
pub mod remote_broker;
pub mod remote_execution;
pub mod remote_invoke;
mod supervisor;
mod worker_stdio;

pub use jobs::{
    artifact as worker_artifact, cancel_rejected as worker_job_cancel_rejected,
    cancel_target as worker_job_cancel_target, get_job as worker_job, list_jobs as worker_jobs,
    register_artifact as register_worker_artifact, WorkerArtifactRecord, WorkerJobEventRecord,
    WorkerJobRecord, WorkerJobStatus,
};
pub(crate) use jobs::{
    begin_request as begin_worker_job, mark_cancelled as mark_worker_job_cancelled,
    mark_failed as mark_worker_job_failed, mark_running as mark_worker_job_running,
    mark_succeeded as mark_worker_job_succeeded, record_named_event as record_worker_job_event,
};
#[cfg(test)]
pub(crate) use jobs::{
    clear_for_tests as clear_worker_jobs_for_tests,
    test_guard_async as worker_jobs_test_guard_async,
    test_guard_async as worker_runtime_test_guard_async,
};
pub use mode::{execution_mode, execution_mode_env, worker_execution_enabled, ExecutionMode};
#[cfg(all(test, unix))]
pub(crate) use process::test_shell_worker_spec;
pub use process::{ManagedWorker, ManagedWorkerSnapshot, WorkerSpawnSpec};
pub use protocol::{
    now_ms as worker_now_ms, ParentFrame, WorkerEvent, WorkerFrame, WorkerKind,
    WorkerLifecycleState, WorkerRequest, WorkerResponse, WORKER_MAX_FRAME_BYTES,
    WORKER_PROTOCOL_VERSION,
};
pub(crate) use protocol::{read_limited_async_line, read_limited_sync_line};
pub use supervisor::{global_worker_supervisor, SharedWorkerSupervisor, WorkerSupervisor};
pub use worker_stdio::{run_worker_stdio, WorkerStdioContext};

pub fn run_auxiliary_worker(kind: &str, admin_host: &str, admin_port: u16) -> Result<(), String> {
    match kind.trim().to_ascii_lowercase().as_str() {
        "browser" => crate::im_gateway::chatgpt_web::worker::run_browser_worker_stdio(),
        "asr" => asr::run_asr_worker_stdio(),
        "im_gateway" | "im-gateway" => {
            im_gateway::run_im_gateway_worker_stdio(admin_host, admin_port)
        }
        "remote_invoke" | "remote-invoke" => {
            remote_invoke::run_remote_invoke_worker_stdio(admin_host, admin_port)
        }
        "remote_execution" | "remote-execution" => {
            remote_execution::run_remote_execution_worker_stdio(admin_host, admin_port)
        }
        other => Err(format!("unsupported auxiliary worker kind '{other}'")),
    }
}

pub async fn shutdown_all_workers() {
    im_gateway::stop_runtime_controller();
    remote_invoke::stop_runtime_controller();
    global_worker_supervisor()
        .stop_all(std::time::Duration::from_secs(5))
        .await;
}
pub async fn worker_snapshots() -> Vec<ManagedWorkerSnapshot> {
    global_worker_supervisor().snapshots().await
}
