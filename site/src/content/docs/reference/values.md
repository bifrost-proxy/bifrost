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

推荐使用 `operation` 文档中的 Values 引用语法：

```txt
pattern file://{mockResponse}
pattern resHeaders://{customHeaders}
```

也可以在规则文件中定义内嵌值：

````txt
``` ua.txt
Mozilla/5.0 (iPhone; CPU iPhone OS 16_6 like Mac OS X)
```
pattern ua://{ua.txt}
````

更多规则侧细节见：

- [operation.md](../operations/)
- [rule.md](../rule-engine/)

## 通过 CLI 管理

```bash
bifrost value list
bifrost value show <name>
bifrost value get <name>
bifrost value add <name> <value>
bifrost value set <name> <value>
bifrost value update <name> <value>
bifrost value delete <name>
bifrost value import <file>
```

## 在脚本中使用

脚本沙箱会把 Values 注入到 `ctx.values`：

```javascript
var token = ctx.values["API_TOKEN"];
if (token) {
  request.headers["Authorization"] = "Bearer " + token;
}
```

Scripts 侧细节见：[scripts.md](../scripting/)。
