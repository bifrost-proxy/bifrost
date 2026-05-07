# Docs Implementation Sync 测试用例

## 功能模块说明

验证 `docs/` 中的 CLI、Scripts、规则协议说明与当前工程实现保持一致。重点覆盖当前源码 `bifrost --help` 暴露的命令表面、Scripts parser 类型的真实 API/运行时支持、规则协议索引中 `bp` / `devtools` 的可发现性，以及规则语法示例与当前 parser/template 行为一致。

## 前置条件

- 工作目录为仓库根目录 `/Users/eden/work/github/bifrost`。
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

## 清理步骤

- 删除命令中创建的临时 `BIFROST_DATA_DIR`（若 shell 已自动回收 `mktemp` 路径变量不可见，则无需额外操作）。
- 不需要停止正式 9900 服务；本测试不启动代理服务。
