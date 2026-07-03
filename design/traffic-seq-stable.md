# Traffic 序号稳定性

## 背景

Bifrost Web/CLI 的 Traffic 列表左侧的行号(sequence/seq)必须是稳定的：

- 用户按 `#12345` 引用某条流量。
- CLI 用 `bifrost traffic get 12345` 精确定位。
- IM/spec/webhook 里贴的序号在几天之后还能对回同一条 record。

早期实现里 `#seq` 来自前端数组下标：清理旧记录、翻页、并发插入、分片渲染都会让相同 record 显示成不同序号。用户在 IM 中贴出的 `#12345` 隔一段时间就指向别的 record，工程排障链路直接断掉。

本方案已经落地：`seq` 完全由后端 SQLite 主键 `traffic_records.sequence` 决定，前端不再重排。

## 用户目标验证清单

### 必须实现

- SQLite schema 里 `traffic_records.sequence INTEGER PRIMARY KEY` 保存全局递增序号。
- 进程启动时通过 `SELECT MAX(sequence) FROM traffic_records` 恢复 `current_sequence`，下一条写入使用 `current_sequence.fetch_add(1)` 分配。
- 插入 record 时 `record.sequence = seq` 由 store 层直接写库，不允许调用方指定。
- Traffic API compact record 里 `seq = record.sequence` 直接透传给前端。
- 前端 `useTrafficStore` 与 `useSearchStore` 排序/去重/比较全部按 `sequence` 字段，不使用数组下标。
- 增量 push（SSE）通过 `after_seq` / `last_sequence` 传递客户端已见的最大序号，服务端只推 `sequence > last_sequence` 的新记录。

### 必须不破坏

- 清理旧记录不改动其它 record 的 `sequence`。
- Compact record 依然含 `id`（ULID）用于详情查询；`seq` 仅用于展示与游标。
- Search / traffic list 分页游标继续以 `sequence` 为参照，`prev_cursor` / `next_cursor` 语义不变。
- 服务重启后已展示序号不重排：客户端刷新拿到的仍是同一份 `sequence`。

### 必须真实验证

- 停止服务、启动服务、连续新增 3 条 record，`sequence` 严格 `+1` 递增，无回退。
- 触发批量清理最老 N 条，剩余 record 的 `sequence` 不变。
- 前端在 SSE reconnect 后传 `last_sequence`，服务端只补推增量。
- IM 里贴出的 `#12345` 隔 24 小时后仍能 `bifrost traffic get 12345` 到同一 record。

## 产品语义

### seq 是持久化事实，不是展示规则

`sequence` 是一次性分配、永不复用、进程启动时从磁盘恢复的全局单调整数：

- 生成语义：`AtomicU64::fetch_add(1, SeqCst)`，即使并发写入也保证唯一。
- 恢复语义：`SELECT MAX(sequence)`，遇到空库返回 0，`current_sequence = current_seq + 1`。
- 展示语义：Web / CLI / IM 卡片显示的 `#N` 就是数据库里的 `sequence`。

清空、清理、压缩不会重新编号；被删除的序号会永久留空。历史 `#N` 引用要么命中同一 record，要么明确返回 not found。

### seq vs id

- `id`：ULID，写入时生成，用于详情查询、跨 record 关联。
- `sequence`：全局单调整数，用于排序、游标、人类可读引用。
- Compact record 里两者并存：`id` 拿详情，`seq` 拿展示与游标。

## 技术细节

### Schema (crates/bifrost-admin/src/traffic_db/schema.rs)

```sql
CREATE TABLE traffic_records (
    sequence INTEGER PRIMARY KEY,
    id TEXT NOT NULL,
    timestamp INTEGER NOT NULL,
    host TEXT,
    method TEXT,
    status INTEGER,
    protocol TEXT,
    ...
);
CREATE INDEX IF NOT EXISTS idx_seq_desc ON traffic_records(sequence DESC);
CREATE INDEX IF NOT EXISTS idx_host_seq ON traffic_records(host, sequence DESC);
CREATE INDEX IF NOT EXISTS idx_status_seq ON traffic_records(status, sequence DESC);
CREATE INDEX IF NOT EXISTS idx_devtools_client_req_id
    ON traffic_records(devtools_client_req_id, sequence)
    WHERE devtools_client_req_id IS NOT NULL;
```

### Store (crates/bifrost-admin/src/traffic_db/store.rs)

- 结构体字段：`current_sequence: AtomicU64`。
- `TrafficDbStore::new(...)` 中 `current_seq = Self::get_max_sequence(&write_conn).unwrap_or(0);` 然后 `AtomicU64::new(current_seq + 1)`。
- `get_max_sequence(conn)` 走 `SELECT MAX(sequence) FROM traffic_records`。
- 写入路径 `insert_record` 与 `insert_records_batch`（约 store.rs:536 / 581）每条 record `let seq = current_sequence.fetch_add(1, Ordering::SeqCst); record.sequence = seq;`。
- 对外只读接口 `current_sequence(&self) -> u64` 用于 stats、push heartbeat。

### Query & 游标 (crates/bifrost-admin/src/traffic_db/query.rs)

- `Direction::Forward => ORDER BY sequence ASC`，`Backward => ORDER BY sequence DESC`。
- `before / after` 游标转换为 `sequence < ?` / `sequence > ?` SQL 条件。
- 结果里 `server_sequence` 与 `next_cursor` / `prev_cursor` 都直接来自 `sequence`。

### Types (crates/bifrost-admin/src/traffic_db/types.rs)

```rust
pub struct CompactTrafficRecord {
    pub seq: u64,           // = record.sequence
    ...
}
pub struct TrafficStats {
    pub current_sequence: u64,
    ...
}
```

### Push (crates/bifrost-admin/src/push.rs)

- SSE 增量推送时把 `server_sequence` 放进 event payload；客户端在断线重连时通过 `?last_sequence=N` 告知服务端已见最大值，服务端只补推 `sequence > N` 的记录。

### 前端 (web/src)

- `types/index.ts`: `sequence: number` / `server_sequence` / `seq` / `after_seq` 全部保留。
- `services/pushService.ts`: 断线重连时 `params.append('last_sequence', String(this.subscription.last_sequence))`。
- `stores/useTrafficStore.ts`:
  - `sequence: c.seq` 从后端 compact record 直接透传。
  - `oldestSequence = oldestRecord?.sequence`、`lastSequence = latestRecord?.sequence`。
  - 排序比较 `if (left.sequence !== right.sequence) return left.sequence - right.sequence;`。
  - 分页 `last_sequence: state.lastSequence`；游标 `after_seq: state.lastSequence`。
  - `serverSequence: response.server_sequence` 用于 push 与 refresh 之间的 gap 检查。

## Sync 边界

- `sequence` 是本地 traffic 数据库属性，不参与跨设备 sync。
- Sync 通道传递的 rule / group / 配置无 `sequence` 语义。
- 远端 `bifrost remote traffic list` 返回目标机器上的 `sequence`；本机与远端序号不能混用，CLI 输出会带 host 前缀。

## Phase 1-4

### Phase 1: schema 与 store

- `traffic_records.sequence INTEGER PRIMARY KEY`。
- `current_sequence: AtomicU64` + `SELECT MAX(sequence)` 恢复。
- 写入路径 `fetch_add(1, SeqCst)` 分配序号。

### Phase 2: query / cursor

- `Direction` + `ORDER BY sequence`。
- `before / after` cursor 换成 `sequence </>` 条件。
- `server_sequence` 出现在所有 list/search 响应。

### Phase 3: push & 前端

- `last_sequence` / `after_seq` 参与 SSE 请求。
- 前端 store 全部用 `sequence`，删除数组下标 fallback。

### Phase 4: 文档 & 迁移

- 旧 record 迁移：`sequence` NULL 的老库通过一次性 ROWID → sequence 回填任务补齐（一次性 backfill，落地在 `design/traffic-db-startup-resequence.md`）。
- 更新 README / docs / IM 卡片模板。

## 测试方案

### 单元测试

- `crates/bifrost-admin/src/traffic_db/store.rs` 自带单元测试覆盖：
  - `sequences[0] < sequences[1]`：批量插入递增。
  - `prev_cursor` / `next_cursor` 与 `sequence` 相等。
  - `current_sequence` restart 后从 `MAX(sequence) + 1` 继续。

### E2E

- `e2e-tests/tests/test_traffic_persistence_e2e.sh` —— 重启后已展示 record 序号不变。
- `e2e-tests/tests/test_traffic_db_e2e.sh` —— 清理后剩余 record 序号稳定。
- `e2e-tests/tests/test_traffic_push_e2e.sh` —— SSE 断线重连按 `last_sequence` 补推。

### human_tests

- `human_tests/cli-traffic-search.md`：TC-CTS-SEQ-01 序号稳定引用。
- `human_tests/traffic-cleanup.md`：TC-TC-SEQ-01 清理后序号不重排。
- `human_tests/api-traffic.md`：TC-API-TRAF-SEQ-01 API 响应中 `seq` / `server_sequence` 语义。

启动 bifrost 时用临时 `BIFROST_DATA_DIR`、非 9900 端口、`BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`、`BIFROST_DISABLE_TRAY=1` 与 `--no-system-proxy`。

## Review/Fix/Test 闭环

### 第 1 轮

- 复核 schema 迁移路径：老库是否可能出现 `sequence` NULL；迁移完成后是否与内存 `current_sequence` 对齐。
- 复核并发插入路径：`fetch_add(SeqCst)` 与 batch insert 是否有中间态。
- 复测 `test_traffic_persistence_e2e.sh` + `test_traffic_push_e2e.sh`。

### 第 2 轮

- 检查前端所有排序/去重/游标点是否还残留数组下标。
- 检查 IM/webhook 卡片模板是否已使用 `#{seq}` 而不是行号。
- 复测 human_tests 中的引用回查用例。

## 风险与决策

- **序号溢出**: `AtomicU64` 上限 ~1.8e19；按当前生产峰值需要数千年才可能触及，不设置软上限。
- **迁移**: 老库缺 `sequence` 走一次性 backfill；backfill 期间 CLI list 输出提示"legacy record without stable seq"，前端隐藏 `#`。
- **跨机器混淆**: 本机与远端序号独立，CLI 输出必须带 host 前缀，UI 里远端流量显示 `remote://<host>#N`。
- **回填不可逆**: 一旦 backfill 完成，`sequence` 是唯一真源；旧行号别再回落到前端数组下标，即使 backend 短暂返回空 `seq` 也直接拒绝渲染而不是伪造。
