# IM Gateway 真实场景测试用例

## 功能模块说明

IM Gateway 是 Bifrost 的顶级网关模块，支持飞书等 IM 平台的长连接事件接收、消息发送、事件路由（收到消息后执行脚本并回复）、定时任务等能力。作为 Settings 中的一级 Tab 和 CLI 顶级命令，独立于 Remote Invoke。

本测试文档覆盖 V1 阶段骨架代码的基本功能验证，重点验证：
- 服务启动不崩溃
- WebUI IM Gateway Tab 可见
- CLI `im` 命令及子命令可用
- API 端点正确响应
- Provider/Target/Route/Schedule CRUD 操作

## 前置条件

```bash
# 启动 Bifrost 测试实例
BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- start -p 8800 --unsafe-ssl --no-system-proxy
```

## 测试用例列表

### TC-IMG-01: Settings 一级 Tab 显示 IM Gateway

- **操作步骤**: 在浏览器中打开 `http://localhost:8800/_bifrost/`，导航到 Settings 页面
- **预期结果**: 侧边 Tab 中出现 `IM Gateway`，且是独立的一级 Tab，不是 Remote Invoke 子面板

### TC-IMG-02: CLI im help 输出

- **操作步骤**: 执行 `cargo run --bin bifrost -- im help`
- **预期结果**: 输出包含 `provider`、`target`、`send`、`route`、`schedule`、`history` 子命令说明

### TC-IMG-03: CLI im provider list（空列表）

- **操作步骤**: 执行 `cargo run --bin bifrost -- -p 8800 im provider list`
- **预期结果**: 输出 "No IM providers configured." 或空的 provider 列表

### TC-IMG-04: API GET /providers 返回空列表

- **操作步骤**: 执行 `curl -s http://127.0.0.1:8800/_bifrost/api/im-gateway/providers`
- **预期结果**: 返回 JSON 数组 `[]`

### TC-IMG-05: API POST /providers 创建 provider

- **操作步骤**: 执行：
  ```bash
  curl -s -X POST http://127.0.0.1:8800/_bifrost/api/im-gateway/providers \
    -H 'Content-Type: application/json' \
    -d '{"id":"test-feishu","provider_type":"feishu","app_id":"cli_test123","app_secret":"fake_secret_for_test","display_name":"Test Feishu"}'
  ```
- **预期结果**: 返回包含 `"id":"test-feishu"` 的 JSON 成功响应

### TC-IMG-06: API GET /providers 返回已创建的 provider

- **操作步骤**: 执行 `curl -s http://127.0.0.1:8800/_bifrost/api/im-gateway/providers`
- **预期结果**: 返回包含 `test-feishu` provider 的 JSON 数组，且 `app_secret` 字段不以明文形式返回

### TC-IMG-07: API GET /targets 返回空列表

- **操作步骤**: 执行 `curl -s http://127.0.0.1:8800/_bifrost/api/im-gateway/targets`
- **预期结果**: 返回 JSON 数组 `[]`

### TC-IMG-08: API POST /targets 创建 target

- **操作步骤**: 执行：
  ```bash
  curl -s -X POST http://127.0.0.1:8800/_bifrost/api/im-gateway/targets \
    -H 'Content-Type: application/json' \
    -d '{"id":"oncall","provider_id":"test-feishu","receive_id_type":"chat_id","receive_id":"oc_test123","display_name":"OnCall Group"}'
  ```
- **预期结果**: 返回包含 `"id":"oncall"` 的 JSON 对象

### TC-IMG-09: API GET /routes 返回空列表

- **操作步骤**: 执行 `curl -s http://127.0.0.1:8800/_bifrost/api/im-gateway/routes`
- **预期结果**: 返回 JSON 数组 `[]`

### TC-IMG-10: API GET /schedules 返回空列表

- **操作步骤**: 执行 `curl -s http://127.0.0.1:8800/_bifrost/api/im-gateway/schedules`
- **预期结果**: 返回 JSON 数组 `[]`

### TC-IMG-11: API GET /history/events 返回空列表

- **操作步骤**: 执行 `curl -s http://127.0.0.1:8800/_bifrost/api/im-gateway/history/events`
- **预期结果**: 返回 JSON 数组 `[]`

### TC-IMG-12: CLI im target list（空列表）

- **操作步骤**: 执行 `cargo run --bin bifrost -- -p 8800 im target list`
- **预期结果**: 输出 "No IM targets configured." 或包含已创建 target 的列表

### TC-IMG-13: CLI im provider list（显示已创建 provider）

- **操作步骤**: 执行 `cargo run --bin bifrost -- -p 8800 im provider list`
- **预期结果**: 输出包含 `test-feishu` 的列表，显示 TYPE、ENABLED、CONNECTION 等列

### TC-IMG-14: API DELETE /providers/:id 删除 provider

- **操作步骤**: 执行 `curl -s -X DELETE http://127.0.0.1:8800/_bifrost/api/im-gateway/providers/test-feishu`
- **预期结果**: 返回成功响应，后续 GET /providers 不再包含 `test-feishu`

### TC-IMG-15: WebUI IM Gateway Tab 内容渲染

- **操作步骤**: 在浏览器中打开 `http://localhost:8800/_bifrost/`，导航到 Settings → IM Gateway
- **预期结果**: 页面正确渲染 IM Gateway 管理界面，包含 Providers、Targets 等区域，无 JS 报错

## 清理步骤

```bash
# 停止 Bifrost 测试实例
# Ctrl+C 或 cargo run --bin bifrost -- stop -p 8800

# 清理临时数据
rm -rf ./.bifrost-test
```

---

## V2 阶段：实时消息收发与 Schedule 执行测试

### 前置条件（V2）

```bash
# 启动带真实飞书配置的 Bifrost 实例
BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- start -p 8800 --unsafe-ssl --no-system-proxy

# Provider 已创建：feishu-bot-1 (app_id=<YOUR_APP_ID>, owner_open_id=<YOUR_OPEN_ID>)
# Target 已创建：eden-user (open_id=<YOUR_OPEN_ID>)
```

### TC-IMG-16: owner_open_id 配置更新

- **操作步骤**: 执行
  ```bash
  curl -s -X PATCH http://127.0.0.1:8800/_bifrost/api/im-gateway/providers/feishu-bot-1 \
    -H 'Content-Type: application/json' \
    -d '{"owner_open_id":"<YOUR_OPEN_ID>"}'
  ```
- **预期结果**: 返回更新后的 provider JSON，包含 `owner_open_id` 字段

### TC-IMG-17: Outbound 消息发送并记录

- **操作步骤**: 执行
  ```bash
  curl -s -X POST http://127.0.0.1:8800/_bifrost/api/im-gateway/messages/send \
    -H 'Content-Type: application/json' \
    -d '{"target_id":"eden-user","msg_type":"text","content":"你好，Bifrost 助手上线了"}'
  ```
- **预期结果**: 返回 `message_id`，飞书上 owner 收到消息

### TC-IMG-18: 消息记录 API — 查看 outbound 记录

- **操作步骤**: 执行
  ```bash
  curl -s http://127.0.0.1:8800/_bifrost/api/im-gateway/providers/feishu-bot-1/messages?direction=outbound
  ```
- **预期结果**: 返回数组含 direction=outbound, status=success, content_preview 包含发送内容

### TC-IMG-19: WebSocket 长连接建立

- **操作步骤**: 执行 `curl -s -X POST http://127.0.0.1:8800/_bifrost/api/im-gateway/providers/feishu-bot-1/connect`
- **预期结果**: 返回 `{"success":true}`，日志显示 "feishu websocket connected"

### TC-IMG-20: Inbound 消息接收 + OK reaction

- **操作步骤**: 在飞书上给 bifrost_IM 机器人发送一条消息
- **预期结果**: 
  - 机器人在该消息上添加 OK 表情
  - 日志显示 "received inbound event from owner" 和 "added OK reaction to message"

### TC-IMG-21: owner 安全校验 — 非 owner 消息被拒绝

- **操作步骤**: 让非 owner 用户向机器人发消息（如果可能）
- **预期结果**: 日志显示 "rejecting message from non-owner user"，消息记录中 status=rejected

### TC-IMG-22: 消息记录 API — 查看 inbound 记录

- **操作步骤**: 执行
  ```bash
  curl -s http://127.0.0.1:8800/_bifrost/api/im-gateway/providers/feishu-bot-1/messages?direction=inbound
  ```
- **预期结果**: 返回数组含 direction=inbound, sender_open_id, reaction_added=true

### TC-IMG-23: Schedule 创建

- **操作步骤**: 执行
  ```bash
  curl -s -X POST http://127.0.0.1:8800/_bifrost/api/im-gateway/schedules \
    -H 'Content-Type: application/json' \
    -d '{"id":"test-schedule-1","name":"测试定时任务","target_id":"eden-user","enabled":true,"trigger":{"type":"interval","every_ms":60000},"script":{"script_text":"echo Hello from schedule at $(date)"},"timeout_ms":5000,"max_output_bytes":4096}'
  ```
- **预期结果**: 返回 `{"success":true}`

### TC-IMG-24: Schedule 手动执行并发送结果给 owner

- **操作步骤**: 执行 `curl -s -X POST http://127.0.0.1:8800/_bifrost/api/im-gateway/schedules/test-schedule-1/run`
- **预期结果**: 
  - 返回 `status: "Success"`, `exit_code: 0`
  - 飞书 owner 收到执行结果消息（包含 ✅ 和脚本输出）
  - 消息记录中新增一条 direction=outbound, trigger=schedule:test-schedule-1

### TC-IMG-25: CLI messages list 命令

- **操作步骤**: 执行 `cargo run --bin bifrost -- -p 8800 im messages list --provider feishu-bot-1`
- **预期结果**: 表格输出包含所有 inbound/outbound 消息，含 ID、方向、状态、内容预览、时间

### TC-IMG-26: CLI messages list 带 --direction 筛选

- **操作步骤**: 执行 `cargo run --bin bifrost -- -p 8800 im messages list --provider feishu-bot-1 --direction inbound`
- **预期结果**: 仅显示 inbound 方向的消息

### TC-IMG-27: CLI messages list 带 --source 筛选

- **操作步骤**: 执行 `cargo run --bin bifrost -- -p 8800 im messages list --provider feishu-bot-1 --source bot`
- **预期结果**: 仅显示 outbound（机器人发出的）消息

### TC-IMG-28: CLI messages clear 命令

- **操作步骤**: 执行 `cargo run --bin bifrost -- -p 8800 im messages clear feishu-bot-1`
- **预期结果**: 输出 "✓ Messages cleared"，再次 list 返回空

### TC-IMG-29: CLI schedule list 命令

- **操作步骤**: 执行 `cargo run --bin bifrost -- -p 8800 im schedule list`
- **预期结果**: 显示已创建的定时任务列表

### TC-IMG-30: CLI schedule logs 命令

- **操作步骤**: 执行 `cargo run --bin bifrost -- -p 8800 im schedule logs test-schedule-1`
- **预期结果**: 显示执行记录，含 run_id、status、duration、exit_code

### TC-IMG-31: Settings IM Gateway 左侧导航按 URL 切换独立面板

- **前置条件**:
  - 使用临时数据目录启动 Bifrost，端口不得使用 9900：
    ```bash
    BIFROST_DATA_DIR=./.bifrost-test-im-nav cargo run --bin bifrost -- start -p 18885 --unsafe-ssl --no-system-proxy
    ```
  - 浏览器打开 `http://127.0.0.1:18885/_bifrost/settings?tab=im-gateway`
- **操作步骤**:
  1. 确认 IM Gateway 页面左侧显示二级导航，包含 Connections、Targets、Routes、Schedules、History。
  2. 确认默认只渲染 `Connections` 面板，不再显示顶部二级 Tabs。
  3. 点击左侧导航中的 `Routes`。
  4. 确认右侧只渲染 `Routes` 面板，且 URL 包含 `imGatewaySection=routes`。
  5. 刷新页面，确认仍恢复到 `Routes` 面板。
  6. 点击左侧导航中的 `History`，确认右侧渲染 `History` 面板；History 面板内部仍保留 Events / Runs 小 Tabs。
  7. 切换到暗色主题后点击 `Targets`。
- **预期结果**:
  - 左侧二级导航固定在 IM Gateway 内容区左侧，不跟随右侧面板内容滚动。
  - 点击导航项后右侧独立渲染对应面板，不再把二级入口放在顶部 Tabs 中。
  - 当前导航项通过高亮和 `aria-current="true"` 标记。
  - URL 中的 `imGatewaySection` 能记录当前面板，页面刷新后恢复到同一面板。
  - 亮色与暗色主题下导航项、文本、边框和高亮状态均清晰可读。
- **执行记录（2026-05-05）**: PASS — `pnpm --dir web exec playwright test tests/ui/admin-settings.spec.ts --grep "Settings IM Gateway 左侧导航按 URL 切换独立面板"` 通过；验证默认仅渲染 Connections、无顶部 Connections 二级 Tab、点击 Routes/History/Targets 后只渲染对应面板、URL `imGatewaySection` 记录并刷新恢复、暗色主题下继续切换且 `aria-current` 正确。
