use bifrost_skills::{SkillInvocation, SkillRecord, SkillRegistry};
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BuiltinCommand {
    Clear,
    Reset,
    Undo,
    Compact,
    Status,
    Resume,
    Remember,
    Memories,
    Forget,
    Skill,
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
        router.register_builtin("/clear", BuiltinCommand::Clear);
        router.register_builtin("/reset", BuiltinCommand::Reset);
        router.register_builtin("/undo", BuiltinCommand::Undo);
        router.register_builtin("/compact", BuiltinCommand::Compact);
        router.register_builtin("/status", BuiltinCommand::Status);
        router.register_builtin("/resume", BuiltinCommand::Resume);
        router.register_builtin("/remember", BuiltinCommand::Remember);
        router.register_builtin("/memories", BuiltinCommand::Memories);
        router.register_builtin("/forget", BuiltinCommand::Forget);
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
}

fn split_command(input: &str) -> (&str, &str) {
    input
        .split_once(char::is_whitespace)
        .map(|(command, args)| (command, args.trim()))
        .unwrap_or((input, ""))
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
            SkillScope::Project,
            dir.path(),
        )]));
        let mut manifest = SkillManifest::minimal_inline("weather", "weather", SkillScope::Project);
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
}
