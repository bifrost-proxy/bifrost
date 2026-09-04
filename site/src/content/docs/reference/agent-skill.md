---
title: "Agent Skill 安装说明"
description: "Bifrost Agent Skill 的安装方式与使用入口。"
editLink: false
---

> 此页面由 `docs/agent-skill.md` 自动同步生成。
# Agent Skill 安装说明

Bifrost 提供标准 `SKILL.md` 技能文件，让支持 Agent Skills 的 AI 编程助手自动掌握 bifrost CLI 的操作能力。

安装器只维护两类目录：

- 通用 Agent Skills：`~/.agents/skills/`
- Claude Code 兼容目录：`~/.claude/skills/`

不再为 Codex、Trae、Cursor、GitHub Copilot 等工具维护各自的私有技能目录；支持标准 `.agents/skills` 的工具统一复用同一份技能。

## 快速安装（推荐）

```bash
bifrost install-skill -y
```

默认同时安装到通用 Agent Skills 目录和 Claude Code 目录。每次执行都会从 GitHub `main` 分支获取最新技能内容；网络不可用时会回退到 CLI 内嵌版本。

安装内容包含两个 Skill：

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

- `bifrost`：本机代理，以及通过 IP、端口、域名或已保存 target 直连另一台 Bifrost Admin API 的 Client 模式。
- `bifrost-remote`：pair code / SSH key 授权、Relay-backed 查询、远程文件、remote run/job 和授权 shell 操作。

## 安装到指定目标

```bash
bifrost install-skill -t universal -y    # 仅 ~/.agents/skills
bifrost install-skill -t claude-code -y  # 仅 ~/.claude/skills
bifrost install-skill -t all -y          # 两者都安装（默认）
```

支持的 `--tool` 值只有：

- `universal`（别名 `agent-skills`）
- `claude-code`（别名 `claude`）
- `all`

历史目标 `codex`、`trae`、`cursor`、`github-copilot` 等不再支持。

## 项目级安装

```bash
bifrost install-skill --cwd -y
```

默认写入：

```text
./.agents/skills/
├── bifrost/SKILL.md
└── bifrost-remote/SKILL.md

./.claude/skills/
├── bifrost/SKILL.md
└── bifrost-remote/SKILL.md
```

也可以只安装一个目标：

```bash
bifrost install-skill --cwd -t universal -y
bifrost install-skill --cwd -t claude-code -y
```

## 自定义目录

```bash
bifrost install-skill -d /custom/path -y
```

`--dir` 与 `--cwd` 互斥。

## 安装行为

安装器会把仓库中的：

```text
SKILL.md        -> bifrost/SKILL.md
skill_remote.md -> bifrost-remote/SKILL.md
```

原样安装，保留标准 YAML frontmatter，不做额外包装。

`install-skill` 只写入技能文档：不会启动代理、修改系统代理、导入规则、保存 Client target、登录 Admin、创建 Remote Invoke 授权或授予 shell 权限。Client 直连需要目标机先在本地开启 Admin Remote Access 并登录；Remote Invoke 仍必须由用户通过 pair code 或 SSH key 显式授权。

## 两种远程模式

已知目标 Bifrost 的 IP、端口或域名，并希望查询 traffic/status 或管理 rule/value/script/config/whitelist 时，Agent 使用通用 Skill 的 Client 工作流：

```bash
bifrost client target add devbox --url http://10.0.0.8:9900 --allow-insecure-http
printf '%s' "$BIFROST_ADMIN_PASSWORD" | \
  bifrost client target login devbox --username admin --password-stdin
bifrost client --target devbox traffic list --format json
bifrost client --target devbox rule list
```

Client 与远程 WebUI 对等，直连 Admin API。目标机需先在本地运行 `bifrost admin remote enable`。只有一个 target 时可省略 `--target`；多个 target 的非交互调用必须显式选择。401 后显式重新登录，Client 失败或命令不受支持时不得读取本机数据，也不得自动降级到 Remote Invoke。

需要读取或修改目标机任意文件、操作远端仓库、执行 shell/构建/进程时，才使用 `bifrost-remote` Skill。该模式走 Relay、pairing 和 grant，不读取 Client target 或 Admin JWT。

## 安装后使用 IM Gateway CLI

安装 `bifrost` Skill 后，Agent 在配置飞书或微信 IM 通道时，应优先使用本机 `bifrost im` CLI，而不是直接修改配置文件：

```bash
bifrost start -d
bifrost im provider add feishu-main --type feishu --runner traex
bifrost im provider add weixin-main --type weixin --runner codex
```

Feishu 交互式配置会输出授权 URL 和二维码；Weixin 交互式配置会输出扫码二维码。CLI 会等待用户完成授权后自动创建并连接 provider。非交互环境必须显式传 `--runner`；交互式终端可以让用户选择 Runner。

## 参数

| 参数 | 说明 |
| --- | --- |
| `-t, --tool <TOOL>` | `universal`、`claude-code` 或 `all`（默认 `all`） |
| `-d, --dir <PATH>` | 自定义安装目录，与 `--cwd` 互斥 |
| `--cwd` | 安装到当前项目目录，与 `--dir` 互斥 |
| `-y, --yes` | 跳过确认提示 |

环境变量：

| 变量 | 说明 |
| --- | --- |
| `BIFROST_INSTALL_SKILL_SOURCE` | 覆盖技能下载源；主要用于开发或验证 |
| `BIFROST_INSTALL_SKILL_DIR` | 覆盖默认全局安装目录；主要用于测试隔离 |

## 手动安装

```bash
# 通用 Agent Skills
mkdir -p ~/.agents/skills/bifrost ~/.agents/skills/bifrost-remote
cp ./SKILL.md ~/.agents/skills/bifrost/SKILL.md
cp ./skill_remote.md ~/.agents/skills/bifrost-remote/SKILL.md

# Claude Code
mkdir -p ~/.claude/skills/bifrost ~/.claude/skills/bifrost-remote
cp ./SKILL.md ~/.claude/skills/bifrost/SKILL.md
cp ./skill_remote.md ~/.claude/skills/bifrost-remote/SKILL.md
```

## 验证与更新

安装后，在对应 Agent 中开启新会话并请求“启动代理”“查看流量”等 Bifrost 操作；如果 Agent 能发现 Skill 并正确调用 `bifrost` CLI，说明安装成功。

验证两种远程模式时：要求“通过已知 IP 登录另一台 Bifrost 并查询流量/修改规则”，Agent 应使用通用 skill 的 `bifrost client target` 流程；要求“通过 pair code 或 SSH key 修改另一台机器的文件或执行 shell”，Agent 才应进入 `bifrost-remote`。两种模式不得自动互相降级。

更新技能：

```bash
bifrost install-skill -y
```

每次执行都会覆盖现有 Skill，保持与最新版本一致。
