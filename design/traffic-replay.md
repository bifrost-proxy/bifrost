# Traffic Export & Replay

## 背景

Bifrost 需要一条最短路径，让用户把一条已捕获的 HTTP 流量 record 拿出来:

- 复制成一条可以贴到终端/浏览器/DevTools 的 curl / fetch / HAR。
- 在本地对 body 做最小化修改（RFC 6902 JSON Patch），重新发到目标 server，观察响应。
- 可选地从最近的历史流量里挑一条同 host 的最新 Authorization / Cookie / X-Tt-* 覆盖进去，避免 token 过期后手工复制 header。

本方案覆盖 export 与 replay + refresh-auth 三个能力，落地在 admin API、CLI 与共享 `bifrost-admin::replay` 模块中，Web UI 复用相同后端。

## 用户目标验证清单

### 必须实现

- `GET /_bifrost/api/traffic/{id}/export?format=curl|fetch|har` 返回 `text/plain` 的对应格式字符串；`format` 缺省 `curl`。
- HAR 输出符合 HAR 1.2 最小子集：`log.version=1.2`、`log.creator`、`log.entries[0]` 含 `request` 与占位 `response`；二进制 body 以 base64 + `encoding=base64` 输出。
- `POST /_bifrost/api/traffic/{id}/replay` 支持仅 `replace` / `add` / `remove` 三种 op 的 JSON Patch，`path` 使用 RFC 6901，支持 `~0` / `~1` 转义与 `/-` 追加。
- `refresh_auth=true` 时从最近历史流量里挑同 host 最新的 `Authorization` / `Cookie` / `X-Tt-*` header 覆盖到 replay 请求上，并回传 `auth_refresh.applied` / `source_traffic_id` / `fields`。
- CLI: `bifrost traffic export <id> --as curl|fetch|har [-o path]`、`bifrost traffic replay <id> [--patch]... [--patch-json ...] [--refresh-auth] [--timeout ...] [--format human|json|json-pretty]`。
- `--patch` 语法糖: `/a/b=val`（默认 replace）、`/a/b+=val`（add）、`-/a/b`（remove）；`val` 优先按 JSON 字面量解析，失败回落为字符串。
- Replay 响应体过大时 base64 编码到 `body_b64`，附带 `headers` / `status` / `duration_ms` / `request` echo。

### 必须不破坏

- 不改变 traffic record 的存储结构：`id` / `request_headers` / `request_body_bytes` / `host` / `method` / `url` 沿用现有字段。
- Traffic list / detail / search 已有 API 不受影响，export 与 replay 只读取现有 record。
- 现有 refresh-auth 老字段 `refresh_auth_from`（string）继续被识别：非空字符串等价 `refresh_auth=true`。
- 非 JSON body 的 replay 请求在提供 `patch_json` 时必须以清晰错误拒绝，不静默丢弃 patch。

### 必须真实验证

- 三种 export 格式对同一条 record 产出可解析文本；HAR 能被 `serde_json::from_str::<Value>` 解析并含 `log.entries[0].request.method/url`。
- Replay 应用 patch 后 upstream 收到的 body 与预期一致；`Content-Length` 由客户端根据新 body 重新计算，不带旧 record 的旧长度。
- Refresh-auth 命中时，覆盖后的 header 内容与源 record header 对齐，`source_traffic_id` 精确指向那条 record；未命中时 `applied=false` 且不改动原 header。
- CLI 与 Admin API 输出字段等价，可跨终端诊断同一条 replay。

## 产品语义

### Export 定位

Export 面向"我要把这条请求原样在别处再发一次"的场景（curl 贴终端、fetch 贴 DevTools、HAR 贴测试报告）。本期不做脱敏，导出即使含 Authorization、Cookie、Set-Cookie、JWT 也照原样写入；所有调用方必须把 export 输出视为敏感数据。完整脱敏方案见 `design/proxy-log-redaction.md` 单独设计，落地前不引入不完整的字段级过滤。

### Replay 定位

Replay 面向"改一个字段再发一次"的调试场景：

- 只支持 `replace` / `add` / `remove` 三种 op，避免 RFC 6902 完整实现带来的边界坑。
- Patch 目标一定是 JSON body；`Content-Type` 或 body 无法解析为 JSON 时明确报错，不做自动 form-urlencoded / multipart 转换。
- Replay 使用 bifrost 自身的 reqwest 客户端出网，不经过 bifrost 主端口，避免形成"replay 打到自己" 的死循环。
- Replay 不会写回 traffic_records，也不生成新的 sequence；调用方要留痕请自行记录。

### Refresh-auth 定位

Refresh-auth 是可选加成：

- 只在 `refresh_auth_enabled()` 为 true 时生效。
- 只挑 `classify_auth_header_field` 命中的三类 header：`Authorization` / `Cookie` / `X-Tt-*`（大小写不敏感）。
- 从 `query_latest_window(200)` 里取最新 200 条粗筛，按 host 精确匹配，再逐条 `get_by_id` 读完整 record，按 recency 挑最新一条。
- 覆盖到 replay 请求 header 时使用大小写不敏感匹配；不存在的字段直接 append。
- 与 P1-4 (auth-status) 的 JWT/Cookie 有效性判断目前仍是分开的，refresh-auth 只按"最近出现过"选择，不主动验证是否过期。P1-4 落地后 `classify_auth_header_field` 与 auth-status 规则合并，改为只挑 valid record。

## 技术细节

### 模块划分

- `crates/bifrost-admin/src/replay.rs`
  - `pub enum ExportFormat { Curl, Fetch, Har }` + `ExportFormat::parse(&str)`。
  - `pub struct ExportOptions { format }`、`pub fn export_request(method, url, headers, body_bytes, opts) -> String`。
  - `fn export_curl` / `fn export_fetch` / `fn export_har`：内部拼装 shell / JS / JSON。
  - `pub enum JsonPatchOp { Replace, Add, Remove }`、`apply_json_patch(&mut Value, &[JsonPatchOp]) -> Result<(), String>`；自实现 ~50 行 subset，避免拉入 `json-patch` crate。
  - `pub struct ReplayOptions { patch_json, refresh_auth_from, refresh_auth, auth_source_host, timeout_ms }` 与 `refresh_auth_enabled()` 兼容开关。
  - `pub trait HttpClient` + `pub struct ReqwestClient`，方便测试注入 mock。
  - `pub fn classify_auth_header_field(name) -> Option<&'static str>`。
  - `pub fn find_refresh_auth_source<'a>(candidates, host) -> Option<(id, headers)>`。
  - `pub fn apply_refresh_auth(target: &mut Vec<(String, String)>, auth: &[(String, String)]) -> Vec<String>`。
  - `pub async fn replay_request(client, method, url, headers, body_bytes, opts, auth_candidates) -> Result<ReplayResult, String>`。
- `crates/bifrost-admin/src/replay_executor.rs`：把 replay.rs 的纯函数拼装成从 `TrafficDb` 拿 record → 调用 replay → 拼 response 的路径。
- `crates/bifrost-admin/src/handlers/traffic.rs`
  - `GET /_bifrost/api/traffic/{id}/export` -> `parse_export_query` + `get_traffic_export`。
  - `POST /_bifrost/api/traffic/{id}/replay` -> 反序列化 `ReplayOptions`，按需从 `query_latest_window(200)` 拉 `auth_candidates`，调 `replay_request`。
- `crates/bifrost-cli/src/commands/traffic.rs` + `src/cli.rs`
  - `run_traffic_export(TrafficExportOptions)`。
  - `run_traffic_replay(TrafficReplayOptions)`：`--patch` sugar 解析、`--patch-json` 直接透传、`--refresh-auth` 布尔映射，`--timeout` 支持 `30s|1500ms` 后缀。

### API 契约

#### Export

```
GET /_bifrost/api/traffic/{id}/export?format=curl|fetch|har
```

- Response: `Content-Type: text/plain; charset=utf-8`，body 为对应格式字符串。
- 参数缺省时按 curl 返回。
- 未知 `id` 返回 404；`format` 值无效时按 curl 处理并在日志中打 warn，不强制 400。

#### Replay

```
POST /_bifrost/api/traffic/{id}/replay
Content-Type: application/json

{
  "patch_json": [
    {"op": "replace", "path": "/a/b", "value": "x"},
    {"op": "add",     "path": "/extras/-", "value": "new"},
    {"op": "remove",  "path": "/foo"}
  ],
  "refresh_auth": true,
  "refresh_auth_from": "latest",
  "auth_source_host": "api.example.com",
  "timeout_ms": 30000
}
```

Response（成功）:

```json
{
  "success": true,
  "data": {
    "status": 200,
    "duration_ms": 412,
    "request":  {"method": "POST", "url": "https://..."},
    "response": {"status": 200, "headers": [["content-type","..."]], "body_b64": "..."},
    "auth_refresh": {
      "applied": true,
      "source_traffic_id": "01H...",
      "fields": ["Authorization", "Cookie"]
    },
    "headers": [["content-type","..."]],
    "body_b64": "..."
  }
}
```

Response（失败）: `success=false`, `error.code` in `{ patch_error, upstream_error, timeout, invalid_body, not_found }`。

#### CLI

```
bifrost traffic export <id> --as curl|fetch|har [-o path]
bifrost traffic replay <id>
    [--patch '/a/b=val'] [--patch '/x+=1']  [--patch '-/y']
    [--patch-json '[{"op":"replace","path":"/a","value":1}]']
    [--refresh-auth]
    [--timeout 30s|1500ms]
    [--format human|json|json-pretty]
```

`--patch` 与 `--patch-json` 可组合；`--patch-json` 先应用，`--patch` sugar 追加。

### Refresh-auth 算法

1. 若 `opts.refresh_auth_enabled()` 为 true：
   1. 取 replay 请求最终 host（大小写归一）。
   2. 从 `traffic_db.query_latest_window(200)` 拿 compact summary，剔除 replay 自身 record。
   3. 按 host 精确匹配 candidate id 集。
   4. 对匹配集逐条 `get_by_id`，抽取 `request_headers`，构造 `[AuthCandidate]`。
   5. `find_refresh_auth_source` 挑 recency 最大且至少含一个 `classify_auth_header_field` 命中的 header 的记录。
   6. `apply_refresh_auth` 用大小写不敏感匹配覆盖 target headers；返回被替换或追加的字段名列表。
2. 结果通过 `auth_refresh { applied, source_traffic_id, fields }` 回传。
3. 未命中时 `applied=false`，`fields=[]`，不修改原 header。

## Sync 边界

- Export / Replay 均本地执行，不写入 sync 通道。
- Replay 请求不进 traffic_records，不影响 sequence，不推送到 IM/webhook。
- 远端 (`bifrost remote traffic export/replay`) 走 remote invoke 转发，等价于在目标机器上执行 CLI；输出仍是原始格式，不做二次脱敏。

## Phase 1-4

### Phase 1: 后端骨架

- `bifrost-admin::replay` 模块：ExportFormat / export_* / JSON Patch / ReplayOptions / ReplayResult / HttpClient trait。
- Handler 挂 `GET /traffic/{id}/export`, `POST /traffic/{id}/replay`。
- Unit test 覆盖 export 三格式、JSON Patch 三种 op、`refresh_auth_enabled` 兼容旧字段。

### Phase 2: CLI

- `bifrost traffic export|replay` 子命令与 `--patch` sugar 解析。
- `run_traffic_export` / `run_traffic_replay` 通过 admin HTTP client 复用后端。
- Format `human|json|json-pretty` 输出。
- `cli_commands.rs` 覆盖 arg parse。

### Phase 3: Refresh-auth

- `classify_auth_header_field`、`find_refresh_auth_source`、`apply_refresh_auth`。
- `replay_request` 端到端集成 mock HttpClient 测试。
- Handler 内根据 `refresh_auth_enabled()` 决定是否拉 `auth_candidates`。

### Phase 4: 远端与文档

- `bifrost remote traffic export|replay` 端到端跑通。
- README / docs / human_tests 更新。
- 与 auth-status 的整合 TODO 记入 `design/auth-status.md`。

## 测试方案

### 单元测试 (crates/bifrost-admin/src/replay.rs)

- `export_curl_contains_method_url_headers_and_body`
- `export_fetch_emits_js_snippet`
- `export_har_parses_as_json_with_entry`
- `apply_json_patch_replace_add_remove_root_and_nested`
- `apply_json_patch_add_appends_when_path_ends_with_dash`
- `replay_request_applies_patch_and_strips_content_length`
- `replay_request_without_patch_passes_body_through`
- `classify_auth_header_field_recognises_authorization_cookie_xtt`
- `find_refresh_auth_source_picks_latest_matching_host`
- `find_refresh_auth_source_returns_none_when_no_match`
- `apply_refresh_auth_overrides_or_appends_headers`
- `replay_request_with_refresh_auth_applies_authorization_and_cookie`
- `replay_request_with_refresh_auth_no_match_returns_not_applied`
- `refresh_auth_enabled_treats_legacy_string_as_truthy`

### CLI 测试 (crates/bifrost-cli/tests/cli_commands.rs)

- `Cli::try_parse_from(["bifrost", "traffic", "export", "42", "--as", "curl"])` 及 fetch/har 变体。
- `--patch` sugar 解析 `replace/add/remove`。
- `--patch-json` 直接透传。
- `--timeout 30s|1500ms` 解析。

### E2E

- `e2e-tests/tests/test_replay_rules.sh`
- `e2e-tests/tests/test_replay_body_decode.sh`
- `e2e-tests/tests/test_replay_websocket_frames.sh`
- `e2e-tests/tests/test_search_traffic_cli_isomorphic_e2e.sh`（对 export CLI/API 输出等价性做交叉验证）

### human_tests

- `human_tests/traffic-replay.md`
  - TC-TR-01 export curl 可粘贴执行。
  - TC-TR-02 export fetch 粘 DevTools 可执行。
  - TC-TR-03 export har 用 Chrome HAR viewer 打开。
  - TC-TR-04 replay 仅改字段。
  - TC-TR-05 replay + refresh-auth 命中。
  - TC-TR-06 replay + refresh-auth 未命中。
- `human_tests/api-replay.md`
  - TC-API-REPLAY-01 admin API replay 契约。
  - TC-API-REPLAY-02 error 分类。
- `human_tests/api-traffic.md`
  - TC-API-TRAFFIC-EXPORT-01 三种 format 响应类型。

所有 human_tests 启动 bifrost 时使用临时 `BIFROST_DATA_DIR`、非 9900 端口、`BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`、`BIFROST_DISABLE_TRAY=1` 与 `--no-system-proxy`。

## Review/Fix/Test 闭环

### 第 1 轮

- 复核 export 三格式对同一 record 的输出：header 顺序、body base64 编码、HAR 占位 response。
- 复核 patch sugar 与 `--patch-json` 组合顺序。
- 复核 refresh-auth 命中与未命中路径的 `auth_refresh` 字段。
- 复测 unit + CLI parse + `test_replay_rules.sh`。

### 第 2 轮

- 检查 refresh-auth 是否吞掉原 header：未命中时原 header 必须完整保留。
- 检查大响应 body_b64 编码性能与内存。
- 复测 `test_replay_body_decode.sh` + `test_replay_websocket_frames.sh`。
- 更新 `design/auth-status.md` 里 refresh-auth 的整合 TODO 状态。

## 风险与决策

- **脱敏**: 本期 export/replay 不做脱敏；调用方必须视作敏感输出。完整脱敏在 `design/proxy-log-redaction.md` 单独推。
- **HAR 完整性**: response 是占位（`status=0`），非真实响应；不适合作为回归对比工具，只用于把请求侧完整移植到别处。
- **IPv6 host**: URL host 提取遇到 `[::1]` 会退化为 `[`；本期不阻塞 IPv4 / 域名主流场景，后续在 `find_refresh_auth_source` 前补统一 host normalization。
- **refresh-auth 有效性**: 当前只按"最近出现"选择，不主动做 JWT/Cookie 有效性判断。P1-4 (auth-status) 落地后合并规则，改为只挑 valid record。
- **JSON Patch subset**: 不支持 `test` / `move` / `copy`。这是显式收敛：`test` 需要额外错误路径，`move/copy` 语义与 replace + add 组合等价，避免维护完整 6902 实现。
- **Replay 死循环**: replay 走独立 reqwest，不经过 bifrost 主端口。若用户显式 `http_proxy` 指到自己，需要在调用端处理，不在本模块拦截。
