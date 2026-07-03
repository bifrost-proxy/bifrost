# 桌面端 Core 保活与资源耗尽防护

## 背景

Bifrost 桌面端以“Tauri 壳 + 内嵌 CLI sidecar”方式运行。上线后收到多起半失效反馈：

- Sidecar core 因为异常退出、被 macOS 内核信号杀死、句柄耗尽等原因悄悄挂掉；桌面窗口仍在，但代理端口无人监听，Web UI 请求 admin 全 5xx，用户以为“Bifrost 假死”。
- traffic 页里“app icon 列”触发按需图标提取，用户快速滚动时多路并发提取会同时读大量 `.app` bundle，把系统文件描述符打爆。
- 长连接场景下 SSE / WebSocket / 大响应体走 `BodyStore` / `WsPayloadStore` 的 stream writer 路径，每条流持有一个磁盘 fd；如果流不 finish，fd 持续累加，最终 `EMFILE`。

本方案在桌面壳层加运行期 watchdog 自动恢复 sidecar，在 admin crate 加 app icon 提取锁 + stream writer 上限 + 统一 `resource_alerts`，并在 Web UI Performance tab 直观呈现。目标是把“半失效”降到最低，同时让用户接近上限前就看到告警。

## 用户目标验证清单

### 必须实现

- 桌面端启动完成后自动 spawn `monitor_desktop_backend` watchdog 线程，按 `BACKEND_WATCHDOG_POLL_INTERVAL`（2s）周期巡检。
- watchdog 每轮：`poll_managed_backend_exit()` 检测 child 是否退出；`probe_backend_health(port)` 检测健康端点是否 200。
- 检测到异常时进入 `attempt_backend_recovery()`：
  - 用 `begin_backend_recovery()` 获取 `BackendRecoveryGuard`（基于 `BackendState.backend_recovery_in_progress` 原子位），避免并发。
  - 标记 `startup_ready=false`。
  - 必要时 `terminate_child()` 清理僵尸 child。
  - 复用 `ensure_backend_running()` 拉起或接管 healthy backend。
  - 成功后更新 port、清空 `startup_error`、调用 `try_start_native_handoff()`、写日志。
  - 失败后按 `BACKEND_WATCHDOG_RECOVERY_RETRY_DELAY`（3s）退避重试。
- 显式端口切换重启与 watchdog 恢复共用同一互斥标记，不并发。
- `AppIconCache` 新增 `extract_lock: Mutex<()>`，进入 `extract_app_icon()` 前先获取锁，获取后重新查内存/磁盘缓存，避免并发提取重复放大 fd。
- `BodyStore` 与 `WsPayloadStore` 各自维护 `active_writers` 计数与硬上限；打开新 stream writer 超上限时拒绝并记 warning。
- `BodyStreamWriter::finish()` 与 `Drop` 都归还槽位，禁止句柄计数失真。
- 新增 `resource_alerts` 模块，暴露 `ResourceAlertLevel { Ok, Warn, Critical }`，规则：`>=95% → Critical`、`>=80% → Warn`、其他 `Ok`。
- Admin API `/_bifrost/api/config/performance`、`/_bifrost/api/system/memory` 与 push scope `performance_config` 均返回 `resource_alerts` 字段。
- Settings → Performance tab 顶部展示汇总告警；Body Cache / WebSocket Payloads 两块用 badge/颜色高亮当前 writer 占用。

### 必须不破坏

- 正常运行时 watchdog 无副作用，不影响启动、handoff、native menu。
- app icon 提取锁串行化只作用在“真正提取”前；命中缓存路径无锁竞争。
- BodyStore/WsPayloadStore 的已有磁盘上限、retention、cleanup 逻辑不变。
- Admin API 兼容旧客户端：新字段可选，未升级前端仍可解析响应。

### 必须真实验证

- 手动 `kill -9 <sidecar pid>` 后，2~5 秒内 watchdog 恢复；`ps aux | grep bifrost` 出现新 pid；`curl 127.0.0.1:<port>/_bifrost/health` 返回 200。
- 构造 backend 健康探针失败（例如手工停响应）后，日志出现 `desktop backend watchdog triggering recovery; reason=...`。
- 压测 traffic 列表快速滚动 100+ 个不同应用，fd 消耗趋于稳定不飙升。
- 构造 200 路 SSE 长连接，`BodyStore.active_stream_writers` 到达上限，后续新流拒开、日志出现 `active writers 200/200 rejected new`。
- Performance tab 在 `active/limit >= 80%` 时出现黄色 badge，`>= 95%` 出现红色。

## 产品语义

### 运行期保活是桌面壳层的兜底职责

Watchdog 与端口切换重启共用同一 recovery guard，语义是“同一时刻只有一个恢复流程”。这保证用户手动 `Restart` 按钮 + 后台 sidecar crash 并发时不出现两次 spawn。

### 资源保护是分层的

- 磁盘：既有 `BodyStore` retention / max_memory_size 控制存储体量。
- fd：本次新增 active writer 上限，控制在线句柄数。
- 系统资源：app icon extract lock 控制“瞬时并发提取”。

### 告警语义 = “不到 80% 保持静默、80~95% 提醒、>=95% 红灯”

`resource_alerts` 有意用简单三档：门槛稳定、易解释、跨渠道复用（HTTP、Push、UI）。用户在 Performance tab 看到红灯就应该考虑降低 `body_stream_max_active_writers` 或清空缓存。

## 技术细节

### 桌面壳层 watchdog

`desktop/src-tauri/src/main.rs`:

```rust
const BACKEND_WATCHDOG_POLL_INTERVAL: Duration = Duration::from_secs(2);
const BACKEND_WATCHDOG_RECOVERY_RETRY_DELAY: Duration = Duration::from_secs(3);

fn monitor_desktop_backend(app: &AppHandle) {
    let state = app.state::<BackendState>();
    append_desktop_bootstrap_log(&state.data_dir, "desktop backend watchdog started");
    loop {
        std::thread::sleep(BACKEND_WATCHDOG_POLL_INTERVAL);
        if state.shutdown_in_progress.load(Ordering::SeqCst) { break; }

        if let Some(reason) = poll_managed_backend_exit(&state) {
            attempt_backend_recovery(app, &reason);
            continue;
        }
        let port = state.proxy_port.load(Ordering::SeqCst);
        if port != 0 && !probe_backend_health(port) {
            attempt_backend_recovery(app, &format!("health probe failed on port {port}"));
        }
    }
}
```

`attempt_backend_recovery`：

1. `let _guard = match begin_backend_recovery(&state) { Some(g) => g, None => return };`
2. `state.startup_ready.store(false, ...)`
3. `terminate_child(...)` 清理旧 child
4. `ensure_backend_running(&binary_path, &data_dir, preferred_port)` 拉起或接管
5. 成功：更新 `proxy_port`、清 `startup_error`、`try_start_native_handoff("backend watchdog recovery")`、写日志
6. 失败：`record_startup_error`、写日志、`sleep(BACKEND_WATCHDOG_RECOVERY_RETRY_DELAY)`

### App icon 提取锁

`crates/bifrost-admin/src/app_icon.rs`:

```rust
pub struct AppIconCache {
    mem_cache: Mutex<HashMap<String, Vec<u8>>>,
    disk_dir: PathBuf,
    extract_lock: Mutex<()>,   // 新增
}

impl AppIconCache {
    pub fn get_or_extract(&self, path: &str) -> Option<Vec<u8>> {
        if let Some(v) = self.check_mem(path) { return Some(v); }
        if let Some(v) = self.check_disk(path) { return Some(v); }

        let _guard = self.extract_lock.lock().unwrap();
        // double-check after lock
        if let Some(v) = self.check_mem(path) { return Some(v); }
        if let Some(v) = self.check_disk(path) { return Some(v); }
        extract_app_icon(path).map(|data| { self.store(path, &data); data })
    }
}
```

锁只在 miss 路径生效；热路径无阻塞。

### Body / WS payload writer 上限

`BodyStore` / `WsPayloadStore` 各自结构：

```rust
pub struct BodyStore {
    ...
    active_writers: Arc<AtomicUsize>,
    max_active_writers: usize,
}

impl BodyStore {
    pub fn start_stream(&self, id: &str, kind: &str) -> std::io::Result<BodyStreamWriter> {
        loop {
            let cur = self.active_writers.load(Acquire);
            if cur >= self.max_active_writers {
                warn!("BodyStore active writers {}/{} rejected new stream id={}",
                      cur, self.max_active_writers, id);
                return Err(io::Error::new(io::ErrorKind::Other, "too many active writers"));
            }
            if self.active_writers.compare_exchange(cur, cur + 1, AcqRel, Acquire).is_ok() {
                break;
            }
        }
        Ok(BodyStreamWriter { active_writers: self.active_writers.clone(), ... })
    }
}

impl BodyStreamWriter {
    pub fn finish(mut self) -> io::Result<()> { self.release_slot(); ... }
}
impl Drop for BodyStreamWriter {
    fn drop(&mut self) { self.release_slot(); }
}
```

`WsPayloadStore` 结构完全对称。

### 统一告警计算

`crates/bifrost-admin/src/resource_alerts.rs`:

```rust
pub enum ResourceAlertLevel { Ok, Warn, Critical }

pub fn resource_alert_level(current: usize, limit: usize) -> ResourceAlertLevel {
    if limit == 0 { return ResourceAlertLevel::Ok; }
    let pct = current as f64 / limit as f64;
    if pct >= 0.95 { Critical } else if pct >= 0.80 { Warn } else { Ok }
}

pub struct ResourceAlert { pub name: String, pub level: ResourceAlertLevel, pub message: String }
pub struct ResourceAlerts { pub overall_level: ResourceAlertLevel, pub items: Vec<ResourceAlert> }
```

`handlers/config.rs`、`handlers/system.rs`、`push.rs::performance_config_payload()` 都在返回结构里挂 `resource_alerts`。

### Web UI

`web/src/pages/Settings/tabs/PerformanceTab.tsx`：

- 页顶：若 `overall_level != Ok`，展示 Alert banner，颜色按 level。
- Body Cache 卡片：`activeWriters / maxActiveWriters` 显示 progress + badge。
- WebSocket Payloads 卡片：同上。
- 通过 `useConfigStore` 订阅 push scope `performance_config`，无需手动刷新。

## CLI 与 Admin API

### CLI

- `bifrost status --format json` 输出中新增 `backend_watchdog: { last_recovery, active_writers, resource_alerts }`（可选）。
- 无新增子命令。

### Admin API

- `GET /_bifrost/api/config/performance`：新增 `resource_alerts` 字段。
- `GET /_bifrost/api/system/memory`：新增 `resource_alerts` 字段。
- Push scope `performance_config`：负载附带 `resource_alerts`。
- 无新增写端点；上限值仍由 `PUT /_bifrost/api/config/performance` 更新 `max_active_writers`。

### 桌面日志

- `desktop-bootstrap.log` 增加 watchdog 生命周期与 recovery 日志。
- `desktop-sidecar.err.log` 保留 sidecar 崩溃 stderr。

## 实现切分

### Phase 1：watchdog

- 常量 + `monitor_desktop_backend` + `attempt_backend_recovery` + `begin_backend_recovery` + `BackendRecoveryGuard`。
- 与端口切换 restart 路径合并到同一 recovery 互斥。
- 日志覆盖启动、检测、恢复成功/失败。

### Phase 2：writer 上限 + app icon 锁

- `BodyStore` / `WsPayloadStore` 增 `active_writers` + `max_active_writers`。
- `BodyStreamWriter::Drop` 归还槽位。
- `AppIconCache.extract_lock`。
- 单元测试覆盖计数正确性与拒绝路径。

### Phase 3：resource_alerts + Admin

- 抽 `resource_alerts` 模块 + 单元测试。
- 三处响应挂字段。
- 单元测试覆盖 Warn/Critical 边界。

### Phase 4：Web UI + 文档

- Performance tab 汇总 banner 与卡片 badge。
- Playwright 覆盖 warn/critical 视觉。
- `docs/desktop.md` 说明“运行期自动恢复”；`README.md` 说明 diagnostics 新增 resource alerts。
- `human_tests/desktop-core-watchdog-resource-guard.md` 新增。

## 测试方案

### 单元测试

- `bifrost-admin::resource_alerts::tests`：
  - `alert_level_ok/warn/critical_thresholds`
  - `overall_level_picks_max_of_items`
- `bifrost-admin::body_store::tests`：
  - `start_stream_respects_max_active_writers`
  - `stream_writer_drop_releases_slot`
- `bifrost-admin::ws_payload_store::tests`：对称。
- `bifrost-admin::app_icon::tests`：
  - `concurrent_get_or_extract_serializes_extraction`

### E2E 测试

- `e2e-tests/tests/test_desktop_watchdog_recovery.sh`（需桌面二进制）：
  - 启动桌面端 → 记录 sidecar pid → `kill -9` → 等 6s → 检查新 pid 且 admin 端 200。
- `e2e-tests/tests/test_body_store_active_writer_limit.sh`：并发打开 N+1 流，第 N+1 失败。
- `e2e-tests/tests/test_resource_alerts_api.sh`：mock stats → 校验 `/api/system/memory.resource_alerts.overall_level`。

### 真实场景测试

`human_tests/desktop-core-watchdog-resource-guard.md`：

- TC-WD-01：桌面端启动、正常运行、无 watchdog 恢复日志。
- TC-WD-02：`kill -9` sidecar，2~5s 内自动恢复，日志有 `recovery succeeded`。
- TC-WD-03：手工阻塞健康端口，watchdog 触发 recovery。
- TC-WD-04：端口切换重启 + `kill` 并发，只有一个 recovery 生效。
- TC-RG-05：并发滚动 traffic 触发 100+ app icon 提取，fd 平稳。
- TC-RG-06：200 路 SSE，第 201 路被拒绝并写日志。
- TC-RG-07：Performance tab 在 80% / 95% 阈值分别显示黄/红。

### 覆盖率与项目校验

- `cargo test -p bifrost-admin resource_alerts body_store ws_payload_store app_icon`
- `cargo fmt --all -- --check`
- `cargo clippy --all-targets --all-features -- -D warnings`
- `cargo test --workspace --all-features`（收尾）
- 桌面 E2E 手工用例按 human_tests 记录
- 本地按 `rust-project-validate` 约定豁免 `make coverage`

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核用户目标：watchdog 自恢复、并发不重复、app icon 串行、writer 上限、alerts 三档、UI 呈现。
- 复核 diff：`main.rs`、`app_icon.rs`、`body_store.rs`、`ws_payload_store.rs`、`resource_alerts.rs`、`handlers/*.rs`、`push.rs`、`PerformanceTab.tsx`。
- 重点 review：
  - `BackendRecoveryGuard` 是否 RAII 归还？
  - `BodyStreamWriter::Drop` 与 `finish()` 是否 double release？（用一个 `bool released` 保护）
  - `probe_backend_health` timeout 是否短到不会拖满 poll 周期？
- 复测：unit + E2E watchdog + 手工 kill。

### 第 2 轮

- 检查 `git status --short`、`git diff` 无遗漏。
- 重点 review：`resource_alerts.overall_level` 是否稳定跨 API/Push 一致；`max_active_writers` 配置是否能热更新。
- 复测：并发 recovery、writer 上限边界、Performance tab 视觉。

## 风险与决策点

- **poll 周期 2s 是否会漏 fast-flap**：短周期 crash-loop 场景可能连触多次恢复。策略是靠 `begin_backend_recovery` 互斥 + 3s 退避控制。
- **`kill -9` 之后 macOS 可能保留僵尸 fd 一段时间**：`terminate_child` 应 `wait()` 后再 spawn，避免端口占用竞争。
- **watchdog 与用户 Restart 按钮竞态**：共用 recovery guard 保证互斥；用户手动优先，watchdog 只在 guard 释放后下一轮才重启。
- **writer 上限过低会误伤合法长连接**：默认值需要根据实际 fd 上限保留 30% headroom；提供 CLI/Admin 配置项。
- **resource_alerts 门槛 80/95 是否合理**：偏保守。若过多 false positive，可加入“持续 X 秒才告警”节流。
- **Web push 频率**：`performance_config` 每次变化都推可能太吵；实现要节流（例如 2s 内合并）。
