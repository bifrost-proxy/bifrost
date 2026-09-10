# Search Include Body / Headers + Traffic Batch Get

## 功能

`bifrost search` / `bifrost traffic search` 可以一次返回匹配记录的 body 和 headers；`bifrost traffic get --ids` 可以批量获取明细，减少逐条往返。

## CLI 与数据协议

- `--include` 接受逗号分隔、可重复的 `request-body|req-body`、`response-body|res-body`、`request-headers|req-headers`、`response-headers|res-headers`。
- `bodies` 与 `headers` 分别代表请求和响应两侧。token 大小写不敏感、去除两端空白，未知 token 在 CLI 解析阶段报错。
- `--max-body` 以字节限制每侧正文；默认 64 KiB，上限 8 MiB。未 include 正文时不会因为上限参数而额外读取正文。
- 搜索结果包含可选 `bodies.request/response` 与 `headers.request/response`；正文使用 `bytes_b64`，超限时 `truncated=true`。headers 为 `[name,value]` 二元数组。
- 不传 include 时保持原有轻量查询，不为输出额外读取正文。
- `traffic get --ids ID1,ID2` 支持完整 ID 和序号，去重，最多 200 个；与位置参数 ID 互斥。
- 批量默认 NDJSON；显式 `--format json` 或 `--format json-pretty` 返回 `{"results":[...]}`。单条缺省格式仍为 json-pretty。
- 成功批量项含 `id`、`ok:true`、`record` 和请求的附加内容；缺失项为 `ok:false,error:"not_found"`，不会中断其他项。
- 单条 get 的 body 输出与批量正文信封不同，不应假设两者 schema 完全相同。

```bash
bifrost -p "$PORT" search token --include bodies,headers --max-body 4096 --format json
bifrost -p "$PORT" traffic get --ids "$ID1,$ID2" --request-body --response-body --max-body 4096
bifrost -p "$PORT" traffic get --ids "$ID1,$ID2" --format json-pretty
```

## 实现与依赖

- search 使用 `SearchInclude`、SearchEngine hydration 和 body store；JSON/NDJSON collector 保留附加 payload。
- batch 使用 `GET /_bifrost/api/traffic/batch`，参数 `ids/include/max_body`；服务端返回 `application/x-ndjson`，当前客户端先读取响应再格式化，不承诺常量内存流式处理。
- batch 缺少 ID、超过上限或 include 不合法时返回 HTTP 400；缺失单个记录使用逐项错误。
- 本地 `traffic search` 是 search 别名，不是 remote 命令。当前 remote search/get 的 CLI 未暴露本节全部 include/batch 选项，不把它描述为完全同构。
- 不改变 SQLite schema、body 存储格式、Sync 或前端界面。

## 安全边界

正文与 headers 不自动脱敏，可能包含 Authorization、Cookie 和业务秘密；不得转发给低信任接收方。测试只生成本地合成数据，不读取正式流量。

## 验证方案

- CLI 单元覆盖 include token、参数互斥、批量格式缺省与显式选择。
- 真实 CLI 矩阵检查五种 search 输出、三种 batch 输出、默认 NDJSON、完整 ID/序号混合、缺失记录、空列表和 201 条上限。
- base64 解码后断言每侧正文实际字节数与 truncated，不以仅存在字段作为通过依据。
- 执行 `human_tests/search-include-body.md` 与 `e2e-tests/tests/test_search_traffic_cli_isomorphic_e2e.sh`。
- Rust 变更需受影响单元、fmt/clippy、workspace tests、`make coverage-changed` 及远端 coverage；E2E 先于 rust-project-validate。

## Review / Fix / Test

第一轮审查参数解析、返回信封、大小上限和错误项行为；第二轮复查修复后的 diff 与真实 CLI 输出，复跑矩阵并同步 README 和 human_tests。发现新问题则继续修复复测，直至无阻塞问题。
