> Language: **English** | [中文](../docs/values.md)

# Values Guide

Values are reusable configuration snippets that rules and scripts can reference. They are stored under the Bifrost data directory.

## Storage

```text
~/.bifrost/values/
```

Each key maps to one file; the file content is the value.

## Rule References

```txt
pattern file://{mockResponse}
pattern resHeaders://{customHeaders}
```

Embedded rule-file values are often better for small rule-local content:

````txt
``` ua.txt
Mozilla/5.0 (iPhone; CPU iPhone OS 16_6 like Mac OS X)
```
pattern ua://{ua.txt}
````

## CLI Management

```bash
bifrost value list
bifrost value show <name>
bifrost value add <name> <value>     # `set` is a hidden alias of `add`
bifrost value update <name> <value>
bifrost value delete <name>
bifrost value import <file>
```

The canonical subcommands are `list`, `show`, `add`, `update`, `delete`, and `import`. `get` (alias of `show`) and `set` (alias of `add`) work but are not shown by `bifrost value --help`.

## Script Access

Scripts read Values from `ctx.values`:

```javascript
var token = ctx.values["API_TOKEN"];
if (token) {
  request.headers["Authorization"] = "Bearer " + token;
}
```
