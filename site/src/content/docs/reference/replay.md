---
title: "请求重放说明"
description: "请求重放能力、支持类型和使用建议。"
editLink: false
---

> 此页面由 `docs/replay.md` 自动同步生成。
# 请求重放说明

Bifrost 提供类似 Postman 的请求管理与重放能力，用于保存、组织和再次执行 HTTP 请求。

## 功能概览

- 支持 HTTP、HTTPS、SSE、WebSocket 请求重放
- 支持请求集合与文件夹分组
- 自动保留历史记录和执行结果
- 重放时可选择是否应用当前代理规则

## 支持的请求类型

| 类型 | 说明 |
| --- | --- |
| HTTP | 标准 HTTP/HTTPS 请求 |
| SSE | Server-Sent Events 流式请求 |
| WebSocket | WebSocket 双向通信 |

## 常见请求体格式

| 格式 | Content-Type |
| --- | --- |
| JSON | `application/json` |
| XML | `application/xml` |
| Text | `text/plain` |
| HTML | `text/html` |
| JavaScript | `application/javascript` |
| Form Data | `multipart/form-data` |
| URL Encoded | `application/x-www-form-urlencoded` |
| Binary | `application/octet-stream` |

## 使用建议

- 通过管理端进入 Replay/Collections 相关页面创建和管理请求
- 结合 Traffic 页面把抓到的请求保存到集合中
- 需要调试代理规则时，可开启“应用规则”后重放同一请求做对比
