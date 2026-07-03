# 多需求 Resolver 缓存设计方案

## 背景

Bifrost 的规则 resolver 需要在每一次 HTTP/HTTPS/CONNECT 请求进入代理时决定：

- 这条请求命中了哪些规则；
- 每条规则的 include/exclude filter 是否成立；
- 是否被别的规则通过 `skip://` 屏蔽；
- 最后需要执行哪些 `host://`、`redirect://`、`statusCode://`、`reqHeaders://`、`resHeaders://`、`replaceStatus://` 等指令。

规则匹配本身依赖 URL / host / path 的正则或 wildcard；相同 URL 会被大量请求重复访问，全量重跑 matcher 是 CPU 热点。历史上早期版本尝试直接缓存 "最终命中规则集合"，但这样做会把某次请求的 header/method/status/body 一起冻结进缓存，导致：

- 同一 URL 在不同 header 下走 `host://` 得到不同 upstream 的场景被冻结成第一次进入缓存的那次结果；
- `includeFilter` / `excludeFilter` 依赖 method、请求头、client_ip、cookie 的规则被跳过或错误命中；
- `skip://` 带 filter 的分支被上一请求的判定结果污染；
- 响应阶段的 filter（`statusCode`、`resHeaders`）在 request 阶段就被算成 false，永远命不中。

多需求 Resolver 缓存把 "URL 结构性匹配" 与 "本次请求语义过滤" 拆开，前者进 LRU，后者每次请求重算。这样既保留正则/wildcard 缓存的性能收益，又保证 filter 语义随请求变化。

## 用户目标验证清单

### 必须实现

- 同一个 URL 的候选规则集合会进入 LRU 缓存，命中率高的场景不必重跑 matcher。
- 缓存 key 只包含 URL / host / path 三个纯结构字段，不掺任何请求上下文。
- `includeFilter`、`excludeFilter`、`skip://` 每次请求都在候选集上重新评估。
- 响应相关的 filter（`Filter::StatusCode`、响应头 `HeaderMatch { is_request: false }`）在 request 阶段被推迟，到响应阶段用 `resolve_uncached` 重算。
- 缓存容量可配置（`with_cache_capacity`），也可以完全关闭（`disable_cache`）。
- 任何修改规则/values 的写入（`add_rule`、`set_value`、`merge_from_store` 等）必须 `clear_cache`，避免旧候选被错误复用。
- CLI 规则解析自检和 resolver 单元测试可以直接调 `resolve_uncached` 绕开缓存。

### 必须不破坏

- 现有 `@important` / matcher specificity / negated rule 优先级不变。
- `bp://`、`value_ref`、内建变量、`resBody`、`resStatus` 等既有指令仍工作。
- CONNECT / HTTPS tunnel / websocket 请求的解析路径不变。
- resolver 对 rule store 快照的读写并发安全（`Arc<RwLock<LruCache>>`）。

### 必须真实验证

- 单元测试覆盖多 header、多 method、多 client_ip、多 cookie 场景。
- 单元测试覆盖 skip / include / exclude / negated / response filter deferred 五类。
- 缓存写入命中/未命中路径都被真实断言，不是只跑一次不检查缓存结果。
- LRU 容量满时的淘汰行为可复现。

## 产品语义

### Phase A：候选缓存（URL 结构层）

`get_matcher_candidates(ctx)`：

1. 计算 `key = "{url}|{host}|{path}"`。
2. LRU `get`：若命中返回 `Vec<CandidateMatch>` 的克隆。
3. 未命中则调用 `collect_matcher_candidates`：逐条跑 `rule.matcher.matches(ctx)`，得到每条命中规则的 `rule_index`、`captures`、`is_negated`。
4. LRU `insert(key, value)`。

`CandidateMatch` 只有这三个字段，等价于 "在这个 URL 上哪些规则的 matcher 命中"，不会包含任何请求上下文，因此可以跨请求安全复用。

### Phase B：filter gate（请求语义层）

`apply_filter_gate(candidates, ctx, ...)`：

1. `collect_active_skips_from(candidates, ctx)`：从当前候选集里挑出仍然有效的 `skip://` 指令（skip 自己也会通过 filter 决定是否激活）。
2. 逐条候选调用：
   - `is_disabled`：`@disabled` 行级语义。
   - `include_filters` / `exclude_filters`：`filter_is_evaluable` 决定是否在 request 阶段能算；不能算的（响应相关）留到响应阶段。
   - `should_skip_rule(candidate, active_skips)`：命中 skip 的规则被丢弃。
3. 通过所有 gate 的规则组装成 `ResolvedRules` 返回。

Phase B 每次请求都算，绝不进入缓存。

### `resolve_uncached`

完全绕开候选缓存，直接 `collect_matcher_candidates` + `apply_filter_gate`。当前使用点：

- **响应阶段**：`set_response` 把 status/响应头写入 context 后重新评估 `Filter::StatusCode` 与响应头 filter。
- **CLI 规则解析自检**：`crates/bifrost-cli/src/parsing/rules.rs`。
- **resolver 自身单元测试**。

## 技术细节

### 关键类型与常量（`crates/bifrost-core/src/rule/resolver.rs`）

- `const DEFAULT_CACHE_CAPACITY: usize = 1000;`
- `struct CandidateMatch { rule_index, captures, is_negated }`
- `struct LruCache { cache: LruCacheImpl<String, Vec<CandidateMatch>> }` 基于 `lru::LruCache`。
- `RulesResolver { cache: Arc<RwLock<LruCache>>, ... }`

### 主要方法

| 方法 | 位置 | 作用 |
| ---- | ---- | ---- |
| `with_cache_capacity(cap)` | resolver.rs:198 | 调整 LRU 容量 |
| `disable_cache()` | resolver.rs:203 | 关闭缓存（容量置 0 或跳过 get/insert） |
| `clear_cache()` | resolver.rs:223 | 写入类操作后清空 |
| `resolve` | resolver.rs | Phase A + Phase B，主入口 |
| `resolve_uncached(ctx)` | resolver.rs:328 | 直接跑 collect + gate |
| `get_matcher_candidates(ctx)` | resolver.rs:350 | Phase A，读/填 LRU |
| `collect_matcher_candidates(ctx)` | resolver.rs:376 | 未命中路径：逐条 matcher.matches |
| `apply_filter_gate(cands, ctx, ...)` | resolver.rs:416 | Phase B，filter/skip |
| `filter_is_evaluable(filter, ctx)` | resolver.rs:514 | 判断当前 phase 能否算这个 filter |
| `collect_active_skips_from(cands, ctx)` | resolver.rs:593 | 激活 skip 集合 |

### Filter 分层

`filter_is_evaluable` 决定 filter 属于 request 阶段还是必须留到响应阶段：

- 请求 header (`HeaderMatch { is_request: true }`)、method、path、URL、client_ip、cookie → request 阶段可算。
- 响应 header (`HeaderMatch { is_request: false }`)、`Filter::StatusCode` → request 阶段跳过，响应阶段用 `resolve_uncached` 重算。

### 缓存失效点

- `add_rule` / `remove_rule` / `set_value` / `merge_from_store` / `replace_all` → `clear_cache`。
- rule 内容变化触发上层 resolver rebuild 时也必然重建 LRU。

### 并发

`Arc<RwLock<LruCache>>` 支持多请求并发读、写入时短暂互斥。写入远小于读取（只在规则修改），冲突可忽略。

## CLI + Web + Admin API

本改动全在核心 resolver 层，没有新增用户可见的 CLI 子命令 / Web 路由 / Admin API。相关外围行为：

- CLI 规则解析自检 (`bifrost rule check <file>`) 通过 `resolve_uncached` 保证与运行时语义一致。
- Web/Traffic panel 展示的 "matched rules" 数据本质上是 `ResolvedRules`，因此本次多需求缓存对现有 UI 完全透明；无需 UI 改动。
- 未来若要给 resolver 缓存加可观察性（命中率、size），可以扩展 `bifrost status --format json` 但当前不做。

## Sync 边界

Resolver 缓存是本机运行时缓存，不参与任何跨机器同步：

- 不落盘，不进 config 文件。
- Rule / values sync 更新到达本机后由上层触发 `clear_cache`，缓存自然重建。
- Group 规则 / 临时端口绑定规则的入参变化都要触发 resolver 重建，不能仅依赖 LRU 淘汰。

## Phase 拆分

### Phase 1：候选缓存与两阶段拆分

- 新增 `CandidateMatch`、`LruCache`、`get_matcher_candidates`、`collect_matcher_candidates`。
- 把 `resolve` 拆成 Phase A + Phase B。
- 所有写入路径接上 `clear_cache`。
- 单元测试覆盖 cache/no-cache 路径。

### Phase 2：filter 分层与响应阶段重算

- `filter_is_evaluable` 分类。
- 响应阶段接入 `resolve_uncached`。
- 单元测试覆盖 `test_response_filter_deferred_to_response_phase`。

### Phase 3：CLI 与自检

- CLI 规则解析自检切到 `resolve_uncached`。
- Traffic panel matched rules 语义确认。

### Phase 4：稳定性回归

- LRU 容量、淘汰、并发写入清空的测试。
- E2E 多需求场景。
- 观测代理热点是否仍在 Phase A 未命中路径，如有必要扩展缓存 key 或分层缓存。

## 测试方案

### 单元测试（`crates/bifrost-core/src/rule/resolver/tests.rs`）

| 测试 ID | 覆盖点 |
| ---- | ---- |
| `test_rules_resolver_cache` | 同 URL 两次请求，第二次命中缓存 |
| `test_rules_resolver_disable_cache` | `disable_cache` 后不复用 |
| `test_rules_resolver_clear_cache` | 写入后清空 |
| `test_lru_cache_eviction` | 超过容量的淘汰行为 |
| `test_rules_resolver_priority_sorting` | matcher priority 不受 Phase A 影响 |
| `test_rules_resolver_resolve` / `test_rules_resolver_no_match` / `test_rules_resolver_with_values` / `test_rules_resolver_with_value_ref` | 基本 resolve 语义 |
| `test_include_filter_method` | method filter 每次请求重算 |
| `test_include_filter_header_exists` | 请求 header filter 不进缓存 |
| `test_include_filter_client_ip` | client_ip filter 不进缓存 |
| `test_exclude_filter_path` / `test_exclude_filter_whistle_style_wildcard_url` / `test_exclude_filter_whistle_style_wildcard_path_prefix` / `test_long_exclude_filter_chain_uses_regular_prefix_matching` | exclude filter 语义 |
| `test_combined_include_exclude_filters` | include+exclude 组合 |
| `test_rules_resolver_skip_by_operation_allows_fallback_rule` / `test_rules_resolver_skip_by_pattern_allows_fallback_rule` | `skip://` 带 filter 时 fallback 规则仍生效 |
| `test_negated_rule_does_not_block_other_patterns` / `test_negated_rule_blocks_matching_pattern` | negated rule 交互 |
| `test_disabled_rule` | 行级 `@disabled` |
| `test_response_filter_deferred_to_response_phase` | 响应 filter 走 `resolve_uncached` |
| `test_multiple_different_protocols_all_match` | 多协议规则同时命中 |
| `test_header_filter_not_stale_across_requests` | **核心回归**：候选缓存命中后仍按当前请求 header 重算，保证不 stale |
| `test_important_priority_ordering` / `test_path_wildcard_double_star_matching` / `test_path_wildcard_via_rule_parser` | 兼容既有 matcher 语义 |
| `test_host_rule_matches_request_with_explicit_port` / `test_domain_only_rule_matches_connect_without_path` / `test_path_specific_rule_takes_priority_over_domain_only_rule` | host / path 匹配交互 |
| `test_bp_remote_url_is_preserved_as_script_reference` / `test_bp_value_ref_expands_profile_without_remote_fetch` | BP 与 value_ref 不被缓存破坏 |

### E2E 覆盖

E2E 由现有 rules 覆盖集 `e2e-tests/rules/COVERAGE.md` 索引：

- 多 header 下 `host://` 路由。
- `includeFilter` / `excludeFilter` 依赖请求 header 的规则。
- `skip://` 带 filter 与 fallback 组合。
- 响应阶段 `resHeaders` / `replaceStatus` 与 `statusCode` filter 触发。

### CLI

- `bifrost rule check <file>` 走 `resolve_uncached`，回归覆盖：syntax check + 语义与运行时一致。

### 真实场景

- 高并发同 URL 多 method / 多 header 的压力场景观察 CPU 曲线，确认缓存收益。
- 打开/关闭 `disable_cache` 对比。
- Traffic panel 中同 URL 多请求 matched rules 是否随 header 不同而不同（不出现全部相同的错误行为）。

## Review/Fix/Test 闭环

### 第 1 轮

- 复核用户目标：URL 缓存收益 + filter 每次请求重算 + 响应 filter deferred。
- 复核 diff：cache 类型、Phase A/B 拆分、写入清空点、响应阶段调用 `resolve_uncached`。
- 重点 review：
  - 缓存 key 是否绝对不包含请求上下文；
  - 所有 rule/values 写入是否都接上 `clear_cache`；
  - `filter_is_evaluable` 是否覆盖了所有响应相关 filter；
  - `resolve_uncached` 是否真的绕过 LRU 且行为等价。
- 复测：单元测试 (`cargo test -p bifrost-core rule::resolver`) + 相关 E2E。

### 第 2 轮

- 复核第 1 轮修复。
- 检查 `git status --short`、`git diff`。
- 重点 review：并发（多请求并发命中/未命中）、LRU 淘汰是否稳定、日志/tracing 是否泄漏 filter 结果被冻结的错误信息。
- 若发现响应 filter 走 request 阶段被算成 false、header filter stale、写入后 stale 复用，追加第 3 轮。

## 校验要求

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test -p bifrost-core rule::resolver`
- 相关 E2E：`e2e-tests` 中 rules coverage 集
- 需要时 `cargo test --workspace --all-features`

本机 no-local-coverage 约定时不跑 `make coverage`。

## 风险与决策点

- **缓存 key 精度**：只用 URL/host/path。若未来引入依赖 scheme 或 port 的语义（非 default port），需要显式加入 key，否则会污染。
- **LRU 容量**：默认 1000。生产 hot URL 数远大于此时命中率会下降，可扩展 `with_cache_capacity` 或引入分层缓存（host 级 + URL 级），但当前不做。
- **`resolve_uncached` 泛用性**：目前只在响应阶段、CLI 自检、测试中使用；不建议把它变成默认路径，否则失去 Phase A 收益。
- **写入清空粒度**：`clear_cache` 是全清。若单条规则修改频繁，未来可以按 rule_index 精细失效，但当前一次性清空简单且不易出错。
- **filter 分层遗漏风险**：新增 filter 类型时必须在 `filter_is_evaluable` 明确列出，否则可能被 request 阶段错误算成 false。回归测试 `test_response_filter_deferred_to_response_phase` 是防线。
- **并发**：`Arc<RwLock<LruCache>>` 在极端场景下写入拥塞可能短暂阻塞读；未观察到瓶颈时不引入更复杂 lock-free 结构。
