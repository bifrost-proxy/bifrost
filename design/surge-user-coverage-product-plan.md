# Surge 用户覆盖产品方案与三迭代技术方案

## 状态

本文描述 Bifrost 面向 Surge 用户群体的产品路线和分阶段技术拆解。

当前已启动迭代一 Surge Bridge 的第一条可合入纵向切片：

- `bifrost_core::profile` 提供 Profile IR、Surge profile parser、兼容性报告、ordered rule explain 和 Bifrost native profile preview。
- `bifrost profile import <file> --dry-run` 只解析、分析和展示报告，不启用 profile。
- `bifrost profile explain --profile <file> <url>` 按 Surge `[Rule]` top-to-bottom first-match 语义解释 DNS/Rule/Policy/MITM 决策摘要。
- `bifrost profile convert <file> --to bifrost` 生成带行为说明的 Bifrost Native Profile 预览，不写入运行时。

尚未实现：

- managed profile URL、远程 `#!include` 拉取和 ETag/Last-Modified/cache。
- Surge ordered evaluator 接入真实代理运行时。
- Policy Group runtime、DNS Center、MITM Center、HTTP Pipeline runtime。
- Transparent Proxy / TUN / VIF、UDP/QUIC/HTTP3 policy scheduling、Team Profile 和 Agent 自动迁移。

## 北极星

Bifrost 的目标不是只做开发调试工具，而是成为全行业最强的代理软件。Agent、Remote、ASR、流量分析、脚本和团队协作都服务于这个核心目标。

面向 Surge 用户群体的产品主张：

```text
Bring your Surge profile. Get a stronger proxy workbench.
```

中文表达：

```text
配置不用重写，能力直接升级。
```

## 三步产品策略

1. 承接：Surge 用户已有 `.conf`、`.dconf`、`.sgmodule`、managed profile、ruleset 和 MITM 配置可以被导入、理解、解释和逐步迁移。
2. 替代：提供 Bifrost Native Profile、Policy、DNS、MITM、HTTP Pipeline 和 Traffic Workbench，满足日常代理使用。
3. 反超：用可观测、可协作、可自动化、可远程诊断的代理平台能力形成代差。

## 迭代一：Surge Bridge

目标是建立迁移信任。用户可以把已有 Surge Profile 带进 Bifrost，并立即看到配置能否被理解、哪些行为可等价执行、哪些地方需要人工确认。

### 本次落地范围

- 本地 Surge profile dry-run 导入。
- Profile IR 保留 source、raw text、section、entry、line、column、comment 和 diagnostics。
- Parser 支持 `[General]`、`[Proxy]`、`[Proxy Group]`、`[Rule]`、`[Host]`、`[MITM]`、`[URL Rewrite]`、`[Map Local]`、`[Header Rewrite]`、`[Script]` 与 directive 行。
- Compatibility Report 按 `Fully supported`、`Translated with behavior note`、`Needs manual review`、`Not supported yet` 分类。
- Explain 支持 `DOMAIN`、`DOMAIN-SUFFIX`、`DOMAIN-KEYWORD`、`IP-CIDR`、`IP-CIDR6` 和 `FINAL` 的 Surge ordered first-match 解释。
- Convert 输出 Bifrost Native Profile preview，把不支持或有行为差异的条目保留为注释。

### 后续迭代一补齐

- managed profile URL 下载。
- 本地和远程 `#!include` 解析。
- `RULE-SET` / `DOMAIN-SET` 远程资源加载与缓存。
- WebUI Import 页面。
- 更完整的 HTTP pipeline 迁移预览。

## 迭代二：Bifrost Native Profile

目标是让 Surge 用户留下来，完成日常替代 Surge。

核心能力：

- 原生 Bifrost Profile 文件格式、include、module、local override、managed profile 和 effective profile dump。
- Policy / Policy Group：`direct`、`reject`、`proxy`、`select`、`fallback`、`url-test`。
- DNS Center：system/custom/per-domain DNS、local host mapping、DNS cache、flush、explain。
- MITM Center：host/app/client/device scope、证书信任状态、QUIC fallback/block。
- HTTP Pipeline：URL/header/body rewrite、map local/mock、request/response script、decode/parser。
- Traffic Decision Timeline：每条请求记录 DNS、Rule、Policy、MITM、rewrite/script 和 upstream 决策链。

## 迭代三：Bifrost Proxy Platform

目标是形成代差，Bifrost 不只兼容 Surge，而是在透明代理、智能策略、多设备、团队协作、远程诊断和 Agent 自动化上成为更强标准。

核心能力：

- Transparent Proxy / TUN / VIF。
- UDP / QUIC / HTTP3 代理和诊断。
- Smart Policy Group 与 Adaptive Policy Scheduler。
- Multi-device Proxy Fabric。
- Team Profile 的 revision、diff、approval、rollback、audit 和 rollout target。
- Agent Automation：自动迁移配置、解释请求未走代理原因、生成规则并验证、远程设备抓包复现。

## 风险与缓解

### Surge 语义漂移

风险：Surge `[Rule]` 是 ordered first-match，Bifrost 现有规则体系有 priority/pattern 行为，直接转换可能改变路由。

缓解：

- Surge-compatible mode 必须使用 ordered evaluator。
- 转换为 Bifrost-native 前展示行为差异。
- `profile explain` 作为迁移验收入口。

### 兼容范围过大

风险：一次承诺完整 Surge 生态会导致实现失控。

缓解：

- 迭代一先交付 dry-run、report、explain 和基础转换预览。
- 不支持项进入 report，禁止静默跳过。

### 网络能力高影响

风险：MITM、TUN、系统代理和 DNS 都可能影响用户网络。

缓解：

- 高影响能力默认 dry-run。
- 所有开关提供 scope、explain、rollback。
- 开发、E2E 和 human_tests 启动服务必须遵守 `--no-system-proxy`、`BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`、`BIFROST_DISABLE_TRAY=1`，除非测试目标明确覆盖这些能力。

## 测试方案

### 单元测试

- Surge section parser。
- directive parser。
- rule parser 和 line number diagnostics。
- compatibility analyzer 支持等级分类。
- ordered first-match explain。
- conversion preview 行为说明。

### E2E 测试

- `e2e-tests/tests/test_profile_surge_bridge_cli.sh` 构造真实 Surge profile。
- 验证 `profile import --dry-run` 输出兼容报告。
- 验证 `profile explain` 命中 ordered `DOMAIN-SUFFIX` 并输出 policy。
- 验证 `profile convert --to bifrost` 输出 preview 和 compatibility summary。

### human_tests

- `human_tests/surge-user-coverage-product-plan.md` 覆盖文档完整性、CLI dry-run、ordered explain、convert preview 和不启用运行时的安全边界。

## Review/Fix/Test 闭环

第 1 轮：

- 复核用户目标、本文档、本次代码变更和测试计划。
- 执行 `git status --short`、`git diff`。
- review parser/compat/explain/CLI 输出边界。
- 运行 targeted unit、CLI check、E2E 和 human_tests 检索/命令验证。

第 2 轮：

- 复核第 1 轮发现项和修复后的 diff。
- 检查 human_tests 索引、E2E 脚本、CLI help、JSON 输出。
- 复跑受影响测试。

## 本地覆盖率说明

本次会改业务代码，但当前 Bifrost 记忆规则要求不运行本地 `make coverage` / `make coverage-unit` / `scripts/ci/coverage-all.sh`，避免本机覆盖率脚本干扰。覆盖率门禁证据交给远端 CI；本地以 targeted unit、E2E、workspace test/build/clippy 和 human_tests 作为主要验证。
