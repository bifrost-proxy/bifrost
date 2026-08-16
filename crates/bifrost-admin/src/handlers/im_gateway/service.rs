use super::*;

pub(super) const IMAGE_ONLY_AGENT_PROMPT: &str = "请理解这张图片，并根据图片内容回答。";
pub(super) const MAX_AGENT_ATTACHMENTS_PER_MESSAGE: usize = 6;
pub(super) const MAX_AGENT_REPLY_IMAGE_BYTES: u64 = 10 * 1024 * 1024;
/// Feishu and the verified Weixin iLink attachment path both use a 30 MiB
/// preflight limit so a generated attachment cannot turn an otherwise
/// successful terminal reply into a doomed upload.
pub(super) const MAX_AGENT_REPLY_ATTACHMENT_BYTES: u64 = 30 * 1024 * 1024;
pub(super) const MAX_FEISHU_REFERENCED_FILE_BYTES: u64 = 100 * 1024 * 1024;
pub(super) const MAX_FEISHU_REFERENCED_TOTAL_FILE_BYTES: u64 = 250 * 1024 * 1024;
const FEISHU_DRY_RUN_FILE_ENV: &str = "BIFROST_FEISHU_DRY_RUN_FILE";

pub(super) static AGENT_REPLY_IMAGE_UPLOAD_CACHE: OnceLock<
    Mutex<HashMap<AgentReplyImageCacheKey, String>>,
> = OnceLock::new();

#[derive(Clone)]
pub(super) struct PendingWeixinLogin {
    pub(super) login: crate::im_gateway::weixin::WeixinLoginStart,
    pub(super) created_at_ms: u64,
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub(super) struct PendingFeishuSetup {
    pub(super) device_code: String,
    pub(super) interval_seconds: u64,
    pub(super) expires_at_ms: u64,
    pub(super) app_id: Option<String>,
    pub(super) app_secret: Option<String>,
    pub(super) owner_open_id: Option<String>,
    pub(super) brand: FeishuSetupBrand,
    #[serde(default)]
    pub(super) provider_payload: Option<serde_json::Value>,
    #[serde(default)]
    pub(super) created_provider_id: Option<String>,
    #[serde(default)]
    pub(super) auto_connect: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(super) enum FeishuSetupBrand {
    Feishu,
    Lark,
}

impl FeishuSetupBrand {
    pub(super) fn accounts_base(self) -> &'static str {
        match self {
            Self::Feishu => "https://accounts.feishu.cn",
            Self::Lark => "https://accounts.larksuite.com",
        }
    }

    pub(super) fn open_base(self) -> &'static str {
        match self {
            Self::Feishu => "https://open.feishu.cn",
            Self::Lark => "https://open.larksuite.com",
        }
    }

    pub(super) fn provider_base_url(self) -> &'static str {
        match self {
            Self::Feishu => "https://open.feishu.cn/open-apis",
            Self::Lark => "https://open.larksuite.com/open-apis",
        }
    }
}

#[derive(Clone)]
pub(super) enum ImProviderClient {
    Feishu(Arc<crate::im_gateway::feishu::FeishuProvider>),
    Weixin(Arc<WeixinProvider>),
    Unsupported(crate::im_gateway::types::ImProviderType),
}

impl ImProviderClient {
    fn unsupported<T>(
        provider_type: crate::im_gateway::types::ImProviderType,
    ) -> bifrost_core::Result<T> {
        Err(bifrost_core::BifrostError::Config(format!(
            "provider type '{}' is not implemented",
            serde_json::to_value(provider_type)
                .ok()
                .and_then(|value| value.as_str().map(str::to_string))
                .unwrap_or_else(|| "unknown".to_string())
        )))
    }
    #[rustfmt::skip]    fn unsupported_capabilities(config: &ImProviderConfig, provider_type: crate::im_gateway::types::ImProviderType) -> crate::im_gateway::types::ImSendCapabilities { crate::im_gateway::types::ImSendCapabilities { provider_id: config.id.clone(), provider_type, destinations: Vec::new(), receive_id_types: Vec::new(), parts: std::collections::BTreeMap::new(), requires_context: false } }
    #[rustfmt::skip]    pub(super) fn send_capabilities(&self, config: &ImProviderConfig) -> crate::im_gateway::types::ImSendCapabilities {
        match self {
            Self::Feishu(provider) => provider.send_capabilities(config),
            Self::Weixin(provider) => provider.send_capabilities(config),
            Self::Unsupported(provider_type) => Self::unsupported_capabilities(config, *provider_type),
        }
    }
    pub(super) fn channel_capabilities(
        &self,
        config: &ImProviderConfig,
    ) -> crate::im_gateway::types::ImChannelCapabilities {
        match self {
            Self::Feishu(provider) => provider.channel_capabilities(config),
            Self::Weixin(provider) => provider.channel_capabilities(config),
            Self::Unsupported(provider_type) => crate::im_gateway::types::ImChannelCapabilities {
                send: Self::unsupported_capabilities(config, *provider_type),
                interaction: Default::default(),
                conversation: Default::default(),
            },
        }
    }
    pub(super) fn proactive_send_ready(
        &self,
        config: &ImProviderConfig,
        target: &crate::im_gateway::types::ImTarget,
    ) -> bool {
        match self {
            Self::Feishu(_) => true,
            Self::Weixin(provider) => provider.send_ready(config, target),
            Self::Unsupported(_) => false,
        }
    }
    pub(super) async fn create_feishu_group_chat(
        &self,
        config: &ImProviderConfig,
        name: &str,
        owner_open_id: &str,
        uuid: &str,
    ) -> bifrost_core::Result<crate::im_gateway::feishu::FeishuCreatedChat> {
        match self {
            Self::Feishu(provider) => {
                provider
                    .create_group_chat(config, name, owner_open_id, uuid)
                    .await
            }
            Self::Weixin(_) => Err(bifrost_core::BifrostError::Config(
                "/new is only supported by Feishu providers".to_string(),
            )),
            Self::Unsupported(provider_type) => Self::unsupported(*provider_type),
        }
    }

    pub(super) fn feishu(&self) -> Option<Arc<crate::im_gateway::feishu::FeishuProvider>> {
        match self {
            Self::Feishu(provider) => Some(provider.clone()),
            Self::Weixin(_) => None,
            Self::Unsupported(_) => None,
        }
    }

    pub(super) fn weixin(&self) -> Option<Arc<WeixinProvider>> {
        match self {
            Self::Weixin(provider) => Some(provider.clone()),
            Self::Feishu(_) | Self::Unsupported(_) => None,
        }
    }

    pub(super) async fn send_card(
        &self,
        config: &ImProviderConfig,
        target: &ImTarget,
        card: serde_json::Value,
        opts: crate::im_gateway::types::SendOptions,
    ) -> bifrost_core::Result<crate::im_gateway::types::SendResult> {
        if matches!(self, Self::Feishu(_)) {
            if let Some(result) =
                capture_feishu_card_dry_run(config, target, None, &card, opts.uuid.as_deref())
            {
                return result;
            }
        }
        match self {
            Self::Feishu(provider) => provider.send_card(config, target, card, opts).await,
            Self::Weixin(provider) => provider.send_card(config, target, card, opts).await,
            Self::Unsupported(provider_type) => Self::unsupported(*provider_type),
        }
    }
    #[rustfmt::skip]    pub(super) async fn send_native_card(&self, config: &ImProviderConfig, target: &ImTarget, card: serde_json::Value, opts: crate::im_gateway::types::SendOptions) -> bifrost_core::Result<crate::im_gateway::types::SendResult> {
        match self {
            Self::Feishu(provider) => provider.send_native_card(config, target, card, opts).await,
            Self::Weixin(provider) => provider.send_native_card(config, target, card, opts).await,
            Self::Unsupported(provider_type) => Self::unsupported(*provider_type),
        }
    }

    /// Reply to the triggering message when Feishu provides a source message
    /// id. Other providers keep their existing send-to-target behavior.
    pub(super) async fn send_reply_card(
        &self,
        config: &ImProviderConfig,
        target: &ImTarget,
        source_message_id: Option<&str>,
        card: serde_json::Value,
        opts: crate::im_gateway::types::SendOptions,
    ) -> bifrost_core::Result<crate::im_gateway::types::SendResult> {
        if matches!(self, Self::Feishu(_)) {
            if let Some(result) = capture_feishu_card_dry_run(
                config,
                target,
                source_message_id,
                &card,
                opts.uuid.as_deref(),
            ) {
                return result;
            }
        }
        match (
            self,
            source_message_id.map(str::trim).filter(|id| !id.is_empty()),
        ) {
            (Self::Feishu(provider), Some(message_id)) => {
                match provider
                    .reply_card(config, message_id, card.clone(), opts.uuid.as_deref())
                    .await
                {
                    Ok(result) => Ok(result),
                    Err(reply_error) => {
                        warn!(
                            message_id,
                            error = %reply_error,
                            "Feishu native card reply failed; falling back to direct send"
                        );
                        provider.send_card(config, target, card, opts).await
                    }
                }
            }
            _ => self.send_card(config, target, card, opts).await,
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
            Self::Unsupported(provider_type) => Self::unsupported(*provider_type),
        }
    }
    #[rustfmt::skip]    pub(super) async fn send_text_with_uuid(&self, config: &ImProviderConfig, target: &ImTarget, text: &str, uuid: Option<&str>) -> bifrost_core::Result<crate::im_gateway::types::SendResult> {
        match self {
            Self::Feishu(provider) => {
                provider
                    .send_text_with_uuid(config, target, text, uuid)
                    .await
            }
            Self::Weixin(provider) => {
                provider
                    .send_text_with_uuid(config, target, text, uuid)
                    .await
            }
            Self::Unsupported(provider_type) => Self::unsupported(*provider_type),
        }
    }

    pub(super) async fn upload_image(
        &self,
        config: &ImProviderConfig,
        image_type: &str,
        file_name: &str,
        bytes: Vec<u8>,
        mime_type: Option<&str>,
    ) -> bifrost_core::Result<crate::im_gateway::types::UploadedImage> {
        match self {
            Self::Feishu(provider) => {
                provider
                    .upload_image(config, image_type, file_name, bytes, mime_type)
                    .await
            }
            Self::Weixin(provider) => {
                provider
                    .upload_image(config, image_type, file_name, bytes, mime_type)
                    .await
            }
            Self::Unsupported(provider_type) => Self::unsupported(*provider_type),
        }
    }

    pub(super) async fn send_image(
        &self,
        config: &ImProviderConfig,
        target: &ImTarget,
        image_key: &str,
        uuid: Option<&str>,
    ) -> bifrost_core::Result<crate::im_gateway::types::SendResult> {
        match self {
            Self::Feishu(provider) => provider.send_image(config, target, image_key, uuid).await,
            Self::Weixin(provider) => provider.send_image(config, target, image_key, uuid).await,
            Self::Unsupported(provider_type) => Self::unsupported(*provider_type),
        }
    }

    pub(super) async fn upload_file(
        &self,
        config: &ImProviderConfig,
        file_name: &str,
        bytes: Vec<u8>,
        mime_type: Option<&str>,
    ) -> bifrost_core::Result<String> {
        match self {
            Self::Feishu(provider) => {
                provider
                    .upload_file(config, file_name, bytes, mime_type)
                    .await
            }
            Self::Weixin(provider) => {
                provider
                    .upload_file(config, file_name, bytes, mime_type)
                    .await
            }
            Self::Unsupported(provider_type) => Self::unsupported(*provider_type),
        }
    }

    pub(super) async fn send_file(
        &self,
        config: &ImProviderConfig,
        target: &ImTarget,
        file_key: &str,
        uuid: Option<&str>,
    ) -> bifrost_core::Result<crate::im_gateway::types::SendResult> {
        match self {
            Self::Feishu(provider) => provider.send_file(config, target, file_key, uuid).await,
            Self::Weixin(provider) => provider.send_file(config, target, file_key, uuid).await,
            Self::Unsupported(provider_type) => Self::unsupported(*provider_type),
        }
    }

    pub(super) async fn add_reaction(
        &self,
        config: &ImProviderConfig,
        message_id: &str,
        reaction: &str,
    ) -> bifrost_core::Result<bool> {
        if matches!(self, Self::Feishu(_)) {
            if let Some(result) = capture_feishu_reaction_dry_run(config, message_id, reaction) {
                return result
                    .map(|_| true)
                    .map_err(bifrost_core::BifrostError::Config);
            }
        }
        match self {
            Self::Feishu(provider) => {
                provider.add_reaction(config, message_id, reaction).await?;
                Ok(true)
            }
            Self::Weixin(_) => Ok(false),
            Self::Unsupported(provider_type) => Self::unsupported(*provider_type),
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
            Self::Unsupported(provider_type) => Self::unsupported(*provider_type),
        }
    }

    pub(super) async fn download_message_file_resource(
        &self,
        config: &ImProviderConfig,
        message_id: &str,
        file: &crate::im_gateway::types::ImFileAttachment,
    ) -> bifrost_core::Result<(String, Vec<u8>)> {
        match self {
            Self::Feishu(provider) => {
                provider
                    .download_message_file_resource(config, message_id, &file.file_key)
                    .await
            }
            Self::Weixin(provider) => provider.download_message_file_resource(config, file).await,
            Self::Unsupported(provider_type) => Self::unsupported(*provider_type),
        }
    }
}

fn capture_feishu_card_dry_run(
    config: &ImProviderConfig,
    target: &ImTarget,
    source_message_id: Option<&str>,
    card: &serde_json::Value,
    uuid: Option<&str>,
) -> Option<bifrost_core::Result<crate::im_gateway::types::SendResult>> {
    let path = std::env::var_os(FEISHU_DRY_RUN_FILE_ENV)?;
    let message_id = format!("dry-run-{}", uuid_short());
    Some(
        append_feishu_dry_run(
            Path::new(&path),
            &serde_json::json!({
                "kind": "card",
                "timestamp": now_ms(),
                "providerId": config.id,
                "receiveIdType": target.receive_id_type,
                "receiveId": target.receive_id,
                "sourceMessageId": source_message_id,
                "uuid": uuid,
                "messageId": message_id,
                "card": card
            }),
        )
        .map(|_| crate::im_gateway::types::SendResult {
            message_id: Some(message_id),
            request_id: None,
        })
        .map_err(bifrost_core::BifrostError::Config),
    )
}

fn capture_feishu_reaction_dry_run(
    config: &ImProviderConfig,
    message_id: &str,
    reaction: &str,
) -> Option<Result<(), String>> {
    let path = std::env::var_os(FEISHU_DRY_RUN_FILE_ENV)?;
    Some(append_feishu_dry_run(
        Path::new(&path),
        &serde_json::json!({
            "kind": "reaction",
            "timestamp": now_ms(),
            "providerId": config.id,
            "messageId": message_id,
            "reaction": reaction
        }),
    ))
}

fn append_feishu_dry_run(path: &Path, row: &serde_json::Value) -> Result<(), String> {
    use std::io::Write;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("create Feishu dry-run directory: {error}"))?;
    }
    let mut bytes = serde_json::to_vec(row)
        .map_err(|error| format!("serialize Feishu dry-run row: {error}"))?;
    bytes.push(b'\n');
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|error| format!("open Feishu dry-run file: {error}"))?;
    file.write_all(&bytes)
        .map_err(|error| format!("write Feishu dry-run row: {error}"))?;
    file.flush()
        .map_err(|error| format!("flush Feishu dry-run row: {error}"))
}

// ---------------------------------------------------------------------------
// ImGatewayService
// ---------------------------------------------------------------------------

struct ProviderEventPipeline {
    sink: crate::im_gateway::provider::EventSink,
    task: tokio::task::JoinHandle<()>,
}

pub struct ImGatewayService {
    pub(super) data_dir: PathBuf,
    pub provider_store: Arc<ImProviderStore>,
    pub target_store: Arc<ImTargetStore>,
    pub route_store: Arc<ImRouteStore>,
    pub schedule_store: Arc<ImScheduleStore>,
    pub scheduler: Arc<ImScheduler>,
    pub event_store: Arc<ImEventStore>,
    pub run_store: Arc<ImRunStore>,
    pub message_log_store: Arc<ImMessageLogStore>,
    pub outbox_store: Arc<crate::im_gateway::ImOutboxStore>,
    pub group_context_store: Arc<ImGroupContextStore>,
    pub connection_manager: Arc<ImConnectionManager>,
    pub agent_config_store: Arc<ImAgentConfigStore>,
    pub agent_session_manager: Arc<ImAgentSessionManager>,
    pub external_cli_config_store: Arc<crate::im_gateway::external_cli::ExternalCliConfigStore>,
    pub queue_manager: Arc<SessionQueueManager>,
    pub progress_registry: Arc<ImAgentProgressRegistry>,
    event_sinks: Arc<RwLock<HashMap<String, ProviderEventPipeline>>>,
    pub(super) provider_connection_lifecycle: Arc<AsyncMutex<()>>,
    pub(super) mock_event_sinks: Arc<RwLock<HashMap<String, mpsc::UnboundedSender<ImEvent>>>>,
    pub(super) weixin_login_pending: Arc<RwLock<HashMap<String, PendingWeixinLogin>>>,
    pub(super) feishu_setup_pending: Arc<RwLock<HashMap<String, PendingFeishuSetup>>>,
}

#[derive(Clone)]
pub struct ChatGptWebStartupAuthRunner {
    pub runner_id: String,
    pub settings: crate::im_gateway::external_cli::ExternalCliAgentSettings,
}

impl ImGatewayService {
    pub fn new(data_dir: &std::path::Path) -> Self {
        Self::new_with_agent_proxy_port(data_dir, None)
    }

    pub fn new_with_agent_proxy_port(data_dir: &std::path::Path, _proxy_port: Option<u16>) -> Self {
        // Store agent config under data_dir/agent/ for unified directory structure
        let agent_data_dir = data_dir.join("agent");
        let _ = std::fs::create_dir_all(&agent_data_dir);
        let cleanup = bifrost_agent::persistence::clean_noncanonical_conversations(&agent_data_dir);
        if cleanup.files_removed > 0 {
            info!(
                files_removed = cleanup.files_removed,
                "discarded noncanonical agent session histories"
            );
        }
        for error in cleanup.failures {
            warn!(error = %error, "failed to discard noncanonical agent session history");
        }
        let agent_config_store = Arc::new(ImAgentConfigStore::new(&agent_data_dir));
        let agent_config = agent_config_store.load();
        let schedule_store = Arc::new(ImScheduleStore::new(data_dir));
        let scheduler = Arc::new(ImScheduler::new());
        let target_store = Arc::new(ImTargetStore::new(data_dir));
        Self {
            data_dir: data_dir.to_path_buf(),
            provider_store: Arc::new(ImProviderStore::new(data_dir)),
            target_store,
            route_store: Arc::new(ImRouteStore::new(data_dir)),
            schedule_store,
            scheduler,
            event_store: Arc::new(ImEventStore::new(data_dir)),
            run_store: Arc::new(ImRunStore::new(data_dir)),
            message_log_store: Arc::new(ImMessageLogStore::new(data_dir)),
            outbox_store: Arc::new(crate::im_gateway::ImOutboxStore::new(data_dir)),
            group_context_store: Arc::new(ImGroupContextStore::new(data_dir)),
            connection_manager: Arc::new(ImConnectionManager::new_with_data_dir(data_dir)),
            agent_config_store,
            agent_session_manager: Arc::new(ImAgentSessionManager::new(
                agent_config.get_session_ttl_secs(),
            )),
            external_cli_config_store: Arc::new(
                crate::im_gateway::external_cli::ExternalCliConfigStore::new(data_dir),
            ),
            queue_manager: Arc::new(SessionQueueManager::new()),
            progress_registry: Arc::new(ImAgentProgressRegistry::new()),
            event_sinks: Arc::new(RwLock::new(HashMap::new())),
            provider_connection_lifecycle: Arc::new(AsyncMutex::new(())),
            mock_event_sinks: Arc::new(RwLock::new(HashMap::new())),
            weixin_login_pending: Arc::new(RwLock::new(HashMap::new())),
            feishu_setup_pending: Arc::new(RwLock::new(load_pending_feishu_setups(data_dir))),
        }
    }

    pub(super) fn provider_client(&self, provider: &ImProviderConfig) -> ImProviderClient {
        match provider.provider_type {
            crate::im_gateway::types::ImProviderType::Weixin
            | crate::im_gateway::types::ImProviderType::WeChat => {
                ImProviderClient::Weixin(self.connection_manager.weixin_provider().clone())
            }
            crate::im_gateway::types::ImProviderType::Feishu => {
                ImProviderClient::Feishu(self.connection_manager.feishu_provider().clone())
            }
            crate::im_gateway::types::ImProviderType::Webhook => {
                ImProviderClient::Unsupported(provider.provider_type)
            }
        }
    }

    /// Return the single long-lived event pipeline for this provider. Transport
    /// reconnects reuse its sink so pending events are not replayed into a
    /// second loop while the first loop still owns active Agent turns.
    pub(super) fn event_sink_for_provider(
        &self,
        provider: &ImProviderConfig,
    ) -> crate::im_gateway::provider::EventSink {
        let mut sinks = self.event_sinks.write();
        if let Some(pipeline) = sinks
            .get(&provider.id)
            .filter(|pipeline| !pipeline.sink.is_closed() && !pipeline.task.is_finished())
        {
            return pipeline.sink.clone();
        }
        if let Some(stale) = sinks.remove(&provider.id) {
            stale.task.abort();
        }

        let (tx, rx) = mpsc::unbounded_channel::<ImEvent>();
        let sink = crate::im_gateway::provider::EventSink::with_durable_store(
            tx,
            Arc::clone(&self.event_store),
            &provider.id,
        );
        let client = self.provider_client(provider);
        let provider_id = provider.id.clone();
        let provider = provider.clone();
        let event_store = Arc::clone(&self.event_store);
        let message_log_store = Arc::clone(&self.message_log_store);
        let group_context_store = Arc::clone(&self.group_context_store);
        let route_store = Arc::clone(&self.route_store);
        let provider_store = Arc::clone(&self.provider_store);
        let agent_config_store = Arc::clone(&self.agent_config_store);
        let schedule_store = Arc::clone(&self.schedule_store);
        let scheduler = Arc::clone(&self.scheduler);
        let target_store = Arc::clone(&self.target_store);
        let connection_manager = Arc::clone(&self.connection_manager);
        let agent_session_manager = Arc::clone(&self.agent_session_manager);
        let external_cli_config_store = Arc::clone(&self.external_cli_config_store);
        let queue_manager = Arc::clone(&self.queue_manager);
        let progress_registry = Arc::clone(&self.progress_registry);
        let task = tokio::spawn(async move {
            run_event_loop(
                rx,
                client,
                provider,
                event_store,
                message_log_store,
                group_context_store,
                route_store,
                provider_store,
                agent_config_store,
                schedule_store,
                scheduler,
                target_store,
                connection_manager,
                agent_session_manager,
                external_cli_config_store,
                queue_manager,
                progress_registry,
            )
            .await;
        });
        sinks.insert(
            provider_id,
            ProviderEventPipeline {
                sink: sink.clone(),
                task,
            },
        );
        sink
    }

    /// Cancel a provider's event loop, including any active Agent turn owned
    /// by that loop, and wait until the task has released its account-bound
    /// client state. Callers must stop and await the transport first.
    pub(super) async fn stop_event_pipeline(&self, provider_id: &str) {
        let pipeline = self.event_sinks.write().remove(provider_id);
        let Some(pipeline) = pipeline else {
            return;
        };
        let ProviderEventPipeline { sink, task } = pipeline;
        drop(sink);
        task.abort();
        let _ = task.await;
    }

    pub fn chatgpt_web_startup_auth_runners(&self) -> Vec<ChatGptWebStartupAuthRunner> {
        self.external_cli_config_store
            .load()
            .runners
            .into_iter()
            .filter(|(_, settings)| settings.adapter == crate::im_gateway::chatgpt_web::ADAPTER_ID)
            .map(|(runner_id, settings)| ChatGptWebStartupAuthRunner {
                runner_id,
                settings,
            })
            .collect()
    }

    pub fn spawn_chatgpt_web_startup_auth_check(self: &Arc<Self>) {
        let service = self.clone();
        tokio::spawn(async move {
            service.ensure_chatgpt_web_startup_auth_ready().await;
        });
    }

    pub async fn ensure_chatgpt_web_startup_auth_ready(self: &Arc<Self>) {
        let runners = self.chatgpt_web_startup_auth_runners();
        if runners.is_empty() {
            debug!("chatgpt_web startup auth: no chatgpt_web runners configured");
            return;
        }
        for runner in runners {
            match crate::im_gateway::chatgpt_web::ensure_startup_auth_ready(
                &runner.runner_id,
                &runner.settings,
            )
            .await
            {
                Ok(status) if status.logged_in => {
                    info!(
                        runner_id = %status.runner_id,
                        opened_login = status.opened_login,
                        dry_run = status.dry_run,
                        "chatgpt_web startup auth: login ready"
                    );
                }
                Ok(status) => {
                    warn!(
                        runner_id = %status.runner_id,
                        state = %status.state,
                        dry_run = status.dry_run,
                        message = ?status.message,
                        "chatgpt_web startup auth: login still required"
                    );
                }
                Err(error) => {
                    warn!(
                        runner_id = %runner.runner_id,
                        error = %error,
                        "chatgpt_web startup auth: check failed"
                    );
                }
            }
        }
    }

    pub fn spawn_feishu_setup_supervisor(self: &Arc<Self>) {
        let service = self.clone();
        tokio::spawn(async move {
            const SUPERVISOR_INTERVAL_SECS: u64 = 5;
            let mut ticker =
                tokio::time::interval(std::time::Duration::from_secs(SUPERVISOR_INTERVAL_SECS));
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                ticker.tick().await;
                let session_ids: Vec<String> = service
                    .feishu_setup_pending
                    .read()
                    .iter()
                    .filter(|(_, pending)| {
                        pending.provider_payload.is_some() && pending.created_provider_id.is_none()
                    })
                    .map(|(session_id, _)| session_id.clone())
                    .collect();
                for session_id in session_ids {
                    match poll_and_complete_feishu_setup_session(&service, &session_id).await {
                        Ok(()) => {}
                        Err(error) => {
                            debug!(
                                session_id = %session_id,
                                error = %error,
                                "Feishu setup supervisor poll did not complete"
                            );
                        }
                    }
                }
            }
        });
    }

    /// Auto-connect all configured providers that have a secret.
    /// If owner_open_id is not set, auto-detect it from the Feishu Application API.
    /// Called on Bifrost startup to restore active connections and send online notifications.
    pub async fn auto_connect_providers(self: &Arc<Self>) {
        let providers = self.provider_store.list();
        for listed_provider in providers {
            let _lifecycle = self.provider_connection_lifecycle.lock().await;
            let Some(mut provider) = self.provider_store.get(&listed_provider.id) else {
                continue;
            };
            if !should_run_provider_event_connection(&provider) {
                info!(
                    provider_id = %provider.id,
                    enabled = provider.enabled,
                    event_connection_enabled = provider.event_connection_enabled,
                    has_secret = provider.secret_ref.as_deref().is_some_and(|s| !s.is_empty()),
                    "skipping auto-connect: provider event connection inactive"
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

            let sink = self.event_sink_for_provider(&provider);

            // Start the long connection
            match self
                .connection_manager
                .start_connection(&provider, &app_secret, sink)
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
                            let _lifecycle = self.provider_connection_lifecycle.lock().await;
                            let Some(provider) = self.provider_store.get(&pid) else {
                                debug!(provider_id = %pid, "supervisor: provider no longer configured, skipping");
                                continue;
                            };
                            if !should_run_provider_event_connection(&provider) {
                                debug!(
                                    provider_id = %pid,
                                    enabled = provider.enabled,
                                    event_connection_enabled = provider.event_connection_enabled,
                                    has_secret = provider.secret_ref.as_deref().is_some_and(|s| !s.is_empty()),
                                    "supervisor: provider event connection inactive, skipping"
                                );
                                continue;
                            }
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
                            let sink = self.event_sink_for_provider(&provider);
                            if let Err(e) = self
                                .connection_manager
                                .start_connection(&provider, &app_secret, sink)
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

const FEISHU_SETUP_STORE_FILENAME: &str = "im_gateway_feishu_setup_sessions.json";

#[derive(serde::Serialize, serde::Deserialize)]
struct PendingFeishuSetupStore {
    version: u32,
    sessions: HashMap<String, PendingFeishuSetup>,
}

fn pending_feishu_setup_path(data_dir: &Path) -> PathBuf {
    data_dir.join("admin").join(FEISHU_SETUP_STORE_FILENAME)
}

fn load_pending_feishu_setups(data_dir: &Path) -> HashMap<String, PendingFeishuSetup> {
    let path = pending_feishu_setup_path(data_dir);
    if !path.exists() {
        return HashMap::new();
    }
    const MAX_STORE_FILE_BYTES: u64 = 16 * 1024 * 1024;
    if std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0) > MAX_STORE_FILE_BYTES {
        let _ = std::fs::remove_file(&path);
        return HashMap::new();
    }
    let Ok(content) = std::fs::read_to_string(&path) else {
        return HashMap::new();
    };
    match serde_json::from_str::<PendingFeishuSetupStore>(&content) {
        Ok(store) if store.version == 1 => store.sessions,
        _ => {
            let _ = std::fs::remove_file(&path);
            HashMap::new()
        }
    }
}

pub(super) fn save_pending_feishu_setups(service: &ImGatewayService) {
    let path = pending_feishu_setup_path(&service.data_dir);
    let store = PendingFeishuSetupStore {
        version: 1,
        sessions: service.feishu_setup_pending.read().clone(),
    };
    if let Err(error) = write_pending_feishu_setup_store(&path, &store) {
        warn!(
            path = %path.display(),
            error = %error,
            "failed to persist Feishu setup sessions"
        );
    }
}

fn write_pending_feishu_setup_store(
    path: &Path,
    store: &PendingFeishuSetupStore,
) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
    }
    let content =
        serde_json::to_string_pretty(store).map_err(|e| format!("serialize store: {e}"))?;
    std::fs::write(path, content).map_err(|e| format!("write {}: {e}", path.display()))
}

pub(super) fn should_run_provider_event_connection(provider: &ImProviderConfig) -> bool {
    provider.enabled
        && provider.event_connection_enabled
        && provider
            .secret_ref
            .as_deref()
            .is_some_and(|secret| !secret.is_empty())
}

#[cfg(test)]
mod provider_event_connection_tests {
    use super::*;

    struct EnvVarGuard {
        key: &'static str,
        old_value: Option<std::ffi::OsString>,
    }

    impl EnvVarGuard {
        fn set(key: &'static str, value: &std::path::Path) -> Self {
            let old_value = std::env::var_os(key);
            std::env::set_var(key, value);
            Self { key, old_value }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            if let Some(value) = self.old_value.take() {
                std::env::set_var(self.key, value);
            } else {
                std::env::remove_var(self.key);
            }
        }
    }

    fn write_legacy_history(data_dir: &Path, filename: &str) -> PathBuf {
        let path = data_dir.join("agent/sessions/2026/07/21").join(filename);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            r#"{"timestamp":1,"event_type":"user_message","session_key":"legacy","content":{"message":"old"}}
"#,
        )
        .unwrap();
        path
    }

    fn provider_with_secret() -> ImProviderConfig {
        ImProviderConfig {
            id: "provider-main".to_string(),
            provider_type: crate::im_gateway::types::ImProviderType::Weixin,
            display_name: "Provider Main".to_string(),
            enabled: true,
            base_url: None,
            app_id: Some("app".to_string()),
            secret_ref: Some("secret".to_string()),
            owner_open_id: None,
            event_connection_enabled: true,
            event_types: vec!["message.receive".to_string()],
            agent_config: None,
            created_at: 0,
            updated_at: 0,
        }
    }

    #[test]
    fn feishu_dry_run_writer_appends_complete_ndjson_rows() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("nested/feishu.jsonl");
        let row = serde_json::json!({
            "kind": "card",
            "providerId": "feishu-main",
            "card": {"schema": "2.0"}
        });

        append_feishu_dry_run(&path, &row).unwrap();
        append_feishu_dry_run(
            &path,
            &serde_json::json!({"kind": "reaction", "reaction": "OK"}),
        )
        .unwrap();

        let rows = std::fs::read_to_string(path)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0], row);
        assert_eq!(rows[1]["kind"], "reaction");
        assert_eq!(rows[1]["reaction"], "OK");
    }

    #[tokio::test]
    async fn feishu_client_dry_run_captures_direct_card_and_reaction() {
        let _lock = crate::im_gateway::external_cli::local_session_test_env_lock()
            .lock()
            .await;
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("capture/feishu.jsonl");
        let _guard = EnvVarGuard::set(FEISHU_DRY_RUN_FILE_ENV, &path);
        let config = crate::handlers::im_gateway::tests::test_provider();
        let target = ImTarget {
            id: "target".to_string(),
            provider_id: config.id.clone(),
            display_name: "Target".to_string(),
            receive_id_type: "open_id".to_string(),
            receive_id: "ou_target".to_string(),
            default_msg_type: "interactive".to_string(),
            enabled: true,
            created_at: 0,
            updated_at: 0,
        };
        let client =
            ImProviderClient::Feishu(Arc::new(crate::im_gateway::feishu::FeishuProvider::new()));
        let result = client
            .send_card(
                &config,
                &target,
                serde_json::json!({"schema": "2.0"}),
                crate::im_gateway::types::SendOptions {
                    uuid: Some("unit-card".to_string()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert!(result
            .message_id
            .as_deref()
            .is_some_and(|message_id| message_id.starts_with("dry-run-")));
        assert!(client
            .add_reaction(&config, "om_source", "DONE")
            .await
            .unwrap());

        let rows = std::fs::read_to_string(path)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["kind"], "card");
        assert_eq!(rows[0]["receiveId"], "ou_target");
        assert_eq!(rows[0]["sourceMessageId"], serde_json::Value::Null);
        assert_eq!(rows[0]["uuid"], "unit-card");
        assert_eq!(rows[1]["kind"], "reaction");
        assert_eq!(rows[1]["messageId"], "om_source");
        assert_eq!(rows[1]["reaction"], "DONE");
    }

    #[test]
    fn event_connection_requires_enabled_long_connection_and_secret() {
        let mut provider = provider_with_secret();
        assert!(should_run_provider_event_connection(&provider));

        provider.enabled = false;
        assert!(!should_run_provider_event_connection(&provider));

        provider.enabled = true;
        provider.event_connection_enabled = false;
        assert!(!should_run_provider_event_connection(&provider));

        provider.event_connection_enabled = true;
        provider.secret_ref = Some(String::new());
        assert!(!should_run_provider_event_connection(&provider));

        provider.secret_ref = None;
        assert!(!should_run_provider_event_connection(&provider));
    }

    #[tokio::test]
    async fn provider_pipeline_is_cancelled_before_same_id_is_reused() {
        let temp_dir = tempfile::tempdir().unwrap();
        let service = ImGatewayService::new(temp_dir.path());
        let provider = provider_with_secret();

        let first = service.event_sink_for_provider(&provider);
        let second = service.event_sink_for_provider(&provider);
        tokio::task::yield_now().await;
        assert!(!first.is_closed());
        assert!(!second.is_closed());
        assert_eq!(service.event_sinks.read().len(), 1);

        service.stop_event_pipeline(&provider.id).await;
        assert!(first.is_closed());
        assert!(second.is_closed());
        assert!(service.event_sinks.read().is_empty());

        let replacement = service.event_sink_for_provider(&provider);
        assert!(!replacement.is_closed());
        assert_eq!(service.event_sinks.read().len(), 1);
        service.stop_event_pipeline(&provider.id).await;
    }

    #[tokio::test]
    async fn weixin_client_dispatches_inbound_file_resource_downloads() {
        use http_body_util::Full;
        use hyper::server::conn::http1;
        use hyper::service::service_fn;
        use hyper::{Request, Response};
        use hyper_util::rt::TokioIo;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock weixin file server");
        let port = listener.local_addr().expect("mock local addr").port();
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accept file download");
            let service = service_fn(|_request: Request<hyper::body::Incoming>| async move {
                Ok::<_, hyper::Error>(
                    Response::builder()
                        .status(200)
                        .header("content-type", "text/markdown")
                        .body(Full::new(bytes::Bytes::from_static(b"hello weixin")))
                        .unwrap(),
                )
            });
            let _ = http1::Builder::new()
                .serve_connection(TokioIo::new(stream), service)
                .await;
        });

        let client = ImProviderClient::Weixin(Arc::new(WeixinProvider::new()));
        let mut provider = provider_with_secret();
        provider.base_url = Some(format!("http://127.0.0.1:{port}"));
        let file = crate::im_gateway::types::ImFileAttachment {
            file_key: "file-key".to_string(),
            name: Some("report.md".to_string()),
            mime_type: Some("text/markdown".to_string()),
            size_bytes: Some(12),
            data_base64: None,
            download_url: Some(format!("http://127.0.0.1:{port}/file")),
            ..Default::default()
        };

        let (mime_type, bytes) = client
            .download_message_file_resource(&provider, "message-id", &file)
            .await
            .expect("weixin downloads file resource");

        assert_eq!(mime_type, "text/markdown");
        assert_eq!(bytes, b"hello weixin");
    }

    #[tokio::test]
    async fn feishu_client_dispatches_inbound_file_resource_downloads() {
        use http_body_util::Full;
        use hyper::server::conn::http1;
        use hyper::service::service_fn;
        use hyper::{Method, Request, Response, StatusCode};
        use hyper_util::rt::TokioIo;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock feishu file dispatch server");
        let port = listener.local_addr().expect("mock local addr").port();

        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    break;
                };
                let io = TokioIo::new(stream);
                tokio::spawn(async move {
                    let service = service_fn(
                        move |req: Request<hyper::body::Incoming>| async move {
                            let method = req.method().clone();
                            let path = req.uri().path().to_string();
                            if method == Method::POST
                                && path == "/open-apis/auth/v3/tenant_access_token/internal"
                            {
                                return Ok::<_, hyper::Error>(
                                    Response::builder()
                                        .status(StatusCode::OK)
                                        .body(Full::new(bytes::Bytes::from_static(
                                            br#"{"code":0,"tenant_access_token":"tenant-token","expire":7200}"#,
                                        )))
                                        .unwrap(),
                                );
                            }
                            if method == Method::GET
                                && path == "/open-apis/im/v1/messages/om_file/resources/file-key"
                            {
                                return Ok::<_, hyper::Error>(
                                    Response::builder()
                                        .status(StatusCode::OK)
                                        .header("content-type", "text/plain")
                                        .body(Full::new(bytes::Bytes::from_static(b"hello")))
                                        .unwrap(),
                                );
                            }
                            Ok::<_, hyper::Error>(
                                Response::builder()
                                    .status(StatusCode::NOT_FOUND)
                                    .body(Full::new(bytes::Bytes::from_static(b"not found")))
                                    .unwrap(),
                            )
                        },
                    );
                    let _ = http1::Builder::new().serve_connection(io, service).await;
                });
            }
        });

        let client =
            ImProviderClient::Feishu(Arc::new(crate::im_gateway::feishu::FeishuProvider::new()));
        let mut provider = provider_with_secret();
        provider.provider_type = crate::im_gateway::types::ImProviderType::Feishu;
        provider.base_url = Some(format!("http://127.0.0.1:{port}/open-apis"));
        let file = crate::im_gateway::types::ImFileAttachment {
            file_key: "file-key".to_string(),
            name: Some("report.txt".to_string()),
            mime_type: Some("text/plain".to_string()),
            size_bytes: Some(5),
            data_base64: None,
            download_url: None,
            ..Default::default()
        };

        let (mime_type, bytes) = client
            .download_message_file_resource(&provider, "om_file", &file)
            .await
            .expect("feishu client downloads file resource");

        assert_eq!(mime_type, "text/plain");
        assert_eq!(bytes, b"hello");
    }

    #[test]
    fn service_startup_discards_noncanonical_session_history() {
        let temp_dir = tempfile::tempdir().unwrap();
        let legacy = write_legacy_history(temp_dir.path(), "session-legacy-1.jsonl");

        let _service = ImGatewayService::new(temp_dir.path());

        assert!(!legacy.exists());
    }

    #[cfg(unix)]
    #[test]
    fn service_startup_reports_noncanonical_cleanup_failure() {
        use std::os::unix::fs::PermissionsExt;

        let temp_dir = tempfile::tempdir().unwrap();
        let legacy = write_legacy_history(temp_dir.path(), "session-legacy-locked.jsonl");
        let parent = legacy.parent().unwrap();
        let original_mode = std::fs::metadata(parent).unwrap().permissions().mode();
        std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o555)).unwrap();

        let _service = ImGatewayService::new(temp_dir.path());

        std::fs::set_permissions(parent, std::fs::Permissions::from_mode(original_mode)).unwrap();
        assert!(legacy.exists());
    }
}
