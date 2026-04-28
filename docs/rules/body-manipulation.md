# Body 操作规则

本章介绍对请求和响应 Body 进行处理的规则。

---

## file

直接返回本地或远程文件内容作为响应，不请求后端服务器。

### 语法

```
pattern file://file_path
pattern file://(inline_content)
pattern file://{embedded_name}
```

### 基础示例

> ⚠️ **注意**：小括号内不能有空格，JSON 冒号后不要加空格，含空格内容必须使用块变量

```bash
# 返回本地文件
www.example.com/api/config file:///path/to/config.json

# 返回内联内容（无空格）
www.example.com/api/status file://({"status":"ok"})

# 返回远程文件
www.example.com/mock file://http://mock-server.com/data.json

# 含空格内容使用块变量
www.example.com/api/health file://{health-response}
```

块变量定义：

````
``` health-response
{"healthy": true, "version": "1.0"}
```
````

### 使用场景

```bash
# Mock API 响应
www.example.com/api/users file:///mock/users.json

# 返回静态 HTML
www.example.com/maintenance file:///static/maintenance.html

# Mock JSON 响应（无空格）
www.example.com/api/health file://({"healthy":true,"version":"1.0"})
```

### 测试用例

| 测试场景  | 规则                              | 预期                       |
| --------- | --------------------------------- | -------------------------- |
| 内联 JSON | `test.com file://({"ok":true})`   | 响应 Body 为 `{"ok":true}` |
| 本地文件  | `test.com file:///path/mock.json` | 响应 Body 为文件内容       |

---

## rawfile

与 `file` 类似，但不会自动添加响应头。

### 语法

```
pattern rawfile://file_path
```

### 示例

```bash
# 返回原始文件内容
www.example.com/raw rawfile:///path/to/data.bin
```

---

## tpl

模板响应，支持变量替换和动态内容。

### 语法

```
pattern tpl://template_content
pattern tpl://(inline_template)
```

### 模板变量

| 变量              | 说明       |
| ----------------- | ---------- |
| `${now}`          | 当前时间戳 |
| `${random}`       | 随机数     |
| `${randomUUID}`   | 随机 UUID  |
| `${url}`          | 请求 URL   |
| `${host}`         | 请求主机   |
| `${path}`         | 请求路径   |
| `${method}`       | 请求方法   |
| `${query.key}`    | URL 参数   |
| `${headers.name}` | 请求头     |

### 示例

> ⚠️ **注意**：
>
> 1. 模板字符串必须用反引号包裹才能进行变量替换
> 2. 小括号内不能有空格，含空格内容必须使用块变量

```bash
# 动态 JSON 响应（无空格）
www.example.com tpl://`({"time":${now},"id":"${randomUUID}"})`

# JSONP 回调（无空格）
www.example.com tpl://`(${query.callback}({"data":"test"}))`

# 回显请求信息（使用块变量处理空格）
www.example.com tpl://`{echo-tpl}`
```

块变量定义：

````
``` echo-tpl
{"method": "${method}", "path": "${path}"}
```
````

### 测试用例

| 测试场景 | 规则                                               | 预期               |
| -------- | -------------------------------------------------- | ------------------ |
| 时间戳   | ``test.com tpl://`({"t":${now}})`  ``              | 响应包含当前时间戳 |
| UUID     | ``test.com tpl://`({"id":"${randomUUID}"})` ``     | 响应包含有效 UUID  |
| 请求信息 | ``test.com tpl://`({"path":"${url.pathname}"})` `` | 响应包含请求路径   |
| JSONP    | ``test.com tpl://`(${query.cb}({}))` ``            | 使用回调函数包装   |

---

## reqBody

设置或替换请求 Body。

### 语法

```
pattern reqBody://(content)
pattern reqBody://file_path
```

### 示例

> ⚠️ **注意**：小括号内不能有空格

```bash
# 设置 JSON Body（无空格）
www.example.com reqBody://({"key":"value"})

# 从文件读取
www.example.com reqBody:///path/to/request.json

# 清空 Body
www.example.com reqBody://()
```

### 测试用例

| 测试场景  | 规则                           | 预期                   |
| --------- | ------------------------------ | ---------------------- |
| 设置 JSON | `test.com reqBody://({"a":1})` | 请求 Body 为 `{"a":1}` |
| 清空 Body | `test.com reqBody://()`        | 请求 Body 为空         |

---

## resBody

设置或替换响应 Body。

### 语法

```
pattern resBody://(content)
pattern resBody://file_path
```

### 示例

> ⚠️ **注意**：小括号内不能有空格，含空格内容必须使用块变量

```bash
# 设置响应内容（无空格）
www.example.com resBody://({"status":"mocked"})

# 从文件读取
www.example.com resBody:///path/to/response.json

# 设置空响应
www.example.com resBody://()

# 含空格内容使用块变量
www.example.com resBody://{hello-response}
```

块变量定义：

````
``` hello-response
hello world
```
````

### 测试用例

| 测试场景  | 规则                               | 预期                       |
| --------- | ---------------------------------- | -------------------------- |
| 设置 JSON | `test.com resBody://({"ok":true})` | 响应 Body 为 `{"ok":true}` |
| 设置文本  | `test.com resBody://{hello-txt}`   | 响应 Body 为块变量内容     |

---

## reqReplace

替换请求 Body 中的内容。

### 语法

```
pattern reqReplace://old=new
pattern reqReplace://(/regex/=replacement)
pattern reqReplace://(/regex/g=replacement)  # 全局替换
```

### 示例

```bash
# 简单替换
www.example.com reqReplace://old_value=new_value

# 正则替换
www.example.com reqReplace://(/\d{4}/=****) # 隐藏数字

# 全局替换
www.example.com reqReplace://(/password/g=******)
```

### 测试用例

| 测试场景 | 规则                              | 原始 Body   | 预期 Body   |
| -------- | --------------------------------- | ----------- | ----------- |
| 简单替换 | `test.com reqReplace://old=new`   | `old value` | `new value` |
| 正则替换 | `test.com reqReplace://(/\d+/=X)` | `id: 123`   | `id: X`     |

---

## resReplace

替换响应 Body 中的内容。

### 语法

```
pattern resReplace://old=new
pattern resReplace://(/regex/=replacement)
pattern resReplace://(/regex/g=replacement)
```

### 示例

```bash
# 简单替换
www.example.com resReplace://production=development

# 正则替换
www.example.com resReplace://(/https:\/\//g=http://)

# 数据脱敏
www.example.com resReplace://(/\d{4}-\d{4}-\d{4}-\d{4}/g=****-****-****-****)
```

### 测试用例

| 测试场景 | 规则                             | 原始 Body  | 预期 Body  |
| -------- | -------------------------------- | ---------- | ---------- |
| 简单替换 | `test.com resReplace://old=new`  | `old text` | `new text` |
| 全局替换 | `test.com resReplace://(/a/g=b)` | `aaa`      | `bbb`      |

---

## params（兼容别名 reqMerge）

合并 JSON 到请求 Body。

### 语法

```txt
pattern params://(key:value)            # 小括号格式（无空格）
pattern params://{varName}              # 引用内嵌值（推荐）
pattern reqMerge://{varName}            # 兼容旧别名
```

> ⚠️ **注意**：
>
> 1. `{name}` 是引用内嵌值的语法，不是直接定义 JSON！
> 2. 小括号内不能有空格，含空格内容必须使用块变量

### 示例

```bash
# 小括号格式添加字段（无空格）
www.example.com params://(version:"2.0")

# 使用模板变量（需要反引号）
www.example.com params://`(timestamp:${now})`

# 使用内嵌值（推荐，支持空格）
www.example.com params://{merge-data}
```

内嵌值定义：

````
``` merge-data
version: "2.0"
meta.source: proxy
```
````

### 测试用例

| 测试场景 | 规则 | 原始 Body | 预期 Body |
| --- | --- | --- | --- |
| 添加字段 | `test.com params://(b:2)` | `{"a": 1}` | `{"a": 1, "b": 2}` |
| 覆盖字段 | `test.com params://(a:99)` | `{"a": 1}` | `{"a": 99}` |

---

## resMerge

合并 JSON 到响应 Body。

### 语法

```
pattern resMerge://(key:value)          # 小括号格式（无空格）
pattern resMerge://{varName}            # 引用内嵌值（推荐）
```

### 示例

```bash
# 小括号格式（无空格）
www.example.com resMerge://(_proxy:true)

# 使用模板变量（需要反引号）
www.example.com resMerge://`(timestamp:${now})`

# 使用内嵌值（推荐，支持空格）
www.example.com resMerge://{res-merge}
```

### 测试用例

| 测试场景 | 规则                            | 原始 Body      | 预期 Body                  |
| -------- | ------------------------------- | -------------- | -------------------------- |
| 添加字段 | `test.com resMerge://(extra:1)` | `{"data": []}` | `{"data": [], "extra": 1}` |

---

## reqAppend / reqPrepend

在请求 Body 前后追加内容。

### 语法

```
pattern reqAppend://(content)           # 小括号格式（无空格）
pattern reqAppend://{varName}           # 引用内嵌值（推荐）
pattern reqPrepend://(content)
pattern reqPrepend://{varName}
```

### 示例

> ⚠️ **注意**：小括号内不能有空格，含空格内容必须使用块变量

```bash
# 在末尾追加（无空格内容）
www.example.com reqAppend://(\n--appended--)

# 含空格内容使用块变量
www.example.com reqAppend://{append-content}
www.example.com reqPrepend://{prefix-content}
```

块变量定义：

````
``` append-content

-- appended --
```

``` prefix-content
prefix:
```
````

---

## resAppend / resPrepend

在响应 Body 前后追加内容。

### 语法

```
pattern resAppend://(content)           # 小括号格式（无空格）
pattern resAppend://{varName}           # 引用内嵌值（推荐）
pattern resPrepend://(content)
pattern resPrepend://{varName}
```

### 示例

> ⚠️ **注意**：小括号内不能有空格，含空格内容必须使用块变量

```bash
# 无空格内容使用小括号
www.example.com resAppend://(\n<!--proxy-->)

# 含空格内容使用块变量
www.example.com resAppend://{res-append}
www.example.com resPrepend://{res-prepend}
```

块变量定义：

````
``` res-append

<!-- proxy -->
```

``` res-prepend
/* injected */
```
````

---

## 规则组合

Body 操作规则可以与其他规则组合：

```bash
# Mock + 状态码（使用块变量处理含空格 JSON）
www.example.com file://{error-response} statusCode://404

# Mock + 响应头
www.example.com resBody://{mock-data} resHeaders://Content-Type=application/json

# 响应追加 + 条件过滤
www.example.com resAppend://{tracking} includeFilter://resH:content-type=text/html
```

块变量定义：

````
``` error-response
{"error": "not found"}
```
````

---

## 注意事项

1. **编码**：内联内容会自动进行适当的编码处理
2. **JSON 合并**：`params`（兼容别名 `reqMerge`）/`resMerge` 只对 JSON 格式的 Body 有效
3. **替换顺序**：多个替换规则按定义顺序执行
4. **文件路径**：本地文件路径建议使用绝对路径（以 `/` 开头）；相对路径的行为会受运行目录影响
5. **CORS**：使用 `file` 协议时，可能需要配合 `resCors` 处理跨域

---

## HTML/CSS/JS 注入规则

这组规则用于向 HTML、CSS、JS 类型的响应内容注入代码，常用于 Web 调试场景。

> ⚠️ **注意**：这些规则仅对响应类型 `content-type` 匹配对应类型，且包含响应内容体的状态码（如 `200`/`500`）才有效。`204`、`304` 等无响应内容体的请求不受影响。

---

### htmlAppend

在 HTML 类型响应的 `<html>` 元素内部后部追加内容，优先插入最后一个 `</html>` 前。

#### 语法

```
pattern htmlAppend://(content)
pattern htmlAppend://{varName}
pattern htmlAppend:///path/to/file.html
```

#### 示例

```bash
# 在 HTML 元素内部后部注入调试脚本
www.example.com htmlAppend://(<script>console.log('debug')</script>)

# 使用块变量注入复杂内容
www.example.com htmlAppend://{debug-script}
```

块变量定义：

````
``` debug-script
<script>
  console.log('Debug mode enabled');
  window.__DEBUG__ = true;
</script>
```
````

---

### htmlPrepend

在 HTML 类型响应的 `<html>` 元素内部前部插入内容，优先插入 `<html>` 开始标签之后。

#### 语法

```
pattern htmlPrepend://(content)
pattern htmlPrepend://{varName}
pattern htmlPrepend:///path/to/file.html
```

#### 示例

```bash
# 在 HTML 元素内部前部注入样式
www.example.com htmlPrepend://(<style>body{border:2px solid red;}</style>)
```

---

### htmlBody

替换 HTML 类型的响应内容。

#### 语法

```
pattern htmlBody://(content)
pattern htmlBody://{varName}
pattern htmlBody:///path/to/file.html
```

#### 示例

```bash
# 替换整个 HTML 页面
www.example.com htmlBody://{maintenance-page}
```

---

### jsAppend

在 JS 类型响应内容末尾追加代码。

#### 语法

```
pattern jsAppend://(content)
pattern jsAppend://{varName}
pattern jsAppend:///path/to/file.js
```

#### 示例

```bash
# 在 JS 末尾追加代码
www.example.com/app.js jsAppend://(;console.log('loaded');)

# 使用块变量
www.example.com/app.js jsAppend://{js-monitor}
```

块变量定义：

````
``` js-monitor
;(function() {
  console.log('Script monitoring enabled');
})();
```
````

---

### jsPrepend

在 JS 类型响应内容开头插入代码。

#### 语法

```
pattern jsPrepend://(content)
pattern jsPrepend://{varName}
pattern jsPrepend:///path/to/file.js
```

#### 示例

```bash
# 在 JS 开头注入代码
www.example.com/app.js jsPrepend://(window.__START__=Date.now();)
```

---

### jsBody

替换 JS 类型的响应内容。

#### 语法

```
pattern jsBody://(content)
pattern jsBody://{varName}
pattern jsBody:///path/to/file.js
```

#### 示例

```bash
# 替换整个 JS 文件
www.example.com/old.js jsBody://{new-script}
```

---

### cssAppend

在 CSS 类型响应内容末尾追加样式。

#### 语法

```
pattern cssAppend://(content)
pattern cssAppend://{varName}
pattern cssAppend:///path/to/file.css
```

#### 示例

```bash
# 在 CSS 末尾追加样式
www.example.com/style.css cssAppend://(body{border:1px solid red;})
```

---

### cssPrepend

在 CSS 类型响应内容开头插入样式。

#### 语法

```
pattern cssPrepend://(content)
pattern cssPrepend://{varName}
pattern cssPrepend:///path/to/file.css
```

#### 示例

```bash
# 在 CSS 开头注入样式
www.example.com/style.css cssPrepend://(*{box-sizing:border-box;})
```

---

### cssBody

替换 CSS 类型的响应内容。

#### 语法

```
pattern cssBody://(content)
pattern cssBody://{varName}
pattern cssBody:///path/to/file.css
```

#### 示例

```bash
# 替换整个 CSS 文件
www.example.com/old.css cssBody://{new-styles}
```

---

### HTML/CSS/JS 注入测试用例

| 测试场景   | 规则                                       | 预期                            |
| ---------- | ------------------------------------------ | ------------------------------- |
| HTML 追加  | `test.com htmlAppend://(<div>test</div>)`  | `</html>` 前添加 `<div>test</div>` |
| HTML 前置  | `test.com htmlPrepend://(<meta>)`          | `<html>` 开始标签后添加 `<meta>` |
| JS 追加    | `test.com/app.js jsAppend://(;alert(1);)`  | JS 末尾追加 `;alert(1);`        |
| JS 前置    | `test.com/app.js jsPrepend://(var x=1;)`   | JS 开头添加 `var x=1;`          |
| CSS 追加   | `test.com/style.css cssAppend://(body{})`  | CSS 末尾追加 `body{}`           |
| 非对应类型 | `test.com jsAppend://(...) resType://html` | 不生效（Content-Type 不匹配）   |

---

### 使用场景

#### 1. 远程调试注入

```bash
# 注入 VConsole 调试工具
www.example.com htmlAppend://{vconsole-inject}
```

````
``` vconsole-inject
<script src="https://unpkg.com/vconsole/dist/vconsole.min.js"></script>
<script>new VConsole();</script>
```
````

#### 2. 样式覆盖调试

```bash
# 临时修改样式
www.example.com/style.css cssAppend://{debug-styles}
```

````
``` debug-styles
.hidden { display: block !important; }
.debug { outline: 2px solid red; }
```
````

#### 3. JS 功能增强

```bash
# 注入性能监控
www.example.com/main.js jsPrepend://{performance-monitor}
```

````
``` performance-monitor
(function(){
  window.__loadStart = Date.now();
  window.addEventListener('load', function(){
    console.log('Load time:', Date.now() - window.__loadStart, 'ms');
  });
})();
```
````
