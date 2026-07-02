# PAC Proxy Auto-Config 方案真实场景测试

## 功能模块说明

验证 PAC Proxy Auto-Config 方案文档是否覆盖用户提出的语法、值来源、filters、Final URL、`enable://proxyHost`、系统代理不变量、测试计划和分阶段落地路径。本用例面向设计阶段，不启动 Bifrost 服务、不修改系统代理。

## 前置条件

- 在仓库根目录执行。
- 当前分支基于最新 `origin/main`。
- 已新增 `design/pac-proxy-auto-config.md`。
- 本轮仅验证设计文档与索引，不验证未实现的 PAC runtime 行为。

## 测试用例列表

### TC-PAC-DESIGN-01：方案覆盖 PAC 规则语法和值来源

操作步骤：

1. 执行：
   ```bash
   rg -n "pattern pac://value|内嵌 PAC|Values|本地文件|远程 PAC|pac://\\{test|pac:///Users|raw.githubusercontent.com" design/pac-proxy-auto-config.md
   ```
2. 检查输出是否包含用户要求的内嵌、Values、本地文件和远程 URL 示例。

预期结果：

- 命令 0 退出。
- 输出包含 `pattern pac://value`、`pac://{...}`、`pac:///Users/...` 和远程 `https://...pac` 示例。

### TC-PAC-DESIGN-02：方案明确系统代理不变量

操作步骤：

1. 执行：
   ```bash
   rg -n "不读取系统代理|HTTP_PROXY|HTTPS_PROXY|通过 Bifrost rules 显式表达|不影响 Sync|runtime outbound" design/pac-proxy-auto-config.md
   ```

预期结果：

- 命令 0 退出。
- 方案明确 Bifrost 代理核心自身出站 client 不隐式读取系统代理。
- 方案明确 PAC 只作用于命中规则的被代理请求。

### TC-PAC-DESIGN-03：方案覆盖 Final URL 二阶段语义

操作步骤：

1. 执行：
   ```bash
   rg -n "Final URL|第一阶段|第二条 PAC|www.example.com/api|www.example.com/path|不会递归" design/pac-proxy-auto-config.md
   ```

预期结果：

- 命令 0 退出。
- 方案说明 PAC 对规则替换后的 Final URL 生效，Final URL 为空时回退原始 URL。
- 方案说明单条 rewrite + pac 规则不应递归二次命中。

### TC-PAC-DESIGN-04：方案覆盖 `enable://proxyHost`

操作步骤：

1. 执行：
   ```bash
   rg -n "enable://proxyHost|proxy_host_override|ProxyHostOverride|CONNECT override_host|1.1.1.1:8080" design/pac-proxy-auto-config.md
   ```

预期结果：

- 命令 0 退出。
- 方案包含用户给出的 `1.1.1.1 enable://proxyHost` 与 `1.1.1.1:8080 enable://proxyHost` 示例。
- 方案说明该能力只在 PAC 返回上游代理时生效。

### TC-PAC-DESIGN-05：方案覆盖 PAC 执行器、安全限制和 helper

操作步骤：

1. 执行：
   ```bash
   rg -n "rquickjs|FindProxyForURL|dnsDomainIs|shExpMatch|isInNet|执行超时|脚本最大|fail-closed|pac_elapsed_ms" design/pac-proxy-auto-config.md
   ```

预期结果：

- 命令 0 退出。
- 方案包含 PAC 专用执行器、常见 helper、超时、大小限制、fail-closed 和 Traffic 诊断字段。

### TC-PAC-DESIGN-06：方案覆盖测试与 Review/Fix/Test 闭环

操作步骤：

1. 执行：
   ```bash
   rg -n "单元测试|E2E|human_tests|coverage 90%|Review/Fix/Test|Phase 1|Phase 2|Phase 3" design/pac-proxy-auto-config.md
   ```

预期结果：

- 命令 0 退出。
- 方案包含单元测试、E2E、human_tests、coverage 门禁、两轮 Review/Fix/Test 和分阶段落地。

### TC-PAC-DESIGN-07：human_tests 索引已同步

操作步骤：

1. 执行：
   ```bash
   rg -n "pac-proxy-auto-config-design.md|PAC Proxy Auto-Config" human_tests/readme.md human_tests/pac-proxy-auto-config-design.md
   ```

预期结果：

- 命令 0 退出。
- `human_tests/readme.md` 包含 PAC 方案测试索引行。
- 本文件标题和索引模块名称一致。

## 清理步骤

- 本用例只执行只读检查命令，无需清理服务、端口、系统代理或临时数据目录。

## 执行记录

| 日期 | 用例 | 结果 | 证据 |
| --- | --- | --- | --- |
| 2026-07-02 | TC-PAC-DESIGN-01..07 | 通过 | 已逐条执行 7 个 `rg -n` 检查命令，均 0 退出；输出分别命中 PAC 语法和值来源、系统代理不变量、Final URL 二阶段语义、`enable://proxyHost`、PAC 执行器安全限制、测试计划与 `human_tests/readme.md` 索引行。 |
| 2026-07-02 | TC-PAC-DESIGN-01..07 | 通过 | Review/Fix/Test 第 1 轮修复 Markdown 示例围栏和设计阶段 human_tests 说明后，第二轮完整复跑 7 个检查命令，均 0 退出。 |
