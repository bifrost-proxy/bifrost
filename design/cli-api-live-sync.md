# CLI 资源写入与 WebUI 动态同步

## 背景与问题

`bifrost script add/update/delete` 和 `bifrost value add/update/delete/import`
过去会在 CLI 进程中直接创建独立的 `ScriptEngine` / `ValuesStorage` 并写入
`BIFROST_DATA_DIR`。当 Bifrost daemon 已运行时，这条路径绕过了 daemon 内存缓存、
`ConfigChangeEvent` 和 Admin Push：

- Scripts daemon 继续持有旧的脚本缓存，WebUI 收不到 `scripts_update`。
- Values daemon 继续持有旧的 Values 缓存，WebUI 收不到 `values_update`。
- 重新启动 daemon 后缓存从磁盘重建，因此用户会观察到“只有重启桌面端才显示”。

Rules 没有暴露同样的问题，是因为 RulesStorage 另有文件系统 watcher 和兜底扫描；
该机制不应复制到 Scripts / Values，否则会增加常驻 watcher、扫描和状态竞争。

另一个独立缺口是 WebUI 切换 tab 时会在同一个 WebSocket 上把
`need_scripts` / `need_values` 从 `false` 改为 `true`，服务端此前只对首次建连、
traffic 和新增 settings scope 补发快照，没有为 Scripts / Values 的热订阅补发快照。

## 用户目标验证清单

### 必须实现

- 同一个 `BIFROST_DATA_DIR` 的 daemon 正在运行时，CLI 的 Scripts / Values 写操作
  通过 loopback Admin API 执行，由 daemon 更新磁盘、缓存并发出 push。
- `value import` 使用一次批量 API 请求完成 upsert，避免按条建立 HTTP 请求。
- `need_scripts` / `need_values` 从 `false` 切换为 `true` 时，仅向该客户端补发一次
  当前全量快照。
- daemon 未运行时保留 CLI 离线文件操作，下一次启动正常加载。

### 必须不破坏

- runtime 记录存在但 API 不可达、PID 不匹配或 PID 已复用时，CLI 必须失败关闭，
  不得静默降级成直接写文件。
- CLI 原有输出和 Add(upsert) / Update(require existing) / Delete(require existing)
  语义保持。
- 不新增轮询、文件 watcher、后台线程、定时器或无界缓存。
- Admin REST、WebUI 自身 CRUD、Rules watcher 和离线使用方式保持兼容。

### 必须真实验证

- 隔离 daemon 下运行 CLI add/update/delete/import，Admin GET 立即读到相同状态。
- 已订阅客户端收到 CLI 写入触发的 `values_update` / `scripts_update`。
- 客户端关闭订阅、发生变更、再开启订阅后立即收到最新快照。
- daemon 停止时 CLI 仍能离线写入；runtime/API 异常时不产生新文件。
- 重复写入和订阅切换后 daemon CPU/RSS 无明显持续增长。

## 方案

### 运行态判定

CLI 只信任当前数据目录内的 `runtime.json`，再使用
`/_bifrost/api/system/overview` 对 loopback listener 做第二次验证：

1. 没有 runtime 记录：判定为离线，允许直接文件操作。
2. runtime PID、端口、进程启动时间与 Admin overview 一致：判定为在线，返回
   `ConfigApiClient`。
3. runtime 记录存在但进程身份或 Admin overview 无法确认：返回错误。

这能避免 `BIFROST_DATA_DIR` 指向测试目录时误写到正式 9900 daemon，也避免在线 API
短暂失败时制造“磁盘已变、daemon 缓存未变”的 split-brain。

### Scripts / Values API 路由

- Scripts:
  - Add / Update: `PUT /api/scripts/{type}/{name}`
  - Delete: `DELETE /api/scripts/{type}/{name}`
  - Rename: `POST /api/scripts/rename/{type}/{name}`
- Values:
  - Add 和 Import: `PUT /api/values`，body 为 `{ "values": { ... } }`，批量 upsert
  - Update: `PUT /api/values/{name}`
  - Delete: `DELETE /api/values/{name}`

API 调用使用 `direct_ureq_agent()`，明确绕过系统代理。只读 CLI 命令仍可直接从磁盘
加载，因为每次 CLI 调用都会重新构建本地 reader；在线一致性由所有写入统一进入
daemon 保证。

### WebSocket 热订阅

收到新的 `ClientSubscription` 后，在覆盖旧订阅之前计算：

- `needs_initial_values = next.need_values && !previous.need_values`
- `needs_initial_scripts = next.need_scripts && !previous.need_scripts`

更新订阅后调用 PushManager 的定向发送方法。该方法复用已有
`build_values_data()` / `build_scripts_data()`，不广播给其他客户端，也不创建新任务。

## 性能与资源边界

- 空闲态：没有新增任务、timer、watcher、channel 或缓存，CPU/RSS 理论增量为零。
- 单次 CLI 写：增加一个 loopback HTTP 请求；磁盘写和 push 本来就是 WebUI/API 写入
  的既有成本。
- Values Import：一个批量请求、一次写锁、一次变更通知，避免 N 次 HTTP 和 N 次广播。
- tab 切换：仅在 `false -> true` 时构造并发送一次快照；重复发送 `true` 不触发快照。

## 测试计划

- 单元测试：
  - runtime 在线、离线、PID 不匹配、启动时间不匹配、API 不可达分类。
  - Values 批量 upsert 的成功、空名称拒绝和单次事件通知。
  - PushManager 对单一客户端定向发送 Values / Scripts 快照。
  - WebSocket 订阅 transition 只在 `false -> true` 触发。
- E2E：
  - 新增 `e2e-tests/tests/test_cli_resource_api_live_sync.sh`，使用动态端口和临时
    `BIFROST_DATA_DIR` 启动真实 daemon，覆盖 API 读回、push、重新订阅和离线回退。
- human_tests：
  - 更新 `human_tests/cli-values-scripts.md`，增加运行态同步、异常 fail-closed、
    离线兼容和资源观测回归用例。
  - 更新 `human_tests/api-push.md`，增加 Scripts / Values 热订阅快照用例。
