# SSE 流式响应脚本

## 目标

新增 `resStreamScript://<name>` 规则，同时支持两种真正的流式模式：

- **mock source**：脚本自身持续生成 event；每次生成一个就立即写给下游。
- **upstream transform**：逐个处理上游 SSE event，并将零个、一个或多个新 event 立即发送给下游。

两种模式都不得预先收集完整输出或等待上游完整响应，不得破坏无该规则时的零拷贝式 SSE 转发，也不得改变现有 `resScript` 的整包语义。

典型真实用例是把一种上游 SSE 协议逐事件转换为另一种下游 SSE 协议，包括文本 delta、function call arguments、usage、错误和 `[DONE]`。

## 非目标

- 不按单个字符调用脚本；网络 chunk 可能切在 UTF-8 或 SSE 行中间，脚本输入必须是完整 SSE event。
- 第一版不对压缩 SSE 做增量解压；命中规则时请求上游使用 `Accept-Encoding: identity`，响应仍压缩则明确失败。
- 不把 `resReplace`、HTML 注入或普通 `resScript` 隐式改成流式操作。

## 规则与脚本契约

```text
api.example.com resStreamScript://chat-to-responses
```

为保证规则可单文件分享，三种脚本规则同时支持块变量内联脚本：

```text
api.example.com reqScript://{request-adapter} resStreamScript://{response-adapter}

``` request-adapter
// request script
```

``` response-adapter
// stream response script
```
```

解析后如果脚本引用来自当前规则或 Values 的 `{block}`，执行块内容；普通名称继续从对应 `scripts/<type>/` 目录加载。流量记录和错误信息使用块变量名而不是整段源码，禁止泄露脚本内容或其中的敏感值。

脚本仍存放于 `scripts/response/<name>.js`。每个 HTTP stream 只初始化一次 QuickJS context，后续回调共享闭包和 JS 对象状态，并提供：

- `ctx.phase`: `stream_start`、`stream_mock_next`、`stream_event`、`stream_end`、`stream_error`
- `stream.event`: `{ id, event, data, retry }`，仅 `stream_event` 存在
- `stream.output`: 字符串、SSE event 对象或数组；数组允许 0..N 输出
- `stream.mode`: 初始化阶段设为 `mock` 或 `transform`
- `stream.next()`: mock 模式由 host 反复调用；每次只返回本轮输出、可选 `delayMs` 与 `done`
- `stream.onEvent(event)`: transform 模式每收到一个完整上游 event 调用一次
- `stream.onEnd()`: transform 上游结束或 mock `done` 时调用一次

输出字符串被视为原始 UTF-8 body bytes，不再经过 JSON 字符串二次编码。event 对象由 Rust SSE encoder 编码。`stream_start` 可以返回初始事件，`stream_end` 返回收尾事件。

## 数据流

1. 请求规则和 `reqScript` 在转发前把上游请求改为 `stream=true`。
2. 收到响应头后初始化一个 stream script session；失败时仍可返回普通 502。
3. 包装 Hyper upstream `Body`，增量缓存不完整行，按空行切出完整 SSE event。
4. 通过有界 channel 把 event 送到单个持久 QuickJS session，保持同一响应内状态。
5. 每个脚本输出立即编码成一个或多个 Hyper DATA frame；HTTP/1.1 使用 chunked，HTTP/2 使用 DATA frame。
6. 转换后 body 再进入现有 SSE tee，Traffic 中记录下游实际看到的 SSE；原始上游 event 可选记入 raw body。
7. 下游取消时终止 worker 并 drop 上游 body；有界 channel 传播背压，禁止无界缓存。

mock 模式不读取上游 body。host 调用 `stream.next()`，将本轮输出写入有界下游 channel，等待该输出被消费后才进入下一轮；`delayMs` 在本轮写出后生效。脚本不能一次返回无限数组，单轮输出数和字节数都有上限。

## Header 与错误语义

- 初始化后移除 `Content-Length`、`Content-Encoding`，设置 `Content-Type: text/event-stream; charset=utf-8`、`Cache-Control: no-cache`、`X-Accel-Buffering: no`。
- 脚本初始化失败发生在响应提交前：返回 502。
- 响应已经开始后，解析/脚本错误不能再改 HTTP status：发送标准 SSE `error` event 后关闭流。
- 单 event 输入、累计未完成 event、单次输出和 session 总输出都受 sandbox limit 限制。
- 每个 event 有独立执行超时；禁止 stream event hook 使用 `net`，文件 API 默认只读/关闭，避免阻塞代理热路径。

## 兼容与冲突

- `resScript` 保持收完整 body 后执行。
- 同一规则同时声明 `resScript` 与 `resStreamScript` 时拒绝/跳过并给出明确诊断，避免顺序含糊。
- 非 SSE 响应命中 `resStreamScript` 时不猜测协议，返回明确错误。
- 普通 SSE 未命中该规则时继续走现有 `create_sse_tee_body` 快速路径。

## 验证清单

- SSE event 跨任意 network chunk 边界仍只调用脚本一次。
- 一个输入 event 可产生 0、1、N 个输出，且在上游完成前下游已经收到 delta。
- mock 脚本的第一个事件在脚本结束前到达客户端；每轮只调用一次 `next()`，不会预收集后续事件。
- 测试记录 `upstream_event_at`、`script_output_at`、`downstream_received_at`、`upstream_done_at`；至少两个 downstream delta 必须满足 `downstream_received_at < upstream_done_at`。
- 用永不结束的上游 SSE 验证客户端仍持续收到转换结果，证明实现不依赖完整 body / `[DONE]`。
- 下游慢读时内存有界；下游断开会取消上游读取。
- HTTP/1.1 与 HTTP/2 均不产生 `Content-Length`，事件边界正确。
- Chat text delta、tool call name/arguments delta、finish、usage、`[DONE]` 能转换成合法 Responses SSE。
- Codex CLI 完成聊天、真实 shell/tool、代码写入和图片理解。
- `test_response_stream_script.sh` 必须进入 `scripts/ci/proxy-coverage-shell-tests.txt`，确保真实 SSE 链路同时贡献 `bifrost-proxy` production 90% coverage gate，而不是只在普通 E2E job 中运行。

## 文档站交付边界

- `docs/scripts.md` 与 `docs-en/scripts.md` 是用户常用的脚本能力入口，构建后分别对应 `/reference/scripting` 与 `/en/reference/scripting`。两页必须能独立说明 `resStreamScript` 的 Transform、Mock、逐事件立即输出、回调超时和组合限制，不能只依赖另一个页面的一行链接。
- `docs/rules/scripts.md` 与 `docs-en/rules/scripts.md` 继续作为完整规则契约，维护 inline block、event 字段、返回值、背压、响应头与 16 MiB 边界。入口页链接到规则参考，但不复制所有底层实现细节。
- `site/scripts/sync-docs.mjs` 在构建前把上述源文档同步到 `site/src/content/docs/`；自动化 E2E 必须检查最终 `site/dist`，避免只检查源文件却漏掉路由映射或静态构建问题。
- 主仓库 `Site` workflow 只做构建校验。正式发布必须使用 `SITE_URL=https://bifrost-proxy.github.io/ BASE_PATH=/ pnpm run site:build`，再把 `site/dist/` 原样同步到 `bifrost-proxy/bifrost-proxy.github.io` 并等待两条 Pages workflow 成功。
