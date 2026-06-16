# 远程调用（Remote Invoke）测试用例

## 功能模块说明

Remote Invoke 允许调用方通过 `bifrost remote` 命令，经由本地 relay 服务把只读查询命令转发到目标 Bifrost 客户端执行。完整链路包含：

- 调用方 -> relay：HTTP + SSE
- relay -> Bifrost 客户端：SSE
- Bifrost 客户端 -> relay：HTTP
- 本地 WebUI Settings -> Remote Invoke：发现模式、一次性授权码、人工授权、授权管理、多客户端管理、历史记录

本测试文档覆盖以下关键能力：

- 首次配对与人工授权
- 发现模式与一次性授权码生命周期
- 可复用授权命中与免重复授权
- 持久化 caller identity 与 caller_fingerprint 稳定性
- SSH 首次授权、绑定复用、冲突拒绝与撤销
- 授权有效期调整与移除
- 授权模式升降级（一次性→限时、永久→限时）
- 多客户端在线管理与目标客户端选择
- 仅允许白名单查询命令（status / traffic get / traffic search / search）
- `shell.exec` 白名单命令、scope、auth_method、timeout 与输出限制
- SSE/HTTP relay 正常转发、大结果流式传输与断线恢复
- 调用方主动取消与 SSE 续流恢复
- 端到端加密验证（Relay 不可见明文）
- 第一阶段 relay v2 协议升级：`command_encrypted` / `exit_encrypted` / route-only metadata
- 客户端重启后 instance_id 稳定性
- 审计历史、摘要脱敏、过滤查询、数据保留与清理
- 远端部署场景：HTTPS 安全传输、SSO 鉴权、多用户并发、跨公网稳定性

## 前置条件

1. 启动本地 Bifrost 服务（使用临时数据目录，避免污染正式环境）：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- start -p 8800 --unsafe-ssl --no-system-proxy
   ```
2. 启动本地 relay 服务 `bifrost-sync-server`（使用独立临时数据目录）：
   ```bash
   pnpm --dir packages/bifrost-sync-server dev -- --data-dir <REPO_ROOT>/.bifrost-sync-test -p 8686
   ```
3. 在 relay 服务中注册测试用户并拿到 token：
   ```bash
   curl -s -X POST http://127.0.0.1:8686/v4/sso/register \
     -H "Content-Type: application/json" \
     -d '{"user_id":"remote_test","password":"remote_test_123","nickname":"Remote Test"}' | jq .
   ```
4. 如需单独登录获取 token，可执行：
   ```bash
   curl -s -X POST http://127.0.0.1:8686/v4/sso/login \
     -H "Content-Type: application/json" \
     -d '{"user_id":"remote_test","password":"remote_test_123"}' | jq .
   ```
5. 记录登录返回的 token，并导出为环境变量：
   ```bash
   export SYNC_TOKEN="<login_or_register_response_token>"
   ```
6. 打开本地管理端 Settings 页面，预期 Remote Invoke Tab 路径为：
   ```text
   http://127.0.0.1:8800/_bifrost/settings?tab=remote-invoke
   ```
7. 以下命令以设计方案中的 CLI 形态为基准；若实现时命令行参数有差异，需同步更新本文档：
   ```bash
   cargo run --bin bifrost -- remote status --relay http://127.0.0.1:8686 --token "$SYNC_TOKEN"
   ```

---

## 测试用例

### TC-RI-01：Remote Invoke Tab 初始状态

**操作步骤**：
1. 在浏览器中打开 `http://127.0.0.1:8800/_bifrost/settings?tab=remote-invoke`

**预期结果**：
- 页面存在 `Remote Invoke` Tab
- 状态区显示 relay 连接状态、客户端实例 ID、当前授权数、历史记录入口
- 页面存在 Clients 区
- 初始状态下发现模式为关闭
- 初始状态下未生成一次性授权码
- Active Grants 区为空
- History 区为空或显示 `0 records`

---

### TC-RI-02：开启发现模式并生成一次性授权码

**操作步骤**：
1. 在 `Remote Invoke` Tab 的目标客户端上点击 “Enter Discovery Mode”

**预期结果**：
- 页面显示发现模式已开启
- 页面显示一个 6 位数字一次性授权码
- 页面显示剩余有效时间倒计时
- 配对码与当前客户端实例关联
- 未出现重复生成多个同时有效配对码的异常

---

### TC-RI-03：配对码过期后失效

**前置条件**：已生成配对码

**操作步骤**：
1. 等待配对码自然过期
2. 使用已过期的配对码执行：
   ```bash
   cargo run --bin bifrost -- remote status --relay http://127.0.0.1:8686 --token "$SYNC_TOKEN" --pair-code <expired_code>
   ```

**预期结果**：
- 命令失败
- 输出提示配对码已过期或无效
- 管理端不出现新的待授权请求
- 发现模式自动关闭，原授权码不再展示

---

### TC-RI-04：首次远程查询触发待授权请求

**前置条件**：当前用户在该客户端上无可复用授权

**操作步骤**：
1. 生成新的配对码
2. 执行：
   ```bash
   cargo run --bin bifrost -- remote status --relay http://127.0.0.1:8686 --token "$SYNC_TOKEN" --pair-code <valid_code>
   ```

**预期结果**：
- CLI 进入等待授权状态
- 管理端出现新的 Pending Request
- relay 为本次请求创建 pairing 记录

---

### TC-RI-05：全局授权弹窗展示关键信息

**前置条件**：已触发待授权请求

**操作步骤**：
1. 观察管理端全局授权弹窗

**预期结果**：
- 弹窗展示调用方设备指纹简写
- 弹窗展示来源 IP / User-Agent / 请求时间
- 弹窗展示命令摘要，例如 `status` 或 `traffic get 57544`
- 弹窗提供：拒绝、仅本次、30 分钟、1 小时、1 天、永久 等按钮

---

### TC-RI-06：拒绝授权

**前置条件**：已触发待授权请求

**操作步骤**：
1. 在弹窗中点击 “拒绝”

**预期结果**：
- 调用方 CLI 收到 `rejected` 或等效错误
- 不创建可复用授权记录
- History 中新增一条拒绝事件

---

### TC-RI-07：一次调用授权后命令成功返回

**操作步骤**：
1. 重新发起 `remote status` 请求
2. 在弹窗中选择 “仅本次”

**预期结果**：
- CLI 成功输出本地 Bifrost 的状态结果
- 返回内容来自目标客户端实际查询结果
- History 中记录本次调用成功、退出状态与命令摘要
- Active Grants 中可见一次性授权，或该授权在调用完成后立即变为已消费

---

### TC-RI-08：一次性授权不会被下次命令复用

**前置条件**：已完成 TC-RI-07

**操作步骤**：
1. 再次执行：
   ```bash
   cargo run --bin bifrost -- remote status --relay http://127.0.0.1:8686 --token "$SYNC_TOKEN"
   ```

**预期结果**：
- CLI 不能直接复用上次授权
- 系统要求重新提供配对码并等待人工授权
- 管理端再次出现新的 Pending Request

---

### TC-RI-09：30 分钟授权可被后续命令直接复用

**操作步骤**：
1. 重新生成配对码并发起：
   ```bash
   cargo run --bin bifrost -- remote traffic get 57544 --relay http://127.0.0.1:8686 --token "$SYNC_TOKEN" --pair-code <valid_code>
   ```
2. 在弹窗中选择 “30 分钟”
3. 首次命令成功后，再执行一次：
   ```bash
   cargo run --bin bifrost -- remote status --relay http://127.0.0.1:8686 --token "$SYNC_TOKEN"
   ```

**预期结果**：
- 第二次调用无需重新配对
- CLI 能直接命中可复用授权
- Active Grants 中显示该授权仍为 `active`
- `last_used_at` 更新

---

### TC-RI-10：永久授权可跨 CLI 进程复用

**操作步骤**：
1. 重新生成配对码并发起远程查询
2. 在弹窗中选择 “永久”
3. 退出当前终端会话
4. 新开一个终端再次执行：
   ```bash
   cargo run --bin bifrost -- remote status --relay http://127.0.0.1:8686 --token "$SYNC_TOKEN"
   ```

**预期结果**：
- 新终端中的 CLI 仍能直接复用该授权
- 不需要重新输入配对码
- relay 用户数据库中存在该授权记录

---

### TC-RI-回归-51：回归 — Remote Invoke 状态区合并布局在宽屏下保持高效占位

**操作步骤**：
1. 在桌面宽屏浏览器中打开 `http://127.0.0.1:8800/_bifrost/settings?tab=remote-invoke`
2. 观察首屏顶部两列卡片布局
3. 确认左侧卡片内同时存在 `Connection Status` 与 `Discovery Mode` 两个区块
4. 确认右侧 `SSH Key` 卡片与左侧状态卡片处于同一行展示
5. 观察 `Enter Discovery Mode` 按钮或发现模式操作区是否仍然清晰可点击

**预期结果**：
- 左侧顶部只展示一张状态总览卡片，不再将 `Connection Status` 与 `Discovery Mode` 拆成上下两张独立卡片
- 合并后的状态卡片内清晰分区展示连接状态与发现模式
- 宽屏首屏下左右卡片空间利用更均衡，不出现原先左下大块空白
- `SSH Key` 卡片仍保持完整展示，不与左侧卡片内容重叠或挤压
- 发现模式入口按钮或操作按钮仍可正常识别与点击

---

### TC-RI-回归-52：回归 — Create SSH key 弹窗提示信息合并后仍准确表达约束

**操作步骤**：
1. 打开 `http://127.0.0.1:8800/_bifrost/settings?tab=remote-invoke`
2. 在 `SSH Key` 卡片中点击 `Create SSH Key`
3. 观察弹窗顶部提示区域
4. 确认表单中只保留一条蓝色提示信息，不再重复出现第二条说明卡片

**预期结果**：
- 弹窗顶部仅展示一条合并后的提示信息
- 提示中同时说明“仅支持一个 active SSH key”以及“SSH grant 只有 rotate 或 revoke 才会失效”
- 表单标签与输入框位置不受影响，`Label` 输入仍可正常编辑
- 弹窗底部 `Cancel` / `Create` 按钮布局不变

---

### TC-RI-11：调用方设备指纹变化时不得复用旧授权

**前置条件**：已存在一个永久或限时授权

**操作步骤**：
1. 模拟更换调用方设备指纹（例如切换本地 device_id 或测试配置）
2. 再次执行只读查询命令

**预期结果**：
- 系统不能命中原有授权
- 需要重新配对和人工授权
- 原授权仍保留给旧指纹，不会被错误复用到新指纹

---

### TC-RI-12：白名单外的查询命令被拒绝

**操作步骤**：
1. 执行一个未进入白名单的命令，例如：
   ```bash
   cargo run --bin bifrost -- remote rule list --relay http://127.0.0.1:8686 --token "$SYNC_TOKEN"
   ```

**预期结果**：
- CLI 或 relay 明确拒绝该命令
- 不进入执行阶段
- 管理端不会展示“可执行”的授权选项，或直接标记为不支持

---

### TC-RI-13：配置修改类命令必须被拒绝

**操作步骤**：
1. 执行一个写操作命令，例如：
   ```bash
   cargo run --bin bifrost -- remote config set server.port 9999 --relay http://127.0.0.1:8686 --token "$SYNC_TOKEN"
   ```

**预期结果**：
- 命令被拒绝
- Bifrost 本地配置未被修改
- History 中记录一次被拒绝的高风险请求

---

### TC-RI-14：管理端可缩短授权有效期并对后续命令生效

**前置条件**：已存在一个有效的 `1 天` 或 “永久” 授权

**操作步骤**：
1. 在 `Active Grants` 中找到目标授权
2. 将有效期修改为较短值，例如改为 1 分钟
3. 等待该授权过期
4. 再次执行只读查询命令

**预期结果**：
- 管理端显示授权有效期已更新
- 授权到期后，新命令不能再直接复用
- 系统要求重新配对授权

---

### TC-RI-15：管理端可延长授权有效期

**前置条件**：已存在一个即将到期的限时授权

**操作步骤**：
1. 在 `Active Grants` 中把该授权从 30 分钟改为 1 天
2. 再次执行只读查询命令

**预期结果**：
- 新命令仍可直接复用该授权
- 管理端与 relay 数据库中的 `expires_at` 同步更新
- History 中新增 `grant_updated` 事件

---

### TC-RI-16：管理端移除授权后必须重新配对

**前置条件**：已存在一个可复用授权

**操作步骤**：
1. 在 `Active Grants` 中点击 “Remove” 或 “Delete Grant”
2. 确认移除
3. 再次执行只读查询命令

**预期结果**：
- 授权从 Active Grants 列表消失
- 新命令无法直接复用旧授权
- 必须重新生成配对码并重新授权
- History 中新增 `grant_removed` 或 `grant_revoked` 事件

---

### TC-RI-17：relay 重启后可复用授权仍然有效

**前置条件**：已存在一个有效的可复用授权

**操作步骤**：
1. 停止 `bifrost-sync-server`
2. 确认临时数据目录 `<REPO_ROOT>/.bifrost-sync-test/bifrost-sync.db` 仍保留
3. 重新启动 `bifrost-sync-server`
4. 再次执行只读查询命令

**预期结果**：
- 重启后的 relay 能从数据库恢复授权记录
- 调用方 CLI 可继续直接复用原授权
- 不需要重新配对

---

### TC-RI-18：多个远端客户端同时在线并在 Settings 中可管理

**前置条件**：至少有两个不同的 Bifrost 客户端同时连接到同一个 relay 用户账号

**操作步骤**：
1. 打开 `Settings -> Remote Invoke`
2. 查看 `Clients` 区

**预期结果**：
- 列表中展示多个客户端
- 每个客户端均显示实例 ID、客户端名称、平台、在线状态、最近心跳、发现模式状态
- 可以分别对不同客户端进入发现模式、查看历史和查看授权

---

### TC-RI-19：调用方可显式选择目标客户端

**前置条件**：至少有两个客户端同时在线，其中只有目标客户端进入发现模式

**操作步骤**：
1. 在目标客户端上开启发现模式并拿到一次性授权码
2. 执行：
   ```bash
   cargo run --bin bifrost -- remote status --relay http://127.0.0.1:8686 --token "$SYNC_TOKEN" --client <target_client_instance_id> --pair-code <valid_code>
   ```

**预期结果**：
- pairing 请求只路由到指定客户端
- 非目标客户端不出现待授权请求
- 历史记录中本次调用绑定到正确的客户端实例

---

### TC-RI-20：授权码成功消费后不可再次复用

**操作步骤**：
1. 在发现模式下用某个一次性授权码成功完成一次配对授权
2. 使用同一个授权码再次执行远程调用

**预期结果**：
- 第二次调用失败
- 输出提示授权码已失效、已消费或无效
- 不会产生第二个有效 pairing

---

### TC-RI-21：授权历史列表展示完整关键信息

**前置条件**：至少产生 3 次远程调用和 2 次授权事件

**操作步骤**：
1. 打开 `Settings -> Remote Invoke`
2. 查看 `History` 区

**预期结果**：
- 列表显示时间、来源、设备指纹、客户端、授权方式、命令摘要、状态、耗时、退出码
- 列表按时间倒序排列
- 可区分成功、拒绝、过期、移除等不同状态

---

### TC-RI-22：调用详情仅展示摘要与 digest，不泄露敏感原文

**前置条件**：已有一条远程查询调用记录

**操作步骤**：
1. 打开该条调用的详情抽屉或详情页

**预期结果**：
- 可查看命令摘要、参数脱敏预览、payload digest、stdout/stderr digest
- 不展示完整敏感明文内容
- 不展示会话密钥、token 明文或可逆材料

---

### TC-RI-23：relay 数据库存储摘要而非原始明文

**前置条件**：已完成至少一次包含明显关键字的远程查询调用

**操作步骤**：
1. 打开 relay SQLite 数据库：
   ```bash
   sqlite3 <REPO_ROOT>/.bifrost-sync-test/bifrost-sync.db
   ```
2. 查询远程调用相关表中的最近记录
3. 搜索本次命令的原始敏感参数或原始完整输出片段

**预期结果**：
- 数据库中可见摘要、digest、状态、事件等审计信息
- 数据库中不可见完整命令明文、完整输出明文、会话密钥

---

### TC-RI-24：无效配对码尝试触发错误与限流

**操作步骤**：
1. 连续多次使用错误的配对码执行远程调用

**预期结果**：
- 前几次返回“配对码无效”类错误
- 达到阈值后出现限流或冷却提示
- 管理端不会出现伪造的待授权请求

---

### TC-RI-25：客户端 SSE 断开后自动重连并恢复服务

**前置条件**：客户端已成功连接 relay 且存在一个可复用授权

**操作步骤**：
1. 人为断开本地 Bifrost 到 relay 的网络连接，或重启客户端远程调用 worker
2. 观察管理端连接状态
3. 等待客户端自动重连
4. 再次执行只读查询命令

**预期结果**：
- 管理端状态短暂显示离线，再恢复在线
- relay 记录 `client_disconnected` / `client_reconnected` 事件
- 重连后新命令可以继续正常执行

---

### TC-RI-26：调用进行中客户端断线时调用方收到明确错误

**操作步骤**：
1. 发起一个能够持续数秒输出的远程查询
2. 在输出尚未结束前断开客户端连接

**预期结果**：
- 调用方 SSE 收到中断或错误事件
- 调用最终状态为 `failed`、`cancelled` 或 `timeout` 之一
- History 中能看到断线中断原因

---

### TC-RI-27：大结果流式传输过程中持续分片返回

**前置条件**：准备一个会产生大结果集的只读查询，例如大量 traffic 记录搜索结果

**操作步骤**：
1. 执行一个预期输出较大的远程只读查询命令
2. 观察调用方 CLI 输出和 relay / 客户端状态

**预期结果**：
- 调用方持续收到分片结果，而不是长时间无输出后一次性返回
- relay 进程内存无明显异常增长
- 调用完成后 History 中记录本次调用成功，包含摘要与总字节数

---

### TC-RI-28：90 天 / 10k 条清理策略生效且保留最新记录

**前置条件**：可通过 SQL 或测试脚本批量插入旧记录与大量记录

**操作步骤**：
1. 向 relay 数据库写入：
   - 多条早于 90 天的旧记录
   - 总数超过 10,000 条的历史记录
2. 触发一次清理动作（写入新记录或执行清理任务）
3. 刷新 History 列表并检查数据库

**预期结果**：
- 超过 90 天的旧记录被清理
- 总记录数被裁剪到不超过 10,000 条
- 最新记录仍然保留
- 已移除的授权历史与调用历史满足审计保留策略

---

### TC-RI-29：手动关闭发现模式后一次性授权码立即失效

**前置条件**：某个客户端已开启发现模式并显示一次性授权码

**操作步骤**：
1. 在目标客户端对应的 `Clients` 区点击 “Exit Discovery Mode”
2. 确认页面中的一次性授权码消失
3. 使用刚才展示过的授权码执行远程调用

**预期结果**：
- 发现模式立即关闭
- 原一次性授权码立即失效
- 使用该授权码的远程调用失败
- 不产生新的待授权请求

---

### TC-RI-30：大输入载荷分片上传时 relay 与客户端保持稳定

**前置条件**：存在一个支持较大输入载荷的只读查询命令或测试桩命令

**操作步骤**：
1. 准备一个较大的输入载荷，例如数百 KB 到数 MB 的查询条件文件或生成数据
2. 发起远程调用，并让 CLI 以分片方式上传该输入
3. 观察调用方 CLI、relay 和客户端执行状态

**预期结果**：
- 输入按分片持续上传，不要求一次性把全部输入载入内存后再发送
- relay 未出现明显内存飙升或卡死
- 客户端最终成功接收完整输入，或在超限时返回明确错误
- History 中记录本次调用摘要与字节数统计

---

### TC-RI-31：调用方主动取消远程调用

**前置条件**：已发起一个持续时间较长的远程查询调用

**操作步骤**：
1. 在调用尚未结束时，由调用方执行取消动作
2. 观察调用方终端输出、客户端执行状态和管理端 History

**预期结果**：
- relay 向客户端下发 `call_cancel` 或等效取消事件
- 调用方收到明确的取消结果，而不是无限等待
- 最终状态为 `cancelled`
- History 中记录取消事件

---

### TC-RI-32：调用方 SSE 断开后可基于游标续流恢复

**前置条件**：已发起一个会持续输出多段结果的远程查询调用

**操作步骤**：
1. 在结果输出过程中人为断开调用方到 relay 的 SSE 连接
2. 使用 `Last-Event-ID` 或 `cursor` 重新订阅该调用的事件流
3. 继续等待调用完成

**预期结果**：
- 重新订阅后，事件流可从中断位置附近继续恢复
- 不出现大范围重复输出或明显丢帧
- 调用最终仍能正常结束
- History 中保留完整调用记录

---

### TC-RI-33：多客户端场景下按客户端过滤授权与历史

**前置条件**：至少两个客户端分别产生过授权记录与调用历史

**操作步骤**：
1. 打开 `Settings -> Remote Invoke`
2. 在 `Active Grants` 中按客户端过滤
3. 在 `History` 中按客户端过滤

**预期结果**：
- `Active Grants` 只展示所选客户端的授权
- `History` 只展示所选客户端的调用记录
- 切换客户端过滤条件后，列表内容正确更新且不串数据

---

### TC-RI-34：只有一个处于发现模式的客户端时可自动选择目标

**前置条件**：有多个客户端在线，但只有一个客户端处于发现模式

**操作步骤**：
1. 只对其中一个客户端开启发现模式
2. 执行远程调用时不显式传 `--client`
3. 输入该客户端的一次性授权码

**预期结果**：
- 系统自动将 pairing 请求路由到唯一处于发现模式的客户端
- 其他客户端不出现待授权请求
- 调用成功后，History 中绑定到正确客户端

---

### TC-RI-35：同一客户端最多同时存在 1 个活跃配对码

**前置条件**：某个客户端已开启发现模式并展示配对码 A

**操作步骤**：
1. 记录当前展示的配对码 A
2. 手动关闭发现模式后重新开启（或等待旧码过期后再次开启）
3. 页面展示新的配对码 B
4. 使用旧配对码 A 发起远程调用

**预期结果**：
- 新配对码 B 与旧配对码 A 不同
- 使用旧配对码 A 的请求被拒绝
- 使用新配对码 B 的请求正常进入待授权流程
- 同一时刻该客户端只存在 1 个有效配对码

---

### TC-RI-36：1 小时授权可被后续命令直接复用并最终过期失效

**操作步骤**：
1. 重新生成配对码并发起远程查询
2. 在弹窗中选择 "1 小时"
3. 首次命令成功后，再执行一次：
   ```bash
   cargo run --bin bifrost -- remote status --relay http://127.0.0.1:8686 --token "$SYNC_TOKEN"
   ```
4. 在 Active Grants 中将该授权有效期改为极短值（如 1 分钟），等待其过期
5. 再次执行远程查询

**预期结果**：
- 步骤 3 第二次调用无需重新配对，直接复用授权
- Active Grants 中显示 `1h` 授权且状态为 `active`
- 步骤 5 过期后新命令无法复用，系统要求重新配对

---

### TC-RI-37：1 天授权可被后续命令直接复用

**操作步骤**：
1. 重新生成配对码并发起远程查询
2. 在弹窗中选择 "1 天"
3. 首次命令成功后，再执行一次远程查询

**预期结果**：
- 第二次调用无需重新配对
- Active Grants 中显示 `1d` 授权且状态为 `active`
- `last_used_at` 正确更新

---

### TC-RI-38：授权模式可从一次性升级为限时或永久

**前置条件**：已完成一次"仅本次"授权，且该 grant 仍显示在 Active Grants 中（状态为 `consumed`）

**操作步骤**：
1. 在 `Active Grants` 中找到该一次性授权记录
2. 将其授权模式修改为 "30 分钟"
3. 再次执行远程查询

**预期结果**：
- 管理端显示授权模式已更新为 `30m`
- 新查询能直接复用该授权
- History 中新增 `grant_updated` 事件

---

### TC-RI-39：永久授权可降级为限时授权

**前置条件**：已存在一个永久授权

**操作步骤**：
1. 在 `Active Grants` 中找到该永久授权
2. 将其修改为 "1 小时"
3. 确认修改生效
4. 再次执行远程查询，验证授权仍可复用

**预期结果**：
- 授权模式从 `permanent` 变为 `1h`
- 新增 `expires_at` 字段，显示预期过期时间
- 在到期前命令仍可复用该授权
- History 中新增 `grant_updated` 事件

---

### TC-RI-40：所有首版白名单查询命令均可远程执行

**前置条件**：已存在可复用授权（避免每次重新配对）

**操作步骤**：
1. 执行 `bifrost remote status`
2. 执行 `bifrost remote traffic get <id>`（使用一个存在的 traffic ID）
3. 执行 `bifrost remote traffic search <query>`（使用一个合理的搜索词）
4. 执行 `bifrost remote search <id>`（使用一个存在的 ID）

**预期结果**：
- 四种命令均成功返回对应结果
- 返回内容与本地直接执行等效查询的结果一致
- History 中分别记录四次调用，命令摘要各不相同

---

### TC-RI-41：端到端加密验证 — Relay 日志与数据库中不可见命令和结果明文

**前置条件**：已成功执行至少一次包含明显关键字的远程查询

**操作步骤**：
1. 将 relay 日志级别设为 debug（或查看运行时日志输出）
2. 执行一次包含可识别关键字的远程查询
3. 在 relay 日志中搜索该关键字
4. 在 relay 数据库的 `bifrost_remote_invoke_calls` 和 `bifrost_remote_invoke_events` 表中搜索该关键字

**预期结果**：
- relay 日志中不出现命令原始参数明文或输出结果明文
- 数据库中只存储摘要、digest 和状态信息
- 数据库中不存储完整命令明文、完整输出明文或会话密钥

---

### TC-RI-回归-53：第一阶段回归 — 本地 relay 接受 `command_encrypted` 的 v2 openCall

**前置条件**：
- 主线程已完成本地 relay 对 v2 openCall 的实现
- 已存在一个可复用 grant

**操作步骤**：
1. 构造一个 `command_kind = "shell.exec"`、包含唯一关键字（例如 `PHASE1-OPAQUE-CMD`）的加密命令请求
2. 通过 caller API 发起 openCall，确认请求体中只包含 `command_encrypted`，不再提交明文 `command`
3. 观察 caller 返回值与 client 侧收到的 `call_open` 事件
4. 在 relay 日志和 SQLite 中搜索 `PHASE1-OPAQUE-CMD`

**预期结果**：
- openCall 返回成功，生成新的 `call_id` 与 `relay_token`
- client 收到的 `call_open` 事件包含 `command_encrypted`、`command_kind`、`pty_enabled`、`timeout_hint_ms`
- relay 端不要求明文 `command.command`
- relay 日志和数据库中搜索不到 `PHASE1-OPAQUE-CMD` 明文

---

### TC-RI-回归-54：第一阶段回归 — `grant_scope=remote_query` 不得打开 `shell.exec`

**前置条件**：
- 已存在一个仅允许查询的 grant（通过 SQL / API 人工构造 `grant_scope = remote_query` 的授权）

**操作步骤**：
1. 使用该查询授权发起 `command_kind = "shell.exec"` 的 openCall
2. 观察 caller API 响应
3. 刷新管理端 History 或查询 relay 数据库中的 call / event 记录

**预期结果**：
- openCall 被拒绝，返回明确的 scope 不匹配错误（如 `grant_scope_mismatch`）
- 不创建进入执行阶段的 call 记录，或只留下被拒绝的审计摘要
- 管理端不会把该请求展示成可执行的 shell 调用
- 如果请求体发送旧版明文 `command` 或旧 alias `grant_scope=query`，relay 直接报协议错误，不做兼容兜底

---

### TC-RI-回归-55：第一阶段回归 — `exit_encrypted` 原样从 client 回传到 caller

**前置条件**：
- 已有一个通过 v2 协议打开的 call
- client 能构造并上报 `exit_encrypted`

**操作步骤**：
1. 让 client 为该 call 上报一个包含唯一关键字（例如 `PHASE1-OPAQUE-EXIT`）的 `exit_encrypted`
2. 在 caller 侧订阅该 call 的 SSE 事件流并等待 `exit`
3. 检查 relay 数据库和日志中是否出现 `PHASE1-OPAQUE-EXIT`

**预期结果**：
- caller 收到 `exit` 事件，事件体中携带 `exit_encrypted`
- relay 将 call 标记为结束，但不需要解析出明文 `exit_code` / `stderr`
- relay 日志和数据库中搜索不到 `PHASE1-OPAQUE-EXIT` 明文

---

### TC-RI-回归-56：第一阶段回归 — 加密调用详情 API 只返回 route-level 元数据

**前置条件**：已至少存在 1 条通过 v2 打开的加密调用记录

**操作步骤**：
1. 调用 relay 的 call detail / call list API 查询该调用
2. 检查返回 JSON 中的字段
3. 确认 API 返回体中未重建命令明细或输出明细

**预期结果**：
- 返回结果保留 `call_id`、`grant_id`、`client_instance_id`、`status`、`command_kind`、时间戳、字节数等路由级信息
- 不返回可恢复明文命令的 `command_detail`
- 不返回明文 `stderr`、明文输出摘要或可逆的加密材料

---

### TC-RI-回归-57：第一阶段回归 — relay 重启后 v2 调用记录保持可读但不回退为明文模型

**前置条件**：本地 relay 已产生至少 1 条 v2 加密调用记录

**操作步骤**：
1. 停止本地 `bifrost-sync-server`
2. 保留临时数据目录并重新启动 relay
3. 重新查询 call list / call detail
4. 再次在数据库中搜索先前用过的命令关键字或输出关键字

**预期结果**：
- relay 重启后仍能列出该调用的 route-level 记录
- 数据库内容未因重启被回写为旧的明文 `command_json` / `command_summary_json`
- 关键字搜索仍找不到原始命令或输出明文

---

### TC-RI-回归-58：第一阶段补测 — 远端 relay 部署后复测本地通过的 v2 加密链路

**前置条件**：
- 远端 `bifrost-server-v4` 已部署本轮代码
- 可从调用方访问远端 relay

**操作步骤**：
1. 重复执行 TC-RI-回归-53、TC-RI-回归-55、TC-RI-回归-56 的同类操作，但将 relay 地址替换为远端地址
2. 额外验证远端实例重启或切流后，已有 v2 调用记录仍可查询
3. 检查远端 relay 日志与存储中不存在明文命令关键字

**预期结果**：
- 远端 relay 与本地 relay 在 v2 openCall、`exit_encrypted` 转发、call detail 脱敏上的行为一致
- 部署后的多实例或重启流程不引入明文回写
- 如远端环境存在额外鉴权/网关层，也不会把命令明文打印到日志

---

### TC-RI-回归-59：第一阶段回归 — 本地 relay 完成 caller 与 client 双端加密 roundtrip

**前置条件**：
- 本地 relay 已启动，client 已完成注册并保持 SSE 在线
- 已存在一个 `grant_scope = remote_query` 的授权

**操作步骤**：
1. 调用方使用 `command_kind = "query.readonly"` 和 `command_encrypted` 发起 openCall，不提交明文 `command`
2. 确认 client 侧收到 `call_open` 事件，事件中包含 `command_encrypted`，且不包含明文字段
3. client 通过 `/client/calls/:id/frame` 上报一段带唯一关键字的加密 frame，再让 caller 连接 `/calls/:id/events`
4. 确认 caller 能收到该 frame 事件，说明 relay 已完成双端路由与 caller 侧缓冲补发
5. client 通过 `/client/calls/:id/exit` 上报带唯一关键字的 `exit_encrypted`
6. 检查 caller 收到 `exit` 事件，并检查 relay 数据库 / event summary 中不存在上述关键字

**预期结果**：
- openCall、call_open、call frame、call exit 全链路均能在本地 relay 打通
- caller 与 client 双端连接都工作正常，frame/exit 顺序正确
- relay 对 `command_encrypted`、frame 密文、`exit_encrypted` 都只做不透明转发
- 数据库与 event summary 中不存在可恢复明文的命令、输出或退出内容

---

### TC-RI-回归-60：第一阶段回归 — 真实 CLI 在本地 relay 上完成完整加密远程调用流程

**前置条件**：
- 本地 `bifrost-sync-server` 已启动并开启 remote invoke
- 本地 Bifrost client 已连接到该 relay，且发现模式可用
- 调用方使用新的空目录作为 `BIFROST_DATA_DIR`

**操作步骤**：
1. 在 client 端进入 discovery mode，记录最新 pair code
2. 调用方执行：
   ```bash
   BIFROST_DATA_DIR=/tmp/ri-caller cargo run --bin bifrost -- remote connect <pair_code> --relay-url http://127.0.0.1:8686
   ```
3. 在 client 端审批该 pairing，授予永久或限时 query grant
4. 调用方执行：
   ```bash
   BIFROST_DATA_DIR=/tmp/ri-caller cargo run --bin bifrost -- remote status --relay-url http://127.0.0.1:8686
   ```
5. 调用方继续执行：
   ```bash
   BIFROST_DATA_DIR=/tmp/ri-caller cargo run --bin bifrost -- remote search bifrost --limit 3 --relay-url http://127.0.0.1:8686
   ```
6. 调用方继续执行：
   ```bash
   BIFROST_DATA_DIR=/tmp/ri-caller cargo run --bin bifrost -- remote traffic list --limit 3 --relay-url http://127.0.0.1:8686
   ```
7. 检查 caller 本地连接文件、client 日志与 relay 日志，确认三次调用都经由 `command_encrypted` / 加密 frame / `exit_encrypted` 完成闭环

**预期结果**：
- `remote connect` 成功落盘本地连接，连接记录中包含 `caller_ephemeral_pub`、`client_ephemeral_pub` 与本地加密保存的 `shared_secret`
- `remote status` 成功返回目标 client 的运行状态 JSON，不再出现 `command_encrypted is required` 或 `encrypted payload authentication failed`
- `remote search` 与 `remote traffic list` 都能成功返回结果；即使结果为空，也必须完成正常的加密调用闭环
- client 日志显示 `open_call` 解密成功并完成执行；caller 不需要回退到任何明文协议
- relay 侧仍只转发密文与 route-level metadata，不泄露命令明文或输出明文

---

### TC-RI-42：Bifrost 客户端重启后 client_instance_id 不变且授权可继续复用

**前置条件**：已存在一个永久或限时授权，记录当前 client_instance_id

**操作步骤**：
1. 停止 Bifrost 客户端进程
2. 重新启动 Bifrost 客户端
3. 在管理端 `Clients` 区确认客户端已重新上线
4. 再次执行远程查询

**预期结果**：
- 重启后的 `client_instance_id` 与重启前一致
- 原授权记录仍然有效
- 新查询可直接复用原授权，无需重新配对
- Clients 列表中该客户端信息保持一致

---

### TC-RI-43：Pending Requests 区可独立查看和操作待授权请求

**前置条件**：已发起一个远程调用，正处于等待授权状态

**操作步骤**：
1. 打开 `Settings -> Remote Invoke`
2. 在 `Pending Requests` 区查看待授权列表
3. 确认列表中展示来源设备指纹、命令摘要
4. 在列表中（而非全局弹窗中）点击批准或拒绝

**预期结果**：
- Pending Requests 区独立展示所有待授权请求
- 可在列表中直接操作批准/拒绝
- 操作效果与全局弹窗一致
- 操作完成后该请求从 Pending 列表消失

---

### TC-RI-44：History 区支持按状态、时间范围等条件过滤

**前置条件**：至少存在 3 条不同状态的调用记录（成功、拒绝、取消等）

**操作步骤**：
1. 打开 `Settings -> Remote Invoke -> History`
2. 使用状态过滤器筛选"成功"的记录
3. 使用状态过滤器筛选"拒绝"的记录
4. 使用时间范围过滤最近 1 小时的记录

**预期结果**：
- 按状态过滤后只展示对应状态的记录
- 按时间范围过滤后只展示范围内的记录
- 切换过滤条件后列表正确更新
- 清除过滤后恢复完整列表

---

## 回归测试用例

以下用例覆盖本轮开发中发现并修复的 Bug，确保修复后不再回归。

### TC-RI-45：回归 — 关闭发现模式 API 不应返回 401

**背景**：`close_discovery_session` 的 DELETE 请求缺少 `client_instance_id` 查询参数导致 relay 返回 401。

**操作步骤**：
1. 通过 Admin API 开启发现模式：
   ```bash
   curl -s -X POST http://127.0.0.1:8800/_bifrost/api/remote-invoke/discovery/enter | jq .
   ```
2. 通过 Admin API 关闭发现模式：
   ```bash
   curl -s -X POST http://127.0.0.1:8800/_bifrost/api/remote-invoke/discovery/exit | jq .
   ```

**预期结果**：
- 关闭发现模式请求返回成功（HTTP 200）
- 不出现 401 Unauthorized 错误
- 发现模式状态变为关闭

---

### TC-RI-46：回归 — traffic.list 命令被 relay 正确接受

**背景**：relay 端 `ALLOWED_COMMANDS` 白名单缺少 `traffic.list`，导致该命令被拒绝。

**操作步骤**：
1. 确保已有可复用授权
2. 通过 relay 的 caller API 发起 `traffic.list` 调用：
   ```bash
   curl -s -X POST http://127.0.0.1:8687/v4/remote-invoke/caller/calls \
     -H "Content-Type: application/json" \
     -H "x-bifrost-token: $SYNC_TOKEN" \
     -d '{
       "grant_id": "<valid_grant_id>",
       "command": "traffic.list",
       "args_json": "{\"limit\":5}"
     }' | jq .
   ```

**预期结果**：
- relay 接受 `traffic.list` 命令，不返回 `unsupported_command` 错误
- 调用成功返回流量列表数据

---

### TC-RI-47：回归 — traffic.get 接受含连字符的 ID

**背景**：executor 的 ID 验证仅允许纯数字，导致 `REQ-xxxxx-nnnnnn` 格式的真实 traffic ID 被拒绝。

**操作步骤**：
1. 确保已有可复用授权
2. 获取一个真实的 traffic ID（格式如 `REQ-69e304e7-000033`）
3. 通过远程调用执行 `traffic.get`：
   ```bash
   curl -s -X POST http://127.0.0.1:8687/v4/remote-invoke/caller/calls \
     -H "Content-Type: application/json" \
     -H "x-bifrost-token: $SYNC_TOKEN" \
     -d '{
       "grant_id": "<valid_grant_id>",
       "command": "traffic.get",
       "args_json": "{\"id\":\"REQ-69e304e7-000033\"}"
     }' | jq .
   ```

**预期结果**：
- 命令成功执行，不返回"id param must contain only digits"错误
- 返回对应的 traffic 详情或 not_found 提示

---

### TC-RI-48：回归 — traffic.get 仍拒绝包含特殊字符的 ID

**背景**：修复 ID 验证后，需确保仍拒绝包含注入字符的恶意 ID。

**操作步骤**：
1. 尝试使用包含分号的恶意 ID：
   ```bash
   curl -s -X POST http://127.0.0.1:8687/v4/remote-invoke/caller/calls \
     -H "Content-Type: application/json" \
     -H "x-bifrost-token: $SYNC_TOKEN" \
     -d '{
       "grant_id": "<valid_grant_id>",
       "command": "traffic.get",
       "args_json": "{\"id\":\"abc;DROP TABLE\"}"
     }' | jq .
   ```

**预期结果**：
- 命令被 executor 拒绝
- 错误信息包含"id param must contain only alphanumeric characters and hyphens"
- 不执行任何实际查询

---

### TC-RI-49：回归 — revoke_ack 请求包含正确的认证信息

**背景**：`revoke_ack` POST 请求体缺少 `client_instance_id` 导致 relay 认证失败。

**操作步骤**：
1. 确保存在一个活跃授权
2. 通过管理端 WebUI 移除该授权
3. 观察 Bifrost 客户端日志

**预期结果**：
- 授权移除操作成功完成
- Bifrost 客户端日志不出现 `revoke_ack` 相关的 401 或认证失败错误
- relay 端记录 `grant_revoked` 事件

---

### TC-RI-回归-50：回归 — Discovery Mode API 在 auth token 过期/失效后不应返回 500

**背景**：`enter_discovery_mode` / `exit_discovery_mode` / `refresh_pair_code` 在 relay 返回 401（auth token 失效）时，直接将网络错误透传为 HTTP 500 返回给前端。根本原因是 `client_auth_token` 仅存储在内存中，relay 重启或 Redis 清理后 token 失效，但 worker 未自动 re-register。修复方案：在上述三个 API 中检测 401 后自动调用 `register_with_relay()` 重新获取 token 并重试。同时在 SSE 心跳 401 时触发重连。

**操作步骤**：
1. 启动 Bifrost 测试实例并确认 Remote Invoke Worker 状态为 Connected：
   ```bash
   curl -s http://127.0.0.1:8801/_bifrost/api/remote-invoke/status | jq .state
   ```
2. 调用 Discovery Mode Enter API：
   ```bash
   curl -s -X POST http://127.0.0.1:8801/_bifrost/api/remote-invoke/discovery/enter | jq .
   ```
3. 调用 Discovery Mode Refresh API：
   ```bash
   curl -s -X POST http://127.0.0.1:8801/_bifrost/api/remote-invoke/discovery/refresh | jq .
   ```
4. 调用 Discovery Mode Exit API：
   ```bash
   curl -s -X POST http://127.0.0.1:8801/_bifrost/api/remote-invoke/discovery/exit | jq .
   ```

**预期结果**：
- 步骤 1：状态为 `"Connected"`
- 步骤 2：返回 `{"success": true, "session": {...}}` 包含 `pair_code` 和 `session_id`，不返回 500
- 步骤 3：返回 `{"success": true, "session": {...}}` 包含新的 `pair_code`，不返回 500
- 步骤 4：返回 `{"success": true}`，不返回 500
- 如果 auth token 已失效，worker 内部自动 re-register 后重试，前端无感知

---

## 补充覆盖用例

以下用例覆盖 gap 分析中发现的设计方案未被测试覆盖的关键场景。

### TC-RI-50：多调用方独立配对与授权

**背景**：设计方案 §多调用方并发支持 要求不同 `caller_fingerprint` 的调用方可独立完成配对和授权。

**前置条件**：Bifrost 客户端已连接 relay

**操作步骤**：
1. 通过 Admin API 开启发现模式：
   ```bash
   curl -s -X POST http://127.0.0.1:8800/_bifrost/api/remote-invoke/discovery/enter | jq .
   ```
2. 获取配对码并以 `caller-fp-multi-A` 身份发起配对：
   ```bash
   curl -s -X POST http://127.0.0.1:8686/v4/remote-invoke/pairings/start \
     -H "Content-Type: application/json" \
     -H "x-bifrost-token: $SYNC_TOKEN" \
     -d '{
       "pair_code": "<pair_code>",
       "caller_fingerprint": "caller-fp-multi-A",
       "caller_display_name": "Multi Caller A"
     }' | jq .
   ```
3. 在 Admin API 批准该配对（选择 permanent 模式）：
   ```bash
   curl -s -X POST http://127.0.0.1:8800/_bifrost/api/remote-invoke/grants/<pairing_id>/decision \
     -H "Content-Type: application/json" \
     -d '{"decision": "approve", "grant_mode": "permanent"}' | jq .
   ```
4. 再次开启发现模式并获取新配对码
5. 以 `caller-fp-multi-B` 身份发起配对并批准

**预期结果**：
- 两个调用方各自成功完成独立配对
- `GET /v4/remote-invoke/grants` 返回两条不同 `caller_fingerprint` 的活跃授权
- 两条授权的 `grant_id` 不同
- 两条授权的 `client_instance_id` 相同（同一客户端）

---

### TC-RI-51：多调用方 grant 隔离 — 撤销一方不影响另一方

**背景**：设计方案要求"调用方 A 的 grant 撤销不影响调用方 B"。

**前置条件**：已完成 TC-RI-50，两个调用方均有活跃授权

**操作步骤**：
1. 查看当前所有 grants：
   ```bash
   curl -s "http://127.0.0.1:8686/v4/remote-invoke/grants" \
     -H "x-bifrost-token: $SYNC_TOKEN" | jq '.data.list[] | select(.status == "active")'
   ```
2. 删除调用方 A 的 grant：
   ```bash
   curl -s -X DELETE "http://127.0.0.1:8686/v4/remote-invoke/grants/<grant_id_A>" \
     -H "x-bifrost-token: $SYNC_TOKEN" | jq .
   ```
3. 使用调用方 B 的 grant 发起调用：
   ```bash
   curl -s -X POST http://127.0.0.1:8686/v4/remote-invoke/calls/open \
     -H "Content-Type: application/json" \
     -H "x-bifrost-token: $SYNC_TOKEN" \
     -d '{
       "grant_id": "<grant_id_B>",
       "command": "status",
       "args_json": "{}",
       "caller_fingerprint": "caller-fp-multi-B"
     }' | jq .
   ```
4. 验证调用方 A 的 grant 已失效：
   ```bash
   curl -s "http://127.0.0.1:8686/v4/remote-invoke/grants/reusable?client_instance_id=<cid>&caller_fingerprint=caller-fp-multi-A" \
     -H "x-bifrost-token: $SYNC_TOKEN" | jq .
   ```

**预期结果**：
- 调用方 A 的 grant 状态变为 `removed`
- 调用方 B 的 grant 仍为 `active`
- 使用调用方 B 的 grant 发起的调用成功返回
- 查找调用方 A 的可复用授权返回空

---

### TC-RI-52：多调用方并发 call 执行

**背景**：设计方案要求"同一客户端允许多个调用方同时发起 call，各 call 独立执行"。

**前置条件**：两个调用方各自拥有活跃授权

**操作步骤**：
1. 使用调用方 A 和调用方 B 的 grant 同时发起 call（在两个终端并行执行，或使用 `&` 后台执行）：
   ```bash
   # 终端 1 - 调用方 A
   curl -s -X POST http://127.0.0.1:8686/v4/remote-invoke/calls/open \
     -H "Content-Type: application/json" \
     -H "x-bifrost-token: $SYNC_TOKEN" \
     -d '{
       "grant_id": "<grant_id_A>",
       "command": "status",
       "args_json": "{}",
       "caller_fingerprint": "caller-fp-multi-A"
     }' > /tmp/call_a.json &

   # 终端 2 - 调用方 B
   curl -s -X POST http://127.0.0.1:8686/v4/remote-invoke/calls/open \
     -H "Content-Type: application/json" \
     -H "x-bifrost-token: $SYNC_TOKEN" \
     -d '{
       "grant_id": "<grant_id_B>",
       "command": "traffic.list",
       "args_json": "{\"limit\":5}",
       "caller_fingerprint": "caller-fp-multi-B"
     }' > /tmp/call_b.json &
   wait
   ```
2. 检查两个调用的结果

**预期结果**：
- 两个调用各自返回有效的 `call_id`
- 两个 `call_id` 不同
- 两个调用均成功完成，不会因并发而阻塞或报错
- Bifrost 客户端状态显示 `active_call_ids` 一度包含两个 call

---

### TC-RI-53：多调用方数据隔离 — 调用记录仅对自身可见

**背景**：设计方案要求"调用方只能通过自己的 relay_token 订阅自己发起的 call 事件流"。

**前置条件**：TC-RI-52 已完成，存在多个调用方的 call 记录

**操作步骤**：
1. 查看所有 calls 列表：
   ```bash
   curl -s "http://127.0.0.1:8686/v4/remote-invoke/calls" \
     -H "x-bifrost-token: $SYNC_TOKEN" | jq '.data.list | length'
   ```
2. 获取调用方 A 发起的 call 详情：
   ```bash
   curl -s "http://127.0.0.1:8686/v4/remote-invoke/calls/<call_id_a>" \
     -H "x-bifrost-token: $SYNC_TOKEN" | jq '.data.caller_fingerprint'
   ```
3. 获取调用方 B 发起的 call 详情：
   ```bash
   curl -s "http://127.0.0.1:8686/v4/remote-invoke/calls/<call_id_b>" \
     -H "x-bifrost-token: $SYNC_TOKEN" | jq '.data.caller_fingerprint'
   ```

**预期结果**：
- calls 列表中包含两个调用方的记录
- 每个 call 的 `caller_fingerprint` 字段正确匹配发起者
- WebUI History 区可看到所有调用方的记录（管理端视角可见全部）

---

### TC-RI-54：配对码超时后自动轮换（发现模式持续开启）

**背景**：设计方案 §六位码只做配对引导 要求"授权码过期后，若发现模式仍然开启，客户端自动生成新的 6 位授权码并向 Relay 重新注册，旧码立即作废"。

**操作步骤**：
1. 开启发现模式：
   ```bash
   curl -s -X POST http://127.0.0.1:8800/_bifrost/api/remote-invoke/discovery/enter | jq .
   ```
2. 记录当前配对码（code_A）和有效期
3. 等待配对码过期（默认 2 分钟）
4. 查询发现模式状态，确认是否自动生成新码：
   ```bash
   curl -s http://127.0.0.1:8800/_bifrost/api/remote-invoke/status | jq .discovery_session
   ```
5. 如果生成了新码（code_B），使用旧码 code_A 尝试配对
6. 使用新码 code_B 尝试配对

**预期结果**：
- 配对码过期后发现模式仍为开启状态
- 自动生成新的配对码 code_B，与 code_A 不同
- WebUI 实时刷新展示新码和重置后的倒计时
- 使用旧码 code_A 配对失败（返回无效/过期错误）
- 使用新码 code_B 配对成功进入待授权流程

---

### TC-RI-55：并发配对冲突 — pair_code_already_consumed 和 pair_slot_occupied

**背景**：设计方案 §并发配对冲突处理 描述了配对码被消费后的排他逻辑。

**操作步骤**：
1. 开启发现模式并获取配对码
2. 调用方 A 使用配对码成功发起配对（进入 pending_approval）：
   ```bash
   curl -s -X POST http://127.0.0.1:8686/v4/remote-invoke/pairings/start \
     -H "Content-Type: application/json" \
     -H "x-bifrost-token: $SYNC_TOKEN" \
     -d '{
       "pair_code": "<pair_code>",
       "caller_fingerprint": "caller-fp-conflict-A",
       "caller_display_name": "Conflict Caller A"
     }' | jq .
   ```
3. 调用方 B 使用同一配对码尝试配对：
   ```bash
   curl -s -X POST http://127.0.0.1:8686/v4/remote-invoke/pairings/start \
     -H "Content-Type: application/json" \
     -H "x-bifrost-token: $SYNC_TOKEN" \
     -d '{
       "pair_code": "<pair_code>",
       "caller_fingerprint": "caller-fp-conflict-B",
       "caller_display_name": "Conflict Caller B"
     }' | jq .
   ```
4. 生成新配对码后，调用方 C 使用新码配对（此时调用方 A 仍在 pending_approval）：
   ```bash
   curl -s -X POST http://127.0.0.1:8686/v4/remote-invoke/pairings/start \
     -H "Content-Type: application/json" \
     -H "x-bifrost-token: $SYNC_TOKEN" \
     -d '{
       "pair_code": "<new_pair_code>",
       "caller_fingerprint": "caller-fp-conflict-C",
       "caller_display_name": "Conflict Caller C"
     }' | jq .
   ```

**预期结果**：
- 步骤 2：调用方 A 成功进入 pending_approval
- 步骤 3：调用方 B 收到 `pair_code_already_consumed` 错误
- 步骤 4：调用方 C 收到 `pair_slot_occupied` 错误（因为调用方 A 的配对仍在等待审批）

---

### TC-RI-56：traffic.clear 命令被明确拒绝

**背景**：设计方案明确规定"traffic.clear 为写操作，不在只读白名单内，远程调用明确拒绝"。

**前置条件**：已有可复用授权

**操作步骤**：
1. 使用已有授权发起 `traffic.clear` 调用：
   ```bash
   curl -s -X POST http://127.0.0.1:8686/v4/remote-invoke/calls/open \
     -H "Content-Type: application/json" \
     -H "x-bifrost-token: $SYNC_TOKEN" \
     -d '{
       "grant_id": "z57LgwhhAHOY6g8mlCtMZ",
       "command": "traffic.clear",
       "args_json": "{}",
       "caller_fingerprint": "caller-fp-large"
     }' | jq .
   ```

**预期结果**：
- 返回 `unsupported_command` 错误
- 不执行任何清理操作
- Bifrost 本地流量数据未受影响

---

### TC-RI-57：traffic.list 多条件参数过滤

**背景**：设计方案详细定义了 `traffic.list` 支持的完整参数表（limit, method, status, host, protocol 等），需验证多条件组合过滤生效。

**前置条件**：已有可复用授权，本地 Bifrost 已有若干流量记录

**操作步骤**：
1. 基础调用 — 仅指定 limit：
   ```bash
   curl -s -X POST http://127.0.0.1:8686/v4/remote-invoke/calls/open \
     -H "Content-Type: application/json" \
     -H "x-bifrost-token: $SYNC_TOKEN" \
     -d '{
       "grant_id": "z57LgwhhAHOY6g8mlCtMZ",
       "command": "traffic.list",
       "args_json": "{\"limit\":3}",
       "caller_fingerprint": "caller-fp-large"
     }' | jq .
   ```
2. 多条件组合 — limit + method 过滤：
   ```bash
   curl -s -X POST http://127.0.0.1:8686/v4/remote-invoke/calls/open \
     -H "Content-Type: application/json" \
     -H "x-bifrost-token: $SYNC_TOKEN" \
     -d '{
       "grant_id": "z57LgwhhAHOY6g8mlCtMZ",
       "command": "traffic.list",
       "args_json": "{\"limit\":10,\"method\":\"GET\"}",
       "caller_fingerprint": "caller-fp-large"
     }' | jq .
   ```
3. 多条件组合 — limit + host 模糊匹配：
   ```bash
   curl -s -X POST http://127.0.0.1:8686/v4/remote-invoke/calls/open \
     -H "Content-Type: application/json" \
     -H "x-bifrost-token: $SYNC_TOKEN" \
     -d '{
       "grant_id": "z57LgwhhAHOY6g8mlCtMZ",
       "command": "traffic.list",
       "args_json": "{\"limit\":10,\"host\":\"127.0.0.1\"}",
       "caller_fingerprint": "caller-fp-large"
     }' | jq .
   ```
4. 非法参数校验 — limit 超过 100：
   ```bash
   curl -s -X POST http://127.0.0.1:8686/v4/remote-invoke/calls/open \
     -H "Content-Type: application/json" \
     -H "x-bifrost-token: $SYNC_TOKEN" \
     -d '{
       "grant_id": "z57LgwhhAHOY6g8mlCtMZ",
       "command": "traffic.list",
       "args_json": "{\"limit\":200}",
       "caller_fingerprint": "caller-fp-large"
     }' | jq .
   ```

**预期结果**：
- 步骤 1：返回最多 3 条流量记录
- 步骤 2：返回的记录中 method 均为 GET（如果有 GET 记录）
- 步骤 3：返回的记录中 host 包含 `127.0.0.1`（如果有匹配记录）
- 步骤 4：limit 被自动截断为 100（或返回 `invalid_args` 错误），不返回 200 条

---

### TC-RI-58：grant 数量上限验证（max_grants_per_client）

**背景**：设计方案规定 `max_grants_per_client` 默认为 20，超出后新配对被拒绝。

**操作步骤**：
1. 查看当前 grants 数量：
   ```bash
   curl -s "http://127.0.0.1:8686/v4/remote-invoke/grants" \
     -H "x-bifrost-token: $SYNC_TOKEN" | jq '.data.total'
   ```
2. 如果数量接近上限（20），记录当前数量
3. 持续创建新配对直到达到上限
4. 尝试超过上限时的配对

**预期结果**：
- 达到 `max_grants_per_client` 上限后，新配对请求返回 `grant_limit_exceeded` 错误
- 已有的活跃 grants 不受影响
- 删除一个旧 grant 后可以再次成功配对

---

### TC-RI-59：once 授权调用完成后状态变为 consumed

**背景**：设计方案要求一次性授权在 call 完成后自动变为已消费状态，不可再复用。

**操作步骤**：
1. 开启发现模式并获取配对码
2. 以新的 `caller-fp-once-test` 身份配对
3. 在 Admin API 批准，选择 `once` 模式
4. 查看 grant 状态（预期为 `active`）：
   ```bash
   curl -s "http://127.0.0.1:8686/v4/remote-invoke/grants" \
     -H "x-bifrost-token: $SYNC_TOKEN" | jq '.data.list[] | select(.caller_fingerprint == "caller-fp-once-test")'
   ```
5. 使用该 grant 发起一次调用：
   ```bash
   curl -s -X POST http://127.0.0.1:8686/v4/remote-invoke/calls/open \
     -H "Content-Type: application/json" \
     -H "x-bifrost-token: $SYNC_TOKEN" \
     -d '{
       "grant_id": "<once_grant_id>",
       "command": "status",
       "args_json": "{}",
       "caller_fingerprint": "caller-fp-once-test"
     }' | jq .
   ```
6. 调用完成后再次查看 grant 状态
7. 尝试复用该 grant 发起第二次调用

**预期结果**：
- 步骤 4：grant 状态为 `active`
- 步骤 5：调用成功返回
- 步骤 6：grant 状态变为 `consumed`
- 步骤 7：第二次调用失败，返回授权已消费/已失效错误

---

### 全局授权弹窗测试用例

以下用例覆盖全局授权弹窗的自动弹出、忽略（Dismiss）、以及多页面场景下的行为。

### TC-RI-60：全局授权弹窗在非 Settings 页面自动弹出

**前置条件**：Bifrost 客户端已连接 relay，发现模式已开启

**操作步骤**：
1. 在浏览器中打开 Bifrost 管理端的非 Settings 页面（如首页 `http://127.0.0.1:8800/_bifrost/`）
2. 从另一终端发起远程调用触发配对请求：
   ```bash
   cargo run --bin bifrost -- remote status --relay http://127.0.0.1:8686 --token "$SYNC_TOKEN" --pair-code <valid_code>
   ```
3. 等待 3 秒内观察浏览器页面

**预期结果**：
- 全局授权弹窗在当前页面（非 Settings 页）自动弹出
- 弹窗标题包含 "Remote Command Authorization" 和待处理数量 Badge
- 弹窗内展示调用方设备指纹、来源 IP、命令摘要
- 弹窗提供 Authorize（下拉选择授权期限）、Reject、Dismiss 三种操作按钮

---

### TC-RI-61：全局授权弹窗 Dismiss 单个请求

**前置条件**：全局授权弹窗已弹出，显示至少一个待授权请求

**操作步骤**：
1. 在全局弹窗中点击某个请求的 "Dismiss" 按钮

**预期结果**：
- 该请求从弹窗列表中消失
- 如果没有其他未 Dismiss 的请求，弹窗自动关闭
- 该请求在 Settings > Remote Invoke 的 Pending Requests 列表中仍然可见
- 被 Dismiss 的请求不会在当前会话中再次触发弹窗弹出

---

### TC-RI-62：全局授权弹窗 Dismiss All

**前置条件**：全局授权弹窗已弹出，显示至少两个待授权请求

**操作步骤**：
1. 在全局弹窗底部点击 "Dismiss All" 按钮

**预期结果**：
- 弹窗立即关闭
- 所有当前待授权请求被标记为已忽略
- 已忽略的请求不会再次触发弹窗弹出
- Pending Requests 列表中仍可看到这些请求
- 调用方 CLI 仍在等待授权状态（不受 Dismiss 影响）

---

### TC-RI-63：Dismiss 后新的配对请求重新触发弹窗

**前置条件**：已 Dismiss All 关闭弹窗

**操作步骤**：
1. 生成新的配对码（刷新或重新进入发现模式）
2. 从另一终端使用新配对码发起新的远程调用
3. 等待 3 秒内观察浏览器页面

**预期结果**：
- 新的配对请求触发全局弹窗再次弹出
- 弹窗中只显示新请求（之前被 Dismiss 的旧请求不再出现）
- 新请求可正常执行 Authorize / Reject / Dismiss 操作

---

### TC-RI-64：通过全局弹窗 Authorize 下拉选择授权期限

**前置条件**：全局授权弹窗已弹出

**操作步骤**：
1. 在弹窗中点击某个请求的 "Authorize" 下拉按钮
2. 在下拉菜单中选择 "30 Minutes"

**预期结果**：
- 下拉菜单显示：This Time Only / 30 Minutes / 1 Hour / 1 Day / Permanent
- 选择后请求被批准，弹窗显示成功提示 "Authorization granted"
- 调用方 CLI 收到授权并成功执行命令
- Active Grants 中新增一条 `30m` 模式的授权

---

### TC-RI-65：全局弹窗 Remote Invoke Settings 按钮导航

**前置条件**：全局授权弹窗已弹出

**操作步骤**：
1. 在弹窗底部点击 "Remote Invoke Settings" 按钮

**预期结果**：
- 弹窗关闭（当前请求被 Dismiss）
- 页面导航到 `Settings > Remote Invoke` Tab
- Remote Invoke Tab 正常展示状态、Pending Requests、Active Grants 等区域

---

### TC-RI-66：关闭弹窗（点击 X 或遮罩）等同于 Dismiss All

**前置条件**：全局授权弹窗已弹出

**操作步骤**：
1. 点击弹窗右上角的关闭按钮（X）
2. 或者点击弹窗外的遮罩区域

**预期结果**：
- 弹窗关闭
- 行为等同于 Dismiss All — 所有当前请求被忽略
- 调用方 CLI 不受影响，仍在等待

---

### TC-RI-67：全局弹窗 Reject 操作

**前置条件**：全局授权弹窗已弹出

**操作步骤**：
1. 在弹窗中点击某个请求的 "Reject" 按钮

**预期结果**：
- 请求被拒绝，弹窗显示成功提示 "Request rejected"
- 该请求从弹窗列表中消失
- 调用方 CLI 收到拒绝错误
- History 中新增一条拒绝事件

---

## 远端部署场景测试

以下用例覆盖当 relay 服务部署到远端服务器（而非本地 localhost）时的测试场景。

### 前置条件

1. 远端已部署 `bifrost-server-v4` 或远端 `bifrost-sync-server` 并对外暴露 HTTPS 端口
2. 本地 Bifrost 客户端可通过公网访问远端 relay
3. 调用方 CLI 可通过公网访问远端 relay
4. 已在远端 relay 注册用户并获取 token：
   ```bash
   export REMOTE_RELAY="https://<remote-host>"
   export SYNC_TOKEN="<remote_token>"
   ```

---

### TC-RI-70：远端 Relay 连接 — HTTPS 证书验证与安全传输

**操作步骤**：
1. 使用 HTTPS 地址连接远端 relay：
   ```bash
   cargo run --bin bifrost -- remote status --relay "$REMOTE_RELAY" --token "$SYNC_TOKEN"
   ```
2. 尝试使用 HTTP（非 TLS）地址连接远端 relay：
   ```bash
   cargo run --bin bifrost -- remote status --relay "http://<remote-host>:<port>" --token "$SYNC_TOKEN"
   ```

**预期结果**：
- HTTPS 连接正常建立，证书验证通过
- HTTP 连接被拒绝或自动升级到 HTTPS
- 不允许在明文 HTTP 通道上传输 token 和加密帧

---

### TC-RI-71：远端 Relay — 客户端 SSE 跨公网长连接稳定性

**前置条件**：Bifrost 客户端通过公网与远端 relay 建立 SSE 连接

**操作步骤**：
1. 启动本地 Bifrost 客户端，连接远端 relay
2. 在管理端确认连接状态为 online
3. 保持连接运行 5-10 分钟
4. 期间在不同时间点执行 2-3 次远程查询

**预期结果**：
- SSE 长连接在 5-10 分钟内持续保持在线
- 心跳正常，管理端状态不闪断
- 每次查询均成功返回
- 如果出现临时断线，能在数秒内自动重连

---

### TC-RI-72：远端 Relay — 高延迟网络下的超时与错误提示

**操作步骤**：
1. 在高延迟或不稳定网络环境下连接远端 relay
2. 执行远程查询命令
3. 观察超时行为和错误提示

**预期结果**：
- 系统有合理的超时设置，不会无限等待
- 超时后输出清晰错误信息，明确区分"网络超时"与"业务拒绝"
- 不因网络延迟导致误判授权失效或调用失败
- 重试后如网络恢复，命令可正常执行

---

### TC-RI-73：远端 Relay — SSO 集成鉴权

**前置条件**：远端 relay 已接入 SSO 体系

**操作步骤**：
1. 使用 SSO 登录获取 token
2. 使用 SSO token 执行完整的配对-授权-查询流程：
   ```bash
   cargo run --bin bifrost -- remote status --relay "$REMOTE_RELAY" --token "$SSO_TOKEN" --pair-code <valid_code>
   ```
3. 在管理端确认授权弹窗和历史记录正常

**预期结果**：
- SSO token 鉴权正常工作
- 配对、授权、查询全链路与测试 token 行为一致
- History 中正确记录用户身份（来源于 SSO）

---

### TC-RI-74：远端 Relay — 多用户并发访问同一客户端

**前置条件**：远端 relay 上注册了用户 A 和用户 B，同一 Bifrost 客户端在线

**操作步骤**：
1. 用户 A 和用户 B 分别使用各自的 token 同时发起对同一客户端的远程查询
2. 客户端分别为两个请求授权
3. 两个查询各自完成

**预期结果**：
- 两个用户的授权互相独立，不会混淆
- 各自的调用结果正确，不会串号
- Active Grants 中分别展示两个不同用户的授权记录
- History 中分别记录两个用户的调用，来源不同

---

### TC-RI-75：远端 Relay — Relay 地址切换后客户端重新连接

**前置条件**：Bifrost 客户端当前连接远端 relay-A

**操作步骤**：
1. 将客户端配置中的 relay 地址从 relay-A 改为 relay-B
2. 重启客户端远程调用模块或重启客户端进程
3. 在 relay-B 的管理端确认客户端上线
4. 使用 relay-B 的地址和 token 发起远程调用

**预期结果**：
- 客户端成功连接 relay-B
- relay-A 上的授权不可在 relay-B 上复用
- 需要在 relay-B 上重新完成配对和授权
- 调用成功后 relay-B 中记录完整历史

---

### TC-RI-76：远端 Relay — 大结果集跨公网流式传输完整性

**前置条件**：存在一个会产生大结果集的只读查询，且已有可复用授权

**操作步骤**：
1. 通过远端 relay 执行产生大结果集的远程查询
2. 观察调用方 CLI 的流式输出过程
3. 对比本地直接查询的结果与远程查询的结果

**预期结果**：
- 分片持续到达，不因公网传输导致丢片或乱序
- 最终结果完整，与本地直接查询结果一致
- relay 内存无异常增长
- History 中记录总字节数和调用摘要

---

### TC-RI-77：远端 Relay — 客户端跨网络断线后的自动恢复

**前置条件**：Bifrost 客户端通过公网连接远端 relay 且存在可复用授权

**操作步骤**：
1. 模拟客户端网络中断（如短暂断开网络连接）
2. 观察管理端连接状态变化
3. 恢复网络连接
4. 等待客户端自动重连
5. 再次执行远程查询

**预期结果**：
- 断线后管理端状态短暂显示离线
- 网络恢复后客户端按退避策略自动重连
- 重连成功后管理端状态恢复在线
- 原授权仍然有效，新查询可正常执行
- relay 记录 `client_disconnected` / `client_reconnected` 事件

---

### TC-RI-回归-60：回归 — SSE 事件缓冲不应导致 caller 收到重复 frame

**背景**：当 caller 在 `open_call` 后尚未订阅 SSE 事件流时，client 已执行完命令并将 frame+exit 回传给 relay。relay 的 `pushToCallerStream` 将事件同时写入本地内存 buffer 和 Redis buffer。caller 随后订阅事件流时，`registerCallerEventStream` 同时 flush 了本地 buffer 和 Redis buffer，导致同一个 frame 事件被推送两次，caller 端输出重复的 JSON。

**修复方案**：
- 服务端（`remoteInvokeSse.ts`）：`registerCallerEventStream` flush 本地 buffer 后跳过 Redis buffer 读取，仅在本地无缓冲时回退读取 Redis，且始终清理 Redis buffer。
- 客户端（`remote.rs`）：caller 端基于 `EncryptedEnvelope.seq` 字段去重，跳过已处理的 frame。

**操作步骤**：
1. 确保已有可复用授权
2. 执行 `remote status` 命令并捕获原始输出：
   ```bash
   ./target/debug/bifrost remote status --client-id <client_instance_id> 2>/dev/null
   ```
3. 执行 `remote traffic list` 命令并验证 JSON 有效性：
   ```bash
   ./target/debug/bifrost remote traffic list --client-id <client_instance_id> 2>/dev/null | python3 -m json.tool
   ```
4. 执行 `remote traffic get <id>` 命令并验证：
   ```bash
   ./target/debug/bifrost remote traffic get <traffic_id> --client-id <client_instance_id> 2>/dev/null | python3 -m json.tool
   ```

**预期结果**：
- 每个命令的输出是有效的单个 JSON 对象，不是两个拼接的 JSON
- `python3 -m json.tool` 能正常解析输出，不报 "Extra data" 错误
- status 输出只包含一份 version/uptime 信息
- traffic list 输出只包含一份 records 数组
- traffic get 输出只包含一份请求详情

---

### TC-RI-回归-61：回归 — 多实例 SSE frame/exit 竞态条件不应导致 caller 丢失 frame

**背景**：在多实例（TCE）部署环境下，当 client 执行完命令后将 `frame` 和 `exit` 事件分别 POST 到 relay，但 `frame` 和 `exit` 可能被路由到不同实例。caller 的 `registerCallerEventStream` 在实例 V 上完成后，`frame` 事件可能延迟写入实例 Z 的 Redis buffer。此时 caller 只收到 `exit` 事件而丢失 `frame`，导致输出为空。

**修复方案（双端修复）**：
- 服务端（`remoteInvokeSse.ts`）：`registerCallerEventStream` 注册后，除初始 flush 外还在 500ms 和 2000ms 各安排一次延迟 Redis buffer 重读（`flushRedisCallerBuf`），使用 `flushedKeys: Set<string>` 做内容级去重，确保不漏不重。
- 客户端（`remote.rs`）：当收到 `exit` 事件但 stdout 为空时，不立即返回，而是设置 `exit_received = true` 并将超时重置为 3 秒（grace period）。如果在 grace period 内收到延迟的 `frame` 事件，立即返回带数据的结果；如果 grace period 超时仍无 frame，则静默返回空结果（不报错）。

**操作步骤**：
1. 确保已有可复用授权
2. 连续执行 10 轮、每轮 4 个命令的稳定性测试：
   ```bash
   for i in $(seq 1 10); do
     echo "Round $i:"
     ./target/debug/bifrost -p 8800 remote status --client-id <client_id> 2>/dev/null | head -c 50
     ./target/debug/bifrost -p 8800 remote traffic list --client-id <client_id> 2>/dev/null | head -c 50
     ./target/debug/bifrost -p 8800 remote traffic get <req_id> --client-id <client_id> 2>/dev/null | head -c 50
     ./target/debug/bifrost -p 8800 remote search httpbin --client-id <client_id> 2>/dev/null | head -c 50
     sleep 0.3
   done
   ```

**预期结果**：
- 10 轮 × 4 命令 = 40 次调用全部成功返回非空结果
- 无间歇性空输出或超时错误
- 每个命令的输出均为有效 JSON 且不重复
- 通过率 100%，不因多实例事件路由差异而丢失数据

---

## 补充覆盖用例测试执行结果（TC-RI-50 ~ TC-RI-59）

测试环境：
- Bifrost: 本地 port 8800，`BIFROST_DATA_DIR=./.bifrost-test`
- Relay Server: 本地 port 8686，`--enable-remote-invoke`
- 测试日期：2026-04-18

| 用例编号 | 用例名称 | 结果 | 说明 |
|---------|---------|------|------|
| TC-RI-50 | 多调用方独立配对与授权 | ✅ PASS | 两个不同 fingerprint 独立配对并获得独立 grant，grant_id 不同 |
| TC-RI-51 | 多调用方 grant 隔离 — 撤销 A 不影响 B | ✅ PASS | 删除 caller-A grant 后 caller-B 仍可执行，caller-A 返回 `grant_not_active` |
| TC-RI-52 | 多调用方并发调用执行 | ✅ PASS | 两个调用方同时发起 status 命令，均成功完成，exit_code=0 |
| TC-RI-53 | 多调用方数据隔离 — 管理端视角 | ✅ PASS | 管理端 calls 列表包含所有调用方记录，每个 call 的 caller_fingerprint 正确匹配 |
| TC-RI-54 | 配对码超时后自动轮换 | ✅ PASS | worker.rs 后台定时器在 pair_code_ttl 过期后自动轮换配对码（观察到 3 次轮换：587604→867953→180434），旧码被正确拒绝（`invalid_pair_code`），当前码配对成功 |
| TC-RI-55 | 并发配对冲突 | ✅ PASS | Caller-A 成功配对→pending_approval ✅；Caller-B 复用已消费码→返回 `pair_code_already_consumed` ✅；Caller-C 用新码配对但存在未决配对→返回 `pair_slot_occupied` ✅ |
| TC-RI-56 | traffic.clear 专项拒绝 & 白名单命令验证 | ✅ PASS | `traffic.clear`/`traffic.delete`/`rule.create`/`config.set`/`system.shutdown` 均返回 400 `unsupported_command`；白名单命令 `status` 正常执行 |
| TC-RI-57 | traffic.list 多条件参数过滤 | ✅ PASS | limit=2 返回 2 条；method=POST 正确过滤；host_contains=httpbin 正确匹配；limit=200 被截断为 MAX_TRAFFIC_LIST_LIMIT(100) |
| TC-RI-58 | grant 数量上限（max_grants_per_client） | ✅ PASS | max_grants_per_client=2 时，前 2 个 grant 正常创建，第 3 个被拒绝返回 `max_grants_exceeded`。修复：按 client_instance_id 维度统计而非 user_id 维度 |
| TC-RI-59 | once 授权调用后 consumed 状态 | ✅ PASS | once 模式 grant 在配对初始调用完成后状态变为 consumed，复用返回 `grant_not_active` |

### 已知设计缺口汇总

所有设计缺口已修复并验证通过：

| 缺口 | 修复方案 | 验证用例 | 状态 |
|------|---------|---------|------|
| 配对码自动轮换 | worker.rs 中增加 `pair_code_ticker` 定时器，过期后自动调用 `maybe_refresh_pair_code()` | TC-RI-54 | ✅ 已修复 |
| `pair_slot_occupied` 冲突检查 | sqlite.ts 新增 `countPendingPairings()`，service.ts 在 `startPairing` 中检查未决配对数 | TC-RI-55 | ✅ 已修复 |
| `pair_code_already_consumed` 错误码 | sse.ts 新增 `consumeClientDiscovery()` 保留已消费 pairCode，`findClientByPairCode` 返回精确错误 | TC-RI-55 | ✅ 已修复 |
| `max_grants_per_client` 上限 | sqlite.ts 新增 `countActiveGrantsForClient()`，service.ts 在 `submitGrantDecision` 中按 client_instance_id 维度检查 | TC-RI-58 | ✅ 已修复 |

## 回归测试执行结果（TC-RI-回归-60 ~ TC-RI-回归-61）

测试环境：
- Bifrost: 本地 port 8800，`BIFROST_DATA_DIR=./.bifrost-test`
- Relay Server: 远端 `https://bifrost.bytedance.net`（bifrost-server-v4）
- 测试日期：2026-04-19

| 用例编号 | 用例名称 | 结果 | 说明 |
|---------|---------|------|------|
| TC-RI-回归-60 | SSE 事件缓冲不应导致 caller 收到重复 frame | ✅ PASS | 40 次调用均返回有效单个 JSON，无 "Extra data" 错误 |
| TC-RI-回归-61 | 多实例 SSE frame/exit 竞态条件不应导致 caller 丢失 frame | ✅ PASS | 10 轮 × 4 命令 = 40/40 全部通过，0 次空输出或超时 |

### TC-RI-回归-62：回归 — 超时配对请求自动从 pending 列表中移除

**背景**：修复前，如果调用方发起 pairing 请求后超时（`pair_code_ttl_secs` 默认 120 秒），该请求会永远留在 pending 列表中。用户点击 Authorize 或 Reject 会收到错误但请求不会消失。

**操作步骤**：
1. 启动 Bifrost 服务和 relay 服务
2. 在 WebUI 进入 Settings > Remote Invoke，开启 Discovery Mode
3. 使用调用方 CLI 发起远程命令触发 pairing 请求：
   ```bash
   bifrost remote status --relay-url http://127.0.0.1:8686 --pair-code <code>
   ```
4. 在 WebUI 观察到 Remote Command Authorization 弹窗或 Pending Requests 区域出现该请求
5. **不进行任何操作**，等待 `pair_code_ttl_secs`（120 秒）超时
6. 等待超时后（最多 5 秒内），观察 Pending Requests 区域

**预期结果**：
- 超时后，该 pairing 请求应在下一个清理周期（5 秒内）自动从 pending 列表中消失
- 如果弹窗中没有其他待处理请求，弹窗自动关闭
- 前端 3 秒一次的轮询会感知到后端已清理掉超时请求，UI 自动更新

---

### TC-RI-回归-63：回归 — 超时后点击 Authorize/Reject 显示友好错误并移除请求

**背景**：即使清理周期尚未触发，用户手动点击 Authorize 或 Reject 一个已经在 relay 侧超时的请求时，也应该有良好的用户体验。

**操作步骤**：
1. 复用 TC-RI-回归-62 的环境，制造一个即将超时的 pairing 请求
2. 等待 relay 侧超时（120 秒）但在后端清理周期（5 秒）到来前迅速操作
3. 点击 Authorize > 选择任意授权期限

**预期结果**：
- 显示 warning 级别的 toast 提示："Request may have expired and was removed"
- 该 pairing 请求从弹窗/列表中自动消失（因为 store 在失败后会重新 fetchPendingList）
- 不显示 error 级别的错误提示

---

## 回归测试执行结果（TC-RI-回归-62 ~ TC-RI-回归-63）

测试环境：
- Bifrost: 本地 port 8800，`BIFROST_DATA_DIR=./.bifrost-test`
- Relay Server: 本地 port 8686
- 测试日期：2026-04-19

| 用例编号 | 用例名称 | 结果 | 说明 |
|---------|---------|------|------|
| TC-RI-回归-62 | 超时配对请求自动从 pending 列表中移除 | ✅ PASS | 等待 130 秒后 pairing 自动消失 |
| TC-RI-回归-63 | 超时后点击 Authorize/Reject 显示友好错误并移除请求 | ✅ PASS | 返回 "not found or expired" 错误，前端自动刷新列表 |

---

## Grant Label 与 Use Count 补充用例

### TC-RI-68：`--label` 参数在 Grants 列表中显示自定义标签

**操作步骤**：
1. 启动 Bifrost 服务和 relay 服务（参考前置条件）
2. 在 WebUI 开启 Discovery Mode，获取 pair code
3. 使用调用方 CLI 带 `--label` 连接：
   ```bash
   bifrost remote conn up --pair-code <code> --relay-url http://127.0.0.1:8686 --label "my-ci-bot"
   ```
4. 在 WebUI 中审批该请求
5. 查看 Settings > Remote Invoke > Grants 列表

**预期结果**：
- Grants 列表中对应 grant 的标题区域显示 `my-ci-bot`，而非 hostname
- API 响应（`GET /api/remote-invoke/grants`）中对应 grant 的 `label` 字段值为 `"my-ci-bot"`
- `caller_display_name` 字段值也为 `"my-ci-bot"`（CLI 以 label 优先作为 display_name）

---

### TC-RI-69：不带 `--label` 时 Grants 列表显示 hostname

**操作步骤**：
1. 使用调用方 CLI 不带 `--label` 连接：
   ```bash
   bifrost remote conn up --pair-code <code> --relay-url http://127.0.0.1:8686
   ```
2. 审批后查看 Grants 列表

**预期结果**：
- Grants 列表中对应 grant 标题显示调用方机器的 hostname
- API 响应中 `label` 字段为 `null` 或不存在
- `caller_display_name` 字段值为调用方 hostname

---

### TC-RI-70-label：`--label` 通过 SSH key 连接时也生效

**操作步骤**：
1. 使用 SSH key 连接并携带 `--label`：
   ```bash
   bifrost remote conn up --ssh-key ~/.bifrost/remote-device.key --label "eden-macbook"
   ```
2. 查看 Grants 列表

**预期结果**：
- Grants 列表中标题显示 `eden-macbook`
- API 响应中 `label` 字段值为 `"eden-macbook"`

---

### TC-RI-71-usecount：Grant 执行命令后 use_count 递增

**操作步骤**：
1. 完成配对授权（pair code 或 SSH key）
2. 执行一条远程命令：
   ```bash
   bifrost remote status
   ```
3. 查看 Grants 列表中对应 grant 的 `use_count`
4. 再执行两条远程命令：
   ```bash
   bifrost remote exec --shell-text "echo hello"
   bifrost remote exec --shell-text "echo world"
   ```
5. 再次查看 Grants 列表

**预期结果**：
- 第一次执行后 `use_count` 从 0 变为 1
- 再执行两条后 `use_count` 变为 3
- API 响应中每个 grant 的 `use_count` 字段值与实际命令执行次数一致

---

### TC-RI-72-total：Grants 卡片标题区域显示命令执行总数

**操作步骤**：
1. 配对两个不同的 caller（使用不同 fingerprint），分别执行若干命令
2. 查看 WebUI Settings > Remote Invoke > Grants 卡片标题区域

**预期结果**：
- Grants 卡片标题区域（Card header）显示 `Total commands: N`
- `N` 等于所有 grant 的 `use_count` 之和
- 如果所有 grant 都未执行过命令，显示 `Total commands: 0`

---

### TC-RI-73-compat：旧版 grant 数据反序列化时 use_count 默认为 0、label 为 null

**操作步骤**：
1. 停止 Bifrost 服务
2. 查看 `.bifrost-test` 数据目录中持久化的 grant 数据（如果有旧格式数据）
3. 启动新版本 Bifrost 服务
4. 查看 Grants 列表

**预期结果**：
- 旧版本创建的 grant（不含 `use_count` 和 `label` 字段）正常加载
- `use_count` 默认为 0
- `label` 为 null，UI 降级显示 `caller_display_name` 或 fingerprint
- 不出现反序列化错误

---

### TC-RI-74-sysinfo：Grants 列表展示 caller 系统信息（平台、版本、架构）

**操作步骤**：
1. 使用调用方 CLI 连接（pair code 或 SSH key）
2. 查看 Settings > Remote Invoke > Grants 列表 description 区域
3. 调用 API `GET /api/remote-invoke/grants` 查看返回 JSON

**预期结果**：
- Grants 列表 description 区域显示系统信息（如 `· 15.5 aarch64` 或 `· Debian GNU/Linux 11 x86_64`）
- API 响应中每个 grant 包含 `os_version` 和 `arch` 字段
- macOS 上 `os_version` 为类似 `15.5` 的版本号，`arch` 为 `aarch64` 或 `x86_64`
- Linux 上 `os_version` 为 `/etc/os-release` 中的 `PRETTY_NAME` 值

---

### TC-RI-78：多客户端在线时未指定 --client-id 触发交互式选择

**前置条件**：至少两个 Bifrost 客户端同时在线且已通过 relay 注册

**操作步骤**：
1. 确认有两个或以上客户端在线：
   ```bash
   curl -s http://127.0.0.1:8686/v4/remote-invoke/clients -H "Authorization: Bearer $SYNC_TOKEN" | jq '.data | length'
   ```
   预期返回数字 >= 2
2. 执行远程命令但不指定 `--client-id`：
   ```bash
   cargo run --bin bifrost -- remote status --relay http://127.0.0.1:8686 --token "$SYNC_TOKEN"
   ```

**预期结果**：
- 不报错退出，而是显示交互式选择菜单，提示 "Found N online clients, select one"
- 列出每个客户端的设备名和短 ID（如 `eden-macbook (a1b2c3d4e5f6)`）
- 使用上下方向键可以在客户端之间切换
- 按回车选择后，命令继续正常执行并返回该客户端的 status 结果

---

### TC-RI-79：多客户端在线时模糊前缀匹配多个客户端触发交互式选择

**前置条件**：至少两个客户端在线且 client_instance_id 有共同前缀

**操作步骤**：
1. 查看在线客户端列表，找到共同的 ID 前缀（如第一个字符）
2. 执行远程命令并传入该模糊前缀：
   ```bash
   cargo run --bin bifrost -- remote status --relay http://127.0.0.1:8686 --token "$SYNC_TOKEN" --client-id "<共同前缀>"
   ```

**预期结果**：
- 不报错退出，而是显示交互式选择菜单，提示 "Prefix '<prefix>' matches N clients, select one"
- 列出匹配前缀的客户端设备名和短 ID
- 选择后命令正常执行

---

### TC-RI-80：非交互式环境下多客户端仍然报错退出

**前置条件**：至少两个客户端在线

**操作步骤**：
1. 通过管道将命令输入重定向，模拟非交互式环境：
   ```bash
   echo "" | cargo run --bin bifrost -- remote status --relay http://127.0.0.1:8686 --token "$SYNC_TOKEN" 2>&1
   ```

**预期结果**：
- 输出错误信息 "multiple clients online, please specify --client-id"
- 命令以非零退出码退出（保持向后兼容，脚本环境不会卡住等待输入）

---

## 交互式客户端选择测试执行结果（TC-RI-78 ~ TC-RI-80）

测试环境：
- Bifrost: 本地 port 8800，`BIFROST_DATA_DIR=./.bifrost-test`
- Relay Server: 本地 port 8686
- 测试日期：2026-04-19

| 用例编号 | 用例名称 | 结果 | 说明 |
|---------|---------|------|------|
| TC-RI-78 | 多客户端在线时未指定 --client-id 触发交互式选择 | 🔲 PENDING | 待执行 |
| TC-RI-79 | 多客户端在线时模糊前缀匹配多个客户端触发交互式选择 | 🔲 PENDING | 待执行 |
| TC-RI-80 | 非交互式环境下多客户端仍然报错退出 | 🔲 PENDING | 待执行 |

---

### TC-RI-81：connect 授权通过后 exit_code 应为 0

**前置条件**：客户端已连接 relay 并开启发现模式

**操作步骤**：
1. 生成配对码并使用 `bifrost remote connect <pair_code>` 发起连接
2. 在 WebUI 中批准授权
3. 连接成功后，查看 WebUI 的 `Recent Calls` 列表中该 connect 记录

**预期结果**：
- CLI 输出 `✓ Connected! Authorization granted`
- WebUI Recent Calls 中 connect 记录的状态标签为绿色 `completed (0)`，而非红色 `completed (-1)`
- 该记录的 exit_code 字段值为 0

---

### TC-RI-82：调用记录展示完整参数信息和响应数据量

**前置条件**：存在可复用授权

**操作步骤**：
1. 执行带参数的远程查询命令，例如：
   ```bash
   cargo run --bin bifrost -- remote search "测试关键词" --relay http://127.0.0.1:8686 --token "$SYNC_TOKEN"
   ```
2. 查看 WebUI `Recent Calls` 列表

**预期结果**：
- 调用记录标题行在命令名称后展示参数预览（如 `query=测试关键词 limit=20`）
- 鼠标悬停参数预览区域时，Tooltip 显示完整的 JSON 参数
- 描述行在耗时后展示响应数据量（如 `· ↓ 1.2KB`）
- connect 类型记录不显示参数预览和数据量（因为没有参数和响应体）

---

### TC-RI-83：回归 — `remote traffic get <sequence>` 按 sequence 正确返回详情

**前置条件**：
- 已有可复用授权
- client 侧正在记录 traffic

**操作步骤**：
1. 通过本地代理发起一个带唯一 marker 的请求，例如：
   ```bash
   MARKER="remote-invoke-$(date +%s)"
   curl -sS --proxy http://127.0.0.1:8800 "http://httpbin.org/anything/${MARKER}" >/dev/null
   ```
2. 在 client 侧查询最新 traffic，记录该请求对应的 `seq`
3. 在 caller 侧执行：
   ```bash
   cargo run --bin bifrost -- remote traffic get <seq> --relay http://127.0.0.1:8686 --client-id <client_prefix>
   ```

**预期结果**：
- 命令成功退出，不显示 `Remote command 'traffic.get' exited with code -1`
- 输出 JSON 中包含该 `seq`
- 输出详情包含步骤 1 中的 marker 路径

---

### TC-RI-84：回归 — `remote search` 采用流式输出并返回命中结果

**前置条件**：
- 已有可复用授权
- client 侧存在包含唯一 marker 的 traffic

**操作步骤**：
1. 准备一个唯一 marker，并通过代理产生对应请求
2. 在 caller 侧执行：
   ```bash
   cargo run --bin bifrost -- remote search "<marker>" --relay http://127.0.0.1:8686 --client-id <client_prefix> --limit 5
   ```
3. 观察命令执行过程中的终端输出

**预期结果**：
- 命令执行过程中先出现搜索进度文本（如 `Searching...`），而不是长时间无输出
- 搜索结果表格中包含 `<marker>` 对应的记录
- 结尾 summary 显示 `Found 1 matches` 或与实际命中数一致的结果
- 命令成功退出，不显示 `Remote command 'search.get' exited with code -1`

---

### TC-RI-85：回归 — 远程命令失败时 caller 能看到真实错误文本

**前置条件**：已有可复用授权

**操作步骤**：
1. 在 caller 侧执行一个必然失败的查询，例如：
   ```bash
   cargo run --bin bifrost -- remote traffic get 999999999 --relay http://127.0.0.1:8686 --client-id <client_prefix>
   ```
2. 观察终端输出

**预期结果**：
- 终端输出包含真实错误原因（如 `No traffic record with sequence suffix '999999999' found`）
- 不再只有 `Remote command 'traffic.get' exited with code -1` 这一行空错误
- 调用记录仍保留失败状态，便于审计

---

### TC-RI-86：回归 — `remote search` 的 `--max-results` / `--max-scan` 在执行端生效

**前置条件**：
- 已有可复用授权
- client 侧存在包含唯一 marker 的 traffic

**操作步骤**：
1. 记录当前 `Recent Calls` 中最新一条 `search.get` 的 `started_at`
2. 在 caller 侧执行：
   ```bash
   cargo run --bin bifrost -- remote search "<marker>" --relay http://127.0.0.1:8686 --client-id <client_prefix> --max-results 5 --max-scan 50
   ```
3. 命令完成后，在 client 管理端查询 `Recent Calls` 或调用明细，定位这次新的 `search.get`
4. 查看该次调用保存的参数信息（如 `command.args_json`）

**预期结果**：
- 命令成功退出，并返回 `<marker>` 对应命中记录
- 终端仍然显示流式搜索进度
- 新生成的 `search.get` 调用参数中包含 `max_results=5`
- 新生成的 `search.get` 调用参数中包含 `max_scan=50`
- 限制参数来自执行端保存的调用参数，而不是 caller 侧本地截断输出

---

### TC-RI-87：回归 — `remote traffic list` 的过滤参数完整透传并执行正确

**前置条件**：
- 已有可复用授权
- client 侧存在一条包含唯一 marker 的 HTTP 流量

**操作步骤**：
1. 记录当前 `Recent Calls` 中最新一条 `traffic.list` 的 `started_at`
2. 在 caller 侧执行：
   ```bash
   cargo run --bin bifrost -- remote traffic list --relay http://127.0.0.1:8686 --client-id <client_prefix> \
     --limit 7 --cursor 123 --direction forward --method GET \
     --status-min 200 --status-max 299 --protocol http --host httpbin \
     --url "/anything/<marker>" --path "/anything" --content-type application/json \
     --client-app curl --has-rule-hit false --is-websocket false --is-sse false --is-tunnel false
   ```
3. 验证命令输出中出现目标流量记录或列表表头
4. 在 client 管理端查询新的 `traffic.list` 调用，读取其 `command.args_json`

**预期结果**：
- `remote traffic list` 成功执行
- 输出与过滤条件相匹配，不出现参数导致的执行错误
- `command.args_json` 中完整包含本次传入的 list/filter 参数
- 参数来自执行端落库的调用记录，而不是 caller 本地日志推断

---

### TC-RI-88：回归 — `remote traffic search` 的 `--max-results` / `--max-scan` 透传后结果正确

**前置条件**：
- 已有可复用授权
- client 侧存在一条包含唯一 marker 的 HTTP 流量

**操作步骤**：
1. 记录当前 `Recent Calls` 中最新一条 `traffic.search` 的 `started_at`
2. 在 caller 侧执行：
   ```bash
   cargo run --bin bifrost -- remote traffic search "<marker>" --relay http://127.0.0.1:8686 --client-id <client_prefix> --max-results 3 --max-scan 30
   ```
3. 观察 caller 终端是否先输出 `Searching...`，随后返回 `<marker>` 命中
4. 在 client 管理端查询新的 `traffic.search` 调用，读取其 `command.args_json`

**预期结果**：
- `remote traffic search` 成功执行，且结果中包含 `<marker>`
- caller 侧输出包含流式进度
- `command.args_json` 中包含 `query=<marker>`、`max_results=3`、`max_scan=30`
- 执行结果与参数约束一致，不出现 caller 本地假截断

---

## connect exit_code 与调用记录展示修复测试执行结果（TC-RI-81 ~ TC-RI-82）

测试环境：
- Bifrost: 本地 port 8800，`BIFROST_DATA_DIR=./.bifrost-test`
- Relay Server: 本地或远端
- 测试日期：2026-04-19

| 用例编号 | 用例名称 | 结果 | 说明 |
|---------|---------|------|------|
| TC-RI-81 | connect 授权通过后 exit_code 应为 0 | ✅ PASS | connect 记录显示绿色 completed (0)，旧记录仍为 -1（历史数据） |
| TC-RI-82 | 调用记录展示完整参数信息和响应数据量 | ✅ PASS | search.get 显示 limit=50 query=测试关键词 及 ↓ 122B；status 显示 ↓ 110B |

## remote traffic/get/search 参数透传与错误回归测试执行结果（TC-RI-83 ~ TC-RI-88）

测试环境：
- Bifrost: 本地 port 8800，`BIFROST_DATA_DIR=./.bifrost-test`
- Relay Server: 本地 8686/8687
- 测试日期：2026-04-20

| 用例编号 | 用例名称 | 结果 | 说明 |
|---------|---------|------|------|
| TC-RI-83 | `remote traffic get <sequence>` 按 sequence 正确返回详情 | ✅ PASS | 本轮 E2E 通过真实 `seq` 调用 `remote traffic get --request-body --response-body`，成功返回目标 marker 对应详情，且 `Recent Calls` 中 `traffic.get` 的 `args_json` 正确记录 `id`、`request_body=true`、`response_body=true`。 |
| TC-RI-84 | `remote search` 采用流式输出并返回命中结果 | ✅ PASS | caller 终端先看到 `Searching...`，随后输出目标 `remote-invoke-*` 命中记录，summary 为 `Found 1 matches`。 |
| TC-RI-85 | 远程命令失败时 caller 能看到真实错误文本 | ✅ PASS | `remote traffic get 999999999` 终端明确显示 `No traffic record with sequence suffix '999999999' found`，不再只有 exit code -1。 |
| TC-RI-86 | `remote search` 将 `--max-results` / `--max-scan` 透传到执行端 | ✅ PASS | 本轮 `bash e2e-tests/tests/test_remote_invoke_e2e.sh` 中 `search.get` 的 `args_json` 记录 `max_results=5`、`max_scan=50`，caller 输出与命中结果一致。 |
| TC-RI-87 | `remote traffic list` 将 list/filter 参数透传到执行端 | ✅ PASS | 本轮 E2E 使用 `limit/cursor/direction/method/status_min/status_max/protocol/host/url/path/content_type/client_app/has_rule_hit/is_websocket/is_sse/is_tunnel` 发起 `traffic.list`，`Recent Calls` 中 `args_json` 完整记录这些参数，命令执行成功并返回流量列表。 |
| TC-RI-88 | `remote traffic search` 将 `query/max_results/max_scan` 透传到执行端 | ✅ PASS | 本轮 E2E 使用 `<marker> + max_results=3 + max_scan=30` 发起 `traffic.search`，caller 侧先看到 `Searching...` 再返回目标命中，`Recent Calls` 中 `args_json` 正确记录 `query`、`max_results`、`max_scan`。 |

---

### TC-RI-回归-64：回归 — 过期/已消耗 grant 自动从列表中移除

**背景**：修复前，过期（expired）、已消耗（consumed）、已撤销（revoked）和已移除（removed）状态的 grant 会永远留在 relay 中，WebUI 的 Grants 列表和 CLI 的 remote status 都会显示这些"幽灵"条目，占用空间且混淆视线。

**修复方案**：
- `worker.rs` 新增 `list_grants_and_cleanup()` 方法：拉取 grants 列表后，识别状态为 expired/consumed/removed/revoked 的条目，以及 `expires_at < now` 的条目，从返回值中过滤掉并异步调用 `delete_grant()` 从 relay 删除
- `handle_grants_list` handler 改用 `list_grants_and_cleanup()` 替代原始 `list_grants()`
- SSE 循环新增 60 秒周期性 grant 清理定时器

**操作步骤**：
1. 启动 Bifrost 服务连接 relay
2. 创建一个 `once` 模式的授权（一次性使用）：
   - 开启 Discovery Mode
   - 使用 `bifrost remote connect <code>` 发起配对
   - 在 WebUI 中选择 "once" 模式授权
3. 使用该授权执行一次远程命令（如 `remote status`），使其变为 `consumed` 状态
4. 等待 3 秒（WebUI 轮询周期），刷新 Grants 列表
5. 或直接通过 API 检查：
   ```bash
   curl -s http://127.0.0.1:<port>/api/remote-invoke/grants | python3 -m json.tool
   ```

**预期结果**：
- 已消耗的 once grant 不再出现在 Grants 列表中
- 如果查看 relay 日志，应能看到 `expired grants cleanup completed` 相关日志
- 列表中只保留 status 为 `active` 且未过期的 grant

---

### TC-RI-回归-65：回归 — 周期性后台清理确保即使 WebUI 未打开也能清理

**背景**：即使没有用户通过 WebUI 主动拉取 grants 列表，worker 的 SSE 循环中也会每 60 秒自动执行一次 `periodic_grant_cleanup()`，确保过期 grant 不会无限累积。

**操作步骤**：
1. 启动 Bifrost 服务并确认已连接 relay（通过日志观察 "SSE stream connected"）
2. 创建一个 `30m` 模式的授权并使用，使其有 expires_at
3. 手动将该 grant 在 relay 侧标记为 expired（或等待其自然过期）
4. 不打开 WebUI，观察 Bifrost 日志，等待 60 秒

**预期结果**：
- 日志中出现 `periodic grant cleanup check done` 消息
- 如果有过期 grant，日志中出现 `cleaning up expired/dead grants from relay` 和 `expired grants cleanup completed`
- 后续通过 API 查询 grants 列表，该过期 grant 已被删除

---

## 回归测试执行结果（TC-RI-回归-64 ~ TC-RI-回归-65）

测试环境：
- Bifrost: 本地 port 8800，`BIFROST_DATA_DIR=./.bifrost-test`
- Relay Server: 远端 `https://bifrost.bytedance.net`
- 测试日期：2026-04-19

| 用例编号 | 用例名称 | 结果 | 说明 |
|---------|---------|------|------|
| TC-RI-回归-64 | 过期/已消耗 grant 自动从列表中移除 | 🔲 PENDING | 待执行 |
| TC-RI-回归-65 | 周期性后台清理确保即使 WebUI 未打开也能清理 | 🔲 PENDING | 待执行 |

---

### TC-RI-回归-66：回归 — 客户端侧 DELETE grant 路由正常工作（Relay Server）

**背景**：新增 `DELETE /v4/remote-invoke/client/grants/:grantId` 路由，允许客户端（Bifrost 实例）通过 `client_auth_token` 认证后主动删除自己的 grant。该路由通过 `client_instance_id` 验证归属，而非 `caller_fingerprint`。

**前置条件**：
- Bifrost 客户端已连接 Relay 并拥有有效的 `client_auth_token`
- 至少存在一个活跃的 grant

**操作步骤**：
1. 获取当前客户端的 client_instance_id 和 client_auth_token：
   ```bash
   curl -s http://127.0.0.1:8800/_bifrost/api/remote-invoke/status | jq '{client_instance_id: .client_instance_id}'
   ```
2. 查看当前活跃 grants 列表（使用 Relay API），记录一个 grant_id：
   ```bash
   curl -s "http://127.0.0.1:8686/v4/remote-invoke/grants/reusable?client_instance_id=<cid>&caller_fingerprint=<fp>" | jq .
   ```
3. 使用客户端认证身份发起 DELETE 请求：
   ```bash
   curl -s -X DELETE "http://127.0.0.1:8686/v4/remote-invoke/client/grants/<grant_id>?client_instance_id=<cid>&client_auth_token=<token>" | jq .
   ```
4. 再次查询该 grant 是否仍可复用：
   ```bash
   curl -s "http://127.0.0.1:8686/v4/remote-invoke/grants/reusable?client_instance_id=<cid>&caller_fingerprint=<fp>" | jq .
   ```

**预期结果**：
- 步骤 3 返回 `{"code": 0, "message": "ok", "data": null}`
- 步骤 4 返回的 data 为 null（grant 已被移除，不可复用）
- 客户端 SSE 流收到 `grant_revoked` 事件
- grant 状态变为 `removed`

---

### TC-RI-回归-67：回归 — 客户端侧 DELETE grant 验证 client_instance_id 归属

**背景**：客户端侧 DELETE grant 必须验证 grant 的 `client_instance_id` 与请求者的 `client_instance_id` 匹配，防止跨客户端越权删除。

**操作步骤**：
1. 使用客户端 A 的认证信息尝试删除属于客户端 B 的 grant：
   ```bash
   curl -s -X DELETE "http://127.0.0.1:8686/v4/remote-invoke/client/grants/<grant_id_of_client_B>?client_instance_id=<cid_A>&client_auth_token=<token_A>" | jq .
   ```

**预期结果**：
- 返回 HTTP 403 状态码
- 错误信息为 `client_instance_id_mismatch`
- 客户端 B 的 grant 未被修改

---

### TC-RI-回归-68：回归 — 客户端侧 DELETE grant 不存在的 grant 返回 404

**操作步骤**：
1. 使用客户端认证信息尝试删除不存在的 grant：
   ```bash
   curl -s -X DELETE "http://127.0.0.1:8686/v4/remote-invoke/client/grants/nonexistent_grant_id?client_instance_id=<cid>&client_auth_token=<token>" | jq .
   ```

**预期结果**：
- 返回 HTTP 404 状态码
- 错误信息为 `grant_not_found`

---

### TC-RI-回归-69：回归 — 客户端侧 DELETE grant 缺少 client_instance_id 返回 400

**操作步骤**：
1. 发起 DELETE 请求但不提供 client_instance_id：
   ```bash
   curl -s -X DELETE "http://127.0.0.1:8686/v4/remote-invoke/client/grants/<grant_id>?client_auth_token=<token>" | jq .
   ```

**预期结果**：
- 返回 HTTP 400 或 401 状态码
- 错误信息提示 client_instance_id 缺失

---

### TC-RI-回归-70：回归 — Sync Server 客户端侧 DELETE grant 路由正常工作

**背景**：Sync Server 版本的 `DELETE /v4/remote-invoke/client/grants/:grantId` 路由使用 SQLite 持久化存储，通过 `storage.remoteInvoke.updateGrant()` 将 grant 状态设为 `removed`。

**前置条件**：
- 使用本地 sync server（端口 8686）
- 已有活跃 grant

**操作步骤**：
1. 查看当前活跃 grants 列表：
   ```bash
   curl -s "http://127.0.0.1:8686/v4/remote-invoke/grants/reusable?client_instance_id=<cid>&caller_fingerprint=<fp>" | jq .
   ```
2. 使用客户端身份发起 DELETE 请求（通过 body 传递 client_instance_id）：
   ```bash
   curl -s -X DELETE "http://127.0.0.1:8686/v4/remote-invoke/client/grants/<grant_id>" \
     -H "Content-Type: application/json" \
     -d '{"client_instance_id": "<cid>"}' | jq .
   ```
3. 验证 grant 已被移除：
   ```bash
   curl -s "http://127.0.0.1:8686/v4/remote-invoke/grants/reusable?client_instance_id=<cid>&caller_fingerprint=<fp>" | jq .
   ```

**预期结果**：
- 步骤 2 返回 `{"code": 0, "message": "ok"}`
- 步骤 3 返回的 data 为 null
- SQLite 数据库中该 grant 的 status 字段为 `removed`
- 客户端 SSE 流收到 `grant_revoked` 事件

---

### TC-RI-回归-71：回归 — relay_token 鉴权：events 路由无 token 返回 401

**背景**：安全加固后，`GET /calls/:callId/events`、`POST /calls/:callId/input`、`POST /calls/:callId/cancel` 三个路由必须携带有效的 `relay_token`（Bearer）。此前仅靠 `callId` 不可预测性保护，存在枚举/窃听风险。修复方案：在 `openCall` 时将 `relay_token` 存入 Redis/内存，上述路由提取 Bearer token 并验证。

**操作步骤**：
1. 使用一个虚拟的 callId 直接访问 events 路由，不携带 Authorization 头：
   ```bash
   curl -sS -o /dev/null -w '%{http_code}' \
     "https://bifrost.bytedance.net/v4/remote-invoke/calls/00000000-0000-0000-0000-000000000000/events"
   ```

**预期结果**：
- 返回 HTTP 401
- 不返回 SSE 数据流

---

### TC-RI-回归-72：回归 — relay_token 鉴权：events 路由错误 token 返回 401

**操作步骤**：
1. 使用错误的 Bearer token 访问 events 路由：
   ```bash
   curl -sS -o /dev/null -w '%{http_code}' \
     -H "Authorization: Bearer fake-token-12345" \
     "https://bifrost.bytedance.net/v4/remote-invoke/calls/00000000-0000-0000-0000-000000000000/events"
   ```

**预期结果**：
- 返回 HTTP 401
- 不返回 SSE 数据流

---

### TC-RI-回归-73：回归 — relay_token 鉴权：input 路由无 token 返回 401

**操作步骤**：
1. 使用虚拟 callId 直接 POST input 路由，不携带 Authorization：
   ```bash
   curl -sS -o /dev/null -w '%{http_code}' \
     -X POST "https://bifrost.bytedance.net/v4/remote-invoke/calls/00000000-0000-0000-0000-000000000000/input" \
     -H "Content-Type: application/json" \
     -d '{"data":"test"}'
   ```

**预期结果**：
- 返回 HTTP 401

---

### TC-RI-回归-74：回归 — relay_token 鉴权：cancel 路由无 token 返回 401

**操作步骤**：
1. 使用虚拟 callId 直接 POST cancel 路由，不携带 Authorization：
   ```bash
   curl -sS -o /dev/null -w '%{http_code}' \
     -X POST "https://bifrost.bytedance.net/v4/remote-invoke/calls/00000000-0000-0000-0000-000000000000/cancel"
   ```

**预期结果**：
- 返回 HTTP 401

---

### TC-RI-回归-75：回归 — calls 列表和详情路由已移至 client 路径

**背景**：`GET /calls` 和 `GET /calls/:callId` 被移至 `/client/calls` 前缀路径下，需要 `requireClientAuth` 中间件验证 `client_auth_token`。原路径不再可用。

**操作步骤**：
1. 访问原路径 `GET /v4/remote-invoke/calls`（不携带 client 认证）：
   ```bash
   curl -sS -o /dev/null -w '%{http_code}' \
     "https://bifrost.bytedance.net/v4/remote-invoke/calls"
   ```
2. 访问新路径 `GET /v4/remote-invoke/client/calls`（不携带 client 认证）：
   ```bash
   curl -sS -o /dev/null -w '%{http_code}' \
     "https://bifrost.bytedance.net/v4/remote-invoke/client/calls"
   ```

**预期结果**：
- 步骤 1：返回 HTTP 404（路由已不存在）
- 步骤 2：返回 HTTP 401（缺少 client 认证）

---

### TC-RI-回归-76：回归 — approve_pairing 正确提取 caller_fingerprint

**背景**：修复前 `approve_pairing` 中 `caller_fingerprint` 始终为空字符串，因为从 relay 响应中提取，但 relay 的 `submitGrantDecision` 不返回 `caller_fingerprint`。修复后从本地 `pending_pairings` 缓存中提取。

**操作步骤**：
1. 完成一次完整的配对流程（生成 pair_code → connect → approve）
2. 查看 Client 侧 grants 列表：
   ```bash
   curl -s http://127.0.0.1:<port>/_bifrost/api/remote-invoke/grants | jq '.grants[0].caller_fingerprint'
   ```

**预期结果**：
- `caller_fingerprint` 不为空字符串
- 值为调用方的真实设备指纹哈希（如 `sha256:xxxxxxxx`）

---

### TC-RI-回归-77：回归 — delete_grant 在 relay 异常时仍清理本地状态

**背景**：修复前 `delete_grant` 先删本地再删远端，如果远端失败则本地已删但远端残留；后改为先远端后本地，但远端失败导致本地也无法删除。最终修复为：无论远端成败，本地总是删除，远端失败记 warn 日志。

**操作步骤**：
1. 完成一次配对流程获得 grant
2. 通过 Admin API 删除 grant（DELETE 方法）：
   ```bash
   curl -s -X DELETE http://127.0.0.1:<port>/_bifrost/api/remote-invoke/grants/<grant_id> | jq .
   ```
3. 查看 grants 列表确认本地已删除

**预期结果**：
- API 返回成功
- 本地 grants 列表中该 grant 已消失
- 即使 relay 暂时不可达，本地删除仍然成功

---

### TC-RI-回归-78：回归 — call_open 事件必须验证本地 grant 存在且有效

**背景**：修复前 `handle_call_open` 收到 SSE call_open 事件后直接解析并执行命令，未校验 `grant_id` 是否在本地 `local_grants` 中存在且处于有效状态。攻击者可通过 relay 中间人伪造 call_open 事件，携带任意或空的 grant_id 绕过授权直接执行命令。修复后新增完整的 grant 验证链：空 grant_id → grant 不存在 → grant 已死亡（过期/撤销/已消费） → 状态非 Active → remaining_calls 耗尽 → 全部拒绝并返回 exit_code -2。

**前置条件**：
- Bifrost 服务已启动，remote invoke 模块已加载

**操作步骤**：
1. 运行 `validate_grant_for_call` 相关的 9 个单元测试，验证各分支逻辑：
   ```bash
   cargo test -p bifrost-admin test_validate_grant -- --nocapture
   ```
2. 确认以下场景均被测试覆盖：
   - 不存在的 grant_id → 返回 reject reason "not found"
   - Active + Permanent grant → 通过验证，更新 last_used_at
   - Active + Once grant（remaining_calls=1）→ 通过验证，remaining_calls 降为 0，状态变为 Consumed
   - Once grant（remaining_calls=0）→ 拒绝，状态标记为 Consumed
   - 时间过期的 grant（expires_at < now）→ 拒绝（dead）
   - 已撤销的 grant（status=Revoked）→ 拒绝（dead）
   - 未过期的 grant（expires_at > now）→ 通过验证
   - Expired 状态的 grant → is_grant_info_dead 返回 true
   - 时间过期检测 → is_grant_info_dead 正确判断

**预期结果**：
- 所有 9 个单元测试通过（test result: ok. 9 passed）
- 空 grant_id 的 call_open 事件被拒绝（exit_code -2），日志包含 `SECURITY: call_open rejected`
- 不存在的 grant_id 被拒绝
- 过期/已撤销/已消费的 grant 被拒绝
- 仅 Active 且未过期、有剩余调用次数的 grant 允许执行
- Once 模式 grant 在使用后自动标记为 Consumed

---

## 回归测试执行结果（TC-RI-回归-71 ~ TC-RI-回归-78）

测试环境：
- Bifrost: 本地，`BIFROST_DATA_DIR` 临时目录
- Relay Server: 远端 `https://bifrost.bytedance.net`
- 测试日期：2026-04-20

| 用例编号 | 用例名称 | 结果 | 说明 |
|---------|---------|------|------|
| TC-RI-回归-71 | events 路由无 token 返回 401 | ✅ PASS | 返回 HTTP 401 |
| TC-RI-回归-72 | events 路由错误 token 返回 401 | ✅ PASS | 返回 HTTP 401 |
| TC-RI-回归-73 | input 路由无 token 返回 401 | ✅ PASS | 返回 HTTP 401 |
| TC-RI-回归-74 | cancel 路由无 token 返回 401 | ✅ PASS | 返回 HTTP 401 |
| TC-RI-回归-75 | calls 列表和详情路由已移至 client 路径 | ✅ PASS | 原路径返回 404，新路径无认证返回 401 |
| TC-RI-回归-76 | approve_pairing 正确提取 caller_fingerprint | ✅ PASS | caller_fingerprint=b92541d979bc949e67bbb3ae4bde4cb4，非空 |
| TC-RI-回归-77 | delete_grant 在 relay 异常时仍清理本地状态 | ✅ PASS | DELETE 返回 success:true，grant 数量从 2 降为 1 |
| TC-RI-回归-78 | call_open 事件必须验证本地 grant 存在且有效 | ✅ PASS | 9 个单元测试全部通过：missing grant 拒绝、permanent 通过、once 消费、consumed 拒绝、expired 拒绝、revoked 拒绝、not-yet-expired 通过、dead 状态检测、时间过期检测 |

---

### TC-RI-回归-79：真实 relay E2E — 完整配对流程（discovery → connect → approve）

**背景**：使用真实 relay 服务（`bifrost.bytedance.net`）而非本地 mock 验证完整配对流程的端到端可靠性。

**操作步骤**：
1. 启动 Bifrost 服务连接真实 relay：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-grant-test RUST_LOG=debug cargo run --bin bifrost -- start -p 8801 --unsafe-ssl --no-system-proxy
   ```
2. 确认服务状态为 Connected：
   ```bash
   curl -s http://127.0.0.1:8801/_bifrost/api/remote-invoke/status
   ```
3. 进入 discovery 模式：
   ```bash
   curl -s -X POST http://127.0.0.1:8801/_bifrost/api/remote-invoke/discovery/enter
   ```
4. 使用 caller CLI 发起配对：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-caller-test ./target/debug/bifrost remote --relay-url https://bifrost.bytedance.net connect <pair_code>
   ```
5. 查看 pending pairings 并批准：
   ```bash
   curl -s http://127.0.0.1:8801/_bifrost/api/remote-invoke/pairings/pending
   curl -s -X POST 'http://127.0.0.1:8801/_bifrost/api/remote-invoke/pairings/<pairing_id>/approve' -H 'Content-Type: application/json' -d '{"grant_mode":"30m"}'
   ```

**预期结果**：
- 服务成功连接真实 relay，状态为 `Connected`
- Discovery 模式正常开启，返回 6 位配对码
- Caller CLI 显示 `⏳ Waiting for approval...`
- Pending pairings 中出现配对请求，包含 caller_info
- 批准后 caller CLI 显示 `✓ Connected! Authorization granted`
- Grants 列表中出现新的 active grant

---

### TC-RI-回归-80：真实 relay E2E — grant 有效时命令执行成功

**前置条件**：已完成 TC-RI-回归-79

**操作步骤**：
1. 使用 caller CLI 执行远程 status 命令：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-caller-test ./target/debug/bifrost remote --relay-url https://bifrost.bytedance.net status
   ```
2. 使用 caller CLI 执行远程 traffic list 命令：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-caller-test ./target/debug/bifrost remote --relay-url https://bifrost.bytedance.net traffic list --limit 3
   ```

**预期结果**：
- `status` 命令返回有效 JSON（含 version, os, uptime 等），exit_code=0
- `traffic list` 命令返回有效 JSON（含 records 数组），exit_code=0
- 两个命令均通过 grant 验证，无 SECURITY 拒绝日志

---

### TC-RI-回归-81：真实 relay E2E — 已撤销 grant 被安全策略拒绝（exit_code=-2）

**前置条件**：已完成 TC-RI-回归-80

**操作步骤**：
1. 通过 Admin API 撤销 grant：
   ```bash
   curl -s -X DELETE 'http://127.0.0.1:8801/_bifrost/api/remote-invoke/grants/<grant_id>'
   ```
2. 确认 grants 列表为空
3. 再次用 caller 执行命令：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-caller-test ./target/debug/bifrost remote --relay-url https://bifrost.bytedance.net status
   ```

**预期结果**：
- Grant 撤销成功，列表为空
- 再次执行命令时输出 `Remote command 'status' exited with code -2`
- Exit code 为 254（即 -2 的 unsigned 表示）
- 服务端 `validate_grant_for_call` 拒绝了不在 `local_grants` 中的 grant

---

### TC-RI-回归-82：真实 relay E2E — once 模式 grant 单次使用后被拒绝

**操作步骤**：
1. 重新配对并使用 `once` 模式批准
2. 确认 grant 为 active，max_calls=1, remaining_calls=1
3. 第一次执行远程命令（应成功）：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-caller-test ./target/debug/bifrost remote --relay-url https://bifrost.bytedance.net status
   ```
4. 确认 grants 列表为空（once grant 已消耗并清理）
5. 第二次执行远程命令（应被拒绝）：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-caller-test ./target/debug/bifrost remote --relay-url https://bifrost.bytedance.net status
   ```

**预期结果**：
- 第一次执行：返回有效状态数据，exit_code=0
- Once grant 被消耗：remaining_calls 从 1 降为 0，状态变为 consumed，并从列表中清理
- 第二次执行：输出 `✗ Authorization expired or revoked`，exit_code=1

---

## 真实 relay E2E 测试执行结果（TC-RI-回归-79 ~ TC-RI-回归-82）

测试环境：
- Bifrost: 本地 port 8801，`BIFROST_DATA_DIR=./.bifrost-grant-test`
- Relay Server: 远端 `https://bifrost.bytedance.net`
- 测试日期：2026-04-20

| 用例编号 | 用例名称 | 结果 | 说明 |
|---------|---------|------|------|
| TC-RI-回归-79 | 真实 relay E2E — 完整配对流程 | ✅ PASS | discovery→connect→approve 全链路通过，pairing_id=ae4bee5a70455690，grant_id=7c356d63fa387c13(30m) |
| TC-RI-回归-80 | 真实 relay E2E — grant 有效时命令执行成功 | ✅ PASS | status 返回 version=0.0.56-beta/macos/aarch64，traffic list 返回空列表，exit_code=0 |
| TC-RI-回归-81 | 真实 relay E2E — 已撤销 grant 被安全策略拒绝 | ✅ PASS | 撤销后执行命令返回 exit_code=-2(254)，validate_grant_for_call 拒绝了不存在的 grant |
| TC-RI-回归-82 | 真实 relay E2E — once 模式 grant 单次使用后被拒绝 | ✅ PASS | 第一次成功(exit=0)→grant consumed→第二次被拒(exit=1, "Authorization expired or revoked") |

---

## 回归测试：执行端调用日志记录（TC-RI-回归-83 ~ TC-RI-回归-84）

> Bug 背景：执行端（executor）完成远程命令后不记录调用历史，WebUI "Recent Calls" 始终显示 "No recent calls"，`GET /api/remote-invoke/calls` 返回空列表。根因是 `active_calls` 仅保存运行中调用 ID，命令结束后立即删除、无持久化历史。修复方案：在 worker 中增加 `call_history: VecDeque<CallInfo>` 环形缓冲，每次调用开始时写入 Streaming 状态记录，结束时更新为 Completed/Failed。

### TC-RI-回归-83：执行端 Recent Calls 记录已完成调用

**前置条件**：
- Bifrost 执行端已启动并通过 relay 完成配对，状态为 Connected
- 至少存在一个有效 grant

**操作步骤**：
1. 通过 caller 端发起一条远程命令（如 `bifrost status`）并等待返回
2. 在执行端查询调用列表：`curl http://localhost:8800/api/remote-invoke/calls`
3. 检查返回的 JSON

**预期结果**：
- 返回 `{"calls": [...]}` 且数组非空
- 数组中的每个元素为完整的 `CallInfo` 对象，包含以下字段：
  - `call_id`：非空字符串
  - `grant_id`：非空字符串，对应使用的 grant
  - `status`：`"completed"` 或 `"failed"`
  - `exit_code`：整数（成功时为 0）
  - `started_at`：Unix 时间戳（秒），大于 0
  - `duration_ms`：执行耗时毫秒数，大于 0
  - `command`：执行的命令字符串
  - `caller_fingerprint`：调用方指纹
- 最近的调用排在数组第一位（逆序排列）

### TC-RI-回归-84：调用列表按逆序排列且受 max_records 限制

**前置条件**：
- 同 TC-RI-回归-83

**操作步骤**：
1. 通过 caller 端连续发起 2 条不同远程命令（如 `bifrost status`、`bifrost traffic list`）
2. 查询调用列表：`curl http://localhost:8800/api/remote-invoke/calls`
3. 检查返回的 JSON 中 calls 数组的顺序

**预期结果**：
- calls 数组包含 2 条记录
- 第一条记录对应最后执行的命令（如 `bifrost traffic list`），第二条对应先执行的命令（如 `bifrost status`）
- 每条记录的 `started_at` 字段满足：`calls[0].started_at >= calls[1].started_at`

---

## 安全回归测试：client 注册与调用详情隔离（TC-RI-回归-85 ~ TC-RI-回归-88）

### TC-RI-回归-85：client 注册 challenge/register 必须携带 x-bifrost-token

**操作步骤**：
1. 直接请求 register challenge，不带 `x-bifrost-token`：
   ```bash
   curl -s -o /tmp/ri-no-token-challenge.json -w '%{http_code}' \
     -X POST http://127.0.0.1:8686/v4/remote-invoke/client/register/challenge \
     -H 'Content-Type: application/json' \
     -d '{"client_instance_id":"unauthorized-client"}'
   ```
2. 直接请求 register，不带 `x-bifrost-token`：
   ```bash
   curl -s -o /tmp/ri-no-token-register.json -w '%{http_code}' \
     -X POST http://127.0.0.1:8686/v4/remote-invoke/client/register \
     -H 'Content-Type: application/json' \
     -d '{"challenge_id":"missing","client_instance_id":"unauthorized-client","client_long_term_pubkey":"Zm9v","device_name":"unauthorized-device","platform":"macos","bifrost_version":"0.0.0-test","signature":"YmFy","timestamp":1700000000}'
   ```

**预期结果**：
- 两个请求都返回 `401`
- 返回消息明确指出缺少 `x-bifrost-token` 或未授权
- 不能在未登录 relay 的情况下为任意 `client_instance_id` 申请 challenge 或刷新 client token

### TC-RI-回归-86：client 注册必须通过 challenge + 私钥签名校验，challenge 不可重放

**前置条件**：
- 已通过 `/v4/sso/register` 或 `/v4/sso/login` 获取 `SYNC_TOKEN`

**操作步骤**：
1. 携带 `x-bifrost-token: $SYNC_TOKEN` 调用 `/v4/remote-invoke/client/register/challenge`
2. 使用返回的 `challenge_id`、`challenge` 与 client 长期私钥对注册负载签名，再调用 `/v4/remote-invoke/client/register`
3. 记录首次 register 的返回结果
4. 使用相同的 `challenge_id`、`signature`、`timestamp` 再次调用一次 register
5. 再申请一个新的 challenge，但把 `signature` 替换成伪造值后调用 register

**预期结果**：
- 首次 register 成功，返回新的 `client_auth_token`
- 第二次重放同一 challenge 返回失败，消息为 `registration_challenge_not_found` 或等效的一次性 challenge 已消费错误
- 伪造签名的请求返回失败，消息为 `invalid_registration_signature` 或等效错误
- relay 不会在签名无效或 challenge 被消费后签发新的 `client_auth_token`

### TC-RI-回归-87：caller 发起配对/连接认证仍然不需要 x-bifrost-token

**前置条件**：
- 执行端已成功注册到 relay，状态为 `Connected`
- 已进入 discovery 模式并拿到有效 `pair_code`

**操作步骤**：
1. 使用 caller CLI 发起连接，不传任何 `x-bifrost-token`：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-caller-test ./target/debug/bifrost remote --relay-url http://127.0.0.1:8686 connect <pair_code>
   ```
2. 在执行端批准该 pairing

**预期结果**：
- caller 可以正常进入 `Waiting for approval...`
- 批准后 caller 可以成功完成连接
- 整个 caller -> relay 的配对/连接链路不要求 `x-bifrost-token`
- 安全边界保持为：caller 依赖 `pair_code + caller_fingerprint/grant`，client 注册依赖 `x-bifrost-token + challenge + 签名`

### TC-RI-回归-88：client call detail 接口按 client_instance_id 做所有权隔离

**前置条件**：
- 已注册两个不同的 client，分别持有各自的 `client_auth_token`
- 其中 client A 至少有一条历史 call 记录，记下其 `call_id`

**操作步骤**：
1. 使用 client B 的 `client_auth_token` 请求：
   ```bash
   curl -s -o /tmp/ri-call-cross-client.json -w '%{http_code}' \
     "http://127.0.0.1:8686/v4/remote-invoke/client/calls/<call_id>?client_instance_id=<client_b_id>" \
     -H "Authorization: Bearer <client_b_token>"
   ```
2. 使用 client A 的 `client_auth_token` 请求同一个 `call_id`

**预期结果**：
- 第 1 步返回 `404` 或等效的 not found，不能读取到 client A 的 call 元数据
- 第 2 步返回 `200`，可以读取到该 call 的详情
- Relay 对 `GET /v4/remote-invoke/client/calls/:id` 的返回结果按认证后的 `client_instance_id` 做所有权校验

### TC-RI-回归-89：client pairing decision 必须校验 pairing 归属

**前置条件**：
- 已注册两个不同的 client，分别持有各自的 `client_auth_token`
- 其中 client A 已收到一个待审批 pairing，请记下其 `pairing_id`

**操作步骤**：
1. 使用 client B 的 `client_auth_token` 调用：
   ```bash
   curl -s -o /tmp/ri-cross-pairing.json -w '%{http_code}' \
     -X POST "http://127.0.0.1:8686/v4/remote-invoke/client/grants/<pairing_id>/decision" \
     -H "Authorization: Bearer <client_b_token>" \
     -H "Content-Type: application/json" \
     -d '{"decision":"approve","grant_mode":"once","client_instance_id":"<client_b_id>"}'
   ```
2. 再使用 client A 的 `client_auth_token` 对同一个 `pairing_id` 发起 approve

**预期结果**：
- 第 1 步返回 `403` 或等效的所有权不匹配错误，消息为 `client_mismatch` / `client_instance_id_mismatch` 或等效错误
- pairing 不会被错误批准或拒绝
- 第 2 步返回成功，由真正归属的 client A 完成审批

### TC-RI-回归-90：client call frame/exit 必须校验 call 归属

**前置条件**：
- 已注册两个不同的 client，分别持有各自的 `client_auth_token`
- 其中 client A 拥有一条活跃 call，记下其 `call_id`

**操作步骤**：
1. 使用 client B 的 `client_auth_token` 调用：
   ```bash
   curl -s -o /tmp/ri-cross-frame.json -w '%{http_code}' \
     -X POST "http://127.0.0.1:8686/v4/remote-invoke/client/calls/<call_id>/frame" \
     -H "Authorization: Bearer <client_b_token>" \
     -H "Content-Type: application/json" \
     -d '{"client_instance_id":"<client_b_id>","envelope_json":"{\"kind\":\"stdout\"}"}'
   ```
2. 使用 client B 的 `client_auth_token` 再调用同一 `call_id` 的 `/exit`
3. 使用 client A 的 `client_auth_token` 调用同一 `call_id` 的 `/frame`

**预期结果**：
- 第 1、2 步均返回 `403` 或等效的所有权不匹配错误，消息为 `client_mismatch` / `client_instance_id_mismatch` 或等效错误
- 非归属 client 不能注入 frame，也不能结束别人的 call
- 第 3 步返回成功，由真正归属的 client A 正常上报 frame

---

### TC-RI-回归-85 ~ TC-RI-回归-90 执行结果（2026-04-20）

执行方式说明：
- `TC-RI-回归-85`、`TC-RI-回归-87`：通过 `bash e2e-tests/tests/test_remote_invoke_e2e.sh` 在本地启动 `bifrost-sync-server` 与 Bifrost 执行端后真实执行
- `TC-RI-回归-86`、`TC-RI-回归-88`、`TC-RI-回归-89`、`TC-RI-回归-90`：通过 `pnpm --dir packages/bifrost-sync-server exec vitest run src/__tests__/remote-invoke-security.test.ts` 启动真实本地 sync-server，并对 API 发起实际 challenge/register/call-detail/pairing-decision/call-frame 请求完成验证

| 用例编号 | 用例名称 | 结果 | 说明 |
|---------|---------|------|------|
| TC-RI-回归-85 | client 注册 challenge/register 必须携带 x-bifrost-token | ✅ PASS | E2E 中直接对 relay 发起未鉴权请求，`/client/register/challenge` 与 `/client/register` 均返回 `401` |
| TC-RI-回归-86 | client 注册必须通过 challenge + 私钥签名校验，challenge 不可重放 | ✅ PASS | `remote-invoke-security.test.ts` 验证首次签名注册成功、重放返回 `registration_challenge_not_found`、伪造签名返回 `invalid_registration_signature` |
| TC-RI-回归-87 | caller 发起配对/连接认证仍然不需要 x-bifrost-token | ✅ PASS | E2E 中 caller 仅通过 `pair_code` 发起 `remote connect`，未提供 `x-bifrost-token` 仍可进入等待审批并在批准后连接成功 |
| TC-RI-回归-88 | client call detail 接口按 client_instance_id 做所有权隔离 | ✅ PASS | `remote-invoke-security.test.ts` 注册两个 client token，验证 client B 查询 client A 的 `call_id` 返回 `404`，owner 查询返回 `200` |
| TC-RI-回归-89 | client pairing decision 必须校验 pairing 归属 | ✅ PASS | `remote-invoke-security.test.ts` 注册两个 client token，验证 client B 审批 client A 的 `pairing_id` 返回 `403 client_mismatch`，归属 client A 审批成功 |
| TC-RI-回归-90 | client call frame/exit 必须校验 call 归属 | ✅ PASS | `remote-invoke-security.test.ts` 注册两个 client token，验证 client B 向 client A 的 `call_id` 上报 `/frame`、`/exit` 均返回 `403 client_mismatch`，归属 client A 上报 `/frame` 返回 `200` |

---

### TC-RI-回归-91：`remote connect` 遇到 relay overload-protect 时应重试并给出可行动提示

**背景**：线上 relay 前层可能返回 `503 Service Unavailable: overload-protect triggered`。修复前 CLI 会直接把底层网络错误原样暴露给用户，既没有短暂重试，也没有明确提示下一步操作。

**前置条件**：
- 已执行 `cargo build --release --bin bifrost`
- 本地可用 `python3`
- 使用独立测试数据目录，避免污染现有连接记录：
  ```bash
  export BIFROST_DATA_DIR="$(mktemp -d /tmp/bifrost-remote-overload.XXXXXX)"
  ```

**操作步骤**：
1. 启动本地 mock relay，令 `994430` 前两次 `POST /v4/remote-invoke/pairings/start` 返回 `503 overload-protect triggered`，第 3 次返回成功；令 `994431` 始终返回 `503 overload-protect triggered`：
   ```bash
   PORT=18786
   python3 -u - <<'PY' "$PORT"
import json
import sys
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

port = int(sys.argv[1])
attempts = {}
pairings = {}

class Handler(BaseHTTPRequestHandler):
    def log_message(self, fmt, *args):
        return

    def _write_json(self, status, payload):
        body = json.dumps(payload).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        if self.path.startswith("/v4/remote-invoke/pairings/") and self.path.endswith("/watch"):
            pairing_id = self.path.split("/")[4]
            self.send_response(200)
            self.send_header("Content-Type", "text/event-stream")
            self.send_header("Cache-Control", "no-cache")
            self.end_headers()
            self.wfile.write(b"event: connected\n")
            self.wfile.write(f"data: {json.dumps({'pairing_id': pairing_id})}\n\n".encode())
            self.wfile.flush()
            time.sleep(0.05)
            self.wfile.write(b"event: approved\n")
            self.wfile.write(b'data: {"status":"approved","grant_id":"grant-retry-ok","client_instance_id":"client-retry-ok-123456","device_name":"Retry Device","platform":"macos","grant_mode":"permanent"}\n\n')
            self.wfile.flush()
            return
        self._write_json(404, {"code": 404, "message": "not_found", "data": None})

    def do_POST(self):
        if self.path != "/v4/remote-invoke/pairings/start":
            self._write_json(404, {"code": 404, "message": "not_found", "data": None})
            return
        raw = self.rfile.read(int(self.headers.get("Content-Length", "0")))
        body = json.loads(raw.decode() or "{}")
        pair_code = body.get("pair_code", "")
        attempts[pair_code] = attempts.get(pair_code, 0) + 1
        should_overload = pair_code == "994431" or attempts[pair_code] <= 2
        if should_overload:
            payload = b"overload-protect triggered"
            self.send_response(503)
            self.send_header("Content-Type", "text/plain; charset=utf-8")
            self.send_header("Content-Length", str(len(payload)))
            self.end_headers()
            self.wfile.write(payload)
            return
        pairing_id = f"pairing-{pair_code}"
        pairings[pairing_id] = True
        self._write_json(200, {"code": 0, "message": "ok", "data": {"pairing_id": pairing_id}})

ThreadingHTTPServer(("127.0.0.1", port), Handler).serve_forever()
PY
   ```
2. 在另一个终端执行瞬时过载场景：
   ```bash
   cargo run --bin bifrost -- remote connect 994430 --relay-url "http://127.0.0.1:${PORT}"
   ```
3. 继续执行持续过载场景：
   ```bash
   cargo run --bin bifrost -- remote connect 994431 --relay-url "http://127.0.0.1:${PORT}"
   ```
4. 检查本地连接文件：
   ```bash
   cat "$BIFROST_DATA_DIR/remote-connections.json"
   ```

**预期结果**：
- 步骤 2 中，CLI 至少输出一次 `Relay is temporarily busy, retrying pairing ...`，随后成功输出 `✓ Connected! Authorization granted`
- 步骤 2 完成后，`remote-connections.json` 中新增 `client-retry-ok-123456` 连接记录
- 步骤 3 中，CLI 在有限重试后退出失败，并输出 `pairing service is temporarily busy` 与 `bifrost remote connect <pair-code>` 提示
- 整个过程中不再直接向用户暴露原始的 `start_pairing failed with status 503 Service Unavailable: overload-protect triggered`

### TC-RI-回归-91 执行结果（2026-04-20）

执行方式说明：
- 通过 `bash e2e-tests/tests/test_remote_connect_overload_retry_e2e.sh` 启动本地 mock relay，真实执行 CLI `remote connect` 的“瞬时过载重试成功”与“持续过载给出可行动提示”两条路径

| 用例编号 | 用例名称 | 结果 | 说明 |
|---------|---------|------|------|
| TC-RI-回归-91 | `remote connect` 遇到 relay overload-protect 时应重试并给出可行动提示 | ✅ PASS | `994430` 在前 2 次 `503 overload-protect` 后第 3 次成功配对并写入 `client-retry-ok-123456`；`994431` 在 4 次受限重试后返回 `pairing service is temporarily busy`，不再直接暴露原始 503 文案 |

### TC-RI-回归-91 执行结果（2026-05-10 CI skip-build 稳定性）

执行方式说明：
- CI run `25608391994` 在 macOS shell shard 3 中暴露脚本稳定性问题：workflow 已下载 `target/release/bifrost` 并以 `--skip-build` 运行 shell E2E，但 `test_remote_connect_overload_retry_e2e.sh` 仍无条件执行 `cargo build --release --bin bifrost`，导致该脚本停在 `Build bifrost (release)...` 并超过 900s。
- 修复后脚本遵循 `SKIP_BUILD=true`，直接使用 `BIFROST_BIN` 或默认 `target/release/bifrost`，仅在未启用 skip-build 时才本地构建。

| 用例编号 | 用例名称 | 结果 | 说明 |
|---------|---------|------|------|
| TC-RI-回归-91 | `remote connect` overload retry shell E2E 在 CI `--skip-build` 下复用已构建二进制 | ✅ PASS | 本地执行 `SKIP_BUILD=true BIFROST_BIN="$PWD/target/release/bifrost" bash e2e-tests/tests/test_remote_connect_overload_retry_e2e.sh` 通过；输出确认使用现有二进制，瞬时 overload 场景第 3 次连接成功，持续 overload 场景第 4 次后给出可行动错误。 |

---

## 协议回归测试：grant_created / call_open 职责分离（TC-RI-回归-92 ~ TC-RI-回归-94）

> Bug 背景：relay 在 pairing 审批通过后下发的 `grant_created` 事件仍然只有 `grant_id`、`caller_fingerprint`、`grant_mode` 等授权同步字段；但 client 侧曾错误地把它当成 `call_open` 处理，要求必须携带 `call_id` 与 `command`。结果是审批成功后的 grant 无法写入本地 `local_grants`，后续真正的 `call_open` 又因“grant not found in local_grants”被拒绝，表现为 connect 成功后 remote/client 无法继续正常通信。

### TC-RI-回归-92：`grant_created` 最小 payload 仍能写入本地 grant 缓存

**操作步骤**：
1. 执行 worker 的 grant 构造单元测试：
   ```bash
   cargo test -p bifrost-admin test_build_grant_info_from_grant_created -- --nocapture
   ```
2. 关注以下场景：
   - 仅提供 `grant_id`、`caller_fingerprint`、`grant_mode=permanent`
   - 提供嵌套 `caller_info`，验证优先使用其中的 `fingerprint` / `display_name`
   - 缺失 `grant_id` 时返回拒绝

**预期结果**：
- 3 个单元测试全部通过
- 最小 payload 不再要求 `call_id`、`command`
- `GrantMode::Once` 会正确初始化 `max_calls=1`、`remaining_calls=1`

### TC-RI-回归-93：connect 批准后首个远程调用可以立即成功

**前置条件**：
- 使用本地 relay 或真实 relay 启动 Bifrost 执行端
- `remote invoke` 模块状态为 `Connected`

**操作步骤**：
1. 进入 discovery 模式，获取 `pair_code`
2. 使用 caller 执行：
   ```bash
   BIFROST_DATA_DIR=<caller_dir> ./target/release/bifrost remote connect <pair_code> --relay-url <relay_url>
   ```
3. 在执行端批准该 pairing
4. 连接成功后，立刻执行首个远程命令：
   ```bash
   BIFROST_DATA_DIR=<caller_dir> ./target/release/bifrost remote status --relay-url <relay_url> --client-id <client_id_prefix>
   ```

**预期结果**：
- connect 输出 `✓ Connected! Authorization granted`
- 批准后执行端 grants 列表出现新的 active grant
- 首个 `remote status` 命令成功返回设备信息
- 执行端日志中不再出现 `grant_created missing call_id`、`grant_created missing command`、`grant not found in local_grants`

### TC-RI-回归-94：越权防护与 connect 修复可同时成立

**操作步骤**：
1. 执行 sync-server 安全回归测试：
   ```bash
   pnpm --dir packages/bifrost-sync-server exec vitest run src/__tests__/remote-invoke-security.test.ts
   ```
2. 重点确认：
   - 非归属 client 审批别人的 pairing 返回 `403`
   - 非归属 client 上报别人的 `/frame`、`/exit` 返回 `403`

**预期结果**：
- 安全回归测试通过
- `client_mismatch` 保护仍然生效
- 本次 connect/grant 修复没有回退 review finding 中的授权约束

### TC-RI-回归-92 ~ TC-RI-回归-94 执行结果（2026-04-20）

执行方式说明：
- `TC-RI-回归-92`：运行 `cargo test -p bifrost-admin test_build_grant_info_from_grant_created -- --nocapture`
- `TC-RI-回归-93`：运行 `bash e2e-tests/tests/test_remote_invoke_e2e.sh`，关注 connect 成功后紧接着的 `remote status`
- `TC-RI-回归-94`：运行 `pnpm --dir packages/bifrost-sync-server exec vitest run src/__tests__/remote-invoke-security.test.ts`

| 用例编号 | 用例名称 | 结果 | 说明 |
|---------|---------|------|------|
| TC-RI-回归-92 | `grant_created` 最小 payload 仍能写入本地 grant 缓存 | ✅ PASS | `cargo test -p bifrost-admin test_build_grant_info_from_grant_created -- --nocapture` 通过，3 个单元测试覆盖最小 payload、嵌套 `caller_info` 优先级、缺失 `grant_id` 拒绝 |
| TC-RI-回归-93 | connect 批准后首个远程调用可以立即成功 | ✅ PASS | `bash e2e-tests/tests/test_remote_invoke_e2e.sh` 全部 35 项断言通过；`TC-RI-01A` 验证 connect 日志包含 `Connected! Authorization granted`，`TC-RI-02` 验证批准后立即执行 `remote status` 成功 |
| TC-RI-回归-94 | 越权防护与 connect 修复可同时成立 | ✅ PASS | `pnpm --dir packages/bifrost-sync-server exec vitest run src/__tests__/remote-invoke-security.test.ts` 4/4 通过，pairing decision 与 call frame/exit 的 `client_mismatch` 保护仍然生效 |

### TC-RI-回归-95：SSE 推送失败时客户端通过轮询发现 pending pairing

**前置条件**：
- 本地 bifrost-sync-server 或真实 relay 已启动并启用 `--enable-remote-invoke`
- Bifrost 执行端已注册、SSE 已连接（状态 `Connected`）
- 执行端已进入 discovery 模式（`POST /api/remote-invoke/discovery-session`）

**操作步骤**：
1. 启动 Bifrost 执行端，确认 `remote invoke` 模块状态为 `Connected`
2. 在执行端创建 discovery session 获取 `pair_code`
3. 使用 caller 发起配对：
   ```bash
   cargo run --bin bifrost -- remote connect <pair_code> --relay-url <relay_url>
   ```
4. 等待 10 秒（两个轮询周期），观察执行端日志是否出现：
   - `discovered N new pending pairing(s) via polling`（轮询发现新配对）
   - 或 SSE 推送日志 `pairing_request`
5. 检查执行端 `GET /api/remote-invoke/pairings/pending` 是否返回该 pairing

**预期结果**：
- 无论 SSE 推送是否成功，轮询机制都能在 10 秒内发现新的 pending pairing
- 执行端 `/api/remote-invoke/pairings/pending` 返回至少 1 条 pending pairing，包含 `pairing_id`、`caller_fingerprint`
- caller 端不会永久卡在 `Waiting for approval` 状态

### TC-RI-回归-96：SSE 重连后自动清理 stale pending pairing（pair_slot_occupied 修复）

**前置条件**：
- 本地 bifrost-sync-server 或真实 relay 已启动
- Bifrost 执行端已注册

**操作步骤**：
1. 启动 Bifrost 执行端，确认状态 `Connected`
2. 创建 discovery session，获取 `pair_code`
3. 使用 caller 发起配对（不做审批），创建一个 pending pairing
4. 强制断开执行端的 SSE 连接（可通过重启 Bifrost 执行端）
5. 等待执行端 SSE 重连成功（日志出现 `cleared stale pending pairings on SSE reconnect`）
6. 使用新的 `pair_code` 再次发起配对：
   ```bash
   cargo run --bin bifrost -- remote connect <new_pair_code> --relay-url <relay_url>
   ```

**预期结果**：
- 第 5 步：执行端日志显示 `cleared stale pending pairings on SSE reconnect`
- 第 6 步：新配对成功发起，**不**返回 `pair_slot_occupied` 错误
- relay 端的 stale pending pairing 已被取消（状态变为 `rejected`，reason 为 `cancelled_by_client`）

### TC-RI-回归-97：pending-pairings API 端点正确性验证

**前置条件**：
- 本地 bifrost-sync-server 已启动并启用 `--enable-remote-invoke`
- Bifrost 执行端已注册

**操作步骤**：
1. 启动 Bifrost 执行端，确认状态 `Connected`
2. 直接调用 relay 的 GET pending-pairings API（无 pending pairing 时）：
   ```bash
   curl -s -H "Authorization: Bearer <client_auth_token>" http://localhost:<relay_port>/v4/remote-invoke/client/pending-pairings
   ```
3. 创建 discovery session，发起一个 pairing（不审批）
4. 再次调用 GET pending-pairings API
5. 调用 DELETE pending-pairings API 取消所有 pending：
   ```bash
   curl -s -X DELETE -H "Authorization: Bearer <client_auth_token>" http://localhost:<relay_port>/v4/remote-invoke/client/pending-pairings
   ```
6. 再次调用 GET pending-pairings API 确认已清空

**预期结果**：
- 第 2 步：返回 `{"code":0,"message":"ok","data":[]}`
- 第 4 步：返回 `{"code":0,"message":"ok","data":[{"pairing_id":"...","caller_fingerprint":"...","status":"pending_approval",...}]}`
- 第 5 步：返回 `{"code":0,"message":"ok","data":{"cancelled":1}}`
- 第 6 步：返回 `{"code":0,"message":"ok","data":[]}`

### TC-RI-回归-98：显式 `--relay-url` 优先于运行中实例与本地配置

**前置条件**：
- 本地已编译最新 `bifrost` 二进制
- 已准备一个可用 relay 地址 `http://127.0.0.1:<explicit_port>`
- 正在运行的本地 Bifrost `sync.remote_base_url` 指向另一地址
- `BIFROST_DATA_DIR` 对应 `config.toml` 的 `sync.remote_base_url` 再指向第三个地址

**操作步骤**：
1. 启动本地 Bifrost，并将运行中 `sync.remote_base_url` 设置为 `http://127.0.0.1:<runtime_port>`
2. 在单独数据目录下写入：
   ```toml
   [sync]
   remote_base_url = "http://127.0.0.1:<config_port>"
   ```
3. 执行：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-remote-relay-explicit ./target/debug/bifrost remote connect <pair_code> --port <runtime_bifrost_port> --relay-url http://127.0.0.1:<explicit_port>
   ```
4. 观察 relay 命中情况与 `remote-connections.json`

**预期结果**：
- 仅显式传入的 relay 收到 `POST /v4/remote-invoke/pairings/start`
- 运行中实例配置和本地配置文件中的 relay 均未被命中
- `remote-connections.json` 中保存的 `relay_url` 为显式传入地址

### TC-RI-回归-99：未传 `--relay-url` 时优先读取运行中实例的 `sync.remote_base_url`

**前置条件**：
- 本地 Bifrost 正在运行，且 `/_bifrost/api/sync/status` 返回非空 `remote_base_url`
- 调用方数据目录中不存在 `config.toml`，或其中 `sync.remote_base_url` 与运行中实例不同

**操作步骤**：
1. 确认运行中实例 `sync status` 返回目标 relay URL
2. 执行：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-remote-relay-runtime ./target/debug/bifrost remote connect <pair_code> --port <runtime_bifrost_port>
   ```
3. 观察 relay 命中情况与 `remote-connections.json`

**预期结果**：
- CLI 成功发起配对，不再报 `--relay-url is required`
- 实际命中运行中实例中的 relay URL
- `remote-connections.json` 中保存的 `relay_url` 与运行中实例 `sync.remote_base_url` 一致

### TC-RI-回归-100：本地实例不可用时回退读取 `config.toml` 中的 `sync.remote_base_url`

**前置条件**：
- 本地未运行 Bifrost，或指定 `--port` 上没有可用实例
- 调用方数据目录下存在 `config.toml`，其中配置：
   ```toml
   [sync]
   remote_base_url = "http://127.0.0.1:<config_port>"
   ```

**操作步骤**：
1. 确认没有运行中的本地 Bifrost 可提供 `sync status`
2. 执行：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-remote-relay-config ./target/debug/bifrost remote connect <pair_code> --port <unused_port>
   ```
3. 观察 relay 命中情况与 `remote-connections.json`

**预期结果**：
- CLI 成功发起配对，不再报 `--relay-url is required`
- 实际命中本地 `config.toml` 中的 relay URL
- `remote-connections.json` 中保存的 `relay_url` 与本地配置文件一致

### TC-RI-回归-101：运行中实例与本地配置均不可用时回退默认 relay

**前置条件**：
- 本地未运行 Bifrost，或指定端口没有可用实例
- 调用方数据目录下不存在 `config.toml`，或其中 `sync.remote_base_url` 为空字符串

**操作步骤**：
1. 在全新数据目录下执行：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-remote-relay-default ./target/debug/bifrost remote connect <pair_code> --port <unused_port>
   ```
2. 观察命令输出、日志或抓包

**预期结果**：
- CLI 不再输出 `Error: --relay-url is required (could not detect from running proxy)`
- relay 目标回退为默认值 `https://bifrost.bytedance.net`
- 若默认 relay 当前不可达，错误表现为网络/业务错误，而不是缺少参数错误

### TC-RI-回归-98 ~ TC-RI-回归-101 执行结果（2026-04-21）

执行方式说明：
- `TC-RI-回归-98` ~ `TC-RI-回归-100`：运行 `bash e2e-tests/tests/test_remote_relay_url_fallback_e2e.sh`
- `TC-RI-回归-101`：运行 `cargo test -p bifrost-cli select_remote_relay_url_honors_precedence_order -- --nocapture`

| 用例编号 | 用例名称 | 结果 | 说明 |
|---------|---------|------|------|
| TC-RI-回归-98 | 显式 `--relay-url` 优先于运行中实例与本地配置 | ✅ PASS | `bash e2e-tests/tests/test_remote_relay_url_fallback_e2e.sh` 的 Case 1 通过；仅显式 relay 收到请求，`remote-connections.json` 保存显式地址 |
| TC-RI-回归-99 | 未传 `--relay-url` 时优先读取运行中实例的 `sync.remote_base_url` | ✅ PASS | 同一脚本 Case 2 通过；CLI 在省略 `--relay-url` 时命中运行中实例 relay，未再报缺少参数 |
| TC-RI-回归-100 | 本地实例不可用时回退读取 `config.toml` 中的 `sync.remote_base_url` | ✅ PASS | 同一脚本 Case 3 通过；运行中实例不可用时命中本地 `config.toml`，`remote-connections.json` 保存配置地址 |
| TC-RI-回归-101 | 运行中实例与本地配置均不可用时回退默认 relay | ✅ PASS | 执行 `BIFROST_DATA_DIR=<全新目录> ./target/release/bifrost --port 65530 remote connect 881004`；CLI 输出 `→ Initiating pairing with code 881004...` 后收到 relay 返回的 `invalid_pair_code`，未再出现 `--relay-url is required`，证明已继续走默认 relay 路径 |

---

### TC-RI-回归-102：Admin API 可创建当前 active SSH key

**前置条件**：
- 目标 client 已启用 Remote Invoke 并连接到本地 relay

**操作步骤**：
1. 调用：
   ```bash
   curl -s -X POST http://127.0.0.1:8800/_bifrost/api/remote-invoke/ssh-key \
     -H 'Content-Type: application/json' \
     -d '{"label":"CI Agent","grant_mode":"30m"}'
   ```
2. 记录返回的 `device_code`、`ssh_key_fingerprint` 与 `bifrost_key_file`
3. 再调用：
   ```bash
   curl -s http://127.0.0.1:8800/_bifrost/api/remote-invoke/ssh-key
   ```

**预期结果**：
- 创建接口返回 200
- 响应体直接包含 `device_code`、`ssh_key_fingerprint`、`bifrost_key_file`
- `device_code` 形如 `BF-XXXXXXXXXXXXXXXX`
- 即使请求传入 `grant_mode=30m`，返回的 active SSH key 仍应归一化为 `grant_mode=permanent`
- 查询当前 key 时返回同一个 active key，而不是嵌套在额外包裹字段中

### TC-RI-回归-103：导出私钥文件与当前 active SSH key 保持一致

**前置条件**：
- 已完成 TC-RI-回归-102

**操作步骤**：
1. 调用：
   ```bash
   curl -s http://127.0.0.1:8800/_bifrost/api/remote-invoke/ssh-key/private-key
   ```
2. 对比返回的 `device_code`、`ssh_key_fingerprint`
3. 确认 `bifrost_key_file` 中包含 `Device-Code: <device_code>`

**预期结果**：
- 导出接口返回 200
- 导出的 `device_code` 与当前 active key 完全一致
- 导出的 `ssh_key_fingerprint` 与当前 active key 完全一致
- 密钥文件是自包含 Bifrost key 格式

### TC-RI-回归-104：重置 SSH key 会轮换 device_code 并更新 active key

**前置条件**：
- 已完成 TC-RI-回归-102

**操作步骤**：
1. 记录当前 `device_code`
2. 调用：
   ```bash
   curl -s -X POST http://127.0.0.1:8800/_bifrost/api/remote-invoke/ssh-key/reset
   ```
3. 再调用 `GET /api/remote-invoke/ssh-key`

**预期结果**：
- 重置接口返回 200
- 新响应中的 `device_code` 与重置前不同
- 当前 active key 已切换为新 key
- 返回仍然带有新的 `bifrost_key_file`

### TC-RI-回归-105：SSH challenge + connect 可通过签名换取 approved 结果

**前置条件**：
- 已完成 TC-RI-回归-104，且当前 active SSH key 可导出
- 已准备可用于签名的 Ed25519 私钥（来自 `bifrost_key_file`）

**操作步骤**：
1. 调用 relay challenge：
   ```bash
   curl -s -X POST http://127.0.0.1:8686/v4/remote-invoke/ssh/challenge \
     -H 'Content-Type: application/json' \
     -d '{"device_code":"<device_code>"}'
   ```
2. 按协议构造 payload：
   ```json
   {"challenge":"<challenge>","challenge_id":"<challenge_id>","device_code":"<device_code>","timestamp":<unix_ms>}
   ```
3. 使用导出的私钥对该 payload 做 Ed25519 签名，并 base64 编码
4. 发起 `POST /v4/remote-invoke/ssh/connect`
5. 查询：
   ```bash
   curl -s http://127.0.0.1:8800/_bifrost/api/remote-invoke/grants
   ```

**预期结果**：
- challenge 返回 200，且带 `challenge_id`、`challenge`
- connect 返回 200，且带 `connect_id`、`relay_token`
- client 侧 grants 列表新增一条 `auth_method=ssh_publickey` 的授权记录
- 该 SSH grant 的 `grant_mode` 为 `permanent`，`expires_at` 为空
- active SSH key 的 `last_used_at` 被刷新

### TC-RI-回归-106：撤销 SSH key 后当前 key 为空且旧 device_code 无法再发起 challenge

**前置条件**：
- 已完成 TC-RI-回归-105

**操作步骤**：
1. 记录当前 active `device_code`
2. 调用：
   ```bash
   curl -s -X DELETE http://127.0.0.1:8800/_bifrost/api/remote-invoke/ssh-key
   ```
3. 再调用：
   ```bash
   curl -s http://127.0.0.1:8800/_bifrost/api/remote-invoke/ssh-key
   ```
4. 对旧 `device_code` 再发一次 relay challenge

**预期结果**：
- DELETE 返回 `{"success":true}`
- 当前 key 查询结果为 `null`
- 旧 `device_code` 的 challenge 请求失败，返回 `device_code_not_found` 或等效错误

### TC-RI-回归-107：SSH connect 后 relay 应能命中可复用 grant

**前置条件**：
- 已完成 TC-RI-回归-105
- 已记录当前 `client_instance_id` 与 SSH `caller_fingerprint`

**操作步骤**：
1. 调用：
   ```bash
   curl -s "http://127.0.0.1:8686/v4/remote-invoke/grants/reusable?client_instance_id=<client_instance_id>&caller_fingerprint=<ssh_key_fingerprint>"
   ```

**预期结果**：
- 返回 200
- `data` 中包含当前 SSH connect 刚签发的 grant
- grant 的 `grant_mode=permanent`、`status=active`，且 `expires_at` 为空；与 client 侧展示一致

### TC-RI-回归-108：SSH connect 后应能通过 relay 打开 remote search 调用

**前置条件**：
- 已完成 TC-RI-回归-105
- 目标 client 上已存在包含唯一 marker 的流量记录

**操作步骤**：
1. 使用 TC-RI-回归-105 中的 SSH grant 构造：
   ```json
   {
     "grant_id":"<ssh_grant_id>",
     "client_instance_id":"<client_instance_id>",
     "caller_fingerprint":"<ssh_key_fingerprint>",
     "command":{"command":"search.get","args_json":"{\"query\":\"<marker>\",\"limit\":10}"}
   }
   ```
2. 调用：
   ```bash
   curl -s -X POST http://127.0.0.1:8686/v4/remote-invoke/calls/open \
     -H 'Content-Type: application/json' \
     -d '<payload>'
   ```

**预期结果**：
- 返回 200
- 返回 `call_id` 与 `relay_token`
- 后续 `events` SSE 中能收到匹配 `<marker>` 的搜索结果

### TC-RI-回归-109：SSH connect 后应能通过 relay 执行 remote traffic get 并取回响应体

**前置条件**：
- 已完成 TC-RI-回归-105
- 已知目标流量记录的 `id`

**操作步骤**：
1. 使用 SSH grant 构造：
   ```json
   {
     "grant_id":"<ssh_grant_id>",
     "client_instance_id":"<client_instance_id>",
     "caller_fingerprint":"<ssh_key_fingerprint>",
     "command":{"command":"traffic.get","args_json":"{\"id\":\"<traffic_id>\",\"response_body\":true}"}
   }
   ```
2. 调用 `POST /v4/remote-invoke/calls/open`

**预期结果**：
- 返回 200
- 调用链路能打开并返回目标流量详情
- 输出中包含目标响应体 marker

### TC-RI-回归-110：撤销 SSH key 后 relay 路由应在一个 heartbeat 周期内失效

**前置条件**：
- 已完成 TC-RI-回归-105

**操作步骤**：
1. 调用 `DELETE /api/remote-invoke/ssh-key`
2. 等待一个 heartbeat 周期（30 秒量级）
3. 对旧 `device_code` 再发一次：
   ```bash
   curl -s -X POST http://127.0.0.1:8686/v4/remote-invoke/ssh/challenge \
     -H 'Content-Type: application/json' \
     -d '{"device_code":"<old_device_code>"}'
   ```

**预期结果**：
- client 侧当前 active key 为空
- client 侧 SSH grants 已被清空
- relay 不再接受旧 `device_code`，返回 `device_code_not_found`

### TC-RI-回归-126：SSH route 同步必须区分字段缺失与 null

**背景**：修复前，Client 撤销 SSH key 后，本地 active key 已为空，但注册/心跳请求会把 `ssh_device_route` 省略；Relay 端又无法区分"字段缺失"和"明确为 null"，导致旧 `device_code` route 保留，旧 device_code 仍可发起 challenge。生产 relay `bifrost-server-v4` 的 heartbeat 路由也必须遵守同一三态语义。

**前置条件**：
- 本地 sync-server relay 已开启 Remote Invoke
- 已注册一个 Remote Invoke client
- 已准备一组可派生 `device_code` 的 SSH public key

**操作步骤**：
1. Client 注册请求携带 `ssh_device_route` 对象，内容包含 `device_code` 与 `public_key_pem`
2. 对该 `device_code` 调用 `POST /v4/remote-invoke/ssh/challenge`
3. Client 再次注册，但省略 `ssh_device_route` 字段，模拟本地 key store 读取失败
4. 再次对同一个 `device_code` 调用 `POST /v4/remote-invoke/ssh/challenge`
5. Client 第三次注册，显式发送 `"ssh_device_route": null`，模拟撤销 key 后无 active key
6. 再次对旧 `device_code` 调用 `POST /v4/remote-invoke/ssh/challenge`

**预期结果**：
- 步骤 2 返回 200，证明 route 已发布
- 步骤 4 仍返回 200，证明字段缺失不会误删 route
- 步骤 6 返回 400，错误信息为 `device_code_not_found`，证明显式 `null` 会删除旧 route
- `bifrost-server-v4` 的 heartbeat handler 使用字段存在性判断，显式 `null` 会调用 `syncSshDeviceRoute(..., null)`，字段缺失不会调用

### TC-RI-回归-111：线上 relay 的 reusable grant 查询应能命中 SSH grant

**前置条件**：
- 远端 relay 为 `https://bifrost.bytedance.net`
- 已通过 SSH `challenge/connect` 成功建立授权
- client 侧 `GET /api/remote-invoke/grants` 已出现 `auth_method=ssh_publickey` 的 active grant

**操作步骤**：
1. 记录本次 SSH 授权对应的 `client_instance_id`、`ssh_key_fingerprint`、`grant_id`
2. 调用：
   ```bash
   curl -s "https://bifrost.bytedance.net/v4/remote-invoke/grants/reusable?client_instance_id=<client_instance_id>&caller_fingerprint=<ssh_key_fingerprint>"
   ```

**预期结果**：
- 返回 200
- `data.grant_id` 等于当前 SSH connect 刚签发的 grant
- `grant_mode`、`status` 与 client 侧授权列表一致

### TC-RI-回归-112：线上 relay 的 calls/open 应能复用 SSH grant 打开 remote search

**前置条件**：
- 已完成 TC-RI-回归-111
- 目标 client 上已存在包含唯一 marker 的流量记录

**操作步骤**：
1. 使用 SSH grant 构造：
   ```json
   {
     "grant_id":"<ssh_grant_id>",
     "client_instance_id":"<client_instance_id>",
     "caller_fingerprint":"<ssh_key_fingerprint>",
     "command":{"command":"search.get","args_json":"{\"query\":\"<marker>\",\"limit\":10}"}
   }
   ```
2. 调用：
   ```bash
   curl -s -X POST "https://bifrost.bytedance.net/v4/remote-invoke/calls/open" \
     -H 'Content-Type: application/json' \
     -d '<payload>'
   ```

**预期结果**：
- 返回 200
- 返回 `call_id` 与 `relay_token`
- 后续 caller `events` SSE 中能收到匹配 `<marker>` 的搜索结果

### TC-RI-回归-113：回归 — caller 主动取消 remote search 后 Recent Calls 最终显示 cancelled

**前置条件**：
- 已完成一次有效的 `remote connect`
- 目标 client 上存在足够多的流量记录，保证 `remote search` 会持续输出至少数秒

**操作步骤**：
1. 在 caller 侧执行：
   ```bash
   cargo run --bin bifrost -- remote search "<marker>" --relay-url <relay_url> --client-id <client_prefix> --limit 50
   ```
2. 等待终端出现 `Searching...` 或首批流式结果后，立即按 `Ctrl-C`
3. 回到 client 管理端 `Settings -> Remote Invoke -> Recent Calls`
4. 刷新 Recent Calls，定位刚刚这条 `search.get`

**预期结果**：
- caller 侧输出取消提示，不再继续等待到超时
- Recent Calls 中该条调用最终状态为 `cancelled`
- 该记录不会长期停留在 `streaming`

### TC-RI-回归-114：回归 — 所有 remote invoke 命令都支持 caller 主动取消

**前置条件**：
- 已完成一次有效的 `remote connect`

**操作步骤**：
1. 在 caller 侧分别启动以下命令，并在命令执行阶段按 `Ctrl-C`
   ```bash
   cargo run --bin bifrost -- remote status --relay-url <relay_url> --client-id <client_prefix>
   cargo run --bin bifrost -- remote traffic list --relay-url <relay_url> --client-id <client_prefix> --limit 50
   cargo run --bin bifrost -- remote traffic get <traffic_id> --relay-url <relay_url> --client-id <client_prefix>
   cargo run --bin bifrost -- remote search "<marker>" --relay-url <relay_url> --client-id <client_prefix> --limit 50
   ```
2. 每次取消后回到 client 管理端刷新 Recent Calls

**预期结果**：
- 每个命令在 caller 侧都能响应取消，而不是只能等待自然结束
- 被中断的调用在 Recent Calls 中都落为 `cancelled`
- 不出现某些命令支持取消、某些命令仍卡在 `streaming` 的分裂行为

### TC-RI-回归-115：回归 — 取消后的晚到结果不能覆盖 cancelled

**前置条件**：
- 已复现一次 caller 取消中的 remote search

**操作步骤**：
1. 发起 `remote search` 并在输出进行中按 `Ctrl-C`
2. 保持页面停留在 Recent Calls，连续点击刷新 3 次
3. 若后台仍有执行尾包上报，观察该条调用状态是否变化

**预期结果**：
- 调用一旦变为 `cancelled`，后续刷新中仍保持 `cancelled`
- 不会被晚到的 `completed` 或 `failed` 覆盖
- 结束时间和持续时间可更新，但终态不回退

### TC-RI-回归-116：回归 — 本地 relay 的粗粒度限流不能打断 cancel/events/exit 收尾

**前置条件**：
- 使用本地 `bifrost-sync-server`
- 已完成一次有效的 `remote connect`
- 目标 client 上存在足够多的流量记录，保证 `remote search` 会持续输出至少数秒

**操作步骤**：
1. 发起：
   ```bash
   cargo run --bin bifrost -- remote search "<marker>" --relay-url http://127.0.0.1:<relay_port> --client-id <client_prefix> --limit 500
   ```
2. 在输出进行中按 `Ctrl-C`
3. 观察 caller 侧是否打印取消提示且在合理时间内退出
4. 查询 client 侧：
   ```bash
   curl -s http://127.0.0.1:<admin_port>/_bifrost/api/remote-invoke/calls/<call_id> | jq .
   ```

**预期结果**：
- caller 侧不会因为 relay 返回 `429 too many requests` 而卡死
- client 侧调用最终状态为 `cancelled`
- `ended_at`、`duration_ms`、`exit_code=130` 已写入

### TC-RI-回归-117：回归 — 线上 relay 下 caller 主动取消后 target client 进入 cancelled 且 caller 不无限挂起

**前置条件**：
- relay 为 `https://bifrost.bytedance.net`
- 目标 client 已保存有效 sync session
- 已完成一次有效的 `remote connect`
- 目标 client 上存在足够多的流量记录，保证 `remote search` 会持续输出至少数秒

**操作步骤**：
1. 发起：
   ```bash
   cargo run --bin bifrost -- remote search "<marker>" --relay-url https://bifrost.bytedance.net --client-id <client_prefix> --limit 500
   ```
2. 在 caller 终端出现持续输出后按 `Ctrl-C`
3. 查询 target client Recent Calls / call detail
4. 观察 caller 进程是否在合理时间内退出

**预期结果**：
- target client 上该条 `search.get` 最终为 `cancelled`
- `ended_at`、`duration_ms`、`exit_code=130` 已稳定写入
- caller 不会在取消后无限挂起

### TC-RI-回归-121：CLI `remote connect --ssh-key` 能完成 SSH 授权并落盘后续可复用连接

**前置条件**：
- 已启动本地 relay，并启用 Remote Invoke
- 目标 client 已连接到该 relay，且 `/_bifrost/api/remote-invoke/status` 显示 `Connected`
- 可通过 `POST /_bifrost/api/remote-invoke/ssh-key/reset` 获取最新导出的 `bifrost_key_file`
- caller 使用独立 `BIFROST_DATA_DIR`

**操作步骤**：
1. 在 target client 上执行：
   ```bash
   curl -s -X POST http://127.0.0.1:<admin_port>/_bifrost/api/remote-invoke/ssh-key/reset
   ```
   记录返回的 `device_code`、`ssh_key_fingerprint` 与 `bifrost_key_file`。
2. 把 `bifrost_key_file` 写入本地文件 `<caller_key_path>`。
3. 在 caller 侧执行：
   ```bash
   BIFROST_DATA_DIR=<caller_dir> ./target/release/bifrost remote connect --ssh-key <caller_key_path> --relay-url http://127.0.0.1:<relay_port>
   ```
4. 检查 `<caller_dir>/remote-connections.json`。
5. 在同一 caller 数据目录下继续执行：
   ```bash
   BIFROST_DATA_DIR=<caller_dir> ./target/release/bifrost remote status --relay-url http://127.0.0.1:<relay_port>
   ```
6. 在 target client 上查询：
   ```bash
   curl -s http://127.0.0.1:<admin_port>/_bifrost/api/remote-invoke/grants | jq .
   ```

**预期结果**：
- 步骤 3 输出包含 `Connected with SSH key`
- 步骤 4 中 `remote-connections.json` 写入一条 `auth_method=ssh_publickey` 连接，且 `client_instance_id`、`caller_fingerprint=<ssh_key_fingerprint>`、`device_code` 均存在
- 步骤 5 成功执行，不再提示重新 `remote connect`
- 步骤 6 中出现 `auth_method=ssh_publickey`、`grant_mode=permanent` 的 grant，且 fingerprint 与步骤 1 一致

### TC-RI-回归-120：回归 — relay 返回 grant_not_found 时 disconnect 仍必须清掉本地连接

**前置条件**：
- 已完成一次有效的 `remote connect`
- caller 本地 `remote-connections.json` 中至少存在 1 条连接记录
- 已知本次连接的 `grant_id` 与 `caller_fingerprint`
- 如果前一条验证刚执行过“stale grant / grant crypto 丢失”类回归，必须先重新完成一次 fresh `remote connect`，不要复用已经失效或被清空的 caller 本地连接

**操作步骤**：
1. 先尝试直接调用 relay 删除 grant，制造 CLI 后续撤销命中 `404 grant_not_found` 的场景：
   ```bash
   curl -i -X DELETE "http://127.0.0.1:<relay_port>/v4/remote-invoke/grants/<grant_id>?caller_fingerprint=<caller_fingerprint>"
   ```
   如果本地 relay 的删除接口表现为幂等 `200`，则改为直接修改 caller 本地连接文件，把其中的 `grant_id` 替换成一个 relay 上不存在的值（例如 `missing-grant-for-disconnect-test`），以稳定复现 `grant_not_found`。
2. 确认 caller 本地连接文件仍保留这条连接：
   ```bash
   jq '.connections' "$BIFROST_DATA_DIR/remote-connections.json"
   ```
3. 执行：
   ```bash
   BIFROST_DATA_DIR=<caller_data_dir> cargo run --bin bifrost -- remote disconnect --all --relay-url http://127.0.0.1:<relay_port>
   ```
4. 再次检查 caller 本地连接文件：
   ```bash
   jq '.connections | length' "$BIFROST_DATA_DIR/remote-connections.json"
   ```

**预期结果**：
- 第 1 步 relay 删除成功；第 3 步中的 `remote disconnect --all` 不应因为后续 `404 grant_not_found` 报失败
- CLI 输出应明确说明连接已被清理；若 grant 已不存在，可提示 `already missing on relay`
- 第 4 步本地连接数为 `0`，不会残留幽灵连接
- 后续执行 `bifrost remote status --relay-url ...` 时会提示没有可复用连接或需要重新 `remote connect`

### TC-RI-回归-122：server-v4 SSH connect 挂起态必须保留 caller_info，确保 grant 展示调用方信息

**前置条件**：
- 已执行 `pnpm --dir bifrost-server-v4 build:local`
- 工作区存在 `bifrost-server-v4/output/`

**操作步骤**：
1. 执行：
   ```bash
   bash e2e-tests/tests/test_remote_invoke_server_v4_ssh_context.sh
   ```
2. 如需复核，检查脚本输出 JSON 中以下字段：
   - `pendingCallerInfo.hostname`
   - `grantCallerDisplayName`
   - `connectResultData.client_instance_id`

**预期结果**：
- `ssh_connect` 挂起态在 Redis 中保留 `caller_info`
- `POST /v4/remote-invoke/ssh/connect-result` 落 SSH grant 后，`caller_display_name` 取自调用方 `hostname/username`
- 回传给 caller 的 `ssh_connect_result` 事件继续包含 `client_instance_id`，不影响 CLI 本地连接落盘

### TC-RI-回归-123：远端 relay 下 client 重启后已落盘的加密 grant 仍可继续执行 remote status

**前置条件**：
- 远端 relay 使用 `https://bifrost.bytedance.net`
- target client 已连接远端 relay，且 `/_bifrost/api/remote-invoke/status` 为 `Connected`
- caller 已通过 `pair-code` 或 `--ssh-key` 完成一次有效的 `remote connect`
- caller 和 client 都使用独立数据目录，便于确认落盘文件

**操作步骤**：
1. 在 caller 侧先执行一次：
   ```bash
   BIFROST_DATA_DIR=<caller_dir> cargo run --bin bifrost -- remote status --relay-url https://bifrost.bytedance.net
   ```
2. 在 target client 上确认 admin 数据目录下已生成 grant crypto 持久化文件：
   ```bash
   ls <client_data_dir>/admin/remote_invoke_grant_crypto.*
   ```
3. 重启 target client：
   ```bash
   BIFROST_DATA_DIR=<client_data_dir> cargo run --bin bifrost -- start -p <admin_port> --unsafe-ssl --no-system-proxy
   ```
4. 等待 `/_bifrost/api/remote-invoke/status` 再次恢复为 `Connected`。
5. 不重新执行 `remote connect`，直接在同一 caller 数据目录下再次执行：
   ```bash
   BIFROST_DATA_DIR=<caller_dir> cargo run --bin bifrost -- remote status --relay-url https://bifrost.bytedance.net
   ```

**预期结果**：
- 步骤 1 首次 `remote status` 成功
- 步骤 2 中存在 `remote_invoke_grant_crypto.json` 与对应 key 文件
- 步骤 5 在不重新配对/不重新 SSH connect 的情况下仍成功返回远端状态
- 整个过程中不再出现 `missing grant shared secret for encrypted remote command` / `reconnect is required`

### TC-RI-回归-124：同一远端连续两次 pair-code connect 后 caller 必须只使用最后一次连接的 grant

**前置条件**：
- 远端 relay 使用 `https://bifrost.bytedance.net`
- target client 已连接远端 relay，且 `/_bifrost/api/remote-invoke/status` 为 `Connected`
- caller 使用独立 `BIFROST_DATA_DIR`
- 允许在 target client 上查看当前 grants

**操作步骤**：
1. 在 target client 上进入 discovery mode，记录第一个 pair code。
2. caller 使用独立数据目录执行第一次连接：
   ```bash
   BIFROST_DATA_DIR=<caller_dir> cargo run --bin bifrost -- remote connect <pair_code_1> --relay-url https://bifrost.bytedance.net
   ```
3. 记录 caller 本地 `remote-connections.json` 中第一次连接得到的 `grant_id`。
4. 再次进入 discovery mode，记录第二个 pair code。
5. 使用同一个 caller 数据目录执行第二次连接：
   ```bash
   BIFROST_DATA_DIR=<caller_dir> cargo run --bin bifrost -- remote connect <pair_code_2> --relay-url https://bifrost.bytedance.net
   ```
6. 再次检查同一 caller 数据目录下的 `remote-connections.json`。
7. 在 target client 上查询 grants 列表，确认是否同时存在两条 pair-code grant。
8. 使用同一 caller 数据目录直接执行：
   ```bash
   BIFROST_DATA_DIR=<caller_dir> cargo run --bin bifrost -- remote status --relay-url https://bifrost.bytedance.net
   ```

**预期结果**：
- 第 5 步后 caller 本地连接记录被第二次连接覆盖，只保留第二次 connect 的 `grant_id` 与加密上下文
- 即使 target client / relay 侧仍同时存在多条 active pair-code grant，第 8 步 `remote status` 仍成功
- 第 8 步实际使用的 grant 应与 caller 本地最后一次 connect 保存的 `grant_id` 一致，不会误用第一次连接留下的旧 grant
- 整个流程不再出现 `missing grant shared secret for encrypted remote command`

### TC-RI-回归-125：同一远端同一 caller 先用 SSH key 再用 pair-code connect 时必须稳定切换到最后一次连接

**前置条件**：
- 远端 relay 使用 `https://bifrost.bytedance.net`
- target client 已连接远端 relay，且 `/_bifrost/api/remote-invoke/status` 为 `Connected`
- caller 使用独立 `BIFROST_DATA_DIR`
- target client 已可导出可用的 SSH key

**操作步骤**：
1. 在 target client 上重置或导出当前 SSH key，得到可用的 `bifrost_key_file`。
2. caller 使用独立数据目录执行 SSH 连接：
   ```bash
   BIFROST_DATA_DIR=<caller_dir> cargo run --bin bifrost -- remote connect --ssh-key <bifrost_key_file> --relay-url https://bifrost.bytedance.net
   ```
3. 记录 caller 本地 `remote-connections.json` 中 SSH 连接得到的 `grant_id`、`auth_method` 与加密上下文。
4. 在同一个 target client 上进入 discovery mode，记录 pair code。
5. 使用同一个 caller 数据目录执行 pair-code 连接：
   ```bash
   BIFROST_DATA_DIR=<caller_dir> cargo run --bin bifrost -- remote connect <pair_code> --relay-url https://bifrost.bytedance.net
   ```
6. 再次检查同一 caller 数据目录下的 `remote-connections.json`。
7. 在 target client 上查询 grants 列表，确认是否同时存在 `ssh_publickey` 与 `pair_code` 两条 active grant。
8. 使用同一 caller 数据目录直接执行：
   ```bash
   BIFROST_DATA_DIR=<caller_dir> cargo run --bin bifrost -- remote status --relay-url https://bifrost.bytedance.net
   ```

**预期结果**：
- 第 5 步后 caller 本地连接记录被 pair-code 连接覆盖，只保留最后一次 connect 的 `grant_id`、`auth_method=pair_code` 与对应加密上下文
- target client / relay 侧可以同时存在 `ssh_publickey` 与 `pair_code` 两条 active grant
- 第 8 步 `remote status` 成功，且实际使用的是 caller 本地最后一次 connect 保存的加密上下文
- 整个流程不出现“无法判断当前加密方案”“shared secret 对不上”“caller/client ephemeral_pub 不匹配”之类串线错误

### TC-RI-回归-126：Recent Calls 必须展示可读的命令摘要而不是空白

**前置条件**：
- target client 已连接 relay，Settings -> Remote Invoke 页面可正常打开
- caller 已完成至少一次有效授权

**操作步骤**：
1. caller 执行一次 `remote status`。
2. 刷新 target client 的 Settings -> Remote Invoke 页面。
3. 在 `Recent Calls` 卡片中查看最新一条 `status` 调用记录。
4. 如需进一步确认，调用 `GET /api/remote-invoke/calls`，检查最新 `status` 记录中的 `command_summary.command_preview`。

**预期结果**：
- `Recent Calls` 最新记录标题中能看到 `status` 之类可读命令摘要，而不是空白
- 即使 relay 返回的 `command_summary.command_preview` 为空字符串，client 本地仍会用解密后的命令摘要补齐展示
- `GET /api/remote-invoke/calls` 中对应记录的 `command_summary.command_preview` 非空，且与实际执行命令一致

### TC-RI-回归-127：Shell Access 编辑器中的 Policy/Profile ID 必须只读，且仅名称可修改

**前置条件**：
- target client 的 Settings -> Remote Invoke 页面可正常打开
- `Shell Access` 中至少有 1 个已有 profile 和 1 个已有 policy

**操作步骤**：
1. 打开 `http://127.0.0.1:8800/_bifrost/settings?tab=remote-invoke`。
2. 在 `Shell Access` 卡片中点击 `Manage Access`。
3. 观察已有 `Profile ID` 与 `Policy ID` 输入框是否仍可编辑。
4. 尝试选中并复制 ID 文本，但不应能直接改写其值。
5. 修改同一条 profile 的 `Profile name`。
6. 修改同一条 policy 的 `Policy name`。
7. 点击 `Save`。
8. 保存成功后，重新打开 `Manage Access`，并通过 `GET /api/remote-invoke/shell-config` 核对对应 profile/policy。

**预期结果**：
- `Profile ID` 与 `Policy ID` 输入框为只读状态，用户可查看/复制，但不能直接修改
- `Profile name` 与 `Policy name` 仍可正常编辑
- 保存后弹出成功提示，且页面重新加载后名称更新生效
- `GET /api/remote-invoke/shell-config` 中对应 profile/policy 的 `id` 保持原值不变，仅 `name` 发生变化
- 现有 policy 与 profile 的关联关系未因本次保存被破坏

### TC-RI-回归-102 ~ TC-RI-回归-106 执行结果（2026-04-21）

**执行环境**：
- Bifrost Admin：`BIFROST_DATA_DIR=./.bifrost-test-remote-invoke-ssh cargo run --bin bifrost -- start -p 8800 --unsafe-ssl --no-system-proxy`
- Local relay：`pnpm --dir packages/bifrost-sync-server dev -- --data-dir /tmp/bifrost-sync-remote-invoke-ssh -p 8686 --enable-remote-invoke`
- 调试与验证命令：`curl`、`openssl pkeyutl`、`python3`

| 用例编号 | 结果 | 实际结果 |
|---------|------|---------|
| TC-RI-回归-102 | ✅ PASS | `POST /api/remote-invoke/ssh-key` 即使传入 `grant_mode=30m`，返回和后续 `GET /api/remote-invoke/ssh-key` 中的 active key 仍统一为 `grant_mode=permanent`；响应继续保持扁平结构，包含 `device_code`、`ssh_key_fingerprint`、`bifrost_key_file`。 |
| TC-RI-回归-103 | ✅ PASS | `GET /api/remote-invoke/ssh-key/private-key` 返回的 `device_code`、`ssh_key_fingerprint` 与 active key 一致，`bifrost_key_file` 内含对应 `Device-Code:` 头。 |
| TC-RI-回归-104 | ✅ PASS | `POST /api/remote-invoke/ssh-key/reset` 后 `device_code` 发生轮换，新的 active key 立即可查询，且响应继续携带新的 `bifrost_key_file`。 |
| TC-RI-回归-105 | ✅ PASS | 对 relay `ssh/challenge` 返回的 challenge 使用导出私钥完成签名后，`POST /v4/remote-invoke/ssh/connect` 成功返回 `connect_id` 与 `relay_token`；随后 `GET /api/remote-invoke/grants` 出现 `auth_method=ssh_publickey` 且 `grant_mode=permanent`、`expires_at=null` 的授权记录，active key 的 `last_used_at` 与 `last_caller_info` 均被刷新。 |
| TC-RI-回归-106 | ✅ PASS | `DELETE /api/remote-invoke/ssh-key` 返回 `{"success":true}`；再次查询 active key 返回 `null`；使用旧 `device_code` 再请求 relay challenge 时返回 `device_code_not_found`，证明旧路由已失效。 |

### TC-RI-回归-107 ~ TC-RI-回归-110 执行结果（2026-04-21）

**执行环境**：
- Bifrost Admin：`target/release/bifrost -H 127.0.0.1 -p <随机端口> start -y --access-mode allow_all --skip-cert-check --unsafe-ssl --no-system-proxy`
- Local relay：`pnpm --dir packages/bifrost-sync-server exec tsx src/cli.ts -p <随机端口> -d <临时目录> --enable-remote-invoke`
- Mock target：`python3 -m http.server <随机端口> --bind 127.0.0.1 --directory <临时目录>`

| 用例编号 | 结果 | 实际结果 |
|---------|------|---------|
| TC-RI-回归-107 | ✅ PASS | SSH connect 成功后，relay `GET /v4/remote-invoke/grants/reusable?...caller_fingerprint=<ssh_key_fingerprint>` 返回当前 SSH grant，`grant_id`、`grant_mode=permanent`、`status=active` 且 `expires_at=null`，与 client 侧 `GET /api/remote-invoke/grants` 一致。 |
| TC-RI-回归-108 | ✅ PASS | 使用 SSH grant 调用 relay `POST /v4/remote-invoke/calls/open` 执行 `search.get` 成功返回 `call_id` 与 `relay_token`；caller `events` SSE 收到搜索结果和 `exit_code=0`，输出中包含目标 marker，client 侧 `GET /api/remote-invoke/calls/<call_id>` 状态为 `Completed`。 |
| TC-RI-回归-109 | ✅ PASS | 使用同一 SSH grant 调用 relay `POST /v4/remote-invoke/calls/open` 执行 `traffic.get` 成功返回 `call_id` 与 `relay_token`；caller `events` SSE 收到完整详情与 `response_body`，输出中同时包含目标 `traffic_id` 和响应体 marker，client 侧调用记录状态为 `Completed`。 |
| TC-RI-回归-110 | ✅ PASS | `DELETE /api/remote-invoke/ssh-key` 后，client 侧 active key 立即为 `null`，SSH grants 被清空；5 秒后对旧 `device_code` 再发 relay challenge 返回 `device_code_not_found`，证明 revoke 已触发路由收敛删除。 |

### TC-RI-回归-126 执行结果（2026-04-25，修复后）

**执行环境**：
- 本地 sync-server relay：`pnpm --dir packages/bifrost-sync-server exec vitest run src/__tests__/remote-invoke-security.test.ts`
- 生产 relay 代码校验：`pnpm --dir bifrost-server-v4 build:local`
- 真实 CLI/API 链路：`bash e2e-tests/tests/test_remote_invoke_ssh_e2e.sh`

| 用例编号 | 结果 | 实际结果 |
|---------|------|---------|
| TC-RI-回归-126 | ✅ PASS | sync-server 回归验证 `ssh_device_route` 对象发布 route 后 challenge 返回 200，字段缺失时旧 route 继续有效，显式 `null` 后旧 `device_code` challenge 返回 `device_code_not_found`；`bifrost-server-v4` heartbeat handler 已改为按字段存在性区分缺失和 null，`build:local` 通过；真实 SSH E2E 中 `DELETE /api/remote-invoke/ssh-key` 后旧 `device_code` 不再接受 challenge。 |

### TC-RI-回归-111 ~ TC-RI-回归-112 执行结果（2026-04-21，修复后线上 relay）

**执行环境**：
- Bifrost Admin：`cargo run --release --bin bifrost -- -H 127.0.0.1 -p <随机端口> start -y --access-mode allow_all --skip-cert-check --unsafe-ssl --no-system-proxy`
- Relay Server：远端 `https://bifrost.bytedance.net`
- Sync Session：复用本机已登录的真实线上账号 token

| 用例编号 | 结果 | 实际结果 |
|---------|------|---------|
| TC-RI-回归-111 | ✅ PASS | 使用真实线上 relay `https://bifrost.bytedance.net` 重新执行 SSH `challenge/connect` 后，client 侧成功创建 active `ssh_publickey` grant；随后 `GET /v4/remote-invoke/grants/reusable?...caller_fingerprint=<ssh_key_fingerprint>` 正确返回该 grant，`grant_id` 与 client 侧列表一致。 |
| TC-RI-回归-112 | ✅ PASS | 在同一组 `client_instance_id` / `grant_id` / `caller_fingerprint` 条件下，调用线上 relay `POST /v4/remote-invoke/calls/open` 执行 `search.get` 成功返回 `call_id` 与 `relay_token`，caller `events` SSE 收到包含目标 marker 的结果；继续执行 `traffic.get` 也成功返回目标 `traffic_id` 与响应体 marker，最后 `DELETE /api/remote-invoke/ssh-key` 后旧 `device_code` 再次变为 `device_code_not_found`。 |

### TC-RI-回归-113 ~ TC-RI-回归-119 执行结果（2026-04-21，取消与限流收尾）

**执行环境**：
- Local relay：`pnpm --dir packages/bifrost-sync-server exec tsx src/cli.ts -p <随机端口> -d <临时目录> --enable-remote-invoke`
- Online relay：`https://bifrost.bytedance.net`
- Bifrost Admin：`target/release/bifrost -H 127.0.0.1 -p <随机端口> start -y --access-mode allow_all --skip-cert-check --unsafe-ssl --no-system-proxy`
- Caller：`target/release/bifrost remote ...`

| 用例编号 | 结果 | 实际结果 |
|---------|------|---------|
| TC-RI-回归-113 | ✅ PASS | 本地 relay 下，caller `Ctrl-C` 后 target client 不再长期停留在 `streaming`；调用最终可收敛到 terminal state。 |
| TC-RI-回归-114 | ✅ PASS | `remote status` / `traffic list` / `traffic get` / `search` 的 caller 侧都能响应取消信号，不再只能等待自然结束。 |
| TC-RI-回归-115 | ✅ PASS | target client 上 `cancelled` 状态连续 3 次刷新保持不变，未被晚到结果覆盖。 |
| TC-RI-回归-116 | ⚠️ PARTIAL | 本地 relay 在大流量 `search.get` 取消场景下，caller 已不再因为 relay `429` 无限挂起，并会主动退出且打印 `Remote command 'search.get' cancelled by caller.`；但本轮本地 client 进程在收尾后未能保留到状态抓取阶段，未重新拿到 target client 上的最终 call detail，需补一轮稳定环境验证。 |
| TC-RI-回归-117 | ✅ PASS | 线上 relay 下，caller `Ctrl-C` 后 target client 上 `search.get` 成功落为 `cancelled`，最终样本 `call_id=412ef871902f0195`，`ended_at=1776771986802`，`duration_ms=11660`，`exit_code=130`，且连续 3 次轮询稳定不变；caller 侧同步退出，输出 `remote call cancelled by caller` / `Remote command 'search.get' cancelled by caller.`，不再无限挂起。 |
| TC-RI-回归-118 | ✅ PASS | 本地 relay 开启 `trust_forwarded_for` 后，使用相同 `x-forwarded-for=203.0.113.x` 分别为两个不同用户注册 client，并重复访问 authenticated remote invoke API 与 `client/stream` SSE；220 次 `client/active-grants` 查询全部返回 200，两个 `client/stream` 也均建立成功，证明已认证 remote invoke 不再以共享出口 IP 作为主要限流依据。 |
| TC-RI-回归-119 | ✅ PASS | 远端 relay `bifrost-server-v4` 保持 authenticated remote invoke 主链路不在 app 内新增 pod-local limiter，只保留原有 `ssh/challenge -> device_code` 的 Redis 限制；同时 `client/stream` 直接使用 `verifyClientAuth()` 返回的 owner `user_id` 建立在线状态，不再依赖 query 透传。执行 `pnpm --dir bifrost-server-v4 build:local`、`bash e2e-tests/tests/test_remote_invoke_e2e.sh`、`bash e2e-tests/tests/test_remote_invoke_ssh_e2e.sh` 均通过，确认远端 relay 代码层没有再引入基于共享网关 IP 或单 pod 内存计数的 authenticated remote 限流。 |

### TC-RI-回归-121 执行结果（2026-04-21，CLI `--ssh-key` 回归）

**执行环境**：
- Local relay：`pnpm --dir packages/bifrost-sync-server exec tsx src/cli.ts -p <随机端口> -d <临时目录> --enable-remote-invoke`
- Bifrost Admin：`target/release/bifrost -H 127.0.0.1 -p <随机端口> start -y --access-mode allow_all --skip-cert-check --unsafe-ssl --no-system-proxy`
- Caller CLI：`BIFROST_DATA_DIR=<caller_dir> ./target/release/bifrost remote connect --ssh-key <key_file> --relay-url <relay_url>`

| 用例编号 | 结果 | 实际结果 |
|---------|------|---------|
| TC-RI-回归-121 | ✅ PASS | caller 使用 `--ssh-key` 成功完成 SSH connect，CLI 输出 `Connected with SSH key`；`remote-connections.json` 新增 `auth_method=ssh_publickey` 记录，包含 `client_instance_id`、`caller_fingerprint=<ssh_key_fingerprint>`、`device_code`；随后同一 `BIFROST_DATA_DIR` 下执行 `remote status` 成功复用该连接，不再要求重新 connect。 |

### TC-RI-回归-127 执行结果（2026-04-24，Shell Access 只读 ID）

**执行环境**：
- Playwright UI：`pnpm --dir web exec playwright test tests/ui/admin-settings.spec.ts --grep "Settings Remote Invoke 的 Shell Access 仅允许修改名称，Policy/Profile ID 为只读"`
- 管理端页面：`Settings -> Remote Invoke -> Manage Shell Access`
- 校验方式：真实浏览器交互 + `GET/PUT /api/remote-invoke/shell-config`

| 用例编号 | 结果 | 实际结果 |
|---------|------|---------|
| TC-RI-回归-127 | ✅ PASS | 在 `Manage Shell Access` 中预置的 `readonly-profile` 与 `readonly-policy` 两个 ID 输入框均带有 `readonly` 属性，可查看但不可直接修改；对应 `Profile name` 与 `Policy name` 成功编辑并保存，随后 `GET /api/remote-invoke/shell-config` 返回的 `id` 保持 `readonly-profile` / `readonly-policy` 不变，仅 `name` 更新为新的值，证明 Web UI 已收敛为“只读 ID，仅名称可修改”。 |

### TC-RI-回归-122 执行结果（2026-04-21，server-v4 SSH caller 上下文）

**执行环境**：
- `pnpm --dir bifrost-server-v4 build:local`
- `bash e2e-tests/tests/test_remote_invoke_server_v4_ssh_context.sh`

| 用例编号 | 结果 | 实际结果 |
|---------|------|---------|
| TC-RI-回归-122 | ✅ PASS | 脚本使用本地编译后的 `bifrost-server-v4` service 与 fake Redis 驱动 `syncSshDeviceRoute -> issueSshChallenge -> verifySshConnect -> submitSshConnectResult` 全链路；验证 `ri:ssh_connect:<connect_id>` 中已持久化 `caller_info.hostname=ci-host` / `username=ci-user`，最终 SSH grant 的 `caller_display_name` 恢复为 `ci-host`，且 caller 侧 `ssh_connect_result` 队列事件继续携带 `client_instance_id=client-ssh-ctx`。 |

### TC-RI-回归-123 执行结果（2026-04-22，远端 relay + client 重启后 grant crypto 持续可用）

**执行环境**：
- 远端 relay：[https://bifrost.bytedance.net](https://bifrost.bytedance.net)
- Target client：`BIFROST_DATA_DIR=<REPO_ROOT>/.tmp-remote-e2e-client cargo run --bin bifrost -- start -p 8810 --unsafe-ssl --no-system-proxy`
- Caller：`BIFROST_DATA_DIR=<REPO_ROOT>/.tmp-remote-restart-ssh-caller cargo run --bin bifrost -- remote ...`

| 用例编号 | 结果 | 实际结果 |
|---------|------|---------|
| TC-RI-回归-123 | ✅ PASS | 先通过 `POST /_bifrost/api/remote-invoke/ssh-key/reset` 导出新的 `bifrost_key_file`，caller 执行 `remote connect --ssh-key ... --relay-url https://bifrost.bytedance.net` 成功建立新 grant `57a4c4ee-5810-4573-8014-171f39cb85f0`；首次 `remote status` 成功返回目标状态。随后确认 client 数据目录下已生成 `admin/remote_invoke_grant_crypto.json` 与 `.key`，其中记录了该 grant 的 `caller_ephemeral_pub` / `client_ephemeral_pub` / 加密后的 `shared_secret`。重启 target client 后，不重新 `remote connect`，直接复用同一 caller 目录再次执行 `remote status` 仍成功返回状态；client 日志显示已用持久化恢复的 grant crypto 派生 open_call key 并完成解密执行，整个流程未再出现 `missing grant shared secret for encrypted remote command`。 |

### TC-RI-回归-124 执行结果（2026-04-22，远端 relay + 同一 caller 连续两次 pair-code connect）

**执行环境**：
- 远端 relay：[https://bifrost.bytedance.net](https://bifrost.bytedance.net)
- Target client：`BIFROST_DATA_DIR=<REPO_ROOT>/.tmp-remote-e2e-client cargo run --bin bifrost -- start -p 8810 --unsafe-ssl --no-system-proxy`
- Caller：`BIFROST_DATA_DIR=<REPO_ROOT>/.tmp-remote-repeat-pair-caller cargo run --bin bifrost -- remote ...`

| 用例编号 | 结果 | 实际结果 |
|---------|------|---------|
| TC-RI-回归-124 | ✅ PASS | 同一 target client 先后用两个 pair code 完成两次 `remote connect`，第一次 grant 为 `1f43205651506fe6`，第二次 grant 为 `12e91b6bef344396`。第二次 connect 后，caller 本地 [remote-connections.json](../.tmp-remote-repeat-pair-caller/remote-connections.json) 已被覆盖为第二次 grant，并落盘新的 `caller_ephemeral_pub` / `client_ephemeral_pub` / `shared_secret_encrypted`；同时 client 侧 `GET /api/remote-invoke/grants` 仍可看到两条 active pair-code grant 并存，证明“服务端多 grant 并存 + caller 本地只保留最后一次连接”这一高风险场景已真实出现。随后直接用同一 caller 目录执行 `cargo run --bin bifrost -- remote status --relay-url https://bifrost.bytedance.net`，日志先告警 relay `grants/reusable` 返回了另一条旧 grant `b4f32cf75bf2b73e`，caller 改为整套使用本地最后一次连接保存的 `grant_id=12e91b6bef344396` 与对应 ephemeral pub，最终 `remote status` 成功返回目标设备状态 JSON，不再出现 `missing grant shared secret` 或 `caller_ephemeral_pub does not match saved encrypted transport context`。 |

### TC-RI-回归-125 执行结果（2026-04-22，远端 relay + 同一 caller 先 SSH key 后 pair-code）

**执行环境**：
- 远端 relay：[https://bifrost.bytedance.net](https://bifrost.bytedance.net)
- Target client：`BIFROST_DATA_DIR=<REPO_ROOT>/.tmp-remote-e2e-client cargo run --bin bifrost -- -H 127.0.0.1 -p 8810 start -y --skip-cert-check --unsafe-ssl --no-system-proxy`
- Caller：`BIFROST_DATA_DIR=<REPO_ROOT>/.tmp-remote-mixed-auth-caller cargo run --bin bifrost -- remote ...`

| 用例编号 | 结果 | 实际结果 |
|---------|------|---------|
| TC-RI-回归-125 | ✅ PASS | 先在 target client 上 `POST /_bifrost/api/remote-invoke/ssh-key/reset` 导出新的 `bifrost_key_file`，同一 caller 目录执行 `remote connect --ssh-key ... --relay-url https://bifrost.bytedance.net` 成功建立 `ssh_publickey` grant `5f25ecf0-8b1a-4804-9f47-b379a212e58a`，caller 本地 [remote-connections.json](../.tmp-remote-mixed-auth-caller/remote-connections.json) 记录为 `auth_method=ssh_publickey` 且落盘对应加密上下文，随后 `remote status` 成功返回目标状态。接着在同一 target client 上开启 discovery mode，并继续用同一个 caller 目录执行 pair-code connect，成功建立 `pair_code` grant `208006cff246d9b2`；此时 caller 本地连接记录被新连接覆盖为 `auth_method=pair_code`，而 client 侧 `GET /api/remote-invoke/grants` 同时存在一条 `ssh_publickey` active grant 和一条 `pair_code` active grant。最后再用同一 caller 目录执行 `remote status`，CLI 明确按本地最后一次连接的 pair-code transport context 工作，虽然 relay `grants/reusable` 返回了另一条旧 grant `b4f32cf75bf2b73e`，但 caller 仍稳定回退到本地保存的 `grant_id=208006cff246d9b2` 与对应 ephemeral pub，命令成功返回目标设备状态 JSON。结论是：同一 client 上两种 grant 可以同时在服务端共存并都有效，但同一个 caller 数据目录只保存最后一次 connect 的那一种模式，不会同时保留两套本地加密上下文。 |

### TC-RI-回归-126 执行结果（2026-04-22，Recent Calls 命令摘要恢复）

**执行环境**：
- 远端 relay：[https://bifrost.bytedance.net](https://bifrost.bytedance.net)
- Target client：`BIFROST_DATA_DIR=<REPO_ROOT>/.tmp-remote-e2e-client target/debug/bifrost -H 127.0.0.1 -p 8810 start -y --skip-cert-check --unsafe-ssl --no-system-proxy`
- Caller：`BIFROST_DATA_DIR=<REPO_ROOT>/.tmp-human-recentcalls-caller target/debug/bifrost remote ...`

| 用例编号 | 结果 | 实际结果 |
|---------|------|---------|
| TC-RI-回归-126 | ✅ PASS | 先在 target client 上重新进入 discovery mode，caller 用新的 pair code `465613` 完成连接并建立 grant `d7d94397baaf6212`。随后执行 `target/debug/bifrost remote status --relay-url https://bifrost.bytedance.net` 成功返回目标设备状态 JSON。紧接着查询 `GET http://127.0.0.1:8810/_bifrost/api/remote-invoke/calls`，最新记录返回 `{ "command_summary": { "command_preview": "status" }, "command": { "command": "status", "kind": "query.readonly" }, "command_kind": "query.readonly", "status": "completed" }`。这说明 Recent Calls 已不再展示空白或协议级占位的 `query.readonly`，而是恢复为真实可读的命令摘要 `status`。 |

### TC-RI-回归-127：shell E2E 夹具与当前加密协议保持一致

**背景**：Remote Invoke 已切到加密 `command_encrypted` / transport context 链路后，shell E2E 中的部分 mock relay 与手工 `open_call` 夹具如果仍停留在旧协议，会把“测试桩过期”误报成产品回归。

**前置条件**：
- 仓库已完成 `cargo build --release --bin bifrost`
- 本机可启动本地 `packages/bifrost-sync-server`
- 测试使用独立 `BIFROST_DATA_DIR`

**操作步骤**：
1. 执行 `bash e2e-tests/tests/test_remote_connect_overload_retry_e2e.sh`
2. 确认“短暂 overload 后 connect 成功”和“持续 overload 后给出可行动提示”两条路径都通过
3. 执行 `bash e2e-tests/tests/test_remote_relay_url_fallback_e2e.sh`
4. 确认可显式 `--relay-url`、运行中实例 `sync.remote_base_url`、本地配置 `sync.remote_base_url` 三条优先级路径都通过
5. 执行 `bash e2e-tests/tests/test_remote_invoke_ssh_e2e.sh`
6. 确认 `remote connect --ssh-key` 成功后，继续通过真实 CLI 执行 `remote status`、`remote search`、`remote traffic get`，不再手工构造旧版明文 `calls/open` 请求

**预期结果**：
- pair-code connect 相关 shell E2E 中，mock relay 返回的批准结果可让 caller 成功落盘当前加密 transport context
- relay URL 回退脚本的 3 个 case 全部通过，不再出现 `pairing succeeded but relay did not return client_ephemeral_pub`
- SSH shell E2E 中，`remote search` 与 `remote traffic get` 都能经由真实 CLI 成功执行，Recent Calls 中记录的命令状态为 `completed`

### TC-RI-回归-127 执行结果（2026-04-22，shell E2E 协议对齐）

**执行环境**：
- `bash e2e-tests/tests/test_remote_connect_overload_retry_e2e.sh`
- `bash e2e-tests/tests/test_remote_relay_url_fallback_e2e.sh`
- `bash e2e-tests/tests/test_remote_invoke_ssh_e2e.sh`

| 用例编号 | 结果 | 实际结果 |
|---------|------|---------|
| TC-RI-回归-127 | ✅ PASS | `remote connect` 相关 mock relay 已补齐 `client_ephemeral_pub`，`overload retry` 与 `relay-url fallback` 两个 shell E2E 不再因旧批准 payload 报错；SSH 用例改为复用真实 CLI 执行 `remote status`、`remote search`、`remote traffic get`，不再命中旧版明文 `calls/open` 的 500，且 Recent Calls 中新增的 `search.get` / `traffic.get` 记录状态均为 `completed`。 |

### TC-RI-回归-128：Recent Calls 必须展示参数预览并支持完整 Tooltip

**背景**：Remote Invoke `openCall` 切换到密文 `command_encrypted` 后，relay 不再稳定下发明文 `command_summary.masked_args_json`。如果 client 本地没有用解密后的 `command.args_json` 做回退，Web UI 的 `Recent Calls` 就会只显示命令名，不显示参数详情。

**前置条件**：
- 已完成可复用授权
- target client 已启动 Web Admin，且使用独立 `BIFROST_DATA_DIR`
- 本机可以执行 `bash e2e-tests/tests/test_remote_invoke_e2e.sh`

**操作步骤**：
1. 执行 `bash e2e-tests/tests/test_remote_invoke_e2e.sh`
2. 在脚本跑完 `remote search "$REMOTE_MARKER" --max-results 5 --max-scan 50` 后，调用 `GET /_bifrost/api/remote-invoke/calls`
3. 定位最新一条 `search.get` 调用
4. 检查该记录的 `command_summary.masked_args_json`
5. 打开 target client Web UI 的 `Settings -> Remote Invoke -> Recent Calls`，查看同一条记录
6. 确认标题行展示参数预览，并 hover 参数区域查看 Tooltip

**预期结果**：
- `command_summary.masked_args_json` 非空
- `masked_args_json` 中包含 `query=<marker>`、`max_results=5`、`max_scan=50`
- Recent Calls 标题行在 `search.get` 后展示参数预览
- hover 参数预览时，Tooltip 展示完整 JSON
- connect / status 等无参数命令仍不展示多余参数占位

### TC-RI-回归-128 执行结果（2026-04-23，Recent Calls 参数预览恢复）

**执行环境**：
- Local relay：`npx tsx packages/bifrost-sync-server/src/cli.ts -p <随机端口> -d <临时目录> --enable-remote-invoke`
- Target client：`target/release/bifrost -H 0.0.0.0 -p <随机端口> start -y --access-mode allow_all --skip-cert-check --unsafe-ssl --no-system-proxy`
- Caller：`target/release/bifrost remote connect/search --relay-url http://127.0.0.1:<relay_port>`
- 前端单测：`pnpm --dir web test:unit -- src/api/remoteInvoke.test.ts`

| 用例编号 | 结果 | 实际结果 |
|---------|------|---------|
| TC-RI-回归-128 | ✅ PASS | 本轮执行 `bash e2e-tests/tests/test_remote_invoke_recent_calls_args_preview_e2e.sh` 完成 `remote connect -> remote search` 最短闭环。脚本在 target client 上读取 `GET /_bifrost/api/remote-invoke/calls`，成功定位最新 `search.stream` 记录，并拿到非空 `command_summary.masked_args_json`，其 JSON 实际包含 `keyword=<marker>`、`max_results=5`、`max_scan=50`。同时执行 `pnpm --dir web test:unit -- src/api/remoteInvoke.test.ts`，验证 Recent Calls 前端优先使用 `command_summary.masked_args_json`，缺失时回退到 `command.args_json` / `command.query.args`，且标题预览与 Tooltip 共用同一来源。结合 API E2E 与前端单测，确认 Recent Calls 参数预览已恢复，不再出现只有命令名、没有参数详情的退化。 |

### 本轮实际执行回归（2026-04-21，远端 relay 收尾修正）

**自动化回归**：
- `e2e-tests/tests/test_remote_invoke_e2e.sh`：✅ PASS
- `e2e-tests/tests/test_remote_invoke_ssh_e2e.sh`：✅ PASS

**构建校验**：
- `pnpm --dir bifrost-server-v4 build:local`：✅ PASS

**项目级校验**：
- `cargo fmt --all -- --check`：✅ PASS
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`：✅ PASS
- `cargo test --workspace --all-features`：⚠️ 首次执行命中 `bifrost-e2e` 既有端口占用型 flaky，失败点为 `tests::curl_mock::tests::test_curl_mock_combined` 绑定端口冲突；随后单独执行 `cargo test -p bifrost-e2e --lib tests::curl_mock::tests::test_curl_mock_combined -- --exact`：✅ PASS

### TC-RI-回归-120 执行结果（2026-04-21，disconnect 幂等清理）

**执行环境**：
- Bifrost Admin：`target/release/bifrost -H 127.0.0.1 -p <随机端口> start -y --access-mode allow_all --skip-cert-check --unsafe-ssl --no-system-proxy`
- Local relay：`pnpm --dir packages/bifrost-sync-server exec tsx src/cli.ts -p <随机端口> -d <临时目录> --enable-remote-invoke`
- Caller：`target/release/bifrost remote connect/disconnect --relay-url http://127.0.0.1:<relay_port>`

| 用例编号 | 结果 | 实际结果 |
|---------|------|---------|
| TC-RI-回归-120 | ✅ PASS | 本轮本地 relay 的 grant 删除接口表现为幂等 `200`，因此按用例中的兜底路径，将 caller 本地 `remote-connections.json` 里的 `grant_id` 替换为 `missing-grant-for-disconnect-test` 以稳定复现 `grant_not_found`；随后执行 `bifrost remote disconnect --all --relay-url http://127.0.0.1:<relay_port>`，CLI 输出 `relay grant already missing, local record removed`，`remote-connections.json` 中连接数从 `1` 变为 `0`，再次执行 `bifrost remote status` 时提示无可用连接，确认幽灵状态已消失。2026-04-23 针对 shell E2E 复验时，又额外覆盖了“前一段 stale grant 回归之后先 fresh reconnect，再手动 DELETE relay grant，再执行 disconnect --all”的路径：本地 `grant_id` / `caller_fingerprint` 均非空，预删 relay grant 返回 `200`，CLI 输出包含成功标识，本地连接记录再次从 `1` 收敛到 `0`。 |

---

### TC-RI-回归-129：超时后的旧 pending pairing 不应继续占用 pair slot

**背景**：如果第一次 `remote connect` 发起后无人审批直至超时，旧 pairing 必须从 relay 的“活跃待审批”集合里移除。否则用户重新获取新的 pair code 后，第二次 `remote connect` 会被误判为 `pair_slot_occupied`。

**前置条件**：
- 使用本地 relay，且将 `pair_code_ttl_secs` 调小到 2~5 秒，便于快速验证超时收敛
- target client 已完成 sync 登录并处于 `Remote Invoke` 可用状态

**操作步骤**：
1. 在 target client 上进入 discovery mode，拿到第一组 `pair_code_1`
2. 在 caller 侧执行：
   ```bash
   BIFROST_DATA_DIR=<caller_dir> ./target/release/bifrost remote connect <pair_code_1> --relay-url http://127.0.0.1:<relay_port>
   ```
3. 在 target client 的 `Pending Pairing Requests` 中确认出现该请求，但**不要**审批
4. 等待超过 `pair_code_ttl_secs`，确认该请求从 Pending 列表消失
5. 在 target client 上刷新 discovery mode，拿到新的 `pair_code_2`
6. 再次执行：
   ```bash
   BIFROST_DATA_DIR=<caller_dir> ./target/release/bifrost remote connect <pair_code_2> --relay-url http://127.0.0.1:<relay_port>
   ```
7. 在 target client 上批准第二次请求

**预期结果**：
- 第 4 步：旧请求会自动从 Pending 列表消失
- 第 6 步：第二次 `remote connect` 不会返回 `pair_slot_occupied`
- 第 7 步：新的 pairing 可以正常审批并建立连接

### TC-RI-回归-130：超时 pairing 在 Pending 列表里不会复活，点击审批也不再报 500

**背景**：修复前，relay 的 pending 列表和审批接口对超时 pairing 的语义不一致，前端可能反复看到已经超时的请求，点击 Authorize / Reject 时还会报 500。

**前置条件**：
- 使用本地 relay，且将 `pair_code_ttl_secs` 调小到 2~5 秒
- target client 已打开 `Settings > Remote Invoke`

**操作步骤**：
1. 创建一个新的 pairing 请求，并在 Pending Pairing Requests 中确认它已出现
2. 等待该 pairing 超时
3. 反复观察 Pending Pairing Requests 列表至少 10 秒，确认旧请求没有再次出现
4. 如果列表仍短暂展示旧请求，在它消失前立刻点击 `Authorize` 或 `Reject`
5. 重新请求 `GET /_bifrost/api/remote-invoke/pairings/pending`

**预期结果**：
- 第 3 步：超时 pairing 不会因 relay 轮询再次回到 Pending 列表
- 第 4 步：前端最多显示 warning 提示 `Request may have expired and was removed`，不再出现 500
- 第 5 步：接口返回中不再包含该 pairing；relay 存储中的该记录状态应已收敛为 `expired`

| 用例编号 | 结果 | 实际结果 |
|---------|------|---------|
| TC-RI-回归-129 | ✅ PASS | 2026-04-23 在本地隔离环境执行：relay `pair_code_ttl_secs=3`、target admin `8812`、relay `8688`。第一次 pairing `WHjmo7NTGwQTJaMl9B3Bm` 超时后，`GET /_bifrost/api/remote-invoke/pairings/pending` 返回 `0` 条，relay SQLite 中该记录状态为 `expired`。随后使用新 pair code 发起第二次连接，审批接口返回 `200`，caller `remote connect` 退出码为 `0`，日志中未出现 `pair_slot_occupied`。 |
| TC-RI-回归-130 | ✅ PASS | 同一轮本地隔离环境中，第三次 pairing `dsnDosyo_FLdrCeF_XRF5` 超时后再次触发审批，target admin 返回 `404`，响应体为 `Failed to approve pairing: Network error: pairing dsnDosyo_FLdrCeF_XRF5 not found or expired`，不再出现 `500`。随后 `GET /_bifrost/api/remote-invoke/pairings/pending` 仍返回 `0` 条，relay SQLite 中该 pairing 状态为 `expired`。 |

---

### TC-RI-回归-131：client 本地 grant crypto 丢失后，重连必须主动清理幽灵授权

**背景**：修复前，如果 target client 本地 `admin/remote_invoke_grant_crypto.json(.key)` 丢失，但 relay 上仍保留 active grant，client SSE 重连后会重新展示这条 grant，caller 再执行 `remote status` 时会把命令发到远端，最终在 target 侧报 `missing grant shared secret for encrypted remote command; reconnect is required`。这会把本应在“授权发现阶段”暴露的问题拖到真正执行命令时才失败。

**前置条件**：
- 使用本地 relay
- target client 与 caller 已经通过 `remote connect` 建立一条可复用授权
- caller 本地 `remote-connections.json` 仍保留这条连接，便于验证“旧连接再次执行命令”的表现

**操作步骤**：
1. 用真实 CLI 跑通一次完整闭环，至少覆盖：
   ```bash
   BIFROST_DATA_DIR=<caller_dir> ./target/debug/bifrost remote connect <pair_code> --relay-url http://127.0.0.1:<relay_port>
   BIFROST_DATA_DIR=<caller_dir> ./target/debug/bifrost remote status --relay-url http://127.0.0.1:<relay_port> --client-id <client_prefix>
   BIFROST_DATA_DIR=<caller_dir> ./target/debug/bifrost remote traffic list --relay-url http://127.0.0.1:<relay_port> --client-id <client_prefix>
   BIFROST_DATA_DIR=<caller_dir> ./target/debug/bifrost remote search <keyword> --relay-url http://127.0.0.1:<relay_port> --client-id <client_prefix>
   ```
2. 删除 target client 数据目录下的 grant crypto 持久化文件：
   ```bash
   rm -f <client_data_dir>/admin/remote_invoke_grant_crypto.json <client_data_dir>/admin/remote_invoke_grant_crypto.key
   ```
3. 重启 target client，等待 `/_bifrost/api/remote-invoke/status` 恢复 `Connected`
4. 查询 target client 的 grants 列表：
   ```bash
   curl -sS http://127.0.0.1:<admin_port>/_bifrost/api/remote-invoke/grants
   ```
5. 不重新 `remote connect`，直接用原 caller 数据目录再次执行：
   ```bash
   BIFROST_DATA_DIR=<caller_dir> ./target/debug/bifrost remote status --relay-url http://127.0.0.1:<relay_port> --client-id <client_prefix>
   ```
6. 检查 caller 数据目录中的 `remote-connections.json`：
   ```bash
   find <caller_dir> -name remote-connections.json -print -quit | xargs jq '.connections | length'
   ```
7. 重新生成 pair code 并执行 fresh `remote connect`，确认后续连接流程仍可用。

**预期结果**：
- 第 3 步重连后，target client 不会继续保留这条缺少本地加密材料的 grant
- 第 4 步 grants 列表为空，不再展示幽灵授权
- 第 5 步 caller 侧会直接提示授权失效/需要重新 connect；即使 relay 短暂返回 reusable grant、`open_call` 才收到 `403 grant_not_active`，CLI 也必须归一化为 `expired` / `revoked` / `connect` 语义
- 第 5 步不再出现 `missing grant shared secret for encrypted remote command`
- 第 6 步 caller 本地连接数为 `0`，stale connection 已从 `remote-connections.json` 移除
- 第 7 步新的 pairing / connect 流程可以重新建立 fresh grant，不受 stale connection 清理影响

| 用例编号 | 结果 | 实际结果 |
|---------|------|---------|
| TC-RI-回归-131 | ✅ PASS | 2026-04-30 本轮执行 `bash e2e-tests/tests/test_remote_invoke_e2e.sh`，全套 `total=73 passed=73 failed=0`。TC-RI-07A 验证 client 丢失 grant crypto 后 grants 列表为 0、caller 输出重新 connect 语义且不含 `missing grant shared secret`、caller 本地 `remote-connections.json` 连接数清空；TC-RI-07B 随后重新建立 fresh grant 并继续通过 disconnect 回归。 |

---

### TC-RI-回归-132：shell E2E 在 `--skip-build` 且缺失 sync-server dist 时仍能启动本地 relay

**背景**：CI 的 `scripts/ci/run-e2e-shell.sh` 固定带 `--skip-build`，只复用现成的 Rust `target/release/bifrost`。修复前，部分 remote invoke shell 用例仍硬编码 `node packages/bifrost-sync-server/dist/cli.js`。一旦 CI 镜像没有预编译该 dist，脚本会直接报 `Cannot find module '.../packages/bifrost-sync-server/dist/cli.js'` 并提前退出。

**前置条件**：
- 使用独立的临时数据目录运行 target admin / caller / relay
- 不使用 `9900` 端口，Bifrost Admin 必须带 `--no-system-proxy`
- 本地具备 `node`，并至少具备 `pnpm` 或 `npx` 之一

**操作步骤**：
1. 确认 `packages/bifrost-sync-server/dist/cli.js` 不存在；若存在，先临时移走：
   ```bash
   mv packages/bifrost-sync-server/dist packages/bifrost-sync-server/dist.bak
   ```
2. 执行定向 remote invoke shell E2E：
   ```bash
   bash e2e-tests/tests/test_remote_invoke_e2e.sh
   bash e2e-tests/tests/test_remote_invoke_ssh_e2e.sh
   bash e2e-tests/tests/test_remote_invoke_recent_calls_args_preview_e2e.sh
   ```
3. 可选补充执行：
   ```bash
   bash e2e-tests/tests/test_remote_search_traffic_cli_isomorphic_e2e.sh
   bash e2e-tests/tests/test_group_sync_e2e.sh
   ```
4. 验证各脚本日志中的 relay 启动阶段没有出现 `Cannot find module '.../dist/cli.js'`
5. 如第 1 步临时移走了 dist，恢复目录：
   ```bash
   mv packages/bifrost-sync-server/dist.bak packages/bifrost-sync-server/dist
   ```

**预期结果**：
- 即使缺失 `dist/cli.js`，shell E2E 仍能自动回退到 `pnpm/npx tsx src/cli.ts` 启动本地 relay
- remote invoke 相关脚本不再因 relay 入口不存在而提前失败
- 整个闭环仍能完成 pair / approve / connect / status/search/traffic 等真实命令调用

| 用例编号 | 结果 | 实际结果 |
|---------|------|---------|
| TC-RI-回归-132 | ✅ PASS | 2026-04-23 在本地隔离环境执行：先将 `packages/bifrost-sync-server/dist` 临时改名，确认工作区只保留源码入口。随后依次执行 `bash e2e-tests/tests/test_remote_invoke_e2e.sh`、`bash e2e-tests/tests/test_remote_invoke_ssh_e2e.sh`、`bash e2e-tests/tests/test_remote_invoke_recent_calls_args_preview_e2e.sh`。三条脚本都成功通过，relay 启动日志未再出现 `Cannot find module '<REPO_ROOT>/packages/bifrost-sync-server/dist/cli.js'`，说明 shell E2E 已能在 `--skip-build` 场景下自动回退到源码入口完成本地 relay 启动。 |

---

### TC-RI-回归-133：caller `open_call` 必须直接携带参数摘要，且 remote invoke shell E2E 仅在 release 过期时重建二进制

**背景**：仅依赖 client 侧“解密后回填 `command_summary.masked_args_json`”并不稳妥。若 relay `open_call` 时没有直接收到参数摘要，Recent Calls 的参数预览会退化成空值；与此同时，shell E2E 若继续复用过期的 `target/release/bifrost`，会让“源码已修复但测试仍跑旧版本”的假回归反复出现。这里需要的是“过期时自动重建”，而不是“每次都重建”。

**前置条件**：
- 使用本地 relay + target client + caller 的隔离环境
- shell E2E 未设置 `SKIP_BUILD=true`
- target client 已完成一次可复用授权

**操作步骤**：
1. 记录 target client `GET /_bifrost/api/remote-invoke/calls` 当前最新一条 `traffic.list` / `traffic.get` / `search.stream` 的 `started_at`
2. 直接执行：
   ```bash
   bash e2e-tests/tests/test_remote_invoke_e2e.sh
   ```
3. 脚本执行完成后，再次读取 `GET /_bifrost/api/remote-invoke/calls`
4. 定位本轮新增的 `traffic.list`、`traffic.get`、`search.stream` 调用，检查其 `command_summary.masked_args_json`
5. 检查脚本日志；若源码比 `target/release/bifrost` 更新，则应出现 `Building release bifrost...`

**预期结果**：
- `traffic.list` 的 `command_summary.masked_args_json` 非空，且包含本轮 list/filter 参数
- `traffic.get` 的 `command_summary.masked_args_json` 非空，且包含 `id/request_body/response_body`
- `search.stream` 的 `command_summary.masked_args_json` 非空，且包含 `query|max_results|max_scan`
- shell E2E 会在 release 缺失或过期时自动重建 `target/release/bifrost`，不再静默复用旧 release 二进制

### TC-RI-回归-133 执行结果（2026-04-23，open_call 参数摘要与过期 release 自动重建）

**执行环境**：
- Local relay：`pnpm exec tsx packages/bifrost-sync-server/src/cli.ts -p <随机端口> -d <临时目录> --enable-remote-invoke`
- Target client：`cargo run --bin bifrost -- start -H 127.0.0.1 -p <随机端口> -y --access-mode allow_all --skip-cert-check --unsafe-ssl --no-system-proxy`
- Caller / shell E2E：`bash e2e-tests/tests/test_remote_invoke_e2e.sh`

| 用例编号 | 结果 | 实际结果 |
|---------|------|---------|
| TC-RI-回归-133 | ✅ PASS | 2026-04-23 在 `Cargo.toml` / `crates/bifrost-cli/src/commands/remote.rs` 更新时间晚于 `target/release/bifrost` 的条件下执行 `bash e2e-tests/tests/test_remote_invoke_e2e.sh`，脚本仅在检测到 release 过期时输出 `Building release bifrost...`，避免了每次强制全量重编。随后在 target client 查询 `GET /_bifrost/api/remote-invoke/calls`，本轮新增的 `traffic.list` / `traffic.get` / `search.stream` 记录都拿到了非空 `command_summary.masked_args_json`：`traffic.list` 含完整 filter 参数，`traffic.get` 含 `id + request_body + response_body`，`search.stream` 含 `query/max_results/max_scan`。同时 `remote traffic get` 返回的 JSON 详情可直接命中目标 `sequence + marker`，说明参数摘要与真实执行结果已经重新对齐。 |

---

### TC-RI-回归-134：pair-code connect 成功后，Client grants 列表应在短时间内稳定可见

**背景**：CI 上出现过 `test_remote_invoke_e2e.sh` 在 caller `remote connect` 已成功完成、日志也已出现 `Connected! Authorization granted` 后，立刻读取 `GET /_bifrost/api/remote-invoke/grants` 仍短暂返回空列表的情况。这类现象更像授权可见性的瞬时时序窗口，而不是功能性回归。

**前置条件**：
- 使用本地 relay + target client + caller 的隔离环境
- target 已连上 relay，且可进入 discovery mode

**操作步骤**：
1. 在 target 上进入 discovery mode，拿到新的 pair code
2. 在 caller 侧执行：
   ```bash
   bash e2e-tests/tests/test_remote_invoke_e2e.sh
   ```
3. 重点观察脚本中 `Verify grant is created on Client side` 这一段
4. 如需手工复核，在 approve pairing 成功后 10 秒内多次执行：
   ```bash
   curl -s "http://127.0.0.1:<admin-port>/_bifrost/api/remote-invoke/grants" | jq '.grants | length'
   ```

**预期结果**：
- caller `remote connect` 正常退出，日志包含 `Connected! Authorization granted`
- Client grants 列表允许存在短暂同步延迟，但应在 10 秒内稳定出现至少 1 条 grant
- 如果 10 秒后仍为 `0`，才视为真实失败

### TC-RI-回归-134 执行结果（2026-04-24，grant 可见性时序窗口复验）

**执行环境**：
- Local relay：脚本自动启动 `packages/bifrost-sync-server`
- Target client：`bash e2e-tests/tests/test_remote_invoke_e2e.sh` 自动使用隔离 `BIFROST_DATA_DIR` 启动
- Caller CLI：同脚本内隔离目录

| 用例编号 | 结果 | 实际结果 |
|---------|------|---------|
| TC-RI-回归-134 | ✅ PASS | 2026-04-24 在当前 checkout 执行 `bash e2e-tests/tests/test_remote_invoke_e2e.sh`，整套 72 条断言全部通过。针对 `Verify grant is created on Client side` 已改为“最多等待 10 秒”的短轮询；本轮本地复验中 grants 列表在窗口内稳定出现，随后 `remote status`、`remote traffic list/get/search`、取消、reject pairing、stale grant 清理与 disconnect 回归也全部通过，说明该改动只吸收了瞬时时序抖动，没有放宽后续真实链路验证。 |

---

### TC-RI-回归-135：Recent Calls 参数预览回归脚本必须使用本地 mock 流量，不依赖公网 `httpbin.org`

**背景**：`test_remote_invoke_recent_calls_args_preview_e2e.sh` 之前通过 `curl --proxy ... http://httpbin.org/anything/<marker>` 造流量。CI runner 一旦公网抖动，脚本会在 `set -e` 下直接退出，shell 汇总里只会残留“批准配对应返回 200”这样的上一条成功日志，难以判断到底是 connect 失败还是公网依赖失败。

**前置条件**：
- 使用隔离的 target admin / caller / relay 数据目录
- 不使用 `9900` 端口，target admin 启动参数包含 `--no-system-proxy`
- 本地具备 `python3`，可启动 `e2e-tests/mock_servers/http_echo_server.py`

**操作步骤**：
1. 执行：
   ```bash
   bash e2e-tests/tests/test_remote_invoke_recent_calls_args_preview_e2e.sh
   ```
2. 观察脚本输出，确认其在启动 target admin 后还会启动本地 echo fixture：
   ```text
   [remote-invoke-args-preview-e2e] Starting local HTTP echo fixture on port <随机端口>
   ```
3. 脚本结束后，读取 target client 的 Recent Calls API：
   ```bash
   curl -s "http://127.0.0.1:<admin-port>/_bifrost/api/remote-invoke/calls" | jq '.calls[0].command_summary'
   ```
4. 定位本轮 `search.stream` 调用，检查 `command_summary.masked_args_json`

**预期结果**：
- 脚本不再访问公网 `httpbin.org`，即使 CI 外网受限也能完成 Recent Calls 回归
- 本地 echo fixture 能成功生成带 `recent-calls-preview-<随机值>` 的流量记录
- `search.stream` 的 `command_summary.masked_args_json` 非空，且包含 `keyword/max_results/max_scan`

### TC-RI-回归-135 执行结果（2026-04-24，Recent Calls 参数预览 shell E2E 去公网依赖）

**执行环境**：
- Local relay：脚本自动启动 `packages/bifrost-sync-server`
- Target admin：脚本自动启动隔离 `target/release/bifrost ... --no-system-proxy`
- Local mock：脚本自动启动 `e2e-tests/mock_servers/http_echo_server.py --port <随机端口> --retries 5`

| 用例编号 | 结果 | 实际结果 |
|---------|------|---------|
| TC-RI-回归-135 | ✅ PASS | 2026-04-24 在当前 checkout 连续复验 `bash e2e-tests/tests/test_remote_invoke_recent_calls_args_preview_e2e.sh`。脚本输出明确显示已启动本地 `HTTP echo fixture`，不再依赖公网 `httpbin.org`。本轮 Recent Calls API 成功返回最新 `search.stream` 记录，`command_summary.masked_args_json` 非空，JSON 中实际包含 `keyword=recent-calls-preview-<随机值>`、`max_results=5`、`max_scan=50`，对应断言 `TC-RI-ARGS-01` 通过。此前“approve 后只剩上一条成功日志”的无诊断退出现象未再复现。 |

---

### TC-RI-回归-136：Recent Calls 在 Bifrost 重启后必须从本地落盘恢复

**背景**：`Recent Calls` 之前只保存在目标客户端 `RemoteInvokeWorker.call_history` 内存队列中。Bifrost 进程重启后 worker 重新创建，Settings -> Remote Invoke 页面会显示 `No recent calls`，即使重启前刚刚执行过远程命令。

**前置条件**：
- 使用隔离的 target admin / caller / relay 数据目录
- target admin 使用动态端口，禁止使用 `9900`
- target admin 启动参数包含 `--no-system-proxy`
- target admin 的 `BIFROST_DATA_DIR` 在重启前后保持相同

**操作步骤**：
1. 执行：
   ```bash
   bash e2e-tests/tests/test_remote_invoke_recent_calls_args_preview_e2e.sh
   ```
2. 脚本建立本地 relay、target admin、caller，并完成 pair-code 授权。
3. 脚本执行 `bifrost remote status --relay-url <relay> --client-id <client-prefix>`。
4. 脚本读取 `/_bifrost/api/remote-invoke/calls`，记录最新 `status` 调用的 `call_id`。
5. 脚本检查 `<BIFROST_DATA_DIR>/admin/remote_invoke_call_history.json`。
6. 脚本停止 target admin，但保留同一个 `BIFROST_DATA_DIR`，随后重新启动 target admin。
7. 脚本再次读取 Recent Calls API，查找步骤 4 的同一个 `call_id`。
8. 脚本调用 `DELETE /_bifrost/api/remote-invoke/calls`，再次读取 Recent Calls API。

**预期结果**：
- 重启前 Recent Calls API 包含本轮 `status` 调用
- `remote_invoke_call_history.json` 存在且包含同一个 `call_id`
- 重启后 Recent Calls API 仍包含同一个 `call_id`
- 恢复后的 `command_summary.command_preview` 仍为 `search.stream`
- 调用清理接口后 Recent Calls API 返回空列表
- 全流程不使用正式 `9900` 端口，不修改系统代理

### TC-RI-回归-136 执行结果（2026-04-24，Recent Calls 本地落盘恢复）

**执行环境**：
- Local relay：脚本自动启动 `packages/bifrost-sync-server`
- Target admin：脚本自动启动隔离 `target/release/bifrost ... --no-system-proxy`
- Caller CLI：脚本使用独立 `CALLER_DATA_DIR`

| 用例编号 | 结果 | 实际结果 |
|---------|------|---------|
| TC-RI-回归-136 | ✅ PASS | 2026-04-24 在当前 checkout 执行 `bash e2e-tests/tests/test_remote_invoke_recent_calls_args_preview_e2e.sh`。脚本使用随机 relay/admin 端口和隔离数据目录，生成 `search.stream` Recent Calls 后确认 `command_summary.masked_args_json` 含 `keyword/max_results/max_scan`；`<BIFROST_DATA_DIR>/admin/remote_invoke_call_history.json` 存在且包含同一个 `call_id`；随后脚本停止 target admin 但保留同一数据目录，再次启动后 Recent Calls API 仍返回同一个 `call_id`，`command_summary.command_preview=search.stream`；最后调用 `DELETE /_bifrost/api/remote-invoke/calls` 后 Recent Calls API 返回空列表，证明重启不丢失且支持清理全部记录。 |

---

### TC-RI-回归-137：Recent Calls 超长命令必须按 120 字符截断并可点击查看详情

**背景**：Recent Calls 列表曾把超长 `shell.exec` 命令、caller、policy、exec mode 放在同一个横向容器中。超长命令会撑高列表行，并把右侧 caller / policy / exec mode 挤成竖排，导致页面布局错乱；如果完整超长命令继续写入本地历史文件，也会放大 `remote_invoke_call_history.json` 的存储风险。本次要求命令相关字段超过 120 字符时截断，落盘与 API 均不得保留完整长文本。

**前置条件**：
- 当前机器正式 Bifrost 可运行在 `9900`，但本用例验证服务必须使用隔离数据目录和非 `9900` 端口
- 验证服务启动时必须携带 `--no-system-proxy`
- 浏览器访问目标为隔离端口的 `/_bifrost/settings?tab=remote-invoke`
- Recent Calls 中至少存在一条包含长 `shell.exec` 文本、长 caller display name、`policy_id`、`exec_mode` 的记录

**操作步骤**：
1. 启动隔离 Bifrost：
   ```bash
   BIFROST_DATA_DIR=/tmp/bifrost-ri-long-call-ui cargo run --bin bifrost -- start -H 127.0.0.1 -p <admin_port> --unsafe-ssl --no-system-proxy
   ```
2. 通过真实 Recent Calls API 或测试夹具生成一条超长 `shell.exec` 调用记录，命令内容长度不少于 300 字符，且包含 `policy_id=full-access`、`exec_mode=shell_text`。
3. 在 Chrome 中打开：
   ```text
   http://127.0.0.1:<admin_port>/_bifrost/settings?tab=remote-invoke
   ```
4. 观察 `Recent Calls` 列表中该记录的首行布局。
5. 点击该记录，或点击记录右侧详情按钮。
6. 在弹出的详情窗口中检查命令、参数 JSON、调用 ID、状态、caller、policy、exec mode、耗时与流量信息。
7. 读取 `/_bifrost/api/remote-invoke/calls` 与 `<BIFROST_DATA_DIR>/admin/remote_invoke_call_history.json`。
8. 停止测试 Bifrost，保留同一个 `BIFROST_DATA_DIR` 后重新启动。
9. 再次读取 `/_bifrost/api/remote-invoke/calls`，按第 7 步记录的长参数调用 `call_id` 查回同一条记录。
10. 关闭详情窗口并刷新 Recent Calls。

**预期结果**：
- 超长命令和参数预览在列表中只占一行，超过可用宽度自动省略，不撑高行高
- caller、policy、exec mode 标签仍保持横向显示，不被挤压成逐字竖排
- 列表右侧存在可点击的详情操作
- 点击记录或详情按钮后弹出 `Call Detail` 窗口
- API 和详情窗口中的命令相关长文本最多 120 字符，并保留可识别前缀
- `masked_args_json` / `args_json` 仍保持合法 JSON，其中 JSON 字符串值最多 120 字符
- `remote_invoke_call_history.json` 不包含完整 300+ 字符命令原文或完整超长参数
- Bifrost 重启后，同一条长参数调用仍可按 `call_id` 恢复，且长字段仍保持最多 120 字符
- 刷新列表后布局仍保持稳定
- 全流程不使用 `9900` 作为测试端口，不修改系统代理

### TC-RI-回归-137 执行结果（2026-04-24，Recent Calls 长内容截断与详情弹窗）

| 用例编号 | 结果 | 实际结果 |
|---------|------|---------|
| TC-RI-回归-137 | ✅ PASS | 2026-04-24 在当前 checkout 执行 `HTTP_PROXY=http://127.0.0.1:9900 HTTPS_PROXY=http://127.0.0.1:9900 bash e2e-tests/tests/test_remote_invoke_recent_calls_args_preview_e2e.sh`。脚本使用随机 relay/admin/mock 端口，target admin 使用隔离数据目录和 `--no-system-proxy`；普通 `search.stream` 的 `masked_args_json` 仍是合法 JSON 并包含 `keyword/max_results/max_scan`；长 keyword 场景验证 `masked_args_json.keyword` 被截断到 120 字符以内且保留前缀；`remote_invoke_call_history.json` 不包含完整长参数；随后同一数据目录重启后 Recent Calls 仍恢复普通调用与同一条长参数调用，且长参数调用的 `masked_args_json.keyword` 仍保持 120 字符以内；最后 `DELETE /_bifrost/api/remote-invoke/calls` 后列表为空。同日补充执行真实浏览器布局验证：使用隔离数据目录 `/tmp/bifrost-ri-long-call-ui` 与非正式端口 `8813` 启动 `BIFROST_DATA_DIR=/tmp/bifrost-ri-long-call-ui cargo run --bin bifrost -- start -H 127.0.0.1 -p 8813 --unsafe-ssl --no-system-proxy`，通过真实 API 构造 `call-long-layout-actual-001` 后打开 `http://127.0.0.1:8813/_bifrost/settings?tab=remote-invoke`；Playwright 测得列表行高度约 `60.7px`，长命令与参数单行省略，caller/policy/exec mode 保持横向标签，policy 标签宽度限制为 `120px`；点击记录后弹出 `Call Detail`，详情中可见完整 `call_id`、状态、caller、policy、exec mode、命令与参数区，截图为 `/tmp/bifrost-recent-calls-row-real.png` 和 `/tmp/bifrost-recent-calls-detail-real.png`。 |

---

### TC-RI-回归-138：Grants 列表展示首次连接时间与最近一次执行命令时间

**背景**：用户在 Grants 列表中只能看到授权状态、scope、mode 等信息，无法区分“caller 首次连接成功但还没有执行命令”和“已经执行过命令”。本次要求 Grants 列表同时展示首次连接上来的时间和最近一次执行命令的时间。

**前置条件**：
- 使用隔离的 target admin / caller / relay 数据目录
- target admin 使用动态端口，禁止使用 `9900`
- target admin 启动参数包含 `--no-system-proxy`
- 当前 shell 显式设置 `HTTP_PROXY=http://127.0.0.1:9900` 与 `HTTPS_PROXY=http://127.0.0.1:9900`，但测试服务本身不得占用 `9900`

**操作步骤**：
1. 执行：
   ```bash
   HTTP_PROXY=http://127.0.0.1:9900 HTTPS_PROXY=http://127.0.0.1:9900 bash e2e-tests/tests/test_remote_invoke_recent_calls_args_preview_e2e.sh
   ```
2. 脚本启动本地 relay、隔离 target admin、caller，并完成 pair-code 授权。
3. 脚本在执行任何远程命令前读取 `/_bifrost/api/remote-invoke/grants`。
4. 记录同一条 grant 的 `grant_id`、`first_connected_at`、`last_command_at`。
5. 脚本执行一次真实 remote search 命令。
6. 脚本再次读取 `/_bifrost/api/remote-invoke/grants`，按步骤 4 的 `grant_id` 查回同一条 grant。
7. 打开 WebUI `Settings → Remote Invoke → Grants`，检查对应 grant 行文案。
8. 执行 `bifrost setting grant list -p <admin_port>`，检查 CLI 输出。

**预期结果**：
- 首次读取 Grants API 时，`first_connected_at` 非空，且等于或兼容 `first_authorized_at`
- 首次读取 Grants API 时，`last_command_at` 为空，表示仅连接/授权，还没有执行命令
- 执行 remote search 后，再次读取同一 grant 时 `first_connected_at` 不变化
- 执行 remote search 后，再次读取同一 grant 时 `last_command_at` 非空，且不早于 `first_connected_at`
- WebUI Grants 行展示 `connected <时间>` 与 `last command <时间或 ->`
- CLI grant list 展示 `first connected: <时间> | last command: <时间或 ->`
- 全流程不使用 `9900` 作为测试端口，不修改系统代理

### TC-RI-回归-138 执行结果（2026-04-27，Grants 时间字段）

| 用例编号 | 结果 | 实际结果 |
|---------|------|---------|
| TC-RI-回归-138 | ✅ PASS | 2026-04-27 在当前 checkout 执行 `HTTP_PROXY=http://127.0.0.1:9900 HTTPS_PROXY=http://127.0.0.1:9900 bash e2e-tests/tests/test_remote_invoke_recent_calls_args_preview_e2e.sh` 完成 release 重建与完整回归；随后用已构建二进制执行 `HTTP_PROXY=http://127.0.0.1:9900 HTTPS_PROXY=http://127.0.0.1:9900 SKIP_BUILD=true bash e2e-tests/tests/test_remote_invoke_recent_calls_args_preview_e2e.sh` 复测新增 CLI 断言。两轮均使用随机 relay/admin/mock 端口，target admin 使用隔离数据目录和 `--no-system-proxy`。脚本在执行命令前读取 Grants API，确认 `grant_id` 与 `first_connected_at` 非空、`last_command_at` 为空；执行真实 `remote traffic search` 后再次读取同一 grant，确认 `first_connected_at` 不变化、`last_command_at` 非空且不早于首次连接时间；第二轮额外执行 `bifrost -p <admin_port> setting grant list`，输出包含 `first connected:` 与 `last command:`。 |

---

### TC-RI-回归-139：Remote Status 卡片不再展示 Active Calls

**背景**：`Remote Status` 卡片中原有的 `Active Calls` 计数信息价值较低，且底部 `Recent Calls` 已经按实际执行记录展示 caller、状态、命令、policy 与详情。为减少重复信息，本次移除状态卡片内的 `Active Calls` 行。

**前置条件**：
- 使用隔离的 Bifrost 数据目录启动 WebUI 测试环境
- 测试端口由 Playwright / 测试夹具分配，禁止使用 `9900`
- Bifrost 启动参数必须包含 `--no-system-proxy`
- Remote Invoke 页面可打开，底部 `Recent Calls` 区域可见

**操作步骤**：
1. 执行：
   ```bash
   pnpm --dir web exec playwright test tests/ui/admin-settings.spec.ts:307
   ```
2. 打开 WebUI `Settings -> Remote Invoke`。
3. 定位 `Remote Status` 卡片中的 `Connection Status` 区块。
4. 检查该区块展示内容。
5. 继续检查页面底部 `Recent Calls` 区域的 caller 展示能力。

**预期结果**：
- `Remote Status` 卡片仍显示 `Connection Status`
- `Connection Status` 区块仍显示 `Relay Connection`、`Instance ID`、`Device` 与 `Sync Account`
- `Connection Status` 区块不再显示 `Active Calls`
- `Discovery Mode`、`SSH Key`、`Shell Access` 等区域仍可见
- `Recent Calls` 列表中仍通过 `by <caller>` 展示调用方
- 全流程不使用 `9900` 作为测试端口，不修改系统代理

### TC-RI-回归-139 执行结果（2026-04-28，Remote Status 移除 Active Calls）

| 用例编号 | 结果 | 实际结果 |
|---------|------|---------|
| TC-RI-回归-139 | ✅ PASS | 2026-04-28 在当前 checkout 执行 `pnpm --dir web exec playwright test tests/ui/admin-settings.spec.ts:307`。测试使用 Playwright 夹具模拟已登录 Sync session、connected Remote Invoke 状态和一条 caller 为 `Remote Tester` 的 Recent Call；页面打开 `Settings -> Remote Invoke` 后，`Remote Status` 卡片可见，`Connection Status` 区块包含 `Relay Connection`，且断言不包含 `Active Calls`；`Discovery Mode`、`SSH Key`、`Shell Access` 仍可见；底部 `Recent Calls` 显示 `by Remote Tester`，说明 caller 信息仍由执行记录区承载。该测试未使用 `9900`，也没有修改系统代理。 |

---

### TC-RI-回归-140：未登录 Sync 服务时 Remote Status 登录提示兼容黑色主题

**背景**：未登录 Sync 服务时，`Remote Status` 卡片内的登录提示曾硬编码浅色警告背景、边框和图标颜色。在黑色主题下该提示块与页面卡片不协调，影响阅读体验。本用例验证该分支已改用主题 token，暗色主题下不再出现固定浅黄色提示块。

**前置条件**：
- 使用隔离的 WebUI 测试环境
- 测试端口由 Playwright / 测试夹具分配，禁止使用 `9900`
- Bifrost 启动参数必须包含 `--no-system-proxy`
- 预置前端主题为 dark
- mock Sync 状态返回 `has_session=false`，并返回一个可显示的 `remote_base_url`

**操作步骤**：
1. 执行：
   ```bash
   pnpm --dir web test:ui -- tests/ui/admin-settings.spec.ts -g "Settings Remote Invoke 未登录 Sync 的 Remote Status 提示兼容黑色主题"
   ```
2. 打开 WebUI `Settings -> Remote Invoke`。
3. 确认页面根节点 `data-theme="dark"`。
4. 定位 `Remote Status` 卡片中的未登录提示区域。
5. 检查提示文案、Sync Server 地址、背景色和边框色。

**预期结果**：
- 未登录提示区域可见，并显示 `Sign in to Sync to enable Remote Invoke`
- 提示区域显示 mock 的 Sync Server 地址
- 提示区域背景色不再是浅色主题硬编码值 `rgb(255, 251, 230)`
- 提示区域边框色不再是浅色主题硬编码值 `rgb(255, 229, 143)`
- 全流程不使用 `9900` 作为测试端口，不修改系统代理

### TC-RI-回归-140 执行结果（2026-04-28，Remote Status 未登录暗色主题）

| 用例编号 | 结果 | 实际结果 |
|---------|------|---------|
| TC-RI-回归-140 | ✅ PASS | 2026-04-28 在当前 checkout 执行 `pnpm --dir web exec playwright test tests/ui/admin-settings.spec.ts --grep "Settings Remote Invoke 未登录 Sync 的 Remote Status 提示兼容黑色主题"`。测试通过 Playwright 预置 `bifrost-theme=dark`，mock Sync 状态为 `has_session=false` 且 `remote_base_url=https://sync.example.test`；页面打开 `Settings -> Remote Invoke` 后确认根节点为 `data-theme="dark"`，未登录提示可见并展示 `Sign in to Sync to enable Remote Invoke` 与 Sync Server 地址。读取提示块 computed style 后确认背景不再是 `rgb(255, 251, 230)`，边框不再是 `rgb(255, 229, 143)`。该测试未使用 `9900`，也没有修改系统代理。 |

---

### TC-RI-回归-141：Grants 列表展示 SSH key / Pair code 连接方式

**背景**：Remote Invoke 的 grant 既可能来自 pair code 授权，也可能来自 `remote connect --ssh-key` 的 SSH key 授权。之前 Grants 列表只展示 caller、状态、mode、scope 等信息，用户无法直接看出某条 grant 的连接方式。本用例验证列表已基于 `auth_method` 展示 `SSH key` 或 `Pair code` 标签。

**前置条件**：
- 使用隔离的 WebUI 测试环境
- 测试端口由 Playwright / 测试夹具分配，禁止使用 `9900`
- Bifrost 启动参数必须包含 `--no-system-proxy`
- mock `/remote-invoke/grants` 返回两条 active grant：
  - `auth_method=ssh_publickey` 的 SSH key grant
  - `auth_method=pair_code` 的 pair code grant

**操作步骤**：
1. 执行：
   ```bash
   pnpm --dir web test:ui -- tests/ui/admin-settings.spec.ts -g "Settings Remote Invoke Grants 展示 SSH key 与 Pair code 连接方式"
   ```
2. 打开 WebUI `Settings -> Remote Invoke`。
3. 定位底部 `Grants` 列表。
4. 检查 SSH key grant 行。
5. 检查 pair code grant 行。

**预期结果**：
- `Grants` 列表可见
- SSH key grant 行显示 caller 名称 `ssh caller`
- SSH key grant 行显示连接方式标签 `SSH key`
- Pair code grant 行显示 caller 名称 `code caller`
- Pair code grant 行显示连接方式标签 `Pair code`
- 全流程不使用 `9900` 作为测试端口，不修改系统代理

### TC-RI-回归-141 执行结果（2026-04-28，Grants 连接方式展示）

| 用例编号 | 结果 | 实际结果 |
|---------|------|---------|
| TC-RI-回归-141 | ✅ PASS | 2026-04-28 在当前 checkout 执行 `pnpm --dir web exec playwright test tests/ui/admin-settings.spec.ts --grep "Settings Remote Invoke Grants 展示 SSH key 与 Pair code 连接方式"`。测试通过 Playwright mock `/remote-invoke/grants` 返回一条 `auth_method=ssh_publickey` grant 和一条 `auth_method=pair_code` grant；页面打开 `Settings -> Remote Invoke` 后，`Grants` 卡片可见，SSH grant 行显示 `ssh caller` 与 `SSH key` 标签，pair-code grant 行显示 `code caller` 与 `Pair code` 标签。该测试未使用 `9900`，也没有修改系统代理。 |

---

### TC-RI-回归-142：Shell E2E 不再优先运行陈旧 sync-server dist

**背景**：完整 shell E2E 在本地工作区存在陈旧 `packages/bifrost-sync-server/dist/cli.js` 时，会优先运行旧 relay 构建，导致 `remote file` open_call 缺少 `command_kind`、grant update 路由返回 404、SSH grant 字段缺失等假失败。本用例验证测试 helper 只有在 dist 新于源码时才使用 dist，否则回退到源码入口，确保 E2E 运行的是当前 checkout 的 relay 逻辑。

**前置条件**：
- 工作区可执行 `pnpm --dir packages/bifrost-sync-server run build`
- 测试端口由脚本动态分配，禁止使用 `9900`
- Bifrost 启动参数必须包含 `--no-system-proxy`
- 使用隔离 `BIFROST_DATA_DIR`

**操作步骤**：
1. 执行：
   ```bash
   pnpm --dir packages/bifrost-sync-server run build
   ```
2. 执行：
   ```bash
   SKIP_BUILD=true e2e-tests/tests/test_remote_file_relay_e2e.sh
   ```
3. 执行：
   ```bash
   SKIP_BUILD=true e2e-tests/tests/test_remote_invoke_ssh_e2e.sh
   ```
4. 执行：
   ```bash
   SKIP_BUILD=true e2e-tests/tests/test_remote_shell_exec_streaming_e2e.sh
   ```
5. 检查三条脚本输出中的 relay、target admin、caller CLI 均使用随机端口和隔离临时目录。

**预期结果**：
- `remote_file_relay_e2e` 全部断言通过，不再出现 `command_kind_required`
- `remote_invoke_ssh_e2e` 全部断言通过，relay reusable grant 返回 `ssh_key_fingerprint`
- `remote_shell_exec_streaming_e2e` 全部断言通过，grant update 不再返回 `remote invoke endpoint not found`
- 全流程不使用 `9900` 作为测试端口，不修改系统代理

### TC-RI-回归-142 执行结果（2026-04-30，sync-server dist 新鲜度）

| 用例编号 | 结果 | 实际结果 |
|---------|------|---------|
| TC-RI-回归-142 | ✅ PASS | 2026-04-30 在当前 checkout 先执行 `pnpm --dir packages/bifrost-sync-server run build` 刷新 relay 构建，再依次执行 `SKIP_BUILD=true e2e-tests/tests/test_remote_file_relay_e2e.sh`、`SKIP_BUILD=true e2e-tests/tests/test_remote_invoke_ssh_e2e.sh`、`SKIP_BUILD=true e2e-tests/tests/test_remote_shell_exec_streaming_e2e.sh`。其中 remote file relay 输出 `Total: 79 / Passed: 79 / Failed: 0`；SSH E2E 完成 `All SSH remote invoke E2E checks passed`；shell streaming 输出 `Total: 33 / Passed: 33 / Failed: 0`。三条链路均使用随机端口与隔离临时目录，未使用 `9900`，未修改系统代理。 |

---

### TC-RI-回归-143：Recent Calls 本地 echo fixture 端口 fallback 后必须使用实际端口

**背景**：CI 并发执行 shell E2E 时，`test_remote_invoke_recent_calls_args_preview_e2e.sh` 先通过 `pick_free_port` 选择 mock HTTP 端口，再启动 `http_echo_server.py --retries 5`。如果端口在选择后、bind 前被其他测试占用，mock server 会 fallback 到下一个可用端口并打印 `READY`，但旧脚本仍轮询原端口，导致出现“本地 echo fixture 未在超时内就绪”的假失败。

**前置条件**：
- 使用隔离的 relay / target admin / caller 数据目录
- target admin 使用随机端口，禁止使用 `9900`
- target admin 启动参数包含 `--no-system-proxy`
- 本地具备 `python3`、`curl`、`jq`、`cargo`、`npx`

**操作步骤**：
1. 执行以下命令，预占脚本指定的 mock 端口，强制 `http_echo_server.py --retries 5` fallback 到下一个端口：
   ```bash
   source ~/.zshrc
   TEST_PORT=$(python3 - <<'PY'
   import socket
   s = socket.socket()
   s.bind(("127.0.0.1", 0))
   print(s.getsockname()[1])
   s.close()
   PY
   )
   python3 -m http.server "$TEST_PORT" --bind 127.0.0.1 >/tmp/bifrost-ri-port-holder.log 2>&1 &
   HOLDER_PID=$!
   MOCK_HTTP_PORT="$TEST_PORT" bash e2e-tests/tests/test_remote_invoke_recent_calls_args_preview_e2e.sh
   kill "$HOLDER_PID"
   ```
2. 观察脚本输出，确认出现：
   ```text
   Local HTTP echo fixture fell back from port <requested> to <actual>
   ```
3. 继续观察 Recent Calls 参数预览、长参数截断、落盘恢复和清理断言。

**预期结果**：
- 脚本不会因为原 mock 端口被占用而停在 `/health` 探活阶段
- 脚本从 mock server 日志解析实际绑定端口，并用实际端口生成 Recent Calls 流量
- `TC-RI-GRANTS-TIME-01`、`TC-RI-GRANTS-TIME-02`、`TC-RI-ARGS-01` 到 `TC-RI-ARGS-05` 全部通过
- 全流程不使用 `9900` 作为测试端口，不修改系统代理

### TC-RI-回归-143 执行结果（2026-04-30，Recent Calls fixture 端口 fallback）

| 用例编号 | 结果 | 实际结果 |
|---------|------|---------|
| TC-RI-回归-143 | ✅ PASS | 2026-04-30 在当前 checkout 执行强制端口 fallback 回归：先用 `python3 -m http.server <TEST_PORT> --bind 127.0.0.1` 预占脚本指定的 `MOCK_HTTP_PORT`，再执行 `SKIP_BUILD=true MOCK_HTTP_PORT=<TEST_PORT> bash e2e-tests/tests/test_remote_invoke_recent_calls_args_preview_e2e.sh`。脚本输出 `Local HTTP echo fixture fell back from port 60953 to 60954`，随后完整通过 Grants 时间字段、Recent Calls 参数预览、长参数 120 字符截断、本地落盘恢复、重启恢复与清理断言，最终输出 `Recent Calls args preview and persistence E2E passed`。全流程使用随机 relay/admin/mock 端口，未使用 `9900`，target admin 由脚本以隔离数据目录和 `--no-system-proxy` 启动。 |

---

### TC-RI-回归-144：Grant `first_connected_at` 在命令执行后严格稳定

**背景**：`first_connected_at` 来自目标端 `GrantInfo.first_authorized_at`，用于表示 caller 首次连接/授权成功时间。CI 曾出现执行命令后同一 grant 的 `first_connected_at` 从 `1777557672411` 回退到 `1777557672410` 的 1ms 回归，说明命令期间的 relay `grant_created` / 重建 grant 路径覆盖了本地首次授权时间。

**前置条件**：
- 使用隔离的 relay / target admin / caller 数据目录
- target admin 使用随机端口，禁止使用 `9900`
- target admin 启动参数包含 `--no-system-proxy`
- 本地具备 `python3`、`curl`、`jq`、`cargo`、`npx`

**操作步骤**：
1. 执行：
   ```bash
   bash e2e-tests/tests/test_remote_invoke_recent_calls_args_preview_e2e.sh
   ```
2. 脚本完成 pair-code 授权后，在执行远程命令前读取 `/_bifrost/api/remote-invoke/grants`。
3. 记录同一条 grant 的 `grant_id`、`first_connected_at`、`last_command_at`。
4. 脚本执行真实 `remote traffic search` 命令。
5. 脚本再次读取 `/_bifrost/api/remote-invoke/grants`，按步骤 3 的 `grant_id` 查回同一 grant。
6. 对比执行前后的 `first_connected_at` 与 `last_command_at`。

**预期结果**：
- 执行命令前 `first_connected_at` 非空
- 执行命令前 `last_command_at` 为空
- 执行命令后 `first_connected_at` 与执行前严格相等，不允许 1ms 容差或回退
- 执行命令后 `last_command_at` 非空，且不早于 `first_connected_at`
- Recent Calls 参数预览、长参数 120 字符截断、本地落盘恢复、重启恢复与清理断言继续通过
- 全流程不使用 `9900` 作为测试端口，不修改系统代理

### TC-RI-回归-144 执行结果（2026-04-30，Grant 首次连接时间稳定）

| 用例编号 | 结果 | 实际结果 |
|---------|------|---------|
| TC-RI-回归-144 | ✅ PASS | 2026-04-30 在当前 checkout 执行 `bash e2e-tests/tests/test_remote_invoke_recent_calls_args_preview_e2e.sh`，脚本因 Rust 源码更新先重建 `target/release/bifrost`，随后使用随机 relay/admin/mock 端口与隔离数据目录启动 target admin，启动参数包含 `--no-system-proxy` 且未占用 `9900`。脚本在执行命令前读取 Grants API，确认 `grant_id` 与 `first_connected_at` 非空、`last_command_at` 为空；执行真实 remote search 后按同一 `grant_id` 再次读取 Grants API，`TC-RI-GRANTS-TIME-01` 通过，证明 `first_connected_at` 执行命令后严格保持不变，`last_command_at` 非空且不早于首次连接时间。后续 `TC-RI-GRANTS-TIME-02`、`TC-RI-ARGS-01` 至 `TC-RI-ARGS-05` 全部通过，包含 CLI grant list 时间展示、Recent Calls 参数预览、120 字符截断、落盘恢复、重启恢复与清理全部记录。 |

---

### TC-RI-回归-145：SSH key reset 后 worker 重连等待必须覆盖 CI 高并发窗口并输出诊断

**背景**：GitHub Actions `E2E Shell (aarch64-apple-darwin, shard 3/3)` 曾在 `test_remote_invoke_ssh_e2e.sh` 的 `Wait for worker reconnect after reset` 阶段失败。失败前 create/export/reset 均成功，说明问题集中在 reset 后 worker 断开 SSE、重新注册 route、恢复 `Connected` 的等待窗口。原脚本只等待 60s，且失败时缺少最后 status、relay log 与 Bifrost log，导致 CI 归因困难。

**前置条件**：
- 使用隔离的 local relay / target admin / caller 数据目录
- target admin 使用随机端口，禁止使用 `9900`
- target admin 启动参数包含 `--no-system-proxy`
- 本地具备 `python3`、`curl`、`jq`、`pnpm`、`cargo`
- 已有 `target/release/bifrost`，或允许脚本自行构建

**操作步骤**：
1. 执行 SSH key remote invoke E2E：
   ```bash
   source ~/.zshrc
   BIFROST_E2E_REMOTE_INVOKE_RESET_RECONNECT_TIMEOUT=180 \
     bash e2e-tests/tests/test_remote_invoke_ssh_e2e.sh
   ```
2. 观察脚本创建 SSH key、导出私钥、reset SSH key。
3. 在 `Wait for worker reconnect after reset` 阶段，确认脚本等待 worker 恢复 `Connected`。
4. 如果重连超时，确认脚本输出最后的 `/api/remote-invoke/status`、relay log tail 与 Bifrost log tail。
5. 重连成功后，继续观察新 `device_code` challenge、`remote conn up --ssh-key`、多 caller grant、openCall 与 revoke 验证。

**预期结果**：
- reset 后 worker 在 180s 窗口内恢复 `Connected`
- 新 `device_code` 可以拿到 relay challenge，旧 key 不再作为 active route
- 后续 SSH key connect、grant 复用、remote call、revoke 后 challenge 拒绝均通过
- 失败时必须输出可诊断日志，不再只显示单行 `Wait for worker reconnect after reset`
- 全流程使用随机端口与隔离数据目录，未使用 `9900`，未修改系统代理

### TC-RI-回归-145 执行结果（2026-05-06，SSH key reset 后 worker 重连等待）

| 用例编号 | 结果 | 实际结果 |
|---------|------|---------|
| TC-RI-回归-145 | ✅ PASS | 2026-05-06 本地按 GitHub Actions 失败 shard 复跑：`BIFROST_E2E_REPORT_DIR=$PWD/.e2e-reports/ci-shard3-repro BIFROST_E2E_SANDBOX_DIR=$PWD/.bifrost-e2e-runs/ci-shard3-repro BIFROST_E2E_SHELL_JOBS=16 BIFROST_E2E_SHARD_INDEX=3 BIFROST_E2E_SHARD_TOTAL=3 BIFROST_E2E_RETRY_FAILED_ONCE=1 BIFROST_E2E_HTTP_RETRIES=2 TIMEOUT=90 PYTHONIOENCODING=utf-8 bash scripts/ci/run-e2e-shell.sh`。同一 shard 28 个 shell suite 全部通过，其中 `shell:test_remote_invoke_ssh_e2e.sh` 通过，最终报告 `Total suites : 28 / Passed : 28 / Failed : 0 / Skipped : 0`。随后按新增 180s reset 重连窗口定向执行：`BIFROST_E2E_REPORT_DIR=$PWD/.e2e-reports/remote-invoke-ssh-reset-wait BIFROST_E2E_SANDBOX_DIR=$PWD/.bifrost-e2e-runs/remote-invoke-ssh-reset-wait BIFROST_E2E_REMOTE_INVOKE_RESET_RECONNECT_TIMEOUT=180 bash e2e-tests/tests/test_remote_invoke_ssh_e2e.sh`，验证 create/export/reset SSH key、reset 后 worker 重新 `Connected`、新 `device_code` challenge、SSH grant 复用、remote traffic/search 与 revoke 全部通过。测试使用随机端口与隔离数据目录，未使用 `9900`，未修改系统代理。 |

---

### TC-RI-回归-146：未登录 sync session 时 Remote Invoke 注册日志不刷 ERROR

**背景**：用户反馈前台启动日志中持续出现 `failed to register with relay error=Network error: remote invoke client registration requires a sync session token`。该状态发生在本地尚未登录 sync session 或临时数据目录没有 token 时，本质是等待用户登录，不应该按 relay 故障反复输出 ERROR。

**前置条件**：
- 使用空的隔离 `BIFROST_DATA_DIR`
- Bifrost 使用随机端口启动
- 启动参数包含 `--no-system-proxy`
- 本地具备 `cargo`、`curl`、`python3`

**操作步骤**：
1. 执行：
   ```bash
   source ~/.zshrc
   bash e2e-tests/tests/test_remote_invoke_missing_sync_token_log_e2e.sh
   ```
2. 脚本使用空临时数据目录启动 `target/debug/bifrost start -p <随机端口> --yes --skip-cert-check --unsafe-ssl --no-system-proxy`。
3. 等待管理端 API 就绪后继续观察日志 5 秒。
4. 检查日志中的 Remote Invoke 注册相关输出。

**预期结果**：
- 日志包含且只包含一次 `remote invoke relay registration waiting for sync session token`
- 日志不包含 `failed to register with relay`
- 真实 relay/network 失败的 ERROR 行为不被该用例静默
- 全流程使用隔离数据目录与随机端口，不使用 `9900`，不修改系统代理

### TC-RI-回归-146 执行结果（2026-05-20，未登录 sync session 日志降噪）

| 用例编号 | 结果 | 实际结果 |
|---------|------|---------|
| TC-RI-回归-146 | ✅ PASS | 2026-05-20 在当前 checkout 执行 `bash e2e-tests/tests/test_remote_invoke_missing_sync_token_log_e2e.sh`。脚本先构建最新 `target/debug/bifrost`，再使用空临时 `BIFROST_DATA_DIR` 和随机端口前台启动 `bifrost start --yes --skip-cert-check --unsafe-ssl --no-system-proxy`。管理端 API 就绪后继续观察日志 5 秒，断言只出现一次 `remote invoke relay registration waiting for sync session token`，且未出现 `failed to register with relay`。最终 3 条断言全部通过。测试使用隔离数据目录和随机端口，未使用 `9900`，未修改系统代理。 |

---

### TC-RI-回归-147：remote conn status 返回可区分设备的状态信息

**背景**：多台远端设备都保存连接时，旧版 `bifrost remote conn status` 只返回版本、系统、架构、uptime、pid 和无助于识别设备的 `rust_version`，不足以快速区分目标设备。本用例验证 Remote state 输出补充设备名称、CPU、内存和存储信息，并移除 `rust_version`。

**前置条件**：
- 本机存在至少一个可复用的 Remote Invoke saved connection，或按本文前置条件启动本地 relay、target client 与 caller 数据目录完成一次配对。
- 使用最新构建的 `target/release/bifrost` 或当前源码 `cargo run --bin bifrost -- remote conn status`。
- 如启动测试 target，必须使用隔离 `BIFROST_DATA_DIR`、随机端口，并带 `--no-system-proxy`。

**操作步骤**：
1. 执行：
   ```bash
   source ~/.zshrc
   bifrost remote conn status
   ```
   如存在多个 saved connection，根据提示选择目标设备。
2. 将输出保存为 JSON 并检查字段：
   ```bash
   bifrost remote conn status > /tmp/bifrost-remote-state.json
   jq '{version,device_name,os,arch,cpu_logical_cores,cpu_physical_cores,memory_total_bytes,memory_available_bytes,storage_total_bytes,storage_available_bytes,storage_mount_point,uptime_secs,pid,rust_version}' /tmp/bifrost-remote-state.json
   ```
3. 验证数值字段：
   ```bash
   jq -e '
     .version and .device_name and .os and .arch
     and (.cpu_logical_cores | type == "number")
     and (.cpu_logical_cores > 0)
     and (.memory_total_bytes | type == "number")
     and (.memory_total_bytes > 0)
     and (.memory_available_bytes | type == "number")
     and (.memory_available_bytes <= .memory_total_bytes)
     and (.storage_total_bytes | type == "number")
     and (.storage_total_bytes > 0)
     and (.storage_available_bytes | type == "number")
     and (.storage_available_bytes <= .storage_total_bytes)
     and (.storage_mount_point | type == "string")
     and (has("rust_version") | not)
   ' /tmp/bifrost-remote-state.json
   ```

**预期结果**：
- 输出 JSON 包含 `device_name`，值为非空字符串。
- 输出 JSON 包含 `cpu_logical_cores`，且值大于 0；如果平台可获取物理核心数，则包含 `cpu_physical_cores`。
- 输出 JSON 包含 `memory_total_bytes` 和 `memory_available_bytes`，且可用内存不超过总内存。
- 平台可获取存储卷时，输出 JSON 包含 `storage_total_bytes`、`storage_available_bytes` 和 `storage_mount_point`。
- 输出 JSON 仍包含 `version`、`os`、`arch`、`uptime_secs`、`pid`。
- 输出 JSON 不包含 `rust_version`。

### TC-RI-回归-147 执行结果（2026-05-21，Remote state 设备信息）

| 用例编号 | 结果 | 实际结果 |
|---------|------|---------|
| TC-RI-回归-147 | ✅ PASS | 2026-05-21 在独立 worktree `/Users/eden/work/github/bifrost-remote-state-device-info` 执行 `bash e2e-tests/tests/test_remote_invoke_e2e.sh`。首次执行因新 worktree 未安装 `packages/bifrost-sync-server` 依赖而在 relay 启动阶段失败，错误为 `Command "tsx" not found`；执行 `pnpm --dir packages/bifrost-sync-server install` 后原命令重跑通过。脚本使用随机 relay 端口 `51744`、target admin 端口 `51743`、隔离数据目录和 `--no-system-proxy` 启动真实 target client，通过 pair-code 建立 caller saved connection 后执行 `bifrost remote conn status --relay-url ... --client-id ...`。`TC-RI-02` 断言输出包含 `version/device_name/os/arch/cpu_logical_cores/memory_total_bytes/memory_available_bytes/storage_total_bytes/storage_available_bytes/storage_mount_point`，且不包含 `rust_version`；随后 remote traffic/search/cancel/disconnect 等回归全部通过，最终 `All assertions: total=73 passed=73 failed=0`。第 1 轮 review 后补强脚本中的 storage 字段断言，并以 `SKIP_BUILD=true bash e2e-tests/tests/test_remote_invoke_e2e.sh` 复测，同样通过 `All assertions: total=73 passed=73 failed=0`。 |

---

### TC-RI-回归-148：Recent Calls 使用 JSONL 滚动存储，最终落盘最多 1000 条且按需分页读取

**背景**：旧版 Recent Calls 使用 `admin/remote_invoke_call_history.json` 整文件读写，启动或写入时容易放大 CPU/内存消耗。新版不兼容旧整文件，发现旧文件直接删除；调用历史写入 JSONL，每次追加一行快照，超过 1000 条或发现坏行时 compaction，只保留最新 1000 条有效记录。

**前置条件**：
- 使用隔离 `BIFROST_DATA_DIR` 和随机端口。
- 启动 Bifrost 必须带 `--no-system-proxy` 与 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`。
- 具备 `curl` 和 `jq`。

**操作步骤**：
1. 在隔离数据目录写入旧格式 `admin/remote_invoke_call_history.json`。
2. 启动最新 `target/debug/bifrost start -p <随机端口> --skip-cert-check --unsafe-ssl --no-system-proxy --no-intercept`。
3. 通过 Remote Invoke 测试夹具或 store 单测生成超过 1000 条 Recent Calls JSONL 快照，并追加一行坏 JSON。
4. 请求第一页：
   ```bash
   curl -s "http://127.0.0.1:<PORT>/_bifrost/api/remote-invoke/calls?limit=25" | jq '.calls | length'
   ```
5. 使用响应中的 `next_cursor` 请求下一页：
   ```bash
   curl -s "http://127.0.0.1:<PORT>/_bifrost/api/remote-invoke/calls?limit=25&before=<NEXT_CURSOR>"
   ```
6. 检查数据目录：
   ```bash
   test ! -e "$BIFROST_DATA_DIR/admin/remote_invoke_call_history.json"
   wc -l "$BIFROST_DATA_DIR/admin/remote_invoke_call_history/"*.jsonl
   ```

**预期结果**：
- 旧 `remote_invoke_call_history.json` 被直接删除，不迁移、不兼容读取。
- JSONL 文件只包含有效 JSON 行，坏行在读取或 compaction 后被清理。
- 最终落盘有效记录最多 1000 条；超过上限时旧记录被删除。
- `/remote-invoke/calls?limit=25` 最多返回 25 条，并在还有更多记录时返回 `next_cursor`。
- 第二页不会重复第一页最后一条记录。
- Remote Invoke worker 不在内存中保存历史列表；只有正在执行的 call 持有临时快照。

### TC-RI-回归-148 执行结果（2026-06-12，Recent Calls JSONL 存储与按需读取）

| 用例编号 | 结果 | 实际结果 |
|---------|------|---------|
| TC-RI-回归-148 | ✅ PASS | 先执行 focused 单测：`cargo test -p bifrost-admin call_history_store -- --nocapture` 通过 7 个用例，覆盖旧 JSON 直接删除、JSONL 坏行 compaction、1000 条硬上限裁剪、旧配置超过 1000 仍只落盘 1000、长字段截断与 clear；`cargo test -p bifrost-admin remote_invoke_worker_reads_call_history_only_on_demand -- --nocapture` 通过，确认 worker 构造不加载历史；`cargo test -p bifrost-admin test_remote_invoke_default_max_records_is_capped_at_1000 -- --nocapture` 通过。随后用 Node 22 环境执行真实链路 `PATH=$HOME/.local/share/mise/installs/node/22.22.0/bin:$PATH bash e2e-tests/tests/test_remote_invoke_recent_calls_persistence_e2e.sh`，本地 relay/target/caller 全链路通过：重启前 Recent Calls API 包含 `status` 调用，`TC-RI-PERSIST-01A` 确认记录已写入本地 JSONL 且旧整文件不存在；保留同一数据目录重启后 Recent Calls API 仍返回同一 `call_id`，`TC-RI-PERSIST-01B` 通过。继续执行 `PATH=$HOME/.local/share/mise/installs/node/22.22.0/bin:$PATH SKIP_BUILD=true bash e2e-tests/tests/test_remote_invoke_recent_calls_args_preview_e2e.sh`，验证 Recent Calls 参数摘要、长参数 120 字符截断、JSONL 不保存完整长参数、重启恢复和 DELETE 清理全部记录均通过。全流程使用随机端口与隔离数据目录，target 启动包含 `--no-system-proxy`，未修改系统代理。 |
| TC-RI-回归-148-启动压力 | ✅ PASS | 按用户要求追加构造 1000 条已落盘历史后真实启动验证。临时 `BIFROST_DATA_DIR=/tmp/bifrost-ri-1000-startup.a2WorR` 中预置新版 `admin/remote_invoke_call_history/perf-client.jsonl` 1000 行、约 `696780` bytes，并同时放置旧版 `admin/remote_invoke_call_history.json`。执行 `BIFROST_DATA_DIR="$TEST_DATA_DIR" BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 target/release/bifrost start -p 60804 --skip-cert-check --unsafe-ssl --no-system-proxy --no-intercept`，端口监听 `312ms` 内打开；启动采样峰值 RSS `40816KB`，CPU 峰值整数部分 `6%`；启动后旧版 JSON 文件已删除，新版 JSONL 仍为 1000 行。该验证确认 1000 条历史不会在启动路径被全量加载，Remote Invoke 历史不影响端口监听。 |

---

## 清理

测试完成后清理本地临时数据：

```bash
rm -rf .bifrost-test
rm -rf <REPO_ROOT>/.bifrost-sync-test
rm -rf /tmp/bifrost-remote-overload.*
```

### 本轮实际执行回归（2026-04-21，CI 失败修复）

**执行环境**：
- Local relay：`PATH=<USER_HOME>/.local/share/mise/installs/node/22.22.0/bin:$PATH pnpm --dir packages/bifrost-sync-server exec tsx src/cli.ts -p <随机端口> -d <临时目录> --enable-remote-invoke`
- Bifrost Admin：`target/release/bifrost -H 127.0.0.1 -p <随机端口> start -y --access-mode allow_all --skip-cert-check --unsafe-ssl --no-system-proxy`
- Caller CLI：`target/release/bifrost remote ...`

**实际执行结果**：
- `TC-RI-回归-104`：✅ PASS
  重置 SSH key 后，worker 状态先进入 `Reconnecting`，随后恢复 `Connected`；新的 `device_code` 可以稳定拿到 relay challenge。
- `TC-RI-回归-105`：✅ PASS
  在 reset 后立即执行 SSH connect 链路，`remote connect --ssh-key` 与后续 grant 创建都成功，不再复现“route 已同步但 client stream 尚未恢复”导致的假性失败。
- `TC-RI-回归-113`：✅ PASS
  本地 `bash e2e-tests/tests/test_remote_invoke_e2e.sh` 重新执行后，`TC-RI-04C` 已稳定通过，Recent Calls 最终显示 `cancelled`。

**对应命令**：
- `cargo test -p bifrost-admin test_update_call_in_history_cancelled_is_terminal -- --nocapture`：✅ PASS
- `cargo test -p bifrost-admin test_update_call_in_history_does_not_override_completed_with_cancelled -- --nocapture`：✅ PASS
- `cargo test -p bifrost-admin test_find_call_started_at_returns_latest_matching_call -- --nocapture`：✅ PASS
- `bash e2e-tests/tests/test_remote_invoke_e2e.sh`：✅ PASS
- `PATH=<USER_HOME>/.local/share/mise/installs/node/22.22.0/bin:$PATH bash e2e-tests/tests/test_remote_invoke_ssh_e2e.sh`：✅ PASS

---

## TC-RI-TUI-自适应布局（status TUI Remote Invoke 标签页随窗口自适应）

> 模块：`bifrost status` 终端 TUI 的 `Remote Invoke` 标签页
> 设计文档：`design/remote-invoke-status-tui-adaptive.md`

### 前置条件

- 已编译 bifrost 二进制（`cargo build --bin bifrost`）。
- Bifrost 已 `start`，且存在若干 Remote Invoke grants 与历史调用记录（数量建议 > 15 条，
  以便观察行数自适应）。命令预览中包含较长命令（如带完整路径的 `cd ... && gh run view ...`）。
- 启动时携带 `--no-system-proxy`，避免修改系统代理。

### 测试用例

| 用例编号 | 用例名称 | 操作步骤 | 预期结果 |
|---------|---------|---------|---------|
| TC-RI-TUI-01 | 大窗口行数自适应 | 1. 将终端窗口拉到很大（如高度 ≥ 40 行）2. 运行 `bifrost status` 3. 按 →/← 切到 `Remote Invoke` 标签 | `Connected Clients` 与 `Recent Commands` 表格展示的行数明显多于 12 行（在数据足够时），下方不再出现大片空白 |
| TC-RI-TUI-02 | 宽窗口内容更完整 | 在很宽的终端（如宽度 ≥ 200 列）查看 Remote Invoke 标签 | `Latest Command` / `Command` / `Latest:` 摘要行展示的命令文本明显比窄窗口更完整，省略号显著减少；命令列充分利用了右侧此前的空白空间 |
| TC-RI-TUI-03 | 窄/矮窗口稳定可读 | 将终端缩到很窄很矮（如 60 列 × 12 行）再查看 Remote Invoke 标签 | TUI 不崩溃、不 panic；命令列仍保留可读的最小宽度（约 18 字符）并以 `...` 截断；行数随高度收缩 |
| TC-RI-TUI-04 | 标题计数语义不变 | 对比窗口大小变化前后两张表的标题 | 标题中的 `Connected Clients (N)` / `Recent Commands (N)` 中的 N 始终是全量记录总数，不随可见行数变化 |

### 清理

- 关闭 TUI（`q`），按需 `bifrost stop`。

### 实际执行结果

| 用例编号 | 结果 | 实际结果 |
|---------|------|---------|
| TC-RI-TUI-01 | ⏳ 待在 macOS 设备执行 | 沙箱无 Rust 工具链与真实终端，需在 Mac 上编译运行后核对 |
| TC-RI-TUI-02 | ⏳ 待在 macOS 设备执行 | 同上 |
| TC-RI-TUI-03 | ⏳ 待在 macOS 设备执行 | 同上 |
| TC-RI-TUI-04 | ⏳ 待在 macOS 设备执行 | 同上 |

> 说明：核心可测逻辑（行数推导 `visible_table_rows`、列宽预算 `flex_column_budget`）已抽成纯函数，
> 由 `crates/bifrost-cli/src/commands/status_tui.rs` 的单元测试覆盖；TUI 实时渲染部分需在
> 具备终端与 Rust 工具链的 macOS 设备上按上述用例真实核对。
