# 规则 @ 引用与编辑器原位展开

## 背景

Bifrost 规则文件支持"在独立行写 `@规则名称` 或 `@组名称/规则名称`，运行时把被引用规则的完整内容在当前位置原位展开后再解析"。这是一种"shared rule"复用模式：

- 通用请求头 / 脚本 / 过滤器保存为 disabled 规则，避免它作为普通规则全局生效。
- 一个 enabled 入口规则通过 `@shared-headers`、`@team-scripts/log-injector` 等把 shared 规则的内容拼进自己。
- 引用是纯文本原位展开，运行时看到的最终规则就是"展开后的完整文本"。

引用能力覆盖后端解析（主代理 / 临时端口 / Replay / rule validate）和 Web UI 编辑器（Monaco decoration、hover 提示、点击展开、补全）。本设计目标是把引用识别、解析、诊断与编辑器体验统一定义，防止不同入口分叉。

## 用户目标验证清单

### 必须实现

- 独立行 `@规则名称` 与 `@组名称/规则名称` 被识别为引用；行首去除空白后以 `@` 开头且不是 `@comment`。
- 引用行支持任意空白后的 `#` 行内注释（`@shared # reuse` 与 `@shared\t# reuse` 都引用 `shared`）。
- 普通 `#` 行注释里出现的 `@xxx` 不参与引用识别。
- disabled 规则可以被引用；引用后它作为 shared 内容原位展开，不再单独 standalone 生效。
- 私有规则用短名引用 `@name`；组规则用 `@group/name` 全限定名引用，避免同名歧义。
- 嵌套引用可展开；缺失引用在运行时 skip 该引用行但继续解析后续规则；strict 场景返回 `E020`；循环引用让入口规则跳过解析并记录明确错误。
- Rules 编辑器在 Monaco 中把 `@` 引用 token 标为可点击 decoration；点击 view zone 在当前行下展开被引用规则只读内容，再次点击收起。
- Monaco hover 显示 validate 缺失引用错误（E020 映射回对应 `@` token 并标红）。
- 进入编辑器自动请求 `/api/rules/reference-candidates` 并注入 Monaco 补全；支持前缀、包含、非连续字符 fuzzy。

### 必须不破坏

- 不含 `@` 引用的规则文件行为不变。
- `@comment` 精确指令与 `commented-*` 规则名不被误认为引用。
- Group 规则原有的启停、共享、reorder 语义不变。
- Rule sync / share URL / import 对 `@` 引用透明；只关心规则文本本身。

### 必须真实验证

- Rust 单测覆盖识别、展开、缺失、循环、tab 注释、注释内 `@`。
- Playwright 覆盖 Monaco decoration / hover / 展开 / 补全。
- E2E 脚本走真实 Admin API + 代理请求验证展开后的 shared header 生效。

## 产品语义

### 独立行引用

引用行必须是"独立一行"：

- 去除首尾空白后以 `@` 开头。
- 精确等于 `@comment` 的指令行不算引用（它保留原有指令语义）。
- 支持行内注释：`@shared` 后可跟任意空白 + `#`；`#` 及其后内容忽略。

### 普通规则 vs 组规则命名

- 私有规则短名：`@shared-headers`。
- 组规则全限定名：`@team-a/shared-headers`。
- 遇到同名歧义，全限定名优先；纯短名只匹配私有规则表。

### 展开语义

- 运行时以全部规则文件构造引用目录（含 disabled，方便被引用）。
- 只把 enabled 规则作为解析入口；disabled 规则不 standalone 生效。
- 展开是纯文本原位替换：`@shared` 一整行被 `shared` 规则的完整内容替代，然后一起进入 rule parser。
- 支持多层嵌套；循环引用检测在 catalog 里。

### 缺失与循环

- **运行时（宽松）**：缺失引用 skip 该行，其余规则继续解析生效。这是主代理 / 临时端口 / Replay 使用的语义。
- **Validate API（严格）**：缺失引用返回 `E020`，UI 用来标红。
- **循环引用**：入口规则整个跳过解析，并记录清晰错误（含涉及的规则名链）。

### Rules 编辑器

- Monaco decoration 只在独立引用行上出现；`@` token 高亮为可点击。
- 点击 view zone 在下方展开被引用规则文本，只读；再次点击收起。
- Hover：validate E020 错误信息附在对应 `@` token 上，红色底色。
- 补全：进入编辑器后拉 `/api/rules/reference-candidates`，返回私有规则短名与已缓存组规则全限定名；fuzzy 搜索命中即可选择。
- 亮色 / 暗色主题一致处理。

## 技术细节

### 引用目录构造

- `bifrost-core::rule::references` 提供纯文本引用识别与展开能力。
- `bifrost-storage::build_rule_reference_catalog(rules)`（`crates/bifrost-storage/src/rules.rs:265`）在所有解析入口构造引用目录。
- `RulesStorage::load_all_with_subdirs*` 读取组规则子目录时保留来源组名，让全限定名 `@group/name` 可以命中。

### 解析入口一览

- 主代理 rule hot reload：`crates/bifrost-cli/src/commands/start.rs:3579`。
- 临时端口绑定：`crates/bifrost-cli/src/commands/port.rs:135`。
- Rules Admin API list/get/update：`crates/bifrost-admin/src/handlers/rules.rs:1021`。
- Replay executor：`crates/bifrost-admin/src/handlers/replay.rs:2537` 与 `crates/bifrost-admin/src/replay_executor.rs:648`。
- CLI rule 子命令：`crates/bifrost-cli/src/commands/rule.rs:176`。

### 缺失与循环诊断

- `E020` 错误码定义与生成：`crates/bifrost-core/src/rule/parser/mod.rs:1188`、`crates/bifrost-core/src/rule/syntax_report.rs:74`。
- 单测覆盖 `E020`：`crates/bifrost-core/src/rule/parser/mod.rs:4822`、`crates/bifrost-cli/src/commands/rule.rs:337`。
- 错误码集合：`crates/bifrost-core/src/rule/parser/mod.rs:4934` 中含 `"E020"`。

### Reference candidates API

- `GET /api/rules/reference-candidates`：`crates/bifrost-admin/src/handlers/rules.rs:502`。
- 返回私有规则短名 + 组规则全限定名。前端 Monaco 用于补全。

## CLI + Web + Admin API

### CLI

- `bifrost rule show <name>`：展示规则原文（含 `@` 行）。
- `bifrost rule validate <name>`：走 strict 语义，缺失引用返回 `E020`。
- `bifrost port active <port>`：显示解析后的规则（含展开后的 shared 内容摘要）。

### Web

- Rules 编辑器：Monaco decoration、hover、click-expand view zone、补全。
- Rules 列表：disabled 规则被引用后，UI 提示"引用中"（可选，属于 Phase 3 的增强）。

### Admin API

- `GET /api/rules/reference-candidates`：返回引用候选清单。
- `POST /api/rules/validate`：执行 strict 展开与错误上报，含 `E020`。
- `GET /api/rules` / `PUT /api/rules/:name`：解析用宽松语义，缺失引用 skip 该行。

## Sync 边界

- 引用是"规则文本 + 引用目录"的组合；rule sync 只同步规则文本，引用目录在本机构造。
- 同步过来的规则如果引用了本机不存在的规则，运行时按缺失 skip；validate 时报 E020。
- Group sync 保留组名与规则名映射，全限定名 `@group/name` 跨设备语义稳定。
- Share URL：分享方带上被引用规则时需要将 `@` 全部内嵌为展开后的文本；或提示导入方缺失引用。默认 share URL 仅分享入口规则，不主动附带 shared，避免污染。

## Phase 1 — 后端识别与展开

- `bifrost-core::rule::references` 实现独立行识别、`@comment` / `commented-*` / tab 注释 / 注释内 `@` 特例、原位展开、嵌套、缺失、循环。
- `bifrost-storage::build_rule_reference_catalog` 统一入口。
- 单测：`rule::references` 全场景 + `load_stored_rules_expands_disabled_rule_references`、`load_stored_rules_expands_group_rule_references`、`load_stored_rules_ignores_missing_reference_line`。

## Phase 2 — 各解析入口接入

- 主代理 / 临时端口 / Replay / CLI rule / Admin API 全部走 `build_rule_reference_catalog`。
- 宽松语义（主代理运行时）与 strict 语义（validate）区分处理。
- Reference candidates API 上线。

## Phase 3 — Rules 编辑器

- Monaco decoration + view zone 展开 + hover 错误提示 + fuzzy 补全。
- 亮暗主题适配。
- Playwright 覆盖：`web/tests/ui/admin-rules-values.spec.ts` 中 `@规则引用` 场景。

## Phase 4 — 文档与真实回归

- `human_tests/webui-rules.md` 中 `TC-WRU-45` 覆盖亮暗主题 / 展开 / 收起 / 详情 / 组规则引用 / 候选 / fuzzy / 缺失 / commented-* / tab 注释 / 注释内 `@`。
- `human_tests/readme.md` 同步。
- 规则语法文档补 `@规则名称` 运行时语义（可待后续文档整理时并入）。

## 测试方案

### 单元测试

- `bifrost-core rule::references`：
  - 引用识别（独立行、非独立行、`@comment` 精确、`commented-*`、tab 行内注释、`#` 注释内 `@`）。
  - 原位展开、嵌套引用、缺失引用宽松跳过、strict 报 `E020`、循环引用报错。
- `bifrost-cli`：
  - `load_stored_rules_expands_disabled_rule_references`
  - `load_stored_rules_expands_group_rule_references`
  - `load_stored_rules_ignores_missing_reference_line`
- `bifrost-storage`：
  - `test_build_rule_reference_catalog`（`crates/bifrost-storage/src/rules.rs:1633`）
- 前端 tokenizer：验证 `@规则名称` 被标记为 reference token；completion 与 decoration 只在独立引用行触发。

### E2E

- `e2e-tests/tests/test_rule_references.sh`：
  - 通过真实 Admin API 创建 disabled shared、`commented-*` disabled shared、预置 disabled 组 shared、enabled entry。
  - Validate API 解析为 4 条规则。
  - 真实代理请求确认 3 个 shared 规则里的请求头生效。
  - 覆盖 tab 行内注释、`#` 注释内 `@` 不参与解析、缺失引用运行时跳过后续规则仍可代理。

### Playwright

- `web/tests/ui/admin-rules-values.spec.ts` 中 `@规则引用` 用例：
  - Validate API 缺失引用错误、缺失引用 token 标红、hover 错误提示。
  - Reference candidates 自动检索。
  - Monaco fuzzy 补全。
  - 点击 `@规则` 与 `@组名/规则名` 展开详情。
  - 注释内 `@` 不被标红。

### 真实场景 human_tests

- `human_tests/webui-rules.md` 中 `TC-WRU-45`：覆盖亮暗主题 / 展开收起 / 详情内容 / 组规则引用 / 候选检索 / fuzzy 补全 / 运行时缺失引用跳过 / UI 缺失引用错误 / `commented-*` 规则名 / tab 行内注释 / 注释内 `@` 不误报。
- `human_tests/readme.md`：同步用例编号。

### 环境约束

- Rust 单测无网络需求。
- E2E 与 Playwright：临时 `BIFROST_DATA_DIR`、非 9900 端口、`--no-system-proxy`、`BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`、`BIFROST_DISABLE_TRAY=1`。

## Review/Fix/Test 闭环

### 第 1 轮

- 复核所有解析入口是否统一走 `build_rule_reference_catalog`；有无遗漏走本地 hand-rolled 识别。
- 复核 disabled 规则 standalone 是否被跳过；enabled entry 能否引用 disabled shared。
- 运行 `cargo test -p bifrost-core rule::references`、`cargo test -p bifrost-cli load_stored_rules_`、`bash e2e-tests/tests/test_rule_references.sh`、Playwright `@规则引用`。

### 第 2 轮

- 基于第 1 轮修复复查最新 diff：WebUI 亮暗主题、human_tests 索引、E020 与运行时跳过是否两处一致。
- 复跑失败用例；确认组规则 hot reload 后 catalog 能同步刷新。

## 风险与决策

- 决策：展开是纯文本原位替换而非"引入被引用规则运行时对象"。这样 rule parser 不需要理解引用，语义最小。
- 决策：宽松运行时 vs strict validate 语义分离。宽松保证在线请求不会因为规则维护 gap 全线中断；strict 让编辑器有明确红色提示。
- 决策：全限定名 `@group/name` 优先于短名，避免同名冲突。
- 风险：嵌套引用理论可能导致 catalog 展开开销大；已通过循环检测限制。
- 风险：Monaco decoration 与 rule parser 的 tokenizer 需要保持"独立行判定"的一致语义；tokenizer 单测覆盖必须存在。
- 风险：分享 URL 携带 `@` 引用时，被分享方缺失 shared 规则会导致规则退化；未来可考虑分享时选择"内嵌展开"。

## 实现现状（截至 2026-07-03）

- 后端：`bifrost-core::rule::references` 提供识别与展开能力；`bifrost-storage::build_rule_reference_catalog`（`crates/bifrost-storage/src/rules.rs:265`）被主代理 (`start.rs:3579`)、临时端口 (`port.rs:135`)、CLI (`rule.rs:176`)、Admin (`handlers/rules.rs:1021`)、Replay (`handlers/replay.rs:2537`、`replay_executor.rs:648`) 全面接入。
- 错误码：`E020` 定义于 `crates/bifrost-core/src/rule/parser/mod.rs:1188` 与 `syntax_report.rs:74`，单测于 `parser/mod.rs:4822`、`syntax_report.rs:177`、`commands/rule.rs:337`。
- Admin API：`GET /api/rules/reference-candidates`（`crates/bifrost-admin/src/handlers/rules.rs:502`）已提供。
- E2E：`e2e-tests/tests/test_rule_references.sh` 覆盖 4 条规则、3 个 shared 请求头、tab 注释、`#` 注释内 `@`、缺失引用宽松跳过。
- Playwright：`web/tests/ui/admin-rules-values.spec.ts` 中 `@规则引用` 系列已覆盖 decoration / hover / expand / fuzzy 补全。
- Human tests：`human_tests/webui-rules.md` TC-WRU-45 覆盖亮暗主题、运行时缺失跳过、UI 缺失提示、`commented-*`、tab 注释、注释内 `@`。
- 本设计文档无待落地项。
