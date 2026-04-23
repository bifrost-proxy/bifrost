# Remote Shell Exec 真实场景测试

## 功能模块说明

当前 `remote shell exec` 的真实契约是：

- caller 通过 `bifrost remote command exec ...` 发起命令
- caller 不允许指定 `policy_id`
- target 根据本地 `Shell Access` 配置和该 caller 对应的 grant binding 自动选择唯一策略
- grant binding / stdin / interactive / policy version snapshot 只保存在 target 本地
- relay 只保留最小 `grant_scope`，不保存具体策略绑定
- 如果没有命中策略、命中多个策略，或者 caller 试图伪造 `policy_id`，都由 target 拒绝
- `policy_id` / `exec_mode` 只作为 target 侧审计结果写入 Recent Calls

## 前置条件

1. 仓库位于 `/Users/eden/work/github/bifrost`
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
| TC-RSE-10 | ✅ PASS | 2026-04-23 在隔离环境 `target=65323`、`relay=65324`、`TARGET_DATA_DIR=/tmp/bifrost-grant-target-lz76j2ye`、`CALLER_DATA_DIR=/tmp/bifrost-grant-caller-7d1nkfjp` 真实执行。先用 pair-code 建立 `remote_query` grant `VkQTKYzVCCokjPCrU6gKv`，caller 执行 `remote command exec -- /bin/pwd` 明确报 `saved remote authorization is read-only and does not allow shell.exec`。随后通过 target 本地 CLI `cargo run --bin bifrost -- --port 65323 remote grant update VkQTKYzVCCokjPCrU6gKv --access selected --policy pwd-argv` 将 grant 升级到 `selected[pwd-argv]`，返回 payload 中已包含 `policy_binding={mode:selected,policy_ids:[pwd-argv]}` 与 `shell_policy_set_version_snapshot=10`；caller 再执行 `/bin/pwd` 成功输出 `/Users/eden/work/github/bifrost`，执行 `/bin/date` 被 target 拒绝：`program '/bin/date' is not allowed by policy 'pwd-argv'`。之后用真实浏览器打开 `http://127.0.0.1:65323/_bifrost/settings?tab=remote-invoke`，点击 Grants 的 `Edit Access`，把同一 grant 改为 `All enabled shell policies`，页面提示 `Grant access updated`；`GET /_bifrost/api/remote-invoke/grants` 随后返回 `policy_binding={mode:all}`，caller 执行 `--shell-text 'printf full-open && /bin/pwd'` 成功输出 `full-open/Users/eden/work/github/bifrost`。最后直接检查 relay SQLite `/tmp/bifrost-grant-relay-k45bp4q4/bifrost-sync.db`：`bifrost_remote_invoke_grants` 表只有 `grant_scope` / `ssh_key_id` / `ssh_key_fingerprint` 等最小列，没有 `policy_binding` / `shell_policy_set_version_snapshot` / `interactive_allowed` / `stdin_allowed`；该 grant 在 relay 中仅记录 `grant_scope=remote_shell_exec`。target 本地 `/tmp/bifrost-grant-target-lz76j2ye/admin/remote_invoke_grant_policy.json` 则保存了 `policy_binding={mode:all}`、`shell_policy_set_version_snapshot=10`、`stdin_allowed=false`、`interactive_allowed=false`，证明策略细节只留在执行侧。 |
| TC-RSE-11 | ✅ PASS | 2026-04-23 在隔离环境 `target=65461`、`relay=65462`、`TARGET_DATA_DIR=/var/folders/2k/nc0_nn9976l02sftpyhc9tz40000gn/T/bifrost-rse11-target-1ol13h2w`、`CALLER_DATA_DIR=/var/folders/2k/nc0_nn9976l02sftpyhc9tz40000gn/T/bifrost-rse11-caller-099y45el`、`RELAY_DATA_DIR=/var/folders/2k/nc0_nn9976l02sftpyhc9tz40000gn/T/bifrost-rse11-relay-4wrcjkru` 真实执行。对同一 `client_instance_id=a32cac09-ce8c-4ebf-8e9f-43238cd189ff` 和 caller 指纹连续完成两次 pair-code connect，第一次 grant 为 `p8DEHNdigh_dKEud4kOT6`，第二次 grant 为 `hOpA0_uoFILU5WqDFMJ4a`。第二次 connect 后直接执行 `target/debug/bifrost remote status --relay-url http://127.0.0.1:65462`，成功返回远端状态 JSON，不再出现 `saved connection transport no longer matches relay reusable authorization`。随后查询 relay `grants/reusable`，返回的正是第二次最新 grant `hOpA0_uoFILU5WqDFMJ4a`。再执行 `target/debug/bifrost remote disconnect --all --relay-url http://127.0.0.1:65462`，CLI 输出 `Revoking 1 connection(s)… ✓ hOpA0_uoFILU (eden)`；之后再次查询 `grants/reusable` 返回 `data=null`。最后直接检查 relay SQLite，两个 grant 都存在但状态均为 `removed`：`p8DEHNdigh_dKEud4kOT6 -> removed`、`hOpA0_uoFILU5WqDFMJ4a -> removed`，证明 reconnect 会覆盖旧 grant，而 `disconnect --all` 会清空该 caller 在该设备上的全部残留 reusable grants。 |
| TC-RSE-12 | ⏳ TODO | 回归 `remote command exec pwd` 与旧版 `full-access` 单策略配置的兼容执行。 |
