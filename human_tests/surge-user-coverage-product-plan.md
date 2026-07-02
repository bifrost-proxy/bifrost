# Surge 用户覆盖与 Surge Bridge 真实场景测试

## 功能模块说明

本用例验证 Bifrost 面向 Surge 用户覆盖方案的第一批 Surge Bridge 能力：本地 Surge profile dry-run 导入、兼容性报告、Surge ordered rule explain、Bifrost Native Profile conversion preview，以及方案文档完整性。

## 前置条件

- 仓库位于 `/Users/eden_studio/work/github/bifrost`。
- 使用当前工作区编译出的 `target/debug/bifrost` 或由测试脚本自动执行 `cargo build --bin bifrost`。
- 所有命令使用临时 `BIFROST_DATA_DIR`。
- 设置 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1` 和 `BIFROST_DISABLE_TRAY=1`。
- 本用例不启动代理服务，不修改系统代理，不启用 Surge profile。

## 测试用例列表

### TC-SURGE-01：方案文档包含三迭代产品与技术边界

操作步骤：

1. 执行 `test -f design/surge-user-coverage-product-plan.md`。
2. 执行 `rg -n "迭代一：Surge Bridge|迭代二：Bifrost Native Profile|迭代三：Bifrost Proxy Platform|尚未实现|本次落地范围" design/surge-user-coverage-product-plan.md`。

预期结果：

- 方案文档存在。
- 检索结果能定位三次迭代、未实现清单和本次落地范围。

### TC-SURGE-02：Surge profile dry-run 导入生成兼容性报告

操作步骤：

1. 创建临时 Surge profile，包含 `[General]`、`[Proxy]`、`[Proxy Group]`、`[MITM]`、`[Rule]`。
2. 执行 `BIFROST_DATA_DIR=<tmp> BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 BIFROST_DISABLE_TRAY=1 target/debug/bifrost profile import <profile> --dry-run`。

预期结果：

- 输出包含 `Surge profile dry-run import`。
- 输出包含兼容性汇总。
- 输出能看到 `DOMAIN-SUFFIX`。
- 输出包含 `Not supported yet`，用于提示 `GEOIP` 等未支持能力。
- 命令不创建运行时 profile，不启动代理服务。

### TC-SURGE-03：profile explain 按 Surge ordered first-match 命中规则

操作步骤：

1. 使用 TC-SURGE-02 的临时 profile。
2. 执行 `target/debug/bifrost profile explain --profile <profile> https://sub.example.com/path`。

预期结果：

- 输出包含 `Surge profile explain`。
- 输出包含 `DOMAIN-SUFFIX`。
- 输出包含 `Selected policy Proxy`。
- 命中 `DOMAIN-SUFFIX,example.com,Proxy`，而不是后续 `FINAL`。

### TC-SURGE-04：profile convert 只输出 Bifrost Native Profile 预览

操作步骤：

1. 使用 TC-SURGE-02 的临时 profile。
2. 执行 `target/debug/bifrost profile convert <profile> --to bifrost`。

预期结果：

- 输出包含 `Bifrost Native Profile Preview`。
- 输出包含 `host suffix example.com -> Proxy`。
- 输出包含 `Compatibility summary`。
- 对存在行为差异或暂不支持的能力以注释或 summary 保留，不静默丢弃。

### TC-SURGE-05：自动 E2E 脚本覆盖 CLI 黑盒链路

操作步骤：

1. 执行 `bash e2e-tests/tests/test_profile_surge_bridge_cli.sh`。

预期结果：

- 脚本自动构造临时 Surge profile。
- `profile import --dry-run`、`profile explain`、`profile convert` 三条链路均通过断言。
- 测试结束后删除临时目录。

## 清理步骤

- 删除测试中创建的临时目录。
- 不需要停止 Bifrost 服务，因为本用例不启动代理服务。
