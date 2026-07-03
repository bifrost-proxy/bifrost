# Bifrost 文件协议 (.bifrost)

## 背景

`.bifrost` 是 Bifrost 全局唯一的结构化交换文件格式。它既承载本地规则文件（`Default.bifrost`、`my-rule.bifrost` 等落盘规则），也承载 WebUI/CLI 的导入导出（rules、network、script、values、template 五类 payload）。协议由一段固定 header + TOML meta + `---` 分隔符 + payload 组成，Parser 与 Writer 都在 `crates/bifrost-core/src/bifrost_file/` 目录下，管理端导入导出 HTTP handler 在 `crates/bifrost-admin/src/handlers/bifrost_file.rs`。

旧文档保留了很多理想化描述（例如“统一 `[options]`”、“全面替代所有旧 JSON 存储”、“默认容错”），没有完全反映 parser / writer / 管理端导入导出的真实行为。本设计以现有代码为准，把协议现状、每类文件真实字段、容错 API 分级、Network 空包边界以及权威实现位置写清楚，作为后续新增文件类型或调整字段时的唯一依据。

## 用户目标验证清单

### 必须实现

- Header 格式固定：`VV TYPE`，Writer 始终以 `{:02} {type}` 输出；当前 `BIFROST_FILE_VERSION = 1`，因此磁盘上第一行必然是 `01 <type>`。
- 支持 5 种 `BifrostFileType`（serde lowercase）：`rules` / `network` / `script` / `values` / `template`。
- 分隔符 `\n---\n`（严格模式）；容错模式可识别行尾 `\n---`。
- 每类文件由 header + TOML meta（部分类型含 `[options]`）+ `---` + payload 组成；rules payload 是原始规则文本，其他四类为 JSON。
- Writer 写出的 `[options]` 字段按类型固定，不假定跨类型一致。
- Parser 分级：严格 API（`parse_rules` / `parse_network` / `parse_script` / `parse_values` / `parse_template`），容错 API（`parse_tolerant` / `parse_rules_tolerant` / `parse_json_tolerant`）。
- Network 导入路径必须拒绝空数组：`network` 文件正文为 `[]` 时返回 400，导出请求必须携带至少一个 ID，WebUI 在选中数量为 0 时禁止发起导出。
- 导出请求中若任一 ID 已不存在，管理端返回 400，避免静默生成 0 记录或少记录 `.bifrost` 包。
- WebUI 导入 Toast 必须读 `record_count`；`success: true, record_count: 0` 也必须显示空包提示，不得自动跳转并套用 Imported 筛选。

### 必须不破坏

- 已经落盘的历史 `.bifrost` 文件（含旧 `[meta]` 字段、缺可选字段、旧 `[options]` 集合）能被严格 API 正常解析；不兼容字段升级要通过 `ParseWarning` 或容错 API，不硬破坏。
- rules 文件与 rules 语义（enabled / sort_order / sync 状态）继续可解、可诊断、可 round-trip。
- template 已经落地的 `groups + requests` 结构继续兼容；`groups` 为空时序列化省略字段。
- script 的 `script_type` 保持自由字符串，不引入枚举，允许 request / response / decode / 用户自定义值。
- CLI / Admin API / WebUI 的现有导入导出流程（含 body/response 全量导出、Imported 筛选）不因空包校验被误伤。

### 必须真实验证

- 单元测试覆盖 5 类 parse 严格 + 3 个容错 API + Network 空包/缺失 ID 校验。
- E2E 至少覆盖 `test_bifrost_file_syntax_admin_api.sh`（syntax 校验）+ Network 空包导入 400 的 admin API 回归。
- 真实场景：CLI `bifrost rule import/export`、WebUI Rules/Network 导入导出 Toast、Traffic 导出选中 0 条时的前端拦截。

## 产品语义

### `.bifrost` 是唯一交换格式

- 本地规则文件、WebUI 导入导出、CLI 导入导出、Share URL 打包，都统一走 `.bifrost`。
- 不同类型（rules / network / script / values / template）通过 header 的 `<type>` 段落区分；严格 API 会在 payload 前先校验 header 类型与调用意图一致，否则返回 `ParseError::TypeMismatch`。
- 未来新增类型（例如 `agent-session`）必须扩展 `BifrostFileType` 枚举，并同步扩展 writer / parser / handler / WebUI；不允许在同一类型下夹带跨语义数据。

### `[meta]` 与 `[options]` 分工

- `[meta]` 描述 payload 的元信息（name、version、created_at、updated_at、可选 description、sync 状态、rule enabled/sort_order 等），供 Writer/Parser 与 UI 展示消费。
- `[options]` 是 Writer 出具的辅助计数与选项，用于展示/统计；Parser **不能** 把 `[options]` 当权威事实，权威事实以 payload 本身为准。

### `[options]` 字段按类型固定，不统一

Writer 输出的 `[options]` 各类型不同：

- `rules`：`rule_count`（按非空、非注释行计数）。
- `network`：`count`、`include_body = true`、`include_response = true`。
- `script` / `values`：`count`。
- `template`：`request_count`、`group_count`。

导入端**不应假定** `[options]` 存在同一字段集，也不应把它当作校验依据；例如 Network 空包校验只看 payload 数组长度，不看 `count`。

### 容错 API 是有分级的

- 严格 API：任何缺字段、非法 TOML、payload 类型错、header 类型不匹配都会返回 `ParseError`；用于程序化调用与集成测试。
- 容错 API：只有 3 个：
  - `parse_tolerant(content) -> ParseResultWithWarnings<BifrostFileRaw>`：raw 层，尝试补默认 header、推断类型。
  - `parse_rules_tolerant(content) -> ParseResultWithWarnings<RuleFile>`：rules 专用容错。
  - `parse_json_tolerant::<T>(content) -> ...`：通用 JSON payload 容错，修复尾逗号、未闭合括号。
- 其他类型（network / script / values / template）**没有**对应的 tolerant 入口；上层不应假设它们能自动降级恢复，只能使用严格 API 或先走 `parse_tolerant` 拿 raw 再手工拼装。

### Network 空包必须显式失败

- 空包（`[]`）导入：即使是合法 JSON，也返回 400，Body 说明「文件包含 0 条记录」；不能显示成功 Toast。
- Network 导出：请求必须包含至少一个选中的 Traffic 记录 ID；服务端 handler 与 WebUI 公共导出入口都要在发起请求前检查数量。
- 导出时任一 ID 已不存在：管理端返回 400，避免静默生成少记录或 0 记录的 `.bifrost` 包。
- WebUI 导入成功 Toast：必须读 `record_count`；若服务端兼容旧路径返回 `success: true, record_count: 0`，前端仍显示空包提示，不做「Imported 筛选跳转」。

## 技术细节

### 头部与常量

```rust
// crates/bifrost-core/src/bifrost_file/types.rs
pub const BIFROST_FILE_VERSION: u8 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BifrostFileType {
    Rules,
    Network,
    Script,
    Values,
    Template,
}

pub struct BifrostFileHeader { pub version: u8, pub file_type: BifrostFileType }
pub struct BifrostFile<M, T> { /* header + meta + payload */ }
pub struct BifrostFileRaw { /* header + meta_raw (String) + payload_raw (String) */ }
```

Writer 逐字节输出 `format!("{:02} {}\n", header.version, header.file_type)`。Parser 读取第一行按空格切分，非法版本或未知类型返回 `ParseError::InvalidHeader`。

### 每类文件 meta

- `RuleFileMeta`：`name`、`enabled`、`sort_order`、`version`、`created_at`、`updated_at`，可选 `description`、`group`，子表 `[meta.sync]`（`rule_id`、`status`、`last_synced_at` 等）。payload 为原始规则文本。
- `ExportMeta`（network / script / values / template 通用）：`name`、`version`、`created_at`、可选 `description`。payload 为 JSON。

### payload 类型

- `rules`：原始文本。
- `network`：`Vec<NetworkRecord>`；`NetworkRecord` 含 request/response/body/matched_rules 等字段（`MatchedRuleExport` / `ActiveRuleExport` 详见 types.rs）。
- `script`：`Vec<ScriptItem>`；`ScriptItem.script_type` 为自由字符串（例如 `request` / `response` / `decode` / 用户自定义）。
- `values`：`ValuesContent`（KV 集合的稳定序列化 shape）。
- `template`：`TemplateContent { groups: Vec<ReplayGroupExport>, requests: Vec<ReplayRequestExport> }`；`groups` 为空时 `#[serde(skip_serializing_if = "Vec::is_empty")]`。

### Parser API

```rust
impl BifrostFileParser {
    // 严格
    pub fn parse_rules(&str) -> Result<RuleFile, ParseError>;
    pub fn parse_network(&str) -> Result<BifrostFile<ExportMeta, Vec<NetworkRecord>>, ParseError>;
    pub fn parse_script(&str) -> Result<BifrostFile<ExportMeta, Vec<ScriptItem>>, ParseError>;
    pub fn parse_values(&str) -> Result<BifrostFile<ExportMeta, ValuesContent>, ParseError>;
    pub fn parse_template(&str) -> Result<BifrostFile<ExportMeta, TemplateContent>, ParseError>;

    // 容错（仅 3 个）
    pub fn parse_tolerant(&str) -> ParseResultWithWarnings<BifrostFileRaw>;
    pub fn parse_rules_tolerant(&str) -> ParseResultWithWarnings<RuleFile>;
    pub fn parse_json_tolerant<T: DeserializeOwned + Default>(&str) -> ParseResultWithWarnings<T>;
}
```

`ParseError` 枚举覆盖：`InvalidHeader`、`TypeMismatch`、`InvalidToml`、`InvalidJson`、`MissingSeparator`、`MissingMeta`、`MissingPayload`、`Custom(String)`。

`ParseResultWithWarnings<T>` 包含 `data: T` + `warnings: Vec<ParseWarning>`；`WarningLevel = Info | Warn | Error`。

### Writer API

```rust
impl BifrostFileWriter {
    pub fn write_rules(rule: &RuleFile) -> String;
    pub fn write_network(meta: &ExportMeta, records: &[NetworkRecord]) -> String;
    pub fn write_script(meta: &ExportMeta, items: &[ScriptItem]) -> String;
    pub fn write_values(meta: &ExportMeta, values: &ValuesContent) -> String;
    pub fn write_template(meta: &ExportMeta, template: &TemplateContent) -> String;
}
```

Writer 使用严格 TOML 字符串转义（含多行字符串），保证 round-trip 稳定。

### 管理端 handler

`crates/bifrost-admin/src/handlers/bifrost_file.rs::handle_bifrost_file`：

- 路由：`GET|POST /_bifrost/api/bifrost-file/*`，含 syntax check、import、export 分支。
- 响应结构（导入）：`{ success, record_count: Option<usize>, warnings: [...] }`。
- Network 导入校验：`validate_network_import_records(record_count) -> Result<(), &'static str>`；`record_count == 0` 直接 400，Body `"Network file contains 0 records"`。
- 导入循环若统计到 record_count > 0 才返回 `success: true`；record_count 为 0 时仍返回 success=false + 说明。
- 导出：入参必须携带非空 ID 数组，任一 ID 未命中直接 400。

## CLI / Web / Admin API 接触面

- CLI：
  - `bifrost rule import|export` 走 rules writer/parser。
  - `bifrost traffic export|import` 走 network writer/parser（导出必须选中 ID）。
  - `bifrost script|values|template` 类命令走对应 writer/parser。
- WebUI：
  - Rules 页导入导出使用 rules parser；导入时若 rules meta 名字为 `Default` 走 Default 归一化路径（见 `default-global-rule.md`）。
  - Network 页导入导出使用 network parser + handler 空包校验；WebUI 公共导出入口在发起请求前检查选中数量。
  - Script/Values/Template 页各自使用对应 parser/writer。
- Admin API：
  - `POST /_bifrost/api/bifrost-file/import`：Body 为 `.bifrost` 文本；根据 header 分发到对应 parser + handler。
  - `POST /_bifrost/api/bifrost-file/export`：Body 为导出请求（ID 数组、include_body 等），Handler 组装 meta + payload → writer 生成 `.bifrost` 文本。
  - `POST /_bifrost/api/bifrost-file/syntax`：只做语法校验，返回 `ParseResultWithWarnings` 序列化。

## Sync 边界

- rules 类型 `.bifrost` 参与 rule sync（`[meta.sync]` 存 `rule_id / status / last_synced_at`），但 `Default.bifrost` 不参与，见 `default-global-rule.md`。
- network / script / values / template 类型 `.bifrost` 是纯本地导入导出物，**不参与** Sync；不应把它们塞进 rule sync channel。
- Share URL：rules 类型可分享（含 Default 的特殊策略）；其他类型的 Share URL 需要单独设计权限与内容脱敏，不在本协议范围。

## 实现切分

### Phase 1：类型 / header / 严格 parser

- `types.rs` 定义 `BifrostFileType`、`BifrostFileHeader`、`BifrostFile`、`RuleFileMeta`、`ExportMeta`、payload 类型、`ParseError`、`ParseResultWithWarnings`。
- `parser.rs` 实现严格 5 类 parse + `TypeMismatch` / `InvalidToml` / `InvalidJson` 错误。
- 单测：round-trip、TypeMismatch、Invalid TOML、Invalid JSON。

### Phase 2：Writer 与 `[options]`

- `writer.rs` 实现 5 类 write；各类型 `[options]` 字段固定。
- 单测：writer 输出 header 版本 = 01；writer + strict parser 双向 round-trip 稳定。

### Phase 3：容错 API

- `parse_tolerant` 补默认 header、推断类型。
- `parse_rules_tolerant` 兼容缺字段（回退 default meta，并回填 warnings）。
- `parse_json_tolerant` 修复尾逗号、未闭合括号。
- 单测：缺 header、legacy record 缺 `active_rules`、坏 JSON 修复成功。

### Phase 4：管理端 handler + Network 空包 + WebUI Toast

- `handlers/bifrost_file.rs` 实现 import/export/syntax；`validate_network_import_records`。
- Network 导入空包 400；导出空 ID 400；缺 ID 400。
- WebUI Rules/Network 页读 `record_count`，`0` 显示空包提示，不跳转 Imported。
- 单测：`validate_network_import_records(0)` 返回错误。
- E2E：`test_bifrost_file_syntax_admin_api.sh`；新增 Network 空包导入回归。

## 测试方案

### 单元测试

`crates/bifrost-core/src/bifrost_file/parser.rs::tests`（部分列举）：

- `test_parse_rules`：完整 rules `.bifrost` round-trip。
- `test_parse_tolerant_missing_header`：无 header 时容错补 default 并 warn。
- `parse_network_accepts_legacy_record_without_active_rules`：legacy record 不含 `active_rules` 也能解。
- `parse_rules_detects_type_mismatch`：header 类型与 API 不一致返回 TypeMismatch。
- `parse_rules_errors_on_invalid_toml_meta`：非法 TOML 返回 InvalidToml。
- `parse_script_round_trip_and_errors`：script round-trip + 类型 mismatch + 错 JSON。
- `parse_values_round_trip_and_mismatch`。
- `parse_template_round_trip_and_mismatch`。

`crates/bifrost-admin/src/handlers/bifrost_file.rs::tests`：

- `validate_network_import_records(0)` 返回错误。
- 导入 network 空数组返回 400。
- 导出请求空 ID 返回 400。
- 导出请求缺失 ID 返回 400。

### E2E 测试

- `e2e-tests/tests/test_bifrost_file_syntax_admin_api.sh`：syntax 校验路径。
- 新增 `test_bifrost_file_network_empty_import.sh`：真实 admin 起停，POST 空 network `.bifrost`，断言 400 + 错误 body 含 `0 records`。
- 新增 `test_bifrost_file_network_export_missing_id.sh`：导出请求带无效 ID，断言 400。

### 真实场景测试 human_tests

`human_tests/bifrost-file-protocol.md`（新增或补齐）：

- TC-BFP-01：Rules 导出/导入 round-trip 一致。
- TC-BFP-02：Network 导出选中 1 条成功；选中 0 条前端拒绝调用 API。
- TC-BFP-03：手工构造 network `.bifrost` payload = `[]`，WebUI/CLI 导入均显示空包提示，无 Imported 跳转。
- TC-BFP-04：template 含 `groups + requests` 混合结构导出/导入。
- TC-BFP-05：script `script_type` 为 `decode` 的条目导入生效。

human_tests 服务启动使用临时 `BIFROST_DATA_DIR`、非 9900 端口、`BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`、`BIFROST_DISABLE_TRAY=1`、`--no-system-proxy`。

### 覆盖率与项目校验

- `cargo fmt --all -- --check`
- `cargo test -p bifrost-core bifrost_file`
- `cargo test -p bifrost-admin bifrost_file`
- `cargo test --workspace --all-features`
- `rust-project-validate`

no-local-coverage 本地约定时不运行 `make coverage`，交付说明豁免。

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核用户目标：5 类文件严格 parser、Writer `[options]` 固定、容错 API 分级、Network 空包 400、WebUI Toast 读 record_count。
- 复核 diff：types / parser / writer / handler / WebUI 是否闭环。
- 重点 review：
  - 严格 API 与容错 API 边界是否清楚，是否有隐式 fallback；
  - Writer 输出 header 与 `[options]` 是否严格遵循每类固定字段；
  - Network 导入的 `validate_network_import_records` 是否覆盖 handler 所有入口。
- 复测：单元 + E2E。

### 第 2 轮

- 复核第 1 轮修复。
- 再看 `git status --short` / `git diff`：human_tests 索引、README / docs 中的旧描述是否同步。
- 重点 review：
  - 旧文档「统一 `[options]`」「全面替代 JSON 存储」等表述是否移除；
  - template `groups` 为空时序列化省略是否 round-trip 一致；
  - script `script_type` 保持自由字符串。
- 复测：human_tests TC-BFP-01..05 全部真实跑；WebUI 导入 Toast 与 record_count 联动手动核对。

## 风险与决策

- **`[options]` 不统一**：这是历史决策；强行统一会破坏现有导出文件。方案是文档明确「不假定跨类型统一」，导入端只把 `[options]` 当展示辅助。
- **容错 API 只覆盖 3 类**：network / script / values / template 没有 tolerant 入口。未来若上游数据源不稳定，可以增加 `parse_network_tolerant` 等，但必须以真实场景需求为触发，不做投机式扩容。
- **Network 空包 400**：明确破坏「宽容成功」的旧行为，是因为静默成功会让用户误以为已同步；改为显式失败风险可控（前端已经在选中 0 条时拦截，正常路径不会触发 400）。
- **BIFROST_FILE_VERSION = 1**：任何 breaking 变更必须先 bump 版本，并让 Parser 支持读多版本；Writer 只输出当前版本。
- **权威实现位置**：所有字段、错误码、容错路径以以下文件为准，文档与其冲突时以代码为准：
  - `crates/bifrost-core/src/bifrost_file/types.rs`
  - `crates/bifrost-core/src/bifrost_file/parser.rs`
  - `crates/bifrost-core/src/bifrost_file/writer.rs`
  - `crates/bifrost-admin/src/handlers/bifrost_file.rs`
