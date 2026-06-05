> Language: **English** | [中文](../../docs/rules/scripts.md)

# Script Rules

Script rules attach QuickJS scripts to request, response, decode, or binary parser phases.

```txt
example.com reqScript://my-request-script
example.com resScript://my-response-script
example.com decode://utf8
example.com bp://my-parser decode://bp
```

Scripts are stored under `~/.bifrost/scripts/{type}/`. Request scripts can mutate `request`, response scripts can mutate `response`, and decode/parser scripts affect stored and displayed traffic.
