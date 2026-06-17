# Replay cURL 导入解析增强

## 背景

Replay 页面支持在 URL 输入框粘贴 cURL 命令并自动导入 method / url / headers / body。实际使用中，从 Chrome DevTools 的 “Copy as cURL (bash)” 粘贴时存在解析不完整问题：

- `-b/--cookie` 参数未被转换成 `Cookie` Header，导致回放请求丢失 Cookie
- 当 cURL 使用 Bash 的 ANSI-C quoting（`$'...'`）表达 body 或 header（常见于包含 `$`、换行、制表符、Unicode 转义等内容）时，解析会把 `$` 前缀当作正文内容或错误保留转义，导致 body 内容不一致/格式错误

## 目标

- 能够正确解析 Chrome 复制的 bash cURL（多行 `\` 续行、单双引号、`$'...'` 等）
- 覆盖 Replay 导入需要的核心字段：
  - method（含 `-X/--request`，以及存在 body 时 GET 自动推断为 POST）
  - url（含 `--url` 以及 positional URL）
  - headers（含 `-H/--header`，以及 `-b/--cookie` 转换为 `Cookie`）
  - body（含 `-d/--data*`、`--json`）
- 解析逻辑可复用，避免散落在页面组件内

## 非目标

- 不尝试在 Replay 执行时 100% 复刻 curl 的所有特性（如 multipart `-F`、`@file` 读文件、`--compressed`、`--http2` 等），本次仅保证“字段导入正确、内容不丢失、不被错误改写”

## 现状分析

- 解析入口位于前端 Replay RequestPanel 的 URL 输入框 `onPaste` 事件，解析函数此前内联在组件文件中
- 旧解析器问题点：
  - `-b/--cookie` 被当作“有值但忽略”的 option 跳过，未生成 Header
  - tokenization 只支持 `'` / `"` / `\`，未支持 `$'...'`，导致 Chrome bash cURL 中常见的 ANSI-C quoting 解析失败或内容不一致

## 方案概述

将 cURL 解析器下沉到 `web/src/utils/curl.ts`，提供 `parseCurl()`：

1. **shell-like 分词**：
   - 支持多行续行：Bash `\\\n`、CMD `^\n`、PowerShell `` `\n `` 统一归一为单行
   - 支持单引号 `'...'`（字面量）
   - 支持双引号 `"..."`（按 bash 规则处理反斜杠：仅对 `\\`、`\"`、`\$`、``\` `` 生效，其它场景保留反斜杠）
   - 支持 ANSI-C quoting `$'...'`：
     - 识别 `$'` 前缀，不保留 `$` 本身
     - 在该模式内解析常见转义：`\n`、`\t`、`\r`、`\uXXXX`、`\UXXXXXXXX`、`\xHH`、八进制等
   - 支持 `--key=value`、`-XPOST`、`-HHeader: v` 等 token 形式展开

2. **语义解析**：
   - method：`-X/--request`
   - method 便捷选项：`-I/--head` → `HEAD`
   - url：`--url` 或 positional 中最后一个 URL candidate
   - headers：`-H/--header`（按首个 `:` 分割）
   - cookie：`-b/--cookie` 转换为 `Cookie` Header（若已有 Cookie Header 则用 `; ` 追加）
   - basic auth：`-u/--user user:pass` → `Authorization: Basic <base64>`（仅在尚未存在 `Authorization` Header 时写入）
   - user-agent：`-A/--user-agent` → `User-Agent` Header
   - body：`-d/--data/--data-raw/--data-binary/--data-urlencode/--data-ascii/--json` 收集 bodyParts，按 curl 语义用 `&` 拼接（`--json` 在未显式指定时自动补 `Content-Type: application/json`）
   - raw_type：根据 `Content-Type` 推断（json/xml/html/javascript/text），用于 UI 语法高亮
   - `-G/--get`：将 `--data*` 内容追加到 URL query（不作为 body 导入）

## 兼容与降级策略

- 未识别或与 Replay 执行能力不匹配的 curl 选项：解析器跳过但不报错，保证导入流程不中断
- `-b @cookiefile`：由于无法读取本地文件内容，解析器不尝试导入该 cookiefile（不生成 Cookie Header）
- 解析失败（非 curl / 未能推断 URL）：返回 `null`，保持原 paste 行为不拦截
- 协议约束与安全：Header name 按 RFC7230 token 校验；Header value 中的 CR/LF 会被折叠为空格，避免 CRLF 注入

## 测试方案

- 增加 Playwright 侧的解析单测（Node 环境直接调用 `parseCurl`）覆盖：
  - `-b/--cookie` → Cookie Header
  - `$'...'` 前缀不进入正文、`$` 字符保留
  - `$'...'` 常见转义的解码结果
  - 双引号内非特殊转义保留反斜杠（bash 兼容）
  - `-u/--user` 生成 Basic Authorization
- 回归验证（UI）：
  - 在 Replay 页粘贴 Chrome Copy as cURL，确认 method/url/headers/body 自动填充，Cookie 不丢失、body 不被错误改写

## 校验要求

- 运行前端 UI 测试：`pnpm -C web test:ui`
- 最终提交前执行：`rust-project-validate`（按项目规则）

## 文档更新要求

- 本次为解析增强与行为修复，不引入新 API/配置项，默认不需要更新 `README.md`
