---
title: "Values Guide"
description: "Values storage, rule references, CLI management, and script access."
editUrl: false
sidebar:
  label: "Values Guide"
  order: 160
---

> This page is automatically synced from `docs-en/values.md`.
> Language: **English** | [中文](../../../reference/values/)

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
bifrost value set <name> <value>
bifrost value delete <name>
bifrost value import <file>
```

## Script Access

Scripts read Values from `ctx.values`:

```javascript
var token = ctx.values["API_TOKEN"];
if (token) {
  request.headers["Authorization"] = "Bearer " + token;
}
```
