# Bifrost

<p align="center">
  <strong>高性能 HTTP/HTTPS/SOCKS5 代理服务器</strong>
</p>

<p align="center">
  <a href="https://github.com/bifrost-proxy/bifrost/actions"><img src="https://github.com/bifrost-proxy/bifrost/workflows/CI/badge.svg" alt="CI Status"></a>
  <a href="https://github.com/bifrost-proxy/bifrost/releases"><img src="https://img.shields.io/github/v/release/bifrost-proxy/bifrost" alt="Release"></a>
  <a href="https://github.com/bifrost-proxy/bifrost/releases"><img src="https://img.shields.io/github/downloads/bifrost-proxy/bifrost/total" alt="Downloads"></a>
  <a href="https://github.com/bifrost-proxy/bifrost/blob/main/LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue.svg" alt="License"></a>
</p>

> [帮助文档](https://github.com/bifrost-proxy/bifrost/tree/main/docs)

Bifrost 是一个用 Rust 编写的高性能，AI 友好的代理服务器，灵感来源于 [Whistle](https://github.com/avwo/whistle)。它提供请求拦截、规则修改、TLS 拦截、脚本扩展、流量查看、请求重放以及 Web UI 管理能力。

## 快速开始

安装 CLI：

方法一：使用脚本安装

```bash
curl -fsSL https://raw.githubusercontent.com/bifrost-proxy/bifrost/main/install-binary.sh | bash
```

安装指定版本

```bash
curl -fsSL https://raw.githubusercontent.com/bifrost-proxy/bifrost/main/install-binary.sh | bash -s -- --version v0.0.48-beta
```

方法二：使用 npm 安装

```bash
npm i -g @bifrost-proxy/bifrost
```

更多安装方法：[`docs/getting-started.md`](docs/getting-started.md)

启动代理：

```bash
bifrost start -d
```

启动后访问管理端：

```text
http://127.0.0.1:9900/_bifrost/
```

## 和AI集成
```bash
bifrost install-skill -y
```

## 特性说明

![network.png](assets/network.png) <img width="1500" height="783" alt="image" src="https://github.com/user-attachments/assets/44062a96-47f3-481b-a2b6-e1bda9b3fda9" />
![scripts.png](assets/scripts.png)
![rules.png](assets/rules.png)
![replay.png](assets/replay.png)
![metrics.png](assets/metrics.png)

- 高性能代理内核：基于 Tokio + Hyper，支持高并发与连接复用
- 多协议支持：HTTP/1.1、HTTP/2、HTTP/3、HTTPS、SOCKS5、WebSocket、SSE、gRPC
- TLS 拦截能力：支持 CA 证书生成、按域名动态签发证书、按规则选择拦截或透传
- 规则引擎：支持路由、请求/响应改写、注入、延迟、限速、Mock、脚本处理
- 管理界面：内置 Web UI，支持规则编辑、流量查看、脚本管理、请求重放
- 资源风险告警：Performance 页与 `/_bifrost/api/system/memory` 会显示 body/ws 文件 writer 占用及接近句柄上限的告警状态
- 脚本沙箱：基于 QuickJS，支持 `reqScript`、`resScript`、`decode`

## 开发初始化

克隆仓库后，先执行一次 Git hook 初始化：

```bash
bash scripts/setup-git-hooks.sh
# 或
make setup
```

这会为当前仓库写入本地 `core.hooksPath=.githooks`，即使机器上配置了全局 `hooksPath`，后续 `git commit` 也会优先执行仓库内的 `.githooks/pre-commit`。默认 pre-commit 会检查工作区格式、桌面端 Tauri 格式，以及 `cargo clippy --workspace --all-targets --all-features -- -D warnings`。

## 用不习惯 CLI？想要使用桌面端 APP？

请直接到[releases](https://github.com/bifrost-proxy/bifrost/releases)中下载对应平台的桌面端程序

## 基本用法摘要

常见命令：

```bash
# 查看状态
bifrost status

# 停止服务
bifrost stop

# 管理端远程访问与鉴权（Web UI）
bifrost admin remote status
bifrost admin remote enable
bifrost admin passwd
bifrost admin revoke-all

# 查看流量
bifrost traffic list
bifrost traffic search "keyword" --method POST --host api.openai.com --path /v1/responses
bifrost search "keyword" --req-header
bifrost search "keyword" --res-body

# 添加规则
bifrost rule add local-dev --content "example.com host://127.0.0.1:3000"
```

搜索命令补充说明：

- `bifrost search` 与 `bifrost traffic search` 等价
- 基础过滤支持 `--method`、`--host`、`--path`、`--status`、`--protocol`
- 搜索范围支持 `--url`、`--req-header`、`--res-header`、`--req-body`、`--res-body`
- 兼容别名：`--headers` 会同时搜索请求头和响应头，`--body` 会同时搜索请求体和响应体

规则示例：

```txt
example.com host://127.0.0.1:3000
api.example.com reqHeaders://x-debug=1
chatgpt.com http3://
```

## Agent Research Pack

Bifrost Agent 可通过 Research Pack 获得固定站点采集、统一搜索、本地 SQLite/FTS 知识库、网页抓取、Markdown 日报和可选微信公众号 fallback。启用后，Agent 只暴露高层工具：`research_search`、`research_fetch`、`knowledge_search`、`knowledge_save`、`research_digest`，Provider、缓存、去重和安全抓取策略由 Bifrost 内部处理。

默认 preset 会安装一组面向 AI/技术主题的高质量固定站点 source，并把 Tavily/Exa/Volc/custom HTTP/MCP 这类通用 API Provider 作为补充：

- `arxiv`：arXiv Atom API，固定 workflow 为 `search -> Atom normalize -> paper detail markdown -> knowledge/report metadata`。
- `hacker_news`：HN Algolia Search API，固定 workflow 为 `search_by_date -> story normalize -> page fetch markdown -> knowledge/report metadata`。
- `github_repositories`：GitHub repository search API，固定 workflow 为 `repo search -> repository normalize -> README/page markdown -> knowledge/report metadata`。
- `sogou_wechat_cdp`：Sogou 微信搜索 + 浏览器 CDP，固定 workflow 为 `search -> /link normalize -> CDP fetch detail -> markdown artifact`，用于用户交互式登录/验证码后复用浏览器状态。

快速初始化示例：

```bash
bifrost agent research init \
  --preset personal-cn \
  --web-provider volc \
  --base-url https://your-search-api-endpoint \
  --api-key '$VOLCENGINE_API_KEY' \
  --yes
```

微信公众号可以接入本地 HTTP bridge，也可以接入已打开远程调试端口的浏览器 CDP 会话。CDP 模式用于 Sogou 微信搜索、需要用户交互式登录/验证码后复用浏览器登录态的站点：

```bash
bifrost agent research init \
  --preset personal-cn \
  --wechat-cdp-endpoint http://127.0.0.1:9222 \
  --yes

bifrost agent research search "AI Agent MCP" --wechat --limit 5
```

搜索时可直接抓取详情并返回可沉淀的 Markdown artifact：

```bash
bifrost agent research search "AI Agent MCP" --limit 5 --fetch-content
```

返回结果会包含标准化源信息和正文沉淀字段，包括 `title`、`url`、`canonical_url`、`source`、`provider`、`site_name`、`author`、`published_at`、`retrieved_at`、`content_markdown` 和 `content_hash`。

## 文档索引

- 文档总览：[`docs/README.md`](docs/README.md)
- 项目概览：[`docs/overview.md`](docs/overview.md)
- 安装与启动：[`docs/getting-started.md`](docs/getting-started.md)
- CLI 详细命令：[`docs/cli.md`](docs/cli.md)
- 桌面版安装与构建：[`docs/desktop.md`](docs/desktop.md)
- 规则语法：[`docs/rule.md`](docs/rule.md)
- 操作符说明：[`docs/operation.md`](docs/operation.md)
- 匹配模式：[`docs/pattern.md`](docs/pattern.md)
- 规则协议手册：[`docs/rules/README.md`](docs/rules/README.md)
- Scripts 模块与脚本开发：[`docs/scripts.md`](docs/scripts.md)
- Values 使用说明：[`docs/values.md`](docs/values.md)
- 请求重放说明：[`docs/replay.md`](docs/replay.md)
- 项目结构与模块说明：[`docs/architecture.md`](docs/architecture.md)
- Agent Skill 安装说明：[`docs/agent-skill.md`](docs/agent-skill.md)
- Agent Research Pack 技术方案：[`design/agent-research-pack.md`](design/agent-research-pack.md)
