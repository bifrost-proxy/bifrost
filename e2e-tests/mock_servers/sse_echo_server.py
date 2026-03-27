#!/usr/bin/env python3
"""
SSE Echo Server for testing bifrost SSE proxy functionality.

Endpoints:
- GET /sse - Stream 5 events at 0.5s intervals
- GET /sse/custom?count=N&interval=M - Custom event count and interval
- GET /sse/echo - Echo Last-Event-ID and continue from there
- POST /sse/trigger - Manual trigger endpoint (returns JSON)
- GET /health - Health check
"""

import argparse
import json
import sys
import time
import asyncio
from http.server import HTTPServer, BaseHTTPRequestHandler
from urllib.parse import urlparse, parse_qs
import threading

if sys.stdout and hasattr(sys.stdout, 'reconfigure'):
    try:
        sys.stdout.reconfigure(encoding='utf-8', errors='replace')
    except Exception:
        pass
if sys.stderr and hasattr(sys.stderr, 'reconfigure'):
    try:
        sys.stderr.reconfigure(encoding='utf-8', errors='replace')
    except Exception:
        pass


class SSEHandler(BaseHTTPRequestHandler):
    server_version = "SSEEchoServer/1.0"
    protocol_version = "HTTP/1.1"

    def log_message(self, format, *args):
        print(f"[SSE] {self.address_string()} - {format % args}")

    def send_sse_headers(self, keep_alive=False):
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Cache-Control", "no-cache")
        if keep_alive:
            self.send_header("Connection", "keep-alive")
        else:
            self.send_header("Connection", "close")
        self.send_header("X-Accel-Buffering", "no")
        self.send_header("Access-Control-Allow-Origin", "*")
        self.end_headers()

    def send_sse_event(self, data, event=None, id=None, retry=None):
        lines = []
        if id is not None:
            lines.append(f"id: {id}")
        if event is not None:
            lines.append(f"event: {event}")
        if retry is not None:
            lines.append(f"retry: {retry}")
        for line in str(data).split("\n"):
            lines.append(f"data: {line}")
        lines.append("")
        lines.append("")

        message = "\n".join(lines)
        try:
            self.wfile.write(message.encode("utf-8"))
            self.wfile.flush()
            return True
        except (BrokenPipeError, ConnectionResetError):
            return False

    def do_GET(self):
        parsed = urlparse(self.path)
        path = parsed.path
        query = parse_qs(parsed.query)

        if path == "/health":
            self.send_response(200)
            content = json.dumps({"status": "ok", "server": "sse"}).encode()
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(content)))
            self.send_header("Connection", "close")
            self.end_headers()
            self.wfile.write(content)
            return

        if path == "/sse":
            count = int(query.get("count", [5])[0])
            interval = float(query.get("interval", [0.5])[0])
            self.handle_sse_stream(count=count, interval=interval)
            return

        if path == "/sse/custom":
            count = int(query.get("count", [5])[0])
            interval = float(query.get("interval", [0.5])[0])
            event_type = query.get("event", [None])[0]
            self.handle_sse_stream(count=count, interval=interval, event_type=event_type)
            return

        if path == "/sse/echo":
            last_event_id = self.headers.get("Last-Event-ID")
            start_id = int(last_event_id) + 1 if last_event_id else 1
            self.handle_sse_stream(count=5, interval=0.5, start_id=start_id)
            return

        if path == "/sse/multiline":
            self.handle_multiline_sse()
            return

        if path == "/sse/json":
            self.handle_json_sse()
            return

        if path == "/sse/openai":
            self.handle_openai_like_sse()
            return

        if path == "/sse/openai-invalid":
            self.handle_invalid_openai_like_sse()
            return

        if path == "/sse/chunked":
            count = int(query.get("count", [3])[0])
            interval = float(query.get("interval", [0.05])[0])
            crlf = query.get("crlf", ["0"])[0] == "1"
            chunk = int(query.get("chunk", [3])[0])
            self.handle_chunked_sse(count=count, interval=interval, crlf=crlf, chunk=chunk)
            return

        self.send_error(404, "Not Found")

    def do_POST(self):
        parsed = urlparse(self.path)
        path = parsed.path

        if path == "/sse/trigger":
            content_length = int(self.headers.get("Content-Length", 0))
            body = self.rfile.read(content_length).decode("utf-8") if content_length > 0 else ""

            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Access-Control-Allow-Origin", "*")
            self.end_headers()

            response = {
                "triggered": True,
                "body_received": body,
                "timestamp": time.time()
            }
            self.wfile.write(json.dumps(response).encode())
            return

        self.send_error(404, "Not Found")

    def do_OPTIONS(self):
        self.send_response(200)
        self.send_header("Access-Control-Allow-Origin", "*")
        self.send_header("Access-Control-Allow-Methods", "GET, POST, OPTIONS")
        self.send_header("Access-Control-Allow-Headers", "Content-Type, Last-Event-ID")
        self.end_headers()

    def handle_sse_stream(self, count=5, interval=0.5, event_type=None, start_id=1):
        self.send_sse_headers()

        for i in range(count):
            event_id = start_id + i
            data = {
                "message": f"Event {event_id}",
                "index": i,
                "total": count,
                "timestamp": time.time()
            }

            if not self.send_sse_event(
                data=json.dumps(data),
                event=event_type or "message",
                id=str(event_id)
            ):
                print(f"[SSE] Client disconnected at event {event_id}")
                return

            if i < count - 1:
                time.sleep(interval)

        self.send_sse_event(
            data=json.dumps({"done": True, "total_events": count}),
            event="done"
        )
        self.wfile.flush()

    def handle_multiline_sse(self):
        self.send_sse_headers()

        multiline_data = "Line 1: Hello\nLine 2: World\nLine 3: Multiline SSE Test"
        self.send_sse_event(data=multiline_data, event="multiline", id="1")

        time.sleep(0.3)

        self.send_sse_event(
            data="Single line after multiline",
            event="single",
            id="2"
        )

        self.send_sse_event(data=json.dumps({"done": True}), event="done")

    def handle_json_sse(self):
        self.send_sse_headers()

        events = [
            {"type": "start", "payload": {"session_id": "abc123"}},
            {"type": "data", "payload": {"items": [1, 2, 3], "nested": {"key": "value"}}},
            {"type": "update", "payload": {"progress": 50, "status": "processing"}},
            {"type": "end", "payload": {"result": "success", "duration_ms": 1234}}
        ]

        for i, event in enumerate(events):
            if not self.send_sse_event(
                data=json.dumps(event),
                event=event["type"],
                id=str(i + 1)
            ):
                return
            time.sleep(0.2)

    def handle_openai_like_sse(self):
        self.send_sse_headers()

        chunks = [
            {
                "id": "chatcmpl-e2e",
                "object": "chat.completion.chunk",
                "model": "kimi-k2.5",
                "choices": [
                    {
                        "index": 0,
                        "delta": {
                            "role": "assistant",
                            "reasoning_content": "reason",
                            "content": "search"
                        },
                        "finish_reason": None
                    }
                ]
            },
            {
                "id": "chatcmpl-e2e",
                "object": "chat.completion.chunk",
                "model": "kimi-k2.5",
                "choices": [
                    {
                        "index": 0,
                        "delta": {
                            "reasoning_content": "ing-e2e",
                            "content": "able-",
                            "tool_calls": [
                                {
                                    "index": 0,
                                    "function": {
                                        "name": "read_file",
                                        "arguments": "{\"path\":\"/tmp/search-e2e\""
                                    }
                                }
                            ]
                        },
                        "finish_reason": None
                    }
                ],
                "usage": {"total_tokens": 12}
            },
            {
                "id": "chatcmpl-e2e",
                "object": "chat.completion.chunk",
                "model": "kimi-k2.5",
                "choices": [
                    {
                        "index": 0,
                        "delta": {
                            "content": "content",
                            "tool_calls": [
                                {
                                    "index": 0,
                                    "function": {
                                        "arguments": "}"
                                    }
                                }
                            ]
                        },
                        "finish_reason": "tool_calls"
                    }
                ]
            }
        ]

        for i, chunk in enumerate(chunks):
            if not self.send_sse_event(data=json.dumps(chunk), event="message", id=str(i + 1)):
                return
            time.sleep(0.05)

        self.send_sse_event(data="[DONE]", event="done", id=str(len(chunks) + 1))

    def handle_invalid_openai_like_sse(self):
        self.send_sse_headers()

        invalid_events = [
            "data: {\"id\":\"chatcmpl-bad\",\"object\":\"chat.completion.chunk\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"zq\"}}]}\n\n",
            "data: {this-is-not-json}\n\n",
            "data: {\"id\":\"chatcmpl-bad\",\"object\":\"chat.completion.chunk\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"xinvalid-merge\"}}]}\n\n",
            "data: [DONE]\n\n",
        ]

        for payload in invalid_events:
            if not self.send_bytes_chunked(payload.encode("utf-8"), chunk=len(payload), interval=0):
                return
            time.sleep(0.05)

    def build_sse_event_bytes(self, data, event=None, id=None, retry=None, crlf=False):
        lines = []
        if id is not None:
            lines.append(f"id: {id}")
        if event is not None:
            lines.append(f"event: {event}")
        if retry is not None:
            lines.append(f"retry: {retry}")
        for line in str(data).split("\n"):
            lines.append(f"data: {line}")
        lines.append("")
        lines.append("")

        sep = "\r\n" if crlf else "\n"
        return sep.join(lines).encode("utf-8")

    def send_bytes_chunked(self, data_bytes, chunk=3, interval=0.05):
        try:
            i = 0
            n = len(data_bytes)
            while i < n:
                self.wfile.write(data_bytes[i:i + chunk])
                self.wfile.flush()
                i += chunk
                if interval > 0:
                    time.sleep(interval)
            return True
        except (BrokenPipeError, ConnectionResetError):
            return False

    def handle_chunked_sse(self, count=3, interval=0.05, crlf=False, chunk=3):
        self.send_sse_headers()

        for i in range(count):
            event_id = i + 1
            data = {
                "message": f"Chunked Event {event_id}",
                "index": i,
                "total": count,
                "timestamp": time.time()
            }
            event_bytes = self.build_sse_event_bytes(
                data=json.dumps(data),
                event="message",
                id=str(event_id),
                crlf=crlf,
            )
            if not self.send_bytes_chunked(event_bytes, chunk=chunk, interval=interval):
                return

        done_bytes = self.build_sse_event_bytes(
            data=json.dumps({"done": True, "total_events": count}),
            event="done",
            crlf=crlf,
        )
        self.send_bytes_chunked(done_bytes, chunk=chunk, interval=interval)


def run_server(port=3003, host="0.0.0.0"):
    server_address = (host, port)
    httpd = HTTPServer(server_address, SSEHandler)
    print(f"[SSE] Server starting on {host}:{port}")
    print(f"[SSE] Endpoints:")
    print(f"[SSE]   GET  /sse              - Stream 5 events")
    print(f"[SSE]   GET  /sse/custom       - Custom ?count=N&interval=M")
    print(f"[SSE]   GET  /sse/echo         - Echo with Last-Event-ID")
    print(f"[SSE]   GET  /sse/multiline    - Multiline data events")
    print(f"[SSE]   GET  /sse/json         - JSON payload events")
    print(f"[SSE]   POST /sse/trigger      - Manual trigger")
    print(f"[SSE]   GET  /health           - Health check")

    try:
        httpd.serve_forever()
    except KeyboardInterrupt:
        print("\n[SSE] Server shutting down...")
        httpd.shutdown()


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description="SSE Echo Server")
    parser.add_argument("--port", type=int, default=3003, help="Port to listen on")
    parser.add_argument("--host", type=str, default="0.0.0.0", help="Host to bind to")
    args = parser.parse_args()

    run_server(port=args.port, host=args.host)
