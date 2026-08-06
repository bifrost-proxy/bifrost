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
