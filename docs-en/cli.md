> Language: **English** | [中文](../docs/cli.md)

# CLI Command Reference

This reference explains the top-level `bifrost` commands, major subcommands, environment variables, and rule template variables. For task-oriented usage, read [CLI quick start](./cli-quick-start.md).

## Help and Documentation

```bash
bifrost --help
bifrost <command> --help
bifrost <command> <subcommand> --help
```

The top-level help intentionally stays short. Exact flag parsing is defined by each command's `--help`; this document explains intent, side effects, and combinations. Commands that modify external state, such as system proxy settings, shell rc files, remote shell execution, cache cleanup, and traffic cleanup, are called out in their relevant sections.

## Command Index

| Command | Purpose |
| --- | --- |
| `start` | Start the HTTP/HTTPS/SOCKS5 proxy with TLS, system proxy, rules, and access control options. |
| `stop`, `restart`, `status` | Manage the service lifecycle. |
| `rule` | Add, update, enable, disable, reorder, and inspect local rules. |
| `group` | Manage remote or shared groups and group rules. |
| `port` | Bind temporary proxy ports to explicit rule sets. |
| `ca` | Generate, install, export, and inspect the Bifrost CA. |
| `whitelist` | Manage local access control, pending approvals, and temporary allow rules. |
| `system-proxy` | Enable, disable, or inspect the OS system proxy. |
| `cli-proxy` | Install or remove proxy and CA environment variables in shell profiles. |
| `value` | Manage `{VALUE_NAME}` rule variables. |
| `script` | Manage request, response, decode, and parser scripts. |
| `upgrade`, `update`, `version-check` | Check for new versions and upgrade the binary; `update` is an alias for `upgrade`. |
| `config` | Inspect and modify runtime config, connections, cache, and performance status. |
| `admin` | Manage Admin remote access, passwords, sessions, and audit logs. |
| `capture` | Wait for the next matching traffic record, useful for browser/app debugging and agent evidence collection. |
| `traffic`, `search` | List, get, search, export, replay, diagnose, and clear traffic records. |
| `install-skill` | Install the Bifrost Agent Skill into AI coding tools. |
| `im` | Manage IM Gateway providers, targets, routes, schedules, messages, and agent chat channels. |
| `remote` | Connect to and operate authorized remote Bifrost instances. |
| `ai` | Manage AI workflows, ASR tasks, and voice capabilities where supported. |

## Important Conventions

- Top-level `--port` and `start --port` mean the proxy listener port. In `traffic list`, `--port` means the Admin API port; use `--listener-port` or `--proxy-port` for traffic entry port filtering.
- `setting` is local-only. Use `remote exec` to change remote settings.
- Prefer temporary ports for isolated debugging instead of creating many temporary data directories.
- Prefer inline rule values or embedded rule-file blocks for small content; use global Values only for large or broadly shared content.

## Common Workflows

```bash
bifrost start -d
bifrost status
bifrost rule add local-dev -c "example.com host://127.0.0.1:3000"
bifrost traffic list --limit 20
bifrost capture wait --host api.example.com --method POST --path /v1/login --timeout 30s
bifrost traffic get <id> --request-body --response-body
bifrost traffic get --ids 1,2,3 --request-body --response-body --format ndjson
bifrost traffic auth-status <id>
bifrost traffic export <id> --as curl
bifrost traffic replay <id> --patch '/json/debug=true'
bifrost search "keyword" --req-header
bifrost search "" --host api.example.com --res-json '$.error.code=invalid_request' --latest 15m --include response-body
bifrost port bind --port 18888 --rule-text "debug.test statusCode://218 resBody://debug"
bifrost port destroy 18888
bifrost im provider add feishu-main --type feishu --runner traex
bifrost im provider add weixin-main --type weixin --runner codex
```

Use the command-specific `--help` output as the source of truth for every flag. This release does not redact Authorization, Cookie, JWT token, or other sensitive values from traffic detail, export, or `search --include` output; a complete redaction design will be handled separately. Treat those outputs as sensitive and manually remove secrets before publishing reusable skills or sharing evidence with lower-trust channels.

## CLI Proxy and CA Environment

Use `cli-proxy` when terminal programs should use Bifrost without changing the OS proxy. It installs proxy variables plus CA variables for Node.js, Python, Go/cURL, Git, Cargo, Deno, AWS SDKs, gRPC, Composer, and related tools:

```bash
bifrost cli-proxy enable
bifrost cli-proxy enable --shell zsh --host 127.0.0.1 --port 9900
bifrost cli-proxy enable --no-proxy "localhost,127.0.0.1,::1,*.local"
bifrost cli-proxy enable --ca-file /path/to/custom.pem --ca-dir /path/to/certs
bifrost cli-proxy disable
```

Without `--shell`, Bifrost detects Bash, Zsh, Fish, or PowerShell. Bash updates `~/.bashrc` and the first existing login profile in Bash precedence order (`.bash_profile`, `.bash_login`, `.profile`), creating `.bash_profile` only when none exists. Zsh updates `.zshrc` and `.zprofile`; Fish updates `~/.config/fish/config.fish`; PowerShell updates its platform profile paths.

Managed values include upper- and lower-case proxy variables and CA variables such as `NODE_EXTRA_CA_CERTS`, `SSL_CERT_FILE`, `SSL_CERT_DIR`, `REQUESTS_CA_BUNDLE`, `CURL_CA_BUNDLE`, `PIP_CERT`, `NPM_CONFIG_CAFILE`, `GIT_SSL_CAINFO`, `AWS_CA_BUNDLE`, `GRPC_DEFAULT_SSL_ROOTS_FILE_PATH`, `CARGO_HTTP_CAINFO`, `COMPOSER_CAFILE`, and `DENO_CERT`. Override-style variables point to a combined system-roots-plus-Bifrost bundle; the command does not disable TLS verification.

Open a new shell or reload the profiles printed by the command. `disable` removes only complete Bifrost marker blocks and cannot mutate the environment of an already-running parent shell. If automatic editing fails, the CLI prints the target profiles, a complete copyable block, or exact marker removal instructions. Normal stop, foreground exit, and crash cleanup remove all managed shell blocks; restart handoff preserves them for the replacement process.

## IM Gateway

`bifrost im` manages chat providers and agent-facing IM channels.

```bash
bifrost im provider list
bifrost im provider add feishu-main --type feishu --runner traex
bifrost im provider add weixin-main --type weixin --runner codex
bifrost im provider add feishu-main --type feishu --app-id cli_xxx --secret env:FEISHU_APP_SECRET --owner-open-id ou_xxx --runner "Claude Code"
bifrost im target add oncall --receive-id-type chat_id --receive-id oc_xxx
bifrost im send --provider feishu-main --text "hello owner"
bifrost im route add deploy --provider feishu-main --event message.receive --regex '^/deploy' --script-file ./deploy.sh
bifrost im schedule add agent-daily --target oncall --cron '0 9 * * *' --agent-prompt 'Summarize traffic' --agent-runner-id codex --agent-model gpt-5 --agent-reasoning-effort high
bifrost im messages list --provider feishu-main --direction inbound
```

For Feishu and Weixin, the shortest interactive setup only needs a provider ID, `--type`, and a Runner. Feishu prints an authorization URL plus a terminal QR code, then keeps the CLI running until the authorization completes and the provider is created and connected. Weixin prints a terminal QR code and waits for scan confirmation before finishing the same provider creation and connection flow.

Runner selection is explicit. In an interactive terminal, omitting `--runner` opens a keyboard-selectable list of enabled runners. In non-interactive stdin, `--runner` is required and the error lists available runners. Invalid or disabled runners are rejected with the same available-runner list. Common aliases such as `codex`, `traex` / `trae`, and `Claude Code` are accepted when the corresponding runner exists. Provider base URLs are fixed by provider type; the CLI rejects `--base-url` for provider creation and updates.

When an IM channel comes online, Bifrost sends the online notification followed by help for the bound external Runner. All runners receive channel commands such as `/help`, `/status`, `/cwd`, `/runner`, `/q`, `/rq`, and `/stop`. Feishu also advertises `/new <group-name>`, restricted to the current Provider's `owner_open_id`. For example, `/new Release Project` creates a same-name private chat, makes the sender its owner, and automatically adds the current app bot as a chat manager; replaying the same Feishu message does not create another chat. The Feishu app needs the chat-creation capability (`im:chat:create`), while its existing message-send permission is used for the welcome message. Codex, Traex, and Claude Code advertise `/models`, `/model`, `/efforts`, and `/effort` only when the adapter supports them. Codex runners additionally support `/fast`, `/fast on`, `/fast off`, and `/fast status` to switch or inspect fast mode for the current IM session; other runners return an explicit unsupported-command reply.

When a Feishu bot joins a group, each group gets an independent session key, context cursor, runner binding, and working directory. Ordinary group messages are recorded without invoking the model. An explicit bot mention, `/g`, `/q`, or an existing slash command triggers processing. A model turn receives only the effective multi-party conversation accumulated after the previous execution and before the current trigger; the final trigger appears exactly once. Each line uses `<at id=sender-open-id>display-name</at>: message`, allowing the Agent to mention the original sender accurately. When an event has no display name, Bifrost uses `<at id=sender-open-id></at>` instead of duplicating the long open ID as visible text. The first prompt that starts a model session for the group adds only the resolved chat name and chat ID. Later prompts do not repeat group information and never expose provider IDs, session keys, message IDs, sequence numbers, timestamps, or mention arrays. When no background messages were delivered, the background section is omitted entirely and the prompt contains only the final attributed trigger. Available tools are injected by installed runtime skills rather than prescribed by Bifrost's business prompt. Group chats and direct messages share the same slash command set; `/cwd` and `/runner` only change the current group binding. To receive non-mention group messages, subscribe to `im.message.receive_v1`, grant the sensitive `im:message.group_msg` scope, and republish the Feishu app. With only `im:message.group_at_msg:readonly`, Feishu does not deliver ordinary group messages to Bifrost, so they cannot appear in the accumulated context. Chat-name resolution also needs the “read chat information” capability (for example, `im:chat:readonly`).

Bifrost-generated Feishu cards omit the large root title bar. Replies triggered by a group or direct message use Feishu's native message-reply relationship, so the client renders a compact quote above the card. Titles inside collapsible plan and tool panels remain. Proactive online notifications, schedules, and management API sends have no source message to quote and are sent directly to their target, while still using the headerless card style.
