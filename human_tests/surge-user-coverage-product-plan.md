# Surge 用户覆盖与 Surge Bridge 真实场景测试

## 功能模块说明

本用例验证 Bifrost 面向 Surge 用户覆盖方案的 Surge Bridge 能力：本地与远程 Surge profile dry-run 导入、兼容性报告、include/ruleset/domain-set resolved plan、managed profile URL、远程资源 ETag/cache、Surge ordered rule explain、Policy Group dry-run decision explain、Bifrost Native Profile conversion preview，以及方案文档完整性。

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
2. 执行 `rg -n "迭代一：Surge Bridge|迭代二：Bifrost Native Profile|迭代三：Bifrost Proxy Platform|尚未实现|本次落地范围|Managed profile URL|ETag" design/surge-user-coverage-product-plan.md`。

预期结果：

- 方案文档存在。
- 检索结果能定位三次迭代、未实现清单、本次落地范围、managed profile URL 和 ETag/cache 说明。

### TC-SURGE-02：Surge profile dry-run 导入生成兼容性报告

操作步骤：

1. 创建临时 Surge profile，包含顶层 `#!include`、远程 `#!include`、`[General]`、`[Proxy]`、`[Proxy Group]`、`[MITM]`、`[Rule]`。
2. 在同一临时目录创建 `included.conf`、`rules.list`、`domains.list`。
3. 启动本地 HTTP server 提供 `remote-include.conf`、`remote-rules.list`、`remote-domains.list`，响应包含 `ETag` 和 `Last-Modified`。
4. 执行 `BIFROST_DATA_DIR=<tmp> BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 BIFROST_DISABLE_TRAY=1 target/debug/bifrost profile import <profile> --dry-run`。

预期结果：

- 输出包含 `Surge profile dry-run import`。
- 输出包含兼容性汇总。
- 输出能看到 `DOMAIN-SUFFIX`。
- 输出包含 `Not supported yet`，用于提示 `GEOIP` 等未支持能力。
- 输出包含 `Resolved resources`，并列出 `included.conf`、`rules.list` 或 `domains.list` 的解析状态。
- 输出包含 `cache sha256:`，证明本地资源生成了可缓存内容身份。
- 输出包含 `remote-rules.list` 和 `etag "surge-remote-v1"`，证明远程资源被拉取并保留 HTTP cache metadata。
- 命令不创建运行时 profile，不启动代理服务。

### TC-SURGE-03：effective profile 展示 dry-run runtime plan

操作步骤：

1. 使用 TC-SURGE-02 的临时 profile 和本地资源文件。
2. 执行 `target/debug/bifrost profile effective <profile>`。

预期结果：

- 输出包含 `Surge effective profile dry-run`。
- 输出包含 `Policy graph`。
- 当策略组引用不存在的成员时，输出包含 `missing members`。
- 输出包含 `RULE-SET:rules.list` 和 `DOMAIN-SET:domains.list`，证明本地资源已展开为 ordered rules。
- 输出包含远程 `RULE-SET:<url>/remote-rules.list` 和 `DOMAIN-SET:<url>/remote-domains.list`，证明远程资源已展开为 ordered rules。
- 第二次解析同一 profile 时输出包含 `cache-hit`，证明 ETag 条件请求和 304 cache 命中生效。
- `Resolved resources` 行包含 cache key，便于后续持久 cache 和审计。

### TC-SURGE-04：profile explain 按 Surge ordered first-match 命中规则

操作步骤：

1. 使用 TC-SURGE-02 的临时 profile。
2. 执行 `target/debug/bifrost profile explain --profile <profile> https://sub.example.com/path`。

预期结果：

- 输出包含 `Surge profile explain`。
- 输出包含 `DOMAIN-SUFFIX`。
- 输出包含 `Selected policy Proxy`。
- 输出包含 `Policy decision: Proxy -> ProxyA`，证明 `select` policy group 已解析到 terminal proxy。
- 命中 `DOMAIN-SUFFIX,example.com,Proxy`，而不是后续 `FINAL`。

### TC-SURGE-05：profile explain 可命中本地 RULE-SET 展开规则

操作步骤：

1. 使用 TC-SURGE-02 的临时 profile，其中 `[Rule]` 包含 `RULE-SET,rules.list,Proxy`，且 `rules.list` 包含 `DOMAIN-SUFFIX,ruleset.example`。
2. 执行 `target/debug/bifrost profile explain --profile <profile> https://api.ruleset.example/path`。

预期结果：

- 输出包含 `RULE-SET:rules.list`。
- 输出包含 `Selected policy Proxy`。
- 命中展开后的 `DOMAIN-SUFFIX,ruleset.example`，而不是后续 `FINAL`。

### TC-SURGE-06：profile explain 展示 url-test dry-run policy decision

操作步骤：

1. 使用 TC-SURGE-02 的临时 profile，其中 `[Proxy Group]` 包含 `Auto = url-test, ProxyA, DIRECT, url=http://example.com/generate_204`，且 `[Rule]` 包含 `DOMAIN,auto.example,Auto`。
2. 执行 `target/debug/bifrost profile explain --profile <profile> https://auto.example/path`。

预期结果：

- 输出包含 `Policy decision: Auto -> ProxyA`。
- 输出包含 `active latency probing is not running`。
- 说明 dry-run explain 不进行真实延迟探测，但会给出当前可解释的 terminal policy。

### TC-SURGE-07：profile convert 只输出 Bifrost Native Profile 预览

操作步骤：

1. 使用 TC-SURGE-02 的临时 profile。
2. 执行 `target/debug/bifrost profile convert <profile> --to bifrost`。

预期结果：

- 输出包含 `Bifrost Native Profile Preview`。
- 输出包含 `host suffix example.com -> Proxy`。
- 输出包含 `host suffix ruleset.example -> Proxy`。
- 输出包含 `Compatibility summary`。
- 对存在行为差异或暂不支持的能力以注释或 summary 保留，不静默丢弃。

### TC-SURGE-08：managed profile URL 可直接作为 profile source

操作步骤：

1. 使用 TC-SURGE-02 的本地 HTTP server 提供 `managed.conf`，内容包含 `[Rule]`、`DOMAIN,managed.example,DIRECT` 和 `FINAL,DIRECT`。
2. 执行 `target/debug/bifrost profile effective http://127.0.0.1:<port>/managed.conf`。

预期结果：

- 输出包含 `Source: http://127.0.0.1:<port>/managed.conf`。
- 输出包含 `ManagedProfile`。
- 输出包含 `managed.example`，证明 managed profile URL 已加载为 dry-run runtime plan。

### TC-SURGE-09：自动 E2E 脚本覆盖 CLI 黑盒链路

操作步骤：

1. 执行 `bash e2e-tests/tests/test_profile_surge_bridge_cli.sh`。

预期结果：

- 脚本自动构造临时 Surge profile。
- 脚本自动启动本地 HTTP server，覆盖远程 include、RULE-SET、DOMAIN-SET、managed profile URL 和 cache-hit。
- `profile import --dry-run`、`profile effective`、`profile explain`、Policy Group decision explain、`profile convert` 五类链路均通过断言。
- 测试结束后删除临时目录。

## 清理步骤

- 删除测试中创建的临时目录。
- 不需要停止 Bifrost 服务，因为本用例不启动代理服务。
