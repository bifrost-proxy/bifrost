# Remote Invoke SSH Key Caller Identity

## 功能模块说明

验证 `bifrost remote conn up --ssh-key` 在多个 caller 沙箱中复用同一 SSH key 时，caller 身份使用本地随机持久 ID，而不是主机名、用户名或 SSH key fingerprint 派生值。目标是避免多个 caller 竞争同一个 grant。

## 前置条件

- 在仓库根目录 `/Users/eden/work/github/bifrost` 执行。
- 测试服务必须使用临时 `BIFROST_DATA_DIR`，禁止使用 9900 端口。
- 启动 Bifrost 时必须使用 `--no-system-proxy`；本用例不验证系统代理。
- 需要 `jq`、`python3`、`curl` 可用。
- 优先执行自动化脚本：`e2e-tests/tests/test_remote_invoke_ssh_e2e.sh`，该脚本会启动本地 relay、目标端、两个 caller 沙箱，并自动清理临时目录。

## 测试用例列表

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

## 清理步骤

- 脚本退出时会清理 relay、target、caller、mock server 临时目录和进程。
- 如脚本异常中断，执行：
  ```bash
  pkill -f 'bifrost-sync-server.*--enable-remote-invoke' || true
  pkill -f 'python3 -m http.server' || true
  ```
