use std::collections::HashSet;
use std::fs;

use bifrost_core::{BifrostError, Result};
use bifrost_storage::{RemoteShellPolicy, RemoteShellProfile, RemoteShellSet, RemoteShellStore};
use serde_json::{json, Value};

use crate::cli::{
    RemoteShellCommands, RemoteShellPolicyAddArgs, RemoteShellPolicyCommands,
    RemoteShellPolicyUpdateArgs, RemoteShellProfileCommands,
};

pub fn handle_remote_shell_command(action: RemoteShellCommands) -> Result<()> {
    let store = RemoteShellStore::new()?;
    match action {
        RemoteShellCommands::List => print_shell_summary(&store.load()?),
        RemoteShellCommands::Show { json } => {
            let set = store.load()?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&set).map_err(config_error)?
                );
            } else {
                print_shell_summary(&set);
            }
        }
        RemoteShellCommands::Apply { file } => {
            let content = fs::read_to_string(&file).map_err(|error| {
                BifrostError::Io(std::io::Error::new(
                    error.kind(),
                    format!(
                        "failed to read shell config '{}': {}",
                        file.display(),
                        error
                    ),
                ))
            })?;
            let requested: RemoteShellSet = serde_json::from_str(&content).map_err(|error| {
                BifrostError::Parse(format!("invalid shell config JSON: {error}"))
            })?;
            let current = store.load()?;
            let next = prepare_for_save(current, requested)?;
            store.save(&next)?;
            println!("Shell access config saved (version {}).", next.version);
        }
        RemoteShellCommands::Profile { action } => handle_profile_command(&store, action)?,
        RemoteShellCommands::Policy { action } => handle_policy_command(&store, *action)?,
    }
    Ok(())
}

fn handle_profile_command(
    store: &RemoteShellStore,
    action: RemoteShellProfileCommands,
) -> Result<()> {
    let mut set = store.load()?;
    match action {
        RemoteShellProfileCommands::Add {
            id,
            name,
            description,
            cwd,
            env,
            default_cwd,
            timeout_ms,
            stdin,
            interactive,
            inherit_env,
            disabled,
        } => {
            reject_duplicate_profile(&set, &id)?;
            set.profiles.push(RemoteShellProfile {
                id: id.clone(),
                name,
                description,
                enabled: !disabled,
                metadata: json!({
                    "cwd_allowlist": cwd,
                    "env_allowlist": env,
                    "default_cwd": default_cwd,
                    "max_timeout_ms": timeout_ms,
                    "stdin_allowed": stdin,
                    "interactive_allowed": interactive,
                    "inherit_env": inherit_env,
                }),
            });
            save_bumped(store, set)?;
            println!("Added shell profile '{id}'.");
        }
        RemoteShellProfileCommands::Delete { id } => {
            let before = set.profiles.len();
            set.profiles.retain(|profile| profile.id != id);
            if before == set.profiles.len() {
                return Err(BifrostError::NotFound(format!(
                    "shell profile '{}' not found",
                    id
                )));
            }
            for policy in &mut set.policies {
                if policy.profile_id.as_deref() == Some(&id) {
                    policy.profile_id = None;
                }
            }
            save_bumped(store, set)?;
            println!("Deleted shell profile '{id}'.");
        }
        RemoteShellProfileCommands::Enable { id } => {
            set_profile_enabled(&mut set, &id, true)?;
            save_bumped(store, set)?;
            println!("Enabled shell profile '{id}'.");
        }
        RemoteShellProfileCommands::Disable { id } => {
            set_profile_enabled(&mut set, &id, false)?;
            save_bumped(store, set)?;
            println!("Disabled shell profile '{id}'.");
        }
    }
    Ok(())
}

fn handle_policy_command(
    store: &RemoteShellStore,
    action: RemoteShellPolicyCommands,
) -> Result<()> {
    let mut set = store.load()?;
    match action {
        RemoteShellPolicyCommands::Add(args) => {
            let RemoteShellPolicyAddArgs {
                id,
                name,
                description,
                mode,
                profile,
                program,
                pattern,
                cwd,
                env,
                default_cwd,
                timeout_ms,
                shell,
                stdin,
                interactive,
                inherit_env,
                disabled,
            } = *args;
            reject_duplicate_policy(&set, &id)?;
            if let Some(profile_id) = profile.as_deref() {
                ensure_profile_exists(&set, profile_id)?;
            }
            set.policies.push(RemoteShellPolicy {
                id: id.clone(),
                name,
                description,
                enabled: !disabled,
                profile_id: profile,
                metadata: json!({
                    "exec_mode": mode,
                    "allowed_executables": program,
                    "allowed_shell_patterns": pattern,
                    "cwd_allowlist": cwd,
                    "env_allowlist": env,
                    "default_cwd": default_cwd,
                    "max_timeout_ms": timeout_ms,
                    "shell": shell,
                    "stdin_allowed": stdin,
                    "interactive_allowed": interactive,
                    "inherit_env": inherit_env,
                }),
            });
            save_bumped(store, set)?;
            println!("Added shell policy '{id}'.");
        }
        RemoteShellPolicyCommands::Update(args) => {
            let RemoteShellPolicyUpdateArgs {
                id,
                name,
                description,
                mode,
                profile,
                program,
                pattern,
                cwd,
                env,
                default_cwd,
                timeout_ms,
                shell,
                stdin,
                interactive,
                inherit_env,
            } = *args;
            let policy_idx = set
                .policies
                .iter()
                .position(|p| p.id == id)
                .ok_or_else(|| {
                    BifrostError::NotFound(format!("shell policy '{}' not found", id))
                })?;
            if let Some(profile_id) = profile.as_deref() {
                ensure_profile_exists(&set, profile_id)?;
            }
            let policy = &mut set.policies[policy_idx];
            if let Some(name) = name {
                policy.name = name;
            }
            if let Some(desc) = description {
                policy.description = Some(desc);
            }
            if let Some(profile_id) = profile {
                policy.profile_id = Some(profile_id);
            }
            let meta = policy.metadata.as_object_mut().ok_or_else(|| {
                BifrostError::Config("policy metadata is not a JSON object".to_string())
            })?;
            if let Some(mode) = mode {
                meta.insert("exec_mode".to_string(), json!(mode));
            }
            if !program.is_empty() {
                meta.insert("allowed_executables".to_string(), json!(program));
            }
            if !pattern.is_empty() {
                meta.insert("allowed_shell_patterns".to_string(), json!(pattern));
            }
            if !cwd.is_empty() {
                meta.insert("cwd_allowlist".to_string(), json!(cwd));
            }
            if !env.is_empty() {
                meta.insert("env_allowlist".to_string(), json!(env));
            }
            if let Some(v) = default_cwd {
                meta.insert("default_cwd".to_string(), json!(v));
            }
            if let Some(v) = timeout_ms {
                meta.insert("max_timeout_ms".to_string(), json!(v));
            }
            if let Some(v) = shell {
                meta.insert("shell".to_string(), json!(v));
            }
            if let Some(v) = stdin {
                meta.insert("stdin_allowed".to_string(), json!(v));
            }
            if let Some(v) = interactive {
                meta.insert("interactive_allowed".to_string(), json!(v));
            }
            if let Some(v) = inherit_env {
                meta.insert("inherit_env".to_string(), json!(v));
            }
            save_bumped(store, set)?;
            println!("Updated shell policy '{id}'.");
        }
        RemoteShellPolicyCommands::Delete { id } => {
            let before = set.policies.len();
            set.policies.retain(|policy| policy.id != id);
            if before == set.policies.len() {
                return Err(BifrostError::NotFound(format!(
                    "shell policy '{}' not found",
                    id
                )));
            }
            save_bumped(store, set)?;
            println!("Deleted shell policy '{id}'.");
        }
        RemoteShellPolicyCommands::Enable { id } => {
            set_policy_enabled(&mut set, &id, true)?;
            save_bumped(store, set)?;
            println!("Enabled shell policy '{id}'.");
        }
        RemoteShellPolicyCommands::Disable { id } => {
            set_policy_enabled(&mut set, &id, false)?;
            save_bumped(store, set)?;
            println!("Disabled shell policy '{id}'.");
        }
    }
    Ok(())
}

fn print_shell_summary(set: &RemoteShellSet) {
    println!("Shell Access");
    println!("  Version: {}", set.version);
    println!("  Policies: {}", set.policies.len());
    for policy in &set.policies {
        let mode = policy
            .metadata
            .get("exec_mode")
            .and_then(Value::as_str)
            .unwrap_or("-");
        let status = if policy.enabled {
            "enabled"
        } else {
            "disabled"
        };
        println!(
            "    - {} ({}, {}, profile: {})",
            policy.id,
            policy.name,
            status,
            policy.profile_id.as_deref().unwrap_or("-")
        );
        println!("      mode: {mode}");
    }
    println!("  Profiles: {}", set.profiles.len());
    for profile in &set.profiles {
        let status = if profile.enabled {
            "enabled"
        } else {
            "disabled"
        };
        println!("    - {} ({}, {})", profile.id, profile.name, status);
    }
}

fn prepare_for_save(
    current: RemoteShellSet,
    mut requested: RemoteShellSet,
) -> Result<RemoteShellSet> {
    validate_set(&requested)?;
    let mut lhs = requested.clone();
    let mut rhs = current.clone();
    lhs.version = 0;
    rhs.version = 0;
    requested.version = if lhs == rhs {
        current.version.max(requested.version)
    } else if requested.version <= current.version {
        current.version.saturating_add(1)
    } else {
        requested.version
    };
    if requested.schema_version == 0 {
        requested.schema_version = current.schema_version.max(1);
    }
    Ok(requested)
}

fn save_bumped(store: &RemoteShellStore, mut set: RemoteShellSet) -> Result<()> {
    validate_set(&set)?;
    set.version = set.version.saturating_add(1);
    if set.schema_version == 0 {
        set.schema_version = 1;
    }
    store.save(&set)
}

fn validate_set(set: &RemoteShellSet) -> Result<()> {
    let profile_ids = set
        .profiles
        .iter()
        .map(|profile| profile.id.as_str())
        .collect::<HashSet<_>>();
    for policy in &set.policies {
        if let Some(profile_id) = policy.profile_id.as_deref() {
            if !profile_id.is_empty() && !profile_ids.contains(profile_id) {
                return Err(BifrostError::Config(format!(
                    "shell policy '{}' references missing profile '{}'",
                    policy.id, profile_id
                )));
            }
        }
    }
    Ok(())
}

fn reject_duplicate_profile(set: &RemoteShellSet, id: &str) -> Result<()> {
    if set.profiles.iter().any(|profile| profile.id == id) {
        return Err(BifrostError::Config(format!(
            "shell profile '{}' already exists",
            id
        )));
    }
    Ok(())
}

fn reject_duplicate_policy(set: &RemoteShellSet, id: &str) -> Result<()> {
    if set.policies.iter().any(|policy| policy.id == id) {
        return Err(BifrostError::Config(format!(
            "shell policy '{}' already exists",
            id
        )));
    }
    Ok(())
}

fn ensure_profile_exists(set: &RemoteShellSet, id: &str) -> Result<()> {
    if !set.profiles.iter().any(|profile| profile.id == id) {
        return Err(BifrostError::Config(format!(
            "shell profile '{}' does not exist",
            id
        )));
    }
    Ok(())
}

fn set_profile_enabled(set: &mut RemoteShellSet, id: &str, enabled: bool) -> Result<()> {
    let profile = set
        .profiles
        .iter_mut()
        .find(|profile| profile.id == id)
        .ok_or_else(|| BifrostError::NotFound(format!("shell profile '{}' not found", id)))?;
    profile.enabled = enabled;
    Ok(())
}

fn set_policy_enabled(set: &mut RemoteShellSet, id: &str, enabled: bool) -> Result<()> {
    let policy = set
        .policies
        .iter_mut()
        .find(|policy| policy.id == id)
        .ok_or_else(|| BifrostError::NotFound(format!("shell policy '{}' not found", id)))?;
    policy.enabled = enabled;
    Ok(())
}

fn config_error(error: serde_json::Error) -> BifrostError {
    BifrostError::Config(format!("failed to serialize shell config: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn empty_set() -> RemoteShellSet {
        RemoteShellSet {
            schema_version: 1,
            version: 0,
            policies: Vec::new(),
            profiles: Vec::new(),
        }
    }

    #[test]
    fn prepare_for_save_same_content_uses_max_version() {
        let mut current = empty_set();
        current.version = 5;
        let mut requested = current.clone();
        requested.version = 3;

        let result = prepare_for_save(current.clone(), requested).unwrap();
        assert_eq!(result.version, 5);

        let mut requested_forward = current.clone();
        requested_forward.version = 7;
        let result2 = prepare_for_save(current, requested_forward).unwrap();
        assert_eq!(result2.version, 7);
    }

    #[test]
    fn prepare_for_save_bumps_version_when_content_diff_and_not_ahead() {
        let current = empty_set();
        let requested = RemoteShellSet {
            schema_version: 0,
            version: 0,
            policies: vec![RemoteShellPolicy {
                id: "p1".to_string(),
                name: "Policy 1".to_string(),
                description: None,
                enabled: true,
                profile_id: None,
                metadata: json!({"exec_mode": "argv_exec"}),
            }],
            profiles: Vec::new(),
        };

        let result = prepare_for_save(current, requested).unwrap();
        assert_eq!(result.version, 1);
        assert_eq!(result.schema_version, 1);
    }

    #[test]
    fn prepare_for_save_preserves_requested_version_when_ahead() {
        let current = empty_set();
        let requested = RemoteShellSet {
            schema_version: 2,
            version: 10,
            policies: vec![RemoteShellPolicy {
                id: "p1".to_string(),
                name: "Policy 1".to_string(),
                description: None,
                enabled: true,
                profile_id: None,
                metadata: json!({"exec_mode": "argv_exec"}),
            }],
            profiles: Vec::new(),
        };

        let result = prepare_for_save(current, requested.clone()).unwrap();
        assert_eq!(result.version, 10);
        assert_eq!(result.schema_version, 2);
        assert_eq!(result.policies.len(), requested.policies.len());
    }

    #[test]
    fn validate_set_errors_on_missing_profile_reference() {
        let set = RemoteShellSet {
            schema_version: 1,
            version: 0,
            profiles: Vec::new(),
            policies: vec![RemoteShellPolicy {
                id: "p-missing".to_string(),
                name: "Missing profile".to_string(),
                description: None,
                enabled: true,
                profile_id: Some("missing".to_string()),
                metadata: json!({}),
            }],
        };

        let err = validate_set(&set).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("shell policy 'p-missing' references missing profile 'missing'"));
    }

    #[test]
    fn reject_duplicate_profile_detects_conflict() {
        let set = RemoteShellSet {
            schema_version: 1,
            version: 0,
            policies: Vec::new(),
            profiles: vec![RemoteShellProfile {
                id: "default".to_string(),
                name: "Default".to_string(),
                description: None,
                enabled: true,
                metadata: json!({}),
            }],
        };

        let err = reject_duplicate_profile(&set, "default").unwrap_err();
        assert!(err
            .to_string()
            .contains("shell profile 'default' already exists"));
    }

    #[test]
    fn reject_duplicate_policy_detects_conflict() {
        let set = RemoteShellSet {
            schema_version: 1,
            version: 0,
            policies: vec![RemoteShellPolicy {
                id: "p1".to_string(),
                name: "Policy".to_string(),
                description: None,
                enabled: true,
                profile_id: None,
                metadata: json!({}),
            }],
            profiles: Vec::new(),
        };

        let err = reject_duplicate_policy(&set, "p1").unwrap_err();
        assert!(err.to_string().contains("shell policy 'p1' already exists"));
    }

    #[test]
    fn ensure_profile_exists_ok_and_error_cases() {
        let set = RemoteShellSet {
            schema_version: 1,
            version: 0,
            profiles: vec![RemoteShellProfile {
                id: "default".to_string(),
                name: "Default".to_string(),
                description: None,
                enabled: true,
                metadata: json!({}),
            }],
            policies: Vec::new(),
        };

        ensure_profile_exists(&set, "default").unwrap();
        let err = ensure_profile_exists(&set, "missing").unwrap_err();
        assert!(err
            .to_string()
            .contains("shell profile 'missing' does not exist"));
    }

    #[test]
    fn set_profile_enabled_toggles_flag() {
        let mut set = RemoteShellSet {
            schema_version: 1,
            version: 0,
            profiles: vec![RemoteShellProfile {
                id: "p1".to_string(),
                name: "Profile".to_string(),
                description: None,
                enabled: false,
                metadata: json!({}),
            }],
            policies: Vec::new(),
        };

        set_profile_enabled(&mut set, "p1", true).unwrap();
        assert!(set.profiles[0].enabled);
    }

    #[test]
    fn set_policy_enabled_toggles_flag() {
        let mut set = RemoteShellSet {
            schema_version: 1,
            version: 0,
            policies: vec![RemoteShellPolicy {
                id: "p1".to_string(),
                name: "Policy".to_string(),
                description: None,
                enabled: false,
                profile_id: None,
                metadata: json!({}),
            }],
            profiles: Vec::new(),
        };

        set_policy_enabled(&mut set, "p1", true).unwrap();
        assert!(set.policies[0].enabled);
    }

    #[test]
    fn config_error_formats_serde_error() {
        let err = serde_json::from_str::<serde_json::Value>("not-json").unwrap_err();
        let wrapped = config_error(err);
        let msg = wrapped.to_string();
        assert!(msg.contains("failed to serialize shell config"));
    }

    #[test]
    fn save_bumped_increments_version_and_schema() {
        let temp_dir = TempDir::new().unwrap();
        let store = RemoteShellStore::with_file(temp_dir.path().join("remote_shell.json")).unwrap();
        let set = RemoteShellSet {
            schema_version: 0,
            version: 0,
            policies: Vec::new(),
            profiles: Vec::new(),
        };

        save_bumped(&store, set).unwrap();
        let saved = store.load().unwrap();
        assert_eq!(saved.version, 1);
        assert_eq!(saved.schema_version, 1);
    }

    #[test]
    fn handle_profile_command_add_and_delete_round_trip() {
        let temp_dir = TempDir::new().unwrap();
        let store = RemoteShellStore::with_file(temp_dir.path().join("remote_shell.json")).unwrap();

        handle_profile_command(
            &store,
            RemoteShellProfileCommands::Add {
                id: "default".to_string(),
                name: "Default".to_string(),
                description: Some("desc".to_string()),
                cwd: vec!["/tmp".to_string()],
                env: vec!["PATH".to_string()],
                default_cwd: Some("/tmp".to_string()),
                timeout_ms: Some(30_000),
                stdin: true,
                interactive: true,
                inherit_env: true,
                disabled: false,
            },
        )
        .unwrap();

        let set = store.load().unwrap();
        assert_eq!(set.profiles.len(), 1);
        let profile = &set.profiles[0];
        assert_eq!(profile.id, "default");
        assert!(profile.enabled);
        assert_eq!(profile.metadata["cwd_allowlist"], json!(["/tmp"]));

        handle_profile_command(
            &store,
            RemoteShellProfileCommands::Delete {
                id: "default".to_string(),
            },
        )
        .unwrap();

        let set_after = store.load().unwrap();
        assert!(set_after.profiles.is_empty());
    }

    #[test]
    fn handle_policy_command_add_creates_policy() {
        let temp_dir = TempDir::new().unwrap();
        let store = RemoteShellStore::with_file(temp_dir.path().join("remote_shell.json")).unwrap();

        let mut set = empty_set();
        set.profiles.push(RemoteShellProfile {
            id: "default".to_string(),
            name: "Default".to_string(),
            description: None,
            enabled: true,
            metadata: json!({}),
        });
        store.save(&set).unwrap();

        let args = RemoteShellPolicyAddArgs {
            id: "allow-bifrost".to_string(),
            name: "Allow Bifrost".to_string(),
            description: Some("desc".to_string()),
            mode: "argv_exec".to_string(),
            profile: Some("default".to_string()),
            program: vec!["/usr/bin/bifrost".to_string()],
            pattern: vec![],
            cwd: vec!["/tmp".to_string()],
            env: vec!["PATH".to_string()],
            default_cwd: Some("/tmp".to_string()),
            timeout_ms: Some(10_000),
            shell: Some("/bin/bash".to_string()),
            stdin: true,
            interactive: true,
            inherit_env: true,
            disabled: false,
        };

        handle_policy_command(&store, RemoteShellPolicyCommands::Add(Box::new(args))).unwrap();

        let set_after = store.load().unwrap();
        assert_eq!(set_after.policies.len(), 1);
        let policy = &set_after.policies[0];
        assert_eq!(policy.id, "allow-bifrost");
        assert!(policy.enabled);
        assert_eq!(policy.profile_id.as_deref(), Some("default"));
        assert_eq!(policy.metadata["exec_mode"], json!("argv_exec"));
    }

    #[test]
    fn print_shell_summary_handles_empty_set() {
        let set = RemoteShellSet::default();
        print_shell_summary(&set);
    }
}
