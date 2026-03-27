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

默认查询 `bifrost-proxy/bifrost` 的 `ci.yml`：

```bash
bash .trae/skills/github-actions-ci-inspect/scripts/github-actions-ci
```

常用参数：

```bash
bash .trae/skills/github-actions-ci-inspect/scripts/github-actions-ci --run latest --fetch-logs
bash .trae/skills/github-actions-ci-inspect/scripts/github-actions-ci --run latest --fetch-logs --failed-only
bash .trae/skills/github-actions-ci-inspect/scripts/github-actions-ci --run 23605768124 --format json
bash .trae/skills/github-actions-ci-inspect/scripts/github-actions-ci --repo owner/repo --workflow ci.yml
```

推荐在排查 CI-only 问题时使用：

```bash
bash .trae/skills/github-actions-ci-inspect/scripts/github-actions-ci --run latest --fetch-logs --failed-only
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
