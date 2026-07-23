use super::*;

struct ActiveSessionMailbox {
    generation: u64,
    sender: mpsc::UnboundedSender<ImEvent>,
    abort_handle: tokio::task::AbortHandle,
    after_close_events: VecDeque<ImEvent>,
}

#[derive(Debug)]
pub(super) struct SessionTaskCompletion {
    pub(super) session_key: String,
    pub(super) generation: u64,
    pub(super) recovered_events: VecDeque<ImEvent>,
}

pub(super) struct SessionMailboxRegistry {
    active: HashMap<String, ActiveSessionMailbox>,
    next_generation: u64,
    completion_tx: mpsc::UnboundedSender<SessionTaskCompletion>,
    completion_rx: mpsc::UnboundedReceiver<SessionTaskCompletion>,
}

pub(super) struct SessionDispatchResult {
    pub(super) unrouted_event: Option<ImEvent>,
    pub(super) delivered: bool,
}

impl SessionMailboxRegistry {
    pub(super) fn new() -> Self {
        let (completion_tx, completion_rx) = mpsc::unbounded_channel();
        Self {
            active: HashMap::new(),
            next_generation: 0,
            completion_tx,
            completion_rx,
        }
    }

    pub(super) fn completion_sender(&self) -> mpsc::UnboundedSender<SessionTaskCompletion> {
        self.completion_tx.clone()
    }

    pub(super) async fn recv_completion(&mut self) -> Option<SessionTaskCompletion> {
        self.completion_rx.recv().await
    }

    pub(super) fn reserve_generation(&mut self) -> u64 {
        self.next_generation = self.next_generation.wrapping_add(1);
        self.next_generation
    }

    pub(super) fn register(
        &mut self,
        session_key: String,
        generation: u64,
        sender: mpsc::UnboundedSender<ImEvent>,
        abort_handle: tokio::task::AbortHandle,
    ) {
        if let Some(previous) = self.active.insert(
            session_key,
            ActiveSessionMailbox {
                generation,
                sender,
                abort_handle,
                after_close_events: VecDeque::new(),
            },
        ) {
            previous.abort_handle.abort();
        }
    }

    /// Distinguishes events delivered to a live mailbox from events buffered
    /// after its receiver closed. The provider loop records deduplication only
    /// for delivered events; buffered events must be replayed through the
    /// normal path before they become visible to the deduplication window.
    pub(super) fn dispatch(&mut self, event: ImEvent) -> SessionDispatchResult {
        let session_key = session_key_for_event(&event);
        let Some(mailbox) = self.active.get_mut(&session_key) else {
            return SessionDispatchResult {
                unrouted_event: Some(event),
                delivered: false,
            };
        };
        match mailbox.sender.send(event) {
            Ok(()) => SessionDispatchResult {
                unrouted_event: None,
                delivered: true,
            },
            Err(error) => {
                mailbox.after_close_events.push_back(error.0);
                SessionDispatchResult {
                    unrouted_event: None,
                    delivered: false,
                }
            }
        }
    }

    pub(super) fn finish(&mut self, completion: SessionTaskCompletion) -> VecDeque<ImEvent> {
        let mut recovered_events = completion.recovered_events;
        if self
            .active
            .get(&completion.session_key)
            .is_some_and(|mailbox| mailbox.generation == completion.generation)
        {
            if let Some(mailbox) = self.active.remove(&completion.session_key) {
                recovered_events.extend(mailbox.after_close_events);
            }
        }
        recovered_events
    }

    #[cfg(test)]
    pub(super) fn contains(&self, session_key: &str) -> bool {
        self.active.contains_key(session_key)
    }

    pub(super) fn is_empty(&self) -> bool {
        self.active.is_empty()
    }
}

impl Drop for SessionMailboxRegistry {
    fn drop(&mut self) {
        for mailbox in self.active.values() {
            mailbox.abort_handle.abort();
        }
    }
}

pub(super) struct SessionTaskCompletionGuard {
    completion_tx: mpsc::UnboundedSender<SessionTaskCompletion>,
    completion: Option<SessionTaskCompletion>,
    agent_session_manager: Arc<ImAgentSessionManager>,
}

impl SessionTaskCompletionGuard {
    pub(super) fn new(
        completion_tx: mpsc::UnboundedSender<SessionTaskCompletion>,
        session_key: String,
        generation: u64,
        agent_session_manager: Arc<ImAgentSessionManager>,
    ) -> Self {
        Self {
            completion_tx,
            completion: Some(SessionTaskCompletion {
                session_key,
                generation,
                recovered_events: VecDeque::new(),
            }),
            agent_session_manager,
        }
    }

    pub(super) fn complete(mut self, recovered_events: VecDeque<ImEvent>) {
        if let Some(mut completion) = self.completion.take() {
            completion.recovered_events = recovered_events;
            let _ = self.completion_tx.send(completion);
        }
    }
}

impl Drop for SessionTaskCompletionGuard {
    fn drop(&mut self) {
        if let Some(completion) = self.completion.take() {
            self.agent_session_manager
                .release_active(&completion.session_key);
            let _ = self.completion_tx.send(completion);
        }
    }
}

pub(super) struct ExternalCliChatTaskContext {
    pub(super) client: ImProviderClient,
    pub(super) provider: ImProviderConfig,
    pub(super) provider_store: Arc<ImProviderStore>,
    pub(super) event: ImEvent,
    pub(super) message_log_store: Arc<ImMessageLogStore>,
    pub(super) agent_config_store: Arc<ImAgentConfigStore>,
    pub(super) external_cli_config_store:
        Arc<crate::im_gateway::external_cli::ExternalCliConfigStore>,
    pub(super) agent_session_manager: Arc<ImAgentSessionManager>,
    pub(super) queue_manager: Arc<SessionQueueManager>,
    pub(super) progress_registry: Arc<ImAgentProgressRegistry>,
    pub(super) event_store: Arc<ImEventStore>,
    pub(super) group_context_store: Arc<ImGroupContextStore>,
}

pub(super) fn spawn_external_cli_agent_chat(
    registry: &mut SessionMailboxRegistry,
    ctx: ExternalCliChatTaskContext,
    input: ExternalCliChatInput,
) {
    let session_key = input.session_key.clone();
    let generation = registry.reserve_generation();
    let completion_tx = registry.completion_sender();
    let (session_tx, mut session_rx) = mpsc::unbounded_channel();
    let guard_session_manager = Arc::clone(&ctx.agent_session_manager);
    let guard_session_key = session_key.clone();
    let task = tokio::spawn(async move {
        let guard = SessionTaskCompletionGuard::new(
            completion_tx,
            guard_session_key,
            generation,
            guard_session_manager,
        );
        run_external_cli_agent_chat(
            ExternalCliChatContext {
                rx: &mut session_rx,
                client: &ctx.client,
                provider: &ctx.provider,
                provider_store: &ctx.provider_store,
                event: &ctx.event,
                message_log_store: &ctx.message_log_store,
                agent_config_store: &ctx.agent_config_store,
                external_cli_config_store: &ctx.external_cli_config_store,
                agent_session_manager: &ctx.agent_session_manager,
                queue_manager: &ctx.queue_manager,
                progress_registry: &ctx.progress_registry,
                event_store: &ctx.event_store,
                group_context_store: &ctx.group_context_store,
            },
            input,
        )
        .await;
        session_rx.close();
        let recovered_events = std::iter::from_fn(|| session_rx.try_recv().ok()).collect();
        guard.complete(recovered_events);
    });
    registry.register(session_key, generation, session_tx, task.abort_handle());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_event(chat_type: &str, chat_id: &str, user_id: &str, event_id: &str) -> ImEvent {
        ImEvent {
            event_id: event_id.to_string(),
            provider_id: "provider".to_string(),
            provider_type: ImProviderType::Feishu,
            event_type: "message.receive".to_string(),
            source: crate::im_gateway::types::ImEventSource {
                chat_id: Some(chat_id.to_string()),
                chat_type: Some(chat_type.to_string()),
                user_id: Some(user_id.to_string()),
                user_name: None,
                sender_type: Some("user".to_string()),
                message_id: Some(event_id.to_string()),
            },
            message: None,
            received_at: 1,
            raw_digest: None,
        }
    }

    fn pending_abort_handle() -> tokio::task::AbortHandle {
        tokio::spawn(std::future::pending::<()>()).abort_handle()
    }

    #[tokio::test]
    async fn dispatches_group_and_direct_events_to_independent_mailboxes() {
        let mut registry = SessionMailboxRegistry::new();
        let group_key =
            crate::im_gateway::group_context::build_group_session_key("provider", "group-a");
        let direct_key = build_session_key("provider", Some("user-a"));
        let (group_tx, mut group_rx) = mpsc::unbounded_channel();
        let group_generation = registry.reserve_generation();
        registry.register(
            group_key.clone(),
            group_generation,
            group_tx,
            pending_abort_handle(),
        );
        let (direct_tx, mut direct_rx) = mpsc::unbounded_channel();
        let direct_generation = registry.reserve_generation();
        registry.register(
            direct_key.clone(),
            direct_generation,
            direct_tx,
            pending_abort_handle(),
        );

        let group_result =
            registry.dispatch(test_event("group", "group-a", "sender", "group-event"));
        assert!(group_result.delivered);
        assert!(group_result.unrouted_event.is_none());
        let direct_result =
            registry.dispatch(test_event("p2p", "direct-chat", "user-a", "direct-event"));
        assert!(direct_result.delivered);
        assert!(direct_result.unrouted_event.is_none());
        assert_eq!(group_rx.recv().await.unwrap().event_id, "group-event");
        assert_eq!(direct_rx.recv().await.unwrap().event_id, "direct-event");
        assert!(registry.contains(&group_key));
        assert!(registry.contains(&direct_key));
    }

    #[tokio::test]
    async fn stale_completion_cannot_remove_replacement_mailbox() {
        let mut registry = SessionMailboxRegistry::new();
        let session_key = build_session_key("provider", Some("user-a"));
        let (first_tx, _first_rx) = mpsc::unbounded_channel();
        let first_generation = registry.reserve_generation();
        registry.register(
            session_key.clone(),
            first_generation,
            first_tx,
            pending_abort_handle(),
        );
        let (replacement_tx, _replacement_rx) = mpsc::unbounded_channel();
        let replacement_generation = registry.reserve_generation();
        registry.register(
            session_key.clone(),
            replacement_generation,
            replacement_tx,
            pending_abort_handle(),
        );

        registry.finish(SessionTaskCompletion {
            session_key: session_key.clone(),
            generation: first_generation,
            recovered_events: VecDeque::new(),
        });
        assert!(registry.contains(&session_key));

        registry.finish(SessionTaskCompletion {
            session_key: session_key.clone(),
            generation: replacement_generation,
            recovered_events: VecDeque::new(),
        });
        assert!(!registry.contains(&session_key));
    }

    #[tokio::test]
    async fn closed_mailbox_replays_event_after_older_buffered_events() {
        let mut registry = SessionMailboxRegistry::new();
        let session_key = build_session_key("provider", Some("user-a"));
        let (sender, mut receiver) = mpsc::unbounded_channel();
        let generation = registry.reserve_generation();
        registry.register(
            session_key.clone(),
            generation,
            sender,
            pending_abort_handle(),
        );
        receiver.close();
        let event = test_event("p2p", "direct-chat", "user-a", "after-close-event");

        let result = registry.dispatch(event);
        assert!(!result.delivered);
        assert!(result.unrouted_event.is_none());
        assert!(registry.contains(&session_key));
        let buffered = test_event("p2p", "direct-chat", "user-a", "buffered-event");
        let recovered = registry.finish(SessionTaskCompletion {
            session_key: session_key.clone(),
            generation,
            recovered_events: VecDeque::from([buffered]),
        });
        assert_eq!(recovered.len(), 2);
        assert_eq!(recovered[0].event_id, "buffered-event");
        assert_eq!(recovered[1].event_id, "after-close-event");
        assert!(!registry.contains(&session_key));
    }

    #[tokio::test]
    async fn completion_returns_events_buffered_by_a_short_lived_task() {
        let mut registry = SessionMailboxRegistry::new();
        let session_key = build_session_key("provider", Some("user-a"));
        let recovered_event = test_event("p2p", "direct-chat", "user-a", "buffered-event");

        let recovered = registry.finish(SessionTaskCompletion {
            session_key,
            generation: 1,
            recovered_events: VecDeque::from([recovered_event]),
        });
        assert_eq!(recovered.len(), 1);
        assert_eq!(recovered[0].event_id, "buffered-event");
    }
}
