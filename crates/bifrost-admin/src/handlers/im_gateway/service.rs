use super::*;

pub(super) const IMAGE_ONLY_AGENT_PROMPT: &str = "请理解这张图片，并根据图片内容回答。";
pub(super) const MAX_AGENT_IMAGES_PER_MESSAGE: usize = 6;
pub(super) const MAX_AGENT_REPLY_IMAGE_BYTES: u64 = 10 * 1024 * 1024;

pub(super) static AGENT_REPLY_IMAGE_UPLOAD_CACHE: OnceLock<
    Mutex<HashMap<AgentReplyImageCacheKey, String>>,
> = OnceLock::new();

#[derive(Clone)]
pub(super) struct PendingWeixinLogin {
    pub(super) login: crate::im_gateway::weixin::WeixinLoginStart,
    pub(super) created_at_ms: u64,
}

#[derive(Clone)]
pub(super) enum ImProviderClient {
    Feishu(Arc<crate::im_gateway::feishu::FeishuProvider>),
    Weixin(Arc<WeixinProvider>),
}

impl ImProviderClient {
    pub(super) fn feishu(&self) -> Option<Arc<crate::im_gateway::feishu::FeishuProvider>> {
        match self {
            Self::Feishu(provider) => Some(provider.clone()),
            Self::Weixin(_) => None,
        }
    }

    pub(super) async fn send_card(
        &self,
        config: &ImProviderConfig,
        target: &ImTarget,
        card: serde_json::Value,
        opts: crate::im_gateway::types::SendOptions,
    ) -> bifrost_core::Result<crate::im_gateway::types::SendResult> {
        match self {
            Self::Feishu(provider) => provider.send_card(config, target, card, opts).await,
            Self::Weixin(provider) => provider.send_card(config, target, card, opts).await,
        }
    }

    pub(super) async fn send_text(
        &self,
        config: &ImProviderConfig,
        target: &ImTarget,
        text: &str,
    ) -> bifrost_core::Result<crate::im_gateway::types::SendResult> {
        match self {
            Self::Feishu(provider) => provider.send_text(config, target, text).await,
            Self::Weixin(provider) => provider.send_text(config, target, text).await,
        }
    }

    pub(super) async fn add_reaction(
        &self,
        config: &ImProviderConfig,
        message_id: &str,
        reaction: &str,
    ) -> bifrost_core::Result<bool> {
        match self {
            Self::Feishu(provider) => {
                provider.add_reaction(config, message_id, reaction).await?;
                Ok(true)
            }
            Self::Weixin(_) => Ok(false),
        }
    }

    pub(super) async fn download_message_image_resource(
        &self,
        config: &ImProviderConfig,
        message_id: &str,
        image: &ImImageAttachment,
    ) -> bifrost_core::Result<(String, Vec<u8>)> {
        match self {
            Self::Feishu(provider) => {
                provider
                    .download_message_image_resource(config, message_id, &image.file_key)
                    .await
            }
            Self::Weixin(provider) => {
                provider
                    .download_message_image_resource(config, image)
                    .await
            }
        }
    }
}

// ---------------------------------------------------------------------------
// ImGatewayService
// ---------------------------------------------------------------------------

pub struct ImGatewayService {
    pub provider_store: Arc<ImProviderStore>,
    pub target_store: Arc<ImTargetStore>,
    pub route_store: Arc<ImRouteStore>,
    pub schedule_store: Arc<ImScheduleStore>,
    pub scheduler: Arc<ImScheduler>,
    pub event_store: Arc<ImEventStore>,
    pub run_store: Arc<ImRunStore>,
    pub message_log_store: Arc<ImMessageLogStore>,
    pub connection_manager: Arc<ImConnectionManager>,
    pub agent_config_store: Arc<ImAgentConfigStore>,
    pub agent_client: Arc<ImAgentClient>,
    pub agent_tools: Arc<ImAgentToolRegistry>,
    pub agent_session_manager: Arc<ImAgentSessionManager>,
    pub queue_manager: Arc<SessionQueueManager>,
    pub progress_registry: Arc<ImAgentProgressRegistry>,
    pub(super) weixin_login_pending: Arc<RwLock<HashMap<String, PendingWeixinLogin>>>,
}

impl ImGatewayService {
    pub fn new(data_dir: &std::path::Path) -> Self {
        Self::new_with_agent_proxy_port(data_dir, None)
    }

    pub fn new_with_agent_proxy_port(data_dir: &std::path::Path, proxy_port: Option<u16>) -> Self {
        // Install embedded system skills on startup (idempotent, fingerprint-checked)
        bifrost_agent::install_system_skills();

        // Store agent config under data_dir/agent/ for unified directory structure
        let agent_data_dir = data_dir.join("agent");
        let _ = std::fs::create_dir_all(&agent_data_dir);
        let agent_config_store = Arc::new(ImAgentConfigStore::new(&agent_data_dir));
        let agent_config = agent_config_store.load();
        let schedule_store = Arc::new(ImScheduleStore::new(data_dir));
        let scheduler = Arc::new(ImScheduler::new());
        let target_store = Arc::new(ImTargetStore::new(data_dir));
        let mut agent_tools = ImAgentToolRegistry::with_defaults();
        crate::im_gateway::schedule_tools::register_schedule_tools(
            &mut agent_tools,
            schedule_store.clone(),
            scheduler.clone(),
            target_store.clone(),
            crate::im_gateway::schedule_tools::ScheduleToolContext::default(),
        );
        let agent_tools = Arc::new(agent_tools);
        let ca_cert_path = data_dir.join("certs").join("ca.crt");
        let agent_client = proxy_port
            .map(|port| ImAgentClient::new_with_bifrost_proxy_and_ca(port, Some(&ca_cert_path)))
            .unwrap_or_default();
        Self {
            provider_store: Arc::new(ImProviderStore::new(data_dir)),
            target_store,
            route_store: Arc::new(ImRouteStore::new(data_dir)),
            schedule_store,
            scheduler,
            event_store: Arc::new(ImEventStore::new(data_dir)),
            run_store: Arc::new(ImRunStore::new(data_dir)),
            message_log_store: Arc::new(ImMessageLogStore::new(data_dir)),
            connection_manager: Arc::new(ImConnectionManager::new()),
            agent_config_store,
            agent_client: Arc::new(agent_client),
            agent_tools,
            agent_session_manager: Arc::new(ImAgentSessionManager::new(
                agent_config.get_session_ttl_secs(),
            )),
            queue_manager: Arc::new(SessionQueueManager::new()),
            progress_registry: Arc::new(ImAgentProgressRegistry::new()),
            weixin_login_pending: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn build_agent_tool_registry(
        &self,
        message_channel: Option<crate::im_gateway::types::ImMessageChannelBinding>,
    ) -> Arc<ImAgentToolRegistry> {
        let mut registry = (*self.agent_tools).clone();
        crate::im_gateway::schedule_tools::register_schedule_tools(
            &mut registry,
            self.schedule_store.clone(),
            self.scheduler.clone(),
            self.target_store.clone(),
            crate::im_gateway::schedule_tools::ScheduleToolContext {
                message_channel: message_channel.clone(),
            },
        );
        registry.register(Arc::new(
            crate::im_gateway::send_msg_tool::SendMsgTool::new(
                self.provider_store.clone(),
                self.target_store.clone(),
                self.message_log_store.clone(),
                self.connection_manager.clone(),
                crate::im_gateway::send_msg_tool::SendMsgToolContext { message_channel },
            ),
        ));
        Arc::new(registry)
    }

    pub(super) fn provider_client(&self, provider: &ImProviderConfig) -> ImProviderClient {
        match provider.provider_type {
            crate::im_gateway::types::ImProviderType::Weixin => {
                ImProviderClient::Weixin(self.connection_manager.weixin_provider().clone())
            }
            _ => ImProviderClient::Feishu(self.connection_manager.feishu_provider().clone()),
        }
    }

    /// Auto-connect all configured providers that have a secret.
    /// If owner_open_id is not set, auto-detect it from the Feishu Application API.
    /// Called on Bifrost startup to restore active connections and send online notifications.
    pub async fn auto_connect_providers(self: &Arc<Self>) {
        let providers = self.provider_store.list();
        for mut provider in providers {
            // Only auto-connect providers that have a secret configured
            let has_secret = provider
                .secret_ref
                .as_deref()
                .is_some_and(|s| !s.is_empty());

            if !has_secret {
                info!(
                    provider_id = %provider.id,
                    "skipping auto-connect: missing secret"
                );
                continue;
            }

            // Auto-detect owner_open_id if not set
            let has_owner = provider
                .owner_open_id
                .as_deref()
                .is_some_and(|s| !s.is_empty());

            if !has_owner
                && provider.provider_type == crate::im_gateway::types::ImProviderType::Feishu
            {
                let feishu = self.connection_manager.feishu_provider().clone();
                match feishu.fetch_bot_owner_open_id(&provider).await {
                    Ok(owner_id) => {
                        info!(
                            provider_id = %provider.id,
                            owner_open_id = %owner_id,
                            "auto-detected bot owner on startup"
                        );
                        provider.owner_open_id = Some(owner_id);
                        if let Err(e) = self.provider_store.update(provider.clone()) {
                            warn!(
                                provider_id = %provider.id,
                                error = %e,
                                "failed to persist auto-detected owner_open_id"
                            );
                        }
                    }
                    Err(e) => {
                        warn!(
                            provider_id = %provider.id,
                            error = %e,
                            "failed to auto-detect owner on startup, skipping auto-connect"
                        );
                        continue;
                    }
                }
            }

            let app_secret = provider.secret_ref.clone().unwrap_or_default();

            // Create event channel
            let (tx, rx) = mpsc::unbounded_channel::<ImEvent>();

            // Spawn the event processing loop
            let client = self.provider_client(&provider);
            let provider_for_loop = provider.clone();
            let event_store = self.event_store.clone();
            let message_log_store = self.message_log_store.clone();
            let route_store = self.route_store.clone();
            let provider_store = self.provider_store.clone();
            let agent_config_store = self.agent_config_store.clone();
            let agent_client = self.agent_client.clone();
            let agent_tools = self.agent_tools.clone();
            let schedule_store = self.schedule_store.clone();
            let scheduler = self.scheduler.clone();
            let target_store = self.target_store.clone();
            let connection_manager = self.connection_manager.clone();
            let agent_session_manager = self.agent_session_manager.clone();
            let queue_manager = self.queue_manager.clone();
            let progress_registry = self.progress_registry.clone();
            tokio::spawn(async move {
                run_event_loop(
                    rx,
                    client,
                    provider_for_loop,
                    event_store,
                    message_log_store,
                    route_store,
                    provider_store,
                    agent_config_store,
                    agent_client,
                    agent_tools,
                    schedule_store,
                    scheduler,
                    target_store,
                    connection_manager,
                    agent_session_manager,
                    queue_manager,
                    progress_registry,
                )
                .await;
            });

            // Start the long connection
            match self
                .connection_manager
                .start_connection(&provider, &app_secret, tx)
                .await
            {
                Ok(()) => {
                    info!(provider_id = %provider.id, "auto-connected provider on startup");
                }
                Err(e) => {
                    error!(
                        provider_id = %provider.id,
                        error = %e,
                        "failed to auto-connect provider on startup"
                    );
                }
            }
        }

        // Kick off the background supervisor that periodically retries
        // providers whose long connection has fallen into a Disconnected /
        // Failed state. Fire-and-forget; the task stays alive for the
        // lifetime of the service.
        self.clone().spawn_reconnect_supervisor();
    }

    /// Start the scheduled-task loop. The loop checks enabled cron/interval
    /// schedules, executes due tasks, persists run history, and recomputes the
    /// next run timestamp after each fire.
    pub fn start_scheduler(self: &Arc<Self>) {
        let service = self.clone();
        tokio::spawn(async move {
            loop {
                let now = now_ms();
                let schedules = service.schedule_store.list();
                for schedule in service.scheduler.get_due_schedules(&schedules, now) {
                    service.scheduler.remove_completed(&schedule.id);
                    if service.scheduler.is_running(&schedule.id) {
                        debug!(
                            schedule_id = %schedule.id,
                            "schedule already running, skipping due tick"
                        );
                        continue;
                    }

                    let mut scheduled = schedule.clone();
                    scheduled.last_run_at = Some(now);
                    scheduled.next_run_at =
                        ImScheduler::compute_next_run_for_schedule(&scheduled, now);
                    scheduled.updated_at = now;
                    if let Err(error) = service.schedule_store.update(scheduled.clone()) {
                        warn!(
                            schedule_id = %scheduled.id,
                            error = %error,
                            "failed to update schedule next_run_at before execution"
                        );
                    }

                    let task_service = service.clone();
                    let schedule_id = scheduled.id.clone();
                    let handle = tokio::spawn(async move {
                        let run_id = uuid_short();
                        let task_run = execute_schedule_once(
                            &task_service,
                            &scheduled,
                            run_id,
                            crate::im_gateway::types::TriggerSource::Schedule,
                        )
                        .await;
                        if let Err(error) = task_service.run_store.add(task_run.clone()) {
                            warn!(
                                schedule_id = %scheduled.id,
                                error = %error,
                                "failed to persist scheduled task run"
                            );
                        }
                        send_schedule_run_notification(&task_service, &scheduled, &task_run).await;
                    });
                    service.scheduler.register_running(&schedule_id, handle);
                }

                let next_run_at = service
                    .schedule_store
                    .list()
                    .into_iter()
                    .filter(|schedule| schedule.enabled)
                    .filter_map(|schedule| schedule.next_run_at)
                    .min();
                let sleep_ms = next_run_at
                    .map(|next| next.saturating_sub(now).clamp(1_000, 60_000))
                    .unwrap_or(60_000);
                let notify = service.scheduler.notify_handle();
                tokio::select! {
                    _ = tokio::time::sleep(std::time::Duration::from_millis(sleep_ms)) => {}
                    _ = notify.notified() => {}
                }
            }
        });
    }

    /// Spawn a background task that periodically re-scans all providers and
    /// attempts to reconnect any whose long connection is currently
    /// Disconnected or Failed.
    ///
    /// This acts as a last-resort safety net: the long-connection task
    /// itself retries internally with exponential backoff, but if its task
    /// ever exits (e.g. due to an unexpected shutdown signal or a panic
    /// caught elsewhere) the ConnectionManager would otherwise be stuck.
    pub(super) fn spawn_reconnect_supervisor(self: Arc<Self>) {
        use crate::im_gateway::types::ConnectionState;
        const SUPERVISOR_INTERVAL_SECS: u64 = 60;
        tokio::spawn(async move {
            let mut ticker =
                tokio::time::interval(std::time::Duration::from_secs(SUPERVISOR_INTERVAL_SECS));
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            // Skip the immediate tick — auto_connect_providers already ran.
            ticker.tick().await;
            loop {
                ticker.tick().await;
                let statuses = self.connection_manager.list_statuses();
                for (pid, st) in statuses {
                    match st.state {
                        ConnectionState::Disconnected | ConnectionState::Failed => {
                            let Some(provider) = self.provider_store.get(&pid) else {
                                debug!(provider_id = %pid, "supervisor: provider no longer configured, skipping");
                                continue;
                            };
                            let Some(app_secret) = provider.secret_ref.clone() else {
                                continue;
                            };
                            if app_secret.is_empty() {
                                continue;
                            }
                            info!(
                                provider_id = %pid,
                                prev_state = ?st.state,
                                "supervisor: attempting reconnect"
                            );
                            let (tx, rx) = mpsc::unbounded_channel::<ImEvent>();
                            let client = self.provider_client(&provider);
                            let provider_for_loop = provider.clone();
                            let event_store = self.event_store.clone();
                            let message_log_store = self.message_log_store.clone();
                            let route_store = self.route_store.clone();
                            let provider_store = self.provider_store.clone();
                            let agent_config_store = self.agent_config_store.clone();
                            let agent_client = self.agent_client.clone();
                            let agent_tools = self.agent_tools.clone();
                            let schedule_store = self.schedule_store.clone();
                            let scheduler = self.scheduler.clone();
                            let target_store = self.target_store.clone();
                            let connection_manager = self.connection_manager.clone();
                            let agent_session_manager = self.agent_session_manager.clone();
                            let queue_manager = self.queue_manager.clone();
                            let progress_registry = self.progress_registry.clone();
                            tokio::spawn(async move {
                                run_event_loop(
                                    rx,
                                    client,
                                    provider_for_loop,
                                    event_store,
                                    message_log_store,
                                    route_store,
                                    provider_store,
                                    agent_config_store,
                                    agent_client,
                                    agent_tools,
                                    schedule_store,
                                    scheduler,
                                    target_store,
                                    connection_manager,
                                    agent_session_manager,
                                    queue_manager,
                                    progress_registry,
                                )
                                .await;
                            });
                            if let Err(e) = self
                                .connection_manager
                                .start_connection(&provider, &app_secret, tx)
                                .await
                            {
                                warn!(
                                    provider_id = %pid,
                                    error = %e,
                                    "supervisor: reconnect attempt failed, will retry next tick"
                                );
                            }
                        }
                        _ => {}
                    }
                }
            }
        });
    }
}

pub type SharedImGatewayService = Arc<ImGatewayService>;
