# 多行规则过滤器 E2E 验证方案

## 功能模块详细描述
- 针对 `line\`...\`` 多行规则补充端到端验证，确认规则在真实代理链路中完成“解析 → 匹配 → 转发 → 修改”闭环。
- 重点覆盖多行规则中的 `includeFilter` 与 `excludeFilter`，验证它们不只是在解析阶段被识别，还会实际影响请求转发与请求/响应修改行为。
- 用 mock HTTP echo 服务作为上游，通过回显请求头、路径和响应头，直接验证规则是否真正生效。

## 实现逻辑
- 在 `e2e-tests/rules/regression/line_block_filter_effect.txt` 提供专用规则夹具，使用两个 `line\`...\`` 规则块：
- 一个规则块负责将 `line-block-filter.local` 无条件转发到 mock HTTP echo 服务（端口由 `__ECHO_HTTP_PORT__` 占位符在运行期渲染），保证所有测试请求都能稳定到达上游。
- 一个规则块负责带 `includeFilter://m:GET`、`includeFilter:///api/`、`excludeFilter:///api/internal/` 的请求头与响应头修改（`reqHeaders://X-Line-Block-Request=matched`、`resHeaders://X-Line-Block-Response=matched`），验证过滤条件命中时才生效。
- 在 `e2e-tests/tests/` 新增专项 shell E2E 脚本：
- 启动独立数据目录（`$BIFROST_DATA_DIR=.bifrost-e2e-line-block-filter-<port>-<pid>`）的 Bifrost 代理，并以 `--skip-cert-check --unsafe-ssl --no-system-proxy --rules-file <fixture>` 启动；若 `target/release/bifrost` 未构建则脚本会执行 `cargo build --release --bin bifrost`。
- 启动 mock HTTP echo 服务。
- 通过代理对同一域名发起多组请求，覆盖：
- `GET /api/...` 命中 include 条件，验证请求头和响应头都被修改。
- `GET /api/internal/...` 命中 exclude 条件，验证修改被抑制但请求仍被基础转发规则送到 mock 服务。
- `POST /api/...` 不满足方法 include 条件，验证修改不生效。
- `GET /home` 不满足路径 include 条件，验证修改不生效。
- 断言层同时校验：
- 响应状态为成功。
- 请求确实到达 mock echo 服务。
- mock echo 回显的请求头是否按预期存在或缺失。
- 代理返回的响应头是否按预期存在或缺失。

## 依赖项
- `e2e-tests/mock_servers/http_echo_server.py`
- `e2e-tests/mock_servers/start_servers.sh`
- `e2e-tests/test_utils/assert.sh`
- `e2e-tests/test_utils/rule_fixture.sh`
- `e2e-tests/test_utils/process.sh`
- `target/release/bifrost`

## 测试方案
- 定向执行新增脚本：
- `bash e2e-tests/tests/test_multiline_rule_filter_e2e.sh`
- 新脚本使用 mock 服务黑盒验证多行规则的真实运行效果，而不是只检查解析结果。
- 断言覆盖请求期与响应期两个阶段，避免只验证单侧行为。
- 将 `test_multiline_rule_filter_e2e.sh` 纳入 `scripts/run_all_e2e.sh` 中的 `STABLE_SHELL_TESTS` 列表，确保日常默认回归入口持续覆盖该场景。
- 同步更新 `e2e-tests/rules/COVERAGE.md`，记录多行规则过滤器回归夹具已经补充。

## 校验要求
- 先执行新增 E2E 脚本，确认行为回归通过。
- 任务结束前执行 `rust-project-validate` 规定的校验流程。
- 若工作区级校验存在与本次改动无关的阻塞，需要在结果里明确说明失败位置和原因。

## 文档更新要求
- 更新 `e2e-tests/rules/COVERAGE.md`，补充多行规则过滤器专项回归夹具说明。
- 本次不涉及新协议、新 Hook、CLI 参数或 README 配置说明，无需更新 `README.md`。
