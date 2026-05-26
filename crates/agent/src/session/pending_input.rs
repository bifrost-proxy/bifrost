//! Pending user input queue helpers.

use super::AgentSession;

pub(super) fn pop_next_non_empty(session: &mut AgentSession) -> Option<String> {
    while let Some(message) = session.pending_messages.pop_front() {
        if !message.trim().is_empty() {
            return Some(message);
        }
    }
    None
}
