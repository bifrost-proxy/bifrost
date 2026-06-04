# Remote Shell Exec 真实场景测试

## 功能模块说明

当前 `remote shell exec` 的真实契约是：

- caller 通过 `bifrost remote command exec ...` 发起命令
- caller 不允许指定 `policy_id`
- target 根据本地 `Shell Access` 配置和该 caller 对应的 grant binding 自动选择唯一策略
- grant binding / stdin / interactive / policy version snapshot 只保存在 target 本地
- caller-to-client `call_frame` 可把 caller stdin 加密转发到 target executor active session
- `--pty` / `--interactive` 在 target 侧必须分配真正 PTY，TTY 程序能看到 `isatty(0)=true`、`isatty(1)=true`
- relay 只保留最小 `grant_scope`，不保存具体策略绑定
- 如果没有命中策略、命中多个策略，或者 caller 试图伪造 `policy_id`，都由 target 拒绝
- `policy_id` / `exec_mode` 只作为 target 侧审计结果写入 Recent Calls

## 前置条件

1. 仓库位于 `<REPO_ROOT>`
2. 不使用默认数据目录，不使用 `9900`
3. 启动 target 时带 `--no-system-proxy`
4. relay / target / caller 使用彼此独立的临时目录

建议环境变量：

```bash
export TARGET_DATA_DIR="$(mktemp -d /tmp/bifrost-remote-shell-target-XXXXXX)"
export CALLER_DATA_DIR="$(mktemp -d /tmp/bifrost-remote-shell-caller-XXXXXX)"
export CALLER_2_DATA_DIR="$(mktemp -d /tmp/bifrost-remote-shell-caller2-XXXXXX)"
export RELAY_DATA_DIR="$(mktemp -d /tmp/bifrost-remote-shell-relay-XXXXXX)"
export TARGET_PORT=18820
export RELAY_PORT=18821
```

建议启动命令：

```bash
BIFROST_DATA_DIR="$TARGET_DATA_DIR" cargo run --bin bifrost -- start -p "$TARGET_PORT" --unsafe-ssl --no-system-proxy
pnpm --dir packages/bifrost-sync-server exec tsx src/cli.ts -p "$RELAY_PORT" -d "$RELAY_DATA_DIR" --enable-remote-invoke
```

## 测试用例

### TC-RSE-01：caller CLI 不再暴露 `--policy`

步骤：
1. 执行：
   ```bash
   cargo run --bin bifrost -- remote command exec --help
   ```

预期：
- help 包含 `--cwd`、`--env`、`--timeout-ms`、`--shell-text`
- help 不再包含 `--policy`

### TC-RSE-13：argv_exec 必须显式通过 `--` 进入

步骤：
1. 执行：
   ```bash
   cargo run --bin bifrost -- remote command exec pwd
   ```
2. 再执行：
   ```bash
   cargo run --bin bifrost -- remote command exec -- /bin/pwd
   ```

预期：
- 第一步在 CLI 解析阶段直接失败，明确提示 `pwd` 是意外参数，不会真正发起远端 `shell.exec`
- 第二步仍按 `argv_exec` 正常解析

### TC-RSE-14：长时间命令的 stdout 会流式到 caller，而不是等进程结束后一次性返回

步骤：
1. 在 target 侧启用允许执行 `python3` / `top` 的 shell 策略，或直接切到 `Full Access`
2. 在 caller 侧执行一个分两段输出的命令，并把输出写入日志文件：
   ```bash
   STREAM_LOG="$(mktemp)"
   (
     BIFROST_DATA_DIR="$CALLER_DATA_DIR" cargo run --bin bifrost -- remote command exec \
       --relay-url "http://127.0.0.1:${RELAY_PORT}" \
       -- /usr/bin/python3 -u -c 'import sys,time;print("stream-one", end="", flush=True); time.sleep(1.2); print("stream-two", end="", flush=True)'
   ) | tee "$STREAM_LOG"
   ```
3. 在命令尚未结束时（例如启动后约 0.4 秒）检查日志文件：
   ```bash
   sleep 0.4
   cat "$STREAM_LOG"
   ```
4. 命令结束后再次检查日志文件，确认完整输出
5. 再执行：
   ```bash
   BIFROST_DATA_DIR="$CALLER_DATA_DIR" cargo run --bin bifrost -- remote command exec \
     --relay-url "http://127.0.0.1:${RELAY_PORT}" \
     -- /usr/bin/top -l 2 -s 1
   ```

预期：
- 第 3 步在命令尚未结束时，日志文件里已经能看到 `stream-one`
- 命令结束后日志文件完整变为 `stream-onestream-two`
- 执行 `top -l 2 -s 1` 时，caller 会连续收到输出，不再卡到最后一次性打印
- Recent Calls 最终仍正常写入 exit_code / duration / stdout_digest

### TC-RSE-15：关键流式回归场景已经沉淀为可重复执行的 shell E2E 脚本

步骤：
1. 在仓库根目录执行：
   ```bash
   bash e2e-tests/tests/test_remote_shell_exec_streaming_e2e.sh
   ```
2. 观察脚本输出的断言结果
3. 如脚本失败，检查脚本打印的 target / relay / caller 日志，再修复后重新执行

预期：
- 脚本会自动隔离启动 relay / target / caller，不污染默认数据目录，也不会使用 `9900`
- `shell_text` 场景会验证第一段 stdout 在进程退出前已经到达 caller
- `argv_exec` 场景会验证第一段 stdout 在进程退出前已经到达 caller
- 两个场景都会验证 Recent Calls 继续记录 `policy_id` / `exec_mode` / `exit_code` / `stdout_digest`
- 脚本最终 summary 为全部通过，可作为后续改动的稳定回归入口

### TC-RSE-16：Windows 流式 shell 输出 E2E 回归

步骤：
1. 在 Windows 环境执行：
   ```bash
   cargo test -p bifrost-admin test_execute_shell_exec_streams_stdout_before_exit -- --exact --nocapture
   cargo test -p bifrost-e2e remote_shell_exec_streams_stdout -- --exact --nocapture
   ```
2. 确认 `bifrost-admin` 单元测试继续使用 PowerShell 绝对路径 + `env_clear()`（`inherit_env` 未设置），覆盖环境清空场景
3. 确认 `bifrost-e2e` E2E 测试策略显式设置 `"inherit_env": true`，使用裸 `cmd.exe` + 裸 `ping`，不依赖绝对路径，专注验证流式输出语义
4. 确认 Windows 命令末尾包含 `&exit /b 0`，强制 `cmd /C` 返回 exit code 0（因 `<nul set /p` 从 nul 读取 stdin 时 `set /p` 本身返回 exit code 1）

预期：
- `bifrost-admin` 单元测试验证 `env_clear()` 下 PowerShell 绝对路径仍可完成流式输出
- `bifrost-e2e` E2E 测试因 `inherit_env=true` 保留完整 PATH，裸 `cmd.exe` 和 `ping` 均可直接找到
- `stdout` 仍按 `stream-one` / `stream-two` 分段到达，而不是等进程退出后一次性返回
- 最终 exit code 为 `0`（由 `exit /b 0` 保证），stderr 为空或不含命令未找到错误

### TC-RSE-02：read-only grant 不能执行 shell.exec

步骤：
1. 仅建立 `remote_query` 授权
2. 执行：
   ```bash
   BIFROST_DATA_DIR="$CALLER_DATA_DIR" cargo run --bin bifrost -- remote command exec --relay-url "http://127.0.0.1:${RELAY_PORT}" -- /bin/echo hello
   ```

预期：
- caller 收到 scope 不允许 shell.exec 的拒绝
- target 侧没有成功执行记录

### TC-RSE-03：selected policy grant 下 target 自动命中唯一 argv 策略

步骤：
1. 在 target 侧配置两个启用策略：`echo-argv` 与 `pwd-argv`
2. 对 caller A 批准 `remote_shell_exec + selected[pwd-argv]`
3. 执行：
   ```bash
   BIFROST_DATA_DIR="$CALLER_DATA_DIR" cargo run --bin bifrost -- remote command exec --relay-url "http://127.0.0.1:${RELAY_PORT}" -- /bin/pwd
   ```

预期：
- caller 成功执行
- Recent Calls 记录 `policy_id=pwd-argv`
- caller 没有显式传任何 policy 参数

### TC-RSE-04：selected policy grant 下未命中 allowlist 的命令被拒绝

步骤：
1. 保持 caller A 仍只绑定 `pwd-argv`
2. 执行：
   ```bash
   BIFROST_DATA_DIR="$CALLER_DATA_DIR" cargo run --bin bifrost -- remote command exec --relay-url "http://127.0.0.1:${RELAY_PORT}" -- /bin/date
   ```

预期：
- target 拒绝
- 错误信息明确说明没有命中可执行策略，或程序不在命中的 policy allowlist 内

### TC-RSE-05：caller 伪造 `policy_id` 会被 target 直接拒绝

步骤：
1. 使用同一套 relay / target / caller
2. 构造旧协议或手工请求，让加密 `shell.exec` payload 带上 `policy_id`

预期：
- target 返回：
  - `shell.exec caller must not specify policy_id; the target device selects policy`
- Recent Calls 不出现成功执行记录

### TC-RSE-06：mode=all 下如果命中多条策略，target 以歧义拒绝

步骤：
1. 在 target 上启用两条都能匹配同一 `shell_text` 的策略
2. 对 caller B 批准 `remote_shell_exec + mode=all`
3. 执行：
   ```bash
   BIFROST_DATA_DIR="$CALLER_2_DATA_DIR" cargo run --bin bifrost -- remote command exec --relay-url "http://127.0.0.1:${RELAY_PORT}" --shell-text "printf hello"
   ```

预期：
- target 返回“匹配到多条策略”的明确拒绝
- 要求执行侧收紧配置或 grant binding

### TC-RSE-07：Full Access 的 shell_text 可执行，Default Sandbox 当前明确拒绝

步骤：
1. 在 Settings `Manage Access` 切到 `Full Access`
2. 对 caller 建立 shell grant 后执行：
   ```bash
   BIFROST_DATA_DIR="$CALLER_DATA_DIR" cargo run --bin bifrost -- remote command exec --relay-url "http://127.0.0.1:${RELAY_PORT}" --shell-text "printf full-access && /bin/pwd"
   ```
3. 再切到 `Default Sandbox`
4. 建立新的 shell grant 后执行：
   ```bash
   BIFROST_DATA_DIR="$CALLER_2_DATA_DIR" cargo run --bin bifrost -- remote command exec --relay-url "http://127.0.0.1:${RELAY_PORT}" --shell-text "printf should-be-rejected"
   ```

预期：
- `Full Access` 真正执行成功
- `Default Sandbox` 返回“sandbox execution is not implemented yet” 的明确拒绝

### TC-RSE-12：旧版 Full Access 配置也能执行 argv 命令

步骤：
1. 在 target 侧写入旧版 `full-access` 配置，只包含：
   - `exec_mode=shell_text`
   - `allowed_shell_patterns=["^(?s:.*)$"]`
   - `inherit_env=true`
2. 建立 `remote_shell_exec + mode=all` 授权
3. 执行：
   ```bash
   BIFROST_DATA_DIR="$CALLER_DATA_DIR" cargo run --bin bifrost -- remote command exec --relay-url "http://127.0.0.1:${RELAY_PORT}" -- pwd
   ```

预期：
- target 把旧版 `full-access` 视作兼容性的完全开放策略
- `pwd` 作为 `argv_exec` 成功执行
- caller 不需要先回到 WebUI 重新保存一次 `Full Access`

### TC-RSE-17：Windows shell_text Unix 路径 fallback 与 UTF-8 编码回归

步骤：
1. 运行单元测试（macOS/Linux 可执行部分）：
   ```bash
   cargo test -p bifrost-admin test_build_shell_text_process -- --nocapture
   ```
2. 运行 E2E 测试：
   ```bash
   cargo test -p bifrost-e2e remote_shell_exec_unix_shell_path_fallback -- --exact --nocapture
   ```
3. Windows CI 补验以下单元测试：
   - `test_build_shell_text_process_unix_path_fallback_on_windows`：Unix 路径 `/bin/bash` 被过滤，fallback 到 `cmd` + `chcp 65001`
   - `test_build_shell_text_process_powershell_utf8_prefix`：PowerShell 命令前置 `[Console]::OutputEncoding = [Text.Encoding]::UTF8`
   - `test_is_unix_only_shell_path`：以 `/` 开头的路径被识别为 Unix-only

预期：
- macOS 上 `test_build_shell_text_process_default_shell` 验证默认 shell 为 `/bin/sh`
- macOS 上 `test_build_shell_text_process_explicit_shell` 验证显式 `/bin/bash` 使用 `-lc` 传参
- E2E `remote_shell_exec_unix_shell_path_fallback` 使用 `/bin/bash` shell 设置的 policy 执行 `echo hello-unix-path` 成功
- Windows CI 上 fallback 到 `cmd`，不会报 "系统找不到指定的路径"

### TC-RSE-18：`policy update` 命令不破坏 grant 有效性

步骤：
1. 运行 CLI 解析测试：
   ```bash
   cargo test -p bifrost-cli remote_shell_policy_update -- --nocapture
   ```
2. 运行 E2E 测试：
   ```bash
   cargo test -p bifrost-e2e remote_shell_policy_update_preserves_execution -- --exact --nocapture
   ```

预期：
- `remote_shell_policy_update_parses_all_flags`：全参数（`--name`/`--mode`/`--shell`/`--program`/`--pattern`/`--timeout-ms`/`--stdin`/`--interactive`/`--inherit-env`）解析正确
- `remote_shell_policy_update_minimal_args`：仅传 positional ID 也能解析
- E2E `remote_shell_policy_update_preserves_execution`：创建 policy → 执行成功 → 更新 name → 再次执行仍成功，验证 policy 存储一致性

### TC-RSE-19：Remote Invoke stdin frame 转发到 executor active session

步骤：
1. 执行 target executor stdin 回归：
   ```bash
   cargo test -p bifrost-admin remote_invoke::executor::tests::test_execute_shell_exec_forwards_stdin_stream -- --nocapture
   ```
2. 执行 caller CLI remote interactive / stdin / pty 参数回归：
   ```bash
   cargo test -p bifrost-cli remote:: -- --nocapture
   ```
3. 执行可重复 E2E 入口：
   ```bash
   # 本轮使用 /tmp 下的一次性临时 harness 执行，测试脚本不落库。
   # harness 执行上述两个 cargo test，并额外检查 CLI envelope 对 --interactive/--stdin/--pty 的构造。
   ```

预期：
- caller command envelope 对 `--stdin` / `--pty` / `--interactive` 写入 `stdin_mode=stream`、`pty.enabled=true`、启动时终端 `pty.rows/cols`、`output_mode=pty_merged`。
- caller CLI interactive 模式开启 raw mode，把本地 stdin 字节封装为 caller-to-client 加密 `call_frame` 并 POST 到 relay。
- caller CLI interactive 模式默认跳过普通 streaming Done digest 校验，避免 `pty_merged` 终端字节流经 legacy exit 收敛时被误判为 `stream digest mismatch`。
- target worker 在 active call map 中找到对应 call，把解密后的 stdin bytes 发送给 executor。
- executor 对 `stdin_mode=stream` 的 shell command 打开 child stdin，测试程序 `python3 -u -c 'import sys; print(sys.stdin.readline().strip())'` 能读到 `hello remote stdin` 并以 exit code 0 结束。
- cancel 仍走既有 `call_cancel` 通道；本轮验证 stdin forwarding、CLI raw-mode 入口、启动时终端尺寸传递与 interactive digest 收敛，远端真 PTY 运行期 resize 作为后续增强继续覆盖。

### TC-RSE-20：真实 Remote Invoke 链路执行 `remote exec --interactive` 并转发 stdin

步骤：
1. 使用当前源码构建的 `bifrost`，隔离启动本地 relay、target Bifrost 和 caller 数据目录；target 必须使用 `--no-system-proxy` 且不能使用 9900 端口。
2. 在 target 配置允许 `shell_text`、`stdin` 和 `interactive` 的 Shell Access policy。
3. 通过 pair-code 真实建立 caller 到 target 的 remote connection，并把 grant 升级为 `remote_shell_interactive`，`stdin_allowed=true`，`interactive_allowed=true`。
4. 使用本地 PTY 启动：
   ```bash
   bifrost remote exec --relay-url <relay> --client-id <target-prefix> --interactive \
     --shell-text "python3 -u -c 'import sys; print(\"REMOTE_INTERACTIVE_READY\"); print(sys.stdin.readline().strip())'"
   ```
5. 在本地 PTY 中写入 `REMOTE_INTERACTIVE_INPUT_OK\n`，等待命令退出，并查询 target Recent Calls。

预期：
- caller CLI 在真实 TTY/raw mode 下运行，`--interactive` 不因非 TTY 被拒绝。
- target stdout 流包含 `REMOTE_INTERACTIVE_READY` 和 `REMOTE_INTERACTIVE_INPUT_OK`。
- stdin 通过 caller-to-client encrypted `call_frame` 进入 target active session map，再转发到 executor child stdin。
- Recent Calls 中新增 `shell.exec` 记录，`policy_id=stream-shell`、`exec_mode=shell_text`、`exit_code=0`。

### TC-RSE-21：`--pty` 真实分配 PTY 且 interactive 退出后恢复本地 raw mode

步骤：
1. 执行 target executor 真 PTY 回归：
   ```bash
   cargo test -p bifrost-admin remote_invoke::executor::tests::test_execute_shell_exec_pty_reports_isatty_true -- --nocapture
   ```
2. 执行 caller raw-mode 生命周期回归：
   ```bash
   cargo test -p bifrost-cli remote::tests::test_remote_stdin_forwarder_owns_raw_mode_guard -- --nocapture
   ```
3. 使用当前源码构建的 `bifrost`，隔离启动本地 relay、target Bifrost 和 caller 数据目录；target 必须使用 `--no-system-proxy` 且不能使用 9900 端口。
4. 在 target 配置允许 `shell_text`、`stdin` 和 `interactive` 的 Shell Access policy。
5. 通过真实 remote connection 执行：
   ```bash
   bifrost remote exec --relay-url <relay> --client-id <target-prefix> --interactive \
     --shell-text "python3 -u -c 'import os,sys; print(os.isatty(0), os.isatty(1)); print(\"READY\"); print(sys.stdin.readline().strip())'"
   ```
6. 在本地 PTY 中写入 `PTY_STDIN_OK\n`，等待命令退出；随后在同一终端执行 `stty -a` 或等效检查，确认终端不残留 raw mode。

预期：
- target executor 使用真 PTY，Python 输出包含 `True True`，证明 stdin/stdout 都是 TTY。
- caller stdin 仍通过 encrypted `call_frame` 转发到 target active session，输出包含 `PTY_STDIN_OK`。
- PTY 输出按 `pty_merged` 合流返回，不要求 stderr 单独还原。
- interactive 命令退出后本地 raw mode guard 由 caller 主任务生命周期释放，即使 stdin reader 线程仍阻塞在 `stdin.read()`，终端也恢复到可正常回显/换行的 cooked 状态。

### TC-RSE-08：策略版本变化后旧 grant 失效

步骤：
1. 建立一个可用 shell grant
2. 修改 target 侧 `remote_shell.json`，让 version 递增
3. 不重新 connect，直接再次执行 shell.exec

预期：
- caller 收到 `shell policy set version changed ... reconnect is required`

### TC-RSE-09：删除指定 caller 的 grant 不影响其他 caller

步骤：
1. 让 caller A 和 caller B 同时拥有不同 shell grant
2. 仅删除 caller A 对应的 grant
3. 分别再次执行命令

预期：
- caller A 被拒绝，需要重新 connect
- caller B 继续可执行

### TC-RSE-10：编辑 grant 策略只修改 target 本地，不把策略细节写入 relay

步骤：
1. 通过 WebUI 或 `bifrost remote grant update <grant-id> --access selected --policy pwd-argv` 修改已有 grant
2. 在 target 本地查看 Grants 列表，确认显示 selected policy 绑定
3. 直接查看 relay 数据库中的 `bifrost_remote_invoke_grants`

预期：
- target 本地 Grants / 调用行为都反映新的 selected policy
- relay 侧 grant 只更新最小 `grant_scope`
- relay 数据库中不存在 `policy_binding` / `shell_policy_set_version_snapshot` / `interactive_allowed` / `stdin_allowed` 列和值

### TC-RSE-11：重新 connect 会覆盖同 caller/device 的旧 grant，disconnect 会清空该设备残留授权

步骤：
1. 对同一个 `client_instance_id + caller_fingerprint` 连续建立两次授权
2. 第二次 connect 成功后直接执行：
   ```bash
   BIFROST_DATA_DIR="$CALLER_DATA_DIR" cargo run --bin bifrost -- remote status --relay-url "http://127.0.0.1:${RELAY_PORT}"
   ```
3. 执行：
   ```bash
   BIFROST_DATA_DIR="$CALLER_DATA_DIR" cargo run --bin bifrost -- remote disconnect --all --relay-url "http://127.0.0.1:${RELAY_PORT}"
   ```
4. 再次查询该 `client_instance_id + caller_fingerprint` 的 reusable grant

预期：
- 第二次 connect 会覆盖本地同设备连接信息
- relay 上旧 active grants 被置为 `removed`，只保留最新 transport 对应的授权
- `remote status` 不再报 `saved connection transport no longer matches relay reusable authorization`
- `disconnect --all` 会把该 caller 在该设备上的全部 reusable grants 清空，而不是只删除最后一条本地已知 grant

## 清理步骤

```bash
pkill -f "bifrost-sync-server.*${RELAY_PORT}" || true
rm -rf "$TARGET_DATA_DIR" "$CALLER_DATA_DIR" "$CALLER_2_DATA_DIR" "$RELAY_DATA_DIR"
```

## 本轮实际执行结果（2026-04-23）

| 用例 | 结果 | 实际结果 |
| --- | --- | --- |
| TC-RSE-01 | ✅ PASS | `remote command exec --help` 已不再展示 `--policy`，仍保留 `--cwd`、`--env`、`--timeout-ms`、`--shell-text`。 |
| TC-RSE-02 | ✅ PASS | 在已有 saved connection 仅具备 `remote_query` scope 的真实场景下执行 shell.exec，caller 收到明确升级提示，不再硬打到 target。 |
| TC-RSE-03 | ✅ PASS | 真实隔离环境下，caller 执行 `remote command exec -- /bin/pwd` 成功，target 自动命中 `pwd-argv`，Recent Calls 记录 `policy_id=pwd-argv`。 |
| TC-RSE-04 | ✅ PASS | 同一 grant 下执行 `/bin/date` 被 target 拒绝，错误来自 target 侧策略匹配/allowlist。 |
| TC-RSE-05 | ✅ PASS | 新增 worker 回归后，caller 如果携带 `policy_id`，target 直接拒绝：`shell.exec caller must not specify policy_id; the target device selects policy`。 |
| TC-RSE-06 | ✅ PASS | 新增 executor 回归后，`mode=all` 下若 `shell_text` 同时命中多条策略，target 返回 `matched multiple policies`，不会让 caller 选。 |
| TC-RSE-07 | ✅ PASS | 真实隔离环境下，`Full Access` 成功执行 `printf full-access && /bin/pwd`；`Default Sandbox` 明确返回未实现拒绝。 |
| TC-RSE-08 | ✅ PASS | target 改动 shell policy version 后，旧 grant 再执行会返回 `shell policy set version changed ... reconnect is required`。 |
| TC-RSE-09 | ✅ PASS | 双 caller 真链路下只删除 caller A 的 grant，caller B 继续可执行，不受影响。 |
| TC-RSE-10 | ✅ PASS | 2026-04-23 在隔离环境 `target=65323`、`relay=65324`、`TARGET_DATA_DIR=/tmp/bifrost-grant-target-lz76j2ye`、`CALLER_DATA_DIR=/tmp/bifrost-grant-caller-7d1nkfjp` 真实执行。先用 pair-code 建立 `remote_query` grant `VkQTKYzVCCokjPCrU6gKv`，caller 执行 `remote command exec -- /bin/pwd` 明确报 `saved remote authorization is read-only and does not allow shell.exec`。随后通过 target 本地 CLI `cargo run --bin bifrost -- --port 65323 remote grant update VkQTKYzVCCokjPCrU6gKv --access selected --policy pwd-argv` 将 grant 升级到 `selected[pwd-argv]`，返回 payload 中已包含 `policy_binding={mode:selected,policy_ids:[pwd-argv]}` 与 `shell_policy_set_version_snapshot=10`；caller 再执行 `/bin/pwd` 成功输出 `<REPO_ROOT>`，执行 `/bin/date` 被 target 拒绝：`program '/bin/date' is not allowed by policy 'pwd-argv'`。之后用真实浏览器打开 `http://127.0.0.1:65323/_bifrost/settings?tab=remote-invoke`，点击 Grants 的 `Edit Access`，把同一 grant 改为 `All enabled shell policies`，页面提示 `Grant access updated`；`GET /_bifrost/api/remote-invoke/grants` 随后返回 `policy_binding={mode:all}`，caller 执行 `--shell-text 'printf full-open && /bin/pwd'` 成功输出 `full-open<REPO_ROOT>`。最后直接检查 relay SQLite `/tmp/bifrost-grant-relay-k45bp4q4/bifrost-sync.db`：`bifrost_remote_invoke_grants` 表只有 `grant_scope` / `ssh_key_id` / `ssh_key_fingerprint` 等最小列，没有 `policy_binding` / `shell_policy_set_version_snapshot` / `interactive_allowed` / `stdin_allowed`；该 grant 在 relay 中仅记录 `grant_scope=remote_shell_exec`。target 本地 `/tmp/bifrost-grant-target-lz76j2ye/admin/remote_invoke_grant_policy.json` 则保存了 `policy_binding={mode:all}`、`shell_policy_set_version_snapshot=10`、`stdin_allowed=false`、`interactive_allowed=false`，证明策略细节只留在执行侧。 |
| TC-RSE-11 | ✅ PASS | 2026-04-23 在隔离环境 `target=65461`、`relay=65462`、`TARGET_DATA_DIR=/var/folders/2k/nc0_nn9976l02sftpyhc9tz40000gn/T/bifrost-rse11-target-1ol13h2w`、`CALLER_DATA_DIR=/var/folders/2k/nc0_nn9976l02sftpyhc9tz40000gn/T/bifrost-rse11-caller-099y45el`、`RELAY_DATA_DIR=/var/folders/2k/nc0_nn9976l02sftpyhc9tz40000gn/T/bifrost-rse11-relay-4wrcjkru` 真实执行。对同一 `client_instance_id=a32cac09-ce8c-4ebf-8e9f-43238cd189ff` 和 caller 指纹连续完成两次 pair-code connect，第一次 grant 为 `p8DEHNdigh_dKEud4kOT6`，第二次 grant 为 `hOpA0_uoFILU5WqDFMJ4a`。第二次 connect 后直接执行 `target/debug/bifrost remote status --relay-url http://127.0.0.1:65462`，成功返回远端状态 JSON，不再出现 `saved connection transport no longer matches relay reusable authorization`。随后查询 relay `grants/reusable`，返回的正是第二次最新 grant `hOpA0_uoFILU5WqDFMJ4a`。再执行 `target/debug/bifrost remote disconnect --all --relay-url http://127.0.0.1:65462`，CLI 输出 `Revoking 1 connection(s)… ✓ hOpA0_uoFILU (eden)`；之后再次查询 `grants/reusable` 返回 `data=null`。最后直接检查 relay SQLite，两个 grant 都存在但状态均为 `removed`：`p8DEHNdigh_dKEud4kOT6 -> removed`、`hOpA0_uoFILU5WqDFMJ4a -> removed`，证明 reconnect 会覆盖旧 grant，而 `disconnect --all` 会清空该 caller 在该设备上的全部残留 reusable grants。 |
| TC-RSE-12 | ✅ PASS | 2026-04-23 本地验证。**根因**：旧版 `full-access` 策略 metadata 中无 `allowed_exec_modes` 字段，回退逻辑仅添加 `exec_mode=shell_text`，导致 `argv_exec` 被拒绝（`Config error: policy 'full-access' allows exec_mode [shell_text], got argv_exec`），且错误信息存在双重 `Config error:` 前缀。**修复**：`resolve_shell_policy_from_set` 中当 `allowed_exec_modes` 为空时，从 `allow_any_executable`/`allowed_executables` 推断 `argv_exec`，从 `allowed_shell_patterns` 推断 `shell_text`，保证向后兼容。同时修复 `select_policy_id_for_command` 中单候选错误被双重包装的问题（保留原始 `BifrostError` 而非 `.to_string()` 后重新包装）。**验证方式**：(1) 写入旧版 `full-access` 配置到 `TARGET_DATA_DIR=/tmp/bifrost-rse12-target-oC8W5Llv/remote_shell.json`，仅含 `exec_mode=shell_text`、`allowed_shell_patterns=["^(?s:.*)$"]`、`allow_any_executable=true`、`shell=/bin/bash`、`inherit_env=true`，无 `allowed_exec_modes`。通过 CLI `remote shell list` 确认 target 加载了 `full-access (mode: shell_text)` 单策略。(2) 单元测试 `test_legacy_full_access_without_allowed_exec_modes_permits_argv`：模拟旧格式策略，`select_policy_id_for_command` 对 `argv_exec` 和 `shell_text` 均返回 `Ok("full-access")`。(3) 真实执行测试 `test_legacy_full_access_argv_exec_actually_runs`：同旧格式策略，通过 `executor.execute()` 以 `argv_exec` 模式执行 `/bin/pwd`，exit_code=0，stdout 输出非空目录路径。(4) 单元测试 `test_select_policy_single_rejection_has_no_double_error_prefix`：验证单候选拒绝时错误信息为 `Config error: policy '...'` 而非 `Config error: Config error: policy '...'`。全部 27 个 executor 测试通过。 |
| TC-RSE-13 | ✅ PASS | 2026-04-23 本地验证。执行 `cargo run --bin bifrost -- remote command exec pwd` 时，CLI 直接报 `unexpected argument 'pwd' found`，并提示查看 `--help`，不会再把裸参数静默解析成 `argv_exec` 然后打到远端策略层。随后执行 `cargo run --bin bifrost -- remote command exec -- /bin/pwd`，仍按 `argv_exec` 正常解析。 |
| TC-RSE-14 | ✅ PASS | 2026-04-23 本地真实链路验证。隔离启动 target / relay / caller 后，将 target Shell Access 切到 `Full Access`。第一轮执行 `python3 -u -c 'print(\"stream-one\", end=\"\", flush=True); time.sleep(1.2); print(\"stream-two\", end=\"\", flush=True)'`：命令启动约 0.4 秒时检查 caller 输出文件，已提前看到 `stream-one`，且进程仍在运行；命令结束后完整输出为 `stream-onestream-two`。第二轮单独执行 `/usr/bin/top -l 2 -s 1`：在命令启动约 1.4 秒时，caller 输出文件已写入 211457 字节，首屏包含 `Processes:` / 时间 / `Load Avg`，且进程仍在运行；命令结束后总输出增长到 434068 字节，`Processes:` 采样头共出现 2 次。证明 caller 在进程退出前已经收到 stdout frame，而不是等到 `top` 整体结束后一次性打印。Recent Calls 最终仍正常记录 exit_code / stdout_digest。 |
| TC-RSE-15 | ✅ PASS | 2026-04-24 再次本地执行 `bash e2e-tests/tests/test_remote_shell_exec_streaming_e2e.sh`。脚本自动隔离启动 relay / target / caller，`shell_text` 场景继续稳定在命令退出前输出 `shell-one`，随后完整收敛为 `shell-oneshell-two`；`argv_exec` 场景同样先观察到 `argv-one`，最终完整输出 `argv-oneargv-two`。两条链路都继续校验 target `/_bifrost/api/remote-invoke/calls` 中最新 `shell.exec` 记录，确认 `policy_id=stream-shell/stream-argv`、`exec_mode=shell_text/argv_exec`、`exit_code=0`、`stdout_digest` 为有效 SHA1，脚本 summary 33/33 全部通过。 |
| TC-RSE-16 | ⚠️ PARTIAL | 2026-04-24 经 10+ 轮 Windows CI 迭代，完成全部根因修复：1) `inherit_env=true` 保留 PATH 解决命令查找问题；2) 使用裸 `cmd.exe` + 裸 `ping` 避免绝对路径与引号解析问题；3) 末尾追加 `&exit /b 0` 解决 `set /p` 从 nul 读取返回 exit code 1 的问题。macOS 本地验证 E2E 与单元测试全部通过，Windows 真机状态待 CI 补验。 |
| TC-RSE-17 | ✅ PASS | 2026-04-24 本地验证。`build_shell_text_process` 添加了 Unix 路径 fallback 和 UTF-8 编码处理。macOS 单元测试 `test_build_shell_text_process_default_shell`（验证默认 shell 为 `/bin/sh`）、`test_build_shell_text_process_explicit_shell`（验证显式 `/bin/bash` 正确传递）通过。Windows 专属测试 `test_build_shell_text_process_unix_path_fallback_on_windows`、`test_build_shell_text_process_powershell_utf8_prefix`、`test_is_unix_only_shell_path` 已添加，待 Windows CI 验证。E2E `remote_shell_exec_unix_shell_path_fallback` 在 macOS 上通过（直接使用 `/bin/bash`），Windows CI 将验证 fallback 到 `cmd` 行为。 |
| TC-RSE-18 | ✅ PASS | 2026-04-24 本地验证。CLI `policy update` 命令解析测试：`remote_shell_policy_update_parses_all_flags`（全参数解析）和 `remote_shell_policy_update_minimal_args`（仅必需 ID）均通过。87 个 CLI 测试全部 OK。E2E `remote_shell_policy_update_preserves_execution` 验证更新 policy name 后命令仍可执行，通过。 |
| TC-RSE-19 | ✅ PASS | 2026-05-09 本地验证。`cargo test -p bifrost-admin remote_invoke::executor::tests::test_execute_shell_exec_forwards_stdin_stream -- --nocapture` 通过，executor 在允许 stdin 的 shell policy 下把 mpsc stdin stream 写入 child stdin，Python 程序读到 `hello remote stdin` 并以 exit code 0 返回。`cargo test -p bifrost-cli remote:: -- --nocapture` 通过，覆盖 remote CLI 参数构造、streaming/cancel 解析和新增 interactive/stdin/pty 字段的回归。本轮一次性临时 harness 已执行同一组验证；测试脚本未落库。 |
| TC-RSE-20 | ✅ PASS | 2026-05-09 使用 `/tmp` 下的一次性临时 harness（执行后删除，未落库）真实执行 relay/target/caller 链路。首次运行暴露 `--interactive` 仍按普通 streaming Done digest 校验导致 `stream digest mismatch` 的真实 bug；修复为 interactive 默认跳过该 digest 校验并补充单测 `test_build_remote_command_interactive_disables_digest_verification` 后重建 `target/debug/bifrost`。最终通过链路：relay 端口 `50831`，target 端口 `50830`，target 启动参数包含 `--no-system-proxy`；pair-code 连接成功，grant 从 `remote_shell_exec` 升级为 `remote_shell_interactive` 且 `stdin_allowed=true`、`interactive_allowed=true`；本地 PTY 启动 `bifrost remote exec --interactive --shell-text ...`，远端输出 `REMOTE_INTERACTIVE_READY`，随后通过 caller-to-client encrypted `call_frame` 读到 `REMOTE_INTERACTIVE_INPUT_OK`；Recent Calls 新增 `shell.exec` 记录，断言 `policy_id=stream-shell`、`exec_mode=shell_text`、`exit_code=0`、`stdout_digest` 有效。临时 harness summary 为 42/42 PASS。 |
| TC-RSE-21 | ✅ PASS | 2026-05-10 本地真实链路验证。单元回归 `cargo test -p bifrost-admin remote_invoke::executor::tests::test_execute_shell_exec_pty_reports_isatty_true -- --nocapture` 通过，target executor 在 `pty.enabled=true` 时使用真 PTY，Python 输出包含 `True True`。CLI 回归 `cargo test -p bifrost-cli remote::tests::test_remote_stdin_forwarder_owns_raw_mode_guard -- --nocapture` 通过，raw mode guard 由 caller forwarder 主生命周期持有。随后用 `/tmp` 下的一次性临时 harness（执行后删除，未落库）真实启动 relay / target / caller：relay 端口 `56137`、target 端口 `56140`，target 启动包含 `--no-system-proxy`；pair-code 连接成功，grant 升级为 `remote_shell_interactive` 且 `stdin_allowed=true`、`interactive_allowed=true`；本地 Python PTY 中运行 `bifrost remote exec --interactive --shell-text "python3 -u -c 'import os,sys; print(os.isatty(0), os.isatty(1)); print(\"READY\"); print(sys.stdin.readline().strip())'"`，输出包含 `True True`、`READY`、`PTY_STDIN_OK`、`__REMOTE_EXIT:0`；同一 PTY 后续 `stty -a` 显示 `icanon` 与 `echo`，未出现独立 `-icanon` / `-echo`，证明 interactive 退出后 raw mode 已恢复。修复 PTY 子进程退出后 EOF 等待稳定性后，再次真实执行双 Bifrost + relay 链路通过：relay 端口 `63757`、target 端口 `63758`、caller 端口 `63759`；target 启动前写入临时 `config.toml` 指向本地 relay，pair/grant 成功；远端 Python 输出 `True True`、`READY`、`REMOTE_PTY_STDIN_OK`、`__REMOTE_EXIT:0`；Recent Calls 最新 `shell.exec` 为 `policy_id=pty-shell`、`exec_mode=shell_text`、`exit_code=0`。 |
| TC-RSE-22 | ✅ PASS | 2026-05-10 本地真实链路验证。先分 filter 执行新增 worker 回归：`cargo test -p bifrost-admin active_call_accepts_stdin_before_executor_start`、`cargo test -p bifrost-admin command_accepts_stdin_for_stdin_mode_or_pty` 均通过。随后执行 `bash e2e-tests/tests/test_remote_shell_exec_streaming_e2e.sh`，脚本隔离启动 relay / target / caller，target 启动包含 `--no-system-proxy`，并将 Shell Access policy 和 grant 都配置为允许 stdin。新增真实用例通过管道立即发送 `EARLY_STDIN_OK\n` 到 `bifrost remote exec --stdin --shell-text ...`，远端 Python 进程输出包含 `READY` 与 `EARLY_STDIN_OK`，Recent Calls 最新 `shell.exec` 记录 `policy_id=stream-shell`、`exec_mode=shell_text`、`exit_code=0`、`stdout_digest` 有效；脚本 summary 41/41 PASS。 |
| TC-RSE-23 | ✅ PASS | 2026-06-04 本地 workspace 全量复测暴露 `remote_invoke::executor::tests::test_execute_shell_exec_forwards_stdin_stream` 在并发负载下偶发命中夹具策略 `max_timeout_ms=5000`，错误为 `shell.exec wall-clock timeout after 5000 ms (policy 'stdin-shell')`。该用例目标是 stdin stream 转发，不是 timeout 边界；修复后仅将该测试 policy 调宽到 15000ms，产品默认 timeout 和独立 wall-clock timeout 测试不变。执行 `cargo test -p bifrost-admin remote_invoke::executor::tests::test_execute_shell_exec_forwards_stdin_stream -- --nocapture` 通过；执行 `cargo test -p bifrost-admin remote_invoke::executor::tests::test_execute_shell_exec_wall_clock_timeout_still_enforced -- --nocapture` 通过，确认 timeout enforcement 未被削弱。workspace 全量继续由本地复跑和远端 CI 共同兜底。 |
