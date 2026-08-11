# Bifrost Remote Skill

> 实现状态：已发布 (implemented, refreshed against code as of 2026-07-03)。
> 主 CLI 逻辑位于 `crates/bifrost-cli/src/commands/install_skill.rs`；产物 SKILL 文本位于仓库根
> `skill_remote.md`（remote skill）与 `SKILL.md`（主 skill）。E2E 与真实场景在
> `human_tests/skill-remote.md` 与 `human_tests/remote-file-transfer.md`。

## 背景

`bifrost install-skill` 早期只安装根目录 `SKILL.md`。这份 skill 覆盖了通用 Bifrost CLI 管理能力，
但把 remote invoke（`bifrost remote ...`）的细节也塞在一起，导致：

- 触发词、能力边界与 caller/target 前置准备表达不清；
- 对 Agent 而言，remote 场景需要一段独立的、更聚焦的 skill 才能稳定触发；
- `bifrost remote` 命令面（`remote exec / remote file / remote job / remote run / remote traffic /
  remote conn`）在 caller 与 target 侧的语义、错误恢复、TLS 信任、连接身份漂移都需要显式说明。

`bifrost-remote` 是与 `bifrost` 并列的 sibling skill，专注 remote invoke 场景：指导 Agent 正确安装、
授权、执行远端操作，并覆盖长任务 / 断线恢复 / relay TLS / login shell / CA 注入等实战细节。
安装器 `bifrost install-skill` 必须同时分发主 skill 与 remote skill 两个 SKILL.md。

## 用户目标验证清单

### 必须实现

- `bifrost install-skill` 默认全局、项目本地安装时，把 `bifrost/` 与 `bifrost-remote/` 两个目录都写入
  目标位置（Codex 全局：`~/.codex/skills/bifrost/SKILL.md`、`~/.codex/skills/bifrost-remote/SKILL.md`；
  agents 全局：`~/.agents/skills/bifrost/SKILL.md`、`~/.agents/skills/bifrost-remote/SKILL.md`）。
- `--dir <path>` 保持主 skill 语义：`<path>/SKILL.md` 是主 skill，remote skill 写入 `<path>` 的
  sibling `bifrost-remote/SKILL.md`。
- Remote skill 文本清晰区分：
  - 只读查询（`remote traffic list/get/search`、`remote conn status`、`remote file read/list/stat/glob/search/hash`）
    的前置条件（目标端启用 Remote Invoke 授权）；
  - 远端设备控制（`remote exec` / `remote run` / `remote file write/edit/patch/upload/download` /
    `remote job list/status/logs/watch`）的额外前置（目标端启用 Shell Access profile/policy 并授权
    `selected` 或 `all`）。
- Remote skill 明确 caller 本地管理命令（`bifrost setting ...`）与目标设备控制路径（`remote exec`）
  的区别，避免 Agent 在 caller 本机误执行管理命令。
- 要求 Agent 在执行任何远端工程任务前，先读目标工作目录下的 `AGENTS.md` / `agents.md` 与
  `.agents/skills/*/SKILL.md` 元信息；skill 详细正文按需加载。
- Remote skill 覆盖 relay HTTPS 信任（`BIFROST_REMOTE_RELAY_CA_BUNDLE` + 系统 CA + 兜底
  `BIFROST_REMOTE_UNSAFE_SSL=1`），并说明兜底只作用于 relay client，不等同代理 `--unsafe-ssl`。
- Remote skill 把 `remote exec --detach` + `remote job list/status/logs/watch` 作为长任务与断线恢复
  主路径，不要求用户手工复制 `--relay-token`。
- Remote skill 覆盖 `remote run --script-file <local> --interpreter <program> -- <args>`：caller 侧
  编排能力必须走 `remote file scratch-dir` + `remote file write` + `remote exec` argv 模式，不使用
  heredoc / shell 拼接。

### 必须不破坏

- 主 skill `SKILL.md` 的现有触发词、内容边界不变；两个 skill 互不重复触发词，避免同时出现在
  Agent 提示中造成噪音。
- `install-skill` 只带 `--dir` 时不改动其他非 sibling 位置的 skill 目录。
- CLI 未安装 remote skill 前的 `bifrost remote ...` 行为不变；skill 只是让 Agent 更容易正确调用。
- 已安装 `bifrost-remote` skill 的 Agent 在无 remote 意图时不应把 remote skill 提示强注入，
  遵循原有 skill 匹配机制。

### 必须真实验证

- 单元测试覆盖：`install_skill` 同时分发主 skill 与 remote skill；`bifrost remote run` 参数解析；
  `remote file write/edit/patch --from-local` 别名；`--client-id` 前缀匹配；idle timeout 文案。
- E2E 覆盖：真实 CLI 执行安装后两个 SKILL.md 都存在，且 remote SKILL.md 包含关键能力段落。
- 真实场景 `human_tests/skill-remote.md` 覆盖三类前置条件、long-running detach、断线恢复、
  scratch-dir、read-many policy fallback、`--login` 与 relay TLS 兜底。

## 产品语义

### 两个并列 skill 目录

| Skill 目录 | 来源文件 | 用途 | 主要触发词 |
| --- | --- | --- | --- |
| `bifrost/` | `SKILL.md` | 通用 Bifrost CLI 管理能力 | 启动/停止、规则、证书、流量查询、系统代理、远程连接管理 |
| `bifrost-remote/` | `skill_remote.md` | Remote Invoke 专用 | 连接另一台电脑、远程执行命令、远端仓库编辑/重构/批量修改文件、远程 grep/find/read/write/edit/patch/upload/download、`remote job watch`、断线恢复 |

`bifrost-remote/SKILL.md` **必须** 是 `bifrost/SKILL.md` 的 sibling 目录，两者共享同一根路径。
Codex 与 agents 全局路径均需要写入。

### 面向用户正式实例的启动约定

Remote skill 明确面向用户正式安装后的 remote target 场景，默认推荐使用：

- 默认数据目录；
- 默认端口（`9900`）；
- 默认系统代理启动行为。

原因：Remote Invoke、Web UI 授权与设备流量采集都应落在同一个用户预期的本机实例上。
只有测试或临时验证场景才使用临时数据目录、非 9900 端口、`--no-system-proxy`。

### 授权模型三层

- **UI access mode**：`query` / `selected` / `all`。
- **协议 grant scope**：`remote_query` / `remote_shell_exec` / `remote_shell_interactive`。
- **前置条件三类**：
  - 查询类：目标 Bifrost 已启动、Relay Connection 在线、目标端 Remote Invoke 页面已通过 SSH key
    或 pair code 授权 caller。可用 `bifrost remote conn status` 验证。
  - shell 执行类：满足查询前置，且目标端已配置 Shell Access profile/policy，授权请求选择 `selected`
    或 `all`，必要时开启 stdin/interactive。
  - 文件操作类：目标端 File Access policy 授权 read 或 read-write；修改远端文件必须优先使用
    `bifrost remote file` 子命令。

### 长任务与断线恢复

- `remote exec --detach` 返回 `call_id`，caller 本地加密保存 relay token。
- 断线后 `remote job list/status/logs/watch` 用于恢复观察、读取日志、取得真实远端 exit code。
- 真实退出码来自 job / watch，不来自断开的 stream。
- Caller 侧 call events idle deadline 是 **300 秒无事件超时**，不是远端进程总 timeout；错误提示必须
  引导长时间静默任务使用 `exec --detach` / `run --detach` + `job watch --output-file`。
- Skill 明确禁止 `remote job ... --relay-token <token>` 手工复制 token 的示例；relay token 完全由
  caller 本地 job cache 管理。

### 连接恢复边界

- **续接已有 call**（不重建连接）：客户端 stream 断开、digest mismatch、本地切线程。
  → 优先 `remote job logs/watch/status`。
- **重建连接**：grant / authorization / transport identity 失效。
  → `remote conn down` + `remote conn up`。

### 调用异常时主动更新 skill

当 `bifrost remote` 出现子命令不存在、参数不兼容、协议错误、行为与本地 skill 不一致等异常时，
Agent 必须主动获取远端最新 `skill_remote.md`，以
`https://github.com/bifrost-proxy/bifrost/blob/main/skill_remote.md` 为权威入口核对命令面、参数、
错误码和恢复流程，避免用过期本地 skill 误判能力不存在。

### Relay TLS 信任顺序

1. 系统 CA（remote relay client 默认读取，无需配置）。
2. 显式 CA：`BIFROST_REMOTE_RELAY_CA_BUNDLE=<path>` 或常见 CA env（`SSL_CERT_FILE` / `REQUESTS_CA_BUNDLE`）
   追加私有根证书。
3. 兜底：`BIFROST_REMOTE_UNSAFE_SSL=1` 跳过 relay client 证书信任校验。
   - 只作用于 remote relay HTTP/SSE client；
   - 不等同代理服务 `--unsafe-ssl`；
   - 文档必须说明这是最终兜底，携带风险提示。

### Shell 环境语义

- 默认 shell-text 使用 **非 login shell**，保持 stdout 干净。
- 需要用户 PATH / rc 时显式 `--login`。
- CLI 会向远端注入 `BIFROST_REMOTE=1`、`TERM=dumb` 等降噪环境变量。
- `--cwd` 在 shell 内部再次生效。

### 本地脚本远端执行

`remote run --script-file <local> --interpreter <program> -- <args>`：

- caller 侧编排：先 `remote file scratch-dir` 拿到临时目录，再 `remote file write` 上传脚本，
  最后 `remote exec` 以 argv 模式执行。
- 不新增底层 relay 协议，也不允许退化为 heredoc / shell 拼接。
- 脚本参数（`--` 之后）不经过 shell 拼接，直接作为 argv 传给 interpreter。

### 多远端协作

- `--client-id` 是 `remote` **父命令** 参数，放在 `remote` 后、子命令前。
- 支持 client id 前缀与设备 label / name 前缀匹配。
- 支持 `BIFROST_REMOTE_CLIENT_ID` 作为默认目标。
- 多连接候选时非交互模式给出提示列出 label + name，避免歧义。

### CLI 易错兼容

- `remote file write --path <p>` 兼容位置参数等价形式；重复传位置参数和 `--path` 时返回可操作错误。
- `remote file write/edit/patch` 均支持 caller 侧 `--from-local <path|->`：
  - `write --from-local` 等价 `--content-file`；
  - `edit --from-local` 读取 edits JSON 数组；
  - `patch --from-local` 等价 `--patch-file`；
  - `mkdir/move/delete` 不存在 caller 本地 payload，不提供该参数。

### 写操作边界

- `traffic.clear` 是写操作，**不提供** `bifrost remote traffic clear` 子命令。
  需要清理目标端记录时，必须先取得 shell 授权，再通过 `remote exec` 执行目标端本机命令或 API。
- rule / config / script / ca / value / 系统代理等没有专门 `bifrost remote <module>` 子命令时，
  通过 `remote exec` 在目标终端执行等价本机命令。
- Shell Access policy / profile 等 caller 本机管理命令使用 `bifrost setting ...`；
  caller 管理目标设备时切换到 `remote exec`。

## 技术细节

### 安装器分发

`crates/bifrost-cli/src/commands/install_skill.rs`：

- 使用 `BIFROST_INSTALL_SKILL_SOURCE=embedded` 默认从嵌入的资源加载；也支持从 network / dir 加载。
- 安装流程：
  1. 解析 `--tool` / `--dir` / `-y` 选项。
  2. 决定主 skill 与 remote skill 的目标路径（sibling）。
  3. 写入 `SKILL.md` 与 `bifrost-remote/SKILL.md`；写入前若已存在，交互模式提示 overwrite，
     `-y` 直接覆盖。
  4. 打印安装总结，包括路径与 skill 名。

关键测试入口在 `crates/bifrost-cli/tests/cli_commands.rs`：

- `install_skill_installs_remote_skill_from_embedded_bundle` (line 1472)
  - `handle_install_skill(...)` 后断言 `<tmp>/skills/bifrost/SKILL.md` 与
    `<tmp>/skills/bifrost-remote/SKILL.md` 均存在；后者含 `name: "bifrost-remote"`。
- `install_skill_listed_in_help` (line 1434)
- `install_skill_options_parse` (line 1443)
- `completions_install_skill_tool_values` (line 1310)

### `remote run` 参数解析

- `remote_run_parses_script_upload_options_and_args` (line 634)
- `remote_run_builds_argv_exec_payload_for_uploaded_script` — 上传后脚本以 argv_exec 方式执行，
  参数不经 shell 拼接。

### `remote file write/edit/patch --from-local`

- `remote_file_write_parses_from_local_alias` (line 687)
- `remote_file_edit_parses_from_local_edits_file` (line 722)
- `remote_file_patch_parses_from_local_alias` (line 756)
- `file_write_accepts_path_flag_compatibility_alias`
- `file_write_rejects_duplicate_positional_and_path_flag`
- `file_edit_reads_edits_from_local_file`

### `--client-id` 与连接解析

- `resolve_local_connection_explicit_device_label_prefix_matches`
- `resolve_local_connection_multiple_noninteractive_lists_choices`

### Idle timeout 提示

- `idle_timeout_message_mentions_seconds_and_job_watch` — 300 秒 idle timeout 错误提示应包含
  detach / job watch 引导。

## CLI + Web + Admin API

### CLI（caller 侧）

- 安装：`bifrost install-skill --tool universal [--dir <path>] [-y]`。
- 远程：`bifrost remote [--client-id <id>] <subcommand>`
  - `conn status/down/up`
  - `traffic list/get/search`
  - `exec [--detach] [--login] [--cwd <p>] [-e K=V] [--stdin] -- <cmd> <args...>`
  - `run [--detach] --script-file <local> --interpreter <program> [--cwd <p>] -- <args...>`
  - `file scratch-dir | list | stat | glob | search | hash | read | write [--path <p>] [--from-local <p>] |
    edit [--from-local <p>] | patch [--from-local <p>] | upload | download | mkdir | move | delete`
  - `job list | status | logs | watch [--output-file <p>]`

### CLI（target 侧管理）

- 本机管理走 `bifrost setting shell-access ...`、`bifrost setting file-access ...`、
  `bifrost setting remote-invoke ...`（与 Shell Access UI / File Access UI 一致）。
- 通过 `remote exec` 触发的目标端命令等价于登录目标机执行本机 CLI。

### Web

Remote Invoke UI（`Settings → Remote Invoke`）负责授权 caller：

- SSH key 或 pair code 两种模式；
- 授权范围 `query` / `selected` / `all`；
- Shell Access profile 与 File Access policy 单独页面管理。

Skill 文本必须引导用户通过 UI 授权，而不是猜测隐式 grant 状态。

### Admin API

Remote skill 层不新增 Admin API；所有能力走 `bifrost remote` CLI，底层复用现有 Remote Invoke
relay 协议。

## Sync 边界

- Remote skill SKILL.md 属于 CLI 安装产物，**不进入 Bifrost Sync**。
- Skill 更新只能通过 `bifrost install-skill` 覆盖式重装，或从
  `https://github.com/bifrost-proxy/bifrost/blob/main/skill_remote.md` 手动拉取更新。
- Relay grant / Shell Access policy / File Access policy 是本机资产，不通过 Sync 上传。
- Job cache（含 relay token）在 caller 本机加密保存，不进入任何 Sync 通道。

## 实现切分

### Phase 1：分发器

- `install_skill` 增加 sibling 目录写入逻辑。
- 增加 `install_skill_installs_remote_skill_from_embedded_bundle` 单测。
- 覆盖 `--dir <path>` sibling 语义。

### Phase 2：Remote skill 文本

- 起草 `skill_remote.md`：
  - 触发词、能力边界、caller/target 前置准备。
  - 三类前置条件表。
  - `AGENTS.md` / `agents.md` / `.agents/skills/` 读取要求。
  - Relay TLS 三层信任。
  - 长任务 detach + job cache。
  - Shell 环境语义。
  - `remote run` + scratch-dir + argv 模式。
  - `--from-local` 别名与错误恢复。
- 提供更新入口链接与 fallback 指引。

### Phase 3：Skill 内容对齐 CLI 单测

- 每个 skill 章节都配对一条 cli_commands.rs 单测断言 skill 文本包含关键语句
  （例如 `assert!(remote_skill_content.contains("--from-local"))`）。
- 目的：CLI 参数或行为变更时能强制驱动 skill 更新。

### Phase 4：真实场景 + 更新链路

- `human_tests/skill-remote.md` 覆盖三类前置、long-running、断线恢复、scratch-dir、read-many
  fallback、`--login`、relay TLS 兜底。
- 主 skill `SKILL.md` remote 小节仅保留“更详细的用法请查看 `bifrost-remote` skill”，避免与 remote
  skill 互相矛盾。

## 测试方案

### 单元测试

- `crates/bifrost-cli/tests/cli_commands.rs`（已覆盖）
  - `install_skill_installs_remote_skill_from_embedded_bundle` (1472)
  - `install_skill_listed_in_help` (1434)
  - `install_skill_options_parse` (1443)
  - `completions_install_skill_tool_values` (1310)
  - `remote_run_parses_script_upload_options_and_args` (634)
  - `remote_run_builds_argv_exec_payload_for_uploaded_script`
  - `remote_file_write_parses_from_local_alias` (687)
  - `remote_file_edit_parses_from_local_edits_file` (722)
  - `remote_file_patch_parses_from_local_alias` (756)
  - `file_write_accepts_path_flag_compatibility_alias`
  - `file_write_rejects_duplicate_positional_and_path_flag`
  - `file_edit_reads_edits_from_local_file`
  - `resolve_local_connection_explicit_device_label_prefix_matches`
  - `resolve_local_connection_multiple_noninteractive_lists_choices`
  - `idle_timeout_message_mentions_seconds_and_job_watch`

Skill 文本断言（在 install_skill 单测里）：

- `remote_skill.contains("name: \"bifrost-remote\"")`
- `!remote_skill.contains("deprecated")`
- `!remote_skill.contains("traffic.clear")` / `!contains("bifrost remote traffic clear")`
- `remote_skill.contains("AGENTS.md")` 且 `contains(".agents/skills/")`
- `remote_skill.contains("exec --detach")` 且 `contains("remote job")` 且 `contains("job watch")`
- `!remote_skill.contains("--relay-token")`
- `remote_skill.contains("BIFROST_REMOTE_RELAY_CA_BUNDLE")` 且
  `contains("BIFROST_REMOTE_UNSAFE_SSL")`
- `remote_skill.contains("https://github.com/bifrost-proxy/bifrost/blob/main/skill_remote.md")`
- `remote_skill.contains("--from-local")` 且 `contains("scratch-dir")`
- `remote_skill.contains("--login")`

### E2E 测试

- `cargo run -p bifrost-cli -- install-skill --tool universal --dir <tmp>/skills/bifrost -y`
  - 断言 `<tmp>/skills/bifrost/SKILL.md` 与 `<tmp>/skills/bifrost-remote/SKILL.md` 均存在。
  - 断言 remote skill 包含 `--from-local`、`AGENTS.md`、`.agents/skills/`、`remote exec --detach`、
    `job list/status/logs/watch`、`scratch-dir`、`file.op_not_permitted`、`--login`、
    连接断开恢复段落。
- Shell CLI E2E 覆盖：`remote run --help`、`remote --client-id <id> run --script-file ...`、
  `remote file write --path <p>` 兼容入口、`remote exec` idle timeout 文案可见性。
- 相关脚本：`e2e-tests/tests/test_remote_relay_tls_trust_e2e.sh`（TLS 信任）、
  `crates/bifrost-e2e/src/tests/install_skill.rs`。

### 真实场景测试

`human_tests/skill-remote.md` 覆盖：

- 安装后 remote skill 可发现（Codex / agents 两处）。
- Remote skill 面向用户正式场景推荐默认目录 + 系统代理启动。
- Remote skill 明确三类 scope 前置准备。
- Remote skill 不把 `remote traffic clear` 描述为 remote CLI 子命令；说明它是写操作。
- Remote skill 使用当前 `bifrost setting ...` 命名空间描述本机管理，`remote exec` 描述目标设备管理，
  不包含 `deprecated` 历史版本信息。
- Remote skill 强制要求任何远端工程任务开始前读 `AGENTS.md` / `agents.md` 与 `.agents/skills/`
  全量 skill 元信息，详细 skill 内容按需加载。
- Remote skill 指导 coding agent 使用长任务 detach、job cache 续接、断线后 job list/watch/logs/status
  恢复、scratch-dir 安全临时目录、read-many policy deny 降级、login shell 降噪、连接身份漂移时重连。
- Remote skill 指导 coding agent 在需要远端执行本地脚本时使用 `remote run`；多远端任务固定
  `--client-id` 或 `BIFROST_REMOTE_CLIENT_ID`；caller 侧避免 sleep 轮询。
- Remote skill 指导 coding agent 在 relay TLS 失败时优先系统 / 显式 CA，兜底才用
  `BIFROST_REMOTE_UNSAFE_SSL=1`。

## Review / Fix / Test 闭环

- 第 1 轮：跑 `install_skill_installs_remote_skill_from_embedded_bundle` 与 skill 文本断言；
  人工 diff `skill_remote.md` 与 `SKILL.md`。
- 第 2 轮：跑 `remote_*` 参数解析 / `--from-local` / `resolve_local_connection` 单测；
  E2E 安装 + 真实机器 remote job watch。
- 第 3 轮：真实场景走 `human_tests/skill-remote.md` 全部小节；抽查 relay TLS 三层信任在无 root CA
  环境下的行为。
- 校验命令：
  - `cargo test -p bifrost-cli install_skill_installs_remote_skill_from_embedded_bundle --test cli_commands`
  - `cargo test -p bifrost-cli --test cli_commands remote_ -- --nocapture`
  - `cargo fmt --all -- --check`
  - `cargo clippy --workspace --all-targets --all-features -- -D warnings`
  - 修改范围涉及 CLI 安装器时，最终按本地 CI 策略执行 `bash scripts/ci/local-ci.sh --skip-e2e`。

## 风险与决策

- **风险：skill 内容与 CLI 行为漂移**。缓解：单测直接断言 skill 文本片段；CI 修改 CLI 时会被强制
  失败提醒同步 skill。
- **风险：安装覆盖用户手工修改**。缓解：交互模式明确提示 overwrite；`-y` 模式在 dry-run 输出预览
  路径，用户可提前备份。
- **风险：过期 skill 导致 Agent 误判能力**。缓解：skill 内部包含更新入口 URL，Agent 遇异常时
  会主动拉取最新版本对齐。
- **风险：relay token 泄漏**。缓解：token 由 caller 本地加密 cache 管理；文档禁止手工复制 token；
  skill 文本单测断言无 `--relay-token` 示例。
- **决策：sibling skill 目录**。理由：与主 skill 解耦触发词，避免 skill 描述过长；两个 skill 各自独立
  更新版本。
- **决策：`traffic.clear` 走 shell 授权**。理由：这是写操作，与 read-only 查询语义不同，必须显式
  升级授权路径，避免默认 query grant 用户不知不觉执行写操作。
- **决策：`BIFROST_REMOTE_UNSAFE_SSL` 仅作用于 relay client**。理由：与代理 `--unsafe-ssl` 语义分离
  可以让用户仅信任 relay 而不放宽代理层。

## 文档更新要求

- 更新 `skill_remote.md`（remote skill 权威文本）。
- 更新 `SKILL.md` 的 remote 小节：只保留“详细用法请查看 `bifrost-remote` skill”，避免与 remote skill
  互相矛盾。
- 更新 `human_tests/readme.md` 索引，确保 `skill-remote.md` 可查。
- README / docs 侧若列出所有 skill 类别，需补 `bifrost-remote` 条目。
