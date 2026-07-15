# 内部域名脱敏与回归门禁

## 背景

公开仓库的代码、测试夹具、设计说明和人工验证记录中混入了多个企业内部
`bytedance[.]net` 子域。除 Bifrost 内网登录与 Remote Invoke 必需的
`bifrost[.]bytedance[.]net` 外，其余地址没有公开仓库保留的必要，测试夹具
也可能误向真实组织域名发起请求。

本次清理只处理公开仓库中的域名引用，不改写 Git 历史，也不处理与域名
无关的组织名称或既有 Provider 持久化标识。

## 用户目标验证清单

### 必须实现

- 仅保留内网登录必需的 `bifrost[.]bytedance[.]net`，清除其他真实内部域名。
- 将代理、规则、重放、同步和 WebUI 测试夹具改为 RFC 2606 保留域名。
- 将同步 Provider 的受管地址判断从企业域名后缀匹配改为受控默认地址比较。
- 增加可在 CI 中执行的全仓精确 allowlist 扫描，防止后续引入其他内部域名。

### 必须不破坏

- URL 的协议、路径、查询参数、大小写匹配和通配符语义保持不变。
- CLI 帮助、同步登录 URL 拼接、Remote Invoke relay 选择和 Provider 状态结构保持不变。
- 不把测试请求改到另一个可解析的真实企业域名。
- 不改写 Git 历史，不触碰凭据或本机正式 Bifrost 数据目录。

### 必须真实验证

- `git grep` 提取出的内部域名都精确等于唯一允许的登录域名。
- 必需登录域名和三类中性夹具域名均存在，证明没有通过删除功能或测试来通过门禁。
- 受影响的 Rust、Web 和 CLI 测试继续通过。
- `human_tests/internal-domain-sanitization.md` 中的用例创建后立即执行。

### 必须交付

- 提交代码、测试、设计说明和 human_tests 索引。
- 完成两轮 Review/Fix/Test。
- 推送任务分支、创建 PR，并看护远端 CI 与 coverage 90% 门禁。

## 域名映射

登录域名保留真实默认值；其余业务夹具位于 IANA 为文档保留的
`example.com` 下：

| 原用途 | 公开夹具 |
| --- | --- |
| 默认同步、内网登录与 Remote Invoke relay | `bifrost.bytedance.net`（保留） |
| 业务 API 请求与重放 | `api.example.com` |
| Web 应用与规则示例 | `app.example.com` |
| CDN 路径通配匹配 | `cdn.example.com` |

映射只替换 host，不改变协议和 path。例如 CDN 的长路径仍保留，用于覆盖
现有 path wildcard 兼容性；规则中的 PPE header 也不在本次域名清理范围内。

## 实现逻辑

### 代码和夹具

- Rust 单元测试、CLI 集成测试、Shell E2E、Web Vitest/Playwright fixture 中，
  只有真实登录链路保留默认域名，普通业务请求使用上述文档保留域名。
- `DEFAULT_REMOTE_BASE_URL` 使用 `https://bifrost.bytedance.net`，保留现有默认值形状，避免把这次安全清理扩展成同步配置协议迁移。
- `is_bytedance_remote` 改为中性的 `is_managed_remote`，通过归一化 URL 与
  `DEFAULT_REMOTE_BASE_URL` 相等来识别受管 Provider，不再扫描任意企业域名后缀。

### 文档和历史验证记录

- `design/`、`docs/`、`site/` 与 `human_tests/` 中的地址统一按用途处理；登录与
  Remote Invoke 操作文档保留可工作的默认域名，普通 API/CDN/Web 示例使用中性域名。
- 历史验证记录保留原有测试结论、时间、路径和断言，只对非必要敏感 host 做脱敏。

### 防回归扫描

新增 `e2e-tests/tests/test_internal_domain_leaks.sh`：

1. 在脚本运行时分段拼接受限后缀与唯一允许的 host，避免扫描规则自匹配。
2. 使用 `git grep -I` 从 tracked 文本文件提取所有以该后缀结尾的域名，忽略
   大小写后逐个精确比较；只有登录域名通过，嵌套子域或其他服务名都会失败。
3. 断言必需登录域名与三类 `example.com` 夹具仍然存在，避免通过删除功能、
   测试或文档规避门禁。

## 测试方案

### 单元测试

- `bifrost-sync`：受管 Provider URL 分类和默认同步配置。
- `bifrost-core`、`bifrost-admin`、`bifrost-cli`、`bifrost-proxy`、`bifrost-script`：
  运行包含替代域名的现有 URL、规则、重放和 CLI 测试。
- Web：运行受影响的规则有效性和 Settings Sync 测试。

### E2E

- 执行 `bash e2e-tests/tests/test_internal_domain_leaks.sh`。
- 执行 shell CI coverage guard，确认新脚本被统一入口收集。

### 真实场景测试

新增 `human_tests/internal-domain-sanitization.md`，逐项执行：

- tracked 文件只命中唯一允许的内部登录域名。
- 非登录夹具均属于 `example.com`，且关键路径/规则测试仍存在。
- CLI help 构建输出展示真实可用的默认 token 登录地址。
- 运行 E2E 防回归脚本并核对退出码。

## Review/Fix/Test 闭环

### 第 1 轮

- 对照用户目标和映射表检查 `git status --short`、`git diff`、新增文件。
- 扫描大小写、隐藏 tracked 文件、Rust/TypeScript/Shell/Markdown 全部文本。
- 运行防回归脚本和受影响模块专项测试，修复所有语义偏差。

2026-07-15 初始结果：全工作区动态后缀扫描与 E2E 门禁均为零命中；审查发现
capture 测试的 URL host 已改为 `api.example.com`，但 `host_contains` 仍保留旧服务短名，
导致测试不能命中，已同步改为 `api` 并通过 Admin 专项与全量测试。另将构建过程因
上游版本 bump 自动产生、与本次需求无关的 `Cargo.lock` 版本漂移从 diff 中移除。
Web 单元测试 162 项全部通过；直接受夹具替换影响的 9 个 Playwright 场景通过。
3 个更宽泛的 Sync 场景在未改动的不可达状态与轮询断言处稳定暴露现有状态问题，
不在本次安全清理 PR 中扩大修复范围。2026-07-16 收到反馈后确认零命中目标误删了
内网登录必需域名，因此恢复该域名并将验收目标修正为精确 allowlist。

2026-07-16 修正第 1 轮结果：逐项恢复默认同步、登录、Remote Invoke、CLI、Web
设置和对应文档中的必需域名；全仓提取后的内部 host 集合只有 allowlist 项。
审查发现首版正则未覆盖带下划线的异常服务标签，已补齐并用普通、嵌套、大小写
和下划线探针验证。防回归 E2E、Provider 专项、CLI 主测试（1210 passed，
2 ignored）及其集成测试和 Web 单元测试（162 passed）均通过。

### 第 2 轮

- 基于第 1 轮修复后的最新 diff 重新检查默认 URL、Provider 分类、文档索引和测试断言。
- 复跑防回归脚本、human_tests、格式检查和受影响测试。
- 若仍有命中或测试失败，追加下一轮直到无阻塞问题。

2026-07-15 初始结果：独立统计确认基线有 212 次受限后缀命中（211 个完整
子域引用加 1 处后缀分类逻辑），清理后为 0；fixture、可执行位和 human test
索引完整。该结果随后被 2026-07-16 的登录可用性反馈否定，不能作为最终验收结论。

2026-07-16 修正第 2 轮结果：重新核对最终 PR diff、默认 URL、Provider 精确分类、
human test 记录和站点/CLI 文档；`bifrost.example.com` 为零命中，内部 host 去重
结果仅为 `bifrost.bytedance.net`。格式检查、全 targets/features Clippy、完整构建、
`cargo test --workspace --all-features` 与 `local-ci.sh --skip-e2e` 均通过，未发现
新的语义偏差。

## 校验要求

- 先完成 E2E 与 human_tests，再执行 `rust-project-validate`。
- 最终执行 `cargo fmt --all -- --check`、desktop fmt、clippy、相关构建、
  `cargo test --workspace --all-features` 和按范围选择的 local-ci。
- 本地不运行高成本全量 coverage；由 PR CI 的 90% coverage gate 兜底。

2026-07-15 实际结果：workspace 与 desktop 格式检查、全 workspace/all-targets/
all-features Clippy、全量构建、`cargo test --workspace --all-features` 均通过；
`scripts/ci/local-ci.sh --skip-e2e` 的 6 项静态/测试/依赖审计门禁全部通过。
Coverage 按仓库约定交由 PR CI 执行。

2026-07-16 修正后复测结果：再次通过两项格式检查、全 targets/features Clippy、
完整构建和 workspace/all-features 测试；统一 local CI 6 passed、0 failed，依赖
审计只有仓库既有的 duplicate/future-compat/deprecation 警告，退出码为 0。

## 文档更新要求

- 更新所有包含非必要内部域名的设计、CLI、站点源文档和 human_tests 记录，
  同时保留登录及 Remote Invoke 所需的默认地址。
- 新增 `human_tests/internal-domain-sanitization.md` 并在 `human_tests/readme.md` 增加模块索引。
- 本次不修改 README，因为 README 当前没有相关内部域名或同步 URL 说明。
