# Header、Cookie 与 Trailer 规则 `&` 分隔兼容

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
- `reqCookies://(sessionid=xxx&a=c&b=...)`、`resCookies://(sid=xxx&theme=dark)` 和
  `trailers://(X-Trace=abc&X-Checksum=xyz)` 使用同一安全的 `&` 多键语法。
- 普通代理、HTTPS tunnel、Replay 与 E2E resolver 使用同一解析契约。

### 必须不破坏

- 单 Header、逗号分隔 Header、JSON 对象和多行 Values 继续可用。
- JSON、多行 Value 或单行引用 Value 内 Header 值中的字面 `&` 不被拆分。
- `${url}` / `${reqHeaders.*}` 等模板展开结果中的 `&` 保持为 Header 值数据，不能注入额外 Header。
- `resCookies` 的 JSON 属性对象（`path`、`domain`、`maxAge`、`secure`、`httpOnly`、
  `sameSite`）继续按结构化 Cookie 解析，不被降级为简单键值。
- Cookie/Trailer 的 JSON、多行、Values 引用、文件和远程内容中的字面 `&` 不被拆分。
- Cookie/Trailer 模板展开生成的 `&` 保持为数据，不能注入额外字段。
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
- 多行及引用内容仅按换行拆分，每行解析最先出现的 `:` 或 `=`；因此单行引用
  Value `X-Query: a=1&b=2` 也保持一个 Header。
- 只有 `ValueSource::Inline`、`InlineParams` 和 `ParenContent` 单行内容按 `&` 或逗号
  拆分；文件、远程 URL 和 `{name}` 引用内容保留字面 `&`。
- 规则 parser 的单行 Values 展开会显式跳过 Header、Cookie 与 Trailer 协议，确保
  `{name}` 以 `ValueSource::ValueRef` 进入 resolver；否则来源信息会在共享 parser 前丢失。
- 对 `reqHeaders` / `resHeaders`，`ResolvedRule` 先按规则作者写下的源文本解析 Header
  边界，再逐个展开 Header 名和值中的模板；不再对已经展开的整串文本重新按 `&` 拆分。
  因此 URL 查询串和复制的请求 Header 即使含 `&X-Injected=...` 也只属于原 Header 值。
- `ResolvedRule` 暴露共享的 source-aware key/value pairs，Header、Cookie 与 Trailer 的
  运行时消费者统一使用；CORS、URL 参数仍走各自 parser。
- `resCookies` 若检测到 JSON 对象值为属性对象，则不生成简单 pairs，而是保留给专用
  response-cookie parser，以继续输出 `Path`、`Max-Age`、`HttpOnly` 等属性。

## 测试方案

- core 单元测试：`rule::header_value::tests`。
- Admin/CLI 针对性测试：验证旧 JSON、多行、注释和 Replay parser 兼容。
- E2E runner：`req_headers_ampersand_separated` 真实启动代理和 mock upstream。
- E2E runner：`req_headers_template_literal_ampersand` 验证 `${url}` 查询串和
  `${reqHeaders.*}` 中的 `&` 不会改变 Header 边界。
- E2E runner：`req_headers_json_scalar_template` 验证 JSON map 的无引号标量模板在展开后
  再解析，并确保真实 upstream 收到对应 Header。
- E2E runner：`req_cookies_ampersand_separated`、
  `req_cookies_value_ref_literal_ampersand`、`res_cookies_ampersand_separated`、
  `trailers_ampersand_separated` 覆盖三个新增场景和引用边界。
- 规则夹具：`e2e-tests/rules/request_modify/headers.txt` 的 R-05。
- 规则夹具：请求/响应 Cookie 与 Trailer 均覆盖直接 `&` 和 Values 字面 `&` 边界；
  请求 Cookie 断言使用必需的 Python 3，在可选 `jq` 缺失时仍校验实际值。
- human test：`human_tests/rule-merge-headers.md` 的 TC-RMH-08、TC-RMH-09、TC-RMH-10；
  TC-RMH-09 同步包含 Dynamic Island Chromium 自动化与 Chrome 人工验证步骤。

## 依赖与影响面

- 依赖 `bifrost-core::ValueSource` 区分规则原始值来源；不新增第三方 crate。
- 共享 parser 的调用面包括 Admin 请求规则、Replay 请求/响应规则、CLI resolver 与
  `bifrost-e2e` adapter，必须同步传递 `ResolvedRule.rule.value_source`。
- Admin、CLI、Replay 和 E2E resolver 都消费同一组已解析 pairs，避免各层重复解析后
  出现行为分叉。

## Review/Fix/Test 计划

### 第 1 轮

- 对照用户配置与全部 MR comments，检查 parser 是否只对真实内联来源拆分 `&`。
- 检查 `git status --short`、`git diff` 和新增文件，复跑 core、Admin、CLI 相关测试，
  以及请求/响应 Header E2E。
- 修复文档、human_tests、规则夹具中与真实执行不一致的内容。

### 第 2 轮

- 基于第 1 轮最新 diff 复查引用 Value、JSON、多行、Referer URL、Cookie 不变性与清理边界。
- 复跑受影响测试、human_tests 与 `make coverage-changed`，确认没有遗漏的文档镜像和
  CI shell 收录问题。

## rust-project-validate 计划

完成 E2E 与 human_tests 后依次执行 workspace/desktop fmt、all-target/all-feature Clippy、
相关 crate 测试、all-target/all-feature build，以及清除桌面继承环境变量后的
`cargo test --workspace --all-features`。

## 文档更新清单

- `README.md`
- `docs/rules/request-modification.md`
- `docs/rules/response-modification.md`
- `site/src/content/docs/reference/rules/request-modification.md`
- `site/src/content/docs/reference/rules/response-modification.md`
- `docs-en/operation.md`、`docs-en/pattern.md` 与英文 request/response rule 文档及 site 镜像
- `human_tests/rule-merge-headers.md`
- `human_tests/readme.md`
- `e2e-tests/rules/COVERAGE.md`

## Coverage 90% 门禁

生产 Rust 变更后，本地执行不带 test filter 的 `make coverage-changed`；远端 CI 继续
以 `bash scripts/ci/coverage-all.sh --json --gate` 作为最终 crate/workspace 门禁。
