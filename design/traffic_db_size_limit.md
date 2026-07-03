# Traffic DB 最大空间限制

## 背景

Bifrost 长期开着就会持续把 HTTP/HTTPS/WebSocket 流量写进 `data_dir` 下的 `traffic.db`、`body_cache/`、`frames/`、`ws_payload/` 四类存储。如果不加约束，一个高吞吐用户几天就能把开发机磁盘顶爆。

早期只有一处 SQLite 文件大小检查，不覆盖 body / frames / ws payload；文档也只说"SQLite 上限"，与真实清理路径不匹配。

当前实现把限制拆成两层：

- **DB 层**：`TrafficDbStore` 在写入过程中盯 `traffic.db` 自身大小，超限时按 25% 低水位删除最老记录并 checkpoint WAL。
- **管理端状态层**：`AdminState::cleanup_total_disk_usage` 定期把 `traffic.db + body_cache + frames + ws_payload` 作为整体磁盘占用做兜底清理，并同步孤儿数据。

上限统一由 `traffic.max_db_size_bytes` 配置（默认 2 GiB），可以通过 `PUT /api/config/performance` 热更新并持久化。

## 用户目标验证清单

### 必须实现

- 配置字段 `traffic.max_db_size_bytes` 存在，默认 2 GiB (`2 * 1024 * 1024 * 1024`)。
- `PUT /api/config/performance` 支持修改并持久化 `max_db_size_bytes`。
- `TrafficDbStore` 在写入路径检测 `traffic.db` 大小；超限时按低水位 `target_size = max - max/4` 删最老记录，并对 WAL 做 `PRAGMA wal_checkpoint(TRUNCATE)`。
- `AdminState::cleanup_total_disk_usage` 周期任务把 `traffic.db + body_cache + frames + ws_payload` 汇总，超限时按最老优先删除，先清孤儿 (body / frame / ws payload 里 orphan 引用) 再删 record。
- 清理完成后 `wal_checkpoint(TRUNCATE)` 保证磁盘释放；`VACUUM` 只在部分 compact 路径执行，不是每次热点清理都跑（避免阻塞写入）。
- CLI (`bifrost config set`) 与 Web UI Performance Tab 都能修改 `max_db_size_bytes`。

### 必须不破坏

- 未超限时不做任何删除，写入路径零开销。
- 磁盘剩余空间充足但写入抖动时不误触发大规模清理。
- 清理不影响活跃 record 引用；`request_body_bytes` 未落到 `body_cache` 的短 record 直接留在 SQLite。
- Sync / group / rule 存储不受本限制影响，`max_db_size_bytes` 只约束 traffic 相关四类目录。

### 必须真实验证

- 设置 `max_db_size_bytes = 1 MiB`（或 1024 bytes 测试用），持续写入直到触发清理；剩余总大小回落至 `max - max/4` 附近。
- Body cache 中 orphan 引用能被清理任务先删除，`traffic_records` 保留。
- 热更新配置后新阈值立即生效，不需要重启进程。
- Admin API `GET /api/config/performance` 返回 `max_db_size_bytes` 与磁盘占用统计。

## 产品语义

### 上限是"traffic 相关总占用"，不是"SQLite 文件"

用户读到的 "traffic.max_db_size_bytes" 应该被理解为：

> 所有 traffic 相关数据（SQLite 主库 + WAL + body_cache + frames + ws_payload）之和的软上限，超过时自动清理最老的 record 与其引用。

命名保留 `max_db_size_bytes` 只是为了向后兼容配置文件；语义已经从"SQLite 上限"扩展到"traffic 相关四类目录合计上限"。

### 两层清理的分工

- **DB 层 (`TrafficDbStore`)**：写入热点保护，只看 `traffic.db` 自身大小，避免单个 SQLite 文件失控。
- **AdminState 兜底 (`cleanup_total_disk_usage`)**：周期扫描四类目录，处理 orphan 与整体上限，避免 body/frame/ws_payload 单方面暴涨。

两层都以 `target_size = max - max/4`（25% 低水位）作为目标；DB 层先删掉最老的 record，AdminState 再补齐 orphan 引用。

### 触发时机

- **DB 层**：写入前后 `fs::metadata(&self.db_path).len() > max_db_size_bytes` 时立即触发。
- **AdminState**：`start_total_disk_cleanup_task` 每隔固定周期跑一次；也可由 `cleanup_total_disk_usage_if_needed` 被 admin API 主动调用（如 config 变更后）。

## 技术细节

### 配置 (crates/bifrost-storage/src/unified_config.rs)

```rust
pub struct TrafficConfig {
    pub max_db_size_bytes: u64,          // default 2 * 1024 * 1024 * 1024
    pub binary_traffic_performance_mode: bool,
    ...
}
```

- Default: `max_db_size_bytes = 2 * 1024 * 1024 * 1024` (2 GiB)。
- Partial update via `TrafficConfigPartial { max_db_size_bytes: Option<u64>, ... }`。
- `TrafficPaths { traffic_dir: data_dir/traffic, ... }` 描述四类目录根。

### 配置管理 (crates/bifrost-storage/src/config_manager.rs)

- `set_traffic(...)` 校验并写盘，触发 `AdminState` 重新 sync `TrafficDbStore::set_max_db_size_bytes` 与总清理阈值。

### Admin API (crates/bifrost-admin/src/handlers/config.rs)

```
GET  /api/config/performance
PUT  /api/config/performance
     body: { "traffic": { "max_db_size_bytes": <u64> } }
```

- 返回体含 `max_db_size_bytes` 当前值与四类目录当前占用。
- PUT 只允许 `>= 0` 的 u64；`0` 表示禁用限制（谨慎使用）。

### DB 层 (crates/bifrost-admin/src/traffic_db/store.rs)

```rust
struct TrafficDbStore {
    max_db_size_bytes: AtomicU64,
    ...
}

fn maybe_cleanup_by_size(&self, ...) {
    let max_db_size_bytes = self.max_db_size_bytes.load(Ordering::Relaxed);
    let needs_size_cleanup = max_db_size_bytes > 0 && {
        let db_size = fs::metadata(&self.db_path).map(|m| m.len()).unwrap_or(0);
        db_size > max_db_size_bytes
    };
    if !needs_size_cleanup { return; }
    let target_size = max_db_size_bytes.saturating_sub(max_db_size_bytes / 4);
    let avg_bytes_per_record = (db_size / current_count as u64).max(1);
    let bytes_to_remove = db_size.saturating_sub(target_size);
    // 按最老优先删除 records
}
```

- WAL 处理：`conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")`（约 store.rs:1431 / 1921）。
- `VACUUM` 只在 compact 路径（例如手动 vacuum admin action）里跑，不在热点循环内。
- `set_max_db_size_bytes(max)`：`AtomicU64.swap(max, SeqCst)`。
- `max_db_size_bytes()`：只读访问。

### 管理端总磁盘兜底 (crates/bifrost-admin/src/state.rs)

```rust
pub fn cleanup_total_disk_usage_if_needed(&self) { self.cleanup_total_disk_usage(); }

fn cleanup_total_disk_usage(&self) {
    let mut total_size = db_stats.db_size;
    total_size += body_sizes.values().sum::<u64>();
    total_size += frame_sizes.values().sum::<u64>();
    total_size += ws_payload_sizes.values().sum::<u64>();
    if total_size <= max_db_size_bytes { return; }
    // 1. 找 orphan body/frame/ws_payload（对应 record 已删除）先删。
    // 2. 若仍超限，按 sequence ASC 删最老 record + 其 body/frames/ws_payload 引用。
    // 3. 循环直至 total_size <= target_size (max - max/4)。
    // 4. 结束后 wal_checkpoint(TRUNCATE)。
}

pub fn start_total_disk_cleanup_task(state: SharedAdminState) -> tokio::task::JoinHandle<()> {
    loop {
        tokio::time::sleep(interval).await;
        state.cleanup_total_disk_usage_if_needed();
    }
}
```

- `ws_payload_store: Option<SharedWsPayloadStore>` 支持 lazy 初始化；未初始化时 ws payload 分支跳过。
- 孤儿检测：`orphan_ws_ids = ws_payload_sizes.keys 中不在 record.ws_payload_ids 里的部分`；body/frame 同理。

### CLI

- `bifrost config get traffic.max_db_size_bytes`
- `bifrost config set traffic.max_db_size_bytes 3221225472`（3 GiB）
- `bifrost config set traffic.max_db_size_bytes 0`（禁用限制，谨慎）
- CLI 参数校验：拒绝负值；给出 `10G/500M/2000` 等 humansize 输入的显式解析。

### Web UI (web/src/pages/Settings/tabs/PerformanceTab.tsx)

- Performance Tab 显示当前 `max_db_size_bytes` 与四类目录实际占用。
- 修改后调用 `PUT /api/config/performance` 保存。

## Sync 边界

- `max_db_size_bytes` 是本地磁盘策略，不参与跨设备 sync。
- Rules / group / config sync 通道不受本限制影响。
- 远端调用 `bifrost remote config` 能读写目标机器上的 `traffic.max_db_size_bytes`，仅影响目标机器磁盘。

## Phase 1-4

### Phase 1: 配置与 DB 层

- `TrafficConfig::max_db_size_bytes`, default 2 GiB。
- `TrafficDbStore::set_max_db_size_bytes` 热更新。
- 写入路径 `fs::metadata + fetch_add` 检测 + 低水位清理。
- `PRAGMA wal_checkpoint(TRUNCATE)`。

### Phase 2: 管理端兜底

- `AdminState::cleanup_total_disk_usage` 汇总四类目录。
- Orphan body/frame/ws_payload 先清理。
- `start_total_disk_cleanup_task` 周期任务。

### Phase 3: Admin API & CLI

- `PUT /api/config/performance` 热更新。
- `bifrost config set traffic.max_db_size_bytes` CLI 命令。
- Response 里 include 四类目录占用。

### Phase 4: Web UI & 文档

- Performance Tab 展示与编辑。
- README / docs 更新。
- Human tests 覆盖 CLI/API/UI 三条路径。

## 测试方案

### 单元测试

- `crates/bifrost-admin/src/traffic_db/store.rs`：
  - `store.max_db_size_bytes.store(1024, Ordering::SeqCst)` 后触发清理，剩余大小 <= 1024 * 3/4。
  - `set_max_db_size_bytes` 热更新可读回。
- `crates/bifrost-admin/src/state.rs`：
  - `cleanup_total_disk_usage_orphans_first`：先删 orphan body/frame/ws_payload。
  - `cleanup_total_disk_usage_records_after_orphans`：仍超限时删最老 record 与引用。

### E2E

- `e2e-tests/tests/test_performance_config_admin_api.sh` —— `PUT /api/config/performance` 热更新。
- `e2e-tests/tests/test_total_size_cleanup_admin_api.sh` —— 四类目录总占用触发兜底清理。
- `e2e-tests/tests/test_body_cache_sync_cleanup_admin_api.sh` —— body_cache 与 SQLite 同步清理。
- `e2e-tests/tests/test_traffic_db_e2e.sh` —— DB 层写入路径清理。

### human_tests

- `human_tests/cli-config.md`：TC-CFG-DB-LIMIT-01/02/03 CLI 修改、单位换算、禁用限制。
- `human_tests/api-config.md`：TC-API-CFG-DB-LIMIT-01 admin API 热更新。
- `human_tests/traffic-cleanup.md`：TC-TC-01/02/03 触发 DB 层清理、AdminState 兜底、orphan 清理。

所有启动使用临时 `BIFROST_DATA_DIR`、非 9900 端口、`BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`、`BIFROST_DISABLE_TRAY=1` 与 `--no-system-proxy`。

## Review/Fix/Test 闭环

### 第 1 轮

- 复核 DB 层清理与 AdminState 兜底是否有重复删除或竞态。
- 复核 orphan 定义：ws payload 引用集合、frame 引用集合、body cache 引用集合是否遍历完整。
- 复测 `test_total_size_cleanup_admin_api.sh`、`test_body_cache_sync_cleanup_admin_api.sh`。

### 第 2 轮

- 检查热更新路径：`PUT /api/config/performance` 后 `TrafficDbStore::set_max_db_size_bytes` 是否立即生效。
- 检查 `wal_checkpoint(TRUNCATE)` 是否释放磁盘（macOS/Linux 各测一次）。
- 复测 human_tests 中的 UI/CLI/API 三条链路。

## 风险与决策

- **命名保留**: `max_db_size_bytes` 语义已扩展为"traffic 四类目录合计上限"；不改名以避免破坏用户 config。文档必须显式解释。
- **低水位**: 采用 25% 低水位（`target = max - max/4`）而非 10%，减少清理频次；代价是磁盘占用波动更明显。
- **VACUUM**: 只在 compact 路径执行；否则每次清理触发 VACUUM 会阻塞写入。缺点是 SQLite 文件回收慢，靠 WAL checkpoint 兜底。
- **磁盘满兜底**: 本方案是软上限，如果磁盘物理满导致 WAL 无法写入，SQLite 会返回 `SQLITE_FULL`；此时依赖 `cleanup_total_disk_usage` 循环重试。真正的硬保护由 OS 层负责。
- **0 = 禁用**: 语义上允许，但生产不建议；CLI/Web UI 修改到 0 时给出 warning。
- **ws_payload_store lazy init**: 未初始化时清理任务跳过 ws payload 分支，不会误删。
