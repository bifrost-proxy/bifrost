#!/usr/bin/env python3
"""Deterministic SSE fixture for response stream script E2E tests."""

import argparse
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer


MAX_EVENT_BYTES = 16 * 1024 * 1024


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, format, *args):
        return

    def _headers(self, content_type="text/event-stream", content_encoding=None):
        self.send_response(200)
        self.send_header("Content-Type", content_type)
        if isinstance(content_encoding, tuple):
            for encoding in content_encoding:
                self.send_header("Content-Encoding", encoding)
        elif content_encoding is not None:
            self.send_header("Content-Encoding", content_encoding)
        self.send_header("Cache-Control", "no-cache")
        self.send_header("Connection", "close")
        self.end_headers()

    def _write(self, payload, fragment_size=64 * 1024):
        for offset in range(0, len(payload), fragment_size):
            self.wfile.write(payload[offset : offset + fragment_size])
            self.wfile.flush()

    def do_GET(self):
        try:
            if self.path == "/json":
                self._headers("application/json")
                self._write(b'{"kind":"not-sse"}')
            elif self.path == "/encoded":
                self._headers(content_encoding="gzip")
                self._write(b"data: encoded\n\n")
            elif self.path == "/identity":
                self._headers(content_encoding=("identity", "identity"))
            else:
                self._headers()

            if self.path == "/stream":
                self._write(b"data: first\n\n")
                time.sleep(0.4)
                self._write(b"data: second\n\n")
                time.sleep(3.0)
            elif self.path == "/large":
                payload = b"x" * (MAX_EVENT_BYTES - 4096)
                self._write(b"data: " + payload + b"\n\n")
                time.sleep(2.0)
            elif self.path == "/oversize":
                payload = b"z" * (MAX_EVENT_BYTES + 1)
                self._write(b"data: " + payload + b"\n\n")
            elif self.path == "/tail":
                self._write(b"data: tail")
            elif self.path in {"/json", "/encoded"}:
                pass
            else:
                self._write(b"data: ready\n\n")
        except (BrokenPipeError, ConnectionResetError):
            pass

    def do_POST(self):
        self.do_GET()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--port", type=int, required=True)
    args = parser.parse_args()
    ThreadingHTTPServer(("127.0.0.1", args.port), Handler).serve_forever()


if __name__ == "__main__":
    main()
