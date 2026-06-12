---
title: "Values 使用说明"
description: "Values 存储、规则引用、CLI 管理与脚本访问说明。"
editUrl: false
sidebar:
  label: "Values 使用说明"
  order: 160
---

> 此页面由 `docs/values.md` 自动同步生成。
# Values 使用说明

Values 是 Bifrost 的统一变量管理机制，用于在规则和脚本中复用配置内容。

## 存储位置

Values 默认存储在数据目录下：

```text
~/.bifrost/values/
```

每个 key 对应一个文件，文件内容即变量值。

## 在规则中引用

规则侧通过在规则文件中定义内嵌值块来引用 Values：先用 ` ``` 名称 ` 起一个值块，块内即变量内容，再在规则行里用 `{名称}` 引用。

````txt
``` ua.txt
Mozilla/5.0 (iPhone; CPU iPhone OS 16_6 like Mac OS X)
```
pattern ua://{ua.txt}
````

内嵌值块同样可用于注入响应体或响应头，例如：

````txt
``` mockResponse
{"ec":0,"data":"mocked"}
```
pattern resBody://{mockResponse}
````

````txt
``` customHeaders
X-Injected: from-value
```
pattern host://127.0.0.1:3000 resHeaders://{customHeaders}
````

> ℹ️ 经实测（bifrost 0.0.96，真实 `bifrost start` 路径）：规则里的 `{名称}` 既能引用**同一规则文件内的内嵌值块**，也能引用通过 `bifrost value add` 存到 `~/.bifrost/values/` 的全局 Values（例如 `resBody://{myval}` 会输出该全局 value 的内容）。注意 `file://` 协议的 value 会被当作文件路径处理，不能用来回放 value 内容；要把某个 value 作为固定响应体回放，请用 `resBody://{key}`。

更多规则侧细节见：

- [operation.md](../operations/)
- [rule.md](../rule-engine/)

## 通过 CLI 管理

```bash
bifrost value list
bifrost value show <name>
bifrost value get <name>            # show 的别名
bifrost value add <name> <value>
bifrost value set <name> <value>    # add 的别名
bifrost value update <name> <value>
bifrost value delete <name>
bifrost value import <file>
```

## 在脚本中使用

脚本沙箱里通过 `ctx.values` 暴露 Values：

```javascript
var token = ctx.values["API_TOKEN"];
if (token) {
  request.headers["Authorization"] = "Bearer " + token;
}
```

> ℹ️ 经实测（bifrost 0.0.96，真实 `bifrost start` 路径）：脚本沙箱的 `ctx.values` 会被全局 Values 填充——`bifrost value add API_TOKEN ...` 后，脚本里 `ctx.values["API_TOKEN"]` 能取到该值，`Object.keys(ctx.values)` 包含它。上面这段示例可正常工作。（`ctx` 字段：`requestId / scriptName / scriptType / phase / values / matchedRules`。）

Scripts 侧细节见：[scripts.md](../scripting/)。
