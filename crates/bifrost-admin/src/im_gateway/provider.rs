use async_trait::async_trait;
use tokio::sync::mpsc;

use bifrost_core::Result;

use super::types::{
    ConnectionHandle, ImEvent, ImProviderConfig, ImProviderType, ImTarget, ProviderValidation,
    SendOptions, SendResult,
};

pub type EventSink = mpsc::UnboundedSender<ImEvent>;

#[async_trait]
pub trait ImProvider: Send + Sync {
    fn provider_type(&self) -> ImProviderType;

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
}
