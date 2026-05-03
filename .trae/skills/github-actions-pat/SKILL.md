---
name: "github-actions-pat"
description: "用 Personal Access Token 通过 GitHub REST API 分析 Actions CI 的失败 run/job/step、拉取日志、轮询运行状态、做 PR code review，并驱动 fix → push → watch → iterate 的闭环。Token 只从 GITHUB_TOKEN 环境变量读取，不落盘、不回显。适合在 bifrost remote / CI 调度器 / 无头环境里跑。仓库锁定在 AGENTS.md 或调用处通过 GH_REPO 显式指定。"
---

# GitHub Actions PAT Inspector

把"用 PAT 分析 GitHub Actions CI"这条路径固化成可复用脚本。本 skill 解决的典型场景：

1. Agent 在 **bifrost remote** 目标机或 caller 上跑，**必须**走 REST API 自动化，不走浏览器 cookie
2. 需要自动化「修 → push → 盯 CI → 失败继续修」的闭环
3. 需要按 run-id / PR / sha / branch 精确定位失败并拉 job log
4. 需要做 PR diff review（生成 markdown 或直接 POST）

**auth 唯一路径：PAT 从 `GITHUB_TOKEN` 环境变量读取**。不支持、不回退到 cookie / OAuth device flow / SSH / gh CLI 登录等其他方式。需要 agent 自主跑的 CI 分析、PR review、fix-push-watch 循环，全部用本 skill。

## 授权契约（唯一输入：`GITHUB_TOKEN` 环境变量）

- 本 skill 只从环境变量 `GITHUB_TOKEN` 读 token。**不落盘、不写日志、不回显原文。**
- 读取方式推荐（适配用户 shell 配置）：
  ```bash
  # macOS / zsh 用户：交互式 shell 才会加载 ~/.zshrc
  zsh -ic 'python3 scripts/gh_ci.py run <run_id>'
  # bash 用户
  bash -lc 'python3 scripts/gh_ci.py run <run_id>'
  # 或在脚本运行前已 export GITHUB_TOKEN 的环境
  python3 scripts/gh_ci.py run <run_id>
  ```
- 未设置 → 脚本 `exit 2`，提示 `ERROR: set GITHUB_TOKEN before running this skill`。
- 推荐 scope：`repo` + `actions:read`（只读分析）。`--post` 发 review 时额外需要 `pull_requests:write`。
- 首选 **fine-grained PAT** 并把 repo 限定到目标仓库，爆炸半径最小。

## 仓库定位策略

脚本默认读环境变量 `GH_REPO`（格式 `owner/repo`），未设置时回退到本仓库。Agent 可在调用前写入：

```bash
export GH_REPO=bifrost-proxy/bifrost
python3 scripts/gh_ci.py pr 567
```

或者在 AGENTS.md 顶部的仓库元信息中固化。

## 目录结构

```
scripts/
├── common.py         # token/http/分页/日志切片/归因
├── gh_ci.py          # run / pr / sha / branch / regression
├── gh_review.py      # PR metadata + diff + 分层建议 + 可选 --post
└── poll_run.py       # 轮询一个 run 直到完成
references/
└── pitfalls.md       # GitHub API 坑点清单（job log 302、Accept 415、system proxy MITM）
```

## 典型调用

### (A) CI 错误定位

```bash
# 按 run-id 分析
python3 scripts/gh_ci.py run 25269751068

# 按 PR 号找最近一次 failed
python3 scripts/gh_ci.py pr 567

# 按 commit sha 找
python3 scripts/gh_ci.py sha a96a4257

# 按分支找最近一次 failed
python3 scripts/gh_ci.py branch feat/agent --only-failed

# 与上一次 green 做 regression 对比（给出 compare URL + 提交区间）
python3 scripts/gh_ci.py regression 25269751068
```

脚本行为：
1. `GET /repos/{owner}/{repo}/actions/runs/...` 拉 run 元数据
2. `GET .../jobs` 筛出 `conclusion=failure`
3. 对每个 failed job：`GET .../jobs/{id}/logs` → **自动处理 302 到 Azure Blob signed URL**（不能带 Authorization）
4. 日志末尾 500 行扫关键词（`error:`、`FAIL`、`panicked`、`##[error]`、`thread '...' panicked`、`test result: FAILED` 等），抽 ±20 行上下文
5. 输出结构化 markdown：失败 job / step / URL / 根因桶（compile / test / lint / fmt / timeout / network / OOM）/ 关键日志片段 / 本地复现命令

### (B) 轮询 run 到完成（fix-push-watch 循环用）

```bash
POLL_SEC=45 MAX_WAIT_SEC=1800 python3 scripts/poll_run.py 25271859306
# exit 0 = success, 2 = failure, 3 = timeout
```

典型闭环（agent 要这么跑，不要反复问用户）：

```bash
# 1) 修代码（用 bifrost remote file edit 走乐观锁）
# 2) push
git push origin feat/agent
# 3) 找到新 run
python3 scripts/gh_ci.py branch feat/agent --only-failed --any-status
# 4) 轮询到完成
python3 scripts/poll_run.py <new_run_id>
# 5) 失败 → 回到 (1) 拉日志继续修；成功 → 汇报
```

### (C) PR code review

```bash
# 只生成 markdown（默认，不发）
python3 scripts/gh_review.py 123

# 聚焦某些路径
python3 scripts/gh_review.py 123 --focus 'crates/agent/**'

# 限制 diff 上下文
python3 scripts/gh_review.py 123 --max-diff-lines 4000

# 显式 post（需要 pull_requests:write）
python3 scripts/gh_review.py 123 --post --event REQUEST_CHANGES
```

默认**只生成 markdown 不发**；只有用户明确说"发出去 / post it / go ahead" 时才加 `--post`。

## 已知坑点（踩过的 ⚠️）

1. **Azure Blob signed URL 不能带 Authorization**
   GitHub 的 `/actions/jobs/{id}/logs` 返回 302 指向 `*.blob.core.windows.net` 签名 URL。如果 HTTP client 自动跟随 redirect 并继续带 `Authorization: Bearer ...`，Azure 会 `401 InvalidAuthenticationInfo`。解决：手动拦截 redirect，二次请求移除 Authorization（本 skill 的 `_fetch_job_log` 已这样做）。

2. **Accept 头不能写 `text/plain`**
   `jobs/{id}/logs` 端点会返回 `415 Unsupported 'Accept' header`。使用默认 `application/vnd.github+json`（由服务端 302 到签名 URL，body 是纯文本，直接 decode）。

3. **system proxy 会导致 SSL 证书 MITM 失败**
   如果用户本机挂了代理（包括 bifrost 自己），Python 的 `urllib` 可能因 `Missing Authority Key Identifier` 而 `CERTIFICATE_VERIFY_FAILED`。跑脚本时 **主动清掉代理环境变量**：
   ```bash
   NO_PROXY=api.github.com,github.com,*.blob.core.windows.net \
   HTTPS_PROXY= HTTP_PROXY= ALL_PROXY= https_proxy= http_proxy= all_proxy= \
   python3 scripts/gh_ci.py run <id>
   ```

4. **bifrost remote exec 的 quoting 噩梦**
   在 bifrost remote 上跑 Python 单行命令，`\"` 反斜杠层级会叠 3 层。**把探针脚本写成文件后上传再执行**（用 `bifrost remote file write` + `bifrost remote exec python3 <script>`）。

5. **PII mask 会吞掉 token 原文**
   如果用户直接在聊天里粘 `ghp_xxx...`，平台 DLP 会把它替换成占位符。Agent 拿不到明文。正确做法：
   - 让用户在本机自己 `export GITHUB_TOKEN=...` 或写到 `~/.bifrost/gh_token`，再告诉 agent 路径
   - Agent 通过 `zsh -ic` / `bash -lc` 加载 shell rc 读取

6. **run log (run-level) vs job log (job-level)**
   run-level 日志是 zip，job-level 是纯文本。**定位问题用 job 级别更快**。本 skill 默认走 job 级别。

7. **GitHub Actions 的 `Accept` 为 `application/vnd.github+json` 时，job log 返回的实际是 302 + 文本 body**；不要用 binary 模式以外的逻辑复杂化解析。

## Agent 工作流约束

1. **执行 API 前先确认 token**：
   ```bash
   [ -n "$GITHUB_TOKEN" ] || { echo "ERROR: GITHUB_TOKEN missing"; exit 2; }
   ```
   或用 `zsh -ic` 加载用户 shell 配置。
2. **设置 `GH_REPO`**：每次调用前确认目标仓库，不要依赖默认值猜测。
3. **日志最小化**：不要把完整 workflow log 回显给用户；只抽关键段 + URL + 根因桶。
4. **PR review 分层**：输出必须分 Must-fix / Should-fix / Nit 三层；不能全塞 nit。
5. **`--post` 加锁**：执行 `--post` 前必须在用户消息里有明确同意（"发出去 / post it / go ahead"）。
6. **token 纪律**：绝不 `echo $GITHUB_TOKEN`、不把 token 写进日志、不塞进 URL。
7. **fix-push-watch 闭环**：用户要求"自动修到绿"时，**不要反复确认**。按（定位失败 → 修 → commit → push → poll_run → 再分析）循环，直到 conclusion=success 或 3 次失败仍定位不到根因时才回报。
8. **proxy 隔离**：所有脚本调用前加 `NO_PROXY=api.github.com,github.com,*.blob.core.windows.net HTTPS_PROXY= HTTP_PROXY= ALL_PROXY=` 前缀，避免 MITM 证书问题。

## 常见排障

| 现象 | 原因 | 处置 |
|---|---|---|
| `ERROR: set GITHUB_TOKEN` | 未 export 或 shell 未加载 rc | `zsh -ic` / `bash -lc`，或让用户手动 export |
| `401 Bad credentials` | token 失效 / 被吊销 | 让用户重新签发并 re-export |
| `401 InvalidAuthenticationInfo` on `*.blob.core.windows.net` | signed URL 被带了 Authorization | 用本 skill 的 `_fetch_job_log`，不要用原生 urllib 自动跟随 |
| `415 Unsupported 'Accept'` | Accept 设置为 text/plain 访问 logs 端点 | 用默认 Accept，服务端会 302 到 blob |
| `CERTIFICATE_VERIFY_FAILED: Missing Authority Key Identifier` | 走了 bifrost 本身的 system proxy | `NO_PROXY=api.github.com,...`、`HTTPS_PROXY= HTTP_PROXY=` |
| `404 Not Found` on run_id | run 在别的 repo / 已清理 / repo 拼错 | 核对 `GH_REPO`，或换 run_id |
| `x-ratelimit-remaining: 0` | 命中限流 | 等 `x-ratelimit-reset`（epoch 秒），或换 token |

## 与其他 skill 的边界

- **本 skill（`github-actions-pat`，PAT + Python）**：所有 GitHub Actions CI 检查、日志分析、PR review、fix-push-watch 闭环的唯一入口。
- **`rust-project-validate`** / **`e2e-test`** / **`e2e-verify`**：本地测试通过再 push，避免把"能在本地跑通"的活儿甩给 CI。
- 提交代码阶段仍按 AGENTS.md 强制走 `cargo fmt` / `cargo clippy -D warnings` / human_tests 流程。本 skill 只做「拉数据 + 归因 + 闭环驱动」，**不替代**本地 CI 前置检查。
