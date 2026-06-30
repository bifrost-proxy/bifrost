# 规则合并策略分析与修复方案

> **状态（截至 2026-06-17）**：本文档针对 `reqHeaders://` / `resHeaders://` 同名覆盖问题给出的修复方案——在 `DomainMatcher::priority()` 中为 `PathPattern::Exact` / `PathPattern::Prefix` 增加基于路径段数的优先级加分——**已落地**到 `crates/bifrost-core/src/matcher/domain.rs`，并伴随 `test_priority_path_depth_exact` / `test_priority_path_depth_prefix` 等单元测试一起合入。下文 §1 描述的「实际」行为为修复前的历史现象，保留以记录问题背景；§4 描述的方案已实施。其余在 §2.2 / §3 中标记为「⚠️ 后续优化」的协议（cookies / urlParams / trailers / params / resMerge）目前仍维持原行为，属 **planned, not yet shipped as of 2026-06-17**。

## 1. 问题背景

当多条规则匹配同一个请求时，不同类型的协议需要不同的合并策略：

- **转发类协议**（如 `host://`、`http://`）：**先匹配优先**（first-match-wins），只使用第一条匹配的转发目标
- **修改类协议**（如 `reqHeaders://`、`resHeaders://`）：**后匹配覆盖**（last-wins for same keys），对于同名字段，后面（更具体）的规则覆盖前面（更宽泛）的规则；不同名字段则累积合并

### 触发场景

```
`https://example.com/` reqHeaders://{env1}
`https://example.com/api/v1/` reqHeaders://{env2}

```env1
x-tt-env: ppe_default
x-use-ppe: 1
```

```env2
x-tt-env: ppe_specific
x-use-ppe: 1
```
```

访问 `https://example.com/api/v1/foo` 时，两条规则都匹配（`reqHeaders://` 是 `multi_match`）。

- **预期**：`x-tt-env` 应为 `ppe_specific`（更具体的路径规则覆盖宽泛规则）
- **实际**：`x-tt-env` 为 `ppe_default`（先匹配到的规则先写入，后续同名 header 被跳过）

### 根因定位

`convert_core_result_to_proxy()` 函数（[rules.rs:471-791](../crates/bifrost-cli/src/parsing/rules.rs)，截至 2026-06-17）中，`ReqHeaders` 和 `ResHeaders` 仍使用「先到先得」的去重逻辑——这一逻辑本身未变，但配合下文 §4 修复后的路径深度优先级，更具体的规则会先被遍历并先写入 header，宽泛规则同名 header 被跳过的语义即等价于「具体覆盖宽泛」：

```rust
Protocol::ReqHeaders => {
    if let Some(headers) = parse_header_value(value) {
        for (k, v) in headers {
            let key_lower = k.to_lowercase();
            if !result.req_headers.iter().any(|(existing, _)| existing.to_lowercase() == key_lower) {
                result.req_headers.push((k, v));
            }
        }
    }
}
```

规则按 priority 降序排列（stable sort），高优先级（更具体的路径）排在前面。但当两条规则优先级相同时（如 `DomainMatcher` + exact path 都为 120），stable sort 保持原始顺序——即文本中先声明的规则排在前面。这导致宽泛规则的 header 值先被写入，更具体规则的同名 header 被跳过。

## 2. 全量协议分类与合并策略分析

### 2.1 协议分类总览

| 分类 | 说明 | 合并策略 |
|------|------|---------|
| 🔀 转发类 | 决定请求发往哪个上游 | first-match-wins |
| ✏️ 修改类 - 标量值 | 只有一个值的修改（如 method、ua） | last-wins（后覆盖前） |
| ✏️ 修改类 - KV 集合 | 键值对集合（如 headers、cookies） | 同名 key 后覆盖前，不同 key 累积 |
| ✏️ 修改类 - 纯累积 | 无冲突的追加项（如 replace 规则） | 全部累积 |
| 🎭 Mock 类 | 直接返回本地内容 | first-match-wins |
| 🔒 控制类 | TLS/通道控制 | last-wins |

### 2.2 每个协议的当前合并策略与预期行为

#### 转发类（first-match-wins）— 当前逻辑正确 ✅

| 协议 | 字段类型 | 当前策略 | 是否 multi_match | 状态 |
|------|---------|---------|-----------------|------|
| `host://` | `Option<String>` | ✅ first-match-wins（通过 `!result.ignored.host` 守卫） | ❌ | ✅ 正确 |
| `xhost://` | `Option<String>` | ✅ 同上 | ❌ | ✅ 正确 |
| `http://` | `Option<String>` | ✅ 同上 | ❌ | ✅ 正确 |
| `https://` | `Option<String>` | ✅ 同上 | ❌ | ✅ 正确 |
| `ws://` | `Option<String>` | ✅ 同上 | ❌ | ✅ 正确 |
| `wss://` | `Option<String>` | ✅ 同上 | ❌ | ✅ 正确 |
| `tunnel://` | `Option<String>` | ✅ 直接赋值（non-multi_match，只匹配一次） | ❌ | ✅ 正确 |

#### Mock 类（first-match-wins）— 当前逻辑正确 ✅

| 协议 | 字段类型 | 当前策略 | 是否 multi_match | 状态 |
|------|---------|---------|-----------------|------|
| `file://` | `Option<String>` | last-wins（Option 直接覆盖） | ❌ | ✅ 正确（non-multi_match 保证只一条） |
| `tpl://` | `Option<String>` | 同上 | ❌ | ✅ 正确 |
| `rawfile://` | `Option<String>` | 同上 | ❌ | ✅ 正确 |
| `redirect://` | `Option<String>` | 同上 | ❌ | ✅ 正确 |

> 2026-06-29 补充：`redirect://` 的值是 HTTP `Location` 头目标，不是 mock/response body 内容源。即使目标是 `http://...` 或 `https://...`，resolver 也必须保留 URL 字符串，不得像 `file://` / `tpl://` 的内容值一样通过 `ValueSource::RemoteUrl` 去下载远程页面。回归覆盖 `test_merge_redirect_non_multi_match`，防止当前网络环境把 `redirect://http://target-a.com` 解析成真实站点 HTML。

#### 修改类 - KV 集合 — ⚠️ 需要修复

| 协议 | 字段类型 | 当前策略 | 是否 multi_match | 预期策略 | 状态 |
|------|---------|---------|-----------------|---------|------|
| `reqHeaders://` | `Vec<(String, String)>` | 先到先得（同名 key 跳过） | ✅ | 同名 key 后覆盖前，不同 key 累积 | ✅ 已修复（通过路径深度优先级，2026-06-17 已合入） |
| `resHeaders://` | `Vec<(String, String)>` | 先到先得（同名 key 跳过） | ✅ | 同名 key 后覆盖前，不同 key 累积 | ✅ 已修复（同上） |
| `reqCookies://` | `Vec<(String, String)>` | 全部累积（无去重） | ✅ | 同名 cookie 后覆盖前，不同 cookie 累积 | ⚠️ **需优化** (planned, not yet shipped as of 2026-06-17) |
| `resCookies://` | `Vec<(String, ResCookieValue)>` | 全部累积（无去重） | ✅ | 同名 cookie 后覆盖前，不同 cookie 累积 | ⚠️ **需优化** (planned, not yet shipped as of 2026-06-17) |
| `urlParams://` | `Vec<(String, String)>` + `delete_url_params` | 全部累积 | ✅ | 同名参数后覆盖前，不同参数累积 | ⚠️ **需优化** (planned, not yet shipped as of 2026-06-17) |
| `trailers://` | `Vec<(String, String)>` | 全部累积（无去重） | ✅ | 同名 trailer 后覆盖前，不同 trailer 累积 | ⚠️ **需优化** (planned, not yet shipped as of 2026-06-17) |

> **注意**：`reqHeaders://` / `resHeaders://` 的同名覆盖问题已通过路径深度优先级修复（更具体的路径排在前面，被先写入；后续宽泛规则的同名 header 在「先到先得」去重逻辑下被跳过，等价于「具体覆盖宽泛」），dedupe 代码本身未动。`reqCookies://`、`resCookies://`、`urlParams://`、`trailers://` 当前仍是全部累积、对同名 key 不做覆盖；虽然在大多数场景下可用（用户很少对同一 cookie/param 设置不同值），但为了一致性应实现「同名后覆盖」——此项 **planned, not yet shipped as of 2026-06-17**。

#### 修改类 - 标量值（Option 类型）— 当前逻辑需确认

对于 `Option<T>` 类型的字段，由于规则按 priority 降序遍历，且是 non-multi_match（只匹配第一条），所以事实上只有一条规则生效。当前直接赋值（`= Some(value)`）的语义等同于 last-wins，但因为 non-multi_match 保证了只有一条规则到达这里，所以没有实际冲突：

| 协议 | 字段类型 | 当前策略 | 是否 multi_match | 状态 |
|------|---------|---------|-----------------|------|
| `statusCode://` | `Option<u16>` | last-wins（直接赋值） | ❌ | ✅ 正确 |
| `replaceStatus://` | `Option<u16>` | last-wins | ❌ | ✅ 正确 |
| `method://` | `Option<String>` | last-wins | ❌ | ✅ 正确 |
| `ua://` | `Option<String>` | last-wins | ❌ | ✅ 正确 |
| `referer://` | `Option<String>` | last-wins | ❌ | ✅ 正确 |
| `proxy://` | `Option<String>` | last-wins | ❌ | ✅ 正确 |
| `auth://` | `Option<String>` | last-wins | ❌ | ✅ 正确 |
| `reqDelay://` | `Option<u64>` | last-wins | ❌ | ✅ 正确 |
| `resDelay://` | `Option<u64>` | last-wins | ❌ | ✅ 正确 |
| `reqSpeed://` | `Option<u64>` | last-wins | ❌ | ✅ 正确 |
| `resSpeed://` | `Option<u64>` | last-wins | ❌ | ✅ 正确 |
| `reqType://` | `Option<String>` | last-wins | ❌ | ✅ 正确 |
| `resType://` | `Option<String>` | last-wins | ❌ | ✅ 正确 |
| `reqCharset://` | `Option<String>` | last-wins | ❌ | ✅ 正确 |
| `resCharset://` | `Option<String>` | last-wins | ❌ | ✅ 正确 |
| `cache://` | `Option<String>` | last-wins | ❌ | ✅ 正确 |
| `attachment://` | `Option<String>` | last-wins | ❌ | ✅ 正确 |
| `pac://` | 设置 host 字段 | last-wins | ❌ | ✅ 正确 |
| `http3://` | `bool` 标志 | last-wins | ❌ | ✅ 正确 |

#### 修改类 - Body/Prepend/Append（内容叠加型）

这些协议是 multi_match，但字段类型是 `Option<Bytes>`，当前行为是后面的值直接覆盖前面的值：

| 协议 | 字段类型 | 当前策略 | 是否 multi_match | 状态 |
|------|---------|---------|-----------------|------|
| `reqBody://` | `Option<Bytes>` | last-wins | ✅ | ✅ 正确（body 替换语义应该是后覆盖前） |
| `resBody://` | `Option<Bytes>` | last-wins | ✅ | ✅ 正确 |
| `reqPrepend://` | `Option<Bytes>` | last-wins | ✅ | ✅ 正确 |
| `reqAppend://` | `Option<Bytes>` | last-wins | ✅ | ✅ 正确 |
| `resPrepend://` | `Option<Bytes>` | last-wins | ✅ | ✅ 正确 |
| `resAppend://` | `Option<Bytes>` | last-wins | ✅ | ✅ 正确 |

#### 修改类 - CORS 配置

| 协议 | 字段类型 | 当前策略 | 是否 multi_match | 状态 |
|------|---------|---------|-----------------|------|
| `reqCors://` | `CorsConfig` | last-wins（直接赋值整个结构） | ✅ | ✅ 正确（CORS 是整体替换语义） |
| `resCors://` | `CorsConfig` | last-wins | ✅ | ✅ 正确 |

#### 修改类 - JSON Merge

| 协议 | 字段类型 | 当前策略 | 是否 multi_match | 状态 |
|------|---------|---------|-----------------|------|
| `params://` | `Option<serde_json::Value>` | last-wins（后面的 JSON 覆盖前面） | ✅ | ⚠️ 应该做 JSON deep merge (planned, not yet shipped as of 2026-06-17) |
| `resMerge://` | `Option<serde_json::Value>` | last-wins | ✅ | ⚠️ 应该做 JSON deep merge (planned, not yet shipped as of 2026-06-17) |

> `params://` 和 `resMerge://` 的 multi_match 特性暗示它们可以从多条规则合并值，但当前实现是直接 `Option` 赋值（后覆盖前）。理想情况应该做 JSON deep merge（多个 JSON 对象合并），但这个行为变更较大，标记为后续优化项——**planned, not yet shipped as of 2026-06-17**。

#### 修改类 - 纯累积型 — 当前逻辑正确 ✅

这些协议天然就是累积式的，多条规则的值独立叠加，没有"覆盖"语义：

| 协议 | 字段类型 | 当前策略 | 是否 multi_match | 状态 |
|------|---------|---------|-----------------|------|
| `reqReplace://` | `Vec<(String, String)>` + regex | 累积 extend | ✅ | ✅ 正确 |
| `resReplace://` | `Vec<(String, String)>` + regex | 累积 extend | ✅ | ✅ 正确 |
| `urlReplace://` | `Vec<(String, String)>` + regex | 累积 extend | ✅ | ✅ 正确 |
| `headerReplace://` | `Vec<HeaderReplaceRule>` | 累积 extend | ✅ | ✅ 正确 |
| `delete://` | 多个 Vec 累积 | 累积 extend | ✅ | ✅ 正确 |
| `dns://` | `Vec<String>` | 累积 push | ❌ | ✅ 正确 |
| `reqScript://` | `Vec<String>` | 累积 push | ✅ | ✅ 正确 |
| `resScript://` | `Vec<String>` | 累积 push | ✅ | ✅ 正确 |
| `decode://` | `Vec<String>` | 累积 push | ✅ | ✅ 正确 |

#### 修改类 - HTML/JS/CSS 注入

| 协议 | 字段类型 | 当前策略 | 是否 multi_match | 状态 |
|------|---------|---------|-----------------|------|
| `htmlAppend://` | `Option<String>` | last-wins | ✅ | ✅ 正确 |
| `htmlPrepend://` | `Option<String>` | last-wins | ✅ | ✅ 正确 |
| `htmlBody://` | `Option<String>` | last-wins | ✅ | ✅ 正确 |
| `jsAppend://` | `Option<String>` | last-wins | ✅ | ✅ 正确 |
| `jsPrepend://` | `Option<String>` | last-wins | ✅ | ✅ 正确 |
| `jsBody://` | `Option<String>` | last-wins | ✅ | ✅ 正确 |
| `cssAppend://` | `Option<String>` | last-wins | ✅ | ✅ 正确 |
| `cssPrepend://` | `Option<String>` | last-wins | ✅ | ✅ 正确 |
| `cssBody://` | `Option<String>` | last-wins | ✅ | ✅ 正确 |

#### 特殊协议

| 协议 | 字段类型 | 当前策略 | 是否 multi_match | 状态 |
|------|---------|---------|-----------------|------|
| `forwardedFor://` | push 到 `req_headers` | 直接 push（无去重） | ❌ | ✅ 正确（X-Forwarded-For 可以有多个值） |
| `responseFor://` | push 到 `res_headers` | 直接 push（无去重） | ❌ | ✅ 正确 |
| `passthrough://` | 设置 `ignored.host` | 直接设为 true | ❌ | ✅ 正确 |
| `skip://` | — | multi_match 但无处理分支 | ✅ | ✅ 正确（skip 的控制在上游） |

#### 控制类

| 协议 | 字段类型 | 当前策略 | 是否 multi_match | 状态 |
|------|---------|---------|-----------------|------|
| `tlsIntercept://` | `Option<bool>` | last-wins | ❌ | ✅ 正确 |
| `tlsPassthrough://` | `Option<bool>` | last-wins | ❌ | ✅ 正确 |
| `tlsOptions://` | `Option<String>` | last-wins | ❌ | ✅ 正确 |
| `sniCallback://` | `Option<String>` | last-wins | ❌ | ✅ 正确 |

## 3. 问题总结

### 🐛 历史 BUG（已修复，2026-06-17 已合入）

| 协议 | 问题 | 影响 | 状态 |
|------|------|------|------|
| `reqHeaders://` | 「先到先得」去重逻辑下宽泛规则的 header 先写入，更具体路径无法覆盖 | 用户无法通过更具体的路径规则覆盖宽泛规则设置的 header 值 | ✅ 已通过 `DomainMatcher::priority()` 路径段数加分修复（见 §4） |
| `resHeaders://` | 同上 | 同上 | ✅ 同上 |

### ⚠️ 后续优化项（planned, not yet shipped as of 2026-06-17）

| 协议 | 问题 | 说明 |
|------|------|------|
| `reqCookies://` | 全部累积无同名去重 | 实际场景中很少对同一 cookie 设置不同值，影响较小 |
| `resCookies://` | 同上 | 同上 |
| `urlParams://` | 同上 | 同上 |
| `trailers://` | 同上 | 同上 |
| `params://` | 多条规则的 JSON 不做 deep merge | 语义较复杂，需单独设计 |
| `resMerge://` | 同上 | 同上 |

## 4. 修复方案（✅ 已落地，2026-06-17）

### 4.1 根因分析

规则遍历管道：parse → sort by Reverse(priority) (stable sort) → match → convert

`convert_core_result_to_proxy` 按 `core_result.rules` 的顺序遍历，规则已按 priority 降序排列。

**核心问题**：`DomainMatcher::priority()` 中，`PathPattern::Exact` 固定加 15 分，不考虑路径长度：

```
https://example.com/           → 100 + 5(protocol) + 15(exact_path) = 120
https://example.com/api/v1/    → 100 + 5(protocol) + 15(exact_path) = 120
```

两条规则优先级相同，stable sort 保持原始声明顺序（宽泛的 `/` 在前，具体的 `/api/v1/` 在后）。在 `reqHeaders://` 的"先到先得"去重逻辑下，宽泛规则的 header 先写入，具体规则的同名 header 被跳过。

### 4.2 修复策略（已实施）

**修复路径优先级**：在 `DomainMatcher::priority()` 的 `PathPattern::Exact` / `PathPattern::Prefix` 分支中，增加基于路径段数（path segment count）的额外加分，使更深的路径获得更高的优先级。当前实际代码为：

```rust
if self.path_pattern.is_some() {
    priority += match &self.path_pattern {
        Some(PathPattern::Exact(path)) => {
            let segments = path.split("/").filter(|s| !s.is_empty()).count() as i32;
            15 + segments
        }
        Some(PathPattern::Prefix(prefix)) => {
            let segments = prefix.split("/").filter(|s| !s.is_empty()).count() as i32;
            10 + segments
        }
        None => 0,
    };
}
```

效果（以 base = `100`、protocol 命中 `+5` 为参照；下表只展示 path 部分加分）：

```
PathPattern::Exact("/")         → 15 + 0 segments = 15
PathPattern::Exact("/api")      → 15 + 1 segment  = 16
PathPattern::Exact("/api/v1/")  → 15 + 2 segments = 17
PathPattern::Prefix("/api/*")   → 10 + 1 segment  = 11
```

修复位置：`crates/bifrost-core/src/matcher/domain.rs` → `DomainMatcher::priority()`（详见同文件 `test_priority_path_depth_exact` / `test_priority_path_depth_prefix` 等测试）。

**保持「先到先得」逻辑不变**：`convert_core_result_to_proxy`（`crates/bifrost-cli/src/parsing/rules.rs:471-791`）中 `Protocol::ReqHeaders` 和 `Protocol::ResHeaders` 的去重逻辑沿用「同名 key 已存在则跳过」未修改。因为修复优先级后，更具体的路径获得更高优先级，排在前面先写入，宽泛规则后遍历时同名 header 被跳过。

### 4.3 修复影响分析

| 影响范围 | 说明 |
|---------|------|
| 转发类协议 | ✅ 无影响，non-multi_match 只匹配一条，优先级更高的先命中 |
| 修改类 KV 集合 | ✅ 修复 reqHeaders/resHeaders 的同名覆盖问题 |
| 修改类标量值 | ✅ 无影响，non-multi_match |
| 修改类纯累积 | ✅ 无影响，不涉及同名去重 |
| 已有测试 | 需验证 `test_priority_with_exact_path` 等测试的预期值是否需要更新 |

### 4.4 测试计划（落地状态）

1. **单元测试**：在 `domain.rs` 中新增测试验证路径深度影响优先级 — ✅ 已落地（`test_priority_path_depth_exact`、`test_priority_path_depth_prefix`、`test_priority_with_exact_path` 等）
2. **单元测试**：在 `rules.rs` 中验证 `reqHeaders://` 和 `resHeaders://` 的同名覆盖行为 — ✅ 已落地（`rules.rs` 内 `parse_rules` + specific-vs-general 用例，见 953 行附近 `bifrost.local /` vs `/api/v1/` 的对照测试）
3. **E2E 测试**：验证真实规则文件中的 header 覆盖场景 — ⚠️ planned, not yet shipped as of 2026-06-17（当前覆盖以单元测试 + human_tests 为主）
4. **human_tests**：在 `human_tests/` 中创建 `rule-merge-headers.md` 测试文档 — ✅ 已落地（同时新增了 `human_tests/rule-merge-strategy.md`）
