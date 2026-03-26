---
name: "site-cookie-login"
description: "Open a target website, wait for user login, verify login with required cookies plus an HTTP probe, and save cookies into .env for later automation. Use this when a site needs controlled browser login, cookie persistence, and automatic login-state checks."
---

# Site Cookie Login

通用的「浏览器登录 → 抓 Cookie → 校验登录态 → 持久化」能力，可接入任意需要浏览器登录的站点。

## 适用场景

- 目标站点不提供 API Token，只能通过浏览器登录获取 Cookie
- 需要把登录态 Cookie 落盘供后续自动化脚本使用
- 需要在 Cookie 失效时自动触发重新登录

## 核心流程

1. 用可视浏览器（Puppeteer）打开目标站点登录页
2. 等待用户在浏览器中完成登录（支持手动触发检测）
3. 从浏览器抓取目标域的全部 Cookie
4. 检查必需 Cookie 是否存在（`requiredCookies`）
5. 用 HTTP 探针请求一个需要鉴权的接口，验证登录态真实可用（`verify`）
6. 校验通过后将 Cookie 保存到指定文件

## 快速使用

### 1. 安装依赖

```bash
cd .trae/skills/site-cookie-login/scripts
npm install
```

### 2. 创建站点配置

复制模板并按目标站点修改参数：

```bash
cp .trae/skills/site-cookie-login/references/config.template.json .env/<your-site>-login.json
```

### 3. 执行登录

```bash
node .trae/skills/site-cookie-login/scripts/site-login.js --config .env/<your-site>-login.json
```

## 配置文件说明

配置模板位于 `references/config.template.json`，所有 `<placeholder>` 需替换为实际值。

### 基础字段

| 字段 | 说明 |
|------|------|
| `name` | 站点显示名称，用于日志输出 |
| `url` | 浏览器打开的登录页 URL |
| `domain` | 目标 Cookie 的域名（用于从浏览器筛选 Cookie） |
| `outputFile` | Cookie 输出文件路径（建议使用相对路径，如 `.env/.cookie.<site>`） |
| `timeout` | 等待登录的超时时间（毫秒），默认 300000（5 分钟） |

### Cookie 相关

| 字段 | 说明 |
|------|------|
| `requiredCookies` | 字符串数组，判定登录成功必须存在的 Cookie 名 |
| `mergeCookieFiles` | 可选，额外的 Cookie 文件路径数组，会与浏览器抓取的 Cookie 合并（如 SSO 共享的 Cookie） |

### 验证探针（`verify`）

用于通过 HTTP 请求验证 Cookie 是否真正有效：

| 字段 | 说明 |
|------|------|
| `url` | 需要鉴权的 API 端点 |
| `method` | HTTP 方法（GET / POST 等） |
| `headers` | 请求头（Cookie 由脚本自动注入，无需手动填写） |
| `body` | 请求体，留 `null` 表示无 body |
| `successStatuses` | 视为成功的 HTTP 状态码数组，如 `[200]` |
| `rejectBodyIncludes` | 响应体中包含任一字符串则判定失败（如 `"not login"`） |
| `successBodyIncludes` | 响应体中需包含任一字符串才判定成功，留空数组表示不检查 |

## 验证策略

建议同时启用两层校验以确保可靠性：

1. **Cookie 存在性检查**：通过 `requiredCookies` 确认关键 Cookie 已获取
2. **HTTP 探针验证**：通过 `verify` 发送真实 API 请求确认登录态有效

仅配置 `requiredCookies` 而不配置 `verify` 也可以工作，但无法保证 Cookie 未过期。

## 接入新站点的工作流

1. **分析目标站点**：在浏览器 DevTools 中登录目标站点，观察哪些 Cookie 是登录态的关键标识
2. **找到鉴权接口**：在 Network 面板找一个需要登录才能正常响应的 API，用于做登录态探针
3. **创建配置文件**：基于模板填写站点信息、必需 Cookie、验证接口
4. **测试登录流程**：运行 `site-login.js`，确认 Cookie 能正常落盘
5. **集成到业务脚本**：让业务脚本读取输出的 Cookie 文件，在鉴权失败时可复用此 skill 重新登录
