use bifrost_core::{
    init_logging_with_config, install_panic_hook, BifrostError, LogConfig, LogOutput,
};
use bifrost_storage::{data_dir, ConfigManager, DEFAULT_REMOTE_BASE_URL};
use bifrost_tls::init_crypto_provider;
use clap::{CommandFactory, Parser};
use clap_complete::generate;
use std::io::IsTerminal;
use std::path::Path;

mod cli;
mod commands;
mod config;
mod help;
mod parsing;
mod process;

use cli::{AiCommands, AiVoiceCommands, Cli, Commands, ImportArgs, TrafficCommands};
use commands::{
    check_and_print_update_notice, handle_admin_command, handle_ai_command, handle_ca_command,
    handle_config_command, handle_export_command, handle_group_command, handle_import_command,
    handle_install_skill, handle_metrics_command, handle_port_command, handle_rule_command,
    handle_script_command, handle_sync_command, handle_system_proxy_command, handle_upgrade,
    handle_value_command, handle_whitelist_command, remote, run_restart, run_search, run_start,
    run_status, run_status_tui, run_stop, run_traffic_clear, run_traffic_get, run_traffic_list,
    spawn_update_check_notice, OutputFormat, RestartOptions, SearchOptions, TrafficGetOptions,
    TrafficListOptions,
};
use process::read_runtime_port;

const DEFAULT_PORT: u16 = 9900;

fn get_effective_port(cli_port: u16) -> u16 {
    if cli_port != DEFAULT_PORT {
        return cli_port;
    }
    read_runtime_port().unwrap_or(DEFAULT_PORT)
}

fn normalize_relay_url(value: Option<String>) -> Option<String> {
    value.and_then(|url| {
        let trimmed = url.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_string())
    })
}

fn select_remote_relay_url(
    explicit: Option<String>,
    runtime: Option<String>,
    configured: Option<String>,
) -> String {
    normalize_relay_url(explicit)
        .or_else(|| normalize_relay_url(runtime))
        .or_else(|| normalize_relay_url(configured))
        .unwrap_or_else(|| DEFAULT_REMOTE_BASE_URL.to_string())
}

fn read_runtime_relay_url(port: u16) -> Option<String> {
    let client = commands::config::client::ConfigApiClient::new("127.0.0.1", port);
    client
        .get_sync_status()
        .ok()
        .and_then(|status| normalize_relay_url(Some(status.remote_base_url)))
}

fn read_configured_relay_url(data_dir_path: &Path) -> Option<String> {
    let config_path = data_dir_path.join("config.toml");
    if !config_path.exists() {
        return None;
    }

    ConfigManager::new(data_dir_path.to_path_buf())
        .ok()
        .and_then(|manager| manager.try_config())
        .and_then(|config| normalize_relay_url(Some(config.sync.remote_base_url)))
}

fn resolve_remote_relay_url(explicit: Option<String>, cli_port: u16) -> String {
    let effective_port = get_effective_port(cli_port);
    let runtime = read_runtime_relay_url(effective_port);
    let configured = read_configured_relay_url(&data_dir());
    select_remote_relay_url(explicit, runtime, configured)
}

fn should_run_update_notice(stdout_is_terminal: bool, command: Option<&Commands>) -> bool {
    if !stdout_is_terminal && std::env::var_os("BIFROST_FORCE_UPDATE_CHECK").is_none() {
        return false;
    }

    !matches!(
        command,
        Some(Commands::VersionCheck) | Some(Commands::Upgrade { .. })
    )
}

fn command_uses_stdout_protocol(command: Option<&Commands>) -> bool {
    matches!(
        command,
        Some(Commands::Ai {
            action: AiCommands::Voice {
                action: AiVoiceCommands::Worker { .. }
            }
        }) | Some(Commands::Agent {
            action: cli::AgentCommands::Worker,
        }) | Some(Commands::Agent {
            action: cli::AgentCommands::ExternalRunnerWorker,
        }) | Some(Commands::AsrDiarizationWorker { .. })
    )
}

fn effective_log_outputs(cli: &Cli) -> Vec<LogOutput> {
    if command_uses_stdout_protocol(cli.command.as_ref()) {
        vec![LogOutput::File]
    } else {
        LogOutput::parse(&cli.log_output)
    }
}

fn main() {
    install_panic_hook();
    init_crypto_provider();

    let cli = Cli::parse();

    let is_daemon_mode = matches!(&cli.command, Some(Commands::Start { daemon: true, .. }));

    let _log_guard = if is_daemon_mode {
        None
    } else {
        let log_dir = cli
            .log_dir
            .clone()
            .unwrap_or_else(|| data_dir().join("logs"));

        let log_outputs = effective_log_outputs(&cli);

        let log_config = LogConfig::new(cli.log_level.clone(), log_dir)
            .with_outputs(log_outputs)
            .with_retention_days(cli.log_retention_days);

        match init_logging_with_config(&log_config) {
            Ok(guard) => Some(guard),
            Err(e) => {
                eprintln!("Failed to initialize logging: {}", e);
                std::process::exit(1);
            }
        }
    };

    if should_run_update_notice(std::io::stdout().is_terminal(), cli.command.as_ref()) {
        match &cli.command {
            Some(Commands::Start { daemon: false, .. }) => spawn_update_check_notice(),
            Some(Commands::Start { daemon: true, .. }) => {}
            _ => check_and_print_update_notice(),
        }
    }

    let result = match cli.command {
        Some(Commands::AsrDiarizationWorker { request }) => {
            bifrost_admin::run_asr_diarization_worker_stdio(&request).map_err(BifrostError::Config)
        }
        Some(Commands::Start {
            port,
            host,
            socks5_port,
            daemon,
            skip_cert_check,
            ref access_mode,
            ref whitelist,
            allow_lan,
            ref proxy_user,
            intercept,
            no_intercept,
            ref intercept_exclude,
            ref intercept_include,
            ref app_intercept_exclude,
            ref app_intercept_include,
            unsafe_ssl,
            enable_badge_injection,
            disable_badge_injection,
            no_disconnect_on_config_change,
            ref rules,
            ref rules_file,
            system_proxy,
            no_system_proxy,
            ref proxy_bypass,
            cli_proxy,
            ref cli_proxy_no_proxy,
            yes,
        }) => {
            let effective_port = port.unwrap_or(cli.port);
            let effective_host = host.clone().unwrap_or_else(|| cli.host.clone());
            let effective_socks5_port = socks5_port.or(cli.socks5_port);
            run_start(
                effective_port,
                effective_host,
                effective_socks5_port,
                &cli.log_level,
                daemon,
                cli.log_dir
                    .clone()
                    .unwrap_or_else(|| data_dir().join("logs")),
                cli.log_retention_days,
                skip_cert_check,
                access_mode.clone(),
                whitelist.clone(),
                allow_lan,
                proxy_user.clone(),
                intercept,
                no_intercept,
                intercept_exclude.clone(),
                intercept_include.clone(),
                app_intercept_exclude.clone(),
                app_intercept_include.clone(),
                unsafe_ssl,
                enable_badge_injection,
                disable_badge_injection,
                no_disconnect_on_config_change,
                rules.clone(),
                rules_file.clone(),
                system_proxy,
                no_system_proxy,
                proxy_bypass.clone(),
                cli_proxy,
                cli_proxy_no_proxy.clone(),
                yes,
            )
        }
        Some(Commands::Stop) => run_stop(),
        Some(Commands::Restart {
            port,
            host,
            log_level,
            force,
        }) => run_restart(RestartOptions {
            port,
            host,
            log_level,
            force,
        }),
        Some(Commands::Status { tui }) => {
            if tui {
                run_status_tui()
            } else {
                run_status()
            }
        }
        Some(Commands::Rule { action }) => handle_rule_command(action),
        Some(Commands::Group { action }) => handle_group_command(action),
        Some(Commands::Port { action }) => handle_port_command(action),
        Some(Commands::Ca { action }) => handle_ca_command(action),
        Some(Commands::Whitelist { action }) => handle_whitelist_command(action),
        Some(Commands::SystemProxy { ref action }) => {
            handle_system_proxy_command(&cli, action.clone())
        }
        Some(Commands::Value { action }) => handle_value_command(action),
        Some(Commands::Admin { action }) => handle_admin_command(action),
        Some(Commands::Ai { action }) => {
            handle_ai_command(action, "127.0.0.1", get_effective_port(cli.port))
        }
        Some(Commands::KeepAwake { action }) => {
            commands::keepawake::handle_local(action, get_effective_port(cli.port))
        }
        Some(Commands::Im { args }) => {
            commands::handle_im_command("127.0.0.1", get_effective_port(cli.port), &args)
        }
        Some(Commands::Agent { action }) => {
            commands::agent::handle_agent_command("127.0.0.1", get_effective_port(cli.port), action)
        }
        Some(Commands::Script { action }) => handle_script_command(action),
        Some(Commands::Upgrade { yes, restart }) => handle_upgrade(yes, restart),
        Some(Commands::InstallSkill {
            tool,
            dir,
            cwd,
            yes,
        }) => handle_install_skill(tool, dir, cwd, yes),
        Some(Commands::Config { action }) => {
            handle_config_command(action, "127.0.0.1", get_effective_port(cli.port))
        }
        Some(Commands::Search {
            keyword,
            interactive,
            limit,
            format,
            url,
            headers,
            body,
            req_header,
            res_header,
            req_body,
            res_body,
            status,
            method,
            host,
            path,
            protocol,
            content_type,
            domain,
            listener_port,
            no_color,
            max_scan,
            max_results,
        }) => {
            let is_interactive = interactive || keyword.is_none();
            let options = SearchOptions {
                keyword: keyword.unwrap_or_default(),
                port: get_effective_port(cli.port),
                limit,
                format: format.parse().unwrap_or(OutputFormat::Table),
                interactive: is_interactive,
                scope_url: url,
                scope_headers: headers,
                scope_body: body,
                scope_request_headers: req_header,
                scope_response_headers: res_header,
                scope_request_body: req_body,
                scope_response_body: res_body,
                filter_status: status,
                filter_method: method,
                filter_protocol: protocol,
                filter_content_type: content_type,
                filter_domain: domain,
                filter_host: host,
                filter_path: path,
                filter_listener_port: listener_port,
                no_color,
                max_scan,
                max_results,
            };
            let exit_code = run_search(options);
            std::process::exit(exit_code);
        }
        Some(Commands::Completions { shell }) => {
            let mut cmd = Cli::command();
            generate(shell, &mut cmd, "bifrost", &mut std::io::stdout());
            Ok(())
        }
        Some(Commands::Metrics { action }) => {
            handle_metrics_command(action, "127.0.0.1", get_effective_port(cli.port))
        }
        Some(Commands::Sync { action }) => {
            handle_sync_command(action, "127.0.0.1", get_effective_port(cli.port))
        }
        Some(Commands::Import { file, detect_only }) => {
            let args = ImportArgs { file, detect_only };
            handle_import_command(args, "127.0.0.1", get_effective_port(cli.port))
        }
        Some(Commands::Export { action }) => {
            handle_export_command(action, "127.0.0.1", get_effective_port(cli.port))
        }
        Some(Commands::VersionCheck) => {
            commands::handle_version_check("127.0.0.1", get_effective_port(cli.port))
        }
        Some(Commands::Setting { action }) => match action {
            cli::SettingCommands::Shell { action } => {
                commands::handle_remote_shell_command(*action)
            }
            cli::SettingCommands::SshKey { action } => commands::handle_remote_ssh_key_command(
                *action,
                "127.0.0.1",
                get_effective_port(cli.port),
            ),
            cli::SettingCommands::Grant { action } => commands::handle_remote_grant_command(
                *action,
                "127.0.0.1",
                get_effective_port(cli.port),
            ),
        },
        Some(Commands::Remote {
            action,
            relay_url,
            client_id,
        }) => {
            let relay_url = resolve_remote_relay_url(relay_url, cli.port);
            remote::handle_remote_command(remote::RemoteOptions {
                relay_url,
                client_id,
                action,
            })
        }
        Some(Commands::Traffic { action }) => match action {
            TrafficCommands::List {
                port,
                limit,
                cursor,
                direction,
                method,
                status,
                status_min,
                status_max,
                protocol,
                host,
                url,
                path,
                content_type,
                client_ip,
                client_app,
                listener_port,
                has_rule_hit,
                is_websocket,
                is_sse,
                is_tunnel,
                format,
                no_color,
            } => {
                let options = TrafficListOptions {
                    port: port.unwrap_or_else(|| get_effective_port(cli.port)),
                    limit,
                    cursor,
                    direction,
                    method,
                    status,
                    status_min,
                    status_max,
                    protocol,
                    host,
                    url,
                    path,
                    content_type,
                    client_ip,
                    client_app,
                    listener_port,
                    has_rule_hit,
                    is_websocket,
                    is_sse,
                    is_tunnel,
                    format: format.parse().unwrap_or(OutputFormat::Table),
                    no_color,
                };
                run_traffic_list(options)
            }
            TrafficCommands::Get {
                port,
                id,
                request_body,
                response_body,
                format,
            } => {
                let options = TrafficGetOptions {
                    port: port.unwrap_or_else(|| get_effective_port(cli.port)),
                    id,
                    request_body,
                    response_body,
                    format: format.parse().unwrap_or(OutputFormat::JsonPretty),
                };
                run_traffic_get(options)
            }
            TrafficCommands::Search {
                keyword,
                interactive,
                limit,
                format,
                url,
                headers,
                body,
                req_header,
                res_header,
                req_body,
                res_body,
                status,
                method,
                host,
                path,
                protocol,
                content_type,
                domain,
                listener_port,
                no_color,
                max_scan,
                max_results,
            } => {
                let is_interactive = interactive || keyword.is_none();
                let options = SearchOptions {
                    keyword: keyword.unwrap_or_default(),
                    port: get_effective_port(cli.port),
                    limit,
                    format: format.parse().unwrap_or(OutputFormat::Table),
                    interactive: is_interactive,
                    scope_url: url,
                    scope_headers: headers,
                    scope_body: body,
                    scope_request_headers: req_header,
                    scope_response_headers: res_header,
                    scope_request_body: req_body,
                    scope_response_body: res_body,
                    filter_status: status,
                    filter_method: method,
                    filter_protocol: protocol,
                    filter_content_type: content_type,
                    filter_domain: domain,
                    filter_host: host,
                    filter_path: path,
                    filter_listener_port: listener_port,
                    no_color,
                    max_scan,
                    max_results,
                };
                let exit_code = run_search(options);
                std::process::exit(exit_code);
            }
            TrafficCommands::Clear { ids, yes } => {
                run_traffic_clear(get_effective_port(cli.port), ids, yes)
            }
        },
        None => run_start(
            cli.port,
            cli.host.clone(),
            cli.socks5_port,
            &cli.log_level,
            false,
            cli.log_dir
                .clone()
                .unwrap_or_else(|| data_dir().join("logs")),
            cli.log_retention_days,
            false,
            None,
            None,
            false,
            vec![],
            false,
            false,
            None,
            None,
            None,
            None,
            false,
            false,
            false,
            false,
            vec![],
            None,
            false,
            false,
            None,
            false,
            None,
            false,
        ),
    };

    if let Err(e) = result {
        eprintln!("Error: {}", e);
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bifrost_storage::UnifiedConfig;
    use tempfile::TempDir;

    #[test]
    fn should_run_update_notice_when_forced_even_without_tty() {
        let previous = std::env::var_os("BIFROST_FORCE_UPDATE_CHECK");
        unsafe {
            std::env::set_var("BIFROST_FORCE_UPDATE_CHECK", "1");
        }

        let should_run = should_run_update_notice(false, Some(&Commands::Status { tui: false }));

        match previous {
            Some(value) => unsafe {
                std::env::set_var("BIFROST_FORCE_UPDATE_CHECK", value);
            },
            None => unsafe {
                std::env::remove_var("BIFROST_FORCE_UPDATE_CHECK");
            },
        }

        assert!(should_run);
    }

    #[test]
    fn normalize_relay_url_trims_and_rejects_empty_values() {
        assert_eq!(normalize_relay_url(None), None);
        assert_eq!(normalize_relay_url(Some("".to_string())), None);
        assert_eq!(normalize_relay_url(Some("   ".to_string())), None);
        assert_eq!(
            normalize_relay_url(Some(" https://relay.example.com/path ".to_string())),
            Some("https://relay.example.com/path".to_string())
        );
    }

    #[test]
    fn select_remote_relay_url_honors_precedence_order() {
        assert_eq!(
            select_remote_relay_url(
                Some("https://cli.example.com".to_string()),
                Some("https://runtime.example.com".to_string()),
                Some("https://config.example.com".to_string()),
            ),
            "https://cli.example.com"
        );

        assert_eq!(
            select_remote_relay_url(
                Some("   ".to_string()),
                Some("https://runtime.example.com".to_string()),
                Some("https://config.example.com".to_string()),
            ),
            "https://runtime.example.com"
        );

        assert_eq!(
            select_remote_relay_url(
                None,
                Some("".to_string()),
                Some("https://config.example.com".to_string()),
            ),
            "https://config.example.com"
        );

        assert_eq!(
            select_remote_relay_url(None, None, None),
            DEFAULT_REMOTE_BASE_URL
        );
    }

    #[test]
    fn read_configured_relay_url_uses_sync_remote_base_url_from_config_file() {
        let temp_dir = TempDir::new().unwrap();
        let manager = ConfigManager::new(temp_dir.path().to_path_buf()).unwrap();

        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            manager
                .update_config(|config: &mut UnifiedConfig| {
                    config.sync.remote_base_url = "https://config.example.com".to_string();
                })
                .await
                .unwrap();
        });

        assert_eq!(
            read_configured_relay_url(temp_dir.path()),
            Some("https://config.example.com".to_string())
        );
    }

    #[test]
    fn read_configured_relay_url_treats_empty_config_as_missing() {
        let temp_dir = TempDir::new().unwrap();
        let manager = ConfigManager::new(temp_dir.path().to_path_buf()).unwrap();

        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            manager
                .update_config(|config: &mut UnifiedConfig| {
                    config.sync.remote_base_url = "   ".to_string();
                })
                .await
                .unwrap();
        });

        assert_eq!(read_configured_relay_url(temp_dir.path()), None);
    }

    #[test]
    fn read_configured_relay_url_returns_none_when_config_file_is_missing() {
        let temp_dir = TempDir::new().unwrap();
        assert_eq!(read_configured_relay_url(temp_dir.path()), None);
    }

    #[test]
    fn should_run_update_notice_skips_non_tty_output() {
        assert!(!should_run_update_notice(
            false,
            Some(&Commands::Status { tui: false })
        ));
    }

    #[test]
    fn should_run_update_notice_skips_explicit_version_related_commands() {
        assert!(!should_run_update_notice(
            true,
            Some(&Commands::VersionCheck)
        ));
        assert!(!should_run_update_notice(
            true,
            Some(&Commands::Upgrade {
                yes: false,
                restart: false,
            })
        ));
        assert!(should_run_update_notice(
            true,
            Some(&Commands::Status { tui: false })
        ));
    }

    #[test]
    fn voice_worker_forces_logs_away_from_stdout_protocol() {
        let cli = Cli::parse_from([
            "bifrost",
            "--log-output",
            "console",
            "ai",
            "voice",
            "worker",
            "--model",
            "Qwen3-ASR-0.6B",
            "--model-dir",
            "/tmp/qwen3",
        ]);

        assert!(command_uses_stdout_protocol(cli.command.as_ref()));
        assert_eq!(effective_log_outputs(&cli), vec![LogOutput::File]);
    }
}
