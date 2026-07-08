# Agent Skill 安装说明

Bifrost 提供 `SKILL.md` 技能文件，可以让 AI 编程助手（Claude Code、Codex、Trae、Cursor、GitHub Copilot 等）自动掌握 bifrost CLI 的完整操作能力；同时也兼容更多遵循通用 Agent Skills 目录约定的运行时。

## 快速安装（推荐）

使用 `bifrost install-skill` 命令一键安装到所有支持的 AI 编程工具：

```bash
bifrost install-skill -y
```

每次执行都会从 GitHub 主干（main 分支）下载最新的 skill 文档，覆盖式安装到目标目录。安装内容包含通用 `bifrost` skill 和专用 `bifrost-remote` skill：本机代理、规则、流量分析和 IM Gateway CLI 配置使用通用 skill；连接另一台机器、使用 pair code / SSH key、远程查流量、上传脚本执行或授权 shell 操作目标设备时使用 `bifrost-remote`。

### 安装到指定工具

```bash
bifrost install-skill -t claude-code -y    # 仅 Claude Code
bifrost install-skill -t codex -y          # Codex + 通用 .agents/skills 目录
bifrost install-skill -t trae -y           # 仅 Trae
bifrost install-skill -t cursor -y         # 仅 Cursor
bifrost install-skill -t github-copilot -y # 仅 GitHub Copilot
bifrost install-skill -t universal -y      # 仅通用 Agent Skills 目录
bifrost install-skill -t all -y            # 自动安装到所有支持的工具（默认）
```

### 安装到当前项目目录

```bash
bifrost install-skill --cwd -y            # 安装到当前目录（所有工具）
bifrost install-skill --cwd -t trae -y    # 安装到当前目录（仅 Trae）
```

### 安装到自定义目录

```bash
bifrost install-skill -d /custom/path -y
```

### 支持的工具、默认路径与安装内容

所有工具统一使用 `skills/bifrost/SKILL.md` 目录结构；同时会安装 `skills/bifrost-remote/SKILL.md`，供远程连接和远端设备操作使用。

| 工具 | 别名 | 全局安装路径 | 项目级安装路径（--cwd） |
| --- | --- | --- | --- |
| Claude Code | `claude-code`, `claude` | `~/.claude/skills/bifrost/SKILL.md` | `./.claude/skills/bifrost/SKILL.md` |
| Codex | `codex`, `openai-codex` | `~/.codex/skills/bifrost/SKILL.md` + `~/.agents/skills/bifrost/SKILL.md` | `./.codex/skills/bifrost/SKILL.md` + `./.agents/skills/bifrost/SKILL.md` |
| Trae | `trae` | `~/.trae/skills/bifrost/SKILL.md` + `~/.trae-cn/skills/bifrost/SKILL.md` | `./.trae/skills/bifrost/SKILL.md` |
| Cursor | `cursor` | `~/.cursor/skills/bifrost/SKILL.md` | `./.cursor/skills/bifrost/SKILL.md` |
| GitHub Copilot | `github-copilot`, `copilot` | `~/.copilot/skills/bifrost/SKILL.md` | `./.github/skills/bifrost/SKILL.md` |
| Universal Agent Skills | `universal` | `~/.agents/skills/bifrost/SKILL.md` | `./.agents/skills/bifrost/SKILL.md` |

> **注意**：
> - Trae 在全局模式下会同时安装到 `.trae` 和 `.trae-cn` 两个目录（适配国内外版本），项目级安装（`--cwd`）仅安装到 `.trae`
> - `codex` 目标会保留历史 `.codex/skills` 路径，同时补充标准 `.agents/skills` 目录，方便兼容更多 agent
> - `universal` 目标仅安装到 `.agents/skills`，适合遵循通用 Agent Skills 规范的运行时

安装的文件始终是从远端下载的原始 skill 内容（含标准 YAML frontmatter），不做任何额外包装或修改。源文件自带 `name` 和 `description` frontmatter 字段，兼容所有工具的 skill 自动发现机制。通用 skill 覆盖本机代理、规则、流量和 Agent 采证工作流；remote skill 覆盖授权连接、remote traffic、remote file、remote run/job 和 shell access 边界。

> **安全提示**：`install-skill` 只写入说明文档，不启动代理、不修改系统代理、不导入规则、不会创建远端授权，也不会授予 shell 权限。远端连接必须由用户通过 pair code 或 SSH key 显式授权。

### 安装后使用 IM Gateway CLI

安装通用 `bifrost` skill 后，Agent 在用户要求配置飞书或微信 IM 通道时，应优先使用当前机器上的 `bifrost im` CLI，而不是直接改配置文件。常见流程如下：

```bash
bifrost start -d
bifrost im provider add feishu-main --type feishu --runner traex
bifrost im provider add weixin-main --type weixin --runner codex
```

Feishu 交互式配置会在终端输出授权 URL 和二维码；Weixin 交互式配置会输出扫码二维码。CLI 会保持等待，直到用户完成授权或扫码确认，然后自动创建并连接 provider。非交互环境必须显式传 `--runner`；交互式终端可以让用户用键盘选择 Runner。Runner 不存在或未启用时，CLI 会列出当前支持的 Runner。Feishu / Weixin provider 的 `base_url` 由系统按 provider 类型固定管理，不要向用户建议传 `--base-url`。

IM 通道上线后，Bifrost 会自动推送上线通知和 runner-aware 帮助。Agent 应按实际 Runner 区分命令范围：内置 Bifrost Agent 支持 memory、goal、compact、guidance 等命令；Codex / Traex / Claude Code 等外部 Runner 只应展示其适配器支持的模型和 reasoning effort 命令。

### 参数说明

| 参数 | 说明 |
| --- | --- |
| `-t, --tool <TOOL>` | 目标工具：`claude-code`、`codex`、`trae`、`cursor`、`github-copilot`、`universal`、`all`（默认 `all`） |
| `-d, --dir <PATH>` | 自定义安装目录（覆盖默认路径，与 `--cwd` 互斥） |
| `--cwd` | 安装到当前目录（项目级别，与 `--dir` 互斥） |
| `-y, --yes` | 跳过确认提示 |

相关环境变量：

| 变量 | 说明 |
| --- | --- |
| `BIFROST_INSTALL_SKILL_SOURCE` | 覆盖 skill 下载源；正常用户通常无需设置，主要用于开发或验证不同来源。 |
| `BIFROST_INSTALL_SKILL_DIR` | 未显式传 `--dir` / `--cwd` 时覆盖全局 skill 安装目录；适合测试隔离，避免写入真实 AI 工具目录。 |

### 错误处理

安装过程中可能遇到的错误及处理建议：

| 错误类型 | 常见原因 | 处理建议 |
| --- | --- | --- |
| 网络错误 | DNS 解析失败、连接超时、GitHub 不可达 | 检查网络连接和 DNS 设置，确认可以访问 github.com |
| HTTP 错误 | 404（文件不存在）、403（被限流）、5xx（服务端故障） | 稍后重试，或检查 GitHub 状态 |
| 权限错误 | 目标目录无写入权限 | 使用 `sudo` 或通过 `--dir` 指定其他目录 |
| 写入失败 | 磁盘空间不足、路径过长 | 清理磁盘空间或使用 `--dir` 指定较短路径 |

## 手动安装

如果不想使用 `install-skill` 命令，也可以手动复制：

```bash
# Claude Code
mkdir -p ~/.claude/skills/bifrost && cp ./SKILL.md ~/.claude/skills/bifrost/SKILL.md

# Codex
mkdir -p ~/.codex/skills/bifrost && cp ./SKILL.md ~/.codex/skills/bifrost/SKILL.md
mkdir -p ~/.agents/skills/bifrost && cp ./SKILL.md ~/.agents/skills/bifrost/SKILL.md

# Trae (国际版 + 国内版)
mkdir -p ~/.trae/skills/bifrost && cp ./SKILL.md ~/.trae/skills/bifrost/SKILL.md
mkdir -p ~/.trae-cn/skills/bifrost && cp ./SKILL.md ~/.trae-cn/skills/bifrost/SKILL.md

# Cursor
mkdir -p ~/.cursor/skills/bifrost && cp ./SKILL.md ~/.cursor/skills/bifrost/SKILL.md

# GitHub Copilot
mkdir -p ~/.copilot/skills/bifrost && cp ./SKILL.md ~/.copilot/skills/bifrost/SKILL.md
```

## 安装后验证

安装完成后，在对应工具中启动新对话，输入与 bifrost 相关的指令（如“启动代理”“查看流量”），如果 AI 能正确识别并调用 `bifrost` CLI，说明通用 skill 安装成功。

可以继续验证远程 skill 是否可被发现：向工具说明“我要连接另一台机器的 Bifrost，并通过 pair code 或 SSH key 授权”，期望 Agent 选择 `bifrost-remote` 流程，先检查 `bifrost remote conn status`，再按授权方式连接；它不应要求你把密钥或 token 粘贴到最终文档里。

## 更新技能文件

技能文件会随 bifrost 版本迭代更新。建议定期执行：

```bash
bifrost install-skill -y
```

每次执行都会从远端主干拉取最新版本并覆盖安装，确保 AI 编程工具始终掌握最新的 CLI 能力。
