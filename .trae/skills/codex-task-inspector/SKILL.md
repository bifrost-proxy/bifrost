---
name: codex-task-inspector
description: Inspect Codex async task progress from the correct data directory. Use when the user asks to check Codex status, inspect async task progress, summarize task state, or determine whether local Codex workers are still running. Prefer Codex default data dir (~/.codex) when given a rollout/session id; use repo .codex-tasks only when the request explicitly targets it.
---

# Codex Task Inspector

用于统一检查 Codex 异步任务状态，避免把“仓库里的任务跟踪文件（`.codex-tasks/`）”与“Codex 默认数据目录（`~/.codex`）里的 session/rollout 日志”混为一谈。

## 关键纠错（高频踩坑）

当用户提供类似 `019df414-...` 这种 **rollout/session id** 并询问“任务进展”，**默认应优先检查 `~/.codex`**：

- ✅ `~/.codex/sessions/YYYY/MM/DD/rollout-...-<id>.jsonl`（权威事实来源：含 task_complete、命令执行、CI watch 记录）
- ⚠️ `.codex-tasks/` 只在用户明确说“看仓库里的 .codex-tasks 跟踪”或你确定该任务是由仓库派发器写入 `.codex-tasks/` 时才用

如果你先去 `.codex-tasks/` 导致找不到 id，这是误判路径；应立即切换到 `~/.codex` 再查。

## 适用场景

当用户提到以下意图时触发：

- “检查 Codex 状态”
- “看看 Codex 任务进展”
- “汇总 `.codex-tasks`”
- “本地任务还在跑吗”
- “CI 这边还剩什么”
- “哪个 Codex 任务失败了”

## 核心原则

1. **先判定本地进程，再看文件产物。** 不要因为 `.codex-tasks` 里还有文件，就误判任务仍在运行。
2. **本地任务状态与 CI 状态分开汇报。** `.pid` 代表本地进程，`*-last.md` / `*-report-*.md` 代表任务产物，`*.log` 里的 poll 代表 CI 轮询。
3. **默认只读。** 不修改 `.codex-tasks/`，不清理 pid/log/report，不擅自重跑任务。
4. **输出必须给出下一步建议，但只限于：继续查失败原因 / 整理状态表 / 指定某任务深挖。**

## 标准检查顺序

### 第 0 步：先选对数据目录（必须）

**输入信号 → 选路由：**

- 用户给的是 `019...` 这种 rollout/session id，或明确说“去 Codex 默认数据目录” → **走 `~/.codex`**
- 用户明确说“看这个仓库 `.codex-tasks`” → 走 **`.codex-tasks/`**

#### 0.1 在 `~/.codex` 按 rollout/session id 定位日志

优先在 `~/.codex/sessions/` 下找 `rollout-*-<id>.jsonl`：

```bash
RID='019df414-235e-74e3-be4b-84f883e0ea17'
python3 - <<'PY'
import os, glob
rid=os.environ['RID']
base=os.path.expanduser('~/.codex/sessions')
paths=glob.glob(f"{base}/**/rollout-*-{rid}.jsonl", recursive=True)
for p in sorted(paths):
  print(p)
PY
```

若直接匹配不到，可扩大为内容扫描（注意控制扫描数量，按 mtime 取最近 N 个文件即可）。

#### 0.2 从 jsonl 里读最终结论

`task_complete` 的 `last_agent_message` 是最直接的任务总结；如需“是否仍在跑”，看最后的事件时间戳 + 是否还有后续 `exec_command_*`。

---

### 第一步：检查本地 Codex 进程

如果走 `.codex-tasks/` 路径：先看 `.codex-tasks/*.pid`，逐个用 `ps` 判断进程是否仍存活。

推荐命令：

```bash
python3 - <<'PY'
import os, glob, subprocess
for path in sorted(glob.glob('.codex-tasks/*.pid')):
    name = os.path.basename(path)
    with open(path) as f:
        pid = f.read().strip()
    if not pid.isdigit():
        print(f'{name}\t{pid}\tINVALID_PID')
        continue
    p = subprocess.run(
        ['ps', '-p', pid, '-o', 'pid=,etime=,state=,ppid=,command='],
        capture_output=True,
        text=True,
    )
    line = ' '.join(p.stdout.split()) if p.stdout.strip() else 'NOT_RUNNING'
    print(f'{name}\t{pid}\t{line}')
PY
```

判定规则：

- 只要 `ps` 没有返回进程行，就记为 `NOT_RUNNING`
- 如果所有 pid 都是 `NOT_RUNNING`，必须明确告诉用户：
  - **本地 Codex 异步任务当前没有在运行**
- 如果存在存活进程，再单独列出活跃项

### 第二步：检查任务产物结论

优先看最近的 `*-last.md`：

```bash
python3 - <<'PY'
from pathlib import Path
base = Path('.codex-tasks')
files = sorted(base.glob('*-last.md'), key=lambda p: p.stat().st_mtime, reverse=True)
for p in files[:10]:
    text = p.read_text(errors='ignore')
    first_nonempty = next((line.strip() for line in text.splitlines() if line.strip()), '')
    print(f'=== {p.name} ===')
    print(first_nonempty[:200])
    print()
PY
```

必要时补充：

- `*-report-*.md`：查看最终报告
- `*.jsonl` / `*.meta`：只在需要确认任务上下文时再读

关注点：

- 是否已经“已完成”
- 是否有明确阻塞原因（例如 push 失败、网络解析失败、CI 失败）
- 是否已经产出了报告文件路径

### 第三步：检查 CI 轮询状态

如果 `.codex-tasks/` 下存在 `*.log`，优先检查与当前话题最相关的 poll log。

例如读取最后几十行：

```bash
tail -n 40 .codex-tasks/skill-creator-ci-poll.log
```

或按需读取尾部：

```bash
python3 - <<'PY'
from pathlib import Path
p = Path('.codex-tasks/skill-creator-ci-poll.log')
if p.exists():
    lines = p.read_text(errors='ignore').splitlines()
    for line in lines[-40:]:
        print(line)
PY
```

关注分类：

- `completed/success`
- `completed/failure`
- `in_progress/pending`
- `queued/pending`

如果看到失败项：

- 必须单独列出失败 job 名称
- 不要把它和“本地 Codex 任务还在运行”混为一谈

## 推荐输出结构

回答时固定分四段：

### 1. 本地 Codex 进程

- 是否有本地活跃进程
- 哪些 pid 已停止 / 仍运行

### 2. 任务产物摘要

- 最近几个 `*-last.md` 的结论
- 是否已有报告、是否有阻塞原因

### 3. CI 状态

- 当前仍在跑的 job
- 已失败的 job
- 已完成的关键 job（只列核心，不必全抄）

### 4. 下一步建议

默认只给这类建议：

- “继续查某个失败 job 的原因”
- “把 `.codex-tasks` 整理成状态表”
- “展开某个具体任务报告”

## 禁止事项

- 不要因为 `.codex-tasks` 文件存在就说“任务还在跑”
- 不要先读一堆 report 再去确认 pid
- 不要把 CI poll log 当成本地任务进程状态
- 不要默认替用户清理 `.codex-tasks`
- 不要直接建议“修代码”或“重跑任务”，除非用户明确要求

## 最小结论模板

如果本地进程全停、CI 仍在跑且有失败项，优先用类似结构：

```text
- 本地 Codex：没有正在运行的任务
- 任务产物：最近若干任务已完成/有报告，某任务存在阻塞原因
- CI：还有若干 job 在跑，且某个 job 已失败
- 下一步：可以继续查失败原因，或者整理状态表
```
