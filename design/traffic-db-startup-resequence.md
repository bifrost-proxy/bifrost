# Traffic DB 启动序列号初始化设计

## 背景

Bifrost 的 traffic 存储层（`bifrost-admin/src/traffic_db/store.rs`）用一个 SQLite 主键
`traffic_records.sequence` 作为流量记录的全局排序主键。前端列表、SSE 推送、
`server_sequence` 增量校验、cursor 分页都依赖它保持“单调递增”。

历史上曾经提出过一份“启动时全表 resequence”的方案：进程重启后把 `traffic_records`
按 timestamp 全表 `UPDATE`，让 `sequence` 变成 `1..N` 的连续序列。这份方案的动机
是让前端展示的“请求编号”看起来连续。

但它有明显问题：

- 全表 `UPDATE` 在几十万条记录场景下会阻塞 SQLite writer 数百毫秒到几秒，
  阻塞 CLI/admin 的启动路径。
- 依赖 `sequence` 稳定的其他链路（详情引用、推送重放、Devtools client req id
  索引、cursor 分页游标）会失效，因为同一条记录在两次启动之间会拿到不同的
  `sequence`。
- WAL 检查点、backup、并发读连接（`ReadPool`）都需要处理这次跨全表的写事务。

因此当前实现只做“单调延续”，不做重排。这份文档把它固化为当前行为说明，
避免任何后续讨论回退到 startup resequence 方案。

## 用户目标验证清单

### 必须实现

- 启动时读取 `MAX(sequence)` 一次，把内存 `current_sequence` 设为 `max + 1`。
- 后续 `record`/`record_batch` 通过 `AtomicU64::fetch_add(1)` 拿新 `sequence`。
- 空库启动时 `current_sequence` 初始化为 `1`（`max` 为 `None` 时按 `0 + 1`）。
- 启动完成后 `TrafficStoreStatus.current_sequence` 与 `server_sequence`
  报告的值一致，供 push 客户端和 admin overview 观察。
- `list/query_latest_window` 等接口通过 `sequence DESC` 的复合索引读取。

### 必须不破坏

- 现有 `traffic_records` schema（`SCHEMA_VERSION=12`）不改动。
- SSE push 的 `server_sequence` 单调递增语义不变，前端 gap 检测继续工作。
- 通过 `sequence` 的 cursor 分页（`sequence DESC`、`host_seq`、`status_seq`、
  `devtools_client_req_id, sequence`）不受影响。
- `INSERT OR REPLACE INTO traffic_records` 语义保持，允许更新同 `id` 记录时
  沿用旧 `sequence`。
- 已经打开的 push 订阅在启动重连后能通过 `server_sequence` 判断是否落后。

### 必须真实验证

- 冷启动一个已有 N 条记录的 DB，`current_sequence` 应为 `MAX(sequence) + 1`。
- 空 DB 冷启动后第一条新写入的记录 `sequence == 1`。
- 记录被删除后再次启动，`current_sequence` 不回退，仍从原 `MAX + 1` 继续。
- 高并发下 `record_batch` 分配的 `sequence` 无重复、无空洞跳跃（正常空洞
  只来自并发批处理内部的分配顺序，不允许分配到已存在值）。

## 产品语义

### `sequence` 是稳定、单调、允许空洞的排序主键

`sequence` 的语义是：

- 单条流量记录的全局 ordering。
- 单调递增：任何后写入的记录 `sequence` 大于所有此前写入过的记录。
- 允许空洞：清理/删除/异常回滚都可能留下不连续区间，前端和 push 层必须
  容忍空洞，`sequence` 不承担“记录条数”的语义。
- 稳定：同一条记录在整个生命周期内 `sequence` 不变，可以作为详情引用键、
  push 增量重放锚点、cursor 分页游标。

前端 “#1234” 之类的编号直接展示 `sequence`。用户能接受编号存在空洞。
不应通过 startup resequence 强行凑成 `1..N`。

### 启动不做 resequence

启动只做“把内存 `current_sequence` 追到 DB 现值之后”这一件事，任何“重排、
压紧、重置”的行为都不属于本设计范围。

### 空库启动

空库 `MAX(sequence)` 返回 `None`，代码回退到 `0`，`current_sequence` 设为
`0 + 1 = 1`。这样第一条真实写入拿到 `sequence == 1`，符合前端展示直觉。

### 清空后启动

`clear` 或 `clear_traffic_by_ids` 只删除行，不会 vacuum，也不会重置
`current_sequence`。清空后立即重启，仍从上次 max 之后继续，前端能通过
push `traffic_deleted` + `server_sequence` 组合判断“列表被清空但序号继续
往前走”。

## 技术细节

### 关键代码入口

- `crates/bifrost-admin/src/traffic_db/store.rs`
  - `TrafficDbStore::new()`：调用 `get_max_sequence(&write_conn)`，把
    `current_sequence` 初始化为 `AtomicU64::new(max + 1)`。
  - `Self::get_max_sequence(conn)`：执行 `SELECT MAX(sequence) FROM
    traffic_records`，返回 `Option<u64>`，`None` 时上层退化为 `0`。
  - `TrafficDbStore::record`：`fetch_add(1, SeqCst)` 拿新序号，赋给
    `record.sequence` 后 broadcast + persist。
  - `TrafficDbStore::record_batch`：同样按顺序 `fetch_add(1)`，保证批次内
    单调。
  - `TrafficDbStore::current_sequence()` / `TrafficStoreStatus.current_sequence`：
    对外暴露当前序号，用于 admin overview 与调试。
- `crates/bifrost-admin/src/traffic_db/schema.rs`
  - `SCHEMA_VERSION = 12`，`traffic_records.sequence INTEGER PRIMARY KEY`。
  - 关键索引：`idx_seq_desc(sequence DESC)`、`idx_host_seq(host, sequence DESC)`、
    `idx_status_seq(status, sequence DESC)`、
    `idx_devtools_client_req_id(devtools_client_req_id, sequence)`。
- `crates/bifrost-admin/src/push.rs`
  - `server_sequence` 直接取 `store.current_sequence()`，随每次 push 广播，
    前端 `useTrafficStore` 用它做 gap 检测。

### 启动流程

1. `TrafficDbStore::new(db_path, ...)` 打开 write connection + `ReadPool`。
2. 执行 `init_database` → `PRAGMA journal_mode=WAL` 等 + `check_schema_version`
   + 建表建索引。
3. 调用 `Self::get_max_sequence(&write_conn).unwrap_or(0)` 得到 `current_seq`。
4. `Self::get_record_count(&write_conn).unwrap_or(0)` 得到当前总数，用于
   overview 和 cleanup 触发阈值。
5. 构造 `TrafficDbStore { current_sequence: AtomicU64::new(current_seq + 1), .. }`。
6. `tracing::info!(current_sequence = current_seq, "SQLite traffic store initialized")`。

启动路径**不**做：

- 不执行 `UPDATE traffic_records SET sequence = ...`。
- 不做 `VACUUM`。
- 不做“把 sequence 压紧到 1..N”的迁移。
- 不改 `SCHEMA_VERSION`。

### 分配序号的并发模型

- `current_sequence: AtomicU64`。
- 单条：`let seq = self.current_sequence.fetch_add(1, Ordering::SeqCst);`
- 批量：`record_batch` 内部循环 `fetch_add(1)`，保证批内单调；`INSERT OR
  REPLACE` 落 SQLite 时按分配顺序执行。
- push broadcast 在 `fetch_add` 之后立刻发生，`server_sequence` 通过
  `current_sequence.load(Relaxed)` 读取，允许略微落后但绝不领先。

### 与 cleanup / delete / clear 的关系

- `delete_by_ids`、`clear`、`retention_hours` 触发的清理只 `DELETE FROM
  traffic_records WHERE ...`，不重置 `current_sequence`。
- 因此清理后 `MAX(sequence)` 可能出现“真实 DB 存在的最大 sequence”与
  “内存 current_sequence 减 1”之间的差异，这是允许的。
- 下次冷启动时 `get_max_sequence` 读到的是清理后剩余的最大值，
  `current_sequence` 会回落到那个值 + 1；这只发生在“清理后立刻重启”的场景，
  不会破坏运行时单调性。

## CLI + Web + Admin API

### CLI

- `bifrost traffic list`：前端 CLI 直接消费 admin 分页接口，编号即 sequence，
  允许空洞。
- `bifrost traffic get <seq_or_id>`：短数字 ID（< 6 位数字）按 sequence 定位，
  UUID 按 `id` 定位；`sequence` 的稳定性保证短 ID 引用可复现。
- 无“resequence”类命令，不新增。

### Web

- Traffic 列表列头显示 `#`，值为 `sequence`。允许非连续。
- SSE push 客户端 (`web/src/services/pushService.ts` + `useTrafficStore`) 会
  比较 `server_sequence` 与本地最大 `sequence`，出现 gap 时按 cursor 重新
  分页补齐；这套机制不假设 `sequence` 从 1 开始，也不假设连续。

### Admin API

- `GET /_bifrost/api/traffic` 分页响应中 `server_sequence`：来源
  `store.current_sequence()`。
- `GET /_bifrost/api/system/overview` 中 `TrafficStoreStatus.current_sequence`：
  同源。
- `DELETE /_bifrost/api/traffic` 与 `clear_traffic_by_ids`：只删除行，不动
  `current_sequence`。
- 没有 `POST /_bifrost/api/traffic/resequence` 类端点。

## Sync 边界

- 不参与云端 sync：`sequence` 是每台设备本地 traffic DB 的排序主键，不同
  设备的 `sequence` 空间相互独立。
- Group / rule sync 与本设计无关。
- 导出 HAR / curl / fetch 时可携带 `sequence` 供人类阅读，但不作为跨设备
  引用键，跨设备仍用 `id` (UUID)。

## Phase 1-4 实施状态

本文档描述的是**当前实现的稳定状态**，无新增 Phase 需要开发。

### Phase 1（历史，已完成）

- 引入 `current_sequence` AtomicU64 + `get_max_sequence` 启动读取。

### Phase 2（历史，已完成）

- 建立 `sequence`-based 复合索引（`idx_seq_desc`、`idx_host_seq`、
  `idx_status_seq`、`idx_devtools_client_req_id`），去掉早期 `idx_flags`。

### Phase 3（历史，已完成）

- push manager 广播 `server_sequence`，前端 gap 检测适配空洞。

### Phase 4（当前）

- 保持“单调、允许空洞、不 resequence”的语义；相关行为固化为本文档 +
  `design/traffic-seq-stable.md`。

### 明确废弃

- 启动 resequence 全表 UPDATE：**不实施**。
- 定时 vacuum + resequence：**不实施**。
- 把 `sequence` 改成 UUID/时间戳字符串：**不实施**。

## 测试方案

### 单元测试（`crates/bifrost-admin/src/traffic_db/store.rs`）

现有并需持续覆盖：

- `test_query_latest_window_returns_latest_records_in_ascending_order`：
  验证按 `sequence DESC` 抓取 + reverse 后 ascending 稳定。
- `test_query_for_search_skips_total_count`：验证 cursor + `sequence` 分页正确。
- `test_devtools_client_req_id_lookup_uses_first_non_replay_record`：验证
  `devtools_client_req_id, sequence` 复合索引的稳定性。
- `test_get_by_ids_keeps_request_content_type_and_rule_summary`：验证批量
  按 `id` 查询后 `sequence` 保持一致。
- `test_clear_removes_pending_records_when_no_active_connections`、
  `test_cleanup_drops_to_target_after_trigger`、
  `test_clear_preserves_active_connection_records`：验证清理不重置
  `current_sequence`。
- `test_schema_does_not_keep_flags_index`：验证 `SCHEMA_VERSION=12` 索引集合。

建议补充（如尚未覆盖）：

- `test_current_sequence_starts_at_one_for_empty_db`
- `test_current_sequence_resumes_after_max_on_restart`
- `test_delete_does_not_rewind_current_sequence`
- `test_record_batch_assigns_monotonic_sequences`

### E2E / 集成测试

- `crates/bifrost-e2e/src/tests/traffic_*`：既有 traffic 分页与推送用例已经
  覆盖“启动后短时间内新写入的 `sequence` 单调、`server_sequence` 广播
  一致”。
- 无需新增 “启动 resequence” 相关用例（该行为不存在）。

### 前端

- `web/src/stores/useTrafficStore.test.ts` 中 push gap 检测用例覆盖“
  `server_sequence` 领先本地最大 `sequence`”的补齐逻辑。
- Playwright `web/tests/ui/traffic.spec.ts` 覆盖 traffic 列表在冷启动、
  clear 之后仍能正常展示编号。

### human_tests

无需为此设计新增 human_test；`human_tests/webui-traffic.md` 中的 traffic
列表用例已经间接覆盖“重启后编号继续”。

## Review / Fix / Test 闭环

### 第 1 轮

- 复核用户目标：`current_sequence` 启动只做一次 `MAX(sequence)+1`，无全表
  重排；空 DB 从 1 开始；清理不 rewind。
- 复核实现：`TrafficDbStore::new()` 内代码路径是否只调用 `get_max_sequence`
  一次；有无遗漏的 `UPDATE traffic_records` 写路径。
- 重点 review：`ReadPool` 打开顺序在 `get_max_sequence` 之后是否会看到
  一致视图（WAL 模式下读连接会看到最新提交）。

### 第 2 轮

- 复核测试是否覆盖空 DB、非空 DB、清理后重启 3 种情况。
- 复核 tracing 输出 `current_sequence` 字段可用于线上排障。
- 确认 `design/traffic-seq-stable.md` 与本文档语义一致，不出现矛盾。

## 风险与决策

- **决策**：启动 resequence 方案已废弃，不重新引入。原因：全表 UPDATE 阻塞
  启动、破坏详情引用与 push 重放锚点、无产品收益。
- **风险**：清理后立刻重启，`current_sequence` 会回落到当时 `MAX + 1`。
  这是允许的，因为 traffic 只在本地 DB 内部有 ordering 意义，不做跨进程/
  跨设备的稳定引用。
- **风险**：极端场景下 `AtomicU64::fetch_add` 单调递增可能溢出。u64 空间
  在实际使用中不会耗尽（每毫秒百万条也需要 500 万年），不做处理。
- **决策**：`sequence` 保持 `INTEGER PRIMARY KEY`，不改成 `AUTOINCREMENT`。
  当前手动分配 + `AtomicU64` 更快、无 SQLite 内部 sqlite_sequence 表开销，
  也方便 batch insert。
- **决策**：不新增 CLI/Admin API 用于“resequence”或“重置 current_sequence”。
  排障需求可以通过 vacuum + 重启的手工路径达到。
