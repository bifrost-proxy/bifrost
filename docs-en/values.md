> Language: **English** | [中文](../docs/values.md)

# Values Guide

Values are reusable configuration snippets that rules and scripts can reference. They are stored under the Bifrost data directory.

## Storage

```text
~/.bifrost/values/
```

Each key maps to one file; the file content is the value.

## Rule References

> ℹ️ `{key}` resolves both **embedded value blocks defined in the same rule file** and **global Values created with `bifrost value add`** (verified on the real `bifrost start` path: `resBody://{myval}` emits the stored value). Note `file://`/`tpl://` treat their value as a file path, so to emit a value as content use a content op like `resBody://{key}`, not `file://{key}`.

Define the value as an embedded block in the rule file and reference it with `{name}`:

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

Scripts are meant to read Values from `ctx.values`:

```javascript
var token = ctx.values["API_TOKEN"];
if (token) {
  request.headers["Authorization"] = "Bearer " + token;
}
```

> ℹ️ `ctx.values` is populated from global Values (verified, 0.0.96, real `bifrost start` path): after `bifrost value add API_TOKEN ...`, a script reads it via `ctx.values["API_TOKEN"]` and it appears in `Object.keys(ctx.values)`. The example above works as written.
