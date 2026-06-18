# Search Include Body / Headers + Traffic Batch Get

针对 `bifrost search` 与 `bifrost traffic get` 在 wave-2 P0-3 新增的「一次往返带回 body+header」与「批量按 id 拉取」能力。

## 范围
- crates/bifrost-admin/src/search/{types.rs,engine.rs,mod.rs}
- crates/bifrost-admin/src/handlers/{search.rs,traffic.rs}
- crates/bifrost-admin/src/query_service.rs
- crates/bifrost-cli/src/cli.rs
- crates/bifrost-cli/src/commands/{search.rs,traffic.rs}
- crates/bifrost-command/src/lib.rs (`TrafficGetArgs.ids/max_body`, `SearchArgs.include`)

## 测试用例

| 序号 | 场景 | 预期 |
|------|------|------|
| 1 | `bifrost search --include req-body,res-body --max-body 65536 errno` | 每条命中带 `bodies.request` / `bodies.response`；`data_b64` 解码即原文（含 JSON / 二进制）；超 64 KiB 的 body `truncated=true` 且解码长度 = 65536；exit code 0 |
| 2 | `bifrost search --include headers --format json-pretty token` | 每条命中带 `headers.request` 与 `headers.response`，每项是 `[name, value]` 二元数组；缺失 header 字段则该侧为空数组而非 null；body 字段缺省不返回 |
| 3 | `bifrost traffic get --ids 1,2,3 --max-body 32768` | 默认 ndjson 输出：3 行，每行独立 `{"id":..,"summary":..,"bodies":..,"headers":..?}`；存在 / 不存在的 id 混排时缺失 id 行变 `{"id":"X","error":"not_found"}`；exit code 0 |
| 4 | `bifrost traffic get --ids 1,2,3 --format json-pretty` | 客户端聚合成 `{"results":[...]}` 信封并 pretty-print；与单条 `traffic get <ID> --request-body --response-body` 字段对齐 |
| 5 | `bifrost traffic get --ids $(seq -s, 1 201)` 与 `bifrost traffic get --ids` 留空 | 超 200 上限：admin 返回 HTTP 400 + `[traffic.batch.too_many_ids]`，CLI 非 0 退出；空 ids：CLI clap 直接 usage error |

## 联调说明
- 旧客户端（不传 `--include`）调用新 admin：`SearchRequest.include` 走 `Default`，整段 wire JSON 中省略；服务端落到原路径不读取 body store，零增量开销。
- 新客户端调旧 admin：`SearchArgs.include` 序列化后旧 admin `#[serde(default)]` 容忍并丢弃；旧 admin 不会返回 bodies，CLI 不报错只是拿不到内容。
- `bifrost search --include` 与 `bifrost traffic search` （远端通道）共用 `command_search_args`，远端 wave-2 admin 一致响应；远端 admin 老于 wave-2 时 fallback 行为同上。
- batch endpoint 是 `application/x-ndjson`；ai-report 等工具应按行逐条消费、避免一次性反序列化整缓冲。

## 边界
- `--include` 与 `--max-body` 是搜索独立配置；`--max-body` 单独给但未启用任何 body include 时，include 块仅含 `max_body_bytes`，admin 视为无 body 输出（仅作为后续 include 启用时的默认上限）。
- `bifrost traffic get --ids` 与位置参数 `<ID>` 严格互斥，clap usage 阶段即报错。
- body 一律 base64 STANDARD 编码；CLI 输出 ndjson / json 信封时保留 `data_b64` 字段，由调用方自行解码。
- 当前实现**不脱敏** Authorization / Cookie / 业务密钥；完整脱敏方案另开需求落地前，严禁把 batch/search include 结果转发给低信任 caller。
