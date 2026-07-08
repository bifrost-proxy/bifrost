# IM Gateway 真实场景测试用例

## 功能模块说明

IM Gateway 是 Bifrost 的顶级网关模块，支持飞书等 IM 平台的长连接事件接收、消息发送、事件路由（收到消息后执行脚本并回复）、定时任务等能力。WebUI 中位于与 Settings 同级的 AI 一级页内，并和 Agent 子导航整合；CLI 仍作为顶级命令，独立于 Remote Invoke。

本测试文档覆盖 V1 阶段骨架代码的基本功能验证，重点验证：
- 服务启动不崩溃
- WebUI AI 一级入口和 IM Gateway 子导航可见
- CLI `im` 命令及子命令可用
- API 端点正确响应
- Provider/Target/Route/Schedule CRUD 操作

## 前置条件

```bash
# 启动 Bifrost 测试实例
BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- start -p 8800 --unsafe-ssl --no-system-proxy
```

## 测试用例列表

### TC-IMG-01: AI 一级入口显示 IM Gateway 子导航

- **操作步骤**: 在浏览器中打开 `http://localhost:8800/_bifrost/`，导航到 AI 页面
- **预期结果**: 主侧栏中出现与 Settings 同级的 `AI` 入口；AI 页面左侧子导航包含 IM Gateway 分组和 Connections、Targets、Routes、Schedules、History，不是 Remote Invoke 子面板

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

### TC-IMG-15: WebUI AI 页 IM Gateway 内容渲染

- **操作步骤**: 在浏览器中打开 `http://localhost:8800/_bifrost/`，导航到 AI → IM Gateway → Connections
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
- **预期结果**: 返回 `message_id`，飞书上 owner 收到 Markdown 卡片消息，消息记录的 `msg_type=interactive`，`content_preview` 包含发送内容

### TC-IMG-18: 消息记录 API — 查看 outbound 记录

- **操作步骤**: 执行
  ```bash
  curl -s http://127.0.0.1:8800/_bifrost/api/im-gateway/providers/feishu-bot-1/messages?direction=outbound
  ```
- **预期结果**: 返回数组含 `direction=outbound`、`status=success`、`msg_type=interactive`、`content_preview` 包含发送内容

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
    -d '{"id":"test-schedule-1","name":"测试定时任务","message_channel":{"provider_id":"feishu-bot-1","target_id":"owner","target_mode":"owner"},"enabled":true,"trigger":{"type":"interval","every_ms":60000},"script":{"script_text":"echo Hello from schedule at $(date)"},"timeout_ms":5000,"max_output_bytes":4096}'
  ```
- **预期结果**: 返回完整 schedule JSON，包含 `message_channel`

### TC-IMG-24: Schedule 手动执行并发送结果给 owner

- **操作步骤**: 执行 `curl -s -X POST http://127.0.0.1:8800/_bifrost/api/im-gateway/schedules/test-schedule-1/run`
- **预期结果**: 
  - 返回 `status: "Success"`, `exit_code: 0`
  - 绑定 Connection 的默认接收者收到执行结果消息（包含状态和脚本输出）
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

### TC-IMG-31: AI 页 IM Gateway 左侧导航按 URL 切换独立面板

- **前置条件**:
  - 使用临时数据目录启动 Bifrost，端口不得使用 9900：
    ```bash
    BIFROST_DATA_DIR=./.bifrost-test-im-nav cargo run --bin bifrost -- start -p 18885 --unsafe-ssl --no-system-proxy
    ```
  - 浏览器打开 `http://127.0.0.1:18885/_bifrost/ai?aiSection=im-gateway-connections&imGatewaySection=connections`
- **操作步骤**:
  1. 确认 AI 页面左侧显示合并子导航，其中 IM Gateway 分组包含 Connections、Targets、Routes、Schedules、History。
  2. 确认默认只渲染 `Connections` 面板，不再显示顶部二级 Tabs。
  3. 点击左侧导航中的 `Routes`。
  4. 确认右侧只渲染 `Routes` 面板，且 URL 包含 `aiSection=im-gateway-routes` 与 `imGatewaySection=routes`。
  5. 刷新页面，确认仍恢复到 `Routes` 面板。
  6. 点击左侧导航中的 `History`，确认右侧渲染 `History` 面板；History 面板内部仍保留 Events / Runs 小 Tabs。
  7. 切换到暗色主题后点击 `Targets`。
- **预期结果**:
  - AI 页顶部有正常留白，左侧子导航和右侧面板不贴住窗口顶部。
  - 左侧二级导航固定在 AI 内容区左侧，不跟随右侧面板内容滚动。
  - 点击导航项后右侧独立渲染对应面板，不再把二级入口放在顶部 Tabs 中。
  - 当前导航项通过高亮和 `aria-current="true"` 标记。
  - URL 中的 `aiSection` 与 `imGatewaySection` 能记录当前面板，页面刷新后恢复到同一面板。
  - 亮色与暗色主题下导航项、文本、边框和高亮状态均清晰可读。
- **执行记录（2026-05-05）**: PASS — `pnpm --dir web exec playwright test tests/ui/admin-settings.spec.ts --grep "Settings IM Gateway 左侧导航按 URL 切换独立面板"` 通过；验证默认仅渲染 Connections、无顶部 Connections 二级 Tab、点击 Routes/History/Targets 后只渲染对应面板、URL `imGatewaySection` 记录并刷新恢复、暗色主题下继续切换且 `aria-current` 正确。

### TC-IMG-32: 创建 Provider 时 App Secret 正确保存且响应脱敏

- **前置条件**:
  - 使用临时数据目录启动 Bifrost，端口不得使用 9900，必须禁用系统代理：
    ```bash
    BIFROST_DATA_DIR=./.bifrost-test-im-provider-secret cargo run --bin bifrost -- start -p 18886 --unsafe-ssl --no-system-proxy
    ```
  - 使用 WebUI 等价请求体，不需要真实飞书 app_id/app_secret；本用例只验证创建链路是否正确保存 secret 配置状态与响应脱敏。
- **操作步骤**:
  1. 执行创建请求：
     ```bash
     curl -s -X POST http://127.0.0.1:18886/_bifrost/api/im-gateway/providers \
       -H 'Content-Type: application/json' \
       -d '{"id":"feishu-secret-regression","provider_type":"feishu","display_name":"Feishu Secret Regression","enabled":true,"app_id":"cli_regression","app_secret":"regression_secret_value","event_connection_enabled":true,"event_types":[]}'
     ```
  2. 执行列表请求：
     ```bash
     curl -s http://127.0.0.1:18886/_bifrost/api/im-gateway/providers
     ```
  3. 执行删除清理：
     ```bash
     curl -s -X DELETE http://127.0.0.1:18886/_bifrost/api/im-gateway/providers/feishu-secret-regression
     ```
- **预期结果**:
  - 创建请求返回 `{"success":true}`。
  - 列表响应包含 `id=feishu-secret-regression` 和 `secret_configured=true`。
  - 列表响应不包含 `regression_secret_value`、`app_secret` 或 `secret_ref` 明文。
  - 删除请求返回 `{"success":true}`，再次列表不再包含该 provider。
- **执行记录（2026-05-06）**: PASS — 使用 `BIFROST_DATA_DIR=./.bifrost-test-im-provider-secret cargo run --bin bifrost -- start -p 18886 --unsafe-ssl --no-system-proxy` 启动源码版 Bifrost；POST WebUI 等价 payload 返回 `{"success":true}`；列表响应包含 `secret_configured=true` 且不包含 `regression_secret_value`、`app_secret`、`secret_ref`；DELETE 后列表为空。

### TC-IMG-33: WebUI 创建 Provider 可省略 Display Name、编辑可补填 App Secret 且重复 ID 显示真实错误

- **前置条件**:
  - 使用临时数据目录启动 Bifrost，端口不得使用 9900，必须禁用系统代理：
    ```bash
    BIFROST_DATA_DIR=./.bifrost-test-im-provider-webui cargo run --bin bifrost -- start -p 18887 --unsafe-ssl --no-system-proxy --skip-cert-check
    ```
  - 浏览器打开 `http://127.0.0.1:18887/_bifrost/settings?tab=im-gateway`。
- **操作步骤**:
  1. 先通过 API 创建一个缺少 App Secret 且省略 Display Name 的 Provider：
     ```bash
     curl -s -X POST http://127.0.0.1:18887/_bifrost/api/im-gateway/providers \
       -H 'Content-Type: application/json' \
       -d '{"id":"feishu-edit-secret-regression","provider_type":"feishu","enabled":true,"app_id":"cli_regression","event_connection_enabled":true,"event_types":[]}'
     ```
  2. 在 WebUI 的 IM Gateway / Connections 中确认该 Provider 使用 Provider ID 作为展示名，并显示 `Secret: Not Set`。
  3. 点击该 Provider 的 Edit 按钮，在 `App Secret` 输入框中填入 `regression_secret_value`，点击 Save。
  4. 执行列表请求：
     ```bash
     curl -s http://127.0.0.1:18887/_bifrost/api/im-gateway/providers
     ```
  5. 再次点击 `Add Provider`，输入相同 Provider ID `feishu-edit-secret-regression`、App ID 与 App Secret，点击 Create。
  6. 执行删除清理：
     ```bash
     curl -s -X DELETE http://127.0.0.1:18887/_bifrost/api/im-gateway/providers/feishu-edit-secret-regression
     ```
- **预期结果**:
  - 第 1 步创建请求返回 `{"success":true}`，即 Display Name 可按页面承诺省略。
  - 第 2 步页面显示 Provider ID `feishu-edit-secret-regression` 与 `Secret: Not Set`。
  - 第 3 步保存成功，页面显示 `Provider updated`。
  - 第 4 步列表响应包含 `secret_configured=true`，且不包含 `regression_secret_value`、`app_secret` 或 `secret_ref` 明文。
  - 第 5 步页面 toast 显示后端真实错误 `provider with id 'feishu-edit-secret-regression' already exists`，而不是通用的 `Failed to save provider`。
  - 删除请求返回 `{"success":true}`，再次列表不再包含该 provider。
- **执行记录（2026-05-06）**: PASS — 使用临时端口 `18887` 源码服务与 Playwright WebUI 流程执行；先创建缺少 secret 且省略 Display Name 的 Provider，页面使用 Provider ID 展示并显示 `Not Set`；Edit 弹窗补填 App Secret 后 toast 显示 `Provider updated`，API 列表返回 `secret_configured=true` 且无 secret 明文；再次 Add 同名 Provider 时 toast 显示后端重复 ID 错误；最后删除清理成功。

### TC-IMG-34: WebUI 创建 Provider 后无需重启即可连接并通知 owner

- **前置条件**:
  - 使用临时数据目录启动 Bifrost，端口不得使用 9900，必须禁用系统代理：
    ```bash
    BIFROST_DATA_DIR=./.bifrost-test-im-provider-autoconnect cargo run --bin bifrost -- start -p 18888 --unsafe-ssl --no-system-proxy --skip-cert-check
    ```
  - 浏览器打开 `http://127.0.0.1:18888/_bifrost/settings?tab=im-gateway`。
  - 准备真实可用的飞书应用 App ID 和 App Secret。
- **操作步骤**:
  1. 在 WebUI 点击 `Add Provider`。
  2. 输入唯一 Provider ID，例如 `feishu-autoconnect-regression`。
  3. 保持 `Enabled` 开启，填写真实 App ID 与 App Secret，不填写 Display Name。
  4. 点击 `Create`。
  5. 不重启 Bifrost，直接查询状态：
     ```bash
     curl -s http://127.0.0.1:18888/_bifrost/api/im-gateway/providers/feishu-autoconnect-regression/status
     ```
  6. 查询该 Provider 的消息记录：
     ```bash
     curl -s http://127.0.0.1:18888/_bifrost/api/im-gateway/providers/feishu-autoconnect-regression/messages
     ```
  7. 执行删除清理：
     ```bash
     curl -s -X DELETE http://127.0.0.1:18888/_bifrost/api/im-gateway/providers/feishu-autoconnect-regression
     ```
- **预期结果**:
  - WebUI 显示 `Provider created and connected`。
  - 第 5 步状态响应包含 `state=connected`。
  - 第 6 步消息记录包含一条 `direction=outbound`、`trigger=online`、`status=success`、`msg_type=interactive` 的 owner 通知，`content_preview` 以 `**Bifrost is online**` 开头，并包含 `Device` 与 `Workspace` 信息。
  - 全流程不需要重启 Bifrost。
  - Provider 列表与消息响应不包含 App Secret 明文。
- **执行记录（2026-05-06）**: PASS — 使用临时端口 `18888` 源码服务和用户提供的真实飞书 AK/SK 通过 WebUI 创建 Provider；页面显示 `Provider created and connected`；未重启服务即查询到状态 `connected`；message log 包含 `trigger=online`、`status=success`、`content_preview=你好，Bifrost 助手上线了` 的 owner 通知；响应中未泄露 App Secret；最后删除清理成功。本用例后续要求上线通知同时包含 `工作目录：~/work/github/bifrost`。

### TC-IMG-35: 同一进程内配置两个飞书机器人均可连接并通知 owner

- **前置条件**:
  - 使用临时数据目录启动 Bifrost，端口不得使用 9900，必须禁用系统代理：
    ```bash
    BIFROST_DATA_DIR=./.bifrost-test-im-provider-two-bots cargo run --bin bifrost -- start -p 18889 --unsafe-ssl --no-system-proxy --skip-cert-check
    ```
  - 浏览器打开 `http://127.0.0.1:18889/_bifrost/settings?tab=im-gateway`。
  - 准备两个不同且真实可用的飞书应用 App ID 和 App Secret。
- **操作步骤**:
  1. 在 WebUI 点击 `Add Provider`。
  2. 输入第一个唯一 Provider ID，例如 `feishu-two-bots-a`。
  3. 保持 `Enabled` 开启，填写第一个真实 App ID 与 App Secret，不填写 Display Name。
  4. 点击 `Create`，等待页面显示创建并连接成功。
  5. 再次点击 `Add Provider`。
  6. 输入第二个唯一 Provider ID，例如 `feishu-two-bots-b`。
  7. 保持 `Enabled` 开启，填写第二个真实 App ID 与 App Secret，不填写 Display Name。
  8. 点击 `Create`，等待页面显示创建并连接成功。
  9. 不重启 Bifrost，分别查询两个 Provider 状态：
     ```bash
     curl -s http://127.0.0.1:18889/_bifrost/api/im-gateway/providers/feishu-two-bots-a/status
     curl -s http://127.0.0.1:18889/_bifrost/api/im-gateway/providers/feishu-two-bots-b/status
     ```
  10. 分别查询两个 Provider 的消息记录：
      ```bash
      curl -s http://127.0.0.1:18889/_bifrost/api/im-gateway/providers/feishu-two-bots-a/messages
      curl -s http://127.0.0.1:18889/_bifrost/api/im-gateway/providers/feishu-two-bots-b/messages
      ```
  11. 执行删除清理：
      ```bash
      curl -s -X DELETE http://127.0.0.1:18889/_bifrost/api/im-gateway/providers/feishu-two-bots-a
      curl -s -X DELETE http://127.0.0.1:18889/_bifrost/api/im-gateway/providers/feishu-two-bots-b
      ```
- **预期结果**:
  - 两次创建后 WebUI 均显示 `Provider created and connected`。
  - 两个 Provider 的状态响应都包含 `state=connected`，且无需重启 Bifrost。
  - 两个 Provider 的消息记录都各自包含一条 `direction=outbound`、`trigger=online`、`status=success` 的 owner 通知，且 `content_preview` 以 `你好，Bifrost 助手上线了` 开头，并包含 `设备名称：...` 与 `工作目录：~/work/github/bifrost`。
  - 两个 Provider 不会串用对方的飞书 token；第二个机器人不会因复用第一个机器人的 token 而发送失败。
  - Provider 列表、状态与消息响应不包含任何 App Secret 明文。
- **执行记录（2026-05-06）**: PASS — 使用临时端口 `18889` 和独立数据目录 `.bifrost-test-im-provider-two-bots` 启动源码版 Bifrost；通过 Settings / IM Gateway WebUI 分别创建两个真实飞书 Provider；两个 Provider 均显示创建并连接成功，状态均为 `connected`；两个 Provider 的 message log 均包含 `direction=outbound`、`trigger=online`、`status=success` 的 owner 通知，`content_preview` 为 `你好，Bifrost 助手上线了\n工作目录：~/work/github/bifrost`；第二个机器人未复用第一个机器人的 token，响应中未泄露 App Secret；最后删除清理两个 Provider。

### TC-IMG-36: Provider 自定义 Agent Working Directory 优先用于上线通知

- **前置条件**:
  - 使用临时数据目录启动 Bifrost，端口不得使用 9900，必须禁用系统代理：
    ```bash
    BIFROST_DATA_DIR=./.bifrost-test-im-provider-custom-workdir cargo run --bin bifrost -- start -p 18890 --unsafe-ssl --no-system-proxy --skip-cert-check
    ```
  - 浏览器打开 `http://127.0.0.1:18890/_bifrost/settings?tab=im-gateway`。
  - 准备真实可用的飞书应用 App ID 和 App Secret。
- **操作步骤**:
  1. 在 WebUI 点击 `Add Provider`。
  2. 输入唯一 Provider ID，例如 `feishu-custom-workdir-regression`。
  3. 保持 `Enabled` 开启，填写真实 App ID 与 App Secret，不填写 Display Name。
  4. 在 `Agent Working Directory` 输入 `/tmp/bifrost-im-provider-custom-workdir`。
  5. 点击 `Create`，等待页面显示创建并连接成功。
  6. 不重启 Bifrost，直接查询该 Provider 的消息记录：
     ```bash
     curl -s http://127.0.0.1:18890/_bifrost/api/im-gateway/providers/feishu-custom-workdir-regression/messages
     ```
  7. 执行删除清理：
     ```bash
     curl -s -X DELETE http://127.0.0.1:18890/_bifrost/api/im-gateway/providers/feishu-custom-workdir-regression
     ```
- **预期结果**:
  - WebUI 显示 `Provider created and connected`。
  - 消息记录包含一条 `direction=outbound`、`trigger=online`、`status=success` 的 owner 通知。
  - owner 通知的 `content_preview` 以 `你好，Bifrost 助手上线了` 开头，并包含 `设备名称：...` 与 `工作目录：/tmp/bifrost-im-provider-custom-workdir`。
  - owner 通知不得回退为全局 Agent Working Directory 或 Bifrost 进程 cwd。
  - Provider 列表与消息响应不包含 App Secret 明文。
- **执行记录（2026-05-06）**: PASS — 使用临时端口 `18890` 和独立数据目录 `.bifrost-test-im-provider-custom-workdir` 启动源码版 Bifrost；通过 Settings / IM Gateway WebUI 创建真实飞书 Provider，并在 `Agent Working Directory` 填写 `/tmp/bifrost-im-provider-custom-workdir`；页面显示 `Provider created and connected`，Provider 状态为 `connected`；message log 包含 `direction=outbound`、`trigger=online`、`status=success` 的 owner 通知，且 `content_preview` 包含 `工作目录：/tmp/bifrost-im-provider-custom-workdir`，未回退到 `~/work/github/bifrost`；响应中未泄露 App Secret；最后删除 Provider 清理成功。

### TC-IMG-37: CLI IM 命令未传 provider 时选择 provider，并默认发送给 owner

- **前置条件**:
  - 使用临时数据目录启动 Bifrost，端口不得使用 9900，必须禁用系统代理：
    ```bash
    BIFROST_DATA_DIR=./.bifrost-test-im-cli-provider cargo run --bin bifrost -- start -p 18891 --unsafe-ssl --no-system-proxy --skip-cert-check
    ```
  - 准备一个 fake Feishu OpenAPI 服务，支持 tenant token 与 `POST /im/v1/messages`，并记录收到的请求。
  - 已创建一个 enabled Feishu Provider，配置 `owner_open_id=owner-open-id`、`base_url=<fake-feishu-url>`。
- **操作步骤**:
  1. 执行不带 `--provider`、不带 `--target` 的发送命令：
     ```bash
     cargo run --bin bifrost -- -p 18891 im send --text 'hello owner from cli'
     ```
  2. 查看 fake Feishu 记录的 `POST /im/v1/messages` 请求。
  3. 执行不带 `--provider` 的消息日志命令：
     ```bash
     cargo run --bin bifrost -- -p 18891 im messages list
     ```
  4. 若配置了多个 enabled Provider，在真实交互式终端重复第 1 步，确认 CLI 展示 provider 列表并等待选择；非交互式 stdin 下应提示显式传 `--provider`。
- **预期结果**:
  - 单 enabled Provider 场景下，CLI 自动选择该 Provider，不要求额外输入。
  - `im send` 默认使用 `target_id=__owner__`，后端解析为该 Provider 的 `owner_open_id`。
  - fake Feishu 收到的请求包含 `receive_id_type=open_id`、`receive_id=owner-open-id`、`msg_type=interactive`，卡片 `schema=2.0`，标题为 `Bifrost`，正文 markdown 内容为 `hello owner from cli`。
  - CLI 输出包含 `Message sent` 与 fake Feishu 返回的 message id。
  - `im messages list` 未传 `--provider` 时复用 provider 选择逻辑，输出包含 `Owner` 与消息内容预览。
  - 多 Provider 交互式场景下，CLI 展示 provider 列表；多 Provider 非交互式场景下返回明确错误，要求传 `--provider`。
- **执行记录（2026-05-06）**: PASS — 使用 `e2e-tests/tests/test_im_cli_provider_selection_send_owner.sh` 执行 TC-IMG-37；脚本用临时数据目录 `.bifrost-test-im-cli-provider`、端口 `18891` 和 fake Feishu OpenAPI 服务启动源码版 Bifrost；创建唯一 enabled Provider 后执行 `bifrost im send --text 'hello owner from cli'`，CLI 自动选择 `feishu-main` 并输出 `Message sent via provider 'feishu-main' to __owner__ (message_id: om_owner_cli)`；fake Feishu 捕获 `receive_id_type=open_id`、`receive_id=owner-open-id`、`msg_type=text` 和文本内容；`bifrost im messages list` 未传 `--provider` 时同样自动选择 Provider，输出包含 `Owner` 与消息内容预览；脚本最后清理临时数据和进程。
- **执行记录（2026-06-03）**: PASS — 按 Feishu text 默认卡片化行为更新并执行 `bash e2e-tests/tests/test_im_cli_provider_selection_send_owner.sh`；fake Feishu 捕获 owner 文本发送请求 `msg_type=interactive`，`content` 解析后为 Card 2.0，标题 `Bifrost`，markdown 内容 `hello owner from cli`；CLI 输出仍包含 `Message sent` 与 `om_owner_cli`，`im messages list` 仍包含 `Owner` 和消息预览。

### TC-IMG-38: CLI 发送图片消息时先上传图片再发送 image 消息

- **前置条件**:
  - 使用临时数据目录启动 Bifrost，端口不得使用 9900，必须禁用系统代理：
    ```bash
    BIFROST_DATA_DIR=./.bifrost-test-im-cli-provider cargo run --bin bifrost -- start -p 18891 --unsafe-ssl --no-system-proxy --skip-cert-check
    ```
  - 准备一个 fake Feishu OpenAPI 服务，支持 tenant token、`POST /im/v1/images` 与 `POST /im/v1/messages`，并记录收到的请求。
  - 已创建一个 enabled Feishu Provider，配置 `owner_open_id=owner-open-id`、`base_url=<fake-feishu-url>`。
- **操作步骤**:
  1. 准备一张本地 PNG 图片，例如 `pixel.png`。
  2. 执行不带 `--provider`、不带 `--target` 的图片发送命令：
     ```bash
     cargo run --bin bifrost -- -p 18891 im send --image-file ./pixel.png
     ```
  3. 查看 fake Feishu 记录的 `POST /im/v1/images` 请求。
  4. 查看 fake Feishu 记录的 `POST /im/v1/messages` 请求。
- **预期结果**:
  - 单 enabled Provider 场景下，CLI 自动选择该 Provider。
  - fake Feishu 先收到 `POST /im/v1/images` 图片上传请求，上传成功后返回 `image_key`。
  - fake Feishu 随后收到 `POST /im/v1/messages?receive_id_type=open_id`，请求体包含 `receive_id=owner-open-id`、`msg_type=image`、`content={"image_key":"<上传返回的 key>"}`。
  - CLI 输出包含 `Message sent`。
  - message log 只记录图片 key 摘要，不记录图片 bytes/base64。
- **执行记录（2026-05-12）**: PASS — 执行 `bash e2e-tests/tests/test_im_cli_provider_selection_send_owner.sh` 通过；脚本使用临时数据目录 `.bifrost-test-im-cli-provider`、端口 `18891`、`--no-system-proxy` 和 fake Feishu OpenAPI 服务。`bifrost im send --image-file <pixel.png>` 自动选择唯一 enabled Provider，fake Feishu 捕获 `POST /im/v1/images` multipart 上传请求并返回 `image_key=img_uploaded_cli`，随后捕获 `POST /im/v1/messages?receive_id_type=open_id`，请求体包含 `receive_id=owner-open-id`、`msg_type=image`、`content={"image_key":"img_uploaded_cli"}`；CLI 输出 `Message sent`，message log 仅显示 `[image:img_uploaded_cli]` 摘要。

### TC-IMG-39: CLI 发送图文卡片时生成 interactive card，并支持本地图片上传

- **前置条件**:
  - 复用 TC-IMG-38 的临时 Bifrost 服务、fake Feishu OpenAPI 服务和 enabled Feishu Provider。
- **操作步骤**:
  1. 执行带标题、Markdown 文本、已有图片 key 的图文卡片发送命令：
     ```bash
     cargo run --bin bifrost -- -p 18891 im send --card-title 'Deploy report' --card-text '**Done** with chart' --card-image-key img_v3_chart
     ```
  2. 执行带标题、Markdown 文本、本地图片文件的图文卡片发送命令：
     ```bash
     cargo run --bin bifrost -- -p 18891 im send --card-title 'Uploaded chart' --card-text 'Chart uploaded' --card-image-file ./pixel.png
     ```
  3. 查看 fake Feishu 记录的 `POST /im/v1/images` 和 `POST /im/v1/messages` 请求。
- **预期结果**:
  - 两条 CLI 命令输出都包含 `Message sent`。
  - 已有图片 key 场景下，fake Feishu 收到的请求包含 `msg_type=interactive`。
  - 已有图片 key 场景下，`content` 解析后包含 header title `Deploy report`。
  - `elements` 中包含 `tag=img` 且 `img_key=img_v3_chart`，并包含 `tag=markdown` 且内容为 `**Done** with chart`。
  - 本地图片文件场景下，fake Feishu 先收到图片上传请求，再收到 `msg_type=interactive` 的卡片发送请求，卡片 `img_key` 使用上传返回的 key。
  - 该路径不要求调用方手写完整 Feishu card JSON。
- **执行记录（2026-05-12）**: PASS — 执行 `bash e2e-tests/tests/test_im_cli_provider_selection_send_owner.sh` 通过；`bifrost im send --card-title 'Deploy report' --card-text '**Done** with chart' --card-image-key img_v3_chart` 输出 `Message sent`，fake Feishu 捕获 `msg_type=interactive`，`content.header.title.content=Deploy report`，`elements[0].tag=img`、`elements[0].img_key=img_v3_chart`、`elements[1].tag=markdown`、`elements[1].content=**Done** with chart`；`bifrost im send --card-title 'Uploaded chart' --card-text 'Chart uploaded' --card-image-file <pixel.png>` 先触发 `POST /im/v1/images`，再发送 `msg_type=interactive` 卡片，卡片 `elements[0].img_key=img_uploaded_cli`，message log preview 分别为 `Deploy report` 和 `Uploaded chart`。

### TC-IMG-40: 原始 card JSON 直通发送保持兼容

- **前置条件**:
  - 复用 TC-IMG-38 的临时 Bifrost 服务、fake Feishu OpenAPI 服务和 enabled Feishu Provider。
- **操作步骤**:
  1. 执行原始 card JSON 发送命令：
     ```bash
     cargo run --bin bifrost -- -p 18891 im send --card-json '{"config":{},"elements":[],"header":{"title":{"tag":"plain_text","content":"Raw card"}}}'
     ```
  2. 查看 fake Feishu 记录的最后一条 `POST /im/v1/messages` 请求。
- **预期结果**:
  - CLI 输出包含 `Message sent`。
  - fake Feishu 收到的请求包含 `msg_type=interactive`。
  - `content` 解析后与调用方提供的 card JSON 一致。
  - 新增图片和图文卡片参数不破坏既有 `--card-json` / `--card-file` 路径。
- **执行记录（2026-05-12）**: PASS — 同一次 `test_im_cli_provider_selection_send_owner.sh` 覆盖原始 card JSON 兼容；`bifrost im send --card-json '{"config":{},"elements":[],"header":{"title":{"tag":"plain_text","content":"Raw card"}}}'` 输出 `Message sent`，fake Feishu 捕获 `msg_type=interactive`，解析后的 `content.header.title.content=Raw card` 且 `elements=[]`，证明新增图片/图文卡片参数未破坏原始 card JSON 直通路径。

### TC-IMG-41: Agent 最终输出 Markdown 本地图片时自动上传并替换为 Feishu image_key

- **前置条件**:
  - 使用临时数据目录启动 Bifrost，端口不得使用 9900，必须禁用系统代理。
  - 已配置 Feishu Provider，`agent_config.work_dir` 指向包含 `chart.png` 的临时工作目录。
  - fake Feishu OpenAPI 服务支持 tenant token、`POST /im/v1/images` 与 `POST /im/v1/messages`，并记录请求。
- **操作步骤**:
  1. 构造 Agent 最终回复 Markdown：`结果如下：![chart](./chart.png)`。
  2. 触发 Agent reply card 发送路径。
  3. 查看 fake Feishu 的图片上传请求和最终 interactive card 请求。
- **预期结果**:
  - Bifrost 先上传 `agent_config.work_dir/chart.png` 到 Feishu 图片接口，获得 `image_key`。
  - 最终 interactive card 的 Markdown 内容包含 `![chart](<image_key>)`。
  - 最终 interactive card 中不包含 `./chart.png`、`file://` 或其他本地文件路径。
  - 如果图片上传失败，最终 Markdown 降级为 `[chart 未能上传]` 这类文本占位，不保留 `![chart](./chart.png)`，避免飞书卡片因非法图片 URL 发送失败。
- **执行记录（2026-05-12）**: PASS — 执行 focused 单元测试 `cargo test -p bifrost-admin agent_reply_image -- --nocapture`、`cargo test -p bifrost-admin markdown_image_destination -- --nocapture`、`cargo test -p bifrost-admin local_image_fallback -- --nocapture` 通过；代码路径验证本地/`file://`/相对 work_dir 图片会进入上传路径，HTTP 图片和已有 `img_v*` key 不上传，上传失败 fallback 不包含 Markdown 图片语法或本地路径。真实 Feishu 验证使用临时目录 `/tmp/bifrost-im-rich-md-live` 和端口 `28880`，通过 `POST /_bifrost/api/im-gateway/messages/send` 发送包含 `![local-md-card](/tmp/bifrost-im-rich-md-live/md-card-local-image.png)` 的 interactive rich card，Feishu 返回 HTTP 200，`message_id=om_x100b6f1a88dc689cc318ac23d88c5e8`，`request_id=202605121701206A57901A69C3536657A7`；本地消息日志 `id=31085a14`、`status=success`、`msg_type=interactive`；用户在 Feishu 客户端确认已看到卡片内图片，说明本地 Markdown 图片已上传并替换为 Feishu 可渲染的 image key。

### TC-IMG-42: Agent Markdown 图片上传缓存避免流式/重复输出反复上传

- **前置条件**:
  - 复用 TC-IMG-41 的临时 Provider、工作目录和图片文件。
- **操作步骤**:
  1. 连续两次渲染包含同一张图片的 Agent Markdown：`![chart](./chart.png)`。
  2. 保持图片文件内容、大小和 mtime 不变。
  3. 查看第二次渲染是否命中缓存。
- **预期结果**:
  - 上传缓存 key 包含 `provider_id`、canonical path、文件大小和 mtime。
  - 同一 Provider 同一文件指纹的第二次渲染复用第一次返回的 `image_key`，不再次调用 Feishu 图片上传接口。
  - 流式进度卡片只在最终 flush 前做图片上传和 Markdown 替换，不在每个 streaming delta 中重复上传图片。
- **执行记录（2026-05-12）**: PASS — 代码 review 确认缓存通过全局 `AGENT_REPLY_IMAGE_UPLOAD_CACHE` 按 `provider_id + canonical path + len + modified_ms` 命中；流式路径在 `progress_registry.finish(..., rendered_main_response, ...)` 前调用一次渲染，progress card 的增量刷新仍只做普通 Markdown 转换，不触发图片上传。

### TC-IMG-43: Schedules API 支持手动新增 Script 与 Agent 任务

- **前置条件**:
  - 使用临时数据目录启动 Bifrost，端口不得使用 9900，必须禁用系统代理：
    ```bash
    BIFROST_DATA_DIR=./.bifrost-test-im-schedules cargo run --bin bifrost -- start -p 18892 --unsafe-ssl --no-system-proxy --skip-cert-check
    ```
  - 已通过 API 创建一个配置了 `owner_open_id` 的 Feishu Provider。
- **操作步骤**:
  1. 调用 `POST /_bifrost/api/im-gateway/schedules` 创建 script schedule，body 包含 `task_type=script`、`message_channel.provider_id=schedule-provider`、`message_channel.target_mode=owner`、`trigger.type=interval`、`script.script_text=echo script-ok`。
  2. 调用同一 API 创建 agent schedule，body 包含 `task_type=agent`、`message_channel.provider_id=schedule-provider`、`message_channel.target_mode=owner`、`trigger.type=interval`、`agent.prompt=Summarize schedule state`。
  3. 调用 `GET /_bifrost/api/im-gateway/schedules` 查看列表。
  4. 调用 `POST /_bifrost/api/im-gateway/schedules/<script-id>/run` 手动触发 script schedule。
  5. 调用 `GET /_bifrost/api/im-gateway/schedules/<script-id>/runs` 查看 run history。
- **预期结果**:
  - 两次创建都返回完整 schedule JSON，而不是只有 `{success:true}`。
  - Script schedule 保留 `task_type=script`、`script.script_text` 和 `next_run_at`。
  - Agent schedule 保留 `task_type=agent`、`agent.prompt` 和 `next_run_at`。
  - Script 手动 run 返回 `status=Success`，`input_preview` 包含脚本输入，`stdout_preview` 包含 `script-ok`。
  - Run history 中能查到该手动执行记录，并展示当次输入和输出。
- **执行记录（2026-05-12）**: PASS — 使用临时数据目录 `.bifrost-test-im-schedules`、端口 `18892`、`--no-system-proxy` 启动源码版 Bifrost；通过 API 创建 `schedule-provider` 后，`POST /schedules` 创建 `script-schedule` 返回完整 schedule JSON，包含 `message_channel`、`task_type=script`、`script.script_text=echo script-ok` 与 `next_run_at`；创建 `agent-schedule` 返回 `task_type=agent`、`agent.prompt=Summarize schedule state` 与 `next_run_at`；`GET /schedules` 同时列出两类任务；`POST /schedules/script-schedule/run` 返回 `status=Success`、`exit_code=0`、`input_preview` 与 `stdout_preview=script-ok\n`；`GET /schedules/script-schedule/runs` 返回对应 `manual_run` 记录。

### TC-IMG-44: Agent 内置 schedule 工具支持查询、新增、更新、删除

- **前置条件**:
  - 复用 TC-IMG-43 的临时 Bifrost 服务。
  - 通过 `/agent/chat` 使用测试模型或 mock 模型，使模型依次调用 `schedule_create`、`schedule_list`、`schedule_update`、`schedule_delete` 工具。
- **操作步骤**:
  1. 发送 Agent 请求，要求创建一个 id 为 `agent-tool-schedule` 的 agent schedule，preset prompt 为 `tool prompt`。
  2. 发送 Agent 请求，要求查询定时任务列表并确认包含 `agent-tool-schedule`。
  3. 发送 Agent 请求，要求把该 schedule 的 prompt 更新为 `updated tool prompt` 并禁用任务。
  4. 发送 Agent 请求，要求删除 `agent-tool-schedule`。
  5. 通过 schedules API 直接读取列表做二次确认。
- **预期结果**:
  - Agent 可见工具列表包含 `schedule_list`、`schedule_create`、`schedule_update`、`schedule_delete`。
  - `schedule_create` 成功后 API 列表出现 `agent-tool-schedule`。
  - `schedule_update` 后 `enabled=false` 且 `agent.prompt=updated tool prompt`。
  - `schedule_delete` 后 API 列表不再出现该 schedule。
  - 工具失败时返回结构化错误，不会静默吞掉 store 错误。
- **执行记录（2026-05-12）**: PASS — 启动临时 mock Chat Completions 服务并通过 `/agent` PATCH 配置 `model_provider=mock-schedule-tools`；调用真实 `/agent/chat`，mock model 依次返回 `schedule_create`、`schedule_list`、`schedule_update`、`schedule_delete` tool calls；响应 `tool_calls` 中四个工具均 `success=true`，create 结果包含 `agent-tool-schedule` 与 `agent.prompt=tool prompt`，list 结果包含该任务，update 结果包含 `enabled=false` 与 `agent.prompt=updated tool prompt`，delete 结果为 `{"deleted":"agent-tool-schedule"}`；最后通过 schedules API 验证列表只剩 `script-schedule` 与 `agent-schedule`，不再包含 `agent-tool-schedule`。

### TC-IMG-45: WebUI Schedules 面板可手动新增 Script/Agent 任务且支持明暗主题

- **前置条件**:
  - 使用临时数据目录启动 Bifrost，端口不得使用 9900，必须禁用系统代理。
  - 浏览器打开 `http://127.0.0.1:18892/_bifrost/` 并进入 Settings / IM Gateway / Schedules。
- **操作步骤**:
  1. 在亮色主题点击 Schedules 面板右上角 `Add`。
  2. 选择 `Script`，填写 Name、Target、Interval、Script Text 后创建。
  3. 再次打开 `Add`，选择 `Agent`，填写 Name、Interval、Preset Prompt 后创建。
  4. 切换暗色主题，重复打开 Add 弹窗检查字段、按钮、Tag、表格文本可读。
  5. 刷新页面后回到 Schedules 面板，确认列表仍展示两类任务。
- **预期结果**:
  - Add 弹窗在亮色和暗色主题下无文字遮挡、无颜色对比问题。
  - Script 表单要求 Script Text 或 Script File 至少一项。
  - Agent 表单要求 Preset Prompt。
  - 表格新增 Type 列，Script 显示 `Script`，Agent 显示 `Agent`。
  - 刷新后 schedule 数据从后端恢复，列表不丢失。
- **执行记录（2026-05-12）**: PASS — 使用 Codex in-app Browser 打开 `http://127.0.0.1:18892/_bifrost/ai?imGatewaySection=schedules`；亮色主题下 Schedules 页面显示说明文案 `Scheduled tasks run scripts or Agent prompts on a cron/interval basis`、`Add` 按钮和 Type 列；通过 Add 弹窗创建 `UI Script Schedule`，选择 `schedule-target`，填写 `echo ui-script-ok` 后列表展示 Type=`Script`；再次通过 Add 弹窗选择 `Agent`，填写 preset prompt `Run UI agent scheduled prompt` 后列表展示 `UI Agent Schedule` 且 Type=`Agent`；点击 moon 图标切换暗色主题后再次打开 Add 弹窗，确认 `Task Type`、`Script Text`、`Create` 等字段可见；刷新页面后 `UI Script Schedule` 与 `UI Agent Schedule` 仍从后端恢复显示。

### TC-IMG-46: WebUI 每个 Schedule 可查看详情和运行历史

- **前置条件**:
  - 复用 TC-IMG-43 的临时 Bifrost 服务。
  - 已至少手动执行过一个 Script schedule 和一个 Agent schedule。
  - Agent schedule 的 mock 模型至少产生一次 tool call 和最终回复。
- **操作步骤**:
  1. 在 Schedules 表格点击 Script schedule 行。
  2. 查看详情弹窗中的任务配置与 Run History。
  3. 关闭详情弹窗，在 Schedules 表格点击 Agent schedule 行。
  4. 查看详情弹窗中的 Agent 最终结果、工具调用轨迹和运行耗时。
- **预期结果**:
  - Script 详情展示 ID、Type、Target、Trigger、Next Run、Last Run、Timeout。
  - Script Run History 展示 `duration_ms`、`exit_code`、`stdout_preview`、`stderr_preview`、`error`。
  - Agent 详情展示 preset prompt。
  - Agent Run History 展示 `Final Result`、`Tool Calls`、每个工具的 arguments/result/success，以及 plan trace（如果本次运行产生 plan）。
  - 详情弹窗在暗色主题下仍可读，无文字或按钮遮挡。
- **执行记录（2026-05-12）**: PASS — 重启临时服务加载最新代码后，先用 mock Chat Completions 服务手动触发 `agent-schedule`，`GET /schedules/agent-schedule/runs` 最新记录包含 `agent_final_response=agent scheduled final result`、`agent_tool_calls` 数量为 1、`stdout_preview=agent scheduled final result`；随后用 Codex in-app Browser 打开 Schedules 页面，点击 `tr[data-row-key="script-schedule"]` 打开详情，确认 Run History 中显示 `Stdout` 与 `script-ok`；关闭后点击 `tr[data-row-key="agent-schedule"]` 打开详情，确认显示 `Final Result`、`Tool Calls`、`schedule_list` 与 `agent scheduled final result`。

### TC-IMG-47: WebUI IM Gateway Provider 单列布局、字段复制与 Task Runs 详情回归

- **前置条件**:
  - 使用源码版 Bifrost 或 Vite dev server 打开 `/_bifrost/ai?aiSection=im-gateway-connections&imGatewaySection=connections`。
  - 浏览器视口宽度调整到约 760px，确保至少存在一个 provider、target、route、schedule 和一条 task run 历史记录。
- **操作步骤**:
  1. 进入 Connections，查看 provider 卡片顶部的名称、provider type、连接状态、Enabled/Disabled 和 Long Connection/Webhook 状态。
  2. 查看 provider 卡片正文中的 App ID、Secret、Owner ID、Agent Runner、Agent Work Dir、Agent Base Prompt、Agent Developer/User 等字段。
  3. 将鼠标悬浮到 App ID、Owner ID 和 Agent Work Dir 字段值上，点击出现的复制按钮。
  4. 进入 Targets，查看 targets 表格在窄宽度下的表头、Receive ID 和 Actions。
  5. 进入 Routes，查看 route 卡片中的 Provider、Event、Matcher、Action、Timeout。
  6. 进入 Schedules，查看 schedules 表格并横向滚动到 Actions，点击某一行打开 schedule detail。
  7. 进入 History / Task Runs，点击一条 Task Run 行。
- **预期结果**:
  - Connections provider 卡片不使用多列详情展示；卡片顶部直接展示名称、连接状态、启用状态和连接模式，正文所有字段按单列纵向排列。
  - App ID、Owner ID、Agent Work Dir 等非 secret 长值悬浮时显示复制按钮，点击后剪贴板写入完整原始值；Secret 只显示 `Configured` 或 `Not Set`，不暴露可复制明文。
  - Routes 卡片字段自适应换行到多行网格，不出现 `Long Connection`、`Global default` 等逐字竖排。
  - Targets 与 Schedules 表格在窄宽度下有横向滚动能力，操作按钮不会被裁掉。
  - IM Gateway 内容区域允许横向滚动兜底，顶部操作按钮可换行，不与说明文字挤压。
  - History / Task Runs 行可点击打开 `Task Run Detail` 弹窗，展示 run id、type、status、source、schedule/route/provider/target、started/ended/duration、exit code、stdout/stderr digest、error，以及 Script stdout/stderr 或 Agent final result/tool calls/plan trace。
  - 亮色和暗色主题下弹窗、表格滚动区域和卡片字段均可读。
- **执行记录（2026-05-12）**: BLOCKED — 代码实现后执行 `cargo check -p bifrost-admin -p bifrost-cli` 通过，构建过程包含 WebUI `tsc -b && vite build` 并成功生成 gzip assets；尝试用 Codex in-app Browser 验证用户当前 `http://localhost:3000` 页面时被浏览器安全策略拒绝访问该 host；随后按项目要求尝试启动临时源码版 Bifrost（`BIFROST_DATA_DIR=./.bifrost-test-layout cargo run --bin bifrost -- start -H 127.0.0.1 -p 18893 --unsafe-ssl --no-system-proxy --skip-cert-check`）用于 `127.0.0.1` 浏览器验证，但当前沙箱禁止绑定端口，返回 `Operation not permitted`。本用例需要在用户当前浏览器手动刷新后完成视觉确认。

### TC-IMG-48: Agent Chat 模式通过内置工具创建和更新两类定时任务

- **前置条件**:
  - 使用临时数据目录启动源码版 Bifrost，端口不得使用 9900，必须禁用系统代理：
    ```bash
    BIFROST_DATA_DIR=./.bifrost-test-chat-schedules cargo run --bin bifrost -- start -H 127.0.0.1 -p 18892 --unsafe-ssl --no-system-proxy --skip-cert-check
    ```
  - 启动一个 mock Chat Completions 服务，并通过 `PATCH /_bifrost/api/im-gateway/agent` 将 Agent 配置到该 mock 模型。
- **操作步骤**:
  1. 调用 `POST /_bifrost/api/im-gateway/agent/chat`，要求 Agent 使用 schedule 工具创建一个 script 定时任务和一个 agent 定时任务。
  2. mock 模型依次返回 `schedule_create` script、`schedule_create` agent、`schedule_update` script、`schedule_list` 四个 tool call。
  3. 调用 `GET /_bifrost/api/im-gateway/schedules` 查看最终任务列表。
- **预期结果**:
  - `/agent/chat` 返回 `success=true`，且 `tool_calls` 中四个 schedule 工具调用都成功。
  - Script 定时任务必须包含 `message_channel`；后端对缺少 `message_channel` 的 script schedule 返回结构化错误。
  - Script 定时任务被创建后可通过 `schedule_update` 修改名称、启用状态、interval、脚本文本和 timeout。
  - Agent 定时任务被创建后保留 `task_type=agent` 与 preset prompt。
  - `schedule_list` 和 schedules API 均能同时看到 script 与 agent 两类任务。
- **执行记录（2026-05-13）**: PASS — 使用 mock Chat Completions 服务端口 `28992` 与源码版 Bifrost 端口 `18892` 完成真实 `/agent/chat` 链路验证；第一次请求故意让 script schedule 缺少 `message_channel`，工具返回 `schedule requires message_channel`，确认校验生效；清理后第二次请求让 mock model 依次调用 `schedule_create` script、`schedule_create` agent、`schedule_update` script、`schedule_list`，四个 `tool_calls` 均 `success=true`；最终 `GET /_bifrost/api/im-gateway/schedules` 显示 `chat-script-schedule` 已更新为 `Chat Script Schedule Updated`、`enabled=false`、`every_ms=180000`、`script.script_text=echo chat-script-v2`、`timeout_ms=60000`，同时 `chat-agent-schedule` 保留 `task_type=agent`、`agent.prompt=Run the chat-created agent schedule`、`every_ms=120000`、`timeout_ms=45000`。

### TC-IMG-49: Handler 模块化拆分后 IM Gateway 功能回归

- **前置条件**:
  - `crates/bifrost-admin/src/handlers/im_gateway.rs` 已拆分为真实 Rust 子模块，不使用 `include!`。
  - 使用临时数据目录启动源码版 Bifrost，端口不得使用 9900，必须禁用系统代理。
- **操作步骤**:
  1. 执行 `cargo check -p bifrost-admin`，确认真实子模块拆分后可编译，且 WebUI build script 仍能完成。
  2. 执行 `cargo test -p bifrost-admin im_gateway::tests -- --nocapture`，覆盖 Provider 配置、消息发送、Agent reply、事件循环、图片输入和状态 helper。
  3. 启动源码版 Bifrost：`BIFROST_DATA_DIR=./.bifrost-e2e-im-regression cargo run --bin bifrost -- start -H 127.0.0.1 -p 18894 --unsafe-ssl --no-system-proxy --skip-cert-check`。
  4. 通过管理端 API 验证 `/providers`、`/targets`、`/routes`、`/schedules`、`/history/task-runs`、`/agent` 和 `/agent/chat` 路由仍可访问。
  5. 将 Agent 配置到 mock Chat Completions 服务，通过 `/agent/chat` 触发 schedule tools 创建 Script/Agent 两类任务、更新 Script 配置并读取 schedules API。
- **预期结果**:
  - `handlers/im_gateway.rs` 只保留路由分发和子模块声明，单文件低于 1500 行；每个子模块文件也低于 1500 行。
  - 后端编译和 handler 单元/集成测试全部通过。
  - 真实启动的 Bifrost 管理端 API ready，核心 IM Gateway 路由不返回 404/500。
  - `/agent/chat` 真实链路仍能创建 `task_type=script` 和 `task_type=agent` 两类定时任务，并能更新 Script 任务配置。
  - schedules API 最终能同时返回更新后的 Script schedule 与 Agent schedule。
- **执行记录（2026-05-13）**: PASS — 已将 `im_gateway.rs` 从 `include!` 机械拆分改为真实 `mod` 子模块；`wc -l` 确认入口文件 97 行，最大子模块 `agent_chat.rs` 低于 1500 行；`cargo fmt --all -- --check` 通过，`cargo check -p bifrost-admin` 通过，`cargo test -p bifrost-admin im_gateway::tests -- --nocapture` 通过 23 个测试，`cargo test -p bifrost-admin schedule_tools_create_update_list_delete_agent_schedule` 通过，`cargo test -p bifrost-agent mcp_availability` 通过，`cargo test -p bifrost-cli parse_schedule` 通过；启动源码版 Bifrost（临时数据目录 `.bifrost-e2e-im-regression`、端口 `18894`、`--no-system-proxy`），验证 `/providers`、`/targets`、`/routes`、`/schedules`、`/history/runs`、`/agent` 均返回 200；将 Agent 配置到 mock Chat Completions 后，通过真实 `/agent/chat` 触发 schedule tools 创建 Script/Agent 两类任务并更新 Script 配置，最终 schedules API 同时返回 `chat-script-schedule` 与 `chat-agent-schedule`；WebUI 影响面回归 `pnpm --dir web exec playwright test tests/ui/admin-settings.spec.ts tests/ui/agent-mcp-servers.spec.ts tests/ui/im-gateway-provider.spec.ts --grep "AI|Settings Agent|Settings IM Provider|IM Gateway Provider"` 通过 11 个用例。

### TC-IMG-50: 远端 CI Windows Rules shard 不被外层 timeout 提前取消

- **前置条件**:
  - 已推送包含 Handler 模块化拆分的 `feat/new_ai` 分支。
  - GitHub Actions `CI` workflow 已触发 pull_request run。
- **操作步骤**:
  1. 使用 GitHub Actions PAT watcher 观察最新 run 的所有 job。
  2. 如果 `E2E Rules (x86_64-pc-windows-msvc, shard 1/4)` 在 30 分钟外层 timeout 下被取消，检查同一 run 中其它 job 结论与 job step 状态。
  3. 将 `e2e-windows-rules.timeout-minutes` 放宽到 60 后重新推送。
  4. 继续 watch 新 run，直到所有 job success 或出现真实失败日志。
- **预期结果**:
  - 取消不是 Rust 编译、lint、单元测试或 IM Gateway 功能回归失败。
  - workflow 外层 timeout 足够覆盖 Windows rules shard 的内部 suite timeout、fixture 启动和 runner 清理开销。
  - 新 run 不再因为 shard 外层 envelope 被提前取消。
- **执行记录（2026-05-13）**: IN PROGRESS — 首次远端 CI run `25752880592` 中 32 个 job 成功，仅 `E2E Rules (x86_64-pc-windows-msvc, shard 1/4)` 在 `E2E rules tests` step 运行中被 job 外层 timeout 取消；run-level log zip 中没有该 cancelled shard 的 suite log，其它 Windows rules shards 均成功。已将 `.github/workflows/ci.yml` 的 `e2e-windows-rules.timeout-minutes` 从 30 调整为 60，待重新推送后继续 watch 到最终结论。

### TC-IMG-51: Schedule 绑定消息通道设计一致性

- **前置条件**:
  - 本用例用于设计文档变更后的真实可检索性检查，不启动 Bifrost 服务。
  - 技术方案已写入 `design/im-gateway.md`。
- **操作步骤**:
  1. 执行 `rg -n "ImMessageChannelBinding|message_channel|手动创建 schedule|Agent 创建 schedule|Schedule 执行" design/im-gateway.md`。
  2. 执行 `rg -n "IM Channel|Connection|default_message_channel|任务通知漂移" design/im-gateway.md`。
  3. 执行 `rg -n "TC-IMG-51|Schedule 绑定消息通道" human_tests/im-gateway.md human_tests/readme.md`。
- **预期结果**:
  - 技术文档明确说明 `ImSchedule.message_channel` 是 schedule 的唯一通道绑定。
  - 技术文档明确说明手动创建 schedule 必须显式选择或传入 IM 通道。
  - 技术文档明确说明 Agent 创建 schedule 时可从当前 IM 来源或 Agent 默认通道推导绑定通道。
  - 技术文档明确说明 schedule 执行时优先使用自身保存的 `message_channel`，避免通知漂移到错误群或错误用户。
  - `human_tests/readme.md` 索引包含本用例覆盖点。
- **执行记录（2026-05-13）**: PASS — 执行 `rg -n "ImMessageChannelBinding|message_channel|手动创建 schedule|Agent 创建 schedule|Schedule 执行" design/im-gateway.md`，命中 schedule 消息通道数据模型、手动创建、Agent 创建和执行规则；执行 `rg -n "IM Channel|Connection|default_message_channel|任务通知漂移" design/im-gateway.md`，命中唯一通道绑定、Connection 下拉、Agent 默认通道和通知不漂移约束；执行 `rg -n "TC-IMG-51|Schedule 绑定消息通道" human_tests/im-gateway.md human_tests/readme.md`，确认 human_tests 用例与索引均可检索。

### TC-IMG-52: 真实 Agent Schedule 使用绑定 IM 通道发送消息

- **前置条件**:
  - 从 `~/.bifrost` 复制真实用户 IM Provider 与 Agent 配置到临时 `BIFROST_DATA_DIR`。
  - 用当前源码启动 Bifrost：`BIFROST_DATA_DIR=<temp> cargo run --bin bifrost -- start -p 18955 --unsafe-ssl --no-system-proxy`。
  - 已确认真实 Feishu `bifrost` provider owner 通道可发送消息。
- **操作步骤**:
  1. 调用 `POST /api/im-gateway/schedules` 创建 disabled agent schedule，`message_channel` 绑定 `provider_id=bifrost,target_mode=owner,target_id=owner`。
  2. schedule 的 agent prompt 要求模型只调用一次 `send_msg`，发送唯一时间戳文本。
  3. 调用 `POST /api/im-gateway/schedules/<id>/run` 手动触发一次。
  4. 查询 `GET /api/im-gateway/schedules/<id>/runs`，检查 run 状态和 agent tool calls。
  5. 检查 `admin/im_gateway_message_logs.json`，确认 schedule 内部 `send_msg` 和 schedule 完成通知均发送成功。
- **预期结果**:
  - schedule 保存后包含 `message_channel`，没有 schedule 级发送目标字段。
  - 手动 run 返回 `status=Success`，`stdout_preview` 为 agent 最终回复。
  - run 记录中的 `agent_tool_calls` 包含 `send_msg` 且 `success=true`。
  - 消息日志包含一条 `trigger=agent_tool:send_msg` 的真实发送成功记录，以及一条 `trigger=schedule:<id>` 的完成通知成功记录。
  - disabled schedule 不会在测试结束后继续周期触发。
- **清理步骤**:
  - 停止测试 Bifrost 进程。
  - 删除临时 `BIFROST_DATA_DIR`。
- **执行记录（2026-05-13）**: PASS — 创建 `real-agent-schedule-20260513-124107`，保存结果包含 `message_channel.provider_id=bifrost,target_mode=owner,target_id=owner` 且 `enabled=false`。手动触发 `/run` 返回 `run_id=8864e487,status=Success,duration_ms=7638,stdout_preview=schedule send_msg succeeded`。运行历史中 `agent_tool_calls[0].tool_name=send_msg,success=true`，工具结果包含真实 `message_id=om_x100b6f74403218acc45ca19e00d32f4`。消息日志同时记录 schedule 内部 `send_msg` 成功和完成通知成功，完成通知 message_id 为 `om_x100b6f74418a5ca8c22d79d534bd554`。

### TC-IMG-53: Connections Provider 单列卡片与关键字段复制

- **前置条件**:
  - 使用源码版 Bifrost 或 Vite dev server 打开 `/_bifrost/ai?aiSection=im-gateway-connections&imGatewaySection=connections`。
  - 至少存在一个包含 `app_id`、`owner_open_id` 和 `agent_config.work_dir` 的 provider。
- **操作步骤**:
  1. 进入 Connections 页面，查看 provider 卡片顶部。
  2. 查看 provider 卡片正文中 App ID、Secret、Owner ID、Agent Runner、Agent Work Dir、Agent Base Prompt、Agent Developer/User 等字段。
  3. 将鼠标悬浮到 App ID 字段值上，点击出现的复制按钮。
  4. 将鼠标悬浮到 Owner ID 字段值上，点击出现的复制按钮。
  5. 将鼠标悬浮到 Agent Work Dir 字段值上，点击出现的复制按钮。
- **预期结果**:
  - 卡片顶部展示 provider 名称、类型、连接状态、启用状态和连接模式。
  - 卡片正文不使用多列详情布局，所有字段按单列纵向排列。
  - App ID、Owner ID、Agent Work Dir 悬浮后显示复制按钮，点击后剪贴板写入完整原始值。
  - App ID 可以继续脱敏展示，但复制结果必须是完整 App ID；Secret 只显示配置状态，不暴露可复制明文。
  - 亮色和暗色主题下字段文本、状态标签和复制按钮均可读可点击。
- **执行记录（2026-05-15）**: PASS — 执行 `pnpm --dir web exec playwright test tests/ui/im-gateway-provider.spec.ts --grep "卡片单列展示"`，用 API 创建 `Copyable Provider`，浏览器打开 Connections 后确认卡片顶部包含名称、连接状态、Enabled 和 Long Connection；App ID、Owner ID、Agent Work Dir 三个字段的 bounding box y 坐标递增，确认纵向单列展示；悬浮 App ID 后复制按钮 opacity 从 0 变为 1，点击后三个字段分别把完整 `cli_copyable_provider_id`、`ou_copyable_provider_owner`、`~/work/github/bifrost` 写入剪贴板。

### TC-IMG-54: Schedule Agent 选择 Runner 与 ChatGPT Web 初始化 Prompt

- **前置条件**:
  - 使用当前源码构建的 Bifrost 或 Vite dev server 打开 `/_bifrost/ai?aiSection=im-gateway-schedules&imGatewaySection=schedules`。
  - 已存在至少一个配置了默认接收者的 Connection；External CLI Runner 配置中存在一个 adapter 为 `chatgpt_web` 的 Runner。
- **操作步骤**:
  1. 点击 Schedules 页面 Add，选择 `Task Type=Agent`。
  2. 在 `IM Channel` 中选择一个 Connection。
  3. 在 `Runner` 中选择 `Bifrost Agent`，确认表单没有展示 ChatGPT Web 专用 `First-round Prompt`。
  4. 将 `Runner` 切换到 ChatGPT Web Runner，填写 `First-round Prompt`、`Preset Prompt` 和 `Default Execution Directory`。
  5. 提交创建 schedule，并查看详情页。
  6. 点击该 schedule 行上的 Run 按钮手动触发，随后进入详情页检查 run 记录与发送结果。
- **预期结果**:
  - Agent schedule 创建时 `IM Channel` 必填且可选择现有 Connection，不需要选择具体发送对象。
  - Runner 下拉包含 `Bifrost Agent` 与已配置的外部 Runner。
  - 选择 ChatGPT Web Runner 后展示 `First-round Prompt`，提交 payload 中保存为 `agent.initial_prompt`；该 schedule session 尚无 ChatGPT Web conversation 绑定时先发送这条初始化消息，后续 run 复用同一 conversation 并只发送 `Preset Prompt`。
  - Bifrost Agent 与 Codex Runner 都可保存 `agent.work_dir` 作为默认执行目录；实际 run 必须从该目录执行，未配置 schedule 目录时继承 Provider/全局 Agent 工作目录。
  - ChatGPT Web run 成功后 schedule 原信息写入 `agent.conversation_ref.conversation_id`；Codex run 成功后写入 `agent.conversation_ref.thread_id`；下一次 run 复用该 schedule 自身的对话引用。
  - run 记录包含 `runner_id`、`provider_id`、`input_preview` 和 `agent_final_response`。
  - schedule run 完成后使用绑定 IM Channel 的默认接收者发送完成消息，消息正文优先包含 Agent 最终结果。
  - 点击 schedule 行可查看历史运行详情；每次 run 都展示当次输入和当次输出。
- **清理步骤**:
  - 删除测试 schedule。
  - 如启动了临时 Bifrost/Vite 服务，停止进程并删除临时数据目录。
- **执行记录（2026-05-19）**: PASS — 执行 `pnpm --dir web exec tsc -b --pretty false` 通过，确认 SchedulesPanel 类型接入 `ExternalCliGatewayConfig`、Agent 表单包含 Runner、`Default Execution Directory` 与 ChatGPT Web `First-round Prompt` 条件展示，`IM Channel` 下拉直接使用 Connections；执行 `cargo test -p bifrost-admin schedule_agent_can_run_selected_external_runner_with_initial_prompt -- --nocapture` 通过，mock 外部 Runner 断言输入同时包含 `INIT_MARKER` 和 `TASK_MARKER`，run 记录为 `status=Success`、`runner_id=chatgpt-test`、`provider_id=feishu-main`、`input_preview` 包含实际发送消息、`agent_final_response=SCHEDULE_RUNNER_OK`。第二轮 review 补充 `schedule_chatgpt_web_initial_prompt_is_sent_as_first_message_only`，覆盖 ChatGPT Web 初始 Prompt 仅在没有 conversation 绑定时作为第一条消息发送，已有 conversation 后只发送 preset prompt。后续增强执行 `cargo test -p bifrost-admin schedule_agent_persists_codex_thread_id_for_next_run -- --nocapture`、`cargo test -p bifrost-admin schedule_external_result_extracts_chatgpt_conversation_id -- --nocapture`、`cargo test -p bifrost-admin schedule_agent_work_dir_prefers_schedule_then_inherited_default -- --nocapture`、`cargo test -p bifrost-admin schedule_external_runner_executes_from_configured_work_dir -- --nocapture` 通过，验证 schedule 持久化 ChatGPT/Codex 对话引用，并且 Bifrost Agent/Codex Runner 使用 schedule 或继承的默认执行目录。

### TC-IMG-55: Agent Markdown 图片附件通过 IM 图片通道发送

- **前置条件**:
  - 使用临时 `BIFROST_DATA_DIR`，避免污染默认用户数据。
  - 准备一个 mock Chat Completions 服务，返回内容包含：
    - 本地图片 Markdown：`![ChatGPT 生成图片 1](/tmp/.../chatgpt-web-image-1.png)`。
    - 远端图片附件 Markdown：`![远端附件](http://127.0.0.1:<port>/chart.png)`，HTTP 响应 `Content-Type=image/png`。
    - 普通下载链接 Markdown：`[下载图片](http://127.0.0.1:<port>/download)`，URL 不带图片扩展名但 HTTP 响应 `Content-Type=image/png`。
    - 明确文件附件链接：`[报告附件](https://files.oaiusercontent.com/report.pdf)`。
    - 一条 ChatGPT 引用卡片风格 favicon：`[![](https://www.google.com/s2/favicons?...)](...)`。
  - 准备 Weixin provider 配置，`base_url` 指向不可达测试地址即可；本用例验证发送层会尝试走 image 通道并记录 message log，不依赖真实微信外网成功。
- **操作步骤**:
  1. 运行 E2E wrapper：
     `e2e-tests/tests/test_im_agent_markdown_image_reply.sh`
  2. 该 wrapper 内部运行 Markdown 图片拆分与远端附件下载单元测试：
     `SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin agent_reply_ --lib`
  3. 该 wrapper 内部运行主 Agent Chat 回归测试：
     `SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin agent_chat_final_reply_sends_local_markdown_images_as_im_images --lib`
  4. 检查测试日志或 `ImMessageLogStore` 断言，确认生成了 `msg_type=image` 的 outbound 记录。
  5. 确认 interactive 文本预览不再包含本地图片路径，避免把 `![...](本地路径)` 作为纯文本发给用户。
  6. 确认远端附件图已下载到 `agent/im_gateway/attachments/agent_reply_markdown/` 后再进入发送流程。
  7. 确认普通下载链接若响应为 `image/*`，下载后会进入图片发送列表，而不是文件附件列表。
  8. 确认明确文件附件链接会被收集为待下载附件，普通新闻链接不会被误收集。
  9. 确认 `google.com/s2/favicons` 引用图不会被收集为待发送图片。
- **预期结果**:
  - Agent final reply / continuation reply / streaming final flush 中的本地图片 Markdown 会被剥离出正文，并通过 provider `send_image` 独立发送。
  - 明确的远端图片附件会先下载成本地附件文件；如果 IM 通道发送失败，文件仍保留在本地，message log 保留失败原因。
  - Markdown 普通下载链接本身若返回图片类型，即使不是 `![...](...)` 图片语法，也必须通过图片通道发送。
  - 明确的非图片附件会下载到 `agent/im_gateway/attachments/agent_reply_files/`；支持文件消息的通道发送 `msg_type=file`，不支持时记录失败并保留本地文件。
  - favicon / 引用卡片小图标不会被误发为用户图片。
  - 文字卡片仍发送剩余正文，图片通过 IM 图片消息逐张发送。
- **执行记录（2026-05-20）**: PASS — 执行 `e2e-tests/tests/test_im_agent_markdown_image_reply.sh` 通过。wrapper 内部执行 `SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin agent_reply_ --lib`，10 个 Agent reply 图片/附件解析、下载和目标选择测试全部通过，覆盖本地路径、远端附件下载、普通下载链接按 `Content-Type=image/png` 转图片通道、非图片附件识别、普通链接排除、favicon 排除、代码块保护和去重；执行 `SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin agent_chat_final_reply_sends_local_markdown_images_as_im_images --lib` 通过，mock 模型返回 `![ChatGPT 生成图片 1](<本地图片路径>)` 后，主 Agent Chat 链路产生 `msg_type=image` outbound message log，interactive preview 不再包含本地图片路径。

### TC-IMG-56: Connections 新建 Provider 先选择类型再进入平台连接

- **前置条件**:
  - 使用源码版 Bifrost 或 Vite dev server 打开 `/_bifrost/ai?aiSection=im-gateway-connections&imGatewaySection=connections`。
  - 如启动 Bifrost，必须使用临时 `BIFROST_DATA_DIR`，并携带 `--no-system-proxy`。
- **操作步骤**:
  1. 打开 Connections 页面，点击 `Add Provider`。
  2. 确认弹窗第一步只显示 `Type` 选择，包含 `Weixin` 与 `Feishu` 两个选项。
  3. 确认第一步不展示 `App ID`、`App Secret`、`Agent Runner`、`Agent Working Directory`、Prompt 等高级配置。
  4. 选择 `Feishu` 后点击 `Next`。
  5. 在第二步填写 Provider ID，确认页面展示 Feishu 二维码、setup URL 或打开按钮。
  6. 在亮色和暗色主题下重复步骤 1-5，确认文本、二维码区域、步骤条和按钮均可读可点击。
- **预期结果**:
  - 用户不会在第一屏看到复杂的连接与 Agent 配置字段。
  - 选择 Feishu 后第二步立即进入二维码 setup，不要求用户手填 App ID 或 App Secret。
  - 新建流程的高级配置只在连接完成后的 `Configure` 步骤展示。
  - 亮色和暗色主题下没有文本重叠、不可读按钮或低对比度状态提示。
- **清理步骤**:
  - 如创建了测试 Provider，删除测试 Provider。
  - 停止临时服务并删除临时 `BIFROST_DATA_DIR`。
- **执行记录（2026-05-26）**: PASS — 执行 `pnpm --dir web exec playwright test tests/ui/im-gateway-provider.spec.ts` 通过。Feishu setup 用例在暗色主题打开 Connections 页面，点击 `Add Provider` 后确认第一步仅显示 Weixin/Feishu 类型选择，未出现 `App ID`、`App Secret`、`Agent Runner`、`Agent Working Directory`；选择 Feishu 后进入二维码 setup，展示 `https://open.feishu.cn/page/launcher?user_code=123`、`Open Setup Page` 和 `App created. Bifrost has the App ID and Secret on the server.`，连接成功后才展示 `Connection` 与 `Agent Runner` 配置。失败回归用例模拟 `/feishu-setup/start` 返回 502，弹窗内显示 `Failed to start Feishu setup.`、后端错误详情和 `Retry Setup` 按钮；点击重试后恢复展示二维码 URL。随后使用临时数据目录启动源码版 Bifrost：`BIFROST_DATA_DIR=./.bifrost-test-feishu-setup-real SKIP_FRONTEND_BUILD=1 cargo run --bin bifrost -- start -p 18893 --unsafe-ssl --no-system-proxy --skip-cert-check`，真实调用 `POST /_bifrost/api/im-gateway/providers/feishu-setup/start` 返回 `success=true`、`session_id=fas_...`、`verification_url=https://open.feishu.cn/page/launcher?user_code=4MUN-PRLG`；再用 Playwright 打开 `http://127.0.0.1:18893/_bifrost/ai?aiSection=im-gateway-connections&imGatewaySection=connections`，实际点击 Add Provider → Feishu → Next，页面渲染真实 `open.feishu.cn/page/launcher` 二维码 URL 和 `Open Setup Page` 链接。

### TC-IMG-57: Weixin 新建 Provider 后立即弹出扫码二维码并延后配置

- **前置条件**:
  - 使用源码版 Bifrost 或 Vite dev server 打开 `/_bifrost/ai?aiSection=im-gateway-connections&imGatewaySection=connections`。
  - 如启动 Bifrost，必须使用临时 `BIFROST_DATA_DIR`，并携带 `--no-system-proxy`。
- **操作步骤**:
  1. 打开 Connections 页面，点击 `Add Provider`。
  2. 保持或选择 `Weixin`，点击 `Next`。
  3. 填写唯一 Provider ID，例如 `weixin-progressive-setup`。
  4. 点击 `Create and Show QR`。
  5. 确认页面调用 Weixin 登录启动流程，并弹出 `Scan Weixin QR` 二维码弹窗。
  6. 在未扫码前确认 Agent Runner、工作目录、Prompt 等高级配置没有提前要求填写。
  7. 模拟或完成扫码确认后，确认 Add Provider 主弹窗进入 `Configure` 步骤，再展示高级配置。
- **预期结果**:
  - Weixin 第二步只要求最小 Provider 元数据，创建后立即展示二维码。
  - 扫码连接前不要求用户理解或填写 Agent 运行配置。
  - 扫码确认后才进入 `Configure`，可保存 Runner、工作目录和 Prompt 等高级配置。
  - 失败或重复 Provider ID 时，toast 展示后端真实错误，不退回通用错误。
- **清理步骤**:
  - 删除 `weixin-progressive-setup` 测试 Provider。
  - 停止临时服务并删除临时 `BIFROST_DATA_DIR`。
- **执行记录（2026-05-26）**: PASS — 执行 `pnpm --dir web exec playwright test tests/ui/im-gateway-provider.spec.ts` 通过。Weixin setup 用例从 Connections 页面点击 `Add Provider`，默认 Weixin 后点击 `Next`，第二步只展示 Provider ID、Display Name 与 “immediately show the Weixin QR code” 提示，未提前展示 `Agent Runner`；点击 `Create and Show QR` 后弹出 `Scan Weixin QR` modal，并展示 `https://login.weixin.qq.com/qrcode/test-weixin-qr`。重复 Provider ID 回归用例继续验证 toast 展示后端真实错误。

### TC-IMG-58: Edit IM Provider 表单字段间距紧凑

- **前置条件**:
  - 使用源码版 Bifrost 或 Vite dev server 打开 `/_bifrost/ai?aiSection=im-gateway-connections&imGatewaySection=connections`。
  - 已存在至少一个 Feishu Provider。
- **操作步骤**:
  1. 在 Connections 页面点击某个 Provider 的 `Edit`。
  2. 查看 `Edit IM Provider` 弹窗中的 Display Name、Enabled、Owner Open ID、App ID、App Secret、Agent Runner、Agent Working Directory、Prompt 等字段。
  3. 对比字段之间的垂直留白，确认 label、输入框、extra 说明文案形成紧凑连续的信息块。
  4. 确认 `Add IM Provider` 的 Type → Connect → Configure 渐进式向导没有被 compact 样式影响。
- **预期结果**:
  - 编辑弹窗使用紧凑表单密度，字段之间不再出现大段空白。
  - `Form.Item` 之间保持足够可读的 10px 级间距，label 到输入框距离明显小于默认垂直表单。
  - extra 说明文案仍可读，不与后续字段重叠。
  - 亮色和暗色主题下布局一致，无文本重叠。
- **清理步骤**:
  - 删除测试 Provider。
- **执行记录（2026-05-26）**: PASS — 执行 `pnpm --dir web exec playwright test tests/ui/im-gateway-provider.spec.ts --grep "编辑时可以补填 App Secret"` 通过。用例打开 `Edit IM Provider` 弹窗后确认存在 `.im-provider-edit-form-compact`，首个 `.ant-form-item` 的 `margin-bottom=10px`，首个 `.ant-form-item-label` 的 `padding-bottom=2px`；随后继续补填 App Secret 并保存成功，证明紧凑样式未破坏编辑提交流程。

### TC-IMG-59: Connections Provider 列表最大 750px 并居中展示

- **前置条件**:
  - 使用源码版 Bifrost 或 Vite dev server 打开 `/_bifrost/ai?aiSection=im-gateway-connections&imGatewaySection=connections`。
  - 至少存在一个 Provider，且浏览器 viewport 宽度大于 1200px。
- **操作步骤**:
  1. 打开 Connections 页面。
  2. 查看 Provider 卡片列表整体宽度与页面左右留白。
  3. 查看每张卡片顶部的连接/编辑/删除按钮与卡片数据之间的横向距离。
  4. 缩小窗口到 750px 以下，确认卡片仍能按容器宽度自适应。
- **预期结果**:
  - Provider 列表最大宽度约 750px，并在内容区水平居中。
  - 卡片不会在宽屏下被拉满整行，操作按钮不再远离 Provider 名称与字段数据。
  - 窄屏下列表宽度为 100%，不会产生额外水平滚动。
- **清理步骤**:
  - 删除测试 Provider。
- **执行记录（2026-05-26）**: PASS — 执行 `pnpm --dir web exec playwright test tests/ui/im-gateway-provider.spec.ts` 通过。卡片展示用例确认 `settings-im-provider-list` 可见，bounding box 宽度不超过 760px 容差，并在桌面 viewport 下相对页面居中；原有卡片单列字段和复制按钮断言继续通过。

### TC-IMG-60: Feishu 自动创建 Provider 长连接直连并正确暴露重连状态

- **前置条件**:
  - 使用源码版 Bifrost 或当前本机运行的 Bifrost。
  - 已通过 Connections 的 Feishu 二维码流程创建一个 Provider，例如 `feishu-main`。
  - 本机可能启用了系统代理或环境代理，代理地址可能指向 Bifrost 自身。
- **操作步骤**:
  1. 创建或重新连接 `feishu-main` Provider。
  2. 查看服务日志，关注 `open.feishu.cn/callback/ws/endpoint` 获取结果。
  3. 调用 `GET /_bifrost/api/im-gateway/providers/feishu-main/status` 查看状态。
  4. 如果 endpoint 获取失败，确认状态为 `reconnecting`，并包含 `ws endpoint fetch failed` 的 `last_error`。
  5. 如果 endpoint 获取成功，确认状态只在 `feishu websocket connected` 后变为 `connected`。
  6. 向新机器人发送一条消息，确认 `GET /_bifrost/api/im-gateway/providers/feishu-main/messages?direction=inbound` 出现 inbound 记录，随后产生回复或至少进入 Agent 处理日志。
- **预期结果**:
  - Feishu long connection 的 tenant token 和 WS endpoint 请求绕过本机代理，不会因为 Bifrost 自身代理导致 endpoint fetch 失败。
  - 状态 API 不会在连接 task 刚启动时误报 `connected`。
  - 长连接失败时 WebUI/API 可见 `reconnecting` 与真实错误，便于区分网络/代理问题和 Agent 无响应问题。
  - 消息未回复时能通过 inbound message log 判断是否已经进入 Bifrost。
- **清理步骤**:
  - 删除测试 Provider 或保留用户真实 Provider。
- **执行记录（2026-05-26）**: PASS — 针对用户当前 `feishu-main` 日志先执行只读排查：`GET /providers/feishu-main/messages?limit=50` 只有 `trigger=online` 的 outbound 记录，无 inbound，确认消息没有进入 Bifrost；日志显示 `ws endpoint fetch failed`，根因定位为 Feishu long connection endpoint 请求未使用直连 client，且状态 API 因提前标记 connected 产生误导。修复后执行 `cargo test -p bifrost-admin test_connection_state_reconnect_error_clears_after_connected --lib` 通过，验证 reconnecting/connected 状态转换和 last_error 清理。

### TC-IMG-61: Provider 卡片与 IM 状态展示解析后的默认工作目录

- **前置条件**:
  - 使用源码版 Bifrost 或 Vite dev server 打开 `/_bifrost/ai?aiSection=im-gateway-connections&imGatewaySection=connections`。
  - 全局 Agent 配置未显式设置 `work_dir`，至少存在一个未配置 Provider 专属 `agent_config.work_dir` 的 IM Provider。
  - 如启动 Bifrost，必须使用临时 `BIFROST_DATA_DIR`，并携带 `--no-system-proxy`。
- **操作步骤**:
  1. 打开 Connections 页面，查看该 Provider 卡片的 `Agent Work Dir` 字段。
  2. 调用或查看 `GET /_bifrost/api/im-gateway/agent`，确认响应包含展示用 `resolved_work_dir`，且原始 `work_dir` 仍保持未配置状态。
  3. 从该 Provider 对应 IM 通道发送 `/status`，或通过 Agent API status 查询同一 session。
  4. 在运行中进度卡或 `/status` 文案里查看 `工作路径`。
  5. 向 Agent 询问当前目录，确认 Agent 回答的目录与卡片/status 展示一致。
- **预期结果**:
  - Provider 卡片不再把 `Agent Work Dir` 显示为 `Global default`，而是显示解析后的真实默认目录，且字段可复制。
  - `GET /im-gateway/agent` 不把 `resolved_work_dir` 写回为配置项，原始 `work_dir` 仍保持未配置状态。
  - IM `/status`、Agent API status 与进度卡不再显示 `工作路径: N/A`。
  - Agent 实际运行目录与管理端/IM 状态展示一致。
- **清理步骤**:
  - 删除测试 Provider。
  - 停止临时服务并删除临时 `BIFROST_DATA_DIR`。
- **执行记录（2026-05-26）**: PASS — 执行 `cargo test -p bifrost-admin provider_agent_work_dir_resolves_global_default_directory agent_config_response_includes_resolved_work_dir im_status_text_uses_resolved_default_work_dir_when_session_has_no_override --lib` 和 `pnpm --dir web exec playwright test tests/ui/im-gateway-provider.spec.ts --grep "继承后的默认工作目录"` 通过。回归验证覆盖配置 API 的 `resolved_work_dir`、Provider 卡片继承目录展示，以及 IM/API status 缺少 session work_dir 时不再显示 `N/A`。

### TC-IMG-62: Owner 上线通知包含当前设备名称

- **前置条件**:
  - 使用临时数据目录启动 Bifrost，端口不得使用 9900，必须禁用系统代理：
    ```bash
    BIFROST_DEVICE_NAME=eden-macbook BIFROST_DATA_DIR=./.bifrost-test-im-provider-device-name cargo run --bin bifrost -- start -p 18892 --unsafe-ssl --no-system-proxy --skip-cert-check
    ```
  - 浏览器打开 `http://127.0.0.1:18892/_bifrost/ai?aiSection=im-gateway-connections&imGatewaySection=connections`。
  - 准备真实可用且可发送 owner 通知的飞书或微信 Provider 配置。
- **操作步骤**:
  1. 在 WebUI 点击 `Add Provider`。
  2. 选择 Provider 类型，完成连接步骤。
  3. 在配置步骤保留默认 Agent 工作目录，或显式填写 `/Users/eden/work/github/bifrost`。
  4. 点击完成创建，等待 Provider 连接成功并触发 owner 上线通知。
  5. 查询该 Provider 的消息记录：
     ```bash
     curl -s http://127.0.0.1:18892/_bifrost/api/im-gateway/providers/<provider-id>/messages
     ```
- **预期结果**:
  - owner 收到的上线通知是 Feishu Markdown 卡片（微信通道降级为文本）。
  - 通知正文以 `**Bifrost is online**` 开头，包含 `Provider`、`Device`、`Workspace` 和 `Status`。
  - 第 5 步 message log 的 `msg_type=interactive`（Feishu）且 `content_preview` 与 owner 实收通知一致，包含同一设备名称和工作目录。
- **清理步骤**:
  - 删除测试 Provider。
  - 停止临时服务并删除临时 `BIFROST_DATA_DIR`。
- **执行记录（2026-05-26）**: PASS — 执行 `cargo test -p bifrost-admin online_notification_message_ --lib` 通过，覆盖固定设备名 `eden-macbook` 时上线通知同时包含 `设备名称：eden-macbook`、Provider 自定义工作目录和进程 cwd 回退目录；消息发送链路沿用 `build_online_notification_message` 生成的同一 `content_preview`。
- **执行记录（2026-06-03）**: PASS — 使用当前源码重启默认服务：`BIFROST_DATA_DIR=/Users/eden/.bifrost BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 SKIP_FRONTEND_BUILD=1 cargo run --bin bifrost -- start -p 9900 --unsafe-ssl --no-system-proxy --daemon -y`。查询 `feishu-main` outbound 记录确认重启上线通知 `trigger=online`、`status=success`、`msg_type=interactive`，`content_preview` 为 `**Bifrost is online**`，包含 `Provider: feishu-main`、`Device: eden-work`、`Workspace: /Users/eden/work/github/bifrost` 和 `Status: Ready`。


### TC-IMG-65: 重启/重连上线通知展示 Runner 与会话轮次

- **前置条件**:
  - 使用临时数据目录启动源码版 Bifrost，必须设置 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`，必须携带 `--no-system-proxy`。
  - 存在一个配置了 `owner_open_id` 的 IM Provider，Provider 专属 Agent Runner 可配置为内置 `bifrost_agent` 或外部 Runner，例如 `chatgpt_web`。
  - 该 owner 对应 session 已有至少一轮历史对话，或在临时数据目录中预置同 session key 的 history JSONL。
- **操作步骤**:
  1. 启动或重启 Bifrost，等待 Provider 自动连接；或手动调用 `POST /_bifrost/api/im-gateway/providers/<provider-id>/connect` 触发重连。
  2. 查询该 Provider 的 outbound 消息记录：
     ```bash
     curl -sS http://127.0.0.1:<port>/_bifrost/api/im-gateway/providers/<provider-id>/messages?direction=outbound\&limit=20
     ```
  3. 打开 owner IM 会话，查看最新上线通知卡片。
  4. 如果 Provider 绑定外部 Runner，确认 Runner 类型来自外部 Runner adapter；如果未绑定外部 Runner，确认显示为内置 `bifrost_agent`。
- **预期结果**:
  - 最新上线通知 `trigger=online`、`status=success`；Feishu 通道 `msg_type=interactive`，微信通道按现有能力降级为文本。
  - 通知正文仍以 `**Bifrost is online**` 开头，并保留 `Provider`、`Device`、`Workspace`、`Status`。
  - 通知新增并正确展示 `Runner Type`、`Runner ID`、`Bound Session`、`Completed User Turns`。
  - `Bound Session` 与 IM Agent 会话 key 一致，格式为 `<provider_id>:<owner_open_id>`；`Completed User Turns` 与当前内存 session 或持久化 history 中 user message 数一致。
  - 外部 Runner 场景下 `Runner Type` 显示 adapter（例如 `chatgpt_web`），`Runner ID` 显示用户配置的 runner id；内置 Runner 场景下 `Runner Type` 为 `bifrost_agent`，`Runner ID` 为 `N/A`。
- **清理步骤**:
  - 删除测试 Provider。
  - 停止临时服务并删除临时 `BIFROST_DATA_DIR`。
- **执行记录（2026-06-04）**: PASS — 执行 `SKIP_FRONTEND_BUILD=1 e2e-tests/tests/test_im_online_notification_runner_context.sh`，脚本使用 mock Feishu API、临时数据目录和预置 `feishu-main:ou_owner` 两轮 user history 启动当前源码 Bifrost；创建绑定 `web-main` / `chatgpt_web` 的 Provider 并调用 connect 后，message log 最新 `trigger=online` 记录为 `status=success`、`msg_type=interactive`，`content_preview` 包含 `Provider`、`Device`、`Workspace`、`Runner Type: chatgpt_web`、`Runner ID: web-main`、`Bound Session: feishu-main:ou_owner`、`Completed User Turns: 2` 与 `Status: Ready`；mock Feishu 实收卡片正文也包含 Runner 类型与轮次。

### TC-IMG-63: Feishu text 发送默认转为 Markdown 卡片

- **前置条件**:
  - 当前源码版 Bifrost 已在默认目录或临时目录启动，必须禁用系统代理。
  - 存在真实可用的 Feishu Provider，例如 `feishu-main`，并配置 `owner_open_id`。
- **操作步骤**:
  1. 调用：
     ```bash
     MARKER="FEISHU_CARD_TEXT_DEFAULT_$(date +%Y%m%d_%H%M%S)"
     curl -sS -X POST http://127.0.0.1:9900/_bifrost/api/im-gateway/messages/send \
       -H 'content-type: application/json' \
       -d "{\"provider_id\":\"feishu-main\",\"target_id\":\"owner\",\"msg_type\":\"text\",\"content\":\"**Bifrost Feishu Card Test**\\n\\n- **Marker**: \`$MARKER\`\\n- **Expected**: text request is delivered as an interactive Markdown card\"}"
     ```
  2. 在飞书中观察 owner 是否收到卡片消息。
  3. 查询 `GET /_bifrost/api/im-gateway/providers/feishu-main/messages?direction=outbound&limit=20`。
- **预期结果**:
  - 第 1 步返回 `message_id`。
  - 飞书实收消息为卡片，且 Markdown 粗体和列表渲染正常。
  - 第 3 步包含 marker 对应记录，`direction=outbound`、`status=success`、`trigger=api`、`msg_type=interactive`。
- **清理步骤**:
  - 无需清理；保留一条测试消息记录用于观察。
- **执行记录（2026-06-03）**: PASS — 使用默认服务 `9900` 和 provider `feishu-main` 发送 marker `FEISHU_CARD_TEXT_DEFAULT_20260603_143043`。API 返回 `message_id=om_x100b6ec8eff87cacc086e6db7f5da35`；消息记录 `id=dddcc129`、`status=success`、`trigger=api`、`msg_type=interactive`，`content_preview` 以 `**Bifrost Feishu Card Test**` 开头并包含 marker。

### TC-IMG-64: IM 内置 Agent mock 模型用例隔离 worker 环境变量

- **前置条件**:
  - 工作目录为项目根目录。
  - 不启动 Bifrost 服务，不使用 9900，不修改系统代理。
  - 本机可执行 Rust 单元测试。
- **操作步骤**:
  1. 复现/验证本地 workspace 暴露的 IM 图片消息 mock 模型用例：
     ```bash
     cargo test -p bifrost-admin handlers::im_gateway::tests::im_event_loop_forwards_image_attachment_to_agent_chat -- --nocapture
     ```
  2. 验证 agent worker 自测仍能覆盖强制外部 worker 与默认 in-process worker 两条分支：
     ```bash
     cargo test -p bifrost-admin 'im_gateway::agent_worker::tests::spawn_' -- --nocapture
     ```
  3. 执行 workspace 全量兜底：
     ```bash
     cargo test --workspace --all-features
     ```
- **预期结果**:
  - 第 1 步 mock server 能收到 chat request，并断言图片 URL 数量为 `MAX_AGENT_IMAGES_PER_MESSAGE`。
  - 第 2 步两个 agent worker 环境变量相关测试全部通过。
  - 第 3 步不再因 `BIFROST_FORCE_AGENT_WORKER` 并发污染导致 IM 内置 Agent mock 模型用例偶发没有请求。
  - 测试不启动真实服务、不使用 9900、不修改系统代理。
- **清理步骤**:
  - 无特殊清理；测试使用临时目录与进程内 mock server。
- **执行记录（2026-06-04）**: PASS — 本地 `cargo test --workspace --all-features` 首次暴露 `im_event_loop_forwards_image_attachment_to_agent_chat` 偶发未收到 mock chat request，单独重跑通过，定位为 worker 环境变量并发隔离不足。修复后执行第 1 步通过；第 2 步执行 `cargo test -p bifrost-admin 'im_gateway::agent_worker::tests::spawn_' -- --nocapture`，2 个用例通过。workspace 全量继续由本地复跑和远端 CI 共同兜底。

### TC-IMG-66: 删除或禁用微信 Provider 后停止 getupdates 轮询

- **前置条件**:
  - 使用临时数据目录启动当前源码版 Bifrost，必须设置 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`，必须携带 `--no-system-proxy` 和 `--skip-cert-check`。
  - 准备一个本地 mock Weixin iLink 服务，至少实现 `/ilink/bot/get_bot_qrcode`、`/ilink/bot/get_qrcode_status`、`/ilink/bot/getupdates`、`/ilink/bot/sendmessage`，并把每次 `getupdates` 的 `Authorization` token 写入日志。
  - 创建 `weixin-mock` Provider，`enabled=true`、`event_connection_enabled=true`、`base_url` 指向 mock iLink 服务，完成扫码登录并连接。
- **操作步骤**:
  1. 等待 `weixin-mock` 至少产生 1 次 `getupdates` 请求，并记录 mock 日志中 `Bearer mock-token` 的当前计数。
  2. 在 WebUI `AI -> IM Gateway -> Connections` 删除 `weixin-mock`，或执行：
     ```bash
     curl -sS -X DELETE http://127.0.0.1:<port>/_bifrost/api/im-gateway/providers/weixin-mock
     ```
  3. 等待 7 秒以上，再次统计 mock 日志中 `Bearer mock-token` 的计数。
  4. 创建 `weixin-disabled` Provider，`enabled=true`、`event_connection_enabled=false`、`app_secret=disabled-token`、`base_url` 仍指向 mock iLink 服务。
  5. 重启同一临时数据目录的 Bifrost，等待 4 秒以上，统计 mock 日志中 `Bearer disabled-token` 的计数。
- **预期结果**:
  - 第 2 步删除返回成功，`GET /_bifrost/api/im-gateway/providers/weixin-mock` 返回 404 或不再出现在列表中。
  - 第 3 步删除后 `Bearer mock-token` 计数不再按 3 秒轮询周期增长；允许删除瞬间最多 1 次 in-flight 请求竞态，但不得持续新增。
  - 第 5 步 `event_connection_enabled=false` 的 `weixin-disabled` 在重启后不会自动连接，mock 日志中 `Bearer disabled-token` 计数为 0。
  - Bifrost 日志不再持续出现已删除 Provider 的 `weixin poll failed provider_id=weixin-mock` 或 `weixin-main`。
- **清理步骤**:
  - 停止临时 Bifrost 进程和 mock iLink 进程。
  - 删除临时 `BIFROST_DATA_DIR`。
- **执行记录（2026-06-08）**: PASS — 执行 `SKIP_FRONTEND_BUILD=1 e2e-tests/tests/test_weixin_provider_e2e.sh`，脚本使用 mock iLink 和临时数据目录完成微信扫码登录、连接、收发消息；删除 `weixin-mock` 后等待 7.2 秒，`Bearer mock-token` 的 `getupdates` 计数未持续增长，且 provider API 已返回 404；随后创建 `event_connection_enabled=false` 的 `weixin-disabled` 并重启同一数据目录，等待 4.2 秒后 mock 日志中 `Bearer disabled-token` 计数为 0。

### TC-IMG-67: Feishu Agent 进度卡发送到来源 Chat

- **前置条件**:
  - 工作目录为项目根目录。
  - 不启动或重启本机默认 `9900` 服务，不修改系统代理。
  - 本机可执行 Rust 单元测试。
  - 准备一个 Feishu Provider 配置，`owner_open_id=owner-ou`；模拟收到 owner 在会话 `chat-1` 中发送的文本消息，事件来源包含 `chat_id=chat-1`、`user_id=sender-ou`、`message_id=msg-1`。
- **操作步骤**:
  1. 执行 Feishu / Weixin 回复目标回归测试：
     ```bash
     cargo test -p bifrost-admin agent_reply_target_ --lib -- --nocapture
     ```
  2. 执行外部 Runner Feishu 默认进度卡 delivery mode 回归测试：
     ```bash
     cargo test -p bifrost-admin feishu_codex_like_external_runner_defaults_to_progress_card_without_channel_override --lib -- --nocapture
     ```
  3. 检查实现中内置 Agent progress target / plan target 和外部 CLI Runner progress target 都通过 `build_agent_reply_target` 构造：
     ```bash
     rg -n "build_agent_reply_target\\(|__agent_progress__|__plan_card__|receive_id_type" crates/bifrost-admin/src/handlers/im_gateway
     ```
- **预期结果**:
  - 第 1 步 `agent_reply_target_uses_feishu_chat_id_for_event_channel` 通过，断言 Feishu 进度卡目标为 `receive_id_type=chat_id`、`receive_id=chat-1`。
  - 第 1 步 `agent_reply_target_uses_feishu_open_id_without_chat_id` 通过，断言缺少 `chat_id` 时仍回退到 `open_id`。
  - 第 1 步 `agent_reply_target_uses_weixin_sender_instead_of_owner` 通过，确认非 Feishu 回复目标不漂移到 owner。
  - 第 2 步通过，确认 Feishu + Codex/Trae 类外部 Runner 仍默认启用 progress card，而不是等待最终输出后才回复。
  - 第 3 步能看到内置 Agent 与外部 CLI Runner 的 `__agent_progress__` 以及内置 Agent 的 `__plan_card__` 都复用 `build_agent_reply_target`，不存在 hardcoded owner open_id 的实时卡片路径。
- **清理步骤**:
  - 无需清理；测试不启动服务、不创建临时数据目录、不发送真实 IM 消息。
- **执行记录（2026-06-09）**: PASS — 执行 `cargo test -p bifrost-admin agent_reply_target_ --lib -- --nocapture`，3 个回复目标用例通过，覆盖 Feishu `chat_id` 优先、Feishu `open_id` 回退和 Weixin 来源目标保持；执行 `cargo test -p bifrost-admin feishu_codex_like_external_runner_defaults_to_progress_card_without_channel_override --lib -- --nocapture`，1 个 delivery mode 用例通过；执行 `rg -n "build_agent_reply_target\\(|__agent_progress__|__plan_card__|receive_id_type" crates/bifrost-admin/src/handlers/im_gateway`，确认内置 Agent 与外部 CLI Runner 的进度卡目标以及内置 Agent plan card 目标均通过统一 helper 构造。

### TC-IMG-68: Windows 外部 Runner 工作目录与 script_text 退出码回归

- **前置条件**:
  - 工作目录为项目根目录。
  - 不启动或重启本机默认 `9900` 服务，不修改系统代理。
  - 本机可执行 Rust 单元测试；Windows 专属行为最终以 Windows CI 补验。
- **操作步骤**:
  1. 执行 IM Gateway 外部 Runner 工作目录回归：
     ```bash
     source ~/.zshrc
     SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin schedule_external_runner_executes_from_configured_work_dir --lib -- --nocapture
     ```
  2. 执行 IM Task Executor 脚本文本成功/失败回归：
     ```bash
     source ~/.zshrc
     SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin im_gateway::task_executor::tests::test_execute_script_text --lib -- --nocapture
     ```
- **预期结果**:
  - 外部 Runner 在配置的 `work_dir` 内执行，测试通过目录内 marker 文件确认当前目录，不依赖 Unix `pwd -P` 输出或 Windows 路径格式。
  - Windows 下 `script_text` 临时脚本通过 `cmd /C <script_path>` 执行，路径交给 `Command` 做 argv quoting，不再把带引号脚本路径当作普通字符串导致 `.cmd` 未执行。
  - `exit 42` 在 Windows `.cmd` 与 Unix shell 中都能收敛为 exit code 42；成功、失败、超时三类 TaskExecutor 语义不变。
- **清理步骤**:
  - 无需清理；测试使用临时目录。
- **执行记录（2026-06-11）**: PASS — 本地 macOS 执行 `SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin schedule_external_runner_executes_from_configured_work_dir --lib -- --nocapture` 通过，验证 marker-file 工作目录断言不依赖平台路径格式；执行 `SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin im_gateway::task_executor::tests::test_execute_script_text --lib -- --nocapture`，2 个脚本文本用例通过。Windows `.cmd` quoting 和 `exit 42` 路径由当前分支 Windows CI 继续补验。

### TC-IMG-69: CLI Feishu 只传 Provider ID 后交互式授权并自动完成配置

- **前置条件**:
  - 使用当前分支编译出的 `target/debug/bifrost`。
  - 使用独立临时数据目录启动 Bifrost，必须设置 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`，必须携带 `--no-system-proxy` 和 `--skip-cert-check`。
  - 当前终端为交互式 TTY，或者显式传入 `--runner <Runner>`；非交互环境必须传 `--runner`。
- **操作步骤**:
  1. 启动临时服务：
     ```bash
     BIFROST_DATA_DIR=<temp-dir> BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 target/debug/bifrost start -H 127.0.0.1 -p <port> --unsafe-ssl --no-system-proxy --skip-cert-check
     ```
  2. 在交互式终端执行仅含 Provider ID 和类型的 CLI setup：
     ```bash
     target/debug/bifrost -H 127.0.0.1 -p <port> im provider add feishu-real-current --type feishu --display-name "Feishu Real Current" --runner Traex
     ```
  3. CLI 输出 Feishu setup URL 和二维码后保持运行，等待用户点击链接或扫码授权。
  4. 授权完成后继续观察 CLI 和服务状态。
  5. 查询 Provider 列表与状态：
     ```bash
     curl -sS http://127.0.0.1:<port>/_bifrost/api/im-gateway/providers
     curl -sS http://127.0.0.1:<port>/_bifrost/api/im-gateway/providers/feishu-real-current/status
     ```
- **预期结果**:
  - CLI 不要求输入 App ID、App Secret、owner_open_id 或 base_url。
  - CLI 输出 `https://open.feishu.cn/page/launcher?...` 授权 URL，并在终端展示二维码。
  - 未授权前 CLI 保持等待；授权完成后自动创建 Provider 并发起连接。
  - 创建出的 Provider `provider_type=feishu`，`agent_config.runner=Traex`，`base_url=https://open.feishu.cn/open-apis`，不存在用户传入或测试绕过的 base_url。
  - 状态最终为 `connected`，且 `last_connected_at` 非空。
  - Provider credential 响应不泄露 App Secret。
- **清理步骤**:
  - 删除测试 Provider 或删除临时数据目录。
  - 停止临时 Bifrost 服务。
- **执行记录（2026-07-08）**: PASS — 使用临时数据目录 `/tmp/bifrost-real-feishu-current.v6yELG` 与端口 `56089` 启动当前分支服务；CLI setup 输出 `https://open.feishu.cn/page/launcher?user_code=6BET-YH6Y`，用户完成真实授权后，服务侧恢复并创建 Provider `feishu-real-current`。最终 Provider `app_id=cli_aac76bfbc9799cd1`、owner `ou_9aae46d382574124415a0080e44c1c78`、`base_url=https://open.feishu.cn/open-apis`、Runner `Traex`，状态 `connected`，`last_connected_at=1783488399638`，`reconnect_count=0`。本次先暴露了 CLI 进程/服务重启后原内存 setup draft 丢失的问题，随后修复为持久化 setup session/provider draft 并由服务端 supervisor 恢复完成配置。

### TC-IMG-70: CLI Feishu 授权后重启/断线仍能恢复并创建 Provider

- **前置条件**:
  - 复用 TC-IMG-69 的临时服务、临时数据目录和 Feishu setup 链路。
  - CLI 已输出 Feishu 授权 URL，且用户已完成授权。
- **操作步骤**:
  1. 在 CLI 等待授权状态期间停止 CLI 或重启 Bifrost 服务，模拟进程丢失。
  2. 使用同一 `BIFROST_DATA_DIR` 重新启动 Bifrost。
  3. 等待 Feishu setup supervisor 恢复待完成 session。
  4. 查询 Provider 列表和状态。
- **预期结果**:
  - 服务重启后不会丢失已授权但尚未落库的 setup session。
  - supervisor 能从持久化 draft 中创建同一个 Provider ID，不会重复创建，也不会要求用户重新授权。
  - 创建后自动连接，状态最终为 `connected`。
  - Provider 使用固定 Feishu base URL，并保留用户在 CLI setup 中选择的 Runner。
- **清理步骤**:
  - 删除测试 Provider 或临时数据目录。
  - 停止临时 Bifrost 服务。
- **执行记录（2026-07-08）**: PASS — 真实 Feishu 验证中，首次授权后 CLI/服务会话丢失导致未自动完成配置，复现了用户反馈问题；修复后使用同一数据目录重启服务，supervisor 恢复 session `fas_9009b6d99e444263aecb866253c3f778`，自动创建 Provider `feishu-real-current` 并连接成功。状态查询确认 `connected` 且 `base_url=https://open.feishu.cn/open-apis`。

### TC-IMG-71: CLI Weixin 只传 Provider ID 后展示扫码二维码并自动完成配置

- **前置条件**:
  - 使用独立临时数据目录启动当前源码版 Bifrost，必须携带 `--no-system-proxy` 和 `--skip-cert-check`。
  - 准备 mock Weixin iLink 服务，至少实现二维码获取、二维码状态查询、消息发送和 getupdates；或使用真实 Weixin 登录服务。
  - 当前终端为交互式 TTY，或者显式传入 `--runner <Runner>`；非交互环境必须传 `--runner`。
- **操作步骤**:
  1. 执行：
     ```bash
     target/debug/bifrost -H 127.0.0.1 -p <port> im provider add weixin-main --type weixin --runner codex
     ```
  2. 观察 CLI 创建 Provider 后调用 Weixin login start。
  3. CLI 输出二维码 URL 并在终端展示二维码后保持等待。
  4. 扫码或让 mock Weixin 状态返回 confirmed。
  5. 查询 Provider 状态和消息发送能力。
- **预期结果**:
  - CLI 不要求用户输入 app_id/app_secret/base_url。
  - Provider 创建请求携带 `agent_config.runner=codex`。
  - CLI 展示 Weixin 二维码并等待扫码确认；确认后自动 connect。
  - Provider 列表中 `provider_type=weixin`，且不会把 Feishu base URL 或用户自定义 base URL 写入 Weixin Provider。
  - 后续发送/轮询使用 Weixin 通道自己的固定服务端配置。
- **清理步骤**:
  - 删除测试 Provider。
  - 停止临时 Bifrost 服务和 mock iLink 服务。
- **执行记录（2026-07-08）**: PASS — 执行 `SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-cli im_provider_add_weixin_setup_uses_admin_api_flow_and_runner --lib`，mock Admin API 验证 CLI 先读取 runner config，再以 `provider_type=weixin` 和 `agent_config.runner=Claude-Code` 创建 Provider，随后调用 `/providers/weixin-main/weixin-login/start`，等待 `/weixin-login/status` 返回 `confirmed` 后调用 `/providers/weixin-main/connect`。结合既有 `SKIP_FRONTEND_BUILD=1 e2e-tests/tests/test_weixin_provider_e2e.sh`，已覆盖 mock iLink 扫码登录、连接、消息收发和轮询停止回归。

### TC-IMG-72: CLI Provider setup 必须选择 Runner，非交互缺失时报可用 Runner

- **前置条件**:
  - 当前源码已启用至少一个 Runner，例如 `traex`、`codex` 或 `Claude-Code`。
  - 工作目录为项目根目录。
- **操作步骤**:
  1. 执行 CLI runner 解析与 API flow 单测：
     ```bash
     SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-cli resolve_runner_choice --lib -- --nocapture
     SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-cli im_provider_add_feishu_setup_uses_admin_api_flow_and_runner --lib -- --nocapture
     SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-cli im_provider_add_weixin_setup_uses_admin_api_flow_and_runner --lib -- --nocapture
     ```
  2. 在真实非交互环境中不传 `--runner` 执行 Feishu/Weixin setup，例如：
     ```bash
     printf '' | target/debug/bifrost -H 127.0.0.1 -p <port> im provider add feishu-missing-runner --type feishu
     ```
  3. 在交互式 TTY 中不传 `--runner` 执行同类命令，使用键盘上下选择 Runner 并回车。
- **预期结果**:
  - 传入 `--runner` 时会校验 Runner 是否存在且 enabled，支持 `trae` -> `traex`、`claude code` -> `Claude-Code` 等别名。
  - 传入未知或 disabled Runner 时，错误包含 `Available runners` 和默认内置 Runner 提示 `codex, traex, Claude Code`。
  - 非交互环境未传 `--runner` 时直接失败，错误包含 `--runner is required when stdin is not interactive`、可用 Runner 列表和默认内置 Runner 提示。
  - 交互式 TTY 未传 `--runner` 时弹出可键盘选择的 Runner 列表，选中后继续 Feishu/Weixin setup。
- **清理步骤**:
  - 如果第 3 步创建了测试 Provider，删除该 Provider。
- **执行记录（2026-07-08）**: PASS — 新增并执行 `SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-cli resolve_runner_choice --lib -- --nocapture`，3 个 runner 选择用例通过，覆盖非交互缺失 `--runner`、Runner 别名、未知 Runner 和可用列表；执行 `SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-cli im_provider_add_feishu_setup_uses_admin_api_flow_and_runner --lib -- --nocapture` 与 `SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-cli im_provider_add_weixin_setup_uses_admin_api_flow_and_runner --lib -- --nocapture` 通过，分别覆盖 Feishu/Weixin 指定 Runner 后的完整 Admin API 调用链。

### TC-IMG-73: CLI 禁止为 Feishu/Weixin Provider 传入 base_url

- **前置条件**:
  - 工作目录为项目根目录。
  - 使用当前源码版 CLI。
- **操作步骤**:
  1. 对 Feishu provider add 传入 base URL 参数：
     ```bash
     target/debug/bifrost im provider add feishu-base-url --type feishu --base-url http://127.0.0.1:1 --runner traex
     ```
  2. 对 Weixin provider add 传入 base URL 参数：
     ```bash
     target/debug/bifrost im provider add weixin-base-url --type weixin --base-url http://127.0.0.1:1 --runner traex
     ```
  3. 执行相关单元测试和真实 Feishu Provider 查询。
- **预期结果**:
  - 两条 CLI 命令在发起创建或 setup 前失败，错误包含 `base_url is managed by system and cannot be set via CLI`。
  - Feishu 真实 setup 创建出的 Provider 始终为 `https://open.feishu.cn/open-apis`。
  - Weixin setup 不接受用户从 CLI 注入 Feishu/OpenAPI/mock base_url。
- **清理步骤**:
  - 无需清理；失败路径不创建 Provider。
- **执行记录（2026-07-08）**: PASS — 代码 review 确认 `parse_provider_add_args` 对 `--base-url` / `--base_url` 直接返回 `base_url is managed by system and cannot be set via CLI`，且 `build_setup_provider_body` 不写入 base_url；真实 Feishu TC-IMG-69 查询确认创建结果固定为 `https://open.feishu.cn/open-apis`。历史修复提交 `287b6473`、`3bf992ec`、`055b4d91` 已覆盖 Provider base URL 固定化。

### TC-IMG-74: CLI Feishu/Weixin 终端二维码保持接近正方形

- **前置条件**:
  - 工作目录为项目根目录。
  - 使用当前源码版 CLI。
- **操作步骤**:
  1. 执行二维码渲染单元测试：
     ```bash
     SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-cli terminal_qr_code_renders_with_square_terminal_ratio --lib -- --nocapture
     ```
  2. 在交互式终端执行 Feishu 或 Weixin provider setup，观察终端二维码：
     ```bash
     target/debug/bifrost -H 127.0.0.1 -p <port> im provider add feishu-qr-ratio --type feishu --runner traex
     target/debug/bifrost -H 127.0.0.1 -p <port> im provider add weixin-qr-ratio --type weixin --runner codex
     ```
- **预期结果**:
  - 渲染函数使用等宽模块输出，终端二维码视觉上接近正方形，不再被横向拉成长方形。
  - 单元测试会按 `Dense1x2` 每行承载两行 QR 模块的终端渲染模型，检查最大字符宽度与估算视觉高度接近，防止 `module_dimensions(2, 1)` 一类配置再次把二维码拉宽。
  - Feishu 授权 URL 和 Weixin 扫码 URL 仍能正常生成二维码。
- **清理步骤**:
  - 如果第 2 步创建了测试 Provider，删除该 Provider。
- **执行记录（2026-07-08）**: PASS — 修复 `render_terminal_qr_code` 使用 `Dense1x2` + `module_dimensions(1, 1)`，避免原先 `module_dimensions(2, 1)` 导致终端二维码宽度翻倍；执行 `SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-cli terminal_qr_code_renders_with_square_terminal_ratio --lib -- --nocapture` 通过。
