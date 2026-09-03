# 远程访问管理 Web UI 测试用例

## 前置条件

1. 启动 Bifrost 服务（使用临时数据目录避免污染正式环境）：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test \
   BIFROST_DISABLE_TRAY=1 \
   BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 \
   cargo run --bin bifrost -- start -p 8800 --unsafe-ssl --no-system-proxy
   ```
2. 准备一台同局域网的设备，或使用本机的局域网 IP（如 `192.168.8.31`）
3. 确保端口 8800 未被防火墙阻止

---

## 测试用例

### TC-RA-01：localhost 访问管理端无需登录

**操作步骤**：
1. 在浏览器中打开 `http://127.0.0.1:8800/_bifrost/`

**预期结果**：
- 直接进入管理端 Traffic 页面，无登录提示
- URL 为 `http://127.0.0.1:8800/_bifrost/traffic`

---

### TC-RA-02：Remote Access Tab 显示及初始状态

**操作步骤**：
1. 在浏览器中打开 `http://127.0.0.1:8800/_bifrost/settings?tab=remote`

**预期结果**：
- Settings 页面显示 "Remote Access" Tab
- Remote Access Status 卡片显示：
  - Remote Access: `Disabled`（灰色标签）
  - Admin Username: `admin`
  - Password: `Not Set`（橙色标签）
- Enable Remote Admin Access 开关处于关闭状态
- 底部有蓝色提示 "Set a password to enable remote admin access"

---

### TC-RA-03：设置管理端用户名和密码

**操作步骤**：
1. 在 Remote Access Tab 中，Username 输入框填入 `admin`
2. New Password 输入框填入 `test123456`
3. Confirm Password 输入框填入 `test123456`
4. 点击 "Save Credentials" 按钮

**预期结果**：
- 显示 Toast 消息 "Password updated"
- Password 状态从 `Not Set` 变为 `Set`（绿色标签）
- 密码输入框被清空

---

### TC-RA-04：密码不匹配时拒绝保存

**操作步骤**：
1. New Password 填入 `abc123`
2. Confirm Password 填入 `abc456`
3. 点击 "Save Credentials"

**预期结果**：
- 显示警告消息 "Passwords do not match"
- 密码未被修改

---

### TC-RA-05：无密码时禁止开启远程访问

**前置条件**：新启动的服务，未设置密码

**操作步骤**：
1. 直接点击 "Enable Remote Admin Access" 开关

**预期结果**：
- 显示错误消息 "Cannot enable remote access without setting a password first"
- 开关保持关闭状态

---

### TC-RA-06：有密码后开启远程访问

**前置条件**：已通过 TC-RA-03 设置密码

**操作步骤**：
1. 点击 "Enable Remote Admin Access" 开关

**预期结果**：
- 显示 Toast 消息 "Remote access enabled"
- Remote Access 状态变为 `Enabled`（绿色标签）

---

### TC-RA-07：远程 IP 访问根路径自动重定向到登录页

**前置条件**：已开启远程访问

**操作步骤**：
1. 在浏览器中打开 `http://192.168.8.31:8800/`

**预期结果**：
- 自动重定向到 `http://192.168.8.31:8800/_bifrost/login?next=...`
- 显示登录页面，标题为 "Bifrost Admin"
- 显示 "远程管理访问 / 鉴权登录"
- 有用户名和密码输入框

---

### TC-RA-08：远程 IP 使用正确密码登录

**操作步骤**：
1. 在远程登录页面，输入用户名 `admin`，密码 `test123456`
2. 点击 "登录" 按钮

**预期结果**：
- 登录成功，跳转到 `http://192.168.8.31:8800/_bifrost/traffic`
- 显示完整的管理端 Traffic 页面
- 可以正常浏览和操作管理端所有功能（Traffic、Rules、Settings 等）

---

### TC-RA-09：远程 IP 使用错误密码登录

**操作步骤**：
1. 在远程登录页面，输入用户名 `admin`，密码 `wrongpassword`
2. 点击 "登录" 按钮

**预期结果**：
- 登录失败
- 显示错误提示（如 "Invalid username or password"）
- 停留在登录页面

---

### TC-RA-10：开启远程访问后 localhost 仍无需登录

**前置条件**：远程访问已开启

**操作步骤**：
1. 在浏览器中打开 `http://127.0.0.1:8800/_bifrost/`

**预期结果**：
- 直接进入管理端 Traffic 页面，**无需登录**
- localhost/127.0.0.1 始终免鉴权

---

### TC-RA-11：吊销所有会话

**前置条件**：远程 IP 已通过 TC-RA-08 登录

**操作步骤**：
1. 通过 localhost 打开 `http://127.0.0.1:8800/_bifrost/settings?tab=remote`
2. 在 "Session Management" 区域点击 "Revoke All Sessions" 按钮
3. 在确认弹窗中点击 "Revoke"

**预期结果**：
- 显示 Toast 消息 "All sessions revoked"

---

### TC-RA-12：吊销后远程 token 失效

**前置条件**：已通过 TC-RA-11 吊销会话

**操作步骤**：
1. 在远程 IP 浏览器中刷新管理端页面（或访问 `http://192.168.8.31:8800/_bifrost/`）

**预期结果**：
- 被重定向回登录页面
- 需要重新输入用户名密码登录
- 之前的 JWT token 已失效

---

### TC-RA-13：关闭远程访问

**操作步骤**：
1. 通过 localhost 打开 Settings → Remote Access Tab
2. 点击 "Enable Remote Admin Access" 开关关闭

**预期结果**：
- 显示 Toast 消息 "Remote access disabled"
- Remote Access 状态变为 `Disabled`
- 远程 IP 无法再访问管理端（连接被拒绝或无响应）

---

### TC-RA-14：Session Management 卡片展示登录记录表格

**前置条件**：已通过 TC-RA-08 远程登录至少一次

**操作步骤**：
1. 通过 localhost 打开 `http://127.0.0.1:8800/_bifrost/settings?tab=remote`
2. 滚动到 "Session Management" 卡片

**预期结果**：
- Session Management 卡片中有一个表格，包含以下列：Time、IP、Username、User Agent
- 表格中至少有 1 条登录记录
- 登录记录中的 IP 为远程登录时的客户端 IP（如 `192.168.8.31`）
- 登录记录中的 Username 为 `admin`
- Time 列显示可读的日期时间格式，鼠标悬停显示 ISO 格式
- User Agent 列显示浏览器 User Agent 信息
- 表格底部显示总记录数（如 "1 records"）

---

### TC-RA-15：Session Management 登录记录分页

**前置条件**：通过远程 IP 多次登录（超过 10 次）

**操作步骤**：
1. 通过 localhost 打开 `http://127.0.0.1:8800/_bifrost/settings?tab=remote`
2. 在 Session Management 表格底部查看分页控件

**预期结果**：
- 每页显示 10 条记录
- 可以通过分页控件翻页
- 总记录数正确反映所有登录次数
- 记录按时间倒序排列（最新的在最前面）

---

### TC-RA-16：Session Management 刷新按钮

**操作步骤**：
1. 通过 localhost 打开 `http://127.0.0.1:8800/_bifrost/settings?tab=remote`
2. 在 Session Management 卡片右上角点击 "Refresh" 按钮

**预期结果**：
- 表格显示加载状态
- 数据重新加载后展示最新的登录记录
- 如果有新的登录事件发生，刷新后可见

---

### TC-RA-17：吊销所有会话后登录记录仍保留

**前置条件**：已有至少 1 条登录记录

**操作步骤**：
1. 通过 localhost 打开 Settings → Remote Access Tab
2. 点击 "Revoke All Sessions" 并确认
3. 查看 Session Management 表格

**预期结果**：
- 显示 "All sessions revoked" 消息
- 登录记录表格仍然保留之前的记录（审计日志不会被清除）
- 记录数不变

---

### TC-RA-18：远程登录后所有浏览器原生流式连接正常鉴权

**前置条件**：已开启远程访问，并通过局域网 IP 完成登录

**操作步骤**：
1. 使用浏览器打开 `http://<局域网IP>:8800/_bifrost/traffic`
2. 在开发者工具 Network 中筛选 `stream` 和 `WS`
3. 确认主实时通道 `/_bifrost/api/push` 完成 WebSocket `101 Switching Protocols`
4. 确认 `/_bifrost/api/whitelist/pending/stream` 和 `/_bifrost/api/config/ip-tls/pending/stream` 返回 `200`，且保持连接
5. 分别打开一条 WebSocket 流量和一条 SSE 流量的 Messages 详情，确认对应 `frames/stream` 与 `sse/stream` 不返回 `401`
6. 打开 DevTools、Replay WebSocket 和 ASR 实时语音入口，确认各自 WebSocket 不因缺少鉴权而返回 `401`
7. 产生一条新的代理请求，观察 Network 列表无需刷新即可出现该请求
8. 在本机执行 `bifrost admin revoke-all`，然后刷新远程页面

**预期结果**：
- 登录响应签发路径限定为 `/_bifrost` 的 `HttpOnly; SameSite=Strict` 会话 Cookie
- 普通 REST、所有 EventSource 和所有 WebSocket 管理端入口均可使用同源会话 Cookie 鉴权
- Network 列表通过 `/_bifrost/api/push` 实时新增流量，不再出现原问题中的 `401`
- 原有 Bearer JWT API 调用继续可用，localhost/127.0.0.1 免登录行为不变
- `revoke-all` 后旧 Bearer token 与旧会话 Cookie 均失效，远程页面返回登录页

---

## 清理

测试完成后清理临时数据：
```bash
rm -rf .bifrost-test
```
