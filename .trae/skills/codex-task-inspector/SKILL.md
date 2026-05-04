---
name: codex-task-inspector
description: Inspect Codex async task progress from .codex-tasks. Use when the user asks to check Codex status, inspect async task progress, summarize .codex-tasks, determine whether local Codex workers are still running, or distinguish local task state from CI poll status.
---

# Codex Task Inspector

用于统一检查仓库内 `.codex-tasks/` 的 Codex 异步任务状态，避免重复走弯路。

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

### 第一步：检查本地 Codex 进程

先看 `.codex-tasks/*.pid`，逐个用 `ps` 判断进程是否仍存活。

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
