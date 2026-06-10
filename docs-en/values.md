> Language: **English** | [中文](../docs/values.md)

# Values Guide

Values are reusable configuration snippets that rules and scripts can reference. They are stored under the Bifrost data directory.

## Storage

```text
~/.bifrost/values/
```

Each key maps to one file; the file content is the value.

## Rule References

> ⚠️ **Global Values are NOT resolved by `{key}` in rules (verified, 0.0.96).** Only **embedded value blocks defined in the same rule file** are expanded. A `{key}` that refers to a global Value created with `bifrost value add` is emitted literally (verified: `resBody://{WFTESTVAL}` outputs the literal text `{WFTESTVAL}`; `file://{key}` reports `File not found`). Also `file://`/`tpl://` treat `{key}` as a disk filename, so carry value references with a response-producing op like `resBody://{key}`, not `file://{key}`.

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

> ⚠️ **`ctx.values` is empty in 0.0.96 (verified).** Global Values are not injected into the script sandbox — `ctx.values["API_TOKEN"]` returns `undefined` and `Object.keys(ctx.values)` is empty (verified with a request script). There is no `ctx.getValue()` either. Until this is wired, put any token/config the script needs directly in the script body, or use an embedded value block on the rule side.
