# Header 规则 JSON Object 兼容

## 背景

`reqHeaders://`、`resHeaders://`、`reqCookies://` 和 `trailers://` 过去主要支持两类写法：

- 单行或多行 header 行：`X-Env: ppe`、`X-Env=ppe`
- 值引用：`reqHeaders://{env_headers}`

但文档和测试示例里长期存在两种容易混淆的信号：

- 一些说明把 JSON object 说成可用于内联参数。
- 一些旧 human_tests 写成 `resHeaders://{X-Test: value}`，这既像对象，又不是合法 JSON，也不是合法值引用。

Coding Agent 在缺少精确约束时很容易选择 JSON map，因为它是表达多 header 的常见结构。对于用户给出的 `reqHeaders://{"x-tt-env":"ppe_next_agent_new","x-use-ppe":"1"}`，这个格式没有空格，语义也明确，应该被兼容处理。

## 问题

旧运行时解析遇到 `{"x":"1","y":"2"}` 时，会先按 header 行格式拆分。由于内容包含冒号，解析结果可能变成非法 header 名，例如 `{"x"`。当代理实际构造请求头时，底层 HeaderName 校验失败，可能表现为接口 502。

另外，语法校验过去把所有 `{...}` 都视作值引用，导致 malformed JSON header map 无法提前暴露。

## 方案

1. 对 header/cookie/trailer 类规则增加 JSON object 兼容。
   - 支持 `reqHeaders://{"X-Env":"ppe","X-Flag":"1"}`。
   - 支持括号包裹形式 `resHeaders://({"Cache-Control":"max-age=3600, public"})`，用于包含空格或逗号的值。
   - JSON value 仅接受 string、number、bool、null；null 转为空字符串。
   - array/object value 不作为 header/cookie 值应用。

2. 保留 `{name}` 值引用。
   - 只有内容看起来像 JSON object 时才按 JSON 处理：空对象、以双引号开头的 key、或包含冒号。
   - `{cn_nextagent_ppe_headers}` 仍按值引用路径处理。

3. 运行时防御 malformed JSON。
   - 如果内容看起来像 JSON object 但 JSON 解析失败，运行时返回空 header 列表，不再回退到旧的冒号拆分。
   - 语法校验负责给用户明确的 E021 错误。

4. 文档收敛。
   - 明确合法 JSON object 写法。
   - 把旧的 `resHeaders://{X: y}` 示例改为 `resHeaders://(X: y)` 或合法 JSON object。

## 测试方案

- 单元测试：
  - CLI header parser 支持 JSON object、括号 JSON、嵌套值跳过、malformed JSON 不回退。
  - Admin replay 请求/响应规则支持 JSON object，并对 malformed JSON 不产生非法 header。
  - Core parser 对合法 JSON object 放行，对 malformed / nested / empty JSON object 报 E021。

- E2E 测试：
  - `bifrost-e2e` 中新增 `req_headers_json_object`，验证代理发往 mock upstream 的请求包含 JSON object 中的三个 header。
  - `e2e-tests/rules/request_modify/headers.txt` 增加 JSON object 规则夹具，保持规则文件示例与运行时能力一致。

- human_tests：
  - `human_tests/rule-merge-headers.md` 增加 TC-RMH-07，验证用户给出的 NextOncall 风格 JSON object header rule 能被解析并进入最终请求头。

## Review/Fix/Test 闭环

- 第 1 轮：复核用户 9900 规则、文档误导源、运行时解析与语法校验；运行相关单元测试和 human_tests 用例。
- 第 2 轮：复核 malformed JSON 不回退、值引用不被误判、旧文档示例清理；复跑相关测试并执行项目级校验。

## 风险

- 兼容 JSON object 会让过去“非法但无效”的规则变为有效规则，这是本次目标行为。
- 对 malformed JSON 采取运行时 no-op，可以避免代理 502；用户侧错误由语法校验和管理端保存校验暴露。
