# IM Gateway Worker 配置热重载与任务连续性

## 问题与根因

IM Gateway controller 当前把 Provider、Schedule、Route 和 Target 的完整持久化内容合并为一个 `runtime_signature`。签名变化后，controller 无条件调用 worker supervisor 的 `restart`。因此下列本应无损的操作都会终止整个 IM Gateway worker：

- 新增、修改、暂停或恢复 Schedule；
- Schedule 执行后更新 `last_run_at` / `next_run_at` 以外的业务字段；
- 新增或修改 Route、Target；
- 修改 Provider 的展示名、owner、Agent/Runner 默认配置等非 transport 字段。

worker 进程同时承载 Provider event pipeline、Schedule loop，以及由 IM 消息触发的外部 Runner 任务。整进程重启会中断正在执行的任务，丢失过程状态，并造成用户侧重复恢复或反复重启。

## 强制不变量

1. 配置变化不是 worker 故障，不得触发 worker restart。
2. 已经开始的 IM Agent / external Runner 任务不得因配置 CRUD、Schedule 刷新或 Provider transport 重连而中断。
3. 只有 worker 已退出、心跳确认失活、启动失败，或用户显式执行 worker stop/restart 时，supervisor 才能重建 worker。
4. 热重载失败必须保留现有 worker 和现有连接，记录错误并在下一次 reconcile 重试；不得用 restart 作为配置应用失败的 fallback。
5. 当最后一个 Provider/Schedule 被删除时，已运行的 worker 保持存活，避免杀死仍在收尾的任务。后续若要回收空闲 worker，必须基于明确的 active-job=0 + idle grace，而不是仅根据配置为空。

## 配置影响分级

| 配置变化 | 应用方式 | 允许影响 |
| --- | --- | --- |
| Schedule | 通知 worker scheduler 立即重新扫描共享持久化 store | 不重启、不重连、不终止当前 run |
| Route / Target | store 在下一次事件或发送时从磁盘刷新 | 不重启、不重连 |
| Provider 展示名、owner、Agent/Runner 默认值、时间戳 | event loop 在处理下一条事件时重新读取 Provider | 不重启、不重连 |
| Provider `provider_type/base_url/app_id/secret_ref/event_connection_enabled/event_types` | 校验新配置后只替换该 Provider transport，并复用原 event pipeline | 不重启 worker，不取消 pipeline 中的活跃任务 |
| Provider 禁用或删除 | 停止该 Provider transport，保留已开始任务的 pipeline 直到自然结束 | 不重启 worker |
| Runner registry/channel 配置 | store 每次解析下一次任务时刷新磁盘快照 | 当前任务继续使用启动时快照；新任务使用新配置 |

## Reconcile 流程

1. main-process controller 继续用持久化 signature 检测配置是否变化，但 signature 只作为“需要 reload”的信号，不再作为 restart 条件。
2. worker 不存在且确有 Provider/Schedule runtime 需求时，通过 `get_or_start` 启动；worker 不健康时也由现有 supervisor 故障恢复路径重建。
3. worker 健康且 signature 变化时，controller 通过独立 control lane 发送 `im.reload_config`：
   - control lane 不等待普通任务 semaphore，因此长任务运行时仍能应用配置；
   - worker 先唤醒 scheduler；
   - 再按 Provider transport fingerprint 做局部 reconcile；
   - Route/Target/Runner 配置依赖各 store 的刷新读取生效。
4. reload 成功后推进 controller 的 applied signature；失败时保留旧 signature，使 15 秒周期或下一次 notify 自动重试。
5. worker 收到通用 `ConfigApply` 时返回 `restartRequired=false`，并复用同一热重载逻辑。

## Provider transport fingerprint

fingerprint 只包含建立或订阅 transport 必需的字段：

- `provider_type`
- 归一化后的 `base_url`
- `app_id`
- `secret_ref`
- `event_connection_enabled`
- 排序去重后的 `event_types`

`display_name`、`owner_open_id`、`agent_config`、`created_at`、`updated_at` 不参与 fingerprint。它们通过 store 热读取生效，不能引发 transport 重连。

Feishu transport 替换沿用“先校验新凭据，再停止旧连接”的既有保证；Weixin 也先验证新配置。两者都复用 provider ID 对应的 event sink，所以 transport 切换不会取消正在处理事件或外部 Runner。

## 可诊断性

每次 controller 检测到配置变化时记录：旧/新 signature（截断）、worker PID、动作 `hot_reload`。worker reload 记录 Schedule wake、Provider transport 的 `kept/started/reconnected/stopped/failed` 数量。日志不得输出 secret 内容。

## 验证

- 单元测试：Schedule/Route/Target/非 transport Provider 字段变化不会产生 restart；transport fingerprint 只对连接关键字段敏感；`ConfigApply` 返回 `restartRequired=false`。
- E2E：启动隔离 IM Gateway worker，记录 PID 与 restart count；在一个阻塞中的 Schedule external Runner 执行期间新增/修改 Schedule、Route、Target 和 Provider 非 transport 字段；断言 PID/restart count 不变，任务最终正常完成；再修改 transport 字段，断言仍是同一 worker，只有 Provider transport generation 更新。
- human test：使用临时数据目录和非正式端口执行同一场景，观察 worker diagnostics、任务结果和结构化 reload 日志，不操作正式 9900 服务或系统代理。
