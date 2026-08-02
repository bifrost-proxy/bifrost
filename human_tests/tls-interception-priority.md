# TLS 解包优先级真实场景测试

## 功能模块说明

验证 TLS 决策固定遵循 `Rules > Domain > App > Client IP > Global`，且同一层内 Passthrough 优先于 Force Intercept。重点回归 Domain Passthrough 与浏览器/命令行 App Force Intercept 同时命中时必须保持 CONNECT 隧道、不解包 HTTPS，也不能注入 Bifrost Badge。

## 前置条件

- 在仓库根目录执行。
- 已安装 Rust、Node.js、pnpm、curl 与 OpenSSL。
- E2E 和 UI 测试只使用临时数据目录、隔离端口和 `--no-system-proxy`，禁止停止或重启正式 `127.0.0.1:9900` 服务。
- 开始前记录正式 9900 listener PID，结束后确认 PID 未变。

## 测试用例列表

### TC-TIP-01：实现、CLI、WebUI 与文档使用同一优先级契约

操作步骤：

1. 执行：
   ```bash
   rg -n "Rules > Domain > App|Priority: Rules|Domain passthrough|域名命中.*intercept-exclude" design/tls-interception-priority.md SKILL.md docs docs-en site/src/content/docs web/src crates/bifrost-cli/src/cli.rs
   ```
2. 检查 `should_intercept_tls_for_client` 的决策顺序。

预期结果：

- 所有用户可见说明均表达 `Rules > Domain > App > Client IP > Global`。
- 同层 Passthrough 先于 Force Intercept。
- 代码先处理规则，再处理 Domain，之后才处理 App。

本次执行结果：通过。2026-08-02 执行静态检索并复核 `should_intercept_tls_for_client`：规则显式决策与规则派生 host rewrite 最先处理，随后依次处理 Domain exclude/include、App exclude/include、Client IP exclude/include 和 Global；`SKILL.md`、中英文 quick start、CLI help、Settings 文案和设计文档均使用同一优先级契约。

### TC-TIP-02：冲突矩阵单元测试

操作步骤：

1. 执行：
   ```bash
   cargo test -p bifrost-proxy --all-features should_intercept -- --nocapture
   cargo test -p bifrost-proxy --all-features should_passthrough -- --nocapture
   ```

预期结果：

- Domain Passthrough 覆盖 App Force Intercept。
- Domain Force Intercept 覆盖 App Passthrough。
- Rules 可覆盖 Domain/App。
- Domain、App、Client IP 各层内部均为 Passthrough 优先。

本次执行结果：通过。首次执行两个聚焦命令分别得到 `21 passed; 0 failed` 和 `3 passed; 0 failed`；第 1 轮 review 补齐 App 与 Client IP 双向冲突后复跑得到 `22 passed; 0 failed` 和 `4 passed; 0 failed`。覆盖 Domain Passthrough > App Force Intercept、Domain Force Intercept > App Passthrough、Rules 覆盖 Domain/App、App > Client IP，以及各层 Passthrough 优先边界。

### TC-TIP-03：真实 HTTPS 隧道不解包且不注入 Badge

操作步骤：

1. 构建当前源码：
   ```bash
   cargo build --bin bifrost
   ```
2. 使用临时数据目录和隔离端口执行：
   ```bash
   TEST_DIR="$(mktemp -d /tmp/bifrost-tls-priority-human.XXXXXX)"
   BIFROST_TEST_DATA_DIR="$TEST_DIR" \
   BIFROST_DATA_DIR="$TEST_DIR" \
   BIFROST_BIN=./target/debug/bifrost \
   PROXY_PORT=19770 MOCK_HTTP_PORT=19771 MOCK_HTTPS_PORT=19773 \
   ONLY_TEST=domain_app_priority \
   bash e2e-tests/tests/test_tls_intercept_e2e.sh
   ```

预期结果：

- curl App Force Intercept 确实命中。
- Domain Passthrough 仍使响应保持上游 HTML 原文且不含 `__bifrost_badge__`。
- Traffic 只有 CONNECT 外层记录，没有 `/test/domain-priority` 内层 HTTPS 记录。
- 测试汇总为 `1 passed, 0 failed`。

本次执行结果：通过。使用临时数据目录和 19770/19771/19773 隔离端口执行，汇总为 `1 passed, 0 failed`。curl 客户端应用命中 Force Intercept，但 127.0.0.1 Domain Passthrough 保持 CONNECT 隧道；响应无 `__bifrost_badge__`，Traffic 无 `/test/domain-priority` 内层记录。首次执行还发现旧 `eval ... &` 启动方式记录了包装 shell PID，已改为参数数组直接后台启动 Bifrost；复跑后全部隔离端口释放。

### TC-TIP-04：Settings 优先级在浅色和暗色主题均可见

操作步骤：

1. 执行聚焦 UI 测试：
   ```bash
   TEST_DIR="$(mktemp -d /tmp/bifrost-tls-priority-ui.XXXXXX)"
   BIFROST_DATA_DIR="$TEST_DIR" \
   pnpm --dir web test:ui tests/ui/admin-settings.spec.ts \
     --grep "Settings TLS 与证书页支持开关、模式和只读展示"
   ```

预期结果：

- Settings Proxy 页显示 `Priority: Rules > Domain > App > Client IP > Global.`。
- 同时显示 `Within each scope, Passthrough takes priority over Force Intercept.`。
- 浅色、暗色主题下提示均可见，Playwright 测试通过。

本次执行结果：通过。执行聚焦 Playwright 用例，最终结果为 `1 passed`，用例耗时 2.8 秒、总耗时 10.4 秒；浅色和暗色主题均显示完整优先级与同层 Passthrough 优先说明。前三次执行被本机真实连接 iPad 的延迟 CA 安装提示遮罩阻挡，测试已改为等待该明确命名的 dialog 并点击 `Not now`，产品优先级断言保持不变；修复后复跑通过。

## 清理步骤

- E2E 脚本和 Playwright teardown 停止各自启动的隔离服务。
- 删除本次测试创建的 `/tmp/bifrost-tls-priority-*` 临时目录。
- 确认 19670/19770 和 UI 动态端口无残留 listener。
- 再次检查正式 `127.0.0.1:9900` listener PID，与测试前保持一致。
