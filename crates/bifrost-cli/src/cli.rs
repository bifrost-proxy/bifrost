use std::path::PathBuf;

use clap::{ArgAction, Parser, Subcommand, ValueHint};
use clap_complete::Shell;

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

────────────────────────────────────────────────────────────────────────────
SUBCOMMAND REFERENCE
────────────────────────────────────────────────────────────────────────────

start [OPTIONS]                   Start the proxy server (default)
  -p, --port <PORT>                   Unified proxy port for HTTP/HTTPS/SOCKS5
  -H, --host <HOST>                   Listen address (overrides global -H)
  --socks5-port <PORT>                Separate SOCKS5 port (optional; default: share main port)
  -d, --daemon                        Run as background daemon
  --skip-cert-check                   Skip CA certificate check
  --access-mode <MODE>                Access mode: local_only|whitelist|interactive|allow_all
  --whitelist <IPS>                   Client IP whitelist (comma-separated, supports CIDR)
  --allow-lan                         Allow LAN (private network) clients
  --proxy-user <USER:PASS>            Proxy user credentials (can be repeated)
  --intercept                         Enable TLS/HTTPS interception
  --no-intercept                      Disable TLS/HTTPS interception
  --intercept-exclude <DOMAINS>       Exclude domains from interception (supports wildcards)
  --intercept-include <DOMAINS>       Force intercept domains (highest priority)
  --app-intercept-exclude <APPS>      Exclude apps from TLS interception (supports wildcards)
  --app-intercept-include <APPS>      Force intercept apps (highest priority)
  --unsafe-ssl                        Skip upstream TLS verification (dangerous)
  --enable-badge-injection            Enable injecting Bifrost badge into HTML pages
  --disable-badge-injection           Disable injecting Bifrost badge into HTML pages
  --no-disconnect-on-config-change    Disable auto-disconnect when TLS config changes
  --rules <RULE>                      Proxy rule, e.g. host:// or http3:// (can be repeated)
  --rules-file <PATH>                 Path to rules file
  --system-proxy                      Enable system proxy
  --proxy-bypass <LIST>               System proxy bypass list
  --cli-proxy                         Enable CLI proxy env vars while proxy is running
  --cli-proxy-no-proxy <LIST>         CLI proxy no-proxy list
  -y, --yes                           Automatically answer yes to prompts

  TLS Interception Priority (highest to lowest):
    1. Rule-based (tlsIntercept://, tlsPassthrough://)
    2. --intercept-include / --app-intercept-include: Always intercept
    3. --intercept-exclude / --app-intercept-exclude: Never intercept
    4. --intercept / --no-intercept: Global switch (default: disabled)

stop                              Stop the running proxy

status (alias: st)                Show proxy status
  --tui                              Show interactive TUI dashboard

rule <ACTION>                     Manage rules
  list                              List all rules
  active                            Show active (enabled) rules summary from running server
  add <name> [-c content|-f file]   Add a new rule
  update <name> [-c content|-f file] Update an existing rule
  enable <name>                     Enable a rule
  disable <name>                    Disable a rule
  show <name>                       Show rule content
  delete <name>                     Delete a rule
  sync                              Sync rules with remote server
  rename <name> <new_name>          Rename a rule
  reorder <name1> <name2> ...       Reorder rules priority

group <ACTION>                    Manage groups and group rules
  list [-k keyword] [-l limit]      List groups
  show <group_id>                   Show group details
  rule list <group_id>              List rules in a group
  rule show <group_id> <name>       Show a group rule
  rule add <group_id> <name> [-c content|-f file]  Add a group rule
  rule update <group_id> <name> [-c content|-f file] Update a group rule
  rule delete <group_id> <name>     Delete a group rule
  rule enable <group_id> <name>     Enable a group rule
  rule disable <group_id> <name>    Disable a group rule

ca <ACTION>                       Manage CA certificates
  install                           Install and trust CA certificate
  info                              Show CA certificate info
  export [-o path]                  Export CA certificate
  generate [-f]                     Generate CA certificate

system-proxy <ACTION> (alias: sp)  Manage system proxy
  status                            Show system proxy status
  enable [--host h] [--port p] [--bypass list]
                                    Enable system proxy
  disable                           Disable system proxy


whitelist <ACTION> (alias: wl)    Manage access control
  list                              List whitelist entries
  add <ip>                          Add IP/CIDR to whitelist
  remove <ip>                       Remove IP/CIDR from whitelist
  allow-lan <true|false>            Enable/disable LAN access
  status                            Show access control settings
  mode [MODE]                       Get or set access mode (local_only|whitelist|interactive|allow_all)
  pending                           List pending access requests
  approve <ip>                      Approve a pending request
  reject <ip>                       Reject a pending request
  clear-pending                     Clear all pending requests
  add-temporary <ip>                Add temporary whitelist entry
  remove-temporary <ip>             Remove temporary whitelist entry

value <ACTION> (alias: val)       Manage values for variable expansion
  list                              List all values
  show|get <name>                   Show a value
  add|set <name> <value>            Add a value
  update <name> <value>             Update a value
  delete <name>                     Delete a value
  import <file>                     Import from file (.txt/.kv/.json)

script <ACTION>                   Manage scripts (request/response/decode)
  list [-t type]                    List all scripts (optionally filter by type)
  add <type> <name> [-c content|-f file]  Add or update a script
  update <type> <name> [-c content|-f file] Update an existing script
  show|get [type] <name>            Show script content; with one arg, fuzzy match by name
  run [type] <name>                 Run a script test and print output + logs
  delete <type> <name>              Delete a script
  rename <type> <name> <new_name>   Rename a script

upgrade [OPTIONS]                 Upgrade bifrost to the latest version
  -y, --yes                         Skip confirmation prompt
  --restart                         Automatically restart the running proxy after upgrade

config [ACTION] (alias: cfg)      Manage runtime configuration
  show [--json] [--section <SECTION>]  Show configuration (default)
  get <key> [--json]                  Get a configuration value (e.g., tls.enabled)
  set <key> <value>                   Set a configuration value
  add <key> <value>                   Add item to a list configuration
  remove <key> <value>                Remove item from a list configuration
  reset <key|all> [-y|--yes]           Reset a configuration to default value
  clear-cache [-y|--yes]               Clear all caches (body, traffic, frame)
  disconnect <domain>                  Disconnect connections by domain pattern
  disconnect-by-app <app>              Disconnect connections by application
  export [-o path] [--format json|toml] Export configuration to file
  performance                          Show performance overview
  websocket                            Show active WebSocket connections
  connections                          List active proxy connections
  memory                               Show memory diagnostics

traffic <ACTION>                  Inspect and query traffic records
  list [OPTIONS]                     List traffic records
  get [id] [OPTIONS]                 Get traffic record details by id/sequence
  search [keyword] [OPTIONS]         Search traffic records (same as `bifrost search`)
  clear [--ids id1,id2] [-y]         Clear traffic records (all or by IDs)

search [keyword] [OPTIONS]         Search traffic records with advanced filtering
  -i, --interactive                   Interactive TUI mode (default if no keyword)
  -l, --limit <N>                     Maximum results to return (default: 50)
  -f, --format <FMT>                  Output format: table|compact|json|json-pretty
  --url                               Search only in URL/path
  --headers|--body                    Search in both request+response headers or bodies
  --req-header|--res-header           Search only in request or response headers
  --req-body|--res-body               Search only in request or response body
  --status <FILTER>                   Status: 2xx|3xx|4xx|5xx|error
  --method <METHOD>                   HTTP method filter
  --host <TEXT>                       Host contains filter
  --path <TEXT>                       Path contains filter
  --protocol <PROTO>                  Protocol: HTTP|HTTPS|WS|WSS
  --content-type <TYPE>               Content-Type filter (json/xml/html/form/text...)
  --domain <PATTERN>                  Domain pattern filter
  --max-scan <N>                      Maximum records to scan (default: 10000)
  --max-results <N>                   Maximum matching results to return (default: 100)
  --no-color                          Disable colored output

completions <SHELL> (alias: comp)  Generate shell completion scripts
  SHELL: bash, zsh, fish, elvish, powershell

metrics <ACTION>                  View real-time metrics and statistics
  summary                           Show metrics summary (default)
  apps                              Show per-application traffic metrics
  hosts                             Show per-host traffic metrics
  history [-l limit]                Show metrics history

sync <ACTION>                     Manage remote sync
  status                            Show sync status
  login                             Login to sync service
  logout                            Logout from sync service
  run                               Trigger manual sync
  config [--enabled] [--auto-sync] [--remote-url]  View or update sync config

import <file> [--detect-only]     Import a .bifrost file
export <ACTION>                   Export to .bifrost file
  rules <names...> [-d desc] [-o path]   Export rules
  values [names...] [-d desc] [-o path]  Export values
  scripts <names...> [-d desc] [-o path] Export scripts

version-check                     Check for new version without upgrading

install-skill [OPTIONS]           Install SKILL.md to AI coding tools
  -t, --tool <TOOL>                   Target: claude-code, codex, trae, cursor, github-copilot, universal, all
  -d, --dir <PATH>                    Custom install directory
  --cwd                               Install to current directory (project-level)
  -y, --yes                           Skip confirmation prompt

TIP:
    Use 'bifrost <command> -h' for the full list of options for any subcommand.

COMMAND SHORTCUTS:
    st    → status          cfg   → config
    sp    → system-proxy    val   → value
    wl    → whitelist       comp  → completions

────────────────────────────────────────────────────────────────────────────
ENVIRONMENT VARIABLES
────────────────────────────────────────────────────────────────────────────

BIFROST_DATA_DIR                  Custom data directory path
                                  Default: ~/.bifrost (platform-specific)
                                  Contains: config, rules, values, certs, logs
                                  Example: BIFROST_DATA_DIR=/tmp/bifrost-test bifrost

RUST_LOG                          Control logging output level and filters
                                  Default: info (set via -l/--log-level)
                                  Example: RUST_LOG=debug bifrost
                                  Advanced: RUST_LOG=bifrost_proxy=debug,info

────────────────────────────────────────────────────────────────────────────
RULE TEMPLATE VARIABLES
────────────────────────────────────────────────────────────────────────────

Rules support variable expansion using ${...} syntax:

  ${name}                         Expand to value stored via 'bifrost value set'
  ${env.VAR_NAME}                 Expand to environment variable VAR_NAME

Example rule with variables:
  example.com host://${LOCAL_SERVER}
  api.example.com reqHeaders://(Authorization: ${env.API_TOKEN})

Manage values:
  bifrost value set LOCAL_SERVER 127.0.0.1:3000
  bifrost value list

────────────────────────────────────────────────────────────────────────────
RULES QUICK START
────────────────────────────────────────────────────────────────────────────

Basic syntax:
  pattern operation [operations...] [filters...] [lineProps://...]

Pattern types (auto-detected):
  Domain            example.com  example.com/api
  IP/CIDR           192.168.1.1  192.168.0.0/16
  Regex             /pattern/    /pattern/i
  Wildcard          *.example.com  api?  $host
  Negation          !*.example.com

Common operations (examples):
  example.com host://127.0.0.1:3000              Forward to upstream
  example.com proxy://127.0.0.1:8080             Chain to another proxy
  example.com reqHeaders://X-Test=1&X-Foo=bar     Inject request headers
  example.com resHeaders://X-Debug=1              Inject response headers
  example.com statusCode://404                    Override status code
  example.com file://({\"ec\":0,\"data\":null})   Mock response body
  chatgpt.com http3://                            Enable upstream HTTP/3 attempts
  api.example.com h3://                           Alias of http3://

Filters and rule props:
  includeFilter://m:GET           Only apply to GET
  excludeFilter:///admin/         Exclude paths matching /admin/
  lineProps://important           Higher priority
  lineProps://disabled            Disable a rule

How to apply rules:
  bifrost start --rules \"example.com host://127.0.0.1:3000\"
  bifrost start --rules \"chatgpt.com http3://\"
  bifrost start --rules-file ./rules.txt
  bifrost rule add demo -c \"example.com reqHeaders://X-Bifrost=1\"

Verify with curl:
  curl -x http://127.0.0.1:9900 http://httpbin.org/headers
  curl -x http://127.0.0.1:9900 https://httpbin.org/headers -k
  # If you installed Bifrost CA, prefer:
  # curl -x http://127.0.0.1:9900 https://httpbin.org/headers --cacert <path-to-bifrost-ca.pem>

More docs:
  https://github.com/bifrost-proxy/bifrost/tree/main/docs
  https://github.com/bifrost-proxy/bifrost/blob/main/docs/rule.md
  https://github.com/bifrost-proxy/bifrost/blob/main/docs/operation.md
  https://github.com/bifrost-proxy/bifrost/blob/main/docs/pattern.md
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
        default_value = "console",
        value_parser = ["console", "file", "console,file"],
        help = "Log output targets: console, file, or both (comma-separated)"
    )]
    pub log_output: String,

    #[arg(long, value_hint = ValueHint::DirPath, help = "Log file directory (default: <data_dir>/logs)")]
    pub log_dir: Option<PathBuf>,

    #[arg(long, default_value = "7", help = "Number of days to retain log files")]
    pub log_retention_days: u32,
}

#[derive(Subcommand)]
pub enum Commands {
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
    },
    #[command(about = "Stop the proxy server")]
    Stop,
    #[command(visible_alias = "st", about = "Show proxy server status")]
    Status {
        #[arg(short, long, help = "Show interactive TUI dashboard")]
        tui: bool,
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
    #[command(about = "Inspect and query traffic records")]
    Traffic {
        #[command(subcommand)]
        action: TrafficCommands,
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
            value_parser = ["table", "compact", "json", "json-pretty"],
            help = "Output format: table, compact, json, json-pretty"
        )]
        format: String,
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
    #[command(about = "Execute commands on a remote Bifrost instance via relay")]
    Remote {
        #[command(subcommand)]
        action: RemoteCommands,
        #[arg(
            long,
            global = true,
            help = "Relay server URL (default: read from sync config)"
        )]
        relay_url: Option<String>,
        #[arg(
            long,
            global = true,
            help = "Target client instance ID prefix (for selecting among saved connections)"
        )]
        client_id: Option<String>,
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
pub enum TrafficCommands {
    #[command(about = "List traffic records")]
    List {
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
    #[command(about = "Get traffic record details by id")]
    Get {
        #[arg(help = "Traffic record id or sequence (optional; prompts if omitted)")]
        id: Option<String>,
        #[arg(long, help = "Include request body (best effort)")]
        request_body: bool,
        #[arg(long, help = "Include response body (best effort)")]
        response_body: bool,
        #[arg(
            short,
            long,
            default_value = "json-pretty",
            value_parser = ["table", "compact", "json", "json-pretty"],
            help = "Output format: table, compact, json, json-pretty"
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
            value_parser = ["table", "compact", "json", "json-pretty"],
            help = "Output format: table, compact, json, json-pretty"
        )]
        format: String,
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
    },
    #[command(about = "Clear traffic records")]
    Clear {
        #[arg(long, help = "Delete specific records by ID (comma-separated)")]
        ids: Option<String>,
        #[arg(short = 'y', long, help = "Skip confirmation prompt")]
        yes: bool,
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
    },
    #[command(about = "Update an existing rule")]
    Update {
        #[arg(help = "Rule name")]
        name: String,
        #[arg(short, long, help = "Rule content")]
        content: Option<String>,
        #[arg(short, long, value_hint = ValueHint::FilePath, help = "Rule file path")]
        file: Option<PathBuf>,
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
pub enum CaCommands {
    #[command(about = "Install and trust CA certificate")]
    Install,
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
    #[command(about = "Login to sync service")]
    Login,
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

#[derive(Subcommand, Clone, Debug)]
pub enum RemoteCommands {
    #[command(about = "Connect to a remote Bifrost instance (first-time pairing)")]
    Connect {
        #[arg(help = "Pair code displayed on the remote device")]
        pair_code: String,
    },
    #[command(
        about = "Revoke authorization for a remote Bifrost instance",
        long_about = "Revoke authorization for a remote Bifrost instance.\n\n\
            Without flags, revokes all grants for the target client (resolved via --client-id).\n\
            Use --all to revoke grants for every connected client at once.\n\
            Use --grant-id to revoke a single specific grant."
    )]
    Disconnect {
        #[arg(long, help = "Revoke grants for ALL clients (no --client-id needed)")]
        all: bool,
        #[arg(long, help = "Revoke a single specific grant by ID")]
        grant_id: Option<String>,
    },
    #[command(about = "Get remote proxy status")]
    Status,
    #[command(about = "Search remote traffic records")]
    Search {
        #[arg(help = "Search keyword")]
        keyword: String,
        #[arg(short, long, default_value = "50", help = "Maximum results")]
        limit: usize,
    },
    #[command(about = "Inspect remote traffic records")]
    Traffic {
        #[command(subcommand)]
        action: RemoteTrafficCommands,
    },
}

#[derive(Subcommand, Clone, Debug)]
pub enum RemoteTrafficCommands {
    #[command(about = "List remote traffic records")]
    List {
        #[arg(short, long, default_value = "50", help = "Maximum records")]
        limit: usize,
        #[arg(long, help = "Cursor for pagination")]
        cursor: Option<u64>,
        #[arg(long, help = "Filter by HTTP method")]
        method: Option<String>,
        #[arg(long, help = "Filter by status code")]
        status: Option<u16>,
    },
    #[command(about = "Get remote traffic record details")]
    Get {
        #[arg(help = "Traffic record ID")]
        id: String,
        #[arg(long, help = "Include request body")]
        request_body: bool,
        #[arg(long, help = "Include response body")]
        response_body: bool,
    },
    #[command(about = "Search remote traffic records")]
    Search {
        #[arg(help = "Search keyword")]
        keyword: String,
        #[arg(short, long, default_value = "50", help = "Maximum results")]
        limit: usize,
    },
}

pub struct ImportArgs {
    pub file: PathBuf,
    pub detect_only: bool,
}
