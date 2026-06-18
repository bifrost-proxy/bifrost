# Docs Implementation Sync 测试用例

## 功能模块说明

验证 `docs/` 中的 CLI、Scripts、规则协议说明与当前工程实现保持一致。重点覆盖当前源码 `bifrost --help` 暴露的命令表面、Scripts parser 类型的真实 API/运行时支持、规则协议索引中 `bp` / `devtools` / `upstreamUnsafeSsl` 的可发现性，以及规则语法示例与当前 parser/template 行为一致。

## 前置条件

- 工作目录为仓库根目录 `~/work/github/bifrost`。
- 执行任何 shell 命令前先运行 `source ~/.zshrc`。
- 不启动正式 9900 测试服务；涉及 CLI help 的命令使用临时 `BIFROST_DATA_DIR="$(mktemp -d)"`。
- 不修改系统代理。

## 测试用例列表

### TC-DIS-01 CLI 顶层命令文档覆盖当前 help

操作步骤：

1. 执行 `source ~/.zshrc && BIFROST_DATA_DIR="$(mktemp -d)" cargo run --bin bifrost -- --help`。
2. 确认输出包含 `restart`、`setting`、`remote`、`keep-awake`、`im`。
3. 打开 `docs/cli.md`，确认这些命令均有示例或说明。

预期结果：

- 顶层 help 中存在上述 5 个命令。
- `docs/cli.md` 中同步存在上述 5 个命令的用户可执行示例。

### TC-DIS-02 Scripts parser 类型说明符合当前 API/CLI 边界

操作步骤：

1. 执行 `source ~/.zshrc && rg -n "parser: Vec|ScriptType::Parser|parserScripts|ScriptType = 'request' \\| 'response' \\| 'decode' \\| 'parser'" crates/bifrost-admin/src/handlers/scripts.rs crates/bifrost-script/src/types.rs web/src/api/scripts.ts web/src/pages/Scripts/index.tsx`。
2. 执行 `source ~/.zshrc && BIFROST_DATA_DIR="$(mktemp -d)" cargo run --bin bifrost -- script add --help`。
3. 打开 `docs/scripts.md` 与 `docs/cli.md`，确认文档说明 parser 脚本由运行时/WebUI/API 支持，同时说明当前 CLI `script add` 参数只暴露 `request` / `response` / `decode`。

预期结果：

- 代码搜索能看到 Admin API、脚本类型、Web API/WebUI 均包含 parser。
- `script add --help` 显示 possible values 只有 `request, response, decode`。
- 文档同时记录 parser 支持面与 CLI 参数边界，不误导用户用 `bifrost script add parser ...`。

### TC-DIS-03 规则协议索引包含 bp 与 devtools

操作步骤：

1. 执行 `source ~/.zshrc && rg -n "Protocol::Bp|Protocol::DevTools|\"bp\" =>|\"devtools\" =>" crates/bifrost-core/src/protocol.rs crates/bifrost-cli/src/parsing/rules.rs`。
2. 打开 `docs/operation.md` 和 `docs/rules/README.md`。
3. 确认 `docs/operation.md` 说明 `bp://`、`decode://bp`、`devtools://` 的当前行为，且 `docs/rules/README.md` 能导航到这些控制/脚本协议。

预期结果：

- 工程代码中存在 `bp` 与 `devtools` 协议解析/解析结果字段。
- 文档中可以从规则协议索引发现 `bp` 与 `devtools`，且说明 `bp` 不改写真实流量、`devtools` 是显式授权控制规则。

### TC-DIS-04 规则语法示例符合当前 parser 与模板变量实现

操作步骤：

1. 执行 `source ~/.zshrc && cargo test -q -p bifrost-core split_rule_parts_preserves_paren_content`。
2. 执行 `source ~/.zshrc && cargo test -q -p bifrost-core parse_multiple_protocols_with_paren_content`。
3. 执行 `source ~/.zshrc && cargo test -q -p bifrost-core test_builtin_path`。
4. 执行 `source ~/.zshrc && cargo test -q -p bifrost-core test_builtin_req_headers`。
5. 执行 `source ~/.zshrc && ! rg -n '不能有空格|JSON 冒号后不要加空格|Headers://\\{[^}\\n]+:[^}\\n]+\\}|Cookies://\\{[^}\\n]+:[^}\\n]+\\}|Cors://\\{[^}\\n]+:[^}\\n]+\\}' docs/rules docs/*.md`。
6. 执行 `source ~/.zshrc && ! rg -n '\\$\\{url\\.(path|pathname)\\}|\\$\\{headers\\.' docs/rules docs/*.md`。
7. 打开 `docs/operation.md`、`docs/rules/body-manipulation.md`、`docs/rules/filters.md`、`docs/rules/status-redirect.md`，确认小括号内容、Values 引用、模板路径变量、header 示例的说明与实现一致。

预期结果：

- parser 单测确认小括号内容可以保留空格，且多协议组合中的小括号值不会被空格切断。
- 文档不再宣称小括号内不能有空格，也不再使用 `{X: 1}` 这类会被误读为 Values 引用的内联 header/cookie/CORS 示例。
- `tpl` / `redirect` 示例使用当前 `TemplateEngine` 支持的 `${path}`、`${reqHeaders.name}` 等变量，不使用会退化为完整 URL 的 `${url.path}` / `${url.pathname}` 示例。

### TC-DIS-05 全量 docs 链接与架构索引不漂移

操作步骤：

1. 执行相对链接检查脚本，遍历 `docs/**/*.md` 的 Markdown 链接，忽略外部 URL 和纯 anchor，确认所有相对文件路径都存在。
2. 执行 `source ~/.zshrc && rg -n "bifrost-command|bifrost-power|crates/agent|crates/skills" docs/architecture.md`。
3. 执行 `source ~/.zshrc && rg -n "remote-invoke|临时端口绑定|Agent Skill" docs/overview.md`。

预期结果：

- docs 内部相对链接无缺失。
- `docs/architecture.md` 覆盖当前 workspace 新增 crate，不再只列旧 crate。
- `docs/overview.md` 能发现多端口、remote-invoke、Agent Skill 等当前核心能力。

### TC-DIS-06 过滤器文档与运行时 resolver 边界一致

操作步骤：

1. 执行 `source ~/.zshrc && rg -n "Filter::Body\\(_regex\\) => false|fn parse_filter|reqH:|resH:" crates/bifrost-core/src/rule/filter.rs crates/bifrost-core/src/rule/resolver.rs`。
2. 执行 `source ~/.zshrc && ! rg -n "includeFilter://p:|excludeFilter://p:|includeFilter://H:|excludeFilter://H:|请求体包含|响应体包含|响应体匹配|p:/path|s:4\\b" docs/rule.md docs/rules/filters.md`。
3. 打开 `docs/rule.md` 与 `docs/rules/filters.md`，确认两处都说明 `b:` / `B:` 当前只被 parser 接受、运行时不参与 body 匹配，并推荐用 `bifrost search --req-body/--res-body` 做内容筛选。

预期结果：

- 源码显示 body filter 当前 resolver 结果为不命中。
- 文档不再推荐不存在的 `p:`、大写 `H:` 响应头过滤或 `s:4` 代表 4xx 的错误写法。
- 文档明确记录 body filter 边界，不误导用户写无法生效的生产规则。

### TC-DIS-07 CLI 示例覆盖当前 help 且可独立执行

操作步骤：

1. 执行 `source ~/.zshrc && BIFROST_DATA_DIR="$(mktemp -d)" cargo run -q -p bifrost-cli -- traffic list --help`。
2. 执行 `source ~/.zshrc && BIFROST_DATA_DIR="$(mktemp -d)" cargo run -q -p bifrost-cli -- search --help`。
3. 执行 `source ~/.zshrc && BIFROST_DATA_DIR="$(mktemp -d)" cargo run -q -p bifrost-cli -- remote file --help`。
4. 执行 `source ~/.zshrc && rg -n "client-ip|has-rule-hit|is-websocket|is-sse|is-tunnel|remote file stat|remote file glob|remote file hash|remote file edit|completions elvish|completions powershell|默认访问控制模式为 `interactive`" docs/cli.md docs/getting-started.md docs/cli-quick-start.md`。
5. 执行 `source ~/.zshrc && ! rg -n "访问默认限制为本机|18888 http://httpbin|18888 https://httpbin|--scope shell|remote file write .* --content " docs/cli.md docs/getting-started.md docs/cli-quick-start.md`。

预期结果：

- CLI help 中存在文档声明的 traffic/search/remote file 参数与子命令。
- 文档覆盖当前 help 暴露的关键过滤器、remote file 操作、补全 shell 和默认访问控制语义。
- 快速开始里的普通 curl 示例使用默认主端口 `9900`，不会引用尚未绑定的临时端口。
- 文档不再出现已失效的 `--scope shell` 或不存在的 `remote file write --content` 示例。

### TC-DIS-08 新增规则协议文档同步覆盖

操作步骤：

1. 执行 `source ~/.zshrc && rg -n "UpstreamUnsafeSsl|upstreamUnsafeSsl|Allow insecure upstream HTTPS certificates" crates/bifrost-core/src/protocol.rs crates/bifrost-core/src/syntax.rs crates/bifrost-cli/src/parsing/rules.rs crates/bifrost-proxy/src/proxy/http/handler.rs`。
2. 执行 `source ~/.zshrc && rg -n "upstreamUnsafeSsl" README.md docs/operation.md docs/rule.md docs/rules/README.md docs/rules/routing.md human_tests/rule-filter-routing-diagnostics.md`。
3. 执行 `source ~/.zshrc && rg -n "upstreamUnsafeSsl://true" e2e-tests/rules/regression/rule_filter_routing_diagnostics.txt e2e-tests/tests/test_rule_filter_routing_diagnostics.sh`。
4. 打开 `docs/operation.md` 与 `docs/rules/routing.md`，确认文档说明该协议是按规则跳过上游 HTTPS 证书校验，而不是全局 `--unsafe-ssl` 或客户端信任配置。

预期结果：

- 源码中存在 `upstreamUnsafeSsl` 协议解析、语法元数据、规则合并和代理报错提示实现。
- README、规则语法、operation 总表、规则协议手册和 routing 专页均能发现 `upstreamUnsafeSsl`。
- E2E 规则文件和脚本覆盖自签名上游成功、未配置时失败、错误响应建议 `upstreamUnsafeSsl://true` 三个场景。
- 文档明确该协议只影响 Bifrost 到上游 HTTPS 的证书校验，不改变客户端到 Bifrost 的 TLS 信任关系。

### TC-DIS-09 PR 260 新增 CLI 指令已同步到使用手册与 Skill 安装文档

操作步骤：

1. 执行 `source ~/.zshrc && rg -n "capture wait|traffic get --ids|auth-status|traffic export|traffic replay|--req-json|--res-json|--include|--show-secrets|bifrost-remote|github-copilot|universal|remote run|remote job watch|read-many|scratch-dir|outline" docs/cli.md docs/cli-quick-start.md docs/agent-skill.md docs-en/cli.md docs-en/cli-quick-start.md docs-en/agent-skill.md SKILL.md`。
2. 执行 `source ~/.zshrc && rg -n "capture wait|traffic get --ids|auth-status|traffic export|traffic replay|--req-json|--res-json|--include|--show-secrets" site/src/content/docs/reference/cli.md site/src/content/docs/getting-started/cli-quick-start.md site/src/content/docs/en/reference/cli.md site/src/content/docs/en/getting-started/cli-quick-start.md`。
3. 执行 `source ~/.zshrc && rg -n "bifrost-remote|github-copilot|universal|BIFROST_INSTALL_SKILL_SOURCE|BIFROST_INSTALL_SKILL_DIR" docs/agent-skill.md docs-en/agent-skill.md site/src/content/docs/reference/agent-skill.md site/src/content/docs/en/reference/agent-skill.md`。
4. 打开 `docs/cli.md` 与 `docs/cli-quick-start.md`，确认流量相关说明明确默认脱敏、`--show-secrets` 是显式危险开关，并覆盖等待捕获、批量读取、授权状态、导出、重放、JSONPath / 时间窗口搜索。
5. 打开 `docs/agent-skill.md` 与 `docs-en/agent-skill.md`，确认安装文档说明会同时安装通用 `bifrost` skill 与专用 `bifrost-remote` skill，且说明 `install-skill` 不启动代理、不创建远端授权、不授予 shell 权限。

预期结果：

- 使用手册覆盖 PR 260 新增的 capture、traffic/search、export/replay/auth-status 与默认脱敏行为。
- Skill 安装文档覆盖 `github-copilot`、`universal`、`all` 目标，说明双 skill 安装和安全边界。
- 站点文档由同步脚本生成后与 `docs/`、`docs-en/` 源文档保持一致。

## 本次执行记录

执行日期：2026-05-09。

| 用例 | 实际结果 |
| --- | --- |
| TC-DIS-01 | 通过。`cargo run -q -p bifrost-cli -- --help` 输出包含 `restart`、`setting`、`remote`、`keep-awake`、`im`，`docs/cli.md` 中均存在对应说明或示例。 |
| TC-DIS-02 | 通过。源码中存在 parser 脚本的 Admin API、运行时类型和 WebUI/API 字段；`script add --help` 只暴露 `request`、`response`、`decode`；文档已说明 parser 的 API/WebUI 边界。 |
| TC-DIS-03 | 通过。源码中存在 `bp` 与 `devtools` 协议解析；`docs/operation.md` 与 `docs/rules/README.md` 已能发现并说明对应行为边界。 |
| TC-DIS-04 | 通过。4 个目标单测均通过；陈旧语法扫描无命中，文档不再使用 `${url.path}` / `${url.pathname}` / `${headers.*}` 等不匹配当前实现的模板变量。 |
| TC-DIS-05 | 通过。docs 相对链接检查无缺失；架构文档覆盖 `bifrost-command`、`bifrost-power`、`agent`、`skills`；概览文档覆盖临时端口、remote-invoke、Agent Skill。 |
| TC-DIS-06 | 通过。源码对照确认 `Filter::Body(_regex) => false`；文档不再推荐 `p:`、大写 `H:` 响应头过滤或 `s:4` 这类错误写法，并明确 body filter 当前边界。 |
| TC-DIS-07 | 通过。`traffic list --help`、`search --help`、`remote file --help` 与文档中的参数/子命令一致；失效示例扫描无命中。 |
| TC-DIS-08 | 通过。源码、README、规则语法、operation 总表、规则协议手册、routing 专页、E2E 规则和 human_tests 均包含 `upstreamUnsafeSsl`；文档明确它是按规则跳过上游 HTTPS 证书校验，并记录失败响应中的修复建议。 |
| TC-DIS-09 | 通过。`docs/cli.md`、`docs/cli-quick-start.md`、中英文 Agent Skill 安装文档、`SKILL.md` 和站点同步产物均已覆盖 `capture wait`、`traffic get --ids`、`auth-status`、`traffic export/replay`、JSONPath / include / 脱敏开关、`bifrost-remote` 双 skill 安装说明，以及 `remote run/job/file` 新能力；当前沙箱无 `~/.zshrc`，实际执行时跳过 source 前置并直接运行等价 `rg` 扫描。 |

## 清理步骤

- 删除命令中创建的临时 `BIFROST_DATA_DIR`（若 shell 已自动回收 `mktemp` 路径变量不可见，则无需额外操作）。
- 不需要停止正式 9900 服务；本测试不启动代理服务。
