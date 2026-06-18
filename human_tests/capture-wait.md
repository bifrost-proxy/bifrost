# Capture Wait — `bifrost capture wait`

Verifies the long-poll capture API and CLI: client posts a filter, server holds
the connection until a matching traffic record arrives (or timeout expires).

## Preconditions

- Start bifrost locally: `bifrost start --port 9900 --no-system-proxy`.
- Have `curl` available (used to generate traffic).
- All commands below run from the repository root.

## Case 1 — Hit before timeout

Goal: a request matching the filter is captured within the timeout window;
CLI exits 0 and prints a one-line summary in human mode.

Terminal A — start the waiter:

```sh
bifrost capture wait \
  --host example.com \
  --method GET \
  --path /api \
  --timeout 30s
```

Expected (terminal A immediately):

```
Waiting for GET example.com /api (timeout 30s)...
```

Terminal B — drive a matching request through bifrost:

```sh
curl -s -x http://127.0.0.1:9900 https://example.com/api/ping
```

Expected (terminal A within ~1s):

```
[<elapsed>s] Captured request id=<uuid> host=example.com method=GET path=/api/ping
```

Verify: `echo $?` reports `0` in terminal A.

## Case 2 — Timeout with no match

Goal: when no matching traffic arrives, the CLI exits 124 with a
human-readable timeout banner.

Run:

```sh
bifrost capture wait \
  --host this-domain-never-resolves.test \
  --timeout 3s
```

Expected stderr (after ~3s):

```
Waiting for * this-domain-never-resolves.test * (timeout 3s)...
Timed out after 3.0s, scanned <N> records
```

Verify: `echo $?` reports `124`.

## Case 3 — JSON mode and `--open`

Goal: in JSON mode the raw server response is printed verbatim, and `--open`
launches the OS opener before the wait begins. Opener failures only emit a
stderr warning and never block the wait.

Run (replace `--open` URL with anything reachable; failures are tolerated):

```sh
bifrost capture wait \
  --path /healthz \
  --timeout 5s \
  --open "https://httpbin.org/anything" \
  --format json
```

Then in another terminal trigger a matching request:

```sh
curl -s -x http://127.0.0.1:9900 https://httpbin.org/healthz
```

Expected (terminal A): the OS opener launches the URL (browser tab, or a
stderr warning like `warning: failed to launch opener ...` on headless boxes),
followed by exactly one JSON line on stdout, e.g.:

```
{"matched":true,"record":{"id":"...","host":"httpbin.org","method":"GET","path":"/healthz",...},"waited_ms":482,"scanned_count":1}
```

Verify: `echo $?` reports `0`. If `--open` cannot launch (no opener installed),
the warning appears on stderr but the wait still completes normally.