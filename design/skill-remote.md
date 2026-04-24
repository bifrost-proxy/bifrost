# Bifrost Remote Skill

## 功能模块说明

`skill_remote.md` 是面向 Agent Skills runtimes 的独立远程调用技能。它用于用户安装 Bifrost skills 后，让 Agent 能够正确处理以下场景：

- 指导目标终端安装并启动 Bifrost remote invoke 所需的服务。
- 通过 SSH key 或 pair code 建立 caller 到 target 的 remote invoke 授权。
- 执行当前 relay 支持的远端只读查询和受控 `shell.exec`，并在授权范围内操作目标设备。
- 明确两类操作的前置准备：只读查询需要目标端启用 Remote Invoke 授权；远程设备控制还需要目标端启用 Shell Access profile/policy 并授权 `selected` 或 `all` 访问模式。
- 明确区分 caller 本地管理命令与通过 `remote command exec` 操作目标设备的路径，避免 Agent 在 caller 本机误执行管理命令。

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

- 支持：`remote status`、`remote search`、`remote traffic list/get/search`、`remote command exec`、`remote disconnect`。
- 只读查询类前置条件：目标 Bifrost 已启动、Relay Connection 在线、目标端 Remote Invoke 页面已通过 SSH key 或 pair code 授权 caller，并可用 `bifrost remote status` 验证。
- 远程设备控制类前置条件：满足只读查询前置条件，且目标端已配置 Shell Access profile/policy，授权请求选择 `selected` 或 `all`，必要时开启 stdin/interactive。
- `remote traffic clear` 当前不是已启用的 relay-backed query 子命令；需要清理目标端记录时，可通过已授权的 `remote command exec` 执行目标端本机 CLI/API 操作。
- rule/config/script/ca/value/系统代理等没有专门的 `bifrost remote <module>` 子命令时，应通过 `remote command exec` 在目标终端执行等价本机命令。
- `remote shell ...` 和 `remote grant ...` 是当前机器本地管理命令；caller 要管理目标设备时，应切换到 `remote command exec`。

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
- 验证 remote skill 不包含 `remote traffic clear` 可执行示例。

### E2E 测试

- 使用 `cargo run -p bifrost-cli -- install-skill --tool codex --dir <tmp>/skills/bifrost -y` 执行真实 CLI 安装。
- 断言 `<tmp>/skills/bifrost/SKILL.md` 和 `<tmp>/skills/bifrost-remote/SKILL.md` 均存在。
- 断言 remote skill 明确说明 `remote shell` / `remote grant` 是目标本地管理命令，不是 relay-backed 远程命令。

### 真实场景测试

新增 `human_tests/skill-remote.md`，覆盖：

- 安装后 remote skill 可发现。
- remote skill 面向用户正式场景推荐默认目录和系统代理启动。
- remote skill 明确两类操作前置准备：只读查询如何启用 Remote Invoke 授权，远程设备控制如何启用 Shell Access policy。
- remote skill 不把 `remote traffic clear` 描述为已启用的 relay-backed query 子命令，并指向 `remote command exec` 替代路径。
- remote skill 不把 caller 本地 `remote shell` / `remote grant` 描述为 relay-backed 管理 API，并指向 `remote command exec` 操作目标设备。

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
