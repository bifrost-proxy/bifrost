# Bifrost 多需求并行开发调研记录

## 1. 原始需求

多需求并行开发场景下，同一个开发者同时开发多个需求（featA、featB），web 和 mobile 各自需要独立的 dev server 端口（如 web:3000/3001, mobile:3002/3003）。现有 Bifrost 规则是"一条规则 → 一个固定端口"，切换需求需要手动 disable/enable 规则，效率极低。

核心诉求：**通过请求维度信息（header/cookie/UA 等）自动路由到对应端口，无需手动切换规则。**

## 2. 最初的想法

全局写一条 JS script 规则（reqScript），在脚本里根据 `x-tt-env-fe` header 的值动态决定路由目标：

```
www.coze.cn reqScript://{script-name}
```

脚本内部读 `req.headers['x-tt-env-fe']`，按值映射到不同端口。前端通过 ModHeader 浏览器扩展注入 `x-tt-env-fe` header，标识当前浏览器 tab 属于哪个需求。

## 3. Q&A

### ModHeader 注入可行性
- ModHeader 支持 per-tab 配置，可以给不同 tab 注入不同 header 值 → 可行。

### reqScript 能力边界
- reqScript 能读 header、能改 header → ✅
- reqScript 能改路由目标（rewrite url/host）吗？→ ❌ 需要验证

### Mobile 真机不走 Bifrost
- 真机不过本机代理 → ModHeader 方案对 mobile 无效
- Mobile 需要独立方案（独立规则 + UA filter 或 ip filter）

## 4. 技术调研过程

### 4.1 reqScript 不能改路由目标

验证结论：reqScript 回调里的 `url`、`host`、`path` 是**只读快照**，赋值不生效。reqScript 只能修改 header/cookie/body，无法改变请求转发目标。

这直接否决了"一条 reqScript 规则搞定所有路由"的方案。

### 4.2 模板字符串从 header 动态读端口

Bifrost 规则支持模板语法：

```
www.coze.cn host://127.0.0.1:${reqH.x-coze-port}
```

`${reqH.x-coze-port}` 会在请求时从 request header 中读取 `x-coze-port` 的值作为端口号。

验证结论：模板展开确实能从 header 读值 → ✅。但这要求前端注入的是**端口号**而不是需求名，后续需要对比两种 header 方案。

### 4.3 rules.push() 是旧版 Whistle API

尝试在 reqScript 里用 `rules.push()` 动态添加规则：

```javascript
rules.push('www.coze.cn host://127.0.0.1:3001');
```

验证结论：`rules.push()` 是旧版 Whistle（Node.js）的 API，当前 Rust 版 Bifrost 的 reqScript 沙箱**不支持**此 API。调用无效果。

### 4.4 reqScript 改 header → 同规则模板读改后 header → 不行

设想链路：
1. reqScript 先把 `x-tt-env-fe=featA-web` 转换为 `x-coze-port=3000`
2. 同一规则用 `host://127.0.0.1:${reqH.x-coze-port}` 读端口

验证结论：**模板展开读的是原始请求头，不是 reqScript 修改后的头。** 模板 `${reqH.*}` 的求值发生在 rule resolve 阶段，早于 reqScript 执行。即使 reqScript 改了 header，模板不会看到改后的值。

此路不通。

### 4.5 `line` 块语法验证

Bifrost 支持 `` line`...` `` 语法，把多行写成一条逻辑规则：

```
line`
www.coze.cn
host://127.0.0.1:3000
includeFilter://h:x-tt-env-fe=featA-web
`
```

验证结论：`line` 块语法**能正常解析**，等价于单行写法。可以用于提高复杂规则的可读性。

### 4.6 @规则引用

`@` 语法能引用外部规则文件，原位展开：

```
@/path/to/rules.txt
```

验证结论：@引用能工作，但和 `\` 续行有交互问题（续行内不能嵌套 @，@内不能用 \）。实用性有限，最终未采用。

## 5. 「x-coze-port 传端口号」vs「x-tt-env-fe 传需求名 + 多规则」

### 方案 A：x-coze-port 传端口号

```
www.coze.cn host://127.0.0.1:${reqH.x-coze-port} includeFilter://m:GET
```

- 优点：只需一条规则，不用按需求增删规则
- 缺点：
  - 前端 ModHeader 需要注入端口号而非语义化需求名
  - 端口号是开发者本地分配的，没有全局注册
  - **致命问题**：模板读原始头（见 4.4），如果 header 不在原始请求里就读不到

### 方案 B：x-tt-env-fe 传需求名 + 多条规则

```
www.coze.cn host://127.0.0.1:3000 includeFilter://h:x-tt-env-fe=featA-web
www.coze.cn host://127.0.0.1:3001 includeFilter://h:x-tt-env-fe=featB-web
```

- 优点：语义清晰，每个需求一条规则，header 值是需求名
- 缺点：需求增减时要增删规则
- **前提**：同 pattern 多条规则 + 不同 header filter 必须能正确路由

方案 B 更符合实际使用习惯，但依赖"同 pattern 不同 filter 能共存"——这成为后续验证的核心。

## 6. 同事 UA 方案验证

同事提出用 UA 字符串区分：

```
www.coze.cn host://127.0.0.1:3000 includeFilter://h:user-agent=/featA/
www.coze.cn host://127.0.0.1:3001 includeFilter://h:user-agent=/featB/
```

验证结果：

- **单条规则 + UA filter**：有效 ✅
- **两条同 pattern + 不同 UA filter 共存**：路由错乱 ❌

现象：只有先被加载的那条规则生效，第二条永远不生效，无论实际 UA 是什么。

## 7. 重大发现：Bifrost 规则解析结果缓存 Bug

### 7.1 现象

两条规则：
```
www.coze.cn host://127.0.0.1:3000 includeFilter://h:x-tt-env-fe=featA-web
www.coze.cn host://127.0.0.1:3001 includeFilter://h:x-tt-env-fe=featB-web
```

第一个带 `x-tt-env-fe=featA-web` 的请求正确路由到 3000。之后带 `x-tt-env-fe=featB-web` 的请求**也被路由到 3000**。

### 7.2 根因

`crates/bifrost-core/src/rule/resolver.rs:258`：

```rust
let cache_key = format!("{}|{}|{}|{}", ctx.url, ctx.host, ctx.path, ctx.method);
```

Cache key 只包含 `url|host|path|method`，**不包含 header**。

第一次请求解析后，结果被缓存。第二次请求 URL 相同（key 命中），直接返回缓存结果，**完全跳过了 filter 评估**。

### 7.3 cache_enabled 默认 true

- `cache_enabled` 字段默认 `true`
- Runtime 没有暴露任何开关（CLI flag、环境变量、配置项）
- `disable_cache()` 方法存在，但没有被任何外部代码调用

### 7.4 实测验证

| 测试场景 | 结果 | 分析 |
|---|---|---|
| 不同 query string（`?v=1` vs `?v=2`） | 两条都生效 | url 不同 → cache key 不同 → 不命中缓存 |
| 不同 pattern 写法（`www.coze.cn` vs `**.coze.cn`） | 仍然只有一条生效 | pattern 不是 cache key 的一部分 |
| 不同 header | 只有第一条生效 | header 不在 cache key 中 |
| 同一规则 disable 后 re-enable | 缓存被清除，重新生效 | rule change 触发 clear_cache |

### 7.5 影响范围

**所有依赖"同 URL + 请求头/cookie/UA 维度做路由分流"的方案全部失效。** 包括但不限于：
- x-tt-env-fe header 路由
- UA filter 路由
- Cookie 维度路由
- IP 维度路由

只要两条规则匹配同一 URL pattern，缓存命中后第二条永远不会被评估。

## 8. 三条出路（不改缓存的前提下）

### 8.1 不同 URL

让不同需求走不同 URL，从而绕开缓存 key 相同的问题：
- 子域名方案：`featA.coze.cn` vs `featB.coze.cn`
- 路径前缀方案：`coze.cn/featA/` vs `coze.cn/featB/`

### 8.2 给 Bifrost 提 PR 修缓存

从根本解决问题——修改缓存机制，让 header filter 的决策不被缓存覆盖。

### 8.3 单规则 enable/disable 切换（现状）

维持现有方案，同时只 enable 一条规则，切换需求时手动 disable 旧的 enable 新的。

## 9. /m/ 前缀方案分析

想法：给 mobile 请求加 `/m/` 路径前缀，让 URL 不同从而绕开缓存：

```
www.coze.cn/m/* host://127.0.0.1:3002
www.coze.cn host://127.0.0.1:3000
```

分析：
- 技术上可行（URL 不同 → cache key 不同）
- 但需要改动打包构建、dev server 路由、线上部署路径
- 引入前端路由层面的差异，增加调试复杂度
- **被否决**：改动过大，超出 Bifrost 层面能解决的范畴

## 10. space.coze.cn 域名方案

想法：不同需求用不同子域名：`space-featA.coze.cn`、`space-featB.coze.cn`

分析：
- URL 不同 → 绕开缓存 ✅
- **Cookie 不共享** ❌：登录态存储在 `coze.cn` 的 cookie 里，子域名不同会导致：
  - 认证 cookie 丢失
  - 需要重新登录
  - API 请求（走 coze.cn）和页面（走 space-featA.coze.cn）的 cookie 域不一致
- **被否决**：cookie/session 问题无法在 Bifrost 层解决

## 11. 缓存系统分析

### 11.1 缓存解决的问题

Bifrost 的 resolve 缓存优化的是**热路径性能**：
- 正则匹配和通配符匹配是 CPU 密集操作
- 同一个页面加载时，几十个请求命中同一组规则
- 缓存避免对相同 URL 重复执行 pattern matcher
- 典型场景：前端页面加载几十个同域资源，pattern 匹配只需做一次

### 11.2 关掉缓存的代价

- 影响维度：**纯 CPU**，不影响网络 IO，不影响正确性
- 本地开发场景：规则数 < 20 条，URL 数量有限
- Pattern matcher 单次耗时：< 1ms（20 条规则全扫）
- 每秒代理请求量：开发场景 < 100 req/s
- 结论：**关掉缓存在本地场景下无感知性能影响**

### 11.3 disable_cache() 存在但未接通

```rust
// resolver.rs 中存在
pub fn disable_cache(&mut self) {
    self.cache_enabled = false;
}
```

但没有任何 runtime 代码调用它：
- CLI 没有 `--no-cache` flag
- 配置文件没有 `cache_enabled` 字段
- 环境变量没有 `BIFROST_DISABLE_CACHE`

即使想临时关缓存做验证，也没有非侵入式的办法。

## 12. 最终结论

### 需要改造 Bifrost 的 resolve 缓存架构

当前的「全量缓存」设计假设不成立（假设"同 URL → 同结果"在有 header filter 时不成立）。需要**拆分为两相**：

#### Phase A: Matcher 候选（可缓存）

- 只做 pattern 匹配（正则/通配符），这是最贵的计算
- Cache key: `url|host|path`
- 输出: 所有 pattern 匹配成功的候选规则列表
- 这部分「同 URL → 同候选集」假设成立

#### Phase B: Filter Gate + 决策（不缓存，每请求执行）

- 对候选规则逐条评估 include/exclude filter
- 依赖完整的 RequestContext（header/cookie/ip/method 等）
- Filter 比较本身是 O(1) 字符串比较，不缓存不影响性能
- 输出: 最终 ResolvedRules

这样既保留了 pattern matcher 的缓存优化（解决热路径问题），又让 header filter 的动态决策在每个请求上独立执行（解决缓存错误复用问题）。

详细技术方案见：[两相改造技术方案.md](./两相改造技术方案.md)
