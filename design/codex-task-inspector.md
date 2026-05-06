# Codex Task Inspector

## 功能模块描述

`codex-task-inspector` 是仓库级只读 skill，用于在用户询问 Codex 异步任务进展、rollout 状态、会话落盘位置时，优先定位到 Codex 的真实默认数据目录，再据此读取 `sessions/`、`session_index.jsonl`、`history.jsonl` 等权威数据。

本次优化聚焦一个高频误判：skill 之前把 `~/.codex` 当作固定事实写死在说明里，缺少对 `CODEX_HOME` 的显式探测步骤，导致在用户自定义 Codex home 时容易读错目录。

## 实现逻辑

1. 在 skill 内新增 `scripts/detect_codex_data_dir.py`，作为统一探测入口。
2. 探测优先级：
   - 先看 `CODEX_HOME`
   - 未设置时回退到 `$HOME/.codex`
3. 探测结果输出结构化 JSON，包含：
   - `selected_path`
   - `selected_source`
   - `selected_exists`
   - `markers`（`config.toml`、`sessions/`、`session_index.jsonl`、`history.jsonl`、`state_*.sqlite`）
   - `candidates`
4. `SKILL.md` 中的所有“默认目录”说明统一改为“先运行探测脚本，再基于返回路径继续检查”。
5. 对 rollout/session id 的定位逻辑继续保持“优先查 Codex 数据目录下的 `sessions/`，只有用户明确点名时才查看仓库 `.codex-tasks/`”。

## 依赖项

- Python 3（运行探测脚本）
- 环境变量 `HOME`
- 可选环境变量 `CODEX_HOME`
- Codex 本地数据目录中的常规落盘结构（如 `config.toml`、`sessions/`）

## 测试方案

### 单元/脚本验证

- 运行 `python3 .agents/skills/codex-task-inspector/scripts/detect_codex_data_dir.py`
  - 验证默认场景下返回 `selected_path=$HOME/.codex`
  - 验证输出含 `selected_source`、`markers`
- 运行 `CODEX_HOME=/tmp/codex-home-test python3 .../detect_codex_data_dir.py`
  - 验证优先选择 `CODEX_HOME`
  - 验证不存在目录时仍能明确返回候选路径与 `selected_exists=false`

### E2E 测试

- 脚本级端到端验证：通过真实 shell 场景分别覆盖默认目录与 `CODEX_HOME` 覆盖目录两条路径，确保 skill 的目录选择逻辑是可执行、可观察的，而不是文档假设。

### 真实场景测试（human_tests）

- 更新 `human_tests/codex-task-inspector.md`
  - 新增默认目录探测用例
  - 新增 `CODEX_HOME` 覆盖用例
  - 新增 rollout id 场景下“先探测目录再读 sessions”用例
- 同步更新 `human_tests/readme.md` 索引说明
- 按用例逐条真实执行并记录结果

## 校验要求

- `python3 $BIFROST_DATA_DIR/agent/skills/.system/skill-creator/scripts/quick_validate.py .agents/skills/codex-task-inspector`
- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test --workspace --all-features`
- `bash scripts/ci/local-ci.sh --skip-e2e`

## 文档更新要求

- 更新 `.agents/skills/codex-task-inspector/SKILL.md`
- 更新 `human_tests/codex-task-inspector.md`
- 更新 `human_tests/readme.md`
