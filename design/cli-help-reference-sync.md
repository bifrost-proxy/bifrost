# CLI Help 短链化与快速开始文档

## 背景

`bifrost --help` 的 `SUBCOMMAND REFERENCE` 及之后区块是手写静态文案，不会自动跟随 `clap` 的子命令 help 输出更新。随着 `start`、`traffic`、`remote`、`port` 等命令持续演进，顶层 help 长参考很容易与真实参数漂移，用户也需要翻很长的终端输出才能找到关键文档入口。

本次调整将顶层 help 定位为短入口：保留常用示例、默认行为、`bifrost <command> -h` 提示和关键文档链接；把原 `SUBCOMMAND REFERENCE` 后面的实用内容整理成独立快速开始文档。快速开始不能只是命令转储，需要按真实使用场景组织，让人类和 Agent 先判断目标，再选择命令路径。

## 修复目标

- 从顶层 `bifrost --help` 删除 `SUBCOMMAND REFERENCE` 及之后的长篇手写参考
- help 末尾仅保留关键文档链接，并包含 `docs/cli-quick-start.md` 与 `docs/cli.md`
- 新增 `docs/cli-quick-start.md`，按使用场景承接原 help 中的启停、代理接入、规则改写、TLS 拦截、流量排查、临时端口、远程调用等快速路径
- 刷新 `docs/cli.md`，明确顶层 help 短链化后的命令发现方式和文档入口，并补齐所有顶层命令、主要子命令、环境变量和模板变量的使用说明

## 实现方案

- 更新 `crates/bifrost-cli/src/cli.rs` 的 `after_help`：
  - 保留 `EXAMPLES` 与 `DEFAULT BEHAVIOR`
  - 删除 `SUBCOMMAND REFERENCE`、`ENVIRONMENT VARIABLES`、`RULE TEMPLATE VARIABLES`、`RULES QUICK START`
  - 新增 `TIP`，提示使用 `bifrost <command> -h`
  - `More docs` 链接包含 docs 总览、CLI 快速开始、CLI 详细命令、安装启动、规则语法、操作符、匹配器和规则协议手册
- 将 remote/setting 相关 Clap 类型从 `crates/bifrost-cli/src/cli.rs` 拆到 `crates/bifrost-cli/src/cli/remote.rs` 并 re-export，保持行为不变（当前 `cli.rs` 仍约 2268 行，进一步收敛至 1500 行限制以内为 planned, not yet shipped as of 2026-06-16）
- 新增 `docs/cli-quick-start.md`：
  - 以“本机排障/联调用主服务+多端口、让流量进入代理、域名转本地、HTTPS 路径规则、改请求/响应、流量定位、同一服务服务多个应用/任务、访问控制、远程操作、备份同步、Agent 协作开发业务 Skill”为一级场景
  - 每个场景解释何时使用、命令含义、常见误区和下一步排查入口
  - Agent 协作开发业务 Skill 场景覆盖 `install-skill`、复用主服务、多端口采集、按 host/listener_port/client_app 抓完整或应用专属流量、用 `traffic list/get/search` 理解真实请求响应、给 Agent 的指令模板和专属 skill 输出结构
  - 同一个服务服务多个应用或开发任务场景覆盖常驻主服务、不同任务端口、独立规则绑定、按 `listener_port` 过滤流量、只销毁任务端口而不停止主服务
  - 默认启动建议走 `bifrost start` 自动启用系统代理；TLS 抓包不作为默认启动参数，优先使用规则级 `tlsIntercept://`、域名白名单 `--intercept-include` 或应用白名单 `--app-intercept-include` 按需开启，并说明 SSL pinning 应用应通过 passthrough/exclude 排除；需要 TLS 抓包时由启动流程自动生成并安装 CA，不把 `ca generate/install`、全局 `--intercept`、`--no-system-proxy`、`--unsafe-ssl`、`-k` 或临时 `BIFROST_DATA_DIR` 作为普通起手式
  - 规则 value 示例默认推荐内联 value 或规则文件内嵌值；只有特别大的 body/header/PAC 或需要跨很多规则长期共享的内容才推荐全局 Values，避免人类或 Agent 误以为 `bifrost value set` 是普通起手式
  - 保留命令快捷别名、`BIFROST_DATA_DIR`、`RUST_LOG`、规则快速入门和 curl 验证锚点，兼容既有 human_tests
- 更新 `docs/cli.md`：
  - 新增命令覆盖索引，覆盖当前 `Commands` 中所有顶层命令
  - 新增环境变量手册，区分用户接口、Agent 运行时变量和内部/测试变量边界
  - 新增规则变量速查，区分 `{VALUE}`、`${template}`、捕获组与转义写法
  - 在流量查看与搜索手册中补充 `--client-app` 应用过滤说明，并明确 `install-skill` 只安装 Agent Skill 文档，不启动代理或授予权限
  - 在规则变量速查、Values 管理和 operation 文档中明确全局 Values 的推荐边界：默认优先内联或规则文件内嵌值，特别大的内容才使用全局 Values
  - 补强 `value`、`config`、`admin`、`setting`、`remote`、`im` 等容易误解的命令说明
- 更新 `docs/README.md` 的文档入口

## 测试方案

### 单元/CLI 测试

- `crates/bifrost-cli/tests/cli_help.rs`
- 新增回归测试：`root_help_is_short_and_links_to_docs`
- 验证点：
  - 顶层 `bifrost --help` 不包含 `SUBCOMMAND REFERENCE`
  - 顶层 help 不再展开 `RULE TEMPLATE VARIABLES` / `RULES QUICK START`
  - 顶层 help 包含 `docs/cli-quick-start.md`、`docs/cli.md`、`docs/rule.md`、`docs/operation.md`、`docs/pattern.md`
  - `bifrost start --help` 仍保留完整 `start` 参数说明

### E2E 测试

- 更新 `e2e-tests/tests/test_cli_offline_commands_e2e.sh`
- 新增 `test_root_help_links_docs_instead_of_subcommand_reference`
- 验证顶层 help 短链化、关键链接存在、长参考不存在

### 真实场景测试（human_tests）

- 更新 `human_tests/cli-start-advanced.md`
- 改造 `TC-CSA-32`：验证顶层 `bifrost --help` 不再包含 `SUBCOMMAND REFERENCE`，末尾包含关键文档链接；同时验证 `docs/cli-quick-start.md` 存在并承接常用命令、环境变量、规则快速入门
- 补充 `TC-CSA-32`：验证 `docs/cli-quick-start.md` 采用使用场景组织，且 `docs/cli.md` 包含命令覆盖索引、环境变量和规则变量速查
- 补充 `TC-CSA-32`：验证快速开始包含和 Agent 协作开发业务 Skill 的使用场景、`install-skill -t codex` 安装命令、按 `--client-app` 过滤应用流量、以及明确要求 Agent 基于真实 traffic 证据而不是 mock/猜测
- 补充 `TC-CSA-32`：验证 quick-start、CLI 手册和 operation 手册均不把全局 Values 作为默认推荐，而是优先推荐内联 value 或规则文件内嵌值
- 补充 `TC-CSA-32`：验证 quick-start 和 CLI/getting-started 文档默认推荐系统代理+主服务+多端口；TLS 抓包按需通过规则级、域名白名单或应用白名单开启，SSL pinning 应用应排除，并明确 `ca generate/install`、全局 `--intercept`、`--no-system-proxy`、`--unsafe-ssl`/`-k`/临时数据目录不是默认路径
- 补充 `TC-CSA-32`：验证 quick-start 和 CLI 手册包含同一个 Bifrost 服务服务多个应用/开发任务的场景，并要求 Agent 指令使用 `listener_port` 隔离任务流量
- 同步更新 `human_tests/readme.md` 对 `cli-start-advanced.md` 的说明；用例数量不变

## 校验要求

- `cargo test -p bifrost-cli --test cli_help root_help_is_short_and_links_to_docs -- --exact`
- `cargo test -p bifrost-cli start_all_options_parse -- --exact`
- `cargo test --workspace --all-features`
- `bash e2e-tests/tests/test_cli_offline_commands_e2e.sh`
- `bash scripts/ci/local-ci.sh --skip-e2e`
- 按项目要求在任务收尾前执行 rust-project-validate 规定项

## 文档更新

- 无需更新 README
- 新增 `docs/cli-quick-start.md`
- 更新 `docs/cli.md`
- 更新 `docs/README.md`
- 需要更新 `human_tests/cli-start-advanced.md`
- 需要更新 `human_tests/readme.md`

## 2026-05-09 docs 全量质检补充

### 质检范围

- 覆盖 `docs/*.md`、`docs/rules/*.md`、`docs/superpowers/plans/*.md` 中的文档入口、内部链接、CLI 示例、规则协议说明、过滤器语法、架构 crate 列表和远程调用示例。
- 对照依据包括：
  - `crates/bifrost-cli/src/cli.rs` 与 `crates/bifrost-cli/src/cli/remote.rs` 的 Clap 命令定义。
  - `crates/bifrost-core/src/protocol.rs` 的 74 个规则协议与别名映射。
  - `crates/bifrost-core/src/rule/filter.rs` 与 `crates/bifrost-core/src/rule/resolver.rs` 的过滤器解析和运行时匹配边界。
  - `crates/bifrost-core/src/rule/template.rs` 的模板变量实现。
  - workspace `Cargo.toml` 的当前 crate 成员。

### 修订目标

- 过滤器文档必须只推荐当前运行时真正生效的语法：`m:`、`s:`、`h:`/`reqH:`、`resH:`、`i:`、路径包含/正则和 URL host/path；明确 `b:`/`B:` 当前只被解析但不参与 resolver 匹配。
- 架构文档必须包含当前 workspace crate：`agent`、`bifrost-command`、`bifrost-power`、`skills` 等，避免用户按旧目录理解项目。
- CLI 文档必须补齐当前 help 暴露的能力边界：`traffic list` 的 `client-ip`、rule-hit、WebSocket/SSE/tunnel 过滤；`remote file` 的 stat/glob/hash/edit/mkdir/move/delete；`completions` 的 elvish/powershell。
- 快速开始中的命令必须可独立执行。普通“让流量进入 Bifrost”场景使用主端口 `9900`，临时端口示例只放在已显式 `port bind` 的上下文里。
- 默认访问控制描述必须符合实现：默认 mode 是 `interactive`，loopback 直接允许，非本机地址需要审批或显式放行，而不是简单写成“只允许本机”。
- docs 内部 Markdown 链接必须能解析到真实文件，历史计划文档里指向 human_tests 的链接也必须使用正确相对路径。

### 验证计划

- 单元/静态验证：
  - 链接检查：遍历 `docs/**/*.md` 中的相对 Markdown 链接，确认目标文件存在。
  - 陈旧表述扫描：确认不再出现 `log://`、`includeFilter://p:`、`excludeFilter://p:`、`includeFilter://H:`、`--scope shell`、未绑定临时端口的 `httpbin` 18888 示例等误导性片段。
  - 实现对照扫描：确认 `docs/cli.md` 能发现新增 remote file 示例、补全 shell、默认 `interactive` 访问控制说明；确认 `docs/rules/filters.md` 和 `docs/rule.md` 都声明 body filter 当前边界。
- E2E/CLI 验证：
  - 使用临时 `BIFROST_DATA_DIR` 执行 `cargo run -q -p bifrost-cli -- --help`、`traffic list --help`、`search --help`、`script --help`，确认文档中的命令和参数来自当前 CLI。
- human_tests：
  - 更新 `human_tests/docs-implementation-sync.md` 增加 docs 全量质检用例，并立即按新增用例逐条执行。
  - 同步更新 `human_tests/readme.md` 索引。
- rust-project-validate：
  - docs-only 变更仍执行 `git diff --check`、目标 CLI/doc 检查和可承受的 Cargo 测试；`cargo test --workspace --all-features` 必须至少尝试，若受无关工作区问题阻塞需记录具体失败点。
