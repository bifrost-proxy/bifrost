---
name: "github-actions-cookie-login"
description: "为 github.com 的 Actions 页面获取登录 Cookie 并保存到 .env/。当需要打开 GitHub Actions 工作流页面、等待用户登录、校验登录态、并把 Cookie 持久化给后续 CI 查询脚本使用时触发。"
---

# GitHub Actions Cookie Login

这个 skill 是 `site-cookie-login` 的 GitHub 站点化封装，目标是稳定拿到 `github.com` 登录态，并把 Cookie 保存到 `.env/.cookie.github.com`。

## 何时使用

- 需要登录 GitHub Actions 页面
- 需要把 GitHub Cookie 落盘到 `.env/`
- 后续 `github-actions-ci-inspect` 要复用同一份登录态

## 默认配置

- 登录页配置：`.env/github-actions-login.json`
- Cookie 输出：`.env/.cookie.github.com`
- 目标页面：`https://github.com/bifrost-proxy/bifrost/actions/workflows/ci.yml`

## 执行方式

先确保 `site-cookie-login` 依赖可用，再运行：

```bash
bash .trae/skills/github-actions-cookie-login/scripts/github-login --config .env/github-actions-login.json
```

如果要改站点或仓库，复制并修改：

- `.trae/skills/github-actions-cookie-login/references/github-actions-login.template.json`

## 登录成功判定

脚本默认同时检查：

1. 关键 Cookie 存在：`user_session`、`logged_in`
2. HTTP 探针通过：访问 `https://github.com/settings/profile`
3. 页面内容不包含 `Sign in to GitHub`

## 结果

登录成功后，Cookie 会保存到 `.env/.cookie.github.com`，供后续 GitHub Actions CI 分析脚本直接读取。
