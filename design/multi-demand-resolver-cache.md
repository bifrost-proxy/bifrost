# 多需求 Resolver 缓存

Bifrost 规则 resolver 现在只缓存 URL、host、path 匹配出的候选规则，不缓存某一次请求的最终命中结果。

这样同一个 URL 在不同请求头或 method 下仍会重新评估 `includeFilter`、`excludeFilter`、`skip` 和规则 disabled 状态，避免多条同 pattern 规则在候选缓存命中后被上一请求的 filter 结果污染。

覆盖场景：

- `host://` 按当前请求 header 路由到不同 upstream。
- 前序 `host://` 规则被 include/exclude filter 跳过时，后续同协议规则和无 filter fallback 仍可继续生效。
- `reqHeaders://`、`resHeaders://`、`replaceStatus://` 按当前请求 header 应用不同修改。
- `redirect://`、`statusCode://` 按当前请求 header 返回不同直接代理决策。
- `excludeFilter://` 与带 filter 的 `skip://` 在候选缓存命中后仍按当前请求重新判断。
