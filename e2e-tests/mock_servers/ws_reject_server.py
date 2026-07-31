#!/usr/bin/env python3
"""Minimal upstream that rejects every WebSocket handshake with HTTP 401."""

import argparse
import socketserver


class RejectHandler(socketserver.BaseRequestHandler):
    def handle(self) -> None:
        data = b""
        self.request.settimeout(5)
        while b"\r\n\r\n" not in data and len(data) < 64 * 1024:
            chunk = self.request.recv(4096)
            if not chunk:
                return
            data += chunk
        body = b"websocket authorization required"
        response = (
            b"HTTP/1.1 401 Unauthorized\r\n"
            b"Content-Type: text/plain\r\n"
            + f"Content-Length: {len(body)}\r\n".encode()
            + b"Connection: close\r\n\r\n"
            + body
        )
        self.request.sendall(response)


class Server(socketserver.ThreadingTCPServer):
    allow_reuse_address = True
    daemon_threads = True


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--port", type=int, required=True)
    args = parser.parse_args()
    with Server((args.host, args.port), RejectHandler) as server:
        server.serve_forever()


if __name__ == "__main__":
    main()
