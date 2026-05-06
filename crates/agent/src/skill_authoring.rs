//! Session-scoped wrapper around `bifrost_skills::SkillAuthoringSession`.
//!
//! Wires the built-in `/skill` slash command to the skill authoring state machine,
//! so a single chat session can drive `start → draft → commit → cancel` via
//! slash subcommands without bypassing the `SkillStore::commit` pipeline
//! (validation, manifest.json persistence, stable checksum, `.history` archival,
//! and hot-reload via `SkillRegistry`).
//!
//! Subcommand grammar:
//! - `/skill list` — list currently registered slash commands
//! - `/skill status` — show the current authoring state (if any)
//! - `/skill start <brief>` — open a new authoring session for this chat
//! - `/skill draft <name>|<description>|<instructions_md>` — submit an inline draft
//! - `/skill commit` — validate + commit the current draft to the store
//! - `/skill cancel` — abandon the in-flight authoring session
//!
//! The draft subcommand uses `|` as the field separator so users can embed
//! whitespace and newlines in the instruction body. All fields are trimmed.
//!
//! This module intentionally keeps the API small: the turn loop in `session.rs`
//! calls [`dispatch`] with the raw args string and receives a user-facing
//! response plus an optional committed record.

use bifrost_skills::{
    Entrypoint, SkillAuthor, SkillAuthoringSession, SkillDraft, SkillManifest, SkillRecord,
    SkillRegistry, SkillScope, SkillStore, TriggerRule,
};
use dashmap::DashMap;
use std::sync::Arc;

/// Hub keyed by `session_key` so each chat has its own authoring state machine.
#[derive(Clone, Default)]
pub struct SkillAuthoringHub {
    by_session: Arc<DashMap<String, SkillAuthoringSession>>,
}

#[derive(Debug, Clone)]
pub struct SkillSlashOutcome {
    pub response: String,
    /// Populated when a `commit` subcommand successfully lands a skill.
    pub committed: Option<SkillRecord>,
}

impl SkillAuthoringHub {
    pub fn new() -> Self {
        Self {
            by_session: Arc::new(DashMap::new()),
        }
    }

    /// Dispatch a `/skill <subcommand> [args]` invocation.
    ///
    /// * `session_key` — stable per-chat key (usually `AgentSession::session_key`).
    /// * `store` — the shared skill store used by `SkillRegistry`.
    /// * `registry` — optional: used to refresh slash bindings after a commit.
    /// * `args` — everything after `/skill ` (may be empty).
    pub fn dispatch(
        &self,
        session_key: &str,
        store: Arc<SkillStore>,
        registry: Option<&SkillRegistry>,
        args: &str,
    ) -> SkillSlashOutcome {
        let (sub, rest) = split_sub(args);
        match sub {
            "" | "help" => SkillSlashOutcome {
                response: help_text(),
                committed: None,
            },
            "list" => {
                let lines: Vec<String> = registry
                    .map(|r| {
                        r.list_slash_commands()
                            .into_iter()
                            .map(|(cmd, desc)| {
                                let d = desc.as_deref().unwrap_or("(no description)");
                                format!("  {cmd:<20} {d}")
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                let body = if lines.is_empty() {
                    "当前没有已注册的 skill slash 命令。".to_string()
                } else {
                    format!("已注册的 skill slash 命令:\n{}", lines.join("\n"))
                };
                SkillSlashOutcome {
                    response: body,
                    committed: None,
                }
            }
            "status" => {
                let state = self
                    .by_session
                    .get(session_key)
                    .map(|s| format!("{:?}", s.state))
                    .unwrap_or_else(|| {
                        "无进行中的 skill 创作会话。使用 /skill start <brief> 开始。".to_string()
                    });
                SkillSlashOutcome {
                    response: state,
                    committed: None,
                }
            }
            "start" => {
                let brief = rest.trim();
                if brief.is_empty() {
                    return SkillSlashOutcome {
                        response: "用法: /skill start <brief>".to_string(),
                        committed: None,
                    };
                }
                let session = SkillAuthoringSession::start(Arc::clone(&store), brief);
                self.by_session.insert(session_key.to_string(), session);
                SkillSlashOutcome {
                    response:
                        "已开始 skill 创作会话。接下来用 `/skill draft <name>|<description>|<instructions_md>` 提交草稿。"
                            .to_string(),
                    committed: None,
                }
            }
            "draft" => {
                let fields: Vec<&str> = rest.splitn(3, '|').map(str::trim).collect();
                if fields.len() < 3 || fields[0].is_empty() || fields[1].is_empty() {
                    return SkillSlashOutcome {
                        response: "用法: /skill draft <name>|<description>|<instructions_md>"
                            .to_string(),
                        committed: None,
                    };
                }
                let (name, description, body) = (fields[0], fields[1], fields[2]);
                if !self.by_session.contains_key(session_key) {
                    // Auto-start if the user jumped straight to draft.
                    let session =
                        SkillAuthoringSession::start(Arc::clone(&store), format!("draft {name}"));
                    self.by_session.insert(session_key.to_string(), session);
                }
                // No mutation of the session state object is required for draft —
                // we only need to remember the payload until commit.
                let skill_md = build_skill_md(name, description, body);
                // Persist the pending draft into the session's custom slot via interview()
                // which stores arbitrary JSON.
                if let Some(mut entry) = self.by_session.get_mut(session_key) {
                    entry.interview(serde_json::json!({
                        "name": name,
                        "description": description,
                        "instructions_md": body,
                        "skill_md": skill_md,
                    }));
                }
                SkillSlashOutcome {
                    response: format!(
                        "草稿已记录。使用 `/skill commit` 校验并提交 `{name}`（scope=Repo）。"
                    ),
                    committed: None,
                }
            }
            "commit" => self.commit(session_key, store.as_ref()),
            "cancel" => {
                let removed = self.by_session.remove(session_key).is_some();
                SkillSlashOutcome {
                    response: if removed {
                        "已取消当前 skill 创作会话。".to_string()
                    } else {
                        "没有进行中的 skill 创作会话。".to_string()
                    },
                    committed: None,
                }
            }
            other => SkillSlashOutcome {
                response: format!("未知 /skill 子命令: {other}\n\n{}", help_text()),
                committed: None,
            },
        }
    }

    fn commit(&self, session_key: &str, store: &SkillStore) -> SkillSlashOutcome {
        let payload = match self.by_session.get(session_key) {
            Some(entry) => match &entry.state {
                bifrost_skills::AuthoringState::Interviewed { partial, .. } => partial.clone(),
                _ => {
                    return SkillSlashOutcome {
                        response: "当前会话还未提交草稿。使用 /skill draft <name>|<desc>|<body>。"
                            .to_string(),
                        committed: None,
                    };
                }
            },
            None => {
                return SkillSlashOutcome {
                    response: "没有进行中的 skill 创作会话。使用 /skill start <brief> 开始。"
                        .to_string(),
                    committed: None,
                };
            }
        };

        let name = payload
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let description = payload
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let body = payload
            .get("instructions_md")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let skill_md = payload
            .get("skill_md")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| build_skill_md(&name, &description, &body));

        let mut manifest = SkillManifest::minimal_inline(&name, &description, SkillScope::Repo);
        manifest.entrypoint = Entrypoint::Inline {
            instructions_md: body,
        };
        manifest.triggers = vec![TriggerRule::DescriptionMatch];
        manifest.created_by = SkillAuthor::Agent {
            session_id: session_key.to_string(),
        };

        let draft = SkillDraft {
            manifest,
            skill_md,
            draft_dir: None,
            assets: Vec::new(),
        };

        match store.commit(draft) {
            Ok(record) => {
                // Clear the authoring state now that the skill is live.
                self.by_session.remove(session_key);
                // The record's `checksum` is read from SKILL.md frontmatter, which
                // doesn't carry it; read manifest.json (written by the store) for
                // the real value.
                let checksum_blurb = store
                    .verify_checksum(record.scope.clone(), &record.name)
                    .ok()
                    .filter(|v| *v)
                    .map(|_| " checksum=verified".to_string())
                    .unwrap_or_default();
                SkillSlashOutcome {
                    response: format!(
                        "✔ skill `{}` 已提交（scope=Repo{}）。",
                        record.name, checksum_blurb
                    ),
                    committed: Some(record),
                }
            }
            Err(err) => SkillSlashOutcome {
                response: format!("✘ 提交失败: {err}"),
                committed: None,
            },
        }
    }

    /// Testing helper: report whether a session is currently tracked.
    pub fn has_session(&self, session_key: &str) -> bool {
        self.by_session.contains_key(session_key)
    }
}

fn split_sub(args: &str) -> (&str, &str) {
    let trimmed = args.trim_start();
    match trimmed.split_once(char::is_whitespace) {
        Some((head, tail)) => (head, tail.trim_start()),
        None => (trimmed, ""),
    }
}

fn build_skill_md(name: &str, description: &str, body: &str) -> String {
    format!(
        "---\nname: {name}\ndescription: {description}\n---\n\n{body}\n",
        name = name,
        description = description,
        body = body,
    )
}

fn help_text() -> String {
    [
        "/skill 子命令:",
        "  list                                     列出当前已注册的 skill slash 命令",
        "  status                                   查看当前创作会话状态",
        "  start <brief>                            开启新创作会话",
        "  draft <name>|<description>|<body>        记录草稿（字段以 | 分隔）",
        "  commit                                   校验并提交到 SkillStore",
        "  cancel                                   取消当前创作会话",
    ]
    .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use bifrost_skills::{ScopeRoot, SkillStore};
    use tempfile::tempdir;

    fn make_store() -> (tempfile::TempDir, Arc<SkillStore>) {
        let dir = tempdir().unwrap();
        let store = Arc::new(SkillStore::new(vec![ScopeRoot::new(
            SkillScope::Repo,
            dir.path(),
        )]));
        (dir, store)
    }

    #[test]
    fn help_subcommand_returns_usage() {
        let hub = SkillAuthoringHub::new();
        let (_dir, store) = make_store();
        let out = hub.dispatch("sess-1", store, None, "");
        assert!(out.response.contains("/skill 子命令"));
        assert!(out.committed.is_none());
    }

    #[test]
    fn start_then_draft_then_commit_lands_skill_in_store() {
        let hub = SkillAuthoringHub::new();
        let (_dir, store) = make_store();

        let out = hub.dispatch(
            "sess-A",
            Arc::clone(&store),
            None,
            "start build weather skill",
        );
        assert!(out.response.contains("skill 创作会话"));
        assert!(hub.has_session("sess-A"));

        let out = hub.dispatch(
            "sess-A",
            Arc::clone(&store),
            None,
            "draft weather-skill|lookup weather|Tell me the forecast.",
        );
        assert!(out.response.contains("草稿已记录"));

        let out = hub.dispatch("sess-A", Arc::clone(&store), None, "commit");
        let record = out.committed.expect("commit must return a record");
        assert_eq!(record.name, "weather-skill");
        assert_eq!(record.scope, SkillScope::Repo);
        // Verify the store actually persisted manifest.json with a non-empty checksum.
        assert!(
            store
                .verify_checksum(SkillScope::Repo, "weather-skill")
                .expect("verify_checksum after commit"),
            "manifest.json checksum must match disk state after commit"
        );
        assert!(!hub.has_session("sess-A"), "state cleared after commit");
        assert!(out.response.contains("已提交"));
    }

    #[test]
    fn commit_without_draft_reports_error() {
        let hub = SkillAuthoringHub::new();
        let (_dir, store) = make_store();
        hub.dispatch("sess-B", Arc::clone(&store), None, "start brief");
        let out = hub.dispatch("sess-B", Arc::clone(&store), None, "commit");
        assert!(out.committed.is_none());
        assert!(out.response.contains("还未提交草稿"));
    }

    #[test]
    fn draft_without_start_auto_starts_session() {
        let hub = SkillAuthoringHub::new();
        let (_dir, store) = make_store();
        let out = hub.dispatch(
            "sess-C",
            Arc::clone(&store),
            None,
            "draft auto-skill|auto desc|body here",
        );
        assert!(out.response.contains("草稿已记录"));
        assert!(hub.has_session("sess-C"));
        let out = hub.dispatch("sess-C", Arc::clone(&store), None, "commit");
        assert_eq!(out.committed.unwrap().name, "auto-skill");
    }

    #[test]
    fn cancel_clears_state() {
        let hub = SkillAuthoringHub::new();
        let (_dir, store) = make_store();
        hub.dispatch("sess-D", Arc::clone(&store), None, "start hello");
        assert!(hub.has_session("sess-D"));
        let out = hub.dispatch("sess-D", Arc::clone(&store), None, "cancel");
        assert!(out.response.contains("已取消"));
        assert!(!hub.has_session("sess-D"));
    }

    #[test]
    fn unknown_subcommand_reports_help() {
        let hub = SkillAuthoringHub::new();
        let (_dir, store) = make_store();
        let out = hub.dispatch("sess-E", store, None, "frobnicate");
        assert!(out.response.contains("未知 /skill 子命令"));
        assert!(out.response.contains("/skill 子命令"));
    }

    #[test]
    fn draft_requires_three_fields() {
        let hub = SkillAuthoringHub::new();
        let (_dir, store) = make_store();
        let out = hub.dispatch("sess-F", store, None, "draft only-name|only-desc");
        assert!(out.response.contains("用法"));
    }

    #[test]
    fn commit_invalid_name_returns_validator_error() {
        let hub = SkillAuthoringHub::new();
        let (_dir, store) = make_store();
        hub.dispatch("sess-G", Arc::clone(&store), None, "start bad");
        hub.dispatch(
            "sess-G",
            Arc::clone(&store),
            None,
            "draft BadName|desc|body",
        );
        let out = hub.dispatch("sess-G", Arc::clone(&store), None, "commit");
        assert!(out.committed.is_none());
        assert!(out.response.contains("提交失败"));
    }
}
