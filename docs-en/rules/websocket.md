> Language: **English** | [中文](../../docs/rules/websocket.md)

# WebSocket Rules

WebSocket rules route WebSocket traffic.

```txt
ws://www.example.com/socket ws://ws-server.example.com/socket
wss://www.example.com/chat ws://internal-ws.example.com/chat
```

WebSocket routing applies to `ws://` and `wss://` upgrade requests. It does not transform ordinary HTTP/HTTPS tunnel requests into WebSocket traffic.

There is no `<...>` / `(...)` path-pinning feature for `ws`/`wss`: wrapping the target path in angle brackets breaks routing (502), and parentheses are silently stripped while path concatenation still happens. A non-intercepted (passthrough) CONNECT tunnel ignores ws/wss rules, but once a tunnel is TLS-intercepted, WebSocket upgrades inside it are matched and rewritten.
