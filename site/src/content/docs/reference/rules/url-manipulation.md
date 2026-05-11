---
title: "URL 改写"
description: "URL、路径与查询参数的改写能力。"
editUrl: false
sidebar:
  label: "URL 改写"
  order: 240
---

> 此页面由 `docs/rules/url-manipulation.md` 自动同步生成。
# URL 操作规则

本章介绍动态修改请求 URL 的规则。

---

## urlParams

添加或修改 URL 查询参数。

### 语法

```
pattern urlParams://key=value              # 内联格式（单个参数）
pattern urlParams://(key: value)           # 小括号格式（可包含空格）
pattern urlParams://{varName}              # 引用内嵌值（推荐）
```

> ⚠️ **注意**：
>
> 1. `{name}` 是引用内嵌值的语法，不是直接定义 JSON！
> 2. 小括号内容会作为一个整体解析，可以包含空格；多个参数建议使用块变量

### 基础示例

```bash
# 内联格式添加单个参数
www.example.com urlParams://debug=true

# 小括号格式
www.example.com urlParams://(version: 2)

# 引用内嵌值（多个参数，推荐）
www.example.com urlParams://{my-params}
```

内嵌值定义：

````
``` my-params
version: 2
lang: zh
```
````

### 使用场景

```bash
# API 版本控制
www.example.com/api urlParams://api_version=v2

# 添加认证参数（使用模板变量需要反引号 + 引用内嵌值）
www.example.com urlParams://`{auth-params}`

# 调试模式
www.example.com urlParams://debug=1

# A/B 测试
www.example.com urlParams://experiment=variant_b
```

### 模板变量

````bash
# 动态参数（使用模板变量需要反引号）
www.example.com urlParams://`t=${now}&id=${randomUUID}`

# 或者使用内嵌值（推荐）
www.example.com urlParams://`{time-params}`

``` time-params
t: ${now}
id: ${randomUUID}
```
````

### 测试用例

| 测试场景       | 规则                              | 原始 URL                    | 预期 URL                         |
| -------------- | --------------------------------- | --------------------------- | -------------------------------- |
| 添加单个参数   | `test.com urlParams://debug=true` | `http://test.com/api`       | `http://test.com/api?debug=true` |
| 添加到已有参数 | `test.com urlParams://b=2`        | `http://test.com/api?a=1`   | `http://test.com/api?a=1&b=2`    |
| 覆盖已有参数   | `test.com urlParams://a=new`      | `http://test.com/api?a=old` | `http://test.com/api?a=new`      |
| 小括号格式     | `test.com urlParams://(x:1)`      | `http://test.com/`          | `http://test.com/?x=1`           |

---

## urlReplace（兼容别名 pathReplace）

替换 URL 路径中的内容。

### 语法

```
pattern urlReplace://old=new
pattern urlReplace://(/regex/=replacement)
```

### 基础示例

```bash
# 简单替换
www.example.com urlReplace://v1=v2

# 正则替换
www.example.com urlReplace://(/v\d+/=v3)

# 删除路径部分
www.example.com urlReplace://api/=
```

### 使用场景

```bash
# API 版本迁移
www.example.com urlReplace://v1=v2

# 路径重写
www.example.com urlReplace://old-service=new-service

# 环境切换
www.example.com urlReplace://prod=staging

# 移除前缀
www.example.com urlReplace://prefix/=
```

### 正则替换

```bash
# 替换所有版本号
www.example.com urlReplace://(/\/v\d+\//=/v999/)

# 捕获组替换
www.example.com urlReplace://(/\/users\/(\d+)/=/api/user/$1)

# 大小写不敏感
www.example.com urlReplace://(/\/API/i=/api)
```

### 测试用例

| 测试场景 | 规则                                 | 原始路径        | 预期路径         |
| -------- | ------------------------------------ | --------------- | ---------------- |
| 简单替换 | `test.com urlReplace://old=new`      | `/old/path`     | `/new/path`      |
| 版本替换 | `test.com urlReplace://v1=v2`        | `/api/v1/users` | `/api/v2/users`  |
| 正则替换 | `test.com urlReplace://(/v\d+/=v99)` | `/api/v1/users` | `/api/v99/users` |
| 删除部分 | `test.com urlReplace://prefix/=`     | `/prefix/api`   | `/api`           |

---

## 规则组合

URL 操作规则可以与其他规则组合：

```bash
# URL 参数 + 路由
www.example.com urlParams://debug=true host://debug-server.local

# 路径替换 + 响应修改
www.example.com urlReplace://v1=v2 resHeaders://X-Api-Version=v2

# 多个 URL 操作
www.example.com urlReplace://old=new urlParams://migrated=true

# 配合过滤器
www.example.com urlParams://test=1 includeFilter://h:X-Test
```

---

## 注意事项

1. **参数编码**：参数值会自动进行 URL 编码
2. **参数覆盖**：同名参数会被覆盖，而非追加
3. **路径替换顺序**：替换按照规则定义顺序执行
4. **正则性能**：复杂正则可能影响性能
