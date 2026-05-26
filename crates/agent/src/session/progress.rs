//! Progress event helpers for session turn loops.

use crate::session_status::{AgentTurnProgressEvent, AgentTurnProgressSender};

pub(super) fn assistant_delta(sender: Option<&AgentTurnProgressSender>, content: String) {
    if let Some(sender) = sender {
        let _ = sender.send(AgentTurnProgressEvent::AssistantDelta { content });
    }
}

pub(super) fn assistant_final(sender: Option<&AgentTurnProgressSender>, content: String) {
    if let Some(sender) = sender {
        let _ = sender.send(AgentTurnProgressEvent::AssistantFinal { content });
    }
}
