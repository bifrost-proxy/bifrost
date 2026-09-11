#!/usr/bin/env python3
import base64
import gzip
import http.client
import http.server
import itertools
import json
import os
from pathlib import Path
import signal
import socket
import subprocess
import tempfile
import threading
import time

ROOT = Path(__file__).resolve().parents[2]
BIN = Path(os.environ.get("BIFROST_BIN", ROOT / "target/debug/bifrost"))
ENV = dict(os.environ, BIFROST_DISABLE_TRAY="1", BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT="1", CI="1", NO_PROXY="*", no_proxy="*")
RESULTS = []


class Fixture(http.server.BaseHTTPRequestHandler):
    def log_message(self, *_):
        pass

    def handle_request(self):
        raw = self.rfile.read(int(self.headers.get("Content-Length", 0)))
        if self.headers.get("Content-Encoding") == "gzip":
            raw = gzip.decompress(raw)
        try:
            body = json.loads(raw)
        except (ValueError, UnicodeDecodeError):
            body = None
        status = int(self.path.split("/")[2]) if self.path.startswith("/status/") else 200
        payload = json.dumps({"path": self.path, "json": body, "reply": "ResponseOnly", "errno": 3}).encode()
        content_type = "application/json"
        if self.path.startswith("/sse"):
            content_type = "text/event-stream"
            payload = b'data: {"text":"StreamNeedle"}\n\ndata: [DONE]\n\n'
        self.send_response(status)
        self.send_header("Content-Type", content_type)
        self.send_header("X-Response-Marker", "ResponseHeader")
        self.send_header("Content-Length", str(len(payload)))
        self.end_headers()
        if self.command != "HEAD":
            self.wfile.write(payload)

    do_GET = do_POST = do_PUT = do_DELETE = do_PATCH = do_HEAD = do_OPTIONS = do_TRACE = handle_request


def free_port():
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def check(name, action):
    try:
        action()
        RESULTS.append({"name": name, "ok": True})
        print("PASS", name, flush=True)
    except Exception as error:
        RESULTS.append({"name": name, "ok": False, "error": str(error)})
        print("FAIL", name, str(error)[:700], flush=True)


def require(condition, message):
    if not condition:
        raise AssertionError(message)


def run(port, *args, success=True):
    command = [str(BIN), "-p", str(port), *map(str, args)]
    result = subprocess.run(command, env=ENV, input="", text=True, capture_output=True, timeout=20)
    require((result.returncode == 0) == success, f"{' '.join(command[3:])}: exit={result.returncode}; {result.stderr[:500]} {result.stdout[:300]}")
    return result.stdout


def load(port, *args):
    return json.loads(run(port, *args, "--format", "json"))


def record_ids(payload):
    return {r["id"] for r in payload.get("records", payload.get("results", []))}


def expect_ids(port, args, expected):
    result = load(port, *args)
    actual = record_ids(result)
    require(actual == set(expected), f"{args}: expected={sorted(expected)}, actual={sorted(actual)}")


def exercise(port, upstream):
    specifications = [("GET", p, None) for p in ["/api/a_b", "/api/axb", "/api/a%25b", "/api/a*b", "/api/UPPER", "/api/%E4%B8%AD%E6%96%87", "/api/query?q=a+b&tag=x%26y", "/api/quote'and%22", "/api/slash/", "/api/slash", "/status/204", "/status/302", "/status/404", "/status/500", "/sse"]]
    obj = {"user": {"id": 42, "name": "Alice", "active": True, "empty": "", "nil": None}, "items": [{"id": 7}, {"id": 9}], "x-y": {"a_b": "A=B"}, "text": "RequestOnly", "padding": "x" * 70000}
    specifications += [("POST", "/json/object", obj), ("POST", "/json/array", [{"id": 42}, {"id": 7}]), ("POST", "/json/scalar", 42), ("POST", "/json/gzip", obj)]
    specifications += [(m, "/method/" + m, {"method": m}) for m in ["PUT", "DELETE", "PATCH", "HEAD", "OPTIONS", "TRACE"]]
    start_ms = int(time.time() * 1000)
    for method, path, body in specifications:
        headers = {"X-Request-Marker": "RequestHeader", "X-Equals": "A=B"}
        payload = json.dumps(body).encode() if body is not None else None
        if payload is not None:
            headers["Content-Type"] = "application/json"
        if path.endswith("/gzip"):
            payload = gzip.compress(payload)
            headers["Content-Encoding"] = "gzip"
        conn = http.client.HTTPConnection("127.0.0.1", port, timeout=15)
        conn.request(method, f"http://127.0.0.1:{upstream}{path}", payload, headers)
        response = conn.getresponse()
        response.read()
        conn.close()
    deadline = time.monotonic() + 20
    while True:
        records = load(port, "traffic", "list", "--limit", 200)["records"]
        if len(records) == len(specifications):
            break
        require(time.monotonic() < deadline, "traffic capture did not settle")
        time.sleep(0.1)
    by_path = {r["p"]: r for r in records}
    all_ids = {r["id"] for r in records}
    pick = lambda predicate: {r["id"] for r in records if predicate(r)}
    search = lambda *args: ["search", "/", "--url", *args]
    check("capture count and unique paths", lambda: require(len(by_path) == len(specifications), "missing fixture paths"))
    for method in ["GET", "POST", "PUT", "DELETE", "PATCH", "HEAD", "OPTIONS", "TRACE", "CONNECT"]:
        expected = pick(lambda r: r["m"] == method)
        check(f"list method {method}", lambda m=method, e=expected: expect_ids(port, ["traffic", "list", "--method", m], e))
        check(f"search method {method}", lambda m=method, e=expected: expect_ids(port, search("--method", m), e))
    for fragment in ["/api/", "a_b", "%25", "*", "UPPER", "%E4%B8%AD", "q=a+b&tag=x%26y", "quote'", "/api/slash/", "/api/slash", "not-present", "_"]:
        expected = pick(lambda r: fragment.lower() in r["p"].lower())
        check(f"list literal path {fragment}", lambda f=fragment, e=expected: expect_ids(port, ["traffic", "list", "--path", f], e))
        check(f"search literal path {fragment}", lambda f=fragment, e=expected: expect_ids(port, search("--path", f), e))
        check(f"list URL + path {fragment}", lambda f=fragment, e=expected: expect_ids(port, ["traffic", "list", "--url", "/api", "--path", f], e & pick(lambda r: "/api" in r["p"])))
    for status in [200, 204, 302, 404, 500, 418]:
        check(f"list status {status}", lambda s=status: expect_ids(port, ["traffic", "list", "--status", s], pick(lambda r: r["s"] == s)))
    for low, high in [(200, 299), (300, 399), (400, 599), (500, 400)]:
        check(f"list status range {low}-{high}", lambda lo=low, hi=high: expect_ids(port, ["traffic", "list", "--status-min", lo, "--status-max", hi], pick(lambda r: lo <= r["s"] <= hi)))
    for status, predicate in [("2xx", lambda r: 200 <= r["s"] < 300), ("3xx", lambda r: 300 <= r["s"] < 400), ("4xx", lambda r: 400 <= r["s"] < 500), ("5xx", lambda r: 500 <= r["s"] < 600), ("error", lambda r: r["s"] == 0 or r["s"] >= 500)]:
        check(f"search status {status}", lambda s=status, p=predicate: expect_ids(port, search("--status", s), pick(p)))
    for flag, value, expected in [("--host", "127.0.0.1", all_ids), ("--host", "no-host.invalid", set()), ("--client-ip", "127.0.0.1", all_ids), ("--client-app", "no-such-application", set()), ("--protocol", "http", all_ids), ("--protocol", "https", set()), ("--content-type", "text/event-stream", {by_path["/sse"]["id"]}), ("--listener-port", port, all_ids), ("--proxy-port", port, all_ids), ("--listener-port", upstream, set()), ("--has-rule-hit", "false", all_ids), ("--has-rule-hit", "true", set()), ("--is-websocket", "false", all_ids), ("--is-websocket", "true", set()), ("--is-sse", "true", {by_path["/sse"]["id"]}), ("--is-sse", "false", all_ids - {by_path["/sse"]["id"]}), ("--is-tunnel", "false", all_ids), ("--is-tunnel", "true", set())]:
        check(f"list {flag}={value}", lambda f=flag, v=value, e=expected: expect_ids(port, ["traffic", "list", f, v], e))
    dimensions = [("--method", "POST"), ("--path", "/json/object"), ("--host", "127.0.0.1"), ("--listener-port", str(port))]
    for count in range(2, 5):
        for combo in itertools.combinations(dimensions, count):
            args = [x for pair in combo for x in pair]
            expected = {by_path["/json/object"]["id"]} if ("--path", "/json/object") in combo else pick(lambda r: r["m"] == "POST") if ("--method", "POST") in combo else all_ids
            check(f"list combination {args}", lambda a=args, e=expected: expect_ids(port, ["traffic", "list", *a], e))
            check(f"search combination {args}", lambda a=args, e=expected: expect_ids(port, search(*a), e))
    def newest_first(args, maximum=None):
        result = load(port, *args)
        rows = result.get("records", result.get("results", []))
        timestamps = [r.get("ts", r.get("timestamp")) for r in rows]
        require(timestamps == sorted(timestamps, reverse=True), f"not newest first: {timestamps}")
        expected = sorted(records, key=lambda r: (r["ts"], r["seq"]), reverse=True)
        if maximum is not None:
            expected = expected[:maximum]
        require([r["id"] for r in rows] == [r["id"] for r in expected], "newest records not prioritized")
    check("list newest first", lambda: newest_first(["traffic", "list"]))
    check("search newest first", lambda: newest_first(search()))
    for flag in ["--limit", "--max-results", "--max-scan"]:
        check(f"search newest first with {flag}", lambda f=flag: newest_first(search(f, 3), 3))
    check("list newest first with limit", lambda: newest_first(["traffic", "list", "--limit", 3], 3))
    for direction in ["backward", "forward"]:
        def paginate(d=direction):
            cursor = None
            seen = []
            for _ in range(len(records) + 2):
                args = ["traffic", "list", "--limit", "3", "--direction", d]
                if cursor is not None:
                    args += ["--cursor", str(cursor)]
                page = load(port, *args)
                seen += [r["id"] for r in page["records"]]
                if not page["has_more"]:
                    break
                cursor = page["next_cursor"]
            require(len(seen) == len(all_ids) and set(seen) == all_ids, f"pagination {d}: duplicate/missing records ({len(seen)} vs {len(all_ids)})")
        check(f"pagination {direction} exact union", paginate)
    for limit in [len(records), len(records) + 1]:
        check(f"list exact last page {limit}", lambda n=limit: require(not load(port, "traffic", "list", "--limit", n)["has_more"], "phantom next page"))
        check(f"search exact last page {limit}", lambda n=limit: require(not load(port, *search("--limit", n))["has_more"], "phantom next page"))
    check("search truncated has more", lambda: require(load(port, *search("--limit", 2))["has_more"], "lost remaining candidates"))
    check("search scan cap has more", lambda: require(load(port, *search("--max-scan", 2))["has_more"], "lost remaining scan candidates"))
    check("explicit max-results overrides limit", lambda: newest_first(search("--limit", 1, "--max-results", 3), 3))
    for limit in [1, 2, 7]:
        check(f"list limit {limit}", lambda n=limit: require(len(load(port, "traffic", "list", "--limit", n)["records"]) == n, "limit ignored"))
        check(f"search limit {limit}", lambda n=limit: require(len(load(port, *search("--limit", n))["results"]) == n, "--limit ignored"))
        check(f"search max-results {limit}", lambda n=limit: require(len(load(port, *search("--max-results", n))["results"]) == n, "max-results ignored"))
        check(f"search max-scan {limit}", lambda n=limit: require(load(port, *search("--max-scan", n))["total_searched"] == n, "max-scan ignored"))
    json_ids = {by_path[p]["id"] for p in ["/json/object", "/json/gzip"]}
    cases = [("$.user.id=42", json_ids), ("user.id=42", json_ids), ("$.user.name=Alice", json_ids), ("$.user.active=true", json_ids), ("$.user.nil=null", json_ids), ("$.user.empty=", json_ids), ("$.items[0].id=7", json_ids), ("$.items[*].id=9", json_ids), ("$.x-y.a_b=A=B", json_ids), ("$.missing=42", set()), ("$[0].id=42", {by_path["/json/array"]["id"]}), ("$=42", {by_path["/json/scalar"]["id"]})]
    for expression, expected in cases:
        check(f"request JSONPath {expression}", lambda x=expression, e=expected: expect_ids(port, search("--req-json", x), e))
    for expression in ["$.json.user.id=42", "$.json.items[*].id=9"]:
        check(f"response JSONPath {expression}", lambda x=expression: expect_ids(port, search("--res-json", x), json_ids))
    check("multiple JSONPath AND", lambda: expect_ids(port, search("--req-json", "$.user.id=42", "--req-json", "$.items[1].id=9", "--res-json", "$.errno=3"), json_ids))
    check("conflicting JSONPath AND", lambda: expect_ids(port, search("--req-json", "$.user.id=42", "--req-json", "$.user.id=43"), set()))
    check("filter-only no keyword", lambda: expect_ids(port, ["search", "--req-json", "$.user.id=42"], json_ids))
    check("traffic search filter-only no keyword", lambda: expect_ids(port, ["traffic", "search", "--path", "/json/object"], {by_path["/json/object"]["id"]}))
    for flag, expression in [("--req-header-eq", "x-request-marker=RequestHeader"), ("--req-header-eq", "X-Equals=A=B"), ("--res-header-eq", "x-response-marker=ResponseHeader")]:
        check(f"header equals {expression}", lambda f=flag, x=expression: expect_ids(port, search(f, x), all_ids))
    for keyword, flag, expected in [("RequestOnly", "--req-body", json_ids), ("ResponseOnly", "--req-body", set()), ("RequestOnly", "--body", json_ids), ("RequestHeader", "--req-header", all_ids), ("RequestHeader", "--res-header", set()), ("ResponseHeader", "--res-header", all_ids), ("StreamNeedle", "--res-body", {by_path["/sse"]["id"]})]:
        check(f"scope {flag} {keyword}", lambda k=keyword, f=flag, e=expected: expect_ids(port, ["search", k, f], e))
    for entry in ["30s", "5m", "2h", "1d", "1w"]:
        check(f"latest {entry}", lambda x=entry: expect_ids(port, search("--latest", x), all_ids))
    check("timestamp bounds", lambda: expect_ids(port, search("--since", start_ms, "--until", int(time.time() * 1000)), all_ids))
    check("empty old time window", lambda: expect_ids(port, search("--until", "1970-01-01T00:00:00Z", "--max-scan", 1), set()))
    check("reversed time window", lambda: expect_ids(port, search("--since", "2030-01-01T00:00:00Z", "--until", "2020-01-01T00:00:00Z"), set()))
    target = by_path["/json/object"]
    for fmt in ["json", "json-pretty", "ndjson", "table", "compact"]:
        def include(f=fmt):
            output = run(port, "search", "", "--path", "/json/object", "--include", "bodies,headers", "--max-body", 32, "--format", f, "--no-color")
            if f in ["table", "compact"]:
                require("/json/object" in output, "missing result")
                require("\x1b[" not in output, "unexpected ANSI")
                return
            result = next(x for x in map(json.loads, output.splitlines()) if x.get("type") == "result") if f == "ndjson" else json.loads(output)["results"][0]
            for side in ["request", "response"]:
                chunk = result["bodies"][side]
                require(len(base64.b64decode(chunk["bytes_b64"])) == 32 and chunk["truncated"], "body cap ignored")
                require(isinstance(result["headers"][side], list), "missing headers")
        check(f"include bodies headers {fmt}", include)
    for fmt in ["json", "json-pretty", "table", "compact", "ndjson"]:
        def get_single(f=fmt):
            output = run(port, "traffic", "get", target["id"], "--request-body", "--response-body", "--format", f)
            require("RequestOnly" in output, "missing captured body")
            if f in ["json", "json-pretty", "ndjson"]:
                require(json.loads(output)["id"] == target["id"], "wrong record")
        check(f"single get format {fmt}", get_single)
    check("single get default JSON", lambda: require(json.loads(run(port, "traffic", "get", target["id"]))["id"] == target["id"], "wrong default format"))
    check("single get full sequence", lambda: require(load(port, "traffic", "get", str(target["seq"]))["id"] == target["id"], "wrong sequence resolution"))
    for use_seq in [False, True]:
        for fmt in ["json", "json-pretty", "ndjson"]:
            def batch(s=use_seq, f=fmt):
                ids = [str(by_path[p]["seq"] if s else by_path[p]["id"]) for p in ["/json/object", "/json/array"]] + ["not-found"]
                output = run(port, "traffic", "get", "--ids", ",".join(ids), "--request-body", "--max-body", 16, "--format", f)
                data = [json.loads(line) for line in output.splitlines()] if f == "ndjson" else json.loads(output)["results"]
                require(len(data) == 3, "wrong batch length")
                require(all(r["ok"] for r in data[:2]), f"existing records not resolved: {[r.get('error') for r in data]}")
                require(data[-1]["error"] == "not_found", "missing id should be per-item error")
                require(len(base64.b64decode(data[0]["bodies"]["request"]["bytes_b64"])) == 16, "batch cap ignored")
            check(f"batch sequence={use_seq} format={fmt}", batch)
    check("batch default NDJSON", lambda: require(len([json.loads(line) for line in run(port, "traffic", "get", "--ids", target["id"] + ",not-found").splitlines()]) == 2, "default is not NDJSON"))
    for prefix in [["search", "/"], ["traffic", "search", "/"]]:
        for flag, value in [("--since", "yesterday-invalid"), ("--until", "bad-time"), ("--latest", "9years"), ("--req-json", "$.user.id"), ("--req-json", "$.=42"), ("--req-json", "$..user.id=42"), ("--res-json", "$[oops]=1"), ("--req-header-eq", "missing-equals"), ("--res-header-eq", "=empty-name"), ("--include", "bodise")]:
            check(f"reject {' '.join(prefix)} {flag} {value}", lambda p=prefix, f=flag, v=value: run(port, *p, f, v, "--format", "json", success=False))
    for args in [["traffic", "list", "--direction", "sideways"], ["traffic", "list", "--method", "get"], ["traffic", "get", target["id"], "--ids", target["id"]], ["traffic", "get", "--ids"], ["traffic", "get", "--ids", ","], ["traffic", "get", "--ids", ",".join(str(x) for x in range(201))], ["traffic", "get", "not-found"]]:
        check(f"reject arguments {str(args)[:85]}", lambda a=args: run(port, *a, success=False))
    for fmt in ["table", "compact"]:
        check(f"no-color empty search {fmt}", lambda f=fmt: require("\x1b[" not in run(port, "search", "NO-SUCH-KEYWORD", "--no-color", "--format", f), "ANSI emitted despite --no-color"))
    check("aliases exact result identity", lambda: require(record_ids(load(port, "search", "RequestOnly")) == record_ids(load(port, "traffic", "search", "RequestOnly")), "aliases differ"))
    for args in [["traffic", "list"], ["search", "test"], ["traffic", "get", "not-found"]]:
        check(f"offline {' '.join(args)}", lambda a=args: run(free_port(), *a, success=False))


def main():
    if os.environ.get("SKIP_BUILD") != "true":
        subprocess.run(["cargo", "build", "--bin", "bifrost"], cwd=ROOT, env=ENV, check=True)
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Fixture)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    port = free_port()
    require(port != 9900, "protected production port")
    with tempfile.TemporaryDirectory(prefix=".bifrost-e2e-traffic-matrix-", dir=ROOT) as directory:
        ENV["BIFROST_DATA_DIR"] = directory
        with open(Path(directory) / "proxy.log", "w+") as log:
            process = subprocess.Popen([str(BIN), "-p", str(port), "start", "--host", "127.0.0.1", "--skip-cert-check", "--no-system-proxy"], env=ENV, stdout=log, stderr=log, start_new_session=True)
            try:
                deadline = time.monotonic() + 60
                while True:
                    try:
                        conn = http.client.HTTPConnection("127.0.0.1", port, timeout=1)
                        conn.request("GET", "/_bifrost/api/system")
                        ready = conn.getresponse().status == 200
                        conn.close()
                        if ready:
                            break
                    except OSError:
                        pass
                    require(process.poll() is None and time.monotonic() < deadline, "isolated proxy failed to start")
                    time.sleep(0.2)
                exercise(port, server.server_port)
            finally:
                os.killpg(process.pid, signal.SIGTERM)
                try:
                    process.wait(timeout=15)
                except subprocess.TimeoutExpired:
                    os.killpg(process.pid, signal.SIGKILL)
                    process.wait(timeout=5)
                server.shutdown()
                server.server_close()
    failed = [r for r in RESULTS if not r["ok"]]
    print(f"SUMMARY passed={len(RESULTS) - len(failed)} failed={len(failed)} total={len(RESULTS)}")
    if os.environ.get("RESULT_FILE"):
        Path(os.environ["RESULT_FILE"]).write_text(json.dumps(RESULTS, indent=2) + "\n")
    raise SystemExit(bool(failed))


if __name__ == "__main__":
    main()
