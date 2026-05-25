# HTTPS Tunnel `resScript` 回归补齐

## 背景
- 已修复 HTTPS 解包后的 intercepted response pipeline 未执行 `resScript` 的问题。
- 当前缺少一条专门覆盖真实 HTTPS 上游的回归测试，后续重构 `tunnel` 或脚本模块时容易再次退化。
- 本轮不处理脚本头部多值语义问题，仅聚焦“HTTPS 解包后 `resScript` 必须执行”这一主链路。

## 目标
- 为 `https://httpbin.org/anything/https-repro` 增加最小回归验证。
- 断言 HTTPS 解包命中 `resScript` 后，最终响应状态码、响应体和 Traffic 详情中的脚本执行结果都被更新。
- 同步补齐 `human_tests/` 文档，确保真实场景验收链路存在。

## 范围
- `e2e-tests/tests/test_req_res_script_e2e.sh`
- `e2e-tests/rules/request_modify/req_res_script.txt`
- `human_tests/proxy-rules-advanced.md`
- `human_tests/readme.md`

## 实现方案
- 复用现有 `req_res_script` Shell E2E，而不是新建一套独立脚本。
- 将规则定义落到 `e2e-tests/rules/request_modify/req_res_script.txt`，在脚本运行时替换 `__ECHO_HTTP_PORT__` 占位符。
- 启动代理时追加 `--intercept-include "httpbin.org"`，只对目标域名启用 HTTPS 解包，避免影响本脚本的本地 HTTP mock 场景。
- 在测试脚本生成的 `res_script.js` 中新增 `http-repro` / `https-repro` 分支：
  - 将响应状态改为 `218`
  - 注入 `x-repro-script: ran`
  - 重写 JSON 响应体，带上 `via` 和 `path`
- 通过 Admin API 读取 Traffic 详情，断言：
  - `status == 218`
  - `res_script_results[0].script_name == "res_script"`
  - `res_script_results[0].success == true`

## 不做事项
- 不修复 `HeaderMap <-> HashMap<String, String>` 导致的多值头折叠问题。
- 不在本轮补 `reqScript` 的 HTTPS intercepted request path 回归。
- 不把脚本回归扩展到 synthetic/mock shortcut response path。

## 验证计划
- Focused Shell E2E：
  - `bash e2e-tests/tests/test_req_res_script_e2e.sh`
- 静态检查：
  - `bash -n e2e-tests/tests/test_req_res_script_e2e.sh`
- 人测文档：
  - 更新 `human_tests/proxy-rules-advanced.md`
  - 同步 `human_tests/readme.md` 索引

## 风险
- 该回归依赖外部 `httpbin.org`，网络异常时测试会降级为跳过通过，不把外部不可达误报为功能回退。
- 该回归只覆盖 HTTPS `resScript` 响应改写主路径，不覆盖多值响应头保真度。
