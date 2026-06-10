> Language: **English** | [中文](../docs/operation.md)

# Operation Reference

Operations define what Bifrost does after a pattern matches. Values can be inline, parenthesized, referenced from an embedded value block defined in the same rule file, or referenced from a global Value created with `bifrost value add` (both forms resolve via `{key}` — verified on the real `bifrost start` path). Note that `file://`/`tpl://` treat their value as a file path, so to emit a value as content use a content op like `resBody://{key}`, not `file://{key}`.

## Value Forms

```txt
pattern reqHeaders://X-Debug=1
pattern resBody://(inline body with spaces)
pattern resBody://{mock-response.json}    # {key} = an embedded block in this rule file or a global Value
pattern urlParams://t=${now}&id=${randomUUID}
```

Template variables (`${...}`) expand unconditionally inside any value — backticks do not enable template parsing and are not special syntax (they pass through into the output literally). Backticks are only useful to protect a value that contains spaces from the rule-line tokenizer. Note: request-context vars (`${host}`, `${reqHeaders.x}`, …) work, but response-phase vars `${statusCode}` / `${resHeaders.x}` / `${resCookies.x}` and `${realHost}` / `${realPort}` / `${realUrl}` currently expand to an empty string (verified).

## Categories

- Routing: `host`, `xhost`, `proxy`, `http`, `https`, `ws`, `wss`, `pac` (not yet implemented — `pac://` only extracts a literal `PROXY host:port`, it does not fetch or evaluate a remote/JS PAC file).
- Request modification: `reqHeaders`, `reqCookies`, `urlParams`, `reqBody`, `method`, `auth`.
- Response modification: `resHeaders`, `resCookies`, `statusCode`, `replaceStatus`, `resBody`, `file`, `tpl`.
- Timing and throttling: `reqDelay`, `resDelay`, `reqSpeed`, `resSpeed`.
- Scripts: `reqScript`, `resScript`, `decode`, `bp`.
- TLS and upstream behavior: `tlsIntercept`, `tlsPassthrough`, `upstreamUnsafeSsl`.

Use command-specific help and the detailed rule documents for exact value syntax.
