# HTTPS Tunnel 脚本链路执行补齐

## 背景

Bifrost 支持通过规则里 `reqScript://` 与 `resScript://` 在请求/响应链路上执行用户自定义 JS 脚本，配合 `decode://…` / `bp://… decode://bp` 可以在 traffic detail 里展示 parsed 或 decoded body。普通 HTTP handler 一直完整具备：脚本执行 → 回写响应 → 写入 traffic record → 展示脚本结果的能力。

现网真实反馈（对照材料 `~/.bifrost/https-res-script-issue.zh-CN.md`、`https-res-script-patch-draft.patch`）表明 HTTPS 解包后的 intercepted response 链路只复用了“修改响应头/body”那部分能力，未执行 `resolved_rules.res_scripts`。表现为：

- 规则命中详情显示 `ResScript` 已匹配，但 traffic detail 的 `res_script_results_blob` 为空。
- 客户端拿到的是上游原始响应，脚本设置的 `x-repro-script`、改写的 body、修改的 status 全部丢失。
- 对照实验：同一 HTTPS 路径下 `statusCode://` / `resBody://` 生效，因此 HTTPS 解包本身正常，问题收敛到脚本阶段缺失。

同时排查发现还有几个同类缺口：HTTPS tunnel 里 `reqScript` 也未执行、tunnel 链路不写 `decode` 落库、mock/`file://`/`tpl://`/`status://` 这类立即响应的 shortcut 也绕过了 `resScript`。此外脚本 API 使用 `HashMap<String, String>` 表达 headers，如果盲目用 HashMap 重建 HeaderMap 会误删多值 `Set-Cookie`。

本文档描述把 HTTP handler 与 HTTPS tunnel 的脚本 & body 元数据能力下沉到公共模块，两条链路复用同一入口，并补齐 traffic record 回写与多值 header 保留。

## 用户目标验证清单

### 必须实现

- HTTPS 命中 `resScript://…` 后客户端拿到脚本改写后的 status、headers、body。
- HTTPS 命中 `reqScript://…` 后上游收到脚本改写后的 method、headers、body。
- HTTPS 命中 `decode://…` 或 `bp://… decode://bp` 后 traffic detail 展示 decoded body / parser output，且写入 raw & decoded body ref。
- Mock / immediate response（`status://`、`file://`、`tpl://`、`rawfile://` 等 shortcut）也执行 `resScript`。
- Traffic record 补齐 `res_script_results` / `req_script_results` / `decode_*_script_results`。
- 脚本未触碰的多值 header（尤其 `Set-Cookie`）在回写响应时全部保留。

### 必须不破坏

- 普通 HTTP handler 的脚本执行、response 改写、traffic record 行为保持一致。
- 脚本 API 仍是 `HashMap<String, String>` 结构，不新增数据结构变更（避免用户脚本兼容性问题）。
- `decode://` 只影响 traffic 落库，不改写真实客户端响应。
- 孤立的 `bp://…` 不触发 TLS intercept；必须配合 `decode://bp` 才进入 parser 链路。
- Content-Encoding gzip/br/deflate/zstd 响应在脚本执行前后行为一致。
- 现有 `test_req_res_script_e2e.sh` 用例保持通过。

### 必须真实验证

- 通过 `e2e-tests/tests/test_req_res_script_e2e.sh` 覆盖 HTTPS `resScript` / `reqScript` / decode / bp decode / mock resScript / 多值 Set-Cookie。
- 使用本地 HTTP/HTTPS echo server，避免依赖外部域名。
- 真实 human_tests 至少走一次 curl + `--proxy` + `--cacert` 完整链路。

## 产品语义

### 三链路对齐

引入公共模块 `crates/bifrost-proxy/src/proxy/http/scripts.rs` 与 `body_metadata.rs`：

- `execute_request_scripts(...)`：入参含解压后 body、headers、method、URL parts、matched rules info；出参含改写后的 method/headers/body 与脚本执行结果集合。
- `execute_response_scripts(...)`：入参含解压后 body、headers、status；出参含改写后的 status/headers/body 与脚本执行结果集合。
- `apply_decode_scripts_for_storage(...)`：仅在写入 body store 前调用，产出 raw ref、decoded ref、decode 结果集合，不修改客户端响应。
- `parse_url_parts(...)`、`build_matched_rules_info(...)`、header/body ↔ 脚本数据结构转换等 helper 集中放置。
- `BodyMode`、`normalize_req_headers`、`normalize_res_headers`、`response_content_encoding`、`set_content_encoding_header` 沉淀到 `body_metadata.rs`，两条链路复用同一 header 归一化逻辑。

### 脚本执行时序

HTTPS tunnel 与普通 HTTP handler 保持一致时序：

1. 请求头/方法规则、请求 body 规则先应用。
2. `execute_request_scripts` 在上游请求构造前执行；若请求体已进入 streaming forward（超过 body buffer 上限）则跳过 reqScript。
3. 上游响应到达后，先应用响应头/body 规则，然后按当前 `Content-Encoding` 解码 body。
4. `execute_response_scripts` 在响应写回客户端前执行；成功后回写 status/headers/body，并按最终 `Content-Encoding` 重新编码（不可用编码降级为 identity）。
5. `apply_decode_scripts_for_storage` 在 body 落库前执行，产出 raw + decoded ref 供 traffic detail 展示。
6. 更新 traffic record：status、content_type、response_size、response_headers、`res_script_results`、`req_script_results`、`decode_req_script_results`、`decode_res_script_results`、`raw_request_body_ref`、`raw_response_body_ref`。

### 多值 header 保留

脚本 API 仍暴露 `HashMap<String, String>`：

- 脚本调用前，把 HeaderMap 折叠为 HashMap（多值取拼接或第一个）。
- 脚本执行成功后，不再无条件把 HashMap 重建为 HeaderMap；改为 header patch：
  - 未被脚本修改/删除的 header 名保留原始所有同名值。
  - 脚本改动的 header 名按脚本结果回写（可能覆盖原来的多个值，符合脚本主动改写语义）。
  - 脚本删除的 header 名从最终 HeaderMap 完整移除。
- 特别覆盖 `Set-Cookie`、`WWW-Authenticate`、`Link` 等多值场景。

### Mock / immediate response

`status://`、`file://`、`tpl://`、`rawfile://`、`resBody://` 这类由规则直接生成的立即响应也必须经过统一的响应处理链：

- 构造 mock 响应后，同样执行 `apply_content_injection`（HTML/JS/CSS 注入协议）与 `execute_response_scripts`。
- traffic record 中标记 `response_source = "mock"`，但脚本结果与真实上游响应无异。

## 技术细节

### 修改点

- `crates/bifrost-proxy/src/proxy/http/tunnel/mod.rs`
  - HTTPS 解包后接入 `execute_request_scripts` / `execute_response_scripts`。
  - 按 `Content-Encoding` 解压、编码。
  - 更新 traffic record。
- `crates/bifrost-proxy/src/proxy/http/handler.rs`
  - 抽出原有脚本逻辑到 `scripts.rs`。
  - handler 与 tunnel 都通过同一入口调用。
- 新增 `crates/bifrost-proxy/src/proxy/http/scripts.rs`
  - `execute_request_scripts` / `execute_response_scripts` / `build_matched_rules_info` / `parse_url_parts` / header 转换 helper。
- 新增 `crates/bifrost-proxy/src/proxy/http/body_metadata.rs`
  - `BodyMode`、`normalize_req_headers`、`normalize_res_headers`、`response_content_encoding`、`set_content_encoding_header`。
- `crates/bifrost-proxy/src/proxy/http/handler/decode.rs`（如仍存在）
  - 迁移到 `scripts.rs` 中的 `apply_decode_scripts_for_storage`。
- Header patch helper 新增到 `body_metadata.rs`，命名 `patch_headers_from_script_map`。
- Traffic record 序列化 `req_script_results` / `res_script_results` / `decode_req_script_results` / `decode_res_script_results` 字段。

### 边界

- body 超过 `max_body_buffer_size` 且进入 streaming forward 时跳过脚本（req 与 res 均如此），避免为了脚本强行缓冲大 body；traffic detail 记录 `script_skipped_reason = "streaming_body"`。
- 脚本执行超时/panic 时降级为原始响应，写入 `res_script_results[*].success = false` 与 `error_message`，不影响其他响应链路。
- 脚本改写后 body 长度变化，`Content-Length` / `Transfer-Encoding` 由 `normalize_res_headers` 重新计算。
- 若脚本删除或改写 `Content-Encoding`，`apply_content_injection` 与最终 header 保持一致，不出现 gzip 字节和明文脚本混拼。
- 孤立 `bp://…` 不触发 TLS intercept：只在 `decode://bp` 命中时才走 parser。

## CLI/Web/Admin API

### CLI

- `bifrost rule check <file>` 检测 `resScript://` 与 HTTPS 目标混用时提示“HTTPS tunnel 已支持脚本，无需额外配置”。
- `bifrost traffic get <id> --format json` 返回 `req_script_results`、`res_script_results`、`decode_*_script_results` 字段。

### Web

- Traffic detail Scripts 面板：分区展示 Request Scripts / Response Scripts / Decode Scripts，每项展示 success/duration/error/output snippet。
- Mock 响应也显示 Scripts 分区。
- 多值 `Set-Cookie` 在 Response Headers 面板保留所有值。

### Admin API

- `GET /api/traffic/:id` 响应字段扩展。
- `GET /api/traffic/:id/scripts` 独立返回脚本结果聚合，便于长脚本 output 独立分页。

## Sync 边界

- Traffic 同步（若开启）需要 schema 版本迁移：新增字段缺省值 `null`，旧客户端读取时忽略。
- rule sync 不受影响；`resScript` / `reqScript` / `decode` 已经是本地规则语言的一部分，格式无变化。

## 实现切分

### Phase 1：公共模块下沉

- 抽出 `scripts.rs` 与 `body_metadata.rs`。
- HTTP handler 迁移到公共入口，覆盖单测保持通过。

### Phase 2：HTTPS tunnel 接入

- tunnel 请求前接入 `execute_request_scripts`。
- tunnel 响应前接入 `execute_response_scripts`。
- tunnel 走 `apply_decode_scripts_for_storage` 落库。

### Phase 3：Mock 与多值 header

- Mock / immediate response 补 `apply_content_injection` + `execute_response_scripts`。
- Header patch helper 保留多值 header。

### Phase 4：E2E 与文档

- 扩展 `test_req_res_script_e2e.sh` 覆盖 HTTPS + reqScript + decode + mock + Set-Cookie。
- 更新 `human_tests/proxy-rules-advanced.md`、`human_tests/readme.md`。

## 测试方案

### 单元测试

- `execute_response_scripts_rewrites_status_headers_body`
- `execute_response_scripts_preserves_multi_value_set_cookie`
- `execute_request_scripts_skips_when_streaming`
- `normalize_res_headers_reencodes_content_length_after_body_change`
- `apply_decode_scripts_for_storage_writes_raw_and_decoded_refs`
- `patch_headers_from_script_map_keeps_untouched_multi_values`
- `mock_response_runs_response_scripts_and_injections`
- `bp_decode_requires_decode_bp_to_trigger_parser`

### E2E 测试

- `bash -n e2e-tests/tests/test_req_res_script_e2e.sh`
- `bash e2e-tests/tests/test_req_res_script_e2e.sh`
- 新增用例断言：
  - HTTPS 命中 `resScript` 后最终状态码变为 `218`。
  - 响应头包含脚本注入的 `x-repro-script: ran`。
  - 响应体为脚本改写后的 JSON。
  - traffic detail 中 `res_script_results[0].success == true`。
  - HTTPS 命中 `reqScript` 后上游收到脚本注入 header、改写 body 与改写 method，traffic detail 包含 `req_script_results`。
  - HTTPS 命中 `decode://decode_script` 后 response body API 展示 decoded body，traffic detail 包含 `decode_res_script_results`。
  - HTTPS 命中 `bp://local_echo decode://bp` 后 response body API 展示 parser output。
  - Mock immediate response 命中 `resScript` 后客户端响应与 traffic detail 均反映脚本结果。
  - resScript 成功执行但未触碰 `Set-Cookie` 时，多个 `Set-Cookie` 响应头全部保留。

### 真实场景测试

- 更新 `human_tests/proxy-rules-advanced.md`：新增 TC-HTS-01 到 TC-HTS-06，覆盖 HTTPS resScript、reqScript、decode、bp decode、mock resScript、多值 Set-Cookie。
- 同步 `human_tests/readme.md` 索引。
- 所有服务启动使用临时 `BIFROST_DATA_DIR`、非 9900 端口、`BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`、`BIFROST_DISABLE_TRAY=1`、`--no-system-proxy`。

### 校验与项目验证

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test -p bifrost-proxy scripts -- --nocapture`
- `cargo test -p bifrost-proxy body_metadata -- --nocapture`
- `cargo test --workspace --all-features`
- `rust-project-validate`

本机 no-local-coverage 约定下豁免 `make coverage`。

## Review/Fix/Test 闭环方案

### 第 1 轮

- 对照用户报告与 patch draft 逐项复核：resScript、reqScript、decode/bp decode、mock resScript、多值 header。
- `git status --short`、`git diff` 检查是否残留旧脚本入口。
- 重点 review：公共模块边界（scripts vs body_metadata）；tunnel 与 handler 是否稳定复用；streaming body 跳过脚本是否有埋点。
- 复测：focused Rust test、`test_req_res_script_e2e.sh`、mock resScript 用例。

### 第 2 轮

- 复查第 1 轮修复后的最新 diff、human_tests 索引、脚本链路覆盖。
- 重点 review：多值 header patch 是否有边界 bug（例如脚本删除 header 但保留其他同名值的场景）；traffic record 字段序列化兼容性；HTTP/2 gzip response 脚本后 Content-Length 是否正确。
- 复测：focused E2E、Rust focused test、进入 `rust-project-validate` 的 fmt/clippy/workspace test；用 curl + `--cacert` + Playwright 各跑一次。

## 风险与决策点

- **多值 header 语义**：脚本 API 仍是 HashMap，因此脚本主动改写同名多值 header 时只能表达单个最终值；文档需说明该限制。
- **公共模块边界**：`scripts.rs` 与 `body_metadata.rs` 需要清晰划分——脚本执行相关 vs body/header 元数据规范化。
- **降级策略**：body 解压失败或重压缩失败时跳过脚本并保持原响应，避免破坏客户端。
- **traffic schema 迁移**：新增字段全部可选，旧数据以 `null` 展示。
- **`bp://` 单协议触发 TLS intercept**：明确禁止，避免误伤只做 parser 展示的用户。
- **性能**：脚本执行为热路径开销，需在响应头、body 已经解压的前提下调用，避免重复解压；tunnel 复用 handler 的 body 缓存即可。
