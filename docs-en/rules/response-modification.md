> Language: **English** | [中文](../../docs/rules/response-modification.md)

# Response Modification Rules

Response modification rules change upstream responses before the client receives them.

```txt
example.com resHeaders://X-Proxy=Bifrost
example.com resHeaders://(X-Proxy=Bifrost&X-Debug=1)
example.com statusCode://503
example.com replaceStatus://200
example.com resBody://({"ok":true})
example.com file://./fixtures/mock.json
```

Use `statusCode` to return a synthetic status without contacting upstream. Use `replaceStatus` when the upstream request should still be sent but the returned status should be changed. Use `file`, `tpl`, or `resBody` for mock bodies.

For single-line inline `resHeaders` maps, separate independent headers with
`&`. Use JSON or a multi-line/referenced Value when a header value itself must
contain a literal ampersand.
