---
title: "Agent Skill Installation"
description: "Install and use the Bifrost Agent Skill."
editUrl: false
sidebar:
  label: "Agent Skill Installation"
  order: 190
---

> This page is automatically synced from `docs-en/agent-skill.md`.
> Language: **English** | [中文](../../../reference/agent-skill/)

# Agent Skill Installation Guide

Bifrost provides skill files that teach AI coding assistants how to operate the Bifrost CLI. It supports Claude Code, Codex, Trae, Cursor, GitHub Copilot, and generic Agent Skills runtimes. The installer writes both the general `bifrost` skill and the dedicated `bifrost-remote` skill: use the general skill for local proxy/rule/traffic work, and use the remote skill when connecting to another machine with a pair code or SSH key, inspecting remote traffic, uploading scripts, or operating an authorized remote shell.

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

Useful options and environment variables:

- `-t, --tool`: `claude-code`, `codex`, `trae`, `cursor`, `github-copilot`, `universal`, or `all`.
- `--cwd`: install into the current project directory.
- `-d, --dir`: install into a custom directory; mutually exclusive with `--cwd`.
- `BIFROST_INSTALL_SKILL_SOURCE`: override the skill download source for development or verification.
- `BIFROST_INSTALL_SKILL_DIR`: override the default global install directory for isolated tests.
