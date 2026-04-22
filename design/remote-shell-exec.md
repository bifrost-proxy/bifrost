# 基于现有 Remote 方案的 Shell 远程执行设计

> 状态：讨论稿 | 更新时间：2026-04-22

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
- 摘要级审计

Relay 不做：

- shell 白名单匹配
- 命令正则校验
- cwd / env 合法性判断
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

### 5. “长期免审批 shell”只建议给 SSH 公钥授权

推荐默认规则：

- `pair_code`：可发起 shell 请求，但默认只允许 `once` 或短时 grant
- `ssh_publickey`：可获得长期 `remote_shell_exec` grant
- `remote_shell_interactive`：默认仅允许 `ssh_publickey`

这样可以把“高频自动化执行”和“人工临时远控”分层。

## 目标

- 基于当前 remote invoke 架构，新增通用 shell 远程执行能力
- 支持目标设备配置白名单，决定哪些命令可远程执行
- 支持 stdout / stderr 流式回传
- 支持超时、取消、输出截断和审计摘要
- 支持执行时间很长的任务，并在长时间运行下保持可观察、可续连、可取消
- 支持 macOS / Linux / Windows 三大平台，并明确能力分级与平台差异
- 沙箱控制策略对齐行业常见 agent 方案，尤其是 Codex 风格的审批、文件系统、网络和命令范围控制
- 支持 CI / Agent / 人工终端三类 caller
- 支持通过 SSH grant 做长期设备绑定
- 支持后续演进到交互式 PTY 会话

## 非目标

- 不在首版引入第二条独立传输协议
- 不让 relay 保存目标设备的明文白名单
- 不默认开放完全无限制的裸 `sh -c`
- 不在首版做文件上传下载隧道
- 不在首版支持完全脱离 call 生命周期的后台 daemon
- 不追求三平台所有 shell 在首版做到完全等价体验，而是先定义统一协议 + 平台能力分级

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

这是最灵活也最危险的能力。推荐只在：

- 设备 owner 显式开启高级模式
- 仅 SSH 公钥调用方
- 仅特定 caller / 特定策略集

时开放。

## 总体架构

```text
Caller
  -> remote connect / ssh connect
  -> openCall(kind=shell.exec)
Relay
  -> 路由 + 摘要审计 + 事件转发
Client Worker
  -> 校验 grant_scope / auth_method / policy_version
  -> 本地匹配 shell policy
  -> 执行进程并流式回传 stdout/stderr
```

### 核心原则

1. 授权与执行解耦
2. Relay 透明，Client 决策
3. 白名单只在目标设备本地生效
4. Shell grant 与 query grant 分离
5. 高风险能力默认只给 SSH 设备授权

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

对应关系可以理解为：

- `manual_every_time`
  - 接近 Suggest 风格
- `auto_within_profile`
  - 接近“只在受限沙箱里自动执行”
- `break_glass_only`
  - 明确比 Full Auto 更危险，需要更高门槛

关键点：

- 自动执行能力必须绑定到具体 `sandbox_profile`
- 不能出现“只要是 SSH 就无限自动执行”

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
  - 仅 break-glass 可用

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
- 审批：可按 SSH 自动

#### `repo_bounded_exec`

- 文件系统：仅仓库根目录 + `tmp`
- 网络：关闭
- 命令：`template` / `argv_exec`
- 审批：首次审批

#### `dependency_build`

- 文件系统：仓库根目录可写
- 网络：`preset_dependencies`
- 命令：受控构建命令
- 审批：首次审批或范围变更审批

#### `ops_break_glass`

- 文件系统：宽范围
- 网络：allowlist 或 full
- 命令：可放开 `shell_text`
- 审批：每次审批

### 9. 决策优先级

推荐最终判定顺序：

1. `grant/binding/scope`
2. `sandbox_profile.filesystem_scope`
3. `sandbox_profile.network_scope`
4. `sandbox_profile.command_scope`
5. `sandbox_profile.process_scope`
6. `approval_mode`
7. 才进入真正执行

也就是说：

- 白名单命中不代表一定能执行
- 还必须满足 sandbox profile 的边界

### 10. 推荐的对齐结论

如果按 Codex 风格总结成一句话，我们这边最值得对齐的是：

- **默认拒绝**
- **边界先于执行**
- **自动化必须绑在受限 profile 上**
- **网络默认关闭，按域名和方法放行**
- **命令范围不只看命令名，还要看路径、参数、进程和文件系统范围**

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

PTY 不可能在三平台首版完全等价。

建议：

- Phase 1：三平台都只支持非 PTY
- Phase 2：
  - macOS / Linux：PTY
  - Windows：ConPTY（要求系统版本满足条件）

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

### 14. 推荐 MVP 兼容策略

如果我们要把三平台一起纳入 MVP，推荐边界是：

- 三平台都支持：
  - `argv_exec`
  - `template`
  - 非 PTY
  - 长任务 heartbeat / resume / logs / cancel
- `shell_text`：
  - macOS / Linux：可选
  - Windows：只建议 `pwsh`，不建议 `cmd`
- PTY：
  - 全部放到 Phase 2

这样可以把“三平台兼容”先建立在最稳的执行模型上，而不是一上来就被 PTY 和 shell 方言拖住。

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
| `grant_mode_policy` | `once_only` / `short_lived` / `ssh_persistent_allowed` |
| `approval_policy` | `always` / `on_first_use` / `on_policy_change` |
| `pty_platform_support_json` | 各平台 PTY 支持情况 |
| `policy_version` | 变更后自增，用于让已有 grant 失效 |
| `created_at` / `updated_at` | 时间戳 |

### 3. 扩展 GrantInfo

建议给 shell grant 增加绑定信息：

| 字段 | 说明 |
| --- | --- |
| `grant_scope` | `remote_query` / `remote_shell_exec` / `remote_shell_interactive` |
| `policy_binding` | 允许访问的 policy 集合或 policy tag |
| `policy_version_snapshot` | 授权时的策略版本 |
| `interactive_allowed` | 是否允许 PTY |
| `stdin_allowed` | 是否允许 stdin 流式输入 |

这样当目标设备修改白名单后，可以拒绝旧 grant：

- `policy_version_snapshot != current_policy_version`

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

建议规则：

- 默认关闭
- 必须显式勾选 `advanced_trusted_shell`
- 默认仅允许 `ssh_publickey`
- 默认不允许 pair code 拿永久 grant

示例：

```json
{
  "policy_id": "trusted-ops",
  "exec_mode": "shell_text",
  "shell_kind": "bash",
  "command_regex": "^cd /srv/(api|web) && (git (status|pull)|pnpm (install|build)|pm2 restart [a-z0-9-]+)$",
  "allowed_auth_methods": ["ssh_publickey"],
  "grant_mode_policy": "ssh_persistent_allowed"
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
- 只允许 `ssh_publickey`
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
| `pty_policy` | `forbidden` / `allowed` / `ssh_only` |
| `approval_policy` | `always` / `on_first_use` / `high_risk_only` |
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
| `low` | SSH 可长期复用，pair_code 可短时 |
| `medium` | SSH 可长期复用，但首次需审批 |
| `high` | 仅 SSH，可一次性或短时，默认每次审批 |
| `critical` | 仅 break-glass，强制一次性审批，不可复用 |

### 10. 推荐 Break-Glass 模式

如果要支持非常宽的 client 自定义白名单范围，必须有一条“紧急高风险通道”，而不是把正常通道无限放大。

建议新增特殊 tag / scope：

- `break-glass`

特征：

- 默认关闭
- 仅设备 owner 手动开启
- 仅 SSH
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
- 仅 SSH
- 限定 cwd 前缀
- 限定 executable 路径
- 限定 env key
- 可长期 grant
- 长任务允许续连

#### 模板 C：人工运维

- tag: `service`
- SSH-only
- 允许 `argv_exec`，可选少量 `shell_text`
- `high_risk_only` 审批

#### 模板 D：Break-Glass

- tag: `break-glass`
- 仅 SSH
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

### 15. 推荐的 MVP 白名单策略

如果想先做一版既强又不复杂的 client 自定义白名单，我建议 MVP 先做：

1. `Policy`
   - 支持 `template` / `argv_exec`
2. `Policy Tag`
   - 支持 tag
3. `Binding`
   - 支持按 `ssh_key_fingerprint` / `auth_method` 绑定 policy/tag
4. `Scope`
   - 只先支持：
     - `cwd_range`
     - `env_range`
     - `quota`
     - `approval_policy`
     - `effect`
5. 决策优先级
   - `deny > allow > default_deny`

先不做：

- 太复杂的布尔表达式规则
- 多层嵌套条件树
- 用户手写复杂 DSL

### 16. 我的建议

我更推荐的方向不是“开放一个超强 regex 白名单”，而是：

- **以 policy/tag 为能力中心**
- **以 scope 为范围中心**
- **以 binding 为主体中心**
- **以 deny 优先的默认拒绝为安全底座**

这样 client 侧不仅能“自定义白名单范围”，而且可以做到可维护、可解释、可审计。

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

推荐做法：

- 目标设备维护 `shell_policy_set_version`
- grant 记录签发时的版本快照
- 执行前校验版本一致

### 4. 默认授权矩阵

| auth_method | query.readonly | shell.exec(template/argv) | shell.exec(shell_text) |
| --- | --- | --- | --- |
| `pair_code` | 支持 | 支持，但默认 `once/短时` | 默认不支持 |
| `ssh_publickey` | 支持 | 支持，可长期 | 支持，但默认需显式开启 |

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

下面这些错误码建议作为首版稳定枚举：

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

### 5.7 CLI / WebUI 展示建议

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

### 5.8 审计字段建议

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
  - 摘要
  - 最近少量回放索引
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

### 10.1 回放接口建议

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

### 10.2 审计和展示建议

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

- `policy_version` 在 `openCall` 时校验
- call 启动后冻结本次执行上下文
- 后续策略更新只影响新 call

### 15. 推荐 MVP 边界

如果我们要先把长任务做稳，推荐 MVP 包含：

- 非 PTY 长任务
- worker 心跳
- caller 断线 10 分钟内续连
- client 本地 spool
- `logs/status/cancel/resume`

先不做：

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
4. `policy_version` 一致性校验
5. `exec_mode` 与请求结构匹配
6. `cwd` 校验
7. `env` 校验
8. `stdin` 大小与模式校验
9. `timeout` 上限校验
10. `pty` 开关校验

任何一步失败都直接返回，不进入进程创建。

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

### 6. PTY 设计

建议分阶段：

- Phase 1：仅非交互式进程，无 PTY
- Phase 2：支持 PTY，会话可 resize，可传 Ctrl-C

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
- 建议仅给 SSH 设备授权
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

### Relay 侧仅保存摘要

建议 relay 保留：

- `policy_id`
- `policy_name`
- `exec_mode`
- `masked_command_preview`
- `cwd_preview`
- `env_keys`
- `auth_method`
- `exit_code`
- `stdout_digest`
- `stderr_digest`
- `duration_ms`

不保存：

- 完整命令明文
- env value
- stdin 明文
- stdout / stderr 全文

### Client 本地保留完整审计

推荐目标设备本地另存一份更完整的审计记录：

- 明文命令
- 变量值
- 完整退出原因
- 调用发起人
- 可选最近 N KB stdout/stderr
- 长任务 spool 文件路径与保留截止时间

这样既不把高风险内容上传 relay，又保留设备 owner 的追溯能力。

## 安全风险与缓解

### 风险 1：任意 shell 导致能力过大

缓解：

- 默认不开 `shell_text`
- 高风险策略只允许 `ssh_publickey`
- shell grant 与 query grant 分离

### 风险 2：命令注入

缓解：

- `template` / `argv_exec` 默认不走 shell
- 变量必须类型校验
- `shell_text` 只在显式规则下允许

### 风险 3：白名单变更后旧 grant 继续可用

缓解：

- 绑定 `policy_version`
- 白名单更新后使旧 shell grant 失效

### 风险 4：环境变量泄露

缓解：

- 不默认继承全环境
- env key allowlist
- 审计只记 key 不记 value

### 风险 5：输出过大或长时间挂起

缓解：

- 输出大小限制
- timeout
- cancel
- 每策略并发上限
- worker heartbeat + caller resume，避免“无输出但其实还活着”的误判

### 风险 6：后台进程脱离会话

缓解：

- MVP 不支持 detach
- call cancel 或 caller 退出时回收进程组

### 风险 7：pair code 获得过强权限

缓解：

- pair code 默认只给 `once` 或短时 shell grant
- 高风险策略默认仅 SSH

## 分阶段落地建议

### Phase 1：受控非交互式 shell

范围：

- `template`
- `argv_exec`
- 非 PTY
- stdout/stderr 流式输出
- stdout/stderr 分流回放 + merged 视图
- 基础二进制输出识别
- 大输出落盘与按 seq 补拉
- stdin 支持 `none` / `inline`
- 长任务 heartbeat / resume / logs / cancel

这是推荐 MVP。

### Phase 2：高级 shell_text

范围：

- `shell_text`
- regex 白名单
- SSH-only 长期 grant
- 策略版本绑定

### Phase 3：交互式 PTY

范围：

- PTY 打开
- resize
- signal / ctrl-c
- stdin 流式输入

## 推荐的 MVP 决策

如果要最快做出可讨论、可上线的第一版，我建议：

1. 先不做真正“任意 shell 文本”
2. 先做 `template + argv_exec`
3. shell grant 只给 `ssh_publickey` 长期复用
4. pair code 只允许一次性 shell 审批
5. Relay 继续只保存摘要
6. Client 本地新增完整 shell policy 与审计能力

这样我们能先把：

- 自动化部署
- 远程诊断
- 服务重启
- 仓库同步构建

这些高价值场景跑起来，再决定是否放开 `shell_text`。

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
- `shell_grant_policy_version_mismatch_rejected`
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

### E2E 测试

- `test_remote_shell_exec_template_e2e.sh`
  - SSH connect 后执行模板命令成功
- `test_remote_shell_exec_pair_code_once_e2e.sh`
  - pair code 授权仅一次可用
- `test_remote_shell_exec_policy_reject_e2e.sh`
  - 非白名单命令被拒绝
- `test_remote_shell_exec_cancel_e2e.sh`
  - 长命令可被 caller cancel，且 client 侧目标进程被真正终止
- `test_remote_shell_exec_cancel_no_resume_e2e.sh`
  - cancel 后不能再通过 `resume` 重新附着
- `test_remote_shell_exec_policy_version_invalidate_e2e.sh`
  - 白名单更新后旧 grant 失效
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
- PTY / ConPTY 相关用例放到 Phase 2，不阻塞 MVP

### Human Tests

实现时必须新增并执行：

- `human_tests/remote-shell-exec.md`

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
- pair code 仅能一次性执行 shell
- shell grant 在策略更新后失效
- 长时间命令超时与取消
- caller 执行 cancel 后，目标设备上的命令进程确实被杀掉，而不是继续后台运行
- cancel 后 call 不能 resume，状态最终为 `cancelled`
- 输出截断与审计摘要正确
- 长任务执行中关闭 caller 后重新续连成功
- 长任务长时间无输出但状态仍显示 running
- 同一 `policy_id` 在 macOS / Linux / Windows 上命中正确的平台变体
- Windows 目标设备上 PowerShell 输出、路径和取消语义正常

## 校验要求

实现阶段应执行：

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test --workspace --all-features`
- 相关 remote shell E2E
- `rust-project-validate`

## 需要重点讨论的决策

1. MVP 是否就开放 `shell_text`，还是只做 `template + argv_exec`
2. `pair_code` 是否允许 shell grant，还是完全限制为 SSH-only
3. relay 历史里是否显示命令预览，还是只显示 policy 名称
4. Client 本地是否保存完整 stdout/stderr，还是只保留尾部摘要
5. PTY 要不要进首版，还是明确放到第二阶段

## 我的建议

从仓库当前的 remote 设计和安全边界看，最稳妥的路线是：

- **协议层不改骨架**
- **执行层新增 `shell.exec`**
- **能力层先做受控白名单，不先做完全裸 shell**
- **长期 shell 自动化默认建立在 SSH grant 上**

这样既能满足“目标设备上远程执行命令”的真实诉求，也不会把现在已经比较清晰的 remote invoke 安全模型一下子打穿。
