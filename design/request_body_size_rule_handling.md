# 请求体大小判断规则

## 背景

Bifrost 代理主链路在处理 HTTP/HTTPS/H3 请求时，需要在"是否读入 body 到内存"和"是否走流式转发"之间做取舍。历史上这个决策与"body 是否被规则替换 / 是否需要脚本处理"混在一起，导致：

- 用户配置了 `req_body://` / `res_body://` 直接替换规则，原 body 再大也不该影响该规则的执行。旧实现会因原 body 过大而放弃规则应用，静默降级到流式转发，请求端 hash 与预期不一致。
- 依赖读取原 body 的 `req_replace/res_replace/req_merge/res_merge/req_prepend/res_prepend/req_append/res_append/req_script/res_script`，在原 body 过大时无法可靠执行，应当明确 skip 并落盘诊断，而不是死等或 OOM。
- HTTP handler 与 tunnel handler 两条链路存在同类判断的重复实现，一处修一处漏。

本设计的目标：显式把"body override"（`req_body`/`res_body` 存在时不读原 body）与"需要原 body 才能处理"两类路径拆开，并统一交由 `max_body_buffer_size` / `max_body_probe_size` 两个可配置阈值控制读取上限；HTTP 与 tunnel 两条链路一致。

## 用户目标验证清单

### 必须实现

- `req_body` / `res_body` 直接替换规则不因原 body 尺寸失效：无论原 body 多大都能应用替换。
- 依赖原 body 的规则和脚本（replace / prepend / append / merge / script）在原 body 大小超过 `max_body_buffer_size` 时明确 skip，走流式转发，并在诊断中标记。
- `max_body_probe_size`（默认 64 KiB）用于探测预读，`max_body_buffer_size`（默认 10 MiB）用于最终缓存上限；两者可在 Admin API 中配置。
- HTTP handler 和 tunnel handler 两处判断使用同一实现语义，避免链路不一致。
- Traffic 记录能看到 body 是否被截断、规则是否被 skip。

### 必须不破坏

- 不影响未启用 body 相关规则的普通请求性能（应保持 zero-copy 流式转发）。
- Content-Length 已知但小于 buffer 上限的请求继续被完整读取以支持规则。
- 已有的 chunked / streaming body 语义保留；非 body 规则（host / status / header / dns / breakpoint 等）不受影响。
- H2/H3 与 WebSocket upgrade 路径不因判断顺序变化产生额外内存占用。

### 必须真实验证

- `e2e-tests/rules/advanced/body_size_strategy.txt` 与 `e2e-tests/tests/test_res_body_override_large.sh` 通过。
- 手工用 10 MiB 以上的响应体 + `res_body://short-answer` 规则验证响应端 hash 与规则一致。

## 产品语义

### 三类 body 处理路径

1. **Override 路径**：`req_body`/`res_body` 已被解析为 `Option<Bytes>`。无论原 body 多大，都直接用 Bytes 替换，跳过原 body 读取。适用于所有大小的原 body。
2. **需读原 body 路径**：`req_replace/req_merge/req_prepend/req_append` 或 `req_script`（响应侧同名）需要拿到原始 body 内容才能执行。原 body 尺寸受 `max_body_buffer_size` 约束；超出则 skip 该规则，改走流式转发。
3. **无需处理路径**：既没有 override，也不需要读 body 的规则/脚本，直接零拷贝流式转发。

### 关键字段

- `has_req_body_override = resolved_rules.req_body.is_some()`
- `has_res_body_override = resolved_rules.res_body.is_some()`
- `needs_req_body_read = !has_req_body_override && (needs_req_processing || has_req_scripts)`
- `needs_res_body_read = !has_res_body_override && (needs_res_processing || has_res_scripts)`

只有 `needs_*_body_read == true` 时才进入 bounded read / probe read 流程。

### 尺寸阈值

- `max_body_buffer_size`：body 完整读入内存的最大上限。默认 10 MiB。超出则 skip body 规则。
- `max_body_probe_size`：探测预读大小，用于 chunked / 未知长度的场景。默认 64 KiB。`probe = max_body_probe_size.min(max_body_buffer_size)`。
- 两者都通过 `crates/bifrost-admin/src/handlers/config.rs` 提供的 `PUT /api/config` 修改，保存到 `crates/bifrost-admin/src/state.rs` 的 `AdminState`。

## 技术细节

### 请求侧（HTTP 主入口）

`crates/bifrost-proxy/src/proxy/http/handler.rs:879` 起：

```rust
let has_req_body_override = resolved_rules.req_body.is_some();
let needs_req_body_read = !has_req_body_override && (needs_req_processing || has_req_scripts);
if needs_req_body_read {
    // bounded read or probe read
    let probe = max_body_probe_size.min(max_body_buffer_size);
    // ...
} else if has_req_body_override {
    // 使用 resolved_rules.req_body 直接替换
}
```

### 请求侧（再处理入口，重定向 / Retry）

同名逻辑在 `crates/bifrost-proxy/src/proxy/http/handler.rs:1882` 再次出现，用于内部再次处理 request 时保持一致语义。

### 请求侧（HTTPS tunnel）

`crates/bifrost-proxy/src/proxy/http/tunnel/mod.rs:2328` 处：tunnel 内部 decrypt 后走同样判定；`has_req_body_override / needs_req_body_read` 字段一致。

### 响应侧

- HTTP：`crates/bifrost-proxy/src/proxy/http/handler.rs:3055`。
- Tunnel：`crates/bifrost-proxy/src/proxy/http/tunnel/mod.rs:3621`。

两处都用：

```rust
let has_res_body_override = resolved_rules.res_body.is_some();
let needs_res_body_read = !has_res_body_override && (needs_res_processing || has_res_scripts);
```

### 直接替换字段

`crates/bifrost-admin/src/request_rules.rs` 中 `ResolvedRequestRules` 保存 `req_body: Option<Bytes>` 与 `res_body: Option<Bytes>`。字段一旦是 `Some`，就代表 override 已生效，body 大小不再影响。

### 阈值来源

- `AdminState { max_body_buffer_size, max_body_probe_size, ... }`。
- `AppConfig` 中默认 `max_body_buffer_size = 10 * 1024 * 1024`；`max_body_probe_size = 64 * 1024`。
- Web 设置页可通过 `PUT /api/config` 修改。

## CLI + Web + Admin API

### CLI

- `bifrost config get max_body_buffer_size` / `bifrost config set max_body_buffer_size <bytes>`。
- `bifrost config get max_body_probe_size` / `bifrost config set max_body_probe_size <bytes>`。

### Web

- Settings → Advanced 面板暴露两个阈值。
- Traffic detail 显示 `body_truncated: true/false` 与 `body_rule_skipped: true/false` 便于诊断。

### Admin API

- `GET /api/config` 返回 `max_body_buffer_size` / `max_body_probe_size`。
- `PUT /api/config` 允许修改，需要合法整数与上限校验（默认允许 1 KiB ~ 1 GiB）。

## Sync 边界

- 阈值属于本机 admin 配置，不加入远端 sync；不同设备可以有不同上限。
- Rule 中的 `req_body://` / `res_body://` 保存在规则文件内，正常参与 rule sync；同步过来的规则自然复用本机阈值判断。
- 分享 URL 中包含 `req_body://` / `res_body://` 的规则不需要特殊处理。

## Phase 1 — 请求侧拆分

- HTTP 主入口 (`handler.rs:879`) 与再处理入口 (`handler.rs:1882`)：抽出 `has_req_body_override` / `needs_req_body_read`。
- Tunnel 主入口 (`tunnel/mod.rs:2328`)：同步。
- 单元与集成：`e2e-tests/rules/advanced/body_size_strategy.txt` 覆盖 req_body override + reqReplace skip。

## Phase 2 — 响应侧拆分

- HTTP 响应链 (`handler.rs:3055`) 与 tunnel 响应链 (`tunnel/mod.rs:3621`) 同步。
- `test_res_body_override_large.sh` 覆盖大响应 override 场景。

## Phase 3 — 阈值配置化

- `AppConfig` 增加 `max_body_buffer_size` / `max_body_probe_size` 默认值与 clamp。
- Admin API + CLI 配置暴露。
- Traffic 记录字段 `body_truncated`、`body_rule_skipped`。

## Phase 4 — 文档与诊断

- Rule 语法文档补充"依赖原 body 的规则在超限时会被 skip"。
- `human_tests/proxy-rules-advanced.md` 增加大 body override / skip 场景。
- Traffic 面板 UI 与 CLI `bifrost traffic get` 输出对应字段。

## 测试方案

### 单元测试

- `crates/bifrost-proxy` handler 单测：mock resolved_rules 分别覆盖：
  - `has_req_body_override=true, body>max_buffer` → 走 override。
  - `has_req_body_override=false, needs_req_body_read=true, body>max_buffer` → skip + 流式。
  - `has_req_body_override=false, needs_req_body_read=true, body<max_buffer` → 完整读入。
- 响应侧对称覆盖。

### E2E

- `e2e-tests/rules/advanced/body_size_strategy.txt`：req/res 侧的 body override + skip 断言。
- `e2e-tests/tests/test_res_body_override_large.sh`：大响应 body override 端到端 hash 一致。
- `e2e-tests/rules/COVERAGE.md` 中 `reqBody / reqPrepend / reqAppend / reqReplace / resBody / resPrepend / resAppend / resReplace / resMerge / htmlAppend` 均引用 `advanced/body_size_strategy.txt` 作为大 body 场景。

### 真实场景 human_tests

- `human_tests/proxy-rules-advanced.md`：
  - TC-PRA-BS-01：>10 MiB 响应 + `res_body://short` 规则，客户端收到 override。
  - TC-PRA-BS-02：>10 MiB 响应 + `res_replace://...` 规则，被 skip，客户端拿到原 body 但 traffic 标记 body_rule_skipped。
  - TC-PRA-BS-03：调低 `max_body_buffer_size` 后小 body 规则仍能生效。
- `human_tests/readme.md`：同步用例编号。

### 环境约束

- 临时 `BIFROST_DATA_DIR`，非 9900 端口，`--no-system-proxy`。
- 测试 mock 使用 `test_utils` 中的 large body server；避免直接下载公网大文件。

## Review/Fix/Test 闭环

### 第 1 轮

- 复核 HTTP 与 tunnel 两处判断字段命名与判定顺序是否一致。
- 复核 `has_*_body_override` 与 `needs_*_body_read` 的短路是否覆盖 script only 场景。
- 运行 body_size_strategy.txt 与 test_res_body_override_large.sh。

### 第 2 轮

- 基于第 1 轮修复复查 diff：确保阈值 clamp 生效、超限日志包含请求 id。
- 复跑失败用例并验证 Traffic 面板中 `body_truncated` / `body_rule_skipped` 字段展示。

## 风险与决策

- 决策：把 override 和"需读原 body"完全拆开，避免旧代码用同一 flag 同时控制两类路径。
- 决策：`max_body_buffer_size` 默认 10 MiB、`max_body_probe_size` 默认 64 KiB；这是当前实测下"大部分 API 响应可完整落盘"与"避免 OOM"之间的折中。
- 决策：H3/H2 与 WebSocket upgrade 沿用 HTTP 主链路的判定；不为其单独设阈值。
- 风险：`max_body_buffer_size` 设得过大可能带来单请求内存峰值，建议 Web UI 上限提示 100 MiB；超过应当明确警告。
- 风险：脚本类规则如果依赖多次读 body，需要复制 Bytes；这部分与本设计无冲突，但脚本引擎需要保证 Bytes clone 是 O(1)。
- 风险：`body_rule_skipped` 字段属于 Traffic 记录扩展，向后兼容旧客户端读取。

## 实现现状（截至 2026-07-03）

- 请求侧 override 判定已落地：`crates/bifrost-proxy/src/proxy/http/handler.rs:879`、`crates/bifrost-proxy/src/proxy/http/handler.rs:1882`、`crates/bifrost-proxy/src/proxy/http/tunnel/mod.rs:2328`。
- 响应侧 override 判定：`crates/bifrost-proxy/src/proxy/http/handler.rs:3055`、`crates/bifrost-proxy/src/proxy/http/tunnel/mod.rs:3621`。
- 阈值配置：`crates/bifrost-admin/src/handlers/config.rs` 与 `crates/bifrost-admin/src/state.rs` 中 `max_body_buffer_size` / `max_body_probe_size`，Admin 可动态调整，默认 10 MiB / 64 KiB。
- 直接替换字段：`crates/bifrost-admin/src/request_rules.rs` 中的 `req_body` / `res_body`（`Option<Bytes>`）。
- E2E 覆盖：`e2e-tests/rules/advanced/body_size_strategy.txt` 与 `e2e-tests/tests/test_res_body_override_large.sh`；`e2e-tests/rules/COVERAGE.md` 表格显示所有 req/res body 相关规则均已被覆盖。
- 本设计文档无待落地项。
