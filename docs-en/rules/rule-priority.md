> Language: **English** | [中文](../../docs/rules/rule-priority.md)

# Rule Priority and Execution Order

Bifrost rule execution follows two principles.

1. Routing rules are exclusive: the first matched routing rule wins. Among forwarding targets, `host`-family operations (`host`/`xhost`/`http`/`https`/`ws`/`wss`) win over `proxy` by protocol precedence, regardless of file order.
2. Modification rules can merge: operations on different fields combine. For duplicate keys the behavior is protocol-specific (see below).

```txt
www.example.com host://server1.local
www.example.com host://server2.local
```

The request goes to `server1.local` because the first routing rule wins.

Duplicate-key merge semantics differ by protocol:

- `reqHeaders` / `resHeaders`: **first occurrence wins** — a later rule with the same header key is ignored (e.g. `X-Custom=value1` then `X-Custom=value2` yields `value1`).
- `reqCookies` / `urlParams`: last occurrence wins for the same key.
- `reqBody` / `resBody` / `resCors`: last matching rule overwrites earlier ones.
- `statusCode`: single-match — the first matching rule applies and later `statusCode` rules are ignored.

Note: the emitted order of merged `urlParams` is not guaranteed to follow definition order.
