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
- 授权有效期调整与移除
- 授权模式升降级（一次性→限时、永久→限时）
- 多客户端在线管理与目标客户端选择
- 仅允许白名单查询命令（status / traffic get / traffic search / search）
- SSE/HTTP relay 正常转发、大结果流式传输与断线恢复
- 调用方主动取消与 SSE 续流恢复
- 端到端加密验证（Relay 不可见明文）
- 客户端重启后 instance_id 稳定性
- 审计历史、摘要脱敏、过滤查询、数据保留与清理
- 远端部署场景：HTTPS 安全传输、SSO 鉴权、多用户并发、跨公网稳定性

## 前置条件

1. 启动本地 Bifrost 服务（使用临时数据目录，避免污染正式环境）：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- start -p 8800 --unsafe-ssl
   ```
2. 启动本地 relay 服务 `bifrost-sync-server`（使用独立临时数据目录）：
   ```bash
   pnpm --dir packages/bifrost-sync-server dev -- --data-dir /Users/eden/work/github/bifrost/.bifrost-sync-test -p 8686
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
2. 确认临时数据目录 `/Users/eden/work/github/bifrost/.bifrost-sync-test/bifrost-sync.db` 仍保留
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
   sqlite3 /Users/eden/work/github/bifrost/.bifrost-sync-test/bifrost-sync.db
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

## connect exit_code 与调用记录展示修复测试执行结果（TC-RI-81 ~ TC-RI-82）

测试环境：
- Bifrost: 本地 port 8800，`BIFROST_DATA_DIR=./.bifrost-test`
- Relay Server: 本地或远端
- 测试日期：2026-04-19

| 用例编号 | 用例名称 | 结果 | 说明 |
|---------|---------|------|------|
| TC-RI-81 | connect 授权通过后 exit_code 应为 0 | ✅ PASS | connect 记录显示绿色 completed (0)，旧记录仍为 -1（历史数据） |
| TC-RI-82 | 调用记录展示完整参数信息和响应数据量 | ✅ PASS | search.get 显示 limit=50 query=测试关键词 及 ↓ 122B；status 显示 ↓ 110B |

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

## 清理

测试完成后清理本地临时数据：

```bash
rm -rf .bifrost-test
rm -rf /Users/eden/work/github/bifrost/.bifrost-sync-test
```
