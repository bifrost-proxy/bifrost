# Shadowrocket / Surge 规则兼容与自动转换

> 实现状态：设计中 (planned)。截至 2026-07-03，仓库内尚未出现 `crates/bifrost-core/src/external_rule/` 或
> `crates/bifrost-sync/src/external_rule/` 模块，也没有 `parse_shadowrocket.rs`、`detect.rs`、`ProfileAst` 等实现符号。
> `crates/bifrost-sync/src/normalize.rs` 只负责 legacy alias 与 `${var}` 展开，`crates/bifrost-core/src/rule/parser`
> 只解析原生 Bifrost 语法。前端 Rules Editor 也没有 paste 识别或导入类型选择器。本文档描述目标设计与落地边界。

## 背景

- Bifrost 具备统一规则语言、matcher、改写、脚本、TLS 控制以及远端规则归一化能力，具备承接外部规则生态的基础。
- 目标用户常见需求不是手写 Bifrost 规则，而是直接复用现有 Shadowrocket / Surge 规则、规则集与改写配置。
- 项目已有远端规则归一化入口 (`crates/bifrost-sync/src/normalize.rs`, `crates/bifrost-sync/src/manager.rs`)，
  适合在导入链路上新增 Shadowrocket 配置识别、解析、转换与告警输出，而不是把适配逻辑散落到多个入口。
- 若采用“兼容子集 + 自动转换 + 明确告警”模型，用户可以在不放弃已有规则资产的前提下逐步迁移到 Bifrost。

## 用户目标验证清单

### 必须实现

- Rules 编辑器粘贴 Shadowrocket / Surge 文本时能自动识别来源，并弹出确认后再插入转换结果。
- 规则导入弹窗允许显式选择来源类型：`Bifrost` / `Legacy` / `Shadowrocket / Surge` / `自动识别`。
- 支持首期兼容矩阵中的规则类型 (`DOMAIN`、`DOMAIN-SUFFIX`、`DOMAIN-KEYWORD`、`IP-CIDR`、`IP-CIDR6`、
  `RULE-SET`、`DOMAIN-SET`、`FINAL,DIRECT`、`FINAL,<static-proxy>`) 与常见改写 (URL Rewrite / Header Rewrite /
  Body Rewrite / MITM / Map Local)。
- 每次识别或导入返回结构化转换报告 (`stats + warnings + report.items`)，含行号与原始规则片段。
- `RULE-SET` / `DOMAIN-SET` 在导入时拉取并展开为普通 Bifrost 规则，遇到不支持子规则时跳过并告警。
- 转换结果能被现有 Bifrost parser 与 matcher 直接消费，无需二次改写。

### 必须不破坏

- 未选择 Shadowrocket 类型的普通粘贴与导入行为与现在完全一致。
- `.bifrost` 文件导入接口 (`/bifrost-file/detect`、`/bifrost-file/import`) 保持原语义，不把 Shadowrocket
  识别硬塞进 `.bifrost` 类型检测。
- 现有 `crates/bifrost-sync/src/normalize.rs` 的 legacy 变量展开、`@user/name`、`ignore://`、`enable://intercept`
  行为不变。
- Group 规则与远端 Sync 链路不因新引入的 external rule 层而改变消费口径。
- 已有的 matcher specificity（`@important`、精确 > 泛匹配）保持不变。

### 必须真实验证

- 单元测试覆盖 AST 解析、每一类规则/改写的转换、`RULE-SET` 展开、DOMAIN-SUFFIX 裸域+子域拆分。
- 前端交互测试验证粘贴识别、确认取消、导入类型选择、warnings 展示、`按原文粘贴` 兜底。
- E2E 覆盖“导入一份真实 Shadowrocket profile → 生成 Bifrost 规则 → 命中真实代理请求”的端到端链路。
- 转换报告字段 (`action=converted|approximated|skipped|unsupported|kept`) 稳定，可被测试断言。

## 产品语义

### 兼容目标：子集 + 报告，不是完整运行时

Shadowrocket / Surge 与 Bifrost 的核心差异是运行时模型：

- Shadowrocket 是“匹配规则后交给 policy / policy group 决策上游出口”。
- Bifrost 是“匹配规则后直接执行具体动作”，上游出口以 `proxy://host:port`、`host://`、`passthrough://` 等
  显式协议表达。

第一版不复刻 policy group 运行时，只落地：

- `DIRECT` → `passthrough://`
- 单一静态代理策略 → `proxy://host:port`
- 无法静态解析的策略 → 转换报告标记 `unsupported_dynamic_policy`，规则文本注释保留原策略名。

### 识别策略：分层且保守

1. 先识别明确的 Bifrost / `.bifrost` 内容（现有 tolerant parser 判定）。
2. 再识别 legacy 语法（`${var}`、`@user/name`、`ignore://host|rule`、`enable://intercept`）。
3. 再识别 Shadowrocket / Surge profile（`[Rule]` / `[URL Rewrite]` / `[MITM]` section 头、`DOMAIN-SUFFIX,` /
   `RULE-SET,` / `IP-CIDR,` 三列逗号格式）。
4. 无法确认时保持原样，不做任何破坏性替换。

识别器返回 `confidence: high | medium | low`。UI 只在 `high` 时默认建议转换；`medium` 需要用户确认；
`low` 建议用户手动选择来源类型。

### 兼容矩阵（首期）

| Shadowrocket / Surge 能力 | 首期策略 | Bifrost 落地方式 |
| --- | --- | --- |
| `DOMAIN` | 直接支持 | 精确 pattern |
| `DOMAIN-SUFFIX` | 近似展开 | 裸域 + `**.` 子域两条规则 |
| `DOMAIN-KEYWORD` | 近似支持 | host regex |
| `IP-CIDR` / `IP-CIDR6` | 直接支持 | IP / CIDR pattern |
| `FINAL,DIRECT` | 直接支持 | `* passthrough://` |
| `FINAL,<static-proxy>` | 直接支持 | `* proxy://host:port` |
| `RULE-SET` | 直接支持 | 导入时拉取并展开 |
| `DOMAIN-SET` | 直接支持 | 导入时拉取并展开 |
| `URL Rewrite` | 直接/近似 | `redirect://` / `urlReplace://` (302/307 走 `redirect://`) |
| `Header Rewrite` | 直接支持 | `reqHeaders` / `resHeaders` / `delete` / `headerReplace` |
| `Body Rewrite` | 直接/近似 | `reqReplace` / `resReplace`；复杂替换降级为脚本 |
| `MITM hostname` | 直接支持 | `tlsIntercept://` / `tlsPassthrough://` |
| `Map Local` | 直接支持 | `file://` / `tpl://` / `rawfile://` |
| `SCRIPT` 改写 | 近似支持 | `reqScript://` / `resScript://` |
| `USER-AGENT` | 近似支持 | `includeFilter://h:User-Agent=...` |
| `GEOIP` / `PROCESS-NAME` / `SRC-IP` / `IN-PORT` | 暂不支持 | 报告标记 unsupported，规则跳过 |
| `AND/OR/NOT` 复合逻辑 | 暂不支持 | 报告标记 unsupported |
| Policy Group (`select` / `url-test` / `fallback` / `load-balance`) | 暂不支持 | 报告标记 unsupported_dynamic_policy |

### 优先级模型差异

- Shadowrocket 是严格 first-match，Bifrost matcher 优先级基于 pattern specificity + `@important`。
- 转换器不改变 matcher 层语义，但当上游 Shadowrocket 规则依赖 first-match 抢占语义时（例如上层 regex
  希望先抢先命中），转换报告必须显式告警 `priority_model_mismatch`，避免 silent drift。

## 技术细节

### 模块划分

新增独立解析层，避免和 Sync 混在一起：

- `crates/bifrost-core/src/external_rule/`
  - `detect.rs` — 识别来源类型 + confidence
  - `ast.rs` — `ProfileAst / SectionAst / RuleItemAst / RewriteItemAst`
  - `parse_shadowrocket.rs` — Shadowrocket / Surge profile parser
  - `convert.rs` — 每一类 AST → Bifrost rule text
  - `report.rs` — `ConversionReport / ReportItem`
- `crates/bifrost-sync/src/normalize.rs` 不新增外部规则识别；改由 admin 导入 handler 显式调用 external_rule 层。

### AST 表达

```rust
pub struct ProfileAst {
    pub sections: Vec<SectionAst>,
}

pub struct SectionAst {
    pub kind: SectionKind, // Rule / Ruleset / UrlRewrite / HeaderRewrite / BodyRewrite / Script / Mitm / MapLocal / Proxy / ProxyGroup / General
    pub line_start: u32,
    pub line_end: u32,
    pub items: Vec<ItemAst>,
}

pub enum ItemAst {
    Rule(RuleItemAst),
    Rewrite(RewriteItemAst),
    Mitm(MitmItemAst),
    MapLocal(MapLocalItemAst),
    Unknown { raw: String, line: u32 },
}
```

每一条 item 保留 `raw`、`line`、`section`，转换失败/近似/跳过时同时挂到 `ReportItem`。

### 转换结果结构

```json
{
  "source_type": "shadowrocket",
  "confidence": "high",
  "normalized_text": "example.com passthrough://\n**.example.com passthrough://",
  "warnings": ["DOMAIN-SUFFIX 已展开为裸域和多级子域两条规则"],
  "stats": {"converted": 1, "approximated": 1, "skipped": 0, "unsupported": 0},
  "report": {
    "items": [
      {
        "line": 1,
        "section": "Rule",
        "source": "DOMAIN-SUFFIX,example.com,DIRECT",
        "action": "approximated",
        "target": "example.com passthrough://\n**.example.com passthrough://",
        "level": "warning",
        "message": "suffix 语义通过裸域 + **.subdomain 双条展开实现"
      }
    ]
  }
}
```

`action` 建议值：`converted / approximated / skipped / unsupported / kept`。
`level` 建议值：`info / warning / error`。

### 策略解析归一化

```rust
pub enum ResolvedPolicy {
    Direct,
    StaticProxy(ProxyTarget),
    UnsupportedDynamic(String),
}
```

- `DIRECT` → `Direct`。
- `[Proxy]` 中静态定义、可直接映射为 `proxy://user:pass@host:port` 的策略 → `StaticProxy`。
- Policy Group (`select` / `url-test` / `fallback` / `load-balance`) → `UnsupportedDynamic(name)`；对应 `[Rule]`
  中命中此策略的规则被 skipped 并写入报告。

## CLI + Web + Admin API

### Rules 编辑器粘贴识别（Web UI）

- 监听 Monaco `onPaste`。
- 只把本次粘贴文本片段送到识别接口，不重扫整个 buffer。
- 若识别 confidence=high 且 `source_type∈{shadowrocket,surge}`：
  - 顶部弹出 inline banner：“已识别为 Shadowrocket/Surge 规则，是否转换为 Bifrost 规则？”
  - 按钮：`转换并插入` / `按原文粘贴` / `查看转换报告`。
- 若 confidence=medium：只在 banner 里默认选中“按原文粘贴”，用户主动选“转换并插入”才走转换。
- 若 confidence=low 或 `unknown`：不弹 banner，直接原文粘贴。

### 导入规则弹窗（Web UI）

- 新增 Rules 专用导入弹窗，与仅支持 `.bifrost` 的“上传文件”按钮分开。
- 弹窗字段：
  - 文本输入区 / 文件上传
  - 来源类型下拉：`自动识别` / `Bifrost` / `Legacy` / `Shadowrocket / Surge`
  - 规则名输入框
  - 解析预览区（Monaco 只读，展示 `normalized_text`）
  - warnings 区（红/黄/信息条）
  - 转换报告展开面板（按 section 分组，含 `action` 徽章、行号跳转）
- 操作按钮：`仅预览` / `转换并创建` / `取消`。

### Admin API 草案

- `POST /_bifrost/api/rules/external/detect`
  - 请求：`{ text, source_type: "auto" | "bifrost" | "legacy" | "shadowrocket" | "surge" }`
  - 响应：`{ source_type, confidence, normalized_text, warnings, stats, report }`
- `POST /_bifrost/api/rules/external/convert`
  - 请求：`{ text, source_type }`
  - 响应同上（`source_type` 用调用者指定值，无自动识别）。
- `POST /_bifrost/api/rules/external/import`
  - 请求：`{ name, content, source_type, save_mode }`
  - `save_mode`：`preview_only` / `create_rule` / `replace_existing`。
  - 响应：`{ rule: RuleFileInfo | null, conversion: ConversionResult }`。
- 现有 `/bifrost-file/detect` 与 `/bifrost-file/import` 保持只处理 `.bifrost`，不吸收外部规则识别。

### CLI（延后）

第一版不新增 CLI 命令。若后续要在 CLI 侧支持，可复用同一个 external_rule crate：

```
bifrost rule import --source shadowrocket --name imported ./config.conf
bifrost rule detect --source auto ./config.conf
```

CLI 输出应包含 `stats` 一行摘要和可选 `--report json` 完整报告。

## Sync 边界

- external rule 层只作用于**导入路径**，不参与远端 Sync 消费与推送。
- Sync 拉到的 `.bifrost` 内容仍走 `crates/bifrost-sync/src/normalize.rs`，不做任何 Shadowrocket 尝试。
- 导入完成、写入本地规则文件后，规则的后续 Sync/Share 与普通本地规则一致，不携带任何“来源=shadowrocket”
  的元信息（source metadata 只落在导入报告日志里，不写进规则文件）。
- 未来若引入“远端共享 Shadowrocket 源”，需要另起设计（`bifrost-shadowrocket-source.md`），不在本次范围。

## 实现切分

### Phase 1：AST 与识别

- 新增 `crates/bifrost-core/src/external_rule/{detect,ast,parse_shadowrocket,report}.rs`。
- 完成 section 解析、`RuleItemAst` / `RewriteItemAst` / `MitmItemAst` / `MapLocalItemAst`。
- 完成识别器 + confidence。
- 单元测试覆盖：section 识别、行号定位、item 级 fatal/item_error/item_warning 分类。

### Phase 2：转换核心

- 完成 `[Rule]` 常见类型转换。
- 完成 `RULE-SET` / `DOMAIN-SET` 拉取展开（网络访问走 `crates/bifrost-core/src/http_client.rs`）。
- 完成 URL / Header / Body Rewrite 转换。
- 完成 MITM / Map Local 转换。
- 完成 `ConversionReport` 生成与 `stats` 汇总。

### Phase 3：Admin API 接入

- 新增 `handlers/external_rule.rs`：detect / convert / import。
- 复用 `crates/bifrost-admin/src/state.rs` 里现有 rules storage + rules changed 通知链路。
- 导入落盘走现有 `RulesStorage::save`，转换报告只写日志与响应，不落规则文件。

### Phase 4：前端接入

- Rules 编辑器 paste hook + banner。
- 独立导入弹窗，与 `.bifrost` 上传按钮解耦。
- 转换报告面板：按 section 分组、`action` 徽章、行号跳转 Monaco 高亮。
- 交互文案与 warnings i18n 化。

## 测试方案

### 单元测试

- `crates/bifrost-core/src/external_rule/detect.rs`
  - `detect_bifrost_native_text_returns_high_confidence`
  - `detect_shadowrocket_profile_by_section_headers`
  - `detect_surge_profile_by_rule_prefix`
  - `detect_legacy_variables_returns_legacy_source`
  - `detect_unknown_text_keeps_original`
- `parse_shadowrocket.rs`
  - `parse_rule_section_produces_rule_items`
  - `parse_ruleset_expands_remote_source` (mock HTTP)
  - `parse_url_rewrite_extracts_action_and_pattern`
  - `parse_header_rewrite_supports_add_del_replace`
  - `parse_mitm_hostname_and_exclude`
- `convert.rs`
  - `convert_domain_produces_exact_pattern`
  - `convert_domain_suffix_expands_bare_and_wildcard`
  - `convert_domain_keyword_produces_host_regex`
  - `convert_final_direct_produces_star_passthrough`
  - `convert_final_static_proxy_produces_star_proxy`
  - `convert_url_rewrite_302_maps_to_redirect`
  - `convert_url_rewrite_replace_maps_to_url_replace`
  - `convert_header_rewrite_add_del_replace`
  - `convert_body_rewrite_simple_uses_req_res_replace`
  - `convert_body_rewrite_complex_downgrades_to_script`
  - `convert_mitm_produces_tls_intercept_and_passthrough`
  - `convert_map_local_file_tpl_rawfile`
  - `convert_geoip_marks_unsupported_and_skips`
  - `convert_policy_group_marks_unsupported_dynamic_policy`

### 前端交互测试

- `web/tests/ui/external-rule-paste.spec.ts`
  - 粘贴 Shadowrocket 文本 → banner 出现 → 点“转换并插入” → editor 内容为 `normalized_text`。
  - 粘贴 Shadowrocket 文本 → 点“按原文粘贴” → editor 内容为原文。
  - 粘贴 Bifrost 文本 → 无 banner。
  - Confidence=medium 时默认按“按原文粘贴”，需要用户主动切换才转换。
- `web/tests/ui/external-rule-import.spec.ts`
  - 选择 `Shadowrocket / Surge` → 预览区展示 `normalized_text` + warnings。
  - 选择 `Bifrost` → 不触发外部规则转换。
  - `仅预览` 不落盘，`转换并创建` 后 Rules 列表出现新规则。

### E2E 测试

- 新增 `e2e-tests/tests/test_external_rule_import_e2e.sh`
  - 启动临时 Bifrost，创建临时数据目录。
  - 通过 Admin API 导入一份内嵌 Shadowrocket profile fixture (`e2e-tests/fixtures/shadowrocket-basic.conf`)。
  - 断言返回 `stats.converted >= N`，规则文件存在。
  - 用真实代理请求命中转换后的规则，断言路由/改写生效。
- 新增 `e2e-tests/tests/test_external_rule_ruleset_expand_e2e.sh`
  - 启动本地 fixture HTTP 服务，暴露 `RULE-SET` 内容。
  - 导入指向该 URL 的 profile，断言展开后的 Bifrost 规则条数 = fixture 条数。

### 真实场景测试

新增 `human_tests/shadowrocket-rule-compatibility.md`：

- 使用真实用户提供的 Shadowrocket profile 走导入弹窗完整链路。
- 覆盖至少一份含 `RULE-SET`、一份含 `MITM`、一份含 `URL Rewrite` 的 profile。
- 覆盖 `按原文粘贴` 与 `转换并插入` 两条路径。

## Review / Fix / Test 闭环

- 第 1 轮：AST + 识别 + 基础规则转换单测通过，Admin API 接入本地手工验证。
- 第 2 轮：Rewrite / MITM / Map Local 转换单测通过，前端粘贴/导入交互测试通过。
- 第 3 轮：E2E 导入 + 真实代理命中通过，`human_tests/shadowrocket-rule-compatibility.md` 走完。
- 每轮结束都要复跑 `cargo fmt --all -- --check`、`cargo clippy --workspace --all-targets --all-features -- -D warnings`、
  `cargo test --workspace --all-features`。

## 风险与决策

- **风险：识别误判**。缓解：分层识别 + confidence + `按原文粘贴` 兜底 + 用户可显式选择来源类型。
- **风险：`DOMAIN-SUFFIX` 展开导致规则条数膨胀**。缓解：只展开为“裸域 + 一条 `**.` 通配”两条，不做多级枚举；
  转换报告显式告警。
- **风险：优先级模型差异**。缓解：转换报告标注 `priority_model_mismatch`，UI 高亮；不改动 matcher。
- **风险：`RULE-SET` 拉取失败或超时**。缓解：使用现有 `crates/bifrost-core/src/http_client.rs`，
  失败时 skip 该 section 并在报告中标 `error`，其它 section 继续。
- **风险：`.bifrost` 文件误识别为 Shadowrocket**。缓解：识别顺序先 Bifrost 后 Shadowrocket，且识别器优先
  尝试 Bifrost tolerant parser 的高置信度解析。
- **决策：本期不实现 policy group 运行时**。理由：与 Bifrost 现有 action-based 模型语义差异过大，
  首期投入产出比低；对应规则 skip 并显式告警。
- **决策：本期不承诺 `GEOIP` / `PROCESS-NAME` / `SRC-IP` / `IN-PORT`**。理由：这些依赖 OS 级信息，
  Bifrost 侧无对应 matcher，需要独立设计。
- **决策：external_rule 只在导入路径生效**。理由：避免污染 Sync 消费与运行时 matcher。

## 文档更新要求

- 本设计文档先落地。功能真正实现并对外暴露导入入口后，需要补充：
  - `README.md` / `README.en.md` 的能力说明。
  - `docs/` / `docs-en/` 中新增 `shadowrocket-import.md`，展示兼容矩阵、限制说明与常见迁移问题。
  - `human_tests/readme.md` 索引新增 `shadowrocket-rule-compatibility.md`。
- 实现前不修改上述用户可见文档，避免误导。
