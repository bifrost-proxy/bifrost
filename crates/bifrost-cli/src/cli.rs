use std::path::PathBuf;

use clap::{ArgAction, Parser, Subcommand, ValueEnum, ValueHint};
use clap_complete::Shell;

#[derive(ValueEnum, Copy, Clone, Debug, PartialEq, Eq)]
pub enum StatusFormat {
    /// Human-readable text output (default).
    Text,
    /// Machine-readable JSON on a single line.
    Json,
    /// Machine-readable JSON with 2-space indentation.
    #[value(name = "json-pretty")]
    JsonPretty,
}

#[derive(Parser)]
#[command(name = "bifrost")]
#[command(version = env!("CARGO_PKG_VERSION"))]
#[command(disable_version_flag = true)]
#[command(about = "High-performance HTTP/HTTPS/SOCKS5/HTTP3 proxy written in Rust")]
#[command(
    long_about = "High-performance HTTP/HTTPS/SOCKS5/HTTP3 proxy written in Rust with TLS interception support.\n\n\
Supported Protocols:\n\
  • HTTP/1.1, HTTP/2, HTTP/3 (QUIC)\n\
  • HTTPS (TLS 1.2/1.3, MITM interception)\n\
  • SOCKS5 TCP/UDP (with authentication)\n\
  • WebSocket (ws/wss)\n\
  • CONNECT-UDP (MASQUE, RFC 9298)\n\
  • gRPC, SSE\n\n\
Running 'bifrost' without a subcommand is equivalent to 'bifrost start'."
)]
#[command(after_help = "\
EXAMPLES:
    bifrost                      Start proxy with defaults (port 9900, TLS disabled)
    bifrost -p 8080              Start proxy on port 8080
    bifrost start --daemon       Start proxy as background daemon
    bifrost start --no-intercept Start proxy without TLS interception
    bifrost start --intercept    Start proxy with TLS interception enabled
    bifrost start --intercept-exclude '*.apple.com,*.microsoft.com'
                                 Exclude domains from TLS interception
    bifrost start --intercept-include '*.api.local'
                                 Force intercept specific domains (works even with --no-intercept)
    bifrost status               Show proxy status
    bifrost stop                 Stop the running proxy

DEFAULT BEHAVIOR:
    When no subcommand is provided, bifrost starts in foreground mode with:
      • HTTP proxy on 0.0.0.0:9900
      • TLS/HTTPS interception disabled
      • Access restricted to localhost only
      • CA certificate auto-generated if missing

TIP:
    Use 'bifrost <command> -h' for command-specific options and examples.

More docs:
  https://github.com/bifrost-proxy/bifrost/tree/main/docs
  https://github.com/bifrost-proxy/bifrost/blob/main/docs/cli-quick-start.md
  https://github.com/bifrost-proxy/bifrost/blob/main/docs/cli.md
  https://github.com/bifrost-proxy/bifrost/blob/main/docs/getting-started.md
  https://github.com/bifrost-proxy/bifrost/blob/main/docs/rule.md
  https://github.com/bifrost-proxy/bifrost/blob/main/docs/operation.md
  https://github.com/bifrost-proxy/bifrost/blob/main/docs/pattern.md
  https://github.com/bifrost-proxy/bifrost/blob/main/docs/rules/README.md
")]
pub struct Cli {
    #[arg(short = 'v', short_alias = 'V', long, action = ArgAction::Version, help = "Print version")]
    pub version: (),

    #[command(subcommand)]
    pub command: Option<Commands>,

    #[arg(short, long, default_value = "9900", help = "HTTP proxy port")]
    pub port: u16,

    #[arg(short = 'H', long, default_value = "0.0.0.0", value_hint = ValueHint::Hostname, help = "Listen address")]
    pub host: String,

    #[arg(
        long,
        help = "Separate SOCKS5 proxy port (by default SOCKS5 shares the main port)"
    )]
    pub socks5_port: Option<u16>,

    #[arg(
        short,
        long,
        default_value = "info",
        value_parser = ["trace", "debug", "info", "warn", "error"],
        help = "Log level [trace|debug|info|warn|error]"
    )]
    pub log_level: String,

    #[arg(
        long,
        default_value = "file",
        value_parser = ["console", "file", "console,file"],
        help = "Log output targets: file (default) or console/console,file (also keeps file logging)"
    )]
    pub log_output: String,

    #[arg(long, value_hint = ValueHint::DirPath, help = "Log file directory (default: <data_dir>/logs)")]
    pub log_dir: Option<PathBuf>,

    #[arg(long, default_value = "7", help = "Number of days to retain log files")]
    pub log_retention_days: u32,
}

#[derive(Subcommand)]
pub enum Commands {
    #[command(hide = true)]
    AsrDiarizationWorker {
        #[arg(long, value_hint = ValueHint::FilePath)]
        request: PathBuf,
    },
    #[command(hide = true)]
    AppIconWorker {
        #[arg(long, value_hint = ValueHint::FilePath)]
        path: PathBuf,
        #[arg(long, value_hint = ValueHint::FilePath)]
        output: PathBuf,
    },
    #[command(about = "Start the proxy server (default when no subcommand provided)")]
    Start {
        #[arg(short, long, help = "HTTP proxy port (overrides global -p)")]
        port: Option<u16>,
        #[arg(short = 'H', long, value_hint = ValueHint::Hostname, help = "Listen address (overrides global -H)")]
        host: Option<String>,
        #[arg(
            long,
            help = "Separate SOCKS5 port (overrides global; omit to share main port)"
        )]
        socks5_port: Option<u16>,
        #[arg(short, long, help = "Run as daemon")]
        daemon: bool,
        #[arg(long, help = "Skip CA certificate installation check")]
        skip_cert_check: bool,
        #[arg(
            long,
            value_parser = ["local_only", "whitelist", "interactive", "allow_all"],
            help = "Access control mode: local_only, whitelist, interactive (default), allow_all"
        )]
        access_mode: Option<String>,
        #[arg(
            long,
            help = "Client IP whitelist (comma-separated, supports CIDR notation)"
        )]
        whitelist: Option<String>,
        #[arg(long, help = "Allow LAN (private network) clients")]
        allow_lan: bool,
        #[arg(
            long,
            help = "Proxy user credentials in USER:PASS format. Can be specified multiple times."
        )]
        proxy_user: Vec<String>,
        #[arg(
            long,
            conflicts_with = "no_intercept",
            help = "Enable TLS/HTTPS interception"
        )]
        intercept: bool,
        #[arg(long, help = "Disable TLS/HTTPS interception (default: disabled)")]
        no_intercept: bool,
        #[arg(
            long,
            help = "Domains to exclude from TLS interception (comma-separated, supports wildcards like *.example.com). Has higher priority than global switch."
        )]
        intercept_exclude: Option<String>,
        #[arg(
            long,
            help = "Domains to force TLS interception (comma-separated, supports wildcards). Has highest priority, works even when interception is disabled."
        )]
        intercept_include: Option<String>,
        #[arg(
            long,
            help = "Applications to exclude from TLS interception (comma-separated, supports wildcards like *Safari). Traffic from these apps will not be intercepted."
        )]
        app_intercept_exclude: Option<String>,
        #[arg(
            long,
            help = "Applications to force TLS interception (comma-separated, supports wildcards). Traffic from these apps will always be intercepted."
        )]
        app_intercept_include: Option<String>,
        #[arg(
            long,
            help = "Skip upstream server TLS certificate verification (dangerous, for testing only)"
        )]
        unsafe_ssl: bool,
        #[arg(
            long,
            conflicts_with = "disable_badge_injection",
            help = "Enable injecting Bifrost badge into HTML pages"
        )]
        enable_badge_injection: bool,
        #[arg(
            long,
            conflicts_with = "enable_badge_injection",
            help = "Disable injecting Bifrost badge into HTML pages"
        )]
        disable_badge_injection: bool,
        #[arg(
            long,
            help = "Disable automatic disconnect of affected connections when TLS config changes"
        )]
        no_disconnect_on_config_change: bool,
        #[arg(
            long,
            help = "Proxy rules (e.g., 'example.com host://127.0.0.1:3000' or 'chatgpt.com http3://'). Can be specified multiple times."
        )]
        rules: Vec<String>,
        #[arg(long, value_hint = ValueHint::FilePath, help = "Path to rules file (one rule per line)")]
        rules_file: Option<PathBuf>,
        #[arg(long, help = "Enable system proxy configuration")]
        system_proxy: bool,
        #[arg(
            long,
            conflicts_with = "system_proxy",
            help = "Disable system proxy configuration"
        )]
        no_system_proxy: bool,
        #[arg(
            long,
            help = "System proxy bypass list (comma-separated, e.g., 'localhost,127.0.0.1,*.local')"
        )]
        proxy_bypass: Option<String>,
        #[arg(
            long,
            help = "Enable CLI proxy env vars while proxy is running (writes to shell rc files)"
        )]
        cli_proxy: bool,
        #[arg(
            long,
            help = "CLI proxy no-proxy list (comma-separated, e.g., 'localhost,127.0.0.1,*.local')"
        )]
        cli_proxy_no_proxy: Option<String>,

        #[arg(short = 'y', long, help = "Automatically answer yes to prompts")]
        yes: bool,

        #[cfg(not(target_os = "linux"))]
        #[arg(long, help = "Disable tray icon helper")]
        no_tray: bool,
    },
    #[command(about = "Stop the proxy server")]
    Stop,
    #[command(
        about = "Stop the proxy (if running) and start a fresh detached daemon",
        long_about = concat!(
            "Stop the proxy (if running) and start a fresh detached daemon.\n",
            "\n",
            "Typical use: after `bifrost upgrade` replaced the binary on disk,\n",
            "run `bifrost restart` to pick up the new version.\n",
            "\n",
            "Safe to invoke remotely via `bifrost remote exec`: the new\n",
            "daemon is fully detached from the caller's shell pipes (setsid +\n",
            "/dev/null stdio) so the shell.exec channel reaping the restart\n",
            "invocation does not take the fresh daemon down with it.\n",
        )
    )]
    Restart {
        #[arg(short, long, help = "HTTP proxy port override for the fresh daemon")]
        port: Option<u16>,
        #[arg(short = 'H', long, value_hint = ValueHint::Hostname, help = "Listen address override")]
        host: Option<String>,
        #[arg(long, help = "Log level override")]
        log_level: Option<String>,
        #[arg(long, help = "Force a start even if no live pid is found")]
        force: bool,
    },
    #[command(visible_alias = "st", about = "Show proxy server status")]
    Status {
        #[arg(short, long, help = "Show interactive TUI dashboard")]
        tui: bool,
        #[arg(
            long,
            value_enum,
            default_value_t = StatusFormat::Text,
            help = "Output format: text (default), json (single-line), or json-pretty (indented)"
        )]
        format: StatusFormat,
    },
    #[command(about = "Manage rules")]
    Rule {
        #[command(subcommand)]
        action: RuleCommands,
    },
    #[command(about = "Manage groups and group rules")]
    Group {
        #[command(subcommand)]
        action: GroupCommands,
    },
    #[command(
        about = "Manage temporary proxy ports bound to explicit rule sets",
        long_about = concat!(
            "Manage temporary proxy ports bound to explicit rule sets.\n",
            "\n",
            "Multi-port model:\n",
            "  - The main proxy port keeps using the normal enabled-rule view.\n",
            "  - Each temporary port uses only the rule refs passed via `bind` or `update`.\n",
            "  - Temporary ports share the same data dir, values, scripts, certs, and traffic DB.\n",
            "  - Destroying a temporary port stops only that listener and does not affect the main port.\n",
            "\n",
            "Typical workflow:\n",
            "  1. Start the main proxy first.\n",
            "  2. Bind one or more temporary ports with `port bind`.\n",
            "  3. Inspect bindings with `port list`, `port show`, and `port active`.\n",
            "  4. Replace a port's bound rule refs with `port update`.\n",
            "  5. Remove the listener with `port destroy` when debugging is done.\n",
            "\n",
            "Rule sources:\n",
            "  --rule         Existing local rule name\n",
            "  --group-rule   Existing group rule in <group_id>/<rule_name> format\n",
            "  --rule-file    One-off rule file bound directly to the port\n",
            "  --rule-text    One-off inline rule text bound directly to the port\n",
            "\n",
            "The `port` command requires the main proxy to already be running."
        )
    )]
    Port {
        #[command(subcommand)]
        action: PortCommands,
    },
    #[command(about = "Manage CA certificates")]
    Ca {
        #[command(subcommand)]
        action: CaCommands,
    },
    #[command(visible_alias = "wl", about = "Manage client IP whitelist")]
    Whitelist {
        #[command(subcommand)]
        action: WhitelistCommands,
    },
    #[command(
        visible_alias = "sp",
        about = "Toggle system proxy (enable/disable/status)"
    )]
    SystemProxy {
        #[command(subcommand)]
        action: SystemProxyCommands,
    },
    #[command(
        visible_alias = "val",
        about = "Manage values for rule variable expansion"
    )]
    Value {
        #[command(subcommand)]
        action: ValueCommands,
    },
    #[command(about = "Manage scripts (request/response/decode)")]
    Script {
        #[command(subcommand)]
        action: ScriptCommands,
    },
    #[command(about = "Upgrade bifrost to the latest version")]
    Upgrade {
        #[arg(short = 'y', long, help = "Skip confirmation prompt")]
        yes: bool,
        #[arg(long, help = "Automatically restart the running proxy after upgrade")]
        restart: bool,
    },
    #[command(
        hide = true,
        about = "Run an unattended background upgrade (used by tray/admin)"
    )]
    SelfUpdate {
        #[arg(long, help = "Target version (informational; engine resolves latest)")]
        target: Option<String>,
        #[arg(
            long,
            default_value = "cli",
            help = "Who initiated the upgrade: tray/admin/cli"
        )]
        source: String,
    },
    #[command(visible_alias = "cfg", about = "Manage runtime configuration")]
    Config {
        #[command(subcommand)]
        action: Option<ConfigCommands>,
    },
    #[command(about = "Manage admin remote access and authentication")]
    Admin {
        #[command(subcommand)]
        action: AdminCommands,
    },
    #[command(about = "Manage local AI tools")]
    Ai {
        #[command(subcommand)]
        action: AiCommands,
    },
    #[command(about = "Inspect and query traffic records")]
    Traffic {
        #[command(subcommand)]
        action: TrafficCommands,
    },
    #[command(about = "Wait for the next traffic record matching a filter")]
    Capture {
        #[command(subcommand)]
        action: CaptureCommands,
    },
    #[command(
        about = "Install bifrost SKILL.md to AI coding tools (Claude Code, Codex, Trae, Cursor, GitHub Copilot, and standard Agent Skills runtimes)"
    )]
    InstallSkill {
        #[arg(
            short,
            long,
            value_parser = [
                "claude-code",
                "codex",
                "trae",
                "cursor",
                "github-copilot",
                "universal",
                "all"
            ],
            help = "Target tool: claude-code, codex, trae, cursor, github-copilot, universal, or 'all' (default: all)"
        )]
        tool: Option<String>,
        #[arg(
            short,
            long,
            value_hint = ValueHint::DirPath,
            help = "Custom install directory (overrides default tool path)"
        )]
        dir: Option<PathBuf>,
        #[arg(
            long,
            help = "Install to current directory (project-level) instead of global directory"
        )]
        cwd: bool,
        #[arg(short = 'y', long, help = "Skip confirmation prompt")]
        yes: bool,
    },
    #[command(about = "Search traffic records with advanced filtering")]
    Search {
        #[arg(help = "Search keyword (searches URL, headers, body)")]
        keyword: Option<String>,
        #[arg(short, long, help = "Interactive TUI mode (default if no keyword)")]
        interactive: bool,
        #[arg(short, long, default_value = "50", help = "Maximum results to return")]
        limit: usize,
        #[arg(
            short,
            long,
            default_value = "table",
            value_parser = ["table", "compact", "json", "json-pretty", "ndjson"],
            help = "Output format: table, compact, json, json-pretty, ndjson"
        )]
        format: String,
        #[arg(
            long = "req-json",
            value_name = "PATH=VALUE",
            help = "JSONPath filter on request body, e.g. $.user.name=alice (repeatable)"
        )]
        req_json: Vec<String>,
        #[arg(
            long = "res-json",
            value_name = "PATH=VALUE",
            help = "JSONPath filter on response body, e.g. $.data.errno=0 (repeatable)"
        )]
        res_json: Vec<String>,
        #[arg(
            long = "req-header-eq",
            value_name = "NAME=VALUE",
            help = "Request header equals filter (repeatable, case-insensitive)"
        )]
        req_header_eq: Vec<String>,
        #[arg(
            long = "res-header-eq",
            value_name = "NAME=VALUE",
            help = "Response header equals filter (repeatable, case-insensitive)"
        )]
        res_header_eq: Vec<String>,
        #[arg(
            long,
            value_name = "TIME",
            help = "Restrict to records since this time (RFC3339 or relative: 30s/5m/2h/1d)"
        )]
        since: Option<String>,
        #[arg(
            long,
            value_name = "TIME",
            help = "Restrict to records until this time (RFC3339 or relative)"
        )]
        until: Option<String>,
        #[arg(
            long,
            value_name = "DURATION",
            help = "Shortcut for --since now-DURATION (e.g. 5m, 2h)"
        )]
        latest: Option<String>,
        #[arg(long, help = "Search only in URL/path")]
        url: bool,
        #[arg(long, help = "Search only in headers")]
        headers: bool,
        #[arg(long, help = "Search only in body")]
        body: bool,
        #[arg(long = "req-header", help = "Search only in request headers")]
        req_header: bool,
        #[arg(long = "res-header", help = "Search only in response headers")]
        res_header: bool,
        #[arg(long = "req-body", help = "Search only in request body")]
        req_body: bool,
        #[arg(long = "res-body", help = "Search only in response body")]
        res_body: bool,
        #[arg(long, value_parser = ["2xx", "3xx", "4xx", "5xx", "error"], help = "Filter by status: 2xx, 3xx, 4xx, 5xx, error")]
        status: Option<String>,
        #[arg(long, value_parser = ["GET", "POST", "PUT", "DELETE", "PATCH", "HEAD", "OPTIONS", "CONNECT", "TRACE"], help = "Filter by HTTP method: GET, POST, PUT, DELETE, etc.")]
        method: Option<String>,
        #[arg(long, help = "Filter host contains")]
        host: Option<String>,
        #[arg(long, help = "Filter path contains")]
        path: Option<String>,
        #[arg(
            long = "listener-port",
            visible_alias = "proxy-port",
            help = "Filter by proxy listener port"
        )]
        listener_port: Option<u16>,
        #[arg(long, value_parser = ["HTTP", "HTTPS", "WS", "WSS"], help = "Filter by protocol: HTTP, HTTPS, WS, WSS")]
        protocol: Option<String>,
        #[arg(long, value_parser = ["json", "xml", "html", "form", "text", "javascript", "css", "image", "font", "binary"], help = "Filter by content type: json, xml, html, form, etc.")]
        content_type: Option<String>,
        #[arg(long, help = "Filter by domain pattern")]
        domain: Option<String>,
        #[arg(long, help = "Disable colored output")]
        no_color: bool,
        #[arg(
            long = "max-scan",
            default_value = "10000",
            help = "Maximum records to scan (default: 10000, use larger value for broader search)"
        )]
        max_scan: Option<usize>,
        #[arg(
            long = "max-results",
            default_value = "100",
            help = "Maximum matching results to return (default: 100)"
        )]
        max_results: Option<usize>,
        #[arg(
            long = "include",
            value_name = "PARTS",
            value_delimiter = ',',
            help = "Attach extras to each result: request-body, response-body, request-headers, response-headers (aliases: req-body, res-body, req-headers, res-headers; shortcuts: bodies, headers)"
        )]
        include: Vec<String>,
        #[arg(
            long = "max-body",
            value_name = "BYTES",
            help = "Max bytes per body when --include includes a body (default: 65536)"
        )]
        max_body: Option<usize>,
    },
    #[command(visible_alias = "comp", about = "Generate shell completion scripts")]
    Completions {
        #[arg(
            value_name = "SHELL",
            help = "Target shell: bash, zsh, fish, elvish, powershell"
        )]
        shell: Shell,
    },
    #[command(about = "View real-time metrics and statistics")]
    Metrics {
        #[command(subcommand)]
        action: MetricsCommands,
    },
    #[command(
        about = "Login to sync service",
        long_about = concat!(
            "Login to the sync service.\n",
            "\n",
            "Equivalent to `bifrost sync login`. Without options, opens the\n",
            "browser login flow. For CI or headless environments, pass a token\n",
            "directly. If --url is omitted, Bifrost uses the configured sync\n",
            "remote URL, which defaults to the built-in Bifrost provider.\n",
            "\n",
            "Get a token from:\n",
            "  https://bifrost.bytedance.net/v4/sso/token-login\n",
            "\n",
            "EXAMPLES:\n",
            "  bifrost login\n",
            "  bifrost login --token \"$BIFROST_SYNC_TOKEN\"\n",
            "  bifrost login --token \"$BIFROST_SYNC_TOKEN\" --url https://bifrost.bytedance.net",
        )
    )]
    Login {
        #[arg(
            long,
            value_name = "TOKEN",
            num_args = 0..=1,
            default_missing_value = "",
            help = "Sync session token for non-interactive login; get one at https://bifrost.bytedance.net/v4/sso/token-login"
        )]
        token: Option<String>,
        #[arg(
            long,
            value_hint = ValueHint::Url,
            help = "Remote sync URL for non-interactive login (default: configured sync remote URL, built-in provider if unchanged)"
        )]
        url: Option<String>,
    },
    #[command(about = "Manage remote sync")]
    Sync {
        #[command(subcommand)]
        action: SyncCommands,
    },
    #[command(about = "Import a .bifrost file (rules, scripts, values)")]
    Import {
        #[arg(value_hint = ValueHint::FilePath, help = "Path to .bifrost file")]
        file: PathBuf,
        #[arg(long, help = "Only detect file type without importing")]
        detect_only: bool,
    },
    #[command(about = "Export rules, scripts, or values to .bifrost file")]
    Export {
        #[command(subcommand)]
        action: ExportCommands,
    },
    #[command(about = "Check for new version without upgrading")]
    VersionCheck,
    #[command(
        about = "Manage local Bifrost settings (shell policies, grants)",
        long_about = concat!(
            "Manage local Bifrost settings and policies on THIS machine.\n",
            "\n",
            "Unlike `bifrost remote` (which operates on a connected remote\n",
            "device), `bifrost setting` always targets the current machine's\n",
            "data directory: Shell Access profiles/policies and local\n",
            "remote-invoke grants.\n",
            "\n",
            "EXAMPLES:\n",
            "\n",
            "  # List local Shell Access policies\n",
            "  bifrost setting shell policy list\n",
            "\n",
            "  # Add a Shell Access profile that the policy below will bind to\n",
            "  bifrost setting shell profile add --id default --name Default \\\n",
            "      --cwd \"$HOME\" --env PATH --env HOME --timeout-ms 30000\n",
            "\n",
            "  # Add a Shell Access policy (shell_text regex mode)\n",
            "  bifrost setting shell policy add --id allow-bifrost-cli \\\n",
            "      --name \"Allow Bifrost CLI\" --mode shell_text \\\n",
            "      --pattern '^bifrost\\s+' --shell /bin/bash --profile default\n",
            "\n",
            "  # Inspect / revoke local remote-invoke grants\n",
            "  bifrost setting grant list\n",
            "  bifrost setting grant revoke --grant-id <grant-id>\n",
            "\n",
            "  # Create/export the local remote-invoke SSH key for reusable access\n",
            "  bifrost setting ssh-key create --label dev-mac --output ~/.bifrost/remote-device.key\n",
            "  bifrost setting ssh-key status\n",
            "\n",
            "Note: these commands never touch a remote device. To configure a\n",
            "remote machine, run the equivalent `bifrost setting ...` there via\n",
            "`bifrost remote exec -- bifrost setting ...`.",
        )
    )]
    Setting {
        #[command(subcommand)]
        action: SettingCommands,
    },
    #[command(about = "Execute commands on a remote Bifrost instance via relay")]
    Remote {
        #[command(subcommand)]
        action: RemoteCommands,
        #[arg(
            long,
            global = true,
            help = "Relay server URL (default: CLI arg > running sync config > local config > built-in default)"
        )]
        relay_url: Option<String>,
        #[arg(
            long,
            global = true,
            help = "Target client instance ID / label prefix. Put this after `remote`, before the subcommand; env fallback: BIFROST_REMOTE_CLIENT_ID"
        )]
        client_id: Option<String>,
    },
    #[command(
        about = "Manage macOS keep-awake (prevent sleep while Bifrost is running)",
        long_about = "Manage macOS keep-awake.\n\n            Prevents display sleep, idle sleep, AND lid-close sleep while the Bifrost process is alive.\n            Uses macOS IOKit IOPMAssertion (user-space, no sudo). Non-macOS platforms return an error."
    )]
    KeepAwake {
        #[command(subcommand)]
        action: crate::commands::keepawake::KeepAwakeAction,
    },
    #[command(
        about = "Manage IM Gateway (providers, targets, routes, schedules)",
        long_about = "Manage IM Gateway for sending messages, routing events to scripts, and scheduling tasks.\n\nSUBCOMMANDS:\n  provider    Manage IM providers (feishu, wechat, webhook)\n  target      Manage message targets (groups, users)\n  send        Send a message to a target\n  route       Manage event routes (message → script)\n  schedule    Manage scheduled tasks\n  history     View event and task run history"
    )]
    Im {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    #[command(
        about = "Run an AI agent (external CLI runner) from the terminal",
        long_about = concat!(
            "Run an AI agent (external CLI runner) interactively from the terminal.\n",
            "\n",
            "The agent executes through the chat-gateway system, using the configured\n",
            "runner and adapter (e.g. codex, chatgpt-web). The result is streamed back\n",
            "and rendered as Markdown in the terminal.\n",
            "\n",
            "If the runner produces images, they are saved to files and their paths\n",
            "are printed as Markdown image links.\n",
            "\n",
            "EXAMPLES:\n",
            "  bifrost agent run \"What is Bifrost?\"\n",
            "  bifrost agent run --runner chatgpt-web \"Draw a cat\"\n",
            "  bifrost agent run --runner chatgpt-web --new \"Hello\"\n",
            "  bifrost agent run --runner chatgpt-web --session my-session \"Continue\"\n",
        )
    )]
    Agent {
        #[command(subcommand)]
        action: AgentCommands,
    },
}

#[derive(Subcommand, Clone)]
pub enum AgentCommands {
    #[command(about = "Run an agent with a message and get the response")]
    Run {
        #[arg(help = "Message to send to the agent")]
        message: String,
        #[arg(long, help = "Runner ID to use (interactive selection if omitted)")]
        runner: Option<String>,
        #[arg(long, help = "Session key for conversation continuity")]
        session: Option<String>,
        #[arg(
            long,
            conflicts_with = "session",
            help = "Force a new conversation (do not reuse existing session)"
        )]
        new: bool,
        #[arg(
            long,
            value_hint = ValueHint::DirPath,
            help = "Directory to save generated images (default: current directory)"
        )]
        output_dir: Option<PathBuf>,
        #[arg(long, help = "Output raw JSON instead of formatted Markdown")]
        json: bool,
    },
    #[command(hide = true, about = "Run isolated built-in agent worker over stdio")]
    Worker,
    #[command(hide = true, about = "Run isolated external runner worker over stdio")]
    ExternalRunnerWorker,
}

#[derive(Subcommand, Clone)]
pub enum AiCommands {
    #[command(about = "Manage Qwen3-ASR speech-to-text")]
    #[cfg_attr(
        not(all(target_os = "macos", target_arch = "aarch64")),
        command(hide = true)
    )]
    Asr {
        #[command(subcommand)]
        action: AiAsrCommands,
    },
    #[command(about = "Run local voice input runtime")]
    #[cfg_attr(
        not(all(target_os = "macos", target_arch = "aarch64")),
        command(hide = true)
    )]
    Voice {
        #[command(subcommand)]
        action: AiVoiceCommands,
    },
}

#[derive(Subcommand, Clone)]
pub enum AiVoiceCommands {
    #[command(about = "List local voice input sources and capability state")]
    Sources {
        #[arg(long, help = "Print JSON")]
        json: bool,
    },
    #[command(about = "Listen to a local voice source and print transcript events")]
    #[cfg_attr(
        not(all(target_os = "macos", target_arch = "aarch64")),
        command(hide = true)
    )]
    Listen {
        #[arg(long, default_value = "mic", value_parser = ["mic", "system", "app", "file"], help = "Voice source kind")]
        source: String,
        #[arg(long, help = "Application name or bundle id for --source app")]
        app: Option<String>,
        #[arg(long, value_hint = ValueHint::FilePath, help = "Audio file to stream in realtime when --source file")]
        input_file: Option<PathBuf>,
        #[arg(long, default_value_t = 5, help = "Capture duration in seconds")]
        duration: u64,
        #[arg(
            long,
            default_value_t = 1000,
            help = "Stateful streaming ASR chunk size in milliseconds"
        )]
        chunk_ms: u64,
        #[arg(
            long,
            default_value = "Qwen3-ASR-0.6B",
            help = "Local ASR model for Voice service"
        )]
        model: String,
        #[arg(
            long,
            default_value = "qwen3_stateful_streaming",
            value_parser = ["qwen3_stateful_streaming"],
            help = "Local Voice ASR provider"
        )]
        provider: String,
        #[arg(long, default_value = "chinese", help = "ASR language")]
        language: String,
        #[arg(long, default_value = "jsonl", value_parser = ["jsonl", "text"], help = "Output format")]
        format: String,
        #[arg(
            long,
            help = "Allow stateful streaming to load large local models such as Qwen3-ASR-1.7B"
        )]
        allow_stateful_large_model: bool,
        #[arg(long, help = "Emit deterministic local events without recording audio")]
        dry_run: bool,
        #[arg(long, default_value = "你好 Bifrost", help = "Dry-run transcript text")]
        text: String,
    },
    #[command(hide = true, about = "Run stateful ASR worker over stdio")]
    Worker {
        #[arg(long)]
        model: String,
        #[arg(long, value_hint = ValueHint::DirPath)]
        model_dir: PathBuf,
        #[arg(long, default_value = "chinese")]
        language: String,
        #[arg(long, default_value_t = 1.0)]
        chunk_size_sec: f32,
        #[arg(long)]
        initial_text: Option<String>,
    },
    #[command(about = "Manage local voice vocabulary")]
    Vocabulary {
        #[command(subcommand)]
        action: AiVoiceVocabularyCommands,
    },
    #[command(about = "Manage local voice wake actions")]
    #[cfg_attr(
        not(all(target_os = "macos", target_arch = "aarch64")),
        command(hide = true)
    )]
    Wake {
        #[command(subcommand)]
        action: AiVoiceWakeCommands,
    },
}

#[derive(Subcommand, Clone)]
pub enum AiVoiceVocabularyCommands {
    #[command(about = "List local voice vocabulary")]
    List {
        #[arg(long, help = "Print JSON")]
        json: bool,
    },
    #[command(about = "Import vocabulary aliases from a text file")]
    Import {
        #[arg(value_hint = ValueHint::FilePath, help = "Terms file, one canonical=alias1,alias2 per line")]
        file: PathBuf,
    },
}

#[derive(Subcommand, Clone)]
pub enum AiVoiceWakeCommands {
    #[command(about = "Show local voice wake status")]
    Status {
        #[arg(long, help = "Print JSON")]
        json: bool,
    },
    #[command(about = "Manage voice wake profiles")]
    Profile {
        #[command(subcommand)]
        action: AiVoiceWakeProfileCommands,
    },
    #[command(about = "Manage trigger phrase bindings")]
    Binding {
        #[command(subcommand)]
        action: AiVoiceWakeBindingCommands,
    },
    #[command(about = "Start or stop the backend voice wake listener")]
    Listener {
        #[command(subcommand)]
        action: AiVoiceWakeListenerCommands,
    },
    #[command(about = "Create a wake binding from one audio sample and one captured shortcut")]
    BindAudio {
        #[arg(
            value_hint = ValueHint::FilePath,
            help = "Wake audio sample for local verification; it is not transcribed"
        )]
        audio: PathBuf,
        #[arg(long, help = "Existing ASR speaker voiceprint profile id")]
        voiceprint_profile_id: String,
        #[arg(
            long,
            help = "Display name for the wake profile; defaults to the voiceprint id"
        )]
        name: Option<String>,
        #[arg(long, help = "Stable wake profile id; generated when omitted")]
        profile_id: Option<String>,
        #[arg(long, help = "Stable binding id; generated when omitted")]
        binding_id: Option<String>,
        #[arg(
            long,
            help = "Wake phrase text to register into sherpa-onnx KWS keywords"
        )]
        phrase: String,
        #[arg(
            long,
            help = "Key name; when omitted, press the shortcut in the terminal"
        )]
        key: Option<String>,
        #[arg(
            long,
            value_delimiter = ',',
            help = "Comma-separated modifiers for --key: cmd,shift,ctrl,option"
        )]
        modifiers: Vec<String>,
        #[arg(long, default_value_t = 1, help = "Number of key presses")]
        press_count: u8,
        #[arg(long, help = "Speaker threshold override")]
        speaker_threshold: Option<f32>,
        #[arg(long, help = "Cooldown after trigger in milliseconds")]
        cooldown_ms: Option<u64>,
        #[arg(long, help = "Print JSON")]
        json: bool,
    },
    #[command(about = "Trigger a wake binding for testing")]
    Trigger {
        #[arg(long, help = "Spoken trigger phrase")]
        phrase: String,
        #[arg(long, help = "Wake profile id that should match")]
        profile: Option<String>,
        #[arg(long, help = "Speaker confidence score for threshold testing")]
        speaker_confidence: Option<f32>,
        #[arg(long, help = "Actually execute the key press instead of dry-run")]
        execute: bool,
        #[arg(long, help = "Print JSON")]
        json: bool,
    },
    #[command(hide = true, about = "Run the voice wake listener worker process")]
    Worker {
        #[arg(long, default_value = "127.0.0.1")]
        admin_host: String,
        #[arg(long)]
        admin_port: u16,
        #[arg(
            long,
            default_value = "lightweight_kws_listener",
            value_parser = ["lightweight_kws_listener"]
        )]
        engine: String,
        #[arg(
            long,
            help = "Backend microphone device for ffmpeg avfoundation capture; defaults to :0"
        )]
        device: Option<String>,
        #[arg(long, default_value_t = 2500)]
        chunk_ms: u64,
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(Subcommand, Clone)]
pub enum AiVoiceWakeListenerCommands {
    #[command(about = "Start backend listening with Bifrost-owned microphone capture")]
    Start {
        #[arg(long, default_value = "mic", value_parser = ["mic", "mock"], help = "Listener audio source; mic uses backend capture")]
        source: String,
        #[arg(
            long,
            default_value = "lightweight_kws_listener",
            value_parser = ["lightweight_kws_listener"],
            help = "Wake listener engine; wake commands use sherpa-onnx lightweight KWS"
        )]
        engine: String,
        #[arg(
            long,
            help = "Backend microphone device for ffmpeg avfoundation capture; defaults to :0"
        )]
        device: Option<String>,
        #[arg(long, help = "Audio chunk size in milliseconds")]
        chunk_ms: Option<u64>,
        #[arg(long, help = "Do not execute matched key press actions")]
        dry_run: bool,
        #[arg(long, value_delimiter = ',', help = "Mock transcripts for tests")]
        mock_transcripts: Vec<String>,
        #[arg(long, help = "Mock transcript interval in milliseconds")]
        mock_interval_ms: Option<u64>,
        #[arg(long, help = "Mock identified speaker profile id")]
        mock_speaker_profile_id: Option<String>,
        #[arg(long, help = "Mock speaker confidence")]
        mock_speaker_confidence: Option<f32>,
        #[arg(long, help = "Print JSON")]
        json: bool,
    },
    #[command(about = "Stop backend voice wake listening")]
    Stop {
        #[arg(long, help = "Print JSON")]
        json: bool,
    },
}

#[derive(Subcommand, Clone)]
pub enum AiVoiceWakeProfileCommands {
    #[command(about = "List voice wake profiles")]
    List {
        #[arg(long, help = "Print JSON")]
        json: bool,
    },
    #[command(about = "Create a voice wake profile")]
    Add {
        #[arg(long, help = "Stable profile id; generated when omitted")]
        id: Option<String>,
        #[arg(long, help = "Display name")]
        name: String,
        #[arg(long, help = "Required linked ASR speaker voiceprint profile id")]
        voiceprint_profile_id: Option<String>,
        #[arg(long, help = "Default speaker match threshold")]
        speaker_threshold: Option<f32>,
        #[arg(long, help = "Print JSON")]
        json: bool,
    },
}

#[derive(Subcommand, Clone)]
pub enum AiVoiceWakeBindingCommands {
    #[command(about = "List voice wake bindings")]
    List {
        #[arg(long, help = "Print JSON")]
        json: bool,
    },
    #[command(about = "Bind one trigger phrase to one key press")]
    Add {
        #[arg(long, help = "Stable binding id; generated when omitted")]
        id: Option<String>,
        #[arg(long, help = "Spoken trigger phrase")]
        phrase: String,
        #[arg(long, help = "Wake profile id")]
        profile: String,
        #[arg(
            long,
            default_value = "space",
            help = "Key name, e.g. space, return, tab, escape, or a single character"
        )]
        key: String,
        #[arg(long, help = "Platform keycode; overrides --key")]
        keycode: Option<u16>,
        #[arg(
            long,
            value_delimiter = ',',
            help = "Comma-separated modifiers: cmd,shift,ctrl,option"
        )]
        modifiers: Vec<String>,
        #[arg(long, default_value_t = 1, help = "Number of key presses")]
        press_count: u8,
        #[arg(long, help = "Create disabled binding")]
        disabled: bool,
        #[arg(long, help = "KWS boosting score recorded for the binding")]
        kws_score: Option<f32>,
        #[arg(long, help = "KWS trigger threshold recorded for the binding")]
        kws_threshold: Option<f32>,
        #[arg(long, help = "Speaker threshold override")]
        speaker_threshold: Option<f32>,
        #[arg(long, help = "Cooldown after trigger in milliseconds")]
        cooldown_ms: Option<u64>,
        #[arg(long, help = "Print JSON")]
        json: bool,
    },
}

#[derive(Subcommand, Clone)]
pub enum AiAsrCommands {
    #[command(about = "Start the local Qwen3-ASR model service")]
    #[cfg_attr(
        not(all(target_os = "macos", target_arch = "aarch64")),
        command(hide = true)
    )]
    Start {
        #[arg(long, default_value = "Qwen3-ASR-0.6B", help = "ASR model name")]
        model: String,
        #[arg(long, default_value = "chinese", help = "Recognition language")]
        language: String,
    },
    #[command(about = "Stop the local Qwen3-ASR model service")]
    #[cfg_attr(
        not(all(target_os = "macos", target_arch = "aarch64")),
        command(hide = true)
    )]
    Stop,
    #[command(about = "Show the local Qwen3-ASR model service status")]
    #[cfg_attr(
        not(all(target_os = "macos", target_arch = "aarch64")),
        command(hide = true)
    )]
    Status {
        #[arg(long, help = "Print JSON")]
        json: bool,
    },
    #[command(about = "Stream-transcribe one audio file to stdout")]
    #[cfg_attr(
        not(all(target_os = "macos", target_arch = "aarch64")),
        command(hide = true)
    )]
    StreamFile {
        #[arg(value_hint = ValueHint::FilePath, help = "Audio file to transcribe")]
        audio: PathBuf,
        #[arg(long, default_value = "Qwen3-ASR-0.6B", help = "ASR model name")]
        model: String,
        #[arg(long, default_value = "chinese", help = "Recognition language")]
        language: String,
        #[arg(
            long,
            help = "Use Bifrost admin speaker diarization and enrolled voiceprints for this file"
        )]
        speaker_aware: bool,
        #[arg(long, default_value = "jsonl", value_parser = ["jsonl"], help = "Output format")]
        format: String,
    },
    #[command(about = "Create offline subtitle artifacts for one audio file")]
    #[cfg_attr(
        not(all(target_os = "macos", target_arch = "aarch64")),
        command(hide = true)
    )]
    Subtitle {
        #[arg(value_hint = ValueHint::FilePath, help = "Audio file to subtitle")]
        audio: PathBuf,
        #[arg(long, default_value = "Qwen3-ASR-0.6B", help = "ASR model name")]
        model: String,
        #[arg(long, default_value = "chinese", help = "Recognition language")]
        language: String,
        #[arg(
            long,
            default_value = "offline-speaker-subtitle-local",
            help = "Speech pipeline profile"
        )]
        profile: String,
        #[arg(long, help = "Enable speaker-aware subtitle timeline")]
        speaker_aware: bool,
        #[arg(
            long,
            value_delimiter = ',',
            default_value = "srt,vtt,txt,timeline_json,metadata",
            help = "Comma-separated output formats: srt,vtt,txt,timeline_json,metadata"
        )]
        format: Vec<String>,
        #[arg(long, value_hint = ValueHint::DirPath, default_value = ".", help = "Output directory")]
        out: PathBuf,
        #[arg(long, help = "Print JSON")]
        json: bool,
    },
    #[command(about = "Inspect and run ASR directory tasks")]
    #[cfg_attr(
        not(all(target_os = "macos", target_arch = "aarch64")),
        command(hide = true)
    )]
    Task {
        #[command(subcommand)]
        action: AiAsrTaskCommands,
    },
    #[command(about = "Initialize and inspect ASR speaker diarization profiles")]
    #[cfg_attr(
        not(all(target_os = "macos", target_arch = "aarch64")),
        command(hide = true)
    )]
    Diarization {
        #[command(subcommand)]
        action: AiAsrDiarizationCommands,
    },
}

#[derive(Subcommand, Clone)]
pub enum AiAsrDiarizationCommands {
    #[command(about = "List available speaker diarization profiles")]
    Profiles {
        #[arg(long, help = "Print JSON")]
        json: bool,
    },
    #[command(about = "Show one speaker diarization profile status")]
    Status {
        #[arg(
            long,
            default_value = "sherpa-onnx-balanced",
            help = "Diarization profile ID"
        )]
        profile: String,
        #[arg(long, help = "Print JSON")]
        json: bool,
    },
    #[command(about = "Initialize local speaker diarization profile assets")]
    Init {
        #[arg(
            long,
            default_value = "sherpa-onnx-balanced",
            help = "Diarization profile ID"
        )]
        profile: String,
        #[arg(long, help = "Print JSON")]
        json: bool,
    },
    #[command(about = "Manage enrolled speaker voiceprints")]
    Speakers {
        #[command(subcommand)]
        action: AiAsrDiarizationSpeakerCommands,
    },
}

#[derive(Subcommand, Clone)]
pub enum AiAsrDiarizationSpeakerCommands {
    #[command(about = "List enrolled speaker voiceprints")]
    List {
        #[arg(long, help = "Print JSON")]
        json: bool,
    },
    #[command(about = "Show one enrolled speaker voiceprint")]
    Show {
        #[arg(help = "Speaker profile ID")]
        profile_id: String,
        #[arg(long, help = "Print JSON")]
        json: bool,
    },
    #[command(about = "Enroll a speaker voiceprint by live prompted reading")]
    EnrollLive {
        #[arg(long, help = "Speaker display name")]
        name: String,
        #[arg(
            long,
            default_value = "sherpa-onnx-balanced",
            help = "Diarization profile ID"
        )]
        profile: String,
        #[arg(long, default_value_t = 4, help = "Seconds to record for each prompt")]
        phrase_seconds: u64,
        #[arg(
            long,
            default_value = ":0",
            help = "Local microphone device for ffmpeg capture"
        )]
        device: String,
        #[arg(long, hide = true, value_hint = ValueHint::FilePath)]
        test_pcm16: Option<PathBuf>,
        #[arg(long, help = "Print JSON")]
        json: bool,
    },
}

#[derive(Subcommand, Clone)]
pub enum AiAsrTaskCommands {
    #[command(about = "Create an ASR directory task")]
    Create {
        #[arg(long, help = "Task display name")]
        name: Option<String>,
        #[arg(long, value_hint = ValueHint::DirPath, help = "Audio directory to watch")]
        dir: PathBuf,
        #[arg(long, default_value = "Qwen3-ASR-0.6B", help = "ASR model name")]
        model: String,
        #[arg(long, default_value = "chinese", help = "Recognition language")]
        language: String,
        #[arg(long, default_value = "reuse_per_file", help = "ASR runtime strategy")]
        runtime_strategy: String,
        #[arg(long, default_value = "02:00", help = "Daily task time in HH:MM")]
        time: String,
        #[arg(long, help = "Disable the task after creation")]
        disabled: bool,
        #[arg(long, help = "Do not scan subdirectories")]
        non_recursive: bool,
        #[arg(long, help = "Disable speaker diarization")]
        no_speaker_diarization: bool,
        #[arg(
            long,
            default_value = "sherpa-onnx-balanced",
            help = "Speaker diarization profile"
        )]
        diarization_profile: String,
        #[arg(long, help = "Expected speaker count for diarization")]
        known_speaker_count: Option<u8>,
        #[arg(long, help = "Disable enrolled voiceprint matching")]
        no_voiceprint_matching: bool,
        #[arg(long, help = "Print JSON")]
        json: bool,
    },
    #[command(about = "List ASR directory tasks")]
    List {
        #[arg(long, help = "Print JSON")]
        json: bool,
    },
    #[command(about = "Show one ASR directory task")]
    Show {
        #[arg(help = "ASR task ID")]
        task_id: String,
        #[arg(long, help = "Print JSON")]
        json: bool,
    },
    #[command(about = "List files discovered by one ASR directory task")]
    Files {
        #[arg(help = "ASR task ID")]
        task_id: String,
        #[arg(long, value_parser = ["pending", "processing", "success", "partial_success", "failed"], help = "Filter by file status")]
        status: Option<String>,
        #[arg(long, default_value = "50", help = "Maximum files to print")]
        limit: usize,
        #[arg(long, help = "Print JSON")]
        json: bool,
    },
    #[command(about = "Run one ASR directory task")]
    Run {
        #[arg(help = "ASR task ID")]
        task_id: String,
        #[arg(long, help = "Wait until the task is no longer running")]
        wait: bool,
        #[arg(long, help = "Print JSON")]
        json: bool,
    },
    #[command(about = "Watch ASR directory task progress in a terminal UI")]
    Watch {
        #[arg(help = "ASR task ID, unique ID prefix, or unique task name")]
        task: Option<String>,
        #[arg(
            long,
            default_value_t = 1000,
            help = "Refresh interval in milliseconds"
        )]
        refresh_ms: u64,
        #[arg(long, help = "Fail instead of opening an interactive task selector")]
        no_interactive_select: bool,
        #[arg(
            long,
            help = "Show all ASR tasks instead of auto-entering the only task"
        )]
        all: bool,
        #[arg(long, help = "Print one watch snapshot as JSON and exit")]
        json_snapshot: bool,
        #[arg(long, help = "Disable run/pause/resume shortcuts")]
        read_only: bool,
    },
    #[command(about = "Alias for `watch`")]
    Tui {
        #[arg(help = "ASR task ID, unique ID prefix, or unique task name")]
        task: Option<String>,
        #[arg(
            long,
            default_value_t = 1000,
            help = "Refresh interval in milliseconds"
        )]
        refresh_ms: u64,
        #[arg(long, help = "Fail instead of opening an interactive task selector")]
        no_interactive_select: bool,
        #[arg(
            long,
            help = "Show all ASR tasks instead of auto-entering the only task"
        )]
        all: bool,
        #[arg(long, help = "Print one watch snapshot as JSON and exit")]
        json_snapshot: bool,
        #[arg(long, help = "Disable run/pause/resume shortcuts")]
        read_only: bool,
    },
    #[command(about = "Inspect daily merged documents for one ASR directory task")]
    Daily {
        #[command(subcommand)]
        action: AiAsrTaskDailyCommands,
    },
}

#[derive(Subcommand, Clone)]
pub enum AiAsrTaskDailyCommands {
    #[command(about = "List daily merged documents for one ASR task")]
    List {
        #[arg(help = "ASR task ID, unique ID prefix, or unique task name")]
        task: Option<String>,
        #[arg(long, help = "Print JSON")]
        json: bool,
    },
    #[command(about = "Show one daily merged document")]
    Show {
        #[arg(help = "Daily document date, or ASR task when DATE is also provided")]
        first: String,
        #[arg(help = "Daily document date for the legacy `<TASK> <DATE>` form")]
        second: Option<String>,
        #[arg(long, help = "ASR task ID, unique ID prefix, or unique task name")]
        task: Option<String>,
        #[arg(long, value_hint = ValueHint::FilePath, help = "Write Markdown content to a file instead of stdout")]
        output: Option<PathBuf>,
        #[arg(long, help = "Print JSON")]
        json: bool,
    },
    #[command(about = "Set or clear the Daily Agent report sync directory")]
    SetSyncDir {
        #[arg(help = "ASR task ID, unique ID prefix, or unique task name")]
        task: Option<String>,
        #[arg(long, value_hint = ValueHint::DirPath, help = "Directory to copy Daily Agent reports into")]
        dir: Option<PathBuf>,
        #[arg(long, help = "Clear the configured report sync directory")]
        clear: bool,
        #[arg(long, help = "Print JSON")]
        json: bool,
    },
    #[command(about = "Manually sync Daily Agent reports to the configured directory")]
    Sync {
        #[arg(help = "ASR task ID, unique ID prefix, or unique task name")]
        task: Option<String>,
        #[arg(long, value_hint = ValueHint::DirPath, help = "Set this sync directory before syncing")]
        dir: Option<PathBuf>,
        #[arg(long, help = "Print JSON")]
        json: bool,
    },
}

#[derive(Subcommand, Clone)]
pub enum AdminCommands {
    #[command(about = "Toggle remote admin access (enable/disable/status)")]
    Remote {
        #[command(subcommand)]
        action: AdminRemoteCommands,
    },
    #[command(about = "Set or change admin password (hidden input)")]
    Passwd {
        #[arg(long, default_value = "admin", help = "Admin username")]
        username: String,
        #[arg(long, help = "Read password from stdin (non-interactive)")]
        password_stdin: bool,
    },
    #[command(about = "Revoke all existing admin sessions")]
    RevokeAll,
    #[command(about = "Read admin login audit logs")]
    Audit {
        #[arg(long, default_value = "50", help = "Maximum records to return")]
        limit: usize,
        #[arg(long, default_value = "0", help = "Offset for pagination")]
        offset: usize,
        #[arg(long, help = "Output JSON")]
        json: bool,
    },
}

#[derive(Subcommand, Clone)]
pub enum AdminRemoteCommands {
    #[command(about = "Enable remote access")]
    Enable,
    #[command(about = "Disable remote access")]
    Disable,
    #[command(about = "Show current remote access status")]
    Status,
}

#[derive(Subcommand, Clone)]
pub enum CaptureCommands {
    #[command(about = "Wait for the next traffic record matching a filter and print it")]
    Wait {
        #[arg(long, help = "Filter: host contains (case-insensitive substring)")]
        host: Option<String>,
        #[arg(long, help = "Filter: HTTP method (e.g. GET, POST)")]
        method: Option<String>,
        #[arg(long, help = "Filter: path contains (substring)")]
        path: Option<String>,
        #[arg(
            long,
            default_value = "60s",
            help = "Timeout duration (e.g. 30s, 2m, 1h, 500ms; max 600s)"
        )]
        timeout: String,
        #[arg(long, help = "URL to open via the OS opener before waiting")]
        open: Option<String>,
        #[arg(
            long,
            default_value = "human",
            value_parser = ["human", "json"],
            help = "Output format: human or json"
        )]
        format: String,
    },
}

#[derive(Subcommand, Clone)]
pub enum TrafficCommands {
    #[command(about = "List traffic records")]
    List {
        #[arg(long, help = "Admin API port (default: global -p or runtime port)")]
        port: Option<u16>,
        #[arg(short, long, default_value = "50", help = "Maximum records to return")]
        limit: usize,
        #[arg(
            long,
            help = "Cursor sequence for pagination (from next_cursor/prev_cursor)"
        )]
        cursor: Option<u64>,
        #[arg(
            long,
            default_value = "backward",
            value_parser = ["backward", "forward"],
            help = "Pagination direction: backward or forward"
        )]
        direction: String,
        #[arg(long, value_parser = ["GET", "POST", "PUT", "DELETE", "PATCH", "HEAD", "OPTIONS", "CONNECT", "TRACE"], help = "Filter by HTTP method")]
        method: Option<String>,
        #[arg(long, help = "Filter by status code (exact)")]
        status: Option<u16>,
        #[arg(long, help = "Filter by status >= value")]
        status_min: Option<u16>,
        #[arg(long, help = "Filter by status <= value")]
        status_max: Option<u16>,
        #[arg(long, value_parser = ["http", "https", "ws", "wss", "h3"], help = "Filter by protocol (http/https/ws/wss/h3)")]
        protocol: Option<String>,
        #[arg(long, help = "Filter host contains")]
        host: Option<String>,
        #[arg(long, help = "Filter URL contains")]
        url: Option<String>,
        #[arg(long, help = "Filter path contains")]
        path: Option<String>,
        #[arg(long, help = "Filter by content type")]
        content_type: Option<String>,
        #[arg(long, help = "Filter by client IP")]
        client_ip: Option<String>,
        #[arg(long, help = "Filter by client app")]
        client_app: Option<String>,
        #[arg(
            long = "listener-port",
            visible_alias = "proxy-port",
            help = "Filter by proxy listener port"
        )]
        listener_port: Option<u16>,
        #[arg(long, help = "Filter by rule hit (true/false)")]
        has_rule_hit: Option<bool>,
        #[arg(long, help = "Filter websocket only (true/false)")]
        is_websocket: Option<bool>,
        #[arg(long, help = "Filter SSE only (true/false)")]
        is_sse: Option<bool>,
        #[arg(long, help = "Filter tunnel only (true/false)")]
        is_tunnel: Option<bool>,
        #[arg(
            short,
            long,
            default_value = "table",
            value_parser = ["table", "compact", "json", "json-pretty"],
            help = "Output format: table, compact, json, json-pretty"
        )]
        format: String,
        #[arg(long, help = "Disable colored output")]
        no_color: bool,
    },
    #[command(about = "Get traffic record details by id (single or --ids batch)")]
    Get {
        #[arg(long, help = "Admin API port (default: global -p or runtime port)")]
        port: Option<u16>,
        #[arg(
            help = "Traffic record id or sequence (optional; prompts if omitted; mutually exclusive with --ids)",
            conflicts_with = "ids"
        )]
        id: Option<String>,
        #[arg(
            long = "ids",
            value_name = "ID,ID,...",
            value_delimiter = ',',
            conflicts_with = "id",
            help = "Batch mode: fetch multiple records in one round-trip (max 200)"
        )]
        ids: Vec<String>,
        #[arg(long, help = "Include request body (best effort)")]
        request_body: bool,
        #[arg(long, help = "Include response body (best effort)")]
        response_body: bool,
        #[arg(
            long = "max-body",
            value_name = "BYTES",
            help = "Batch mode: max bytes per body (default: 65536)"
        )]
        max_body: Option<usize>,
        #[arg(
            short,
            long,
            default_value = "json-pretty",
            value_parser = ["table", "compact", "json", "json-pretty", "ndjson"],
            help = "Output format: table, compact, json, json-pretty, ndjson (ndjson auto-selected for --ids unless overridden)"
        )]
        format: String,
    },
    #[command(about = "Search traffic records (same as `bifrost search`)")]
    Search {
        #[arg(help = "Search keyword (searches URL, headers, body)")]
        keyword: Option<String>,
        #[arg(short, long, help = "Interactive TUI mode (default if no keyword)")]
        interactive: bool,
        #[arg(short, long, default_value = "50", help = "Maximum results to return")]
        limit: usize,
        #[arg(
            short,
            long,
            default_value = "table",
            value_parser = ["table", "compact", "json", "json-pretty", "ndjson"],
            help = "Output format: table, compact, json, json-pretty, ndjson"
        )]
        format: String,
        #[arg(
            long = "req-json",
            value_name = "PATH=VALUE",
            help = "JSONPath filter on request body (repeatable)"
        )]
        req_json: Vec<String>,
        #[arg(
            long = "res-json",
            value_name = "PATH=VALUE",
            help = "JSONPath filter on response body (repeatable)"
        )]
        res_json: Vec<String>,
        #[arg(
            long = "req-header-eq",
            value_name = "NAME=VALUE",
            help = "Request header equals filter (repeatable)"
        )]
        req_header_eq: Vec<String>,
        #[arg(
            long = "res-header-eq",
            value_name = "NAME=VALUE",
            help = "Response header equals filter (repeatable)"
        )]
        res_header_eq: Vec<String>,
        #[arg(
            long,
            value_name = "TIME",
            help = "Restrict to records since this time (RFC3339 or 30s/5m/2h/1d)"
        )]
        since: Option<String>,
        #[arg(
            long,
            value_name = "TIME",
            help = "Restrict to records until this time (RFC3339 or relative)"
        )]
        until: Option<String>,
        #[arg(
            long,
            value_name = "DURATION",
            help = "Shortcut for --since now-DURATION"
        )]
        latest: Option<String>,
        #[arg(long, help = "Search only in URL/path")]
        url: bool,
        #[arg(long, help = "Search only in headers")]
        headers: bool,
        #[arg(long, help = "Search only in body")]
        body: bool,
        #[arg(long = "req-header", help = "Search only in request headers")]
        req_header: bool,
        #[arg(long = "res-header", help = "Search only in response headers")]
        res_header: bool,
        #[arg(long = "req-body", help = "Search only in request body")]
        req_body: bool,
        #[arg(long = "res-body", help = "Search only in response body")]
        res_body: bool,
        #[arg(long, value_parser = ["2xx", "3xx", "4xx", "5xx", "error"], help = "Filter by status: 2xx, 3xx, 4xx, 5xx, error")]
        status: Option<String>,
        #[arg(long, value_parser = ["GET", "POST", "PUT", "DELETE", "PATCH", "HEAD", "OPTIONS", "CONNECT", "TRACE"], help = "Filter by HTTP method: GET, POST, PUT, DELETE, etc.")]
        method: Option<String>,
        #[arg(long, help = "Filter host contains")]
        host: Option<String>,
        #[arg(long, help = "Filter path contains")]
        path: Option<String>,
        #[arg(
            long = "listener-port",
            visible_alias = "proxy-port",
            help = "Filter by proxy listener port"
        )]
        listener_port: Option<u16>,
        #[arg(long, value_parser = ["HTTP", "HTTPS", "WS", "WSS"], help = "Filter by protocol: HTTP, HTTPS, WS, WSS")]
        protocol: Option<String>,
        #[arg(long, value_parser = ["json", "xml", "html", "form", "text", "javascript", "css", "image", "font", "binary"], help = "Filter by content type: json, xml, html, form, etc.")]
        content_type: Option<String>,
        #[arg(long, help = "Filter by domain pattern")]
        domain: Option<String>,
        #[arg(long, help = "Disable colored output")]
        no_color: bool,
        #[arg(
            long = "max-scan",
            default_value = "10000",
            help = "Maximum records to scan (default: 10000, use larger value for broader search)"
        )]
        max_scan: Option<usize>,
        #[arg(
            long = "max-results",
            default_value = "100",
            help = "Maximum matching results to return (default: 100)"
        )]
        max_results: Option<usize>,
        #[arg(
            long = "include",
            value_name = "PARTS",
            value_delimiter = ',',
            help = "Attach extras to each result: request-body, response-body, request-headers, response-headers (aliases: req-body, res-body, req-headers, res-headers; shortcuts: bodies, headers)"
        )]
        include: Vec<String>,
        #[arg(
            long = "max-body",
            value_name = "BYTES",
            help = "Max bytes per body when --include includes a body (default: 65536)"
        )]
        max_body: Option<usize>,
    },
    #[command(about = "Clear traffic records")]
    Clear {
        #[arg(long, help = "Delete specific records by ID (comma-separated)")]
        ids: Option<String>,
        #[arg(short = 'y', long, help = "Skip confirmation prompt")]
        yes: bool,
    },
    #[command(about = "Diagnose JWT / Cookie auth status for a captured request")]
    AuthStatus {
        #[arg(long, help = "Admin API port (default: global -p or runtime port)")]
        port: Option<u16>,
        #[arg(help = "Traffic record id or sequence")]
        id: String,
        #[arg(
            short,
            long,
            default_value = "human",
            value_parser = ["human", "json"],
            help = "Output format: human or json"
        )]
        format: String,
    },
    #[command(about = "Export a captured request as curl/fetch/HAR template")]
    Export {
        #[arg(long, help = "Admin API port (default: global -p or runtime port)")]
        port: Option<u16>,
        #[arg(help = "Traffic record id or sequence suffix")]
        id: String,
        #[arg(long = "as", default_value = "curl", value_parser = ["curl", "fetch", "har"], help = "Export format")]
        as_format: String,
        #[arg(short = 'o', long, value_hint = ValueHint::FilePath, help = "Write output to file")]
        output: Option<PathBuf>,
    },
    #[command(about = "Replay a captured request, optionally with JSON Patch tweaks")]
    Replay {
        #[arg(long, help = "Admin API port (default: global -p or runtime port)")]
        port: Option<u16>,
        #[arg(help = "Traffic record id or sequence suffix")]
        id: String,
        #[arg(
            long,
            help = "JSON Patch shorthand, repeatable: '/path=value' (value auto-detected as number or string)"
        )]
        patch: Vec<String>,
        #[arg(long = "patch-json", help = "Raw RFC6902 JSON array of patch ops")]
        patch_json: Option<String>,
        #[arg(
            long = "refresh-auth",
            help = "Reserved: refresh auth from latest record"
        )]
        refresh_auth: bool,
        #[arg(
            long,
            default_value = "30s",
            help = "Request timeout, e.g. 30s / 1500ms"
        )]
        timeout: String,
        #[arg(short, long, default_value = "human", value_parser = ["human", "json", "json-pretty"], help = "Output format")]
        format: String,
    },
}

#[derive(Subcommand, Clone)]
pub enum RuleCommands {
    #[command(about = "List all rules")]
    List,
    #[command(about = "Show active (enabled) rules summary from running server")]
    Active,
    #[command(about = "Add a new rule")]
    Add {
        #[arg(help = "Rule name")]
        name: String,
        #[arg(short, long, help = "Rule content")]
        content: Option<String>,
        #[arg(short, long, value_hint = ValueHint::FilePath, help = "Rule file path")]
        file: Option<PathBuf>,
        #[arg(
            long,
            help = "Save the rule even when syntax validation returns errors"
        )]
        allow_invalid: bool,
        #[arg(long, help = "Print a machine-readable JSON result")]
        json: bool,
    },
    #[command(about = "Update an existing rule")]
    Update {
        #[arg(help = "Rule name")]
        name: String,
        #[arg(short, long, help = "Rule content")]
        content: Option<String>,
        #[arg(short, long, value_hint = ValueHint::FilePath, help = "Rule file path")]
        file: Option<PathBuf>,
        #[arg(
            long,
            help = "Save the rule even when syntax validation returns errors"
        )]
        allow_invalid: bool,
        #[arg(long, help = "Print a machine-readable JSON result")]
        json: bool,
    },
    #[command(about = "Delete a rule")]
    Delete {
        #[arg(help = "Rule name")]
        name: String,
    },
    #[command(about = "Enable a rule")]
    Enable {
        #[arg(help = "Rule name")]
        name: String,
    },
    #[command(about = "Disable a rule")]
    Disable {
        #[arg(help = "Rule name")]
        name: String,
    },
    #[command(alias = "get", about = "Show rule content")]
    Show {
        #[arg(help = "Rule name")]
        name: String,
    },
    #[command(about = "Create a share URL for a rule")]
    Share {
        #[arg(help = "Rule name")]
        name: String,
        #[arg(help = "Target website URL that will carry the Bifrost rule query")]
        target_url: String,
        #[arg(
            short,
            long,
            help = "Rule content; when omitted, an existing local rule is loaded"
        )]
        content: Option<String>,
        #[arg(short, long, value_hint = ValueHint::FilePath, help = "Rule file path; overrides loading an existing local rule")]
        file: Option<PathBuf>,
        #[arg(long, default_value = "my_rules", value_parser = ["my_rules"], help = "Exclusive enable scope for the imported rule")]
        exclusive_scope: String,
    },
    #[command(about = "Sync rules with remote server")]
    Sync,
    #[command(about = "Rename a rule")]
    Rename {
        #[arg(help = "Current rule name")]
        name: String,
        #[arg(help = "New rule name")]
        new_name: String,
    },
    #[command(about = "Reorder rules priority")]
    Reorder {
        #[arg(num_args = 1.., help = "Rule names in desired order")]
        names: Vec<String>,
    },
}

#[derive(Subcommand, Clone)]
pub enum GroupCommands {
    #[command(about = "List groups")]
    List {
        #[arg(short, long, help = "Search keyword")]
        keyword: Option<String>,
        #[arg(short, long, default_value = "50", help = "Maximum results")]
        limit: usize,
        #[arg(short, long, default_value = "0", help = "Offset for pagination")]
        offset: usize,
    },
    #[command(about = "Show group details")]
    Show {
        #[arg(allow_hyphen_values = true, help = "Group ID")]
        group_id: String,
    },
    #[command(about = "Manage group rules")]
    Rule {
        #[command(subcommand)]
        action: GroupRuleCommands,
    },
}

#[derive(Subcommand, Clone)]
pub enum GroupRuleCommands {
    #[command(about = "List rules in a group")]
    List {
        #[arg(allow_hyphen_values = true, help = "Group ID")]
        group_id: String,
    },
    #[command(alias = "get", about = "Show a group rule")]
    Show {
        #[arg(allow_hyphen_values = true, help = "Group ID")]
        group_id: String,
        #[arg(help = "Rule name")]
        name: String,
    },
    #[command(about = "Add a rule to a group")]
    Add {
        #[arg(allow_hyphen_values = true, help = "Group ID")]
        group_id: String,
        #[arg(help = "Rule name")]
        name: String,
        #[arg(short, long, help = "Rule content")]
        content: Option<String>,
        #[arg(short, long, value_hint = ValueHint::FilePath, help = "Rule file path")]
        file: Option<PathBuf>,
        #[arg(
            long,
            help = "Save the rule even when syntax validation returns errors"
        )]
        allow_invalid: bool,
    },
    #[command(about = "Update a group rule")]
    Update {
        #[arg(allow_hyphen_values = true, help = "Group ID")]
        group_id: String,
        #[arg(help = "Rule name")]
        name: String,
        #[arg(short, long, help = "Rule content")]
        content: Option<String>,
        #[arg(short, long, value_hint = ValueHint::FilePath, help = "Rule file path")]
        file: Option<PathBuf>,
        #[arg(
            long,
            help = "Save the rule even when syntax validation returns errors"
        )]
        allow_invalid: bool,
    },
    #[command(about = "Delete a group rule")]
    Delete {
        #[arg(allow_hyphen_values = true, help = "Group ID")]
        group_id: String,
        #[arg(help = "Rule name")]
        name: String,
    },
    #[command(about = "Enable a group rule")]
    Enable {
        #[arg(allow_hyphen_values = true, help = "Group ID")]
        group_id: String,
        #[arg(help = "Rule name")]
        name: String,
    },
    #[command(about = "Disable a group rule")]
    Disable {
        #[arg(allow_hyphen_values = true, help = "Group ID")]
        group_id: String,
        #[arg(help = "Rule name")]
        name: String,
    },
}

#[derive(Subcommand, Clone)]
pub enum PortCommands {
    #[command(
        about = "Bind a temporary proxy port to explicit rule sets",
        long_about = concat!(
            "Bind a temporary proxy port to explicit rule sets.\n",
            "\n",
            "This creates an extra listener inside the running Bifrost process. The new port\n",
            "shares the same BIFROST_DATA_DIR, values, scripts, certificates, and traffic DB,\n",
            "but it does not inherit the main port's enabled-rule view. Only the rule refs\n",
            "passed here become active on the temporary port.\n",
            "\n",
            "At least one of the following must be provided:\n",
            "  --rule         Bind an existing local rule by name\n",
            "  --group-rule   Bind an existing group rule in <group_id>/<rule_name> format\n",
            "  --rule-file    Bind a rule file directly without writing to shared rules/\n",
            "  --rule-text    Bind inline rule text directly for one-off debugging\n",
            "\n",
            "Use `--port 0` to let Bifrost allocate a free port automatically."
        )
    )]
    Bind {
        #[arg(
            long,
            help = "Temporary proxy port; use 0 to allocate an available port"
        )]
        port: u16,
        #[arg(
            short = 'H',
            long,
            help = "Listen host; defaults to the running proxy host"
        )]
        host: Option<String>,
        #[arg(long, help = "Optional display name")]
        name: Option<String>,
        #[arg(long = "rule", help = "Local rule name to bind; can be repeated")]
        rules: Vec<String>,
        #[arg(
            long = "rule-file",
            value_hint = ValueHint::FilePath,
            help = "Rule file path to bind directly; can be repeated"
        )]
        rule_files: Vec<PathBuf>,
        #[arg(
            long = "rule-text",
            help = "Inline rule content to bind directly; can be repeated"
        )]
        rule_texts: Vec<String>,
        #[arg(
            long = "group-rule",
            help = "Group rule reference in <group_id>/<rule_name> format; can be repeated"
        )]
        group_rules: Vec<String>,
    },
    #[command(
        about = "List temporary proxy port bindings",
        long_about = "List all currently active temporary proxy port bindings created inside the running Bifrost process. Use this to see which extra listeners exist and which ports are already occupied by temporary bindings."
    )]
    List,
    #[command(
        about = "Show a temporary proxy port binding",
        long_about = "Show the persisted binding metadata for one temporary proxy port, including host, optional display name, status, and the explicit rule refs currently attached to that listener."
    )]
    Show {
        #[arg(help = "Temporary proxy port")]
        port: u16,
    },
    #[command(
        about = "Show active rules for a temporary proxy port",
        long_about = "Show the resolved active-rule summary for one temporary proxy port. This is the port-scoped view and should differ from the main port whenever the temporary port binds a different rule set."
    )]
    Active {
        #[arg(help = "Temporary proxy port")]
        port: u16,
    },
    #[command(
        about = "Replace the rule bindings for a temporary proxy port",
        long_about = concat!(
            "Replace the explicit rule refs for a temporary proxy port.\n",
            "\n",
            "Like `port bind`, this command works against the running main proxy process and\n",
            "rebuilds only the selected temporary port's rule view. The main port and other\n",
            "temporary ports are not affected.\n",
            "\n",
            "Use the same rule source flags as `port bind`:\n",
            "  --rule, --group-rule, --rule-file, --rule-text\n",
            "\n",
            "If you need a different mix of rules on a port, provide the full replacement set\n",
            "in this command."
        )
    )]
    Update {
        #[arg(help = "Temporary proxy port")]
        port: u16,
        #[arg(long, help = "Optional display name")]
        name: Option<String>,
        #[arg(long = "rule", help = "Local rule name to bind; can be repeated")]
        rules: Vec<String>,
        #[arg(
            long = "rule-file",
            value_hint = ValueHint::FilePath,
            help = "Rule file path to bind directly; can be repeated"
        )]
        rule_files: Vec<PathBuf>,
        #[arg(
            long = "rule-text",
            help = "Inline rule content to bind directly; can be repeated"
        )]
        rule_texts: Vec<String>,
        #[arg(
            long = "group-rule",
            help = "Group rule reference in <group_id>/<rule_name> format; can be repeated"
        )]
        group_rules: Vec<String>,
    },
    #[command(
        about = "Destroy a temporary proxy port binding",
        long_about = "Destroy a temporary proxy port binding and stop only that listener. Shared rule data, values, scripts, certs, traffic history, and the main proxy port remain unchanged."
    )]
    Destroy {
        #[arg(help = "Temporary proxy port")]
        port: u16,
    },
}

#[derive(Subcommand, Clone)]
pub enum CaCommands {
    #[command(about = "Install and trust CA certificate")]
    Install {
        #[arg(
            long,
            help = "Install the CA to a connected mobile device instead of the local system trust store"
        )]
        mobile: bool,
        #[arg(long, help = "Show iPhone/iPad profile installation and trust steps")]
        ios: bool,
        #[arg(
            long,
            requires = "ios",
            help = "Use macOS Apple Configurator cfgutil to install the iOS profile. Unsupervised devices may still require Trust, unlock, or onscreen confirmation."
        )]
        configurator: bool,
        #[arg(
            long,
            help = "Target Android ADB serial, or iOS USB device id when used with --ios"
        )]
        device: Option<String>,
        #[arg(
            long,
            help = "Skip interactive mobile device selection. With one ready device it is selected automatically; with multiple ready devices pass --device."
        )]
        yes: bool,
    },
    #[command(about = "Generate CA certificate")]
    Generate {
        #[arg(short, long, help = "Force regenerate")]
        force: bool,
    },
    #[command(about = "Export CA certificate")]
    Export {
        #[arg(short, long, value_hint = ValueHint::FilePath, help = "Output path")]
        output: Option<PathBuf>,
    },
    #[command(about = "Show CA certificate info")]
    Info,
}

#[derive(Subcommand, Clone)]
pub enum WhitelistCommands {
    #[command(about = "List current whitelist entries")]
    List,
    #[command(about = "Add IP or CIDR to whitelist")]
    Add {
        #[arg(help = "IP address or CIDR (e.g., 192.168.1.100 or 192.168.1.0/24)")]
        ip_or_cidr: String,
    },
    #[command(about = "Remove IP or CIDR from whitelist")]
    Remove {
        #[arg(help = "IP address or CIDR to remove")]
        ip_or_cidr: String,
    },
    #[command(about = "Enable or disable LAN (private network) access")]
    AllowLan {
        #[arg(value_parser = ["true", "false"], help = "Enable (true) or disable (false) LAN access")]
        enable: String,
    },
    #[command(about = "Show current access control settings")]
    Status,
    #[command(about = "Get or set access control mode")]
    Mode {
        #[arg(value_parser = ["local_only", "whitelist", "interactive", "allow_all"], help = "Set access mode (omit to show current)")]
        mode: Option<String>,
    },
    #[command(about = "List pending access requests")]
    Pending,
    #[command(about = "Approve a pending access request")]
    Approve {
        #[arg(help = "IP address to approve")]
        ip: String,
    },
    #[command(about = "Reject a pending access request")]
    Reject {
        #[arg(help = "IP address to reject")]
        ip: String,
    },
    #[command(about = "Clear all pending access requests")]
    ClearPending,
    #[command(about = "Add a temporary whitelist entry")]
    AddTemporary {
        #[arg(help = "IP address for temporary access")]
        ip: String,
    },
    #[command(about = "Remove a temporary whitelist entry")]
    RemoveTemporary {
        #[arg(help = "IP address to remove from temporary list")]
        ip: String,
    },
}

#[derive(Subcommand, Clone)]
pub enum SystemProxyCommands {
    #[command(about = "Show system proxy status")]
    Status,
    #[command(about = "Enable system proxy")]
    Enable {
        #[arg(long, help = "Bypass list (comma-separated)")]
        bypass: Option<String>,
        #[arg(long, value_hint = ValueHint::Hostname, help = "Proxy host (default: 127.0.0.1)")]
        host: Option<String>,
        #[arg(long, help = "Proxy port (default: global -p)")]
        port: Option<u16>,
    },
    #[command(about = "Disable system proxy")]
    Disable,
    #[cfg(target_os = "macos")]
    #[command(about = "Manage macOS LaunchDaemon cleanup fallback")]
    Launchd {
        #[command(subcommand)]
        action: SystemProxyLaunchdCommands,
    },
    #[command(hide = true, about = "Run system proxy lifecycle cleanup helper")]
    LifecycleHelper {
        #[arg(long, value_hint = ValueHint::DirPath)]
        data_dir: PathBuf,
        #[arg(long)]
        parent_pid: Option<u32>,
        #[arg(long)]
        parent_started_at_ms: Option<u64>,
        #[arg(long, default_value_t = 2)]
        poll_secs: u64,
    },
    #[command(hide = true, about = "Clean stale Bifrost-managed system proxy state")]
    Cleanup {
        #[arg(long, value_hint = ValueHint::DirPath)]
        data_dir: Option<PathBuf>,
    },
    #[cfg(target_os = "macos")]
    #[command(hide = true, about = "Repair macOS system proxy lock permissions")]
    RepairLock {
        #[arg(long, value_hint = ValueHint::DirPath)]
        data_dir: PathBuf,
    },
    #[cfg(target_os = "macos")]
    #[command(hide = true, about = "Run macOS system proxy launchd cleanup daemon")]
    CleanupDaemon {
        #[arg(long, value_hint = ValueHint::DirPath)]
        data_dir: PathBuf,
        #[arg(long)]
        installed_version: Option<String>,
    },
}

#[cfg(target_os = "macos")]
#[derive(Subcommand, Clone)]
pub enum SystemProxyLaunchdCommands {
    #[command(about = "Show macOS LaunchDaemon cleanup fallback status")]
    Status {
        #[arg(long, help = "LaunchDaemon label")]
        label: Option<String>,
        #[arg(long, value_hint = ValueHint::FilePath, help = "LaunchDaemon plist path")]
        plist_path: Option<PathBuf>,
    },
    #[command(about = "Install or repair macOS LaunchDaemon cleanup fallback")]
    Install {
        #[arg(long, value_hint = ValueHint::DirPath, help = "Bifrost data directory to clean")]
        data_dir: Option<PathBuf>,
        #[arg(long, value_hint = ValueHint::FilePath, help = "Bifrost executable path")]
        program: Option<PathBuf>,
        #[arg(long, help = "LaunchDaemon label")]
        label: Option<String>,
        #[arg(long, value_hint = ValueHint::FilePath, help = "LaunchDaemon plist path")]
        plist_path: Option<PathBuf>,
        #[arg(long, help = "Print the plist without installing it")]
        dry_run: bool,
    },
    #[command(about = "Uninstall macOS LaunchDaemon cleanup fallback")]
    Uninstall {
        #[arg(long, help = "LaunchDaemon label")]
        label: Option<String>,
        #[arg(long, value_hint = ValueHint::FilePath, help = "LaunchDaemon plist path")]
        plist_path: Option<PathBuf>,
    },
}

#[derive(Subcommand, Clone)]
pub enum ValueCommands {
    #[command(about = "List all values")]
    List,
    #[command(alias = "get", about = "Show a value by name")]
    Show {
        #[arg(help = "Value name")]
        name: String,
    },
    #[command(alias = "set", about = "Add a value")]
    Add {
        #[arg(help = "Value name")]
        name: String,
        #[arg(help = "Value content")]
        value: String,
    },
    #[command(about = "Update an existing value")]
    Update {
        #[arg(help = "Value name")]
        name: String,
        #[arg(help = "New value content")]
        value: String,
    },
    #[command(about = "Delete a value")]
    Delete {
        #[arg(help = "Value name")]
        name: String,
    },
    #[command(about = "Import values from file")]
    Import {
        #[arg(value_hint = ValueHint::FilePath, help = "File path (supports .txt, .kv, .json)")]
        file: PathBuf,
    },
}

#[derive(Subcommand, Clone)]
pub enum ScriptCommands {
    #[command(about = "List all scripts")]
    List {
        #[arg(short, long, value_parser = ["request", "response", "decode"], help = "Filter by type: request, response, decode")]
        r#type: Option<String>,
    },
    #[command(about = "Add or update a script")]
    Add {
        #[arg(value_parser = ["request", "response", "decode"], help = "Script type: request, response, decode")]
        r#type: String,
        #[arg(help = "Script name")]
        name: String,
        #[arg(short, long, help = "Script content (JavaScript)")]
        content: Option<String>,
        #[arg(short, long, value_hint = ValueHint::FilePath, help = "Script file path (.js)")]
        file: Option<PathBuf>,
    },
    #[command(about = "Update an existing script")]
    Update {
        #[arg(value_parser = ["request", "response", "decode"], help = "Script type: request, response, decode")]
        r#type: String,
        #[arg(help = "Script name")]
        name: String,
        #[arg(short, long, help = "Script content (JavaScript)")]
        content: Option<String>,
        #[arg(short, long, value_hint = ValueHint::FilePath, help = "Script file path (.js)")]
        file: Option<PathBuf>,
    },
    #[command(about = "Delete a script")]
    Delete {
        #[arg(value_parser = ["request", "response", "decode"], help = "Script type: request, response, decode")]
        r#type: String,
        #[arg(help = "Script name")]
        name: String,
    },
    #[command(alias = "get", about = "Show script content")]
    Show {
        #[arg(
            value_name = "TYPE_OR_NAME",
            num_args = 1..=2,
            help = "Script type + name, or just name for fuzzy match"
        )]
        args: Vec<String>,
    },
    #[command(about = "Run a script test using built-in mock input")]
    Run {
        #[arg(
            value_name = "TYPE_OR_NAME",
            num_args = 1..=2,
            help = "Script type + name, or just name for fuzzy match"
        )]
        args: Vec<String>,
    },
    #[command(about = "Rename a script")]
    Rename {
        #[arg(value_parser = ["request", "response", "decode"], help = "Script type: request, response, decode")]
        r#type: String,
        #[arg(help = "Current script name")]
        name: String,
        #[arg(help = "New script name")]
        new_name: String,
    },
}

#[derive(Subcommand, Clone)]
pub enum ConfigCommands {
    #[command(about = "Show configuration (default when no subcommand provided)")]
    Show {
        #[arg(long, help = "Output in JSON format")]
        json: bool,
        #[arg(long, value_parser = ["server", "tls", "traffic", "access"], help = "Show specific section: server, tls, traffic, access")]
        section: Option<String>,
    },
    #[command(about = "Get a configuration value")]
    Get {
        #[arg(help = "Configuration key (e.g., tls.enabled, traffic.max-records)")]
        key: String,
        #[arg(long, help = "Output in JSON format")]
        json: bool,
    },
    #[command(about = "Set a configuration value")]
    Set {
        #[arg(help = "Configuration key")]
        key: String,
        #[arg(help = "Value to set (use comma-separated for lists)")]
        value: String,
    },
    #[command(about = "Add item to a list configuration")]
    Add {
        #[arg(help = "Configuration key (must be a list type, e.g., tls.exclude)")]
        key: String,
        #[arg(help = "Value to add")]
        value: String,
    },
    #[command(about = "Remove item from a list configuration")]
    Remove {
        #[arg(help = "Configuration key (must be a list type)")]
        key: String,
        #[arg(help = "Value to remove")]
        value: String,
    },
    #[command(about = "Reset a configuration to default value")]
    Reset {
        #[arg(help = "Configuration key (use 'all' to reset everything)")]
        key: String,
        #[arg(short = 'y', long, help = "Skip confirmation prompt")]
        yes: bool,
    },
    #[command(about = "Clear all caches (body, traffic, frame)")]
    ClearCache {
        #[arg(short = 'y', long, help = "Skip confirmation prompt")]
        yes: bool,
    },
    #[command(about = "Disconnect connections by domain pattern")]
    Disconnect {
        #[arg(help = "Domain pattern to match")]
        domain: String,
    },
    #[command(about = "Export configuration to file")]
    Export {
        #[arg(short, long, value_hint = ValueHint::FilePath, help = "Output file path (default: stdout)")]
        output: Option<PathBuf>,
        #[arg(long, default_value = "toml", value_parser = ["json", "toml"], help = "Export format: json, toml")]
        format: String,
    },
    #[command(about = "Disconnect connections by application name")]
    DisconnectByApp {
        #[arg(help = "Application name")]
        app: String,
    },
    #[command(about = "Show performance overview (body store, frame store, sandbox)")]
    Performance,
    #[command(about = "Show active WebSocket connections")]
    Websocket,
    #[command(about = "List active proxy connections")]
    Connections,
    #[command(about = "Show memory diagnostics (process RSS, caches, connection stats)")]
    Memory,
}

#[derive(Subcommand, Clone)]
pub enum MetricsCommands {
    #[command(about = "Show metrics summary (default)")]
    Summary,
    #[command(about = "Show per-application traffic metrics")]
    Apps,
    #[command(about = "Show per-host traffic metrics")]
    Hosts,
    #[command(about = "Show metrics history")]
    History {
        #[arg(short, long, help = "Maximum history entries to show")]
        limit: Option<usize>,
    },
}

#[derive(Subcommand, Clone)]
pub enum SyncCommands {
    #[command(about = "Show sync status")]
    Status,
    #[command(
        about = "Login to sync service",
        long_about = concat!(
            "Login to the sync service.\n",
            "\n",
            "Without options, opens the browser login flow. For CI or headless\n",
            "environments, pass a token directly. If --url is omitted, Bifrost\n",
            "uses the configured sync remote URL, which defaults to the built-in\n",
            "Bifrost provider.\n",
            "\n",
            "Get a token from:\n",
            "  https://bifrost.bytedance.net/v4/sso/token-login\n",
            "\n",
            "EXAMPLES:\n",
            "  bifrost sync login\n",
            "  bifrost sync login --token \"$BIFROST_SYNC_TOKEN\"\n",
            "  bifrost sync login --token \"$BIFROST_SYNC_TOKEN\" --url https://bifrost.bytedance.net",
        )
    )]
    Login {
        #[arg(
            long,
            value_name = "TOKEN",
            num_args = 0..=1,
            default_missing_value = "",
            help = "Sync session token for non-interactive login; get one at https://bifrost.bytedance.net/v4/sso/token-login"
        )]
        token: Option<String>,
        #[arg(
            long,
            value_hint = ValueHint::Url,
            help = "Remote sync URL for non-interactive login (default: configured sync remote URL, built-in provider if unchanged)"
        )]
        url: Option<String>,
    },
    #[command(about = "Logout from sync service")]
    Logout,
    #[command(about = "Trigger manual sync")]
    Run,
    #[command(about = "View or update sync configuration")]
    Config {
        #[arg(long, help = "Enable or disable sync")]
        enabled: Option<bool>,
        #[arg(long, help = "Enable or disable auto-sync")]
        auto_sync: Option<bool>,
        #[arg(long, value_hint = ValueHint::Url, help = "Set remote sync URL")]
        remote_url: Option<String>,
    },
}

#[derive(Subcommand, Clone)]
pub enum ExportCommands {
    #[command(about = "Export rules to .bifrost file")]
    Rules {
        #[arg(num_args = 1.., help = "Rule names to export")]
        names: Vec<String>,
        #[arg(short, long, help = "Description for the export")]
        description: Option<String>,
        #[arg(short, long, value_hint = ValueHint::FilePath, help = "Output file path (default: stdout)")]
        output: Option<PathBuf>,
    },
    #[command(about = "Export values to .bifrost file")]
    Values {
        #[arg(help = "Value names to export (default: all)")]
        names: Vec<String>,
        #[arg(short, long, help = "Description for the export")]
        description: Option<String>,
        #[arg(short, long, value_hint = ValueHint::FilePath, help = "Output file path (default: stdout)")]
        output: Option<PathBuf>,
    },
    #[command(about = "Export scripts to .bifrost file")]
    Scripts {
        #[arg(num_args = 1.., help = "Script names to export (format: type/name)")]
        names: Vec<String>,
        #[arg(short, long, help = "Description for the export")]
        description: Option<String>,
        #[arg(short, long, value_hint = ValueHint::FilePath, help = "Output file path (default: stdout)")]
        output: Option<PathBuf>,
    },
}

pub mod remote;
pub use remote::*;

pub struct ImportArgs {
    pub file: PathBuf,
    pub detect_only: bool,
}
