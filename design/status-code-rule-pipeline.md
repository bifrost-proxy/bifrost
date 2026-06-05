# statusCode 规则流水线修复方案

## 功能模块说明

`statusCode://code` 用于直接生成本地响应并阻断上游请求。它不应该终止同一条命中规则内其他协议的处理：请求头、请求体、响应头、响应体和脚本规则仍应按请求/响应阶段执行，并在 Traffic 中可观察。

## 问题与影响

修复前 HTTP 和 HTTPS MITM 路径都在请求规则处理前返回 `statusCode` 直接响应，导致 `reqHeaders`、`reqBody` 等请求侧规则没有执行；HTTPS MITM 的直接响应路径也缺少与 HTTP 路径一致的响应体规则处理。

## 实现逻辑

- HTTP 直接响应路径先执行请求规则和请求体规则，生成本地响应后再执行响应体规则与响应脚本，并把改写后的请求/响应内容写入 Traffic。
- HTTPS MITM 路径把 `statusCode` 直接响应移动到请求规则处理之后，避免建立上游连接，同时补齐响应体规则处理和 Traffic 记录。
- 保持 `statusCode` 不请求 upstream 的核心语义不变；`replaceStatus` 仍是请求后端后的状态码替换。

## 测试方案

- 单元/编译：执行 `cargo check -p bifrost-proxy -p bifrost-e2e` 验证改动可编译。
- E2E：新增 `status_statusCode_preserves_rule_pipeline`，覆盖 `statusCode + reqHeaders + reqBody + resHeaders + resBody + resAppend`，断言不请求 upstream 且 Traffic 可观察请求侧改写。
- 真实场景测试：更新 `human_tests/status-code-direct-response.md`，新增 TC-SCDR-03 并真实执行对应 E2E 命令。

## Review/Fix/Test 闭环方案

- 第 1 轮：复核用户目标、HTTP/HTTPS 两条路径和新增 E2E，运行新增用例与既有 `statusCode` 回归用例。
- 第 2 轮：复查 diff、文档、human_tests 索引和格式，复跑受影响用例并执行 Rust 项目校验。

## 校验要求

- 最小验证：`cargo check -p bifrost-proxy -p bifrost-e2e`、新增 E2E、既有 statusCode E2E。
- 收尾验证：执行 `cargo fmt --all -- --check`、`cargo clippy --workspace --all-targets --all-features -- -D warnings`、`cargo test --workspace --all-features`，并跟进远端 CI。
