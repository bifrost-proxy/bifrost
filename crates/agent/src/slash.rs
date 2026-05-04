use bifrost_skills::{SkillInvocation, SkillRecord, SkillRegistry};
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BuiltinCommand {
    Help,
    Clear,
    Reset,
    Undo,
    Compact,
    Status,
    Resume,
    Remember,
    Memories,
    Forget,
    Goal,
    Skill,
}

impl BuiltinCommand {
    /// Returns `true` if this command can be handled without taking the session.
    /// These commands don't read or mutate session history/state, so they can
    /// respond immediately even while another turn loop is running.
    pub fn is_session_free(&self) -> bool {
        matches!(
            self,
            BuiltinCommand::Help
                | BuiltinCommand::Remember
                | BuiltinCommand::Memories
                | BuiltinCommand::Forget
        )
    }
}

#[derive(Clone, Debug)]
pub struct BuiltinHandler {
    pub command: BuiltinCommand,
}

#[derive(Clone, Debug)]
pub enum Dispatch {
    Builtin {
        command: BuiltinCommand,
        args: String,
    },
    RunSkill {
        record: Box<SkillRecord>,
        invocation: SkillInvocation,
    },
    NotACommand,
    Unknown(String),
}

#[derive(Clone, Default)]
pub struct SlashCommandRouter {
    builtin: HashMap<String, BuiltinHandler>,
    skills: Option<Arc<SkillRegistry>>,
}

impl SlashCommandRouter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_default_builtins() -> Self {
        let mut router = Self::new();
        router.register_builtin("/help", BuiltinCommand::Help);
        router.register_builtin("/clear", BuiltinCommand::Clear);
        router.register_builtin("/reset", BuiltinCommand::Reset);
        router.register_builtin("/undo", BuiltinCommand::Undo);
        router.register_builtin("/compact", BuiltinCommand::Compact);
        router.register_builtin("/status", BuiltinCommand::Status);
        router.register_builtin("/resume", BuiltinCommand::Resume);
        router.register_builtin("/remember", BuiltinCommand::Remember);
        router.register_builtin("/memories", BuiltinCommand::Memories);
        router.register_builtin("/forget", BuiltinCommand::Forget);
        router.register_builtin("/goal", BuiltinCommand::Goal);
        router.register_builtin("/skill", BuiltinCommand::Skill);
        router
    }

    pub fn with_skills(mut self, skills: Arc<SkillRegistry>) -> Self {
        self.skills = Some(skills);
        self
    }

    pub fn register_builtin(&mut self, name: &str, command: BuiltinCommand) {
        self.builtin
            .insert(name.to_string(), BuiltinHandler { command });
    }

    pub fn dispatch(&self, input: &str) -> Dispatch {
        let trimmed = input.trim();
        if !trimmed.starts_with('/') {
            return Dispatch::NotACommand;
        }
        let (command, args) = split_command(trimmed);
        if let Some(handler) = self.builtin.get(command) {
            return Dispatch::Builtin {
                command: handler.command.clone(),
                args: args.to_string(),
            };
        }
        if let Some(skills) = &self.skills {
            if let Some(record) = skills.resolve_slash(command) {
                return Dispatch::RunSkill {
                    record: Box::new(record),
                    invocation: SkillInvocation {
                        input: serde_json::json!({ "args": args }),
                        timeout_ms: None,
                    },
                };
            }
        }
        Dispatch::Unknown(command.to_string())
    }

    /// Quick dispatch for builtin-only commands (no skill resolution).
    /// Used before taking a session to handle session-free commands immediately.
    pub fn dispatch_builtin_only(input: &str) -> Dispatch {
        let trimmed = input.trim();
        if !trimmed.starts_with('/') {
            return Dispatch::NotACommand;
        }
        let (command, args) = split_command(trimmed);
        let builtin_cmd = match command {
            "/help" => BuiltinCommand::Help,
            "/remember" => BuiltinCommand::Remember,
            "/memories" => BuiltinCommand::Memories,
            "/forget" => BuiltinCommand::Forget,
            _ => return Dispatch::NotACommand, // not a session-free builtin
        };
        Dispatch::Builtin {
            command: builtin_cmd,
            args: args.to_string(),
        }
    }

    /// Generate help text listing all available commands.
    pub fn help_text(&self) -> String {
        let mut lines = vec!["可用命令:".to_string()];
        lines.push(String::new());
        lines.push("内置命令:".to_string());
        // Sort builtin commands for stable output
        let mut builtins: Vec<_> = self.builtin.keys().collect();
        builtins.sort();
        for name in builtins {
            if let Some(handler) = self.builtin.get(name) {
                let desc = builtin_description(&handler.command);
                lines.push(format!("  {name:<16} {desc}"));
            }
        }
        if let Some(skills) = &self.skills {
            let skill_cmds = skills.list_slash_commands();
            if !skill_cmds.is_empty() {
                lines.push(String::new());
                lines.push("Skill 命令:".to_string());
                for (cmd, desc) in &skill_cmds {
                    let desc_str = desc.as_deref().unwrap_or("(无描述)");
                    lines.push(format!("  {cmd:<16} {desc_str}"));
                }
            }
        }
        lines.push(String::new());
        lines.push("提示: 直接输入文本即可与 AI 对话。".to_string());
        lines.join("\n")
    }
}

fn split_command(input: &str) -> (&str, &str) {
    input
        .split_once(char::is_whitespace)
        .map(|(command, args)| (command, args.trim()))
        .unwrap_or((input, ""))
}

fn builtin_description(cmd: &BuiltinCommand) -> &'static str {
    match cmd {
        BuiltinCommand::Help => "显示此帮助信息",
        BuiltinCommand::Clear => "清除会话历史，开始新对话",
        BuiltinCommand::Reset => "重置会话（同 /clear）",
        BuiltinCommand::Undo => "回退最近 N 轮对话（默认 1），用法: /undo [N]",
        BuiltinCommand::Compact => "手动压缩会话历史以节省 token",
        BuiltinCommand::Status => "显示当前会话状态（消息数、token 用量等）",
        BuiltinCommand::Resume => "恢复最近一次保存的会话历史",
        BuiltinCommand::Remember => "保存一条长期记忆，用法: /remember <text>",
        BuiltinCommand::Memories => "列出当前可见的所有长期记忆",
        BuiltinCommand::Forget => "删除一条长期记忆，用法: /forget <id|last>",
        BuiltinCommand::Goal => {
            "管理当前目标，用法: /goal [show|set <objective>|set --budget N <objective>|complete]"
        }
        BuiltinCommand::Skill => "启动 Skill Creator，创建或编辑 skill",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bifrost_skills::{
        ScopeRoot, SkillDraft, SkillManifest, SkillScope, SkillStore, TriggerRule,
    };
    use tempfile::tempdir;

    #[test]
    fn dispatches_builtins_with_args() {
        let router = SlashCommandRouter::with_default_builtins();
        match router.dispatch("/remember hello") {
            Dispatch::Builtin { command, args } => {
                assert_eq!(command, BuiltinCommand::Remember);
                assert_eq!(args, "hello");
            }
            other => panic!("unexpected dispatch: {other:?}"),
        }
    }

    #[test]
    fn dispatches_skill_slash_when_registered() {
        let dir = tempdir().unwrap();
        let store = std::sync::Arc::new(SkillStore::new(vec![ScopeRoot::new(
            SkillScope::Repo,
            dir.path(),
        )]));
        let mut manifest = SkillManifest::minimal_inline("weather", "weather", SkillScope::Repo);
        manifest.slash_command = Some("/weather".into());
        manifest.triggers = vec![TriggerRule::SlashCommand];
        store
            .commit(SkillDraft {
                manifest,
                skill_md: "---\nname: weather\n---\n# Weather".into(),
                draft_dir: None,
                assets: Vec::new(),
            })
            .unwrap();
        let registry = std::sync::Arc::new(SkillRegistry::without_watcher(store).unwrap());
        let router = SlashCommandRouter::with_default_builtins().with_skills(registry);
        match router.dispatch("/weather Paris") {
            Dispatch::RunSkill { record, invocation } => {
                assert_eq!(record.name, "weather");
                assert_eq!(invocation.input["args"], "Paris");
            }
            other => panic!("unexpected dispatch: {other:?}"),
        }
    }

    #[test]
    fn dispatches_help() {
        let router = SlashCommandRouter::with_default_builtins();
        match router.dispatch("/help") {
            Dispatch::Builtin { command, args } => {
                assert_eq!(command, BuiltinCommand::Help);
                assert!(args.is_empty());
            }
            other => panic!("unexpected dispatch: {other:?}"),
        }
    }

    #[test]
    fn help_text_contains_all_builtins() {
        let router = SlashCommandRouter::with_default_builtins();
        let text = router.help_text();
        assert!(text.contains("/help"));
        assert!(text.contains("/clear"));
        assert!(text.contains("/reset"));
        assert!(text.contains("/undo"));
        assert!(text.contains("/compact"));
        assert!(text.contains("/status"));
        assert!(text.contains("/resume"));
        assert!(text.contains("/remember"));
        assert!(text.contains("/memories"));
        assert!(text.contains("/forget"));
        assert!(text.contains("/goal"));
        assert!(text.contains("/skill"));
    }

    #[test]
    fn help_text_includes_skill_slash_commands() {
        let dir = tempdir().unwrap();
        let store = std::sync::Arc::new(SkillStore::new(vec![ScopeRoot::new(
            SkillScope::Repo,
            dir.path(),
        )]));
        let mut manifest =
            SkillManifest::minimal_inline("weather", "lookup forecast", SkillScope::Repo);
        manifest.slash_command = Some("/weather".into());
        manifest.triggers = vec![TriggerRule::SlashCommand];
        store
            .commit(SkillDraft {
                manifest,
                skill_md: "---\nname: weather\n---\n# Weather".into(),
                draft_dir: None,
                assets: Vec::new(),
            })
            .unwrap();
        let registry = std::sync::Arc::new(SkillRegistry::without_watcher(store).unwrap());
        let router = SlashCommandRouter::with_default_builtins().with_skills(registry);
        let text = router.help_text();
        assert!(text.contains("Skill 命令:"));
        assert!(text.contains("/weather"));
        assert!(text.contains("lookup forecast"));
    }
}
