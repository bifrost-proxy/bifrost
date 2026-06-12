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

### 缺口

- 校验逻辑嵌在 HTTP handler 内，不能被 create/update/group rules/CLI 复用。
- 保存成功响应只有 `message`，没有 `syntax` 诊断；保存失败响应也只有字符串错误。
- API 没有明确区分“存储失败”和“规则语法失败”，Agent 无法稳定决策是否应改规则后重试。
- CLI 本地写入绕过 admin validate API，Agent 用 CLI 管理规则时也学不到语法错误。

## 技术方案

### 1. 抽出规则语法检查服务

新增 admin 侧服务模块，建议路径：

- `crates/bifrost-admin/src/rule_syntax.rs`

核心类型：

```rust
pub struct RuleSyntaxInput {
    pub source_name: String,
    pub content: String,
    pub global_values: HashMap<String, String>,
    pub current_group_name: Option<String>,
}

pub struct RuleSyntaxReport {
    pub valid: bool,
    pub rule_count: usize,
    pub errors: Vec<ParseError>,
    pub warnings: Vec<ParseError>,
    pub defined_variables: Vec<VariableInfo>,
    pub script_references: Vec<ScriptReference>,
    pub guidance: RuleSyntaxGuidance,
}

pub struct RuleSyntaxGuidance {
    pub summary: String,
    pub retryable: bool,
    pub next_actions: Vec<String>,
}
```

服务逻辑从现有 `validate_rule` handler 提取：

1. 从 `state.rules_storage.load_all_with_subdirs()` 构建 reference catalog。
2. 把当前待保存内容以 `source_name` 覆盖进 catalog，确保 `@当前规则` / 同名更新用新内容校验。
3. 调用 `expand_rule_references_strict`。
4. 调用 `validate_rules_with_context`。
5. 如果有 `script_manager`，追加缺失脚本 warning。
6. 根据错误码和 suggestion 生成 `guidance.summary` 与 `guidance.next_actions`。

现有 `POST /api/rules/validate` 改为调用该服务，保持响应字段兼容，只新增 `guidance` 字段。

### 2. 保存/更新 API 响应契约

新增统一响应结构：

```json
{
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
        "Read syntax.errors[0].line and syntax.errors[0].suggestion.",
        "Edit the rule content, then retry the same create or update request."
      ]
    }
  }
}
```

兼容策略：

- `POST /api/rules`、`PUT /api/rules/{name}` 默认 strict：有 `errors` 时不保存，返回 422。
- `warnings` 不阻止保存，但随成功响应返回，便于 Agent 后续优化。
- 如产品需要保留“保存无效草稿”，显式增加请求字段 `allow_invalid: true`；该模式仍返回 `syntax.valid=false` 和 `saved=true`，但必须由调用方明确选择。
- WebUI 普通保存默认使用 strict；若后续要支持草稿，应新增“保存草稿”语义，而不是静默保存无效启用规则。

### 3. API 入口改造

本地规则：

- `create_rule`
  - 解析请求后先检查 name/existence。
  - 调用 `RuleSyntaxService::validate_for_save`。
  - errors 非空且 `allow_invalid != true` 时返回 422，不写 storage，不触发 push。
  - 通过后保存，并返回 `RuleMutationResponse { saved: true, syntax }`。
- `update_rule`
  - 仅 `content` 变化时执行校验；只改 `enabled` 时不重复解析内容。
  - 如果 `enabled` 从 false 改 true，建议校验现有内容，避免启用一条已损坏规则。

组规则：

- `handle_create_rule` / `handle_update_rule`
  - 在调用远端 `/v4/env` 前执行本地语法检查。
  - `source_name` 使用 `group_name/rule_name`，与 `@组名/规则名` reference catalog 语义一致。
  - strict 失败时直接返回 422，不调用远端。
  - 成功后再提交远端，并在返回的 `GroupRuleDetail` 外层增加 `syntax`。

独立校验：

- 保留 `POST /api/rules/validate`。
- 可新增别名 `POST /api/rules/check`，语义更偏 service；但不是必须，避免 API 面扩大。

### 4. CLI 改造

`bifrost rule add/update` 在本地写入前调用 core 级校验 helper：

- 新增 `bifrost-cli` 内部 helper，复用 `RuleParser`、`validate_rules_with_context`、`expand_rule_references_strict` 和 `RulesStorage::load_all_with_subdirs()`。
- 默认 strict：errors 非空时不写入，exit code 使用 `2` 表示规则语法错误。
- 增加 `--allow-invalid` 作为显式逃生口。
- 增加 `--json`，输出与 API 同构的 `syntax` 对象，供 Agent 稳定解析。

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

## 依赖项

- `bifrost-core::validate_rules_with_context`
- `bifrost-core::expand_rule_references_strict`
- `bifrost-core::ParseError` / `ValidationResult`
- `bifrost_storage::build_rule_reference_catalog`
- `RulesStorage::load_all_with_subdirs`
- `bifrost-admin` 的 `SharedAdminState` 与 `script_manager`
- `web/src/components/BifrostEditor/snippet/validation.ts`

## 测试方案

### 单元测试

- `cargo test -p bifrost-admin rule_syntax_valid_rule_returns_guidance_success`
  - 校验有效规则返回 `valid=true`、`rule_count>0`、`guidance.next_actions=[]`。
- `cargo test -p bifrost-admin rule_syntax_missing_reference_returns_e020_guidance`
  - 校验缺失 `@规则引用` 返回 `E020`，包含 retryable guidance。
- `cargo test -p bifrost-admin create_rule_rejects_invalid_content_without_storage_write`
  - 调用 create handler，断言 422、`saved=false`，storage 中不存在该规则。
- `cargo test -p bifrost-admin update_rule_rejects_invalid_content_preserves_existing_content`
  - 先保存有效规则，再更新为无效内容，断言原内容未变。
- `cargo test -p bifrost-cli rule_add_invalid_content_exits_without_write`
  - CLI helper 层验证无效规则不会写入本地 storage。

### E2E 测试

使用 `.agents/skills/e2e-test/` 规范补充：

- `rule_save_returns_syntax_report_for_valid_content`
  - 真实启动 Bifrost，`POST /api/rules` 创建有效规则，断言 200、`saved=true`、`syntax.valid=true`。
- `rule_save_rejects_invalid_content_with_agent_guidance`
  - 创建错误协议规则，断言 422、`saved=false`、`syntax.errors[0].line`、`suggestion`、`guidance.retryable=true`。
- `rule_update_invalid_content_preserves_previous_rule`
  - 更新已有规则为无效内容，断言 API 422 后 `GET /api/rules/{name}` 仍是旧内容。
- `group_rule_update_invalid_content_does_not_call_remote_write`
  - 使用 group rules mock/fixture 验证 strict 失败发生在远端提交前。
- `cli_rule_add_json_reports_syntax_error`
  - 真实 `cargo run --bin bifrost -- rule add ... --json`，断言 exit code 2 和 JSON 诊断。

### 真实场景测试

实现阶段必须更新并执行：

- `human_tests/api-rules.md`
  - 新增 `TC-ARU-语法-01`：有效规则创建返回 `syntax.valid=true`。
  - 新增 `TC-ARU-语法-02`：错误规则创建返回 422、`saved=false`、`syntax.errors` 和 `guidance.next_actions`。
  - 新增 `TC-ARU-语法-03`：错误规则更新不会覆盖原规则内容。
- `human_tests/cli-rule-management.md`
  - 新增 `TC-CRM-语法-01`：`bifrost rule add --json` 对错误规则返回 exit code 2 和 JSON 诊断。
  - 新增 `TC-CRM-语法-02`：`--allow-invalid` 能显式保存，但输出 `syntax.valid=false`。
- `human_tests/webui-rules.md`
  - 新增保存错误规则的真实 WebUI 验证，确认错误来自保存 API 响应并能被用户看到。
- `human_tests/readme.md`
  - 只更新相关模块索引行，不维护全局数量。

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

实现阶段必须执行：

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test -p bifrost-admin rule_syntax`
- `cargo test -p bifrost-cli rule_`
- `cargo test -p bifrost-e2e rule_save`
- `cargo test --workspace --all-features`
- 按 human_tests 文档逐条真实执行 API、CLI、WebUI 场景。
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
