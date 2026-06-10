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

> ⚠️ 0.0.96 限制：规则里的 `{名称}` 只展开**同一规则文件内的内嵌值块**，不会展开通过 `bifrost value add` 存到 `~/.bifrost/values/` 的全局 Values。直接写 `file://{key}`、`resHeaders://{key}` 引用全局 Value 时，`{key}` 会被当成字面量（`file://` 报 `File not found`，`resHeaders://{key}` 不注入任何头）。需要在规则里复用全局 Value 时，请把内容写成内嵌值块。`file://` 协议本身只解析文件路径，不能用来回放值内容；要回放固定响应体请改用 `resBody://`。

更多规则侧细节见：

- [operation.md](./operation.md)
- [rule.md](./rule.md)

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

> ⚠️ 0.0.96 限制：`ctx.values` 当前是一个**空对象** —— 全局 Values 并没有被注入进脚本上下文，`ctx.values["API_TOKEN"]` 返回 `undefined`，`Object.keys(ctx.values)` 为空。脚本沙箱里也没有 `ctx.getValue()` / `ctx.value()` 等其它读取入口（`ctx` 仅含 `requestId / scriptName / scriptType / phase / values / matchedRules`）。因此上面这段示例在当前版本里不会真正设置 `Authorization` 头，仅作为待修复后的目标用法保留。脚本里需要用到的 token / 配置，目前请直接写进脚本内容，或在规则侧用内嵌值块。

Scripts 侧细节见：[scripts.md](./scripts.md)。
