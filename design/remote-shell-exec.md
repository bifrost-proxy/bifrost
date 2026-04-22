# 基于现有 Remote 方案的 Shell 远程执行设计

> 状态：实施方案 | 更新时间：2026-04-23

## 背景

当前仓库里的远程能力已经有一条比较稳定的主链路：

- 配对与授权：`pair_code -> approve -> grant`
- 长期设备绑定：`ssh_publickey -> grant`
- 中转传输：caller -> relay（HTTP + SSE），relay -> client（SSE），client -> relay（HTTP）
- 调用执行：`openCall -> client worker -> executor -> frame/exit`

现状的问题不是“怎么把命令送过去”，而是“**怎么在不推翻现有安全模型的前提下，把只读查询扩展成可控的 shell 远程执行**”。

现有设计文档里已经明确：

- `design/remote-command-bridge.md` 首版只支持只读查询，不做任意 shell
- `design/remote-invoke-ssh-shell-extension.md` 已经把 SSH 公钥作为长期 grant 的签发方式

因此这次方案的目标不是重做 remote 架构，而是在现有架构上新增一层执行协议：

- 保留现有 relay / grant / SSE / 审计骨架
- 新增 `shell.exec` 命令族
- 把“能执行什么”收口到**目标设备本地白名单**
- 把“谁能长期免审批执行”收口到**grant scope + auth_method**

## 方案结论

### 1. 不新造第二套 remote，直接扩展现有 Remote Invoke

推荐保留现有 remote invoke 主协议，只把执行层从：

- `query.readonly`

扩展为：

- `query.readonly`
- `shell.exec`

这样可以复用现有：

- pair code / SSH connect
- grant 生命周期
- relay 路由与事件流
- caller 本地连接文件
- cancel / timeout / stream / history

### 2. Relay 不保存 shell 白名单，也不理解具体命令语义

Relay 仍然只做：

- grant / call 路由
- 调用会话编排
- 事件转发

Relay 不做：

- shell 白名单匹配
- 命令正则校验
- cwd / env 合法性判断
- 审计信息存储（审计完全由 Client 本地负责）
- shell 风险级别决策

这些都必须留在**目标设备本地**，否则远端 relay 会变成高风险的策略中心。

### 3. “任何命令”能力通过白名单策略表达，而不是默认裸透传

为了满足“目标设备上可执行任何命令，但必须设备本地白名单允许”的诉求，推荐把 shell 能力做成三层：

1. `template`：推荐默认方案。命令模板固定，只开放少量参数变量。
2. `argv_exec`：显式指定二进制和参数规则，通过 `Command::new().arg()` 执行，不走 shell 拼接。
3. `shell_text`：高级模式。允许远端传 shell 文本，但必须命中目标设备上的显式白名单规则。

其中：

- `template` 适合部署、重启、同步、脚本入口这类稳定动作
- `argv_exec` 适合工具型命令，例如 `git` / `docker` / `pm2`
- `shell_text` 才是真正接近“任何命令”的模式，但默认应为高风险能力

### 4. shell 执行必须与只读查询使用不同 scope

不能让现有只读 grant 直接复用到 shell 执行。

推荐新增 grant scope：

- `remote_query`
- `remote_shell_exec`
- `remote_shell_interactive`

并且把 grant 与本地策略版本绑定。只要目标设备上的 shell 白名单发生变化，已有 shell grant 就应该失效或要求重新确认。

### 5. 长期 shell grant 的主体边界

已确认规则：

- `pair_code`：可发起 shell 请求，也可获得长期 `remote_shell_exec` grant
- `ssh_publickey`：可获得长期 `remote_shell_exec` grant
- `remote_shell_interactive`：支持 Unix PTY 与 Windows ConPTY

这样可以把“高频自动化执行”和“人工临时远控”分层。

## 目标

- 基于当前 remote invoke 架构，新增通用 shell 远程执行能力
- 支持目标设备配置白名单，决定哪些命令可远程执行
- 支持 stdout / stderr 流式回传
- 支持超时、取消、输出截断，Client 本地完整审计
- 支持执行时间很长的任务，并在长时间运行下保持可观察、可续连、可取消
- 支持 macOS / Linux / Windows 三大平台，并明确能力分级与平台差异
- 沙箱控制策略对齐行业常见 agent 方案，尤其是 Codex 风格的审批、文件系统、网络和命令范围控制
- 支持 CI / Agent / 人工终端三类 caller
- 支持通过 SSH grant 做长期设备绑定
- 支持后续演进到交互式 PTY 会话

## 非目标

- 不引入第二条独立传输协议
- 不让 relay 保存目标设备的明文白名单
- 不默认开放完全无限制的裸 `sh -c`
- 不做文件上传下载隧道
- 不支持完全脱离 call 生命周期的后台 daemon
- 不追求三平台所有 shell 完全等价体验，而是定义统一协议 + 平台能力分级

## 使用场景

### 场景 A：固定动作模板

例如：

- 重启某个服务
- 执行部署脚本
- 同步仓库并执行构建
- 拉起某个 diagnostic script

推荐使用 `template`。

### 场景 B：工具型命令

例如：

- `git status`
- `docker compose ps`
- `pm2 restart api`
- `launchctl print ...`

推荐使用 `argv_exec`。

### 场景 C：高级远程 Shell

例如：

- `bash -lc 'cd /srv/app && git pull && pnpm install && pnpm build'`
- `zsh -lc 'tail -n 200 /tmp/foo.log | rg ERROR'`
- `python3 - <<'PY' ... PY`

这是最灵活也最危险的能力。实现规则如下：

- 必须由设备 owner 显式创建 `shell_text` policy
- 必须命中显式 `policy + scope + binding`
- 默认每次审批
- `pair_code` 与 `ssh_publickey` 都可使用，但是否可长期复用由设备 owner 配置决定
- 默认不允许 break-glass 之外的宽范围 `cwd/env/network` 组合

## 总体架构

```text
Caller
  -> remote connect / ssh connect
  -> openCall(kind=shell.exec)
Relay
  -> 路由 + 事件转发
Client Worker
  -> 校验 grant_scope / auth_method / shell_policy_set_version
  -> 本地匹配 shell policy
  -> 执行进程并流式回传 stdout/stderr
```

### 核心原则

1. 授权与执行解耦
2. Relay 透明，Client 决策
3. 白名单只在目标设备本地生效
4. Shell grant 与 query grant 分离
5. 高风险能力必须命中显式 policy / scope / binding，并通过审批门

## 行业对齐：Codex 风格沙箱控制策略

### 1. 参考方向

这份方案建议显式对齐 Codex / shell agent 常见的 4 个控制面：

1. 审批模式
2. 文件系统范围
3. 网络范围
4. 命令执行范围

参考 OpenAI 官方文档里比较明确的几条思路：

- Codex CLI 把代理权限分成不同审批模式，执行能力不是“全有或全无”
- Shell 工具文档明确建议：执行任意命令前应做沙箱、allowlist / denylist 和审计
- Codex cloud 的网络访问默认关闭；开启后也建议只放行必要域名和 HTTP 方法

基于这些思路，我们不建议把 remote shell 只做成“命令白名单”一层，而是要把它升级成：

- `policy/binding/scope`
- `sandbox_profile`
- `approval_mode`

三者共同决定最终执行能力。

### 2. 推荐新增 Sandbox Profile

建议在现有 policy 之外，引入独立的：

- `remote_invoke_shell_sandbox_profiles`

它的职责类似 Codex 这类 agent 的运行环境配置，决定：

- 这个 policy 在什么文件系统范围内运行
- 能不能访问网络
- 哪些命令能力是彻底禁止的
- 是否允许后台子进程 / PTY / 提权

建议字段：

| 字段 | 说明 |
| --- | --- |
| `sandbox_profile_id` | 稳定 ID |
| `name` | 配置名称 |
| `description` | 用途说明 |
| `approval_mode` | 审批模式 |
| `filesystem_scope_json` | 文件系统范围 |
| `network_scope_json` | 网络范围 |
| `command_scope_json` | 命令范围 |
| `process_scope_json` | 进程控制范围 |
| `tty_scope_json` | PTY / 交互范围 |
| `secret_scope_json` | 可注入的 secrets 范围 |
| `audit_level` | 审计强度 |
| `enforcement_backend` | 当前平台实际使用的 enforcement 后端标识（`bubblewrap+seccomp` / `seatbelt` / `landlock+seccomp` / `restricted_token+job_objects` / `none`），详见"OS 级安全沙箱 Enforcement Backend §4.1" |
| `enforcement_capabilities` | 该后端实际能提供的能力集合（`filesystem_isolation` / `network_isolation` / `process_isolation` / `syscall_filter` / `resource_limits`） |
| `enforcement_status` | 后端就绪状态（`active` / `degraded` / `unavailable`） |
| `required_enforcement_capabilities` | 该 profile 要求的最低 enforcement 能力集合，运行时必须满足（否则拒绝执行或降级审批模式） |
| `enabled` | 是否启用 |
| `version` | 配置版本 |
| `created_at` / `updated_at` | 时间戳 |

推荐关系：

- `policy` 负责表达“业务动作”
- `sandbox_profile` 负责表达“运行边界”
- `scope` / `binding` 再对主体和范围进一步收紧

### 3. 审批模式对齐

建议参考 Codex 常见的分级思路，但改成更适合 remote shell 的名字：

| 模式 | 含义 |
| --- | --- |
| `manual_every_time` | 每次执行都要审批 |
| `manual_on_scope_change` | 首次或范围变更时审批 |
| `auto_within_profile` | 在 sandbox profile 范围内自动执行 |
| `break_glass_only` | 仅紧急模式可执行，且默认人工审批 |
| `full_trust_no_sandbox` | 完全信任模式——跳过 OS 级沙箱，命令以当前用户原生权限直接执行 |

对应关系可以理解为：

- `manual_every_time`
  - 接近 Suggest 风格
- `auto_within_profile`
  - 接近“只在受限沙箱里自动执行”
- `break_glass_only`
  - 明确比 Full Auto 更危险，需要更高门槛
- `full_trust_no_sandbox`
  - 面向高级用户的完全开放模式：用户自行承担安全责任，不施加技术层面的 OS 级强制隔离
  - 设备 owner 必须在 Client WebUI 中逐设备显式开启，不可通过 Relay / Caller 远程激活
  - 仍然执行应用层策略检查（`command_scope` / `filesystem_scope` 白名单过滤等），但不启动 OS 级沙箱包裹子进程
  - 子进程以当前用户身份直接运行，拥有完整的文件系统、网络、进程权限
  - 审计日志中明确标记 `enforcement_backend = "none"` + `enforcement_status = "user_opted_out"`
  - 适用场景：完全信任 caller、需要访问宿主机完整环境（硬件设备、docker socket、特权端口等）、或沙箱限制与业务不兼容

关键点：

- 自动执行能力必须绑定到具体 `sandbox_profile`（`full_trust_no_sandbox` 除外，该模式使用独立的 `unrestricted_full_trust` profile）
- 不能出现“只要是 SSH 就无限自动执行”
- `full_trust_no_sandbox` 的启用是一个设备级决策，而非策略级——即使 policy 允许 `full_trust_no_sandbox`，设备 owner 未开启时仍会回退到沙箱模式

### 4. 文件系统范围控制

这块建议显式对齐“当前目录作用域 + 可写根目录 + 临时目录”这类行业常见思路。

推荐 `filesystem_scope_json` 至少支持：

| 字段 | 说明 |
| --- | --- |
| `read_roots` | 允许读取的根目录 |
| `write_roots` | 允许写入的根目录 |
| `exec_roots` | 允许在其中执行脚本/二进制的目录 |
| `tmp_roots` | 允许写临时文件的目录 |
| `deny_roots` | 明确禁止访问的路径前缀 |
| `path_mode` | `strict_roots` / `inherit_policy_cwd` |

推荐默认策略：

- 默认只允许：
  - policy 指定 cwd
  - cwd 下的受控子目录
  - 系统临时目录
- 明确禁止：
  - 用户主目录整体
  - SSH key / credential store
  - 系统敏感目录
  - 其他业务无关项目目录

也就是说：

- 即使命令本身在白名单里
- 只要访问路径超出 `filesystem_scope`
- 也必须被拒绝

### 5. 网络范围控制

建议直接对齐 Codex cloud 那种“默认关闭，按域名和方法放行”的控制风格。

推荐 `network_scope_json`：

| 字段 | 说明 |
| --- | --- |
| `mode` | `off` / `allowlist` / `preset_dependencies` / `full` |
| `allowed_domains` | 允许访问的域名 |
| `allowed_methods` | 允许的 HTTP 方法 |
| `allowed_ports` | 允许的端口 |
| `allow_loopback` | 是否允许访问本机 |
| `deny_private_ranges` | 是否拒绝私网段 |

推荐默认值：

- `mode = off`
- 只对确有需要的 policy 开启网络

推荐预设：

- `off`
  - 完全断网
- `preset_dependencies`
  - 仅常见依赖源
- `allowlist`
  - 明确列出域名 + 方法
- `full`
  - 仅 `break_glass_only` 和 `full_trust_no_sandbox` 可用

### 6. 命令范围控制

这块是用户点名要对齐的重点。

建议 `command_scope_json` 不只是一条 regex，而是至少包含：

| 字段 | 说明 |
| --- | --- |
| `mode` | `argv_only` / `template_only` / `shell_text_allowed` |
| `allowed_executables` | 允许的可执行文件 |
| `denied_executables` | 明确禁止的可执行文件 |
| `denied_patterns` | 危险参数或命令模式 |
| `dangerous_verbs` | 如 `rm`, `dd`, `mkfs`, `shutdown` 等高危动作 |
| `allow_shell_operators` | 是否允许 `&&`, `|`, `>`, `$(...)` 等 |
| `max_argv_count` | 最大参数数量 |
| `max_command_length` | 最大命令长度 |
| `structural_analysis` | `"required"` / `"skip"`，`shell_text` 模式下默认 `"required"`，强制 Token 级结构分析 |
| `allowed_variable_expansions` | 允许在 shell_text 中出现的 `$VAR` 名称白名单，默认 `[]` |
| `allow_command_substitution` | 是否允许 `$()` 和反引号，默认 `false` |
| `allow_process_substitution` | 是否允许 `<()` 和 `>()`，默认 `false` |
| `allow_brace_expansion` | 是否允许 `{a,b}`，默认 `false` |
| `allow_glob` | 是否允许 `*`, `?`, `[...]`，默认 `false` |
| `allow_heredoc` | 是否允许 `<<` / `<<<`，默认 `false` |
| `input_charset` | `"ascii_printable"` / `"utf8_restricted"`，输入字符集限制，默认 `"ascii_printable"` |
| `command_regex_role` | 固定 `"convenience_filter"`，明确标注 regex 不作为安全边界 |

推荐默认：

- `argv_only`
- 禁止 shell 拼接
- 禁止 heredoc / command substitution
- 禁止写系统级敏感路径

只有高风险 profile 才允许：

- `shell_text_allowed`
- shell operators
- 宽范围重定向

### 7. 进程控制范围

建议 `process_scope_json` 至少支持：

| 字段 | 说明 |
| --- | --- |
| `max_child_processes` | 最多可拉起多少子进程 |
| `allow_detach` | 是否允许 detach |
| `allow_background` | 是否允许后台运行 |
| `allow_privilege_escalation` | 是否允许 sudo / runas / elevate |
| `allow_service_control` | 是否允许服务管理 |
| `max_runtime_ms` | 最长运行时间 |
| `kill_on_caller_cancel` | caller cancel 是否强制 kill |

这里建议直接写死默认值：

- `allow_detach = false`
- `allow_background = false`
- `allow_privilege_escalation = false`
- `kill_on_caller_cancel = true`

### 8. 推荐预设 Sandbox Profiles

为了让产品真正可用，建议内置几套预设：

#### `diagnostic_readonly`

- 文件系统：只读
- 网络：关闭
- 命令：`argv_only`
- 审批：可按绑定主体自动执行
- **enforcement 要求**：`auto_within_profile` 必须要求 `filesystem_isolation` + `network_isolation` 全覆盖（详见 §4.6）

#### `repo_bounded_exec`

- 文件系统：仅仓库根目录 + `tmp`
- 网络：关闭
- 命令：`template` / `argv_exec`
- 审批：首次审批
- **enforcement 要求**：`filesystem_isolation` + `network_isolation`

#### `dependency_build`

- 文件系统：仓库根目录可写
- 网络：`preset_dependencies`
- 命令：受控构建命令
- 审批：首次审批或范围变更审批
- **enforcement 要求**：`filesystem_isolation` + `network_isolation`（`preset_dependencies` 展开为 allowlist 后由 enforcement 强制）

#### `ops_break_glass`

- 文件系统：宽范围
- 网络：allowlist 或 full
- 命令：可放开 `shell_text`
- 审批：每次审批
- **enforcement 要求**：`enforcement_backend` 可为 `none`（break-glass 豁免），但审计必须明确标记 `enforcement_status = unavailable`

#### `unrestricted_full_trust`

- 文件系统：无限制（继承当前用户完整权限）
- 网络：`full`（无隔离）
- 命令：`shell_text_allowed`，不限制 shell operators / heredoc / command substitution
- 进程：`allow_privilege_escalation = true`、`allow_detach = true`、`allow_background = true`
- 审批：由 `approval_mode` 单独控制（默认 `manual_every_time`，设备 owner 可按需切换为 `auto_within_profile`）
- **enforcement 要求**：`enforcement_backend = "none"`，`enforcement_status = "user_opted_out"`
- **启用条件**：必须同时满足：
  1. 设备 owner 在 Client WebUI 的设备安全设置中显式开启了 `allow_full_trust_mode`
  2. grant 的 `grant_scope` 允许 `remote_shell_exec`
  3. policy 明确绑定了 `unrestricted_full_trust` profile
- **审计要求**：所有通过此 profile 执行的命令，审计记录中必须包含 `enforcement_status = "user_opted_out"` + `full_trust_enabled_by = "<device_owner_fingerprint>"` + `full_trust_enabled_at = "<timestamp>"`
- **适用场景**：高级用户完全信任 caller，自行承担安全风险；需要访问宿主机完整环境（硬件、docker socket、GPU 等）；沙箱与目标工作负载不兼容（如需要 mount 操作、内核模块交互等）

### 9. 决策优先级

推荐最终判定顺序：

1. `grant/binding/scope`
2. `sandbox_profile.filesystem_scope`
3. `sandbox_profile.network_scope`
4. `sandbox_profile.command_scope`
5. `sandbox_profile.process_scope`
6. `approval_mode`
7. **`full_trust_no_sandbox` 旁路检查**：如果 `approval_mode = full_trust_no_sandbox` 且设备已开启 `allow_full_trust_mode`，跳过步骤 8/9/10，直接以当前用户权限启动子进程
8. **enforcement backend 就绪检查**（详见"OS 级安全沙箱 Enforcement Backend §4.5"）
9. **OS 级沙箱构建 + 参数映射**
10. 才在沙箱内启动子进程

也就是说：

- 白名单命中不代表一定能执行
- 还必须满足 sandbox profile 的边界
- `full_trust_no_sandbox` 是唯一的沙箱豁免路径，且必须由设备 owner 显式授权

### 10. 推荐的对齐结论

如果按 Codex 风格总结成一句话，我们这边最值得对齐的是：

- **默认拒绝**
- **边界先于执行**
- **自动化必须绑在受限 profile 上**
- **网络默认关闭，按域名和方法放行**
- **命令范围不只看命令名，还要看路径、参数、进程和文件系统范围**
- **提供 `full_trust_no_sandbox` 选项**：尊重高级用户的自主权，允许完全跳过沙箱，但必须由设备 owner 显式开启且审计全程标记

## 三平台兼容性设计

### 1. 兼容目标

这里的“三大平台”明确指：

- macOS
- Linux
- Windows

设计目标不是把三者做成“看起来都像 bash”，而是：

- 协议统一
- 策略统一
- 平台执行器分层
- 能力按平台分级暴露

### 2. 统一协议，平台适配器执行

推荐在 client worker 内部引入平台适配层：

- `PlatformExecAdapter::MacOs`
- `PlatformExecAdapter::Linux`
- `PlatformExecAdapter::Windows`

所有 remote shell 请求先落到统一结构：

- `policy_id`
- `exec_mode`
- `cwd`
- `env`
- `stdin`
- `timeout`
- `pty`

然后再由平台适配器完成最后一步映射：

- 选用哪个 shell
- 如何创建进程组
- 如何发送终止信号
- 如何创建 PTY / ConPTY
- 如何规范化路径和环境变量

这样 relay 和 caller 都不需要关心平台细节，平台差异只在目标设备本地处理。

### 3. 策略必须支持“按平台变体”

不能假设一个 `policy_id` 在三平台上对应同一条命令。

例如“重启服务”：

- macOS 可能是 `launchctl kickstart`
- Linux 可能是 `systemctl restart`
- Windows 可能是 `sc stop/start` 或 `Restart-Service`

因此建议每条 policy 支持：

- `platform_targets_json`
- `platform_variants_json`

示例：

```json
{
  "policy_id": "restart-agent",
  "platform_targets": ["macos", "linux", "windows"],
  "platform_variants": {
    "macos": {
      "exec_mode": "argv_exec",
      "executable": "/bin/launchctl",
      "fixed_args": ["kickstart", "-k", "system/com.example.agent"]
    },
    "linux": {
      "exec_mode": "argv_exec",
      "executable": "/bin/systemctl",
      "fixed_args": ["restart", "example-agent.service"]
    },
    "windows": {
      "exec_mode": "argv_exec",
      "executable": "C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe",
      "fixed_args": ["-NoProfile", "-NonInteractive", "-Command", "Restart-Service -Name example-agent"]
    }
  }
}
```

推荐规则：

- 同一 `policy_id` 代表同一业务意图
- 每个平台可以有不同变体
- caller 只感知 `policy_id`，不直接感知平台分支

### 4. 优先级：argv_exec > template > shell_text

为了提升三平台兼容性，推荐优先级：

1. `argv_exec`
2. `template`
3. `shell_text`

原因：

- `argv_exec` 最不依赖平台 shell 语法
- `template` 可做平台变体，但仍可能含 shell 差异
- `shell_text` 天然最不兼容，尤其是 Unix shell 和 PowerShell / CMD 语法完全不同

结论：

- 真正要跨三平台复用的高价值动作，优先写成 `argv_exec`
- `shell_text` 更适合“某个平台专用的高级能力”

### 5. shell_kind 兼容矩阵

建议支持的 shell 类型如下：

| shell_kind | macOS | Linux | Windows | 说明 |
| --- | --- | --- | --- | --- |
| `none` | 支持 | 支持 | 支持 | 直接执行二进制，最推荐 |
| `sh` | 支持 | 支持 | 不支持 | 仅 Unix |
| `bash` | 支持 | 支持 | 不推荐 | Windows 不保证存在 |
| `zsh` | 支持 | 可选 | 不支持 | macOS 常见 |
| `pwsh` | 可选 | 可选 | 支持 | 推荐作为 PowerShell Core |
| `powershell` | 不推荐 | 不推荐 | 支持 | Windows PowerShell 兼容模式 |
| `cmd` | 不支持 | 不支持 | 支持 | 仅做遗留兼容 |

默认建议：

- macOS：`none` / `zsh` / `bash`
- Linux：`none` / `sh` / `bash`
- Windows：`none` / `pwsh`，必要时 `powershell`

不建议默认使用：

- 登录 shell
- 依赖用户 rc 文件的 shell

### 6. 进程创建与取消语义

三平台最容易踩坑的是进程组和取消模型。

#### macOS / Linux

- 使用独立 process group / session
- 优雅取消：
  - `SIGTERM`
- 强制终止：
  - `SIGKILL`
- PTY：
  - 使用伪终端

#### Windows

- 必须使用独立 process group，建议同时配合 Job Object
- 优雅取消优先级：
  1. `CTRL_BREAK_EVENT`（适用于控制台进程）
  2. `taskkill /T` 或 Job Object close
  3. `TerminateJobObject`
- PTY：
  - 使用 ConPTY
- 不能假设存在 `SIGTERM` / `SIGKILL` 语义

因此文档中的“`SIGTERM` / 平台等价信号”应明确落成：

- Unix：`SIGTERM -> SIGKILL`
- Windows：`CTRL_BREAK_EVENT -> job terminate`

### 7. PTY / ConPTY 能力分级

PTY 不可能在三平台完全等价。当前版本的实施边界如下：

- macOS / Linux：支持 PTY
- Windows：支持 ConPTY（要求系统版本满足条件）

Windows 特殊约束：

- ConPTY 只在较新系统版本可靠可用
- 某些 GUI 子进程、服务进程、非控制台程序并不适合 PTY

因此 policy 里应新增：

- `pty_platform_support_json`

caller 在 `shell.policy.list` 中能看到：

- 当前目标设备是否支持 PTY

### 8. 路径与 cwd 兼容

路径处理不能按 Unix 方式硬写。

必须考虑：

- `/` vs `\`
- 盘符路径：`C:\work\app`
- UNC 路径：`\\server\share`
- 大小写敏感差异

推荐规则：

1. policy 中的 `cwd_value` 与 `allowed_cwd_prefixes_json` 必须按目标平台存储
2. 执行前先做平台本地规范化，再做前缀匹配
3. caller 不负责拼平台路径
4. 若 policy 需要跨平台，应通过平台变体给出各自 cwd

### 9. 环境变量兼容

环境变量在三平台也不完全一致：

- Unix 通常大小写敏感
- Windows 通常大小写不敏感
- Windows 常见关键变量：
  - `Path`
  - `PATHEXT`
  - `ComSpec`

建议：

- policy 层统一把 env key 存成逻辑名
- Windows 适配器在落地时做大小写规范化
- 审计层记录原始 key + 规范化 key

同时必须禁止：

- caller 假设 `PATH` / `Path` 等价并绕过 allowlist

### 10. 编码与换行兼容

这是三平台最容易被忽略，但实际会反噬 CLI 展示和日志比对的地方。

推荐协议层不要假设输出一定是 UTF-8 文本。

建议：

- frame 在传输层以字节序列为准
- JSON 传输时使用 base64
- 附带：
  - `stream`：`stdout` / `stderr`
  - `encoding_hint`
  - `newline_hint`

默认策略：

- 优先 UTF-8
- Windows PowerShell / CMD 输出如不是 UTF-8，由 Windows 适配器提供 `encoding_hint`
- spool 文件保留原始字节，不提前做换行归一化
- caller 展示时再做：
  - UTF-8 解码优先
  - 平台 hint fallback
  - 仅展示层可做 `CRLF -> LF`

这样可以避免：

- Windows 中文输出乱码
- digest 因换行归一化而变化

### 11. 可执行文件解析兼容

三平台的 executable lookup 规则也不同：

- Unix 常走 `PATH`
- Windows 受 `PATHEXT`、`.exe/.bat/.cmd` 影响

推荐：

- `argv_exec` 尽量要求 policy 指定绝对路径
- 如果允许裸命令名：
  - 必须由平台适配器解析
  - 审计里记录最终命中的可执行文件路径

Windows 下尤其要避免：

- 调用方写 `python`
- 实际命中 `python.bat` / 非预期 shim

### 12. 服务管理命令兼容

很多远程 shell 场景本质上是在做“服务管理”，这在三平台实现完全不同。

常见映射：

| 业务意图 | macOS | Linux | Windows |
| --- | --- | --- | --- |
| 重启服务 | `launchctl kickstart` | `systemctl restart` | `Restart-Service` / `sc` |
| 查看状态 | `launchctl print` | `systemctl status` | `Get-Service` / `sc query` |
| 查看日志 | `log show` / 文件 | `journalctl` / 文件 | Event Log / 文件 |

所以建议 WebUI 内置若干“跨平台业务模板”，而不是让用户每次手写三套命令。

### 13. 审计与错误码兼容

建议 `termination_reason` 保持跨平台统一枚举，但允许平台细节挂在扩展字段里。

例如：

```json
{
  "termination_reason": "signal_killed",
  "platform_detail": {
    "platform": "windows",
    "kill_method": "TerminateJobObject",
    "native_exit_code": 1
  }
}
```

这样 caller 看到的是统一结果，而本地审计仍可追到平台细节。

### 14. 当前版本兼容策略

当前版本的平台边界固定如下：

- 三平台都支持：
  - `argv_exec`
  - `template`
  - `shell_text`
  - 长任务 heartbeat / resume / logs / cancel
- PTY：
  - macOS / Linux：支持
  - Windows：支持 ConPTY
- Windows shell：
  - `shell_text` 仅支持 `pwsh` / `powershell`
  - 不支持 `cmd` 作为 `shell_text` 执行后端

这样三平台可以共享统一协议和控制模型，同时把平台差异收敛在执行器适配层。

## 数据模型设计

### 1. 扩展 RemoteCommand

当前 `RemoteCommand` 只有：

- `command`
- `args_json`

建议演进为带 kind 的结构：

```json
{
  "kind": "shell.exec",
  "policy_id": "deploy-api",
  "exec_mode": "template",
  "argv": ["./scripts/deploy.sh", "--env", "prod"],
  "shell": null,
  "command_text": null,
  "cwd": "/srv/api",
  "env": {
    "NODE_ENV": "production"
  },
  "stdin_mode": "none",
  "timeout_ms": 600000,
  "pty": {
    "enabled": false
  }
}
```

兼容策略：

- 原只读命令继续走 `kind=query.readonly`
- shell 请求走 `kind=shell.exec`

### 2. 新增目标设备本地白名单表

建议新增 `remote_invoke_shell_policies`：

| 字段 | 说明 |
| --- | --- |
| `policy_id` | 稳定 ID，供 caller 引用 |
| `name` | 展示名称 |
| `description` | 说明用途 |
| `enabled` | 是否启用 |
| `sandbox_profile_id` | 关联的沙箱配置 |
| `risk_level` | `low` / `medium` / `high` |
| `exec_mode` | `template` / `argv_exec` / `shell_text` |
| `shell_kind` | `bash` / `zsh` / `sh` / `pwsh` / `powershell` / `cmd` / `none` |
| `platform_targets_json` | 支持哪些平台 |
| `platform_variants_json` | 每个平台对应的执行变体 |
| `executable` | 直接执行时的二进制路径 |
| `fixed_args_json` | 固定参数 |
| `template_json` | 模板定义与变量 schema |
| `command_regex` | `shell_text` 模式下的命令匹配规则 |
| `cwd_mode` | `fixed` / `allow_override` |
| `cwd_value` | 固定工作目录 |
| `allowed_cwd_prefixes_json` | 可覆盖工作目录前缀 |
| `env_mode` | `none` / `allowlist` |
| `allowed_env_schema_json` | 允许传入的环境变量及校验规则 |
| `stdin_policy` | `forbidden` / `inline` / `stream` |
| `max_stdin_bytes` | stdin 最大大小 |
| `pty_allowed` | 是否允许 PTY |
| `max_timeout_ms` | 单次最大超时 |
| `max_output_bytes` | 输出总量上限 |
| `max_concurrency` | 该策略同时最多执行多少个 call |
| `allowed_auth_methods_json` | `pair_code` / `ssh_publickey` |
| `grant_mode_policy` | `once_only` / `short_lived` / `persistent_allowed` |
| `approval_policy` | `always` / `on_first_use` / `on_scope_change` / `high_risk_only` |
| `pty_platform_support_json` | 各平台 PTY 支持情况 |
| `policy_local_version` | policy 自身配置版本，policy 内容变化时递增 |
| `created_at` / `updated_at` | 时间戳 |

### 3. 扩展 GrantInfo

建议给 shell grant 增加绑定信息：

| 字段 | 说明 |
| --- | --- |
| `grant_scope` | `remote_query` / `remote_shell_exec` / `remote_shell_interactive` |
| `policy_binding` | 允许访问的 policy 集合或 policy tag |
| `shell_policy_set_version_snapshot` | 授权时的全局策略版本快照 |
| `interactive_allowed` | 是否允许 PTY |
| `stdin_allowed` | 是否允许 stdin 流式输入 |

这样当目标设备修改白名单后，可以拒绝旧 grant：

- `shell_policy_set_version_snapshot != shell_policy_set_version`

### 4. 扩展 CallInfo

shell call 需要额外记录：

| 字段 | 说明 |
| --- | --- |
| `command_kind` | `query.readonly` / `shell.exec` |
| `policy_id` | 命中的白名单策略 |
| `exec_mode` | `template` / `argv_exec` / `shell_text` |
| `cwd_preview` | 工作目录摘要 |
| `env_keys_json` | 仅记录 env key，不记录 value |
| `pty_enabled` | 是否使用 PTY |
| `stdin_bytes` | 输入大小 |
| `truncated_stdout` | stdout 是否被截断 |
| `truncated_stderr` | stderr 是否被截断 |
| `output_mode` | `split_streams` / `pty_merged` |
| `stdout_bytes` | 已采集 stdout 字节数 |
| `stderr_bytes` | 已采集 stderr 字节数 |
| `binary_output_present` | 是否出现过二进制输出 |
| `artifact_count` | 产生了多少个输出附件 |
| `runtime_heartbeat_at` | 最近一次运行心跳时间 |
| `viewer_attached` | 当前是否有 caller 正在跟随输出 |
| `resume_token` | 用于断线续连的会话标识 |
| `spool_cursor` | 当前输出回放游标 |
| `retained_until` | 任务结束后输出保留到何时 |
| `enforcement_backend` | 实际使用的 enforcement 后端标识（`bubblewrap+seccomp` / `seatbelt` / `landlock+seccomp` / `none`），详见"OS 级安全沙箱 Enforcement Backend §6" |
| `enforcement_status` | 后端就绪状态（`active` / `degraded` / `unavailable`） |
| `enforcement_capabilities` | 后端提供的能力集 |
| `sandbox_config_digest` | enforcement 参数的 SHA256 摘要 |
| `degraded_dimensions` | 如有降级，列出降级的维度和原因 |
| `enforcement_setup_ms` | 沙箱构建耗时（性能监控） |

## 白名单模型

### 推荐模型：template 优先

`template` 不是“固定脚本名 + 自由字符串拼进去”，而是：

- 固定入口命令
- 固定参数骨架
- 少量参数位通过变量传值
- 每个变量都有类型和校验规则

示例：

```json
{
  "policy_id": "deploy-api",
  "exec_mode": "template",
  "shell_kind": "bash",
  "template": {
    "command": "./scripts/deploy.sh --env ${env} --sha ${sha}",
    "variables": {
      "env": {
        "type": "enum",
        "values": ["staging", "prod"]
      },
      "sha": {
        "type": "regex",
        "pattern": "^[0-9a-f]{7,40}$"
      }
    }
  }
}
```

优点：

- 最容易审计
- 最容易做 UI
- 最适合自动化

### 工具模型：argv_exec

`argv_exec` 用于不需要 shell 展开，但需要执行系统工具的场景。

示例：

```json
{
  "policy_id": "pm2-restart",
  "exec_mode": "argv_exec",
  "shell_kind": "none",
  "executable": "/usr/local/bin/pm2",
  "fixed_args": ["restart"],
  "variable_args": [
    {
      "name": "app",
      "pattern": "^[a-z0-9-]{1,32}$"
    }
  ]
}
```

执行时必须使用：

- `Command::new(executable).arg(...)`

禁止：

- 拼接成 `sh -c`

### 高级模型：shell_text

`shell_text` 是为了满足“白名单允许后可执行任意 shell 命令”。

但这里的“任意”应理解为：

- caller 可以提交 shell 文本
- 只要命中目标设备上的 regex / rule set 即可执行

而不是：

- 无规则裸透传到 `bash -lc`

已确认规则：

- 当前版本开放 `shell_text`
- `shell_text` 仍必须命中显式白名单规则，不能裸透传
- `shell_text` 默认每次审批
- `pair_code` 与 `ssh_publickey` 都可按策略使用 `shell_text`
- 是否允许长期 grant 由设备 owner 配置与 policy / scope / binding 决定

示例：

```json
{
  "policy_id": "trusted-ops",
  "exec_mode": "shell_text",
  "shell_kind": "bash",
  "command_regex": "^cd /srv/(api|web) && (git (status|pull)|pnpm (install|build)|pm2 restart [a-z0-9-]+)$",
  "allowed_auth_methods": ["pair_code", "ssh_publickey"],
  "grant_mode_policy": "persistent_allowed",
  "approval_policy": "always"
}
```

## Client 侧白名单控制策略

### 1. 设计目标

这里的“client 侧自定义白名单范围”不应只理解成：

- 允许命令 A
- 拒绝命令 B

更推荐把它做成一套本地控制面，回答 5 个问题：

1. **谁**可以执行
2. **能执行什么**
3. **在什么范围内执行**
4. **需要什么级别的审批**
5. **发生策略冲突时谁优先**

核心原则：

- 默认拒绝
- 显式允许
- 显式拒绝优先于允许
- 白名单是多维约束的交集，不是单一命令正则

### 2. 推荐控制模型：Policy + Scope + Binding + Override

推荐把 client 侧控制拆成四层：

1. `Policy`
   - 定义“允许执行的动作”
2. `Scope`
   - 定义“动作允许覆盖到什么范围”
3. `Binding`
   - 定义“哪些 caller / auth_method / grant 可以拿到这些动作”
4. `Override`
   - 定义“显式拒绝或更严格限制”

换句话说：

- `Policy` 决定能力
- `Scope` 决定边界
- `Binding` 决定主体
- `Override` 决定例外

### 3. 白名单范围不应只有“命令字符串”

建议 client 侧允许自定义的白名单范围至少包含以下维度：

| 维度 | 说明 |
| --- | --- |
| `platform` | 只在某个平台可用 |
| `policy_id / policy_tag` | 哪些策略可用 |
| `auth_method` | `pair_code` / `ssh_publickey` |
| `caller_identity` | SSH key fingerprint、caller fingerprint、caller tag |
| `exec_mode` | `template` / `argv_exec` / `shell_text` |
| `executable_range` | 哪些可执行文件允许 |
| `cwd_range` | 哪些目录允许作为工作目录 |
| `arg_schema` | 参数结构和取值范围 |
| `env_range` | 哪些环境变量允许传入 |
| `stdin_range` | 是否允许输入、大小多大 |
| `time_window` | 哪些时间段允许 |
| `concurrency` | 能并发开几个 |
| `runtime_limit` | 最长可执行多久 |
| `pty` | 是否允许交互式会话 |
| `approval_level` | 是否必须人工确认、是否一次性 |

### 4. 推荐两层白名单：能力白名单 + 范围白名单

这是这份方案里最重要的一条。

不要把白名单只做成“命令允许/不允许”，而应该拆成两层：

#### 第一层：能力白名单

定义：

- 能否执行这个动作

例如：

- 允许 `deploy-api`
- 允许 `restart-agent`
- 允许 `trusted-ops`

#### 第二层：范围白名单

定义：

- 这个动作在多大范围内可执行

例如：

- `deploy-api` 只允许在 `/srv/api` 下执行
- 只允许传 `env=staging|prod`
- 只允许命中显式 binding 的主体
- 只允许 `19:00-23:00`
- 只允许 caller tag 为 `release-bot`

只有两层都命中，才算真正放行。

### 5. 推荐新增 Scope 表

在已有 `remote_invoke_shell_policies` 之外，建议新增：

- `remote_invoke_shell_scopes`

建议字段：

| 字段 | 说明 |
| --- | --- |
| `scope_id` | 稳定 ID |
| `name` | 作用域名称 |
| `description` | 描述 |
| `enabled` | 是否启用 |
| `policy_selector_json` | 命中哪些 `policy_id` / `policy_tag` |
| `auth_method_selector_json` | 命中哪些认证方式 |
| `caller_selector_json` | 命中哪些 caller |
| `platform_selector_json` | 命中哪些平台 |
| `cwd_constraints_json` | 允许的 cwd 前缀 |
| `executable_constraints_json` | 允许的 executable 前缀 / 精确路径 |
| `arg_constraints_json` | 参数 schema |
| `env_constraints_json` | env allowlist |
| `time_window_json` | 允许执行的时间窗口 |
| `quota_json` | 并发数、日调用次数、最长运行时间 |
| `pty_policy` | `forbidden` / `allowed` / `restricted_by_binding` |
| `approval_policy` | `always` / `on_first_use` / `on_scope_change` / `high_risk_only` |
| `effect` | `allow` / `deny` |
| `priority` | 优先级，数值越高越先匹配 |
| `version` | 变更版本 |
| `created_at` / `updated_at` | 时间戳 |

### 6. 推荐新增 Binding 表

仅有 scope 还不够，还需要把“主体”抽出来。

建议新增：

- `remote_invoke_shell_bindings`

作用：

- 把 caller 身份映射到可访问的 policy / scope 集

建议字段：

| 字段 | 说明 |
| --- | --- |
| `binding_id` | 稳定 ID |
| `subject_type` | `ssh_key` / `caller_fingerprint` / `auth_method` / `caller_tag` |
| `subject_value` | 具体值 |
| `allowed_policy_ids_json` | 显式允许的 policy |
| `allowed_policy_tags_json` | 显式允许的 tag |
| `denied_policy_ids_json` | 显式拒绝的 policy |
| `denied_policy_tags_json` | 显式拒绝的 tag |
| `allowed_scope_ids_json` | 可命中的 scope |
| `max_grant_mode` | 最高可拿到的 grant 等级 |
| `require_approval` | 是否强制审批 |
| `enabled` | 是否启用 |
| `version` | 变更版本 |

这样 client 侧可以表达：

- 某个 SSH key 只能跑 `deploy:*`
- 某个 pair code 只能用 `diagnostic:*`
- 某个 caller tag 可以用 `staging`，但不能用 `prod`

### 7. 推荐 Policy Tag 体系

如果只靠 `policy_id` 管理，后期会很难维护。

建议所有 policy 都支持 tag：

- `diagnostic`
- `deploy`
- `service`
- `repo`
- `network`
- `high-risk`
- `prod`
- `staging`
- `interactive`
- `break-glass`

这样 scope / binding 可以按 tag 控制，而不是每次手动列很多 `policy_id`。

### 8. 推荐默认策略：Default Deny + Explicit Allow + Explicit Deny Override

优秀的白名单策略，最核心是优先级足够清楚。

推荐决策顺序：

1. 全局安全门
   - grant scope / auth_method / platform 是否允许
2. 显式 deny binding
   - 主体是否被显式禁止
3. 显式 deny scope
   - 当前请求是否命中拒绝范围
4. allow binding
   - 主体是否具备候选能力
5. allow scope
   - 请求是否落在允许范围内
6. approval gate
   - 当前风险级别是否仍需人工审批
7. 默认拒绝

可以概括成：

- `deny > allow > default_deny`

### 9. 推荐风险分层

建议每个 policy 和 scope 都带风险分层：

- `low`
- `medium`
- `high`
- `critical`

推荐默认约束：

| 风险级别 | 默认策略 |
| --- | --- |
| `low` | `pair_code` / `ssh_publickey` 均可使用，可长期复用，是否审批取决于 scope |
| `medium` | `pair_code` / `ssh_publickey` 均可使用，默认首次审批 |
| `high` | `pair_code` / `ssh_publickey` 均可使用，默认每次审批 |
| `critical` | 仅 break-glass，强制一次性审批，不可复用 |

### 10. 推荐 Break-Glass 模式

如果要支持非常宽的 client 自定义白名单范围，必须有一条“紧急高风险通道”，而不是把正常通道无限放大。

建议新增特殊 tag / scope：

- `break-glass`

特征：

- 默认关闭
- 仅设备 owner 手动开启
- `pair_code` / `ssh_publickey` 均可被单独授予
- 仅 `once`
- 必须人工审批
- 本地完整审计
- 可设置自动失效时间

这样“任意 shell 文本 + 宽范围 cwd/env + 高权限执行”这类需求，可以被收拢到 break-glass，而不污染常规策略。

### 11. 推荐的 caller 侧主体分层

建议 client 侧至少支持以下主体维度：

1. `ssh_key_fingerprint`
   - 最稳定，适合长期绑定
2. `caller_fingerprint`
   - 适合非 SSH 临时调用
3. `caller_tag`
   - 例如 `release-bot`、`human-operator`、`ci-runner`
4. `auth_method`
   - 作为兜底维度

推荐优先级：

- `ssh_key_fingerprint` > `caller_tag` > `caller_fingerprint` > `auth_method`

### 12. 推荐的范围控制模板

为了让用户真的“好用”，建议 WebUI 内置几套 client 侧白名单模板：

#### 模板 A：只读诊断

- tag: `diagnostic`
- 仅 `argv_exec` / `template`
- 禁止 `shell_text`
- 禁止 PTY
- 禁止自定义 env
- pair_code 可一次性使用

#### 模板 B：部署机器人

- tag: `deploy`
- `pair_code` / `ssh_publickey` 均可配置
- 限定 cwd 前缀
- 限定 executable 路径
- 限定 env key
- 可长期 grant
- 长任务允许续连

#### 模板 C：人工运维

- tag: `service`
- `pair_code` / `ssh_publickey` 均可配置
- 允许 `argv_exec`，可选少量 `shell_text`
- `high_risk_only` 审批

#### 模板 D：Break-Glass

- tag: `break-glass`
- `pair_code` / `ssh_publickey` 均可被显式配置
- 一次性授权
- 必须审批
- 强审计

### 13. 推荐的“范围”控制维度细化

下面这几项是我认为最值得做成强约束而不是弱提示的：

#### `cwd_range`

推荐支持：

- 精确目录
- 前缀目录
- 只读目录标签

例如：

- `/srv/api`
- `/srv/api/releases/`
- `C:\\work\\app`

#### `executable_range`

推荐支持：

- 精确路径
- 前缀路径
- 内置别名映射

例如：

- `/usr/bin/git`
- `/usr/local/bin/pm2`
- `C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe`

#### `arg_schema`

推荐支持：

- enum
- regex
- integer range
- boolean
- path under prefix

#### `env_range`

推荐支持：

- 允许哪些 key
- 每个 key 的值模式
- 是否允许 caller 覆盖

#### `time_window`

推荐支持：

- 工作时间
- 发布窗口
- 临时维护窗口

#### `quota`

推荐支持：

- 同时最多运行几个
- 每小时最多几次
- 每天最多几次
- 最长运行多久

### 14. 推荐的判定算法

client 侧执行前，建议按下面顺序判定：

1. 解析 caller 主体
2. 解析 grant scope
3. 找到候选 policy
4. 解析当前平台变体
5. 收集命中的 binding
6. 收集命中的 scope
7. 先执行 deny 规则
8. 再求 allow 规则交集
9. 做 risk / approval 判定
10. 生成最终执行约束

最终执行约束应是一个已经收敛过的对象，例如：

```json
{
  "policy_id": "deploy-api",
  "effective_exec_mode": "argv_exec",
  "effective_cwd": "/srv/api",
  "effective_env_keys": ["NODE_ENV"],
  "effective_timeout_ms": 600000,
  "effective_pty": false,
  "requires_approval": true
}
```

### 15. 已确认的白名单策略

已确认当前版本直接落完整控制模型：

1. `Policy`
   - 支持 `template` / `argv_exec` / `shell_text`
2. `Policy Tag`
   - 支持 tag
3. `Binding`
   - 支持按 `ssh_key_fingerprint` / `caller_fingerprint` / `caller_tag` / `auth_method` 绑定 policy/tag
4. `Scope`
   - 支持 `cwd_range` / `executable_range` / `arg_schema` / `env_range` / `stdin_range` / `time_window` / `quota` / `pty` / `approval_policy` / `effect`
5. `Override`
   - 支持显式 deny override
6. `Break-Glass`
   - 纳入完整模型
7. 决策优先级
   - `deny > allow > default_deny`

当前版本不包含的部分主要是：

- 复杂 DSL 编辑体验
- 过度通用的嵌套表达式编排 UI

## 授权模型

### 1. pair_code 与 SSH 继续并行

配对入口不需要变化：

- `pair_code`
- `ssh_publickey`

变化点只在 grant 的 scope。

### 2. shell grant 不复用 query grant

如果 caller 只有 `remote_query` grant，却请求 `shell.exec`，必须直接拒绝。

### 3. shell grant 绑定策略版本

假设目标设备的 `trusted-ops` 规则从允许 `git pull` 改成不允许：

- 旧 grant 不能继续执行
- caller 需要重新审批或重新 connect

最终规则：

- 目标设备维护一个全局 `shell_policy_set_version`
- 其值由 `policy + scope + binding + sandbox_profile` 四类配置版本组合计算
- grant 记录签发时的 `shell_policy_set_version_snapshot`
- `openCall` 时只校验全局版本是否一致，不做多表逐项比对
- 任何一类配置发生变化，都必须提升全局版本并使旧 shell grant 失效

这样实现层只有一套稳定的失效判定，而不是在执行路径上临时拼装多个版本字段。

### 4. 已确认的授权矩阵

| auth_method | query.readonly | shell.exec(template/argv) | shell.exec(shell_text) |
| --- | --- | --- | --- |
| `pair_code` | 支持 | 支持，可一次性、短时或长期，取决于用户配置 | 支持，但默认每次审批 |
| `ssh_publickey` | 支持 | 支持，可长期 | 支持，但默认每次审批 |

## 执行协议设计

### 1. Caller CLI 设计

建议新增：

```text
bifrost remote shell list
bifrost remote shell exec --policy <policy_id> [--var k=v ...]
bifrost remote shell exec --policy <policy_id> -- [argv...]
bifrost remote shell run --policy <policy_id> --text "cd /srv/api && git pull"
bifrost remote shell run --policy <policy_id> --stdin-file deploy.sh
```

推荐语义：

- `shell list`：拉取可见策略摘要
- `shell exec`：面向 `template` / `argv_exec`
- `shell run`：面向 `shell_text`

长任务相关补充：

```text
bifrost remote shell resume <call_id>
bifrost remote shell logs <call_id> --follow
bifrost remote shell cancel <call_id>
```

其中：

- `resume`：断线后重新挂回进行中的 call
- `logs --follow`：查看仍被保留的 stdout / stderr 回放，并继续跟随
- `cancel`：显式取消长任务，并要求目标设备终止对应进程

### 2. shell policy 查询

为了让 caller 不必记忆所有 policy，建议新增一个受 grant 保护的只读能力：

- `shell.policy.list`

返回：

- `policy_id`
- `name`
- `description`
- `exec_mode`
- `risk_level`
- `auth_requirements`
- `supports_stdin`
- `supports_pty`

注意：

- 这里只返回摘要，不回传完整 regex / 明文规则

### 2.1 审批协议

审批不是 UI 附属能力，而是 shell 调用协议的一部分。实现规则如下：

1. `openCall` 在命中需要审批的 policy / scope 时，不直接启动进程
2. client 创建一条 `approval_request`
3. relay 向 caller 返回 `status=pending_approval`
4. 设备 owner 审批后，client 才允许真正进入进程创建
5. 审批被拒绝、过期或策略版本变化时，call 直接结束为拒绝态

建议新增审批对象：

```json
{
  "approval_id": "uuid",
  "call_id": "uuid",
  "policy_id": "trusted-ops",
  "shell_policy_set_version_snapshot": 42,
  "approval_scope_digest": "sha256:...",
  "requested_by": {
    "caller_fingerprint": "fp",
    "auth_method": "pair_code"
  },
  "status": "pending",
  "expires_at": "2026-04-22T09:30:00Z"
}
```

审批状态固定为：

- `pending`
- `approved`
- `rejected`
- `expired`
- `invalidated`

协议行为固定为：

- `pending`：call 状态为 `pending_approval`
- `approved`：call 继续进入执行阶段
- `rejected`：返回 `approval_rejected`
- `expired`：返回 `approval_expired`
- `invalidated`：审批期间若 policy / scope / binding 版本变化，原审批作废，caller 必须重新发起请求

### 2.2 审批接口契约

审批结果通过显式 API 提交，不通过隐式 UI 状态同步。固定接口如下：

```text
POST /remote/approvals/:approval_id/approve
POST /remote/approvals/:approval_id/reject
GET  /remote/approvals/:approval_id
```

`approve` 请求体：

```json
{
  "actor": {
    "actor_type": "device_owner",
    "actor_id": "user-123"
  },
  "expected_call_id": "uuid",
  "expected_shell_policy_set_version_snapshot": 42,
  "reason": "approved by owner"
}
```

`reject` 请求体：

```json
{
  "actor": {
    "actor_type": "device_owner",
    "actor_id": "user-123"
  },
  "expected_call_id": "uuid",
  "reason": "request exceeds allowed risk"
}
```

接口规则固定为：

1. 审批对象由 client 本地持久化，relay 仅保存审批路由索引（不保存审批内容摘要）
2. `approve/reject` 必须幂等；同一 `approval_id` 重复提交返回相同最终状态
3. `approve` 时必须校验：
   - `approval_id` 存在
   - 状态仍为 `pending`
   - `expected_call_id` 匹配
   - `expected_shell_policy_set_version_snapshot` 与当前审批对象一致
4. 若审批对象已 `expired/rejected/approved/invalidated`，再次提交不得改变最终结果
5. client 接收到 `approved` 后负责把 call 从 `pending_approval` 推进到真正执行
6. client 接收到 `rejected` 或 `expired` 后负责把 call 终态写回并回传 relay
7. **enforcement 联动约束**：如果审批后的 call 对应的 `approval_mode = auto_within_profile`，approve 时必须额外校验当前 enforcement backend 是否提供 `sandbox_profile.required_enforcement_capabilities` 的全维度覆盖。如果 enforcement 不完整，即使审批通过，也必须将 approval_mode 强制降级为 `manual_every_time`，并在审批摘要中标记 `enforcement_downgraded = true`。详见"OS 级安全沙箱 Enforcement Backend §4.6"。

审批摘要对象至少包含：

- `approval_id`
- `call_id`
- `status`
- `policy_id`
- `shell_policy_set_version_snapshot`
- `created_at`
- `expires_at`
- `resolved_at`
- `resolved_by`

### 3. openCall 请求体

建议在 `calls/open` 中继续复用现有 `command` 字段，但支持 `kind=shell.exec`。

示例：

```json
{
  "grant_id": "uuid",
  "caller_fingerprint": "fp",
  "command": {
    "kind": "shell.exec",
    "policy_id": "deploy-api",
    "exec_mode": "template",
    "template_vars": {
      "env": "prod",
      "sha": "abc1234"
    },
    "timeout_ms": 600000,
    "pty": {
      "enabled": false
    }
  }
}
```

如果请求需要审批，`calls/open` 的同步返回应为：

```json
{
  "call_id": "uuid",
  "state": "pending_approval",
  "approval_required": true,
  "approval_id": "uuid"
}
```

只有当审批通过后，worker 才会产出真正的执行态 frame。

### 4. frame 协议

现有 frame / exit 机制可以直接扩展，但输出能力不能只理解成“打印两路文本”。

设计上建议把输出分成 3 层：

1. 原始输出通道
   - `stdout`
   - `stderr`
2. 控制与状态通道
   - `control`
   - `status`
3. 输出附件通道
   - `artifact`

这样既能覆盖标准输出 / 标准错误输出，也能覆盖：

- 长任务心跳
- 进度上报
- 二进制输出
- 过大输出的附件化
- PTY 合流输出

建议统一 frame 类型：

- `stdin`
- `stdout`
- `stderr`
- `control`
- `status`
- `artifact`

其中 `control` 继续细分：

- `eof`
- `resize`
- `signal`

其中 `status` 建议至少支持：

- `heartbeat`
- `progress`
- `notice`
- `warning`

其中 `artifact` 用于：

- 输出不是可直接按文本展示的内容
- 输出体积过大，不适合持续内联推流
- 输出本身就是结构化结果，如 JSON 报告、二进制日志包、压缩包

### 4.1 输出通道原则

必须明确几条基础原则：

1. **非 PTY 模式下，必须保留 stdout 和 stderr 两条独立通道**
2. **`stderr` 非空不等于执行失败，真正的成功/失败以 `exit_code + termination_reason` 为准**
3. **PTY 模式下不强求还原 stdout/stderr 分离，因为终端语义上本来就是合流的**
4. **所有回放与审计都要基于 frame seq 排序，而不是基于“看起来像一行日志”**

因此推荐：

- 非 PTY：
  - `output_mode = split_streams`
  - 原样采集 `stdout` / `stderr`
- PTY：
  - `output_mode = pty_merged`
  - 统一以单一终端输出流回传
  - 不再承诺 stderr 可精确拆分

### 4.2 frame 公共字段

建议所有输出相关 frame 都带统一头部：

| 字段 | 说明 |
| --- | --- |
| `seq` | 单调递增的 frame 序号 |
| `ts` | 采集时间 |
| `frame_type` | `stdout` / `stderr` / `control` / `status` / `artifact` |
| `stream` | `stdout` / `stderr` / `pty` / `meta` |
| `offset` | 对应 spool 中的字节偏移 |
| `chunk_bytes` | 当前 frame 的原始字节数 |
| `encoding_hint` | 编码提示 |
| `newline_hint` | 换行提示 |
| `is_binary` | 是否应按二进制处理 |
| `truncated` | 当前 frame 是否已被截断 |

对于文本或字节输出：

- 传输层仍以字节序列为准
- JSON 传输时使用 base64
- digest 基于原始字节计算

### 4.3 输出 frame 示例

`stdout` 示例：

```json
{
  "seq": 42,
  "ts": "2026-04-22T09:12:01Z",
  "frame_type": "stdout",
  "stream": "stdout",
  "offset": 8192,
  "chunk_bytes": 128,
  "encoding_hint": "utf-8",
  "newline_hint": "lf",
  "is_binary": false,
  "truncated": false,
  "data_b64": "Li4u"
}
```

`status.progress` 示例：

```json
{
  "seq": 43,
  "ts": "2026-04-22T09:12:03Z",
  "frame_type": "status",
  "stream": "meta",
  "status": {
    "kind": "progress",
    "label": "cargo build",
    "percent": 63
  }
}
```

### 4.4 二进制输出与附件

必须考虑并不是所有命令输出都适合按文本展示，例如：

- `xxd`, `sqlite3 .db`, `tar`, `zip`, `openssl`, `hexdump`
- 生成 JSON 报告、JUnit XML、coverage 文件
- PowerShell 输出对象序列化结果

推荐策略：

- 小体积二进制输出：
  - 仍可走 frame 流
  - `is_binary = true`
- 大体积或结构化输出：
  - 本地写入 artifact
  - 流上回一个 `artifact` frame
  - 返回 `artifact_id`、`name`、`mime_type`、`size_bytes`、`sha256`

artifact 建议用于：

- 二进制日志包
- JSON / XML / HTML 报告
- 需要单独下载或局部读取的结果文件

### 4.5 输出视图

同一份底层输出，建议支持 3 种读取视图：

1. `raw_stdout`
   - 只看 stdout
2. `raw_stderr`
   - 只看 stderr
3. `merged`
   - 按 `seq` 做时间序合流展示

这样可以同时满足：

- CLI 默认看合流日志
- 调试时单独看 stderr
- 审计时保留原始分流事实

对于 PTY：

- 只提供 `pty` / `merged` 视图
- 不承诺 `raw_stderr`

### 4.6 背压与慢消费者

输出链路必须假设 caller 可能很慢、relay 可能抖动、任务可能极其能刷屏。

推荐原则：

- **client 本地 spool 永远是权威输出源**
- relay 只是转发与少量缓存
- caller 跟不上时，不应直接反压到目标进程导致命令行为异常

建议：

- worker 读子进程 pipe / PTY 后优先写本地 spool
- relay 只负责尽力转发最近 frame
- caller 慢消费时允许掉出实时跟随，再通过 `cursor` 补拉

### 4.7 输出上限策略

仅有 `max_output_bytes` 不够，建议拆分成：

| 字段 | 说明 |
| --- | --- |
| `max_stdout_bytes` | stdout 上限 |
| `max_stderr_bytes` | stderr 上限 |
| `max_combined_output_bytes` | 总输出上限 |
| `max_artifact_bytes` | 单个附件大小上限 |
| `output_limit_policy` | `truncate_capture` / `terminate_process` |

推荐默认：

- `output_limit_policy = truncate_capture`
- 超限后停止继续采集对应输出
- 但命令本身是否继续运行，由策略决定
- 最终 `exit` 必须明确标记哪些通道被截断

这样可以避免：

- 因日志过大把 relay 或 caller 拖死
- 因单一路 stderr 爆量导致 stdout 被误伤

### 4.8 零输出和仅 stderr 输出

还要明确两个容易被忽略的场景：

- 某些命令成功但完全没有输出
- 某些命令成功但只写 stderr，例如告警型工具或平台特定 CLI

因此：

- `stdout` 为空不能视为异常
- `stderr` 非空不能自动映射为失败
- `status` / `exit` 必须能独立说明任务仍在运行或已完成

示例：

```json
{
  "frame_type": "control",
  "control": {
    "kind": "resize",
    "cols": 160,
    "rows": 48
  }
}
```

### 5. exit 语义

保留现有 exit 事件，并扩展：

- `exit_code`
- `duration_ms`
- `stdout_digest`
- `stderr_digest`
- `stdout_truncated`
- `stderr_truncated`
- `stdout_bytes`
- `stderr_bytes`
- `output_mode`
- `binary_output_present`
- `artifact_count`
- `last_frame_seq`
- `termination_reason`

其中 `termination_reason` 可能为：

- `completed`
- `timeout`
- `cancelled`
- `policy_rejected`
- `spawn_failed`
- `signal_killed`

### 5.1 失败原因必须可解释

这里必须明确一个产品要求：

- **执行失败时，caller 必须拿到明确失败原因**

不能只返回：

- `policy_rejected`
- `permission denied`
- `exit_code = -1`

因为这类结果对用户几乎不可操作。

至少要能区分：

- 不在白名单范围
- policy 存在但当前 caller 没绑定
- `cwd` 超出允许范围
- `env` 不在 allowlist
- 当前 `auth_method` 不允许
- 需要人工审批但未审批
- 平台不支持该 policy
- grant 已过期或 scope 不匹配

### 5.2 统一错误模型

建议所有“未进入进程执行”的失败，都走统一错误结构，而不是只塞进 `stderr`：

```json
{
  "status": "rejected",
  "error_code": "scope_cwd_not_allowed",
  "error_category": "policy_scope",
  "user_message": "当前请求的工作目录不在该策略允许的白名单范围内。",
  "operator_detail": {
    "policy_id": "deploy-api",
    "field": "cwd",
    "matched_scope_id": "deploy-prod-scope"
  },
  "remediation_hint": "请改用允许的工作目录，或在目标设备上调整该策略的 cwd 范围。",
  "retryable": false
}
```

推荐字段：

| 字段 | 说明 |
| --- | --- |
| `error_code` | 稳定错误码，给 CLI / WebUI / 自动化判断 |
| `error_category` | 错误大类 |
| `user_message` | 面向终端用户的可读提示 |
| `operator_detail` | 面向设备 owner / 审计的结构化详情 |
| `remediation_hint` | 下一步建议 |
| `retryable` | 是否适合重试 |

### 5.3 推荐错误分类

建议错误大类至少包含：

- `grant`
- `auth`
- `policy_binding`
- `policy_scope`
- `approval`
- `platform`
- `runtime`
- `transport`

### 5.4 推荐错误码

下面这些错误码建议作为稳定枚举：

#### Grant / Auth

| 错误码 | 说明 |
| --- | --- |
| `grant_not_found` | 本地或 relay 未找到 grant |
| `grant_expired` | grant 已过期 |
| `grant_scope_mismatch` | 当前 grant 不允许 shell.exec |
| `auth_method_not_allowed` | 当前认证方式不允许该 policy |
| `caller_not_bound` | 当前 caller 未绑定到该 policy/tag |

#### Policy / Scope

| 错误码 | 说明 |
| --- | --- |
| `policy_not_found` | 请求的 policy 不存在 |
| `policy_disabled` | policy 已被禁用 |
| `sandbox_profile_missing` | policy 未绑定可用的 sandbox profile |
| `policy_tag_not_allowed` | policy tag 不在允许范围 |
| `scope_default_deny` | 没有任何 allow scope 命中，按默认拒绝 |
| `scope_denied_by_binding` | 命中显式 deny binding |
| `scope_denied_by_rule` | 命中显式 deny scope |
| `scope_platform_not_allowed` | 当前平台不允许该 policy |
| `scope_exec_mode_not_allowed` | 当前 exec_mode 不被允许 |
| `scope_executable_not_allowed` | executable 不在白名单范围 |
| `scope_cwd_not_allowed` | cwd 不在允许范围 |
| `scope_arg_not_allowed` | 参数不符合 schema |
| `scope_env_key_not_allowed` | env key 不允许 |
| `scope_env_value_not_allowed` | env value 不符合规则 |
| `scope_stdin_not_allowed` | stdin 模式或大小不允许 |
| `scope_time_window_not_allowed` | 当前时间不在允许窗口 |
| `scope_quota_exceeded` | 触发并发或调用配额限制 |
| `scope_runtime_limit_exceeded` | 请求的 runtime 超过允许范围 |
| `scope_pty_not_allowed` | 当前策略不允许 PTY |
| `sandbox_fs_denied` | 文件系统访问超出 sandbox profile 范围 |
| `sandbox_network_denied` | 网络访问不在 sandbox profile 允许范围 |
| `sandbox_command_denied` | 命令形态不在 sandbox profile 允许范围 |
| `sandbox_process_denied` | 进程行为不在 sandbox profile 允许范围 |

#### Approval

| 错误码 | 说明 |
| --- | --- |
| `approval_required` | 该请求需要审批后才能执行 |
| `approval_rejected` | 审批被拒绝 |
| `approval_expired` | 审批窗口已过期 |

#### Platform / Runtime

| 错误码 | 说明 |
| --- | --- |
| `platform_variant_missing` | 当前平台没有配置可用变体 |
| `platform_pty_unsupported` | 当前平台或系统版本不支持 PTY/ConPTY |
| `spawn_failed` | 进程创建失败 |
| `process_cancelled` | 任务被取消 |
| `process_timeout` | 任务运行超时 |
| `worker_lost` | client worker 心跳丢失 |

#### Enforcement

| 错误码 | 说明 |
| --- | --- |
| `sandbox_enforcement_unavailable` | 当前平台无可用的 OS 级 enforcement backend，无法满足 sandbox profile 所需的隔离要求 |
| `sandbox_enforcement_degraded` | enforcement backend 可用但部分维度降级（如 Landlock 回退无 PID 隔离），审计中标记降级维度 |
| `sandbox_enforcement_setup_failed` | OS 级沙箱构建失败（如 bwrap 启动失败、SBPL 语法错误） |

### 5.5 面向用户的错误提示要求

`user_message` 应满足 3 个要求：

1. 直接说明失败原因
2. 直接指出是哪一类限制触发
3. 给出下一步建议

推荐示例：

- `scope_default_deny`
  - “当前请求不在目标设备允许的白名单范围内。”
- `scope_cwd_not_allowed`
  - “当前请求的工作目录不在该策略允许的范围内。”
- `scope_env_key_not_allowed`
  - “当前请求包含未被允许的环境变量：NODE_OPTIONS。”
- `caller_not_bound`
  - “当前调用方没有被绑定到该策略，无法执行此命令。”
- `approval_required`
  - “该请求属于高风险操作，需要在目标设备上人工确认后才能执行。”

### 5.6 不要泄露过多策略细节

虽然需要“明确反馈原因”，但也不能把完整白名单规则原文回给 caller。

推荐原则：

- caller 看到“哪一类限制被触发”
- 设备 owner / 本地审计看到“具体是哪条 scope / binding 触发”
- 不向 caller 直接返回：
  - 完整 regex
  - 完整 deny 规则表达式
  - 全量允许路径列表
  - 其他 caller 的绑定信息

例如：

- 可以返回：
  - “工作目录不在允许范围内”
- 不建议返回：
  - “仅允许 `/srv/api/releases/prod`、`/srv/api/hotfix`、`/mnt/secret-x`”

### 5.7 CLI / WebUI 展示规范

CLI 建议展示两行：

```text
Error: current working directory is not allowed by the target policy
Hint: use an allowed working directory or update the client-side shell scope
```

WebUI / Admin History 建议展示：

- `status = rejected`
- `error_code = scope_cwd_not_allowed`
- `user_message`
- `policy_id`
- `matched_scope_id`（仅设备 owner 可见）

### 5.8 审计字段规范

对于被拒绝的请求，建议 CallInfo / 审计记录额外保存：

| 字段 | 说明 |
| --- | --- |
| `rejection_code` | 稳定错误码 |
| `rejection_category` | 错误大类 |
| `rejection_message` | 用户可见提示 |
| `matched_binding_id` | 命中的 binding |
| `matched_scope_id` | 命中的 scope |
| `rejected_field` | 触发拒绝的字段，如 `cwd` / `env` / `auth_method` |
| `rejected_value_digest` | 被拒绝值的 digest，避免明文落库 |

### 5.9 判定顺序必须产出“首个明确失败原因”

client 侧不要把所有失败都折叠成一个总的 “policy rejected”。

推荐：

- 判定链路按固定顺序执行
- 一旦命中首个阻断条件，就返回该条件对应的错误码
- 记录完整内部 trace 到本地审计
- 但 caller 只看到首个最可操作的失败原因

例如：

1. `grant_scope_mismatch`
2. `caller_not_bound`
3. `scope_cwd_not_allowed`
4. `approval_required`

如果第 2 步已经失败，就不继续返回第 3、4 步给 caller。

### 5.10 推荐错误输出策略

推荐分层：

- **前置拒绝**
  - 通过 `status=rejected + error_code + user_message`
  - 不伪装成进程 exit code
- **运行时失败**
  - 保留 `exit_code`
  - 同时补充 `termination_reason` / `error_code`

这样 caller 能区分：

- “命令没被允许执行”
- “命令已经启动，但执行失败”

### 6. 长任务执行模型

“长任务”不能只靠把 `timeout_ms` 调大来处理，必须把以下 4 个时间维度拆开：

1. `max_runtime_ms`
   - 进程允许实际运行多久
2. `idle_heartbeat_timeout_ms`
   - 多久没有 worker 心跳就认为执行端异常
3. `viewer_resume_ttl_ms`
   - caller 断线后，保留多久允许重新挂回
4. `retention_ttl_ms`
   - 任务结束后，stdout / stderr 回放保留多久

建议默认值：

| 参数 | 建议值 | 说明 |
| --- | --- | --- |
| `heartbeat_interval_ms` | `5000` | worker 每 5 秒上报一次运行心跳 |
| `idle_heartbeat_timeout_ms` | `30000` | 30 秒无心跳视为执行端失联 |
| `viewer_resume_ttl_ms` | `10 min` | caller 断线后 10 分钟内可续连 |
| `retention_ttl_ms` | `24 h` | 已结束任务的回放默认保留 24 小时 |

核心语义：

- **运行时长** 不等于 **连接时长**
- caller 的 SSE 断开，不应立刻杀死长任务
- 只要 client worker 仍在稳定心跳，任务就应继续
- caller 恢复后，应可基于 `call_id + resume_token` 续连并补拉缺失输出

### 7. 长任务状态机

建议给 shell call 增加更细的状态：

- `pending`
- `running`
- `streaming`
- `detached_waiting_resume`
- `completed`
- `failed`
- `cancelled`
- `timeout`
- `lost_worker`

其中：

- `detached_waiting_resume`
  - 进程仍在跑，但 caller 当前未附着
- `lost_worker`
  - worker 心跳超时，不能确认任务是否还活着，需要进入异常处理

这里要特别强调两种不同事件：

- **caller 断线**
  - 进入 `detached_waiting_resume`
  - 进程继续运行
  - 允许后续 `resume`
- **caller 显式 cancel**
  - 不进入 `detached_waiting_resume`
  - 必须进入取消流程并终止目标进程
  - 不允许后续 `resume`

### 8. 心跳与保活

长任务经常会出现“很久没有 stdout/stderr”的情况，例如：

- `pnpm install`
- `cargo build`
- `docker build`
- 部署脚本等待远端服务 ready

因此不能用“长时间没输出”判断任务死亡，必须单独做 **运行心跳**。

推荐：

- worker 在任务执行期间固定频率发送 `call_heartbeat`
- 即使没有 stdout/stderr，也要更新：
  - `runtime_heartbeat_at`
  - `pid` / `process_group_id`（可选）
  - `bytes_out_so_far`
  - `last_output_at`
- relay 向 caller 转发轻量 `heartbeat` 事件，避免 CLI 误判卡死

### 9. 断线续连

长任务必须支持 caller 断线后重新挂回。

建议机制：

1. `openCall` 成功后立即返回 `call_id`
2. relay 额外签发 `resume_token`
3. caller 本地把 `call_id + resume_token` 写入连接缓存
4. SSE 断开时：
   - 任务不立即取消
   - call 进入 `detached_waiting_resume`
5. caller 重连时：
   - `GET /calls/:call_id/events?resume_token=...&cursor=...`
   - 从上次游标继续收流

这里的 `cursor` 建议是单调递增的 frame 序号，而不是字节偏移，这样更容易做幂等与去重。

### 10. 输出落盘与回放

长任务输出可能非常大，不能只靠内存 buffer。

推荐策略：

- **client 本地 spool 文件** 作为权威输出副本
- relay 只保存：
  - 当前游标
  - 最近少量回放索引（用于 caller 续连时的 frame 补拉路由）
- caller 续连时优先通过 relay 请求 client 补拉缺失 frame

建议本地 spool 结构：

- `stdout.log`
- `stderr.log`
- `pty.log`
- `index.json`
- `artifacts/`
- `manifest.json`

其中 `index.json` 记录：

- frame seq
- stream type
- byte range
- timestamp
- encoding hint
- binary flag

这样可以支持：

- 断线后按 seq 补拉
- 结束后查看 tail
- 审计时只读取局部片段
- 按 stream 做定向回放
- 定位 artifact 与原始输出的关系

`manifest.json` 建议额外记录：

- `output_mode`
- `stdout_bytes`
- `stderr_bytes`
- `pty_bytes`
- `artifact_count`
- `stdout_digest`
- `stderr_digest`
- `merged_tail_preview`

### 10.1 回放接口规范

建议 `shell logs` 支持：

- `shell logs <call_id> --stream stdout`
- `shell logs <call_id> --stream stderr`
- `shell logs <call_id> --stream merged`
- `shell logs <call_id> --stream pty`
- `shell logs <call_id> --since-seq <n>`
- `shell logs <call_id> --tail <bytes>`

如果后续支持 artifact，还建议新增：

- `shell artifacts <call_id>`
- `shell artifact get <call_id> <artifact_id>`

### 10.2 审计和展示规范

CLI / WebUI 默认建议：

- 默认展示 `merged` 视图
- 明确用颜色或标签区分 stderr
- 标记：
  - `binary`
  - `truncated`
  - `artifact emitted`
  - `pty merged`

同时保留高级视图：

- 只看 stderr
- 只看 status / progress
- 查看输出摘要与 digest

### 11. 长任务结束后的结果获取

对于持续几十分钟甚至数小时的任务，caller 不一定能一直挂着。

因此任务结束后应支持：

- `shell logs <call_id>`
- `shell logs <call_id> --follow`
- `shell status <call_id>`

其中 `status` 应至少返回：

- 当前状态
- 起始时间 / 结束时间
- 最后心跳时间
- 输出大小
- 是否截断
- exit_code
- termination_reason

### 12. 长任务取消语义

长任务取消不能只做一个 HTTP cancel 然后立刻认为完成。

推荐 3 段式：

1. `cancel_requested`
   - relay 标记 call 已请求取消
2. `graceful_terminate`
   - worker 向进程组发送 `SIGTERM` / 平台等价信号
3. `force_kill`
   - 超过 `cancel_grace_ms` 仍未退出，则强杀

### 12.1 caller cancel = client kill 的强一致语义

这里建议作为强约束写死：

- **当 caller 显式执行** `bifrost remote shell cancel <call_id>` **时，client 必须终止对应命令**

不能接受的实现：

- 只关闭 caller 侧 SSE
- 只把 call 标记成 cancelled
- 只停止 relay 转发，但目标进程继续在 client 上运行

正确语义应当是：

1. caller 发起 cancel
2. relay 将 `call_cancel` 作为高优先级控制事件下发给 client
3. client worker 定位到该 `call_id` 对应的进程 / 进程组 / Job Object
4. client 先执行优雅终止
5. 超过 grace period 后执行强杀
6. client 回传最终取消结果
7. relay 将 call 标记为 `cancelled`

也就是说：

- **cancel 是针对“执行实体”的终止命令**
- 不只是针对“流”或“订阅”的关闭命令

### 12.2 cancel 的对象必须是进程组，而不是单一 pid

很多 shell 命令会再拉起子进程，例如：

- `bash -lc 'pnpm build'`
- `powershell -Command "npm run deploy"`
- `python script.py` 再拉子进程

因此 client 侧取消时，目标不应只是 shell 父进程，而应是：

- Unix：整个 process group / session
- Windows：整个 Job Object / 进程树

否则会出现：

- caller 看到 cancelled
- shell 父进程退出
- 真正的子进程还在后台继续跑

这是必须避免的。

### 12.3 cancel ack 语义

建议把取消结果拆成两个阶段：

1. `cancel_ack`
   - client 已收到取消命令，并开始终止流程
2. `cancel_exit`
   - 目标进程已实际退出，call 真正结束

原因：

- 长任务 kill 不一定瞬间完成
- caller 需要知道“取消请求已送达”和“进程已真正结束”是两个不同状态

推荐 caller 展示：

- `Cancelling remote command...`
- `Remote command terminated.`

而不是在发送 cancel 之后立即假设任务已经结束。

### 12.4 cancel 超时与失败语义

如果 client 在取消流程中出现异常，也要明确反馈：

- `cancel_delivery_timeout`
  - relay 无法把 cancel 送达 client
- `cancel_ack_timeout`
  - client 未在预期时间内确认收到 cancel
- `cancel_kill_timeout`
  - client 已收到 cancel，但进程在强杀窗口后仍未确认退出
- `cancel_target_not_found`
  - 对应进程句柄已丢失或 call 映射不存在

建议：

- `cancel_target_not_found`
  - 如果本地审计表明该 call 已经结束，可视为幂等成功
- `cancel_kill_timeout`
  - call 进入异常态，要求本地审计标红，并禁止 resume

### 12.5 cancel 后不允许 resume

需要明确：

- caller 断线 -> 可 `resume`
- caller 显式 cancel -> 不可 `resume`

因此一旦 call 进入：

- `cancel_requested`
- `cancelled`

就应撤销：

- `resume_token`
- 活跃输出订阅资格

避免出现“用户已经取消，但又重新 attach 回一个理论上应该被杀掉的任务”。

### 12.6 平台 kill 策略

#### macOS / Linux

- 优雅取消：
  - 向 process group 发送 `SIGTERM`
- 强制取消：
  - 向 process group 发送 `SIGKILL`

#### Windows

- 优雅取消：
  - `CTRL_BREAK_EVENT` 到控制台进程组，或平台等价的温和终止
- 强制取消：
  - `TerminateJobObject` / 终止整个 Job Object

约束：

- shell policy 执行器必须在 spawn 时就记录可取消句柄
- 不能等到 cancel 时再尝试“猜”该杀哪个进程

### 12.7 caller 取消后的最终结果

当 cancel 成功完成后，建议最终回传：

- `termination_reason = process_cancelled`
- `error_code = process_cancelled`
- `exit_code`
  - 可为平台本地退出码，或内部约定的 cancelled code

caller 看到的结论应当是：

- 命令已被远端终止

而不是：

- 连接已关闭
- 输出流已停止

建议新增字段：

- `cancel_requested_at`
- `cancel_ack_at`
- `killed_at`
- `kill_reason`
- `cancel_delivery_status`
- `cancel_delivery_error`
- `cancel_target_kind`

### 13. 长任务与 grant 过期的关系

grant 过期不应影响**已经启动**的长任务。

推荐规则：

- grant 只在 `openCall` 时校验
- call 一旦成功创建，就拥有独立生命周期
- 即使运行期间 grant 过期：
  - 已启动 call 可继续执行
  - 但不能再新开 call

否则会出现：

- 1 小时 grant 启动了 2 小时构建
- 构建跑到 1 小时 1 分时被系统强杀

这类体验会非常差。

### 14. 长任务与策略变更的关系

与 grant 类似，策略变更不应立即打断**已经开始**的 call。

推荐：

- `shell_policy_set_version` 在 `openCall` 时校验
- call 启动后冻结本次执行上下文
- 后续策略更新只影响新 call

### 15. 长任务能力边界

当前版本必须包含：

- 非 PTY 长任务
- Unix PTY 长任务
- Windows ConPTY 长任务
- worker 心跳
- caller 断线 10 分钟内续连
- client 本地 spool
- `logs/status/cancel/resume`

当前版本不做：

- 无限期 detach
- 多 caller 同时 attach 同一 PTY
- 任务结束后永久保存全量 stdout/stderr

## 目标设备本地执行器

### 1. 推荐新增 RemoteShellExecutor

建议与当前只读 executor 并列：

- `RemoteQueryExecutor`
- `RemoteShellExecutor`

不要把两者硬塞进同一分支，避免安全边界被混淆。

### 2. 执行阶段

执行前必须依次经过：

1. `grant_scope` 校验
2. `auth_method` 校验
3. `policy_id` 存在且启用
4. `shell_policy_set_version` 一致性校验
5. `exec_mode` 与请求结构匹配
6. `sandbox_profile` 应用层边界检查（`filesystem_scope` / `network_scope` / `command_scope` / `process_scope`）
7. `cwd` 校验
8. `env` 校验
9. `stdin` 大小与模式校验
10. `timeout` 上限校验
11. `pty` 开关校验
12. `approval_mode` 审批门（如需审批则阻塞等待，详见"§2.1 审批协议"）
    - 如果 `approval_mode = full_trust_no_sandbox` 且设备已开启 `allow_full_trust_mode`，跳过步骤 13-15，直接以当前用户权限启动子进程（详见 §4.5 执行流程步骤 1.5 和 4b）
13. **Enforcement Backend 就绪检查**（详见"OS 级安全沙箱 Enforcement Backend §4.5"）：
    - 查询当前平台的 `enforcement_backend`
    - 验证 `enforcement_capabilities` 覆盖 `sandbox_profile.required_enforcement_capabilities` 所有维度
    - 如果任一必需维度无法 enforce → 返回 `sandbox_enforcement_unavailable`
    - 如果 `approval_mode = auto_within_profile` 但 enforcement 不完整 → 强制降级为 `manual_every_time`
    - 记录 `enforcement_backend` / `enforcement_status` 到审计字段
14. **OS 级沙箱构建**：
    - 将 `sandbox_profile` 映射为平台 enforcement 参数（Linux: BwrapConfig + SeccompPolicy；macOS: SeatbeltConfig + SBPL）
    - 验证沙箱配置完整性（writable 路径存在、deny 路径不与 write 冲突等）
15. **在沙箱内启动子进程**（而非直接 `Command::new()`），或在 `full_trust_no_sandbox` 模式下直接 `Command::new()` 启动

任何一步失败都直接返回，不进入进程创建。

> ⚠️ **跨模块对齐提示**：步骤 13-15 的详细映射规则、降级路径和平台差异，参见"OS 级安全沙箱 Enforcement Backend"章节的 §4.4（映射规则）、§4.5（执行流程）、§4.6（必须性规则）。开发 RemoteShellExecutor 时必须同时参照该章节。

### 3. 环境变量策略

默认不要把 Bifrost 进程当前环境全量继承给远程 shell。

推荐模式：

- 基础最小环境
- 可选继承安全白名单变量
- caller 只能传 policy 明确允许的 env key

例如：

- 允许：`PATH`, `HOME`, `LANG`, `NODE_ENV`
- 不允许默认透传：`AWS_SECRET_ACCESS_KEY`, `SSH_AUTH_SOCK`, `OPENAI_API_KEY`

### 4. 工作目录策略

默认不允许 caller 任意覆盖 `cwd`。

推荐：

- `fixed`: 完全固定
- `allow_override`: 只能在允许的前缀目录下覆盖

### 5. 输出和资源限制

每条策略都应具备：

- `max_timeout_ms`
- `max_output_bytes`
- `max_stdout_bytes`
- `max_stderr_bytes`
- `max_combined_output_bytes`
- `max_artifact_bytes`
- `output_limit_policy`
- `max_stdin_bytes`
- `max_concurrency`
- `quota_window`
- `quota_bucket_key`

执行器应支持：

- 实时流式输出
- stdout / stderr 分流采集
- PTY 模式下显式标记为合流输出
- 超过上限后截断并标记
- 二进制输出识别与附件化
- cancel 时先优雅终止，超时后强杀
- 长任务运行时独立心跳，不依赖 stdout/stderr 活跃度
- caller 断线后进入可续连状态，而不是立即中止进程
- caller 显式 cancel 后必须终止对应进程组 / Job Object，而不是只中断流

### 5.1 Quota 计数语义

`quota_json` 在实现上按固定语义计数：

- 并发数：
  - 以 `running` / `streaming` / `detached_waiting_resume` 状态中的 call 计数
  - `pending_approval` 不占用并发 quota
- 时间窗口：
  - `per_hour` 使用滚动 60 分钟窗口
  - `per_day` 使用目标设备本地时区自然日窗口
- 计数粒度：
  - 默认按 `subject + policy_id` 计数
  - 其中 `subject` 取 binding 解析后的最终主体
- 持久化：
  - quota 计数必须落本地数据库
  - client 重启后恢复
- 幂等：
  - 同一个 `call_id` 不重复记次

### 6. PTY 设计

已确认：

- macOS / Linux 支持 PTY
- Windows 支持 ConPTY，会话可 resize，可传 Ctrl-C

如果做 PTY：

- macOS / Linux 侧使用伪终端
- caller 侧输出 raw stream
- relay 仍只转 frame，不感知终端语义

## WebUI 设计

### 1. Remote Invoke 页面新增 Shell Policies 区域

推荐拆成两个区：

- `Remote Access / Grants`
- `Shell Policies`

### 2. 策略列表字段

显示：

- `name`
- `policy_id`
- `exec_mode`
- `risk_level`
- `allowed_auth_methods`
- `pty_allowed`
- `enabled`
- `last_used_at`

### 3. 创建策略表单

推荐支持三类模板：

1. 受控模板命令
2. argv 工具命令
3. 高级 shell 文本

对高级 shell 文本要有明显红色警告：

- 允许远端提交 shell 文本
- 默认每次审批
- 会提升远控风险

### 4. 审批弹窗

如果 shell 请求需要审批，弹窗应展示：

- caller 信息
- auth_method
- policy_id / policy name
- 风险级别
- 命令预览
- cwd 摘要
- env key 列表
- 请求的 timeout / pty / stdin 大小

## 审计策略

### Relay 透明原则：不存储执行信息

Relay 作为透明中继，**不保存任何与用户执行内容相关的信息**。Relay 仅保留路由级元数据（call_id、device_code、状态、时间戳）用于连接管理，call 结束后不保留命令内容、执行结果、策略匹配结果或 enforcement 状态。

这一设计确保：
- 即使 Relay 被入侵，攻击者无法获取任何用户执行的命令或输出
- 用户的操作隐私完全由设备 owner 本地管控
- Relay 的运维复杂度最小化，无需处理审计数据的存储、加密、过期清理和访问控制

### Client 本地保留完整审计

推荐目标设备本地另存一份更完整的审计记录：

- 明文命令
- 变量值
- 完整退出原因
- 调用发起人
- 可选最近 N KB stdout/stderr
- 长任务 spool 文件路径与保留截止时间

这样既不把高风险内容上传 relay，又保留设备 owner 的追溯能力。

### Secret 注入与本地审计规则

`secret_scope_json` 的执行语义固定如下：

- secret 只允许从目标设备本地 secret store 读取
- caller 不能直接上传 secret 明文作为 env value
- policy / scope 只能引用 secret 标识符，例如 `db.prod.password`
- 执行器在本地解析后注入环境变量或临时文件
- Relay 永远只看到 secret 标识符，不看到 secret 明文
- 本地审计默认也不记录 secret 明文，只记录 secret 标识符、注入位置和使用时间

如果命令输出中疑似回显 secret：

- Relay 侧回放必须二次遮罩
- Client 本地保留原始输出，但该 call 的审计级别必须升高并标记 `secret_leak_suspected`

### 网络限制执行后端

> ⚠️ **已合并至全维度 Enforcement Backend**：以下 `network_enforcement_backend` 的独立字段设计已被下方"OS 级安全沙箱 Enforcement Backend §4.1"的全维度 `enforcement_backend` 抽象取代。网络隔离能力现在作为 `enforcement_capabilities` 中的 `network_isolation` 维度统一管理。开发时请以 §4.1 的 `enforcement_backend` / `enforcement_capabilities` 为准，本节仅保留为历史上下文和网络维度的详细语义参考。

网络隔离作为 enforcement 全维度之一，其执行语义固定如下：

1. `network_scope_json.mode = off`
   - enforcement backend 必须提供 `network_isolation` 能力（Linux: `--unshare-net` / seccomp socket 过滤；macOS: SBPL `(deny network*)`）
2. `network_scope_json.mode = allowlist`
   - enforcement backend 必须支持域名/端口级网络控制（Linux: UDS proxy bridge；macOS: SBPL `(remote tcp ...)`）
3. `network_scope_json.mode = preset_dependencies`
   - 必须展开为确定的 allowlist 集合后再执行
4. `network_scope_json.mode = full`
   - 仅允许 break-glass profile 使用，`enforcement_backend` 不要求 `network_isolation`

平台 enforcement 后端的网络隔离能力（已纳入 §4.2 能力矩阵）：

- Linux bubblewrap+seccomp：✅ 完整网络隔离（network namespace + seccomp socket 过滤 + UDS proxy bridge）
- Linux Landlock+seccomp（降级）：⚠️ 仅 TCP bind/connect 端口级控制（Landlock v4+），v1-v3 无网络控制
- macOS Seatbelt：✅ 完整网络隔离（SBPL `deny network*` + 按域名/端口放行）

降级规则（已纳入 §4.6 Enforcement 必须性规则）：

- 若 profile 要求网络限制（`mode ≠ full`），但当前 enforcement backend 不提供 `network_isolation` 能力，则 `openCall` 必须拒绝，返回 `sandbox_enforcement_unavailable`
- 不允许因为平台不支持就自动降级为"仅审批后放行"

审计字段（已纳入 §6 Enforcement 审计）：

- `enforcement_backend`（全维度标识，含网络隔离能力）
- `enforcement_capabilities`（含 `network_isolation` 维度状态）
- `enforcement_status`
- `sandbox_config_digest`（含网络策略摘要）

## OS 级安全沙箱 Enforcement Backend

> 🔴 **当前版本必须确定并实现**：当前 Sandbox Profile 的所有维度（文件系统、网络、进程、命令）的控制仅停留在应用层策略检查。一旦子进程被 spawn，应用层无法阻止它访问白名单之外的文件、发起网络请求或提权。OS 级 enforcement 是沙箱安全的最后一道物理屏障，必须在当前版本中落地。

### 1. 问题定义

当前沙箱控制模型（`sandbox_profile` 的 `filesystem_scope` / `network_scope` / `process_scope` / `command_scope`）所有校验都发生在**执行器 dispatch 之前**——也就是说，一旦校验通过、子进程启动，子进程本身并不受任何 OS 级约束。这意味着：

- 一条通过白名单的 `git pull` 命令，其子进程可以自由读写 `/etc/passwd`
- 一条 `network_scope.mode = off` 的命令，其子进程仍然可以发起 TCP 连接
- 一条 `allow_privilege_escalation = false` 的命令，其子进程仍然可以调用 `setuid` 二进制

**结论：应用层策略检查是"意图声明"，OS 级 enforcement 才是"物理隔离"。两者必须同时存在。**

### 2. 行业调研：主流 AI Agent 沙箱方案对比（2025-2026）

#### 2.1 OpenAI Codex CLI

Codex CLI 是目前开源 AI Agent 中 OS 级沙箱实现最完整的参考。

**Linux 端：bubblewrap + seccomp 双层隔离**

| 层 | 技术 | 控制面 |
| --- | --- | --- |
| 文件系统 | bubblewrap（`--ro-bind / /` + `--bind <writable_root>`） | 默认只读挂载整个 `/`，仅白名单目录可写；`.git`/`.codex` 等受保护路径强制只读覆盖 |
| 网络 | bubblewrap `--unshare-net`（完全隔离）或 managed proxy 模式（`--unshare-net` + UDS bridge） | 完全断网或仅通过代理访问允许的端点 |
| 进程 | bubblewrap `--unshare-user` + `--unshare-pid` + `--proc /proc` | 用户命名空间隔离 + PID 命名空间隔离 |
| 系统调用 | `PR_SET_NO_NEW_PRIVS` + seccomp-bpf 过滤器 | 阻止提权 + 阻止 socket 创建（网络受限时）；managed proxy 模式下额外阻止 `AF_UNIX`/`socketpair` |
| 文件保护 | 符号链接检测 + glob 扫描展开 + `/dev/null` 覆盖 | 防止通过符号链接逃逸沙箱边界 |

**macOS 端：sandbox-exec (Seatbelt) SBPL 策略**

| 控制面 | 实现 |
| --- | --- |
| 文件系统 | SBPL `(allow file-read*)` + `(allow file-write* (subpath <writable_root>))`；受保护子路径（`.git`）通过 `(require-not (subpath ...))` 排除 |
| 网络 | SBPL `(deny network*)` 或 `(allow network* (remote tcp ...))`，按域名/端口精确放行 |
| 进程 | SBPL `(deny process-exec)` + `(deny process-fork)` 按需限制 |

**降级策略**：bubblewrap 优先；如果 bwrap 不可用或 user namespace 被禁用（如 WSL1），拒绝沙箱命令执行。Legacy Landlock 作为可选回退，但仅在策略可等价映射时启用。

#### 2.2 Anthropic Claude Code

**Linux 端**：bubblewrap 隔离（文件系统 + 网络）

**macOS 端**：Seatbelt (sandbox-exec) 隔离

**核心设计原则**：

- 文件系统隔离和网络隔离必须**同时存在**——缺少网络隔离，被劫持的 Agent 可以窃取 SSH key；缺少文件系统隔离，被劫持的 Agent 可以逃逸沙箱获取网络权限
- 沙箱边界由 OS 级机制强制执行，所有子进程继承相同的安全边界
- 沙箱定义预设边界后，Agent 在边界内自由工作，大幅减少权限弹窗

#### 2.3 Google Gemini CLI

Gemini CLI 提供了最丰富的沙箱后端选择，覆盖所有主流平台：

| 平台 | 后端 | 隔离强度 | 说明 |
| --- | --- | --- | --- |
| macOS | sandbox-exec (Seatbelt) | 中 | 6 种内置 profile（permissive-open/permissive-proxied/restrictive-open/restrictive-proxied/strict-open/strict-proxied） |
| Linux | Docker / Podman | 高 | 完整容器隔离 |
| Linux | gVisor (runsc) | 最高 | 用户态内核拦截所有系统调用 |
| Linux | LXC/LXD | 高 | 完整系统容器，支持 systemd/snapd |
| Windows | Integrity Level (icacls Low) | 低 | 限制文件写入到低完整性级别目录 |

**独特设计：Sandbox Expansion**——当沙箱命令因权限不足失败时，动态请求用户授权扩展沙箱权限（如额外目录或网络访问），一次性生效。

#### 2.4 行业趋势总结

| 维度 | 行业共识 |
| --- | --- |
| Linux 文件系统 | bubblewrap（mount namespace）是事实标准，Landlock 作为轻量补充 |
| Linux 网络 | network namespace（完全隔离）+ seccomp（socket syscall 过滤）双层 |
| Linux 进程 | PID namespace + `PR_SET_NO_NEW_PRIVS` + seccomp |
| macOS 全维度 | sandbox-exec (Seatbelt SBPL) 是唯一可用的进程级沙箱方案 |
| Windows | 生态最弱；Integrity Level / Job Objects / AppContainer 各有局限 |
| 设计原则 | 文件系统隔离 + 网络隔离必须同时存在，缺一不可 |
| 降级策略 | 无可用 enforcement 时拒绝执行，不允许降级为纯应用层检查 |

### 3. 底层技术详解

#### 3.1 Linux: bubblewrap (bwrap)

**原理**：通过 `clone(2)` / `unshare(2)` 创建新的 Linux namespaces，在隔离的挂载树中运行子进程。

**关键能力**：

| Namespace | 作用 | bwrap 参数 |
| --- | --- | --- |
| mount | 构建独立文件系统视图：默认只读 + 白名单可写 | `--ro-bind / /` + `--bind <path> <path>` |
| user | 无需 root 权限创建其他 namespace | `--unshare-user` |
| PID | 进程树隔离，沙箱内 PID 1 负责回收子进程 | `--unshare-pid` + `--proc /proc` |
| network | 完全断网（仅保留 loopback） | `--unshare-net` |
| UTS | 隔离 hostname | `--unshare-uts` |
| IPC | 隔离 System V IPC / POSIX 消息队列 | `--unshare-ipc` |

**文件系统策略映射**：

```text
sandbox_profile.filesystem_scope → bwrap 参数
─────────────────────────────────────────────
read_roots: ["/"]          → --ro-bind / /
write_roots: ["/srv/app"]  → --bind /srv/app /srv/app
tmp_roots: ["/tmp"]        → --bind /tmp /tmp
deny_roots: ["~/.ssh"]     → --ro-bind /dev/null ~/.ssh  (或不挂载)
exec_roots: ["/usr/bin"]   → (已在 ro-bind 中，可执行权限保留)
```

**优势**：无需 root、子进程自动继承、成熟稳定（Flatpak 基础设施）、Codex/Claude 生产验证。

**限制**：依赖 user namespace（部分内核配置可能禁用）；WSL1 不支持。

**Rust 集成方式**：bubblewrap 作为独立外部二进制，通过 `std::process::Command` spawn 调用（`bwrap --ro-bind ... -- <cmd>`），**不需要 Rust FFI 绑定**。`bubblewrap-sys` crate (v0.7.5) 可从源码构建并 bundle bwrap 二进制，但直接依赖系统安装的 bwrap 更灵活（更易升级、减小产物体积）。当前版本要求检测系统 bwrap 可用性并将其纳入启动期能力检测。

#### 3.2 Linux: seccomp-bpf

**原理**：通过 BPF 程序过滤系统调用，在内核层面阻止子进程执行特定操作。

**关键用途**：

| 过滤目标 | 实现 | 效果 |
| --- | --- | --- |
| 网络 socket 创建 | 拦截 `socket(2)` syscall，检查 `domain` 参数，仅允许 `AF_UNIX`（或完全禁止） | 防止子进程创建 TCP/UDP socket |
| 提权 | `prctl(PR_SET_NO_NEW_PRIVS, 1)` | 禁止通过 setuid 二进制提权 |
| 危险系统调用 | 拦截 `ptrace`、`mount`、`pivot_root`、`reboot` 等 | 防止沙箱逃逸 |

**Rust 生态**：`seccompiler` crate (v0.4.0) 由 AWS Firecracker 团队维护（`rust-vmm` org），已在 Firecracker 生产环境大规模验证。提供 JSON → BPF 编译和 Rust API 两种使用方式，支持 x86_64 和 aarch64。`extrasafe` crate (v0.5.1) 在 `seccompiler` 之上封装了 builder pattern API 和预定义的 `RuleSet`（SystemIO、Networking、Threads 等），降低使用门槛。

**与 bubblewrap 的关系**：bubblewrap 管文件系统 + 命名空间隔离，seccomp 管系统调用级别的精细控制。两者叠加使用。

#### 3.3 Linux: Landlock LSM

**原理**：Linux 5.13+ 的内核安全模块，允许无特权进程为自身施加不可逆的文件访问限制。Linux 6.7+ (ABI v4) 新增网络端口控制。

**Rust 生态**：`landlock` crate (v0.4.4, 9 releases) 由 Landlock LSM 内核子系统的作者亲自维护（`landlock-lsm` org），是**官方 Rust 绑定**。API 采用 builder pattern + `CompatLevel` 兼容性协商机制，能优雅处理不同内核版本间的 ABI 差异（无需手动比较 ABI 版本号）。依赖极少（仅 `libc` + `enumflags2` + `thiserror`），MSRV 1.68，MIT/Apache-2.0 双许可。

**关键能力**：

| ABI 版本 | 内核版本 | 能力 |
| --- | --- | --- |
| v1 | 5.13+ | 文件系统 read/write/execute 路径级控制 |
| v2 | 5.19+ | 新增 file truncation、file refer 控制 |
| v3 | 6.2+ | 新增 IOCTL 控制 |
| v4 | 6.7+ | 新增 TCP bind/connect 端口级网络控制 |

**与 bubblewrap 的关系**：Landlock 是进程内自限（in-process self-restriction），不需要创建 namespace，开销极低。可作为 bubblewrap 不可用时的降级方案，也可与 bubblewrap 叠加使用（双层防护）。

**限制**：无法控制 UDP/ICMP（v4 仅支持 TCP）；无法隔离 PID/IPC；策略精细度低于 namespace 方案。

#### 3.4 macOS: sandbox-exec (Seatbelt)

**原理**：macOS 内核级沙箱，通过 SBPL (Sandbox Profile Language) 定义策略。所有子进程继承父进程的沙箱策略。

**调用方式**：

```bash
/usr/bin/sandbox-exec -p '<SBPL_POLICY>' <command>
```

**SBPL 策略映射**：

```scheme
;; filesystem_scope 映射
(version 1)
(deny default)                           ; 默认拒绝
(allow process*)                          ; 允许进程执行
(allow file-read*)                        ; 全局可读
(allow file-write*
  (subpath (param "WRITABLE_ROOT_0"))     ; write_roots[0]
  (subpath "/private/tmp"))               ; tmp_roots
(deny file-write*
  (subpath (param "DENY_ROOT_0")))        ; deny_roots[0]

;; network_scope 映射
(deny network*)                           ; mode = off
;; 或
(allow network-outbound
  (remote tcp "*:443"))                   ; allowlist 模式
```

**现状**：Apple 官方标记为 deprecated，但实际上：
- Seatbelt 内核机制并未废弃，Apple 内部持续使用
- App Sandbox 底层就是 Seatbelt + Cocoa container
- 所有主流 AI Agent（Codex、Claude Code、Gemini）均在生产环境使用
- Apple 不希望第三方直接使用 SBPL 的原因是 SBPL 语法随内核版本变化，但基础文件/网络/进程控制语法多年稳定

**优势**：内核级强制、零依赖（系统自带）、子进程自动继承。

**限制**：SBPL 未公开文档化（需逆向/社区知识）；策略随 macOS 版本可能变化；不支持资源限制（内存/CPU）。

**Rust 集成方式**：与 bubblewrap 类似，sandbox-exec 作为系统自带二进制，通过 `std::process::Command` spawn 调用（`/usr/bin/sandbox-exec -p '<SBPL>' <cmd>`），**不需要额外 Rust crate**。SBPL 策略由 Rust 代码动态生成（字符串模板 + 参数替换），无需 SBPL 解析库。社区有 `sbexec` crate (v0.1.0) 提供薄封装，但过于简单且不活跃，建议自研。

#### 3.5 Windows: 多层防护方案

Windows 平台没有统一的进程级沙箱原语，需要组合多种机制：

| 技术 | 控制面 | 隔离强度 | 复杂度 |
| --- | --- | --- | --- |
| Restricted Token + Low Integrity Level | 文件写入限制（低完整性进程不能写入中/高完整性对象） | 低-中 | 低 |
| Job Objects | 进程组管理、资源限制（内存/CPU/进程数）、`JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` | 中 | 中 |
| AppContainer | 完整沙箱（SID 隔离、capability-based 访问控制、网络限制） | 高 | 高 |
| Windows Filtering Platform (WFP) | 网络级 per-process 防火墙规则 | 高 | 高 |

**当前版本推荐组合**：Restricted Token + Job Objects + ConPTY（覆盖基础文件保护、资源限制、进程回收与交互式控制）。

**增强方向**：AppContainer 完整沙箱（需要 `CreateAppContainerProfile` + capability 声明）。

### 4. Bifrost Enforcement Backend 架构设计

#### 4.1 统一 Enforcement Backend 抽象

将现有仅针对网络的 `network_enforcement_backend` 扩展为全维度 enforcement 抽象：

| 字段 | 说明 |
| --- | --- |
| `enforcement_backend` | 当前平台实际使用的 enforcement 后端标识 |
| `enforcement_capabilities` | 该后端实际能提供的能力集合 |
| `enforcement_status` | 后端就绪状态（`active` / `degraded` / `unavailable`） |

在 `remote_invoke_shell_sandbox_profiles` 中新增：

```
enforcement_backend: "bubblewrap+seccomp" | "seatbelt" | "restricted_token+job_objects" | "landlock+seccomp" | "none"
enforcement_capabilities: ["filesystem_isolation", "network_isolation", "process_isolation", "syscall_filter", "resource_limits"]
```

#### 4.2 Enforcement 能力矩阵

| 能力维度 | bubblewrap+seccomp (Linux) | Landlock+seccomp (Linux 降级) | Seatbelt (macOS) | Restricted Token+Job (Windows) | AppContainer (Windows 增强) |
| --- | --- | --- | --- | --- | --- |
| 文件系统只读/可写隔离 | ✅ mount namespace | ✅ Landlock path rules | ✅ SBPL file-read/write | ⚠️ 仅限制低→高写入 | ✅ SID+capability |
| 网络完全隔离 | ✅ network namespace | ⚠️ 仅 TCP bind/connect (v4+) | ✅ SBPL `(deny network*)` | ❌ 需 WFP | ✅ 默认无网络 |
| 网络 allowlist 放行 | ✅ UDS proxy bridge | ⚠️ 仅端口级 | ✅ SBPL remote tcp | ❌ 需 WFP | ⚠️ capability 声明 |
| PID 隔离 | ✅ PID namespace | ❌ | ❌ | ❌ | ❌ |
| 提权阻止 | ✅ `NO_NEW_PRIVS` + seccomp | ✅ seccomp | ✅ SBPL process rules | ⚠️ Restricted Token | ✅ Low privilege |
| 资源限制 (CPU/Mem/PID数) | ⚠️ 需额外 cgroup | ❌ | ❌ | ✅ Job Objects | ✅ Job Objects |
| 子进程强制继承 | ✅ namespace 天然继承 | ✅ Landlock 天然继承 | ✅ Seatbelt 天然继承 | ✅ Job association | ✅ AppContainer token |
| 无需 root | ✅ user namespace | ✅ | ✅ | ✅ | ✅ |

#### 4.3 平台 Enforcement 选型决策

**Linux—— 首选 bubblewrap + seccomp，Landlock 作为降级**

```text
启动时检测：
  1. bwrap 是否可用？（PATH 查找 + --help 测试）
  2. user namespace 是否可用？（尝试 clone(CLONE_NEWUSER)）
  3. 如果 bwrap 可用 + user ns 可用 → enforcement_backend = "bubblewrap+seccomp"
  4. 如果 bwrap 不可用但 Landlock 可用 → enforcement_backend = "landlock+seccomp"
  5. 如果都不可用 → enforcement_backend = "none"，拒绝非 break-glass 的沙箱命令执行
```

**macOS—— 使用 sandbox-exec (Seatbelt)**

```text
启动时检测：
  1. /usr/bin/sandbox-exec 是否存在且可执行？
  2. 执行简单 SBPL 测试策略验证 Seatbelt 内核支持正常？
  3. 如果通过 → enforcement_backend = "seatbelt"
  4. 如果失败 → enforcement_backend = "none"，拒绝非 break-glass 的沙箱命令执行
```

**Windows—— Restricted Token + Job Objects**

```text
启动时检测：
  1. CreateRestrictedToken API 可用？
  2. Job Object 可创建？
  3. 如果通过 → enforcement_backend = "restricted_token+job_objects"
  4. 如果失败 → enforcement_backend = "none"
```

#### 4.4 Sandbox Profile → OS Enforcement 映射规则

每次 `openCall(kind=shell.exec)` 进入执行阶段时，Executor 必须将 `sandbox_profile` 的声明式策略映射为 OS 级 enforcement 参数：

**Linux bubblewrap 映射**：

```rust
struct BwrapConfig {
    ro_bind: Vec<(PathBuf, PathBuf)>,       // filesystem_scope.read_roots → --ro-bind
    rw_bind: Vec<(PathBuf, PathBuf)>,       // filesystem_scope.write_roots → --bind
    dev_null_bind: Vec<PathBuf>,            // filesystem_scope.deny_roots → --ro-bind /dev/null
    unshare_net: bool,                      // network_scope.mode == "off" → true
    unshare_user: bool,                     // 始终 true
    unshare_pid: bool,                      // 始终 true（确保进程回收）
    seccomp_filter: SeccompPolicy,          // 始终加载：NO_NEW_PRIVS + 按 network_scope 决定 socket 过滤
    proxy_bridge: Option<ProxyBridgeConfig>,// network_scope.mode == "allowlist" → UDS→TCP 代理桥
    cwd: PathBuf,                           // 映射后的工作目录
    env_allowlist: Vec<(String, String)>,   // env_scope 过滤后的环境变量
}
```

**macOS Seatbelt 映射**：

```rust
struct SeatbeltConfig {
    sbpl_policy: String,                    // 动态生成的 SBPL 策略文本
    writable_roots: Vec<PathBuf>,           // filesystem_scope.write_roots → SBPL (subpath ...)
    deny_roots: Vec<PathBuf>,              // filesystem_scope.deny_roots → SBPL (deny file-* (subpath ...))
    network_mode: NetworkMode,              // network_scope → SBPL network rules
    allowed_endpoints: Vec<(String, u16)>,  // network_scope.allowed_domains → SBPL (remote tcp ...)
    env_allowlist: Vec<(String, String)>,
}
```

#### 4.5 Enforcement 执行流程

```text
openCall(kind=shell.exec) 到达 Client Worker
  │
  ├─ 1. 应用层策略检查（现有逻辑）
  │    ├─ grant_scope 校验
  │    ├─ shell_policy 白名单匹配
  │    ├─ sandbox_profile 边界检查
  │    └─ approval_mode 审批门
  │
  ├─ 1.5 full_trust_no_sandbox 旁路检查 ← 新增
  │    ├─ 如果 approval_mode = full_trust_no_sandbox：
  │    │    ├─ 检查设备级 allow_full_trust_mode 是否已开启
  │    │    ├─ 如果未开启 → 回退到 sandbox_profile 绑定的 approval_mode
  │    │    ├─ 如果已开启 → 跳过步骤 2/3/4，直接进入步骤 4b
  │    │    └─ 记录审计：enforcement_backend="none", enforcement_status="user_opted_out"
  │    └─ 否则继续步骤 2
  │
  ├─ 2. Enforcement Backend 就绪检查 ← 新增
  │    ├─ 查询当前平台的 enforcement_backend
  │    ├─ 验证 enforcement_capabilities 覆盖 sandbox_profile 所有维度
  │    ├─ 如果任一必需维度无法 enforce → 返回 sandbox_enforcement_unavailable
  │    └─ 记录 enforcement_backend 到审计字段
  │
  ├─ 3. OS 级沙箱构建 ← 新增
  │    ├─ 将 sandbox_profile 映射为平台 enforcement 参数
  │    ├─ Linux: 构建 bwrap 命令行 + seccomp filter
  │    ├─ macOS: 生成 SBPL 策略文本
  │    ├─ Windows: 配置 Restricted Token + Job Object
  │    └─ 验证沙箱配置完整性（writable 路径存在、deny 路径不与 write 冲突等）
  │
  ├─ 4. 在沙箱内启动子进程
  │    ├─ Linux: bwrap --ro-bind / / --bind ... -- <command>
  │    ├─ macOS: sandbox-exec -p '<SBPL>' <command>
  │    ├─ Windows: CreateProcessAsUser(restricted_token, ...) + AssignProcessToJobObject
  │    └─ 子进程自动继承 OS 级沙箱限制
  │
  ├─ 4b. 无沙箱直接启动子进程（仅 full_trust_no_sandbox）
  │    ├─ 直接 Command::new(<command>)，不包裹任何沙箱
  │    ├─ 子进程继承当前用户的完整权限
  │    └─ 应用层策略检查仍然生效（步骤 1 的白名单过滤）
  │
  └─ 5. 输出流式回传 + 退出处理（现有逻辑）
```

#### 4.6 Enforcement 必须性规则

| sandbox_profile 维度 | 要求的 enforcement 能力 | 无法 enforce 时的行为 |
| --- | --- | --- |
| `filesystem_scope` 有任何 `deny_roots` 或限制 | `filesystem_isolation` | 拒绝执行，返回 `sandbox_enforcement_unavailable` |
| `network_scope.mode` ≠ `full` | `network_isolation` | 拒绝执行 |
| `process_scope.allow_privilege_escalation = false` | `syscall_filter` | 拒绝执行 |
| `process_scope.max_child_processes` / `max_runtime_ms` 有限制 | `resource_limits`（可选，降级为应用层） | 允许降级为应用层超时控制，但必须在审计中标记 `enforcement_degraded` |
| `approval_mode = auto_within_profile` | 上述所有必需能力 | 不允许进入 auto 模式，强制降级为 `manual_every_time` |

**关键约束**：

- `auto_within_profile` 审批模式**必须**要求 OS 级 enforcement 全维度覆盖，否则自动执行等于裸执行
- 只有 `break_glass_only` 和 `full_trust_no_sandbox` 允许 `enforcement_backend = "none"`
- `manual_every_time` 模式下允许 `enforcement_degraded`，但审计必须明确标记
- `full_trust_no_sandbox` 模式完全跳过 enforcement 检查和沙箱构建，直接以当前用户权限执行。此模式需要设备 owner 显式开启 `allow_full_trust_mode`，审计中标记 `enforcement_status = "user_opted_out"`

#### 4.7 资源限制补充

OS 级沙箱主要解决文件系统/网络/进程隔离，**资源限制**需要额外机制：

| 平台 | 资源限制方案 | 控制面 |
| --- | --- | --- |
| Linux | cgroups v2 (通过 systemd-run 或直接写 cgroup 文件) | 内存上限、CPU 配额、最大 PID 数 |
| macOS | `setrlimit(2)` (RLIMIT_AS, RLIMIT_CPU, RLIMIT_NPROC) | 内存上限、CPU 时间、最大进程数（粗粒度） |
| Windows | Job Objects (`JOBOBJECT_EXTENDED_LIMIT_INFORMATION`) | 内存上限、CPU 时间、最大进程数 |

**当前版本资源限制策略**：

- `max_runtime_ms`：应用层 watchdog（所有平台统一）+ OS 级 SIGKILL 兜底
- `max_child_processes`：Linux cgroup `pids.max`、macOS `RLIMIT_NPROC`、Windows Job `ActiveProcessLimit`
- 内存限制：当前版本按平台能力提供约束；若某平台只能部分 enforce，必须在审计中明确标记降级维度

### 5. 当前版本实施要求

#### 5.1 当前版本范围（必须）

| 平台 | Enforcement Backend | 覆盖维度 |
| --- | --- | --- |
| Linux | bubblewrap + seccomp | 文件系统隔离 ✅、网络隔离 ✅、提权阻止 ✅、PID 隔离 ✅ |
| macOS | sandbox-exec (Seatbelt) | 文件系统隔离 ✅、网络隔离 ✅、提权阻止 ✅ |
| Windows | Restricted Token + Job Objects | 文件系统保护 ✅、资源限制 ✅、进程组回收 ✅、配合 ConPTY 提供交互控制 ✅ |

#### 5.2 当前版本降级路径

| 平台 | 降级条件 | 降级后端 | 降级代价 |
| --- | --- | --- | --- |
| Linux | bwrap 不可用但 Landlock v1+ 可用 | Landlock + seccomp | 无 PID 隔离；网络控制仅 TCP 端口级（v4+）或无（v1-v3） |
| Linux | Landlock 也不可用 | `none` | 拒绝非 break-glass 命令 |
| macOS | sandbox-exec 不可用 | `none` | 拒绝非 break-glass 命令 |
| Windows | Restricted Token / Job Objects 任一关键能力不可用 | `none` | 拒绝非 break-glass 命令 |

#### 5.3 当前版本必须交付的代码模块

| 模块 | 职责 |
| --- | --- |
| `bifrost-sandbox` (新 crate 或模块) | 平台 enforcement backend 抽象、能力检测、参数构建 |
| `sandbox::linux::bwrap` | bubblewrap 命令行构建、进程启动 |
| `sandbox::linux::seccomp` | seccomp-bpf 过滤器构建与加载 |
| `sandbox::linux::landlock` | Landlock 规则构建（降级路径） |
| `sandbox::macos::seatbelt` | SBPL 策略生成、sandbox-exec 调用 |
| `sandbox::detect` | 启动时 enforcement backend 能力检测 |
| `executor` 改造 | 将子进程启动从直接 `Command::new()` 改为通过 sandbox wrapper 启动 |

#### 5.4 Rust 依赖选型与架构分层

**选型原则**：底层原语使用经过生产验证的成熟 crate；跨平台产品层自研，确保 SandboxProfile 映射逻辑完全可控。

##### 5.4.1 Rust 沙箱生态全景评估

**第一层：底层原语（Production-Ready，直接依赖）**

| Crate | 版本 | 维护方 | 能力 | 生产验证 | 评级 |
| --- | --- | --- | --- | --- | --- |
| `landlock` | v0.4.4 (9 releases) | Landlock LSM 官方 (`landlock-lsm` org) | Linux 文件系统路径级访问控制 (5.13+)、TCP 网络端口控制 (6.7+) | 内核子系统作者维护，builder pattern + CompatLevel 兼容性协商 | ⭐⭐⭐ |
| `seccompiler` | v0.4.0 | AWS Firecracker (`rust-vmm` org) | seccomp-bpf 系统调用过滤，JSON → BPF 编译，支持 x86_64 + aarch64 | Firecracker 大规模生产环境 | ⭐⭐⭐ |

**第二层：中间组合库（参考借鉴，不直接依赖）**

| Crate | 版本 | 能力 | 不采用原因 |
| --- | --- | --- | --- |
| `extrasafe` | v0.5.1 | seccompiler + landlock + user namespace 的 builder 封装 | 仅 Linux x86_64；抽象层次与我们需求不完全匹配 |
| `sandlock-core` | v0.6.0 | Landlock + seccomp + COW 文件系统 + HTTP ACL | 要求 Linux 6.12+，不适合广泛部署 |
| `hakoniwa` | v1.5.0 | namespaces + cgroups + landlock + seccomp | 偏容器化方向，抽象过重 |

**第三层：跨平台高层库（不适合安全基础设施）**

| Crate | 版本 | 能力 | 不采用原因 |
| --- | --- | --- | --- |
| `ai-sandbox` | v0.2.0 | Linux (bwrap+seccomp+Landlock) + macOS (Seatbelt) + Windows (Restricted Token) + FreeBSD (Capsicum) | 2026.3 刚创建，单个维护者，5.6K SLoC，未经安全审计 |
| `sandbox-runtime` | v0.1.1 | 跨平台: bwrap+seccomp (Linux) + Seatbelt (macOS) + 代理网络过滤 | v0.1.1 极早期 |
| `build-wrap` | v0.6.0 | 跨平台: bwrap (Linux) + sandbox-exec (macOS) | 设计目标是保护 build scripts，非通用沙箱 |

##### 5.4.2 确定的依赖方案

```toml
# Cargo.toml (bifrost-sandbox crate)

[target.'cfg(target_os = "linux")'.dependencies]
landlock = "0.4"           # 官方 Landlock LSM Rust 绑定（降级路径 + 叠加防护）
seccompiler = "0.4"        # Firecracker 出品的 seccomp-bpf 过滤器

# macOS: 不需要额外 crate
# - sandbox-exec 通过 std::process::Command spawn 调用
# - SBPL 策略通过字符串模板动态生成
# - bubblewrap 同理，通过 Command spawn 调用系统 bwrap 二进制
```

##### 5.4.3 架构分层

```text
┌──────────────────────────────────────────────────────┐
│              Bifrost Enforcement Layer                │  ← 自研
│   EnforcementBackend trait + SandboxProfile 映射     │
│   能力检测 + 审计日志 + 降级决策                      │
├──────────────────┬───────────────────────────────────┤
│     Linux        │           macOS                   │
├──────────────────┼───────────────────────────────────┤
│  bwrap (spawn)   │  sandbox-exec (spawn + SBPL生成)  │  ← 外部进程
│  ┌────────────┐  │                                   │
│  │ landlock   │  │  (macOS 无需 in-process 内核沙箱  │  ← Rust crate
│  │ crate      │  │   Seatbelt 已涵盖)               │
│  └────────────┘  │                                   │
│  ┌────────────┐  │                                   │
│  │seccompiler │  │                                   │  ← Rust crate
│  │ crate      │  │                                   │
│  └────────────┘  │                                   │
└──────────────────┴───────────────────────────────────┘
```

**自研产品层职责**：

| 模块 | 职责 | 复杂度 |
| --- | --- | --- |
| `EnforcementBackend` trait | 统一抽象 Linux / macOS 差异，定义 `detect()` → `prepare()` → `spawn()` 生命周期 | 低 |
| `SandboxProfile → BwrapConfig` | 将声明式策略翻译为 bwrap 命令行参数（`--ro-bind` / `--unshare-net` / `--seccomp` 等） | 中 |
| `SandboxProfile → SBPL` | 动态生成 macOS Seatbelt 策略文本（模板 + 参数替换） | 中 |
| seccomp filter 生成 | 基于 `seccompiler` 组装网络/syscall BPF 过滤规则 | 中 |
| Landlock fallback | bwrap 不可用时通过 `landlock` crate 做 in-process 文件系统限制 | 低 |
| 能力检测 | 运行时检测 bwrap / sandbox-exec / 内核版本 / Landlock ABI 可用性 | 低 |
| 审计与可观测性 | enforcement 结果记录（backend / capabilities / status / degraded_dimensions） | 低 |

**不采用跨平台高层库的理由**：

1. **安全基础设施不能依赖未经审计的单人项目** — `ai-sandbox` 等库创建仅数月，维护者单一
2. **SandboxProfile → OS 映射是核心业务逻辑** — 必须完全掌控，不能交给第三方抽象
3. **底层原语已足够成熟** — `landlock` + `seccompiler` 都是经过生产验证的官方/准官方库
4. **外部进程调用 bwrap / sandbox-exec 最简单可靠** — 不需要在 Rust 中重实现 namespace 管理
5. **跨平台差异在产品层而非库层** — Linux 和 macOS 的沙箱机制本质不同，统一抽象的价值在业务映射层

#### 5.5 增强方向

- Windows AppContainer 完整沙箱
- Linux cgroups v2 资源限制集成
- Sandbox Expansion 动态权限扩展（参考 Gemini CLI）
- enforcement 运行时健康检查（验证沙箱未被绕过）

### 6. 审计与可观测性

每次沙箱执行必须在审计记录中包含以下 enforcement 相关字段：

| 审计字段 | 说明 |
| --- | --- |
| `enforcement_backend` | 实际使用的后端标识 |
| `enforcement_capabilities` | 后端提供的能力集 |
| `enforcement_status` | `active` / `degraded` / `unavailable` |
| `sandbox_config_digest` | enforcement 参数的 SHA256 摘要（可审计沙箱配置是否与 profile 一致） |
| `degraded_dimensions` | 如有降级，列出降级的维度和原因 |
| `enforcement_setup_ms` | 沙箱构建耗时（性能监控） |

### 7. 已确认的 Enforcement Backend 决策

1. **当前版本同时落地 Linux (bubblewrap+seccomp)、macOS (Seatbelt) 与 Windows (Restricted Token + Job Objects + ConPTY) 的 OS 级 enforcement / 交互能力**
2. **`auto_within_profile` 审批模式必须要求 enforcement 全维度覆盖，否则降级为 `manual_every_time`**
3. **enforcement 不可用时拒绝执行（除 `break_glass_only` profile 外），不允许降级为纯应用层检查**
4. **Linux 降级路径：bubblewrap → Landlock+seccomp → none（拒绝）**
5. **macOS 降级路径：Seatbelt → none（拒绝）**
6. **Windows enforcement 与交互能力纳入当前版本交付，不接受“Windows 仅支持 break-glass 或 manual_every_time”的阶段性裁剪**
7. **审计记录必须包含 enforcement backend 信息，确保每次执行的沙箱状态可追溯**
8. **文件系统隔离和网络隔离必须同时存在，不允许单独只有一个维度的隔离（参考 Claude Code 设计原则）**
9. **Rust 依赖选型：底层原语仅依赖 `landlock` (v0.4, 官方) 和 `seccompiler` (v0.4, Firecracker)，bubblewrap 和 sandbox-exec 通过 `std::process::Command` spawn 外部进程调用**
10. **不采用跨平台高层沙箱库（`ai-sandbox`、`sandbox-runtime` 等）— 安全基础设施不依赖未经审计的单人/早期项目，SandboxProfile → OS 映射作为核心业务逻辑自研**

## Caller-Client E2E 加密层

### 1. 问题定义

**当前 Relay 对 caller 与 client 之间传输的所有数据具有完全的明文可见性。** 具体暴露路径：

| 暴露点 | 数据内容 | 当前状态 |
| --- | --- | --- |
| openCall 请求 | 完整命令文本、参数、环境变量、cwd | 明文 JSON，Relay 存储 `command_json` |
| call_open SSE 推送 | 完整 command 对象透传给 Client | 明文 JSON |
| frame（Client→Caller） | stdout/stderr 输出 | `EncryptedEnvelope` 外壳但 nonce/tag 为空，ciphertext 字段放明文 |
| frame（Caller→Client） | stdin 输入 | 同上 |
| exit 事件 | exit_code、stderr、duration、digest | 明文 JSON，Relay 存储并推送 |
| event_summary | 命令摘要、exit_code、grant_id | 明文写入 Relay 事件日志 |

这意味着：
- Relay 服务器被入侵时，攻击者可获取所有正在执行和历史执行的完整命令与输出
- Relay 运维人员（或底层基础设施）可以监听所有远程执行内容
- "不存储"只是行政约束，不是技术保障——代码层任何 bug 或日志配置都可能意外记录明文

**设计目标：Relay 对 caller-client 之间传输的业务数据（命令、参数、输入、输出、执行结果）实现密码学级别的零知识。不需要兼容当前明文传输的历史版本。**

### 2. 密钥交换协议

采用 X25519 ECDH + HKDF 协商会话密钥，利用已有的 SSH Ed25519 密钥对提供身份绑定。

#### 2.1 密钥层次

```
Device SSH Key (Ed25519, 长期)
  └─ 身份认证 + 签名
  
Caller Ephemeral Key (X25519, per-grant)
  └─ caller_ephemeral_pub 已在 GrantDecisionRequest 中预留

Client Ephemeral Key (X25519, per-grant)
  └─ client_ephemeral_pub 已在 GrantDecisionRequest 中预留

Session Key (256-bit, per-call)
  └─ HKDF-SHA256(shared_secret, call_id || caller_pub || client_pub)
  └─ 用于 AEAD 加密所有 call 数据
```

#### 2.2 密钥交换时机：Grant 创建阶段

密钥交换在 grant 建立时完成，而非每次 openCall：

1. **Caller 发起配对请求**时，生成一次性 X25519 密钥对，将 `caller_ephemeral_pub` 随 pairing request 发送
2. **Client 收到 pairing_request**后，生成对应的 X25519 密钥对，将 `client_ephemeral_pub` 随 grant decision（approve）返回
3. **Relay 转发**：Relay 原样传递双方的 ephemeral 公钥，但无法从公钥推导出 shared secret
4. **双方各自计算 shared secret**：`shared_secret = X25519(my_ephemeral_priv, peer_ephemeral_pub)`
5. **Caller 本地持久化** `shared_secret`（与 grant 绑定存储，使用本地 AES-256-GCM 加密保存）
6. **Client 本地持久化** `shared_secret`（与 grant 绑定存储，使用本地加密保存）

对于 SSH 直连方式创建的 grant，密钥交换同样在 SSH connect result 阶段完成。

#### 2.3 Per-Call Session Key 派生

每次 openCall 不重新做密钥交换（避免额外往返），而是从 grant 的 shared_secret 派生 per-call 会话密钥：

```
session_key = HKDF-SHA256(
    ikm = shared_secret,
    salt = call_id,                          // 确保每个 call 的密钥不同
    info = "bifrost-e2e-v1" || caller_pub || client_pub
)
```

这确保：
- 每个 call 使用不同的对称密钥
- 即使某个 call 的 session_key 泄露，不影响其他 call
- call_id 由 Relay 生成（不受 caller/client 控制），防止 caller 重放旧 call 的密文

#### 2.4 密钥交换失败处理

- 如果 `caller_ephemeral_pub` 或 `client_ephemeral_pub` 缺失，grant 拒绝创建
- 如果 ECDH 计算失败（非法公钥），grant 拒绝创建
- 如果 shared_secret 本地存储丢失（如数据库损坏），该 grant 作废，需重新配对

### 3. 帧加密方案

#### 3.1 AEAD 算法选择

使用 **ChaCha20-Poly1305**（RFC 8439）：
- 比 AES-GCM 在无硬件加速的平台上性能更好
- 不存在 AES-GCM 的 nonce 复用灾难性安全后果（ChaCha20-Poly1305 的 nonce misuse 更温和）
- Rust 生态成熟：`ring` 和 `chacha20poly1305` crate 均有生产级实现
- 与 SSH 协议栈已选用的密码学生态一致

#### 3.2 加密信封格式

```rust
struct EncryptedEnvelope {
    version: u32,          // 协议版本，当前固定为 2（v1 = 遗留明文格式，已废弃）
    call_id: String,       // 明文，用于 Relay 路由
    seq: u64,              // 明文，用于 Relay 和接收端排序、去重
    direction: FrameDirection,  // 明文，用于 Relay 区分转发方向
    nonce: [u8; 12],       // 96-bit nonce (big-endian seq || random_prefix)
    ciphertext: Vec<u8>,   // ChaCha20-Poly1305 加密后的密文
    tag: [u8; 16],         // Poly1305 认证标签
    aad_json: String,      // 附加认证数据（明文，但被 tag 保护完整性）
}
```

**AAD（Additional Authenticated Data）**：

```json
{
    "version": 2,
    "call_id": "...",
    "seq": 42,
    "direction": "client_to_caller",
    "frame_type": "stdout"
}
```

AAD 是明文但受认证标签保护——Relay 可以看到 frame_type（用于流控），但无法篡改。

#### 3.3 Nonce 构造

采用 `seq-based nonce` 避免随机 nonce 碰撞：

```
nonce[0..4]  = random_prefix (per-call 随机，在 call 建立时由发送方生成)
nonce[4..12] = big_endian(seq)
```

- `seq` 单调递增，保证同一 call 内 nonce 不重复
- `random_prefix` 防止跨 call 的 nonce 碰撞（不同 call 使用不同 session_key，但双重保险）
- 发送方（caller/client）各自维护独立的 seq 计数器

#### 3.4 加密覆盖范围

| 数据 | 是否加密 | 说明 |
| --- | --- | --- |
| frame payload（stdout/stderr/stdin/control/status/artifact） | ✅ 加密 | 密文封装在 `ciphertext` |
| frame_type | 明文但受 AAD 保护 | Relay 需要做类型级流控 |
| call_id / seq / direction | 明文但受 AAD 保护 | Relay 需要路由和排序 |
| openCall command 对象 | ✅ 加密 | 密文字段，Relay 不可见 |
| exit 事件（exit_code、termination_reason 等） | ✅ 加密 | 密文封装在 exit frame 中 |
| grant_id / caller_fingerprint | 明文 | Relay 路由必需 |
| call_id / device_code / 状态 | 明文 | Relay 生命周期管理必需 |

### 4. openCall 改造

openCall 是当前最严重的暴露点——命令全文明文传输并存储在 Relay。改造后：

#### 4.1 Caller 侧

```json
{
    "grant_id": "uuid",
    "caller_fingerprint": "fp",
    "command_encrypted": {
        "version": 2,
        "nonce": "base64...",
        "ciphertext": "base64...",
        "tag": "base64..."
    },
    "command_kind": "shell.exec",
    "pty_enabled": false,
    "timeout_hint_ms": 600000
}
```

- `command_encrypted`：完整 command 对象的 AEAD 加密密文，只有 Client 能解密
- `command_kind`：明文，Relay 用于路由决策和 rate limit
- `pty_enabled` / `timeout_hint_ms`：明文元数据，Relay 用于连接管理（TTL、帧模式）
- Relay **不再存储** `command_json` 和 `command_summary_json`

#### 4.2 Relay 侧

Relay 收到 openCall 后：
1. 验证 grant 有效性（不变）
2. 创建 call 记录，仅存储 `command_kind`、`pty_enabled` 等路由级明文字段
3. 将 `command_encrypted` **原样不透明转发**给 Client（通过 `call_open` SSE 事件）
4. **不解析、不存储、不记录** `command_encrypted` 的内容

#### 4.3 Client 侧

Client 收到 `call_open` 事件后：
1. 从本地查找该 grant 对应的 `shared_secret`
2. 用 `session_key = HKDF(shared_secret, call_id, ...)` 派生会话密钥
3. 解密 `command_encrypted` 得到完整 command 对象
4. 执行后续策略校验、沙箱准备、命令执行

### 5. Exit 事件改造

exit 事件同样改为加密传输：

#### 5.1 Client 发送加密 exit

```json
{
    "call_id": "uuid",
    "client_instance_id": "...",
    "exit_encrypted": {
        "version": 2,
        "nonce": "base64...",
        "ciphertext": "base64...",
        "tag": "base64..."
    }
}
```

`exit_encrypted` 密文内包含：
- `exit_code`
- `termination_reason`
- `duration_ms`
- `stdout_digest` / `stderr_digest`
- `enforcement_backend` / `enforcement_status`
- `stderr`（如果有）

#### 5.2 Relay 侧

Relay 收到 exit 后：
1. 将 call 状态标记为 `completed`
2. 将 `exit_encrypted` **原样转发**给 Caller（通过 SSE `exit` 事件）
3. **不解析** exit_code 等字段——Relay 只知道"这个 call 结束了"，不知道成功还是失败

#### 5.3 Caller 侧

Caller 解密 `exit_encrypted` 得到完整退出信息，按需展示和记录。

### 6. 审批事件的加密边界

审批流程（pending_approval）涉及设备 owner 通过 WebUI 审批命令。审批弹窗需要展示命令预览，但这个展示发生在 **Client 本地 WebUI**，不经过 Relay：

- Client 收到 `call_open` 事件后解密命令
- Client 在本地 WebUI 展示审批弹窗（命令预览、策略信息等）
- 设备 owner 在本地做出审批决策
- Client 将审批结果（approve/reject，不含命令内容）上报 Relay
- Relay 仅转发审批结果状态

### 7. Relay 可见的最小元数据集

E2E 加密后，Relay 能看到的信息严格限制为：

| 信息 | 用途 | 可否进一步缩减 |
| --- | --- | --- |
| caller_fingerprint | 身份验证 | 否，路由必需 |
| device_code / client_instance_id | 设备路由 | 否，路由必需 |
| grant_id | 权限校验 | 否，校验必需 |
| call_id | 会话标识 | 否，路由必需 |
| command_kind (`query.readonly` / `shell.exec`) | TTL / rate limit | 可考虑混淆但收益低 |
| pty_enabled | 帧模式 | 可考虑混淆但收益低 |
| frame seq / direction / frame_type | 排序、转发、流控 | frame_type 可考虑加密但影响流控 |
| call 状态（running / completed / cancelled） | 生命周期管理 | 否，连接管理必需 |
| 密文长度 | 流量分析侧信道 | 可选 padding 缓解 |

### 8. 密码学库选型

| 用途 | 推荐库 | 说明 |
| --- | --- | --- |
| X25519 ECDH | `ring` 或 `x25519-dalek` | ring 已在项目中使用（SSH 密钥加密） |
| HKDF-SHA256 | `ring::hkdf` | ring 内置 |
| ChaCha20-Poly1305 | `ring::aead` | ring 内置，统一依赖 |
| 随机数生成 | `ring::rand` | ring 内置 |
| Base64 编码 | `base64` | 密文/nonce 的序列化 |

统一使用 `ring` 库，与项目现有 SSH 密钥管理（`ssh_keys.rs` 中已使用 `ring::aead::AES_256_GCM`）保持一致。

### 9. 安全性分析

| 威胁 | 缓解 |
| --- | --- |
| Relay 服务器被入侵 | 攻击者只能看到密文和路由元数据，无法解密任何业务数据 |
| Relay 运维人员监听 | 同上，密码学保障而非行政约束 |
| 重放攻击 | per-call session_key + seq-based nonce 确保每个帧唯一；AAD 绑定 call_id 防止跨 call 重放 |
| 中间人篡改 | AEAD 的 Poly1305 tag 检测任何篡改（含 AAD 篡改） |
| 前向保密 | 使用一次性 ephemeral 密钥对，grant 过期后删除 ephemeral 私钥 |
| nonce 复用 | seq 单调递增 + random_prefix 双重保障 |
| 密文长度侧信道 | 可选 padding 到固定块大小（如 256 字节对齐） |
| 密钥泄露（单个 call） | per-call session_key 隔离，不影响其他 call |

### 10. 不兼容历史版本

**此改造为破坏性变更，不做向后兼容**：
- `EncryptedEnvelope.version` 固定为 `2`
- 移除 version=1 的明文模式支持
- 旧版 Caller/Client 无法与新版 Relay 交互，需同步升级
- Relay 不再存储 `command_json`、`command_summary_json`、`exit_code`、`stdout_digest`、`stderr_digest` 等业务字段
- `RemoteInvokeCall` 数据模型移除所有业务字段，仅保留路由级字段

## 安全风险与缓解

### 风险 1：任意 shell 导致能力过大

缓解：

- 当前版本开放 `shell_text`
- 高风险策略允许 `pair_code` 与 `ssh_publickey`，但默认每次审批
- shell grant 与 query grant 分离

### 风险 2：命令注入

缓解：

- `template` / `argv_exec` 默认不走 shell
- 变量必须类型校验
- `shell_text` 只在显式规则下允许

### 风险 3：白名单变更后旧 grant 继续可用

缓解：

- 绑定 `shell_policy_set_version`
- 白名单更新后使旧 shell grant 失效

### 风险 4：环境变量泄露

缓解：

- 不默认继承全环境
- env key allowlist
- 审计只记 key 不记 value

### 风险 4.1：沙箱限制停留在应用层，无 OS 级强制执行

缓解：

- **全维度 OS 级 enforcement 已纳入当前版本必交付范围**（详见"OS 级安全沙箱 Enforcement Backend"章节）
- Linux 采用 bubblewrap (mount/network/PID namespace) + seccomp-bpf 双层隔离
- macOS 采用 sandbox-exec (Seatbelt SBPL) 内核级沙箱
- 文件系统隔离和网络隔离必须同时存在（参考 Claude Code 设计原则）
- enforcement 不可用时拒绝执行，不允许降级为纯应用层检查
- `auto_within_profile` 审批模式必须要求 enforcement 全维度覆盖，否则强制降级为 `manual_every_time`
- 执行前必须把 enforcement 状态解析并落入审计记录（`enforcement_backend` + `enforcement_status` + `enforcement_capabilities`）

### 风险 4.2：secret 通过 preview / 输出 / 审计泄露

缓解：

- Relay preview 统一脱敏
- secret 只允许本地 store 注入
- caller 不得直接上传 secret 明文
- 审计按标识符记录，不记明文

### 风险 5：输出过大或长时间挂起

缓解：

- 输出大小限制
- timeout
- cancel
- 每策略并发上限
- worker heartbeat + caller resume，避免“无输出但其实还活着”的误判

### 风险 6：后台进程脱离会话

缓解：

- 当前版本不支持 detach
- call cancel 或 caller 退出时回收进程组

### 风险 7：pair code 获得过强权限

缓解：

- pair code 可按用户配置拿一次性、短时或长期 shell grant
- 高风险策略不限制必须为 SSH，但仍要求命中显式 policy / scope / binding 并通过审批

## 当前版本落地范围

当前版本按以下边界落地：

1. 同时开放 `template` / `argv_exec` / `shell_text`
2. `pair_code` 与 `ssh_publickey` 都允许申请 shell grant，且都可长期复用，具体由用户配置决定
3. `template/argv_exec` 按策略自动执行或首次审批
4. `shell_text` 每次审批
5. 默认 `network=off`，按 policy 单独放开
6. 默认命令边界是 `argv_only`，禁止 shell operator、heredoc、command substitution；只有命中允许 `shell_text` 的 policy / scope 时才放开对应能力
7. Client 本地保存完整 `stdout/stderr` 和审计记录，Relay 不保留任何执行内容
8. Relay 对 caller-client 业务数据保持零知识，不展示也不持久化命令预览、命中策略、作用范围或执行约束摘要
9. Unix PTY 与 Windows ConPTY 都纳入当前版本
10. Client 侧直接落完整控制模型：
    - `Policy + Scope + Binding + Override + break-glass`
11. OS 级 enforcement backend 必须在当前版本落地：
    - Linux: bubblewrap + seccomp（降级：Landlock + seccomp）
    - macOS: sandbox-exec (Seatbelt)
    - Windows: Restricted Token + Job Objects
    - 文件系统隔离 + 网络隔离缺一不可
    - enforcement 不可用时拒绝非 break-glass 命令

这意味着当前版本覆盖：

- 自动化部署
- 远程诊断
- 服务重启
- 仓库同步构建
- 高风险受控 shell 文本执行
- Unix / Windows 平台下的交互式会话

## 测试方案

### 单元测试

- `shell_policy_template_validate_vars`
- `shell_policy_argv_exec_validate_args`
- `shell_policy_shell_text_regex_match`
- `shell_policy_platform_variants_resolve_current_platform`
- `shell_binding_resolves_subject_priority`
- `shell_scope_deny_overrides_allow`
- `shell_scope_intersection_builds_effective_constraints`
- `shell_scope_time_window_rejects_outside_window`
- `shell_scope_quota_rejects_excess_calls`
- `shell_sandbox_profile_filesystem_scope_denies_out_of_root_access`
- `shell_sandbox_profile_network_scope_denies_non_allowlisted_domain`
- `shell_sandbox_profile_command_scope_denies_shell_text_when_argv_only`
- `shell_sandbox_profile_process_scope_denies_detach_or_privilege_escalation`
- `shell_error_code_scope_default_deny_is_explicit`
- `shell_error_code_cwd_not_allowed_is_explicit`
- `shell_error_code_caller_not_bound_is_explicit`
- `shell_grant_scope_reject_query_grant`
- `shell_grant_shell_policy_set_version_mismatch_rejected`
- `shell_executor_env_allowlist_filters_values`
- `shell_executor_timeout_kills_process_group`
- `shell_executor_output_truncation_marks_result`
- `shell_executor_stdout_and_stderr_are_captured_independently`
- `shell_executor_merged_view_orders_frames_by_seq`
- `shell_executor_binary_output_sets_is_binary_and_artifact_metadata`
- `shell_executor_non_utf8_output_preserves_raw_bytes_and_encoding_hint`
- `shell_executor_windows_path_normalization`
- `shell_executor_windows_env_case_insensitive_allowlist`
- `shell_executor_platform_signal_strategy_selects_correct_backend`
- `shell_cancel_kills_process_group_not_just_stream`
- `shell_cancel_revokes_resume_token`
- `sandbox_enforcement_detect_bubblewrap_available`
- `sandbox_enforcement_detect_seatbelt_available`
- `sandbox_enforcement_detect_landlock_version`
- `sandbox_enforcement_bwrap_config_maps_filesystem_scope`
- `sandbox_enforcement_bwrap_config_maps_network_scope_off_to_unshare_net`
- `sandbox_enforcement_bwrap_config_maps_deny_roots_to_dev_null_bind`
- `sandbox_enforcement_seatbelt_generates_valid_sbpl_for_readonly_profile`
- `sandbox_enforcement_seatbelt_generates_valid_sbpl_for_workspace_write_profile`
- `sandbox_enforcement_seatbelt_generates_network_deny_for_mode_off`
- `sandbox_enforcement_seatbelt_generates_network_allowlist_for_mode_allowlist`
- `sandbox_enforcement_seccomp_blocks_socket_creation_when_network_off`
- `sandbox_enforcement_seccomp_sets_no_new_privs`
- `sandbox_enforcement_rejects_auto_profile_without_full_capabilities`
- `sandbox_enforcement_allows_break_glass_without_enforcement`
- `sandbox_enforcement_degraded_landlock_fallback_on_no_bwrap`
- `sandbox_enforcement_rejects_when_no_backend_available`

### E2E 测试

- `test_remote_shell_exec_template_e2e.sh`
  - SSH connect 后执行模板命令成功
- `test_remote_shell_exec_pair_code_persistent_e2e.sh`
  - pair code 可按配置获得长期 shell grant
- `test_remote_shell_exec_policy_reject_e2e.sh`
  - 非白名单命令被拒绝
- `test_remote_shell_exec_cancel_e2e.sh`
  - 长命令可被 caller cancel，且 client 侧目标进程被真正终止
- `test_remote_shell_exec_cancel_no_resume_e2e.sh`
  - cancel 后不能再通过 `resume` 重新附着
- `test_remote_shell_exec_shell_policy_set_version_invalidate_e2e.sh`
  - 全局 shell 策略版本更新后旧 grant 失效
- `test_remote_shell_exec_scope_binding_e2e.sh`
  - caller 只能命中绑定到自身的 policy/tag
- `test_remote_shell_exec_deny_override_e2e.sh`
  - 显式 deny scope 能压过 allow
- `test_remote_shell_exec_cwd_scope_e2e.sh`
  - cwd 超出白名单范围时拒绝执行
- `test_remote_shell_exec_env_scope_e2e.sh`
  - 未授权 env key/value 被拒绝
- `test_remote_shell_exec_sandbox_profile_fs_e2e.sh`
  - 超出文件系统根范围的命令被拒绝
- `test_remote_shell_exec_sandbox_profile_network_e2e.sh`
  - 非 allowlist 域名或方法被拒绝
- `test_remote_shell_exec_sandbox_profile_command_e2e.sh`
  - `argv_only` profile 下 shell_text / shell operator 被拒绝
- `test_remote_shell_exec_error_feedback_e2e.sh`
  - caller 能看到明确错误码和人类可读错误提示
- `test_remote_shell_exec_streaming_e2e.sh`
  - stdout/stderr 分片流式返回
- `test_remote_shell_exec_stdout_stderr_split_e2e.sh`
  - stdout 和 stderr 可独立查看，merged 视图按 seq 合流
- `test_remote_shell_exec_stderr_success_e2e.sh`
  - stderr 非空但 exit_code=0 时不被误判为失败
- `test_remote_shell_exec_binary_output_e2e.sh`
  - 二进制输出被正确标记，必要时落为 artifact
- `test_remote_shell_exec_non_utf8_output_e2e.sh`
  - 非 UTF-8 / Windows 编码输出可按 hint 正确展示
- `test_remote_shell_exec_long_running_resume_e2e.sh`
  - 长任务执行中 caller 断线后可续连并补拉输出
- `test_remote_shell_exec_heartbeat_e2e.sh`
  - 长任务无输出阶段仍持续上报 heartbeat
- `test_remote_shell_exec_logs_after_finish_e2e.sh`
  - 长任务完成后仍可通过 call_id 查看保留输出
- `test_remote_shell_exec_unix_pty_e2e.sh`
  - macOS / Linux 下可创建 PTY 会话，输出为 merged 流
  - 支持 resize control frame
  - 支持 Ctrl-C / signal 中断
  - cancel 后 PTY 进程组被真正终止
- `test_remote_shell_exec_windows_conpty_e2e.ps1`
  - Windows 下可创建 ConPTY 会话，输出为 merged 流
  - 支持 resize control frame
  - 支持 Ctrl-C / cancel 中断
  - cancel 后 Job Object 内进程被真正终止
- `test_sandbox_enforcement_linux_bwrap_fs_isolation_e2e.sh`
  - bubblewrap 沙箱下命令无法写入 deny_roots 路径
  - bubblewrap 沙箱下命令可正常写入 write_roots 路径
- `test_sandbox_enforcement_linux_bwrap_network_isolation_e2e.sh`
  - network_scope.mode=off 时 bubblewrap 沙箱下命令无法发起 TCP 连接
  - network_scope.mode=allowlist 时仅允许连接白名单端点
- `test_sandbox_enforcement_macos_seatbelt_fs_isolation_e2e.sh`
  - Seatbelt 沙箱下命令无法写入 deny_roots 路径
  - Seatbelt 沙箱下命令可正常写入 write_roots 路径
- `test_sandbox_enforcement_macos_seatbelt_network_isolation_e2e.sh`
  - network_scope.mode=off 时 Seatbelt 沙箱下命令无法发起网络连接
- `test_sandbox_enforcement_unavailable_rejects_execution_e2e.sh`
  - enforcement backend 不可用时非 break-glass 命令被拒绝
- `test_sandbox_enforcement_auto_profile_requires_full_enforcement_e2e.sh`
  - auto_within_profile 模式下缺少 enforcement 时自动降级为 manual_every_time

三平台覆盖要求：

- macOS：
  - 覆盖 `argv_exec`、`template`、长任务 resume、取消
- Linux：
  - 覆盖 `argv_exec`、`template`、长任务 resume、取消
- Windows：
  - 覆盖 `argv_exec`、`template`、PowerShell 变体、长任务 heartbeat / cancel

说明：

- Windows 不应复用 Unix shell 脚本作为唯一 E2E 入口
- Windows 应有独立的验证入口，例如 PowerShell 测试脚本或 Rust 集成测试驱动
- Unix PTY 与 Windows ConPTY 都属于当前版本必测范围

### Human Tests

实现时必须新增并执行：

- 第一阶段（本轮 relay 新协议与通信加密）先复用并增量更新 `human_tests/remote-invoke.md`
- 第二阶段（Client shell 执行、policy、sandbox、PTY）再新增 `human_tests/remote-shell-exec.md`

建议至少覆盖：

- 设备上创建 shell policy
- SSH key 连接后查看 policy 列表
- 执行模板命令成功
- 非白名单命令被拒绝
- 同一个 caller 只能看到和使用自己绑定的 policy 范围
- 显式 deny 规则能覆盖 allow 规则
- cwd / env / quota / time window 等范围限制实际生效
- sandbox profile 的文件系统 / 网络 / 命令范围限制实际生效
- `argv_only` 与 `network=off` 这类行业常见沙箱预设可稳定阻断越界执行
- 执行失败时能明确看到失败原因，而不是只有笼统的 rejected
- `不在白名单范围`、`cwd 不允许`、`caller 未绑定` 等错误能被区分
- stdout / stderr 可以分别查看，也可以按时间合流查看
- stderr 有内容但命令成功时，界面和 CLI 不误判为失败
- 非 UTF-8 / Windows 编码输出显示正常，digest 不因展示层换行转换而变化
- 大体积输出会落盘并支持续连补拉，不会因为 caller 跟不上而阻塞目标命令
- 二进制输出不会把 CLI 弄乱码，必要时能以 artifact 形式查看
- pair code 可按配置执行一次性、短时或长期 shell grant
- shell grant 在策略更新后失效
- 长时间命令超时与取消
- Unix PTY 会话可正常显示合流输出、支持 resize，并能通过 Ctrl-C 中断
- caller 执行 cancel 后，目标设备上的命令进程确实被杀掉，而不是继续后台运行
- cancel 后 call 不能 resume，状态最终为 `cancelled`
- 输出截断与审计摘要正确
- 长任务执行中关闭 caller 后重新续连成功
- 长任务长时间无输出但状态仍显示 running
- 同一 `policy_id` 在 macOS / Linux / Windows 上命中正确的平台变体
- Windows 目标设备上 PowerShell 输出、路径和取消语义正常
- **OS 级 enforcement 验证**：
  - Linux 上 `network_scope.mode=off` 的命令确实无法发起网络连接（非仅配置层拦截，而是子进程级物理隔离）
  - Linux 上 `deny_roots` 中的路径确实不可写入（非仅 pre-check，而是 mount namespace 隔离）
  - macOS 上 `network_scope.mode=off` 的命令确实无法发起网络连接（Seatbelt 内核级拦截）
  - macOS 上 `deny_roots` 中的路径确实不可写入（Seatbelt SBPL 文件系统策略）
  - enforcement backend 不可用时非 break-glass 命令被拒绝，且错误信息明确标识 `sandbox_enforcement_unavailable`
- `auto_within_profile` 模式在缺少 enforcement 时自动降级为 `manual_every_time`
- 审计记录中包含 `enforcement_backend`、`enforcement_status` 字段

## 第一阶段实施边界（relay 新协议 + 通信加密）

### 目标

本轮只落地 relay 侧的协议升级与端到端加密中转能力，确保：

- relay 能接受并转发新的 `shell.exec` / `query.readonly` 加密协议信封
- relay 对 command / frame / exit 均按密文不透明处理中转
- relay 只持久化路由所需最小元数据，不再依赖明文 `command_json` / `command_summary_json`
- 本地 `packages/bifrost-sync-server` 可完成自动化测试与手工验证
- `bifrost-server-v4` 先完成同构代码改造，真实部署联调由主线程在远端环境执行

### 本轮不包含

- Client 侧 shell policy 匹配、沙箱 enforcement、PTY、resume 补拉
- 远端 relay 的真实部署验证与公网稳定性验证
- Web UI 的 shell policy 配置界面与审批弹窗细节
- shell.exec 真正执行命令的端到端联调

### 第一阶段交付拆分

#### 1. Relay 数据模型最小闭环

- Grant 侧统一使用 `remote_query` / `remote_shell_exec` / `remote_shell_interactive`，不再接受 `query` / `remote_invoke` 历史 alias
- Call 侧新增或切换到 route-only 字段：
  - `command_kind`
  - `command_encrypted_json`
  - `command_envelope_version`
  - `pty_enabled`
  - `timeout_hint_ms`
  - `exit_encrypted_json`
- v2 密文链路启用后，旧的明文 `command_json` / `command_summary_json` / `exit_code` / `stderr` 等字段不再作为协议来源；新请求只允许密文入口，不做历史版本兼容

#### 2. API / 路由改造

- `POST /v4/remote-invoke/caller/calls`
  - 必须提供 `command_kind`
  - 必须提供 `command_encrypted`
  - 支持 `pty_enabled`
  - 支持 `timeout_hint_ms`
  - 不再读取或接受明文 `command`
- `call_open` SSE 事件：
  - relay 原样把 `command_encrypted`、`command_kind`、`pty_enabled`、`timeout_hint_ms` 转发给 Client
  - 不透传可被 relay 解析的命令明细
- `POST /v4/remote-invoke/client/calls/:id/frame`
  - 继续只接收 `envelope_json`
  - relay 只做 token、call_id、client_instance_id 校验与转发
- `POST /v4/remote-invoke/client/calls/:id/exit`
  - 支持 `exit_encrypted`
  - relay 只记录 call 结束态与必要时间戳，原样把密文 exit 转给 Caller

#### 3. 存储与审计要求

- relay 数据库中不得新增可恢复命令明文或输出明文的字段
- event summary 仅保留路由级摘要，例如 `grant_id`、`command_kind`、`frame_size`、`status`
- 所有 shell v2 调用的人类可读预览只能在 Client 本地生成和展示

#### 4. 第一阶段本地自动化测试

本轮至少补齐以下本地 relay 自动化测试定义（`packages/bifrost-sync-server/src/__tests__`）：

- `remote invoke relay v2 openCall stores encrypted command as opaque payload`
- `remote invoke relay v2 call_open SSE forwards encrypted command without plaintext expansion`
- `remote invoke relay v2 postClientExit forwards exit_encrypted without plaintext exit fields`
- `remote invoke relay v2 list/get call API does not reconstruct plaintext command_detail for encrypted calls`
- `remote invoke relay v2 shell scope rejects grant_scope=remote_query when command_kind=shell.exec`

如当前主线程尚未完成实现，可先以 `it.todo(...)` 形式落测试定义，待实现到位后再转为可执行断言。

### 第一阶段真实场景验证清单

本轮主线程必须紧跟 `human_tests/remote-invoke.md` 中新增的第一阶段用例执行，至少覆盖：

- 本地 relay 接受 `command_encrypted` 打开的 v2 调用
- relay 数据库与日志不落命令/输出明文
- `exit_encrypted` 能从 client 原样回传到 caller
- `remote_query` grant 不可执行 `shell.exec`
- relay 重启后 v2 路由级元数据仍可恢复，且不会回退到明文存储
- 真实 CLI `remote connect -> remote status -> remote search -> remote traffic list` 在本地 relay 上完成一次完整加密黑盒闭环

### 第一阶段实现补充（2026-04-22）

结合本轮实际联调结果，第一阶段的 E2E 载荷实现再补充两条约束：

- `openCall command`、`frame`、`exit` 三类密文在第一阶段统一使用 `ChaCha20-Poly1305 + 空 AAD`，避免 Caller 与 Client 因 JSON AAD 字段形状差异导致认证失败
- Relay 继续只保留 route-level 元数据；`grant_scope`、`command_kind` 等需要路由/审计的字段由外层协议显式传递，不依赖解密密文
- Client 侧 grant 的 ECDH `shared_secret` / `caller_ephemeral_pub` / `client_ephemeral_pub` 必须本地持久化；如果 admin 本地存储协议版本不匹配或文件损坏，直接删除并按新协议重建，不做历史兼容迁移
- Caller 侧如果发现 relay `grants/reusable` 返回的是同一 caller/client 对下另一条旧 grant，必须整套回退到本地最后一次 connect 保存的 transport context：不仅使用保存的 `grant_id`，也要同时使用保存的 `caller_ephemeral_pub` / `client_ephemeral_pub`，禁止把旧 grant 的 ephemeral key 与新 grant 的 shared secret 交叉拼接
- Recent Calls / 本地审计 UI 展示的命令摘要不能依赖 relay 落库明文。若 relay 下发的 `command_summary.command_preview` 为空，client 必须用解密后的 `RemoteCommand.summary_label()` 在本地补齐可展示摘要，避免 Recent Calls 出现空白标题。

这样做的目的不是降低加密强度，而是把第一阶段优先级收敛到“强制密文传输 + 双端稳定互通”。待第二阶段 shell policy / PTY 演进时，如需恢复更丰富的 AAD 绑定，再以单一共享结构一次性升级。

## 校验要求

实现阶段应执行：

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test --workspace --all-features`
- 相关 remote shell E2E
- `rust-project-validate`

## Relay 服务改动清单

本章节基于设计方案与当前代码实际状态，系统化列出两套 Relay 服务需要做的具体改动。

**核心原则回顾**：Relay 保持透明中转角色，不存储 grant/scope/policy 等业务决策数据，也不存储任何执行审计信息。所有鉴权、执行决策与审计在 Client 完成。Relay 的职责是：路由、事件转发。

以下改动按影响面从数据模型到 API 接口逐层展开。

---

### 一、涉及的两套 Relay 服务

| 服务 | 路径 | 用途 | 存储后端 | 运行环境 |
| --- | --- | --- | --- | --- |
| bifrost-sync-server | `packages/bifrost-sync-server/` | 本地快速测试 | SQLite + 内存 | 单实例 Node.js |
| bifrost-server-v4 | `bifrost-server-v4/` | 线上生产 | Redis | 多实例 Node.js (Gulu) |

两套服务 API 路径完全一致（`/v4/remote-invoke/...`），逻辑对称，改动项相同，但存储层实现不同。

---

### 二、Grant 模型扩展

#### 2.1 现状

两套服务的 Grant 当前字段：

```
grant_id, client_instance_id, caller_fingerprint, caller_display_name,
grant_mode, grant_scope, status, created_at, first_authorized_at,
expires_at, last_used_at, max_calls, remaining_calls,
auth_method, ssh_key_fingerprint
```

其中 `grant_scope` 当前需要统一为新的 `remote_*` 枚举（sync-server 在 `service.ts` 中 `submitGrantDecision()` 设置；v4 在 `remoteInvoke.ts` 的 `forwardDecision()` 和 `submitSshConnectResult()` 中设置）。

#### 2.2 需要新增的 Grant 字段

| 字段 | 类型 | 说明 | 来源 |
| --- | --- | --- | --- |
| `grant_scope` | `string` | 使用 `'remote_query'` / `'remote_shell_exec'` / `'remote_shell_interactive'`，拒绝旧 alias | Client 在 grant decision 时传入 |
| `policy_binding` | `string` / JSON | 允许访问的 policy 集合或 policy tag | Client 在 grant decision 时传入 |
| `shell_policy_set_version_snapshot` | `number` | 授权时的全局策略版本快照 | Client 在 grant decision 时传入 |
| `interactive_allowed` | `boolean` | 是否允许 PTY | Client 在 grant decision 时传入 |
| `stdin_allowed` | `boolean` | 是否允许 stdin 流式输入 | Client 在 grant decision 时传入 |

#### 2.3 变更逻辑

- **grant decision 接口**（`POST /v4/remote-invoke/client/grants/:pairingId/decision`）：
  - body 中新增可选字段 `grant_scope`、`policy_binding`、`shell_policy_set_version_snapshot`、`interactive_allowed`、`stdin_allowed`
  - 这些字段由 Client 决定后传入，Relay 原样存储用于后续 openCall 的权限范围校验，不做业务决策
  - `grant_scope` 不再硬编码，改为从 body 取值，缺省值为 `'remote_query'`

- **SSH connect result 接口**（`POST /v4/remote-invoke/ssh/connect-result`）：
  - body 中同样新增上述可选字段
  - SSH 方式创建的 grant 也需要支持 shell 权限

- **toGrantApi 映射函数**：
  - 输出中新增上述字段的透传

- **存储层**：
  - **sync-server (SQLite)**：grants 目前以 JSON 存储在内存 Map 中，仅需在序列化/反序列化时包含新字段
  - **v4 (Redis)**：grants 以 JSON 存储在 Redis key `ri:grant:{grantId}` 中，仅需在 JSON 对象中包含新字段

---

### 三、openCall 扩展

#### 3.1 现状

当前 `calls/open` 的处理逻辑：

**sync-server** (`service.ts` `openCall()`)：
- 从 body 接收 `command` 对象
- 无命令白名单校验（直接转发）

**v4** (`remoteInvoke.ts` `openCall()`)：
- 从 body 接收 `command` 对象
- 检查 `command.command` 是否在 `ALLOWED_COMMANDS` 白名单中（`connect`, `status`, `traffic.list`, `traffic.get`, `traffic.search`, `search.get`）
- 白名单不通过则返回 `unsupported_command`

#### 3.2 需要变更的逻辑

1. **命令白名单调整**：
   - `kind=query.readonly` 的命令继续走现有 `ALLOWED_COMMANDS` 校验
   - `kind=shell.exec` 的命令**跳过** Relay 侧白名单校验（命令合法性由 Client 本地 policy 校验）
   - 判断逻辑：`command.kind === 'shell.exec'` 时不检查 `isAllowedCommand()`

2. **grant_scope 校验**（Relay 侧轻量校验）：
   - 如果 `command.kind === 'shell.exec'`，grant 的 `grant_scope` 必须为 `remote_shell_exec` 或 `remote_shell_interactive`
   - 如果 `grant_scope` 为 `remote_query` 但请求 `shell.exec`，直接返回 `grant_scope_mismatch`
   - 这是 Relay 侧唯一新增的业务校验，目的是提前拒绝明显越权请求，减少对 Client 的无效推送

3. **openCall E2E 加密改造**（详见"Caller-Client E2E 加密层 §4"）：
   - Caller 发送 `command_encrypted`（AEAD 密文），不再发送明文 `command` 对象
   - Relay 仅从请求中读取明文路由字段：`command_kind`、`pty_enabled`、`timeout_hint_ms`
   - Relay **不存储** `command_json` / `command_summary_json`，call 记录仅保留路由级字段
   - 推送给 Client 的 `call_open` SSE 事件中，**原样转发** `command_encrypted` 密文
   - Relay 无法解密命令内容——即使 Relay 被入侵也无法获知用户执行的具体命令

4. **openCall 返回值扩展**：
   - 当 Client 返回 `status=pending_approval` 时，Relay 需将此状态透传给 caller
   - 返回新增可选字段：`state`、`approval_required`、`approval_id`

#### 3.3 代码改动点

**sync-server** (`routes/remote-invoke.ts` `handleOpenCall()`):
- `service.openCall()` 的 body 中新增 `command_summary` 提取逻辑
- 对 `command.kind === 'shell.exec'` 做 `grant_scope` 校验

**v4** (`service/remoteInvoke.ts` `openCall()`):
- `isAllowedCommand()` 调用处增加 `kind` 判断分支
- 新增 `grant_scope` 校验逻辑
- `call_meta` 中存储扩展字段

---

### 四、Call 元数据扩展

#### 4.1 现有 call_meta 字段

```
call_id, grant_id, pairing_id, client_instance_id, caller_fingerprint,
caller_display_name, status, command_summary, command, source_ip,
created_at, started_at, ended_at, exit_code, duration_ms,
stdout_digest, stderr_digest, bytes_in, bytes_out
```

#### 4.2 新增 call_meta 字段（shell.exec 专用）

Relay 仅存储路由和连接管理所需的最小字段，不存储任何审计信息：

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `command_kind` | `string` | `query.readonly` / `shell.exec`（影响 TTL、rate limit 等路由行为） |
| `pty_enabled` | `boolean` | 是否使用 PTY（影响 SSE 事件帧格式） |
| `output_mode` | `string` | `split_streams` / `pty_merged`（影响帧解释方式） |

以下字段**不**在 Relay 存储，由 Client 本地审计记录：`policy_id`、`exec_mode`、`masked_command_preview`、`cwd_preview`、`env_keys`、`auth_method`、`matched_scope_id`、`matched_binding_id`、`enforcement_backend`。

#### 4.3 exit 事件扩展字段

在 Client 回传 exit 时，exit 事件内容通过 E2E 加密传输（详见"Caller-Client E2E 加密层 §5"）。Client 发送 `exit_encrypted` 密文，Relay 仅更新 call 状态为 completed 并原样转发密文，**无法解密 exit_code、termination_reason 等字段**。

加密密文内包含以下字段（仅 Caller 可解密）：

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `stdout_truncated` | `boolean` | stdout 是否被截断 |
| `stderr_truncated` | `boolean` | stderr 是否被截断 |
| `output_mode` | `string` | `split_streams` / `pty_merged` |
| `stdout_bytes` | `number` | 已采集 stdout 字节数 |
| `stderr_bytes` | `number` | 已采集 stderr 字节数 |
| `binary_output_present` | `boolean` | 是否出现过二进制输出 |
| `artifact_count` | `number` | 产生了多少个输出附件 |
| `termination_reason` | `string` | `completed` / `timeout` / `cancelled` / `policy_rejected` / `spawn_failed` / `signal_killed` |
| `rejection_code` | `string` | 被拒绝时的错误码 |
| `rejection_category` | `string` | 错误大类 |
| `rejection_message` | `string` | 用户可见提示 |

#### 4.4 存储层改动

- **sync-server**：`RemoteInvokeCall` 接口（`types.ts`）中 call 记录仅保留路由级字段（`command_kind`、`pty_enabled`、`output_mode`）；exit 扩展字段不持久化到 call 记录，仅通过 SSE 事件流透传
- **v4**：`call_meta` Redis JSON 对象中同样仅保留路由级字段；exit 扩展字段通过 SSE 事件流透传给 caller，不写入 `call_meta`

---

### 五、Frame 协议扩展

#### 5.1 现状与 E2E 加密改造

当前 frame 转发路径：
- Client → Relay：`POST /v4/remote-invoke/client/calls/:callId/frame`，body 含 `envelope_json`
- Relay → Caller：通过 SSE 推送 `frame` 事件
- Caller → Client：`POST /v4/remote-invoke/calls/:callId/input`，body 含 `envelope_json`

改造后，`envelope_json` 内的 `ciphertext` 字段为 E2E 加密密文（ChaCha20-Poly1305），Relay 仅能读取 AAD 中的明文元数据（call_id、seq、direction、frame_type）用于路由和流控，**无法解密帧内容**。详见"Caller-Client E2E 加密层 §3"。

#### 5.2 shell.exec 对 frame 的影响

**Relay 无需改动 frame 转发逻辑**。shell.exec 引入的新 frame 类型（`control`/`status`/`artifact`/`stdin`/`stdout`/`stderr`）全部封装在 `envelope_json` 的密文内部，Relay 继续做密文不透明转发。Relay 可通过 AAD 中的 `frame_type` 字段做类型级流控，但无法读取帧 payload。

但有一个例外需要处理：

- **cancel 事件的 3 段式语义**：当 caller 发起 cancel 后，Client 会依次回传 `cancel_ack` → `cancel_exit` frame。Relay 需要在接收到最终 `cancel_exit` 或超时后才更新 call_meta 状态为 `cancelled`。
  - 当前实现（v4 `forwardCallCancel()`）：立即标记为 `cancelled`
  - 需改为：先标记为 `cancel_requested`，等待 Client 回传 `cancel_exit` frame 后再标记为 `cancelled`
  - 超时后（建议 `cancel_ack_timeout_ms = 30000`）如果 Client 未确认，仍标记为 `cancelled` 并记录 `cancel_delivery_timeout`

#### 5.3 rate limit 调整

shell.exec 的输出量远大于 query.readonly。需要调整：

| 限制项 | 当前值 | 建议值 | 影响范围 |
| --- | --- | --- | --- |
| `clientDataLimiter` | 1500/10s | 5000/10s | sync-server frame 上报频率 |
| frame endpoint rate limit | 1500 req/10s | 按 `command_kind` 分级 | 两套服务 |

建议：`query.readonly` 保持现有限制；`shell.exec` 使用更宽松的 frame rate limit。

---

### 六、审批路由（新增接口）

#### 6.1 审批事件转发

审批不是 Relay 的业务逻辑，但 Relay 需要作为审批事件的路由层。审批对象由 Client 本地持久化，Relay 仅保存审批路由索引（approval_id 与 call_id 的映射），不保存审批内容或摘要。

#### 6.2 新增 API 接口

| 方法 | 路径 | 说明 | 认证 |
| --- | --- | --- | --- |
| `GET` | `/v4/remote-invoke/client/calls/:callId/approval` | Client 查询审批状态 | client_auth_token |
| `POST` | `/v4/remote-invoke/client/calls/:callId/approval-result` | Client 上报审批结果（approved/rejected/expired） | client_auth_token |

#### 6.3 实现说明

- 审批的 approve/reject 操作在 Client 的 WebUI 上由设备 owner 执行，Client 直接处理
- Client 处理完后通过 `approval-result` 接口通知 Relay 审批结果
- Relay 收到结果后：
  - 更新 `call_meta.status`（`pending_approval` → `authorized` 或 `rejected`）
  - 通过 caller 的 SSE 推送审批结果事件

---

### 七、长任务支持扩展

#### 7.1 新增 API 接口

| 方法 | 路径 | 说明 | 认证 |
| --- | --- | --- | --- |
| `POST` | `/v4/remote-invoke/calls/:callId/resume` | caller 断线后重新挂回 | relay_token |
| `GET` | `/v4/remote-invoke/calls/:callId/logs` | 查看历史输出 | relay_token |
| `GET` | `/v4/remote-invoke/calls/:callId/status` | 查询 call 详细状态 | relay_token |

#### 7.2 resume 逻辑

1. caller 使用原 `relay_token` 调用 `resume`
2. Relay 验证 call 状态为 `running` / `streaming` / `detached_waiting_resume`
3. Relay 返回新的 SSE 事件流 URL（复用 `calls/:callId/events`）
4. Client 将 spool 中已缓存的输出从 caller 断点处开始重新推送

#### 7.3 call_token TTL 扩展

当前 `CALL_ROUTE_TTL = 7200`（2小时）。shell.exec 长任务可能超过此时长。

建议：
- `query.readonly`：保持 `CALL_ROUTE_TTL = 7200`
- `shell.exec`：使用 `SHELL_CALL_ROUTE_TTL = 86400`（24小时）
- call_token 和 call route TTL 跟随 call 的 `max_timeout_ms` 动态调整，取 `max(command.timeout_ms * 2, 7200)` 秒

---

### 八、审计策略：Relay 零存储 + Client 全量审计

**核心原则：Relay 是透明中继，不存储任何用户执行信息，不提供审计能力。所有审计职责由 Client（被控端）本地承担。**

#### 8.1 Relay 的审计定位：无

Relay 在 shell.exec 链路中仅承担以下职责：

- 签名验证与身份路由
- call 生命周期状态机管理（pending → running → completed）
- SSE 事件流的不透明转发

Relay **不存储**以下任何信息：

- 命令内容（无论明文还是脱敏版本）
- 执行结果（exit_code、stdout/stderr digest、duration 等）
- 策略/scope/binding 匹配结果
- enforcement 后端状态
- 环境变量 key 或 value

Relay 对 shell.exec 事件帧（`envelope_json`）保持不透明转发——既不解析也不持久化帧内容。call 结束后，Relay 仅保留路由级元数据（call_id、device_code、状态、时间戳），不保留任何与用户执行内容相关的字段。

#### 8.2 Client 本地完整审计

所有审计记录由 Client 在目标设备本地生成和存储，设备 owner 拥有完整追溯能力：

| 审计字段 | 说明 |
| --- | --- |
| `call_id` | 调用 ID（可与 Relay 路由记录关联） |
| `caller_fingerprint` | 调用发起人的公钥指纹 |
| `policy_id` / `policy_name` | 命中的策略 |
| `matched_scope_id` / `matched_binding_id` | 命中的 scope 和 binding |
| `exec_mode` | 执行模式 |
| `command_text` | 完整命令明文（本地加密存储） |
| `masked_command_preview` | 脱敏命令预览（用于本地 UI 展示） |
| `cwd` | 工作目录 |
| `env_keys` | 环境变量 key 列表（不记录 value） |
| `exit_code` | 退出码 |
| `termination_reason` | 终止原因 |
| `stdout_digest` / `stderr_digest` | 输出摘要哈希 |
| `duration_ms` | 执行时长 |
| `enforcement_backend` | OS enforcement 后端标识 |
| `enforcement_status` | enforcement 就绪状态 |
| `enforcement_capabilities` | enforcement 能力维度 |
| `sandbox_config_digest` | 沙箱配置哈希 |
| `timestamp` | 执行时间 |

Client 本地审计记录可选保留最近 N KB 的 stdout/stderr 片段、长任务 spool 文件路径与保留截止时间，供设备 owner 按需查阅。

#### 8.3 命令预览脱敏

命令预览脱敏逻辑完全在 Client 侧执行，脱敏结果仅用于本地 UI 展示和设备 owner 的审批弹窗。Relay 不接收也不存储任何命令预览信息。

脱敏规则：
1. 仅保留前 256 字符，超出部分截断
2. 对敏感模式做值级遮罩（`Authorization: Bearer ...`、`token=...`、`password=...` 等）
3. 模板变量标记为 `secret=true` 的只展示变量名
4. `stdin` 内容不进入预览
5. here-doc / 多行文本仅保留首行摘要

---

### 九、toCallApi / toGrantApi 映射扩展

#### 9.1 toGrantApi 新增字段

```typescript
{
  // ... 现有字段 ...
  grant_scope: grant.grant_scope || 'remote_query',
  policy_binding: grant.policy_binding || null,
  shell_policy_set_version_snapshot: grant.shell_policy_set_version_snapshot || null,
  interactive_allowed: grant.interactive_allowed || false,
  stdin_allowed: grant.stdin_allowed || false,
}
```

#### 9.2 toCallApi 新增字段

Relay 仅透传路由和连接级字段，不存储或暴露任何审计信息（命令内容、执行结果、策略匹配、enforcement 状态等均由 Client 本地审计负责）：

```typescript
{
  // ... 现有字段 ...
  command_kind: call.command_kind || 'query.readonly',
  pty_enabled: call.pty_enabled || false,
  output_mode: call.output_mode || null,
}
```

> 注意：`exit_code`、`termination_reason`、`enforcement_backend` 等执行结果字段通过 SSE 事件流实时传递给 caller，不在 Relay 的 call 模型中持久化。caller 如需保留这些信息，应在接收 SSE 事件时自行记录。

---

### 十、两套服务的差异化改动点

**以下改动不兼容历史版本**。两套服务同步改造，旧版 Caller/Client 无法与新版 Relay 交互。

#### 10.1 bifrost-sync-server（SQLite 存储）

##### 10.1.1 E2E 加密改造

| 改动项 | 文件 | 具体改造 |
| --- | --- | --- |
| Grant 密钥交换 | `src/remote-invoke/service.ts` `submitGrantDecision()` | 从 body 读取 `client_ephemeral_pub`，与请求中的 `caller_ephemeral_pub` 一起存入 grant 记录。Relay 仅做公钥原样存储和转发，不参与 ECDH 计算 |
| Grant 类型扩展 | `src/types.ts` `RemoteInvokeGrant` | 新增 `caller_ephemeral_pub: string` 和 `client_ephemeral_pub: string` 字段 |
| 密钥透传 | `src/remote-invoke/service.ts` `submitGrantDecision()` | approve 时将 `client_ephemeral_pub` 推送给 Caller（通过 SSE `grant_decision` 事件） |

##### 10.1.2 openCall 改造

| 改动项 | 文件 | 具体改造 |
| --- | --- | --- |
| 请求体变更 | `src/remote-invoke/service.ts` `openCall()` | 不再从 body 读取 `command`/`command_summary` 明文对象，改为接收 `command_encrypted`（密文 JSON）和明文路由字段（`command_kind`、`pty_enabled`、`timeout_hint_ms`） |
| call 记录精简 | `src/remote-invoke/service.ts` `openCall()` | call 创建时不再写入 `command_summary_json`/`command_json`（当前 L511-512），改为写入 `command_kind`。`stdout_digest`/`stderr_digest`/`exit_code` 等审计字段移除初始化 |
| call_open 推送 | `src/remote-invoke/service.ts` `openCall()` | 推送给 Client 的 `call_open` SSE 事件中，用 `command_encrypted` 替代明文 `command`/`command_summary`（当前 L528-536）。Relay 原样转发密文 |
| event 记录精简 | `src/remote-invoke/service.ts` `openCall()` | `appendEvent` 的 `event_summary_json` 不再包含 grant_id 以外的业务字段（当前 L544 已基本合规） |
| Call 类型精简 | `src/types.ts` `RemoteInvokeCall` | 移除 `command_summary_json`/`command_json`/`stdout_digest`/`stderr_digest`/`exit_code`/`duration_ms`/`bytes_in`/`bytes_out` 字段，新增 `command_kind: string`、`command_encrypted_json: string`（仅内存流转，不持久化） |

##### 10.1.3 Exit + Frame 改造

| 改动项 | 文件 | 具体改造 |
| --- | --- | --- |
| exit 请求体变更 | `src/remote-invoke/service.ts` `postClientExit()` | 不再从 body 读取 `exit_code`/`duration_ms`/`stdout_digest`/`stderr_digest`/`bytes_in`/`bytes_out` 等明文字段（当前 L621-629），改为接收 `exit_encrypted`（密文 JSON）|
| exit 存储精简 | `src/remote-invoke/service.ts` `postClientExit()` | `updateCall()` 仅更新 `status='completed'` 和 `ended_at`（当前 L621-630 的 exit_code/digest 等字段移除）|
| exit SSE 推送 | `src/remote-invoke/service.ts` `postClientExit()` | `pushToCallerStream()` 推送 `exit_encrypted` 密文替代当前的明文 exit 字段（当前 L632-639）|
| exit event 记录 | `src/remote-invoke/service.ts` `postClientExit()` | `event_summary_json` 不再包含 `exit_code`/`duration_ms`（当前 L652），仅记录 `call_completed` 事件类型 |
| frame 转发 | `src/remote-invoke/service.ts` `postClientFrame()`/`postCallerInput()` | 保持 `envelope_json` 不透明转发不变。`envelope_json` 内已是 E2E 加密密文（由 Caller/Client 加密），Relay 不解析 |
| frame event 记录 | `src/remote-invoke/service.ts` `postClientFrame()` | `event_summary_json` 仅记录 `size`（当前 L608 已合规），不记录帧内容 |

##### 10.1.4 shell.exec 业务扩展

| 改动项 | 文件 | 说明 |
| --- | --- | --- |
| 白名单调整 | `src/remote-invoke/service.ts` `openCall()` | `kind=shell.exec` 跳过 `isAllowedCommand()` 校验（当前 L492-494） |
| grant_scope 校验 | `src/remote-invoke/service.ts` `openCall()` | 新增：`shell.exec` 要求 `grant.grant_scope` 为 `remote_shell_exec` 或 `remote_shell_interactive` |
| Grant 类型扩展 | `src/types.ts` `RemoteInvokeGrant` | 新增 `grant_scope`、`policy_binding` 等字段 |
| grant decision 逻辑 | `src/remote-invoke/service.ts` `submitGrantDecision()` | 从 body 读取 `grant_scope` 等新字段 |
| cancel 改造 | `src/remote-invoke/service.ts` `cancelCall()` | 改为 `cancel_requested` + 等待 Client 确认 |
| 新路由 | `src/routes/remote-invoke.ts` | 新增 resume / logs / status / approval-result 路由 |
| rate limit | `src/routes/remote-invoke.ts` | 按 `command_kind` 分级 frame rate limit |

---

#### 10.2 bifrost-server-v4（Redis 存储）

##### 10.2.1 E2E 加密改造

| 改动项 | 文件 | 具体改造 |
| --- | --- | --- |
| Grant 密钥交换 | `app/service/remoteInvoke.ts` `forwardDecision()` | 从 body 读取 `client_ephemeral_pub`，存入 grant JSON（Redis）。将 `client_ephemeral_pub` 通过 SSE `grant_decision` 事件推送给 Caller |
| SSH Grant 密钥交换 | `app/service/remoteInvoke.ts` `submitSshConnectResult()` | 同理，SSH 配对时传入 `client_ephemeral_pub` |
| toGrantApi 扩展 | `app/service/remoteInvoke.ts` `toGrantApi()` | 新增 `caller_ephemeral_pub`/`client_ephemeral_pub` 字段映射（当前 L160-179 函数） |

##### 10.2.2 openCall 改造

| 改动项 | 文件 | 具体改造 |
| --- | --- | --- |
| 请求体变更 | `app/service/remoteInvoke.ts` `openCall()` | 不再读取 `body.command`/`body.command_summary` 明文（当前 L717-718），改为接收 `body.command_encrypted` 和明文路由字段 |
| 白名单绕过 | `app/service/remoteInvoke.ts` `openCall()` | 当前 L662 的 `isAllowedCommand()` 校验需按 `command_kind` 分支：`shell.exec` 跳过白名单 |
| call_meta 精简 | `app/service/remoteInvoke.ts` `openCall()` | Redis call_meta JSON（当前 L709-722）移除 `command_summary`/`command` 字段，新增 `command_kind`。密文 `command_encrypted` 不写入 call_meta |
| call_open 推送 | `app/service/remoteInvoke.ts` `openCall()` | `pushToClient` 推送（当前 L729-736）用 `command_encrypted` 替代明文 `command`/`command_summary` |
| toCallApi 精简 | `app/service/remoteInvoke.ts` `toCallApi()` | 移除 `command_summary`（L195）、`command`（L196）、`command_detail`（L197）字段映射，新增 `command_kind` |

##### 10.2.3 Exit + Frame 改造

| 改动项 | 文件 | 具体改造 |
| --- | --- | --- |
| exit 请求体变更 | `app/service/remoteInvoke.ts` `forwardClientExit()` | 不再从 `data` 读取 `exit_code`/`duration_ms`/`stdout_digest`/`stderr_digest`/`bytes_in`/`bytes_out`（当前 L597-606），改为接收 `data.exit_encrypted` |
| exit call_meta 更新 | `app/service/remoteInvoke.ts` `forwardClientExit()` | `meta` 更新（当前 L612-620）仅设置 `status='completed'` + `ended_at`，移除 `exit_code`/`duration_ms`/`stdout_digest`/`stderr_digest`/`bytes_in`/`bytes_out` 的写入 |
| exit SSE 推送 | `app/service/remoteInvoke.ts` `forwardClientExit()` | `pushToCallerStream` 推送（当前 L597-606）改为转发 `exit_encrypted` 密文 |
| frame 转发 | `app/service/remoteInvoke.ts` | frame 路径保持 `envelope_json` 不透明转发不变（当前 L585/L592 已是不透明转发） |
| cancel SSE 推送 | `app/service/remoteInvoke.ts` | `cancelCall()` 中的 SSE 推送（当前 L650-656）仅包含 `cancel_requested` 状态，不含业务数据 |

##### 10.2.4 shell.exec 业务扩展

| 改动项 | 文件 | 说明 |
| --- | --- | --- |
| grant_scope 校验 | `app/service/remoteInvoke.ts` `openCall()` | 新增 `grant_scope` 与 `command_kind` 匹配校验 |
| grant decision 逻辑 | `app/service/remoteInvoke.ts` `forwardDecision()` | 从 body 读取 `grant_scope` 等新字段存入 grant |
| SSH grant 创建 | `app/service/remoteInvoke.ts` `submitSshConnectResult()` | 支持传入 `grant_scope` 等新字段 |
| cancel 改造 | `app/service/remoteInvoke.ts` `forwardCallCancel()` | 改为先 `cancel_requested`，等 Client 确认后才 `cancelled` |
| call_token TTL | `app/service/remoteInvoke.ts` | shell.exec 使用动态 TTL |
| 新路由 | `app/routes/remoteInvoke.ts` | 新增 resume / logs / status / approval-result 路由 |
| toGrantApi | `app/service/remoteInvoke.ts` `toGrantApi()` | 透传新增 grant 字段（grant_scope 等） |
| Redis key TTL | `app/service/remoteInvoke.ts` | shell.exec call 使用更长的 TTL |

---

### 十一、向后兼容策略

**本次改造不兼容历史版本**，采用一刀切升级策略：

1. **E2E 加密为强制要求**：所有 openCall 请求必须包含 `command_encrypted`，发送明文 `command` 的旧版 Caller 将收到协议版本不匹配错误
2. **Grant 密钥交换为强制要求**：所有 grant 创建必须完成 ephemeral 公钥交换，缺少 `caller_ephemeral_pub` 或 `client_ephemeral_pub` 的 grant 创建请求将被拒绝
3. **数据模型不兼容**：`RemoteInvokeCall` 移除了 `command_json`、`command_summary_json`、`exit_code`、`stdout_digest`、`stderr_digest` 等字段，旧版 UI 读取这些字段将得到空值
4. **EncryptedEnvelope version=2**：帧协议强制 v2 格式（真加密），v1 格式（明文伪装）不再接受
5. **Grant 存量处理**：升级后所有存量 grant 因缺少 ephemeral 密钥对而自动失效，需重新配对
6. **API 接口**：新增接口使用新路径，现有 query.readonly 链路的请求体格式同步变更（command → command_encrypted）

---

### 十二、实施顺序

1. **E2E 加密基础设施**：实现 X25519 ECDH 密钥交换（grant 创建时）、HKDF per-call 会话密钥派生、ChaCha20-Poly1305 帧加解密，改造 EncryptedEnvelope 为 v2 真加密格式
2. **openCall + 数据模型改造**：openCall 改为 `command_encrypted` 密文传输，Relay 移除 `command_json`/`command_summary_json` 存储，call 记录精简为路由级字段
3. **Client 审计 + exit 加密**：exit 事件改为 `exit_encrypted` 密文传输，Client 本地审计记录实现，Relay 不再解析 exit_code 等字段
4. **长任务支持**：resume / logs / status 新接口、cancel 3 段式语义、动态 TTL
5. **审批路由**：approval-result 接口、pending_approval 状态管理
6. **三平台交互能力闭环**：Unix PTY、Windows ConPTY、resize、signal / Ctrl-C、stdin 流式输入、平台专属测试入口全部打通

bifrost-sync-server 优先实现（便于本地调试），验证通过后同步到 bifrost-server-v4。
