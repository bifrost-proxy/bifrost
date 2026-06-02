#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentSlashMode {
    pub message: String,
    pub collaboration_mode: Option<bifrost_agent::CollaborationMode>,
}

/// Parse user-facing slash modes that should map to Agent runtime settings.
///
/// `/plan <message>` is intentionally stripped before reaching the model so
/// the user request remains clean while the turn runs under Plan Mode.
pub fn parse_agent_slash_mode(message: &str) -> AgentSlashMode {
    let trimmed = message.trim_start();
    let Some(rest) = trimmed.strip_prefix("/plan") else {
        return AgentSlashMode {
            message: message.to_string(),
            collaboration_mode: None,
        };
    };

    let is_command_boundary = rest.chars().next().is_none_or(|ch| ch.is_whitespace());
    if !is_command_boundary {
        return AgentSlashMode {
            message: message.to_string(),
            collaboration_mode: None,
        };
    }

    AgentSlashMode {
        message: rest.trim_start().to_string(),
        collaboration_mode: Some(bifrost_agent::CollaborationMode::Plan),
    }
}

pub fn merge_collaboration_mode(
    explicit: Option<bifrost_agent::CollaborationMode>,
    slash: Option<bifrost_agent::CollaborationMode>,
) -> Option<bifrost_agent::CollaborationMode> {
    explicit.or(slash)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plan_slash_and_strips_command() {
        let parsed = parse_agent_slash_mode("  /plan   对照 Codex 做方案");

        assert_eq!(parsed.message, "对照 Codex 做方案");
        assert_eq!(
            parsed.collaboration_mode,
            Some(bifrost_agent::CollaborationMode::Plan)
        );
    }

    #[test]
    fn keeps_non_plan_slash_messages_unchanged() {
        let parsed = parse_agent_slash_mode("/planner should be normal text");

        assert_eq!(parsed.message, "/planner should be normal text");
        assert_eq!(parsed.collaboration_mode, None);
    }

    #[test]
    fn supports_multiline_plan_slash_messages() {
        let parsed = parse_agent_slash_mode("/plan\n先分析\n再给方案");

        assert_eq!(parsed.message, "先分析\n再给方案");
        assert_eq!(
            parsed.collaboration_mode,
            Some(bifrost_agent::CollaborationMode::Plan)
        );
    }

    #[test]
    fn explicit_collaboration_mode_wins_over_slash_mode() {
        assert_eq!(
            merge_collaboration_mode(
                Some(bifrost_agent::CollaborationMode::Default),
                Some(bifrost_agent::CollaborationMode::Plan),
            ),
            Some(bifrost_agent::CollaborationMode::Default)
        );
    }
}
