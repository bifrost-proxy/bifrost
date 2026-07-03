# Replay 转发规则路径前缀重写

## 背景

Bifrost 的普通代理链路在遇到 `http(s)://source http(s)://target/prefix` 这类带路径前缀的转发规则时，会剥离原请求中已经匹配的 source 路径前缀，再把剩余 suffix 拼到 target 路径后。这样：

```text
Rule:    https://internal.example.test/labor_cost/static/ http://localhost:9000/labor_cost/static/
Request: https://internal.example.test/labor_cost/static/07c1d7e1fb3e13436b958af5f90ec9c8.svg
Target:  http://localhost:9000/labor_cost/static/07c1d7e1fb3e13436b958af5f90ec9c8.svg
```

历史上 Replay（`POST /_bifrost/api/replay/execute` 和 `execute/sse`）没有走同一套 path 重写，直接拼接会得到 `/labor_cost/static/labor_cost/static/07c1d7e1...` 这种双重前缀，本地上游 (dev server / mock) 返回 404 或 502。

同时还要覆盖 path wildcard 场景：pattern 以 `^` 开头时代表 target path 需要精确使用规则声明值，不再从原请求剥离 suffix，这是普通代理链路已有的语义。

本方案让 Replay 与普通代理转发链路对齐，共用同一套 source path suffix rewrite / exact target path 语义。

## 用户目标验证清单

### 必须实现

- `http/https/ws/wss` 类型的 Replay 转发规则，遇到带路径前缀 pattern 时按 suffix 语义重写目标 URL。
- pattern 以 `^` 前缀标记时，target path 保持规则声明值 exact，不再吞入 request suffix。
- Replay 转发保留原请求 query string。
- 目标 URL 不含 path（例如 `http://localhost:9000`）时，Replay 完全保留原 request path，与老行为一致。
- `Passthrough://` 命中后清空已记录的 forward source path，避免残留干扰。

### 必须不破坏

- `host://IP`、`xhost://`、`dns://` 等非 URL 重写型规则不受影响。
- 普通代理链路的 path rewrite 行为保持原样。
- Replay 的 request/response 修改规则（`reqHeaders`、`resBody` 等）继续生效。
- 已启用规则 + 自定义规则叠加时，与原优先级一致。

### 必须真实验证

- 单元测试覆盖：suffix 重写、query 保留、无 target path 保持原样、custom rule / path wildcard 两种来源。
- E2E 测试通过 mock echo server 验证真实的转发目标路径。

## 产品语义

Replay 与普通代理必须共享同一套 URL forwarding 语义，让用户拿一条 `.bifrost` 规则文件既可以喂给主端口，也可以喂给 Replay。这项修复的核心语义有两个：

1. **Suffix 语义（默认）**：规则 pattern 含 path 前缀时，把请求 path 中匹配 pattern path 的部分剥离，剩余 suffix 追加到 target path 后。
2. **Exact 语义（`^` 前缀）**：pattern 以 `^` 开头（可与 `!` 组合出现，如 `!^foo`）时，target path 保持规则声明值不变，用于 path wildcard 情形。

`Passthrough` 命中意味着后续所有转发类规则失效，因此必须清空已经缓存的 forward source path，避免把 passthrough 之后的规则错误参与到 path rewrite。

## 技术细节

### 结构体：`AppliedRules`

`crates/bifrost-admin/src/request_rules.rs`：

```rust
pub struct AppliedRules {
    // ...
    pub forward_url: Option<Url>,
    pub forward_source_path: Option<String>,
    pub forward_target_path_exact: bool,
    pub forwarding_passthrough: bool,
    pub host: Option<String>,
    // ...
}
```

### 核心函数

- `extract_path_from_pattern(pattern: &str) -> Option<String>`
  - 从规则 pattern 中提取 path 部分（去掉 scheme/host、去掉前置 `!` 与 `^` 标记）。
- `rewrite_path_with_prefix(request_path: &str, source_path: Option<&str>, target_path: &str) -> String`
  - 若 `source_path.is_some()`，trim 末尾 `/`，判断 `request_path` 是否以其开头。
  - 是：取 `request_path` 剥离 source 后的 suffix，与 `target_path` 拼接（避免双 `/`）。
  - 否：返回 `target_path` 原值（保守回退）。

### `build_applied_rules` 关键分支

```rust
Rule::Forward(rule) if !applied.forwarding_passthrough && applied.forward_url.is_none() => {
    // 记录首条转发规则的 target URL、source path、exact 标记
    applied.forward_url = Some(target_url);
    applied.forward_source_path = extract_path_from_pattern(&rule.rule.pattern);
    applied.forward_target_path_exact = pattern_starts_with_caret_after_bang(&rule.rule.pattern);
}
Rule::Host(rule) if !applied.forwarding_passthrough && applied.host.is_none() => {
    applied.host = Some(...);
}
Rule::Passthrough(_) if !applied.forwarding_passthrough => {
    applied.forwarding_passthrough = true;
    applied.forward_source_path = None;
    applied.forward_target_path_exact = false;
}
```

### 执行时应用

在 unified replay 构造最终目标 URL 时（`crates/bifrost-admin/src/request_rules.rs` 附近 line 324）：

```rust
let rewritten_path = if applied_rules.forward_target_path_exact {
    target_url_path.to_string()
} else {
    rewrite_path_with_prefix(
        original_request_path,
        applied_rules.forward_source_path.as_deref(),
        target_url_path,
    )
};
```

Query string 保留：单独拼接原 request 的 query。

### 无 target path 兼容

`target_url` path 为空或 `/` 时，直接保留原 request 的 path 与 query，不做 rewrite。

## CLI / Web / Admin API

### Admin API

- `POST /_bifrost/api/replay/execute`
- `POST /_bifrost/api/replay/execute/sse`
- Replay WebSocket 执行入口

三条入口共享 `build_applied_rules` 与 rewrite 逻辑；请求 body 保持不变，转发目标 URL 由 rewrite 结果决定。

### Web UI

Replay 页面不新增控件；预览 URL / 执行结果自然反映新的 rewrite 结果。响应区若命中 forward 规则，请求头 `X-Bifrost-Rules` 或 traffic matched rules 应能显示命中规则。

### CLI

不新增 CLI 子命令。Replay 目前主要通过 Web / Admin API 调用；如果未来 CLI 提供 replay 子命令，也应共享 `build_applied_rules`。

## Sync 边界

- 转发规则本身可能来自本地规则或 Group 规则，Sync 行为不变。
- Replay 自定义规则文本随执行请求一次性传入，不落地不 Sync。

## 实现切分

### Phase 1：结构体与提取器

- `AppliedRules` 增加 `forward_source_path`、`forward_target_path_exact`、`forwarding_passthrough` 字段。
- `extract_path_from_pattern` / `rewrite_path_with_prefix` / caret 判定函数完备。
- 单元测试覆盖辅助函数。

### Phase 2：`build_applied_rules` 集成

- Forward / Host / Passthrough 分支按上表逻辑更新。
- 保证 passthrough 之后不再累加 forward 状态。

### Phase 3：Replay 执行链路

- 应用新的 rewrite 分支到 unified replay 请求构造。
- 保留 query。
- 无 target path 场景保持旧行为。

### Phase 4：测试与 human_tests

- 补齐单元 + E2E。
- 更新 `human_tests/api-replay.md` TC-ARP-18。
- 更新 `human_tests/readme.md` Replay API 用例数量。

## 测试方案

### 单元测试（`crates/bifrost-admin/src/request_rules.rs`）

- `test_apply_forward_rule_uses_suffix_after_matched_source_path`（line 1197）
- `test_apply_forward_rule_preserves_query_after_prefix_rewrite`（line 1228）
- `test_apply_forward_rule_without_target_path_preserves_original_path`（line 1252）
- `test_apply_forward_rule_from_resolved_custom_rule_uses_source_path_suffix`（line 1273）
- `test_apply_forward_rule_from_path_wildcard_uses_exact_target_path`（line 1296）
- `test_passthrough_blocks_later_forward_rule_from_resolved_rules`（line 1316）

### E2E 测试

- Fixture：`e2e-tests/rules/replay/path_prefix_forward.txt`
- 运行器：`e2e-tests/run_all_tests_parallel.sh` 已经把该 fixture 列入 replay 集合 (line 221)。
- 用例：`e2e-tests/tests/test_replay_rules.sh::test_path_prefix_forward_rule`（line 356，line 1124 dispatch 调用）
  - 启动本地 HTTP echo server。
  - 通过 `POST /_bifrost/api/replay/execute` 发起 `https://internal.example.test/labor_cost/static/xxx.svg`。
  - 断言 echo server 收到 `parsed_path=/labor_cost/static/xxx.svg`，不出现双重前缀。

### human_tests

- `human_tests/api-replay.md::TC-ARP-18`（line 360）“带路径前缀的转发规则不会重复拼接路径”。
- `human_tests/readme.md` Replay API 一行更新用例数量描述。

## Review / Fix / Test 闭环

### 第 1 轮

- 复核目标：suffix 重写、query 保留、无 target path 保持原样、path wildcard exact、passthrough 清空。
- 复核 diff：`request_rules.rs`、replay handler、E2E fixture / 用例、human_tests 是否同步。
- 重点 review：pattern 里含 `!^` 或 `^!` 前缀时的 caret 判定；query 中含 `#` 分片时的解析是否正确。
- 复测：
  - `cargo test -p bifrost-admin request_rules -- --nocapture`
  - `bash e2e-tests/tests/test_replay_rules.sh test_path_prefix_forward_rule`

### 第 2 轮

- 复核第 1 轮问题修复。
- 再次 diff 检查 human_tests 数量一致性。
- 重点 review：与 host / xhost / dns 规则组合时，path rewrite 是否会误触发。

## 风险与决策点

- **风险**：`extract_path_from_pattern` 对边界字符（末尾 `/`、query 混入 pattern 等）如果处理不严，可能出现 off-by-one。缓解：辅助函数配套单元测试，覆盖 `/labor_cost/static/`、`/labor_cost/static`、`labor_cost/static/` 等变体。
- **风险**：Group 规则中的路径前缀转发不常见但存在，需要 group 场景实测。
- **决策**：只在首条 forward 规则记录 source path，后续 forward 忽略。若产品需求变为“多条 forward 叠加 rewrite”，另立设计。
- **决策**：不引入独立 Replay 专用 rewrite；共享普通代理链路的语义降低维护成本。
