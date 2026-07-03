# CLI `rule list` 仅扫描 `.bifrost` 规则文件

## 背景

Bifrost 的本地规则存储 (`crates/bifrost-storage/src/rules.rs`) 使用 `<data_dir>/rules/` 目录承载规则文件，同时用子目录承载 Group（远端拉下来的规则集合）。历史上 `RulesStorage::list()` 的扫描候选集合是「目录中所有普通文件」，而不是「扩展名为 `.bifrost` 的普通文件」。

这带来了两个具体故障：

1. **一坏全坏**：本地 `rules/` 目录里只要有一个 legacy 规则文件缺字段或 JSON 格式损坏，`bifrost rule list` 就直接 `Err` 退出，其余可正常解析的本地规则完全无法展示。
2. **误报**：Group 同步机制会在 `rules/` 目录写一个 `.group_cache`（元数据缓存）。这个文件不是规则但落在扫描范围里，被喂给 legacy 解析器时产生 `missing field 'name'` 之类的误导性错误，用户以为规则损坏。

Group 规则的子目录读取路径 (`load_enabled_with_subdirs` / `load_all_with_subdirs`) 依赖 `list()` 的候选集，因此本次改动必须保证：**只把扩展名为 `.bifrost` 的普通文件当规则候选**，同时**必须跳过目录不删**（子目录承载 Group 规则，需要在 subdirs 路径里继续可见）。

## 用户目标验证清单

### 必须实现

- `RulesStorage::list()` 仅返回扩展名为 `.bifrost` 的**普通文件**（非目录、非符号链接指向目录）的规则名。
- `.group_cache`、`.json`、`.bak`、无扩展名文件、隐藏文件等一律不再被当成规则候选。
- 目录（含 Group 子目录）在 `list()` 中被显式跳过，不进入规则候选集。
- `load_all()`、`load_all_with_subdirs()`、`load_all_with_subdirs_filtered()`、`list_summaries()` 全部复用 `list()` 的候选集，因此都不会再尝试解析 `.group_cache` / `.json` 等非规则文件。
- `load_enabled_with_subdirs()` 继续能读取 Group 子目录中的 `.bifrost` 文件，Group 语义不受破坏。
- 单个损坏的 `.bifrost` 规则文件不会导致 `bifrost rule list` 整体失败：损坏项被跳过或标记为 error entry，其它规则继续展示（既有 `load_all()` 已支持逐文件错误容忍时按此语义；如果尚未支持，本次至少保证损坏的非 `.bifrost` 文件被过滤，避免把 `.group_cache` 拖进来）。

### 必须不破坏

- Group 规则读取范围不变：子目录本身在 `list()` 里被跳过但在 `load_all_with_subdirs*` 路径里继续被枚举。
- 显式 legacy 规则加载能力（`load_legacy_rule_path` 或类似入口，如果存在）不变；本次只调整目录扫描候选集。
- `.bifrost` 文件名大小写敏感性保持与文件系统一致（macOS/Windows 默认大小写不敏感，Linux 默认敏感），不引入新语义。
- 已启用规则、rule_id 映射、sync 元数据不受影响。
- 规则名编码 (`encode_rule_name` / `decode_rule_name`) 语义不变。
- 现有 `.bifrost` 文件解析路径不变，legacy JSON 迁移逻辑（`legacyrule.json → legacyrule.bifrost`）保留。

### 必须真实验证

- 真实执行 CLI 场景：临时数据目录中放 1 个合法 `.bifrost` 规则、1 个 `.group_cache` 普通文件、1 个 group 子目录（内含 `.bifrost` 规则），运行 `bifrost rule list` 只展示本地 `.bifrost` 规则，不报 `missing field 'name'` 类错误。
- 真实执行 `bifrost rule enable/disable/get`，确认候选集变化不影响已启用规则加载。
- 真实运行单测：`test_list_ignores_non_bifrost_files`、`test_load_all_ignores_non_bifrost_files`、`test_list_summaries_ignores_non_bifrost_files`、`test_load_enabled_with_subdirs_keeps_group_directories`。

## 产品语义

### 规则文件的唯一合法扩展名是 `.bifrost`

从本方案落地起，Bifrost 本地规则存储层认可的规则文件形态只有一种：**`<data_dir>/rules/<encoded_name>.bifrost` 普通文件**。任何其它文件（`.json`、`.bak`、`.group_cache`、无扩展名、以点开头的隐藏文件）都不会被 storage 层枚举为规则候选。

这是**扫描层的过滤规则**，不是**语义层的强制转换**：如果用户手工把 `foo.json` 放进 `rules/` 目录，Bifrost 只是「看不见」它，不会主动删除、不会尝试迁移、不会报错。

### `.group_cache` 是 Group 同步的实现细节，不是规则

`.group_cache` 是 Group 拉取过程中 storage 层写入的元数据缓存文件（缓存 remote group 的 rule 列表、版本号等）。它一直不应该被规则扫描器看到；本次修复即是把它从规则候选集里彻底剥离。

### Group 子目录 = 规则容器，而不是规则本身

`rules/` 目录下的子目录（例如 `rules/team-shared/`）用于承载 Group：每个 Group 是一个子目录，子目录里放 `.bifrost` 规则文件。因此：

- `list()`（顶层目录）跳过所有目录 —— 目录本身不是规则文件。
- `load_all_with_subdirs()` 显式遍历子目录 —— 子目录里的 `.bifrost` 文件仍然被加载。
- `load_enabled_with_subdirs()` 在 subdirs 层过滤 enabled 状态。

两条路径职责清晰，`.bifrost` 过滤规则在两处保持一致。

### 损坏文件不影响正常规则展示

- 非 `.bifrost` 文件：直接不进候选集，天然不影响。
- 损坏的 `.bifrost` 文件（缺字段、TOML 语法错误）：走既有 `load_all()` 的逐文件错误处理路径（如果已实现），只把该文件标记为 error 而不打断整个 list。若当前 `load_all()` 尚未支持这一容错，本次至少保证「非规则文件不再引入错误」，从而消除本次修复目标场景。

## 技术细节

### 修改点

`crates/bifrost-storage/src/rules.rs::RulesStorage::list()`：

```rust
pub fn list(&self) -> Result<Vec<String>> {
    let mut names = Vec::new();
    if !self.base_dir.exists() {
        return Ok(names);
    }
    for entry in std::fs::read_dir(&self.base_dir)? {
        let entry = entry?;
        let ft = entry.file_type()?;
        if ft.is_dir() {
            continue; // Group 子目录不在此处枚举
        }
        if !ft.is_file() {
            continue; // 跳过符号链接等
        }
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("bifrost") {
            continue; // 只识别 .bifrost 文件
        }
        if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
            if let Some(name) = Self::decode_rule_name(stem) {
                names.push(name);
            }
        }
    }
    Ok(names)
}
```

### 复用链

- `load_all()`（`rules.rs:713`）→ 遍历 `list()` 返回的名字 → 分别 `load(name)`。
- `load_enabled()` → `load_all().into_iter().filter(|r| r.enabled)`。
- `load_all_with_subdirs()`（`rules.rs:733`）→ 先 `load_all()` 拿顶层规则，再显式 `read_dir` 遍历子目录并 `sub_storage.load_all()`。
- `load_enabled_with_subdirs()`（`rules.rs:798`）→ filter enabled。
- `list_summaries()`（`rules.rs:842`）→ 走 `list()` + 元数据读取。

因此本次只改 `list()` 一处，即可让所有列表/加载入口都跳过非 `.bifrost` 文件。子目录路径不受影响：`load_all_with_subdirs*` 直接 `read_dir(base_dir)` 找子目录，与 `list()` 的过滤逻辑正交。

### 相关文件

- `crates/bifrost-storage/src/rules.rs`：`list()` 修改；新增回归测试。
- `crates/bifrost-storage/src/data_dir.rs`：无改动（仅确认 `<data_dir>/rules/` 路径未变）。
- `crates/bifrost-cli/src/commands/rule.rs`：无改动（CLI 层继续调用 `list_summaries` 等）。
- `human_tests/cli-rule-list-legacy-skip.md`：新增/更新。
- `human_tests/readme.md`：索引同步。

## CLI + Web + Admin API

### CLI

- `bifrost rule list`：行为变化 —— 之前会因 `.group_cache` 报错，现在正常输出本地 `.bifrost` 规则。
- `bifrost rule enable/disable/get/delete <name>`：不变（不依赖非 `.bifrost` 文件）。
- 无新增 CLI 命令，无新增参数。

### Admin API

- `GET /api/rules`：底层调用 `list_summaries()`，本次修复后不再因 `.group_cache` 返回 500。
- `GET /api/rules/:name`：无行为变化。
- 无 API 变化。

### Web UI

- Rules 页面：之前在含 `.group_cache` 的目录里 list 报错空白，现在正常展示；无 UI 组件改动。

## Sync 边界

- Sync 层不受影响：`.group_cache` 由 Group 同步逻辑本身写入和读取，不通过 `RulesStorage::list()`。
- Group 子目录中的 `.bifrost` 文件通过 `load_enabled_with_subdirs` 参与运行时；sync 语义不变。
- 本次改动不引入新 sync 字段、不新增 remote 元数据。

## 实现切分

### Phase 1：过滤逻辑

- 修改 `list()`：只收集 `.bifrost` 普通文件；显式跳过目录、符号链接、其它扩展名。
- 单元测试：
  - `test_list_ignores_non_bifrost_files`
  - `test_load_all_ignores_non_bifrost_files`
  - `test_list_summaries_ignores_non_bifrost_files`
  - `test_load_enabled_with_subdirs_keeps_group_directories`

### Phase 2：CLI E2E

- 使用临时数据目录构造 1 合法 `.bifrost` + 1 `.group_cache` + 1 group 子目录（含 `.bifrost`）。
- `bifrost rule list` 只展示本地 `.bifrost` 规则，不打印 `missing field 'name'`。
- group 子目录场景由 storage 单测 `test_load_enabled_with_subdirs_keeps_group_directories` 兜底，避免依赖远端 group API 的场景被误当成本地可验证行为。

### Phase 3：human_tests + 文档

- 更新 `human_tests/cli-rule-list-legacy-skip.md` 加入两条用例：
  - 「非 `.bifrost` 文件自动忽略」
  - 「group 子目录规则不受影响」
- 在临时数据目录下逐条执行。
- `human_tests/readme.md` 索引同步。

### Phase 4：回归验证

- `cargo test -p bifrost-storage`
- `cargo test --workspace --all-features`
- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- rust-project-validate

## 测试方案

### 单元测试

（位于 `crates/bifrost-storage/src/rules.rs` 测试模块）

- `test_list_ignores_non_bifrost_files`：目录中放一个 `.bifrost`、一个 `.group_cache`、一个 `.json`；`list()` 只返回 `.bifrost` 规则名。
- `test_load_all_ignores_non_bifrost_files`：同上；`load_all()` 只解析 `.bifrost`，不再抛出 legacy JSON 解析错误。
- `test_list_summaries_ignores_non_bifrost_files`：同上；`list_summaries()` 只返回 `.bifrost` 规则的 summary。
- `test_load_enabled_with_subdirs_keeps_group_directories`：构造顶层 `.bifrost` + 子目录（内含 `.bifrost`）；`load_enabled_with_subdirs()` 同时返回顶层规则与子目录规则。
- `test_list_skips_directories`：只放一个子目录，`list()` 返回空 Vec，不 panic、不把目录当规则。
- `test_list_ignores_symlinks_to_directories`：可选，若跨平台稳定。

### E2E 测试

在临时 `BIFROST_DATA_DIR` / 非 9900 端口 / `--no-system-proxy` 环境下：

1. 手动准备 `<data_dir>/rules/rule-a.bifrost`（合法内容）、`<data_dir>/rules/.group_cache`（任意 JSON）、`<data_dir>/rules/team/rule-b.bifrost`（合法）。
2. 运行 `bifrost rule list`：断言输出包含 `rule-a`（顶层）；不出现 `missing field 'name'`；不因 `.group_cache` 失败。
3. Group 子目录场景由存储层单测 `test_load_enabled_with_subdirs_keeps_group_directories` 覆盖；CLI 场景不强制依赖远端 group API 拉取。

### 真实场景测试（human_tests）

更新 `human_tests/cli-rule-list-legacy-skip.md`：

- TC-CRL-01：临时数据目录 + `.group_cache`：`bifrost rule list` 无 `.group_cache` 相关错误，本地 `.bifrost` 规则正常展示。
- TC-CRL-02：临时数据目录 + `.json`（legacy 残留）：`bifrost rule list` 忽略 `.json`，不当规则；显式 legacy 迁移路径（如启用）仍可用。
- TC-CRL-03：临时数据目录 + Group 子目录（含 `.bifrost`）：主端口加载子目录中的 `.bifrost` 规则；`bifrost rule list` 顶层输出不包含子目录名。
- TC-CRL-04：损坏的 `.bifrost` 文件不影响其它规则展示（若 `load_all()` 已支持容错）。

`human_tests/readme.md` 同步索引；用例数量从既有基础上按顺序编号。

### 覆盖率与项目校验

- `cargo test -p bifrost-storage`
- `cargo test --workspace --all-features`
- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `bash scripts/ci/local-ci.sh --skip-e2e`
- rust-project-validate
- no-local-coverage 约定下不跑 `make coverage`；说明豁免依据。

## Review/Fix/Test 闭环

### 第 1 轮

- 复核用户目标：`.bifrost` 独占识别、`.group_cache` 与其它非规则文件被跳过、Group 子目录仍可读、损坏文件不影响其它规则展示。
- 复核 diff：`rules.rs::list()` 改动最小、其它入口无副作用；单测 4 条覆盖全部关键分支。
- 重点 review：是否有其它入口绕过 `list()` 直接读目录（若有，需要同步过滤）；子目录路径 `load_all_with_subdirs*` 是否被误改。
- 复测：`cargo test -p bifrost-storage`、`cargo test --workspace --all-features`。

### 第 2 轮

- 复查第 1 轮修复后的 diff、E2E 断言、human_tests 索引和验证命令。
- 手动在真机上放 `.group_cache` + `.bifrost` + Group 子目录跑一遍 CLI；截图 human_tests 需要的输出。
- rust-project-validate 通过或记录豁免。

## 风险与决策

- 用户误依赖 `.json` 规则文件：如果历史用户手工放 `.json` 到 `rules/`，本次改动后这些文件「消失」在规则列表中。风险低（`.json` 早已不是官方支持格式，legacy 迁移在其它路径完成）；文档在 `docs/rule.md` / `docs/operation.md` 明确唯一支持扩展名。
- 大小写敏感差异：macOS/Windows 默认大小写不敏感，`rule.BIFROST` 可能被文件系统当作 `.bifrost`。本方案严格 `to_str() == Some("bifrost")`（小写比较），不对文件系统大小写做适配。若用户遇到大小写混合的历史文件，可通过重命名解决；本次不做自动修复。
- 隐藏文件：`.group_cache` 是点开头的文件，历史 `list()` 也没有跳过隐藏文件（`.bifrost` 结尾的隐藏文件被视为规则）。本方案不改隐藏文件语义，只按扩展名过滤；如果未来引入 `.foo.bifrost` 备份文件也算规则会有歧义，则单独设计。
- 显式 legacy 加载路径：如果存在 `load_legacy_rule_path` 之类的显式入口，本次改动**不影响**它 —— 用户显式指定 `.json` 路径仍可加载。扫描层与显式入口分离。
- 符号链接：`is_file()` 对符号链接指向的普通文件返回 true；对指向目录的链接返回 false（因为 `is_dir()` 也 false，`is_file()` 也 false）。这是期望行为；不引入 follow_symlinks 复杂度。
- 子目录路径命名冲突：如果用户创建了一个名为 `foo.bifrost` 的**目录**（罕见），旧行为可能把它误当文件；新代码显式 `is_dir()` 跳过 + `is_file()` 校验，双重保护。
