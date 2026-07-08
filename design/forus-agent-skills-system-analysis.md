# 开发版 Forus Agent/Skills 工程体系分析

> 本仓库没有直接出现 `Forus/forus` 命名；本文按用户业务称呼，把当前仓库中用于开发版 Forus 的这套 `AGENTS.md`、repo-local skills、Bifrost skill 分发、Skills runtime、验证与 CI 闭环统称为“开发版 Forus Agent/Skills 工程体系”。证据来自当前仓库的 `AGENTS.md`、`SKILL.md`、`skill_remote.md`、`.agents/skills/*`、`docs/agent-skill.md`、`design/agent-skill.md` 和相关源码。

## 一句话结论

这套体系的核心不是某个单一 Agent，而是一个“可被多个 AI 编程工具消费的工程操作系统”：

- `AGENTS.md` 定义开发任务的行为宪法：启动检查、工作区隔离、目标验证清单、human_tests、两轮 Review/Fix/Test、覆盖率、提交/PR/CI 看护。
- `.agents/skills/` 定义项目内的专用技能：E2E、UI/API 验证、Rust 收尾校验、GitHub Actions 诊断、Codex 任务巡检、设计规范、站点 Cookie 登录。
- 根 `SKILL.md` 和 `skill_remote.md` 负责把 Bifrost CLI 能力分发给 Claude Code、Codex、Trae、Cursor、GitHub Copilot 和通用 `.agents/skills` runtime。
- `crates/skills` 和 `crates/agent` 提供内建 Skills runtime：scope、manifest、watcher、authoring、packaging、slash command、prompt digest、memory tools。
- `human_tests/` 把“真实用户感知体验”固化成可执行验收文档，和单测/E2E/CI 形成三层验证。

这套设计的亮点是约束强、证据链完整、平台覆盖广；问题是规则重、学习门槛高、文档和 runtime 状态混杂，且很多闭环仍依赖 Agent 认真执行。

## 总体架构

```text
用户需求
  -> AGENTS.md 判定任务模式和完成定义
  -> repo-local skills 按场景加载
  -> Agent 使用本地命令 / Bifrost CLI / MCP / 浏览器 / GitHub REST / human_tests
  -> 本地验证：unit + E2E + human_tests + rust-project-validate + coverage
  -> 提交 / 推送 / PR
  -> github-actions-pat fail-fast 看护远端 CI
  -> 最终交付证据台账

技能生产与分发
  -> SKILL.md / skill_remote.md / repo-local .agents/skills
  -> bifrost install-skill 覆盖式安装
  -> Claude Code / Codex / Trae / Cursor / GitHub Copilot / .agents/skills
  -> Bifrost Agent runtime 通过 SkillRegistry/SkillsManager 消费
```

## AGENTS.md：开发行为的总控层

`AGENTS.md` 是这套体系里最重要的控制面。它定义的不是代码风格，而是 Agent 如何完成工程任务。

### 任务模式

| 模式 | 触发 | 约束 |
| --- | --- | --- |
| 开发模式 | 实现、修复、优化、改造、提交、推送 | 默认完整开发流程，含提交、PR、CI 看护 |
| 检查模式 | 用户明确“只检查/只分析/不要修改” | 只读，未经同意不得编辑、提交、推送 |
| CI 闭环模式 | 查 CI、修到绿、push 后盯 CI | 必须加载 `.agents/skills/github-actions-pat/` |
| 文档/流程变更模式 | 仅改文档、流程、测试说明 | Rust/E2E/local-ci 可不适用，但 human_tests 仍强制 |

### 完成定义

AGENTS.md 把完成定义压得很具体：

- 开始前执行 `git status --short --branch`。
- 脏工作区默认 worktree 隔离。
- 规划阶段必须有用户目标验证清单。
- Todo/plan 里必须出现 human_tests、两轮 Review/Fix/Test、提交/MR/CI 看护、最终交付自检。
- 所有开发场景必须创建或更新 `human_tests/` 并真实执行。
- 改业务代码必须跑 coverage gate。
- 提交前需要 rust-project-validate 和至少一次 workspace all-features 测试；文档变更可解释为不适用。
- 默认提交、推送、创建/更新 PR，并看 CI 到绿或给出阻塞证据。

这等于把“工程师应该有的责任感”写成了 Agent 的执行协议。

### 优点

- 降低 Agent “写完就走”的概率。
- 强制区分 read-only 和开发交付。
- 通过目标清单减少需求误解。
- 通过两轮 review 降低补丁遗漏。
- 通过 human_tests 保留真实链路知识。
- 通过 CI skill 把远端失败归因变成可执行流程。

### 问题

- 对小任务成本高，尤其是纯学习文档也要 human_tests。
- 规则多，Agent 容易漏掉某个门禁。
- 最终证据仍依赖 Agent 主动记录，缺少机器强制校验。
- 部分规则适合 Bifrost 主仓，但用于其它 Forus 子项目时需要裁剪。

## Repo-local Skills：项目内技能层

当前 `.agents/skills/` 下有 7 个项目内技能：

| Skill | 作用 | 关键约束 |
| --- | --- | --- |
| `e2e-test` | 创建和执行 Bifrost 代理端到端测试 | 新功能/bug 修复后使用；必须优先于 `rust-project-validate`；启动服务要临时数据目录 |
| `e2e-verify` | 管理端 UI/API 验证 | 默认独立代理进程；启动必须 `--no-system-proxy`；优先已有场景 |
| `rust-project-validate` | Rust 项目收尾校验 | 任务结束前调用；在 E2E/API/交互验证之后执行 |
| `github-actions-pat` | GitHub Actions CI 诊断与 fix-push-watch | 只从 `GITHUB_TOKEN` 读 PAT；用 REST API；fail-fast 看护 |
| `design-md` | WebUI/desktop/site/docs 设计一致性 | 修改视觉/交互前读 `DESIGN.md` 和 `design/design-md-system.md` |
| `codex-task-inspector` | Codex 异步任务巡检 | 先探测真实 `CODEX_HOME`/`~/.codex`，不要误用仓库 `.codex-tasks` |
| `site-cookie-login` | 浏览器登录、Cookie 抓取、登录态持久化 | Puppeteer 打开站点，required cookies + HTTP probe 双校验 |

### 技能触发逻辑

技能不是手动“导入库”，而是由任务语义触发：

- 端到端测试、新增 E2E、bug 回归 -> `e2e-test`
- WebUI/API 验证、页面快照、push websocket 排查 -> `e2e-verify`
- 任务收尾、提交前规范校验 -> `rust-project-validate`
- GitHub Actions、CI 失败、PR review、push 后 watch -> `github-actions-pat`
- UI/桌面/site/docs 视觉与交互 -> `design-md`
- Codex 异步任务状态 -> `codex-task-inspector`
- 网站需要浏览器登录 Cookie -> `site-cookie-login`

### 技能约束的共同模式

这些 skill 都不是只给命令，而是写了“执行边界”：

- 先读哪些 reference。
- 什么时候必须调用。
- 什么时候禁止复用共享端口或共享数据目录。
- 如何做验证。
- 输出里必须包含哪些证据。
- 哪些路径是权威事实源。
- token/cookie/日志等敏感信息如何处理。

这说明 Forus 这套体系把 skill 当作“可执行操作规程”，不是简单命令备忘录。

## 关键 Skill 分析

### e2e-test：端到端测试开发规范

它解决的是“怎么写和跑 Bifrost/Forus 类项目的真实链路测试”。

能力：

- 指导新增 E2E 测试。
- 要求规则行为夹具放在 `e2e-tests/rules/`，脚本放在 `e2e-tests/tests/`。
- 要求启动服务用临时 `BIFROST_DATA_DIR`。
- 对 CLI 测试提供黑盒模板：独立数据目录、复用 release binary、grep/jq 断言、trap 清理。
- 场景细分到快速构建、SSE 分批推送、全量测试、单个测试、调试方法、真实人类测试。

好处：

- 避免临时脚本散落根目录。
- 避免污染用户本机 `~/.bifrost`。
- 把 E2E 夹具组织成可维护资产。

风险：

- 文档引用了 `../../rules/project_rules.md`，当前仓库里未检索到这个路径；后续应检查并修正引用。

### e2e-verify：管理端 UI/API 黑盒验证

它是面向 Web 管理端的实操工具包。

能力：

- `browser-test.js scenario --list` 查看场景。
- `browser-test.js scenario stream-sse|stream-ws|traffic-delete` 运行内置 UI 场景。
- `api-test.js` 请求管理端 API。
- `push-debug.js` 抓 websocket/push 证据。
- 独立代理进程、独立端口、独立 `BIFROST_DATA_DIR`。

关键约束：

- 默认不要复用共享 `9900`。
- 手工启动服务必须 `--no-system-proxy`，除非测试目标就是系统代理。
- UI 现象和 API 不一致时，必须抓 `/api/push` websocket frame。

这类 skill 很适合开发版 Forus，因为开发版问题常常不是“代码能不能编译”，而是“真实页面、真实 API、真实推送是否一致”。

### github-actions-pat：远端 CI 闭环

这是整个交付闭环的远端证据工具。

能力：

- 用 GitHub REST API 查询 run / PR / branch / sha。
- 拉 failed job 日志并处理 GitHub job logs 的 302 Azure Blob signed URL。
- `watch_jobs.py` fail-fast：任一 job 失败立即退出并输出归因。
- `gh_review.py` 生成 PR code review markdown，可选 POST。

关键约束：

- PAT 只从 `GITHUB_TOKEN` 环境变量读取。
- 不走 cookie、OAuth device flow、`gh auth login`。
- 不回显完整 workflow log，只输出失败 job/step、根因桶、关键片段和 URL。
- 跑脚本时清代理环境，避免本机 Bifrost MITM 影响 GitHub TLS。
- `--post` 必须有用户明确同意。

这是一条很成熟的 CI automation 线路。它把“看 CI”从浏览器人工点击，变成可重复的 API 工作流。

### rust-project-validate：本地收尾门禁

能力：

- `cargo fmt --all -- --check`
- desktop Tauri 单独 fmt
- `cargo clippy --all-targets --all-features -- -D warnings`
- 按范围运行 E2E、cargo test、build
- 最后至少一次 `cargo test --workspace --all-features`

关键顺序：

- 不应在 API/UI/E2E 还没验证前就跑。
- 它是收尾门禁，不是探索阶段工具。
- 跑之前要确认没有残留 `cargo`、`rustc`、旧 `bifrost` 进程抢锁。

### design-md：设计一致性门禁

能力：

- 修改 WebUI、desktop、site、docs 视觉和交互时触发。
- 要求读 `DESIGN.md` 和 `design/design-md-system.md`。
- 把 `DESIGN.md` frontmatter 当作设计 token 权威来源。
- 强制 operational UI 保持高密度、可扫描、不要营销化。
- 修改 design token 时跑 `pnpm design:lint`。

这让 UI 不会被不同 Agent 改成不同风格，是跨 Agent 协作非常重要的一层。

### codex-task-inspector：异步任务事实源纠偏

能力：

- 先探测 `CODEX_HOME` 或 `$HOME/.codex`。
- 根据 rollout/session id 找真实 jsonl。
- 只在用户明确要求时才检查仓库 `.codex-tasks/`。
- 分开汇报本地 Codex 进程、任务产物、CI 状态和下一步建议。

这类 skill 的价值是“纠正高频路径误判”。它把过去踩过的坑写进流程。

### site-cookie-login：浏览器登录态能力

能力：

- 用 Puppeteer 打开登录页。
- 等待用户完成登录。
- 抓目标域 Cookie。
- 检查 required cookies。
- 用 HTTP probe 验证 Cookie 真实可用。
- 保存到 `.env` 类文件供后续脚本读取。

平台与工具：

- Node.js / Puppeteer。
- 站点配置 JSON。
- HTTP 探针。

这适合没有 API token、必须依赖浏览器 SSO 的平台。

## 根 SKILL.md 与 skill_remote.md：对外分发层

### 根 SKILL.md

根 `SKILL.md` 是 Bifrost 通用技能，重点教外部 AI 工具如何使用 `bifrost` CLI。

覆盖能力：

- 生命周期：start/stop/status。
- TLS/CA、规则、Group、临时端口规则绑定。
- 脚本、变量值、访问控制、代理认证、系统代理、运行时配置。
- 流量查询、搜索、capture wait、status JSON。
- 导入导出、远程同步、Admin 远程访问、IM Gateway。
- `install-skill` 分发。
- Remote 调用入口索引。
- 远程文件 API 的 coding-agent 工作流。

关键约束：

- 正式执行 Bifrost 命令前先自检：确认 `bifrost` 是否存在、必要时安装/升级。
- 不绕过 CLI 直接改底层数据文件。
- 敏感输出不要写入低信任日志或可复用 skill。
- 规则调试要用真实 traffic 证据。

### skill_remote.md

`skill_remote.md` 是远端设备操作专用 skill。

覆盖能力：

- 远端连接、pair code、SSH key。
- 三类 scope：remote query、remote shell exec、remote file read/write。
- 远端 traffic 查询。
- 远端 shell 执行和 job 模型。
- 远端文件读写、find、outline、edit、patch、upload/download。

关键黄金法则：

- 修改远端文件用 `remote file`，不要用 `remote exec + base64`。
- 先读目标工作目录的 `AGENTS.md/agents.md` 和 repo-local skills 元信息。
- 跑测试才用 remote exec；文件修改用 File API。
- 长任务/大输出用 job 模型续接。

这说明 Forus 这套体系不仅支持本机开发，也支持“另一台机器上的 coding agent 级操作”。

## Skills 是如何开发、安装和运行的

### 标准格式

Skill 以 `SKILL.md` 为入口，通常包含 YAML frontmatter：

```yaml
---
name: example-skill
description: When to use this skill and what it does.
---
```

正文写：

- 何时触发。
- 必读 reference。
- 操作步骤。
- 禁止事项。
- 验证命令。
- 常见坑点。
- 输出模板。

Repo-local skill 放在：

```text
.agents/skills/<skill-name>/SKILL.md
.agents/skills/<skill-name>/scripts/
.agents/skills/<skill-name>/references/
```

### 安装分发

`bifrost install-skill` 把根 `SKILL.md` 和 `skill_remote.md` 覆盖安装到多种 AI 编程工具：

| 平台 | 全局路径 |
| --- | --- |
| Claude Code | `~/.claude/skills/bifrost/SKILL.md` |
| Codex | `~/.codex/skills/bifrost/SKILL.md` + `~/.agents/skills/bifrost/SKILL.md` |
| Trae | `~/.trae/skills/bifrost/SKILL.md` + `~/.trae-cn/skills/bifrost/SKILL.md` |
| Cursor | `~/.cursor/skills/bifrost/SKILL.md` |
| GitHub Copilot | `~/.copilot/skills/bifrost/SKILL.md` |
| Universal | `~/.agents/skills/bifrost/SKILL.md` |

支持：

- `--tool/-t`
- `--dir/-d`
- `--cwd`
- `--yes`
- `BIFROST_INSTALL_SKILL_SOURCE`
- `BIFROST_INSTALL_SKILL_DIR`

实现层有网络下载和 embedded fallback：GitHub raw 下载失败时回退内嵌副本，升级后也会 best-effort 自动安装 skills。

### 内建 runtime

`crates/skills` 和 `crates/agent` 让 Bifrost 自身也能消费 skill：

- `SkillManifest`：name、version、description、scope、allowed tools、slash command、entrypoint。
- `SkillScope`：System、Global、User、Repo，Repo 优先级最高。
- `SkillRegistry`：加载/监听 skills，支持 watcher 精准 reload。
- `SkillStore`：多 scope root 聚合。
- `SkillAuthoringSession`：start -> draft -> validate -> test -> commit 状态机。
- `SkillPackager`：导入/导出 skill 包。
- `SkillsManager::build_skills_instructions()`：只注入 name/description/path digest，不 eager 注入正文。
- `SlashCommandRouter`：把 skill 声明的 slash command 接入 session。
- `SkillToolBridge`：按 manifest allowed tools 控制 memory read/write 等能力。

这套 runtime 的设计重点是“渐进式披露”：让模型先知道有哪些 skill 和路径，真正需要时再读 `SKILL.md`，避免 prompt 膨胀。

### 技能开发流程

开发一个新 skill 时，建议按这条路径：

1. 明确触发条件：用户说什么、改什么文件、遇到什么任务时必须用。
2. 写 `SKILL.md` frontmatter：`name` 和 `description` 要能被搜索和自动发现。
3. 把长脚本放进 `scripts/`，不要把大段代码塞正文。
4. 把平台坑点、配置模板放进 `references/`。
5. 明确前置检查、权限、敏感信息规则。
6. 明确输出模板和失败处理。
7. 增加 human_tests 覆盖：skill 是否被索引、触发条件是否清楚、脚本是否可运行、文档是否与 CLI 参数同步。
8. 如果进入 Bifrost runtime，还要走 manifest 校验、authoring test、packager/import、Web editor 或 Admin API 测试。

## 用了哪些平台

| 平台 | 用途 |
| --- | --- |
| GitHub / GitHub Actions | PR、CI、workflow run、job logs、code review |
| GitHub REST API | `github-actions-pat` 查询 run/jobs/logs/reviews |
| Azure Blob signed URL | GitHub job log 302 后实际日志下载位置 |
| Claude Code | skill 分发目标 |
| OpenAI Codex | skill 分发目标、异步任务巡检对象 |
| Trae / Trae CN | skill 分发目标、外部 runner/开发工具 |
| Cursor | skill 分发目标 |
| GitHub Copilot | skill 分发目标 |
| Universal `.agents/skills` runtime | 通用技能目录 |
| Bifrost CLI | 本机代理、规则、流量、IM、remote 的统一操作入口 |
| Bifrost Admin API/WebUI | UI/API/E2E 验证对象 |
| Feishu / Weixin IM Gateway | provider、Agent 通道、上线通知、消息卡片 |
| Chrome / Puppeteer / Playwright | UI 验证、登录 Cookie、浏览器自动化 |
| MCP | 外部工具、resources、memory tools、Chrome DevTools MCP |

## 用了哪些工具和能力

### 工程命令

- `git status` / `git diff` / `git worktree` / `git commit` / `git push`
- `rg` / `sed` / shell
- `cargo fmt` / `cargo clippy` / `cargo test` / `cargo build`
- `pnpm` / Node.js
- `make coverage` / `scripts/ci/coverage-all.sh`
- `scripts/ci/local-ci.sh`

### 测试能力

- Rust 单元测试。
- Shell E2E。
- `bifrost-e2e` Rust E2E runner。
- Playwright/UI 验证。
- `e2e-verify` 的 browser/api/push-debug 脚本。
- `human_tests/` 真实场景测试。

### 自动化与诊断能力

- GitHub Actions REST polling。
- fail-fast job watcher。
- PR diff review generator。
- Codex rollout/session jsonl 巡检。
- Cookie 登录态持久化。
- Bifrost traffic capture/search/export/replay。
- Remote file/shell/job 操作。

### 安全与边界能力

- PAT 只读环境变量，不落盘不回显。
- Cookie 写 `.env`，用 required cookies + HTTP probe 验证。
- Bifrost remote 用 scope 和 policy 控制 shell/file/traffic 能力。
- E2E 使用临时数据目录，避免污染本机。
- UI/API 验证默认 `--no-system-proxy`，避免测试破坏网络。
- Skills runtime 用 scope 和 allowed tools 限制能力。

## 亮点

### 1. 规则和 skill 分层清晰

`AGENTS.md` 管总流程，`.agents/skills` 管专业场景，根 `SKILL.md` 管对外 CLI 能力，`skill_remote.md` 管远端设备。这种分层避免一个超大提示词包打天下。

### 2. Skills 不是文档，而是操作规程

每个 skill 都包含何时用、怎么用、哪些坑、怎么验证、什么不能做。它更像 runbook，而不是 API 文档。

### 3. 平台覆盖广

同一套 Bifrost/Forus 能力可以分发给 Claude Code、Codex、Trae、Cursor、GitHub Copilot、通用 `.agents/skills`，避免绑定单一 AI 工具。

### 4. 强证据闭环

从 `git status`、diff、human_tests、E2E、本地校验、PR、CI run id 到最终交付摘要，设计上每一步都要留下证据。

### 5. 远端能力设计成熟

`skill_remote.md` 把远端连接、文件读写、shell、job、traffic 查询都分成不同 scope，并明确“改文件用 remote file，不用 remote exec + base64”。这是很重要的安全边界。

### 6. 渐进式披露降低上下文成本

Skills runtime 不 eager 注入所有 skill 正文，只注入 digest 和路径；需要时再读。这是大规模 skill 体系能跑起来的关键。

## 问题和风险

### 1. AGENTS.md 规则过重

优点是质量高，缺点是任何小任务都可能被拉进完整交付流程。对开发版 Forus 来说，如果任务类型很多，最好补充轻量任务豁免矩阵。

### 2. Skill 引用存在漂移风险

例如 `e2e-test` 引用 `../../rules/project_rules.md`，当前仓库检索不到这个路径。repo-local skill 一旦引用旧路径，Agent 会在执行时浪费时间或误判。

建议增加一个 skill lint：检查 `SKILL.md` 中相对链接是否存在。

### 3. 平台路径太多

Claude/Codex/Trae/Cursor/Copilot/Universal 的路径都要维护，`install-skill` 一旦漏一个目标或目标工具改标准，就会出现“某些 Agent 学不到最新技能”的问题。

建议把路径矩阵和测试夹具保持强绑定。

### 4. CI skill 和 AGENTS 有重复

`AGENTS.md` 和 `.agents/skills/github-actions-pat/SKILL.md` 都写了 CI 闭环。重复有利于发现，但也容易漂移。

建议把 AGENTS 保留原则和入口，具体命令尽量只在 skill 中维护。

### 5. Skill 开发文档偏实现，不够教学化

`design/agent-skill.md` 很完整，但新同学要从“如何写第一个 skill”开始仍然会有压力。

建议新增一篇短教程：`docs/skill-authoring-quickstart.md`，用一个最小 skill 展示 frontmatter、scripts、references、human_tests、install、runtime import。

### 6. Human tests 规模大但机器可读性弱

`human_tests/readme.md` 索引很强，但执行记录多为自然语言。后续可以定义轻量结构：

```text
执行记录（YYYY-MM-DD）：PASS
命令：
结果：
证据：
```

这样能让 Agent 自动抽取“哪些用例最近执行过”。

## 学习路径

建议按这个顺序理解：

1. `AGENTS.md`
   先理解这套体系的完成定义和强制门禁。

2. `.agents/skills/e2e-test/SKILL.md`
   学 E2E 目录、临时数据目录、测试组织方式。

3. `.agents/skills/github-actions-pat/SKILL.md`
   学远端 CI 如何用 REST API 做 fail-fast 闭环。

4. `.agents/skills/rust-project-validate/SKILL.md`
   学本地收尾验证顺序。

5. `docs/agent-skill.md`
   学 skill 如何安装到 Claude/Codex/Trae/Cursor/Copilot。

6. `design/agent-skill.md`
   学 Skills runtime、manifest、registry、authoring、packager、Admin/Web。

7. `SKILL.md` 和 `skill_remote.md`
   学对外分发给 AI 工具的 Bifrost 本机/远端能力。

8. `crates/skills/src/model.rs`、`registry.rs`、`authoring.rs`、`packager.rs`
   学实现细节。

9. `crates/agent/src/session.rs`、`crates/agent/src/skills/*`
   学 Agent session 如何加载 repo-local skills 和 slash command。

## 建议改进清单

- 增加 `scripts/ci/check-skills-links.sh`，扫描 `.agents/skills/*/SKILL.md` 和 `docs/agent-skill.md` 的相对链接。
- 新增 skill authoring quickstart，用最小可运行 skill 教新同学。
- 给 `human_tests` 执行记录加半结构化模板。
- AGENTS 中保留 CI 原则，CI 命令细节收敛到 `github-actions-pat` skill，降低漂移。
- 给 repo-local skills 建一个总览索引文档，说明每个 skill 的触发词、输入、输出、依赖平台和维护人。
- 为 Forus 子项目补充“哪些 AGENTS 规则继承，哪些降级”的项目画像，避免把 Bifrost 主仓重型门禁原样套到所有仓库。

## 本次分析依据

- `AGENTS.md`
- `SKILL.md`
- `skill_remote.md`
- `docs/agent-skill.md`
- `design/agent-skill.md`
- `.agents/skills/e2e-test/SKILL.md`
- `.agents/skills/e2e-verify/SKILL.md`
- `.agents/skills/rust-project-validate/SKILL.md`
- `.agents/skills/github-actions-pat/SKILL.md`
- `.agents/skills/design-md/SKILL.md`
- `.agents/skills/codex-task-inspector/SKILL.md`
- `.agents/skills/site-cookie-login/SKILL.md`
- `crates/bifrost-cli/src/commands/install_skill.rs`
- `crates/skills/src/model.rs`
- `crates/skills/src/authoring.rs`
- `crates/skills/src/tool_bridge.rs`
- `crates/agent/src/session.rs`
