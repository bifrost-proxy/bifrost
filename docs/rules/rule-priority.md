# 规则优先级与执行顺序

本章介绍 Bifrost 规则的优先级和执行顺序。

---

## 概述

Bifrost 规则的执行遵循两个核心原则：

1. **转发类规则**：按规则优先级从高到低排序后，第一个匹配的生效
2. **修改类规则**：相同部位的操作会合并，后面的覆盖前面的

> ⚠️ **先决条件：优先级排序在文件顺序之前。** 引擎先按每条规则的 `priority()`（由 pattern 类型与精确度决定，`lineProps://important` 额外 +10000）从高到低排序，再处理匹配。**文件中的定义顺序只在"同优先级"的规则之间充当 tiebreaker。** 因此，更精确的 Domain 规则即使写在宽泛的 Wildcard 规则下面，仍然先生效。本章下文「先定义的优先」「顺序敏感」等描述，仅适用于 **pattern 相同（即优先级相同）** 的规则之间；不同优先级时由 pattern 类型/精确度决定胜负（详见 [pattern.md](../pattern.md) 优先级表）。

---

## 规则分类

### 转发类规则（互斥）

转发类规则决定请求的目标地址，**只有第一个匹配的规则生效**。

| 协议       | 说明                     |
| ---------- | ------------------------ |
| `host`     | 重定向到指定主机         |
| `xhost`    | 强制重定向（优先级更高） |
| `proxy`    | HTTP 代理转发            |
| `pac`      | PAC 路由                 |
| `http` / `https` | 显式协议转发      |
| `ws` / `wss` | WebSocket 转发        |
| `redirect` | URL 重定向               |
| `file` / `tpl` / `rawfile` | 本地文件/模板响应 |

**执行特点**：

- 第一个匹配的转发规则生效，后续转发规则被忽略
- `xhost` 优先级高于普通 `host`

### 修改类规则（可合并）

修改类规则对请求/响应进行修改，**相同类型的规则会合并执行**。

| 协议         | 说明        | 合并行为     |
| ------------ | ----------- | ------------ |
| `reqHeaders` | 请求头      | 后面覆盖前面 |
| `resHeaders` | 响应头      | 后面覆盖前面 |
| `reqCookies` | 请求 Cookie | 后面覆盖前面 |
| `resCookies` | 响应 Cookie | 后面覆盖前面 |
| `urlParams`  | URL 参数    | 后面覆盖前面 |
| `reqBody`    | 请求 Body   | 最后一个生效 |
| `resBody`    | 响应 Body   | 最后一个生效 |
| `statusCode` | 状态码      | 最后一个生效 |

---

## 转发类规则优先级

### 基本原则

```bash
# 规则文件中定义顺序
www.example.com host://server1.local
www.example.com host://server2.local
```

**结果**：请求转发到 `server1.local`（第一个匹配的生效）

### xhost 优先级

```bash
# xhost 优先于 host
www.example.com host://server1.local
www.example.com xhost://server2.local
```

**结果**：请求转发到 `server2.local`（xhost 优先级更高）

### 不同转发类型

```bash
# host 和 proxy 是互斥的
www.example.com host://server.local
www.example.com proxy://proxy.local:8080
```

**结果**：请求转发到 `server.local`（host 先匹配）

### 测试用例

| 测试场景 | 规则 | 预期结果 |
| --- | --- | --- |
| host 顺序 | `test.com host://s1` + `test.com host://s2` | 转发到 s1 |
| xhost 优先 | `test.com host://s1` + `test.com xhost://s2` | 转发到 s2 |
| host 先于 proxy | `test.com host://s1` + `test.com proxy://p1:8080` | 转发到 s1 |

---

## 修改类规则合并

### 相同头部字段覆盖

```bash
# 同一字段，后面覆盖前面
www.example.com reqHeaders://(X-Custom:value1)
www.example.com reqHeaders://(X-Custom:value2)
```

**结果**：请求头 `X-Custom` 的值为 `value2`

### 不同头部字段合并

```bash
# 不同字段，都会添加
www.example.com reqHeaders://(X-Header-A:valueA)
www.example.com reqHeaders://(X-Header-B:valueB)
```

**结果**：请求头同时包含 `X-Header-A: valueA` 和 `X-Header-B: valueB`

### Cookie 合并

```bash
# 不同 Cookie 合并，相同 Cookie 覆盖
www.example.com reqCookies://(a:1)
www.example.com reqCookies://(b:2)
www.example.com reqCookies://(a:99)
```

**结果**：Cookie 包含 `a=99; b=2`

### URL 参数合并

```bash
# 不同参数合并，相同参数覆盖
www.example.com urlParams://(x:1)
www.example.com urlParams://(y:2)
www.example.com urlParams://(x:99)
```

**结果**：URL 参数 `?x=99&y=2`

### Body 替换

```bash
# Body 只能有一个，最后一个生效
www.example.com resBody://(body1)
www.example.com resBody://(body2)
```

**结果**：响应 Body 为 `body2`

### 测试用例

| 测试场景      | 规则                                        | 预期结果   |
| ------------- | ------------------------------------------- | ---------- |
| 头部覆盖      | `reqHeaders://(X:1)` + `reqHeaders://(X:2)` | X: 2       |
| 头部合并      | `reqHeaders://(A:1)` + `reqHeaders://(B:2)` | A: 1, B: 2 |
| Cookie 覆盖   | `reqCookies://(a:1)` + `reqCookies://(a:2)` | a=2        |
| Cookie 合并   | `reqCookies://(a:1)` + `reqCookies://(b:2)` | a=1; b=2   |
| 参数覆盖      | `urlParams://(x:1)` + `urlParams://(x:2)`   | x=2        |
| 参数合并      | `urlParams://(x:1)` + `urlParams://(y:2)`   | x=1&y=2    |
| Body 最后生效 | `resBody://(a)` + `resBody://(b)`           | b          |

---

## 混合规则执行

转发规则和修改规则可以同时生效：

```bash
# 转发 + 修改同时生效
www.example.com host://backend.local reqHeaders://(X-Proxy:true)
www.example.com reqHeaders://(X-Version:2)
```

**结果**：
- 请求转发到 `backend.local`（转发规则）
- 请求头包含 `X-Proxy: true` 和 `X-Version: 2`（修改规则合并）

### 测试用例

| 测试场景 | 规则 | 预期结果 |
|---------|------|---------|
| 转发+修改 | `host://s1 reqHeaders://(A:1)` + `reqHeaders://(B:2)` | 转发 s1, 头部 A+B |
| 多转发+修改 | `host://s1` + `host://s2 reqHeaders://(X:1)` | 转发 s1, 头部 X:1 |

---

## 规则文件内顺序

### 同一行多个规则

```bash
# 同一行的规则同时应用
www.example.com host://backend.local reqHeaders://(X-A:1) resHeaders://(X-B:2)
```

### 多行规则

```bash
# 多行规则按顺序处理
www.example.com host://backend.local
www.example.com reqHeaders://(X-A:1)
www.example.com resHeaders://(X-B:2)
```

**两种写法效果相同**：转发到 backend.local，添加请求头 X-A 和响应头 X-B

---

## 优先级总结

### 转发类规则优先级

1. `xhost`
2. `host` / `proxy` / `pac` / `http` / `https` / `ws` / `wss` / `redirect` / `file` / `tpl` / `rawfile`
3. 同类型：**先定义的优先**

### 修改类规则合并顺序

1. 相同字段/参数：**后定义的覆盖前面的**
2. 不同字段/参数：**全部合并**
3. Body/StatusCode：**最后定义的生效**

---

## 注意事项

1. **转发规则互斥**：一个请求只能转发到一个目标
2. **修改规则可叠加**：多个修改规则可以同时生效
3. **xhost 更强**：`xhost` 会覆盖普通 `host`
4. **顺序敏感**：规则文件中的定义顺序会影响最终结果
5. **调试技巧**：使用 Bifrost 管理端的 Traffic/Network 面板查看实际生效的规则
