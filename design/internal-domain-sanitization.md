# 内部域名脱敏与回归门禁

## 背景

公开仓库的代码、测试夹具、设计说明和人工验证记录中混入了多个企业内部
`bytedance[.]net` 子域。Bifrost 内网登录与 Remote Invoke 仍依赖其中一个
默认 host，但公开仓库无需保存它的明文；其余内部地址也没有保留的必要，测试
夹具还可能误向真实组织域名发起请求。

本次清理只处理公开仓库中的域名引用，不改写 Git 历史，也不处理与域名
无关的组织名称或既有 Provider 持久化标识。

## 用户目标验证清单

### 必须实现

- 清除所有 tracked 文件中的明文内部域名，包括内网登录所需的默认 host。
- 默认 host 只以 Base64 保存，在 Rust、Web 与 Shell 的实际消费点按需解码。
- 将代理、规则、重放、同步和 WebUI 测试夹具改为 RFC 2606 保留域名。
- 将同步 Provider 的受管地址判断从企业域名后缀匹配改为受控默认地址比较。
- 增加可在 CI 中执行的全仓明文扫描，并校验多端 Base64 常量一致且可解码。

### 必须不破坏

- URL 的协议、路径、查询参数、大小写匹配和通配符语义保持不变。
- CLI 帮助、同步登录 URL 拼接、Remote Invoke relay 选择和 Provider 状态结构保持不变。
- 不把测试请求改到另一个可解析的真实企业域名。
- 不改写 Git 历史，不触碰凭据或本机正式 Bifrost 数据目录。

### 必须真实验证

- 动态拼接受限后缀后，`git grep` 对 tracked 文件返回零命中。
- Rust、Web、Shell 使用同一 Base64 值，且能解码出 HTTPS 默认 URL。
- 三类中性夹具域名仍存在，证明没有通过删除测试覆盖来通过门禁。
- 受影响的 Rust、Web 和 CLI 测试继续通过。
- `human_tests/internal-domain-sanitization.md` 中的用例创建后立即执行。

### 必须交付

- 提交代码、测试、设计说明和 human_tests 索引。
- 完成两轮 Review/Fix/Test。
- 推送任务分支、创建 PR，并看护远端 CI 与 coverage 90% 门禁。

## 域名映射

登录域名保留真实运行时语义，但源码只保存 Base64；其余业务夹具位于 IANA
为文档保留的 `example.com` 下：

| 原用途 | 公开夹具 |
| --- | --- |
| 默认同步、内网登录与 Remote Invoke relay | Base64 host，消费时解码 |
| 业务 API 请求与重放 | `api.example.com` |
| Web 应用与规则示例 | `app.example.com` |
| CDN 路径通配匹配 | `cdn.example.com` |

映射只替换 host，不改变协议和 path。例如 CDN 的长路径仍保留，用于覆盖
现有 path wildcard 兼容性；规则中的 PPE header 也不在本次域名清理范围内。

## 实现逻辑

### 代码和夹具

- `bifrost-storage` 使用 `LazyLock<String>` 保存 `DEFAULT_REMOTE_BASE_URL`。初始化器
  从 Base64 解出 host 并拼接 HTTPS scheme，首次读取默认值时才执行。
- CLI 的 Clap help 不再包含编译期明文字面量；构建命令树时读取解码后的默认 URL，
  因而内网用户仍能在 `login --help` 看到可用 token 地址。
- Web API 模块从相同 Base64 值生成默认 URL，Settings fallback 与 Playwright
  fixture 统一引用该运行时常量。
- Shell E2E 通过 `e2e-tests/test_utils/default_remote.sh` 解码，登录与 Remote
  Invoke 测试不再硬编码 host。
- `is_bytedance_remote` 改为中性的 `is_managed_remote`，通过归一化 URL 与
  `DEFAULT_REMOTE_BASE_URL` 相等来识别受管 Provider，不再扫描任意企业域名后缀。

### 文档和历史验证记录

- `design/`、`docs/`、`site/` 与 `human_tests/` 不保存明文；需要显式 relay 的
  命令使用 `${BIFROST_DEFAULT_REMOTE_URL}`，由测试 helper 在运行时赋值。
- 历史验证记录保留原有测试结论、时间、路径和断言，只把敏感 host 改为运行时变量。

### 防回归扫描

新增 `e2e-tests/tests/test_internal_domain_leaks.sh`：

1. 在脚本运行时分段拼接受限后缀，避免门禁脚本自匹配。
2. 使用 `git grep -I` 扫描 tracked 文本文件，任何大小写形式的明文命中都失败。
3. 从共享 Shell helper 解码默认 URL，并断言 Rust、Web、Shell 三处 Base64 一致。
4. 断言三类 `example.com` 夹具仍存在，避免通过删除测试规避门禁。

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

- tracked 文件零明文内部域名命中。
- Base64 默认 host 在运行时成功解码，并被真实 CLI help 消费。
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
内网登录必需域名，因此曾短暂恢复该明文并把验收目标改为单项例外。随后根据
进一步反馈，改为只保存 Base64、运行时解码，最终目标重新收紧为明文零命中；
前一中间方案不作为最终验收依据。

2026-07-16 中间修正第 1 轮结果：曾逐项恢复默认同步、登录、Remote Invoke、CLI、
Web 设置和对应文档中的必需域名，并验证仅有该单项例外。该中间方案已被 Base64
按需解码方案取代，不作为最终验收结果；当时的 Provider、CLI 与 Web 测试结果只
用于说明运行语义已恢复。

2026-07-16 Base64 修正第 1 轮结果：全工作区与 tracked 文本的动态后缀扫描均为
零命中；审查发现 Web 首版在 API 模块加载时立即解码，已改为导出 getter，只在
Settings fallback 或测试读取默认值时解码。Rust Storage、Sync Provider、Admin
Remote Invoke 和两条 CLI 参数/帮助专项均通过，Web 单元测试 162 项全部通过；
真实 `login --help` 与 `sync login --help` 均消费共享 helper 解码出的期望 URL。

### 第 2 轮

- 基于第 1 轮修复后的最新 diff 重新检查默认 URL、Provider 分类、文档索引和测试断言。
- 复跑防回归脚本、human_tests、格式检查和受影响测试。
- 若仍有命中或测试失败，追加下一轮直到无阻塞问题。

2026-07-15 初始结果：独立统计确认基线有 212 次受限后缀命中（211 个完整
子域引用加 1 处后缀分类逻辑），清理后为 0；fixture、可执行位和 human test
索引完整。该结果随后被 2026-07-16 的登录可用性反馈否定，不能作为最终验收结论。

2026-07-16 中间修正第 2 轮结果：重新核对 PR diff、默认 URL、Provider 精确分类、
human test 记录和站点/CLI 文档；该单项明文例外方案随后被 Base64 按需解码方案
取代。最终两轮结果在完成明文零命中与全量复测后追加。

2026-07-16 Base64 修正第 2 轮结果：基于第 1 轮最新 diff 复查 Rust、Web、Shell、
CLI help、公开文档与 human test，工作区（含未跟踪文件）和 tracked 文件动态后缀
扫描均为零命中。Clippy 首次发现 CLI 单元测试模块未导入父模块默认值与路径常量，
已补齐引用；对应 lib/main 单测复跑通过，随后全 targets/features Clippy、完整构建、
workspace/all-features 测试、Web 163 项与 local CI 6 项全部通过，未发现需追加第 3
轮的阻塞问题。

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

2026-07-16 Base64 修正最终结果：E2E 明文门禁与 4 项 human test 全部通过；
workspace/desktop 格式检查、全 targets/features Clippy、完整构建、
`cargo test --workspace --all-features`、Web 35 files/163 tests 和 local CI 6 passed、
0 failed 全部通过。完整 sync-login E2E 通过，运行时解码后的正式 SSO check 返回
预期 HTTP 401，证明 Provider 可达且登录链路没有因脱敏失去默认地址。构建期间产生
的 workspace 版本号 lockfile 漂移已从 diff 移除；本地不运行高成本 coverage，
继续由 PR CI 的 90% 门禁执行。

## 文档更新要求

- 更新所有包含内部域名的设计、CLI、站点源文档和 human_tests 记录；登录及
  Remote Invoke 所需地址只在实际运行时解码。
- 新增 `human_tests/internal-domain-sanitization.md` 并在 `human_tests/readme.md` 增加模块索引。
- 本次不修改 README，因为 README 当前没有相关内部域名或同步 URL 说明。
