//! Slash command dispatch helpers for session turns.

use crate::slash::{Dispatch, SlashCommandRouter};

pub(super) fn dispatch(router: &SlashCommandRouter, input: &str) -> Dispatch {
    router.dispatch(input)
}
