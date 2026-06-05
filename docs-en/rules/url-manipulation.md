> Language: **English** | [中文](../../docs/rules/url-manipulation.md)

# URL Manipulation Rules

URL manipulation rules add, replace, or remove URL components.

```txt
example.com urlParams://debug=true
example.com urlParams://(version: 2)
example.com urlReplace://v1=v2
example.com urlReplace://(/v\d+/=v2)
```

Use backticks when runtime template variables are needed, for example `urlParams://` with `${now}` or `${randomUUID}`. Embedded values are recommended for multi-parameter configuration.
