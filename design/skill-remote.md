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
- 本地脚本远端执行：`remote run --script-file <local> --interpreter <program> -- <args>` 是 caller 侧编排能力，必须复用 `remote file scratch-dir` + `remote file write` 上传脚本，再通过 `remote exec` argv 模式执行；不新增底层 relay 协议，也不允许退化为 heredoc / shell 拼接。
- 长任务支持：`remote exec --detach` 返回 `call_id` 并在 caller 本地加密保存 relay token，`remote job list/status/logs/watch` 用于断线后恢复观察、读取日志和取得真实远端 exit code；skill 必须把它写成 build/test/CI watch 的默认路径，且不得要求用户手工复制 `--relay-token`。
- Caller 侧 call events 的 idle deadline 是 300 秒无事件超时，不是远端进程总 timeout；错误提示必须引导长时间静默任务使用 `exec --detach` / `run --detach` + `job watch --output-file`。
- 多远端协作：`--client-id` 是 `remote` 父命令参数，放在 `remote` 后、子命令前；支持 client id 前缀与设备 label/name 前缀，并支持 `BIFROST_REMOTE_CLIENT_ID` 作为默认目标。
- CLI 易错兼容：`remote file write --path <p>` 应被兼容为位置参数等价形式，重复传位置参数和 `--path` 时返回可操作错误。
- 本地 payload 入口：`remote file write/edit/patch` 均应支持 caller 侧 `--from-local <path|->`。`write --from-local` 等价 `--content-file`，`edit --from-local` 读取 edits JSON 数组，`patch --from-local` 等价 `--patch-file`；`mkdir/move/delete` 不存在 caller 本地 payload，不提供该参数。
- 连接恢复边界：客户端 stream 断开、digest mismatch 或本地切线程时，优先用 `remote job logs/watch/status` 续接已有 call；只有 grant/authorization/transport identity 失效时，才执行 `remote conn down/up` 重建连接。
- 调用异常恢复：当 `bifrost remote` 调用出现子命令不存在、参数不兼容、协议错误、行为与本地 skill 不一致等异常时，Agent 必须主动获取远端最新 `skill_remote.md`，以 https://github.com/bifrost-proxy/bifrost/blob/main/skill_remote.md 为权威入口核对命令面、参数、错误码和恢复流程，避免用过期本地 skill 误判能力不存在。
- Relay HTTPS 信任：remote skill 必须说明 remote relay client 默认读取系统 CA，支持 `BIFROST_REMOTE_RELAY_CA_BUNDLE` 与常见 CA env 追加私有根证书；当 CA 无法注入时，`BIFROST_REMOTE_UNSAFE_SSL=1` 可作为最终兜底跳过 remote relay 证书信任校验，且该开关只作用于 remote relay HTTP/SSE client，不等同于代理服务 `--unsafe-ssl`。
- shell 环境语义：默认 shell-text 使用非 login shell 保持 stdout 干净；需要用户 PATH/rc 时显式 `--login`，并说明 CLI 注入 `BIFROST_REMOTE=1`、`TERM=dumb` 等降噪环境，`--cwd` 在 shell 内部再次生效。
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
- 验证 remote skill 明确把 `exec --detach` + `remote job list/status/logs/watch` 作为长任务与断线恢复主路径，并说明真实 exit code 来自 job/watch，而不是断开的 stream。
- 验证 remote skill 不包含 `remote job ... --relay-token <token>` 这类手工复制 token 示例，说明 relay token 由 caller 本地 job cache 管理。
- 验证 remote skill 对 `remote file scratch-dir`、`read-many` policy deny fallback、`--login`/stdout 降噪、连接漂移重建边界均有说明。
- `remote_run_parses_script_upload_options_and_args`：验证 `remote run --script-file`、interpreter、cwd、env、detach 与 `--` 后脚本参数解析。
- `remote_run_builds_argv_exec_payload_for_uploaded_script`：验证上传后的脚本以 `argv_exec` 方式执行，脚本参数不经过 shell 拼接。
- `file_write_accepts_path_flag_compatibility_alias` / `file_write_rejects_duplicate_positional_and_path_flag`：验证 `--path` 兼容与重复路径错误。
- `remote_file_write_parses_from_local_alias` / `remote_file_edit_parses_from_local_edits_file` / `remote_file_patch_parses_from_local_alias`：验证 `write/edit/patch --from-local` 分别落到本地文件内容、edits JSON 文件和 patch 文件入口。
- `file_edit_reads_edits_from_local_file`：验证 `edit --from-local` 读取 caller 本地 edits JSON 并构建 `file.edit` payload。
- `resolve_local_connection_explicit_device_label_prefix_matches` / `resolve_local_connection_multiple_noninteractive_lists_choices`：验证 label/name 前缀选择与多连接候选提示。
- `idle_timeout_message_mentions_seconds_and_job_watch`：验证 300 秒 idle timeout 的错误提示包含 detach/job watch 引导。
- 验证 remote skill 要求调用异常时主动获取远端最新技能，并包含 https://github.com/bifrost-proxy/bifrost/blob/main/skill_remote.md 链接。
- 验证 remote skill 包含 `BIFROST_REMOTE_RELAY_CA_BUNDLE`、常见 CA env 和 `BIFROST_REMOTE_UNSAFE_SSL` 的使用顺序、作用范围与风险说明。

### E2E 测试

- 使用 `cargo run -p bifrost-cli -- install-skill --tool codex --dir <tmp>/skills/bifrost -y` 执行真实 CLI 安装。
- 断言 `<tmp>/skills/bifrost/SKILL.md` 和 `<tmp>/skills/bifrost-remote/SKILL.md` 均存在。
- 断言安装后的 `bifrost-remote/SKILL.md` 包含 `--from-local`，并说明 `write/edit/patch` 的本地 payload 语义。
- 断言 remote skill 明确说明当前本机 Shell Access policy / grant 管理使用 `bifrost setting ...`，远端管理使用 `remote exec`，并且不包含历史别名迁移文案。
- 断言 remote skill 的安装产物包含远端工程约束读取要求：先读 `AGENTS.md` / `agents.md`，再读 `.agents/skills/*/SKILL.md` 元信息，详细 skill 内容按需加载。
- 断言 remote skill 的安装产物包含 `remote exec --detach`、`remote job list/status/logs/watch`、`remote file scratch-dir`、`file.op_not_permitted` read-many fallback、`--login` 和连接断开恢复说明。
- 新增 shell CLI E2E 覆盖 `remote run --help`、`remote --client-id <id> run --script-file ...` 参数解析、`file write --path` 兼容入口和 `remote exec` idle timeout 文案可见性。

### 真实场景测试

新增 `human_tests/skill-remote.md`，覆盖：

- 安装后 remote skill 可发现。
- remote skill 面向用户正式场景推荐默认目录和系统代理启动。
- remote skill 明确三类 scope 前置准备：查询如何启用 Remote Invoke 授权，shell 如何启用 Shell Access policy，文件操作如何使用 File Access policy。
- remote skill 不把 `remote traffic clear` 描述为可直接使用的 remote CLI 子命令，并说明这是写操作。
- remote skill 使用当前 `bifrost setting ...` 命名空间描述本机管理命令，并使用 `remote exec` 描述目标设备管理路径，不包含 `deprecated` 等历史版本信息。
- remote skill 强制要求任何远端工程任务开始前先读取目标工程约束信息：`AGENTS.md` / `agents.md` 与 `.agents/skills/` 全量 skill 元信息，详细 skill 内容按需加载。
- remote skill 指导 coding agent 使用新远程能力：长任务 detach/job cache 续接、断线后 job list/watch/logs/status 恢复、scratch-dir 安全临时目录、read-many policy deny 降级、login shell 降噪和连接身份漂移时的重连边界。
- remote skill 指导 coding agent 在需要远端执行本地脚本时使用 `remote run`，在多远端任务中固定 `--client-id` 或 `BIFROST_REMOTE_CLIENT_ID`，并在 caller 侧避免 sleep 轮询。
- remote skill 指导 coding agent 在 relay TLS 失败时优先使用系统/显式 CA，只有 CA 无法注入时才使用 `BIFROST_REMOTE_UNSAFE_SSL=1`。

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
