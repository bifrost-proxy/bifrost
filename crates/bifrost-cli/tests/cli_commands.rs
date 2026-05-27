use std::fs;
use std::process::Command;

use bifrost_cli::cli::{
    AiAsrCommands, AiAsrTaskCommands, AiAsrTaskDailyCommands, AiCommands, AiVoiceCommands,
    AiVoiceWakeBindingCommands, AiVoiceWakeCommands, AiVoiceWakeListenerCommands, Cli, Commands,
    RemoteCommands, SettingCommands, SyncCommands,
};
use clap::Parser;

fn bifrost_cmd() -> Command {
    Command::new(env!("CARGO_BIN_EXE_bifrost"))
}

fn run_help(args: &[&str]) -> String {
    let mut cmd = bifrost_cmd();
    for a in args {
        cmd.arg(a);
    }
    cmd.arg("--help");
    let output = cmd.output().expect("failed to run bifrost");
    String::from_utf8_lossy(&output.stdout).to_string()
        + String::from_utf8_lossy(&output.stderr).as_ref()
}

#[test]
fn metrics_subcommands_parse() {
    let help = run_help(&["metrics"]);
    assert!(
        help.contains("summary"),
        "metrics help should contain summary"
    );
    assert!(help.contains("apps"), "metrics help should contain apps");
    assert!(help.contains("hosts"), "metrics help should contain hosts");
    assert!(
        help.contains("history"),
        "metrics help should contain history"
    );
}

#[test]
fn sync_subcommands_parse() {
    let help = run_help(&["sync"]);
    assert!(help.contains("status"), "sync help should contain status");
    assert!(help.contains("login"), "sync help should contain login");
    assert!(help.contains("logout"), "sync help should contain logout");
    assert!(help.contains("run"), "sync help should contain run");
    assert!(help.contains("config"), "sync help should contain config");
}

#[test]
fn ai_asr_commands_parse() {
    let help = run_help(&["ai", "asr"]);
    if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        assert!(help.contains("start"), "ai asr help should contain start");
        assert!(help.contains("stop"), "ai asr help should contain stop");
        assert!(
            help.contains("stream-file"),
            "ai asr help should contain stream-file"
        );
        assert!(help.contains("task"), "ai asr help should contain task");
    } else {
        assert!(
            !help.contains("stream-file"),
            "ai asr help should hide local ASR commands on unsupported platforms"
        );
    }

    let cli = Cli::try_parse_from([
        "bifrost",
        "ai",
        "asr",
        "stream-file",
        "/tmp/input.wav",
        "--language",
        "chinese",
    ])
    .expect("ai asr stream-file should parse");

    match cli.command.expect("command should exist") {
        Commands::Ai {
            action: AiCommands::Asr { action },
        } => match action {
            AiAsrCommands::StreamFile {
                audio, language, ..
            } => {
                assert_eq!(audio, std::path::PathBuf::from("/tmp/input.wav"));
                assert_eq!(language, "chinese");
            }
            _ => panic!("unexpected ai asr action"),
        },
        _ => panic!("unexpected command"),
    }

    let cli = Cli::try_parse_from([
        "bifrost",
        "ai",
        "asr",
        "task",
        "daily",
        "show",
        "task-123",
        "2026-05-17",
        "--output",
        "/tmp/asr-day.md",
    ])
    .expect("ai asr task daily show should parse");

    match cli.command.expect("command should exist") {
        Commands::Ai {
            action: AiCommands::Asr { action },
        } => match action {
            AiAsrCommands::Task {
                action:
                    AiAsrTaskCommands::Daily {
                        action:
                            AiAsrTaskDailyCommands::Show {
                                first,
                                second,
                                task,
                                output,
                                json,
                            },
                    },
            } => {
                assert_eq!(first, "task-123");
                assert_eq!(second.as_deref(), Some("2026-05-17"));
                assert!(task.is_none());
                assert_eq!(output, Some(std::path::PathBuf::from("/tmp/asr-day.md")));
                assert!(!json);
            }
            _ => panic!("unexpected ai asr task action"),
        },
        _ => panic!("unexpected command"),
    }
}

#[test]
fn ai_voice_wake_commands_parse() {
    let help = run_help(&["ai", "voice", "wake"]);
    assert!(
        help.contains("status"),
        "voice wake help should contain status"
    );
    assert!(
        help.contains("profile"),
        "voice wake help should contain profile"
    );
    assert!(
        help.contains("binding"),
        "voice wake help should contain binding"
    );
    assert!(
        help.contains("listener"),
        "voice wake help should contain listener"
    );
    assert!(
        help.contains("bind-audio"),
        "voice wake help should contain bind-audio"
    );
    assert!(
        help.contains("trigger"),
        "voice wake help should contain trigger"
    );

    let cli = Cli::try_parse_from([
        "bifrost",
        "ai",
        "voice",
        "wake",
        "binding",
        "add",
        "--phrase",
        "打开录音",
        "--profile",
        "wake_profile_eden",
        "--key",
        "space",
        "--modifiers",
        "cmd,shift",
    ])
    .expect("voice wake binding add should parse");

    match cli.command.expect("command should exist") {
        Commands::Ai {
            action: AiCommands::Voice { action },
        } => match action {
            AiVoiceCommands::Wake {
                action:
                    AiVoiceWakeCommands::Binding {
                        action:
                            AiVoiceWakeBindingCommands::Add {
                                phrase,
                                profile,
                                key,
                                modifiers,
                                ..
                            },
                    },
            } => {
                assert_eq!(phrase, "打开录音");
                assert_eq!(profile, "wake_profile_eden");
                assert_eq!(key, "space");
                assert_eq!(modifiers, vec!["cmd".to_string(), "shift".to_string()]);
            }
            _ => panic!("unexpected ai voice wake action"),
        },
        _ => panic!("unexpected command"),
    }

    let cli = Cli::try_parse_from([
        "bifrost",
        "ai",
        "voice",
        "wake",
        "bind-audio",
        "/tmp/wake.wav",
        "--voiceprint-profile-id",
        "speaker_eden",
        "--phrase",
        "打开录音",
        "--key",
        "space",
        "--modifiers",
        "cmd,shift",
        "--profile-id",
        "wake_profile_eden",
        "--binding-id",
        "wake_binding_eden",
    ])
    .expect("voice wake bind-audio should parse");

    match cli.command.expect("command should exist") {
        Commands::Ai {
            action: AiCommands::Voice { action },
        } => match action {
            AiVoiceCommands::Wake {
                action:
                    AiVoiceWakeCommands::BindAudio {
                        audio,
                        voiceprint_profile_id,
                        phrase,
                        key,
                        modifiers,
                        profile_id,
                        binding_id,
                        ..
                    },
            } => {
                assert_eq!(audio, std::path::PathBuf::from("/tmp/wake.wav"));
                assert_eq!(voiceprint_profile_id, "speaker_eden");
                assert_eq!(phrase.as_deref(), Some("打开录音"));
                assert_eq!(key.as_deref(), Some("space"));
                assert_eq!(modifiers, vec!["cmd".to_string(), "shift".to_string()]);
                assert_eq!(profile_id.as_deref(), Some("wake_profile_eden"));
                assert_eq!(binding_id.as_deref(), Some("wake_binding_eden"));
            }
            _ => panic!("unexpected ai voice wake bind-audio action"),
        },
        _ => panic!("unexpected command"),
    }

    let cli = Cli::try_parse_from([
        "bifrost",
        "ai",
        "voice",
        "wake",
        "listener",
        "start",
        "--device",
        ":2",
        "--dry-run",
        "--chunk-ms",
        "2500",
        "--json",
    ])
    .expect("voice wake listener start should parse");

    match cli.command.expect("command should exist") {
        Commands::Ai {
            action: AiCommands::Voice { action },
        } => match action {
            AiVoiceCommands::Wake {
                action: AiVoiceWakeCommands::Listener { action },
            } => match action {
                AiVoiceWakeListenerCommands::Start {
                    source,
                    device,
                    chunk_ms,
                    dry_run,
                    json,
                    ..
                } => {
                    assert_eq!(source, "mic");
                    assert_eq!(device.as_deref(), Some(":2"));
                    assert_eq!(chunk_ms, Some(2500));
                    assert!(dry_run);
                    assert!(json);
                }
                _ => panic!("unexpected voice wake listener action"),
            },
            _ => panic!("unexpected ai voice wake listener action"),
        },
        _ => panic!("unexpected command"),
    }
}

#[test]
fn sync_config_options_parse() {
    let help = run_help(&["sync", "config"]);
    assert!(
        help.contains("--enabled"),
        "sync config should have --enabled"
    );
    assert!(
        help.contains("--auto-sync"),
        "sync config should have --auto-sync"
    );
    assert!(
        help.contains("--remote-url"),
        "sync config should have --remote-url"
    );
}

#[test]
fn sync_login_direct_options_parse() {
    let help = run_help(&["sync", "login"]);
    assert!(help.contains("--token"), "sync login should have --token");
    assert!(help.contains("--url"), "sync login should have --url");
    assert!(
        help.contains("https://bifrost.bytedance.net/v4/sso/token-login"),
        "sync login help should explain where to get a token"
    );

    let cli = Cli::try_parse_from(["bifrost", "sync", "login", "--token", "ci-token"])
        .expect("sync login token-only options should parse");

    match cli.command {
        Some(Commands::Sync {
            action:
                SyncCommands::Login {
                    token: Some(token),
                    url: None,
                },
        }) => {
            assert_eq!(token, "ci-token");
        }
        _ => panic!("expected sync login token-only command"),
    }

    let cli = Cli::try_parse_from([
        "bifrost",
        "sync",
        "login",
        "--token",
        "ci-token",
        "--url",
        "https://bifrost.bytedance.net",
    ])
    .expect("sync login direct options should parse");

    match cli.command {
        Some(Commands::Sync {
            action:
                SyncCommands::Login {
                    token: Some(token),
                    url: Some(url),
                },
        }) => {
            assert_eq!(token, "ci-token");
            assert_eq!(url, "https://bifrost.bytedance.net");
        }
        _ => panic!("expected sync login command"),
    }
}

#[test]
fn remote_connect_help_explains_one_time_pairing() {
    let help = run_help(&["remote", "conn", "up"]);
    assert!(
        help.contains("one-time"),
        "remote conn up help should mention one-time pair code usage"
    );
    assert!(
        help.contains("remote conn status"),
        "remote conn up help should direct users to remote conn status after pairing"
    );
    assert!(
        help.contains("--ssh-key"),
        "remote conn up help should document --ssh-key"
    );
}

#[test]
fn remote_status_help_explains_reusing_existing_authorization() {
    let help = run_help(&["remote", "conn", "status"]);
    assert!(
        help.contains("saved authorization") || help.contains("reusable authorization"),
        "remote conn status help should explain that existing authorization can be reused"
    );
    assert!(
        help.contains("remote conn up"),
        "remote conn status help should reference remote conn up as the initial pairing step"
    );
}

#[test]
fn remote_command_exec_help_lists_shell_exec_options() {
    let help = run_help(&["remote", "exec"]);
    assert!(
        help.contains("--cwd"),
        "remote exec help should contain --cwd"
    );
    assert!(
        help.contains("--env"),
        "remote exec help should contain --env"
    );
    assert!(
        help.contains("--timeout-ms"),
        "remote exec help should contain --timeout-ms"
    );
    assert!(
        help.contains("--shell-text"),
        "remote exec help should contain --shell-text"
    );
}

#[test]
fn remote_command_exec_parses_shell_text_mode() {
    let cli = Cli::try_parse_from([
        "bifrost",
        "remote",
        "exec",
        "--cwd",
        "/srv/api",
        "--env",
        "NODE_ENV=production",
        "--env",
        "REGION=sg",
        "--timeout-ms",
        "600000",
        "--shell-text",
        "printf hello",
    ])
    .expect("remote exec --shell-text should parse");

    match cli.command.expect("command should exist") {
        Commands::Remote { action, .. } => match action {
            RemoteCommands::Exec(exec) => {
                let exec = exec.as_ref();
                assert_eq!(exec.cwd.as_deref(), Some("/srv/api"));
                assert_eq!(exec.timeout_ms, Some(600000));
                assert_eq!(exec.shell_text.as_deref(), Some("printf hello"));
                assert!(exec.argv.is_empty());
                assert_eq!(
                    exec.env,
                    vec![
                        ("NODE_ENV".to_string(), "production".to_string()),
                        ("REGION".to_string(), "sg".to_string())
                    ]
                );
            }
            _ => panic!("unexpected remote action"),
        },
        _ => panic!("unexpected command"),
    }
}

#[test]
fn remote_command_exec_parses_argv_after_double_dash() {
    let cli = Cli::try_parse_from([
        "bifrost",
        "remote",
        "exec",
        "--timeout-ms",
        "5000",
        "--",
        "/bin/echo",
        "hello",
        "--flag",
    ])
    .expect("remote exec argv mode should parse");

    match cli.command.expect("command should exist") {
        Commands::Remote { action, .. } => match action {
            RemoteCommands::Exec(exec) => {
                let exec = exec.as_ref();
                assert_eq!(exec.timeout_ms, Some(5000));
                assert_eq!(exec.shell_text, None);
                assert_eq!(
                    exec.argv,
                    vec![
                        "/bin/echo".to_string(),
                        "hello".to_string(),
                        "--flag".to_string()
                    ]
                );
            }
            _ => panic!("unexpected remote action"),
        },
        _ => panic!("unexpected command"),
    }
}

#[test]
fn remote_command_exec_rejects_argv_without_double_dash() {
    let err = match Cli::try_parse_from(["bifrost", "remote", "exec", "pwd"]) {
        Ok(_) => panic!("bare argv without `--` should be rejected"),
        Err(err) => err,
    };
    let rendered = err.to_string();

    assert!(
        rendered.contains("unexpected argument 'pwd'"),
        "expected clap to reject bare argv, got: {rendered}"
    );
    assert!(
        rendered.contains("--shell-text"),
        "error should still point users at the explicit shell_text flag: {rendered}"
    );
}

#[test]
fn import_command_parse() {
    let help = run_help(&["import"]);
    assert!(help.contains("file"), "import should require file arg");
    assert!(
        help.contains("detect-only"),
        "import should have --detect-only"
    );
}

#[test]
fn export_subcommands_parse() {
    let help = run_help(&["export"]);
    assert!(
        help.contains("rules"),
        "export should have rules subcommand"
    );
    assert!(
        help.contains("values"),
        "export should have values subcommand"
    );
    assert!(
        help.contains("scripts"),
        "export should have scripts subcommand"
    );
}

#[test]
fn export_rules_options_parse() {
    let help = run_help(&["export", "rules"]);
    assert!(
        help.contains("--description"),
        "export rules should have --description"
    );
    assert!(
        help.contains("--output"),
        "export rules should have --output"
    );
}

#[test]
fn traffic_clear_command_parse() {
    let help = run_help(&["traffic", "clear"]);
    assert!(help.contains("--ids"), "traffic clear should have --ids");
    assert!(help.contains("--yes"), "traffic clear should have --yes");
}

#[test]
fn remote_traffic_clear_is_not_exposed() {
    let help = run_help(&["remote", "traffic"]);
    assert!(help.contains("list"), "remote traffic should list records");
    assert!(help.contains("get"), "remote traffic should get records");
    assert!(
        help.contains("search"),
        "remote traffic should search records"
    );
    assert!(
        !help.contains("clear"),
        "remote traffic clear is a mutating operation and must not be exposed"
    );
}

#[test]
fn rule_rename_command_parse() {
    let help = run_help(&["rule", "rename"]);
    assert!(help.contains("NAME"), "rule rename should have name arg");
    assert!(
        help.contains("NEW_NAME"),
        "rule rename should have new_name arg"
    );
}

#[test]
fn rule_reorder_command_parse() {
    let help = run_help(&["rule", "reorder"]);
    assert!(
        help.contains("names") || help.contains("NAMES"),
        "rule reorder should have names arg"
    );
}

#[test]
fn script_rename_command_parse() {
    let help = run_help(&["script", "rename"]);
    assert!(
        help.contains("TYPE") || help.contains("type"),
        "script rename should have type arg"
    );
    assert!(
        help.contains("NEW_NAME"),
        "script rename should have new_name arg"
    );
}

#[test]
fn whitelist_mode_command_parse() {
    let help = run_help(&["whitelist", "mode"]);
    assert!(
        help.contains("local_only") || help.contains("access mode"),
        "whitelist mode help should mention modes"
    );
}

#[test]
fn whitelist_pending_command_parse() {
    let help = run_help(&["whitelist"]);
    assert!(
        help.contains("pending"),
        "whitelist should have pending subcommand"
    );
    assert!(
        help.contains("approve"),
        "whitelist should have approve subcommand"
    );
    assert!(
        help.contains("reject"),
        "whitelist should have reject subcommand"
    );
    assert!(
        help.contains("clear-pending"),
        "whitelist should have clear-pending"
    );
    assert!(
        help.contains("add-temporary"),
        "whitelist should have add-temporary"
    );
    assert!(
        help.contains("remove-temporary"),
        "whitelist should have remove-temporary"
    );
}

#[test]
fn config_performance_command_parse() {
    let help = run_help(&["config"]);
    assert!(
        help.contains("performance"),
        "config should have performance subcommand"
    );
    assert!(
        help.contains("websocket"),
        "config should have websocket subcommand"
    );
    assert!(
        help.contains("disconnect-by-app"),
        "config should have disconnect-by-app"
    );
}

#[test]
fn remote_connect_accepts_ssh_key_without_pair_code() {
    let cli = Cli::try_parse_from([
        "bifrost",
        "remote",
        "conn",
        "up",
        "--ssh-key",
        "/tmp/test.bifrost",
    ])
    .expect("remote conn up --ssh-key should parse");

    match cli.command.expect("command should exist") {
        Commands::Remote { action, .. } => match action {
            RemoteCommands::Conn {
                action:
                    bifrost_cli::cli::RemoteConnCommands::Up {
                        pair_code,
                        ssh_key,
                        device_code,
                        label,
                    },
            } => {
                assert_eq!(pair_code, None);
                assert_eq!(ssh_key.as_deref(), Some("/tmp/test.bifrost"));
                assert_eq!(device_code, None);
                assert_eq!(label, None);
            }
            _ => panic!("unexpected remote action"),
        },
        _ => panic!("unexpected command"),
    }
}

#[test]
fn version_check_command_parse() {
    let help = run_help(&[]);
    assert!(
        help.contains("version-check"),
        "main help should list version-check command"
    );
}

#[test]
fn metrics_command_listed_in_help() {
    let help = run_help(&[]);
    assert!(
        help.contains("metrics"),
        "main help should list metrics command"
    );
}

#[test]
fn sync_command_listed_in_help() {
    let help = run_help(&[]);
    assert!(help.contains("sync"), "main help should list sync command");
}

#[test]
fn import_export_commands_listed_in_help() {
    let help = run_help(&[]);
    assert!(
        help.contains("import"),
        "main help should list import command"
    );
    assert!(
        help.contains("export"),
        "main help should list export command"
    );
}

#[test]
fn completions_still_works() {
    let output = bifrost_cmd()
        .arg("completions")
        .arg("zsh")
        .output()
        .expect("failed to run bifrost completions");
    assert!(
        output.status.success(),
        "completions command should succeed"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("compdef"),
        "zsh completions should contain compdef"
    );
}

#[test]
fn completions_includes_new_commands() {
    let output = bifrost_cmd()
        .arg("completions")
        .arg("zsh")
        .output()
        .expect("failed to run bifrost completions");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("metrics"),
        "completions should include metrics"
    );
    assert!(stdout.contains("sync"), "completions should include sync");
    assert!(
        stdout.contains("import"),
        "completions should include import"
    );
    assert!(
        stdout.contains("export"),
        "completions should include export"
    );
    assert!(
        stdout.contains("version-check"),
        "completions should include version-check"
    );
}

#[test]
fn value_parser_completions_for_log_level() {
    let output = bifrost_cmd()
        .arg("completions")
        .arg("zsh")
        .output()
        .expect("failed");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("trace"),
        "completions should contain log level trace"
    );
    assert!(
        stdout.contains("debug"),
        "completions should contain log level debug"
    );
}

#[test]
fn value_parser_completions_for_access_mode() {
    let output = bifrost_cmd()
        .arg("completions")
        .arg("zsh")
        .output()
        .expect("failed");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("local_only"),
        "completions should contain access mode local_only"
    );
    assert!(
        stdout.contains("allow_all"),
        "completions should contain access mode allow_all"
    );
}

#[test]
fn metrics_history_limit_option() {
    let help = run_help(&["metrics", "history"]);
    assert!(
        help.contains("--limit"),
        "metrics history should have --limit option"
    );
}

#[test]
fn export_values_options_parse() {
    let help = run_help(&["export", "values"]);
    assert!(
        help.contains("--description"),
        "export values should have --description"
    );
    assert!(
        help.contains("--output"),
        "export values should have --output"
    );
}

#[test]
fn export_scripts_options_parse() {
    let help = run_help(&["export", "scripts"]);
    assert!(
        help.contains("--description"),
        "export scripts should have --description"
    );
    assert!(
        help.contains("--output"),
        "export scripts should have --output"
    );
}

#[test]
fn whitelist_approve_requires_ip() {
    let help = run_help(&["whitelist", "approve"]);
    assert!(
        help.contains("ip") || help.contains("IP"),
        "whitelist approve should require ip"
    );
}

#[test]
fn config_disconnect_by_app_requires_app() {
    let help = run_help(&["config", "disconnect-by-app"]);
    assert!(
        help.contains("app") || help.contains("APP"),
        "disconnect-by-app should require app"
    );
}

fn get_zsh_completions() -> String {
    let output = bifrost_cmd()
        .arg("completions")
        .arg("zsh")
        .output()
        .expect("failed to run bifrost completions zsh");
    assert!(output.status.success(), "completions zsh should succeed");
    String::from_utf8_lossy(&output.stdout).to_string()
}

#[test]
fn visible_alias_status_st() {
    let completions = get_zsh_completions();
    assert!(
        completions.contains("'st:"),
        "completions should include 'st' alias for status"
    );
}

#[test]
fn visible_alias_config_cfg() {
    let completions = get_zsh_completions();
    assert!(
        completions.contains("'cfg:"),
        "completions should include 'cfg' alias for config"
    );
}

#[test]
fn visible_alias_system_proxy_sp() {
    let completions = get_zsh_completions();
    assert!(
        completions.contains("'sp:"),
        "completions should include 'sp' alias for system-proxy"
    );
}

#[test]
fn visible_alias_whitelist_wl() {
    let completions = get_zsh_completions();
    assert!(
        completions.contains("'wl:"),
        "completions should include 'wl' alias for whitelist"
    );
}

#[test]
fn visible_alias_value_val() {
    let completions = get_zsh_completions();
    assert!(
        completions.contains("'val:"),
        "completions should include 'val' alias for value"
    );
}

#[test]
fn visible_alias_completions_comp() {
    let completions = get_zsh_completions();
    assert!(
        completions.contains("'comp:"),
        "completions should include 'comp' alias for completions"
    );
}

#[test]
fn completions_file_path_hints() {
    let completions = get_zsh_completions();
    assert!(
        completions.contains("_files"),
        "completions should contain _files for file path hints"
    );
}

#[test]
fn completions_allow_lan_true_false() {
    let completions = get_zsh_completions();
    assert!(
        completions.contains("(true false)"),
        "completions should offer true/false for allow-lan"
    );
}

#[test]
fn completions_script_type_values() {
    let completions = get_zsh_completions();
    assert!(
        completions.contains("request")
            && completions.contains("response")
            && completions.contains("decode"),
        "completions should include script type values"
    );
}

#[test]
fn completions_traffic_list_method_values() {
    let completions = get_zsh_completions();
    assert!(
        completions.contains("GET")
            && completions.contains("POST")
            && completions.contains("DELETE"),
        "completions should include HTTP method values"
    );
}

#[test]
fn completions_traffic_list_protocol_values() {
    let completions = get_zsh_completions();
    assert!(
        completions.contains("https") || completions.contains("HTTPS"),
        "completions should include protocol values"
    );
}

#[test]
fn completions_search_status_values() {
    let completions = get_zsh_completions();
    assert!(
        completions.contains("2xx") && completions.contains("4xx") && completions.contains("5xx"),
        "completions should include status filter values"
    );
}

#[test]
fn completions_config_export_format_values() {
    let completions = get_zsh_completions();
    assert!(
        completions.contains("toml"),
        "completions should include config export format toml"
    );
}

#[test]
fn completions_search_format_values() {
    let completions = get_zsh_completions();
    assert!(
        completions.contains("json-pretty") && completions.contains("compact"),
        "completions should include search output format values"
    );
}

#[test]
fn completions_content_type_values() {
    let completions = get_zsh_completions();
    assert!(
        completions.contains("json") && completions.contains("xml") && completions.contains("html"),
        "completions should include content type filter values"
    );
}

#[test]
fn completions_install_skill_tool_values() {
    let completions = get_zsh_completions();
    assert!(
        completions.contains("claude-code")
            && completions.contains("codex")
            && completions.contains("cursor")
            && completions.contains("trae")
            && completions.contains("github-copilot")
            && completions.contains("universal"),
        "completions should include install-skill tool values"
    );
}

#[test]
fn completions_config_section_values() {
    let completions = get_zsh_completions();
    assert!(
        completions.contains("tls")
            && completions.contains("traffic")
            && completions.contains("access"),
        "completions should include config section values"
    );
}

#[test]
fn completions_log_output_values() {
    let completions = get_zsh_completions();
    assert!(
        completions.contains("console,file"),
        "completions should include log output combined value"
    );
}

#[test]
fn completions_whitelist_mode_values() {
    let completions = get_zsh_completions();
    assert!(
        completions.contains("interactive") && completions.contains("whitelist"),
        "completions should include whitelist mode values"
    );
}

#[test]
fn completions_traffic_direction_values() {
    let completions = get_zsh_completions();
    assert!(
        completions.contains("backward") && completions.contains("forward"),
        "completions should include traffic direction values"
    );
}

#[test]
fn bash_completions_work() {
    let output = bifrost_cmd()
        .arg("completions")
        .arg("bash")
        .output()
        .expect("failed to run bifrost completions bash");
    assert!(output.status.success(), "bash completions should succeed");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("complete") && stdout.contains("bifrost"),
        "bash completions should contain complete and bifrost"
    );
}

#[test]
fn fish_completions_work() {
    let output = bifrost_cmd()
        .arg("completions")
        .arg("fish")
        .output()
        .expect("failed to run bifrost completions fish");
    assert!(output.status.success(), "fish completions should succeed");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("complete") && stdout.contains("bifrost"),
        "fish completions should contain complete and bifrost"
    );
}

#[test]
fn alias_st_works_as_status() {
    let output = bifrost_cmd()
        .arg("st")
        .arg("--help")
        .output()
        .expect("failed to run bifrost st --help");
    let combined = String::from_utf8_lossy(&output.stdout).to_string()
        + String::from_utf8_lossy(&output.stderr).as_ref();
    assert!(
        combined.contains("proxy") || combined.contains("status") || combined.contains("TUI"),
        "st alias should work as status command"
    );
}

#[test]
fn alias_cfg_works_as_config() {
    let output = bifrost_cmd()
        .arg("cfg")
        .arg("--help")
        .output()
        .expect("failed to run bifrost cfg --help");
    let combined = String::from_utf8_lossy(&output.stdout).to_string()
        + String::from_utf8_lossy(&output.stderr).as_ref();
    assert!(
        combined.contains("show") && combined.contains("set") && combined.contains("get"),
        "cfg alias should work as config command"
    );
}

#[test]
fn alias_sp_works_as_system_proxy() {
    let output = bifrost_cmd()
        .arg("sp")
        .arg("--help")
        .output()
        .expect("failed to run bifrost sp --help");
    let combined = String::from_utf8_lossy(&output.stdout).to_string()
        + String::from_utf8_lossy(&output.stderr).as_ref();
    assert!(
        combined.contains("enable") && combined.contains("disable"),
        "sp alias should work as system-proxy command"
    );
}

#[test]
fn alias_val_works_as_value() {
    let output = bifrost_cmd()
        .arg("val")
        .arg("--help")
        .output()
        .expect("failed to run bifrost val --help");
    let combined = String::from_utf8_lossy(&output.stdout).to_string()
        + String::from_utf8_lossy(&output.stderr).as_ref();
    assert!(
        combined.contains("list") && combined.contains("add"),
        "val alias should work as value command"
    );
}

#[test]
fn alias_wl_works_as_whitelist() {
    let output = bifrost_cmd()
        .arg("wl")
        .arg("--help")
        .output()
        .expect("failed to run bifrost wl --help");
    let combined = String::from_utf8_lossy(&output.stdout).to_string()
        + String::from_utf8_lossy(&output.stderr).as_ref();
    assert!(
        combined.contains("list") && combined.contains("add") && combined.contains("remove"),
        "wl alias should work as whitelist command"
    );
}

#[test]
fn alias_comp_works_as_completions() {
    let output = bifrost_cmd()
        .arg("comp")
        .arg("zsh")
        .output()
        .expect("failed to run bifrost comp zsh");
    assert!(
        output.status.success(),
        "comp alias should work as completions command"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("compdef"),
        "comp alias should produce valid zsh completions"
    );
}

#[test]
fn install_skill_listed_in_help() {
    let help = run_help(&[]);
    assert!(
        help.contains("install-skill"),
        "main help should list install-skill command"
    );
}

#[test]
fn install_skill_options_parse() {
    let help = run_help(&["install-skill"]);
    assert!(
        help.contains("--tool"),
        "install-skill should have --tool option"
    );
    assert!(
        help.contains("--dir"),
        "install-skill should have --dir option"
    );
    assert!(
        help.contains("--cwd"),
        "install-skill should have --cwd option"
    );
    assert!(
        help.contains("--yes"),
        "install-skill should have --yes option"
    );
    assert!(
        help.contains("github-copilot"),
        "install-skill help should include github-copilot target"
    );
    assert!(
        help.contains("universal"),
        "install-skill help should include universal target"
    );
}

#[test]
fn install_skill_installs_remote_skill_from_embedded_bundle() {
    let temp = tempfile::tempdir().expect("temp dir");
    let target_dir = temp.path().join("skills").join("bifrost");

    let output = bifrost_cmd()
        .env("BIFROST_INSTALL_SKILL_SOURCE", "embedded")
        .arg("install-skill")
        .arg("--tool")
        .arg("codex")
        .arg("--dir")
        .arg(&target_dir)
        .arg("-y")
        .output()
        .expect("failed to run install-skill");

    assert!(
        output.status.success(),
        "install-skill should succeed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let primary_skill = target_dir.join("SKILL.md");
    let remote_skill = target_dir
        .parent()
        .expect("target dir parent")
        .join("bifrost-remote")
        .join("SKILL.md");

    assert!(
        primary_skill.exists(),
        "main bifrost skill should be installed"
    );
    assert!(
        remote_skill.exists(),
        "bifrost-remote skill should be installed as sibling"
    );

    let remote_content = fs::read_to_string(remote_skill).expect("remote skill content");
    assert!(remote_content.contains("name: \"bifrost-remote\""));
    // Current namespace: `bifrost setting shell ...`. The doc is kept lean
    // and should describe the current command surface, not legacy aliases.
    assert!(remote_content.contains("bifrost setting shell profile add"));
    assert!(remote_content.contains("bifrost setting shell policy add"));
    assert!(!remote_content.contains("deprecated"));
    assert!(!remote_content.contains("old `bifrost remote"));
    // Access mode vocabulary must stay stable for doc consumers.
    assert!(remote_content.contains("query"));
    assert!(remote_content.contains("selected"));
    assert!(remote_content.contains("all"));
    // Bifrost remote file-API guidance (core value of the rewrite).
    assert!(remote_content.contains("bifrost remote file"));
    assert!(remote_content.contains("FileAccessPolicy"));
    // `traffic.clear` is mutating and must not be advertised by the installed
    // remote skill.
    assert!(!remote_content.contains("traffic.clear"));
    assert!(remote_content.contains("remote traffic {list,get,search}"));
}

#[test]
fn main_help_points_to_quick_start_for_shortcuts() {
    let output = bifrost_cmd()
        .arg("--help")
        .output()
        .expect("failed to run bifrost --help");
    let combined = String::from_utf8_lossy(&output.stdout).to_string()
        + String::from_utf8_lossy(&output.stderr).as_ref();
    assert!(
        !combined.contains("COMMAND SHORTCUTS"),
        "main help should keep shortcuts in docs instead of the old long section"
    );
    assert!(
        combined.contains("docs/cli-quick-start.md"),
        "main help should link to the quick start doc that lists shortcuts"
    );
}

#[test]
fn all_subcommands_have_about() {
    let help = run_help(&[]);
    for cmd in &[
        "start",
        "stop",
        "status",
        "rule",
        "group",
        "port",
        "ca",
        "whitelist",
        "system-proxy",
        "value",
        "script",
        "upgrade",
        "config",
        "traffic",
        "search",
        "completions",
        "metrics",
        "sync",
        "import",
        "export",
        "version-check",
        "install-skill",
    ] {
        assert!(
            help.contains(cmd),
            "main help should list '{}' command",
            cmd
        );
    }
}

#[test]
fn group_rule_subcommands_parse() {
    let help = run_help(&["group", "rule"]);
    for sub in &[
        "list", "show", "add", "update", "delete", "enable", "disable",
    ] {
        assert!(
            help.contains(sub),
            "group rule help should contain '{}' subcommand",
            sub
        );
    }
}

#[test]
fn ca_subcommands_parse() {
    let help = run_help(&["ca"]);
    for sub in &["install", "generate", "export", "info"] {
        assert!(
            help.contains(sub),
            "ca help should contain '{}' subcommand",
            sub
        );
    }
}

#[test]
fn rule_subcommands_complete_list() {
    let help = run_help(&["rule"]);
    for sub in &[
        "list", "active", "add", "update", "delete", "enable", "disable", "show", "rename",
        "reorder",
    ] {
        assert!(
            help.contains(sub),
            "rule help should contain '{}' subcommand",
            sub
        );
    }
}

#[test]
fn script_subcommands_complete_list() {
    let help = run_help(&["script"]);
    for sub in &["list", "add", "update", "delete", "show", "run", "rename"] {
        assert!(
            help.contains(sub),
            "script help should contain '{}' subcommand",
            sub
        );
    }
}

#[test]
fn value_subcommands_complete_list() {
    let help = run_help(&["value"]);
    for sub in &["list", "show", "add", "update", "delete", "import"] {
        assert!(
            help.contains(sub),
            "value help should contain '{}' subcommand",
            sub
        );
    }
}

#[test]
fn config_subcommands_complete_list() {
    let help = run_help(&["config"]);
    for sub in &[
        "show",
        "get",
        "set",
        "add",
        "remove",
        "reset",
        "clear-cache",
        "disconnect",
        "export",
        "disconnect-by-app",
        "performance",
        "websocket",
        "connections",
        "memory",
    ] {
        assert!(
            help.contains(sub),
            "config help should contain '{}' subcommand",
            sub
        );
    }
}

#[test]
fn config_connections_command_parse() {
    let help = run_help(&["config", "connections"]);
    assert!(
        help.contains("active") || help.contains("connection") || help.contains("proxy"),
        "config connections help should describe active connections"
    );
}

#[test]
fn config_memory_command_parse() {
    let help = run_help(&["config", "memory"]);
    assert!(
        help.contains("memory") || help.contains("Memory") || help.contains("diagnostic"),
        "config memory help should describe memory diagnostics"
    );
}

#[test]
fn config_show_section_accepts_server() {
    let output = bifrost_cmd()
        .arg("config")
        .arg("show")
        .arg("--section")
        .arg("server")
        .arg("--help")
        .output()
        .expect("failed to run bifrost config show --section server --help");
    let combined = String::from_utf8_lossy(&output.stdout).to_string()
        + String::from_utf8_lossy(&output.stderr).as_ref();
    assert!(
        !combined.contains("error") || combined.contains("Show"),
        "config show --section server should be accepted by clap parser"
    );
}

#[test]
fn config_show_section_value_parser_includes_all() {
    let completions = get_zsh_completions();
    assert!(
        completions.contains("server"),
        "completions should include config section value 'server'"
    );
}

#[test]
fn traffic_subcommands_complete_list() {
    let help = run_help(&["traffic"]);
    for sub in &["list", "get", "search", "clear"] {
        assert!(
            help.contains(sub),
            "traffic help should contain '{}' subcommand",
            sub
        );
    }
}

#[test]
fn remote_command_exec_parses_streaming_flags() {
    let cli = Cli::try_parse_from([
        "bifrost",
        "remote",
        "exec",
        "--stream",
        "--output-file",
        "/tmp/out.bin",
        "--resume-call-id",
        "00000000-0000-4000-8000-000000000001",
        "--no-verify-digest",
        "--shell-text",
        "echo hi",
    ])
    .expect("remote exec streaming flags should parse");

    match cli.command.expect("command should exist") {
        Commands::Remote { action, .. } => match action {
            RemoteCommands::Exec(exec) => {
                let exec = exec.as_ref();
                assert!(exec.stream, "--stream must be true");
                assert_eq!(exec.output_file.as_deref(), Some("/tmp/out.bin"));
                assert_eq!(
                    exec.resume_call_id.as_deref(),
                    Some("00000000-0000-4000-8000-000000000001")
                );
                assert!(exec.no_verify_digest, "--no-verify-digest must be true");
                assert_eq!(exec.shell_text.as_deref(), Some("echo hi"));
            }
            _ => panic!("unexpected remote action"),
        },
        _ => panic!("unexpected command"),
    }
}

#[test]
fn remote_command_exec_streaming_flags_default_to_off() {
    let cli = Cli::try_parse_from(["bifrost", "remote", "exec", "--shell-text", "true"])
        .expect("minimal exec should parse");
    match cli.command.expect("command should exist") {
        Commands::Remote { action, .. } => match action {
            RemoteCommands::Exec(exec) => {
                let exec = exec.as_ref();
                assert!(!exec.stream);
                assert!(exec.output_file.is_none());
                assert!(exec.resume_call_id.is_none());
                assert!(!exec.no_verify_digest);
            }
            _ => panic!("unexpected remote action"),
        },
        _ => panic!("unexpected command"),
    }
}

#[test]
fn setting_shell_policy_add_parses_semantic_flags() {
    let cli = Cli::try_parse_from([
        "bifrost",
        "setting",
        "shell",
        "policy",
        "add",
        "--id",
        "pwd-argv",
        "--name",
        "Pwd argv",
        "--mode",
        "argv_exec",
        "--program",
        "/bin/pwd",
    ])
    .expect("setting shell policy add should parse");

    let command = cli.command.expect("command should exist");
    match command {
        Commands::Setting { action } => match action {
            SettingCommands::Shell { action } => {
                let rendered = format!("{action:?}");
                assert!(rendered.contains("pwd-argv"));
                assert!(rendered.contains("argv_exec"));
            }
            _ => panic!("unexpected setting action"),
        },
        _ => panic!("unexpected command"),
    }
}

#[test]
fn setting_grant_list_parses() {
    let cli = Cli::try_parse_from(["bifrost", "setting", "grant", "list"])
        .expect("setting grant list should parse");
    let command = cli.command.expect("command should exist");
    match command {
        Commands::Setting { action } => match action {
            SettingCommands::Grant { .. } => {}
            _ => panic!("unexpected setting action"),
        },
        _ => panic!("unexpected command"),
    }
}

#[test]
fn setting_grant_revoke_parses() {
    let cli = Cli::try_parse_from(["bifrost", "setting", "grant", "revoke", "grant-abc"])
        .expect("setting grant revoke should parse");
    let command = cli.command.expect("command should exist");
    match command {
        Commands::Setting { action } => match action {
            SettingCommands::Grant { action } => {
                let rendered = format!("{action:?}");
                assert!(rendered.contains("grant-abc"));
            }
            _ => panic!("unexpected setting action"),
        },
        _ => panic!("unexpected command"),
    }
}

// ----- restart subcommand parsing -----

#[test]
fn restart_parses_with_no_flags() {
    use bifrost_cli::cli::Commands;
    let cli = bifrost_cli::cli::Cli::try_parse_from(["bifrost", "restart"])
        .expect("bifrost restart should parse");
    match cli.command {
        Some(Commands::Restart {
            port,
            host,
            log_level,
            force,
        }) => {
            assert!(port.is_none());
            assert!(host.is_none());
            assert!(log_level.is_none());
            assert!(!force);
        }
        _ => panic!("expected Commands::Restart"),
    }
}

#[test]
fn restart_parses_with_port_host_force() {
    use bifrost_cli::cli::Commands;
    let cli = bifrost_cli::cli::Cli::try_parse_from([
        "bifrost",
        "restart",
        "--port",
        "9901",
        "--host",
        "127.0.0.1",
        "--log-level",
        "debug",
        "--force",
    ])
    .expect("bifrost restart with flags should parse");
    match cli.command {
        Some(Commands::Restart {
            port,
            host,
            log_level,
            force,
        }) => {
            assert_eq!(port, Some(9901));
            assert_eq!(host.as_deref(), Some("127.0.0.1"));
            assert_eq!(log_level.as_deref(), Some("debug"));
            assert!(force);
        }
        _ => panic!("expected Commands::Restart"),
    }
}

#[test]
fn restart_options_default_is_all_none() {
    // Smoke test for the RestartOptions struct re-exported from commands.
    let o = bifrost_cli::commands::RestartOptions::default();
    assert!(o.port.is_none());
    assert!(o.host.is_none());
    assert!(o.log_level.is_none());
    assert!(!o.force);
}
