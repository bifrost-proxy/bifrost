# Header 规则 `&` 分隔兼容

## 背景

用户配置：

```txt
reqHeaders://(x-tt-env=ppe_doubao_connect_lark&x-flow-env=ppe_doubao_connect_lark&x-use-ppe=1)
```

期望写入三个独立请求头，但旧实现把第一个 `=` 之后的全部内容都作为
`x-tt-env` 的值。旧文档也把这一运行时缺陷记录成既定语义，导致规则写法与用户
熟悉的 Whistle 风格不一致。

## 用户目标验证清单

### 必须实现

- `reqHeaders` 单行内联值支持用 `&` 分隔多个 Header。
- `resHeaders` 使用同一 Header 语法，避免请求/响应规则行为不一致。
- 普通代理、HTTPS tunnel、Replay 与 E2E resolver 使用同一解析契约。

### 必须不破坏

- 单 Header、逗号分隔 Header、JSON 对象和多行 Values 继续可用。
- JSON 或多行 Value 内 Header 值中的字面 `&` 不被拆分。
- `reqCookies` / `resCookies` 不改变分隔语义。
- malformed JSON Header 对象不能回退成部分生效的非法 Header。

### 必须真实验证

- 用与用户截图等价的规则经真实代理访问本地 mock upstream。
- upstream 分别收到 `x-tt-env`、`x-flow-env`、`x-use-ppe`，不存在把后两项拼进
  `x-tt-env` 值的情况。
- 单元测试覆盖分隔、保留字面 `&`、单 Header、非法片段和 malformed JSON。

## 设计

在 `bifrost-core` 提供共享 Header parser：

- 先剥离可选的最外层小括号。
- 先识别并解析 JSON 对象，JSON 字符串值原样保留。
- 多行内容仅按换行拆分，每行解析第一个 `:` 或 `=`；因此
  `X-Query: a=1&b=2` 保持一个 Header。
- 单行 `reqHeaders` / `resHeaders` 内容按 `&` 或逗号拆分，每段解析第一个
  `:` 或 `=`。
- Cookie、CORS、URL 参数继续走各自现有 parser，不共享 Header 的 `&` 分隔规则。

## 测试方案

- core 单元测试：`rule::header_value::tests`。
- Admin/CLI 针对性测试：验证旧 JSON、多行、注释和 Replay parser 兼容。
- E2E runner：`req_headers_ampersand_separated` 真实启动代理和 mock upstream。
- 规则夹具：`e2e-tests/rules/request_modify/headers.txt` 的 R-05。
- human test：`human_tests/rule-merge-headers.md` 的 TC-RMH-08。

## Coverage 90% 门禁

生产 Rust 变更后，本地执行不带 test filter 的 `make coverage-changed`；远端 CI 继续
以 `bash scripts/ci/coverage-all.sh --json --gate` 作为最终 crate/workspace 门禁。
