use bifrost_cli::cli::Cli;
use clap::CommandFactory;

fn run_help(args: &[&str]) -> String {
    let mut argv = Vec::with_capacity(args.len() + 2);
    argv.push("bifrost");
    argv.extend(args.iter().copied());
    argv.push("--help");

    let mut cmd = Cli::command();
    let err = cmd
        .try_get_matches_from_mut(argv)
        .expect_err("--help should short-circuit with rendered help");
    err.render().ansi().to_string()
}

#[test]
fn root_help_is_short_and_links_to_docs() {
    let help = run_help(&[]);

    for removed in &[
        "SUBCOMMAND REFERENCE",
        "RULE TEMPLATE VARIABLES",
        "RULES QUICK START",
        "COMMAND SHORTCUTS",
    ] {
        assert!(
            !help.contains(removed),
            "root help should not contain the old long reference section: {removed}"
        );
    }

    for link in &[
        "https://github.com/bifrost-proxy/bifrost/tree/main/docs",
        "https://github.com/bifrost-proxy/bifrost/blob/main/docs/cli-quick-start.md",
        "https://github.com/bifrost-proxy/bifrost/blob/main/docs/cli.md",
        "https://github.com/bifrost-proxy/bifrost/blob/main/docs/getting-started.md",
        "https://github.com/bifrost-proxy/bifrost/blob/main/docs/rule.md",
        "https://github.com/bifrost-proxy/bifrost/blob/main/docs/operation.md",
        "https://github.com/bifrost-proxy/bifrost/blob/main/docs/pattern.md",
        "https://github.com/bifrost-proxy/bifrost/blob/main/docs/rules/README.md",
    ] {
        assert!(help.contains(link), "root help should link to {link}");
    }

    assert!(
        help.contains("Use 'bifrost <command> -h'"),
        "root help should direct users to subcommand help"
    );
}

#[test]
fn agent_guide_help_exposes_live_turn_arguments() {
    let help = run_help(&["agent", "guide"]);

    assert!(help.contains("Guide an active Codex or Traex runner turn"));
    assert!(help.contains("--session"));
    assert!(help.contains("Guidance message to append to the active turn"));
    assert!(help.contains("--json"));
}

#[test]
fn port_help_explains_multi_port_model() {
    let help = run_help(&["port"]);
    for snippet in &[
        "Multi-port model",
        "main proxy port keeps using the normal enabled-rule view",
        "Each temporary port uses the global Default rule plus the refs passed via `bind` or `update`",
        "Bind one or more temporary ports with `port bind`",
        "Replace a port's bound rule refs with `port update`",
        "destroy",
    ] {
        assert!(
            help.contains(snippet),
            "port help should contain '{snippet}'"
        );
    }
}

#[test]
fn port_bind_help_lists_all_rule_sources() {
    let help = run_help(&["port", "bind"]);
    for snippet in &[
        "At least one of the following must be provided",
        "--rule",
        "--group-rule",
        "--rule-file",
        "--rule-text",
        "Use `--port 0` to let Bifrost allocate a free port automatically",
    ] {
        assert!(
            help.contains(snippet),
            "port bind help should contain '{snippet}'"
        );
    }
}

#[test]
fn port_update_help_lists_all_rule_sources() {
    let help = run_help(&["port", "update"]);
    for snippet in &[
        "Replace the explicit rule refs for a temporary proxy port",
        "main proxy process",
        "The main port and other",
        "temporary ports are not affected",
        "--rule, --group-rule, --rule-file, --rule-text",
    ] {
        assert!(
            help.contains(snippet),
            "port update help should contain '{snippet}'"
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
        assert!(help.contains(opt), "start help should contain '{opt}'");
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
        assert!(help.contains(opt), "search help should contain '{opt}'");
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
        "--listener-port",
        "--proxy-port",
        "--has-rule-hit",
        "--is-websocket",
        "--is-sse",
        "--is-tunnel",
        "--format",
        "--no-color",
    ] {
        assert!(
            help.contains(opt),
            "traffic list help should contain '{opt}'"
        );
    }
}

#[test]
fn traffic_search_help_lists_listener_port_filter() {
    let help = run_help(&["traffic", "search"]);
    assert!(
        help.contains("--listener-port"),
        "traffic search help should list listener port filter"
    );
    assert!(
        help.contains("--proxy-port"),
        "traffic search help should list proxy-port alias"
    );
}
