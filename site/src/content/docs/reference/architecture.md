---
title: "项目结构与模块说明"
description: "Bifrost 代码仓库结构与核心模块说明。"
editLink: false
---

> 此页面由 `docs/architecture.md` 自动同步生成。
# 项目结构与模块说明

## 项目结构

```text
.
├── crates/
│   ├── bifrost-core/
│   ├── bifrost-command/
│   ├── bifrost-proxy/
│   ├── bifrost-tls/
│   ├── bifrost-storage/
│   ├── bifrost-script/
│   ├── bifrost-admin/
│   ├── bifrost-cli/
│   ├── bifrost-power/
│   ├── bifrost-e2e/
│   ├── bifrost-tests/
│   ├── bifrost-sync/
│   ├── agent/
│   └── skills/
├── web/
├── desktop/
├── docs/
├── e2e-tests/
└── tests/
```

## 模块说明

### `bifrost-core`

核心规则库，负责规则解析、匹配器和协议定义。

### `bifrost-command`

远程调用与命令执行共享模型，承载 remote-invoke 的命令 payload、结果结构和传输边界。

### `bifrost-proxy`

代理服务器实现，负责 HTTP/HTTPS/SOCKS5/WebSocket/隧道等协议处理。

### `bifrost-tls`

TLS 证书管理模块，负责 CA 证书生成、动态签发与缓存。

### `bifrost-storage`

配置、规则、Values、状态等持久化能力。

### `bifrost-script`

基于 QuickJS 的脚本引擎与沙箱执行环境。

### `bifrost-admin`

管理后台静态资源与 Admin API。

### `bifrost-cli`

命令行工具，提供服务启动、规则管理、流量查询、配置维护等命令。

### `bifrost-power`

macOS 防睡眠能力封装，用于本机和远端 `keep-awake` 命令。

### `bifrost-e2e`

Rust 端到端测试 runner。

### `bifrost-tests`

测试辅助 crate。

### `bifrost-sync`

远程同步模块，负责规则与配置的远程同步能力。

### `agent`

Agent 会话与外部 Runner 编排能力，负责持久化会话、状态和运行记录，并把 Codex、Claude Code 等外部执行器接入 Chat、IM 与定时任务场景。

### `skills`

Agent Skill 模板与安装内容，配合 `bifrost install-skill` 给 Claude Code、Codex、Trae、Cursor、GitHub Copilot 和通用 Agent Skills 运行时提供 Bifrost 使用说明。
