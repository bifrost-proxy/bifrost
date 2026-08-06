> Language: **English** | [中文](../../docs/rules/scripts.md)

# Script Rules

Script rules attach QuickJS scripts to request, response, streaming response, decode, or binary parser phases.

```txt
example.com reqScript://{my_request_script}
example.com resScript://{my_response_script}
example.com resStreamScript://{my_stream_script}
example.com decode://utf8
example.com bp://my-parser decode://bp

```my_request_script
request.headers["X-Trace-Id"] = ctx.requestId;
```

```my_response_script
response.headers["X-Processed-By"] = "bifrost";
```

```my_stream_script
stream.mode = "transform";
stream.onEvent = (event) => ({ event: event.event, data: event.data });
```
```

For rules created by a CLI or AI, prefer inline blocks like the example above. The rule remains self-contained and can be copied or shared without a separate script file. The Scripts page remains useful for interactive testing, manual editing, and locally reusable named scripts.

Named scripts are still supported under `~/.bifrost/scripts/<dir>/<name>.js`, where the on-disk directory is `request`, `response`, `decode`, or `parser` — not the protocol prefix. The mapping is `reqScript://` → `scripts/request/`, `resScript://` and `resStreamScript://` → `scripts/response/`, `decode://` → `scripts/decode/`, and `bp://` (parser) → `scripts/parser/`.

## True SSE streaming with resStreamScript

`resStreamScript` creates one persistent QuickJS context per HTTP response. State and closures persist between events in that response but are isolated from other requests. The worker lives for the entire HTTP response. `timeout_ms` applies independently to each JavaScript callback; waiting 10 or 20 minutes for the next upstream SSE event does not consume that callback timeout.

Transform an upstream SSE stream event by event:

```javascript
stream.mode = "transform";
let index = 0;

stream.onEvent = (event) => ({
  event: "mapped.delta",
  data: JSON.stringify({ index: ++index, upstream: event.data }),
});

stream.onEnd = () => "data: [DONE]\n\n";
```

Bifrost buffers only an incomplete SSE event. As soon as it sees `\n\n` or `\r\n\r\n`, it invokes `stream.onEvent(event)` and forwards the returned output immediately. It never waits for upstream EOF or `[DONE]`.

Generate a mock stream one step at a time:

```javascript
stream.mode = "mock";
let index = 0;

stream.next = () => ({
  output: { event: "mock.delta", data: String(++index) },
  delayMs: 100,
  done: index === 3,
});
```

Each callback may return a raw SSE string, an event object `{ id?, event?, data, retry? }`, an array of those values, a step object `{ output }` or `{ outputs: [...] }` with optional `delayMs` and `done`, or `null`/`undefined` for no output.

Backpressure is propagated through a bounded downstream queue. Arbitrarily split network body frames are reassembled without truncation. A complete SSE event may be up to 16 MiB; an oversized event produces an explicit SSE error and terminates the stream rather than silently truncating or dropping bytes. Use `resScript` only for whole-body response changes, because it collects the body; use `resStreamScript` for realtime SSE. One matched response currently supports one stateful stream script.
