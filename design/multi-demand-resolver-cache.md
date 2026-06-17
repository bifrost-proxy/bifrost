# 多需求 Resolver 缓存

Bifrost 规则 resolver 现在只缓存 URL、host、path 匹配出的候选规则，不缓存某一次请求的最终命中结果。

这样同一个 URL 在不同请求头或 method 下仍会重新评估 `includeFilter`、`excludeFilter`、`skip` 和规则 disabled 状态，避免多条同 pattern 规则在候选缓存命中后被上一请求的 filter 结果污染。

## 实现位置

核心代码在 `crates/bifrost-core/src/rule/resolver.rs` 的 `RulesResolver` 中：

- 缓存类型为 `LruCache<String, Vec<CandidateMatch>>`，默认容量 `DEFAULT_CACHE_CAPACITY = 1000`，可通过 `with_cache_capacity` 调整或 `disable_cache` 关闭。
- 缓存 key 形如 `"{url}|{host}|{path}"`（见 `get_matcher_candidates`）。**故意**不包含 method、请求/响应 header、status code、client_ip、body 等任何请求上下文，确保候选集复用安全。
- 缓存 value 是 `Vec<CandidateMatch>`，每个元素仅记录 `rule_index`、`captures` 和 `is_negated` 三个字段，等价于 "哪些规则的 matcher 命中了这个 URL" 的纯结构匹配结果。
- 任何修改 rules / values 的写入（`add_rule`、`set_value`、`merge_from_store` 等）都会通过 `clear_cache` 把缓存清空。

## 两阶段解析

`resolve` 流程被拆成两步：

1. **Phase A — `get_matcher_candidates`**：按 URL/host/path 查（或填充）缓存，得到 `CandidateMatch` 列表。这一步只跑 `rule.matcher.matches(...)`，不看请求 header / method / status / body。
2. **Phase B — `apply_filter_gate`**：对候选集逐条评估 `is_disabled`、`include_filters`、`exclude_filters` 与 `skip://` 指令（`collect_active_skips_from` + `should_skip_rule`），每个请求都重算，不进缓存。响应相关的过滤器（`Filter::StatusCode`、响应头 `HeaderMatch { is_request: false }`）通过 `filter_is_evaluable` 在 request 阶段被跳过、留到响应阶段再判。

另外暴露了 `resolve_uncached`：完全绕过候选缓存、直接调用 `collect_matcher_candidates` + `apply_filter_gate`。当前用于 (a) 响应阶段在 `set_response` 把 status / 响应头写入 context 后重新评估那些响应相关 filter；(b) CLI `crates/bifrost-cli/src/parsing/rules.rs` 的规则解析自检；(c) resolver 自身的单元测试。

## 覆盖场景

- `host://` 按当前请求 header 路由到不同 upstream。
- 前序 `host://` 规则被 include/exclude filter 跳过时，后续同协议规则和无 filter fallback 仍可继续生效。
- `reqHeaders://`、`resHeaders://`、`replaceStatus://` 按当前请求 header 应用不同修改。
- `redirect://`、`statusCode://` 按当前请求 header 返回不同直接代理决策。
- `excludeFilter://` 与带 filter 的 `skip://` 在候选缓存命中后仍按当前请求重新判断。
