# 规则语法检查服务与保存接口诊断反馈

## 功能模块详细描述

当前 Rules 编辑器已经通过 `POST /_bifrost/api/rules/validate` 获得语法错误、警告、变量、脚本引用和 `@规则引用` 诊断；但真正写入规则的入口仍以“直接保存”为主：

- 本地规则 API：`POST /_bifrost/api/rules`、`PUT /_bifrost/api/rules/{name}` 在 `crates/bifrost-admin/src/handlers/rules.rs` 中直接调用 `RulesStorage::save`。
- 组规则 API：`POST /_bifrost/api/group-rules/{group}`、`PUT /_bifrost/api/group-rules/{group}/{rule}` 在 `crates/bifrost-admin/src/handlers/group_rules.rs` 中先调用远端 env API，再写回本地 group rules storage。
- 本地 CLI：`bifrost rule add/update` 在 `crates/bifrost-cli/src/commands/rule.rs` 中直接调用 `RulesStorage::save` / `update_content`。
- WebUI 的 Monaco 校验只影响编辑器提示；Agent 直接调用保存/更新 API 或 CLI 时拿不到稳定的机器可读诊断。

本方案目标是在规则保存和更新链路上提供统一的语法检查服务与结构化反馈，让 Agent 在提交错误规则时能收到可解析的错误位置、错误码、修复建议和下一步指导，并基于反馈自动修正后重试。

## 用户目标验证清单

### 必须实现

- 新增可复用的规则语法检查服务，复用当前 `validate_rules_with_context`、`expand_rule_references_strict`、脚本引用检查和 `ParseError` 结构。
- 本地规则创建/更新 API 在保存前执行语法检查，并在响应中返回 `syntax` 诊断对象。
- 本地 CLI `rule add/update` 在写入前执行语法检查；失败时输出明确错误与指导，JSON 模式输出稳定结构。
- 组规则创建/更新 API 在提交远端前执行同样的语法检查，避免无效规则先写入远端再暴露问题。
- Agent 能从接口响应中直接读取：是否保存、是否可重试、错误行列、错误码、错误信息、修复建议、可选 code fixes。

### 必须不破坏

- 现有 `POST /api/rules/validate` 继续可用，作为编辑器即时校验和独立校验 API。
- WebUI 当前 Monaco markers、hover、code action 的诊断结构保持兼容。
- 警告不阻止保存；错误是否阻止保存由明确策略控制，避免误伤需要暂存草稿的场景。
- `@规则引用` 的 strict 校验继续使用当前目录语义：保存当前规则时把待保存内容覆盖进 reference catalog 后再展开。
- 不改变运行时宽松解析策略；本方案只影响保存/更新时的反馈与可选门禁。
- 不在保存时模拟运行时请求/响应上下文；例如最新主干中的 response-phase filter 评估仍属于 resolver 运行时行为，语法检查只验证 filter value 可解析、协议值合法和引用关系完整。

### 必须真实验证

- Admin API 真实请求验证：有效规则保存成功并返回 `syntax.valid=true`；错误规则返回结构化诊断且不会在 strict 模式写入。
- CLI 真实命令验证：`bifrost rule add/update` 对错误规则返回非 0 exit code、可读错误、JSON 模式可机器解析。
- WebUI 保存链路验证：保存错误规则时展示后端返回的同一诊断，不再只有编辑器本地提示。
- Group rules 验证：错误组规则不会先写入远端 env；错误响应可被 WebUI/Agent 解析。

### 必须交付

- 设计文档、API 契约、错误响应示例、测试计划、human_tests 计划同步。
- 实现阶段必须补单元测试、E2E、human_tests，并完成至少两轮 Review/Fix/Test。

## 当前实现分析

### 已有能力

- `crates/bifrost-core/src/rule/parser/mod.rs`
  - `validate_rules_with_context(text, global_values)` 返回 `ValidationResult`。
  - `ParseError` 已包含 `line`、`start_column`、`end_column`、`message`、`severity`、`suggestion`、`code`、`fixes`。
  - parser 已支持 markdown value block、line block、变量引用、filter/protocol value 诊断和 tolerant parsing。
- `crates/bifrost-core/src/rule/references.rs`
  - `expand_rule_references_strict(source_name, content, catalog)` 能对缺失引用和循环引用返回明确错误。
- `crates/bifrost-admin/src/handlers/rules.rs`
  - `validate_rule` 已把 `@规则引用` 展开、panic 捕获、脚本引用 warning 汇总到 `ValidateRuleResponse`。
  - `create_rule` 和 `update_rule` 尚未复用该校验逻辑，仍直接保存。
- `web/src/components/BifrostEditor/snippet/validation.ts`
  - WebUI 已消费 `errors` / `warnings` 并设置 Monaco markers。

### 缺口（已基本闭合，按当前 main 分支刷新）

- ~~校验逻辑嵌在 HTTP handler 内，不能被 create/update/group rules/CLI 复用~~ — 已抽到 `bifrost_core::validate_rule_syntax_report` + `bifrost_admin::handlers::rules::build_rule_syntax_report`，由 admin（本地 + 组规则）与 CLI 共同复用。
- ~~保存成功响应只有 `message`，没有 `syntax` 诊断；保存失败响应也只有字符串错误~~ — 已落地 `RuleMutationResponse { success, message, saved, syntax }` 与 422 `syntax_rejection_response`；组规则同构。
- ~~API 没有明确区分“存储失败”和“规则语法失败”~~ — 语法失败固定 422、存储失败仍走 500、name/exists 走 400/409；`syntax.guidance.retryable` 帮助 Agent 决策是否值得重试。
- ~~CLI 本地写入绕过 admin validate API~~ — `bifrost rule add/update` 已内置 `validate_local_rule_syntax`、`--allow-invalid`、`--json` 与 exit code 2。
- 残留缺口：CLI 本地校验拿不到运行中服务的 `script_manager` 列表，因此 `W003` 缺失脚本 warning 仅在 admin API 路径下出现；如 Agent 需要在 CLI 模式拿到完整脚本诊断，仍需调用 admin HTTP。

## 技术方案

### 1. 抽出规则语法检查服务（已落地，与最初设计有调整）

实际落地的拆分如下（按 2026-06-17 当前代码刷新）：

- 核心数据结构和无状态校验入口放在 `crates/bifrost-core/src/rule/syntax_report.rs`，由 `bifrost_core` 顶层重新导出（`bifrost_core::{validate_rule_syntax_report, RuleSyntaxReport, RuleSyntaxGuidance}`）。
- admin 侧的 reference catalog 装配、panic 捕获、script_manager 缺失脚本 warning 拼装放在 `crates/bifrost-admin/src/handlers/rules.rs` 中的 `pub(crate) async fn build_rule_syntax_report(state, source_name, content, global_values)` helper，由 `validate_rule` / `create_rule` / `update_rule` / 组规则 handler 共同复用。
- CLI 侧在 `crates/bifrost-cli/src/commands/rule.rs` 复用 `bifrost_core::validate_rule_syntax_report` 与 `bifrost_storage::build_rule_reference_catalog`，不经过 admin HTTP 层。

核心类型（实际实现，位于 `bifrost-core`）：

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuleSyntaxGuidance {
    pub summary: String,
    pub retryable: bool,
    pub next_actions: Vec<String>,
}

pub fn validate_rule_syntax_report(
    source_name: &str,
    content: &str,
    reference_catalog: &HashMap<String, String>,
    global_values: &HashMap<String, String>,
) -> RuleSyntaxReport;
```

说明：最初设计中的 `RuleSyntaxInput` 结构体没有落地——调用方直接传 `source_name` / `content` / `reference_catalog` / `global_values` 四个参数；`current_group_name` 由调用侧自行拼成 `"<group>/<rule>"` 作为 `source_name`（见 `validate_group_rule_for_save`）。`RuleSyntaxService` 这一面向对象封装也没有出现，逻辑分散在 `bifrost_core::validate_rule_syntax_report`（纯校验）与 `bifrost_admin::handlers::rules::build_rule_syntax_report`（叠加 admin 状态）两层。

服务逻辑（`build_rule_syntax_report` 的实际流程）：

1. 从 `state.rules_storage.load_all_with_subdirs()` 构建 reference catalog（`bifrost_storage::build_rule_reference_catalog`）。
2. 把当前待保存内容以 `source_name` 覆盖进 catalog，确保 `@当前规则` / 同名更新用新内容校验。
3. 用 `std::panic::catch_unwind` 包住 `validate_rule_syntax_report`（内部依次跑 `expand_rule_references_strict` 与 `validate_rules_with_context`），把 panic 转成 `Err(String)`。
4. 如果有 `script_manager`，列出 request / response / decode 三类脚本（decode 类预置 `utf8` / `default` / `bp` 三个内建项），通过 `append_missing_script_reference_warnings` 把缺失的引用追加成 `W003` warning，并 `refresh_guidance` 更新 summary / next_actions。
5. 返回 `RuleSyntaxReport`。

现有 `POST /api/rules/validate` 已改造为调用该 helper，响应类型为 `type ValidateRuleResponse = RuleSyntaxReport;`——原有字段保持，新增 `guidance` 子对象。

### 2. 保存/更新 API 响应契约（已落地）

统一响应结构（即 `RuleMutationResponse`，相比原设计增加了 `success` 字段以与既有 admin 接口风格一致）：

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

语法错误响应使用 HTTP `422 Unprocessable Entity`：

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

实际固化的 `next_actions` 文案略宽于最初设计（指向 `syntax.errors[]` 数组整体，而不是首项）；详见 `bifrost_core::rule::syntax_report::build_guidance`。

组规则 422 响应同构，结构体为 `GroupRuleSyntaxRejectionResponse { success, message, saved, syntax }`；成功响应是 `GroupRuleDetail`，其中 `syntax: Option<RuleSyntaxReport>`（仅 create/update 路径下携带，list/get 不带）。

兼容策略（实际行为）：

- `POST /api/rules`、`PUT /api/rules/{name}` 默认 strict：有 `errors` 时不保存，返回 422。
- `warnings` 不阻止保存，随成功响应返回，便于 Agent 后续优化。
- 请求体的 `allow_invalid: true` 已落地（`CreateRuleRequest` / `UpdateRuleRequest` / `CreateGroupRuleRequest` / `UpdateGroupRuleRequest` 都带 `#[serde(default)] allow_invalid: bool`），开启后即便 `syntax.valid=false` 也会保存，响应 `saved=true` 且 `syntax.valid=false`。
- `update_rule` 的 `should_validate` 仅在 `content_changed` 或 `enabled` 从 false 翻到 true 时强制门禁；只改 `enabled=false` 之类的字段即便规则现状已无效也不会触发 422。
- WebUI 普通保存默认使用 strict；若后续要支持草稿，应新增“保存草稿”语义，而不是静默保存无效启用规则。

### 3. API 入口改造（已落地）

本地规则（`crates/bifrost-admin/src/handlers/rules.rs`）：

- `create_rule`
  - 解析请求后先检查 name/existence（缺名 400、重名 409）。
  - 调用 `build_rule_syntax_report` 取 `RuleSyntaxReport`。
  - `!syntax.valid && !request.allow_invalid` 时走 `syntax_rejection_response` 返回 422，不写 storage、不触发 push。
  - 通过后保存，再 `notify_rules_changed` + `invalidate_overview_cache`，返回 `RuleMutationResponse { success: true, message, saved: true, syntax }`（由 `syntax_saved_response` 拼装）。
- `update_rule`
  - 仅当 `content_changed` 或 `enabled` 从 false 翻到 true 时走门禁；只改 `enabled=false` / 其它字段不会因为现有内容无效就阻断。
  - 触发门禁时同样返回 422，旧 storage 内容保留不变。
  - 成功后用 `chrono::Utc::now().to_rfc3339()` 写 `updated_at`，并按需 `touch_local_change`。

组规则（`crates/bifrost-admin/src/handlers/group_rules.rs`）：

- `handle_create_rule` / `handle_update_rule`
  - 在调用远端 `/v4/env`（POST / PATCH）之前调 `validate_group_rule_for_save`，内部转发给 admin 的 `build_rule_syntax_report`。
  - `source_name` 拼成 `format!("{group_name}/{rule_name}")`，与 `@组名/规则名` reference catalog 语义一致。
  - strict 失败时返回 422 `GroupRuleSyntaxRejectionResponse`，不调用远端、也不更新本地 group rule storage。
  - 成功后再提交远端、把远端回来的 `RemoteEnv` 写入本地 group rules storage，并在 `GroupRuleDetail` 中携带 `syntax: Some(report)`（仅 mutation 路径，list/get 不带）。

独立校验：

- 保留 `POST /api/rules/validate`，响应类型即 `RuleSyntaxReport`（`type ValidateRuleResponse = RuleSyntaxReport;`）。
- 没有引入 `POST /api/rules/check` 别名（planned, not yet shipped as of 2026-06-17）。

### 4. CLI 改造（已落地）

`bifrost rule add/update` 在本地写入前调用 core 级校验 helper（`crates/bifrost-cli/src/commands/rule.rs`）：

- `validate_local_rule_syntax{,_with_values}` 内部 helper 复用 `bifrost_core::validate_rule_syntax_report` + `bifrost_storage::build_rule_reference_catalog` + `RulesStorage::load_all_with_subdirs()`，把待保存内容覆盖进 catalog 后跑同一套校验。
- 默认 strict：`!syntax.valid && !allow_invalid` 时调 `print_rule_syntax_rejection` 后 `std::process::exit(2)`，不写 `RulesStorage`。
- `--allow-invalid` 显式逃生口已实现：`RuleCommands::Add` / `RuleCommands::Update` 都接 `allow_invalid: bool`。
- `--json` 已实现，输出 `RuleMutationCliResponse { success, message, saved, syntax }`，结构与 admin `RuleMutationResponse` 字段对齐，便于 Agent 解析。
- 仅覆盖本地 `bifrost rule add/update`；CLI 当前没有「先连服务再走 admin API」的等价命令（如需享受 `script_manager` 的 W003 warning，仍要走 admin HTTP）。

CLI 人类可读输出示例：

```text
Rule 'demo' was not saved: 1 syntax error
E001 line 1: Unknown protocol 'htp'
Suggestion: Use a supported protocol such as host://, proxy://, reqHeaders:// or resHeaders://.
Next: edit the rule content and retry `bifrost rule update demo ...`.
```

CLI JSON 输出示例：

```json
{
  "message": "Rule 'demo' was not saved because syntax validation failed",
  "saved": false,
  "syntax": {
    "valid": false,
    "rule_count": 0,
    "errors": [
      {
        "line": 1,
        "start_column": 1,
        "end_column": 16,
        "message": "Unknown protocol 'htp'",
        "severity": "error",
        "code": "E001",
        "suggestion": "Use a supported protocol such as host://, proxy://, reqHeaders:// or resHeaders://."
      }
    ],
    "warnings": [],
    "defined_variables": [],
    "script_references": [],
    "guidance": {
      "summary": "Fix 1 rule syntax error before retrying this save.",
      "retryable": true,
      "next_actions": []
    }
  }
}
```

### 5. Agent 指导信息原则

给 Agent 的字段必须稳定、短、可执行：

- `syntax.errors[].code`：稳定错误码，不依赖英文文案。
- `syntax.errors[].suggestion`：单条问题的局部修复建议。
- `syntax.errors[].fixes`：已有 code action 时原样返回。
- `syntax.guidance.summary`：汇总本次保存为什么失败。
- `syntax.guidance.next_actions`：告诉 Agent 如何改内容并重试。
- `syntax.guidance.retryable`：语法类错误为 true，存储/权限/网络错误为 false 或缺省。

不建议在指导信息中塞长篇规则文档；需要文档时只返回短链接或错误码，避免 Agent 被大段说明污染上下文。

## 依赖项（按实际实现）

- `bifrost-core::validate_rules_with_context`
- `bifrost-core::expand_rule_references_strict`
- `bifrost-core::ParseError` / `ValidationResult` / `VariableInfo` / `ScriptReference`
- `bifrost-core::{validate_rule_syntax_report, RuleSyntaxReport, RuleSyntaxGuidance}`（新增，位于 `crates/bifrost-core/src/rule/syntax_report.rs`）
- `bifrost_storage::build_rule_reference_catalog`
- `RulesStorage::load_all_with_subdirs`
- `bifrost-admin` 的 `SharedAdminState`、`script_manager` 与 `handlers::rules::build_rule_syntax_report` helper
- `web/src/types/index.ts` 中的 `RuleSyntaxReport` / `RuleSyntaxGuidance` 类型与 `web/src/api/rules.ts` / `web/src/api/group.ts` 的 `RuleMutationResponse` 透传
- `web/src/components/BifrostEditor/snippet/validation.ts`

## 测试方案

### 单元测试（按实际落地，名字与最初设计有调整）

- `cargo test -p bifrost-core rule::syntax_report::tests::valid_rule_returns_success_guidance` — 校验有效规则返回 `valid=true`、`rule_count>0`、`guidance.next_actions=[]`、summary="Rule syntax is valid."。
- `cargo test -p bifrost-core rule::syntax_report::tests::missing_reference_returns_e020_guidance` — 校验缺失 `@规则引用` 返回单条 `E020` 错误、`guidance.retryable=true`、summary 含 "Fix 1 rule syntax error"。
- `cargo test -p bifrost-cli rule::tests::validate_local_rule_syntax_reports_valid_rules` / `..._reports_missing_reference` / `..._rejects_invalid_port` / `..._rejects_invalid_status_code` / `..._allows_warning_only_rules` / `..._expands_existing_references`（共 6 个 CLI helper 单元测试，覆盖正确、缺失引用、端口错误、状态码错误、warning-only、catalog 展开）。
- 设计文档中提到的以下 admin/CLI 单测目前未单独落地（planned, not yet shipped as of 2026-06-17）；实际等价语义由 `bifrost-core` 单测 + e2e shell 脚本覆盖：
  - `cargo test -p bifrost-admin rule_syntax_valid_rule_returns_guidance_success`
  - `cargo test -p bifrost-admin rule_syntax_missing_reference_returns_e020_guidance`
  - `cargo test -p bifrost-admin create_rule_rejects_invalid_content_without_storage_write`
  - `cargo test -p bifrost-admin update_rule_rejects_invalid_content_preserves_existing_content`
  - `cargo test -p bifrost-cli rule_add_invalid_content_exits_without_write`

### E2E 测试（按实际落地）

实际以两个 `e2e-tests/tests/` 真实 shell 脚本承载（使用临时 `BIFROST_DATA_DIR` + 动态端口 + 当前分支 debug 二进制）：

- `e2e-tests/tests/test_rule_syntax_save_api.sh`（覆盖原设计的 `rule_save_returns_syntax_report_for_valid_content` / `rule_save_rejects_invalid_content_with_agent_guidance` / `rule_update_invalid_content_preserves_previous_rule` 三条语义）：真实启动 Bifrost，验证有效创建 200 + `saved=true` / `syntax.valid=true`；缺失 `@` 引用创建返回 422 且 `GET /api/rules/{name}` 为 404；缺失引用更新返回 422 且旧内容保持不变。
- `e2e-tests/tests/test_rule_syntax_cli.sh`（覆盖原设计的 `cli_rule_add_json_reports_syntax_error`）：验证有效新增返回 JSON `saved=true` / `syntax.valid=true`；缺失 `@` 引用新增退出码 2 且不落盘；缺失引用更新退出码 2 且旧内容保持不变；`--allow-invalid` 可显式保存。
- `group_rule_update_invalid_content_does_not_call_remote_write`：暂无独立 e2e 脚本（planned, not yet shipped as of 2026-06-17）；现阶段由 `validate_group_rule_for_save` 的代码路径配合 admin 单元测试间接保障（在调远端 `/v4/env` 前 422 出去）。

### 真实场景测试

实现阶段必须更新并执行：

- `human_tests/api-rules.md` 已新增 PASS 记录引用 `e2e-tests/tests/test_rule_syntax_save_api.sh`，覆盖：
  - 有效规则创建返回 `saved=true` / `syntax.valid=true`。
  - 错误规则（缺 `@` 引用）创建返回 HTTP 422 且 `GET /api/rules/{name}` 为 404。
  - 错误规则更新返回 HTTP 422 且旧内容保持不变。
  - 注：原设计中提议的 `TC-ARU-语法-01/02/03` 显式 TC 编号未直接落入文档（planned, not yet shipped as of 2026-06-17）；当前以单条 PASS 记录 + e2e 脚本覆盖等价语义。
- `human_tests/cli-rule-management.md` 已新增 PASS 记录引用 `e2e-tests/tests/test_rule_syntax_cli.sh`，覆盖：
  - 有效新增返回 JSON `saved=true` / `syntax.valid=true`。
  - 缺失 `@` 引用新增退出码 2 且不落盘。
  - 缺失引用更新退出码 2 且旧内容保持不变。
  - `--allow-invalid` 能显式保存。
  - 注：原设计中提议的 `TC-CRM-语法-01/02` 显式 TC 编号未直接落入文档（planned, not yet shipped as of 2026-06-17）。
- `human_tests/webui-rules.md`：保存错误规则的真实 WebUI 验证步骤（planned, not yet shipped as of 2026-06-17）。文件存在，但尚未追加该模块的 PASS 记录。
- `human_tests/readme.md`：保持索引同步即可，不维护全局数量。

## Review/Fix/Test 闭环方案

### 第 1 轮

- 目标复核：检查保存/更新 API、CLI、group rules、Agent JSON 诊断是否都覆盖。
- 代码 review：重点看 handler 复用是否避免 validate/create/update 三份逻辑漂移；422 是否不会触发 storage write、push invalidation 或远端 group env 写入。
- 测试运行：执行 admin handler 单元测试、CLI helper 单元测试、最小 E2E。
- 修复策略：优先修语义不一致、响应结构不稳定、旧内容被覆盖等问题。

### 第 2 轮

- 再次目标复核：对照第 1 轮问题和最新 diff，检查 warnings、不改 content 的 enabled 更新、`allow_invalid`、`@规则引用` catalog 覆盖。
- 再次代码 review：检查 WebUI API 类型、错误展示、human_tests 索引和设计文档是否同步。
- 复跑测试：复跑第 1 轮失败或修改相关测试，再执行 `cargo test --workspace --all-features`。

如任一轮仍发现保存无效规则、错误响应不可解析、group 远端先写入或 CLI 无法被 Agent 稳定解析，必须追加第 3 轮。

## 校验要求

实现阶段实际执行 / 仍需执行的命令（按已落地代码刷新）：

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test -p bifrost-core rule::syntax_report` — 覆盖 `validate_rule_syntax_report` 的 valid / missing-reference 行为。
- `cargo test -p bifrost-cli rule::tests` — 覆盖 CLI 端的 6 个 helper 单测。
- `cargo test -p bifrost-admin rule_syntax`（planned, not yet shipped as of 2026-06-17 —— 该 package 目前没有命中 `rule_syntax` 名称的单测，admin handler 行为以 e2e shell 脚本兜底；命令可保留以备未来补齐 admin handler 单测后启用）。
- `cargo test -p bifrost-e2e rule_save`（planned, not yet shipped as of 2026-06-17 —— 实际 e2e 走 `e2e-tests/tests/test_rule_syntax_save_api.sh` / `test_rule_syntax_cli.sh` shell 脚本，没有同名 Rust e2e 用例）。
- `BIFROST_BIN="$PWD/target/debug/bifrost" bash e2e-tests/tests/test_rule_syntax_save_api.sh`
- `BIFROST_BIN="$PWD/target/debug/bifrost" bash e2e-tests/tests/test_rule_syntax_cli.sh`
- `cargo test --workspace --all-features`
- 按 human_tests 文档逐条真实执行 API、CLI、WebUI 场景（见下方文档更新清单）。
- 根据修改范围最后执行 `scripts/ci/local-ci.sh`；如未执行必须说明原因和风险。

## 文档更新要求

实现阶段同步更新：

- `crates/bifrost-admin/ADMIN_API.md`：补充 mutation response 与 422 语法错误响应。
- `docs/rule.md` / `docs/rules/README.md`：补充保存时校验策略和 `allow_invalid` 逃生口。
- `human_tests/api-rules.md`
- `human_tests/cli-rule-management.md`
- `human_tests/webui-rules.md`
- `human_tests/readme.md`

## 风险与待确认问题

- 是否默认 strict：本方案建议 create/update 默认阻止 errors，warnings 允许保存；如果需要保留无效草稿，必须由调用方显式 `allow_invalid=true`。
- Group rules 远端 API 是否已有自己的语法校验未知；本方案仍在本地先校验，避免 Agent 收到远端不稳定错误。
- CLI 本地模式无法完整检查运行中服务的用户脚本列表；可先覆盖 parser、变量和引用检查，脚本缺失 warning 仅在 admin API 服务端具备完整能力时返回。
- 现有 WebUI store 忽略 mutation response body；实现阶段需要把 `syntax` 透传到错误展示和可能的通知文案。
