# Remote Invoke SSH Key Caller Identity

## 功能模块说明

验证本机 CLI 可以快速生成 Remote Invoke SSH key 文件，并验证 `bifrost remote conn up --ssh-key` 在多个 caller 沙箱中复用同一 SSH key 时，caller 身份使用本地随机持久 ID，而不是主机名、用户名或 SSH key fingerprint 派生值。目标是让 target 机器无需打开 WebUI 就能输出可分发的 key，同时让 caller 可通过文件路径或固定环境变量 `BIFROST_REMOTE_SSH_KEY` 使用 key，避免多个 caller 竞争同一个 grant。

## 前置条件

- 在仓库根目录 `<REPO_ROOT>` 执行。
- 测试服务必须使用临时 `BIFROST_DATA_DIR`，禁止使用 9900 端口。
- 启动 Bifrost 时必须使用 `--no-system-proxy`；本用例不验证系统代理。
- 需要 `jq`、`python3`、`curl` 可用。
- 本机 CLI 生成 key 的快速回归优先执行：`e2e-tests/tests/test_setting_ssh_key_cli.sh`。
- CI 验证入口必须包含该脚本：`scripts/run_all_e2e.sh --ci --skip-rules --skip-runner --skip-ui --list-shell-tests` 应列出 `test_setting_ssh_key_cli.sh`，且 stable shell 模式也应列出它。
- 优先执行自动化脚本：`e2e-tests/tests/test_remote_invoke_ssh_e2e.sh`，该脚本会启动本地 relay、目标端、两个 caller 沙箱，并自动清理临时目录。

## 测试用例列表

### TC-RISK-00：本机 CLI 生成并导出 Remote Invoke SSH key

**操作步骤**

1. 执行：
   ```bash
   BIFROST_DATA_DIR="$(mktemp -d)" bash e2e-tests/tests/test_setting_ssh_key_cli.sh
   ```
2. 观察脚本中 `Create local remote-invoke SSH key to an output file`、`Status shows active key metadata`、`Generated key is accepted by caller-side --ssh-key parser from fixed env` 和 `Revoke active key` 四段输出。
3. 确认脚本生成的 key 文件包含 `-----BEGIN BIFROST KEY-----` 和 `Device-Code: BF-...`。
4. 确认脚本使用 `bifrost remote conn up --ssh-key <key>` 的 caller 侧解析路径读取生成 key；relay 端口故意不可达时，只允许出现网络连接失败，不允许出现 key 解析失败。

**预期结果**

- `bifrost setting ssh-key create --output <file>` 成功创建 key 文件。
- Unix/macOS 下 key 文件权限为 `0600`。
- `bifrost setting ssh-key status` 显示 label、device code、fingerprint 和 grant mode。
- `bifrost setting ssh-key export` 能导出与 active key 一致的文件，且不带 `--force` 时拒绝覆盖已有输出文件。
- 生成的 key 可被 caller 侧 `--ssh-key` parser 通过固定环境变量 `BIFROST_REMOTE_SSH_KEY` 接受。
- `bifrost setting ssh-key revoke` 后 `status` 显示没有 active key。

### TC-RISK-01：同一 SSH key 在两个 caller 沙箱中生成不同 caller ID

**操作步骤**

1. 执行：
   ```bash
   BIFROST_DATA_DIR="$(mktemp -d)" bash e2e-tests/tests/test_remote_invoke_ssh_e2e.sh
   ```
2. 观察脚本中 `Use CLI remote conn up --ssh-key` 和 `Use same SSH key from another caller sandbox and verify caller identity isolation` 两段输出。
3. 确认脚本读取两个 caller 沙箱的 `remote-connections.json`。
4. 确认脚本通过 target Admin API `/_bifrost/api/remote-invoke/grants` 查询 grant 列表。

**预期结果**

- 两个 caller 沙箱中的 `.connections[0].caller_fingerprint` 都以 `caller-` 开头。
- 两个 caller 沙箱的 `caller_fingerprint` 不相等。
- 两个 caller 的 `caller_fingerprint` 都不等于 SSH key fingerprint。
- target grant 列表中存在两条 `auth_method=ssh_publickey` 且 `ssh_key_fingerprint` 相同的 active grant。
- 两条 grant 的 `caller_fingerprint` 分别匹配两个 caller 沙箱的随机 ID，互不覆盖。
- 后续 `remote conn status`、`remote traffic search`、`remote traffic get` 仍可通过第一个 caller 的 saved connection 正常执行。

### TC-RISK-02：`--ssh-key` 不带路径时从固定环境变量读取

**操作步骤**

1. 在 target 侧生成或导出 Bifrost SSH key 文件：
   ```bash
   BIFROST_DATA_DIR="$(mktemp -d)" bifrost setting ssh-key create --label "ci-target" --output ./bifrost-device.key
   ```
2. 在 caller 侧把 key 内容放入固定环境变量：
   ```bash
   export BIFROST_REMOTE_SSH_KEY="$(cat ./bifrost-device.key)"
   ```
3. 使用不带路径的 `--ssh-key` 发起连接：
   ```bash
   BIFROST_DATA_DIR="$(mktemp -d)" bifrost remote conn up --ssh-key --label "ci-agent"
   ```
4. 执行 `bifrost remote conn status`，确认 saved connection 可复用。
5. 在自动化回归中执行：
   ```bash
   BIFROST_DATA_DIR="$(mktemp -d)" bash e2e-tests/tests/test_setting_ssh_key_cli.sh
   BIFROST_DATA_DIR="$(mktemp -d)" bash e2e-tests/tests/test_remote_invoke_ssh_e2e.sh
   ```

**预期结果**

- 第 3 步 CLI 不要求 `--ssh-key <path>`，而是读取固定环境变量 `BIFROST_REMOTE_SSH_KEY`。
- 如果 `BIFROST_REMOTE_SSH_KEY` 未设置或为空，CLI 给出明确错误提示。
- `remote-connections.json` 中 SSH key 连接的 `auth_method=ssh_publickey`；通过 env 模式连接时 `ssh_key_source=env:BIFROST_REMOTE_SSH_KEY`。
- 不支持任意环境变量名；用户不需要也不能通过 `env:OTHER_NAME` 自定义。

### TC-RISK-03：SSH key 连接默认 Full Trust 且 grant 级别可通过 CLI 切换

**操作步骤**

1. 执行：
   ```bash
   BIFROST_DATA_DIR="$(mktemp -d)" bash e2e-tests/tests/test_remote_invoke_ssh_e2e.sh
   ```
2. 观察脚本中 `Wait for ssh_publickey grant created by CLI` 和 `Execute remote shell via SSH-key Full Trust grant` 两段输出。
3. 通过 target Admin API `/_bifrost/api/remote-invoke/grants` 查询 SSH key grant，确认 `grant_scope`、`file_access`、`interactive_allowed` 和 `stdin_allowed`。
4. 使用 `bifrost setting grant update --device <caller-fingerprint> --level files` 切换同一 grant 至 Files only。
5. 再使用 `bifrost setting grant update --device <caller-fingerprint> --level full` 切回 Full Trust。
6. 对于人工交互验证，可执行 `bifrost setting grant update`，在 TTY 中先选择设备/grant，再选择权限级别。

**预期结果**

- SSH key 首次连接 target 时自动生成的 `ssh_publickey` grant 为 Full Trust：`grant_scope=remote_shell_interactive`、`file_access=read_write`、`interactive_allowed=true`、`stdin_allowed=true`。
- SSH key grant 绑定内置 `ssh-key-full-access` command group；即使用户未手动创建 shell policy，也能直接运行允许的任意命令。
- 第 2 步中的 `bifrost remote exec --shell-text "printf ssh-full-trust-ok"` 成功输出 `ssh-full-trust-ok`，不会再因 `grant_scope_mismatch` 退回到只读授权。
- `bifrost setting grant update --device <caller-fingerprint> --level files` 成功把 grant 切换为 `grant_scope=remote_query` 且 `file_access=read_write`。
- `bifrost setting grant update --device <caller-fingerprint> --level full` 成功恢复 Full Trust。
- 仅通过 pair/code 的交互式授权流程允许用户手动选择权限级别；SSH key 自动连接默认必须是 Full Trust。

### TC-RISK-04：SSH v5 once-with-budget 不会在多次调用后误消费 grant

**操作步骤**

1. 执行：
   ```bash
   NODE_BIN="$HOME/.local/share/mise/installs/node/22.22.0/bin/node" \
   BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 \
   BIFROST_DISABLE_TRAY=1 \
   BIFROST_BIN="$PWD/target/release/bifrost" \
   SKIP_BUILD=true \
   bash e2e-tests/tests/test_remote_invoke_ssh_e2e.sh
   ```
2. 观察脚本中以下阶段：
   - `Verify default SSH-key Full Trust can run commands and read/write files`
   - `Switch SSH grant to Run commands & read/write files and verify capabilities`
   - `Switch SSH grant to Files only and verify command denial plus file access`
   - `Switch SSH grant back to Full Trust and verify command access is restored`
   - `Verify SSH grant remains visible on target before traffic.get`
3. 在 `Verify SSH grant remains visible on target before traffic.get` 前后查询 target Admin API：
   ```bash
   curl -s "$ADMIN_BASE_URL/api/remote-invoke/grants"
   ```
4. 确认第一个 caller 的 saved connection 继续执行：
   - `remote conn status`
   - `remote search`
   - `remote traffic get`
   - target-local `traffic get --ids`、`auth-status`、`export`、`replay --refresh-auth`

**预期结果**

- SSH key grant 在 relay 侧使用 v5 PoP claim/session token，并可配置多次调用预算；第一次 `file.write` 后不会把 relay grant 标记为 consumed。
- target worker 从 relay 同步 active grant 时保留 relay 返回的 `max_calls` / `remaining_calls`，不会把 once-with-budget 降级为本地 1 次调用。
- target Admin API 中同一 SSH key grant 在多次 file/exec/traffic 调用后仍可见，`grant_mode=permanent`，`expires_at` 为空，`caller_fingerprint` 和 `ssh_key_fingerprint` 保持不变。
- 权限级别在 `shell` / `files` / `query` / `full` 之间切换后，原 saved connection 不需要重新授权即可继续按当前权限执行允许的操作。
- 脚本最终输出 `All SSH remote invoke E2E checks passed`。

### TC-RISK-05：SSH 双 caller grant 列表从持久化 store 自愈

**操作步骤**

1. 先构建当前分支二进制：
   ```bash
   cargo build --bin bifrost
   ```
2. 连续执行 Remote Invoke pair-code 与 SSH key 两条相关 shell E2E：
   ```bash
   BIFROST_BIN="$PWD/target/debug/bifrost" \
   BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 \
   BIFROST_DISABLE_TRAY=1 \
   bash e2e-tests/tests/test_remote_invoke_e2e.sh && \
   BIFROST_BIN="$PWD/target/debug/bifrost" \
   BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 \
   BIFROST_DISABLE_TRAY=1 \
   bash e2e-tests/tests/test_remote_invoke_ssh_e2e.sh
   ```
3. 观察 `Use same SSH key from another caller sandbox and verify caller identity isolation` 阶段。
4. 如果断言失败，检查脚本自动输出的 caller-2 `remote-connections.json`、target `remote_invoke_grant_info.json` 和 `remote_invoke_grant_crypto.json`。

**预期结果**

- 第二个 caller 使用同一个 SSH key 连接后会得到新的 `grant_id` 和新的 `caller_fingerprint`。
- target 持久化的 `remote_invoke_grant_info.json` 与 `remote_invoke_grant_crypto.json` 中均包含两个 SSH grant。
- target Admin API `/_bifrost/api/remote-invoke/grants` 在返回前会从持久化 grant info + crypto store 恢复缺失的本地 grants，列表中可见两个 `auth_method=ssh_publickey` grants。
- 两个 SSH grant 的 `caller_fingerprint` 分别匹配两个 caller 沙箱，不会被同一个 SSH key fingerprint 合并或覆盖。
- 后续 target-local `traffic get --ids`、`auth-status`、`export`、`replay --refresh-auth` 和 revoke 流程继续通过，脚本最终输出 `All SSH remote invoke E2E checks passed`。

## 清理步骤

- 脚本退出时会清理 key 生成、relay、target、caller、mock server 临时目录和进程。
- 如脚本异常中断，执行：
  ```bash
  pkill -f 'bifrost-sync-server.*--enable-remote-invoke' || true
  pkill -f 'python3 -m http.server' || true
  ```

## 执行结果

### 2026-06-02 固定环境变量 SSH key 回归

| 用例 | 结果 | 证据 |
| --- | --- | --- |
| TC-RISK-00 | PASS | 执行 `bash e2e-tests/tests/test_setting_ssh_key_cli.sh` 通过；输出包含 `Generated key is accepted by caller-side --ssh-key parser from fixed env` 和最终 `PASS`，证明生成的 key 可通过固定 `BIFROST_REMOTE_SSH_KEY` + `--ssh-key` 无路径形式进入 caller 侧 parser。 |
| TC-RISK-01 | PASS | 执行 `bash e2e-tests/tests/test_remote_invoke_ssh_e2e.sh` 通过；脚本启动本地 relay、target 与两个 caller，确认两个 caller 使用同一 SSH key 时 caller fingerprint 不同且 target grants 中存在对应 `ssh_publickey` grants。 |
| TC-RISK-02 | PASS | 同一 `test_remote_invoke_ssh_e2e.sh` 中第二个 caller 将导出的 key 内容写入 `BIFROST_REMOTE_SSH_KEY` 后执行 `bifrost remote conn up --ssh-key --relay-url ...` 成功连接，并断言 `remote-connections.json` 中 `ssh_key_source=env:BIFROST_REMOTE_SSH_KEY`；后续 `remote conn status`、`remote traffic search`、`remote traffic get` 均通过。 |

### 2026-06-15 SSH key 默认 Full Trust 与 grant 级别切换回归

| 用例 | 结果 | 证据 |
| --- | --- | --- |
| TC-RISK-03 | PASS | 2026-06-15 执行 `bash e2e-tests/tests/test_remote_invoke_ssh_e2e.sh` 通过。脚本启动本地 relay、真实 Bifrost target、mock target 和两个 caller；通过 `bifrost setting ssh-key create --label "CI Agent" --grant-mode permanent --output ...` 在运行中的 target 上创建 key，再用该 key 执行 `remote conn up --ssh-key`。验证默认 SSH key grant 可真实 `file.write`/`file.read` 和 `remote exec`，并确认 `setting grant update --device ... --level shell/files/query/full` 后分别满足 commands+files、files-only、read-only watch、恢复 Full Trust 的能力矩阵；files-only/query 下 `remote exec` 被拒绝且不会自动重新 SSH 授权绕过降权。脚本还验证同一 SSH key 的第二 caller 使用不同 `caller_fingerprint`，target 保留两条 `auth_method=ssh_publickey` grant，随后 `remote conn status`、`remote traffic search/get` 和 revoke 全部通过。 |

### 2026-06-29 SSH v5 once-with-budget grant 消费回归

| 用例 | 结果 | 证据 |
| --- | --- | --- |
| TC-RISK-04 | PASS | 2026-06-29 执行 `NODE_BIN="$HOME/.local/share/mise/installs/node/22.22.0/bin/node" BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 BIFROST_DISABLE_TRAY=1 BIFROST_BIN="$PWD/target/release/bifrost" SKIP_BUILD=true bash e2e-tests/tests/test_remote_invoke_ssh_e2e.sh` 通过。脚本完成默认 Full Trust file.read/write、remote exec、grant level shell/files/query/full 多轮切换、第二 caller 隔离、`remote conn status`、`remote search`、`remote traffic get`、target-local `traffic get --ids` / `auth-status` / `export` / `replay --refresh-auth`，并在 `Verify SSH grant remains visible on target before traffic.get` 阶段确认原 SSH grant 仍为 `grant_mode=permanent` 且未被 consumed，最终输出 `All SSH remote invoke E2E checks passed`。 |

### 2026-06-29 合并主干后 SSH grant 回归

| 用例 | 结果 | 证据 |
| --- | --- | --- |
| TC-RISK-04 | PASS | 2026-06-29 将 `origin/main` 合入 `feat/remote-invoke-v5-pop` 并解决 `site/src/content/docs/reference/rule-engine.md` 冲突后，重新执行 `NODE_BIN="$HOME/.local/share/mise/installs/node/22.22.0/bin/node" PATH="$HOME/.local/share/mise/installs/node/22.22.0/bin:$PATH" BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 BIFROST_DISABLE_TRAY=1 BIFROST_BIN="$PWD/target/release/bifrost" SKIP_BUILD=true bash e2e-tests/tests/test_remote_invoke_ssh_e2e.sh` 通过。脚本完成 SSH key grant 创建、默认 Full Trust 文件读写与命令执行、shell/files/query/full 权限切换、第二 caller 隔离、`remote conn status`、`remote search`、`remote traffic get`、target-local `traffic get --ids` / `auth-status` / `export` / `replay --refresh-auth` 和 revoke，最终输出 `All SSH remote invoke E2E checks passed`。 |

### 2026-07-07 SSH 双 caller grant 列表自愈回归

| 用例 | 结果 | 证据 |
| --- | --- | --- |
| TC-RISK-05 | PASS | 2026-07-07 本地先复现到 caller-2 `remote-connections.json` 中已有新 `grant_id` 和 `caller_fingerprint`，且 target `remote_invoke_grant_info.json` / `remote_invoke_grant_crypto.json` 已持久化两条 SSH grant，但 target Admin grants API 只返回第一条内存 grant。修复后执行 `SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin remote_invoke::worker::coverage_boost --lib` 通过 83 个 worker 单测；执行 `BIFROST_BIN="$PWD/target/debug/bifrost" BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 BIFROST_DISABLE_TRAY=1 bash e2e-tests/tests/test_remote_invoke_e2e.sh && BIFROST_BIN="$PWD/target/debug/bifrost" BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 BIFROST_DISABLE_TRAY=1 bash e2e-tests/tests/test_remote_invoke_ssh_e2e.sh` 通过，前者输出 `All assertions: total=73 passed=73 failed=0`，后者输出 `All SSH remote invoke E2E checks passed`。 |

### 2026-07-09 SSH grant 权限恢复传播回归

| 用例 | 结果 | 证据 |
| --- | --- | --- |
| TC-RISK-06 | PASS | GitHub Actions PR 361 run `29000407368` 的 `E2E Shell (aarch64-apple-darwin, shard 1/2)` 在 `test_remote_invoke_ssh_e2e.sh` 中执行 `setting grant update --level query` 后再恢复 `--level full`，随后的 `remote exec --shell-text "printf ssh-full-restored-ok"` 偶发收到 `grant scope RemoteQuery / file_access None does not allow command kind ShellExec`。脚本已更新为只对该权限传播型错误做短暂重试；真实拒绝、输出缺失或其它错误仍立即失败并打印 grant diagnostics。2026-07-09 执行 `BIFROST_BIN="$PWD/target/debug/bifrost" BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 BIFROST_DISABLE_TRAY=1 bash e2e-tests/tests/test_remote_invoke_ssh_e2e.sh` 通过；本地复现到 `grant scope update not visible to remote exec yet (1/20); retrying ssh-full-restored-ok`，随后完成第二 caller 隔离、remote traffic/search、target-local traffic replay 和 revoke，最终输出 `All SSH remote invoke E2E checks passed`。 |
