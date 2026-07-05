# macOS Enhanced Proxy 真实场景测试用例

## 功能模块说明

验证 macOS Enhanced Proxy 增强模式的 CLI、Admin API、Web UI 和 macOS helper 授权边界。该模式用于让不支持系统代理的应用在 macOS Network Extension 安装并批准后走 Bifrost。

## 前置条件

1. 使用临时数据目录，避免污染正式运行状态：
   ```bash
   export BIFROST_DATA_DIR="$(mktemp -d /tmp/bifrost-enhanced-proxy.XXXXXX)"
   ```
2. 构建或运行当前工作区 CLI：
   ```bash
   cargo run -p bifrost-cli --bin bifrost -- enhanced-proxy status
   ```
3. 所有启动 Bifrost 的命令必须带 `--no-system-proxy`，并设置：
   ```bash
   export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1
   export BIFROST_DISABLE_TRAY=1
   ```

## 测试用例

### TC-MEP-01：CLI 默认状态不启用增强模式

**操作步骤**：
1. 执行：
   ```bash
   cargo run -p bifrost-cli --bin bifrost -- enhanced-proxy status --format json-pretty
   ```

**预期结果**：
- 命令退出码为 0。
- JSON 包含 `configured_enabled: false`。
- JSON 包含 `enabled: false`。
- 非 macOS 平台 `state` 为 `unsupported`；macOS 且未安装 helper 时启用前仍为 `disabled`。
- `policy.exclude_apps` 包含 Bifrost 自身相关名称。

### TC-MEP-02：CLI enable 只写 desired state，不谎称 active

**操作步骤**：
1. 执行：
   ```bash
   cargo run -p bifrost-cli --bin bifrost -- -p 9900 enhanced-proxy enable
   ```
2. 再执行：
   ```bash
   cargo run -p bifrost-cli --bin bifrost -- enhanced-proxy status --format json-pretty
   ```

**预期结果**：
- enable 命令退出码为 0。
- `configured_enabled: true`。
- `proxy_host` 为 `127.0.0.1`，`proxy_port` 为 `9900`。
- 在 helper/controller 未安装连接前，`enabled` 必须为 `false`，状态必须是 `unsupported`、`helper_missing`、`extension_missing` 或 `approval_required` 之一。
- 输出包含明确的 `remediation`。

### TC-MEP-03：CLI disable 回落到关闭状态

**操作步骤**：
1. 执行：
   ```bash
   cargo run -p bifrost-cli --bin bifrost -- enhanced-proxy disable
   ```
2. 执行 status：
   ```bash
   cargo run -p bifrost-cli --bin bifrost -- enhanced-proxy status --format json
   ```

**预期结果**：
- disable 命令退出码为 0。
- `configured_enabled: false`。
- `enabled: false`。

### TC-MEP-04：Admin API GET/PUT 状态闭环

**操作步骤**：
1. 启动临时 Bifrost：
   ```bash
   cargo run -p bifrost-cli --bin bifrost -- start -p 18880 --skip-cert-check --no-system-proxy
   ```
2. 查询状态：
   ```bash
   curl -s http://127.0.0.1:18880/_bifrost/api/proxy/enhanced
   ```
3. 开启 desired：
   ```bash
   curl -s -X PUT http://127.0.0.1:18880/_bifrost/api/proxy/enhanced \
     -H 'content-type: application/json' \
     -d '{"enabled":true}'
   ```
4. 关闭 desired：
   ```bash
   curl -s -X PUT http://127.0.0.1:18880/_bifrost/api/proxy/enhanced \
     -H 'content-type: application/json' \
     -d '{"enabled":false}'
   ```

**预期结果**：
- GET 返回 200 和 JSON。
- 开启后 `configured_enabled: true` 且 `proxy_port: 18880`。
- helper/controller 未连接前 `enabled` 不得为 true。
- 关闭后 `configured_enabled: false` 且 `enabled: false`。

### TC-MEP-05：Web UI Settings Proxy 展示增强模式诊断

**操作步骤**：
1. 打开 `http://127.0.0.1:18880/_bifrost/`。
2. 进入 Settings -> Proxy。
3. 找到 `Enhanced Local Capture`。
4. 切换开关为开启，再关闭。

**预期结果**：
- 页面显示 `Enhanced Local Capture` 开关和状态 tag。
- 开启后状态 tag 不得显示 Running，除非 controller socket 已连接。
- helper 缺失、extension 缺失或待授权时页面显示对应诊断和下一步动作。
- 关闭后开关回到关闭状态。

### TC-MEP-06：macOS helper 授权边界

**操作步骤**：
1. 在 macOS 上用签名 entitlement 构建 `apps/macos-enhanced-proxy`。
2. 将 `Bifrost Enhanced Proxy.app` 安装到 `/Applications`，或设置：
   ```bash
   export BIFROST_ENHANCED_PROXY_APP="/path/to/Bifrost Enhanced Proxy.app"
   ```
3. 执行 `enhanced-proxy enable`。
4. 打开 helper app，并在 macOS 系统设置中批准 Network Extension。
5. 再次执行 `enhanced-proxy status`。

**预期结果**：
- helper 缺失时状态为 `helper_missing`。
- helper 存在但 extension 缺失时状态为 `extension_missing`。
- extension 存在但 controller 未连接时状态为 `approval_required`。
- 只有 controller socket 存在并连接时状态才允许为 `running`，`enabled` 才允许为 true。

## 清理步骤

1. 停止临时 Bifrost。
2. 删除临时数据目录：
   ```bash
   rm -rf "$BIFROST_DATA_DIR"
   ```
3. 如安装过 helper app，按 macOS 系统扩展卸载流程移除测试版 helper。
