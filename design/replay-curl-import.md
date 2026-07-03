# Replay cURL 导入解析增强

> 状态：已实现 | 更新时间：2026-07-03

## 背景

Replay 页面在 URL 输入框粘贴 cURL 命令时会尝试自动导入 method / url / headers / body。实际使用中，从 Chrome DevTools 的 "Copy as cURL (bash)" 粘贴时存在两个高频解析问题：

- `-b/--cookie` 参数没有转换成 `Cookie` Header，导致回放请求丢失 Cookie。
- 当 cURL 使用 Bash 的 ANSI-C quoting（`$'...'`）表达 body 或 header（Chrome 处理 `$`、换行、制表符、Unicode 转义时会输出这种格式）时，旧解析器把 `$` 当作正文内容或错误保留反斜杠，body 内容与原请求不一致。

同时，旧解析函数内联在 Replay `RequestPanel` 组件内，难以覆盖单元测试，也无法在其他复用场景（例如未来把 Replay 组件迁移到独立 devtool 面板）里共享。

## 用户目标验证清单

### 必须实现

- 正确解析 Chrome 复制的 bash cURL：
  - 多行 `\` 续行、单双引号、`$'...'` ANSI-C quoting。
  - CMD `^\n`、PowerShell `` `\n `` 也归一化。
- 覆盖 Replay 导入需要的核心字段：
  - method：`-X/--request`；GET 存在 body 时自动推断为 POST；`-I/--head` → `HEAD`。
  - url：`--url`；positional URL 中最后一个 URL candidate。
  - headers：`-H/--header`（按首个 `:` 分割，name 走 RFC7230 token 校验，value CR/LF 折叠）。
  - cookies：`-b/--cookie` → `Cookie` Header；若已有 Cookie 则用 `; ` 追加。
  - basic auth：`-u/--user user:pass` → `Authorization: Basic base64(...)`（仅当未存在 `Authorization` 时写入）。
  - user-agent：`-A/--user-agent` → `User-Agent` Header。
  - body：`-d/--data/--data-raw/--data-binary/--data-urlencode/--data-ascii/--json`；未指定 Content-Type 时 `--json` 自动补 `application/json`。
  - `-G/--get`：把 `--data*` 内容追加到 URL query，不作为 body。
  - `raw_type`：根据 `Content-Type` 推断 json / xml / html / javascript / text。
- 解析逻辑下沉到 `web/src/utils/curl.ts`，提供 `parseCurl()` 与 `ParsedCurl` 接口。

### 必须不破坏

- 非 cURL / 未能推断 URL 的粘贴保持原 paste 行为，不拦截也不报错。
- 现有 `RequestPanel` 的 `onPaste` 事件语义不变。
- 未识别的 curl 选项跳过但不报错，导入流程不中断（如 `--compressed / --http2 / -k` 等）。
- Header value 中的 CR/LF 折叠为空格，避免 CRLF 注入。

### 必须真实验证

- 在 Chrome DevTools 复制真实请求（包含 Cookie、复杂 JSON body、`$'...'` header）粘贴到 Replay URL 输入框，method / url / headers / body 全部自动填充且内容与原请求一致。
- Playwright 单元测试覆盖上述所有解析分支。

## 非目标

- 不尝试 100% 复刻 curl 的全部特性：`multipart -F`、`@file` 读取本地文件、`--compressed`、`--http2` 等选项本次不支持。
- 不尝试跨主机变量替换或 shell 展开（`$VAR`、`$(cmd)`）。

## 现状分析

- 解析入口：`web/src/pages/Replay/components/RequestPanel.tsx::onPaste`（line 46 导入、line 209 调用 `parseCurl`）。
- 旧解析问题：
  - `-b/--cookie` 当作有值 option 跳过，未生成 Header。
  - Tokenization 只支持 `'` / `"` / `\`，未支持 `$'...'`。
  - `-u/--user` 未生成 Basic Authorization。
  - Header value 未做 CRLF 折叠，存在潜在注入。

## 产品语义

### `ParsedCurl` 接口

`web/src/utils/curl.ts::ParsedCurl`（line 260）：

```ts
export interface ParsedCurl {
  method: string;
  url: string;
  headers: Array<{ name: string; value: string }>;
  body?: string;
  raw_type?: 'json' | 'xml' | 'html' | 'javascript' | 'text';
}
```

`parseCurl(curl: string): ParsedCurl | null` 返回 `null` 表示未识别到有效 cURL。

### 分词规则

1. **续行归一**：Bash `\\\n`、CMD `^\n`、PowerShell `` `\n `` 统一变成单空格。
2. **单引号 `'...'`**：字面量，反斜杠不做处理。
3. **双引号 `"..."`**：按 bash 规则处理反斜杠，仅对 `\\`、`\"`、`\$`、``\` `` 生效（`curl.ts::183`），其它 `\x` 保留反斜杠。
4. **ANSI-C `$'...'`**（line 201）：
   - 前缀 `$` 不进入 token 内容。
   - 支持 `\n / \t / \r / \\ / \'` 等常见转义；`\xHH`、八进制、`\uXXXX`、`\UXXXXXXXX` 解码为对应字符。
5. **`--key=value`** 与 **`-XPOST`**、`-HHeader: v` 短选项形式展开。

### 语义解析

- `-X/--request` → method。
- `-I/--head` → method=HEAD。
- `--url` 或 positional 中最后一个 URL candidate → url。
- `-H/--header value` → 按首个 `:` 拆分，name 校验 RFC7230 token，value 折叠 CR/LF。
- `-b/--cookie` → `Cookie` Header（`; ` 追加），`-b @file` 因无法读取本地文件，跳过。
- `-u/--user user:pass` → `Authorization: Basic <base64>`（仅在未存在 `Authorization` 时）。
- `-A/--user-agent` → `User-Agent` Header。
- body 相关：`-d / --data / --data-raw / --data-binary / --data-urlencode / --data-ascii / --json` 收集到 `bodyParts`，按 curl 语义用 `&` 拼接；`--json` 在未显式 Content-Type 时补 `application/json`。
- `-G/--get`：body 追加到 URL query，不作为 body 导入。
- `raw_type`：根据 `Content-Type` 推断 json / xml / html / javascript / text。

## 技术细节

### 文件与入口

- `web/src/utils/curl.ts`（585 行）：`generateCurl(record)`（line 7，导出反向 helper）与 `parseCurl(curlCommand)`（line 351）。
- 关键选项集：`--json / --cookie / --cookie-jar / --user-agent / -b / -u / -X / -H / -A / -d / --data-* / -G`（line 380-393、470、499、517、525）。
- 双引号反斜杠处理：line 183。
- ANSI-C `$'...'`：line 201。
- 消费入口：`web/src/pages/Replay/components/RequestPanel.tsx::onPaste`（line 209）。

### 兼容与降级

- 未识别或与 Replay 执行不兼容的 curl 选项：解析器跳过，不影响其他字段。
- 解析失败（非 curl / 未能推断 URL）：返回 `null`，`RequestPanel` 保留原 paste 行为。
- `-b @cookiefile`：因浏览器沙箱无法读取本地文件，跳过不生成 Cookie。
- Header name 不合法：跳过并 warning，避免生成非法 Header。
- Header value CR/LF：折叠为单空格，避免 CRLF 注入。

### CLI + Web + Admin API

- CLI / Admin API 不涉及本次改动。
- Web：Replay `RequestPanel` 的 URL 输入 `onPaste` 事件仍是唯一入口；未来若加入“从 cURL 导入”菜单，可直接复用 `parseCurl`。

### Sync 边界

- 纯前端解析，不涉及后端或 sync。
- 解析结果只写入前端表单 state，用户 confirm 后才可能触发 replay 请求。

## Phase 1-4 拆分

### Phase 1：解析器下沉

- 把 `parseCurl` 从 `RequestPanel` 内联搬到 `web/src/utils/curl.ts`。
- 保留原有 `generateCurl` 反向工具函数（Replay → cURL）。
- 抽出常量集合：allowed options、value-required flags、raw_type mapping。

### Phase 2：分词能力升级

- 支持 `$'...'` ANSI-C quoting。
- 支持 CMD / PowerShell 续行归一。
- 双引号内非特殊转义保留反斜杠。

### Phase 3：语义扩展

- `-b/--cookie` → Cookie Header。
- `-u/--user` → Basic Authorization。
- `-A/--user-agent`、`-I/--head`、`-G/--get`、`--json` 支持。
- Header 校验（RFC7230 token、CR/LF 折叠）。

### Phase 4：测试与回归

- 新增 Playwright 单测（Node 环境直接调用 `parseCurl`）。
- 更新 Replay 手工回归清单。
- 在 Chrome DevTools 场景真实回归。

## 测试方案

### 单元测试（Playwright 直接调用 parseCurl）

`web/tests/ui/curl-parse.spec.ts`（61 行）：

| 用例 | 覆盖 |
| --- | --- |
| `test("解析 Chrome cURL 的 -b/--cookie 为 Cookie Header")` | `-b/--cookie` → Cookie |
| `test("解析 Chrome cURL 的 $'...' ANSI-C quoting，不保留前缀并正确处理 $ 字符")` | `$'...'` 前缀吞掉、`$` 保留 |
| `test("解析 $'...' 内常见转义（\\n、\\t、\\u、\\x）")` | 转义解码 |
| `test("双引号内非特殊转义保留反斜杠（bash 兼容）")` | 双引号反斜杠语义 |
| `test("解析 -u/--user 为 Basic Authorization Header")` | Basic Auth 生成 |
| `test("解析 -G/--get 将 data 追加到 URL query")` | `-G` body → query |
| `test("Header value 不允许 CRLF 注入（自动折叠为单行）")` | CR/LF 折叠 |
| `test("忽略非法 header name（不符合 RFC7230 token）")` | 非法 header 跳过 |

新增计划：

- `test("$'...' 内嵌 \\uXXXX 与 \\UXXXXXXXX 完整 Unicode 解码")`。
- `test("--data-urlencode 键值单独 URL 编码")`。
- `test("--json 未显式 Content-Type 时自动补 application/json")`。

### 回归验证（UI）

在 Replay 页粘贴 Chrome Copy as cURL：

- method / url / headers / body 自动填充。
- Cookie 不丢失。
- body 未被错误改写（尤其是 `$'{"a": "b\\nc"}'` 这类）。
- `Authorization` / `User-Agent` / `Content-Type` 正确写入。

## Review/Fix/Test 闭环

### 第 1 轮

- 目标复核：`ParsedCurl` 字段是否与 Replay 表单一一对应；Cookie / basic auth 生成不与既有 Header 冲突。
- 代码 review：`parseCurl` 内每个选项分支是否有 fallback；异常输入不 throw。
- 复测：`pnpm -C web test:ui` 全部通过；真实 Chrome cURL 场景 3 例。

### 第 2 轮

- 复核 `--json` 与 `Content-Type` 交互；`-G` 与已有 `?` query 追加。
- 检查 `git diff` 确认 `RequestPanel.tsx` 除 import 与调用外无其他改动。
- 复测：Playwright + Chrome 真机回归。

## 校验要求

- 前端 UI 测试：`pnpm -C web test:ui`
- 最终提交前执行：`rust-project-validate`

## 文档更新要求

- 本次为解析增强与行为修复，不引入新 API/配置项，默认不需要更新 `README.md`。
- `design/replay-curl-import.md`（本文档）与 `human_tests/replay.md`（若存在）同步更新真实回归条目。

## 风险与决策

- **未识别选项静默跳过 vs 报错**：为了不打断 Replay 导入体验，默认静默跳过；未来如果需要，可加 `console.warn` 或前端 toast 提示“部分 curl 选项已跳过”。
- **`-b @file` 无法读取本地文件**：浏览器沙箱限制，无法解决；文档明确指出并让用户手工在 Header 中补 Cookie。
- **ANSI-C `\uXXXX` / `\UXXXXXXXX` 边界**：JavaScript `String.fromCodePoint` 上界为 `0x10FFFF`，超出时按 curl 行为返回原始字节；解析器需保证不 throw。
- **双引号反斜杠语义**：与 bash `set -o` 一致，仅四种转义生效；如果有用户从 zsh / fish 复制得到不同格式，可能出现兼容差异，需在文档提示优先使用 bash 复制。
