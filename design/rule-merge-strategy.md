# 规则合并策略分析与修复方案

> **状态（截至 2026-07-02）**：本文档针对 `reqHeaders://` / `resHeaders://` 同名覆盖问题给出的修复方案——在 `DomainMatcher::priority()` 中为 `PathPattern::Exact` / `PathPattern::Prefix` 增加基于路径段数的优先级加分——**已落地**到 `crates/bifrost-core/src/matcher/domain.rs`（第 271-283 行）。伴随合入的单元测试为 `test_priority_path_depth_exact`（第 516 行）、`test_priority_path_depth_prefix`（第 529 行）、`test_priority_specific_path_beats_root_with_protocol`（第 539 行）以及回归 `test_priority_full`（第 510 行）。§4 描述的修复已实施；§2.2 / §3 中标记为「后续优化」的其他协议（cookies / urlParams / trailers / params / resMerge）目前仍维持原行为，属 **planned, not yet shipped as of 2026-07-02**。

## 背景

Bifrost 的多规则合并模型：`RulesResolver` 对输入的规则列表按 `priority` 降序（`stable sort`）排列，然后 `convert_core_result_to_proxy`（`crates/bifrost-cli/src/parsing/rules.rs::471-791`）按顺序遍历 `core_result.rules`，把每条命中的规则里的协议合入 `ProxyResolvedRule`。不同协议的合并策略应当不同：

- 转发类：先命中优先（first-match-wins），只使用第一条 forwarding target。
- 修改类（KV 集合）：不同 key 累积，同名 key 由更具体规则覆盖更宽泛规则。
- 修改类（标量 Option）：last-wins（因为多数是 non-multi_match，实际只会命中一条）。
- 累积类：多条规则直接叠加。

真实用户反馈的问题：

```
`https://example.com/`         reqHeaders://{env1}   # x-tt-env: ppe_default
`https://example.com/api/v1/`  reqHeaders://{env2}   # x-tt-env: ppe_specific
```

访问 `https://example.com/api/v1/foo` 时，两条都匹配（`reqHeaders://` 是 multi_match），预期 `x-tt-env=ppe_specific`（更具体的路径覆盖宽泛），实际返回 `ppe_default`。根因是历史 `DomainMatcher::priority()` 对 `PathPattern::Exact` 固定 +15，不考虑路径段数，两条规则 priority 相同，stable sort 保持源文件顺序（宽泛在前），header dedupe 采用「已存在则跳过」，导致更具体规则的同名 header 被忽略。

## 用户目标验证清单

### 必须实现

- 相同宿主下，路径更深的规则获得更高 priority：
  - `example.com/` (15) < `example.com/api` (16) < `example.com/api/v1/` (17)
  - `example.com/api/*` (11) < `example.com/api/v1/*` (12)
- 协议命中额外 +5，端口 +10，与原语义一致。
- `reqHeaders://` / `resHeaders://` 同名 header 场景：更具体路径覆盖更宽泛路径。
- `first-match-wins` 转发协议 (`host://`, `xhost://`, `http://`, `https://`, `ws://`, `wss://`, `tunnel://`) 语义不变；由更具体规则优先命中，宽泛规则被 `!result.ignored.host` 守卫跳过。
- `Option<T>` 类型 non-multi_match 协议行为不变（只匹配到一条）。
- Body / prepend / append / html/js/css 注入 / CORS / TLS 控制协议保持 last-wins 语义。

### 必须不破坏

- 累积型协议（`reqReplace://`, `resReplace://`, `urlReplace://`, `headerReplace://`, `delete://`, `dns://`, `reqScript://`, `resScript://`, `decode://`）继续 extend / push 全部叠加。
- `forwardedFor://` / `responseFor://` 允许多值。
- `redirect://` 仍是 `Location` 头目标：即使值是 `http://…` / `https://…`，resolver 不得走 `ValueSource::RemoteUrl` 抓取远程内容作为 body。
- `@important` 提升 priority 的语义不变。
- Group rules 排序依旧走 `sort_order`；path depth 只加在 matcher priority 上。

### 必须真实验证

- 真实规则：宽泛 `https://example.com/` 与 具体 `https://example.com/api/v1/` 各挂一个同名 header，代理请求 `/api/v1/foo` 返回具体值。
- 真实规则：宽泛 `host://server_a` 与 具体路径 `host://server_b`，具体路径命中时转发到 `server_b`，宽泛 fallback 到 `server_a`。
- `test_priority_path_depth_exact` / `test_priority_path_depth_prefix` / `test_priority_specific_path_beats_root_with_protocol` 全部通过。

## 产品语义

### 协议分类总览

| 分类 | 说明 | 合并策略 |
|------|------|---------|
| 转发类 | 决定请求发往哪个上游 | first-match-wins |
| 修改类 - 标量 Option | 只有一个值的修改（method / ua） | last-wins |
| 修改类 - KV 集合 | 键值对集合（headers / cookies） | 同名 key 后覆盖前，不同 key 累积 |
| 修改类 - 纯累积 | 无冲突的追加项（replace / dns / scripts） | 全部累积 |
| Mock 类 | 直接返回本地内容 | first-match-wins |
| 控制类 | TLS / passthrough | last-wins |

### KV 集合的「后覆盖前」语义在当前 dedupe 上的实现

当前 `Protocol::ReqHeaders` / `Protocol::ResHeaders` 的 dedupe 仍是「已存在则跳过」——本次修复没改这段代码。但配合 §4 的 priority 修复，遍历顺序变为「更具体路径在前」，先写入的 header 就是更具体规则的值，宽泛规则同名 header 被跳过 —— 等价于「具体覆盖宽泛」。

## 全量协议合并策略（现状）

### 2.1 转发类（first-match-wins）— 正确

| 协议 | 字段类型 | 策略 | multi_match | 状态 |
|------|---------|------|-------------|------|
| `host://` | `Option<String>` | first-match-wins（`!result.ignored.host` 守卫） | 否 | OK |
| `xhost://` | `Option<String>` | 同上 | 否 | OK |
| `http://` / `https://` / `ws://` / `wss://` | `Option<String>` | 同上 | 否 | OK |
| `tunnel://` | `Option<String>` | 直接赋值 | 否 | OK |

### 2.2 修改类 KV 集合

| 协议 | 字段类型 | 当前策略 | multi_match | 预期 | 状态 |
|------|---------|---------|-------------|------|------|
| `reqHeaders://` | `Vec<(String,String)>` | 「先写者胜」dedupe | 是 | 同名 key 后覆盖前 | 已通过路径深度优先级修复 (2026-06-17) |
| `resHeaders://` | `Vec<(String,String)>` | 「先写者胜」dedupe | 是 | 同名 key 后覆盖前 | 同上 |
| `reqCookies://` | `Vec<(String,String)>` | 全部累积（无 dedupe） | 是 | 同名 cookie 后覆盖前 | planned, not yet shipped as of 2026-07-02 |
| `resCookies://` | `Vec<(String,ResCookieValue)>` | 全部累积 | 是 | 同名 cookie 后覆盖前 | planned |
| `urlParams://` + `delete_url_params` | `Vec<(String,String)>` | 全部累积 | 是 | 同名参数后覆盖前 | planned |
| `trailers://` | `Vec<(String,String)>` | 全部累积 | 是 | 同名 trailer 后覆盖前 | planned |

### 2.3 修改类 标量 Option（last-wins，non-multi_match）

- `statusCode://` / `replaceStatus://` / `method://` / `ua://` / `referer://` / `proxy://` / `auth://` / `reqDelay://` / `resDelay://` / `reqSpeed://` / `resSpeed://` / `reqType://` / `resType://` / `reqCharset://` / `resCharset://` / `cache://` / `attachment://` / `pac://` / `http3://`
- 全部 non-multi_match，只匹配第一条；`= Some(value)` 直接赋值，语义等同 last-wins。

### 2.4 Body / Prepend / Append（multi_match，但语义应为 last-wins）

- `reqBody://` / `resBody://` / `reqPrepend://` / `reqAppend://` / `resPrepend://` / `resAppend://`
- 直接 `= Some(bytes)`，后覆盖前；body 替换/追加应当只用最新一份。

### 2.5 Mock 类（non-multi_match）

- `file://` / `tpl://` / `rawfile://` / `redirect://`
- Option 直接赋值；non-multi_match 保证只命中一次。
- 2026-06-29 补充：`redirect://` 值是 `Location` 头目标，即便看起来像 URL，resolver 必须保留字符串，不能走 `ValueSource::RemoteUrl` 抓取远程页面作为 body。回归覆盖 `test_merge_redirect_non_multi_match`（`crates/bifrost-cli/src/parsing/rules.rs`）。

### 2.6 CORS

- `reqCors://` / `resCors://`：`CorsConfig` 整体赋值，last-wins（CORS 是整体替换语义）。

### 2.7 JSON Merge

- `params://` / `resMerge://`：当前 Option 直接赋值（last-wins）。理想应做 JSON deep merge —— **planned, not yet shipped as of 2026-07-02**。

### 2.8 纯累积

- `reqReplace://` / `resReplace://` / `urlReplace://` / `headerReplace://` / `delete://` / `dns://` / `reqScript://` / `resScript://` / `decode://`
- extend / push 全部叠加，无 dedupe，符合预期。

### 2.9 HTML / JS / CSS 注入

- `htmlAppend://` / `htmlPrepend://` / `htmlBody://` / `jsAppend://` / `jsPrepend://` / `jsBody://` / `cssAppend://` / `cssPrepend://` / `cssBody://`：last-wins。

### 2.10 特殊 / 控制

- `forwardedFor://` / `responseFor://`：push 到 headers，允许多值。
- `passthrough://`：`ignored.host = true`。
- `skip://`：无处理分支，上游控制。
- `tlsIntercept://` / `tlsPassthrough://` / `tlsOptions://` / `sniCallback://`：last-wins。

## 问题总结

### 已修复 (2026-06-17)

| 协议 | 问题 | 修复 |
|------|------|------|
| `reqHeaders://` | 宽泛路径写入的 header 先命中，具体路径无法覆盖 | `DomainMatcher::priority()` 加路径段数加分 |
| `resHeaders://` | 同上 | 同上 |

### 后续优化项 (planned, not yet shipped as of 2026-07-02)

- `reqCookies://` / `resCookies://` / `urlParams://` / `trailers://`：需要为同名 key 增加 dedupe。
- `params://` / `resMerge://`：JSON deep merge。

## 修复方案（已落地，2026-06-17）

### 4.1 根因分析

规则遍历管道：`parse → sort by Reverse(priority) (stable sort) → match → convert`

`DomainMatcher::priority()` 原实现对 `PathPattern::Exact` 固定 +15：

```
https://example.com/           → 100 + 5(protocol) + 15(exact_path)          = 120
https://example.com/api/v1/    → 100 + 5(protocol) + 15(exact_path)          = 120
```

两条同 priority，stable sort 保持源文件顺序，「先写者胜」dedupe 导致宽泛规则的 header 值占位。

### 4.2 修复策略（已实施）

`crates/bifrost-core/src/matcher/domain.rs::priority`（第 260-286 行，摘录：

```rust
if self.path_pattern.is_some() {
    priority += match &self.path_pattern {
        Some(PathPattern::Exact(path)) => {
            let segments = path.split('/').filter(|s| !s.is_empty()).count() as i32;
            15 + segments
        }
        Some(PathPattern::Prefix(prefix)) => {
            let segments = prefix.split('/').filter(|s| !s.is_empty()).count() as i32;
            10 + segments
        }
        None => 0,
    };
}
```

效果（path 部分加分）：

```
PathPattern::Exact("/")          → 15 + 0 = 15
PathPattern::Exact("/api")       → 15 + 1 = 16
PathPattern::Exact("/api/v1/")   → 15 + 2 = 17
PathPattern::Prefix("/api/*")    → 10 + 1 = 11
PathPattern::Prefix("/api/v1/*") → 10 + 2 = 12
```

Header dedupe 逻辑未改（`crates/bifrost-cli/src/parsing/rules.rs::488` `Protocol::ReqHeaders`、`::502` `Protocol::ResHeaders`）。「先写者胜」+ 更具体规则先遍历 = 具体覆盖宽泛。

### 4.3 影响分析

| 影响范围 | 说明 |
|---------|------|
| 转发类 | 无影响，non-multi_match，本来就取优先级最高一条 |
| 修改类 KV 集合 | ✅ 修复 reqHeaders / resHeaders 覆盖问题 |
| 修改类 标量 Option | 无影响，non-multi_match |
| 修改类 body/prepend/append | 略有影响：具体路径的 body 先命中，last-wins 语义下宽泛覆盖，但多路径 body 场景本身极少见；已通过 body 规范化测试保证行为稳定 |
| 累积类 | 无影响 |
| 已有测试 | `test_priority_full` 从旧的 130 更新到 132 |

### 4.4 测试落地状态

1. Unit / domain.rs：
   - `test_priority_path_depth_exact` ✅ 已落地（第 517 行）
   - `test_priority_path_depth_prefix` ✅ 已落地（第 530 行）
   - `test_priority_specific_path_beats_root_with_protocol` ✅ 已落地（第 540 行）
   - `test_priority_full` ✅ 值 132（第 511 行）
2. Unit / rules.rs：`bifrost.local /` vs `/api/v1/` 的具体覆盖宽泛用例 ✅ 已落地。
3. E2E：真实规则文件 header 覆盖场景 —— **planned, not yet shipped as of 2026-07-02**（目前覆盖以 unit + human_tests 为主）。
4. `human_tests/rule-merge-headers.md`、`human_tests/rule-merge-strategy.md` ✅ 已落地。

## CLI / Web / Admin API

- CLI：`bifrost rule show <name>` 会看到规则；`bifrost port active` 展示合并后的实际有效规则顺序；`bifrost traffic get <id>` 显示 matched rules 与协议合并结果。
- Web UI：Traffic detail 面板显示 matched rules 顺序，帮助诊断合并结果。
- Admin API：`GET /api/traffic/:id` 返回 `matched_rules`（按最终 priority 顺序）。
- 无新 CLI/API 表面；本次改动是内部 priority 算法。

## Sync 边界

- Priority 算法是运行时行为，不改变规则内容，也不影响 sync payload。
- Rule 内容 sync 完全不受影响。
- Group 排序仍走 `sort_order`；`DomainMatcher::priority` 只在 matcher 层作用。

## 实现切分

- Phase 1 (已完成)：`DomainMatcher::priority` 加路径段数；补 unit 测试；更新 `test_priority_full` 断言。
- Phase 2 (已完成)：`human_tests/rule-merge-headers.md` 场景加 `example.com /` vs `example.com /api/v1/` 覆盖用例；新增 `human_tests/rule-merge-strategy.md` 索引所有合并策略。
- Phase 3 (planned)：为 `reqCookies://` / `resCookies://` / `urlParams://` / `trailers://` 增 dedupe；协议 owner 一次性设计 JSON deep merge。
- Phase 4 (planned)：E2E 场景覆盖 header 覆盖 real proxy。

## 测试方案

### 单元测试（现状）

- `cargo test -p bifrost-core matcher::domain::tests::test_priority_path_depth_exact`
- `cargo test -p bifrost-core matcher::domain::tests::test_priority_path_depth_prefix`
- `cargo test -p bifrost-core matcher::domain::tests::test_priority_specific_path_beats_root_with_protocol`
- `cargo test -p bifrost-core matcher::domain::tests::test_priority_full`
- `cargo test -p bifrost-cli parsing::rules`（含 header 覆盖 case 与 `test_merge_redirect_non_multi_match`）

### E2E

- 现有 `e2e-tests/test_rules.sh` 保持通过。
- 新增 header 覆盖 real proxy 场景 —— planned。

### human_tests

- `human_tests/rule-merge-headers.md`
  - TC-RMH-01～06：header 合并主路径覆盖。
- `human_tests/rule-merge-strategy.md`：所有协议合并策略表格 + 手工验证清单。

### 项目校验

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test --workspace --all-features`
- 本机 no-local-coverage 生效。

## Review/Fix/Test 闭环方案

### 第 1 轮

- Review：`priority` 分段加分是否影响 `test_priority_with_exact_path` 等历史断言；stable sort 是否保留 `@important` 提升语义；redirect 是否未走 remote fetch。
- 复测：上述单元测试全部执行；`bifrost.local /` vs `/api/v1/` header 覆盖回归。

### 第 2 轮

- Review：human_tests 索引；文档中 planned 项与 shipped 项标注一致；协议表 (§2) 未遗漏新增协议。
- 复测：workspace 级 `cargo test`；关键真实场景手工验证。

## 风险与决策

- **priority 加分带来的排序漂移**：可能影响历史规则的相对顺序。回归靠 unit test 锁定；已知一处旧断言从 130 → 132。
- **未来 KV 集合 dedupe 落地时的兼容**：目前 cookies / urlParams / trailers 全部累积；后续引入 dedupe 需评估同名多值合法性（e.g. `Set-Cookie` 天然允许多条）。dedupe 应仅针对 client → server 方向单值 header/cookie/param 场景。
- **JSON deep merge**：`params://` / `resMerge://` 的 deep merge 是语义复杂变更，需独立设计并给出兼容开关。
- **`@important` 交互**：`priority()` 加分是内部数值调整，`@important` 提升机制不变；两者相加仍然协同。
