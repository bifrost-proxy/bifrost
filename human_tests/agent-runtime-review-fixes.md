# Agent Runtime Review Fixes 真实场景测试用例

## 功能模块说明

覆盖 `feat/agent` review 修复中 Agent runtime 的三类用户可感知能力：Session 自动装配 SkillRegistry、长期记忆文件并发追加安全、system prompt 中可用 skill 摘要注入。

## 前置条件

```bash
cd <REPO_ROOT>
export CARGO_TARGET_DIR=./.codex-target/fixpass
```

## 测试用例列表

### TC-ARF-01: AgentSession 自动装配 SkillRegistry

- **操作步骤**:
  ```bash
  CARGO_TARGET_DIR=./.codex-target/fixpass cargo test -p bifrost-agent --test session_skills_integration --quiet
  ```
- **预期结果**: 测试通过；`AgentSession::new_with_work_dir` 直接加载 work dir 下 `.agents/skills/weather/SKILL.md`，执行 `/skill list` 不返回“未知命令”，响应包含 `/weather`。
- **本次执行结果**: 2026-05-03 通过，结果 `1 passed, 0 failed`。

### TC-ARF-02: MEMORY.md 并发 append 文件锁回归

- **操作步骤**:
  ```bash
  CARGO_TARGET_DIR=./.codex-target/fixpass cargo test -p bifrost-agent append_line_locks_concurrent_writers --quiet
  ```
- **预期结果**: 测试通过；8 个并发 writer 各追加 1000 行后文件总行数为 8000，且每行都是合法 JSON，没有交错写入。
- **本次执行结果**: 2026-05-03 通过，结果 `1 passed, 0 failed`。

### TC-ARF-03: System prompt 注入 Skill 摘要

- **操作步骤**:
  ```bash
  CARGO_TARGET_DIR=./.codex-target/fixpass cargo test -p bifrost-agent system_prompt_includes_bounded_skill_registry_digest --quiet
  ```
- **预期结果**: 测试通过；构建的 system prompt 包含 `## Available Skills`，并列出 `alpha`、`beta`、`gamma` 三个 enabled skill。
- **本次执行结果**: 2026-05-03 通过，结果 `1 passed, 0 failed`。

## 清理步骤

测试使用 `tempfile::TempDir` 自动清理，无需手动清理。
