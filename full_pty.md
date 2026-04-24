# Remote Invoke Full PTY 可执行方案

> 状态：审查修订版，可按 PR 拆分执行
> 更新时间：2026-04-24

## 0. 结论

本方案在现有 Remote Invoke / relay / shell.exec 链路上补齐交互式 PTY，不重写 relay，不新增 WebSocket 隧道，也不引入 SSH tunnel。

核心判断：

- relay 已具备 `open_call`、caller input POST、client frame POST、client exit POST、caller/client 双 SSE 推送能力。
- relay 继续只做认证、授权、生命周期、限流、大小限制和 opaque encrypted envelope 转发。
- 真正缺口在两端：
  - caller CLI：本地 terminal runtime、raw mode、stdin/resize/cancel pump、有序发送队列、PTY output bytes 写回 stdout。
  - target admin worker/executor：PTY session runtime、caller input 路由、PTY reader/writer/waiter 生命周期、cancel/idle/backpressure。

最终交互入口统一为：

```bash
bifrost remote command shell [--cwd <path>] [--term <term>] [--command <text>]
```

现有 `bifrost remote shell ...` 保持为 target 本地 Shell Access policy/profile 管理入口，不承载交互会话。

## 1. 当前代码事实

### 1.1 Relay 已具备可复用通道

当前 relay 已有这些路径：

```text
POST /v4/remote-invoke/calls/open
POST /v4/remote-invoke/calls/{call_id}/input
GET  /v4/remote-invoke/calls/{call_id}/events
POST /v4/remote-invoke/client/calls/{call_id}/frame
POST /v4/remote-invoke/client/calls/{call_id}/exit
```

`OpenCallRequest` 已有 `pty_enabled?: boolean`，`ClientCallFrameRequest` / `ClientCallExitRequest` 已支持 `envelope_json` 和 `bytes_in` / `bytes_out` 等统计字段。

需要补的是 gate 和保护：

- `pty_enabled=true` 必须要求 `command_kind=shell.exec`
- `pty_enabled=true` 必须要求 `grant_scope=remote_shell_interactive`
- caller input 只允许进入 `pty_enabled=true` 的 call
- route 和 service 层都必须限制 body/envelope 大小

### 1.2 Rust 协议层已有 PTY 预留但未闭环

当前 admin `types.rs` 已有：

- `GrantScope::RemoteShellExec`
- `GrantScope::RemoteShellInteractive`
- `CommandKind::ShellExec`
- `StdinMode::{None, Inline, Stream}`
- `OutputMode::{SplitStreams, PtyMerged}`
- `RemoteCommand::{stdin_mode, pty, output_mode}`
- `RemotePtyRequest { enabled: bool }`
- `EncryptedEnvelope` / `EnvelopeAad`

当前 worker 收到 `call_frame` 仍只是日志：

```text
call_frame received (stdin forwarding not yet implemented)
```

因此 PTY 改造必须把 `call_frame` 消费、解密、校验、路由到 active PTY session。

### 1.3 当前 policy 安全边界必须保持

caller 不允许指定 `policy_id`。

当前生效模型是：

- caller 只表达要执行什么命令。
- relay 只看最小 `grant_scope` 和路由级 metadata。
- target 基于本地 Shell Access 配置、grant binding、policy version snapshot 自动选择唯一 policy。
- 如果 caller payload 里出现非空 `policy_id`，target 必须拒绝：

```text
shell.exec caller must not specify policy_id; the target device selects policy
```

PTY 也遵守同一规则，不能重新引入 `--policy` 或 `policy_id`。

## 2. 目标链路

```text
bifrost remote command shell
  |
  | open_call: shell.exec + pty_enabled=true
  v
relay /v4/remote-invoke/calls/open
  |
  | call_open
  v
target RemoteInvokeWorker
  |
  | decrypt command, target selects policy, create PTY session
  v
remote PTY process

caller stdin / resize / cancel
  |
  | CLI local ordered frame queue
  | POST /v4/remote-invoke/calls/{call_id}/input
  v
relay opaque call_frame
  |
  v
target worker decrypts TerminalFrame::Stdin / Resize / Signal / Eof
  |
  v
PTY writer / resize / signal

PTY output
  |
  | target POST /v4/remote-invoke/client/calls/{call_id}/frame
  v
relay caller SSE frame
  |
  v
CLI decrypts TerminalFrame::Output and writes raw bytes to stdout

PTY terminal completion
  |
  | target POST /v4/remote-invoke/client/calls/{call_id}/exit
  v
relay exit event + call history terminal state
```

## 3. 协议扩展

### 3.1 `RemotePtyRequest`

`crates/bifrost-admin/src/remote_invoke/types.rs`：

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemotePtyRequest {
    #[serde(default)]
    pub enabled: bool,

    #[serde(default)]
    pub cols: Option<u16>,

    #[serde(default)]
    pub rows: Option<u16>,

    #[serde(default)]
    pub term: Option<String>,
}
```

新增 helper：

```rust
impl RemoteCommand {
    pub fn pty_enabled(&self) -> bool {
        self.pty.as_ref().map(|p| p.enabled).unwrap_or(false)
    }

    pub fn wants_streaming_stdin(&self) -> bool {
        matches!(self.stdin_mode, Some(StdinMode::Stream))
    }

    pub fn wants_pty_merged_output(&self) -> bool {
        matches!(self.output_mode, Some(OutputMode::PtyMerged))
    }
}
```

### 3.2 `TerminalFrame`

PTY frame 不能复用当前 `EncryptedFramePayload { chunk: String }`。PTY 是字节流，包含 ANSI escape、控制字符和可能的非 UTF-8 字节，必须 base64 bytes。

建议在 admin 新建：

```text
crates/bifrost-admin/src/remote_invoke/terminal_frame.rs
```

CLI 侧先 mirror 同 shape，后续可抽共享 crate：

```text
crates/bifrost-cli/src/commands/remote_terminal.rs
```

结构：

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum TerminalFrame {
    Stdin { data_b64: String },
    Resize { cols: u16, rows: u16 },
    Signal { name: RemoteSignal },
    Eof,

    Ready { cols: Option<u16>, rows: Option<u16> },
    Output { data_b64: String },
    Error { message: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteSignal {
    Sigint,
    Sigterm,
    Sigkill,
}
```

不在 `TerminalFrame` 内定义 `Exit`。最终终态只由现有 relay `exit` 事件和 `post_call_exit()` 负责，避免 frame exit 与 relay exit 双终态竞争。

方向规则：

```text
CallerToClient: stdin / resize / signal / eof
ClientToCaller: ready / output / error
```

## 4. AAD 前置升级

当前 frame/exit 路径仍兼容无 AAD：

- Admin `encrypt_call_frame()` 使用 `encrypt_encrypted_payload_without_aad()`
- CLI frame decrypt 在 payload 无 AAD 时使用 `Aad::empty()`

PTY 交互帧必须使用 AAD 绑定 `call_id`、`seq`、`direction`、`frame_type`、`command_kind`、`grant_scope`，防止跨 call/跨方向/跨类型重放或重定向。

AAD 升级作为 PR 0，先于 PTY 代码合入：

1. Admin frame 加密改用 `encrypt_encrypted_payload()`。
2. CLI frame 解密校验相同 AAD shape。
3. 保持 exit payload 暂可继续无 AAD，除非同 PR 同步升级 CLI/Admin exit 路径。
4. 老的非 PTY stdout streaming 必须继续通过回归测试。

统一 helper：

```rust
fn terminal_aad(
    call_id: &str,
    seq: u64,
    direction: FrameDirection,
    frame_type: &str,
    grant_scope: GrantScope,
) -> EnvelopeAad {
    EnvelopeAad {
        version: 2,
        call_id: call_id.to_string(),
        seq,
        direction,
        token_hash: None,
        frame_type: Some(frame_type.to_string()),
        command_kind: Some(CommandKind::ShellExec),
        grant_scope: Some(grant_scope),
        sender_key_id: None,
        metadata: None,
    }
}
```

## 5. Relay 改造

### 5.1 `openCall()`

`packages/bifrost-sync-server/src/remote-invoke/service.ts`：

```ts
const commandKind = resolveCommandKind(req.command_kind);
if (!grantScopeAllowsCommand(grant.grant_scope, commandKind)) {
  throw new Error('grant_scope_mismatch');
}

const ptyEnabled = !!req.pty_enabled;
if (ptyEnabled && commandKind !== 'shell.exec') {
  throw new Error('pty_requires_shell_exec');
}
if (ptyEnabled && normalizeGrantScope(grant.grant_scope) !== 'remote_shell_interactive') {
  throw new Error('pty_requires_remote_shell_interactive');
}
```

### 5.2 `postCallerInput()`

caller input 只允许 PTY call：

```ts
const MAX_CALL_INPUT_ENVELOPE_BYTES = 128 * 1024;

async postCallerInput(callId: string, envelopeJson: string): Promise<void> {
  const call = await this.storage.remoteInvoke.getCall(callId);
  if (!call) throw new Error('call_not_found');
  if (call.status === 'completed' || call.status === 'failed' || call.status === 'cancelled') {
    throw new Error('call_already_ended');
  }

  const meta = this.callRuntimeMeta.get(callId);
  if (!meta?.ptyEnabled) {
    throw new Error('call_input_not_allowed');
  }
  if (Buffer.byteLength(envelopeJson, 'utf8') > MAX_CALL_INPUT_ENVELOPE_BYTES) {
    throw new Error('call_input_too_large');
  }

  if (call.status === 'authorized') {
    await this.storage.remoteInvoke.updateCall(callId, { status: 'streaming' });
  }

  pushToClient(call.client_instance_id, 'call_frame', { call_id: callId, envelope_json: envelopeJson });
}
```

### 5.3 route body 限制

`packages/bifrost-sync-server/src/routes/remote-invoke.ts` 的 `handleCallInput()` 在 parse body 前先挡大包：

```ts
const MAX_CALL_INPUT_BODY_BYTES = 192 * 1024;
if (Buffer.byteLength(ctx.body, 'utf8') > MAX_CALL_INPUT_BODY_BYTES) {
  sendError(ctx.res, 413, 'call input too large');
  return true;
}
```

relay 不 replay stdin。现有 caller event buffer 只用于 caller 侧 output 恢复，继续保持。

## 6. Admin 端 PTY runtime

### 6.1 文件变更

| 文件 | 动作 |
| --- | --- |
| `crates/bifrost-admin/Cargo.toml` | 新增 `portable-pty = "0.9"` |
| `crates/bifrost-admin/src/remote_invoke/mod.rs` | 新增 `pub mod terminal_frame; pub mod pty_session;` |
| `crates/bifrost-admin/src/remote_invoke/types.rs` | 扩展 `RemotePtyRequest`，新增 helper |
| `crates/bifrost-admin/src/remote_invoke/terminal_frame.rs` | 新建 typed PTY frame |
| `crates/bifrost-admin/src/remote_invoke/pty_session.rs` | 新建 PTY backend |
| `crates/bifrost-admin/src/remote_invoke/executor.rs` | 抽 `prepare_shell_exec()`，非 PTY 保持原路径 |
| `crates/bifrost-admin/src/remote_invoke/worker.rs` | active call 挂 PTY input sender，处理 `call_frame` |

### 6.2 `prepare_shell_exec()`

不要在 PTY backend 里重写 policy 校验。`executor.rs` 抽出 target 侧唯一 policy 准备逻辑：

```rust
pub struct PreparedShellExec {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: Option<String>,
    pub env: Vec<(String, String)>,
    pub timeout_ms: u64,
    pub policy_id: String,
    pub interactive_allowed: bool,
    pub stdin_allowed: bool,
}
```

规则：

- caller payload 中出现非空 `policy_id` 直接拒绝。
- `pty.enabled=true` 必须命中 `interactive_allowed=true` 的 policy。
- `stdin_mode=stream` 必须命中 `stdin_allowed=true` 的 policy。
- `output_mode=pty_merged` 是 PTY 必填。
- `remote_shell_exec` grant 只能执行非 PTY shell.exec。
- `remote_shell_interactive` grant 才能执行 PTY。

### 6.3 `pty_session.rs` 可执行线程模型

PTY session 必须拆成明确的四个角色，不能用单个 control loop 顺手 drain output：

```text
owner task:
  - 创建 PTY pair 和 child
  - 创建 input_rx / output_rx / waiter_rx
  - 启动 reader thread
  - 启动 writer/control thread
  - 启动 waiter thread
  - tokio select output / exit / cancel / timeout / relay backpressure

reader thread:
  - blocking read PTY master
  - output_tx.blocking_send(Vec<u8>)
  - 读到 EOF 后退出

writer/control thread:
  - blocking_recv PtyInput
  - Stdin -> writer.write_all
  - Resize -> master.resize
  - Sigint -> 写 0x03
  - Eof -> Unix 写 0x04；Windows 先按 ConPTY 兼容策略处理
  - Cancel -> kill child/process group，退出

waiter thread:
  - child.wait()
  - exit_tx.blocking_send(exit_code)
```

owner task 是唯一允许 await relay HTTP 的地方：

- output bytes -> `TerminalFrame::Output` -> `post_call_frame()`
- start ready -> `TerminalFrame::Ready` -> `post_call_frame()`
- PTY error -> `TerminalFrame::Error` -> `post_call_frame()`，随后 `post_call_exit(exit_code=-2)`
- child exit -> `post_call_exit(exit_code=code, bytes_in, bytes_out, duration_ms)`
- cancel/idle/timeout -> kill PTY，再 `post_call_exit()`，并移除 `active_calls`

资源清理要求：

- `RemoteInvokeWorker::stop()` 必须 mark cancelled、向 PTY session 发送 Cancel、abort task，并等待有限时间。
- drop writer 不能作为唯一 kill 机制；必须显式 kill child 或进程组。
- Unix 后续可加 process group kill；Windows 通过 ConPTY/child kill 起步，后续可加 Job Object。

### 6.4 ActiveCallControl

```rust
enum ActiveCallIo {
    None,
    Pty {
        input_tx: tokio::sync::mpsc::Sender<PtyInput>,
    },
}

struct ActiveCallControl {
    grant_id: String,
    started_at: u64,
    cancelled: AtomicBool,
    task: Mutex<Option<tokio::task::JoinHandle<()>>>,
    io: Mutex<ActiveCallIo>,
    last_caller_seq: AtomicU64,
    bytes_in: AtomicU64,
    bytes_out: AtomicU64,
    last_io_at: Mutex<std::time::Instant>,
    input_frame_count: AtomicU64,
    input_rate_window_start: Mutex<std::time::Instant>,
}
```

`send_pty_input()`：

- 检查 active call 是否存在。
- 检查 input frame rate。
- 更新 `last_io_at`。
- 发送到 PTY writer/control thread。
- 收到重复 seq 或旧 seq 时丢弃，但不能让乱序正常帧被误杀；见 CLI 有序发送队列。

### 6.5 worker 处理 `call_open`

```text
1. 解密 command
2. 校验 command.kind == transport command_kind
3. 校验 event.pty_enabled == command.pty_enabled()
4. shell.exec 进入 target policy 自动选择逻辑
5. PTY:
   - grant_scope 必须 remote_shell_interactive
   - stdin_mode 必须 stream
   - output_mode 必须 pty_merged
   - policy.interactive_allowed 必须 true
   - policy.stdin_allowed 必须 true
   - start PTY session, attach input_tx to active_calls
6. 非 PTY:
   - 保持现有 stdout streaming executor 路径
```

### 6.6 worker 处理 `call_frame`

```text
1. parse envelope
2. 校验 direction == CallerToClient
3. 校验 AAD call_id / seq / direction / frame_type / grant_scope
4. 解密 TerminalFrame
5. 校验 frame 类型属于 caller->client
6. 根据 frame 转 PtyInput
7. 写入 active PTY session
```

## 7. CLI 端 PTY runtime

### 7.1 命令形态

`crates/bifrost-cli/src/cli.rs`：

```rust
#[derive(Args, Clone, Debug)]
pub struct RemoteCommandShellArgs {
    #[arg(long, help = "Working directory on the remote host")]
    pub cwd: Option<String>,

    #[arg(long, default_value = "xterm-256color")]
    pub term: String,

    #[arg(long, help = "Shell command, default: target policy default shell")]
    pub command: Option<String>,

    #[arg(last = true)]
    pub argv: Vec<String>,
}

#[derive(Subcommand, Clone, Debug)]
pub enum RemoteCommandCommands {
    Exec(Box<RemoteCommandExecArgs>),
    Shell(Box<RemoteCommandShellArgs>),
}
```

入口为：

```bash
bifrost remote command shell
bifrost remote command shell --command "bash -l"
bifrost remote command shell --cwd /srv/app -- /bin/bash -l
```

不提供 `--policy`。

### 7.2 command envelope

CLI 需要给 `ShellExecPayload` / `CommandEnvelope` 增加 PTY 字段，但不得增加 caller-controlled `policy_id`。

```rust
#[derive(Debug, Serialize)]
struct RemotePtyRequest {
    enabled: bool,
    cols: Option<u16>,
    rows: Option<u16>,
    term: Option<String>,
}

#[derive(Debug, Serialize)]
struct ShellExecPayload {
    exec_mode: String,
    argv: Option<Vec<String>>,
    command_text: Option<String>,
    cwd: Option<String>,
    env: Option<BTreeMap<String, String>>,
    timeout_ms: Option<u64>,
    stdin_mode: Option<String>,
    pty: Option<RemotePtyRequest>,
    output_mode: Option<String>,
}
```

PTY command 示例：

```json
{
  "kind": "shell.exec",
  "exec_mode": "shell_text",
  "command_text": "bash -l",
  "cwd": null,
  "env": null,
  "timeout_ms": 0,
  "stdin_mode": "stream",
  "pty": {
    "enabled": true,
    "cols": 120,
    "rows": 40,
    "term": "xterm-256color"
  },
  "output_mode": "pty_merged"
}
```

### 7.3 open_call metadata

CLI 的 `OpenCallRequest` 增加：

```rust
pty_enabled: bool,
timeout_hint_ms: Option<u64>,
```

PTY shell 发送：

```json
{
  "command_kind": "shell.exec",
  "pty_enabled": true,
  "timeout_hint_ms": 0
}
```

非 PTY shell.exec 显式发送 `pty_enabled=false`。

### 7.4 本地有序发送队列

stdin pump 和 resize pump 不能各自直接 POST。共享 `AtomicU64` 只能保证 seq 唯一，不能保证网络到达顺序。

必须采用单一 ordered sender：

```text
stdin pump  \
resize pump  -> mpsc::Sender<OutboundTerminalFrame> -> ordered sender task -> POST /calls/{call_id}/input
cancel pump /
```

```rust
struct OutboundTerminalFrame {
    frame: TerminalFrame,
}

async fn ordered_sender(
    mut rx: mpsc::Receiver<OutboundTerminalFrame>,
    relay: CallerRelayClient,
    call_id: String,
    relay_token: String,
    session_key: [u8; 32],
) -> Result<(), String> {
    let mut seq = 1u64;
    while let Some(item) = rx.recv().await {
        post_terminal_frame(
            &relay,
            &call_id,
            &relay_token,
            &session_key,
            seq,
            item.frame,
            FrameDirection::CallerToClient,
        ).await?;
        seq += 1;
    }
    Ok(())
}
```

target 侧仍保留 `last_caller_seq` 防重复，但正常路径必须依赖 CLI 单队列保证顺序。

### 7.5 raw mode 与 escape

进入 raw mode 必须使用 guard：

```rust
struct RawModeGuard;

impl RawModeGuard {
    fn enter() -> std::io::Result<Self> {
        crossterm::terminal::enable_raw_mode()?;
        Ok(Self)
    }
}

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        let _ = crossterm::terminal::disable_raw_mode();
    }
}
```

`~.` 语义：

- `~.` 不是只退出本地 stdin pump。
- `~.` 必须调用现有 cancel endpoint：`POST /v4/remote-invoke/calls/{call_id}/cancel`。
- CLI 发送 cancel 后恢复 raw mode，等待 relay `exit` / `cancelled` 终态一个短窗口；超时后本地退出但打印远端终止状态未知。
- worker 收到 `call_cancel` 后必须 kill PTY child/process group，写 Recent Calls 终态。

### 7.6 output loop

PTY frame 分支只处理：

```rust
match decrypt_terminal_frame(event.envelope_json, &session_key)? {
    TerminalFrame::Output { data_b64 } => stdout.write_all(&decode(data_b64)?)?,
    TerminalFrame::Ready { .. } => {}
    TerminalFrame::Error { message } => eprintln!("{message}"),
    other => return Err(format!("unexpected caller frame: {other:?}")),
}
```

最终 exit code 来自 relay `exit` event 或现有 call result，不来自 `TerminalFrame::Exit`。

## 8. Backpressure、断线和 timeout

### 8.1 caller 断线

- caller SSE 断开后，relay 可继续 buffer output。
- caller reconnect 后只恢复 output，不 replay stdin。
- reconnect 后 CLI 立即发送一次 `Resize`。

### 8.2 target relay SSE 短断

- target admin 进程仍在：PTY session 可继续运行。
- output HTTP 失败进入 per-call 有界缓冲，默认 1 MiB。
- 缓冲超过 1 MiB 或连续发送失败超过 60 秒，kill PTY 并 `post_call_exit(exit_code=-2)`。

### 8.3 idle timeout

默认 idle timeout：30 分钟。

`last_io_at` 在这些路径更新：

- 收到 caller input / resize / signal / eof
- 成功发送 output frame
- 成功发送 ready/error frame

worker reconcile loop 每 30 秒检查 PTY active call，超时则 cancel PTY 并发送终态。

### 8.4 frame 大小和速率

建议初始值：

```text
PTY output chunk: 8 KiB
caller stdin chunk: 4 KiB
caller input envelope max: 128 KiB
route body max: 192 KiB
per-call output buffer: 1 MiB
caller input rate: 1000 frames/sec
```

## 9. PR 拆分

### PR 0：AAD 统一升级

改动：

- `crates/bifrost-admin/src/remote_invoke/types.rs`
- `crates/bifrost-admin/src/remote_invoke/worker.rs`
- `crates/bifrost-cli/src/commands/remote.rs`

验证：

- 单元测试：frame AAD roundtrip、错误 AAD 拒绝、旧无 AAD 兼容按最终决策覆盖。
- E2E：现有 non-PTY stdout streaming 继续通过。
- human_tests：更新并执行 `human_tests/remote-shell-exec.md` 的 non-PTY streaming 回归。
- 收尾：`cargo test --workspace --all-features`，再执行 `rust-project-validate`。

### PR 1：协议扩展和 relay gate

改动：

- `RemotePtyRequest` 扩字段
- `TerminalFrame` 类型
- relay `pty_enabled` scope gate
- caller input body/envelope size limit
- caller input only for PTY call

验证：

- 单元测试：`remote_shell_exec` grant + `pty_enabled=true` 被 relay 拒绝；`remote_shell_interactive` 允许。
- E2E：open_call metadata 与 relay error code。
- human_tests：新增 `TC-RSE-PTY-01`，验证 read-only / shell_exec grant 不能打开 PTY。
- 收尾：`cargo test --workspace --all-features`，再执行 `rust-project-validate`。

### PR 2：admin PTY session

改动：

- `portable-pty`
- `pty_session.rs`
- `prepare_shell_exec()`
- `ActiveCallControl`
- `call_frame` decrypt/route
- cancel/idle/backpressure

验证：

- 单元测试：
  - PTY policy gate
  - call_frame decrypt direction 校验
  - duplicate seq 丢弃
  - cancel kills PTY
  - idle timeout kills PTY
- E2E：
  - Unix PTY command 输出
  - resize frame 路由
  - cancel 后进程终止
- human_tests：新增并执行 `TC-RSE-PTY-02` 到 `TC-RSE-PTY-05`。
- 收尾：`cargo test --workspace --all-features`，再执行 `rust-project-validate`。

### PR 3：CLI interactive shell

改动：

- `RemoteCommandCommands::Shell`
- `remote_terminal.rs`
- ordered sender queue
- raw mode guard
- `~.` cancel
- output bytes writer

验证：

- 单元测试：
  - command envelope 不含 `policy_id`
  - `bifrost remote shell ...` 仍是 policy 管理入口
  - `bifrost remote command shell` 是交互入口
  - ordered sender seq 单调递增
  - escape detector `~.` 触发 cancel action
- E2E：
  - `bifrost remote command shell --command 'echo hello; exit'`
  - stdin 输入 `echo ok`
  - Ctrl-C 回到远端 prompt
  - resize 后远端 `stty size` 变化
  - `~.` 后远端 PTY 被 kill
- human_tests：执行所有 PTY 用例并更新结果表。
- 收尾：`cargo test --workspace --all-features`，再执行 `rust-project-validate`。

## 10. human_tests 计划

实现 PR 必须更新：

```text
human_tests/remote-shell-exec.md
human_tests/readme.md
```

新增用例：

| 用例 | 验证点 |
| --- | --- |
| `TC-RSE-PTY-01` | `remote_shell_exec` grant 打开 PTY 被拒绝，`remote_shell_interactive` 才允许 |
| `TC-RSE-PTY-02` | `bifrost remote command shell --command 'echo hello; exit'` 经真实 relay 返回输出和 exit |
| `TC-RSE-PTY-03` | 交互 stdin 输入 `echo ok`，caller 看到 PTY merged output |
| `TC-RSE-PTY-04` | resize 后远端 `stty size` 变化 |
| `TC-RSE-PTY-05` | Ctrl-C 只发送远端 Sigint，不杀本地 CLI |
| `TC-RSE-PTY-06` | `~.` 触发 caller cancel，target PTY child 被终止，Recent Calls 收敛到 cancelled/failed terminal state |
| `TC-RSE-PTY-07` | caller 断线重连只恢复 output，不 replay stdin |
| `TC-RSE-PTY-08` | 大输出触发 backpressure 上限后 session 被终止且无内存无限增长 |
| `TC-RSE-PTY-09` | command envelope 不含 `policy_id`，伪造 `policy_id` 仍被 target 拒绝 |
| `TC-RSE-PTY-10` | `bifrost remote shell list` 仍是 policy 管理，`bifrost remote command shell` 才是交互入口 |

执行约束：

- 所有测试代理必须使用临时 `BIFROST_DATA_DIR`。
- 测试禁止使用 9900。
- 启动 Bifrost 时必须带 `--no-system-proxy`，除非测试目标就是系统代理。
- human_tests 文档更新后必须立即按用例逐条真实执行，记录实际结果。

## 11. 最终完成门禁

每个实现 PR 完成前必须满足：

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
```

涉及 E2E 的 PR 还必须先执行对应 e2e-test 套件，再执行 rust-project-validate：

```bash
bash e2e-tests/tests/test_remote_shell_exec_streaming_e2e.sh
# 新增后：
bash e2e-tests/tests/test_remote_shell_exec_pty_e2e.sh
```

最终收尾：

```text
1. human_tests/remote-shell-exec.md 已更新
2. human_tests/readme.md 已更新
3. human_tests 中本次新增/修改用例已逐条真实执行
4. E2E 已通过
5. cargo test --workspace --all-features 已通过
6. rust-project-validate 已通过
7. 临时数据目录已清理
```

## 12. 关键风险闭环

| 风险 | 方案闭环 |
| --- | --- |
| `remote shell` 命令名冲突 | 交互入口改为 `remote command shell`，现有 `remote shell` 继续管理 policy |
| caller 重新引入 `policy_id` | CLI payload 不含 `policy_id`，target 继续拒绝伪造字段 |
| stdin/resize 并发 POST 乱序 | CLI 单一 ordered sender task 分配 seq 并发送 |
| `~.` 只断本地不杀远端 | `~.` 调用 cancel endpoint，worker cancel kill PTY |
| PTY session loop 挂住 | reader/writer/waiter/owner 明确分工，owner 统一 post frame/exit |
| 双终态竞争 | `TerminalFrame` 不含 Exit，relay `exit` 是唯一终态 |
| 验证计划不满足仓库规则 | 每个 PR 都包含 unit/E2E/human_tests/rust-project-validate 门禁 |
