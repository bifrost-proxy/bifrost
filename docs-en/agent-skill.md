> Language: **English** | [中文](../docs/agent-skill.md)

# Agent Skill Installation Guide

Bifrost provides a `SKILL.md` file that teaches AI coding assistants how to operate the Bifrost CLI. It supports Claude Code, Codex, Trae, Cursor, GitHub Copilot, and generic Agent Skills runtimes.

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
```

## Project-local Install

```bash
bifrost install-skill --cwd -y
bifrost install-skill --cwd -t trae -y
```

All tools use the `skills/bifrost/SKILL.md` structure. Codex also keeps the historical `.codex/skills` path while adding `.agents/skills` for broader compatibility.
