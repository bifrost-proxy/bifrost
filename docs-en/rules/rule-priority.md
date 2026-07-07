> Language: **English** | [中文](../../docs/rules/rule-priority.md)

# Rule Priority and Execution Order

Bifrost resolves rules in two layers:

1. **Matcher priority first.** Rules are sorted by `priority()` before they are evaluated. The score comes from the matcher type and specificity, and `lineProps://important` adds `10000`. File order only breaks ties between rules with the same priority.
2. **Protocol merge strategy second.** Routing protocols are exclusive and keep the first selected routing decision. Modification protocols may merge, and duplicate fields or whole-value targets usually let the later equal-priority rule replace the earlier one.

This behavior is covered by real `bifrost-e2e` tests: `priority_host_order` verifies routing first-win, while `priority_header_override`, `merge_reqheaders_same_key_override`, `priority_cookie_override`, `priority_urlparams_override`, and `priority_resbody_last_wins` verify later-wins modification behavior.

## Routing Rules

Routing rules choose the upstream target. After matcher priority sorting, only the first matching routing decision is used.

```txt
www.example.com host://server1.local
www.example.com host://server2.local
```

The request goes to `server1.local`.

Routing protocols include:

| Protocol | Behavior |
| --- | --- |
| `host`, `xhost` | Route to another host |
| `http`, `https`, `ws`, `wss` | Route with an explicit upstream scheme |
| `passthrough` | Select direct passthrough at this priority position |
| `proxy`, `pac` | Select an upstream proxy path |
| `redirect` | Return a redirect response |
| `file`, `tpl`, `rawfile` | Return local/template/mock content |

Host-family routing (`host` / `xhost` / `http` / `https` / `ws` / `wss`) wins over `proxy` in the final forwarding decision. For same-priority duplicate routing rules of the same type, the earlier rule wins.

## Modification Rules

Modification rules can stack. Different fields are merged; duplicate fields or duplicate whole-value targets follow the protocol strategy below.

| Protocols | Duplicate behavior |
| --- | --- |
| `reqHeaders`, `resHeaders` | Same header key: later equal-priority rule wins |
| `reqCookies`, `resCookies` | Same cookie key: later equal-priority rule wins |
| `urlParams`, `params`, `trailers` | Same field/key: later equal-priority rule wins |
| `reqBody`, `resBody`, `reqPrepend`, `resPrepend`, `reqAppend`, `resAppend`, `resMerge`, `reqCors`, `resCors` | Whole value: later equal-priority rule wins |
| `statusCode`, `method`, `auth`, `ua`, `referer`, `cache`, `attachment` | Single-match protocol: first selected rule wins |

Example: duplicate request headers use later-wins semantics for the same key.

```txt
www.example.com reqHeaders://(X-Custom:value1)
www.example.com reqHeaders://(X-Custom:value2)
```

The final request header is `X-Custom: value2`.

Different keys merge:

```txt
www.example.com reqHeaders://(X-Header-A:valueA)
www.example.com reqHeaders://(X-Header-B:valueB)
```

The request includes both `X-Header-A: valueA` and `X-Header-B: valueB`.

Whole-value body modifications use later-wins semantics:

```txt
www.example.com resBody://(body1)
www.example.com resBody://(body2)
```

The final response body is `body2`.

## Priority Before File Order

File order only decides between equal-priority rules. A more specific matcher still wins even if it appears earlier or later than a broader matcher.

```txt
www.example.com/api/internal/ reqHeaders://(X-Env:narrow)
www.example.com/api/ reqHeaders://(X-Env:broad)
```

For `/api/internal/...`, the narrower matcher has higher priority, so `X-Env:narrow` wins. For other `/api/...` paths, the broader rule still applies. This is the "partially effective" case shown in the Web UI merged-rule analysis.

## Summary

- Routing and other single-match protocols are first-win after priority sorting.
- Keyed modification protocols merge different keys and use later-wins for duplicate keys at the same priority.
- Whole-value modification protocols such as `resBody` use later-wins at the same priority.
- Matcher priority always comes before file order.
