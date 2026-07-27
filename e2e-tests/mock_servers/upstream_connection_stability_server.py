#!/usr/bin/env python3
"""Threaded HTTP fixture for upstream connection backpressure tests."""

import http.server
import json
import threading
import time
import urllib.parse
import sys


class Counters:
    def __init__(self):
        self._lock = threading.Lock()
        self.active = 0
        self.peak = 0
        self.total = 0

    def begin(self):
        with self._lock:
            self.active += 1
            self.total += 1
            self.peak = max(self.peak, self.active)

    def end(self):
        with self._lock:
            self.active -= 1

    def reset(self):
        with self._lock:
            self.active = 0
            self.peak = 0
            self.total = 0

    def snapshot(self):
        with self._lock:
            return {
                "active": self.active,
                "peak": self.peak,
                "total": self.total,
            }


COUNTERS = Counters()


class StabilityHandler(http.server.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, _format, *_args):
        pass

    def _send_json(self, payload, status=200):
        body = json.dumps(payload, separators=(",", ":")).encode("utf-8")
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Connection", "close")
        self.end_headers()
        self.wfile.write(body)
        self.close_connection = True

    def do_GET(self):
        parsed = urllib.parse.urlparse(self.path)
        if parsed.path == "/stats":
            self._send_json(COUNTERS.snapshot())
            return
        if parsed.path == "/reset":
            COUNTERS.reset()
            self._send_json(COUNTERS.snapshot())
            return

        query = urllib.parse.parse_qs(parsed.query)
        delay_ms = int(query.get("delay_ms", ["120"])[0])
        request_id = query.get("id", [""])[0]
        COUNTERS.begin()
        try:
            time.sleep(max(delay_ms, 0) / 1000)
            self._send_json(
                {
                    "ok": True,
                    "id": request_id,
                    "method": self.command,
                    "path": parsed.path,
                }
            )
        finally:
            COUNTERS.end()


class ThreadedServer(http.server.ThreadingHTTPServer):
    daemon_threads = True


def main():
    port = int(sys.argv[1]) if len(sys.argv) > 1 else 18080
    server = ThreadedServer(("127.0.0.1", port), StabilityHandler)
    print(f"READY {port}", flush=True)
    server.serve_forever()


if __name__ == "__main__":
    main()
