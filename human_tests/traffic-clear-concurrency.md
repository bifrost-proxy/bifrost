# 并发清空抓包回归

## 功能与前置条件

验证并发清空不会阻塞后续抓包，以及 WebUI 清空按钮的等待、失败恢复和重试。
使用 Linux 隔离云环境，官方 Rust 1.95.0 构建当前分支；禁止使用真实数据库、
系统代理或 9900 端口。所有服务采用临时数据目录，设置
`BIFROST_DISABLE_TRAY=1`、`BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`，
并携带 `--no-system-proxy`。

先执行 `cargo build --bin bifrost`。浏览器测试会分配临时端口和数据目录，
可通过以下命令复跑对应真实链路（从 `web/` 执行）：

```bash
BIFROST_UI_TEST_PREBUILT_BINARY=target/debug/bifrost \
PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH="$(command -v chromium)" \
BIFROST_DISABLE_TRAY=1 BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 \
pnpm exec playwright test tests/ui/traffic-clear-concurrency.spec.ts --workers=1
```

## 用例

### TC-CLEAR-回归-01：并发清空与持续转发

1. 打开测试代理的 `/_bifrost/traffic`，等待列表和 push 连接就绪。
2. 用 8 路本机 mock HTTP 流量连续各发送 12 个请求，同时发 32 个
   `DELETE /_bifrost/api/traffic`，每个清空请求超时限制 15 秒。
3. 等待全部操作结束，再通过代理发送 5 个带唯一 path 的请求。
4. 检查 mock 收到 96 个持续请求；所有 clear 返回成功；5 个后续请求均出现在
   API 数据库查询、浏览器列表和 `/api/push` WebSocket frame 中。
5. 对照旧实现：两个重叠 clear 可互相持锁，后续转发成功但无法写入/推送；
   修复后所有操作必须完成。

### TC-CLEAR-回归-02：连点、失败和恢复

1. 打开 Traffic，发送一条唯一 path 的代理请求并确认行可见。
2. 在浏览器暂挂 DELETE 响应，连续点击清空按钮两次。
3. 确认仅一条 DELETE，按钮 disabled，原记录仍可见且未显示成功消息。
4. 返回 HTTP 500，错误为 `injected clear failure`。
5. 确认错误可见、原记录仍可见、按钮恢复 enabled。
6. 再次点击，放行真实 DELETE；确认成功消息、记录消失、按钮恢复，DELETE 共两次。

### TC-CLEAR-回归-03：同步读取与另一客户端删除重叠

1. 打开 Traffic，在浏览器清空按钮对应 DELETE 完成后，通过本机 mock 代理请求生成一条带唯一 path 的记录。
2. 截取随后 `traffic/updates` GET 的真实响应快照，暂挂该响应，取得该记录的 ID。
3. 测试明确暂挂清空触发的替换 WebSocket 注册；取得旧 GET 快照后放行重连，等待替换连接收到 `connected` 且发送 `need_traffic` 订阅，再发另一客户端的按 ID DELETE。确认响应正文实际清除 1 条，随后该 ID 的 GET 为 404，浏览器收到包含该 ID 的 `traffic_deleted` frame。不把 HTTP 快照就绪当作 push 连接就绪。
4. 用真实旧快照中的该行与 metadata 构造 `traffic_delta` update，在删除 frame 已被页面处理后经浏览器 WebSocket route 投递；保持 GET 暂挂，等待 UI 批次刷新，确认旧行未短暂恢复。该投递是受控时序注入，不声称真实后端此次运行恰好出现队列乱序。
5. 再发送一条带不同唯一 path 的代理请求；放行此前暂挂的旧 GET 响应。
6. 确认清空按钮恢复 enabled，新记录可见，旧记录没有恢复，Local 客户端计数为 1。
7. 对照旧 additive merge：迟到更新可清除删除标记，旧快照记录会被重新加入；修复后删除保持权威，读取失效并恢复最新成员信息。

### TC-CLEAR-回归-04：清空成功与同步失败分离

1. 打开 Traffic，让真实 DELETE 成功；在服务端清空与响应完成之间生成一条旧快照记录，暂挂随后的真实 `traffic/updates` GET。
2. 确认清空成功消息与 enabled 按钮已出现，不等待这个 GET。
3. 暂停浏览器 RAF 调度，生成一条不同 path 的真实代理请求，确认浏览器收到含该 path 的 push frame，使写入确定性滞留待刷批次。
4. 用另一客户端按 ID 删除旧快照行，确认实际清除 1 条及删除 frame；放行旧 GET，并对替代 GET 注入 HTTP 500。
5. 恢复 RAF；确认新请求行仍可见，旧快照行没有恢复，清空按钮 enabled，Local 客户端计数最终为 1。
6. 测试结束无论成功与否都释放 GET/RAF、关闭本机 mock 服务；不使用真实数据库、系统代理或用户设备。

## 清理与工具边界

Playwright 全局 teardown 停止隔离代理和流量生成器；删除本次运行的临时数据目录。
仓库要求 human_tests 通过 Chrome DevTools MCP 执行。若当前环境没有该 MCP，
上述真实 Chromium 自动测试只能作为 UI/E2E 证据，严格的 MCP 人类场景门禁仍待验证，
不能标记为通过。
