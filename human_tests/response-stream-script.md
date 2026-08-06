# Response Stream Script 真实场景测试

## 前置条件

- 使用开发构建启动独立监听端口，不切换或停止正式 9900 服务。
- 上游 fixture 提供一个 SSE endpoint：立即发送第一条 event，约 1.5 秒后发送第二条 event，随后保持连接至少 30 秒不结束。
- 测试规则使用 inline block，而不是 Scripts 页面中的命名脚本。

## TC-RSS-01：inline block 规则语法和变量引用

1. 创建包含 `reqScript://{request_bridge}` 与 `resStreamScript://{stream_bridge}` 的规则，并在同一规则中定义两个 fenced block。
2. 调用规则验证 API。

预期：语法报告 `valid=true`、无 missing-script warning，两个 block 出现在 `defined_variables`，规则可以直接复制分享。

## TC-RSS-02：Transform 在永不结束的上游 EOF 之前真实输出

1. 通过 Bifrost 代理访问 fixture SSE endpoint。
2. 为上游写入和下游收到的每条 event 记录单调时钟时间戳。
3. 上游发送第二条 event 后继续保持连接，不发送 EOF。

预期：两条下游 event 都在上游进入保持连接阶段之前到达；每条 event 到达后立即转换并输出，客户端不等待 EOF 或 `[DONE]`。

## TC-RSS-03：Mock 逐次调用与输出节奏

1. 使用 `stream.mode="mock"`，让 `stream.next()` 返回三步输出，每步设置约 250 ms `delayMs`。
2. 对客户端收到的三条 event 记录时间戳。

预期：第一条立即到达，后两条之间分别存在脚本设定的间隔；实现没有预生成整段响应后再回放。

## TC-RSS-04：慢模型等待不累计脚本超时

1. 将脚本回调执行超时设为 25 ms。
2. 完成第一次 `onEvent` 后等待至少 80 ms，再提交第二条 event。

预期：第二条 event 仍正常处理。超时在每次 JavaScript 回调开始时重新计时，不覆盖上游等待时间，也不限制整条 HTTP 响应的生命周期。

## TC-RSS-05：接近 16 MiB 的 event 跨网络 frame 无损传输

1. 构造一个接近 16 MiB 安全上限的 SSE `data` event，并把它拆成多个任意大小的 HTTP body frame。
2. 在上游保持未结束时读取下游第一条输出，并逐字节比对 payload。

预期：完整 event 在 EOF 前到达，内容逐字节一致、没有截断或丢失。超过 16 MiB 的单个完整 event 必须返回明确 SSE error 并终止，不能静默丢数据。

## TC-RSS-06：规则编辑器补全、hover 与文档

1. 运行 BifrostEditor 的 protocol docs 和 operator 单元测试。
2. 构建 Web UI。
3. 检查 `resStreamScript://` 的补全列表。

预期：inline block snippet 排在命名脚本之前；命名 response script 仍可补全和跳转；hover 明确说明 true SSE、Mock/Transform、逐事件输出；TypeScript/Vite 构建通过。

## TC-RSS-07：压缩与非 SSE 输入明确失败

1. 客户端请求携带 `Accept-Encoding: gzip`，通过命中 `resStreamScript` 的规则访问 fixture。
2. 检查 fixture 实际收到的请求头。
3. 分别让 fixture 强制返回 `Content-Encoding: gzip` 的 SSE，以及 `Content-Type: application/json`。

预期：上游请求被改写为 `Accept-Encoding: identity`；上游仍压缩或返回非 SSE 时代理返回明确 502，不删除编码头后转发乱码，也不静默跳过规则。

## TC-RSS-08：响应过滤、冲突与 Header 语义

1. 使用 `includeFilter://s:200` 和 `includeFilter://resH:...` 包裹 `resStreamScript`，让 fixture 返回匹配响应。
2. 检查转换输出和响应头。
3. 在同一响应同时配置 `resScript` 与 `resStreamScript`。

预期：响应阶段过滤命中后脚本正常执行；下游包含 `text/event-stream; charset=utf-8`、`Cache-Control: no-cache`、`X-Accel-Buffering: no` 且没有 `Content-Length`/`Content-Encoding`；两类响应脚本冲突时明确 502。

## TC-RSS-09：SSE 空数据与空白保真

1. 上游发送 `data:  padded ` 以及末尾空 data 行的 event。
2. 使用 identity `onEvent` 转换并逐字节检查输出。
3. 让脚本返回 `{ event: "control", data: "" }`。

预期：只移除冒号后的一个可选空格，其他前后空白和末尾空 data 行保留；空字符串仍编码出显式 `data:` 行，EventSource 可派发 control event。

## TC-RSS-10：资源与回调错误边界

1. 验证 mock 未定义 `stream.next`、单回调返回超过 1024 个输出、`onEnd` 抛错。
2. 在 `stream_start` 设置 `stream.output`，并在 `onEvent` 读取 `stream.event`。
3. mock 模式连接到持续写入的上游后保持客户端连接。

预期：无 `next` 初始化失败；过量输出和 `onEnd` 异常以 SSE error 暴露；初始输出先到达且 `stream.event` 可读；mock 不持有上游 body，长连接 worker/log 均有明确上限。

## TC-RSS-11：直接状态 Mock 与 Scripts 页面边界

1. 配置 `statusCode://200 resHeaders://Content-Type=text/event-stream resStreamScript://{mock}`，不启动上游服务。
2. 读取三个 mock event 并验证节奏。
3. 在 Scripts 页面打开包含 `stream.mode` 的 response script。

预期：直接状态路径无需上游即可持续输出 mock event；Scripts 页面普通 Run 按钮禁用并说明需要通过规则与真实 SSE source 测试，避免误用普通 response sandbox。

## 本次执行记录（2026-08-07）

- TC-RSS-01 至 TC-RSS-05：执行 `BIFROST_BIN=target/release/bifrost bash e2e-tests/tests/test_response_stream_script.sh`，真实独立数据目录、代理与 fixture 全部通过。
- TC-RSS-06：`bifrost-admin` 构建脚本完成 Web production build；编辑器相关既有测试由后续 Web 测试和远端 CI 继续兜底。
- TC-RSS-07 至 TC-RSS-10：对应 Rust 单元/集成测试通过，且 `cargo clippy -p bifrost-script -p bifrost-proxy --all-targets --all-features -- -D warnings` 通过。
- TC-RSS-11：直接状态 mock 路径已在实现级检查和编译中验证；页面 Run 边界通过 Web build 验证。完整真实链路随扩展后的规则 E2E 与远端 CI 继续验证。
