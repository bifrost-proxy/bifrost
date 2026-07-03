# statusCode 规则流水线修复方案

## 背景

`statusCode://code` 是 Bifrost 规则语法中最常用的“本地生成响应”原语：命中即由代理侧构造响应，不发送到上游。产品语义要求 statusCode 只“阻断上游请求”，并不要求“阻断同条规则内的其他协议行为”。因此同一规则内的：

- `reqHeaders://` / `reqBody://` / 请求脚本
- `resHeaders://` / `resBody://` / `resAppend://` / 响应脚本
- Traffic 记录的最终 request/response 内容

都必须按请求 → 本地响应 → 响应改写的顺序执行，并在 Traffic 中可观察。

修复前的实现存在两条错误短路路径：

1. HTTP 明文 handler（`crates/bifrost-proxy/src/proxy/http/handler.rs`）在请求规则处理**之前**判定 `is_direct_status_response`，导致 `reqHeaders/reqBody` 未生效。
2. HTTPS MITM tunnel（`crates/bifrost-proxy/src/proxy/http/tunnel/mod.rs`）走 `statusCode` 直接响应分支时也未执行响应体规则与响应脚本，Traffic 中只能看到 status_code 与固定 body，与 HTTP 分支不一致。

修复目标是让两条路径都在“不请求 upstream”的前提下补齐规则流水线并统一 Traffic 展示。

## 用户目标验证清单

### 必须实现

- 单条规则同时写 `statusCode://` 与请求侧规则（`reqHeaders://` / `reqBody://`）时，请求侧改写生效并在 Traffic 中可见。
- 单条规则同时写 `statusCode://` 与响应侧规则（`resHeaders://` / `resBody://` / `resAppend://`）时，响应侧改写生效并在 Traffic 中可见。
- 单条规则同时写 `statusCode://` 与请求/响应脚本时，脚本在对应阶段执行，副作用（extra_headers、body 改写、日志）都被采集。
- HTTP 明文与 HTTPS MITM 两条路径在同一规则下有相同的可观察结果（status_code、req/res headers、req/res body、matched rules）。
- 命中 `statusCode://` 后不建立上游连接、不发送任何字节到 upstream。

### 必须不破坏

- `statusCode://` 仍是不请求 upstream 的语义。
- `replaceStatus://` 仍是“先请求 upstream 再替换 status”，不被 statusCode 影响。
- `redirect://` / `locationhref://` 现有短路语义不变。
- 无 `statusCode://` 的规则组合行为完全不变，回归 E2E 覆盖率不下降。
- Traffic 记录格式与已有 recorder schema 不变；只补齐字段值。

### 必须真实验证

- 新增 E2E 覆盖 `statusCode + reqHeaders + reqBody + resHeaders + resBody + resAppend`，直连 mock upstream 断言请求计数为 0。
- 手测复现修复前场景：单条规则命中后应能在 Traffic 详情看到 `reqHeaders` 已改写并附加自定义头，`resBody` 已从 base 拼成 `base-tail`。
- 覆盖 HTTPS 场景：CONNECT + MITM + `statusCode` 组合规则同样验证。

## 产品语义

`statusCode://` 的完整语义定义为：

1. 请求侧：正常执行请求规则（`reqHeaders/reqBody/reqAppend/reqReplace` 与请求脚本），生成最终请求体。
2. 上游侧：不建立 TCP、不发起 HTTP 请求，视为“本地代理生成响应”。
3. 响应生成：使用规则中的 `statusCode` 作为初始响应状态码，`resBody` 作为初始响应体（缺省为空），可继续被 `resAppend/resReplace` 与响应脚本改写。
4. Traffic：记录改写后的最终请求头/请求体与最终响应头/响应体，`upstream_request_count=0`。

与 `replaceStatus://` 的区别继续保留：`replaceStatus` 必须先联通 upstream，再在响应阶段替换 status；因此 `replaceStatus` 走的是普通 upstream forward + response transform 路径，不受本方案影响。

## 技术细节

### HTTP 明文路径（`proxy/http/handler.rs`）

关键落地点：

- `resolved_rules` 的判定移到请求规则处理之后：
  ```text
  if is_direct_status_response(&resolved_rules) {
      // resolved_rules 已包含 statusCode，并已消费 reqHeaders/reqBody 阶段。
      let status = resolved_rules.status_code.unwrap_or(200);
      let mut mock_response = build_status_response(status, &resolved_rules);
      // 继续执行响应体规则与响应脚本。
  }
  ```
- `is_direct_status_response()` 定义在同文件 line 221，判据只看 `rules.status_code.is_some()`。
- Traffic recorder 在请求侧改写完成后 flush 请求 body，本地响应生成后 flush 响应 body。

### HTTPS MITM 路径（`proxy/http/tunnel/mod.rs`）

关键落地点：

- tunnel 已解密 client hello 并处于 MITM 状态时，仍走 `resolved_rules.status_code.is_some()` 分支（line 2015 附近）。
- 分支内构造 `build_redirect_response` 或 `serve_mock_file` 的等价 statusCode 响应；随后调用响应体规则改写与响应脚本执行。
- 修复代码见 `crates/bifrost-proxy/src/proxy/http/tunnel/mod.rs` line 2697 附近的 `if let Some(status) = resolved_rules.status_code { ... }` 段，注释 `#188 "preserve statusCode rule pipeline"` 明确说明该修复来自本方案。
- MITM 分支执行 `resScript` 的补丁位于同一文件 line 2736 注释块附近。

### 通用注意事项

- 请求 body 超限时（默认 `REQ_BODY` 上限），跳过 body 规则并写 warn，避免大 body 直接响应崩溃；日志见 handler.rs line 935 `body too large ..., skipping body rules and scripts for statusCode direct response`。
- Traffic recorder 必须先记录本地生成响应的 matched rules，再由响应脚本注入的 extra headers 补齐；已存在的 recorder API 不需要新增字段。

## CLI / Web / Admin API 边界

本方案是规则执行 pipeline 修复，不新增用户面命令：

- CLI：`bifrost rule update` / `bifrost proxy start --rule` 现有语义完全复用。
- Web：Rules 编辑器和 Traffic 详情页无需改动，Traffic 详情自然会显示改写后的 req/res。
- Admin API：`GET /_bifrost/api/traffic/:id` 不变；只是响应中的 `request.headers`、`request.body`、`response.headers`、`response.body` 会包含改写结果。

## Sync 边界

- 规则文件本身通过 Sync 分发，`statusCode + reqHeaders + reqBody + resXxx` 的组合在同步后行为一致。
- 无需新增同步字段。

## Phase 1 - 4

### Phase 1：HTTP 明文修复

- 移动 `is_direct_status_response` 分支到请求规则处理之后。
- 补齐 Traffic 请求 body 改写记录。
- 单元测试与 E2E 局部验证。

### Phase 2：HTTPS MITM 修复

- 在 MITM 分支同样先执行请求规则，再进入 statusCode 分支。
- 补齐响应体规则、响应脚本、Traffic。
- 与 Phase 1 输出对齐后统一 E2E 断言。

### Phase 3：E2E 与 human_tests

- 新增 `status_statusCode_preserves_rule_pipeline`（`crates/bifrost-e2e/src/tests/status_redirect.rs::test_statuscode_preserves_rule_pipeline`）。
- 在 `human_tests/status-code-direct-response.md` 中新增 `TC-SCDR-03`（已落地）。
- 保持 `status_statusCode_direct_no_upstream`（`test_statuscode_direct_no_upstream`）断言不请求 upstream。

### Phase 4：文档与回归

- 更新 `design/status-code-rule-pipeline.md`（本文件）反映真实实现细节。
- 复跑现有 `status_statusCode_*` 组测试确认无回归。
- 保留 `test_combined_statuscode_headers` 覆盖历史组合。

## 测试方案

### E2E 用例

位置：`crates/bifrost-e2e/src/tests/status_redirect.rs`

- `status_statusCode_404` — 单一 statusCode 返回。
- `status_statusCode_500` / `status_statusCode_200` — 基础状态码矩阵。
- `status_statusCode_with_body` — statusCode + resBody 基础组合。
- `status_statusCode_direct_no_upstream` — 断言 upstream 请求计数为 0。
- `status_statusCode_preserves_rule_pipeline` — 本方案核心用例：`statusCode://451` + `reqHeaders`/`reqBody`/`resHeaders`/`resBody`/`resAppend` 全部命中，upstream 请求计数为 0，Traffic 记录请求侧改写。
- `status_combined_statusCode_headers` — statusCode + resHeaders 组合。
- `status_replaceStatus_200` — 边界回归：`replaceStatus` 仍请求 upstream 后再替换 status。

### 单元测试

`crates/bifrost-proxy/src/proxy/http/handler.rs::tests` 已存在的组合覆盖：

- `extra_build_overridden_error_response_prefers_status_code_over_replace_status`（line 7680 附近）
- `extra_build_overridden_error_response_uses_replace_status_when_no_status_code`（line 7697 附近）
- `build_connection_error_response_uses_bad_gateway_for_invalid_status_code`（line 6420）

### human_tests

`human_tests/status-code-direct-response.md`：

- `TC-SCDR-01`：statusCode + host 直接返回且不请求 upstream。
- `TC-SCDR-02`：replaceStatus 仍请求 upstream。
- `TC-SCDR-03`：statusCode 不短路请求/响应规则流水线（本方案新增，验证 Traffic 中 req/res 改写全部生效）。

### 编译与静态校验

- 最小编译验证：`cargo check -p bifrost-proxy -p bifrost-e2e`。
- 收尾校验：`cargo fmt --all -- --check`；`cargo clippy --workspace --all-targets --all-features -- -D warnings`。

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核用户目标：statusCode 不短路请求/响应规则流水线。
- 复核 diff：HTTP 与 HTTPS 两条路径都调整；is_direct_status_response 触发点在请求规则处理之后；MITM 分支执行 resScript。
- 复测：`cargo check`；`status_statusCode_preserves_rule_pipeline`；`status_statusCode_direct_no_upstream`；一次 `TC-SCDR-03` 真实手测。

### 第 2 轮

- 复核第 1 轮问题修复与 diff 收敛。
- 复查 human_tests 索引与格式；确认 Traffic 详情页展示与描述一致。
- 复测：`status_statusCode_*` 全套 + `status_replaceStatus_200`；`cargo test --workspace --all-features`（本地视需要跳过 network-heavy）。

## 风险与决策

- 修复引入的 body 改写路径必须严格保留 `body too large` 短路，避免大 body 触发 OOM。
- 请求脚本抛错时不应回退到旧短路路径；应继续按 statusCode 生成响应并将脚本错误写入 Traffic script log。
- Traffic 记录顺序：先写 req 改写，再写本地生成响应；`recorder` API 已支持顺序写入，不需要新增接口。
- 未来若引入更复杂的 statusCode extension（例如流式 body），需要重新评估 pipeline 顺序；本方案锁定当前一次性 body 语义。
- 回滚方案：若真实用户反馈 statusCode 语义变化，可以在 handler/tunnel 分支加短路开关 `BIFROST_STATUSCODE_LEGACY_SHORTCIRCUIT=1`；不推荐默认启用。
