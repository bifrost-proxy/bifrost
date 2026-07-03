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
| `value` | Manage `{VALUE_NAME}` rule variables. |
| `script` | Manage request, response, decode, and parser scripts. |
| `upgrade`, `version-check` | Check for new versions and upgrade the binary. |
| `config` | Inspect and modify runtime config, connections, cache, and performance status. |
| `admin` | Manage Admin remote access, passwords, sessions, and audit logs. |
| `capture` | Wait for the next matching traffic record, useful for browser/app debugging and agent evidence collection. |
| `traffic`, `search` | List, get, search, export, replay, diagnose, and clear traffic records. |
| `install-skill` | Install the Bifrost Agent Skill into AI coding tools. |
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
```

Use the command-specific `--help` output as the source of truth for every flag. This release does not redact Authorization, Cookie, JWT token, or other sensitive values from traffic detail, export, or `search --include` output; a complete redaction design will be handled separately. Treat those outputs as sensitive and manually remove secrets before publishing reusable skills or sharing evidence with lower-trust channels.
