use bifrost_admin::AdminState;

pub(super) fn persist_socket_summary(state: &AdminState, record_id: &str, total_bytes: usize) {
    if state.get_super_performance_mode() {
        return;
    }
    let status = state
        .sse_hub
        .get_socket_status(record_id)
        .map(|mut status| {
            status.is_open = false;
            status
        });
    let frame_count = status
        .as_ref()
        .map(|status| status.frame_count)
        .unwrap_or(0);
    let last_frame_id = frame_count as u64;
    let mut response_size = status
        .as_ref()
        .map(|status| status.receive_bytes)
        .unwrap_or(0) as usize;
    if response_size == 0 {
        response_size = total_bytes;
    }
    state.update_traffic_by_id(record_id, move |record| {
        record.response_size = response_size;
        record.download_bytes = response_size;
        record.frame_count = frame_count;
        record.last_frame_id = last_frame_id;
        if let Some(ref status) = status {
            record.socket_status = Some(status.clone());
        }
    });
}
