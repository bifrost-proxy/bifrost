# 规则语法检查服务与保存接口诊断反馈

## 背景

Bifrost Rules 编辑器早期通过 `POST /_bifrost/api/rules/validate` 提供语法诊断，帮助 Web 端在 Monaco 上标记错误、警告、变量与脚本引用；但真正落盘的接口（`POST /api/rules`、`PUT /api/rules/{name}` 与组规则 `POST /api/group-rules/{group}` / `PUT /api/group-rules/{group}/{rule}`、CLI `bifrost rule add/update`）历史上直接调用 `RulesStorage::save`，不复用 validate 逻辑。Agent 通过 CLI/API 提交错误规则时，只能拿到自由文本错误，无法结构化解析行列、错误码或修复建议，也无法判断是否值得自动修复后重试。

本方案统一保存/更新链路上的语法检查服务，让 Admin API、组规则 API、CLI 与 Web UI 共享同一份 `RuleSyntaxReport`，并在响应中携带 Agent 可直接消费的 `guidance`（summary、retryable、next_actions）。已落地代码见 `crates/bifrost-core/src/rule/syntax_report.rs` 与 `crates/bifrost-admin/src/handlers/rules.rs::build_rule_syntax_report`。

## 用户目标验证清单

### 必须实现

- 抽出可复用的规则语法检查服务：复用 `validate_rules_with_context`、`expand_rule_references_strict`、脚本引用检查和 `ParseError`。
- 本地规则 create/update 在保存前执行 syntax check，并在响应中带 `syntax` 诊断。
- 本地 CLI `rule add/update` 在写入前执行 syntax check；失败时输出明确错误，JSON 模式输出稳定结构。
- 组规则 create/update 在提交远端 `/v4/env` 前执行同样的 syntax check，避免脏数据先入远端再暴露问题。
- Agent 能从响应直接读取：是否保存 (`saved`)、是否可重试 (`syntax.guidance.retryable`)、错误行列 (`syntax.errors[].line/start_column/end_column`)、错误码 (`code`)、错误信息 (`message`)、修复建议 (`suggestion`)、可选 code fixes (`fixes`)。

### 必须不破坏

- 现有 `POST /api/rules/validate` 继续可用（响应类型已改为 `type ValidateRuleResponse = RuleSyntaxReport;`）。
- WebUI Monaco markers、hover、code action 的诊断结构保持兼容。
- Warning 不阻止保存。
- `@规则引用` strict 校验继续使用当前目录语义：保存当前规则时先把待保存内容覆盖进 reference catalog 再展开。
- 不改变运行时宽松解析策略。
- 不在保存时模拟运行时请求/响应上下文；response-phase filter 评估仍属 resolver 运行时行为。

### 必须真实验证

- Admin API：有效规则保存返回 `success=true`、`saved=true`、`syntax.valid=true`；错误规则默认 422 且不落盘、不触发 push。
- CLI：`bifrost rule add/update` 对错误规则退出码 2、可读错误、JSON 模式可机器解析；`--allow-invalid` 能显式保存。
- WebUI：保存错误规则时展示后端返回的同一诊断，不只有编辑器本地提示。
- 组规则：错误组规则不会先写入远端 env；错误响应可被 WebUI/Agent 解析。

## 产品语义

### RuleSyntaxReport 是所有入口的公共契约

统一响应类型（`bifrost-core::rule::syntax_report`）：

```rust
pub struct RuleSyntaxGuidance {
    pub summary: String,
    pub retryable: bool,
    pub next_actions: Vec<String>,
}

pub struct RuleSyntaxReport {
    pub valid: bool,
    pub rule_count: usize,
    pub errors: Vec<ParseError>,
    pub warnings: Vec<ParseError>,
    pub defined_variables: Vec<VariableInfo>,
    #[serde(default)]
    pub script_references: Vec<ScriptReference>,
    pub guidance: RuleSyntaxGuidance,
}

pub fn validate_rule_syntax_report(
    source_name: &str,
    content: &str,
    reference_catalog: &HashMap<String, String>,
    global_values: &HashMap<String, String>,
) -> RuleSyntaxReport;
```

Admin 侧的 stateful helper `build_rule_syntax_report(state, source_name, content, global_values)` 在此基础上叠加：
1. 从 `state.rules_storage.load_all_with_subdirs()` 构建 catalog。
2. 用待保存内容覆盖同名条目。
3. `std::panic::catch_unwind` 包裹校验。
4. 有 `script_manager` 时列出 request/response/decode 三类脚本（decode 类预置 `utf8` / `default` / `bp` 三个 built-in），把缺失引用追成 `W003` warning，`refresh_guidance` 更新 summary / next_actions。

原设计中的 `RuleSyntaxService` OO 封装与 `RuleSyntaxInput` 结构体没有落地——调用方直接传四个参数；`current_group_name` 由调用侧拼成 `"<group>/<rule>"` 作为 source_name。

### 保存/更新响应结构（RuleMutationResponse）

成功：

```json
{
  "success": true,
  "message": "Rule 'demo' updated successfully",
  "saved": true,
  "syntax": {
    "valid": true,
    "rule_count": 1,
    "errors": [],
    "warnings": [],
    "defined_variables": [],
    "script_references": [],
    "guidance": {
      "summary": "Rule syntax is valid.",
      "retryable": false,
      "next_actions": []
    }
  }
}
```

Syntax error → HTTP `422 Unprocessable Entity`：

```json
{
  "success": false,
  "message": "Rule 'demo' was not saved because syntax validation failed",
  "saved": false,
  "syntax": {
    "valid": false,
    "rule_count": 0,
    "errors": [
      {
        "line": 1,
        "start_column": 1,
        "end_column": 42,
        "message": "Unknown protocol 'htp'",
        "severity": "error",
        "code": "E001",
        "suggestion": "Use a supported protocol such as host://, proxy://, reqHeaders:// or resHeaders://.",
        "fixes": []
      }
    ],
    "warnings": [],
    "defined_variables": [],
    "script_references": [],
    "guidance": {
      "summary": "Fix 1 rule syntax error before retrying this save.",
      "retryable": true,
      "next_actions": [
        "Read syntax.errors[].line, syntax.errors[].message, and syntax.errors[].suggestion.",
        "Edit the rule content, then retry the same create or update request."
      ]
    }
  }
}
```

组规则 422 响应结构 `GroupRuleSyntaxRejectionResponse { success, message, saved, syntax }` 同构；组规则成功响应是 `GroupRuleDetail`，`syntax: Option<RuleSyntaxReport>` 仅在 create/update 路径带出。

### Strict 与 allow_invalid 边界

- `CreateRuleRequest` / `UpdateRuleRequest` / `CreateGroupRuleRequest` / `UpdateGroupRuleRequest` 均带 `#[serde(default)] allow_invalid: bool`。
- 默认 strict：`!syntax.valid && !allow_invalid` → 422，不落盘、不推送。
- `allow_invalid = true`：即便 `syntax.valid=false` 也会保存，响应 `saved=true` 且 `syntax.valid=false`。
- `update_rule` 仅在 `content_changed` 或 `enabled` 从 false 翻到 true 时强制门禁；只改 `enabled=false` 不会因现状无效而 422。
- Warnings 不阻止保存，随成功响应返回，便于 Agent 后续优化。

## 技术细节

### 核心校验（bifrost-core）

- `crates/bifrost-core/src/rule/syntax_report.rs::validate_rule_syntax_report`：先跑 `expand_rule_references_strict`，返回 `Err` 时通过 `RuleSyntaxReport::from_reference_error` 汇总；成功后跑 `validate_rules_with_context` 汇总成 `RuleSyntaxReport::from_validation_result`，最后 `refresh_guidance` 更新 summary / next_actions / retryable。
- `build_guidance(valid, error_count, warning_count) -> RuleSyntaxGuidance`：`valid` → summary `Rule syntax is valid.`；否则 `Fix N rule syntax error(s) before retrying this save.` + `retryable=true`。

### Admin handler（bifrost-admin）

- `crates/bifrost-admin/src/handlers/rules.rs`：
  - `create_rule`：解析请求 → 名称/存在检查（缺名 400、重名 409）→ `build_rule_syntax_report` → 422 `syntax_rejection_response` 或 200 `syntax_saved_response`。
  - `update_rule`：仅 `content_changed` 或 `enabled` 从 false 翻到 true 时门禁；成功后写 `updated_at = chrono::Utc::now().to_rfc3339()`，按需 `touch_local_change`。
  - `POST /api/rules/validate` 响应类型改为 `type ValidateRuleResponse = RuleSyntaxReport;`。
- `crates/bifrost-admin/src/handlers/group_rules.rs`：
  - `handle_create_rule` / `handle_update_rule` 在调用远端前调 `validate_group_rule_for_save`，内部转发 `super::rules::build_rule_syntax_report(...)`。
  - source_name = `format!("{group_name}/{rule_name}")`。
  - strict 失败 → 422，不调远端也不写本地。
  - 成功后提交远端 → 把远端 `RemoteEnv` 写回本地 → `GroupRuleDetail::syntax = Some(report)`。

### CLI helper（bifrost-cli）

- `crates/bifrost-cli/src/commands/rule.rs::validate_local_rule_syntax{,_with_values}`：复用 `bifrost_core::validate_rule_syntax_report` + `bifrost_storage::build_rule_reference_catalog` + `RulesStorage::load_all_with_subdirs()`。
- 默认 strict：`!syntax.valid && !allow_invalid` → `print_rule_syntax_rejection` + `std::process::exit(2)`。
- `--allow-invalid` 已实现于 `RuleCommands::Add` / `RuleCommands::Update`。
- `--json`：输出 `RuleMutationCliResponse { success, message, saved, syntax }`，字段与 Admin 对齐。
- 仅覆盖 CLI 本地写入；CLI 没有等价 admin HTTP 路径，因此 `W003` 缺失脚本 warning 目前只在 admin API 侧出现。

CLI 人类可读输出：

```text
Rule 'demo' was not saved: 1 syntax error
E001 line 1: Unknown protocol 'htp'
Suggestion: Use a supported protocol such as host://, proxy://, reqHeaders:// or resHeaders://.
Next: edit the rule content and retry `bifrost rule update demo ...`.
```

### Agent 指导原则

- `syntax.errors[].code`：稳定错误码，不依赖英文文案。
- `syntax.errors[].suggestion`：单条问题的局部修复建议。
- `syntax.errors[].fixes`：已有 code action 时原样返回。
- `syntax.guidance.summary`：汇总本次为什么失败。
- `syntax.guidance.next_actions`：Agent 下一步动作。
- `syntax.guidance.retryable`：语法类错误 true；存储/权限/网络错误 false 或缺省。
- 不在指导信息中塞长篇规则文档；如需文档只返回短链接或错误码。

## CLI + Web + Admin API

### CLI

```bash
bifrost rule add demo --content "@rule"                # 缺失引用 → exit 2
bifrost rule add demo --content "@rule" --allow-invalid # 允许保存
bifrost rule update demo --content "example.com host://10.0.0.53" --json
```

### Admin API

- `POST /api/rules/validate` → 200 `RuleSyntaxReport`。
- `POST /api/rules` → 200 `RuleMutationResponse` 或 422 syntax rejection。
- `PUT /api/rules/{name}` → 同上；只改 `enabled=false` 不触发门禁。
- `POST /api/group-rules/{group}` / `PUT /api/group-rules/{group}/{rule}` → 200 `GroupRuleDetail` (含 `syntax`) 或 422 `GroupRuleSyntaxRejectionResponse`。
- 400：缺 name；409：同名已存在；500：存储失败。

### Web UI

- `web/src/components/BifrostEditor/snippet/validation.ts` 消费 `errors` / `warnings` → Monaco markers 与 hover。
- `web/src/api/rules.ts` / `web/src/api/group.ts` 已透传 `RuleMutationResponse`；WebUI store 需消费 body 里的 `syntax` 才能展示后端诊断，而不是只依赖 Monaco 本地校验。
- `web/src/types/index.ts` 中新增 `RuleSyntaxReport` / `RuleSyntaxGuidance` 类型定义。

## Sync 边界

- 语法检查在写入前发生，不影响 Sync 流程。
- 组规则 422 时不调用远端 `/v4/env`，避免脏数据先入远端。
- 成功保存后仍走原有 `notify_rules_changed` + `invalidate_overview_cache` 路径；`RulesChanged` 事件带来源以避免自激环（详见 rules-filesystem-hot-reload 文档）。

## Phase 划分

### Phase 1：核心校验抽出

- 新增 `bifrost-core::rule::syntax_report`，提供 `RuleSyntaxReport` / `RuleSyntaxGuidance` / `validate_rule_syntax_report` / `build_guidance`。
- 单元测试覆盖 valid / missing-reference。

### Phase 2：Admin/CLI 接入

- `create_rule` / `update_rule` 复用 `build_rule_syntax_report`，`syntax_rejection_response` + `syntax_saved_response`。
- 组规则 handler 在调用远端前接 `validate_group_rule_for_save`。
- CLI `rule add/update` 用 `validate_local_rule_syntax(_with_values)`，加 `--allow-invalid` / `--json` / exit code 2。

### Phase 3：Web UI 透传

- WebUI store 消费 `RuleMutationResponse.syntax`，把后端诊断展示到保存错误提示。
- Monaco markers / hover 与后端诊断字段对齐。

### Phase 4：文档、测试与真实验证

- 更新 `crates/bifrost-admin/ADMIN_API.md`、`docs/rule.md`、`docs/rules/README.md`。
- 更新 `human_tests/api-rules.md`、`human_tests/cli-rule-management.md`、`human_tests/webui-rules.md`、`human_tests/readme.md`。
- 补齐 e2e-tests shell 用例并跑通两轮 Review/Fix/Test。

## 测试方案

### 单元测试（实际落地）

- `cargo test -p bifrost-core rule::syntax_report::tests::valid_rule_returns_success_guidance`
- `cargo test -p bifrost-core rule::syntax_report::tests::missing_reference_returns_e020_guidance`
- `cargo test -p bifrost-cli rule::tests::validate_local_rule_syntax_reports_valid_rules`
- `cargo test -p bifrost-cli rule::tests::validate_local_rule_syntax_reports_missing_reference`
- `cargo test -p bifrost-cli rule::tests::validate_local_rule_syntax_rejects_invalid_port`
- `cargo test -p bifrost-cli rule::tests::validate_local_rule_syntax_rejects_invalid_status_code`
- `cargo test -p bifrost-cli rule::tests::validate_local_rule_syntax_allows_warning_only_rules`
- `cargo test -p bifrost-cli rule::tests::validate_local_rule_syntax_expands_existing_references`

尚未独立落地（planned as of 2026-07-02）：`cargo test -p bifrost-admin rule_syntax_valid_rule_returns_guidance_success`、`cargo test -p bifrost-admin rule_syntax_missing_reference_returns_e020_guidance`、`cargo test -p bifrost-admin create_rule_rejects_invalid_content_without_storage_write`、`cargo test -p bifrost-admin update_rule_rejects_invalid_content_preserves_existing_content`、`cargo test -p bifrost-cli rule_add_invalid_content_exits_without_write`。Admin 侧等价语义由 e2e shell 脚本兜底。

### E2E 测试

- `e2e-tests/tests/test_rule_syntax_save_api.sh`（覆盖 `rule_save_returns_syntax_report_for_valid_content` / `rule_save_rejects_invalid_content_with_agent_guidance` / `rule_update_invalid_content_preserves_previous_rule`）：真实启动 Bifrost，验证有效创建 200 + `saved=true` / `syntax.valid=true`；缺失 `@` 引用创建 422 + `GET /api/rules/{name}` 404；缺失引用更新 422 且旧内容保持不变。
- `e2e-tests/tests/test_rule_syntax_cli.sh`（覆盖 `cli_rule_add_json_reports_syntax_error`）：有效新增 JSON `saved=true` / `syntax.valid=true`；缺失 `@` 引用新增退出码 2 且不落盘；缺失引用更新退出码 2 且旧内容保持不变；`--allow-invalid` 可显式保存。
- `group_rule_update_invalid_content_does_not_call_remote_write` 暂无独立 e2e 脚本（planned as of 2026-07-02），由 `validate_group_rule_for_save` 代码路径 + admin 单元测试间接保障。

### human_tests

- `human_tests/api-rules.md` 已新增 PASS 记录，引用 `e2e-tests/tests/test_rule_syntax_save_api.sh`：
  - 有效规则创建返回 `saved=true` / `syntax.valid=true`。
  - 错误规则创建返回 HTTP 422 且 `GET /api/rules/{name}` 为 404。
  - 错误规则更新返回 HTTP 422 且旧内容保持不变。
- `human_tests/cli-rule-management.md` 已新增 PASS 记录，引用 `e2e-tests/tests/test_rule_syntax_cli.sh`。
- `human_tests/webui-rules.md` 保存错误规则的真实 WebUI 验证步骤仍待补齐（planned as of 2026-07-02）。
- `human_tests/readme.md` 索引同步即可，不维护全局数量。
- 原设计的 `TC-ARU-语法-01/02/03`、`TC-CRM-语法-01/02` 显式 TC 编号未直接落入文档，等价语义由 PASS 记录 + e2e 脚本承担。

## Review/Fix/Test 闭环

### 第 1 轮

- 目标复核：保存/更新 API、CLI、group rules、Agent JSON 诊断是否都覆盖。
- 代码 review：validate/create/update 是否避免三份逻辑漂移；422 是否不会触发 storage write、push invalidation 或远端 group env 写入。
- 测试运行：admin handler 单元测试、CLI helper 单元测试、最小 E2E。
- 修复优先级：语义不一致、响应结构不稳定、旧内容被覆盖。

### 第 2 轮

- 再次目标复核：warnings、不改 content 的 enabled 更新、`allow_invalid`、`@规则引用` catalog 覆盖。
- 再次代码 review：WebUI API 类型、错误展示、human_tests 索引、设计文档同步。
- 复跑测试：受影响 case 全量 + `cargo test --workspace --all-features`。

若仍出现保存无效规则、错误响应不可解析、group 远端先写入、CLI 无法被 Agent 稳定解析，追加第 3 轮。

## 校验命令

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test -p bifrost-core rule::syntax_report`
- `cargo test -p bifrost-cli rule::tests`
- `cargo test -p bifrost-admin rule_syntax`（planned as of 2026-07-02，命名 `rule_syntax` 的 admin 单测尚未落地）
- `cargo test -p bifrost-e2e rule_save`（planned as of 2026-07-02，实际由 shell e2e 承担）
- `BIFROST_BIN="$PWD/target/debug/bifrost" bash e2e-tests/tests/test_rule_syntax_save_api.sh`
- `BIFROST_BIN="$PWD/target/debug/bifrost" bash e2e-tests/tests/test_rule_syntax_cli.sh`
- `cargo test --workspace --all-features`
- 按 human_tests 文档逐条真实执行 API / CLI / WebUI。
- 视修改范围最后执行 `scripts/ci/local-ci.sh`；未执行需说明原因。

## 风险与决策

- **是否默认 strict**：本方案确认默认阻止 errors、允许 warnings；如需保留无效草稿必须 `allow_invalid=true`。
- **Group 远端是否有自身校验**：未知；本地先校验避免 Agent 收到远端不稳定错误。
- **CLI 无法完整检查 script_manager**：`W003` 缺失脚本 warning 目前只在 admin API 具备完整能力时返回；如 Agent 需要完整脚本诊断需走 admin HTTP。
- **WebUI store 忽略 mutation body**：Web 侧必须消费 `syntax` 才能展示后端诊断；否则用户只能看到 Monaco 本地提示，等同没接。
- **`allow_invalid` 逃生口的可观测性**：日志需明确记录 `saved with syntax_invalid=true`，避免线上意外积累无效规则。
