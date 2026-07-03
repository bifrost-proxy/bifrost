# Remote Invoke Recent Calls 本地落盘

## 背景

Remote Invoke 的 `Recent Calls` 是 target client（被远程调用的这台机器）本地 Settings 页面展示的审计历史。它必须保留本地已解密的命令摘要（`kind`、`command`、参数预览）、执行状态、退出码、digest、开始/结束时间，以便：

- 用户在被访问的机器上审计"谁在什么时候通过哪个 grant 让我做了什么"；
- 出现 `Cancelled` / `Failed` / `Timeout` 时可以复盘完整时间线；
- Bifrost 重启后不丢失历史；
- 大规模长运行场景（每天几百上千次 remote invoke）不把 admin 进程内存打爆、不把启动流程拖慢。

历史实现里 Recent Calls 走整文件 `remote_invoke_call_history.json` 加载到内存、写入前 read-modify-write，且 worker 构造时就把全量历史读进 map。这带来三个问题：
1. 每次写入都要重新读整文件、序列化、覆盖回去，长历史下 IO 与 CPU 明显放大；
2. worker 冷启动阻塞在读整文件（几千条时几百 ms）；
3. worker 进程内长期常驻整个历史列表，对多 relay/多 client 场景内存不可控。

本方案改造为按 `<relay_url, client_instance_id>` 分片的 JSONL 滚动存储，按需读、按 append 写、内存不缓存历史列表。

## 用户目标验证清单

### 必须实现

- Recent Calls 数据落盘在 `BIFROST_DATA_DIR/admin/remote_invoke_call_history/<hash>.jsonl`。
- 每个 `<relay_url, client_instance_id>` 对应一个独立 `.jsonl` 文件（`hash` 为 `DefaultHasher` 64-bit 十六进制）。
- `RemoteInvokeWorker::new()` 不加载历史；只在 `/api/remote-invoke/calls*` HTTP handler 命中时按需读 JSONL。
- 写入路径 append-only，不做 read-modify-write。
- 单个 `<relay, client>` 分片默认最多保留 1000 条 (`remote_invoke.max_records`)，硬上限 1000（配置传更大值也 clamp）。
- 保留窗口默认 90 天 (`remote_invoke.retention_days`)。
- 单行 JSONL >2 MiB、整文件 >256 MiB 时视为坏文件，直接丢弃/重建，避免 log 被撑爆。
- 命令参数中所有长文本字符串在落盘前最多保留 120 字符（`command_summary.command_preview`、`masked_args_json` 中的字符串值都会走 truncate）。
- 遗留 `BIFROST_DATA_DIR/admin/remote_invoke_call_history.json` 整文件在启动时直接删除，不迁移。
- `Cancelled` / `Completed` / `Failed` / `Timeout` 等终态覆盖 `Streaming` 记录（通过同一 `call_id` upsert）。
- `Cancelled` 终态一旦写入，晚到 `Completed` 不能覆盖（由 worker `should_apply_call_result` 守卫，此存储只负责 append + compaction）。

### 必须不破坏

- Web UI Settings → Remote Invoke → Recent Calls 分页/详情/清空能力保持工作。
- CLI / admin API 现有分页 (`limit` / `before`) 语义不变。
- 多 client（同一 target 同时连多个 relay）互不干扰。

### 必须真实验证

- 单元测试覆盖 max_records、retention、legacy 迁移、compaction、hard cap、on-demand 读取。
- E2E：重启后 Recent Calls 未清空、长参数被 truncate。
- Human tests：`TC-RI-回归-136`。

## 产品语义

### 分片 key

存储 key 维度：`(relay_url, client_instance_id, call_id)`。同一 `(relay_url, client_instance_id)` 归入同一个 `<hash>.jsonl`；不同 relay 或不同 client 隔离，避免删除某 relay 数据时误伤其他 relay。

`hash` 生成规则：`std::collections::hash_map::DefaultHasher` 对 `<relay_url>|<client_instance_id>` 求 64-bit hash，格式化为固定长度十六进制。

### 快照粒度

每次状态推进 append 一行完整 `CallInfo` 快照：

- `call_open` 且本地解密完成后 → 写入 `streaming` 快照（`kind` / `command` / `command_summary.command_preview` / `masked_args_json` / `grant_id` / `started_at`）。
- 命令完成 / 失败 / 取消 / 超时 → 写入终态快照（补 `ended_at` / `duration_ms` / `exit_code` / `digest`）。

读取时按 `call_id` 折叠，只保留最新一条快照。这样 upsert = append，写路径不需要读整文件。

### Compaction 触发条件

发生任一情况会 compact 当前分片：

- 分片行数 > `effective_max_records`；
- 读取时发现坏 JSONL 行；
- 读取时发现单行超 2 MiB 或整文件超 256 MiB。

Compaction 策略：按 `call_id` 只保留最新快照 → 按 `started_at` 保留最新 N 条 → 按 `retention_days` 裁剪过期 → 原子 rename 覆盖。

### 长参数保护

命令 `command_preview`、`masked_args_json` 中所有字符串值在写入前经过 `truncate_command_field` 处理，最多保留前 120 字符 + 长度尾标。原始完整长文本不会写入磁盘，避免 `search --include body:<megabyte-string>` 被完整持久化。

## 技术细节

### 存储实现（`crates/bifrost-admin/src/remote_invoke/call_history_store.rs`）

关键结构：

```rust
pub struct CallHistoryStore {
    root_dir: PathBuf,                 // BIFROST_DATA_DIR/admin/remote_invoke_call_history/
    max_records: usize,                // clamp 到 [1, 1000]
    retention: Duration,               // retention_days
    max_line_bytes: usize,             // 2 * 1024 * 1024
    max_file_bytes: u64,               // 256 * 1024 * 1024
}
```

关键方法（简化签名）：

- `pub fn new(cfg: &RemoteInvokeConfig, data_dir: &Path) -> Self`：构造时删除遗留 `remote_invoke_call_history.json`（不迁移），确保 `root_dir` 存在。
- `pub fn upsert(&self, relay_url, client_id, info: &CallInfo)`：append 一行，超上限或坏行时触发 compaction。
- `pub fn list_page(&self, relay_url, client_id, limit, before) -> Vec<CallInfo>`：按 `call_id` 折叠、按 `started_at` 倒序、按 `before` 游标裁剪、按 `limit` clamp `[1, 200]`。
- `pub fn get(&self, relay_url, client_id, call_id) -> Option<CallInfo>`。
- `pub fn clear_for_client(&self, relay_url, client_id)`：仅删除当前分片文件，不动其他 relay/client。
- `pub fn finalize_non_terminal_restored_calls(&self, ...)`：进程启动时把遗留 `streaming` 记录写成 `failed` / `restart_lost`，避免 UI 长期显示 processing。

### Worker 端调用点（`crates/bifrost-admin/src/remote_invoke/worker.rs`）

- `RemoteInvokeWorker::new()`：仅 `Arc::new(CallHistoryStore::new(...))`，不 load。
- `apply_cancelled_call` / `apply_completed_call` / `apply_failed_call` / `apply_timeout_call` 内 `mark_*_call` 完成后调用 `call_history_store.upsert(...)`。
- `list_calls_page` / `get_call` / `clear_calls` 是 admin HTTP handler 的 delegate；handler 命中时才读 JSONL。
- `ActiveCallControl` 仅在内存保留当前调用的临时快照（`call_info`、`stdin_tx`、`task`），命令结束/取消后释放；历史链路完全走 `CallHistoryStore::upsert` → JSONL append。

### Admin HTTP 表面（`crates/bifrost-admin/src/handlers/remote_invoke.rs`）

- `GET /_bifrost/api/remote-invoke/calls?limit=100&before=<u64>`：list_page；`limit` 默认 100，clamp `[1, 200]`；`before` 解析为 `u64`（`started_at` ms 游标）；返回 `{calls: [...], next_cursor: <u64>?}`。
- `DELETE /_bifrost/api/remote-invoke/calls`：清空当前 relay/client 的 Recent Calls。
- `GET /_bifrost/api/remote-invoke/calls/{call_id}`：单条详情，同样走 on-demand JSONL 读取。

### 配置（`crates/bifrost-admin/src/remote_invoke/config.rs`）

- `remote_invoke.max_records`：默认 1000，`effective_max_records` clamp `[1, 1000]`；即使用户配置 5000 也只保留最新 1000 条。
- `remote_invoke.retention_days`：默认 90。
- 单行 / 整文件大小上限为编译期常量，不暴露配置。

## CLI / Web / Admin API 表面

### CLI

- `bifrost remote call list [--limit N] [--before TS]`：调 admin API。
- `bifrost remote call get <call_id>`：单条详情。
- `bifrost remote call clear`：清空当前 relay/client。

### Web UI

Settings → Remote Invoke tab → Recent Calls 列表。默认拉一页（100 条），滚动到底部加载下一页；每条支持展开查看 `masked_args_json` / `digest` / `duration_ms` / `exit_code`。

### Admin API

见上文技术细节。

## Sync 边界

Recent Calls 严格本地：

- 不上行到 Bifrost Sync；
- 不参与规则 sync；
- 不同设备的历史互相独立；
- 卸载 / 清空 data dir 即彻底清除。

## Phase 1-4 实施路径

### Phase 1：存储抽象

- 新建 `crates/bifrost-admin/src/remote_invoke/call_history_store.rs`。
- 定义 `CallHistoryStore` + JSONL 分片布局 + hash 生成。
- 构造时删除遗留 `remote_invoke_call_history.json`。

### Phase 2：Worker 接入

- 把 worker 内所有历史读写替换为 `CallHistoryStore` 调用。
- 移除内存中 `calls: HashMap<call_id, CallInfo>` 常驻缓存。
- `ActiveCallControl` 仅保留内存临时快照。

### Phase 3：Admin API + Web UI

- HTTP handler 走 on-demand 读取。
- Web UI 补分页、`before` 游标。

### Phase 4：文档 + E2E + Human tests

- `human_tests/remote-invoke.md` 新增 `TC-RI-回归-136`。
- `e2e-tests/tests/test_remote_invoke_recent_calls_persistence_e2e.sh` 覆盖重启恢复。
- `e2e-tests/tests/test_remote_invoke_recent_calls_args_preview_e2e.sh` 覆盖长参数 truncate。

## 测试方案

### 单元测试（`crates/bifrost-admin/src/remote_invoke/call_history_store.rs` 内 `mod tests`）

- `test_call_history_store_prunes_by_retention_and_max_records`（call_history_store.rs:819）：验证 `max_records` + `retention_days` 联合裁剪。
- `test_call_history_store_clear_for_client_removes_only_current_client`（call_history_store.rs:849）：删单一分片不影响其他 relay/client。
- `test_call_history_store_truncates_command_fields_before_persisting`（call_history_store.rs:883）：`command_preview` / `masked_args_json` 中所有字符串值 ≤120 字符，且长文本不落盘。
- `test_call_history_store_removes_legacy_json_without_migrating`（call_history_store.rs:957）：遗留整 JSON 文件在启动时删除，不迁移不读取。
- `test_call_history_store_compacts_bad_jsonl_lines_and_caps_records`（call_history_store.rs:987）：坏 JSONL 行触发 compaction，最终只保留上限内有效记录。
- `test_call_history_store_hard_caps_configured_max_records_at_1000`（call_history_store.rs:1021）：即使配置 5000，落盘最多 1000。
- `test_finalize_non_terminal_restored_calls_marks_streaming_failed`（call_history_store.rs:1046）：重启后遗留 `streaming` 记录被 finalize 为 failed，避免 UI 假 processing。

### Worker 单元测试（`crates/bifrost-admin/src/remote_invoke/worker.rs`）

- `remote_invoke_worker_reads_call_history_only_on_demand`：构造 worker 不读历史，list API 触发时才读。

### E2E 测试

- `e2e-tests/tests/test_remote_invoke_recent_calls_persistence_e2e.sh`：
  - 临时 `BIFROST_DATA_DIR`、动态 admin 端口、本地 relay；
  - `remote connect` → `remote status`；
  - `curl /_bifrost/api/remote-invoke/calls` 确认 call 存在；
  - 停 Bifrost，用同一 data dir 重启；
  - 再次读 Recent Calls，断言同 `call_id`、`command_preview=status` 仍在。
- `e2e-tests/tests/test_remote_invoke_recent_calls_args_preview_e2e.sh`：
  - 生成长参数 `remote search`；
  - 断言 `masked_args_json.keyword` 最多 120 字符；
  - 确认 legacy `remote_invoke_call_history.json` 不存在，`.jsonl` 不含完整长参数；
  - 重启 Bifrost 后同 `call_id` 长字段仍最多 120 字符；
  - `/_bifrost/api/remote-invoke/calls?limit=25` 最多返回 25 条，超时返回 `next_cursor`。

### Human tests

- `TC-RI-回归-136`（`human_tests/remote-invoke.md:4488`）：真实执行——生成调用、重启 Bifrost、刷新 Settings Remote Invoke，Recent Calls 未清空。执行记录见 `TC-RI-回归-136 执行结果（2026-04-24）`。

## Review / Fix / Test 闭环

### 第 1 轮

- 复核用户目标：worker 构造是否不 load、写路径是否 append-only、legacy JSON 是否被删除、长参数是否被 truncate。
- 复核 diff：`worker.rs` 是否还有旧的 `calls` 内存 map 引用；handler 是否复用 `CallHistoryStore::list_page`。
- 重点 review：
  - 多 client 场景下 hash 冲突概率（DefaultHasher 64-bit 足够）；
  - 单行 2 MiB / 整文件 256 MiB 触发条件是否会误伤合法长参数（正常参数远低于阈值）；
  - Compaction 是否是原子 rename（避免中途崩溃导致数据丢失）。
- 复测：所有单元测试 + 两条 E2E + TC-RI-回归-136。

### 第 2 轮

- 复核第 1 轮问题的修复。
- 检查 `git status --short` / `git diff`，确保 human_tests 索引同步更新。
- 重点 review：
  - `retention_days` 与 `max_records` 联合裁剪的顺序（先按时间再按上限）；
  - `finalize_non_terminal_restored_calls` 只在启动后执行一次，不循环执行。
- 复测：Human tests `TC-RI-回归-136` 复跑，观察 Web UI 表现。

## 风险与决策

- **JSONL vs SQLite**：JSONL 简单、易于人工查看、天然 append；SQLite 支持索引但需要额外依赖。在本地审计规模（单 client 最多 1000 条）下 JSONL 完全够用，决定采用 JSONL。
- **DefaultHasher 冲突**：64-bit hash 冲突概率对 client_instance_id 规模（几十上百）可忽略；即使冲突，两个 relay/client 会共享一个文件，Recent Calls 仍能通过 `relay_url` 字段过滤。
- **retention_days 与 max_records 顺序**：先按时间裁剪再按上限裁剪。这样"最近 100 条但都是 91 天前"会被裁到 0；对审计场景更符合用户直觉。
- **Legacy JSON 迁移**：曾经考虑读旧文件迁移到 JSONL，但（a）旧文件可能被 read-modify-write 撕裂；（b）迁移工作量大；（c）Recent Calls 是审计辅助而非关键数据。因此选择直接删除。
- **是否支持"合并多 relay 的历史查询"**：目前不支持，用户需要切换 relay/client 后再查。未来若需要，可以在 handler 层聚合读多个分片。

## 实现现状校验（2026-06-16）

- `CallHistoryStore` 位于 `crates/bifrost-admin/src/remote_invoke/call_history_store.rs`（1065 行），构造时删除遗留 `remote_invoke_call_history.json`，工作目录 `BIFROST_DATA_DIR/admin/remote_invoke_call_history/<hash>.jsonl`。
- Worker 端调用点在 `crates/bifrost-admin/src/remote_invoke/worker.rs`（8437 行）：`RemoteInvokeWorker::new` 仅 `Arc::new(CallHistoryStore::new(...))`，不读历史；`list_calls_page` / `get_call` / `clear_calls` 在 HTTP handler 命中时才读 JSONL。
- HTTP 入口在 `crates/bifrost-admin/src/handlers/remote_invoke.rs`：`/api/remote-invoke/calls`（GET 列表、DELETE 清空）、`/api/remote-invoke/calls/{call_id}`（GET 详情）；`limit` 默认 100、clamp `[1, 200]`；`before` 解析为 `u64`。
- `ActiveCallControl` 仅在内存保存当前调用快照（`call_info`、`stdin_tx`、`task`），命令结束/取消后释放；历史链路完全走 `CallHistoryStore::upsert` → JSONL append。
- 上述实现条目均与文档一致，无 planned 项。
