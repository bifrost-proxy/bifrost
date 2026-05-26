# HTTPS Tunnel `resScript` 执行补齐

## 功能模块详细描述

- 修复 HTTPS 解包后的 intercepted response pipeline 未执行 `resScript` 的问题。
- 让 HTTPS tunnel/MITM 链路与普通 HTTP 响应链路在 response script 语义上保持一致。
- 保证脚本执行结果不仅体现在最终返回给客户端的响应上，也会同步写入 traffic record。

## 问题背景

- 本次 PR 的背景材料见 `/Users/bytedance/.bifrost/https-res-script-issue.zh-CN.md` 与 `/Users/bytedance/.bifrost/https-res-script-patch-draft.patch`。
- 现网问题表现为：HTTPS 请求命中 `resScript://...` 后，规则元数据里能看到 `ResScript`，但最终响应仍是上游原始响应，`res_script_results_blob` 为空。
- 对照实验表明 HTTPS 解包本身是正常的：同一路径下 `statusCode://...` 与 `resBody://...` 可以生效，因此问题收敛到 HTTPS intercepted response path 缺少 response script 执行步骤。
- 普通 HTTP handler 已经具备完整的 response script 执行、回写响应、写入 traffic record 的逻辑；HTTPS tunnel 链路只复用了部分响应修改能力，没有复用脚本阶段。

## 实现逻辑

### 1. 在 HTTPS intercepted response path 接入 `resScript`

- 修改 `crates/bifrost-proxy/src/proxy/http/tunnel/mod.rs`，在 HTTPS 解包后的响应处理链路中，放在常规响应头/body 修改之后、最终响应写回客户端之前执行 `resolved_rules.res_scripts`。
- 进入脚本前，按当前响应头上的 `Content-Encoding` 将 `final_body` 解码为脚本可处理的字符串。
- 脚本执行成功后，把脚本修改后的 `status`、`headers`、`body` 回写到 `res_parts` 与 `final_body`，保证客户端拿到的是脚本最终产物。
- 脚本写回 body 后，按脚本执行后的最终 `Content-Encoding` 重新编码；如果编码不可用，则回退为 identity body。

### 2. 对齐 HTTP 与 tunnel 的公共脚本能力

- 将原先散落在 `handler.rs` / `handler/decode.rs` 中的脚本公共逻辑抽到 `crates/bifrost-proxy/src/proxy/http/scripts.rs`：
  - `execute_request_scripts(...)`
  - `execute_response_scripts(...)`
  - `build_matched_rules_info(...)`
  - `parse_url_parts(...)`
  - header/body 与脚本数据结构之间的转换函数
- 这样普通 HTTP handler 与 HTTPS tunnel 都复用同一套脚本执行入口，减少两条链路后续再次漂移的风险。

### 3. 抽离响应体元数据与 header 归一化逻辑

- 新增 `crates/bifrost-proxy/src/proxy/http/body_metadata.rs`，统一沉淀以下能力：
  - `BodyMode`
  - `normalize_req_headers(...)`
  - `normalize_res_headers(...)`
  - `response_content_encoding(...)`
  - `set_content_encoding_header(...)`
- tunnel 链路在脚本改写响应后使用统一的 header 归一化逻辑，确保 `Content-Length` / `Transfer-Encoding` / `Content-Encoding` 与最终 body 一致。

### 4. 补齐 traffic record 回写

- HTTPS tunnel 链路在脚本执行成功后，同步更新 traffic record：
  - `status`
  - `content_type`
  - `response_size`
  - `response_headers`
  - `res_script_results`
- 这样 traffic detail 中的执行结果、响应状态和最终响应头会与客户端观测结果保持一致，不再出现“规则已命中但脚本执行结果为空”的不一致状态。

## 依赖项

- `crates/bifrost-proxy/src/proxy/http/tunnel/mod.rs`
- `crates/bifrost-proxy/src/proxy/http/handler.rs`
- `crates/bifrost-proxy/src/proxy/http/scripts.rs`
- `crates/bifrost-proxy/src/proxy/http/body_metadata.rs`
- 复用现有 `bifrost_script`、body 压缩/解压与 traffic record 更新能力

## 不做事项

- 不在本轮修复 `HeaderMap <-> HashMap<String, String>` 导致的多值 header 折叠语义。
- 不在本轮补齐 `reqScript`、`decode`、`bp + decode://bp` 在 HTTPS 解包链路上的完整支持。
- 不把本次修复扩展到 synthetic/mock shortcut response path，只聚焦真实 HTTPS 上游响应的 `resScript` 主链路。

## 测试方案

- Focused E2E：
  - 复用 `e2e-tests/tests/test_req_res_script_e2e.sh`
  - 在 `e2e-tests/rules/request_modify/req_res_script.txt` 增加 HTTPS `resScript` 场景
  - 使用本地 HTTP/HTTPS echo server，避免依赖外部域名
- 断言项：
  - HTTPS 命中 `resScript` 后最终状态码变为 `218`
  - 响应头包含脚本注入的 `x-repro-script: ran`
  - 响应体为脚本改写后的 JSON
  - traffic detail 中 `res_script_results[0].success == true`
- 文档验收：
  - 更新 `human_tests/proxy-rules-advanced.md`
  - 同步 `human_tests/readme.md` 索引

## 风险与兼容性

- 当前脚本 header 读写仍基于 `HashMap<String, String>`，多值 header 会被折叠；本次只保证 tunnel 链路与既有 HTTP 语义对齐，不扩大语义面。
- body 进入脚本前需要按 `Content-Encoding` 解压，若上游返回异常编码或解压失败，仍需沿用现有降级策略，避免生成损坏响应。
- 本次改动涉及 HTTP handler 与 tunnel 的公共能力下沉，后续评审需重点关注模块边界是否清晰，以及公共函数是否被两条链路稳定复用。

## 校验要求

- 执行 focused E2E 与脚本静态检查：
  - `bash -n e2e-tests/tests/test_req_res_script_e2e.sh`
  - `bash e2e-tests/tests/test_req_res_script_e2e.sh`
- 按修改范围执行相关 Rust 测试与编译校验；若环境阻塞，需记录具体失败点。

## 文档更新要求

- 本设计文档记录 HTTPS tunnel `resScript` 的实现方案与边界。
- `human_tests/proxy-rules-advanced.md` 增加真实 HTTPS `resScript` 回归用例。
- `human_tests/readme.md` 同步用例索引。
