use async_trait::async_trait;
use tokio::sync::mpsc;

use bifrost_core::Result;

use super::types::{
    ConnectionHandle, ImChannelCapabilities, ImConversationCapabilities, ImEvent,
    ImInteractionCapabilities, ImProviderConfig, ImProviderType, ImSendCapabilities, ImTarget,
    ProviderValidation, SendOptions, SendResult, UploadedImage,
};

pub type EventSink = mpsc::UnboundedSender<ImEvent>;

#[async_trait]
pub trait ImProvider: Send + Sync {
    fn provider_type(&self) -> ImProviderType;

    fn send_capabilities(&self, config: &ImProviderConfig) -> ImSendCapabilities;

    fn channel_capabilities(&self, config: &ImProviderConfig) -> ImChannelCapabilities {
        ImChannelCapabilities {
            send: self.send_capabilities(config),
            interaction: ImInteractionCapabilities::default(),
            conversation: ImConversationCapabilities {
                direct: true,
                requires_context: false,
                ..Default::default()
            },
        }
    }

    async fn validate_config(&self, config: &ImProviderConfig) -> Result<ProviderValidation>;

    async fn connect_events(
        &self,
        config: &ImProviderConfig,
        sink: EventSink,
    ) -> Result<ConnectionHandle>;

    async fn send_card(
        &self,
        config: &ImProviderConfig,
        target: &ImTarget,
        card: serde_json::Value,
        opts: SendOptions,
    ) -> Result<SendResult>;

    async fn send_text(
        &self,
        config: &ImProviderConfig,
        target: &ImTarget,
        text: &str,
    ) -> Result<SendResult>;

    async fn send_text_with_uuid(
        &self,
        config: &ImProviderConfig,
        target: &ImTarget,
        text: &str,
        _uuid: Option<&str>,
    ) -> Result<SendResult> {
        self.send_text(config, target, text).await
    }

    async fn upload_image(
        &self,
        config: &ImProviderConfig,
        image_type: &str,
        file_name: &str,
        bytes: Vec<u8>,
        mime_type: Option<&str>,
    ) -> Result<UploadedImage>;

    async fn send_image(
        &self,
        config: &ImProviderConfig,
        target: &ImTarget,
        image_key: &str,
        uuid: Option<&str>,
    ) -> Result<SendResult>;

    async fn upload_file(
        &self,
        _config: &ImProviderConfig,
        _file_name: &str,
        _bytes: Vec<u8>,
        _mime_type: Option<&str>,
    ) -> Result<String> {
        Err(bifrost_core::BifrostError::Config(format!(
            "{} provider does not support generic file attachments",
            serde_json::to_value(self.provider_type())
                .ok()
                .and_then(|value| value.as_str().map(str::to_string))
                .unwrap_or_else(|| "unknown".to_string())
        )))
    }

    async fn send_file(
        &self,
        _config: &ImProviderConfig,
        _target: &ImTarget,
        _file_key: &str,
        _uuid: Option<&str>,
    ) -> Result<SendResult> {
        Err(bifrost_core::BifrostError::Config(format!(
            "{} provider does not support generic file attachments",
            serde_json::to_value(self.provider_type())
                .ok()
                .and_then(|value| value.as_str().map(str::to_string))
                .unwrap_or_else(|| "unknown".to_string())
        )))
    }

    async fn send_native_card(
        &self,
        config: &ImProviderConfig,
        target: &ImTarget,
        card: serde_json::Value,
        opts: SendOptions,
    ) -> Result<SendResult> {
        self.send_card(config, target, card, opts).await
    }
}
