# 规则 @ 引用与编辑器原位展开

## 功能描述

规则文件支持在独立行写 `@规则名称` 或 `@组名称/规则名称`，表示把被引用规则文件的内容在当前位置原位展开后再解析。典型用法是把通用请求头、脚本、过滤器等规则保存为禁用规则，再由一个启用的入口规则引用它，避免 shared 规则单独全局生效。

## 实现逻辑

- 引用语法只识别独立行：去除首尾空白后以 `@` 开头，且不是 `@comment`。
- 运行时以全部规则文件建立引用目录，但只把 enabled 规则作为解析入口；disabled 规则可以被引用，不会 standalone 生效。
- 私有规则使用短名引用：`@规则名称`。组规则使用 qualified name 引用：`@组名称/规则名称`，避免组规则与个人私有规则同名时产生歧义。
- `RulesStorage::load_all_with_subdirs*` 读取组规则子目录时会保留来源组名，统一通过 `build_rule_reference_catalog` 生成引用目录。
- 展开支持嵌套引用；运行时遇到缺失引用会跳过该引用行并继续解析后续规则，循环引用仍让入口规则跳过解析并记录明确错误。
- 主代理规则热加载、临时端口绑定和 Replay 规则解析复用宽松运行时语义；Rules validate API 使用 strict 展开语义，对缺失引用或循环引用返回 `E020`。
- Rules 编辑器扫描 `@规则名称` 与 `@组名称/规则名称`，用 Monaco decoration 标记可点击区域；点击后通过 Monaco view zone 在当前行下方展开被引用规则的只读内容，再次点击收起。
- Rules 编辑器把 validate API 返回的缺失引用 `E020` 映射回对应 `@` 引用 token，使用错误色标红，并在 Monaco hover 中展示错误详情。
- WebView 进入规则编辑器后自动请求 `/api/rules/reference-candidates`，把私有规则短名和已缓存组规则 qualified name 注入 Monaco 补全；补全支持前缀、包含和非连续字符 fuzzy 搜索。

## 依赖项

- `bifrost-core::rule::references` 提供纯文本引用展开能力。
- `RulesStorage::load_all_with_subdirs*` 提供引用目录所需的全部规则。
- WebUI 复用现有 Monaco 编辑器、Rules API、reference candidates API 和 group rule API。

## 测试方案

- 单元测试：
  - `rule::references` 覆盖引用识别、原位展开、嵌套引用、缺失引用运行时跳过、strict 缺失引用报错、循环引用。
  - `load_stored_rules_expands_disabled_rule_references` 验证 disabled shared 规则可被 enabled entry 引用。
  - `load_stored_rules_expands_group_rule_references` 验证私有 entry 可通过 `@组名/规则名` 引用 disabled 组规则。
  - `load_stored_rules_ignores_missing_reference_line` 验证缺失引用行被跳过，后续运行时规则仍生效。
  - tokenizer 测试验证 `@规则名称` 被标记为 reference token。
- E2E 测试：
  - `e2e-tests/tests/test_rule_references.sh` 通过真实 Admin API 创建 disabled shared、预置 disabled 组 shared 和 enabled entry，验证 validate API 解析为 3 条规则，并通过真实代理请求确认两个 shared 规则里的请求头生效；同时创建包含缺失引用和后续 host 规则的 enabled 规则，验证运行时跳过缺失引用行且后续规则仍可代理。
  - `web/tests/ui/admin-rules-values.spec.ts` 验证 validate API 缺失引用错误、缺失引用 token 标红和 hover 错误提示、reference candidates 自动检索、Monaco 模糊补全和 Rules 编辑器点击 `@规则` / `@组名/规则名` 展开详情。
- 真实场景测试：
  - 更新 `human_tests/webui-rules.md`，新增 `TC-WRU-45`，覆盖亮色和暗色主题下的 `@` 展开、收起、详情内容、组规则引用、候选检索、模糊补全、运行时缺失引用跳过和 UI 缺失引用错误提示。

## Review/Fix/Test 闭环方案

- 第 1 轮：复核用户目标、后端展开语义、UI 点击展开、禁用规则引用和缺失/循环引用诊断；运行 core/cli 相关单元测试、前端 tokenizer 测试、规则引用 E2E 和 human_tests。
- 第 2 轮：基于第 1 轮修复后的最新 diff，再复查所有解析入口是否一致、WebUI 亮暗主题和 human_tests 索引是否同步；复跑受影响测试。

## 校验要求

- `cargo test -p bifrost-core rule::references`
- `cargo test -p bifrost-cli load_stored_rules_`
- `npm --prefix web run test:unit -- tokenizer`
- `bash e2e-tests/tests/test_rule_references.sh`
- `npx --prefix web playwright test web/tests/ui/admin-rules-values.spec.ts -g "@规则引用"`
- `cargo test --workspace --all-features`
- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`

## 文档更新要求

- 更新 `human_tests/webui-rules.md` 和 `human_tests/readme.md`。
- 若规则文档存在 `@` 引用语法章节，补充 `@规则名称` 的运行时语义；否则以后规则语法文档重整时补入。
