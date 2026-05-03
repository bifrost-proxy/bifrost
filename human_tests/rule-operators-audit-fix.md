# 规则操作符审计修复验证

## 功能模块说明

本测试验证审计报告 `ap.md` 中 P0 和 P1 问题的修复：
1. P0-1: `forwardedFor` applier 实现（X-Forwarded-For 链式追加）
2. P0-2: `responseFor` applier 实现（x-bifrost-response-for 响应头）
3. P0-3: `pac` 标记为未实现
4. P1-1: test_rules.sh 中 fake-pass 清理（改为 _log_fail）

## 前置条件

1. 启动 Bifrost 代理：
```bash
BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- start -p 8800 --unsafe-ssl --no-system-proxy
```

2. 启动 echo server（用于验证请求头透传）：
```bash
cd e2e-tests && node test_data/echo_server.js &
```

## 测试用例

### TC-ROA-01: forwardedFor 设置固定 IP

**操作步骤**：
1. 通过 API 创建规则：`forwardedFor-test.local http://127.0.0.1:3000 forwardedFor://1.2.3.4`
2. 通过代理请求 `http://forwardedFor-test.local/`
3. 检查 echo server 返回的请求头中 `x-forwarded-for` 值

**预期结果**：echo server 收到的请求头包含 `x-forwarded-for: 1.2.3.4`

### TC-ROA-02: forwardedFor 链式追加

**操作步骤**：
1. 使用同一规则，发送请求时携带 `X-Forwarded-For: 10.0.0.1` 请求头
2. 检查 echo server 返回的请求头中 `x-forwarded-for` 值

**预期结果**：echo server 收到的请求头包含 `x-forwarded-for: 10.0.0.1, 1.2.3.4`

### TC-ROA-03: responseFor 设置响应头

**操作步骤**：
1. 通过 API 创建规则：`responseFor-test.local http://127.0.0.1:3000 responseFor://upstream-1`
2. 通过代理请求 `http://responseFor-test.local/`
3. 检查响应头

**预期结果**：响应头包含 `x-bifrost-response-for: upstream-1`

### TC-ROA-04: pac 协议标记为未实现

**操作步骤**：
1. 读取 `crates/bifrost-core/src/syntax.rs` 中 pac 协议的描述
2. 确认描述包含 "not yet implemented"

**预期结果**：pac 描述为 "PAC proxy auto-config (not yet implemented)"

### TC-ROA-05: test_rules.sh fake-pass 已清理

**操作步骤**：
1. 在 `e2e-tests/test_rules.sh` 中搜索 `else` 分支中的 `_log_pass ".*已配置"`
2. 确认不存在 else 分支中的 fake-pass 模式

**预期结果**：无 `else` 分支中的 `_log_pass "规则已配置"` 或类似伪通过

### TC-ROA-06: 单元测试通过

**操作步骤**：
1. 运行 `cargo test -p bifrost-proxy -- forwarded_for`
2. 运行 `cargo test -p bifrost-proxy -- response_for`

**预期结果**：所有 forwarded_for 和 response_for 相关单元测试通过

## 清理步骤

```bash
rm -rf ./.bifrost-test
```
