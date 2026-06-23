# 规则语法

Bifrost 通过简洁的规则配置来修改请求和响应。

## 语法结构

```txt
pattern operation [operations...] [filters...] [lineProps://...]
```

| 组成部分      | 是否必填 | 描述                                                          |
| :------------ | :------- | :------------------------------------------------------------ |
| **pattern**   | 是       | 匹配请求 URL 的表达式，详见 [pattern](./pattern.md)           |
| **operation** | 是       | 操作指令 `protocol://value`，详见 [operation](./operation.md) |
| **filters**   | 否       | 过滤条件，详见下文                                            |
| **lineProps** | 否       | 规则属性，详见下文                                            |

## Pattern 类型

Pattern 根据格式自动识别类型，优先级影响匹配顺序：

| 类型         | 格式示例                                    | 优先级 |
| :----------- | :------------------------------------------ | :----- |
| Domain       | `example.com` `example.com/api`             | 100+   |
| IP（精确）   | `192.168.1.1`                               | 95     |
| CIDR         | `192.168.0.0/16`                            | 70-78  |
| Regex        | `/pattern/` `/pattern/i`                    | 80     |
| PathWildcard | `^example.com/api/*` `^example.com/api/**`  | 60-70  |
| Wildcard     | `*.example.com` `$example.com` `example?.com` | 40-60  |

Domain 优先级以 100 为基准，显式协议（`http(s)://`）+5、显式端口 +10，因此带协议带端口的 Domain pattern 可达 100-115。CIDR 优先级为 `70 + prefix_len/4`（约 70-78），低于 Regex（80），因此一条宽泛的 CIDR（如 `/16`）可能排在 Regex 之后；只有精确 IP 才是 95。

取反匹配：所有类型的 pattern 都接受 `!` 前缀（如 `!*.example.com`），且能正常解析、规则状态为 Running；但实测在当前版本（0.0.96）取反匹配在运行时**不产生效果**——非匹配的请求并不会因取反而命中规则（无论是 `resHeaders://` 注入还是 `host://` 改目标，均未触发），因此暂不要依赖 `!` 前缀。完整的类型检测顺序、优先级与协议前缀注意事项见 [pattern.md](./pattern.md)。

## 高级配置

### 1. 组合配置

单条规则支持多个操作指令：

```txt
www.example.com file:///static-files cache://3600 resCors://*
chatgpt.com http3://
```

### 2. 位置调换

operation 和 pattern 可调换位置，便于批量配置：

```txt
proxy://127.0.0.1:8080 www.example.com api.example.com
```

### 3. 简写支持

`host:port[/path]` 格式自动识别为 `host://` 协议：

```txt
example.com 127.0.0.1:3000/api
# 等价于
example.com host://127.0.0.1:3000/api
```

当右侧已经出现 pattern，且目标值为不带协议的 `domain[/path]`、`domain:port[/path]`、`localhost[/path]`、IP/IPv6 带路径形式时，也会自动识别为 `host://`，下游协议按实际发起请求自动补偿：

```txt
gamingpop-boe.bifrost.local/manager gamingpop-boe.bifrost.local/manager
# 等价于
gamingpop-boe.bifrost.local/manager host://gamingpop-boe.bifrost.local/manager
```

### 4. 多行配置

**反斜杠续行**：行末 `\` 将下一行合并

```txt
example.com \
host://127.0.0.1 \
reqHeaders://{test=1}
```

**line 块语法**：块内换行自动转空格

```txt
line`
proxy://127.0.0.1:8080
www.example.com
api.example.com
includeFilter://m:GET
excludeFilter:///admin/
`
```

### 5. 过滤器

通过 `includeFilter://` 和 `excludeFilter://` 添加过滤条件：

```txt
example.com host://127.0.0.1 includeFilter://m:GET excludeFilter:///admin/
```

**过滤条件类型**：

| 前缀      | 说明       | 示例                                |
| :-------- | :--------- | :---------------------------------- |
| `m:`      | HTTP 方法  | `m:GET` `m:GET,POST,PUT`            |
| `s:`      | 状态码（当前运行时未生效，见下方说明） | `s:200` `s:200-299` `s:200,404,500` |
| `h:`      | 请求头存在或匹配（仅 `User-Agent`/`Accept`/`Host` 等标准头，见下方说明） | `h:User-Agent=curl` `h:Accept` |
| `reqH:`   | 请求头正则匹配（仅标准头，见下方说明） | `reqH:User-Agent=/curl/`            |
| `resH:`   | 响应头匹配（当前运行时未生效，见下方说明） | `resH:Content-Type=/json/`          |
| `i:`      | 客户端 IP  | `i:192.168.1.1` `i:192.168.0.0/16`  |
| `/path`   | 路径包含   | `/api`                              |
| `/regex/` | 路径正则   | `/^\/api\/v\d+/`                    |
| `domain.com/path` | URL host/path | `api.example.com/v1` |

> `b:` / `B:` body 过滤器当前只被 parser 接受，运行时 resolver 不读取 body 做过滤，**实测行为随写法不同**：文档使用的 `b:/regex/` 形式（如 `b:/error/`）写在 `includeFilter://b:/.../`  里会让规则**永远不命中**（fail-closed，实测无论有无 body 都返回 502），写在 `excludeFilter://b:/.../`  里则**永远不排除**（规则照常生效）；而不带斜杠的裸值形式（如 `b:foo`）会被直接丢弃忽略，规则照常命中。两种写法都无法真正按 body 内容过滤。请用 `bifrost search --req-body/--res-body` 做内容筛选，不要把 body 过滤写成生产规则依赖。

> `s:` 状态码过滤器当前只被 parser 接受，运行时 resolver 不会拿真实响应状态码去判定，因此**完全不生效**：写在 `includeFilter://s:200` 里永远不命中（即使响应确实是 200 也不会注入受控操作），写在 `excludeFilter://s:404` 里永远不排除（响应为 200 或 404 都不排除）。`s:200-299`、`s:200,404,500` 等区间/列表写法同样无效。需要按状态码筛选请改用 `bifrost search`，不要把状态码过滤写成生产规则依赖。

> `h:` / `reqH:` 请求头过滤器只对**标准请求头**生效，例如 `User-Agent`、`Accept`、`Host`。实测可用形式：`h:User-Agent=curl`（值为大小写敏感的子串匹配）、`h:Accept`（存在性匹配）、`reqH:User-Agent=/curl/`（值正则匹配）。`Content-Type` 与自定义头（如 `X-Custom-Header`、`X-Tag`）**不会**被这两个过滤器读取——即使请求确实带了该头、值也确实匹配，规则仍判定为不命中（命中 `host://` 类规则时表现为请求落空、返回 502）。这些头会照常转发到上游，只是不参与过滤判定。如需按 `Content-Type` 或自定义头筛选，请改用 `bifrost search`。

> `resH:` 响应头过滤器当前只被 parser 接受，运行时 resolver 不会读取真实响应头去判定，因此**完全不生效**：值正则形式 `resH:Content-Type=/json/`（乃至 `resH:Content-Type=/.*/`）永远不命中，存在性形式 `resH:Content-Type` 则被静默忽略、无论该头是否真实存在都按命中处理——两种写法都不反映真实响应头。需要按响应头筛选请改用 `bifrost search`，不要把响应头过滤写成生产规则依赖。

### 6. 规则属性

通过 `lineProps://` 设置规则属性：

| 属性        | 说明                 |
| :---------- | :------------------- |
| `important` | 提升优先级（+10000） |
| `disabled`  | 禁用规则             |

```txt
example.com host://127.0.0.1 lineProps://important
example.com host://127.0.0.1 lineProps://important,disabled
```

### 7. 变量替换

使用 `{varName}` 引用预定义变量，支持嵌套展开（最多 10 次迭代）：

```txt
example.com host://{myHost}
example.com resBody://{mockBody}
```

`${varName}` 格式为模板变量，不会被预处理展开。

### 8. 规则引用

在独立行写 `@规则名称` 可以引用个人私有规则；写 `@组名称/规则名称` 可以引用本机已缓存的组规则。Bifrost 会在解析前把被引用规则的内容原位展开：

```txt
@shared-headers # 行内说明
@team-alpha/shared-headers	# tab 后的行内说明也会被忽略
example.com host://127.0.0.1:3000
```

常见用法是把 `shared-headers` 设为 disabled，避免它 standalone 全局生效，再由启用的入口规则通过 `@shared-headers` 或 `@team-alpha/shared-headers` 复用。

个人私有规则不需要带组名称。组规则必须使用 `组名称/规则名称`，避免和同名私有规则冲突。规则引用只在独立 `@` 行生效；普通 `#` 注释里的 `@规则名称` 不会被当作引用。`@comment` 保持为注释指令，`commented-*` 这类真实规则名仍可正常引用。规则引用支持嵌套；运行时解析遇到缺失引用会跳过该引用行并继续解析后续规则，避免因为删除或尚未同步的引用规则导致整条入口规则失效。循环引用仍会让入口规则解析失败。Rules 编辑器的语法校验会对缺失引用返回 `E020`，并把对应 `@规则` 标红显示悬浮错误提示，便于用户及时修正。

### 9. 保存时语法检查

通过 Admin API、Group Rule API 或 CLI 新增/更新规则时，Bifrost 会在落盘前运行同一套语法检查服务：

- 规则有效时正常保存，并在响应中返回 `syntax.valid=true`、`rule_count`、`warnings` 和 `guidance`。
- 规则无效时默认拒绝保存；Admin API 返回 HTTP 422 且 `saved=false`，CLI 返回退出码 2。
- 响应中的 `syntax.errors[]` 会包含 `line`、`start_column`、`end_column`、`code`、`message` 和 `suggestion`，Agent 调用方应优先按第一条 error 修复后重试。
- 如果确实需要保存临时无效规则，可以显式传 `allow_invalid=true` 或 CLI `--allow-invalid`；此时会保存，但 `syntax.valid` 仍为 `false`，调用方不能把它当作已通过校验。

CLI 示例：

```bash
bifrost rule add bad --json --content "@missing-shared"
# exit code: 2，JSON 中 saved=false、syntax.errors[0].code=E020

bifrost rule add draft --json --allow-invalid --content "@missing-shared"
# exit code: 0，JSON 中 saved=true、syntax.valid=false
```

### 10. 规则分享链接

Bifrost 支持把一条个人规则编码到任意 HTTP/HTTPS URL 的特殊 query 中，用于把规则分享给其他本机 Bifrost 用户或自动化 Agent。协议 query 名固定为 `__bifrost_rule`，内容是 URL-safe base64 编码的 JSON payload，包含规则名称、规则内容、版本号、内容 hash、导入模式和独占启用范围。

第一版导入行为固定为 `mode=enable_exclusive`、`exclusive_scope=my_rules`：当 Bifrost 代理劫持到带 `__bifrost_rule` 的请求时，不会静默写入规则，而是重定向到本机 `/_bifrost/share/rule` 确认页面。确认页会展示规则名称、内容 hash、独占范围、返回目标和完整规则内容；只有用户点击 Apply Rule 后，Bifrost 才会把 payload 导入到个人规则列表，启用该规则，并禁用其他个人规则；不会创建、修改或禁用 Group 规则。确认完成后会跳回移除私有 query 的 clean URL，避免目标页面 JavaScript 读取到规则内容。

生成分享链接时，目标网站支持完整 `http://` / `https://` 地址，也支持 `a.com`、`example.com/path`、`localhost:3000` 这类裸域名输入；裸域名会默认规范成 `http://...`，确保普通 HTTP 代理请求能在不依赖 TLS 拦截的情况下看到并导入分享 query。显式输入 `https://...` 时会保持 HTTPS；显式非 HTTP(S) scheme 会被拒绝。

导入规则统一落在 `share/` 命名空间，避免覆盖用户已有的普通个人规则。例如 payload 名称为 `local-dev` 时，本地规则名为 `share/local-dev`。Bifrost 会在导入规则的描述中记录原始分享名和内容 hash：重复打开同一个分享链接会复用并覆盖同一条 `share/...` 规则，不会持续创建新规则；同名但内容不同的分享链接会创建 `share/规则名 2`、`share/规则名 3` 这类递增后缀的新规则。对已导入的 `share/...` 规则再次分享时，协议 payload 会自动剥掉 `share/` 前缀并优先使用描述中的原始分享名，避免把本地命名空间传播出去。

CLI 生成示例：

```bash
bifrost rule share local-dev https://example.com/app
bifrost rule share adhoc-debug https://example.com/app --content "api.example.com bp://127.0.0.1:3000"
```

## 注意事项

### 规则优先级

1. `lineProps://important` 规则优先匹配
2. 规则按数值优先级 `priority()` 排序：Domain（100+）> 精确 IP（95）> Regex（80）> PathWildcard（60-70）> Wildcard（40-60）；CIDR 为 70-78，因此宽泛 CIDR 可能排在 Regex 之后。不同 Pattern 类型的优先级互不相同，不存在「相同优先级按类型」的二级比较。
3. 仅当优先级相同（即同类型）时，规则才按从上到下的文件顺序匹配

### 调试技巧

1. **逐步验证**：从简单规则开始，逐步添加复杂条件
2. **日志查看**：使用 Bifrost Network 界面的 Overview 面板查看规则匹配情况
3. **临时禁用**：使用 `#` 注释或 `lineProps://disabled` 暂时禁用规则

### HTTPS 自动 TLS 解包边界

当全局 TLS 拦截关闭时，Bifrost 仍会为了执行必须读取 HTTPS 内层 HTTP 的规则而自动开启 TLS 解包，例如路径级 `http://` / `https://` 转发、`reqHeaders`、`resHeaders`、body 修改、脚本、mock 或状态码类规则。但这个自动解包有明确边界：

- 带具体域名/IP 作用域的路由会自动开启 TLS 解包，即使 matcher 前没有写 `https://`。这是因为 CONNECT 阶段只能看到 host，必须先解包才能让内层 path 和路由优先级继续生效。
- 仅用于 `proxy://` 选择下游代理的规则严格不会自动开启 TLS 解包；HTTPS `CONNECT` 会保持隧道透传并转发给下游代理。即使 proxy 规则的 matcher 带具体路径，只要目标协议只有下游代理转发，也不能因为这条规则解包。
- 规则驱动的自动解包必须有明确 host 作用域：Domain、IP/CIDR、带具体域名或 IP 片段的 Wildcard/PathWildcard 可以触发。
- 纯 regex 或纯 wildcard 范围过大，不能单独触发自动 TLS 解包，例如 `* resHeaders://...`、`*/api/* resHeaders://...`、`/api\/v\d+/ resHeaders://...`。
- 如果确实需要让宽泛匹配规则处理 HTTPS 明文，请先把 pattern 收窄到明确域名/IP，或显式配置 `tlsIntercept://` / 全局 TLS include。
- `passthrough://` 与 `http://` / `https://` / `host://` 等路由目标遵循同一套优先级 first-win 语义：如果更高优先级的具体路由已经选中，后续更宽泛的 passthrough 不会覆盖它。

示例：

```txt
# 会自动解包，因为 matcher 有明确域名作用域
example.com host://127.0.0.1:3000

# 会自动解包，因为具体域名路径路由需要读取 HTTPS 内层 path；
# matcher 不必写成 https://example.com/api
example.com/api https://10.0.0.10:8443

# 不会自动解包，只把隧道交给下游代理
example.com proxy://127.0.0.1:8080

# 会自动解包，因为响应修改规则绑定到明确域名
api.example.com resHeaders://X-Debug=1

# 不会自动解包，因为 pattern 没有明确 host 作用域
* resHeaders://X-Debug=1
/api\/v\d+/ resHeaders://X-Debug=1
```

### 上游 HTTP/3 规则

`http3://` 用于为命中的请求启用“代理到目标服务”的上游 HTTP/3 尝试，默认关闭。

```txt
chatgpt.com http3://
api.example.com h3://
```

- `h3://` 是 `http3://` 的别名
- 仅在代理自己能够读取 HTTP 请求时生效
- 对普通绝对 URI 代理请求可直接生效
- 对浏览器常见的 HTTPS `CONNECT` 流量，通常需要启用 TLS interception 后，代理才能在解密后的上游转发阶段尝试 H3
- 纯 `CONNECT` 透传隧道不会把上游 TCP 连接自动切换成 QUIC/H3

### 上游不安全 HTTPS 证书规则

`upstreamUnsafeSsl://true` 仅对命中的规则允许 Bifrost 到上游 HTTPS 服务时跳过证书校验。它用于某个测试环境、内网服务或自签名上游，不需要在启动整个代理时使用全局 `--unsafe-ssl`。

```txt
internal-api.example.test https://10.37.102.138:8080 upstreamUnsafeSsl://true
```

- 该规则只影响代理到上游的 HTTPS 连接，不会改变客户端到 Bifrost 的 TLS 信任关系。
- 没有命中该规则的请求仍按默认安全证书校验执行。
- 如果上游证书不可信且没有配置该规则，默认错误响应 body 会提示在匹配规则中追加 `upstreamUnsafeSsl://true`。
- 如果目标上游证书可信，应不要使用该规则；它是针对单个连接/规则的显式例外。

## 扩展阅读

- [规则协议手册](./rules/README.md)：按协议查看各能力说明与示例
