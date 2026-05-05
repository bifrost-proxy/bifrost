use std::collections::BTreeMap;

use regex::Regex;
use tracing::{debug, warn};

use super::types::{ImEvent, ImEventType, ImRoute};

/// Result of matching an event against a route.
#[derive(Debug, Clone)]
pub struct RouteMatch {
    pub route: ImRoute,
    pub named_captures: BTreeMap<String, String>,
    pub message_text: Option<String>,
}

pub struct ImEventRouter;

impl ImEventRouter {
    /// Match an inbound event against a list of routes.
    /// Returns all matching routes (first-match semantics is left to the caller).
    pub fn match_routes(event: &ImEvent, routes: &[ImRoute]) -> Vec<RouteMatch> {
        let mut matches = Vec::new();

        for route in routes {
            if !route.enabled {
                continue;
            }

            // Filter by provider_id
            if route.provider_id != event.provider_id {
                continue;
            }

            if let Some(route_match) = Self::try_match(event, route) {
                matches.push(route_match);
            }
        }

        debug!(
            event_id = %event.event_id,
            matched_count = matches.len(),
            "event routing completed"
        );

        matches
    }

    /// Attempt to match a single event against a single route.
    /// Returns `Some(RouteMatch)` if all matcher criteria are satisfied.
    fn try_match(event: &ImEvent, route: &ImRoute) -> Option<RouteMatch> {
        // 1. Match event_type
        let expected_event_type = event_type_to_string(&route.event_type);
        if event.event_type != expected_event_type {
            return None;
        }

        // 2. Match chat_ids (if specified, event must match one)
        if !route.matcher.chat_ids.is_empty() {
            let chat_id = event.source.chat_id.as_deref().unwrap_or("");
            if !route.matcher.chat_ids.iter().any(|id| id == chat_id) {
                return None;
            }
        }

        // 3. Match user_ids (if specified, event must match one)
        if !route.matcher.user_ids.is_empty() {
            let user_id = event.source.user_id.as_deref().unwrap_or("");
            if !route.matcher.user_ids.iter().any(|id| id == user_id) {
                return None;
            }
        }

        // Get message text for keyword/regex matching
        let message_text = event
            .message
            .as_ref()
            .map(|m| m.text.clone())
            .unwrap_or_default();

        // 4. Match keyword (case-insensitive substring match)
        if let Some(ref keyword) = route.matcher.keyword {
            if !keyword.is_empty()
                && !message_text
                    .to_lowercase()
                    .contains(&keyword.to_lowercase())
            {
                return None;
            }
        }

        // 5. Match regex (with named captures)
        let mut named_captures = BTreeMap::new();
        if let Some(ref pattern) = route.matcher.regex {
            if !pattern.is_empty() {
                match Regex::new(pattern) {
                    Ok(re) => {
                        if let Some(caps) = re.captures(&message_text) {
                            // Extract named captures
                            for name in re.capture_names().flatten() {
                                if let Some(m) = caps.name(name) {
                                    named_captures.insert(name.to_string(), m.as_str().to_string());
                                }
                            }
                        } else {
                            // Regex didn't match
                            return None;
                        }
                    }
                    Err(e) => {
                        warn!(
                            route_id = %route.id,
                            pattern = %pattern,
                            error = %e,
                            "invalid regex pattern in route matcher"
                        );
                        return None;
                    }
                }
            }
        }

        Some(RouteMatch {
            route: route.clone(),
            named_captures,
            message_text: if message_text.is_empty() {
                None
            } else {
                Some(message_text)
            },
        })
    }
}

fn event_type_to_string(event_type: &ImEventType) -> &'static str {
    match event_type {
        ImEventType::MessageReceive => "message.receive",
        ImEventType::AppMention => "app.mention",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::im_gateway::types::*;

    fn make_event(
        event_type: &str,
        provider_id: &str,
        chat_id: Option<&str>,
        user_id: Option<&str>,
        text: &str,
    ) -> ImEvent {
        ImEvent {
            event_id: "ev_001".to_string(),
            provider_id: provider_id.to_string(),
            provider_type: ImProviderType::Feishu,
            event_type: event_type.to_string(),
            source: ImEventSource {
                chat_id: chat_id.map(|s| s.to_string()),
                user_id: user_id.map(|s| s.to_string()),
                message_id: None,
            },
            message: Some(ImEventMessage {
                text: text.to_string(),
                mentions: vec![],
                images: vec![],
                raw_type: None,
            }),
            received_at: 1000,
            raw_digest: None,
        }
    }

    fn make_route(
        id: &str,
        provider_id: &str,
        event_type: ImEventType,
        chat_ids: Vec<&str>,
        user_ids: Vec<&str>,
        keyword: Option<&str>,
        regex: Option<&str>,
    ) -> ImRoute {
        ImRoute {
            id: id.to_string(),
            provider_id: provider_id.to_string(),
            name: format!("route_{id}"),
            enabled: true,
            event_type,
            matcher: ImEventMatcher {
                chat_ids: chat_ids.into_iter().map(|s| s.to_string()).collect(),
                user_ids: user_ids.into_iter().map(|s| s.to_string()).collect(),
                keyword: keyword.map(|s| s.to_string()),
                regex: regex.map(|s| s.to_string()),
            },
            action: ImRouteAction::RunScriptAndReply {
                script_text: Some("echo ok".to_string()),
                script_file: None,
                cwd: None,
                env: BTreeMap::new(),
                reply_target: ReplyTarget::OriginalChat,
                reply_mode: ReplyMode::TextSummary,
            },
            timeout_ms: 30000,
            max_output_bytes: 4096,
            created_at: 0,
            updated_at: 0,
        }
    }

    #[test]
    fn test_basic_event_type_match() {
        let event = make_event(
            "message.receive",
            "prov1",
            Some("chat1"),
            Some("user1"),
            "hello",
        );
        let routes = vec![make_route(
            "r1",
            "prov1",
            ImEventType::MessageReceive,
            vec!["chat1"],
            vec![],
            None,
            None,
        )];

        let matches = ImEventRouter::match_routes(&event, &routes);
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].route.id, "r1");
    }

    #[test]
    fn test_event_type_mismatch() {
        let event = make_event("app.mention", "prov1", Some("chat1"), None, "@bot hi");
        let routes = vec![make_route(
            "r1",
            "prov1",
            ImEventType::MessageReceive,
            vec!["chat1"],
            vec![],
            None,
            None,
        )];

        let matches = ImEventRouter::match_routes(&event, &routes);
        assert!(matches.is_empty());
    }

    #[test]
    fn test_provider_id_mismatch() {
        let event = make_event("message.receive", "prov2", Some("chat1"), None, "hello");
        let routes = vec![make_route(
            "r1",
            "prov1",
            ImEventType::MessageReceive,
            vec!["chat1"],
            vec![],
            None,
            None,
        )];

        let matches = ImEventRouter::match_routes(&event, &routes);
        assert!(matches.is_empty());
    }

    #[test]
    fn test_chat_id_filter() {
        let event = make_event(
            "message.receive",
            "prov1",
            Some("chat_other"),
            None,
            "hello",
        );
        let routes = vec![make_route(
            "r1",
            "prov1",
            ImEventType::MessageReceive,
            vec!["chat1", "chat2"],
            vec![],
            None,
            None,
        )];

        let matches = ImEventRouter::match_routes(&event, &routes);
        assert!(matches.is_empty());
    }

    #[test]
    fn test_user_id_filter() {
        let event = make_event(
            "message.receive",
            "prov1",
            Some("chat1"),
            Some("user_bob"),
            "hi",
        );
        let routes = vec![make_route(
            "r1",
            "prov1",
            ImEventType::MessageReceive,
            vec![],
            vec!["user_alice", "user_carol"],
            None,
            None,
        )];

        let matches = ImEventRouter::match_routes(&event, &routes);
        assert!(matches.is_empty());
    }

    #[test]
    fn test_keyword_match_case_insensitive() {
        let event = make_event(
            "message.receive",
            "prov1",
            Some("chat1"),
            None,
            "Please DEPLOY now",
        );
        let routes = vec![make_route(
            "r1",
            "prov1",
            ImEventType::MessageReceive,
            vec!["chat1"],
            vec![],
            Some("deploy"),
            None,
        )];

        let matches = ImEventRouter::match_routes(&event, &routes);
        assert_eq!(matches.len(), 1);
    }

    #[test]
    fn test_keyword_no_match() {
        let event = make_event(
            "message.receive",
            "prov1",
            Some("chat1"),
            None,
            "hello world",
        );
        let routes = vec![make_route(
            "r1",
            "prov1",
            ImEventType::MessageReceive,
            vec!["chat1"],
            vec![],
            Some("deploy"),
            None,
        )];

        let matches = ImEventRouter::match_routes(&event, &routes);
        assert!(matches.is_empty());
    }

    #[test]
    fn test_regex_with_named_captures() {
        let event = make_event(
            "message.receive",
            "prov1",
            Some("chat1"),
            None,
            "/check bifrost",
        );
        let routes = vec![make_route(
            "r1",
            "prov1",
            ImEventType::MessageReceive,
            vec!["chat1"],
            vec![],
            None,
            Some(r"^/check (?P<service>\S+)$"),
        )];

        let matches = ImEventRouter::match_routes(&event, &routes);
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].named_captures.get("service").unwrap(), "bifrost");
        assert_eq!(
            matches[0].message_text.as_deref().unwrap(),
            "/check bifrost"
        );
    }

    #[test]
    fn test_regex_no_match() {
        let event = make_event(
            "message.receive",
            "prov1",
            Some("chat1"),
            None,
            "hello world",
        );
        let routes = vec![make_route(
            "r1",
            "prov1",
            ImEventType::MessageReceive,
            vec!["chat1"],
            vec![],
            None,
            Some(r"^/check (?P<service>\S+)$"),
        )];

        let matches = ImEventRouter::match_routes(&event, &routes);
        assert!(matches.is_empty());
    }

    #[test]
    fn test_invalid_regex_skips_route() {
        let event = make_event("message.receive", "prov1", Some("chat1"), None, "hello");
        let routes = vec![make_route(
            "r1",
            "prov1",
            ImEventType::MessageReceive,
            vec!["chat1"],
            vec![],
            None,
            Some(r"[invalid("),
        )];

        let matches = ImEventRouter::match_routes(&event, &routes);
        assert!(matches.is_empty());
    }

    #[test]
    fn test_disabled_route_skipped() {
        let event = make_event("message.receive", "prov1", Some("chat1"), None, "hello");
        let mut route = make_route(
            "r1",
            "prov1",
            ImEventType::MessageReceive,
            vec!["chat1"],
            vec![],
            None,
            None,
        );
        route.enabled = false;

        let matches = ImEventRouter::match_routes(&event, &[route]);
        assert!(matches.is_empty());
    }

    #[test]
    fn test_multiple_routes_matched() {
        let event = make_event(
            "message.receive",
            "prov1",
            Some("chat1"),
            Some("user1"),
            "/deploy api",
        );
        let routes = vec![
            make_route(
                "r1",
                "prov1",
                ImEventType::MessageReceive,
                vec!["chat1"],
                vec![],
                Some("deploy"),
                None,
            ),
            make_route(
                "r2",
                "prov1",
                ImEventType::MessageReceive,
                vec![],
                vec!["user1"],
                None,
                Some(r"^/deploy (?P<target>\S+)$"),
            ),
        ];

        let matches = ImEventRouter::match_routes(&event, &routes);
        assert_eq!(matches.len(), 2);
        assert_eq!(matches[0].route.id, "r1");
        assert_eq!(matches[1].route.id, "r2");
        assert_eq!(matches[1].named_captures.get("target").unwrap(), "api");
    }

    #[test]
    fn test_empty_chat_ids_matches_any() {
        let event = make_event("message.receive", "prov1", Some("any_chat"), None, "hello");
        let routes = vec![make_route(
            "r1",
            "prov1",
            ImEventType::MessageReceive,
            vec![], // empty = match any
            vec![],
            Some("hello"),
            None,
        )];

        let matches = ImEventRouter::match_routes(&event, &routes);
        assert_eq!(matches.len(), 1);
    }

    #[test]
    fn test_no_message_with_keyword_fails() {
        let mut event = make_event("message.receive", "prov1", Some("chat1"), None, "");
        event.message = None;
        let routes = vec![make_route(
            "r1",
            "prov1",
            ImEventType::MessageReceive,
            vec!["chat1"],
            vec![],
            Some("deploy"),
            None,
        )];

        let matches = ImEventRouter::match_routes(&event, &routes);
        assert!(matches.is_empty());
    }
}
