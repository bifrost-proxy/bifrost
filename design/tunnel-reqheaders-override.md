# Tunnel 请求头覆盖修复

## 背景

线上请求 `11117` 暴露出一个严重问题：当客户端原始请求已经携带某个 header，同时命中的 `reqHeaders://` 规则也配置了同名 header 时，HTTPS passthrough / tunnel 分支发往上游的请求会同时保留两个值，导致服务端收到重复 header。

已确认该问题出现在 `crates/bifrost-proxy/src/proxy/http/tunnel/mod.rs` 的上游请求构造链路中：

- 先通过 `Request::builder().header(...)` 复制客户端原始 header
- 再通过同一个 builder 的 `.header(...)` 追加规则 header
- `http::request::Builder::header` 对同名 key 的行为是追加而不是覆盖

普通 HTTP 改写链路走 `HeaderMap::insert`，不会出现这个问题，所以缺陷集中在 tunnel 分支。

## 目标

1. `reqHeaders://` 在 tunnel 分支中对同名 header 必须是覆盖语义
2. 客户端原始 header 与规则 header 同名时，上游只能收到一个最终值
3. 不改变 cookie、多值原始 header、其他转发逻辑的既有行为

## 实现方案

1. 保持 tunnel 分支原有的请求 builder 流程，仅移除“规则 header 通过 builder `.header(...)` 追加”的做法
2. 在 `outgoing_req` 构建完成后，单独应用 `resolved_rules.req_headers`
3. 规则 header 统一改为 `outgoing_req.headers_mut().insert(...)`
4. 如果规则 header 名称或值非法，记录错误并返回 `502 Bad Gateway`

这样改动最小，同时能精准修复“规则覆盖客户端 header”这一路径。

## 测试方案

### 单元测试

- 在 `crates/bifrost-proxy/src/proxy/http/tunnel/mod.rs` 新增测试
- 场景：请求初始带有 `x-same-key: client-original`，应用 `reqHeaders://x-same-key=rule-value` 后，只保留一个 `rule-value`

### E2E 测试

- 复用 `crates/bifrost-e2e/src/tests/rule_merge_strategy.rs::test_merge_reqheaders_same_key_override_e2e`
- 增强 mock 记录结构，保留 `header_pairs`，真实统计重复 header 个数
- 验证客户端原始值被覆盖，服务端只收到一个同名 header

### Human Tests

- 更新 `human_tests/rule-merge-headers.md`
- 新增 passthrough/HTTPS tunnel 回归用例，验证客户端原始 header 与规则同名时，上游仅收到一个最终值
- 同步更新 `human_tests/readme.md`

## 校验要求

1. 运行新增单元测试
2. 运行 `cargo test -p bifrost-e2e -- test_reqheaders_same_key_override --no-capture`
3. 按 `human_tests/rule-merge-headers.md` 逐条执行相关用例
4. 提交前执行：
   - `cargo fmt --all -- --check`
   - `cargo clippy --workspace --all-targets --all-features -- -D warnings`
   - `cargo test --workspace --all-features`

## 文档更新

- 新增 `design/tunnel-reqheaders-override.md`
- 更新 `human_tests/rule-merge-headers.md`
- 更新 `human_tests/readme.md`
