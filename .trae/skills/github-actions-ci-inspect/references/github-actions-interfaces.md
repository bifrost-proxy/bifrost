# GitHub Actions 接口线索

下面这些模式来自对 `https://github.com/bifrost-proxy/bifrost/actions/workflows/ci.yml` 的实时页面抓取，抓取时间为 2026-03-27（Asia/Shanghai，对应 GitHub 响应日期 2026-03-26）。

## 1. workflow 列表页

- 页面：`/{owner}/{repo}/actions/workflows/{workflow_file}`
- 示例：`/bifrost-proxy/bifrost/actions/workflows/ci.yml`

页面内可直接看到：

- workflow runs 列表分页 partial
  - `/{owner}/{repo}/actions/workflow-runs?lab=false&page=1&workflow_file_name=ci.yml`
- 单条 run 行 partial
  - `/{owner}/{repo}/actions/workflow-run/{check_suite_id}`
- run 详情页链接
  - `/{owner}/{repo}/actions/runs/{run_id}`

## 2. run 详情页

- 页面：`/{owner}/{repo}/actions/runs/{run_id}`

关键 partial：

- execution graph
  - `/{owner}/{repo}/actions/runs/{run_id}/graph_partial`
- approvals banner
  - `/{owner}/{repo}/actions/runs/{run_id}/approvals_banner_partial`
- concurrency banner
  - `/{owner}/{repo}/actions/runs/{run_id}/concurrency_banner_partial`

run 页同时提供：

- 单个 job 页面
  - `/{owner}/{repo}/actions/runs/{run_id}/job/{job_id}`
- matrix 展开 partial
  - `/{owner}/{repo}/actions/runs/{run_id}/graph/matrix/{matrix_token}?attempt=1&expanded=true`
  - 这个接口建议带 `X-Requested-With: XMLHttpRequest`

## 3. job 详情页

- 页面：`/{owner}/{repo}/actions/runs/{run_id}/job/{job_id}`

页面内关键字段：

- job header partial
  - `/{owner}/{repo}/actions/runs/{run_id}/header_partial?selected_check_run_id={job_id}`
- log header partial
  - `/{owner}/{repo}/runs/{job_id}/header`
- step 节点
  - `<check-step data-name="..." data-number="..." data-conclusion="..." data-log-url="...">`

## 4. step 日志 URL

job 页的每个 step 会暴露：

- `/{owner}/{repo}/commit/{sha}/checks/{job_id}/logs/{step_number}`

这个 URL 可作为后续拉取 raw step log 的入口。未登录时可能返回 `404 Not Found` 或要求登录。

## 5. 注解与失败定位

run 页和 job 页都能看到 annotation：

- run 页锚点
  - `/{owner}/{repo}/actions/runs/{run_id}/job/{job_id}#step:{step_number}:{column}`
- job 页锚点
  - `#annotation:{step_number}:{column}`

这些 annotation 通常能提前给出：

- 失败 step 编号
- warning / deprecation
- 可直接用于问题分析的错误文本

## 6. 推荐的排查顺序

1. workflow 列表页找最新 run
2. run 页找失败 job / 正在运行的 matrix
3. 如 matrix 折叠，调 expanded partial 展开
4. job 页读取 `check-step`
5. 对失败 step 读取 `data-log-url`
6. 结合 run/job annotations 做错误归因
