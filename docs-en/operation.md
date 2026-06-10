> Language: **English** | [中文](../docs/operation.md)

# Operation Reference

Operations define what Bifrost does after a pattern matches. Values can be inline, parenthesized, embedded in the rule file, or referenced from global Values depending on the protocol.

## Value Forms

```txt
pattern reqHeaders://X-Debug=1
pattern resBody://(inline body with spaces)
pattern file://{mock-response.json}
pattern urlParams://t=${now}&id=${randomUUID}
```

Template variables (`${...}`) expand unconditionally inside any value — backticks do not enable template parsing and are not special syntax (they pass through into the output literally). Backticks are only useful to protect a value that contains spaces from the rule-line tokenizer.

## Categories

- Routing: `host`, `xhost`, `proxy`, `http`, `https`, `ws`, `wss`, `pac` (not yet implemented — `pac://` only extracts a literal `PROXY host:port`, it does not fetch or evaluate a remote/JS PAC file).
- Request modification: `reqHeaders`, `reqCookies`, `urlParams`, `reqBody`, `method`, `auth`.
- Response modification: `resHeaders`, `resCookies`, `statusCode`, `replaceStatus`, `resBody`, `file`, `tpl`.
- Timing and throttling: `reqDelay`, `resDelay`, `reqSpeed`, `resSpeed`.
- Scripts: `reqScript`, `resScript`, `decode`, `bp`.
- TLS and upstream behavior: `tlsIntercept`, `tlsPassthrough`, `upstreamUnsafeSsl`.

Use command-specific help and the detailed rule documents for exact value syntax.
