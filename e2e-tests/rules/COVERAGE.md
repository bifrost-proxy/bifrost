# `old_doc/rules` 对照覆盖清单

本文档只按 `old_doc/rules/*.md` 逐个核对 `e2e-tests/rules` 的规则夹具设计是否有落点。

## 覆盖状态说明

- `✅` 已覆盖: 有明确规则夹具，且当前仓库已有对应脚本或稳定回归场景
- `⚠️` 部分覆盖: 有夹具，但仍混合在大场景中，或存在已知缺口/回归
- `🔄` 已补夹具待验证: 本轮已补到 `e2e-tests/rules`，但还缺专门断言或专项执行
- `➖` 非叶子协议: 这是语法/索引/概念文档，不单独作为协议用例统计

## 本轮 review 结论

- 本次对 `old_doc/rules` 中的条目做了逐个检查。
- 对真正可测试但原来没有独立夹具的协议，已补充到 `e2e-tests/rules/`。
- 对 `rule`、`pattern`、`operation`、`protocols`、`filters` 这类元文档，改为单独标记为 `➖`，不再和叶子协议混算覆盖率。
- `cipher.md` 实际描述的是 `tlsOptions`，这里按 `tlsOptions` 语义归档。

## 1. 路由与传输

| `old_doc` 条目          | 状态 | 规则夹具                       | 说明                                                 |
| ----------------------- | ---- | ------------------------------ | ---------------------------------------------------- |
| `host`                  | ✅   | `forwarding/http_to_http.txt`  | 基础 host 转发已覆盖                                 |
| `http`                  | ✅   | `forwarding/http_to_http.txt`  | HTTP 转发已覆盖                                      |
| `https`                 | ✅   | `forwarding/http_to_https.txt` | HTTP 到 HTTPS 已覆盖                                 |
| `ws`                    | ✅   | `forwarding/ws_forward.txt`    | WebSocket 已覆盖                                     |
| `wss`                   | ✅   | `forwarding/ws_forward.txt`    | WSS 已覆盖                                           |
| `proxy`                 | ✅   | `advanced/proxy.txt`           | 二级代理已覆盖                                       |
| `pac`                   | ✅   | `forwarding/pac.txt`           | PAC 已覆盖                                           |
| `redirect`              | ✅   | `redirect/redirect.txt`        | 301/302 与相对路径已覆盖                             |
| `locationHref`          | ✅   | `redirect/redirect.txt`        | 与 redirect 同组维护                                 |
| `file`                  | ✅   | `template/values.txt`          | 文件型值来源已覆盖                                   |
| `tunnel`                | 🔄   | `forwarding/tunnel.txt`        | 本轮新增隧道转发与默认端口场景                       |
| `cipher` (`tlsOptions`) | 🔄   | `tls/tls_options.txt`          | 本轮补 `tlsOptions` 多种取值方式                     |
| `sniCallback`           | 🔄   | `tls/sni_callback.txt`         | 本轮补插件型 TLS 证书回调场景                        |

## 2. 请求修改

| `old_doc` 条目 | 状态 | 规则夹具                                                                       | 说明                                           |
| -------------- | ---- | ------------------------------------------------------------------------------ | ---------------------------------------------- |
| `reqHeaders`   | ⚠️   | `request_modify/headers.txt`, `regression/rule_semantics_split_parsing.txt`    | 头部增删改已覆盖，但分词/复杂 value 仍属高风险 |
| `reqBody`      | ✅   | `request_modify/body.txt`, `advanced/body_size_strategy.txt`                   | 已覆盖                                         |
| `reqPrepend`   | ✅   | `request_modify/body.txt`, `advanced/body_size_strategy.txt`                   | 已覆盖                                         |
| `reqAppend`    | ✅   | `request_modify/body.txt`, `advanced/body_size_strategy.txt`                   | 已覆盖                                         |
| `reqReplace`   | ✅   | `request_modify/body.txt`, `advanced/body_size_strategy.txt`                   | 已覆盖                                         |
| `reqCookies`   | ⚠️   | `request_modify/cookies.txt`                                                   | 已有基础夹具，边界断言仍偏少                   |
| `reqCors`      | 🔄   | `request_modify/req_cors.txt`                                                  | 本轮新增快捷模式与详细模式                     |
| `reqDelay`     | ✅   | `response_modify/delay.txt`                                                    | 与响应延迟共用场景                             |
| `reqSpeed`     | ✅   | `advanced/speed.txt`                                                           | 已覆盖                                         |
| `reqType`      | ✅   | `advanced/content_type.txt`                                                    | 已覆盖                                         |
| `reqCharset`   | ✅   | `advanced/content_type.txt`                                                    | 已覆盖                                         |
| `reqMerge`     | 🔄   | `request_modify/req_merge.txt`                                                 | 本轮新增 form/json 合并设计                    |
| `reqScript`    | ✅   | `request_modify/req_res_script.txt`                                            | 已有专项脚本驱动                               |
| `forwardedFor` | 🔄   | `request_modify/forwarded_for.txt`                                             | 本轮补固定 IP 与模板变量透传                   |
| `method`       | ✅   | `request_modify/method.txt`                                                    | 已覆盖                                         |
| `auth`         | ✅   | `advanced/auth.txt`                                                            | 已覆盖                                         |
| `ua`           | ⚠️   | `request_modify/ua.txt`                                                        | 已有基础覆盖，复杂 UA 字符串仍需更强断言       |
| `referer`      | ✅   | `request_modify/referer.txt`                                                   | 已覆盖                                         |
| `urlParams`    | ✅   | `request_modify/url_params.txt`                                                | 已覆盖                                         |
| `pathReplace`  | ⚠️   | `request_modify/url_params.txt`, `regression/rule_semantics_split_parsing.txt` | 有回归保护，但 full URL + regex 组合仍有风险   |

## 3. 响应修改

| `old_doc` 条目  | 状态 | 规则夹具                                                                                            | 说明                                     |
| --------------- | ---- | --------------------------------------------------------------------------------------------------- | ---------------------------------------- |
| `resHeaders`    | ⚠️   | `response_modify/headers.txt`                                                                       | 基础增删改已覆盖，复杂组合仍偏少         |
| `resBody`       | ✅   | `response_modify/body.txt`, `response_modify/res_body_large.txt`, `advanced/body_size_strategy.txt` | 已覆盖                                   |
| `resPrepend`    | ✅   | `response_modify/body.txt`, `advanced/body_size_strategy.txt`                                       | 已覆盖                                   |
| `resAppend`     | ✅   | `response_modify/body.txt`, `advanced/body_size_strategy.txt`                                       | 已覆盖                                   |
| `resReplace`    | ✅   | `response_modify/body.txt`, `advanced/body_size_strategy.txt`                                       | 已覆盖                                   |
| `resCookies`    | ⚠️   | `response_modify/cookies.txt`                                                                       | 基础覆盖已有，组合断言仍偏少             |
| `resCors`       | ⚠️   | `response_modify/cors.txt`                                                                          | 已有基础夹具，详细字段与预检场景仍可加强 |
| `resDelay`      | ✅   | `response_modify/delay.txt`                                                                         | 已覆盖                                   |
| `resSpeed`      | ✅   | `advanced/speed.txt`                                                                                | 已覆盖                                   |
| `resType`       | ✅   | `advanced/content_type.txt`                                                                         | 已覆盖                                   |
| `resCharset`    | ✅   | `advanced/content_type.txt`                                                                         | 已覆盖                                   |
| `resMerge`      | ✅   | `advanced/body_size_strategy.txt`                                                                   | 已覆盖                                   |
| `resScript`     | ✅   | `request_modify/req_res_script.txt`                                                                 | 已有专项脚本驱动                         |
| `responseFor`   | 🔄   | `response_modify/response_for.txt`                                                                  | 本轮补 `x-bifrost-response-for` 语义     |
| `statusCode`    | ✅   | `response_modify/status.txt`                                                                        | 已覆盖                                   |
| `replaceStatus` | ✅   | `response_modify/status.txt`                                                                        | 已覆盖                                   |
| `trailers`      | ✅   | `response_modify/trailers.txt`                                                                      | 已覆盖                                   |
| `headerReplace` | ✅   | `advanced/header_replace.txt`                                                                       | 已覆盖                                   |
| `cache`         | ✅   | `advanced/cache.txt`                                                                                | 已覆盖                                   |
| `attachment`    | ✅   | `advanced/cache.txt`                                                                                | 已覆盖                                   |

## 4. 内容注入与静态内容

| `old_doc` 条目 | 状态 | 规则夹具                                                     | 说明   |
| -------------- | ---- | ------------------------------------------------------------ | ------ |
| `htmlAppend`   | ✅   | `content_inject/html.txt`, `advanced/body_size_strategy.txt` | 已覆盖 |
| `htmlPrepend`  | ✅   | `content_inject/html.txt`, `advanced/body_size_strategy.txt` | 已覆盖 |
| `htmlBody`     | ✅   | `content_inject/html.txt`, `advanced/body_size_strategy.txt` | 已覆盖 |
| `jsAppend`     | ✅   | `content_inject/js.txt`, `advanced/body_size_strategy.txt`   | 已覆盖 |
| `jsPrepend`    | ✅   | `content_inject/js.txt`, `advanced/body_size_strategy.txt`   | 已覆盖 |
| `jsBody`       | ✅   | `content_inject/js.txt`, `advanced/body_size_strategy.txt`   | 已覆盖 |
| `cssAppend`    | ✅   | `content_inject/css.txt`, `advanced/body_size_strategy.txt`  | 已覆盖 |
| `cssPrepend`   | ✅   | `content_inject/css.txt`, `advanced/body_size_strategy.txt`  | 已覆盖 |
| `cssBody`      | ✅   | `content_inject/css.txt`, `advanced/body_size_strategy.txt`  | 已覆盖 |

## 5. 控制与匹配

| `old_doc` 条目  | 状态 | 规则夹具                     | 说明                                                        |
| --------------- | ---- | ---------------------------- | ----------------------------------------------------------- |
| `enable`        | ⚠️   | `control/enable_disable.txt` | 当前主要覆盖“规则启停”能力，不等于穷举所有 `enable://` 开关 |
| `disable`       | ⚠️   | `control/enable_disable.txt` | 同上                                                        |
| `delete`        | 🔄   | `control/delete.txt`         | 本轮新增请求头/响应头删除夹具                               |
| `ignore` (`passthrough`) | ✅   | `control/ignore.txt`         | 已覆盖；旧 `ignore://` 输入会自动归一化为 `passthrough://` |
| `skip`          | 🔄   | `control/skip.txt`           | 本轮新增按 pattern / operation 跳过规则                     |
| `includeFilter` | ⚠️   | `control/include_filter.txt` | 已有基础场景，但仍以条件组合为主                            |
| `excludeFilter` | ⚠️   | `control/exclude_filter.txt` | 已有基础场景，但尚缺更多响应期断言                          |
| `lineProps`     | ✅   | `control/line_props.txt`     | 已覆盖                                                      |

## 6. 非叶子文档

这些条目在 `old_doc/rules` 中属于语法说明、概念说明或索引页，不单独要求 `e2e-tests/rules` 为其创建协议级规则文件。

| `old_doc` 条目 | 状态 | 说明                                                          |
| -------------- | ---- | ------------------------------------------------------------- |
| `rule`         | ➖   | 总体规则语法说明                                              |
| `pattern`      | ➖   | 匹配模式总览；其细分语义由 `pattern/` 与 `combination/` 覆盖  |
| `operation`    | ➖   | `protocol://value` 结构说明                                   |
| `protocols`    | ➖   | 协议索引页                                                    |
| `filters`      | ➖   | 过滤器总览；具体由 `includeFilter` / `excludeFilter` 夹具承接 |

## 7. 本轮新增夹具

- `forwarding/tunnel.txt`
- `request_modify/forwarded_for.txt`
- `request_modify/req_cors.txt`
- `request_modify/req_merge.txt`
- `response_modify/response_for.txt`
- `control/delete.txt`
- `control/skip.txt`
- `tls/tls_options.txt`
- `tls/sni_callback.txt`

## 8. 仍需后续专项执行的高风险项

- `pathReplace`: full URL + regex 组合仍需继续回归
- `reqHeaders`: 复杂 value、引用值和 host target 连写仍是高风险解析点
- `tlsOptions` / `sniCallback`: 需要结合真实 TLS 证书或插件环境执行
