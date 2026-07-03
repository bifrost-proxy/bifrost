# CLI Help 短链化与快速开始文档

## 背景

`bifrost --help` 顶层输出历史上把大量参考内容（`SUBCOMMAND REFERENCE`、`ENVIRONMENT VARIABLES`、`RULE TEMPLATE VARIABLES`、`RULES QUICK START`）直接手写在 `after_help` 里。三个问题在长期演进中越来越明显：

- 手写文案不会跟随 clap 的子命令定义漂移。`start`、`traffic`、`remote`、`port`、`install-skill` 等命令持续新增/改名参数后，顶层 help 里的参考区块总是滞后甚至错误。
- 用户和 Agent 打开 `--help` 时先扑面而来几百行未分类的命令罗列，找不到「我到底该走哪条路径」的入口。
- 相同的 CLI 使用场景（本机排障、让流量进入代理、域名转本地、TLS 拦截、流量定位、临时端口、远程调用、Agent 协作等）在 `docs/*.md` 中散落多处，没有一个「场景 → 命令 → 常见误区」的骨干文档。

本次把顶层 help 收敛为短入口，仅保留 EXAMPLES、DEFAULT BEHAVIOR、`bifrost <command> -h` 提示和关键文档链接；把参考区块拆成两份文档：`docs/cli-quick-start.md` 按使用场景组织快速路径，`docs/cli.md` 承担完整命令/环境变量/模板变量手册。同时对 `docs/**/*.md` 做一次全量对齐，把过滤器语法、架构 crate 列表、CLI 能力边界与实际实现拉齐。

## 用户目标验证清单

### 必须实现

- 顶层 `bifrost --help` 不再包含 `SUBCOMMAND REFERENCE`、`ENVIRONMENT VARIABLES`、`RULE TEMPLATE VARIABLES`、`RULES QUICK START` 长参考区块。
- 顶层 `after_help` 只保留 `EXAMPLES`、`DEFAULT BEHAVIOR`、`TIP`（提示 `bifrost <command> -h`）、`More docs`。
- `More docs` 链接必须包含 `docs/README.md`、`docs/cli-quick-start.md`、`docs/cli.md`、`docs/rule.md`、`docs/operation.md`、`docs/pattern.md`、`docs/rules/protocols.md`（规则协议手册）。
- 新增 `docs/cli-quick-start.md`，按使用场景组织；每个场景包含「何时使用 / 命令 / 常见误区 / 下一步排查入口」。
- 刷新 `docs/cli.md`，包含命令覆盖索引、环境变量手册、规则模板变量速查、每个顶层命令的用法要点。
- `docs/README.md` 文档入口指向 quick-start 与 cli 手册。
- 所有 `docs/**/*.md` 内部 Markdown 链接指向真实存在的文件；历史计划文档中指向 `human_tests/` 的链接使用正确相对路径。

### 必须不破坏

- 每个子命令的 `bifrost <cmd> --help` 完整输出保持原有覆盖（`start --help`、`traffic list --help`、`search --help`、`script --help`、`remote file --help` 等）。
- CLI 参数解析行为不变；顶层 help 精简纯粹是 `after_help` 文案调整。
- 已有 human_tests 引用的命令锚点（curl 验证、`BIFROST_DATA_DIR`、`RUST_LOG`）在 quick-start 中保留。
- Clap 命令定义（`Commands` enum、参数、别名）不改语义，只是可能把 remote/setting 相关类型从 `cli.rs` 拆到 `cli/remote.rs`。
- `cargo test -p bifrost-cli --test cli_help` 现有其它用例继续通过。

### 必须真实验证

- 真实执行 `cargo run -q -p bifrost-cli -- --help`，验证长参考区块消失、文档链接齐全。
- 真实执行 `bifrost start --help`、`bifrost traffic list --help`、`bifrost remote file --help`，验证子命令 help 完整。
- 打开 `docs/cli-quick-start.md`、`docs/cli.md`、`docs/README.md`，人工过一遍链接与场景。
- 静态扫描 `docs/**/*.md`：所有相对链接目标存在；不再出现 `log://`、`includeFilter://p:`、`--scope shell`、未绑定端口的 `httpbin` 18888 示例等旧表述。

## 产品语义

### 顶层 help = 入口，不是手册

`bifrost --help` 是用户第一次接触 CLI 的界面，必须在 80×24 终端一屏内让用户知道：

1. 常见操作长什么样（EXAMPLES 5-6 行）。
2. 默认行为是什么（DEFAULT BEHAVIOR 3-5 行）。
3. 每个子命令怎么继续深入（TIP：`bifrost <command> -h`）。
4. 完整手册在哪里（More docs 链接段）。

参考型内容（子命令逐个说明、环境变量、模板变量、规则快速入门）全部下沉到 `docs/*.md`，靠链接引导。

### quick-start = 场景优先，不是命令转储

`docs/cli-quick-start.md` 的组织维度是「用户在做什么」，不是「命令按字母排序」。一级场景：

1. 本机排障 / 联调：主服务 + 多端口。
2. 让流量进入代理：`system-proxy` / `cli-proxy` / 手动 `http_proxy`。
3. 域名转本地 / HTTPS 路径规则。
4. 改请求 / 响应：body、header、status、模板变量。
5. 流量定位：`traffic list/get/search`、`--client-app`、`--host`、`--listener_port`。
6. 同一 Bifrost 服务多个应用 / 开发任务：常驻主服务 + 独立任务端口 + 独立规则绑定。
7. 访问控制：默认 `interactive` mode、loopback 允许、非本机审批。
8. 远程操作：`bifrost remote conn`、`remote file`、`remote exec`。
9. 备份 / 同步：`config export/import`、rule sync。
10. Agent 协作开发业务 Skill：`install-skill -t codex`、复用主服务、按 `--client-app` 抓应用流量、给 Agent 的指令模板。

每个场景讲：**何时使用**（识别信号）、**命令**（可独立执行的最小命令序列）、**常见误区**（例如临时端口示例不能脱离 `port bind` 上下文）、**下一步**（链到 `docs/cli.md`、`docs/rule.md` 对应章节）。

### 默认起手式的边界

- 默认起手式：`bifrost start`（自动启用系统代理），命令行代理走 `--cli-proxy`。
- 非默认起手式：`ca generate/install`、全局 `--intercept`、`--no-system-proxy`、`--unsafe-ssl` / `-k`、临时 `BIFROST_DATA_DIR`；这些是「按需」路径，不放在 quick-start 起手示例里，避免用户/Agent 误以为「必须先装 CA + 关系统代理」。
- TLS 抓包按需通过规则级 `tlsIntercept://`、`--intercept-include` 域名白名单、`--app-intercept-include` 应用白名单开启；SSL pinning 应用通过 passthrough / exclude 排除；需要 CA 时由启动流程自动生成并安装。
- 规则 value 默认推荐**内联 value 或规则文件内嵌值**；只有大 body/header/PAC 或跨规则长期共享的才推荐全局 `bifrost value set`，避免用户以为 `value set` 是普通起手式。

### docs/cli.md = 完整手册

`docs/cli.md` 是命令级 reference：

- 命令覆盖索引：列出当前 clap `Commands` 里全部顶层命令（包括别名）。
- 每个顶层命令：用途 + 主要子命令 + 关键参数 + 典型示例。
- 环境变量手册：区分用户接口变量（`BIFROST_DATA_DIR`、`RUST_LOG`、`HTTPS_PROXY`）、Agent 运行时变量（`BIFROST_AGENT_*`、`BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT`）和内部/测试变量（`BIFROST_DISABLE_TRAY`、`BIFROST_INTERNAL_*`）。
- 规则变量速查：`{VALUE}`、`${template}`、捕获组、转义写法。
- `install-skill` 明确「只安装 Agent Skill 文档，不启动代理不授权」。
- `remote file` 完整覆盖 read/write/edit/patch/mkdir/move/delete/list/find/stat/glob/hash。
- `traffic list` 覆盖 `client-ip`、rule-hit、WebSocket / SSE / tunnel 过滤参数。
- 默认访问控制文案：默认 `interactive`，loopback 直接允许，非本机地址需要审批或显式放行。

## 技术细节

### cli.rs `after_help` 改动

```rust
// crates/bifrost-cli/src/cli.rs
#[command(after_help = "\
EXAMPLES:
    bifrost start                       Start proxy on 9900 with system proxy
    bifrost -p 18888 start              Start proxy on custom port
    bifrost start --cli-proxy           Also set http_proxy env for this shell
    bifrost traffic list                Recent traffic on the main port
    bifrost rule list                   Show local rules
    bifrost remote conn status          Check the current remote connection

DEFAULT BEHAVIOR:
    - Logs go to <data_dir>/logs/ (use --log-output console to also print to terminal).
    - Access control mode defaults to interactive; loopback is auto-allowed.
    - `start` will attempt to enable the system proxy unless --no-system-proxy is passed.

TIP:
    Run `bifrost <command> -h` for full flags and subcommands.

More docs:
    docs/README.md              Docs index
    docs/cli-quick-start.md     Task-oriented quick paths
    docs/cli.md                 Full CLI reference
    docs/rule.md                Rule syntax
    docs/operation.md           Rule operations
    docs/pattern.md             Matchers and filters
    docs/rules/protocols.md     Rule protocol handbook
")]
```

### 拆分 remote / setting Clap 类型

- `crates/bifrost-cli/src/cli/remote.rs`：承载 `Remote*`、`RemoteFile*`、`RemoteTraffic*` 等 Clap struct/enum。
- `crates/bifrost-cli/src/cli/setting.rs`（可选后续）：承载 `Setting*` 相关。
- `cli.rs` re-export，行为不变。
- 当前 `cli.rs` 约 2268 行；目标 <1500 行为 planned，不作为本次强制指标（as of 2026-06-16）。

### 新增文档文件

- `docs/cli-quick-start.md`
- `docs/cli.md`（重写扩充，非首次创建）
- `docs/README.md`（入口更新）

### 相关源文件

- `crates/bifrost-cli/src/cli.rs`：`after_help` 精简 + 类型拆分点。
- `crates/bifrost-cli/src/cli/remote.rs`：新增。
- `crates/bifrost-cli/src/help.rs`：如果 help 组装逻辑分离在这里，同步更新。
- `crates/bifrost-cli/tests/cli_help.rs`：新增回归测试。
- `crates/bifrost-core/src/protocol.rs`：对照 74 个规则协议，同步 `docs/rules/protocols.md`。
- `crates/bifrost-core/src/rule/filter.rs` / `resolver.rs`：对照 filter 语法，同步 `docs/rules/filters.md`。
- `crates/bifrost-core/src/rule/template.rs`：对照模板变量，同步 `docs/cli.md` 规则变量速查。
- workspace `Cargo.toml`：对照当前 crate 成员（`agent`、`bifrost-command`、`bifrost-power`、`skills` 等），同步架构文档。

## CLI + Web + Admin API

### CLI

顶层 help 短链化；子命令 help 不变。行为示意：

```text
$ bifrost --help
Usage: bifrost [OPTIONS] [COMMAND]

Commands:
  start          Start proxy
  stop           Stop proxy
  ...
  help           Print this message

Options:
  -p, --port <PORT>
  --log-output <OUTPUT>
  ...

EXAMPLES:
  ...
DEFAULT BEHAVIOR:
  ...
TIP:
  Run `bifrost <command> -h` for full flags and subcommands.
More docs:
  docs/README.md
  docs/cli-quick-start.md
  docs/cli.md
  ...
```

### Web + Admin

本次纯 CLI + 文档改动；Web UI 和 Admin API 无变更。

## Sync 边界

无 sync 影响。文档和 CLI help 均随二进制发布。

## 实现切分

### Phase 1：顶层 help 短链化

- 改 `cli.rs` `after_help`：删除长参考区块，加入 TIP 和完整 More docs 链接。
- 添加 `crates/bifrost-cli/tests/cli_help.rs` 中 `root_help_is_short_and_links_to_docs` 测试。

### Phase 2：新增 quick-start 文档

- 创建 `docs/cli-quick-start.md`，按 10 个场景组织。
- 覆盖 Agent 协作 Skill 场景：`install-skill -t codex`、复用主服务、按 `--client-app` 过滤、给 Agent 的指令模板、要求 Agent 基于真实 traffic 证据。
- 覆盖同一服务多任务场景：常驻主服务 + 任务端口 + `listener_port` 过滤。
- 默认起手式与 TLS 抓包边界说明。
- Values 边界：默认推荐内联，特别大内容才用全局 Values。

### Phase 3：刷新 `docs/cli.md`

- 命令覆盖索引：对照 clap `Commands` 全量。
- 环境变量手册：三层分类。
- 规则模板变量速查。
- `remote file` / `traffic list` / `install-skill` / `value` / `config` / `admin` / `setting` / `im` 详细说明。
- 默认 `interactive` 访问控制文案。

### Phase 4：docs 全量质检

- 过滤器文档只推荐运行时真正生效的语法：`m:`、`s:`、`h:` / `reqH:` / `resH:`、`i:`、path contains / regex、URL host/path；`b:` / `B:` 明确当前只解析不参与 resolver 匹配。
- 架构文档包含当前 crate：`agent`、`bifrost-command`、`bifrost-power`、`skills`。
- CLI 文档补齐 `completions` elvish/powershell、`traffic list` client-ip / rule-hit / WebSocket / SSE / tunnel 过滤。
- 快速开始命令可独立执行；普通「让流量进入 Bifrost」场景使用主端口 `9900`，临时端口示例只放在已显式 `port bind` 的上下文里。
- docs 内部 Markdown 链接目标存在。
- 更新 `human_tests/docs-implementation-sync.md`，同步 `human_tests/readme.md` 索引。

## 测试方案

### CLI 单元测试

`crates/bifrost-cli/tests/cli_help.rs` 新增：

- `root_help_is_short_and_links_to_docs`：
  - 顶层 `--help` 不包含 `SUBCOMMAND REFERENCE`。
  - 不包含 `RULE TEMPLATE VARIABLES` / `RULES QUICK START`。
  - 包含 `docs/cli-quick-start.md`、`docs/cli.md`、`docs/rule.md`、`docs/operation.md`、`docs/pattern.md`。
- `start_all_options_parse`：现有测试继续通过。
- `bifrost start --help` 仍保留完整 start 参数说明（回归断言）。

### E2E 测试

`e2e-tests/tests/test_cli_offline_commands_e2e.sh` 新增：

- `test_root_help_links_docs_instead_of_subcommand_reference`：验证顶层 help 短链化、关键链接存在、长参考不存在。
- 执行 `cargo run -q -p bifrost-cli -- --help`、`traffic list --help`、`search --help`、`script --help`、`remote file --help`：确认文档中的命令和参数来自当前 CLI。

### 真实场景测试（human_tests）

更新 `human_tests/cli-start-advanced.md`，改造 `TC-CSA-32` 并补充多条子断言：

- `TC-CSA-32-a`：顶层 `--help` 不含 `SUBCOMMAND REFERENCE`，末尾包含关键文档链接；`docs/cli-quick-start.md` 存在并承接常用命令、环境变量、规则快速入门。
- `TC-CSA-32-b`：`docs/cli-quick-start.md` 采用使用场景组织；`docs/cli.md` 包含命令覆盖索引、环境变量和规则变量速查。
- `TC-CSA-32-c`：快速开始包含「和 Agent 协作开发业务 Skill」场景、`install-skill -t codex`、按 `--client-app` 过滤应用流量、明确要求 Agent 基于真实 traffic 证据而不是 mock/猜测。
- `TC-CSA-32-d`：quick-start、CLI 手册和 operation 手册均不把全局 Values 作为默认推荐，而是优先内联或规则文件内嵌值。
- `TC-CSA-32-e`：quick-start 和 CLI/getting-started 文档默认推荐系统代理 + 主服务 + 多端口；TLS 抓包按需通过规则级、域名白名单或应用白名单开启；SSL pinning 应用应排除；`ca generate/install`、全局 `--intercept`、`--no-system-proxy`、`--unsafe-ssl`/`-k`/临时数据目录不是默认路径。
- `TC-CSA-32-f`：quick-start 和 CLI 手册包含「同一个 Bifrost 服务服务多个应用/开发任务」场景；Agent 指令使用 `listener_port` 隔离任务流量。

新增 `human_tests/docs-implementation-sync.md`（如尚无）：docs 全量质检用例；`human_tests/readme.md` 同步索引。用例数量不变（`TC-CSA-*` 只加子断言，不增编号）。

### 覆盖率与项目校验

- `cargo test -p bifrost-cli --test cli_help root_help_is_short_and_links_to_docs -- --exact`
- `cargo test -p bifrost-cli start_all_options_parse -- --exact`
- `cargo test --workspace --all-features`
- `bash e2e-tests/tests/test_cli_offline_commands_e2e.sh`
- `bash scripts/ci/local-ci.sh --skip-e2e`
- `cargo fmt --all -- --check`
- `git diff --check`
- rust-project-validate

docs-only 改动仍需运行 `cargo test --workspace --all-features`；若被无关工作区问题阻塞需记录具体失败点。

## Review/Fix/Test 闭环

### 第 1 轮

- 复核用户目标：短链化、场景化 quick-start、完整 cli.md 手册、docs 全量对齐。
- 复核 diff：`cli.rs` `after_help`、`docs/cli-quick-start.md`、`docs/cli.md`、`docs/README.md`、`docs/**/*.md` 过滤器/架构/CLI 段落、`human_tests/cli-start-advanced.md`、`human_tests/docs-implementation-sync.md`。
- 重点 review：quick-start 是否真正按场景组织（不是命令堆），Values 边界与 TLS 抓包边界是否符合默认起手式约束，链接是否解析到真实文件。
- 复测：`cargo test cli_help root_help_is_short_and_links_to_docs`、`test_root_help_links_docs_instead_of_subcommand_reference`。

### 第 2 轮

- 复查第 1 轮修复：cli.rs 短链化残留检查、link check 扫描、陈旧表述扫描（`log://`、`includeFilter://p:`、`--scope shell`、未绑定的 `httpbin` 18888）。
- 再次运行完整 offline commands E2E、workspace test。
- 检查 human_tests 索引与用例数量一致；rust-project-validate 通过或记录豁免。

## 风险与决策

- 手写 help 再次漂移：quick-start 和 cli.md 也是手写文档，仍有漂移风险。缓解：docs 全量质检用例（`docs-implementation-sync.md`）在每次 CLI 显著变化时执行；关键断言写进 CLI 单测（例如 clap `Commands` 里出现的每个顶层命令名必须出现在 `docs/cli.md` 的命令覆盖索引中，可以后续加自动扫描测试）。
- 顶层 help 太短导致老用户找不到东西：保留 EXAMPLES + DEFAULT BEHAVIOR + TIP + docs 链接，覆盖 90% 起手式；剩余高级路径靠 `bifrost <cmd> -h` 和 quick-start。
- Values 默认边界调整可能与已有用户手册冲突：本次同时更新 `docs/operation.md` 和 quick-start，措辞对齐「优先内联 / 特别大才用全局 Values」。
- Agent 协作场景写在 quick-start 里可能让 Agent 抢占用户视角：quick-start 场景 10 明确标注「Agent 协作开发」，用户视角场景 1-9 保持独立。
- 拆分 remote / setting 类型到子模块的行为不变承诺：只做 mod re-export，不改字段和参数，靠现有 CLI parse 单测兜底；如果有 clap derive 在子模块的边界问题，回退到把 `cli.rs` 保持单文件仅调整 `after_help`（本次强制目标只有短链化 + docs，拆分是 planned 优化）。
