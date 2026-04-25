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
