# Bifrost Remote Skill

## 功能模块说明

`skill_remote.md` 是面向 Agent Skills runtimes 的独立远程调用技能。它用于用户安装 Bifrost skills 后，让 Agent 能够正确处理以下场景：

- 指导目标终端安装并启动 Bifrost remote invoke 所需的服务。
- 通过 SSH key 或 pair code 建立 caller 到 target 的 remote invoke 授权。
- 执行当前 relay 支持的远端只读查询和受控 `shell.exec`，并在授权范围内操作目标设备。
- 明确两类操作的前置准备：只读查询需要目标端启用 Remote Invoke 授权；远程设备控制还需要目标端启用 Shell Access profile/policy 并授权 `selected` 或 `all` 访问模式。
- 明确区分 caller 本地管理命令与通过 `remote command exec` 操作目标设备的路径，避免 Agent 在 caller 本机误执行管理命令。
- 要求 Agent 在执行任何远端工程任务前，先阅读目标工作目录下的 `AGENTS.md` / `agents.md` 手册，并读取 `.agents/skills/` 下所有 skill 元信息；skill 详细正文只在任务命中时按需加载。

## 实现逻辑

### Skill 分发

`bifrost install-skill` 原先只安装根目录 `SKILL.md`。本模块要求安装器同时分发两个 skill：

| skill 目录 | 来源文件 | 用途 |
| --- | --- | --- |
| `bifrost/` | `SKILL.md` | 通用 Bifrost CLI 管理能力 |
| `bifrost-remote/` | `skill_remote.md` | remote invoke 专用能力 |

默认全局、项目本地安装时，`bifrost-remote` 必须作为 `bifrost` 的 sibling skill 目录写入。例如 Codex 全局安装应写入：

- `~/.codex/skills/bifrost/SKILL.md`
- `~/.codex/skills/bifrost-remote/SKILL.md`
- `~/.agents/skills/bifrost/SKILL.md`
- `~/.agents/skills/bifrost-remote/SKILL.md`

`--dir <path>` 保持原有主 skill 语义：`<path>/SKILL.md` 是主 skill，remote skill 写入 `<path>` 的 sibling `bifrost-remote/SKILL.md`。

### 目标终端启动语义

面向用户正式安装后的 remote target 指引，应默认使用用户的正式 Bifrost 实例：

- 默认数据目录。
- 默认端口。
- 默认系统代理启动行为。

原因是 Remote Invoke、Web UI 授权与设备流量采集应落在同一个用户预期的本机实例上。只有测试或临时验证场景才使用临时数据目录、非 9900 端口和 `--no-system-proxy`。

### Remote skill 内容边界

`skill_remote.md` 描述 relay-backed remote invoke 能力：

- 支持：`remote conn status/down/up`、`remote traffic list/get/search`、`remote exec`、`remote file ...`。
- 查询类前置条件：目标 Bifrost 已启动、Relay Connection 在线、目标端 Remote Invoke 页面已通过 SSH key 或 pair code 授权 caller，并可用 `bifrost remote conn status` 验证。
- shell 执行类前置条件：满足查询类前置条件，且目标端已配置 Shell Access profile/policy，授权请求选择 `selected` 或 `all`，必要时开启 stdin/interactive。
- 文件操作类前置条件：目标端 File Access policy 授权 read 或 read-write，修改远端文件时优先使用 `bifrost remote file`。
- `traffic.clear` 是写操作，不提供 `bifrost remote traffic clear` 子命令；需要清理目标端记录时，必须先取得 shell 授权，再通过 `remote exec` 执行目标端本机命令或 API。
- rule/config/script/ca/value/系统代理等没有专门的 `bifrost remote <module>` 子命令时，应通过 `remote exec` 在目标终端执行等价本机命令。
- Shell Access policy/profile 等当前本机管理命令应使用 `bifrost setting ...`；caller 要管理目标设备时，应切换到 `remote exec`。
- 面向远端仓库/工程的 coding-agent 任务，必须把工程约束读取放在任何搜索、读取、编辑、测试之前：先读取工作目录下 `AGENTS.md` / `agents.md`，再读取 `.agents/skills/*/SKILL.md` 的元信息（frontmatter、名称、描述、触发条件、路径等），详细 skill 内容只在实际需要对应流程时加载。

### 安全语义

`shell.exec` 当前由 Shell Access policy 做白名单、cwd、env、stdin、timeout 等限制，并通过目标终端本机进程执行。不能描述为已具备 OS 级沙箱隔离；默认 sandbox policy 不可执行时会拒绝并提示 sandbox 未实现。

授权语义需区分：

- UI access mode：`query`、`selected`、`all`。
- 协议 grant scope：`remote_query`、`remote_shell_exec`、`remote_shell_interactive`。

## 依赖项

- `crates/bifrost-cli/src/commands/install_skill.rs`
- `SKILL.md`
- `skill_remote.md`
- `crates/bifrost-cli/tests/cli_commands.rs`
- `human_tests/skill-remote.md`

## 测试方案

### 单元测试

- `install_skill_installs_remote_skill_from_embedded_bundle`：使用 `BIFROST_INSTALL_SKILL_SOURCE=embedded` 和 `--dir` 安装，验证主 skill 与 sibling `bifrost-remote/SKILL.md` 均写入。
- 验证 remote skill 内容包含 `name: "bifrost-remote"`。
- 验证 remote skill 不包含 `deprecated` 等历史版本迁移文案。
- 验证 remote skill 不包含 `traffic.clear` 或 `bifrost remote traffic clear` 命令示例。
- 验证 remote skill 明确要求执行任何远端工程任务前先阅读 `AGENTS.md` / `agents.md` 和 `.agents/skills/` 下所有 skill 元信息，且 skill 详细内容按需加载。

### E2E 测试

- 使用 `cargo run -p bifrost-cli -- install-skill --tool codex --dir <tmp>/skills/bifrost -y` 执行真实 CLI 安装。
- 断言 `<tmp>/skills/bifrost/SKILL.md` 和 `<tmp>/skills/bifrost-remote/SKILL.md` 均存在。
- 断言 remote skill 明确说明当前本机 Shell Access policy / grant 管理使用 `bifrost setting ...`，远端管理使用 `remote exec`，并且不包含历史别名迁移文案。
- 断言 remote skill 的安装产物包含远端工程约束读取要求：先读 `AGENTS.md` / `agents.md`，再读 `.agents/skills/*/SKILL.md` 元信息，详细 skill 内容按需加载。

### 真实场景测试

新增 `human_tests/skill-remote.md`，覆盖：

- 安装后 remote skill 可发现。
- remote skill 面向用户正式场景推荐默认目录和系统代理启动。
- remote skill 明确三类 scope 前置准备：查询如何启用 Remote Invoke 授权，shell 如何启用 Shell Access policy，文件操作如何使用 File Access policy。
- remote skill 不把 `remote traffic clear` 描述为可直接使用的 remote CLI 子命令，并说明这是写操作。
- remote skill 使用当前 `bifrost setting ...` 命名空间描述本机管理命令，并使用 `remote exec` 描述目标设备管理路径，不包含 `deprecated` 等历史版本信息。
- remote skill 强制要求任何远端工程任务开始前先读取目标工程约束信息：`AGENTS.md` / `agents.md` 与 `.agents/skills/` 全量 skill 元信息，详细 skill 内容按需加载。

## 校验要求

- `cargo test -p bifrost-cli install_skill_installs_remote_skill_from_embedded_bundle --test cli_commands`
- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test --workspace --all-features`
- 修改范围涉及 CLI 安装器时，最终按本地 CI 策略执行 `bash scripts/ci/local-ci.sh --skip-e2e`。

## 文档更新要求

- 更新 `skill_remote.md`。
- 更新 `SKILL.md` 的 remote 小节，避免主 skill 与专用 remote skill 互相矛盾。
- 更新 `human_tests/readme.md` 索引。
