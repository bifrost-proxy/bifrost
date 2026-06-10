> Language: **English** | [中文](../../docs/rules/timing-throttle.md)

# Delay and Throttling Rules

Timing and throttling rules simulate slow networks and backend delays.

```txt
example.com reqDelay://1000
example.com resDelay://2000
example.com reqSpeed://10
example.com resSpeed://100
```

`reqDelay` delays before sending a request upstream. `resDelay` delays before returning a response to the client. `reqSpeed` and `resSpeed` limit upload or download throughput in KB/s.

Caveat: `reqDelay` only takes effect on TLS-intercepted (HTTPS) traffic; for plain HTTP requests it is currently a no-op. `resDelay`, `reqSpeed`, and `resSpeed` work on plain HTTP. Use HTTPS hosts (with TLS interception enabled) when relying on `reqDelay`.
