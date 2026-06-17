# Replay 转发规则路径前缀重写

## 功能模块说明

Replay 执行自定义规则或已启用规则时，`http://` / `https://` / `ws://` / `wss://` 转发规则必须与普通代理转发链路保持一致：如果规则的匹配 pattern 带路径前缀，转发到带路径前缀的目标地址时，只追加原请求中超出匹配前缀的 suffix。

示例：

```text
https://internal.example.test/labor_cost/static/ http://localhost:9000/labor_cost/static/
```

Replay 请求：

```text
https://internal.example.test/labor_cost/static/07c1d7e1fb3e13436b958af5f90ec9c8.svg
```

应转发到：

```text
http://localhost:9000/labor_cost/static/07c1d7e1fb3e13436b958af5f90ec9c8.svg
```

不应转发到重复路径：

```text
http://localhost:9000/labor_cost/static/labor_cost/static/07c1d7e1fb3e13436b958af5f90ec9c8.svg
```

## 实现逻辑

- `crates/bifrost-admin/src/request_rules.rs` 的 `build_applied_rules` 在构建 Replay 的 `AppliedRules` 时，对首条 `http/https/ws/wss` 转发规则调用 `extract_path_from_pattern`，将 source path 保存到 `AppliedRules.forward_source_path`；遇到 `Passthrough` 规则时会清空该字段。
- 同时根据 pattern 是否以 `^` 开头（去掉前置 `!` 后判断）记录 `forward_target_path_exact`，用于 path wildcard 规则保持目标 path 精确匹配。
- 实际重写在 `rewrite_path_with_prefix(request_path, source_path, target_path)` 中完成：用 `source_path`（trim 末尾 `/`）去剥离 `request_path` 前缀，再把剩余 suffix 拼接到 `target_path` 之后，避免目标路径出现重复段。
- 若目标 URL 不含 path 或 `source_path` 为空，则保持旧行为：仅替换 scheme/host/port，原始 path/query 原样保留。
- query string 必须在 prefix rewrite 后继续保留。

## 依赖项

- `bifrost-core` 的规则解析与匹配结果提供 `rule.pattern`、`rule.protocol` 和 `resolved_value`。
- `url` crate 用于解析和重建 Replay 的最终 URL。

## 测试方案

- 单元测试：
  - `test_apply_forward_rule_uses_suffix_after_matched_source_path` 验证 `labor_cost/static` 不重复。
  - `test_apply_forward_rule_preserves_query_after_prefix_rewrite` 验证 query string 保留。
  - `test_apply_forward_rule_without_target_path_preserves_original_path` 验证无目标 path 的旧行为不变。
  - `test_apply_forward_rule_from_resolved_custom_rule_uses_source_path_suffix` 与 `test_apply_forward_rule_from_path_wildcard_uses_exact_target_path` 覆盖 custom rule 与 path wildcard 两种来源下的 suffix / exact 行为。
- E2E 测试：
  - `e2e-tests/rules/replay/path_prefix_forward.txt` 定义 HTTPS origin 到本地 HTTP echo server 的路径前缀转发规则。
  - `e2e-tests/tests/test_replay_rules.sh` 新增 `test_path_prefix_forward_rule`，通过 Replay Admin API 执行请求并断言 echo server 收到的 `parsed_path` 正确。
- 真实场景测试：
  - 更新 `human_tests/api-replay.md`，新增 `TC-ARP-18`，按 API 场景验证 Replay 路径前缀转发不会重复路径。
  - 同步更新 `human_tests/readme.md` Replay API 用例数量。

## 校验要求

- 先执行 Replay 相关单元测试和 E2E 测试。
- 再执行 `cargo fmt --all -- --check`。
- 再执行 `cargo clippy --workspace --all-targets --all-features -- -D warnings`。
- 最后至少执行一次 `cargo test --workspace --all-features`。

## 文档更新要求

- 本设计文档记录 Replay 与普通代理转发链路的 URL path 语义。
- `human_tests/api-replay.md` 与 `human_tests/readme.md` 必须同步更新。
