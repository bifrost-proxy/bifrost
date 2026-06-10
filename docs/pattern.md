# 匹配器

`pattern` 是 Bifrost 规则中的第一部分，用于匹配请求 URL。

## Pattern 类型

Bifrost 支持五种 Pattern 类型，系统会按以下顺序自动检测：

| 类型         | 触发条件                                    | 优先级范围 |
| ------------ | ------------------------------------------- | ---------- |
| Regex        | 以 `/` 开头且以 `/`、`/i`、`/u`、`/iu` 结尾 | 80         |
| PathWildcard | 以 `^` 开头                                 | 60-70      |
| IP           | 符合 IPv4/IPv6/CIDR 格式                    | 70-95      |
| Wildcard     | 包含 `*`、`?` 或以 `$` 开头                 | 40-60      |
| Domain       | 默认类型                                    | 100-133+   |

所有类型均支持 `!` 前缀表示否定匹配。

## Domain 匹配

精确匹配域名、端口和路径。

### 格式

```txt
[protocol://]domain[:port][/path]
```

### 协议匹配

| 格式        | 说明               |
| ----------- | ------------------ |
| `http://`   | 仅匹配 HTTP        |
| `https://`  | 仅匹配 HTTPS       |
| `ws://`     | 仅匹配 WebSocket   |
| `wss://`    | 仅匹配 WSS         |
| `tunnel://` | 仅匹配隧道代理     |
| `http*://`  | 匹配 HTTP 和 HTTPS |
| `ws*://`    | 匹配 WS 和 WSS     |
| `//`        | 匹配所有协议       |
| 无协议      | 匹配所有协议       |

### 端口通配符

| 格式  | 示例              | 说明                       |
| ----- | ----------------- | -------------------------- |
| `*`   | `example.com:*`   | 匹配任意端口（必须有端口） |
| `8*`  | `example.com:8*`  | 匹配以 8 开头的端口        |
| `*80` | `example.com:*80` | 匹配以 80 结尾的端口       |
| `8*8` | `example.com:8*8` | 匹配 88、808、8008 等      |

### 路径匹配

| 格式      | 说明                 |
| --------- | -------------------- |
| `/path`   | 匹配该路径及其子路径 |
| `/path/*` | 匹配该路径前缀       |

**示例**：

```txt
example.com/api              # 匹配 /api、/api/users、/api?q=1
example.com/api/*            # 匹配 /api/ 开头的所有路径
https://example.com:8443/api # 完整匹配
```

> ℹ️ 含 `*` 的 `example.com/api/*` 在类型检测时会被归类为 **Wildcard**（优先级约 60），而非 Domain。只有裸 `/path` 前缀形式（如 `example.com/api`）才保持为 Domain 模式。这里仅描述其运行时匹配行为（命中 `/api/users`、`/api/`，但不命中裸 `/api`）。

## IP 匹配

匹配 IP 地址或 CIDR 网段。

| 格式 | 示例                           | 说明     |
| ---- | ------------------------------ | -------- |
| IPv4 | `192.168.1.1`                  | 精确 IP  |
| IPv6 | `::1`、`2001:db8::1`           | 精确 IP  |
| CIDR | `192.168.0.0/16`、`10.0.0.0/8` | 网段匹配 |

## Wildcard 匹配

域名通配符匹配，自动识别包含 `*`、`?` 或以 `$` 开头的 pattern。

### 通配符

| 通配符 | 说明                 | 示例                                                             |
| ------ | -------------------- | ---------------------------------------------------------------- |
| `*`    | 匹配单级（不含 `.`） | `*.example.com` 匹配 `www.example.com`，不匹配 `a.b.example.com` |
| `**`   | 匹配多级（含 `.`）   | `**.example.com` 匹配 `a.b.c.example.com`                        |
| `?`    | 匹配单个字符         | `example?.com` 匹配 `example1.com`                               |
| `$`    | 域名通配符前缀       | `$example.com` 匹配 `http(s)://example.com` 及其路径             |

### 示例

```txt
*.example.com                # 单级子域名
**.example.com               # 多级子域名
*example*                    # 包含 example
example.*/api/*              # 域名后缀 + 路径
$*.example.com               # 域名通配符，匹配单级子域名的所有路径
$**.example.com              # 域名通配符，匹配多级子域名的所有路径
```

> ⚠️ **不要在 Wildcard 模式前加协议前缀**。协议前缀（`http://`、`https://`、`http*://`、`ws*://`、`//`）只对 Domain 和 PathWildcard（`^` 前缀）模式可靠；与 Wildcard 模式组合会出现以下错误行为：
>
> | 写法 | 实际行为 |
> | ---- | -------- |
> | `*.example.com`（裸写，推荐） | 正确匹配单级子域名（HTTP + HTTPS） |
> | `http://*.example.com` / `https://*.example.com` | 能匹配，但单星 `*` 会跨越 `.`，等价于 `**`（丢失单级限制） |
> | `http*://*.example.com` / `ws*://*.example.com` | **永不匹配**（静默失效） |
> | `//*.example.com` | **匹配所有 host**（过度匹配，等价于无差别命中，存在误伤风险） |
>
> 需要按协议限定时：裸 Wildcard 已覆盖 HTTP/HTTPS；要限定单一协议或匹配 WS/WSS，请改用 Domain 模式（如 `http*://api.example.com`）或 PathWildcard（如 `^http*://example.com/api/*`）。

## PathWildcard 匹配（`^` 前缀）

路径通配符匹配，以 `^` 开头显式声明。支持三种级别的通配符：

| 通配符 | 正则等价 | 说明                        |
| ------ | -------- | --------------------------- |
| `*`    | `[^?/]*` | 单级路径（不含 `/` 和 `?`） |
| `**`   | `[^?]*`  | 多级路径（不含 `?`）        |
| `***`  | `.*`     | 任意字符（含 `/` 和 `?`）   |

### 示例

```txt
^example.com/api/*/info      # 匹配 /api/users/info，不匹配 /api/a/b/info
^example.com/api/**          # 匹配 /api/a/b/c，不匹配 /api/a?q=1
^example.com/api/***         # 匹配任意内容，包括 /api/a/b?q=1
```

## Regex 匹配

正则表达式匹配，语法与 JavaScript 正则兼容。

### 格式

```txt
/pattern/[flags]
```

### Flags

| Flag | 说明         |
| ---- | ------------ |
| `i`  | 大小写不敏感 |
| `u`  | Unicode 模式 |

### 示例

```txt
/\.example\.com/             # 匹配 .example.com
/api\/v\d+/i                 # 大小写不敏感匹配
/测试/u                      # Unicode 匹配
```

## 否定匹配

所有类型均支持 `!` 前缀表示否定：

```txt
!example.com                 # 排除 example.com
!192.168.0.0/16              # 排除内网 IP
!*.internal.com              # 排除 internal.com 子域名
!/\.test\./                  # 排除包含 .test. 的 URL
```

## 子匹配传值

Wildcard、PathWildcard 和 Regex 支持捕获组，通过 `$1`-`$9` 引用：

### Wildcard 传值

```txt
*.example.com file:///data/$1        # $1 = 子域名
example.*/api/* file:///mock/$1/$2   # $1 = TLD, $2 = 路径
```

### PathWildcard 传值

```txt
^example.com/api/*/info file:///mock/$1.json   # $1 = 单级路径
^example.com/*/*       file:///data/$1/$2      # $1, $2 = 各级路径
```

### Regex 传值

```txt
/api\/v(\d+)\/users\/(\d+)/ reqHeaders://X-Version=$1 reqHeaders://X-ID=$2
```

> `reqHeaders://` 不以 `&` 拆分多个 header：写成 `reqHeaders://X-Version=$1&X-ID=$2` 只会设置单个请求头 `X-Version: <值>&X-ID=<值>`（`&X-ID=...` 成为值的一部分）。要设置多个 header，请用多个独立的 `reqHeaders://` 操作（如上例），详见 operation.md。

## 优先级

规则按优先级从高到低匹配：

| 类型           | 优先级 | 说明                   |
| -------------- | ------ | ---------------------- |
| Domain（完整） | ≥130   | 100 基础 +5 协议 +10 端口 +（15+路径段数）精确路径 |
| Domain（基础） | 100    | 仅域名                 |
| IP（精确）     | 95     | 单个 IP                |
| Regex          | 80     | 正则表达式             |
| IP（CIDR）     | 70-78  | 按前缀长度             |
| PathWildcard   | 60-70  | `*` > `**` > `***`     |
| Wildcard       | 40-60  | 按类型                 |

## 配置示例

### 域名匹配

```txt
example.com proxy://127.0.0.1:8080
http*://api.example.com cache://3600
example.com:8*/api/* file:///mock
```

> 上例第三行的 `example.com:8*/api/*` 是裸 Wildcard，不要写成 `//example.com:8*/api/*`：`//` 前缀与 Wildcard 组合会匹配所有 host（见上文 Wildcard 协议前缀说明）。

### IP 匹配

```txt
192.168.1.1 proxy://127.0.0.1:3000
10.0.0.0/8 reqHeaders://X-Matched-Private=1
!192.168.0.0/16 proxy://external
```

### 通配符匹配

```txt
*.example.com proxy://127.0.0.1:8080
**.cdn.example.com cache://86400
$api.example.com file:///mock
```

### 路径通配符匹配

```txt
^example.com/api/*/info file:///mock/$1.json
^example.com/static/** cache://3600
^api.example.com/v*/users/*** reqHeaders://X-Api-Pattern=path-wildcard
```

### 正则匹配

```txt
/^https?:\/\/.*\.example\.com/ proxy://127.0.0.1:8080
/\/api\/v(\d+)\// reqHeaders://X-Version=$1
/\.(jpg|png|gif)$/i cache://86400
```

## 扩展阅读

- [规则语法文档](./rule.md)：了解完整的规则语法结构
- [操作指令文档](./operation.md)：学习如何配置操作指令
- [规则协议手册](./rules/README.md)：按协议查看各能力说明与示例
