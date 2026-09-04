> Language: **English** | [中文](../docs/agent-skill.md)

# Agent Skill Installation Guide

Bifrost ships standard `SKILL.md` files so AI coding assistants that support Agent Skills can discover and use the Bifrost CLI.

The installer now maintains only two target locations:

- Universal Agent Skills: `~/.agents/skills/`
- Claude Code compatibility: `~/.claude/skills/`

Bifrost no longer installs separate copies into vendor-specific Codex, Trae, Cursor, or GitHub Copilot skill directories. Tools that support the standard `.agents/skills` location share the same portable skill source.

## Quick Install

```bash
bifrost install-skill -y
```

By default, this installs to both Universal Agent Skills and Claude Code. Each run fetches the latest skill content from the GitHub `main` branch; if the network is unavailable, the CLI falls back to its embedded copy.

The installer writes two skills:

```text
~/.agents/skills/
├── bifrost/
│   └── SKILL.md
└── bifrost-remote/
    └── SKILL.md

~/.claude/skills/
├── bifrost/
│   └── SKILL.md
└── bifrost-remote/
    └── SKILL.md
```

- `bifrost`: local proxy management plus Client mode for direct Admin API access to another Bifrost by IP, port, domain, or saved target.
- `bifrost-remote`: pair-code / SSH-key authorization, Relay-backed queries, remote files, remote run/job, and authorized shell operations.

## Install a Specific Target

```bash
bifrost install-skill -t universal -y    # ~/.agents/skills only
bifrost install-skill -t claude-code -y  # ~/.claude/skills only
bifrost install-skill -t all -y          # both targets (default)
```

Supported `--tool` values are limited to:

- `universal` (alias: `agent-skills`)
- `claude-code` (alias: `claude`)
- `all`

Legacy targets such as `codex`, `trae`, `cursor`, and `github-copilot` are no longer supported.

## Project-local Install

```bash
bifrost install-skill --cwd -y
```

The default project-local layout is:

```text
./.agents/skills/
├── bifrost/SKILL.md
└── bifrost-remote/SKILL.md

./.claude/skills/
├── bifrost/SKILL.md
└── bifrost-remote/SKILL.md
```

You can also install only one target:

```bash
bifrost install-skill --cwd -t universal -y
bifrost install-skill --cwd -t claude-code -y
```

## Custom Directory

```bash
bifrost install-skill -d /custom/path -y
```

`--dir` and `--cwd` are mutually exclusive.

## Installation Behavior

The installer maps repository sources as follows:

```text
SKILL.md        -> bifrost/SKILL.md
skill_remote.md -> bifrost-remote/SKILL.md
```

The original skill content, including standard YAML frontmatter, is installed without extra wrapping.

`install-skill` only writes skill documentation. It does not start the proxy, change system proxy settings, import rules, save Client targets, log in to Admin, create Remote Invoke grants, or authorize shell access. Client access requires Admin Remote Access to be enabled locally on the target and an Admin login. Remote Invoke still requires explicit pair-code or SSH-key authorization.

## Two Remote Modes

When the target Bifrost IP, port, or domain is known and the task is to inspect traffic/status or manage rules, values, scripts, config, or whitelist state, the agent uses Client mode from the general skill:

```bash
bifrost client target add devbox --url http://10.0.0.8:9900 --allow-insecure-http
printf '%s' "$BIFROST_ADMIN_PASSWORD" | \
  bifrost client target login devbox --username admin --password-stdin
bifrost client --target devbox traffic list --format json
bifrost client --target devbox rule list
```

Client mode is equivalent to remote WebUI administration and connects directly to the Admin API. The target must first run `bifrost admin remote enable` locally. `--target` may be omitted when exactly one target exists; non-interactive calls with multiple targets must select one explicitly. After a 401, log in explicitly again. A Client error or unsupported command must never fall back to local data or Remote Invoke.

Use the `bifrost-remote` skill only for arbitrary target files, remote repository changes, shell commands, builds, or process execution. That mode uses Relay, pairing, and grants; it does not read Client targets or Admin JWTs.

## Using IM Gateway After Install

After the `bifrost` skill is installed, agents should use the local `bifrost im` CLI for Feishu or Weixin setup instead of editing provider files directly.

```bash
bifrost start -d
bifrost im provider add feishu-main --type feishu --runner traex
bifrost im provider add weixin-main --type weixin --runner codex
```

Feishu interactive setup prints an authorization URL and QR code. Weixin interactive setup prints a scan QR code. The CLI waits for the user to finish authorization and then creates and connects the provider. Non-interactive runs must pass `--runner`; interactive terminals can present a selectable runner list.

## Options

| Option | Description |
| --- | --- |
| `-t, --tool <TOOL>` | `universal`, `claude-code`, or `all` (default: `all`) |
| `-d, --dir <PATH>` | Custom install directory; mutually exclusive with `--cwd` |
| `--cwd` | Install into the current project; mutually exclusive with `--dir` |
| `-y, --yes` | Skip the confirmation prompt |

Environment variables:

| Variable | Description |
| --- | --- |
| `BIFROST_INSTALL_SKILL_SOURCE` | Override the skill download source, mainly for development or verification |
| `BIFROST_INSTALL_SKILL_DIR` | Override the default global install directory, mainly for isolated tests |

## Manual Install

```bash
# Universal Agent Skills
mkdir -p ~/.agents/skills/bifrost ~/.agents/skills/bifrost-remote
cp ./SKILL.md ~/.agents/skills/bifrost/SKILL.md
cp ./skill_remote.md ~/.agents/skills/bifrost-remote/SKILL.md

# Claude Code
mkdir -p ~/.claude/skills/bifrost ~/.claude/skills/bifrost-remote
cp ./SKILL.md ~/.claude/skills/bifrost/SKILL.md
cp ./skill_remote.md ~/.claude/skills/bifrost-remote/SKILL.md
```

## Verify and Update

After installation, start a new session in the target agent and ask it to perform a Bifrost operation such as “start the proxy” or “inspect traffic.” If it discovers the skill and calls the `bifrost` CLI correctly, the general skill is working.

To verify both remote modes, ask the agent to log in to another Bifrost by a known IP and inspect traffic or edit a rule; it should use `bifrost client target` from the general skill. Ask it to modify files or run a shell through a pair code or SSH key; only that request should enter the `bifrost-remote` workflow. The modes must never silently fall back to each other.

Update the skills with:

```bash
bifrost install-skill -y
```

Each run overwrites the installed skills with the latest version.
