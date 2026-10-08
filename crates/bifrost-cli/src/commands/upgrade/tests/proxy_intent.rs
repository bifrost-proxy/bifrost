use super::*;

#[test]
fn saved_disable_overrides_stale_runtime_enable_during_upgrade() {
    let runtime = RuntimeInfo::new(123, 18891, None, None, RuntimeStartMode::Daemon)
        .with_system_proxy(true, "stale.example")
        .with_system_proxy_config_revision(4);
    let configured = bifrost_storage::NewSystemProxyConfig {
        enabled: false,
        intent_revision: 5,
        ..Default::default()
    };
    let intent = resolved_restart_system_proxy_config(Some(&runtime), &configured);
    assert_eq!(intent.intent_revision, configured.intent_revision);
    let args = build_restart_args(RestartArgsSource::Runtime(&runtime), Some(&intent));
    let mut command = Command::new("bifrost");
    set_restart_intent_revision(&mut command, Some(intent.intent_revision));
    assert_eq!(
        command
            .get_envs()
            .find(|(key, _)| *key == crate::process::SYSTEM_PROXY_INTENT_REVISION_ENV)
            .unwrap()
            .1
            .unwrap(),
        configured.intent_revision.to_string().as_str()
    );
    assert!(args.iter().any(|arg| arg == "--no-system-proxy"));
    assert!(!args.iter().any(|arg| arg == "--system-proxy"));
}

#[test]
fn latest_enable_survives_suspension_and_old_disabled_startup() {
    let runtime = RuntimeInfo::new(123, 18891, None, None, RuntimeStartMode::Daemon)
        .with_system_proxy(false, "stale.example")
        .with_system_proxy_config_revision(4);
    let configured = bifrost_storage::NewSystemProxyConfig {
        enabled: true,
        bypass: "latest.example".to_string(),
        intent_revision: 5,
        ..Default::default()
    };
    let intent = resolved_restart_system_proxy_config(Some(&runtime), &configured);
    assert_eq!(intent.intent_revision, configured.intent_revision);
    let args = build_restart_args(RestartArgsSource::Runtime(&runtime), Some(&intent));
    let mut command = Command::new("bifrost");
    set_restart_intent_revision(&mut command, Some(intent.intent_revision));
    assert_eq!(
        command
            .get_envs()
            .find(|(key, _)| *key == crate::process::SYSTEM_PROXY_INTENT_REVISION_ENV)
            .unwrap()
            .1
            .unwrap(),
        configured.intent_revision.to_string().as_str()
    );
    assert!(args.iter().any(|arg| arg == "--system-proxy"));
    assert!(!args.iter().any(|arg| arg == "--no-system-proxy"));
    assert_eq!(args.last().unwrap(), "latest.example");
}

#[test]
fn explicit_session_disable_survives_upgrade_until_new_user_toggle() {
    let runtime = RuntimeInfo::new(123, 18891, None, None, RuntimeStartMode::Daemon)
        .with_system_proxy(false, "session.example")
        .with_system_proxy_config_revision(4);
    let configured = bifrost_storage::NewSystemProxyConfig {
        enabled: true,
        intent_revision: 4,
        ..Default::default()
    };
    let intent = resolved_restart_system_proxy_config(Some(&runtime), &configured);
    assert_eq!(intent.intent_revision, configured.intent_revision);
    let args = build_restart_args(RestartArgsSource::Runtime(&runtime), Some(&intent));
    let mut command = Command::new("bifrost");
    set_restart_intent_revision(&mut command, Some(intent.intent_revision));
    assert_eq!(
        command
            .get_envs()
            .find(|(key, _)| *key == crate::process::SYSTEM_PROXY_INTENT_REVISION_ENV)
            .unwrap()
            .1
            .unwrap(),
        configured.intent_revision.to_string().as_str()
    );
    assert!(args.iter().any(|arg| arg == "--no-system-proxy"));
    assert!(!args.iter().any(|arg| arg == "--system-proxy"));
}

#[test]
fn explicitly_empty_bypass_survives_restart() {
    let args = build_restart_args(
        RestartArgsSource::DefaultConfig,
        Some(&RestartSystemProxyConfig {
            intent_revision: 0,
            enabled: true,
            bypass: String::new(),
        }),
    );
    assert_eq!(&args[args.len() - 2..], &["--proxy-bypass", ""]);
}

#[test]
fn automatic_restart_carries_exact_revision_and_clears_unknown_inherited_revision() {
    let mut command = Command::new("bifrost");
    set_restart_intent_revision(&mut command, Some(17));
    let value = command
        .get_envs()
        .find(|(key, _)| *key == crate::process::SYSTEM_PROXY_INTENT_REVISION_ENV)
        .unwrap()
        .1;
    assert_eq!(value.unwrap(), "17");
    set_restart_intent_revision(&mut command, None);
    let value = command
        .get_envs()
        .find(|(key, _)| *key == crate::process::SYSTEM_PROXY_INTENT_REVISION_ENV)
        .unwrap()
        .1;
    assert!(value.is_none());
}
