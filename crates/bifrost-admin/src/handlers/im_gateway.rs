use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

use base64::Engine as _;
use http_body_util::BodyExt;
use hyper::{body::Incoming, Method, Request, Response, StatusCode};
use parking_lot::RwLock;
use serde::Deserialize;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

use crate::handlers::{error_response, full_body, json_response, method_not_allowed, BoxBody};
use crate::im_gateway::event_router::ImEventRouter;
use crate::im_gateway::progress_card::ImAgentProgressRegistry;
use crate::im_gateway::provider::ImProvider;
use crate::im_gateway::types::{
    normalize_provider_base_url, ImEvent, ImImageAttachment, ImMessageLog, ImProviderAgentConfig,
    ImProviderConfig, ImProviderType, ImRoute, ImRouteAction, ImSchedule, ImTarget,
    MessageDirection, MessageStatus,
};
use crate::im_gateway::weixin::WeixinProvider;
use crate::im_gateway::{
    ImAgentConfigStore, ImAgentSessionManager, ImConnectionManager, ImEventStore,
    ImGroupContextStore, ImMessageLogStore, ImProviderStore, ImRouteStore, ImRunStore,
    ImScheduleStore, ImScheduler, ImTargetStore, SessionQueueManager,
};
use bifrost_agent::persistence::ConversationRecorder;
use bifrost_agent::{PlanStep, SessionDetail, ToolCallLog};

mod agent_api;
mod agent_chat;
mod agent_reply;
mod agent_reply_attachments;
mod agent_reply_target;
mod busy_message_mode;
mod chat_gateway;
mod debug_inbound;
mod event_loop;
mod messages;
mod providers;
mod schedules;
mod service;
mod utils;

#[cfg(test)]
mod tests;

#[allow(unused_imports)]
use agent_api::*;
use agent_chat::*;
use agent_reply::*;
use agent_reply_attachments::*;
use agent_reply_target::*;
use busy_message_mode::*;
use event_loop::*;
#[allow(unused_imports)]
use messages::*;
use providers::*;
use schedules::*;
use service::{
    save_pending_feishu_setups, FeishuSetupBrand, ImProviderClient, PendingFeishuSetup,
    PendingWeixinLogin, AGENT_REPLY_IMAGE_UPLOAD_CACHE, IMAGE_ONLY_AGENT_PROMPT,
    MAX_AGENT_IMAGES_PER_MESSAGE, MAX_AGENT_REPLY_ATTACHMENT_BYTES, MAX_AGENT_REPLY_IMAGE_BYTES,
};
pub use service::{ImGatewayService, SharedImGatewayService};
use utils::*;

pub async fn handle_im_gateway(
    req: Request<Incoming>,
    service: Option<SharedImGatewayService>,
    path: &str,
) -> Response<BoxBody> {
    let Some(service) = service else {
        return error_response(StatusCode::SERVICE_UNAVAILABLE, "IM Gateway not configured");
    };

    let sub = path.strip_prefix("/api/im-gateway").unwrap_or(path);

    if let Some(rest) = sub.strip_prefix("/attachments") {
        return utils::handle_attachment(req, rest).await;
    }
    if let Some(rest) = sub.strip_prefix("/providers") {
        return providers::handle_providers(req, &service, rest).await;
    }
    if let Some(rest) = sub.strip_prefix("/targets") {
        return messages::handle_targets(req, &service, rest).await;
    }
    if sub == "/messages/send" || sub == "/messages/send/" {
        return messages::handle_messages_send(req, &service).await;
    }
    if let Some(rest) = sub.strip_prefix("/routes") {
        return messages::handle_routes(req, &service, rest).await;
    }
    if let Some(rest) = sub.strip_prefix("/agent") {
        return agent_api::handle_agent(req, &service, rest).await;
    }
    if let Some(rest) = sub.strip_prefix("/chat") {
        return chat_gateway::handle_chat_gateway(req, &service, rest).await;
    }
    if let Some(rest) = sub.strip_prefix("/debug") {
        return debug_inbound::handle_debug(req, &service, rest).await;
    }
    if let Some(rest) = sub.strip_prefix("/schedules") {
        return schedules::handle_schedules(req, &service, rest).await;
    }
    if let Some(rest) = sub.strip_prefix("/history") {
        return utils::handle_history(&req, &service, rest);
    }

    error_response(StatusCode::NOT_FOUND, "IM Gateway endpoint not found")
}
