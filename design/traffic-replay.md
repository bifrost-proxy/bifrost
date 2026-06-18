# Traffic Export & Replay (P2-6)

## 目标
为 bifrost 提供 1) 把已捕获的 HTTP 请求导出为 curl / fetch / HAR 模板，2) 基于 RFC 6902 JSON Patch 对 body 做最小化改写后重放，并可选地从历史流量里拉最新认证 header 套到重放请求上（refresh-auth）。

## API 契约

### Export
`GET /_bifrost/api/traffic/{id}/export?format=curl|fetch|har`

- 返回 `text/plain`，body 为对应格式的字符串。
- `format` 缺省 `curl`。
- 本期不做导出脱敏；导出的 curl / fetch / HAR 使用已捕获请求的原始 header 与 body。完整脱敏方案另开需求设计与实现。
- HAR：HAR 1.2 最小子集（`log.version=1.2`、`log.creator`、`log.entries[0]` 含 `request` + 占位 `response`）；二进制 body 以 base64 编码 + `encoding=base64` 注出。
- CLI：`bifrost traffic export <id> --as curl|fetch|har [-o <path>]`。

### Replay
`POST /_bifrost/api/traffic/{id}/replay`

请求体（JSON）：
```json
{
  "patch_json": [
    {"op": "replace", "path": "/a/b", "value": "x"},
    {"op": "add",     "path": "/extras/-", "value": "new"},
    {"op": "remove",  "path": "/foo"}
  ],
  "refresh_auth": true,
  "refresh_auth_from": "latest",
  "auth_source_host": "bits.bytedance.net",
  "timeout_ms": 30000
}
```

- `patch_json`：RFC 6902 子集，仅 `replace` / `add` / `remove`。`path` 采用 RFC 6901 JSON Pointer，支持 `~0`/`~1` 转义；array 末尾追加用 `/-`。非 JSON body 不允许带 patch。
- `refresh_auth`：bool。`refresh_auth_from` 为兼容旧版字段，非空字符串等价 `refresh_auth=true`。
- `auth_source_host`：可选，限定从哪个 host 提取 auth header，缺省 = 原 record 的 host。
- `timeout_ms`：缺省 30000。
- 响应：
  ```json
  {
    "success": true,
    "data": {
      "status": 200,
      "duration_ms": 412,
      "request":  {"method": "POST", "url": "..."},
      "response": {"status": 200, "headers": [...], "body_b64": "..."},
      "auth_refresh": {"applied": true, "source_traffic_id": "...", "fields": ["Authorization", "Cookie"]},
      "headers": [...],
      "body_b64": "..."
    }
  }
  ```

CLI：
```
bifrost traffic replay <id> [--patch '/a/b=val' ...]
                            [--patch-json '[...]']
                            [--refresh-auth]
                            [--timeout 30s|1500ms]
                            [--format human|json|json-pretty]
```

#### `--patch` 糖
- `'/a/b=val'`：默认 `replace`。`val` 优先尝试 `null` / `true` / `false` / `i64` / `f64` / `{...}` / `[...]` / `"..."` JSON 字面量；都失败时当字符串。
- `'/a/b+=val'`：`+` 后缀触发 `add`。
- `'-/a/b'`：`-` 前缀触发 `remove`。

## 实现备忘

### 脱敏边界
- 本期 export/replay 不做脱敏，避免引入不完整的授权 header、Cookie、JWT、业务 token 处理方案。
- 后续完整脱敏需求需要统一覆盖 CLI、Admin API、远端调用、文档与 human_tests；在该需求落地前，调用方必须把 export 输出视为敏感数据。

### JSON Patch
- 自实现 50 行级别的子集，不引入 `json-patch` crate（依赖少）。
- 错误信息保留原始 path 便于排错。

### refresh-auth
1. 从 `traffic_db.query_latest_window(200)` 取最近 200 条 compact summary。
2. 用 `host`（小写）过滤；排除当前 replay 自身的 record。
3. 对粗筛命中逐条 `get_by_id` 拉完整 record，提取 `request_headers`。
4. 在 [`AuthCandidate`] 列表上调用 `find_refresh_auth_source`：取 `recency` 最大的命中，提取 `Authorization` / `Cookie` / `X-Tt-*` 三类 header。
5. `apply_refresh_auth` 覆盖到 replay 请求 header 上（大小写不敏感匹配）。
6. 结果通过 `auth_refresh: { applied, source_traffic_id, fields }` 回传。

### 与 P1-4（auth-status）整合 TODO
- P1-4 落地后，把 `classify_auth_header_field` 与 P1-4 的 JWT/Cookie 识别共用一份规则；refresh-auth 改为只挑选 P1-4 标记为 "valid" 的记录。

## 风险
- refresh-auth 当前实现以请求 header 为来源，不会主动调用 `auth_inspect` 验证 JWT 是否过期；只要历史里有同 host 的 Authorization/Cookie 就视为可用。
- HAR 输出仅满足最小 spec，response 字段是占位（status=0），不是真实响应。
- IPv6 URL 的 host 提取退化为 `[`；本任务不阻塞 IPv4 / 域名场景。
