use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use bifrost_agent::AgentTurnProgressEvent;
use tracing::{debug, warn};

use super::types::{ImProviderConfig, ImTarget};
use super::weixin::{WeixinProvider, WeixinToolProgress};

struct PendingToolCall {
    tool_name: String,
    arguments_key: String,
    tool_call_id: Option<String>,
    sequence: u64,
}

pub(super) struct PreparedWeixinProgress {
    client_msg_id: String,
    tool_name: String,
    tool_call_id: Option<String>,
    finished_status: Option<&'static str>,
}

pub struct WeixinProgressSession {
    provider: Arc<WeixinProvider>,
    config: ImProviderConfig,
    target: ImTarget,
    channel_run_id: String,
    next_sequence: u64,
    pending_tools: VecDeque<PendingToolCall>,
    typing_shutdown: Option<tokio::sync::oneshot::Sender<()>>,
    typing_task: Option<tokio::task::JoinHandle<()>>,
}

impl WeixinProgressSession {
    pub async fn start(
        provider: Arc<WeixinProvider>,
        config: ImProviderConfig,
        target: ImTarget,
    ) -> Self {
        let channel_run_id = provider.begin_channel_run(&config, &target);
        let mut session = Self {
            provider,
            config,
            target,
            channel_run_id,
            next_sequence: 0,
            pending_tools: VecDeque::new(),
            typing_shutdown: None,
            typing_task: None,
        };
        let _ = tokio::time::timeout(Duration::from_secs(1), session.start_typing()).await;
        session
    }

    #[cfg(test)]
    pub(crate) async fn start_for_test(
        provider: Arc<WeixinProvider>,
        config: ImProviderConfig,
        target: ImTarget,
        keepalive_interval: Duration,
    ) -> Self {
        let channel_run_id = provider.begin_channel_run(&config, &target);
        let mut session = Self {
            provider,
            config,
            target,
            channel_run_id,
            next_sequence: 0,
            pending_tools: VecDeque::new(),
            typing_shutdown: None,
            typing_task: None,
        };
        session.start_typing_with_interval(keepalive_interval).await;
        session
    }

    pub fn channel_run_id(&self) -> &str {
        &self.channel_run_id
    }

    async fn start_typing(&mut self) {
        self.start_typing_with_interval(Duration::from_secs(5))
            .await;
    }

    async fn start_typing_with_interval(&mut self, keepalive_interval: Duration) {
        let mut active_ticket = None;
        for attempt in 0..2 {
            let ticket = match self
                .provider
                .typing_ticket(&self.config, &self.target)
                .await
            {
                Ok(ticket) => ticket,
                Err(error) => {
                    debug!(
                        provider_id = %self.config.id,
                        attempt,
                        error = %error,
                        "weixin typing unavailable; continuing without typing"
                    );
                    return;
                }
            };
            match self
                .provider
                .send_typing_status(&self.config, &self.target, &ticket, 1)
                .await
            {
                Ok(()) => {
                    active_ticket = Some(ticket);
                    break;
                }
                Err(error) => {
                    self.provider
                        .invalidate_typing_ticket(&self.config, &self.target);
                    warn!(
                        provider_id = %self.config.id,
                        attempt,
                        error = %error,
                        "failed to start weixin typing"
                    );
                }
            }
        }
        let Some(ticket) = active_ticket else {
            return;
        };
        let provider = Arc::clone(&self.provider);
        let config = self.config.clone();
        let target = self.target.clone();
        let (shutdown_tx, mut shutdown_rx) = tokio::sync::oneshot::channel();
        self.typing_shutdown = Some(shutdown_tx);
        self.typing_task = Some(tokio::spawn(async move {
            let mut interval = tokio::time::interval(keepalive_interval);
            interval.tick().await;
            loop {
                tokio::select! {
                    _ = &mut shutdown_rx => break,
                    _ = interval.tick() => {
                        if let Err(error) = provider
                            .send_typing_status(&config, &target, &ticket, 1)
                            .await
                        {
                            provider.invalidate_typing_ticket(&config, &target);
                            warn!(
                                provider_id = %config.id,
                                error = %error,
                                "weixin typing keepalive failed"
                            );
                            break;
                        }
                    }
                }
            }
            if let Err(error) = provider
                .send_typing_status(&config, &target, &ticket, 2)
                .await
            {
                warn!(
                    provider_id = %config.id,
                    error = %error,
                    "failed to cancel weixin typing"
                );
            }
        }));
    }

    pub(super) fn prepare_events(
        &mut self,
        events: Vec<AgentTurnProgressEvent>,
    ) -> Vec<PreparedWeixinProgress> {
        let mut prepared = Vec::new();
        for event in events {
            match event {
                AgentTurnProgressEvent::ToolStarted {
                    tool_name,
                    arguments,
                } => {
                    self.next_sequence = self.next_sequence.saturating_add(1);
                    let tool_call_id = format!(
                        "{}-tool-{:04}",
                        self.channel_run_id.chars().take(8).collect::<String>(),
                        self.next_sequence
                    );
                    let client_id = format!("{}-{}-start", self.channel_run_id, self.next_sequence);
                    prepared.push(PreparedWeixinProgress {
                        client_msg_id: client_id,
                        tool_name: tool_name.clone(),
                        tool_call_id: Some(tool_call_id.clone()),
                        finished_status: None,
                    });
                    self.pending_tools.push_back(PendingToolCall {
                        tool_name,
                        arguments_key: normalize_arguments_key(&arguments),
                        tool_call_id: Some(tool_call_id),
                        sequence: self.next_sequence,
                    });
                }
                AgentTurnProgressEvent::ToolFinished { log, .. } => {
                    let arguments_key = normalize_arguments_key(&log.arguments);
                    let position = self
                        .pending_tools
                        .iter()
                        .position(|pending| {
                            pending.tool_name == log.tool_name
                                && pending.arguments_key == arguments_key
                        })
                        .or_else(|| {
                            let mut matches = self
                                .pending_tools
                                .iter()
                                .enumerate()
                                .filter(|(_, pending)| pending.tool_name == log.tool_name);
                            let first = matches.next().map(|(position, _)| position);
                            first.filter(|_| matches.next().is_none())
                        });
                    let pending = position
                        .and_then(|position| self.pending_tools.remove(position))
                        .unwrap_or_else(|| {
                            self.next_sequence = self.next_sequence.saturating_add(1);
                            PendingToolCall {
                                tool_name: log.tool_name.clone(),
                                arguments_key,
                                tool_call_id: None,
                                sequence: self.next_sequence,
                            }
                        });
                    let client_id = format!("{}-{}-result", self.channel_run_id, pending.sequence);
                    prepared.push(PreparedWeixinProgress {
                        client_msg_id: client_id,
                        tool_name: pending.tool_name,
                        tool_call_id: pending.tool_call_id,
                        finished_status: Some(if log.success { "success" } else { "failed" }),
                    });
                }
                _ => {}
            }
        }
        prepared
    }

    pub(super) fn prepare_delivery(
        &mut self,
        events: Vec<AgentTurnProgressEvent>,
    ) -> (
        Arc<WeixinProvider>,
        ImProviderConfig,
        ImTarget,
        String,
        Vec<PreparedWeixinProgress>,
    ) {
        let prepared = self.prepare_events(events);
        (
            Arc::clone(&self.provider),
            self.config.clone(),
            self.target.clone(),
            self.channel_run_id.clone(),
            prepared,
        )
    }

    pub(super) async fn deliver_prepared(
        provider: Arc<WeixinProvider>,
        config: ImProviderConfig,
        target: ImTarget,
        channel_run_id: String,
        prepared: Vec<PreparedWeixinProgress>,
    ) {
        for progress in prepared {
            let finished = progress.finished_status.is_some();
            if let Err(error) = provider
                .send_tool_progress(
                    &config,
                    &target,
                    WeixinToolProgress {
                        channel_run_id: &channel_run_id,
                        client_msg_id: &progress.client_msg_id,
                        tool_name: &progress.tool_name,
                        tool_call_id: progress.tool_call_id.as_deref(),
                        finished_status: progress.finished_status,
                    },
                )
                .await
            {
                warn!(
                    provider_id = %config.id,
                    tool_name = %progress.tool_name,
                    error = %error,
                    finished,
                    "failed to send weixin tool progress"
                );
            }
        }
    }

    #[cfg(test)]
    pub(crate) async fn apply_events(&mut self, events: Vec<AgentTurnProgressEvent>) {
        let (provider, config, target, channel_run_id, prepared) = self.prepare_delivery(events);
        Self::deliver_prepared(provider, config, target, channel_run_id, prepared).await;
    }

    pub async fn finish(&mut self) {
        if let Some(shutdown) = self.typing_shutdown.take() {
            let _ = shutdown.send(());
        }
        if let Some(mut task) = self.typing_task.take() {
            if tokio::time::timeout(Duration::from_secs(5), &mut task)
                .await
                .is_err()
            {
                task.abort();
            }
        }
    }
}

impl Drop for WeixinProgressSession {
    fn drop(&mut self) {
        if let Some(shutdown) = self.typing_shutdown.take() {
            let _ = shutdown.send(());
        }
    }
}

fn normalize_arguments_key(arguments: &str) -> String {
    arguments.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::im_gateway::types::ImProviderType;

    #[tokio::test]
    async fn finish_aborts_a_typing_task_that_ignores_shutdown() {
        let typing_task = tokio::spawn(std::future::pending::<()>());
        let abort_handle = typing_task.abort_handle();
        let mut session = WeixinProgressSession {
            provider: Arc::new(WeixinProvider::new()),
            config: ImProviderConfig {
                id: "weixin-test".to_string(),
                provider_type: ImProviderType::Weixin,
                display_name: "Weixin Test".to_string(),
                enabled: true,
                base_url: None,
                app_id: None,
                secret_ref: None,
                owner_open_id: None,
                event_connection_enabled: false,
                event_types: Vec::new(),
                agent_config: None,
                created_at: 0,
                updated_at: 0,
            },
            target: ImTarget {
                id: "target".to_string(),
                provider_id: "weixin-test".to_string(),
                display_name: "Target".to_string(),
                receive_id_type: "open_id".to_string(),
                receive_id: "user".to_string(),
                default_msg_type: "text".to_string(),
                enabled: true,
                created_at: 0,
                updated_at: 0,
            },
            channel_run_id: "run".to_string(),
            next_sequence: 0,
            pending_tools: VecDeque::new(),
            typing_shutdown: None,
            typing_task: Some(typing_task),
        };

        session.finish().await;
        tokio::task::yield_now().await;

        assert!(abort_handle.is_finished());
        assert!(session.typing_task.is_none());
    }
}
