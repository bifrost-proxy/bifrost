pub mod agent;
pub mod chatgpt_web;
pub mod connection;
pub mod event_router;
pub mod event_store;
pub mod external_cli;
pub mod feishu;
pub mod group_context;
pub mod markdown_converter;
pub mod message_log_store;
pub mod outbox_store;
pub mod progress_card;
pub mod provider;
pub mod provider_store;
pub mod queue_manager;
pub mod route_store;
pub mod run_store;
pub mod schedule_store;
pub mod schedule_tools;
pub mod scheduler;
pub mod session_state;
pub mod target_store;
pub mod task_executor;
pub mod types;
pub mod weixin;
mod weixin_context_store;
pub mod weixin_progress;
mod weixin_sync_store;

pub use agent::{ImAgentConfig, ImAgentConfigStore, ImAgentSessionManager};
pub use connection::ImConnectionManager;
pub use event_store::ImEventStore;
pub use group_context::ImGroupContextStore;
pub use message_log_store::ImMessageLogStore;
pub use outbox_store::{ImOutboxBegin, ImOutboxStore};
pub use progress_card::{ImAgentProgressRegistry, ImProgressCardCapability};
pub use provider::ImProvider;
pub use provider_store::ImProviderStore;
pub use queue_manager::SessionQueueManager;
pub use route_store::ImRouteStore;
pub use run_store::ImRunStore;
pub use schedule_store::ImScheduleStore;
pub use scheduler::ImScheduler;
pub use target_store::ImTargetStore;
pub use types::*;
pub use weixin_progress::WeixinProgressSession;

#[cfg(test)]
mod tests {
    use super::WeixinProgressSession;

    #[test]
    fn weixin_progress_session_remains_sendable_through_public_gateway_api() {
        fn assert_send<T: Send>() {}

        assert_send::<WeixinProgressSession>();
    }
}
