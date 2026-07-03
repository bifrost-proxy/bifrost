# Surge 用户覆盖与 Surge Bridge 真实场景测试

## 功能模块说明

本用例验证 Bifrost 面向 Surge 用户覆盖方案的 Profile Bridge 能力：本地与远程 Surge profile dry-run 导入、兼容性报告、include/ruleset/domain-set resolved plan、managed profile URL、远程资源 ETag/cache、Surge ordered rule explain、Policy Group dry-run decision explain、DNS/MITM/HTTP Pipeline dry-run explain、Bifrost Native Profile conversion preview、Bifrost Native Profile validate/effective dry-run 基座、非 dry-run 保存 disabled Bifrost rule file、URL Rewrite / Map Local 安全转换、未交付 runtime/platform 技术方案、WebUI Profile 工作台，以及方案文档完整性。

## 前置条件

- 仓库位于 `/Users/eden_studio/work/github/bifrost`。
- 使用当前工作区编译出的 `target/debug/bifrost` 或由测试脚本自动执行 `cargo build --bin bifrost`。
- 所有命令使用临时 `BIFROST_DATA_DIR`。
- 设置 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1` 和 `BIFROST_DISABLE_TRAY=1`。
- 本用例默认不启动代理服务，不修改系统代理，不启用 Surge profile；非 dry-run 导入只保存 disabled 规则，除非用例显式传 `--enable`。

## 测试用例列表

### TC-SURGE-01：方案文档包含三迭代产品与技术边界

操作步骤：

1. 执行 `test -f design/surge-user-coverage-product-plan.md`。
2. 执行 `rg -n "迭代一：Surge Bridge|迭代二：Bifrost Native Profile|迭代三：Bifrost Proxy Platform|仍需平台级 runtime|本次落地范围|Managed profile URL|ETag|DNS/MITM/HTTP Pipeline|WebUI 新增 Profile 工作台" design/surge-user-coverage-product-plan.md`。

预期结果：

- 方案文档存在。
- 检索结果能定位三次迭代、平台级 runtime 边界、本次落地范围、managed profile URL、ETag/cache、DNS/MITM/HTTP Pipeline explain 和 WebUI Profile 工作台说明。

### TC-SURGE-02：Surge profile dry-run 导入生成兼容性报告

操作步骤：

1. 创建临时 Surge profile，包含顶层 `#!include`、远程 `#!include`、`[General]`、`[Host]`、`[Proxy]`、`[Proxy Group]`、`[MITM]`、`[URL Rewrite]`、`[Map Local]`、`[Header Rewrite]`、`[Script]`、`[Rule]`。
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

### TC-SURGE-07：profile explain 展示 DNS/MITM/HTTP Pipeline dry-run 决策

操作步骤：

1. 使用 TC-SURGE-02 的临时 profile，其中 `[Host]` 包含 `api.hosted.example = 203.0.113.10`。
2. 确认 `[MITM]` 包含 `hostname = %APPEND% *.example.com, -private.example.com`。
3. 确认 `[URL Rewrite]` 包含 `^https://rewrite\.example/path https://target.example/path 302`。
4. 确认 `[Script]` 包含 `http-response ^https://script\.example script-path=scripts/response.js`。
5. 执行 `target/debug/bifrost profile explain --profile <profile> https://api.hosted.example/path`。
6. 执行 `target/debug/bifrost profile explain --profile <profile> https://private.example.com/path`。
7. 执行 `target/debug/bifrost profile explain --profile <profile> https://rewrite.example/path`。
8. 执行 `target/debug/bifrost profile explain --profile <profile> https://script.example/path`。

预期结果：

- Host mapping 输出包含 `DNS decision: Host mapping api.hosted.example -> 203.0.113.10`。
- MITM exclusion 输出包含 `MITM decision: host private.example.com is excluded from MITM`。
- URL Rewrite 输出包含 `HTTP pipeline: 1 matched` 和 `matched [URL Rewrite]`。
- Script 输出包含 `matched [Script]`。
- 所有 explain 都只做 dry-run 解释，不启用真实 DNS/MITM/HTTP rewrite runtime。

### TC-SURGE-08：profile convert 只输出 Bifrost Native Profile 预览

操作步骤：

1. 使用 TC-SURGE-02 的临时 profile。
2. 执行 `target/debug/bifrost profile convert <profile> --to bifrost`。

预期结果：

- 输出包含 `Bifrost Native Profile Preview`。
- 输出包含 `host suffix example.com -> Proxy`。
- 输出包含 `host suffix ruleset.example -> Proxy`。
- 输出包含 `Compatibility summary`。
- 对存在行为差异或暂不支持的能力以注释或 summary 保留，不静默丢弃。

### TC-SURGE-09：managed profile URL 可直接作为 profile source

操作步骤：

1. 使用 TC-SURGE-02 的本地 HTTP server 提供 `managed.conf`，内容包含 `[Rule]`、`DOMAIN,managed.example,DIRECT` 和 `FINAL,DIRECT`。
2. 执行 `target/debug/bifrost profile effective http://127.0.0.1:<port>/managed.conf`。

预期结果：

- 输出包含 `Source: http://127.0.0.1:<port>/managed.conf`。
- 输出包含 `ManagedProfile`。
- 输出包含 `managed.example`，证明 managed profile URL 已加载为 dry-run runtime plan。

### TC-SURGE-10：自动 E2E 脚本覆盖 CLI 黑盒链路

操作步骤：

1. 执行 `bash e2e-tests/tests/test_profile_surge_bridge_cli.sh`。

预期结果：

- 脚本自动构造临时 Surge profile。
- 脚本自动启动本地 HTTP server，覆盖远程 include、RULE-SET、DOMAIN-SET、managed profile URL 和 cache-hit。
- `profile import --dry-run`、`profile effective`、`profile explain`、Policy Group decision explain、DNS/MITM/HTTP Pipeline explain、`profile convert` 五类链路均通过断言。
- 非 dry-run 导入后的 disabled rule file 包含 URL Rewrite `redirect://`、Map Local `file://`，并保留 Header Rewrite / Script 人工 review 注释。
- 测试结束后删除临时目录。

### TC-SURGE-11：非 dry-run import 保存 disabled Bifrost rule file

操作步骤：

1. 创建临时 `BIFROST_DATA_DIR` 和 Surge profile，内容包含 `[Proxy] ProxyA = http, 127.0.0.1, 8080`、`[Proxy Group] Proxy = select, ProxyA, DIRECT`、`DOMAIN,api.example.com,DIRECT`、`DOMAIN-SUFFIX,example.com,Proxy`、`FINAL,REJECT`。
2. 执行 `BIFROST_DATA_DIR=<tmp>/data BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 BIFROST_DISABLE_TRAY=1 target/debug/bifrost profile import <profile> --name profile/surge-smoke`。
3. 执行 `BIFROST_DATA_DIR=<tmp>/data target/debug/bifrost rule show profile/surge-smoke`。

预期结果：

- import 输出包含 `Saved Bifrost rule 'profile/surge-smoke' [disabled for review]`。
- `rule show` 输出 `Status: disabled`。
- 规则内容包含 `api.example.com passthrough://`。
- 规则内容包含 `*.example.com proxy://http://127.0.0.1:8080`。
- 规则内容包含 `/.*/ statusCode://403`。
- 本用例不启动代理服务、不修改系统代理、不把生成规则默认启用。

### TC-SURGE-12：HTTP Pipeline 安全子集导入为可审阅规则

操作步骤：

1. 创建临时 `BIFROST_DATA_DIR` 和 Surge profile，内容包含 `[URL Rewrite] ^https://rewrite\.example/path https://target.example/path 302`、`[Map Local] ^https://assets\.example/app\.js data/app.js`、`[Header Rewrite] ^https://headers\.example header-replace User-Agent Bifrost`、`[Script] http-response ^https://script\.example script-path=scripts/response.js`、`[Rule] FINAL,DIRECT`。
2. 执行 `BIFROST_DATA_DIR=<tmp>/data BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 BIFROST_DISABLE_TRAY=1 target/debug/bifrost profile import <profile> --name profile/surge-pipeline-smoke`。
3. 执行 `BIFROST_DATA_DIR=<tmp>/data target/debug/bifrost rule show profile/surge-pipeline-smoke`。

预期结果：

- `rule show` 输出 `Status: disabled`。
- 规则内容包含 `/^https:\/\/rewrite\.example\/path/ redirect://302:https://target.example/path`。
- 规则内容包含 `/^https:\/\/assets\.example\/app\.js/ file://data/app.js`。
- 规则内容包含 `Header Rewrite requires request/response/header-scope review before activation`。
- 规则内容包含 `Script entries reference external JavaScript that must be imported into Bifrost scripts before activation`。
- 本用例不启动代理服务、不修改系统代理、不自动启用 Header Rewrite 或 Script。

### TC-SURGE-13：WebUI Profile 工作台展示兼容性与决策时间线

操作步骤：

1. 使用临时数据目录从源码启动 Bifrost：`BIFROST_DATA_DIR=<tmp>/data BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 BIFROST_DISABLE_TRAY=1 cargo run -p bifrost-cli --bin bifrost -- start --host 127.0.0.1 --port <port> --no-system-proxy --skip-cert-check`。
2. 在浏览器打开 `http://127.0.0.1:<port>/_bifrost/`。
3. 点击侧边栏 `Profile`。
4. 在 Profile 页面保留默认示例或粘贴 TC-SURGE-11 的 profile。
5. 点击 `Analyze`。
6. 切换 `Compatibility`、`Runtime Plan`、`Explain`、`Resources`、`Native Preview` 标签。

预期结果：

- 页面 URL 进入 `/profile`。
- 兼容性汇总显示 `Fully Supported`、`Behavior Notes`、`Manual Review` 和 `Not Supported` 四项。
- Runtime Plan 显示 mode、rules、policy groups、proxies、DNS 和 HTTP Pipeline 数量。
- Explain 标签展示包含 rule/policy/MITM 等 stage 的 timeline。
- Native Preview 显示 `Bifrost Native Profile Preview`。
- 启动命令包含 `--no-system-proxy`，且本用例不启用系统代理。

### TC-SURGE-14：未交付 runtime/platform 能力技术方案完整性

操作步骤：

1. 执行 `test -f design/surge-runtime-platform-roadmap.md`。
2. 执行 `rg -n "迭代二：Bifrost Native Profile 推进方案|迭代三：Bifrost Proxy Platform 推进方案|Native Profile schema|Platform device registry|动态 Policy Group Runtime|真实 DNS / MITM / Rewrite / Script Runtime|Transparent Proxy / TUN / VIF|UDP / QUIC / HTTP3 Scheduling|Team Profile|Agent 自动迁移|RuntimePlanVersion|Decision Timeline|Milestone A|Milestone E|评审问题" design/surge-runtime-platform-roadmap.md`。

预期结果：

- 技术方案文档存在。
- 检索结果覆盖迭代二 Native Profile 推进方案、迭代三 Proxy Platform 推进方案、六大未交付能力、共同 runtime 基础、Decision Timeline、里程碑拆分和待评审问题。
- 文档明确这些能力为未交付设计，不把平台级 runtime 写成已上线能力。

### TC-SURGE-15：Bifrost Native Profile validate/effective 生成 dry-run RuntimePlanVersion

操作步骤：

1. 创建临时 `native.bifrost-profile.toml`，包含 `[profile]`、`[[policies]]` proxy、`[[policy_groups]]` url-test、`[[rules]]` domain/domain_suffix/final、`[dns]`、`[dns.hosts]`、`[mitm]` 和 `[[http_pipeline]]`。
2. 执行 `BIFROST_DATA_DIR=<tmp>/data BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 BIFROST_DISABLE_TRAY=1 target/debug/bifrost profile native validate <native-profile>`。
3. 执行 `BIFROST_DATA_DIR=<tmp>/data BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 BIFROST_DISABLE_TRAY=1 target/debug/bifrost profile native effective <native-profile>`。

预期结果：

- validate 输出包含 `Bifrost Native Profile validate`。
- validate 输出包含 `Plan: sha256:`、`Source hash: sha256:` 和 `Mode: bifrost-native-dry-run`。
- validate 输出包含 `Runtime plan: 1 proxies, 1 policy groups, 3 rules, 2 dns entries, 2 mitm entries, 1 pipeline entries`。
- 当 policy group 引用 `MissingProxy` 时，validate 输出包含 `native.policy_group.missing_member`，且 safety 标记为 `dry-run-only`。
- effective 输出包含 `Bifrost Native Profile effective`、`Policies`、`Policy graph`、`Ordered rules`、`DNS entries`、`MITM entries` 和 `HTTP Pipeline entries`。
- 本用例不启动代理服务、不修改系统代理、不启用 DNS/MITM/TUN runtime。

## 清理步骤

- 删除测试中创建的临时目录。
- 如果执行了 TC-SURGE-13，停止对应临时 Bifrost 进程。
