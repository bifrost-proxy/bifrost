---
title: "Client Remote Administration"
description: "Configure targets, authenticate, and manage another Bifrost directly through its Admin API."
editLink: false
---

> This page is automatically synced from `docs-en/client-admin.md`.
> Language: **English** | [中文](../../reference/client-admin)

# Client Remote Administration Guide

`bifrost client` connects directly to another running Bifrost instance by IP address, port, or domain name. It uses the same Admin API as the remote WebUI, so you can inspect traffic, manage rules, and change configuration with familiar CLI commands.

## When to Use Client

| Need | Recommended mode |
| --- | --- |
| Inspect traffic, rules, config, ports, or runtime status at a known IP or domain | `bifrost client` |
| Manage the target Bifrost in a browser | Remote WebUI |
| Read arbitrary target files, edit repositories, run a shell, or manage processes | `bifrost remote` |
| Manage Bifrost on the current machine | Normal local commands such as `bifrost status` and `bifrost traffic ...` |

Client does not use Relay, pair codes, SSH keys, or Remote Invoke grants. If a Client command fails or is unsupported, it returns an error instead of reading local business data or falling back to `bifrost remote`.

## Quick Start

### 1. Enable Remote Admin on the Target

Run these commands locally on the target. Set an Admin password, then enable remote access:

```bash
bifrost admin passwd
bifrost admin remote enable
bifrost admin remote status
```

`remote enable` exposes the remote WebUI and Admin API. It does not enable Remote Invoke. Initial enablement cannot be performed through `bifrost client`, preventing an unauthenticated client from expanding its own access.

Make sure the target Bifrost listener is reachable from the caller. The default port is `9900`; use the target's actual port in the URL if it differs.

### 2. Save the Target on the Caller

Prefer an HTTPS endpoint with a valid certificate:

```bash
bifrost client target add devbox --url https://bifrost.example.com
```

Plain HTTP on a trusted LAN requires explicit risk acceptance:

```bash
bifrost client target add devbox \
  --url http://10.0.0.8:9900 \
  --allow-insecure-http
```

Plain HTTP exposes the Admin password and JWT to anyone who can observe the network. Use HTTPS, a VPN, or an SSH tunnel across untrusted networks.

### 3. Log In

An interactive terminal reads the password without echoing it:

```bash
bifrost client target login devbox --username admin
```

Automation should read the password from standard input so it does not enter command arguments or shell history:

```bash
printf '%s' "$BIFROST_ADMIN_PASSWORD" | \
  bifrost client target login devbox --username admin --password-stdin
```

Client stores only the Admin JWT and expiration returned after a successful login. It never stores the password.

### 4. Run Remote Administration Commands

When exactly one saved target exists, `--target` may be omitted:

```bash
bifrost client status --format json
bifrost client traffic list --limit 20
```

Select the target explicitly when multiple targets exist or when the destination should be unambiguous:

```bash
bifrost client --target devbox status --format json
bifrost client --target devbox traffic list --format json
bifrost client --target devbox rule list
```

`--target` must appear before the business command. Business-command arguments and stdout schemas stay aligned with local commands, so existing JSON or NDJSON scripts normally need only the `client --target <name>` prefix.

## Manage Targets

```bash
bifrost client target list
bifrost client target show devbox
bifrost client target rename devbox lab
bifrost client target logout lab
bifrost client target remove lab
```

- `target add` stores the endpoint and default username only; it does not accept a password.
- `target logout` removes only the JWT saved on the caller. It does not revoke other browser or CLI sessions.
- `target remove` removes both the target and its local session.
- To invalidate every Admin session on the target, run `bifrost client --target lab admin revoke-all`.

A target URL may be an IP address, `host:port`, or a complete HTTP(S) URL. A bare host defaults to `http://<host>:9900`. URLs cannot include user info, a query, a fragment, or an application path. A trailing `/_bifrost` or `/_bifrost/api` is normalized to the service origin.

## Target Selection

Client selects a target in this order:

1. Command-line `--target <name-or-address>`;
2. `BIFROST_CLIENT_TARGET`;
3. The only saved target;
4. An interactive list when multiple targets exist in a terminal;
5. An error listing available targets in a non-interactive process.

Set a default alias for a script:

```bash
export BIFROST_CLIENT_TARGET=devbox
bifrost client status --format json
```

You may also connect to an unsaved address. The address must be explicit, and the current process must provide its JWT through `BIFROST_ADMIN_TOKEN`:

```bash
BIFROST_ADMIN_TOKEN="$TOKEN" \
  bifrost client --target https://bifrost.example.com status --format json
```

An environment token is never implicitly bound to a saved profile when `--target` is omitted.

## Common Workflows

### Inspect Traffic and Wait for a Request

```bash
bifrost client --target devbox traffic list --limit 50
bifrost client --target devbox traffic get <id> --request-body --response-body
bifrost client --target devbox search "invalid_request" --res-body
bifrost client --target devbox capture wait \
  --host api.example.com \
  --method POST \
  --timeout 30s
```

Traffic details may contain Authorization headers, cookies, JWTs, and application data. Remove sensitive fields before sharing the output.

### Manage Rules, Values, and Scripts

```bash
bifrost client --target devbox rule list
bifrost client --target devbox rule add debug-api \
  -c "api.example.com reqHeaders://X-Debug=1"
bifrost client --target devbox rule enable debug-api

bifrost client --target devbox value list
bifrost client --target devbox script list
```

These mutations apply directly to the target instance. Client does not automatically retry mutations after a timeout, connection loss, or server 5xx response because doing so could duplicate side effects. Inspect target state before manually retrying.

### Inspect Configuration and Runtime State

```bash
bifrost client --target devbox status --format json
bifrost client --target devbox config show --json
bifrost client --target devbox metrics summary
bifrost client --target devbox port list
bifrost client --target devbox whitelist list
```

Client V1 also supports target-side Admin API capabilities under `group`, `account`, `admin`, `login`, `sync`, `import`, `export`, and `version-check`. Run `bifrost client --target devbox <command> --help` for business-command arguments.

## Security Notes

- HTTPS uses normal system certificate validation. Client has no option to skip certificate checks.
- Client requests ignore `HTTP_PROXY`, `HTTPS_PROXY`, and `ALL_PROXY`, preventing requests from looping through the current Bifrost.
- HTTP redirects are disabled, so a Bearer token is not forwarded to another origin.
- JWTs are stored under the active `BIFROST_DATA_DIR` in `cli/admin-credentials.toml`; target profiles are stored in `cli/admin-targets.toml`.
- On Unix, the credential directory uses mode `0700` and files use mode `0600`. V1 does not use the system keychain, so a malicious process running as the same OS user may still read the credentials.
- High-risk mutations retain the confirmation behavior of their corresponding local commands. Always confirm the active target before proceeding.

## Unsupported Commands

Client V1 rejects these local-only or different-transport capabilities before sending a request:

- `start`, `stop`, `restart`, and `status --tui`;
- Nested `client` and all `remote` commands;
- `rule sync` and `script run`;
- `cli-proxy`, `system-proxy`, `keep-awake`, `upgrade`, `completions`, `install-skill`, `app`, and `ca`;
- `setting`, `ai`, `im`, and `agent`.

Rejected commands are not executed locally on the caller. To operate target files, shells, processes, or repositories, explicitly switch to `bifrost remote`; see the [CLI command reference](./cli).

## Troubleshooting

### Remote Admin Is Disabled

Run the following locally on the target:

```bash
bifrost admin remote enable
bifrost admin remote status
```

Also verify the target listener address, firewall, and network route are reachable from the caller.

### Not Logged In, Expired Token, or HTTP 401

Client does not cache passwords, automatically log in again, or replay the original request. Log in explicitly, then rerun the command:

```bash
bifrost client target login devbox --username admin
bifrost client --target devbox status
```

### Multiple Targets Are Rejected

An interactive terminal can display a selection list. Scripts and pipelines must use `--target` or `BIFROST_CLIENT_TARGET` so they cannot operate on the wrong device.

### A Command Is Unsupported in Client Mode

The capability has not migrated to the Admin API or belongs to the local/Remote Invoke boundary. Client never falls back automatically. Follow the error and either run the command locally on the target or explicitly use `bifrost remote`.
