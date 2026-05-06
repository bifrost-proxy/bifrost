# Skill Loading E2E 端到端测试

## 功能模块说明

验证 Skill 系统的加载和消费一致性。Skill 有两套入口：
- **管理端**：`SkillStore` / `SkillRegistry`（skills crate）— WebUI API 管理 Skill 的 CRUD 和启用/停用
- **消费端**：`SkillsManager`（agent crate）— Agent 运行时加载 Skill 元信息并注入到系统 prompt；完整 `SKILL.md` body 采用按需读取，不在基础 prompt 中 eager 注入

两套系统必须对 4 层 scope（Repo > User > Global > System）的路径、优先级、启用/停用状态保持一致。

## 前置条件

1. 项目已编译：`cargo build -p bifrost-e2e`
2. E2E 测试模块 `skill_loading` 已注册到 `tests/mod.rs` 和 `all_tests()`

## 测试用例列表

### TC-SL-01：E2E 测试编译通过

- **操作步骤**：执行 `cargo check -p bifrost-e2e`
- **预期结果**：编译成功，无错误

### TC-SL-02：运行全部 skill_loading E2E 测试

- **操作步骤**：执行 `cargo run --bin bifrost-e2e -- -c skill_loading -v`
- **预期结果**：16 个测试全部 PASS，输出 "All tests passed!"

### TC-SL-03：4 Scope 加载可见性（测试 1-4）

验证 Repo/User/Global/System 四种 scope 的 skill 在两套系统中均可见：
- `skill_loading_repo_scope_visible_in_both_systems`
- `skill_loading_user_scope_visible_in_both_systems`
- `skill_loading_global_scope_visible_in_both_systems`
- `skill_loading_system_scope_visible_in_both_systems`

- **预期结果**：4 个测试全部 PASS

### TC-SL-04：优先级覆盖（测试 5-6）

验证高优先级 scope 覆盖低优先级同名 skill：
- `skill_loading_repo_overrides_user_global_system`：Repo 覆盖所有
- `skill_loading_user_overrides_global_and_system`：User 覆盖 Global 和 System

- **预期结果**：2 个测试全部 PASS

### TC-SL-05：启用/停用一致性（测试 7-9）

验证管理端停用操作在消费端生效：
- `skill_loading_disable_via_store_hides_from_manager`：SkillStore 写 .disabled → SkillsManager 不加载
- `skill_loading_disable_via_store_marks_disabled_in_registry`：SkillStore 写 .disabled → SkillRegistry.enabled() 不含
- `skill_loading_disable_consistent_across_manager_and_store`：完整停用/启用循环两端一致

- **预期结果**：3 个测试全部 PASS

### TC-SL-06：Prompt 渐进式披露一致性（测试 10-11）

验证启用的 skill 元信息注入 prompt、完整 body 不 eager 注入，停用的 skill 不注入：
- `skill_loading_enabled_skill_appears_in_prompt`
- `skill_loading_disabled_skill_excluded_from_prompt`

- **操作步骤**：
  1. 执行 `cargo run --bin bifrost-e2e -- --test skill_loading_enabled_skill_appears_in_prompt`
  2. 执行 `cargo run --bin bifrost-e2e -- --test skill_loading_disabled_skill_excluded_from_prompt`
- **预期结果**：
  - 启用 skill 的 prompt 包含 skill name、description 和 `SKILL.md` path。
  - 启用 skill 的 prompt 不包含完整 body 文本 `Use this tool to do amazing things.`。
  - 停用 skill 的 name/body 均不出现在 prompt 中。
  - 2 个测试全部 PASS。
- **实际结果（2026-05-05）**：
  - `cargo run --bin bifrost-e2e -- --test skill_loading_enabled_skill_appears_in_prompt`：PASS。
  - `cargo run --bin bifrost-e2e -- --test skill_loading_disabled_skill_excluded_from_prompt`：PASS。

### TC-SL-07：Slash 命令解析（测试 12-13）

验证 slash 命令在 SkillRegistry 中的解析行为：
- `skill_loading_slash_command_resolves_via_registry`
- `skill_loading_disabled_slash_command_still_resolves`

- **预期结果**：2 个测试全部 PASS

### TC-SL-08：default_roots 路径对齐（测试 14）

验证 `default_roots()` 返回的 4 个 scope 路径与 SkillsManager 使用的路径一致：
- System: `~/.bifrost/agent/skills/.system/`
- Global: `~/.agents/skills/`
- User: `~/.bifrost/agent/skills/`
- Repo: `<work_dir>/.agents/skills/`

- **预期结果**：1 个测试 PASS

### TC-SL-09：隐藏目录过滤（测试 15）

验证以 `.` 开头的目录不被加载为 skill：
- `skill_loading_hidden_dirs_are_skipped`

- **预期结果**：1 个测试 PASS

### TC-SL-10：嵌套 skill 发现（测试 16）

验证 BFS 扫描能发现深度 > 1 的嵌套 skill：
- `skill_loading_nested_skills_discovered`

- **预期结果**：1 个测试 PASS

### TC-SL-11：单元测试回归

- **操作步骤**：执行 `cargo test -p bifrost-agent -- skills`
- **预期结果**：所有 skills 模块单元测试通过

## 清理步骤

E2E 测试使用 `tempfile::TempDir` 自动清理，无需手动清理。
