---
name: "github-actions-ci-inspect"
description: "读取 github.com 登录 Cookie，查询指定仓库 workflow 的最新 GitHub Actions run、jobs、steps、失败注解与日志入口；同时提供基于 bifrost search/traffic 的接口抓取线索，便于后续排查和修复 CI 问题。"
---

# GitHub Actions CI Inspect

这个 skill 用于把 GitHub Actions 的排查路径固化成脚本：

1. 读取 `.env/.cookie.github.com`
2. 找到 workflow 最新 run
3. 列出 jobs、matrix jobs、steps
4. 汇总失败 step、warning、annotation、raw log URL
5. 自动标出 job 的平台上下文（windows / macos / linux、x64 / arm64）
6. 直接抽取失败日志片段，方便只看 CI 就分析问题
7. 给后续问题定位和修复提供结构化输出

## 前置条件

先用 `github-actions-cookie-login` 拿到 Cookie：

```bash
bash .trae/skills/github-actions-cookie-login/scripts/github-login
```

## 直接查询 CI

默认查询 `bifrost-proxy/bifrost` 的 `ci.yml`。

### Watch 模式（默认）

启动后自动进入 **watch 模式**，持续输出正在运行的第一个 job 的日志，直到 job 完成或手动终止（Ctrl+C）：

```bash
bash .trae/skills/github-actions-ci-inspect/scripts/github-actions-ci
```

指定某个 job（支持 job ID 或名称模糊匹配）：

```bash
bash .trae/skills/github-actions-ci-inspect/scripts/github-actions-ci --job 69012632349
bash .trae/skills/github-actions-ci-inspect/scripts/github-actions-ci --job "Linux Test"
```

输出所有失败 job 的日志：

```bash
bash .trae/skills/github-actions-ci-inspect/scripts/github-actions-ci --failed-only
```

调整轮询间隔（默认 5000ms）：

```bash
bash .trae/skills/github-actions-ci-inspect/scripts/github-actions-ci --poll-interval 3000
```

### 经典模式（一次性输出）

使用 `--no-watch` 回退到原来的一次性汇总输出：

```bash
bash .trae/skills/github-actions-ci-inspect/scripts/github-actions-ci --no-watch
bash .trae/skills/github-actions-ci-inspect/scripts/github-actions-ci --no-watch --fetch-logs --failed-only
bash .trae/skills/github-actions-ci-inspect/scripts/github-actions-ci --no-watch --run 23605768124 --format json
bash .trae/skills/github-actions-ci-inspect/scripts/github-actions-ci --no-watch --repo owner/repo --workflow ci.yml
```

### 参数一览

| 参数 | 说明 |
|---|---|
| `--job <id\|name>` | 指定要 watch 的 job（ID 或名称模糊匹配） |
| `--failed-only` | 仅输出失败的 job 日志 |
| `--no-watch` | 禁用 watch 模式，使用经典一次性输出 |
| `--poll-interval <ms>` | watch 模式轮询间隔（默认 5000ms） |
| `--run <id\|latest>` | 指定 run ID，默认 latest |
| `--fetch-logs` | 经典模式下拉取 step 日志 |
| `--format <text\|json>` | 经典模式输出格式 |
| `--repo <owner/repo>` | 仓库 |
| `--workflow <file>` | workflow 文件名 |

推荐在排查 CI-only 问题时使用：

```bash
bash .trae/skills/github-actions-ci-inspect/scripts/github-actions-ci --failed-only
```

这会优先输出：

- 失败 job 摘要
- 失败 step
- job 对应平台 / 架构
- 失败测试名（如果日志里有 `Failed tests:` 这类结构）
- 疑似根因摘要（例如 `Detail ...`、`panic`、`timed out`、`assertion`）
- 失败日志 excerpt
- 错误命中点上下各 50 行的日志上下文
- 更像根因的 annotation / exit code / timeout 信息

## 用 Bifrost 抓 GitHub Actions 接口

当需要确认 GitHub 前端近期是否换了接口，先让浏览器走 Bifrost 打开 workflow / run / job 页面，再执行：

```bash
bash .trae/skills/github-actions-ci-inspect/scripts/trace-github-actions-interfaces.sh 9900 github.com
```

它会用 `bifrost search` 搜这些关键路径：

- `actions/workflow-runs`
- `actions/workflow-run/`
- `actions/runs/`
- `graph_partial`
- `graph/matrix/`
- `/job/`
- `/checks/.../logs/...`

## 参考

关键接口模式和解析说明见：

- `.trae/skills/github-actions-ci-inspect/references/github-actions-interfaces.md`
