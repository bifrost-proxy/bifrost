# CLI `rule list` 仅扫描 `.bifrost` 规则文件

## 功能模块详细描述

修复规则解析模块在扫描本地规则目录时误把非规则文件当成规则文件加载的问题。规则文件现在要求必须以 `.bifrost` 结尾，像 `.group_cache` 这样的普通文件需要被自动忽略，同时不能影响 group 子目录中的规则文件读取。

### 问题表现
1. 本地 `rules/` 目录里只要有一个 legacy 规则文件缺字段或 JSON 格式损坏，`bifrost rule list` 就直接报错退出。
2. 其余可正常解析的本地规则无法展示，用户无法继续查看已有规则状态。
3. `.group_cache` 这类非规则文件也会被喂给 legacy 解析器，产生 `missing field 'name'` 之类的误导性报错。

### 范围边界
- 本次修复只调整规则目录扫描候选集，统一只识别 `.bifrost` 文件。
- 不修改 group 规则读取范围；group 仍然使用子目录存放规则文件，子目录本身不能被过滤掉。
- 不在本次改动中移除显式 legacy 加载能力，只是目录扫描阶段不再把 `.json` 和其他普通文件当成规则候选项。

## 实现逻辑

### 存储层回归 (`crates/bifrost-storage/src/rules.rs`)
- `RulesStorage::list()` 仅收集扩展名为 `.bifrost` 的普通文件
- 扫描时显式跳过目录，避免把 group 子目录误当成规则文件项处理
- `load_all()` 与 `list_summaries()` 继续复用 `list()` 的候选集合，因此不会再尝试解析 `.group_cache`、`.json` 等非规则文件
- 增加 group 子目录回归测试，确认 `.bifrost` 文件过滤不会影响 `load_enabled_with_subdirs()` 合并 group 规则

## 依赖项
- 无新增依赖

## 测试方案

### 单元测试
- `test_list_ignores_non_bifrost_files`
- `test_load_all_ignores_non_bifrost_files`
- `test_list_summaries_ignores_non_bifrost_files`
- `test_load_enabled_with_subdirs_keeps_group_directories`

### E2E 测试
- 使用 CLI 场景回归，在独立数据目录下创建 1 个合法 `.bifrost` 规则文件、1 个 `.group_cache` 普通文件和 1 个 group 子目录规则文件
- 执行 `bifrost rule list`，断言只展示本地 `.bifrost` 规则，不尝试把 `.group_cache` 或 group 目录当成规则加载
- group 子目录保留能力由存储层单元测试 `test_load_enabled_with_subdirs_keeps_group_directories` 覆盖，避免把依赖远端 group API 的 CLI 场景误当成本地可验证行为

### 真实场景测试
- 更新 `human_tests/cli-rule-list-legacy-skip.md`
- 新增“非 `.bifrost` 文件自动忽略”和“group 子目录规则不受影响”用例，并在临时数据目录下逐条执行

## 校验要求
- `cargo test -p bifrost-storage`
- `cargo test --workspace --all-features`
- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `bash scripts/ci/local-ci.sh --skip-e2e`
- `rust-project-validate`

## 文档更新要求
- 更新 `human_tests/cli-rule-list-legacy-skip.md`
- 更新 `human_tests/readme.md`
