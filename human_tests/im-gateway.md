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
  - 第 6 步消息记录包含一条 `direction=outbound`、`trigger=online`、`status=success` 的 owner 通知，`content_preview` 以 `你好，Bifrost 助手上线了` 开头，并包含 `工作目录：/Users/eden/work/github/bifrost`。
  - 全流程不需要重启 Bifrost。
  - Provider 列表与消息响应不包含 App Secret 明文。
- **执行记录（2026-05-06）**: PASS — 使用临时端口 `18888` 源码服务和用户提供的真实飞书 AK/SK 通过 WebUI 创建 Provider；页面显示 `Provider created and connected`；未重启服务即查询到状态 `connected`；message log 包含 `trigger=online`、`status=success`、`content_preview=你好，Bifrost 助手上线了` 的 owner 通知；响应中未泄露 App Secret；最后删除清理成功。本用例后续要求上线通知同时包含 `工作目录：/Users/eden/work/github/bifrost`。

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
  - 两个 Provider 的消息记录都各自包含一条 `direction=outbound`、`trigger=online`、`status=success` 的 owner 通知，且 `content_preview` 以 `你好，Bifrost 助手上线了` 开头，并包含 `工作目录：/Users/eden/work/github/bifrost`。
  - 两个 Provider 不会串用对方的飞书 token；第二个机器人不会因复用第一个机器人的 token 而发送失败。
  - Provider 列表、状态与消息响应不包含任何 App Secret 明文。
- **执行记录（2026-05-06）**: PASS — 使用临时端口 `18889` 和独立数据目录 `.bifrost-test-im-provider-two-bots` 启动源码版 Bifrost；通过 Settings / IM Gateway WebUI 分别创建两个真实飞书 Provider；两个 Provider 均显示创建并连接成功，状态均为 `connected`；两个 Provider 的 message log 均包含 `direction=outbound`、`trigger=online`、`status=success` 的 owner 通知，`content_preview` 为 `你好，Bifrost 助手上线了\n工作目录：/Users/eden/work/github/bifrost`；第二个机器人未复用第一个机器人的 token，响应中未泄露 App Secret；最后删除清理两个 Provider。

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
  - owner 通知的 `content_preview` 以 `你好，Bifrost 助手上线了` 开头，并包含 `工作目录：/tmp/bifrost-im-provider-custom-workdir`。
  - owner 通知不得回退为全局 Agent Working Directory 或 Bifrost 进程 cwd。
  - Provider 列表与消息响应不包含 App Secret 明文。
- **执行记录（2026-05-06）**: PASS — 使用临时端口 `18890` 和独立数据目录 `.bifrost-test-im-provider-custom-workdir` 启动源码版 Bifrost；通过 Settings / IM Gateway WebUI 创建真实飞书 Provider，并在 `Agent Working Directory` 填写 `/tmp/bifrost-im-provider-custom-workdir`；页面显示 `Provider created and connected`，Provider 状态为 `connected`；message log 包含 `direction=outbound`、`trigger=online`、`status=success` 的 owner 通知，且 `content_preview` 包含 `工作目录：/tmp/bifrost-im-provider-custom-workdir`，未回退到 `/Users/eden/work/github/bifrost`；响应中未泄露 App Secret；最后删除 Provider 清理成功。

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
  - fake Feishu 收到的请求包含 `receive_id_type=open_id`、`receive_id=owner-open-id`、`msg_type=text`，文本内容为 `hello owner from cli`。
  - CLI 输出包含 `Message sent` 与 fake Feishu 返回的 message id。
  - `im messages list` 未传 `--provider` 时复用 provider 选择逻辑，输出包含 `Owner` 与消息内容预览。
  - 多 Provider 交互式场景下，CLI 展示 provider 列表；多 Provider 非交互式场景下返回明确错误，要求传 `--provider`。
- **执行记录（2026-05-06）**: PASS — 使用 `e2e-tests/tests/test_im_cli_provider_selection_send_owner.sh` 执行 TC-IMG-37；脚本用临时数据目录 `.bifrost-test-im-cli-provider`、端口 `18891` 和 fake Feishu OpenAPI 服务启动源码版 Bifrost；创建唯一 enabled Provider 后执行 `bifrost im send --text 'hello owner from cli'`，CLI 自动选择 `feishu-main` 并输出 `Message sent via provider 'feishu-main' to __owner__ (message_id: om_owner_cli)`；fake Feishu 捕获 `receive_id_type=open_id`、`receive_id=owner-open-id`、`msg_type=text` 和文本内容；`bifrost im messages list` 未传 `--provider` 时同样自动选择 Provider，输出包含 `Owner` 与消息内容预览；脚本最后清理临时数据和进程。

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
  - 已通过 API 创建一个 Feishu Provider 和一个 Target，Target id 为 `schedule-target`。
- **操作步骤**:
  1. 调用 `POST /_bifrost/api/im-gateway/schedules` 创建 script schedule，body 包含 `task_type=script`、`target_id=schedule-target`、`trigger.type=interval`、`script.script_text=echo script-ok`。
  2. 调用同一 API 创建 agent schedule，body 包含 `task_type=agent`、`trigger.type=interval`、`agent.prompt=Summarize schedule state`。
  3. 调用 `GET /_bifrost/api/im-gateway/schedules` 查看列表。
  4. 调用 `POST /_bifrost/api/im-gateway/schedules/<script-id>/run` 手动触发 script schedule。
  5. 调用 `GET /_bifrost/api/im-gateway/schedules/<script-id>/runs` 查看 run history。
- **预期结果**:
  - 两次创建都返回完整 schedule JSON，而不是只有 `{success:true}`。
  - Script schedule 保留 `task_type=script`、`script.script_text` 和 `next_run_at`。
  - Agent schedule 保留 `task_type=agent`、`agent.prompt` 和 `next_run_at`。
  - Script 手动 run 返回 `status=Success`，`stdout_preview` 包含 `script-ok`。
  - Run history 中能查到该手动执行记录。
- **执行记录（2026-05-12）**: PASS — 使用临时数据目录 `.bifrost-test-im-schedules`、端口 `18892`、`--no-system-proxy` 启动源码版 Bifrost；通过 API 创建 `schedule-provider` 与 `schedule-target` 后，`POST /schedules` 创建 `script-schedule` 返回完整 schedule JSON，包含 `task_type=script`、`script.script_text=echo script-ok` 与 `next_run_at`；创建 `agent-schedule` 返回 `task_type=agent`、`agent.prompt=Summarize schedule state` 与 `next_run_at`；`GET /schedules` 同时列出两类任务；`POST /schedules/script-schedule/run` 返回 `status=Success`、`exit_code=0`、`stdout_preview=script-ok\n`；`GET /schedules/script-schedule/runs` 返回对应 `manual_run` 记录。

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

### TC-IMG-47: WebUI IM Gateway 窄宽度布局与 Task Runs 详情回归

- **前置条件**:
  - 使用源码版 Bifrost 或 Vite dev server 打开 `/_bifrost/ai?aiSection=im-gateway-connections&imGatewaySection=connections`。
  - 浏览器视口宽度调整到约 760px，确保至少存在一个 provider、target、route、schedule 和一条 task run 历史记录。
- **操作步骤**:
  1. 进入 Connections，查看 provider 卡片中的 Status、App ID、Connection Mode、Agent Work Dir、Agent Base Prompt、Agent Developer/User 等字段。
  2. 进入 Targets，查看 targets 表格在窄宽度下的表头、Receive ID 和 Actions。
  3. 进入 Routes，查看 route 卡片中的 Provider、Event、Matcher、Action、Timeout。
  4. 进入 Schedules，查看 schedules 表格并横向滚动到 Actions，点击某一行打开 schedule detail。
  5. 进入 History / Task Runs，点击一条 Task Run 行。
- **预期结果**:
  - Connections 与 Routes 卡片字段自适应换行到多行网格，不出现 `Long Connection`、`Global default` 等逐字竖排。
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
  - Script 定时任务必须包含非空 `target_id`，后端对缺少 `target_id` 的 script schedule 返回结构化错误。
  - Script 定时任务被创建后可通过 `schedule_update` 修改名称、启用状态、interval、脚本文本和 timeout。
  - Agent 定时任务被创建后保留 `task_type=agent` 与 preset prompt。
  - `schedule_list` 和 schedules API 均能同时看到 script 与 agent 两类任务。
- **执行记录（2026-05-13）**: PASS — 使用 mock Chat Completions 服务端口 `28992` 与源码版 Bifrost 端口 `18892` 完成真实 `/agent/chat` 链路验证；第一次请求故意让 script schedule 缺少 `target_id`，工具返回 `script schedules require target_id`，确认校验生效；清理后第二次请求让 mock model 依次调用 `schedule_create` script、`schedule_create` agent、`schedule_update` script、`schedule_list`，四个 `tool_calls` 均 `success=true`；最终 `GET /_bifrost/api/im-gateway/schedules` 显示 `chat-script-schedule` 已更新为 `Chat Script Schedule Updated`、`enabled=false`、`every_ms=180000`、`script.script_text=echo chat-script-v2`、`timeout_ms=60000`，同时 `chat-agent-schedule` 保留 `task_type=agent`、`agent.prompt=Run the chat-created agent schedule`、`every_ms=120000`、`timeout_ms=45000`。

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
