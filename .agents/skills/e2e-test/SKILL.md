---
name: "e2e-test"
description: "创建和执行 Bifrost 代理的端到端测试；在添加新功能或修复 bug 后用于验证。必须优先于 rust-project-validate 技能执行。"
---

# E2E 测试创建与执行

该技能用于创建和执行 Bifrost 的端到端测试，优先覆盖本次变更涉及的能力。

## 何时调用

- 添加新功能后需要验证
- 修复 bug 后需要回归测试
- 需要创建新的测试用例
- 需要运行现有的 E2E 测试

## 参考文档

在执行任务前，**必须先阅读** E2E 测试框架的详细文档：

- [e2e-tests README](e2e-tests/readme.md) - 测试架构、目录结构、断言库、现有用例
- [测试覆盖率](e2e-tests/rules/COVERAGE.md) - 当前覆盖范围与缺口
- [项目规则](../../rules/project_rules.md) - 仓库级开发与验证要求

## 使用说明

- 启动代理服务时，必须显式设置临时数据目录，避免覆盖本机已有数据；目录前缀统一使用 `./.bifrost-e2e`
- 若需要手动启动服务，优先使用“先编译、再启动”的方式，而不是直接假设 `cargo run` 或旧进程已生效
- 新增或修改“规则行为”相关测试时，规则定义必须存放在 `e2e-tests/rules/` 下，不能散落到根目录、临时文件或其他测试目录
- `e2e-tests/rules/` 下的规则文件必须按“模块功能 + 测试目标”组织：
  - 先按模块功能拆分子目录，例如 `forwarding/`、`request_modify/`、`response_modify/`
  - 再按单一测试目标拆分 `.txt` 文件；一个文件只表达一组紧密相关的规则语义，避免把多个无关目标塞进同一个文件
- 规则文件命名要直接表达测试目标，使用小写下划线风格，例如 `headers.txt`、`wildcard_level.txt`、`tls_intercept_rule.txt`
- 每个规则文件顶部必须先写清楚测试目标，再写测试规则本体；顶部说明至少应回答“这个文件验证什么语义/行为”
- 与规则文件配套的测试脚本必须存放在 `e2e-tests/tests/`，不能继续放在 `scripts/`、根目录或其他位置；脚本命名建议与测试目标对应，例如 `test_header_replace.sh`
- 如果某个测试需要额外样例数据、模板或 mock 文件，分别放到现有的 `e2e-tests/test_data/`、`e2e-tests/mock_servers/` 等目录，不要和规则文件混放
- 对涉及前端静态资源或管理端推送逻辑的改动，优先执行：

```bash
CARGO_TARGET_DIR=./.bifrost-ui-target cargo build --bin bifrost
BIFROST_DATA_DIR=./.bifrost-e2e-test ./.bifrost-ui-target/debug/bifrost start -p 8800 --unsafe-ssl
```

- 启动后，必须同时检查：
  - 目标端口是否真的由最新 `bifrost` 进程监听，例如 `lsof -nP -iTCP:8800 -sTCP:LISTEN`
  - 管理端 API 是否 ready，例如 `curl -sS http://127.0.0.1:8800/_bifrost/api/proxy/address`
- 如果启动过程中出现证书安装交互，先显式处理掉，再继续测试，避免把“服务未启动完成”误判为功能问题
- 不同场景的步骤已拆分为独立文档；按需打开对应文件执行即可
- 当 UI 现象和 API 现象不一致时，优先做“进程级 + API + WebSocket frame”三层交叉验证：
  - 先确认当前测试页面连到的是哪一个 `bifrost` 进程
  - 再确认 `/_bifrost/api/traffic`、`/_bifrost/api/traffic/{id}` 返回的真实状态
  - 最后抓浏览器 `/api/push` 的 `framesent/framereceived`，判断是“服务端没推”还是“页面没订阅/没消费”
- 新建规则测试后，顺手检查对应的规则文件与脚本是否一一对应；若无法建立清晰对应关系，应继续拆分目录或文件
- 测试结束后清理临时目录和残留进程

## CLI 测试方法

当本次变更涉及 `bifrost` CLI 子命令（例如 `bifrost value ...`、`bifrost rule ...`、`bifrost script ...`）时，推荐使用“黑盒 + 独立数据目录”的方式编写与执行测试：

- 使用独立 `BIFROST_DATA_DIR`：用 `mktemp -d` 创建测试目录并导出，避免污染本机 `~/.bifrost`
- 优先复用已编译二进制：默认使用 `target/release/bifrost`，不存在时再 `cargo build --release --bin bifrost`
- 断言策略：
  - 基于 CLI 输出：用 `grep`/`jq`（如有）做关键字段断言
  - 基于落盘结果：检查 `BIFROST_DATA_DIR` 下对应文件/目录是否按预期创建与更新
- 清理策略：用 `trap cleanup EXIT` 确保结束后停止残留进程并删除临时目录

现有 CLI 测试用例可直接参考与复用：

- `e2e-tests/test_values_cli.sh`：示例化展示了 build、`BIFROST_DATA_DIR` 隔离、以及 set/get/list/delete/import 的断言方式
- `e2e-tests/tests/test_cli_proxy_start_e2e.sh`：启动相关 CLI 的端到端覆盖
- `e2e-tests/tests/test_upgrade_cli.sh`：升级相关 CLI 覆盖

### 针对新 CLI 子命令的脚本模板（示例）

以 `bifrost script` 为例，核心验证点建议包括：add → show → list → delete 以及脚本文件落盘路径：

```bash
TEST_DIR="$(mktemp -d)"
trap 'rm -rf "$TEST_DIR"' EXIT

export BIFROST_DATA_DIR="$TEST_DIR"
BIFROST_BIN="./target/release/bifrost"

"$BIFROST_BIN" script add request cli_test -c 'log.info("hello");'
"$BIFROST_BIN" script show request cli_test | grep -q 'hello'
"$BIFROST_BIN" script list -t request | grep -q 'cli_test'
test -f "$BIFROST_DATA_DIR/scripts/request/cli_test.js"
"$BIFROST_BIN" script delete request cli_test
```

## 场景文档

- [01-快速构建启动](01-快速构建启动.md)
- [02-SSE请求详情分批推送验证](02-SSE请求详情分批推送验证.md)
- [03-运行全量测试](03-运行全量测试.md)
- [04-运行单个测试](04-运行单个测试.md)
- [05-创建新测试](05-创建新测试.md)
- [06-调试与端到端验证方法](06-调试与端到端验证方法.md)
- [07-真实人类测试](07-真实人类测试.md)
