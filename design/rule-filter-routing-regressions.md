# 规则过滤与路由回归修复

## 背景

从真实用户反馈包（`.bifrost` network export）里发现两类可以复现的规则调试回归，本方案统一收敛：

1. **纯路径 filter 应保留前缀语义**：例如 `includeFilter:///account` 需要匹配 `/account`、`/account/`、`/account?...` 和 `/account-center`。历史实现在部分路径上退化成子串匹配，导致用户误认为规则不生效。
2. **Whistle 兼容的 URL wildcard filter**：例如 `excludeFilter://*/api`、`excludeFilter://*/alice/*`。Whistle 文档里“不带前缀”的 filter 就是完整请求 URL 的 wildcard；Bifrost 早期只支持带 `/` 或 `regex:` 前缀，Whistle 用户直接搬规则会失效。
3. **`upstreamUnsafeSsl://true` 单规则 HTTPS 证书豁免**：过去只能靠全局 `--unsafe-ssl` 关闭校验，粒度太粗。
4. **`NetworkRecord` 导出丢失路由诊断字段**：`actual_url` / `actual_host` / `listener_port` / `has_rule_hit` / `error_message` 在 traffic 详情里存在，但 `.bifrost` network export 与再次 import 后消失，让“规则命中但上游失败”的 case 看起来像 miss。

截至 2026-06-17 上面 4 项已经全部落地并验证。本文档同时收敛了 `lf6-cdn2-tos.bytegoofy.com … 10.37.102.138:8081`（bare host:port 走 http 未标 https 导致 502）与 `qianchuan.jinritemai.com … excludeFilter://…` 长链回归两个真实用户 case。

## 用户目标验证清单

### 必须实现

- 无前缀的路径 filter `/account` 使用「路径前缀 + 允许 `-`, `?`, `/` 等边界」的匹配语义：
  - 命中：`/account`, `/account/`, `/account/detail`, `/account-center`, `/account?tab=1`
  - 不命中：`/v1/account`, `/user-account`
- Whistle 风格 URL wildcard filter：
  - `*/api` 命中 `/api`, `/api?...`, `/api/...`，但不命中 `/apiary`。
  - `*/alice/*` 命中所有 URL 含 `/alice/` 的请求。
  - `?` 精确匹配一个 URL 字符。
- `upstreamUnsafeSsl://true` 可作用于单条规则，让该规则命中的 HTTPS 上游跳过证书校验，不需要全局 `--unsafe-ssl`。
- `NetworkRecord` 导出携带 `actual_url`, `actual_host`, `listener_port`, `has_rule_hit`, `error_message`，import 后 traffic 详情能完整还原。
- `actual_url`/`actual_host` 在普通 HTTP、CONNECT tunnel、大 body / streaming 分支都被正确填充。
- 规则文档在 `docs/rule.md`, `docs/operation.md`, `docs/rules/routing.md`, `docs/rules/README.md`, `docs-en/rule.md`, `docs-en/operation.md`, 顶层 `README.md` 同步描述前缀语义和 whistle 兼容。

### 必须不破坏

- 已有 `regex:` 前缀的 path filter 语义不变，仍按 regex 全匹配。
- 带 `includeFilter://regex:^/api/$` 的严格路径段匹配不受影响。
- 已有 `*` / `**` 域名匹配、`m:GET` / `m:POST` method filter 不受影响。
- 全局 `--unsafe-ssl` 行为不变。
- 未升级到新的 network export schema 的旧包仍可读，只是缺失新字段。

### 必须真实验证

- 真实代理请求 `/account-center` 被 `includeFilter:///account` 包含并命中后续规则。
- 长 `excludeFilter` 链（qianchuan 场景）解析成一条规则的多个 exclusion，被排除路径落到后续 host 规则。
- Whistle 兼容规则 `excludeFilter://*/api`, `excludeFilter://*/alice/*` 生效。
- `upstreamUnsafeSsl://true` 对 self-signed HTTPS upstream 成功；同一目标没有该规则时失败。
- traffic 详情与 `.bifrost` network 导出都能看到 `actual_url`, `actual_host`, `listener_port`, `has_rule_hit`。

## 产品语义

### 路径 filter 语义

- 「裸路径 filter」采用「路径前缀」语义，但边界允许 `-`, `?`, `/`, `#` 等非字母数字字符继续，保证 `/account-center` 也命中 `/account`。
- 期望更严格的段边界匹配请显式使用 `regex:` 前缀，例如 `includeFilter://regex:^/account(/|$)`。
- Whistle 兼容 wildcard 走独立分支：`*` 匹配任意 URL 字符（包含 `/`），`?` 匹配一个字符，两者不参与新的“路径前缀”分支。

### `upstreamUnsafeSsl://true`

- 每条规则可独立声明；命中此规则的上游 HTTPS 请求跳过证书校验。
- 不影响未命中该规则的其他上游。
- 如果同时存在 `--unsafe-ssl` 全局开关，全局仍以更宽的语义生效。

### 网络导出诊断字段

- `TrafficRecord` 内已有 `actual_url` / `actual_host` / `listener_port` / `has_rule_hit` / `error_message`；`NetworkRecord` 只是把它们 optional 序列化，import 时优先使用导出字段，否则回退到旧 fallback。

## 技术细节

### 涉及模块与关键落点

- `crates/bifrost-core/src/rule/filter.rs`
  - 路径前缀匹配：`test_parse_plain_path_filter_matches_prefix`
  - Whistle 风格 wildcard：`test_parse_whistle_style_wildcard_path_prefix_filter`, `test_parse_whistle_style_wildcard_url_filter`, `test_parse_whistle_style_wildcard_filter_with_literal_query`
  - 关键函数：`contains_wildcard`, `whistle_wildcard_filter_to_regex`
- `crates/bifrost-core/src/rule/resolver/tests.rs`
  - `test_exclude_filter_whistle_style_wildcard_path_prefix` — 端到端解析器验证
- `crates/bifrost-core/src/protocol.rs`, `crates/bifrost-core/src/syntax.rs`
  - `Protocol::UpstreamUnsafeSsl` 枚举、字符串映射、注册表（`crates/bifrost-core/src/protocol.rs` 91/360/442/456/641/731 行附近）
- `crates/bifrost-cli/src/parsing/rules.rs`
  - `upstreamUnsafeSsl` 被解析并挂载到 `ProxyResolvedRule.upstream_unsafe_ssl`
- `crates/bifrost-proxy/src/proxy/http/handler.rs`
  - 命中规则时读取 `upstream_unsafe_ssl` 覆盖 TLS 校验；提示 `error_message` 里带 hint（“target requires HTTPS” 之类）
- `crates/bifrost-proxy/src/proxy/http/tunnel/mod.rs`, `crates/bifrost-proxy/src/proxy/socks/tcp.rs`
  - `actual_url` / `actual_host` 填充：CONNECT tunnel 与大 body / SOCKS 分支
- `crates/bifrost-core/src/bifrost_file/types.rs`
  - `NetworkRecord` 增加 optional 诊断字段
- `crates/bifrost-admin/src/handlers/bifrost_file.rs`
  - 导入 / 导出映射保留诊断字段

### CLI / Rule DSL

裸路径 filter：

```
example.com/api includeFilter:///account
```

Whistle 兼容 wildcard：

```
example.com 10.37.102.138:8080 excludeFilter://*/api excludeFilter://*/alice/*
```

单规则 HTTPS 跳过校验：

```
internal.corp.test https://10.37.102.138:8443 upstreamUnsafeSsl://true
```

裸 `host:port` 写法（无 `http://` / `https://`）会被视作 `host://…`，也即 HTTP 上游。当真实上游是 HTTPS 时必须显式写 `https://10.37.102.138:8081`，或加 `upstreamUnsafeSsl://true` 处理 self-signed 证书。文档中把这个陷阱写到 `docs/rule.md` 与 `docs/rules/routing.md`。

### Web UI

- Rule editor 高亮 `upstreamUnsafeSsl://true` 为 control 协议。
- Traffic 详情面板显示 `actual_url` / `actual_host` / `listener_port` / `has_rule_hit`，并在 rule miss 但 upstream 失败时给出可读提示。
- `.bifrost` network export/import 按钮走 `crates/bifrost-admin/src/handlers/bifrost_file.rs`，UI 层无需额外改动。

### Admin API

- `POST /api/rules/validate`：校验 `upstreamUnsafeSsl://true` 和 whistle wildcard，返回 E0xx 错误码。
- `POST /api/network/export`, `POST /api/network/import`：写入/读取新字段，字段缺失时对导入方保持兼容。
- Traffic detail JSON (`GET /api/traffic/:id`) 已有诊断字段，本次只补 export 通道。

### Sync 边界

- `upstreamUnsafeSsl://true` 是本地 TLS 决策，随规则内容一起 sync；远端拉到本地后仍需本地 CA 与 socket 支持才能生效。
- Network export 是本地 traffic 快照，不参与 rule sync。

## 实现切分（回溯记录）

### Phase 1：filter 语义（已完成）

- `filter.rs` 中改造 non-regex path filter 的匹配函数，允许边界后续字符（`-`, `?`, `/`, `#`）。
- 新增 `contains_wildcard` / `whistle_wildcard_filter_to_regex`，覆盖 `*` 与 `?`。
- 补 `resolver/tests.rs` 端到端断言。

### Phase 2：`upstreamUnsafeSsl://true`（已完成）

- `Protocol::UpstreamUnsafeSsl` + 注册表。
- CLI parsing 挂到 `ProxyResolvedRule.upstream_unsafe_ssl`。
- Proxy handler 在建立上游 TLS 连接前读取标志。
- Error path 添加更清晰的 hint。

### Phase 3：Network export 诊断字段（已完成）

- 扩展 `NetworkRecord`，字段 optional 保证兼容。
- 完善 tunnel / large-body / SOCKS 分支的 `actual_url` / `actual_host` 填充。
- Admin import 与 traffic 详情 UI 使用新字段。

### Phase 4：文档 + E2E（已完成）

- 文档同步：`docs/rule.md`, `docs/operation.md`, `docs/rules/routing.md`, `docs/rules/README.md`, `docs-en/rule.md`, `docs-en/operation.md`, `README.md`。
- E2E：`e2e-tests/tests/test_rule_filter_routing_diagnostics.sh` 覆盖 `.bifrost` 导出、`upstreamUnsafeSsl`、长 excludeFilter 链、`/account-center` 前缀。
- Fixture：`e2e-tests/rules/regression/rule_filter_routing_diagnostics.txt` 排除在 `FIXTURE_ONLY_RULES`（`e2e-tests/run_all_tests_parallel.sh` 第 213 行）之外，避免通用 runner 因 filter 断言差异而误判。

## 测试方案

### 单元测试

- `crates/bifrost-core`：
  - `filter::test_parse_plain_path_filter_matches_prefix`
  - `filter::test_parse_whistle_style_wildcard_url_filter`
  - `filter::test_parse_whistle_style_wildcard_path_prefix_filter`
  - `filter::test_parse_whistle_style_wildcard_filter_with_literal_query`
  - `rule::resolver::tests::test_exclude_filter_whistle_style_wildcard_path_prefix`
- `crates/bifrost-cli`：`parsing/rules.rs` 中 `upstreamUnsafeSsl` 解析回归。
- `crates/bifrost-admin`：`handlers/bifrost_file.rs` 中 network export/import 对 diagnostic 字段的 round-trip 测试。
- `crates/bifrost-core/src/bifrost_file/types.rs` 中 `NetworkRecord` 序列化默认值测试。

### E2E

- `e2e-tests/tests/test_rule_filter_routing_diagnostics.sh`：
  - `/account-center` includeFilter 命中
  - 长 excludeFilter 链的排除路径正确落到 fallback host 规则
  - `upstreamUnsafeSsl://true` self-signed HTTPS upstream
  - traffic 详情 + `.bifrost` network 导出包含 `actual_url`, `actual_host`, `listener_port`, `has_rule_hit`
- Fixture：`e2e-tests/rules/regression/rule_filter_routing_diagnostics.txt`；被 `FIXTURE_ONLY_RULES` 排除在通用 runner 之外（`e2e-tests/run_all_tests_parallel.sh` 第 213 行）。
- 通用 runner 保留 `e2e-tests/test_rules.sh` 的 `excludeFilter 语义验证` 段（第 1800/1977/3025 行附近）。

### 真实场景 human_tests

- `human_tests/rule-filter-routing-diagnostics.md`
  - TC-RFR-01：`/account-center` 场景，验证前缀语义。
  - TC-RFR-02：qianchuan 长 excludeFilter 链。
  - TC-RFR-03：Whistle wildcard `*/api`, `*/alice/*`。
  - TC-RFR-04：`upstreamUnsafeSsl://true` 单规则豁免。
  - TC-RFR-05：`.bifrost` network 导出 → 导入 → 诊断字段完整。
- 索引：`human_tests/readme.md` 已收录。

### 项目校验

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- 关键 crate：`cargo test -p bifrost-core rule::filter`, `cargo test -p bifrost-core rule::resolver`, `cargo test -p bifrost-cli parsing::rules`, `cargo test -p bifrost-admin bifrost_file`
- E2E：`e2e-tests/tests/test_rule_filter_routing_diagnostics.sh` 直接执行
- 本机 no-local-coverage 生效，不跑 `make coverage`

## Review/Fix/Test 闭环方案

### 第 1 轮

- Review：filter 分支是否覆盖 regex / wildcard / plain 三条路径；`NetworkRecord` 新字段是否 optional；`upstream_unsafe_ssl` 是否在所有上游建连位点生效。
- 复测：上面 5 个单元测试 + `test_rule_filter_routing_diagnostics.sh`。

### 第 2 轮

- Review：文档 (`docs/rule.md` / `docs-en/rule.md`) 与实现是否一致；`FIXTURE_ONLY_RULES` 是否遗漏；`error_message` hint 是否稳定可测。
- 复测：`human_tests/rule-filter-routing-diagnostics.md` 全流程；workspace `cargo test`。

## 风险与决策

- **路径 filter 语义变更**：从子串 → 前缀边界，会让一些历史规则从「意外命中」变为「不再命中」。评估后仍保持前缀语义，因为它符合 Whistle 语义、更贴近用户预期；提供 `regex:` 兜底。
- **Whistle 兼容语义**：`*` 覆盖 `/` 与 Whistle 一致，但和常见 glob 不同。文档中显式说明；提供 `regex:` 前缀作为逃生舱。
- **`upstreamUnsafeSsl://true` 安全性**：给出显式 warning，只对命中规则生效，避免误关全局校验。
- **`NetworkRecord` schema 兼容**：新字段 optional；导出方标记版本号；导入方缺字段时不报错。
- **`bare host:port` 陷阱**：不改语义，文档明确说明，避免破坏历史规则。
