# Path Wildcard Scheme Compatibility

## 背景

Bifrost 的路径通配 matcher 使用 `^` 前缀，例如：

```text
^example.com/assets/*/index.html
```

历史用户还写过带 scheme 前缀的路径通配规则：

```text
^https://cdn.example.com/obj/archi/obj/okrx-web/approvals-web/1.0.0.*/index.html
```

早期 `PathWildcardMatcher` 只识别裸 host，没有 scheme 感知，用户不得不手动删掉 `https://`。目标是让 matcher 无损兼容带 scheme 的历史规则，同时允许 `http*://` 与 `//` 表示 scheme 通配，避免破坏既有 `^example.com/...` 语义。

## 用户目标验证清单

### 必须实现

- `^https://host/path` 只匹配 HTTPS 请求，不匹配 HTTP。
- `^http://host/path` 只匹配 HTTP 请求，不匹配 HTTPS。
- `^http*://host/path` 与 `^//host/path` 同时匹配 HTTP 与 HTTPS。
- 没有 scheme 前缀的 `^host/path` 保留旧语义：同时匹配 HTTP/HTTPS。
- 目标 URL 若带有 path，例如 `http://127.0.0.1:8999/approvals`，转发时使用目标 path 原样，不再追加原请求 CDN 尾巴。
- Single `*` 仍只匹配一个 path 段（不跨 `/`、`?`）；`**` 匹配 path 内容（不跨 `?`）；`***` 匹配剩余全部。

### 必须不破坏

- 现有 `^example.com/...` 无 scheme 用例保持双向匹配。
- 单/双/三星通配语义、`@important`、否定规则、priority 计算不变。
- `is_path_wildcard_pattern` 判定逻辑保持一致，不误伤普通 host wildcard。

### 必须真实验证

- 单元测试覆盖 explicit http、explicit https、scheme wildcard、无 scheme 四种形态。
- 真实 E2E 用带 scheme 的路径通配规则转发到本地 upstream，验证 forward path 不被拼接。
- human_tests 用旧 approvals 规则做回归。

## 产品语义

### Scheme 前缀映射表

| 用户写法 | 转换后的 regex 协议段 |
| --- | --- |
| 无 scheme | `^https?://` |
| `http://` | `^http://` |
| `https://` | `^https://` |
| `http*://` | `^https?://` |
| `//` | `^https?://` |

去掉 scheme 后，剩余 host + path 部分继续按现有 `path wildcard → regex` 转换规则处理。

### Forward path 语义

带 scheme 的 `^...` 路径通配转发到含 path 的目标时，目标 path 视为绝对结果，不再拼接原请求 path。示例：

```text
^https://cdn.example.com/obj/archi/obj/okrx-web/approvals-web/1.0.0.*/index.html \
  http://127.0.0.1:8999/approvals
```

命中请求会被转发到：

```text
http://127.0.0.1:8999/approvals
```

而不是 `http://127.0.0.1:8999/approvals/obj/archi/...`。这一行为与既有非 scheme 版本 `^host/path` 的既定语义保持一致。

## 技术细节

实现全部落在 `crates/bifrost-core/src/matcher/path_wildcard.rs`：

- `PathWildcardMatcher::new(pattern)` 在剥离 `^` 与可选 negation 前缀后，通过 `strip_prefix` 逐一匹配 `https://`、`http://`、`http*://`、`//`。命中即固定 regex 协议段，未命中则默认 `^https?://`。
- 转换后调用既有 path wildcard → regex 编译逻辑，生成的 `Regex` 存放在结构体上，`is_match(url)` 与 host wildcard 保持一致的接口。
- `is_path_wildcard_pattern` 保留旧判定，只要看到 `^` 前缀且 body 含通配符或 path 片段就走 path wildcard，避免与 host wildcard 混淆。

规则解析和 resolver：

- `crates/bifrost-core/src/rule/resolver.rs` 中的 rule matcher factory 无需额外改动，`PathWildcardMatcher` 已经封装 scheme 判定。
- `crates/bifrost-proxy/src/utils/url.rs` 的 Final URL 构造在 forward target 含 path 时直接使用目标 path，不拼接原请求 path。

### CLI + Web + Admin API

- CLI 语法检查（`bifrost rule check <file>`）自动通过新的 matcher factory，识别带 scheme 的 `^...` 规则不再报语法错。
- Web UI Rule editor 现在允许保存 `^https://...` 型规则；Monaco 高亮无需专门改动。
- Admin API `POST /api/rules` / `PUT /api/rules/:name` 请求体只要 body 字符串合法即可保存，backend 通过 `RulesStorage::save` → matcher factory 编译。

### Sync 边界

- 路径通配规则通过普通规则同步；scheme 前缀在字符串里透明保留。
- Group 规则同上，不需要额外迁移。

## Phase 1-4

### Phase 1：Matcher scheme 解析

- 在 `PathWildcardMatcher::new` 中新增 `http://` / `https://` / `http*://` / `//` 前缀识别。
- 单元测试 `test_explicit_https_path_wildcard_support`、`test_explicit_http_path_wildcard_support`、`test_scheme_wildcard_path_wildcard_support` 覆盖 4 种前缀。

### Phase 2：Forward path 语义

- 确认 `bifrost-proxy` URL 组装在目标带 path 时不拼接原路径。
- 补充 E2E `matcher_scheme_qualified_path_wildcard`：本地 upstream 收到请求的 path 必须是目标 path 原样。

### Phase 3：Regression 与 human_tests

- 更新 `human_tests/proxy-http-https.md`，加入 `^https://` approvals 回归。
- 更新 `human_tests/readme.md` 用例数与覆盖范围。

### Phase 4：文档与 changelog

- 因为不引入新 CLI 语法，只需在 `docs/rules/routing.md` 与 `docs-en/operation.md` 的 path wildcard 段落说明 scheme 前缀矩阵。
- SKILL.md 的规则语法段落保持同步。

## 测试方案

### 单元测试

位置：`crates/bifrost-core/src/matcher/path_wildcard.rs` 的 `#[cfg(test)]`。

- `test_explicit_https_path_wildcard_support`（line 378）：approvals 旧规则匹配 HTTPS、拒绝 HTTP。
- `test_explicit_http_path_wildcard_support`（line 401）：`^http://...` 只匹配 HTTP。
- `test_scheme_wildcard_path_wildcard_support`（line 412）：`^http*://...` 与 `^//...` 同时匹配 HTTP/HTTPS。
- `test_https_support`（line 367）：无 scheme 前缀继续双向匹配。
- 通配基线：`test_single_star_path_wildcard`、`test_double_star_path_wildcard`、`test_triple_star_path_wildcard`、`test_single_star_no_slash_no_query`、`test_negated_path_wildcard`、`test_multiple_wildcards`、`test_priority_values`、`test_special_chars_escaped`、`test_capture_groups_count`、`test_complex_path_patterns`（同一文件内）。

### E2E 测试

位置：`crates/bifrost-e2e/src/tests/matchers.rs`。

- `matcher_scheme_qualified_path_wildcard`：启动隔离代理与本地 upstream，发送 `^http://...` 请求，断言 upstream 收到的 path 是 forward target 的固定 path。
- 覆盖 `test_pattern.sh` / `test_rules.sh` 中已有的 path wildcard 用例，确保回归干净。

### human_tests

- `human_tests/proxy-http-https.md`：新增 TC-PWSC-01 approvals `^https://...` 回归。
- `human_tests/readme.md`：更新 case 总数与覆盖描述。

### 校验要求

- `cargo test -p bifrost-core matcher::path_wildcard`
- `cargo test -p bifrost-core explicit_https_path_wildcard`
- `cargo test -p bifrost-e2e matchers::matcher_scheme_qualified_path_wildcard`
- `cargo test --workspace --all-features`
- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- 收尾运行 `rust-project-validate`；本机维持 no-local-coverage 约定时，只在 CI 侧要求 coverage。

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核 4 种 scheme 前缀是否覆盖所有历史写法。
- 复核 negation `!^https://...` 是否仍能编译。
- 跑单元测试与 `matcher_scheme_qualified_path_wildcard` E2E。

### 第 2 轮

- 复核 forward target path 覆盖 corner case：目标带 query string、fragment、末尾斜杠。
- 复核 `is_path_wildcard_pattern` 不会误把 `^http` 前缀识别为普通字符串。
- 复跑单元/E2E/human_tests。

## 风险与决策

- **决策**：不引入新语法，仅扩展已有 `^...` 前缀识别，兼容旧规则零迁移成本。
- **决策**：转发目标带 path 时不拼接原路径，保持与非 scheme 版一致；文档必须提示这与 host proxy 的默认拼接不同。
- **风险**：Regex 前缀改动可能影响后续 priority 计算，需要通过 `test_priority_values` 兜底。
- **风险**：某些用户可能想让 `^https://` 也匹配 HTTP，本方案严格按 scheme 精确匹配；如有回归诉求，应通过 `http*://` 明确表达。
