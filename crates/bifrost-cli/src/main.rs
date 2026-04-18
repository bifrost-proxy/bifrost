use bifrost_core::{init_logging_with_config, install_panic_hook, LogConfig, LogOutput};
use bifrost_storage::data_dir;
use bifrost_tls::init_crypto_provider;
use clap::{CommandFactory, Parser};
use clap_complete::generate;

mod cli;
mod commands;
mod config;
mod help;
mod parsing;
mod process;

use cli::{Cli, Commands, ImportArgs, TrafficCommands};
use commands::{
    check_and_print_update_notice, handle_admin_command, handle_ca_command, handle_config_command,
    handle_export_command, handle_group_command, handle_import_command, handle_install_skill,
    handle_metrics_command, handle_rule_command, handle_script_command, handle_sync_command,
    handle_system_proxy_command, handle_upgrade, handle_value_command, handle_whitelist_command,
    remote, run_search, run_start, run_status, run_status_tui, run_stop, run_traffic_clear,
    run_traffic_get, run_traffic_list, spawn_update_check_notice, OutputFormat, SearchOptions,
    TrafficGetOptions, TrafficListOptions,
};
use process::read_runtime_port;

const DEFAULT_PORT: u16 = 9900;

fn get_effective_port(cli_port: u16) -> u16 {
    if cli_port != DEFAULT_PORT {
        return cli_port;
    }
    read_runtime_port().unwrap_or(DEFAULT_PORT)
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

        let log_outputs = LogOutput::parse(&cli.log_output);

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

    match &cli.command {
        Some(Commands::Start { daemon: false, .. }) => spawn_update_check_notice(),
        Some(Commands::Start { daemon: true, .. }) => {}
        _ => check_and_print_update_notice(),
    }

    let result = match cli.command {
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
        Some(Commands::Status { tui }) => {
            if tui {
                run_status_tui()
            } else {
                run_status()
            }
        }
        Some(Commands::Rule { action }) => handle_rule_command(action),
        Some(Commands::Group { action }) => handle_group_command(action),
        Some(Commands::Ca { action }) => handle_ca_command(action),
        Some(Commands::Whitelist { action }) => handle_whitelist_command(action),
        Some(Commands::SystemProxy { ref action }) => {
            handle_system_proxy_command(&cli, action.clone())
        }
        Some(Commands::Value { action }) => handle_value_command(action),
        Some(Commands::Script { action }) => handle_script_command(action),
        Some(Commands::Admin { action }) => handle_admin_command(action),
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
        Some(Commands::Remote {
            action,
            relay_url,
            token,
            client_id,
            pair_code,
        }) => {
            let relay_url = match relay_url {
                Some(url) => url,
                None => {
                    let client = commands::config::client::ConfigApiClient::new(
                        "127.0.0.1",
                        get_effective_port(cli.port),
                    );
                    match client.get_sync_status() {
                        Ok(status) if !status.remote_base_url.is_empty() => status.remote_base_url,
                        _ => {
                            eprintln!("Error: --relay-url is required (could not detect from running proxy)");
                            std::process::exit(1);
                        }
                    }
                }
            };
            let token = match token {
                Some(t) => t,
                None => {
                    eprintln!("Error: --token is required for relay authentication");
                    std::process::exit(1);
                }
            };
            remote::handle_remote_command(remote::RemoteOptions {
                relay_url,
                token,
                client_id,
                pair_code,
                action,
            })
        }
        Some(Commands::Traffic { action }) => match action {
            TrafficCommands::List {
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
                has_rule_hit,
                is_websocket,
                is_sse,
                is_tunnel,
                format,
                no_color,
            } => {
                let options = TrafficListOptions {
                    port: get_effective_port(cli.port),
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
                id,
                request_body,
                response_body,
                format,
            } => {
                let options = TrafficGetOptions {
                    port: get_effective_port(cli.port),
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
