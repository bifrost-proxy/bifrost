use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::mpsc;

use bifrost_core::{BifrostError, Result};

use super::event_store::ImEventStore;
use super::types::{
    ConnectionHandle, ImChannelCapabilities, ImConversationCapabilities, ImEvent,
    ImInteractionCapabilities, ImProviderConfig, ImProviderType, ImSendCapabilities, ImTarget,
    ProviderValidation, SendOptions, SendResult, UploadedImage,
};

#[derive(Clone)]
pub struct EventSink {
    sender: mpsc::UnboundedSender<ImEvent>,
    durable_store: Option<Arc<ImEventStore>>,
}

impl EventSink {
    pub fn with_durable_store(
        sender: mpsc::UnboundedSender<ImEvent>,
        durable_store: Arc<ImEventStore>,
    ) -> Self {
        Self {
            sender,
            durable_store: Some(durable_store),
        }
    }

    pub fn send(
        &self,
        event: ImEvent,
    ) -> std::result::Result<(), Box<mpsc::error::SendError<ImEvent>>> {
        self.sender.send(event).map_err(Box::new)
    }

    pub fn persist_and_send(&self, event: ImEvent) -> Result<()> {
        let store = self.durable_store.as_ref().ok_or_else(|| {
            BifrostError::Config("durable IM event store is unavailable".to_string())
        })?;
        store.add(event.clone())?;
        self.sender
            .send(event)
            .map_err(|_| BifrostError::Config("IM event sink is closed".to_string()))
    }

    pub fn is_closed(&self) -> bool {
        self.sender.is_closed()
    }
}

impl From<mpsc::UnboundedSender<ImEvent>> for EventSink {
    fn from(sender: mpsc::UnboundedSender<ImEvent>) -> Self {
        Self {
            sender,
            durable_store: None,
        }
    }
}

#[cfg(test)]
mod event_sink_tests {
    use super::*;

    fn event(event_id: &str) -> ImEvent {
        ImEvent {
            event_id: event_id.to_string(),
            provider_id: "provider".to_string(),
            provider_type: ImProviderType::Weixin,
            event_type: "message.receive".to_string(),
            source: Default::default(),
            message: None,
            received_at: 1,
            raw_digest: None,
        }
    }

    #[test]
    fn send_delivers_open_channel_and_returns_closed_channel_event() {
        let (sender, mut receiver) = mpsc::unbounded_channel();
        let sink = EventSink::from(sender);

        sink.send(event("delivered")).unwrap();
        assert_eq!(receiver.try_recv().unwrap().event_id, "delivered");

        drop(receiver);
        let error = sink.send(event("closed")).unwrap_err();
        assert_eq!(error.0.event_id, "closed");
    }
}

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
