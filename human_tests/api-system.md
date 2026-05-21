# System 管理 API 测试用例

## 前置条件

1. 启动 Bifrost 服务（使用临时数据目录避免污染正式环境）：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- start -p 8800 --unsafe-ssl
   ```
2. 服务启动成功后，确认管理端可访问：`http://127.0.0.1:8800/_bifrost/`

---

## 测试用例

### TC-ASY-01：获取系统基本信息 — 字段完整性验证

**操作步骤**：
1. 执行命令：
   ```bash
   curl -s http://127.0.0.1:8800/_bifrost/api/system | jq .
   ```

**预期结果**：
- HTTP 状态码为 200
- 返回 JSON 对象包含以下字段：
  - `version`（字符串，如 `"0.x.x"`，与 Cargo.toml 版本一致）
  - `device_name`（字符串，设备名称 / hostname，用于区分多台设备）
  - `os`（字符串，如 `"macos"`、`"linux"`、`"windows"`）
  - `arch`（字符串，如 `"aarch64"`、`"x86_64"`）
  - `cpu_logical_cores`（数字，逻辑 CPU 核心数，> 0）
  - `cpu_physical_cores`（可选数字，物理 CPU 核心数）
  - `memory_total_bytes`（数字，总内存字节数，> 0）
  - `memory_available_bytes`（数字，可用内存字节数，>= 0 且 <= `memory_total_bytes`）
  - `storage_total_bytes`（可选数字，当前工作目录所在存储卷总容量字节数）
  - `storage_available_bytes`（可选数字，当前工作目录所在存储卷可用容量字节数）
  - `storage_mount_point`（可选字符串，当前工作目录所在存储卷挂载点）
  - `uptime_secs`（数字，服务运行秒数，>= 0）
  - `pid`（数字，进程 ID，> 0）
  - 不包含 `rust_version`

---

### TC-ASY-02：获取系统基本信息 — version 格式合法

**操作步骤**：
1. 执行命令：
   ```bash
   curl -s http://127.0.0.1:8800/_bifrost/api/system | jq -r '.version'
   ```

**预期结果**：
- 返回的 version 字符串符合语义化版本格式（如 `0.6.0`）
- 不为空字符串

---

### TC-ASY-03：获取系统基本信息 — uptime 持续递增

**操作步骤**：
1. 执行命令记录第一次 uptime：
   ```bash
   UPTIME1=$(curl -s http://127.0.0.1:8800/_bifrost/api/system | jq '.uptime_secs')
   ```
2. 等待 3 秒：
   ```bash
   sleep 3
   ```
3. 执行命令记录第二次 uptime：
   ```bash
   UPTIME2=$(curl -s http://127.0.0.1:8800/_bifrost/api/system | jq '.uptime_secs')
   ```
4. 验证：
   ```bash
   echo "UPTIME1=$UPTIME1, UPTIME2=$UPTIME2"
   ```

**预期结果**：
- `UPTIME2` > `UPTIME1`
- 差值约为 3（允许 ±1 秒误差）

---

### TC-ASY-04：获取系统基本信息 — pid 匹配实际进程

**操作步骤**：
1. 执行命令：
   ```bash
   curl -s http://127.0.0.1:8800/_bifrost/api/system | jq '.pid'
   ```
2. 对比实际 bifrost 进程的 PID：
   ```bash
   pgrep -f "bifrost.*start.*8800"
   ```

**预期结果**：
- API 返回的 `pid` 与实际运行的 bifrost 进程 PID 一致

---

### TC-ASY-05：获取系统概览 — 字段完整性验证

**操作步骤**：
1. 执行命令：
   ```bash
   curl -s http://127.0.0.1:8800/_bifrost/api/system/overview | jq 'keys'
   ```

**预期结果**：
- HTTP 状态码为 200
- 返回 JSON 对象包含以下顶层字段：
  - `system`（对象，包含 version、os、arch、uptime_secs、pid 等）
  - `metrics`（对象，包含 MetricsSnapshot 的完整字段）
  - `rules`（对象，包含 total 和 enabled）
  - `traffic`（对象，包含 recorded）
  - `server`（对象，包含 port 和 admin_url）
  - `pending_authorizations`（数字）

---

### TC-ASY-06：获取系统概览 — system 子对象结构

**操作步骤**：
1. 执行命令：
   ```bash
   curl -s http://127.0.0.1:8800/_bifrost/api/system/overview | jq '.system'
   ```

**预期结果**：
- `.system.version` 为非空字符串
- `.system.device_name` 为非空字符串
- `.system.os` 为当前操作系统名称
- `.system.arch` 为当前 CPU 架构
- `.system.cpu_logical_cores` > 0
- `.system.memory_total_bytes` > 0
- `.system.memory_available_bytes` <= `.system.memory_total_bytes`
- `.system` 不包含 `rust_version`
- `.system.uptime_secs` >= 0
- `.system.pid` > 0

---

### TC-ASY-07：获取系统概览 — rules 子对象结构

**操作步骤**：
1. 执行命令：
   ```bash
   curl -s http://127.0.0.1:8800/_bifrost/api/system/overview | jq '.rules'
   ```

**预期结果**：
- `.rules.total` 为数字（>= 0），表示规则总数
- `.rules.enabled` 为数字（>= 0），表示已启用规则数
- `.rules.enabled` <= `.rules.total`

---

### TC-ASY-08：获取系统概览 — server 子对象结构

**操作步骤**：
1. 执行命令：
   ```bash
   curl -s http://127.0.0.1:8800/_bifrost/api/system/overview | jq '.server'
   ```

**预期结果**：
- `.server.port` 为 8800
- `.server.admin_url` 为 `"http://127.0.0.1:8800/_bifrost/"`

---

### TC-ASY-09：获取系统概览 — metrics 子对象包含实时数据

**操作步骤**：
1. 执行命令：
   ```bash
   curl -s http://127.0.0.1:8800/_bifrost/api/system/overview | jq '.metrics | {timestamp, memory_used, cpu_usage, qps}'
   ```

**预期结果**：
- `.metrics.timestamp` > 0（毫秒级时间戳）
- `.metrics.memory_used` > 0
- `.metrics.cpu_usage` >= 0
- `.metrics.qps` >= 0

---

### TC-ASY-10：获取系统概览 — traffic 子对象

**操作步骤**：
1. 执行命令：
   ```bash
   curl -s http://127.0.0.1:8800/_bifrost/api/system/overview | jq '.traffic'
   ```

**预期结果**：
- `.traffic.recorded` 为数字（>= 0），表示已记录的流量条目数

---

### TC-ASY-11：获取内存诊断信息 — 字段完整性验证

**操作步骤**：
1. 执行命令：
   ```bash
   curl -s http://127.0.0.1:8800/_bifrost/api/system/memory | jq 'keys'
   ```

**预期结果**：
- HTTP 状态码为 200
- 返回 JSON 对象包含以下顶层字段：
  - `system`（对象，SystemInfo 结构）
  - `process`（对象，进程级内存信息）
  - `traffic_db`（对象或 null，流量数据库统计）
  - `connections`（对象，连接统计）
  - `stores`（对象，存储/缓存统计）

---

### TC-ASY-12：获取内存诊断信息 — process 进程信息验证

**操作步骤**：
1. 执行命令：
   ```bash
   curl -s http://127.0.0.1:8800/_bifrost/api/system/memory | jq '.process'
   ```

**预期结果**：
- `.process.pid` > 0，且与实际 bifrost 进程 PID 一致
- `.process.rss_kib` > 0（进程 RSS 内存占用，单位 KiB）
- `.process.vms_kib` > 0（进程虚拟内存，单位 KiB）
- `.process.cpu_usage_percent` >= 0（CPU 使用率百分比）
- `.process.system_total_kib` > 0（系统总内存，单位 KiB）

---

### TC-ASY-13：获取内存诊断信息 — connections 连接统计

**操作步骤**：
1. 执行命令：
   ```bash
   curl -s http://127.0.0.1:8800/_bifrost/api/system/memory | jq '.connections'
   ```

**预期结果**：
- `.connections.tunnel_registry_active` 为数字（>= 0）
- `.connections.ws_monitor` 为对象（WebSocket 监控统计）
- `.connections.sse` 为对象，包含 `connections` 和 `open` 两个数字字段

---

### TC-ASY-14：获取内存诊断信息 — stores 存储统计与资源告警

**操作步骤**：
1. 执行命令：
   ```bash
   curl -s http://127.0.0.1:8800/_bifrost/api/system/memory | jq '.stores'
   ```

**预期结果**：
- `.stores.body_store` 存在（对象或 null）
- `.stores.frame_store` 存在（对象或 null）
- `.stores.ws_payload_store` 存在（对象或 null）
- `.stores.resource_alerts` 为数组（资源告警列表，空数组表示无告警）
- `.stores.max_body_buffer_size` 为数字（最大 body 缓冲区大小）
- `.stores.max_body_probe_size` 为数字（最大 body 探测大小）

---

### TC-ASY-15：不支持的 HTTP 方法返回 405

**操作步骤**：
1. 执行命令：
   ```bash
   curl -s -o /dev/null -w "%{http_code}" -X POST http://127.0.0.1:8800/_bifrost/api/system
   ```

**预期结果**：
- HTTP 状态码为 405（Method Not Allowed）

---

### TC-ASY-16：不存在的 system 子路径返回 404

**操作步骤**：
1. 执行命令：
   ```bash
   curl -s -o /dev/null -w "%{http_code}" http://127.0.0.1:8800/_bifrost/api/system/nonexistent
   ```

**预期结果**：
- HTTP 状态码为 404（Not Found）

---

## 清理

测试完成后清理临时数据：
```bash
rm -rf .bifrost-test
```
