> Language: **English** | [中文](../docs/agent-skill.md)

# Agent Skill Installation Guide

Bifrost provides skill files that teach AI coding assistants how to operate the Bifrost CLI. It supports Claude Code, Codex, Trae, Cursor, GitHub Copilot, and generic Agent Skills runtimes. The installer writes both the general `bifrost` skill and the dedicated `bifrost-remote` skill: use the general skill for local proxy/rule/traffic work and IM Gateway CLI setup, and use the remote skill when connecting to another machine with a pair code or SSH key, inspecting remote traffic, uploading scripts, or operating an authorized remote shell.

## Quick Install

```bash
bifrost install-skill -y
```

Each run downloads the latest `SKILL.md` from the GitHub `main` branch and installs it into supported target directories.

## Install for a Specific Tool

```bash
bifrost install-skill -t claude-code -y
bifrost install-skill -t codex -y
bifrost install-skill -t trae -y
bifrost install-skill -t cursor -y
bifrost install-skill -t github-copilot -y
bifrost install-skill -t universal -y
bifrost install-skill -t all -y
```

## Project-local Install

```bash
bifrost install-skill --cwd -y
bifrost install-skill --cwd -t trae -y
```

All tools use the `skills/bifrost/SKILL.md` structure. The installer also writes `skills/bifrost-remote/SKILL.md` for remote workflows. Codex keeps the historical `.codex/skills` path while adding `.agents/skills` for broader compatibility.

`install-skill` only installs documentation for agents. It does not start the proxy, enable the system proxy, import rules, create remote grants, or authorize shell access. Remote access must still be explicitly granted by the user through a pair code or SSH key.

## Using IM Gateway After Install

After the general `bifrost` skill is installed, agents should use the local `bifrost im` CLI for Feishu or Weixin channel setup instead of editing provider files directly.

```bash
bifrost start -d
bifrost im provider add feishu-main --type feishu --runner traex
bifrost im provider add weixin-main --type weixin --runner codex
```

Feishu interactive setup prints an authorization URL and terminal QR code. Weixin interactive setup prints a scan QR code. The CLI keeps waiting until the user authorizes or scans, then creates and connects the provider. Non-interactive runs must pass `--runner`; interactive terminals can present a keyboard-selectable runner list. Invalid runners are rejected with the current supported runner list. Feishu and Weixin provider base URLs are fixed by provider type, so agents should not suggest `--base-url`.

When an IM channel comes online, Bifrost sends the online notification followed by runner-aware help. External runners such as Codex, Traex, and Claude Code should advertise model or reasoning-effort commands only when their adapter supports them.

Useful options and environment variables:

- `-t, --tool`: `claude-code`, `codex`, `trae`, `cursor`, `github-copilot`, `universal`, or `all`.
- `--cwd`: install into the current project directory.
- `-d, --dir`: install into a custom directory; mutually exclusive with `--cwd`.
- `BIFROST_INSTALL_SKILL_SOURCE`: override the skill download source for development or verification.
- `BIFROST_INSTALL_SKILL_DIR`: override the default global install directory for isolated tests.
