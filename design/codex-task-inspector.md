# Codex Task Inspector 设计方案

## 背景

Codex CLI 与 Codex Desktop 是团队常用的异步 coding agent 运行时，会把 session、rollout、history 落盘到本地的 Codex 数据目录，默认位置是 `$HOME/.codex`，但用户可以通过环境变量 `CODEX_HOME` 覆盖。

`codex-task-inspector` 是仓库级只读 skill，负责回答“Codex 当前跑到哪一步”“最近有哪些 rollout”“某个 session id 落在哪里”这类问题。之前 skill 说明里把 `~/.codex` 当成不可变事实写死，缺少对 `CODEX_HOME` 的显式探测：

- 用户设置了 `CODEX_HOME=/data/codex-alt` 时，skill 仍在 `~/.codex` 下找 `sessions/`，直接给出“没有会话”的错误结论。
- rollout id 对应的会话可能落在自定义 `CODEX_HOME` 里，skill 却在错误的目录反复搜。
- Desktop 与 CLI 共用同一份 data dir，但排查时如果目录本身选错，两条路径都会给出误导性结果。

本方案把“Codex 数据目录选择”从 skill 文档假设升级为一次真实探测：新增独立探测脚本 `scripts/detect_codex_data_dir.py`，作为 skill 内所有目录相关操作的唯一入口，并配套 e2e 与 human_tests 用例，保证目录选择逻辑是可执行、可观察的。

## 用户目标验证清单

### 必须实现

- 在 skill 目录下新增 `scripts/detect_codex_data_dir.py`，返回结构化 JSON。
- 探测优先级：`CODEX_HOME`（若设置，无论是否存在）> `$HOME/.codex`。
- 输出字段：`selected_path`、`selected_source`、`selected_exists`、`markers`（`config.toml` / `sessions_dir` / `session_index` / `history` / `state_db`）、`candidates`。
- `SKILL.md` 中所有“Codex 默认数据目录”“会话目录”说明改为“先运行探测脚本，再基于返回路径继续检查”。
- rollout / session id 定位逻辑：优先在 Codex data dir 的 `sessions/` 中查，只有用户显式指名时才回落到仓库 `.codex-tasks/`。
- 覆盖 Codex CLI（命令行 `codex exec`）与 Codex Desktop 两种运行形态。
- 提供 e2e 脚本 `e2e-tests/tests/test_codex_task_inspector_data_dir.sh`，同时校验默认目录与 `CODEX_HOME` 覆盖两条路径。
- 提供 `human_tests/codex-task-inspector.md` 与 `human_tests/readme.md` 索引，方便人工回归。

### 必须不破坏

- skill 仍是 **只读**，不写入 Codex data dir，不改动用户 session / rollout。
- 探测脚本无第三方依赖，只用 Python 3 标准库；skill 加载时无额外安装步骤。
- 不修改 Codex CLI / Desktop 的行为，不假设 Codex 版本。
- 与 `bifrost` 主链路解耦，探测脚本不依赖任何 Bifrost crate。
- 如果 `HOME` 未设置或 `CODEX_HOME` 非法，脚本明确报错，不静默使用错误路径。

### 必须真实验证

- 未设置 `CODEX_HOME` 时脚本返回 `selected_source=default:$HOME/.codex`。
- 设置 `CODEX_HOME=/tmp/custom-codex-home`（目录可不存在）时脚本返回 `selected_source=env:CODEX_HOME`、`selected_exists=false`。
- e2e 脚本可以在 CI 与本地环境无副作用地跑通。
- skill 在真实调用场景下，切换 `CODEX_HOME` 后能报出不同的会话统计。

## 产品语义

### 探测脚本是 skill 内所有目录操作的唯一入口

skill 内的 `SKILL.md` 与其他辅助脚本一律不再硬编码 `~/.codex`。任何需要定位 Codex 落盘目录的地方都先跑 `python3 scripts/detect_codex_data_dir.py`，拿到 JSON 后再决定下一步。这样：

- 用户自定义 `CODEX_HOME` 时 skill 能立即适配。
- Codex 升级后如果引入新目录结构（例如更改 sessions 子目录名），markers 一目了然，减少 skill 反复更新的成本。
- 报错时能带上完整 candidates，便于快速定位是环境变量错、目录不存在，还是 marker 缺失。

### 输出契约

```json
{
  "selected_path": "/Users/eden/.codex",
  "selected_source": "default:$HOME/.codex",
  "selected_exists": true,
  "markers": {
    "config_toml": true,
    "sessions_dir": true,
    "session_index": true,
    "history": true,
    "state_db": false
  },
  "candidates": [
    { "source": "default:$HOME/.codex", "path": "/Users/eden/.codex", "exists": true }
  ]
}
```

- `selected_source` 枚举：`env:CODEX_HOME`、`default:$HOME/.codex`。
- `markers.state_db` 匹配 `state_*.sqlite`；Codex 版本切换会影响这几个 marker 是否齐全，skill 消费方可以据此判断是不是识别到了真实 Codex data dir。
- `candidates` 保持完整（当前一定包含 default，且当 `CODEX_HOME` 设置时排在前面），方便调试。

### rollout / session id 的定位顺序

1. 拿到 `selected_path`。
2. 在 `{selected_path}/sessions/` 下搜索匹配 rollout id / session id 的目录或文件。
3. 只有当用户明确点名仓库 `.codex-tasks/`（例如提到某个 task 名字或点名 desktop app 的项目路径）时，skill 才回落到仓库路径查询。
4. 找到之后，只做只读展示：目录路径、事件数量、最近一次更新时间、终止原因。skill 不修改任何文件。

### Desktop 与 CLI 的差异

- Codex CLI：`codex exec`、`codex chat` 直接在同一个 `$CODEX_HOME` / `~/.codex` 下写 sessions。
- Codex Desktop：Desktop app 使用同一份 Codex data dir（除非用户显式配置），因此 `detect_codex_data_dir.py` 的输出对 Desktop 同样有效。
- skill 消费方应在报告里明确“数据来自 `<selected_path>` 下的 sessions”，避免用户误以为 skill 检查了别的目录。

## 技术细节

### 探测脚本关键逻辑

```python
def resolve_home() -> Path:
    home = os.environ.get("HOME")
    if not home:
        raise RuntimeError("HOME is not set; cannot resolve Codex default data dir")
    return Path(os.path.abspath(os.path.expanduser(home)))

def candidate_from_env(home: Path):
    raw = os.environ.get("CODEX_HOME")
    if not raw:
        return None
    path = Path(raw).expanduser()
    if not path.is_absolute():
        path = home / path
    return { "source": "env:CODEX_HOME", "path": normalize(path), "exists": path.exists() }

def choose(candidates):
    if candidates and candidates[0]["source"] == "env:CODEX_HOME":
        return candidates[0]
    for c in candidates:
        if c["exists"]:
            return c
    return candidates[0]
```

关键约束：

- `CODEX_HOME` 一旦设置就直接选中，**即使目录不存在**也返回给用户。这样能第一时间暴露“环境变量配错”这类问题，而不是静默 fallback 到 `~/.codex` 得到错误结论。
- 相对路径 `CODEX_HOME=./custom` 会展开到 `$HOME` 下。
- `HOME` 缺失时脚本立即抛错，不假设“空字符串等价于当前目录”。

### markers 判定

只做文件系统 stat，不真的解析：

- `config.toml`：Codex 主配置。
- `sessions/`：目录形式，存放 rollout。
- `session_index.jsonl`：会话索引。
- `history.jsonl`：全局历史。
- `state_*.sqlite`：Codex 内部状态 DB，采用 glob 匹配以适配版本差异。

markers 缺失不代表探测失败，只是提示 “这个目录看上去不像 Codex data dir”。skill 消费方在报告里带上 markers，可以让用户判断是不是选到了错误目录。

### 与 skill 文档的集成

- `SKILL.md` 的 “Environment discovery” 段落：改成 “Always run `scripts/detect_codex_data_dir.py` first; use `selected_path` as the base for every subsequent read.”。
- “Rollout / session id lookup” 段落：先给出探测命令，再走 `{selected_path}/sessions/` 查找路径。
- “Repo `.codex-tasks/` fallback” 单独一节，强调只有用户点名时才走这条路径。

### 探测脚本的调用规范

- skill 内所有 shell / python 步骤都通过 `python3 "$SKILL_DIR/scripts/detect_codex_data_dir.py"` 调用，不要在 `SKILL.md` 里 inline 复制探测逻辑（避免漂移）。
- 消费 JSON 时用 `python3 -c "import json,sys; ..."` 或 `jq` 等；skill 不要自己造 KV 解析。
- 出错时（例如 `HOME` 未设置）保留原始异常输出，让用户能直接定位配置问题。

## CLI / Web / API 触点

`codex-task-inspector` 不新增任何 Bifrost CLI 命令或 API 端点，只在 skill 内部提供脚本。原因：

- 探测 Codex 数据目录不属于 Bifrost 主链路能力。
- skill 已经是 Bifrost 外部 Runner 生态的一等公民，通过 skill 调用即可复用。

## 数据 / Sync 边界

- 只读，不写入 Codex data dir。
- 不上报任何 session / rollout 内容到远端。
- 不参与 Bifrost 的 sync / group sync。
- 探测脚本不落盘 cache；每次都实时 stat，避免 cache 与真实环境漂移。

## 实现切分

### Phase 1：探测脚本

- 新增 `scripts/detect_codex_data_dir.py`。
- 覆盖 `CODEX_HOME` / `$HOME/.codex` 两条路径。
- markers 与 candidates 输出。
- 无外部依赖，Python 3.9+ 可跑。

### Phase 2：SKILL.md 集成

- 把所有硬编码 `~/.codex` 替换为“先跑探测脚本”。
- 更新“Rollout / session id 定位”“Desktop vs CLI”“报告样例”段落。
- 增加“Repo `.codex-tasks/` 只在显式点名时使用”声明。

### Phase 3：E2E 与人工回归

- `e2e-tests/tests/test_codex_task_inspector_data_dir.sh`：覆盖默认与 `CODEX_HOME` override（目录不存在也要通过）。
- `human_tests/codex-task-inspector.md`：新增三条真实场景 case。
- `human_tests/readme.md`：把新增 case 挂到索引里。

### Phase 4：Skill 质量校验

- `python3 $BIFROST_DATA_DIR/agent/skills/.system/skill-creator/scripts/quick_validate.py .agents/skills/codex-task-inspector`。
- 手动跑一次真实场景，确认 skill 报告里带上探测输出的 `selected_path` 与 markers。

## 测试方案

### 单元 / 脚本验证

- `python3 .agents/skills/codex-task-inspector/scripts/detect_codex_data_dir.py`
  - 默认场景 `selected_path=$HOME/.codex`、`selected_source=default:$HOME/.codex`。
  - `markers` 字段存在且为 bool。
- `CODEX_HOME=/tmp/codex-home-test python3 .../detect_codex_data_dir.py`
  - `selected_source=env:CODEX_HOME`。
  - 目录不存在时 `selected_exists=false`，但 `selected_path` 仍指向目标。
- `CODEX_HOME=./relative python3 .../detect_codex_data_dir.py`
  - 相对路径展开到 `$HOME/relative`。

### E2E

`e2e-tests/tests/test_codex_task_inspector_data_dir.sh`：

- `test_default_dir_detection`：清空 `CODEX_HOME`，验证 `selected_source` 与 `selected_path`。
- `test_codex_home_override_even_when_missing`：`CODEX_HOME` 指向 `mktemp -d` 下一个未创建的子目录，验证 override 生效、`selected_exists=false`。
- 通过 `assert_json_field` 断言字段值，脚本失败时明确 exit 非 0。

### 真实场景测试（human_tests/codex-task-inspector.md）

- TC-CTI-01：默认目录探测——skill 报告里必须包含 `selected_path=$HOME/.codex` 与 markers。
- TC-CTI-02：`CODEX_HOME` 覆盖——切换环境变量后 skill 报告里的 `selected_path` 变化。
- TC-CTI-03：rollout id 场景——用一个存在的 rollout id 让 skill 先探测、再定位 session。
- TC-CTI-04：Desktop 场景——Desktop 运行时验证 skill 报告与 CLI 场景使用同一目录。
- 每条 case 都记录：命令、输出片段、结论。

### 校验清单

- `python3 .../quick_validate.py .agents/skills/codex-task-inspector`
- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test --workspace --all-features`
- `bash scripts/ci/local-ci.sh --skip-e2e`
- 单独跑一次 `bash e2e-tests/tests/test_codex_task_inspector_data_dir.sh` 确认 skill e2e 通过。

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核探测脚本：`HOME` 缺失、`CODEX_HOME` 相对路径、`CODEX_HOME` 指向不存在目录三种边界。
- 复核 `SKILL.md`：所有原来写 `~/.codex` 的地方是否都改为“先跑脚本”。
- 复核 e2e 脚本能否在无 Codex 安装的环境跑通（不能强依赖真实 sessions 文件）。
- 跑 quick_validate 与 skill 单测。

### 第 2 轮

- 复核第 1 轮问题的修复。
- 抽 1 条 human_tests case 真实执行，确认脚本输出与报告一致。
- 再次 `git status --short`、`git diff` 确认没有漏改 skill 文档。

## 风险与决策点

- **`CODEX_HOME` 目录不存在时是否 fallback**：当前策略是**不 fallback**，直接返回 override 路径并把 `selected_exists=false`。理由：静默 fallback 会掩盖“环境变量配错”这个真实问题。若用户希望 fallback，应主动 unset。
- **markers 覆盖范围**：Codex 未来可能增删文件；markers 只做“提示”，不做“判定”。skill 消费方需要接受 markers 部分缺失的情况。
- **多用户 / sudo 场景**：探测脚本从 `HOME` 读，不做 `pwuid` fallback；如果 skill 在 sudo 环境跑，`HOME` 可能指向 root，需要用户显式设置 `HOME` 或 `CODEX_HOME`。
- **仓库 `.codex-tasks/` 语义**：只有显式点名才使用；如果未来 Bifrost 引入自动 sync 到仓库路径的机制，需要重新评估默认查找顺序。
- **Desktop 版本差异**：Codex Desktop 若在某些平台使用不同 data dir（例如 macOS 沙盒版本），需要为 skill 增加平台特定 candidate；本方案预留 candidates 列表接口，便于未来扩展。
