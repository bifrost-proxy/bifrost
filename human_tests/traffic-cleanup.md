# 流量记录清理逻辑测试

## 功能模块说明

验证 Bifrost 流量记录数据库的清理逻辑：当记录数超过 `max_records` 的 115% 时触发清理，删除最旧的记录直到剩余 `max_records` 的 80%，保持 35% 的缓冲区域避免高频清理。同时验证 size-based cleanup 和 `cleanup_total_disk_usage` 不会过度删除。

### 核心策略参数

| 参数 | 值 | 说明 |
|------|-----|------|
| 触发阈值 | max_records + min(max_records × 15%, 2000) | 溢出超过 15% 时触发，溢出上限 2000 条 |
| 目标水位 | max_records × 80% | 删除后剩余量 |
| 缓冲区域 | 35% | 80% → 115% 之间的缓冲空间 |
| Size cleanup 删除上限 | 当前记录数 × 20% | 防止 DB 文件过大时过度删除 |

## 前置条件

1. 编译并启动 Bifrost 代理（使用临时数据目录）：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test RUST_LOG=bifrost_admin=debug,info cargo run --bin bifrost -- start -p 8800 --unsafe-ssl
   ```
2. 通过 API 将 `max_records` 设置为最小值 1000：
   ```bash
   curl -s -X PUT http://localhost:8800/_bifrost/api/config/performance -H "Content-Type: application/json" -d '{"max_records": 1000}'
   ```
3. 确认配置生效：
   ```bash
   curl -s http://localhost:8800/_bifrost/api/config/performance | python3 -c "import sys,json; d=json.load(sys.stdin); print(f'max_records: {d[\"traffic\"][\"max_records\"]}')"
   ```
   预期输出：`max_records: 1000`

## 测试用例

### TC-CL-01: 记录数未达触发阈值时不清理

**操作步骤**：
1. 通过代理发送 1000 个 HTTP 请求：
   ```bash
   for i in $(seq 1 1000); do curl -s -o /dev/null -x http://127.0.0.1:8800 "http://httpbin.org/get?n=$i"; done
   ```
2. 等待 5 秒让异步写入完成
3. 查询当前记录数：
   ```bash
   curl -s "http://localhost:8800/_bifrost/api/traffic?page=1&page_size=1" | python3 -c "import sys,json; d=json.load(sys.stdin); print(f'total: {d[\"total\"]}')"
   ```

**预期结果**：记录数 = 1000（未触发清理，因为 1000 < trigger=1150）

### TC-CL-02: 超过触发阈值后自动清理到 80% 水位

**操作步骤**：
1. 继续发送请求直到超过 trigger=1150，同时监控记录数变化：
   ```bash
   for i in $(seq 1001 1500); do
     curl -s -o /dev/null -x http://127.0.0.1:8800 "http://httpbin.org/get?n=$i" &
     if [ $((i % 50)) -eq 0 ]; then
       wait
       COUNT=$(curl -s "http://localhost:8800/_bifrost/api/traffic?page=1&page_size=1" | python3 -c "import sys,json; d=json.load(sys.stdin); print(d['total'])")
       echo "sent $i, count=$COUNT"
     fi
   done
   wait
   ```
2. 等待 5 秒后查询最终记录数

**预期结果**：
- 记录数增长到 ~1200 后回落到 ~800（target = 1000 × 80%）
- 代理日志中可见 `[TRAFFIC_DB] Cleaned up old records (trigger: 15%, target: 80%)`
- 清理后新写入的记录能正常增长（从 800 继续增长）

### TC-CL-03: 清理期间新流量正常落盘

**操作步骤**：
1. 确保当前记录数约 800-1000
2. 快速并发发送 500 个请求，每 50 个检查一次记录数：
   ```bash
   for i in $(seq 1501 2000); do
     curl -s -o /dev/null -x http://127.0.0.1:8800 "http://httpbin.org/get?n=$i" &
     if [ $((i % 50)) -eq 0 ]; then
       wait
       COUNT=$(curl -s "http://localhost:8800/_bifrost/api/traffic?page=1&page_size=1" | python3 -c "import sys,json; d=json.load(sys.stdin); print(d['total'])")
       echo "checkpoint at $i: count=$COUNT"
     fi
   done
   wait
   ```

**预期结果**：
- 记录数在 800-1200 范围内波动
- 不存在记录数突然降到 800 以下的情况（不过度删除）
- 不存在记录数停止增长的情况（新流量正常入库）

### TC-CL-04: 删除的是最旧的记录

**操作步骤**：
1. 发送足够请求触发至少一次清理
2. 查询最早和最新的记录 URL：
   ```bash
   # 最早的记录
   curl -s "http://localhost:8800/_bifrost/api/traffic?page=1&page_size=1&sort=asc" | python3 -c "import sys,json; d=json.load(sys.stdin); r=d['records'][0]; print(f'oldest: seq={r.get(\"sequence\")}, url={r[\"url\"]}')"
   # 最新的记录
   curl -s "http://localhost:8800/_bifrost/api/traffic?page=1&page_size=1" | python3 -c "import sys,json; d=json.load(sys.stdin); r=d['records'][0]; print(f'newest: seq={r.get(\"sequence\")}, url={r[\"url\"]}')"
   ```

**预期结果**：最旧的记录 sequence 号 > 初始发送的记录号（说明早期记录已被清理）

### TC-CL-05: Body 缓存文件随记录删除被正确清理

**操作步骤**：
1. 发送足够请求触发多次清理
2. 检查 body_cache 文件数量与 DB 记录数的一致性：
   ```bash
   BODY_COUNT=$(ls .bifrost-test/body_cache/ 2>/dev/null | wc -l | tr -d ' ')
   DB_COUNT=$(curl -s "http://localhost:8800/_bifrost/api/traffic?page=1&page_size=1" | python3 -c "import sys,json; d=json.load(sys.stdin); print(d['total'])")
   echo "body_cache files: $BODY_COUNT, DB records: $DB_COUNT"
   ```
3. 检查 performance stats：
   ```bash
   curl -s http://localhost:8800/_bifrost/api/config/performance | python3 -c "
   import sys,json
   d=json.load(sys.stdin)
   bs = d.get('body_store_stats', {})
   print(f'body_store file_count: {bs.get(\"file_count\")}')
   "
   ```

**预期结果**：
- body_cache 文件数 ≤ DB 记录数（有些记录可能没有 body 文件）
- 不存在已删除记录对应的 orphan body 文件

### TC-CL-06: 记录数不会异常降低到几百条（回归验证）

**操作步骤**：
1. 发送 3000+ 个请求，持续监控记录数：
   ```bash
   for i in $(seq 1 3000); do
     curl -s -o /dev/null -x http://127.0.0.1:8800 "http://httpbin.org/get?n=$i" &
     if [ $((i % 100)) -eq 0 ]; then
       wait
       COUNT=$(curl -s "http://localhost:8800/_bifrost/api/traffic?page=1&page_size=1" | python3 -c "import sys,json; d=json.load(sys.stdin); print(d['total'])")
       echo "sent $i, count=$COUNT"
       if [ "$COUNT" -lt 500 ] 2>/dev/null; then
         echo "REGRESSION BUG: count dropped below 500!"
       fi
     fi
   done
   wait
   ```

**预期结果**：
- 记录数始终在 800-1200 范围内波动
- 不出现记录数降到 500 以下的情况
- 这是对旧 bug（记录数从 4000 骤降到数百条）的回归验证

### TC-CL-07: 磁盘总量清理一次性完成 body 文件清理（回归验证）

**操作步骤**：
1. 设置较小的 `max_db_size_bytes` 和 `max_body_memory_size`：
   ```bash
   curl -s -X PUT http://localhost:8800/_bifrost/api/config/performance \
     -H "Content-Type: application/json" \
     -d '{"max_db_size_bytes": 262144, "max_body_memory_size": 1, "max_records": 1000}'
   ```
2. 发送 150 个包含 32KB body 的请求：
   ```bash
   PAYLOAD=$(python3 -c "print('a' * 32768)")
   for i in $(seq 1 150); do
     curl -s -o /dev/null -x http://127.0.0.1:8800 -X POST "http://httpbin.org/post" -d "$PAYLOAD"
   done
   ```
3. 等待最多 60 秒，检查记录数和 body 文件数是否回落：
   ```bash
   for i in $(seq 1 60); do
     TOTAL=$(curl -s "http://localhost:8800/_bifrost/api/traffic?limit=1" | python3 -c "import sys,json; d=json.load(sys.stdin); print(d['total'])")
     BODY=$(curl -s http://localhost:8800/_bifrost/api/config/performance | python3 -c "import sys,json; d=json.load(sys.stdin); print(d.get('body_store_stats',{}).get('file_count',0))")
     echo "[$i] records=$TOTAL, body_files=$BODY"
     if [ "$TOTAL" -lt 150 ] && [ "$BODY" -lt 150 ]; then
       echo "PASS: both records and body files cleaned"
       break
     fi
     sleep 1
   done
   ```

**预期结果**：
- 在 60 秒内，记录数降到 150 以下
- body_cache 文件数同步降到 150 以下
- cleanup_total_disk_usage 能在一次调用中通过多轮删除将磁盘占用降到目标水位以下
- 这是对旧 bug（body 文件清理滞后于记录清理）的回归验证

### TC-CL-08: 不兼容旧 Traffic DB 启动时自动重建（回归验证）

**操作步骤**：
1. 执行旧 schema 启动回归 E2E：
   ```bash
   e2e-tests/tests/test_traffic_db_schema_reset_e2e.sh
   ```
2. 脚本会在临时 `BIFROST_DATA_DIR` 下预置一个旧版 `traffic/traffic.db`：
   - `metadata.schema_version = 10`
   - `traffic_records` 缺少当前 schema 的 `devtools_client_req_id`
   - 预置一条历史流量记录 `legacy-record`
3. 脚本用当前源码构建 `target/debug/bifrost`，通过临时端口启动：
   ```bash
   BIFROST_DATA_DIR=<tmp> target/debug/bifrost start -p <free-port> --host 127.0.0.1 --skip-cert-check --no-system-proxy
   ```
4. 脚本等待 `/_bifrost/api/proxy/address` ready 后，检查 `traffic.db`。

**预期结果**：
- `bifrost start` 不再因为 `no such column: devtools_client_req_id` panic。
- 不兼容旧 `traffic.db` 被删除并重建。
- 重建后的 `traffic_records` 包含 `devtools_client_req_id` 列。
- 重建后的索引包含 `idx_devtools_client_req_id`。
- 历史临时流量记录被清空，`SELECT COUNT(*) FROM traffic_records` 返回 0。
- `metadata.schema_version` 为当前版本 `14`。

## 清理步骤

```bash
# 停止代理进程
kill $(lsof -ti:8800) 2>/dev/null
# 删除测试数据目录
rm -rf .bifrost-test
```

## 执行记录

- 2026-07-16：通过。执行 `e2e-tests/tests/test_traffic_db_schema_reset_e2e.sh`，脚本先真实复现旧 debug binary 对旧 `traffic.db` 的 `no such column: devtools_client_req_id` panic；修复后脚本强制构建当前源码并重跑，通过临时端口启动成功，断言旧 Traffic DB 被重建、历史记录数为 0、`devtools_client_req_id` 列与 `idx_devtools_client_req_id` 索引存在、`schema_version=14`。
