> Language: **English** | [中文](../../docs/rules/websocket.md)

# WebSocket Rules

WebSocket rules route WebSocket traffic.

```txt
ws://www.example.com/socket ws://ws-server.example.com/socket
wss://www.example.com/chat ws://internal-ws.example.com/chat
```

WebSocket routing applies to `ws://` and `wss://` requests. It does not transform ordinary HTTP/HTTPS tunnel requests into WebSocket traffic. Use angle brackets or parentheses around the target path to disable automatic path concatenation.
