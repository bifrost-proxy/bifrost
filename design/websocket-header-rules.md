# WebSocket Header Rules

## 功能模块详细描述

WebSocket 连接在协议升级前仍然是 HTTP 握手。Bifrost 的 `reqHeaders` / `resHeaders` 等头部规则必须作用在这次升级握手上：

- 下游客户端发起 `ws://` 或 TLS 解包后的 `wss://` 请求时，代理在发往远端前修改请求头。
- 远端返回 `101 Switching Protocols` 或 H2 extended CONNECT 的升级响应时，代理在返回客户端前修改响应头。
- Replay 模块作为独立 WebSocket 客户端发起握手时，也必须应用相同的请求/响应头规则。
- WebSocket 升级成功后的 frame 转发路径不改变 payload。

## 实现逻辑

- 普通 HTTP 代理入口识别 WebSocket upgrade 后会提前进入 `handle_http_websocket`，因此不能依赖普通 HTTP 请求/响应 transform。
- TLS intercept 入口识别 HTTP/1.1 upgrade 或 HTTP/2 extended CONNECT 后会进入 `handle_intercepted_websocket`，同样需要独立应用握手头规则。
- 新增 WebSocket 握手专用头部规则 helper，仅处理升级握手上的 header 类规则：
  - 请求侧：`delete_req_headers`、`req_headers`、`ua`、`referer`、`auth`、request `headerReplace`
  - 响应侧：`delete_res_headers`、`res_headers`、response `headerReplace`
- 不把普通 HTTP body/status/cache/attachment 等规则套到 WebSocket 101 握手，避免破坏协议升级。
- `Upgrade`、`Connection`、`Sec-WebSocket-Accept`、`Sec-WebSocket-Protocol`、`Sec-WebSocket-Extensions` 等协议协商关键头仍由现有 WebSocket 协商逻辑负责。
- Replay WebSocket 已有请求侧规则应用；本次补齐响应侧 WebSocket 握手头规则，并同步更新 Replay Traffic 记录中的原始/最终响应头。

## 依赖项

- 复用现有 `ResolvedRules`
- 复用现有普通 WS 和 TLS intercept WSS 握手构造函数
- 复用现有 WebSocket frame 转发、捕获和连接监控逻辑

## 测试方案

- 单元/集成测试：
  - `test_http_websocket_applies_request_and_response_header_rules`：普通 `ws://` 通过代理时，上游收到 `reqHeaders` 注入头，客户端收到 `resHeaders` 注入头。
  - `test_https_interception_websocket_applies_request_and_response_header_rules`：TLS intercept `wss://` 通过代理时，上游收到 `reqHeaders` 注入头，客户端收到 `resHeaders` 注入头。
  - `replay_websocket_response_rules_apply_headers_only`：Replay WebSocket 响应侧只应用握手头规则，不让 `statusCode` / body 规则破坏升级。
- E2E 测试：
  - 执行 `e2e-tests/tests/test_websocket_frames.sh`，加载 `e2e-tests/rules/websocket/header_rules.txt`，覆盖普通 WS 代理中 `reqHeaders` 到达 mock 远端、`resHeaders` 到达客户端的脚本 E2E。
  - 执行上述真实 TCP/TLS WebSocket 集成用例，补充验证普通代理和 TLS intercept 升级链路。
  - 执行 `e2e-tests/tests/test_replay_websocket_frames.sh`，覆盖 Replay WS `rule_config` 下的请求/响应头规则。
- 真实场景测试：
  - 更新并执行 `human_tests/proxy-websocket-sse.md` 的 `TC-PWS-09`。
  - 更新并执行 `human_tests/webui-replay.md` 的 Replay WS 规则头回归。

## Review/Fix/Test 闭环方案

- 第 1 轮：复查用户目标、普通 WS 分支、TLS intercept WSS 分支和新增测试，运行定向 WebSocket 头部规则测试。
- 第 2 轮：复查第 1 轮后的 diff、design、human_tests/readme 索引和相关普通 HTTP 头部注入对照测试，复跑受影响测试。

## 校验要求

- 先执行 WebSocket 头部规则定向测试。
- 再执行普通 HTTP 头部注入对照测试，确认未破坏非 WebSocket 路径。
- 收尾前按 rust-project-validate 技能执行格式、clippy、测试和 workspace all-features 兜底；如环境或耗时阻塞，必须记录风险。

## 文档更新要求

- 更新 `human_tests/proxy-websocket-sse.md`。
- 更新 `human_tests/webui-replay.md`。
- 更新 `human_tests/readme.md` 用例数量和说明。
- 本次不新增 CLI 参数或配置项，README 无需更新。
