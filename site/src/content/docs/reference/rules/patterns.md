---
title: "规则模式"
description: "Rules 内的匹配模式与优先级细节。"
editUrl: false
sidebar:
  label: "规则模式"
  order: 310
---

> 此页面由 `docs/rules/patterns.md` 自动同步生成。
# 匹配模式详解

本章详细介绍 Bifrost 规则的 URL 匹配模式语法。

---

## 匹配模式概述

匹配模式用于指定哪些请求应该应用规则。Bifrost 支持多种匹配方式：

| 类型 | 示例 | 说明 |
|------|------|------|
| 域名匹配 | `www.example.com` | 精确匹配域名 |
| 路径匹配 | `www.example.com/api` | 匹配域名和路径前缀 |
| 端口匹配 | `www.example.com:8080` | 匹配特定端口 |
| 端口通配匹配 | `www.example.com:8*` | 端口通配符匹配 |
| 前缀通配 | `*.example.com` | 单级子域名通配（不含 `.`） |
| 后缀通配 | `example.*` | 后缀通配（TLD 通配） |
| 多级通配 | `**.example.com` | 多级子域名通配（含 `.`） |
| 包含通配 | `*example*` | 域名包含匹配 |
| 混合通配 | `ex*le.com` | 域名中间通配 |
| 单字符通配 | `example?.com` | `?` 匹配任意单个字符 |
| 域名通配 `$` | `$example.com` | 匹配域名及其所有路径 |
| 路径通配 `^` | `^example.com/api/*` | 精确路径通配（`*`/`**`/`***`） |
| 正则匹配 | `/\/api\/v\d+/` | 正则表达式匹配 |
| IP 精确匹配 | `192.168.1.1` | 精确匹配 IP 地址 |
| CIDR 网段匹配 | `192.168.0.0/16` | 匹配 IP 网段 |
| 协议匹配 | `http://example.com` | 限定协议类型 |
| 协议通配 | `http*://example.com` | HTTP 和 HTTPS 通配 |

---

## 域名匹配

### 精确域名匹配

```bash
# 精确匹配 www.example.com
www.example.com host://127.0.0.1

# 不会匹配 api.example.com 或 example.com
```

### 裸域名匹配

```bash
# 匹配 example.com（不带子域名）
example.com host://127.0.0.1
```

> 域名匹配忽略大小写，`Example.COM` 和 `example.com` 等价。

### 测试用例

| 测试场景 | 规则模式 | 请求 URL | 是否匹配 |
|---------|---------|---------|---------|
| 精确域名 | `www.example.com` | `http://www.example.com/` | ✅ |
| 精确域名 | `www.example.com` | `http://api.example.com/` | ❌ |
| 精确域名 | `www.example.com` | `http://www.example.com/path` | ✅ |
| 大小写不敏感 | `Example.COM` | `http://example.com/path` | ✅ |

---

## 路径匹配

### 路径前缀匹配

路径匹配默认为**智能前缀匹配**：匹配指定路径本身、以该路径开头的子路径（`/` 分隔）、以及带查询参数的路径。同时防止路径段误匹配（如 `/api` 不会匹配 `/apitest`）。

```bash
# 匹配 /api, /api/, /api/users, /api?q=1 等
# 不匹配 /apitest（防止路径段误匹配）
www.example.com/api host://api-server.local
```

匹配规则细节：
- `example.com/api` 匹配 URL 路径 `/api`（精确）、`/api/...`（子路径）、`/api?...`（查询参数）
- 路径以 `/` 结尾时（如 `example.com/api/`），匹配该路径及所有子路径

### 路径前缀通配

使用 `*` 结尾的路径可匹配任意子路径：

```bash
# 匹配 /api/ 下的所有路径（DomainMatcher 的 PathPrefix 模式）
www.example.com/api/* host://api-server.local
```

### 测试用例

| 测试场景 | 规则模式 | 请求 URL | 是否匹配 |
|---------|---------|---------|---------|
| 路径前缀 | `example.com/api` | `http://example.com/api` | ✅ |
| 路径前缀 | `example.com/api` | `http://example.com/api/users` | ✅ |
| 路径前缀 | `example.com/api` | `http://example.com/api?q=1` | ✅ |
| 路径前缀 | `example.com/api` | `http://example.com/apitest` | ❌ |
| 路径前缀 | `example.com/api` | `http://example.com/v2/api` | ❌ |
| 路径前缀（/结尾） | `example.com/api/` | `http://example.com/api/` | ✅ |
| 路径前缀（/结尾） | `example.com/api/` | `http://example.com/api/users` | ✅ |
| 路径前缀通配 | `example.com/api/*` | `http://example.com/api/users` | ✅ |

---

## 端口匹配

### 特定端口匹配

```bash
# 只匹配 8080 端口
www.example.com:8080 host://127.0.0.1

# 不会匹配 80 或 443 端口
```

### 默认端口

```bash
# HTTP 默认 80 端口
www.example.com:80 host://127.0.0.1

# HTTPS 默认 443 端口
www.example.com:443 host://127.0.0.1
```

> 当请求 URL 没有显式端口时，HTTP 默认为 80，HTTPS 默认为 443。规则 `example.com:80` 可匹配 `http://example.com/`。

### 端口通配符

端口部分支持 `*` 通配符：

```bash
# 匹配所有以 8 开头的端口（80, 8080, 8888 等）
example.com:8* host://127.0.0.1

# 匹配所有以 80 结尾的端口
example.com:*80 host://127.0.0.1

# 中间通配（匹配 88, 808, 8888, 8008 等）
example.com:8*8 host://127.0.0.1

# 匹配所有端口
example.com:* host://127.0.0.1
```

> 端口通配符对默认端口也生效：`example.com:8*` 会匹配 `http://example.com/`（默认 80 端口，符合 `8*` 模式）。

### 测试用例

| 测试场景 | 规则模式 | 请求 URL | 是否匹配 |
|---------|---------|---------|---------|
| 特定端口 | `example.com:8080` | `http://example.com:8080/` | ✅ |
| 特定端口 | `example.com:8080` | `http://example.com/` | ❌ |
| 默认端口 | `example.com:80` | `http://example.com/` | ✅ |
| 默认端口 | `example.com:443` | `https://example.com/` | ✅ |
| 端口通配前缀 | `example.com:8*` | `http://example.com:8080/` | ✅ |
| 端口通配前缀 | `example.com:8*` | `http://example.com:9000/` | ❌ |
| 端口通配前缀 | `example.com:8*` | `http://example.com/` | ✅ |
| 端口通配前缀 | `example.com:8*` | `https://example.com/` | ❌ |
| 端口通配后缀 | `example.com:*80` | `http://example.com:8080/` | ✅ |
| 端口通配后缀 | `example.com:*80` | `http://example.com:8081/` | ❌ |
| 端口中间通配 | `example.com:8*8` | `http://example.com:808/` | ✅ |
| 端口中间通配 | `example.com:8*8` | `http://example.com:80/` | ❌ |
| 全端口通配 | `example.com:*` | `http://example.com:12345/` | ✅ |
| 全端口通配 | `example.com:*` | `https://example.com/` | ✅ |

---

## 通配符匹配

通配符匹配是 Bifrost 最灵活的匹配方式，支持多种通配类型。

### `*` 和 `**` 在域名中的区别

- `*`（单星号）：匹配不含 `.`、`/`、`?` 的内容（即单个域名层级）
- `**`（双星号）：匹配不含 `/`、`?` 的内容（可跨越 `.`，即多个域名层级）

### 前缀通配（Prefix）

以 `*` 开头，匹配域名前缀：

```bash
# 匹配 api.example.com, www.example.com
# 不匹配 a.b.example.com（* 不匹配 .）
*.example.com host://backend.local

# 匹配多级子域名
**.example.com host://backend.local
```

### 后缀通配（Suffix）

以 `*` 结尾，匹配域名后缀：

```bash
# 匹配 example.com, example.org, example.co.uk
example.* host://backend.local
```

### 包含通配（Contains）

`*` 同时出现在开头和结尾，匹配域名中包含指定内容：

```bash
# 匹配任何包含 "example" 的域名
# 如 www.example.com, myexample.org
*example* host://backend.local
```

### 混合通配（Mixed）

`*` 出现在域名中间位置：

```bash
# 匹配 example.com, exaaample.com 等
ex*le.com host://backend.local

# 匹配多级子域名组合
*.*.example.com host://backend.local
```

### 单字符通配符 `?`

`?` 匹配任意单个字符：

```bash
# 匹配 example1.com, exampleA.com 等
example?.com host://backend.local
```

### 域名通配 `$`

以 `$` 开头的模式匹配域名及其所有路径（仅限 HTTP/HTTPS 协议），可与 `*`/`**` 组合：

```bash
# 匹配 example.com 及其所有路径
$example.com host://backend.local

# 匹配 *.example.com（$* 中 * 不含 .）
$*.example.com host://backend.local

# 匹配 **.example.com（$** 中 ** 含 .）
$**.example.com host://backend.local
```

### 路径通配

路径中的 `*` 匹配任意字符（含 `/`）：

```bash
# 匹配 /api/ 下任何内容
www.example.com/api/* host://api-server.local

# 嵌套路径通配
example.com/api/*/details host://api-server.local
```

### 捕获组

每个 `*` 和 `**` 都会产生一个捕获组，可通过 `$1`、`$2` 等在操作中引用：

```bash
# $1 捕获子域名部分
*.example.com host://$1.backend.local

# $1 捕获后缀
example.* redirect://`https://example.com/$1`

# $1, $2 捕获多个通配部分
*.*.example.com host://$2.$1.backend.local

# 路径中的捕获
example.com/api/* redirect://`https://new.example.com/v2/$1`
```

### 协议与通配符组合

通配符模式可与协议前缀组合使用：

```bash
# 仅匹配 HTTP 协议的通配符
http://*.example.com host://backend.local

# 仅匹配 HTTPS 协议的通配符
https://*.example.com host://backend.local
```

### 测试用例

| 测试场景 | 规则模式 | 请求 URL | 是否匹配 |
|---------|---------|---------|---------|
| 前缀通配 | `*.example.com` | `http://api.example.com/` | ✅ |
| 前缀通配 | `*.example.com` | `http://a.b.example.com/` | ❌ |
| 多级前缀 | `**.example.com` | `http://a.b.example.com/` | ✅ |
| 多级前缀 | `**.example.com` | `http://a.b.c.example.com/` | ✅ |
| 多级前缀 | `**.example.com` | `http://example.com/` | ❌ |
| 后缀通配 | `example.*` | `http://example.com/` | ✅ |
| 后缀通配 | `example.*` | `http://example.org/` | ✅ |
| 后缀通配 | `example.*` | `http://example.co.uk/` | ✅ |
| 包含通配 | `*example*` | `http://www.example.com/` | ✅ |
| 包含通配 | `*example*` | `http://myexample.org/` | ✅ |
| 包含通配 | `*example*` | `http://test.com/` | ❌ |
| 混合通配 | `*.*.example.com` | `http://a.b.example.com/` | ✅ |
| 单字符通配 | `example?.com` | `http://example1.com/` | ✅ |
| 单字符通配 | `example?.com` | `http://exampleAB.com/` | ❌ |
| 域名通配 | `$example.com` | `http://example.com/api/test` | ✅ |
| 域名通配 | `$example.com` | `https://example.com/` | ✅ |
| 域名通配 | `$*.example.com` | `http://www.example.com/path` | ✅ |
| 域名通配 | `$*.example.com` | `http://a.b.example.com/path` | ❌ |
| 域名通配 | `$**.example.com` | `http://a.b.example.com/path` | ✅ |
| 路径通配 | `example.com/api/*` | `http://example.com/api/users` | ✅ |
| 嵌套路径通配 | `example.com/api/*/details` | `http://example.com/api/users/details` | ✅ |
| 协议+通配 | `http://*.example.com` | `http://www.example.com/` | ✅ |
| 协议+通配 | `https://*.example.com` | `https://api.example.com/` | ✅ |
| 捕获-前缀 | `*.example.com` | `http://www.example.com/` | ✅ (captures: `www`) |
| 捕获-后缀 | `example.*` | `http://example.com/` | ✅ (captures: `com`) |
| 捕获-多级 | `**.example.com` | `http://a.b.c.example.com/` | ✅ (captures: `a.b.c`) |
| 捕获-路径 | `example.com/*` | `http://example.com/api/users` | ✅ (captures: `api/users`) |

---

## 路径通配模式 `^`

以 `^` 前缀开启**精确路径通配模式**，提供三种粒度的路径通配。与普通通配符不同，`^` 模式下的 `*`/`**`/`***` 有严格的路径段匹配语义。

### `*` — 单段匹配

匹配不含 `/` 和 `?` 的单个路径段：

```bash
# 匹配 /api/users, /api/products
# 不匹配 /api/users/nested, /api/users?id=1
^example.com/api/* host://api-server.local
```

### `**` — 多段匹配

匹配不含 `?` 的多段路径（可跨 `/`）：

```bash
# 匹配 /api/users, /api/users/123/details
# 不匹配 /api/users?id=123
^example.com/api/** host://api-server.local
```

### `***` — 全匹配

匹配任意内容（含 `/` 和 `?`）：

```bash
# 匹配 /api/users, /api/users/123, /api/users?id=123
^example.com/api/*** host://api-server.local
```

### 组合使用

可在路径中放置多个通配段：

```bash
# 匹配 /v1/{any}/items/{any}/details
^api.example.com/v1/*/items/*/details host://backend.local
```

### 捕获组

每个 `*`/`**`/`***` 都生成一个捕获组，可在操作中通过 `$1`、`$2` 引用：

```bash
^example.com/*/action/* redirect://`https://new.example.com/$1/do/$2`
```

### 测试用例

| 测试场景 | 规则模式 | 请求 URL | 是否匹配 |
|---------|---------|---------|---------|
| 单段 `*` | `^example.com/api/*` | `http://example.com/api/users` | ✅ |
| 单段 `*` | `^example.com/api/*` | `http://example.com/api/users/nested` | ❌ |
| 单段 `*` | `^example.com/api/*` | `http://example.com/api/users?id=1` | ❌ |
| 多段 `**` | `^example.com/api/**` | `http://example.com/api/users/123/details` | ✅ |
| 多段 `**` | `^example.com/api/**` | `http://example.com/api/users?id=123` | ❌ |
| 全匹配 `***` | `^example.com/api/***` | `http://example.com/api/users?id=123` | ✅ |
| 全匹配 `***` | `^example.com/api/***` | `http://example.com/api/a/b?x=1` | ✅ |
| 多通配 | `^example.com/*/action/*` | `http://example.com/users/action/delete` | ✅ |

---

## 正则表达式匹配

### 基础正则

正则匹配对完整的 URL 进行匹配，以 `/` 开头和结尾：

```bash
# 匹配 URL 中包含 /api/v1, /api/v2, /api/v3 等
/\/api\/v\d+/ host://api-server.local
```

### 大小写不敏感

```bash
# i 标志表示忽略大小写
/\/API\/users/i host://users-service.local

# 匹配 /API/users, /api/users, /Api/Users 等
```

### 捕获组

```bash
# 使用捕获组
/\/api\/(v\d+)\/users/ host://users-$1.service.local

# 将 /api/v2/users 的请求转发到 users-v2.service.local
```

### 常用正则模式

| 模式 | 说明 | 示例匹配 |
|------|------|---------|
| `\d+` | 一个或多个数字 | `123`, `456` |
| `\w+` | 字母数字下划线 | `user_1`, `abc` |
| `[a-z]+` | 小写字母 | `abc`, `xyz` |
| `.*` | 任意字符 | 任何内容 |
| `[^/]+` | 非斜杠字符 | 路径段 |

### 测试用例

| 测试场景 | 规则模式 | 请求 URL | 是否匹配 |
|---------|---------|---------|---------|
| 版本号匹配 | `/\/api\/v\d+/` | `http://example.com/api/v1` | ✅ |
| 版本号匹配 | `/\/api\/v\d+/` | `http://example.com/api/latest` | ❌ |
| 大小写不敏感 | `/\/api/i` | `http://example.com/API` | ✅ |
| 大小写不敏感 | `/\/api/i` | `http://example.com/Api` | ✅ |

---

## 协议匹配

### HTTP/HTTPS 匹配

```bash
# 只匹配 HTTP 请求
http://www.example.com host://127.0.0.1

# 只匹配 HTTPS 请求
https://www.example.com host://127.0.0.1

# 匹配所有协议（默认）
www.example.com host://127.0.0.1
```

### WebSocket 匹配

```bash
# 匹配 WebSocket 请求
ws://www.example.com host://ws-server.local

# 匹配安全 WebSocket
wss://www.example.com host://wss-server.local
```

### Tunnel 匹配

```bash
# 匹配 CONNECT 隧道请求
tunnel://www.example.com host://tunnel-server.local
```

### 协议通配符

```bash
# http*:// — 匹配 HTTP 和 HTTPS
http*://www.example.com host://127.0.0.1

# ws*:// — 匹配 WS 和 WSS
ws*://www.example.com host://ws-backend.local

# // — 匹配所有协议（http, https, ws, wss, tunnel）
//www.example.com host://127.0.0.1
```

### 测试用例

| 测试场景 | 规则模式 | 请求 URL | 是否匹配 |
|---------|---------|---------|---------|
| HTTP 协议 | `http://example.com` | `http://example.com/` | ✅ |
| HTTP 协议 | `http://example.com` | `https://example.com/` | ❌ |
| 默认匹配 | `example.com` | `http://example.com/` | ✅ |
| 默认匹配 | `example.com` | `https://example.com/` | ✅ |
| HTTP 通配 | `http*://example.com` | `http://example.com/` | ✅ |
| HTTP 通配 | `http*://example.com` | `https://example.com/` | ✅ |
| HTTP 通配 | `http*://example.com` | `ws://example.com/` | ❌ |
| WS 通配 | `ws*://example.com` | `ws://example.com/` | ✅ |
| WS 通配 | `ws*://example.com` | `wss://example.com/` | ✅ |
| 全协议 | `//example.com` | `http://example.com/` | ✅ |
| 全协议 | `//example.com` | `wss://example.com/` | ✅ |
| 全协议 | `//example.com` | `tunnel://example.com/` | ✅ |
| Tunnel | `tunnel://example.com` | `tunnel://example.com/` | ✅ |
| Tunnel | `tunnel://example.com` | `http://example.com/` | ❌ |

---

## IP 地址匹配

### IPv4 精确匹配

```bash
# 匹配特定 IP
192.168.1.100 host://internal-server.local

# 带端口的 IP 同样支持
192.168.1.100:8080 host://internal-server.local
```

### CIDR 网段匹配

```bash
# 匹配 192.168.x.x 整个子网
192.168.0.0/16 host://lan-server.local

# 匹配 10.x.x.x 子网
10.0.0.0/8 host://internal.local

# 匹配 192.168.1.0 ~ 192.168.1.255
192.168.1.0/24 host://subnet.local
```

### IPv6 匹配

```bash
# 匹配 IPv6 地址
::1 host://localhost-server.local

# 带方括号格式也支持
[::1] host://localhost-server.local

# CIDR 网段
2001:db8::/32 host://ipv6-server.local
```

### 测试用例

| 测试场景 | 规则模式 | 请求 URL | 是否匹配 |
|---------|---------|---------|---------|
| IPv4 精确 | `192.168.1.1` | `http://192.168.1.1/` | ✅ |
| IPv4 精确 | `192.168.1.1` | `http://192.168.1.2/` | ❌ |
| CIDR /16 | `192.168.0.0/16` | `http://192.168.1.1/` | ✅ |
| CIDR /16 | `192.168.0.0/16` | `http://192.169.0.1/` | ❌ |
| CIDR /24 | `192.168.1.0/24` | `http://192.168.1.254/` | ✅ |
| CIDR /24 | `192.168.1.0/24` | `http://192.168.2.1/` | ❌ |
| CIDR /8 | `10.0.0.0/8` | `http://10.255.255.255/` | ✅ |
| IPv6 精确 | `::1` | `http://[::1]/` | ✅ |
| IPv6 CIDR | `2001:db8::/32` | `http://[2001:db8::1]/` | ✅ |
| IPv6 CIDR | `2001:db8::/32` | `http://[2001:db9::1]/` | ❌ |

---

## 特殊匹配

### 全局匹配

```bash
# 匹配所有请求
* host://proxy-server.local
```

### 排除匹配

使用 `!` 前缀对匹配结果取反：

```bash
# 排除特定域名（匹配所有非 www.example.com 的请求）
!www.example.com host://127.0.0.1

# 排除特定通配
!*.example.com host://127.0.0.1

# 排除路径通配
!^example.com/api/* host://127.0.0.1

# 排除特定 IP 段
!192.168.0.0/16 host://127.0.0.1

# 排除正则匹配
!/\.css$/ host://127.0.0.1
```

> `!` 可与所有匹配类型组合使用：域名、通配符、路径通配 `^`、正则、IP/CIDR 等。

> 注意：排除匹配的 `matches_host()` 始终返回 `false`，且不产生捕获组。

---

## 匹配优先级

当多个规则都匹配时，按以下优先级顺序应用（数值越高越优先）：

| 匹配类型 | 优先级 | 说明 |
|---------|-------|------|
| 域名 + 协议 + 端口 + 路径 | 130 | 最高优先级（如 `https://example.com:8443/api/users`） |
| 域名 + 端口 + 路径 | 125 | |
| 域名 + 协议 + 路径 | 120 | |
| 域名 + 路径（精确匹配） | 115 | 如 `example.com/api/users` |
| 域名 + 协议 + 端口 | 115 | |
| 域名 + 端口 | 110 | 如 `example.com:8080` |
| 域名 + 路径（前缀通配） | 110 | 如 `example.com/api/*` |
| 域名 + 协议 | 105 | 如 `https://example.com` |
| 精确域名 | 100 | 如 `example.com` |
| IP 精确匹配 | 95 | 如 `192.168.1.1` |
| 正则匹配 | 80 | 如 `/\/api\/v\d+/` |
| CIDR 网段 | 70-78 | 前缀越长优先级越高（/8=72, /16=74, /24=76, /32=78） |
| `^` 单段通配 `*` | 70 | 如 `^example.com/api/*` |
| `^` 多段通配 `**` | 65 | 如 `^example.com/api/**` |
| 路径通配（Wildcard） | 60 | 如 `example.com/api/*`（走通配符匹配器） |
| `^` 全匹配通配 `***` | 60 | 如 `^example.com/api/***` |
| 前缀/后缀通配 | 55 | 如 `*.example.com`、`example.*` |
| `$` 域名通配 | 50 | 如 `$example.com` |
| 混合通配 | 45 | 如 `ex*le.com` |
| 包含通配 | 40 | 如 `*example*` |

### 优先级示例

```bash
*example*       host://catch-all.local         # 优先级 40
*.example.com   host://general-backend.local   # 优先级 55
www.example.com host://www-backend.local       # 优先级 100
www.example.com/api host://api-backend.local   # 优先级 115

# 请求 www.example.com/api/users 会匹配到 api-backend.local（优先级最高）
```

---

## 匹配类型判定流程

Bifrost 按以下顺序判定匹配模式的类型：

1. 以 `/` 开头且以 `/` 或 `/i` 结尾 → **正则匹配**
2. 以 `^`（或 `!^`）开头 → **`^` 路径通配匹配**
3. 是 IPv4 地址、IPv6 地址或 CIDR 格式 → **IP 匹配**
4. 包含 `*` 或 `?`（排除纯端口通配的情况），或以 `$` 开头 → **通配符匹配**
5. 其他情况 → **域名匹配**

> 注意：纯端口通配（如 `example.com:8*`，域名部分不含 `*`/`?`）会走**域名匹配**而非通配符匹配。

---

## 测试用例汇总

| 测试场景 | 规则模式 | 请求 URL | 预期 |
|---------|---------|---------|------|
| 精确域名 | `test.com` | `http://test.com/` | 匹配 |
| 路径前缀 | `test.com/api` | `http://test.com/api/users` | 匹配 |
| 路径前缀防误匹配 | `test.com/api` | `http://test.com/apitest` | 不匹配 |
| 前缀通配 | `*.test.com` | `http://api.test.com/` | 匹配 |
| 前缀通配 | `*.test.com` | `http://a.b.test.com/` | 不匹配 |
| 多级通配 | `**.test.com` | `http://a.b.test.com/` | 匹配 |
| 后缀通配 | `test.*` | `http://test.com/` | 匹配 |
| 包含通配 | `*test*` | `http://mytest.org/` | 匹配 |
| 单字符通配 | `test?.com` | `http://test1.com/` | 匹配 |
| 端口匹配 | `test.com:8080` | `http://test.com:8080/` | 匹配 |
| 端口通配 | `test.com:8*` | `http://test.com:8080/` | 匹配 |
| CIDR 网段 | `192.168.0.0/16` | `http://192.168.1.1/` | 匹配 |
| 正则匹配 | `/\/api\/v\d+/` | `http://test.com/api/v1` | 匹配 |
| 大小写不敏感 | `/\/api/i` | `http://test.com/API` | 匹配 |
| HTTP 协议 | `http://test.com` | `https://test.com/` | 不匹配 |
| HTTP 通配 | `http*://test.com` | `https://test.com/` | 匹配 |
| WS 通配 | `ws*://test.com` | `wss://test.com/` | 匹配 |
| 全协议 | `//test.com` | `wss://test.com/` | 匹配 |
| `^` 单段 | `^test.com/api/*` | `http://test.com/api/users` | 匹配 |
| `^` 多段 | `^test.com/api/**` | `http://test.com/api/a/b` | 匹配 |
| `^` 全匹配 | `^test.com/api/***` | `http://test.com/api/a?q=1` | 匹配 |
| 域名通配 `$` | `$test.com` | `http://test.com/any/path` | 匹配 |
| 排除匹配 | `!test.com` | `http://other.com/` | 匹配 |
| 全局匹配 | `*` | 任何 URL | 匹配 |
