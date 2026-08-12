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
        session.start_typing().await;
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

    pub async fn apply_events(&mut self, events: Vec<AgentTurnProgressEvent>) {
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
                    if let Err(error) = self
                        .provider
                        .send_tool_progress(
                            &self.config,
                            &self.target,
                            WeixinToolProgress {
                                channel_run_id: &self.channel_run_id,
                                client_msg_id: &client_id,
                                tool_name: &tool_name,
                                tool_call_id: Some(&tool_call_id),
                                finished_status: None,
                            },
                        )
                        .await
                    {
                        warn!(
                            provider_id = %self.config.id,
                            tool_name,
                            error = %error,
                            "failed to send weixin tool start progress"
                        );
                    }
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
                    let status = if log.success { "success" } else { "failed" };
                    if let Err(error) = self
                        .provider
                        .send_tool_progress(
                            &self.config,
                            &self.target,
                            WeixinToolProgress {
                                channel_run_id: &self.channel_run_id,
                                client_msg_id: &client_id,
                                tool_name: &pending.tool_name,
                                tool_call_id: pending.tool_call_id.as_deref(),
                                finished_status: Some(status),
                            },
                        )
                        .await
                    {
                        warn!(
                            provider_id = %self.config.id,
                            tool_name = %pending.tool_name,
                            error = %error,
                            "failed to send weixin tool result progress"
                        );
                    }
                }
                _ => {}
            }
        }
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
