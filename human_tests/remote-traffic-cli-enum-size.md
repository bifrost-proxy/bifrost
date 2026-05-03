# Remote Traffic CLI 枚举体瘦身真实场景测试

## 功能模块说明

验证 `bifrost remote traffic list/get/search` 在 CLI 内部结构调整后仍保持既有行为，重点确认：

- `remote traffic list` 的大量过滤参数仍可正常解析并透传
- `remote traffic get` 仍可按请求 ID/sequence 获取详情
- `remote traffic search` 仍可按 `--max-results` / `--max-scan` 执行搜索

本文件用于本次 `RemoteTrafficCommands` large enum variant 修复的真实场景回归。

## 前置条件

1. 仓库位于 `<REPO_ROOT>`
2. 可在本地执行 Cargo 命令
3. 使用临时数据目录，且测试端口不能使用 `9900`
4. 如需启动 Bifrost，必须带 `--no-system-proxy`

建议准备命令：

```bash
export BIFROST_DATA_DIR="$(mktemp -d /tmp/bifrost-remote-cli-XXXXXX)"
cargo run --bin bifrost -- start -p 8800 --unsafe-ssl --no-system-proxy
```

## 测试用例列表

### TC-RTCE-01：`remote traffic list` 过滤参数仍可完整解析

**操作步骤**：
1. 在仓库根目录执行：
   ```bash
   cargo test -p bifrost-cli test_build_remote_command_for_traffic_list_includes_all_filters -- --exact
   ```
2. 观察测试是否通过
3. 如测试失败，记录失败字段与错误信息

**预期结果**：
- 测试通过
- 生成的 `traffic.list` `args_json` 仍包含 `limit/cursor/direction/method/status/status_min/status_max/protocol/host/url/path/content_type/client_ip/client_app/has_rule_hit/is_websocket/is_sse/is_tunnel`

### TC-RTCE-02：`remote traffic search` 参数透传回归通过

**操作步骤**：
1. 在仓库根目录执行：
   ```bash
   cargo test -p bifrost-cli test_build_remote_command_for_traffic_search_uses_streaming_command -- --exact
   ```
2. 观察测试是否通过

**预期结果**：
- 测试通过
- `traffic.search` 生成的参数仍包含 `query/max_results/max_scan`

### TC-RTCE-03：Clippy 不再报 `large_enum_variant`

**操作步骤**：
1. 在仓库根目录执行：
   ```bash
   cargo clippy -p bifrost-cli --all-targets --all-features -- -D warnings
   ```
2. 检查输出中是否仍出现 `RemoteTrafficCommands` 的 `large_enum_variant`

**预期结果**：
- clippy 通过
- 不再出现 `RemoteTrafficCommands` large enum variant 报错

## 清理步骤

1. 删除临时数据目录：
   ```bash
   rm -rf "$BIFROST_DATA_DIR"
   ```
2. 如本次手动启动过 Bifrost，停止对应进程

## 本次执行结果

测试日期：2026-04-22

| 用例编号 | 用例名称 | 结果 | 说明 |
|------|------|------|------|
| TC-RTCE-01 | `remote traffic list` 过滤参数仍可完整解析 | ✅ PASS | 执行 `cargo test -p bifrost-cli build_remote_command_for_traffic_list_includes_all_filters -- --nocapture`，真实命中并通过；`traffic.list` 的 `args_json` 仍完整包含所有过滤字段。 |
| TC-RTCE-02 | `remote traffic search` 参数透传回归通过 | ✅ PASS | 执行 `cargo test -p bifrost-cli build_remote_command_for_traffic_search_uses_streaming_command -- --nocapture`，真实命中并通过；`traffic.search` 的 `query/max_results/max_scan` 保持不变。 |
| TC-RTCE-03 | Clippy 不再报 `large_enum_variant` | ✅ PASS | 执行 `cargo clippy -p bifrost-cli --all-targets --all-features -- -D warnings` 通过，`RemoteTrafficCommands` 不再出现 `large_enum_variant`。 |
