# resReplace JSON Object 与 Whistle Pattern 兼容

## 背景

Whistle 允许把 `resReplace` 的替换表放进 Values/内嵌代码块，并用 JSON object 表达：

````text
```replace
{
  ".doupay.com\"": ".nodoupay.com\"",
  "\"inf.baohuaxia.com\"": "\"inf.nobaohuaxia.com\""
}
```

*/get_domains/v5 resReplace://{replace}
````

Bifrost 0.0.168 已支持 Values 引用，但 `reqReplace` / `resReplace` / `urlReplace`
只按 `old=new&old2=new2` 拆分；合法 JSON object 会退化成“删除整段 JSON 文本”的单条规则。
同时，`*/get_domains/v5` 会因为 host 末尾的 `*/` 被误判成 path wildcard，丢失 Whistle
普通 URL fragment 的 path 前缀与 query 匹配语义。

## 用户目标验证清单

### 必须实现

- `reqReplace://{value}`、`resReplace://{value}`、`urlReplace://{value}` 接受严格 JSON object。
- JSON string 的引号、反斜杠和 Unicode escape 按 JSON 语义还原。
- JSON object 的输入顺序稳定；重复 key 使用最后一个 value，但保留首次出现的位置。
- JSON value 为 string 时使用解码后的字符串；number/bool/null/array/object 使用紧凑 JSON 文本，行为确定且不会 panic。
- JSON object 的 key 仍可使用现有 `/regex/[g|i|gi]` 语法。
- `*/get_domains/v5` 按 host 单星 + 普通 path fragment 处理：匹配单标签 host 的目标 path、query 和子路径。
- `**/get_domains/v5` 可匹配包含 `.` 的任意 host，保持 Whistle 单星/双星 host 语义。

### 必须不破坏

- `old=new&old2=new2`、删除式 `old=`、URL percent decode 继续生效。
- 现有正则 replace 的 global / case-insensitive 语义不变。
- 非 object JSON、malformed JSON 和普通文本继续走旧 parser，不改变历史行为。
- `example.com/api/*` 等 Bifrost 既有 path wildcard 行为不变。
- CLI 真实代理、`bifrost-e2e` runner、Admin replay 请求/响应四条执行路径语义一致。

### 必须真实验证

- 使用用户提供的 `noEtag` 与 `replace` fenced Values，通过隔离代理请求
  `/get_domains/v5`，上游收到两个请求头，客户端收到三项域名替换。
- E2E 同时覆盖 query string，证明 `*/get_domains/v5` 不再静默失配。
- Admin replay 单元测试覆盖 JSON replace，避免“真实代理支持但 replay 不支持”。

## 产品语义

### JSON object 分派

1. 先尝试把完整 resolved value 解析为 JSON object。
2. 是合法 object：按 object entry 生成 replace pairs，不做 URL percent decode。
3. 不是 object 或 JSON 解析失败：完整回退到现有 `&` + `=` parser，并保留 URL decode。
4. object 中重复 key 更新旧 entry 的 value，避免同一个 key 被执行两次。

JSON value 转换：

| JSON 类型 | replacement |
| --- | --- |
| string | 解码后的字符串，不含 JSON 外层引号 |
| number / bool / null | 紧凑 JSON 文本 |
| array / object | 紧凑 JSON 文本 |

### `*/path` matcher

- host/path 分界线前的 `*` 是 host wildcard，不是 path wildcard。
- 只有 `/` 之后仍出现 `*` 时，才沿用 Bifrost 既有 `PathWildcard` 分支。
- 普通 wildcard + 明确 path 使用 URL fragment 边界：允许精确 path、`/subpath`、`?query`，拒绝 `pathSuffix`。
- 单星 host 不跨 `.`；需要任意带点域名时使用 `**/path`。

## 实现边界

- 新增 `bifrost-core::parse_json_replace_pairs`，所有 replace parser 共用 JSON object 分派。
- CLI、E2E、Admin replay 保留各自现有 regex 输出类型，仅复用 JSON pair 解析，避免 crate 依赖环。
- `reqHeaders://{value}` 继续接受 JSON/行格式 Values；`headerReplace://{value}` 继续接受 Whistle 的 `req.header:old=new` / `res.header:old=new` 文本语法。`headerReplace` 的 Value 不是 body replace JSON 映射，不能混用两种对象结构。
- 调整 `WildcardMatcher::detect_type` 与普通 path fragment 的 regex 尾部边界。
- 本轮不支持 Whistle 文档里的单引号 object 或 `key: value` 宽松格式；用户给出的严格 JSON 模式是交付边界。

## 验证方案

- 单元：`bifrost-core` JSON pair parser、Wildcard matcher；`bifrost-cli` replace parser；
  `bifrost-admin` request/replay body rules。
- E2E：`e2e-tests/rules/response_modify/res_replace_json_object.txt` +
  `e2e-tests/tests/test_body_replace.sh` 的 BR-12；通用 Rules fixture runner 将 host wildcard
  具体化为合法测试 host，并保留 pattern 自带 path。
- human_tests：`human_tests/rules-e2e-fixtures.md` 新增 Whistle JSON object 回归并逐条执行。
- 项目门禁：E2E 后执行 `rust-project-validate`；本地不运行 coverage，远端 CI 执行
  `bash scripts/ci/coverage-all.sh --json --gate`。

## Review/Fix/Test 闭环

### 第 1 轮

- Review JSON 转义、顺序、重复 key、regex key、malformed fallback。
- Review `*/path` / `**/path` 的 exact、query、subpath、false-positive。
- 复跑 core/cli/admin 单元与专项 E2E。

### 第 2 轮

- 复查四条执行路径是否全部调用共享 helper。
- 复查中英文文档、E2E fixture、human_tests 与实际行为一致。
- 复跑受影响测试和 workspace all-features 门禁。
