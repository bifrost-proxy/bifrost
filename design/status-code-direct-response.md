# statusCode 直接响应优化

## 背景

Bifrost 规则语言里 `statusCode://code` 的语义是「命中后直接返回指定 HTTP 状态码，不向后端服务器发送请求」。这个协议的目标是让用户可以在不启动 mock 服务的情况下，通过一条规则模拟错误（404/500/451），或者在故障演练场景快速返回特定状态码，避免真实请求到达 upstream 造成副作用。

历史实现存在一致性问题：

1. **HTTP 明文路径**：当规则里没有 `host` 时（例如 `test.local statusCode://404`），确实走 mock response 直接返回；但当规则里同时带 `host` 时（例如 `test.local host://127.0.0.1:8080 statusCode://451`），旧实现会先请求 upstream，再在响应上把状态码替换成 451，与 `docs/rules/status-redirect.md` 中的规则说明「statusCode 直接返回」不一致，并且 upstream 会收到无谓请求。
2. **HTTPS TLS 拦截路径**：同样的 `host + statusCode` 组合在 TLS 拦截路径下也会先转发再改，直接响应分支缺失。
3. **replaceStatus 混淆**：`replaceStatus://code` 的原语义是「后端仍请求，仅在响应返回后替换状态码」；由于 `statusCode` 行为不一致，用户经常把两者搞混。

本优化把 `statusCode` 收敛为强制「直接响应」语义：只要命中 `statusCode` 且没有更具体的 mock 响应载体（file / template / rawfile / locationHref），就在转发前构造响应并返回，不请求 upstream。`replaceStatus` 保持原语义不变。

## 用户目标验证清单

### 必须实现

- `statusCode` 与 `host` 同时存在时，HTTP 明文路径不再请求 upstream，直接返回指定状态码。
- HTTPS TLS 拦截路径同等生效：命中 `statusCode` 后在转发前直接构造响应。
- `statusCode` 可与 `resBody://`、`resHeaders://`、`resType://`、`resCharset://`、`cache://` 组合，由直接响应构造器写入响应。
- `file://` / `tpl://` / `rawfile://` / `locationHref://` 等 mock 内容规则仍优先使用自身响应体，`statusCode` 作为这些响应的状态码。
- `replaceStatus://code` 保持「请求 upstream 后替换状态码」的语义，回归测试通过。

### 必须不破坏

- 已有的 `test.local statusCode://404`（无 host）行为不变。
- `test.local host://127.0.0.1:port statusCode://200`（希望后端仍被访问的场景）用户可以改用 `replaceStatus`，文档明确指引。
- 规则解析、`@important`、matcher specificity、规则 pipeline 短路语义（`ignored.all`）保持不变。
- 现有 E2E 测试 `status_statusCode_404` / `status_statusCode_200` / `status_statusCode_500` / `status_statusCode_with_body` / `status_replaceStatus_200` / `status_combined_statusCode_headers` 保持通过。

### 必须真实验证

- 单元测试：`generate_mock_response` 在 `ResolvedRules { host: Some, status_code: Some }` 下返回直接响应且不发起 upstream 请求。
- E2E：`status_statusCode_direct_no_upstream` 场景下 mock upstream 请求计数 = 0，客户端收到期望状态码与 body。
- CLI 真实测试：临时端口绑定 `statusCode + host` 规则，curl 命中后返回 451，本地 mock server 收到 0 个请求。

## 产品语义

### `statusCode`：命中即直接响应

`statusCode://code` 是一个「响应生成器指令」，与 `host://` 的「上游选择器指令」正交。当规则同时携带二者，直接响应指令必须优先：因为用户显式声明了「我要这个状态码作为响应」，upstream 的选择在语义上已经不重要。

例外：若同一 `ResolvedRules` 中还带有更具体的响应体来源（`mock_file`、`mock_rawfile`、`mock_template`、`location_href`），直接响应生成器让位给内容生成器，`statusCode` 作为它们的响应状态码使用。

### `replaceStatus`：请求 upstream 后替换

`replaceStatus://code` 保留原意：请求仍完整发送到后端，只在响应返回后替换 status line。这适用于「后端返回 500 但业务想让浏览器认为是 200 便于降级」等场景。文档需要明确二者边界，避免用户误用 `statusCode` 期望后端仍被访问。

### 组合规则

- `statusCode + resBody`：直接响应体使用 resBody。
- `statusCode + resHeaders`：写入指定响应头。
- `statusCode + resType/resCharset`：写入 Content-Type。
- `statusCode + cache`：写入 Cache-Control（`0` → `no-cache, no-store, must-revalidate`；正整数 → `max-age=N`）。
- `statusCode + file/tpl/rawfile/locationHref`：内容生成器优先，`statusCode` 作为其响应状态码。

## 技术细节

### 关键源文件

- `crates/bifrost-proxy/src/utils/mock.rs`
  - 行 52 `generate_mock_response`：入口，检查 `rules.status_code` 并在没有更具体内容生成器时调用 `build_status_response`（行 62-73）。
  - 行 152 `build_status_response`：写入 status + 可选 Content-Type + Cache-Control + resHeaders + resBody。
  - 行 197 `build_redirect_response`、行 226/285 mock file/template/rawfile 分支均使用 `rules.status_code.unwrap_or(200)`。
- `crates/bifrost-proxy/src/proxy/http/tunnel/mod.rs`
  - 行 263、2015：TLS 拦截分支同样在转发前判定 `resolved_rules.status_code.is_some()`。
  - 行 2697、2995、3546：确保 status 覆盖只在直接响应路径外的场景走 `replace_status`。
- `crates/bifrost-e2e/src/tests/status_redirect.rs`：定义所有 status-* 系列 E2E 用例，其中 `status_statusCode_direct_no_upstream`（行 18-19）验证 upstream 请求计数 = 0（断言在行 136）。
- `crates/bifrost-proxy/src/proxy/http/handler.rs`：HTTP 明文路径入口调用 `generate_mock_response`。

### CLI + Web + Admin API

- CLI：
  - `bifrost rule update <name> --content "target statusCode://451"`
  - `bifrost port bind --port 18888 --rule <name>`
  - `bifrost port active 18888`（应显示对应规则）
- Web：
  - Rule Editor 的 `statusCode` 协议提示文案更新为「直接返回该状态码，不访问 upstream」。
  - Traffic 详情页命中该规则时 matched rules 面板显示 `statusCode`，upstream 字段为空。
- Admin API：
  - `PUT /api/rules/{name}` 更新规则；行为不变，仅语义收敛在 proxy 层实现。
  - `GET /api/traffic/{id}` 中 upstream 字段为 null 表示未访问 upstream。

### Sync 边界

- 本方案只涉及 proxy 层响应构造逻辑，不改变规则文件格式，不影响 rule sync / group sync。
- 已同步到远端的规则文本无需迁移；行为一致性由本地 proxy 层保证。

## 阶段拆分

### Phase 1：HTTP 明文路径收敛

- 调整 `generate_mock_response`：`status_code` 命中且无 file/template/rawfile/locationHref 时直接返回。
- 单元测试覆盖 `test_status_code_with_host_generates_direct_response`。

### Phase 2：TLS 拦截路径对齐

- `tunnel/mod.rs` 中在转发前检查 `resolved_rules.status_code.is_some()`，若是则复用同一 mock 构造分支。
- 保持 `replace_status` 分支在正常转发后执行。

### Phase 3：E2E 与 human_tests

- 新增 `status_statusCode_direct_no_upstream` E2E：mock upstream 请求计数 = 0，返回体为 `(blocked)`。
- 复跑 `status_replaceStatus_200`，确认后端仍被访问。
- 新增 `human_tests/status-code-direct-response.md`。

### Phase 4：文档与 Web 提示

- 更新 `docs/rules/status-redirect.md`（如存在）明确 statusCode 直接响应边界。
- Web Rule Editor 协议提示文案同步。
- `human_tests/readme.md` 索引补齐。

## 测试方案

### 单元测试

- `crates/bifrost-proxy/src/utils/mock.rs`：
  - `test_status_code_with_host_generates_direct_response`：构造 `ResolvedRules { host: Some, status_code: Some(451) }`，断言返回 `Some(Response)` 且状态码 = 451。
  - `test_status_code_yields_to_mock_file`：`status_code + mock_file` 命中，验证响应体来自 mock file 而不是 canonical reason。
  - `test_build_redirect_response_sets_status_and_headers`（已有，行 7937-7938）。

### E2E 测试

- `crates/bifrost-e2e/src/tests/status_redirect.rs`：
  - `status_statusCode_404`（行 12）
  - `status_statusCode_direct_no_upstream`（行 18，本方案关键）
  - `status_statusCode_500`（行 24）
  - `status_statusCode_200`（行 30）
  - `status_statusCode_with_body`（行 36）
  - `status_statusCode_preserves_rule_pipeline`（行 42）：确认 pipeline 中的 `@important` / matcher specificity 仍生效
  - `status_replaceStatus_200`（行 48）：确认 replaceStatus 仍访问 upstream
  - `status_combined_statusCode_headers`（行 78）：statusCode + resHeaders 组合

关键断言：
```rust
assert_eq!(mock.request_count(), 0, "statusCode should not contact upstream, got {} requests", mock.request_count());
```

（`crates/bifrost-e2e/src/tests/status_redirect.rs` 行 136）

### 真实场景测试 human_tests

- `human_tests/status-code-direct-response.md`（新增）：
  - TC-SCDR-01：`test.local statusCode://404`（无 host），curl 命中返回 404。
  - TC-SCDR-02：`test.local host://127.0.0.1:<mock> statusCode://451 resBody://(blocked)`，curl 命中返回 451 + body，mock server 收到 0 请求。
  - TC-SCDR-03：`test.local file:///path/to/mock.json statusCode://503`，curl 命中返回 503，body 来自 mock 文件。
  - TC-SCDR-04：`test.local host://127.0.0.1:<mock> replaceStatus://200`，curl 命中返回 200，mock server 收到 1 请求。
- 服务启动使用临时 `BIFROST_DATA_DIR`、非 9900 端口、`BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`、`BIFROST_DISABLE_TRAY=1`、`--no-system-proxy`。
- 更新 `human_tests/readme.md` 索引。

### 覆盖率与项目校验

- `cargo test -p bifrost-proxy test_status_code_with_host_generates_direct_response`
- `cargo test -p bifrost-proxy build_redirect_response`
- `cargo run -p bifrost-e2e -- --test status_statusCode_direct_no_upstream`
- `cargo run -p bifrost-e2e -- --test status_statusCode_preserves_rule_pipeline`
- `cargo run -p bifrost-e2e -- --test status_replaceStatus_200`
- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test --workspace --all-features`
- `rust-project-validate`
- 本机 no-local-coverage 约定：不跑 `make coverage`。

## Review/Fix/Test 闭环

### 第 1 轮

- 复核用户目标：`statusCode + host` 是否直接响应且不访问 upstream；`replaceStatus` 是否仍走 upstream；组合规则是否保持内容生成器优先。
- 复核 diff：`utils/mock.rs`、`tunnel/mod.rs`、`status_redirect.rs`、`human_tests/status-code-direct-response.md`、`docs/rules/status-redirect.md`、Web Rule Editor 协议提示。
- 重点 review：HTTPS TLS 拦截路径分支是否漏改；`replace_status` 与 `status_code` 是否有互相污染的代码路径。
- 复测：上述单元与 E2E。

### 第 2 轮

- 修复后复跑，人工在 Web Rule Editor 输入组合规则并验证 traffic 详情页 upstream 字段为空。
- 复核 `docs/rules/status-redirect.md` 说明与实现一致；Web 提示文案更新可见。
- 确认 `status_statusCode_preserves_rule_pipeline` 覆盖了短路语义 (`ignored.all` 优先直接返回 None 而不走 statusCode)。

## 校验要求

- `cargo test -p bifrost-proxy test_status_code_with_host_generates_direct_response`
- `cargo run -p bifrost-e2e -- --test status_statusCode_direct_no_upstream`
- `cargo run -p bifrost-e2e -- --test status_replaceStatus_200`
- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test --workspace --all-features`

## 文档更新要求

- 更新 `human_tests/readme.md` 索引，加入 `status-code-direct-response.md`。
- 更新 `docs/rules/status-redirect.md`：明确 `statusCode` 与 `replaceStatus` 边界（直接返回 vs 请求后替换）。
- 更新 Web 编辑器 `statusCode` 协议提示，明确「直接返回该状态码且不访问 upstream」。

## 风险与决策

- **老规则语义变化**：曾经依赖「statusCode + host 会请求 upstream 再改状态码」的用户会发现 upstream 不再被调用；本方案接受这一破坏性变化，因为原有行为与文档不一致，且用户可用 `replaceStatus` 显式表达旧语义。
- **组合规则优先级**：`statusCode + mock_file/template/rawfile/locationHref` 时内容生成器优先；如果未来要引入 `statusCode` 直接盖内容的模式，需要新增显式协议（例如 `statusCodeForce`），不要复用 `statusCode`。
- **HTTPS TLS 拦截依赖**：TLS 拦截路径与明文路径必须共享同一构造器，避免出现「明文直接返回，TLS 仍请求 upstream」的分裂现象。
- **规则 pipeline 短路**：`ResolvedRules.ignored.all` 命中时 `generate_mock_response` 直接返回 None，`statusCode` 不生效；这符合 pipeline 语义，`status_statusCode_preserves_rule_pipeline` 用例保护该行为。
- **CI 稳定性**：`status_statusCode_direct_no_upstream` 需要断言 mock upstream 请求计数 = 0；测试实现必须使用可控 mock server（`ProxyInstance` + 内嵌 counter），不要依赖网络行为。
