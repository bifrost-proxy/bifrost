use std::process::Command;

use bifrost_cli::cli::{Cli, Commands, RemoteCommandCommands, RemoteCommands};
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
fn remote_connect_help_explains_one_time_pairing() {
    let help = run_help(&["remote", "connect"]);
    assert!(
        help.contains("one-time"),
        "remote connect help should mention one-time pair code usage"
    );
    assert!(
        help.contains("remote status"),
        "remote connect help should direct users to remote status after pairing"
    );
    assert!(
        help.contains("--ssh-key"),
        "remote connect help should document --ssh-key"
    );
}

#[test]
fn remote_status_help_explains_reusing_existing_authorization() {
    let help = run_help(&["remote", "status"]);
    assert!(
        help.contains("saved authorization") || help.contains("reusable authorization"),
        "remote status help should explain that existing authorization can be reused"
    );
    assert!(
        help.contains("remote connect"),
        "remote status help should reference remote connect as the initial pairing step"
    );
}

#[test]
fn remote_command_exec_help_lists_shell_exec_options() {
    let help = run_help(&["remote", "command", "exec"]);
    assert!(
        help.contains("--cwd"),
        "remote command exec help should contain --cwd"
    );
    assert!(
        help.contains("--env"),
        "remote command exec help should contain --env"
    );
    assert!(
        help.contains("--timeout-ms"),
        "remote command exec help should contain --timeout-ms"
    );
    assert!(
        help.contains("--shell-text"),
        "remote command exec help should contain --shell-text"
    );
}

#[test]
fn remote_command_exec_parses_shell_text_mode() {
    let cli = Cli::try_parse_from([
        "bifrost",
        "remote",
        "command",
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
    .expect("remote command exec --shell-text should parse");

    match cli.command.expect("command should exist") {
        Commands::Remote { action, .. } => match action {
            RemoteCommands::Command { action } => match action {
                RemoteCommandCommands::Exec(exec) => {
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
            },
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
        "command",
        "exec",
        "--timeout-ms",
        "5000",
        "--",
        "/bin/echo",
        "hello",
        "--flag",
    ])
    .expect("remote command exec argv mode should parse");

    match cli.command.expect("command should exist") {
        Commands::Remote { action, .. } => match action {
            RemoteCommands::Command { action } => match action {
                RemoteCommandCommands::Exec(exec) => {
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
            },
            _ => panic!("unexpected remote action"),
        },
        _ => panic!("unexpected command"),
    }
}

#[test]
fn remote_command_exec_rejects_argv_without_double_dash() {
    let err = match Cli::try_parse_from(["bifrost", "remote", "command", "exec", "pwd"]) {
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
fn remote_shell_help_lists_management_subcommands() {
    let help = run_help(&["remote", "shell"]);
    assert!(
        help.contains("list"),
        "remote shell help should contain list"
    );
    assert!(
        help.contains("show"),
        "remote shell help should contain show"
    );
    assert!(
        help.contains("apply"),
        "remote shell help should contain apply"
    );
    assert!(
        help.contains("profile"),
        "remote shell help should contain profile"
    );
    assert!(
        help.contains("policy"),
        "remote shell help should contain policy"
    );
}

#[test]
fn remote_grant_help_lists_management_subcommands() {
    let help = run_help(&["remote", "grant"]);
    assert!(
        help.contains("list"),
        "remote grant help should contain list"
    );
    assert!(
        help.contains("update"),
        "remote grant help should contain update"
    );
    assert!(
        help.contains("revoke"),
        "remote grant help should contain revoke"
    );
}

#[test]
fn remote_grant_update_parses_access_and_policy_flags() {
    let cli = Cli::try_parse_from([
        "bifrost",
        "remote",
        "grant",
        "update",
        "grant-123",
        "--access",
        "selected",
        "--policy",
        "pwd-argv",
        "--policy",
        "echo-argv",
        "--stdin",
        "true",
        "--interactive",
        "false",
    ])
    .expect("remote grant update should parse");

    let command = cli.command.expect("command should exist");
    match command {
        Commands::Remote { action, .. } => match action {
            RemoteCommands::Grant { action } => {
                let rendered = format!("{action:?}");
                assert!(rendered.contains("grant-123"));
                assert!(rendered.contains("selected"));
                assert!(rendered.contains("pwd-argv"));
                assert!(rendered.contains("echo-argv"));
            }
            _ => panic!("unexpected remote action"),
        },
        _ => panic!("unexpected command"),
    }
}

#[test]
fn remote_shell_policy_add_parses_semantic_flags() {
    let cli = Cli::try_parse_from([
        "bifrost",
        "remote",
        "shell",
        "policy",
        "add",
        "--id",
        "pwd-argv",
        "--name",
        "Pwd argv",
        "--mode",
        "argv_exec",
        "--profile",
        "repo-default",
        "--program",
        "/bin/pwd",
        "--cwd",
        "/Users/eden/work/github/bifrost",
        "--timeout-ms",
        "5000",
    ])
    .expect("remote shell policy add should parse");

    let command = cli.command.expect("command should exist");
    match command {
        Commands::Remote { action, .. } => match action {
            RemoteCommands::Shell { action } => {
                let rendered = format!("{action:?}");
                assert!(rendered.contains("pwd-argv"));
                assert!(rendered.contains("argv_exec"));
                assert!(rendered.contains("/bin/pwd"));
                assert!(rendered.contains("repo-default"));
            }
            _ => panic!("unexpected remote action"),
        },
        _ => panic!("unexpected command"),
    }
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
        "connect",
        "--ssh-key",
        "/tmp/test.bifrost",
    ])
    .expect("remote connect --ssh-key should parse");

    match cli.command.expect("command should exist") {
        Commands::Remote { action, .. } => match action {
            RemoteCommands::Connect {
                pair_code,
                ssh_key,
                device_code,
            } => {
                assert_eq!(pair_code, None);
                assert_eq!(ssh_key.as_deref(), Some("/tmp/test.bifrost"));
                assert_eq!(device_code, None);
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
fn main_help_shows_shortcuts() {
    let output = bifrost_cmd()
        .arg("--help")
        .output()
        .expect("failed to run bifrost --help");
    let combined = String::from_utf8_lossy(&output.stdout).to_string()
        + String::from_utf8_lossy(&output.stderr).as_ref();
    assert!(
        combined.contains("COMMAND SHORTCUTS"),
        "main help should show COMMAND SHORTCUTS section"
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
fn start_all_options_parse() {
    let help = run_help(&["start"]);
    for opt in &[
        "--port",
        "--host",
        "--socks5-port",
        "--daemon",
        "--skip-cert-check",
        "--access-mode",
        "--whitelist",
        "--allow-lan",
        "--proxy-user",
        "--intercept",
        "--no-intercept",
        "--intercept-exclude",
        "--intercept-include",
        "--app-intercept-exclude",
        "--app-intercept-include",
        "--unsafe-ssl",
        "--no-disconnect-on-config-change",
        "--rules",
        "--rules-file",
        "--system-proxy",
        "--proxy-bypass",
        "--cli-proxy",
        "--cli-proxy-no-proxy",
    ] {
        assert!(
            help.contains(opt),
            "start help should contain '{}' option",
            opt
        );
    }
}

#[test]
fn root_help_start_reference_stays_in_sync_with_start_flags() {
    let help = run_help(&[]);
    for opt in &[
        "start [OPTIONS]",
        "--skip-cert-check",
        "--access-mode",
        "--proxy-user <USER:PASS>",
        "--system-proxy",
        "--no-system-proxy",
        "--proxy-bypass",
        "--cli-proxy",
        "--cli-proxy-no-proxy",
        "-y, --yes",
    ] {
        assert!(
            help.contains(opt),
            "root help subcommand reference should contain '{}'",
            opt
        );
    }
}

#[test]
fn search_all_options_parse() {
    let help = run_help(&["search"]);
    for opt in &[
        "--interactive",
        "--limit",
        "--format",
        "--url",
        "--headers",
        "--body",
        "--req-header",
        "--res-header",
        "--req-body",
        "--res-body",
        "--status",
        "--method",
        "--host",
        "--path",
        "--protocol",
        "--content-type",
        "--domain",
        "--no-color",
        "--max-scan",
        "--max-results",
    ] {
        assert!(
            help.contains(opt),
            "search help should contain '{}' option",
            opt
        );
    }
}

#[test]
fn traffic_list_all_options_parse() {
    let help = run_help(&["traffic", "list"]);
    for opt in &[
        "--limit",
        "--cursor",
        "--direction",
        "--method",
        "--status",
        "--status-min",
        "--status-max",
        "--protocol",
        "--host",
        "--url",
        "--path",
        "--content-type",
        "--client-ip",
        "--client-app",
        "--has-rule-hit",
        "--is-websocket",
        "--is-sse",
        "--is-tunnel",
        "--format",
        "--no-color",
    ] {
        assert!(
            help.contains(opt),
            "traffic list help should contain '{}' option",
            opt
        );
    }
}
