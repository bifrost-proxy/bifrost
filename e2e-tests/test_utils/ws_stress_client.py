#!/usr/bin/env python3
import argparse
import base64
import hashlib
import os
import socket
import struct
import time

from typing import Optional


WS_MAGIC_KEY = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11"


def _recv_exact(sock: socket.socket, n: int) -> bytes:
    buf = bytearray()
    while len(buf) < n:
        chunk = sock.recv(n - len(buf))
        if not chunk:
            raise ConnectionError("unexpected EOF")
        buf.extend(chunk)
    return bytes(buf)


def _read_http_headers(sock: socket.socket, timeout_s: float) -> tuple:
    sock.settimeout(timeout_s)
    data = bytearray()
    while b"\r\n\r\n" not in data:
        chunk = sock.recv(4096)
        if not chunk:
            break
        data.extend(chunk)
        if len(data) > 64 * 1024:
            raise ValueError("http header too large")
    full = bytes(data)
    idx = full.find(b"\r\n\r\n")
    if idx >= 0:
        return full[: idx + 4], full[idx + 4 :]
    return full, b""


def _parse_http_response(data: bytes):
    head = data.split(b"\r\n\r\n", 1)[0].decode("utf-8", errors="replace")
    lines = head.split("\r\n")
    status_line = lines[0]
    parts = status_line.split(" ", 2)
    code = int(parts[1]) if len(parts) > 1 and parts[1].isdigit() else 0
    headers = {}
    for line in lines[1:]:
        if ":" in line:
            k, v = line.split(":", 1)
            headers[k.strip().lower()] = v.strip()
    return code, headers


def _ws_make_client_text_frame(payload: bytes) -> bytes:
    return _ws_make_client_frame(payload, opcode=1, fin=True, rsv1=False)


def _ws_make_client_close_frame(code: int = 1000) -> bytes:
    payload = struct.pack(">H", code)
    return _ws_make_client_frame(payload, opcode=8, fin=True, rsv1=False)


def _ws_make_client_frame(
    payload: bytes,
    opcode: int,
    fin: bool = True,
    rsv1: bool = False,
    mask: bool = True,
    payload_len_override: Optional[int] = None,
    omit_payload: bool = False,
) -> bytes:
    first = opcode & 0x0F
    if fin:
        first |= 0x80
    if rsv1:
        first |= 0x40

    length = len(payload)
    if payload_len_override is not None:
        length = int(payload_len_override)
        if length < 0:
            length = 0

    mask_bit = 0x80 if mask else 0x00
    hdr = bytearray([first])
    if length <= 125:
        hdr.append(mask_bit | length)
    elif length <= 65535:
        hdr.append(mask_bit | 126)
        hdr.extend(struct.pack(">H", length))
    else:
        hdr.append(mask_bit | 127)
        hdr.extend(struct.pack(">Q", length))

    mask_key = b""
    if mask:
        mask_key = os.urandom(4)
        hdr.extend(mask_key)

    if omit_payload or not payload:
        return bytes(hdr)

    if mask:
        masked = bytes(b ^ mask_key[i % 4] for i, b in enumerate(payload))
        return bytes(hdr) + masked
    return bytes(hdr) + payload


class _BufferedSocket:
    def __init__(self, sock: socket.socket, initial: bytes = b""):
        self._sock = sock
        self._buf = bytearray(initial)

    def settimeout(self, t: float):
        self._sock.settimeout(t)

    def sendall(self, data: bytes):
        self._sock.sendall(data)

    def recv(self, n: int) -> bytes:
        if self._buf:
            chunk = bytes(self._buf[:n])
            del self._buf[:n]
            return chunk
        return self._sock.recv(n)

    def recv_exact(self, n: int) -> bytes:
        buf = bytearray()
        while len(buf) < n:
            if self._buf:
                take = min(n - len(buf), len(self._buf))
                buf.extend(self._buf[:take])
                del self._buf[:take]
            else:
                chunk = self._sock.recv(n - len(buf))
                if not chunk:
                    raise ConnectionError("unexpected EOF")
                buf.extend(chunk)
        return bytes(buf)

    def close(self):
        self._sock.close()


def _ws_read_frame(sock, timeout_s: float):
    if isinstance(sock, _BufferedSocket):
        sock.settimeout(timeout_s)
        b1, b2 = sock.recv_exact(2)
    else:
        sock.settimeout(timeout_s)
        b1, b2 = _recv_exact(sock, 2)
    fin = (b1 & 0x80) != 0
    opcode = b1 & 0x0F
    masked = (b2 & 0x80) != 0
    length = b2 & 0x7F
    if isinstance(sock, _BufferedSocket):
        _recv = sock.recv_exact
    else:
        _recv = lambda n: _recv_exact(sock, n)
    if length == 126:
        length = struct.unpack(">H", _recv(2))[0]
    elif length == 127:
        length = struct.unpack(">Q", _recv(8))[0]
    mask_key = b""
    if masked:
        mask_key = _recv(4)
    payload = _recv(length) if length else b""
    if masked and payload:
        payload = bytes(b ^ mask_key[i % 4] for i, b in enumerate(payload))
    return fin, opcode, payload


def run(
    proxy_host: str,
    proxy_port: int,
    request_host_header: str,
    path: str,
    message_text: str,
    messages: int,
    timeout_s: float,
    expect_binary: bool,
    extensions: str,
    protocol: str,
    expect_protocol: str,
    expect_extensions: str,
    sleep_before_send_s: float,
    no_send: bool,
    read_seconds: float,
    expect_text_contains: str,
    expect_text_count: int,
    send_invalid_control_fin0: bool,
    send_oversize_len: int,
    expect_close: bool,
):
    key = base64.b64encode(os.urandom(16)).decode("ascii")
    expected_accept = base64.b64encode(
        hashlib.sha1((key + WS_MAGIC_KEY).encode("utf-8")).digest()
    ).decode("ascii")

    sock = socket.create_connection((proxy_host, proxy_port), timeout=timeout_s)
    try:
        req_lines = [
            f"GET {path} HTTP/1.1\r\n",
            f"Host: {request_host_header}\r\n",
            "Upgrade: websocket\r\n",
            "Connection: Upgrade\r\n",
            "Sec-WebSocket-Version: 13\r\n",
            f"Sec-WebSocket-Key: {key}\r\n",
        ]
        if extensions:
            req_lines.append(f"Sec-WebSocket-Extensions: {extensions}\r\n")
        if protocol:
            req_lines.append(f"Sec-WebSocket-Protocol: {protocol}\r\n")
        req_lines.append("\r\n")
        req = "".join(req_lines).encode("utf-8")
        sock.sendall(req)
        resp_header, leftover = _read_http_headers(sock, timeout_s)
        code, headers = _parse_http_response(resp_header)
        if code != 101:
            raise RuntimeError(f"handshake failed: status={code}")
        accept = headers.get("sec-websocket-accept", "")
        if accept != expected_accept:
            raise RuntimeError("handshake failed: bad sec-websocket-accept")

        bsock = _BufferedSocket(sock, leftover)

        if expect_protocol:
            got = headers.get("sec-websocket-protocol", "")
            if got != expect_protocol:
                raise RuntimeError(
                    f"handshake failed: sec-websocket-protocol mismatch, got={got!r}"
                )

        if expect_extensions:
            got = headers.get("sec-websocket-extensions", "")
            if expect_extensions not in got:
                raise RuntimeError(
                    f"handshake failed: sec-websocket-extensions mismatch, got={got!r}"
                )

        if sleep_before_send_s > 0:
            time.sleep(sleep_before_send_s)

        if send_invalid_control_fin0:
            bsock.sendall(_ws_make_client_frame(b"x", opcode=9, fin=False, mask=True))
        elif send_oversize_len > 0:
            bsock.sendall(
                _ws_make_client_frame(
                    b"",
                    opcode=1,
                    fin=True,
                    mask=True,
                    payload_len_override=send_oversize_len,
                    omit_payload=True,
                )
            )

        payload = message_text.encode("utf-8")
        if not no_send and messages > 0 and not send_invalid_control_fin0 and send_oversize_len <= 0:
            for _ in range(messages):
                bsock.sendall(_ws_make_client_text_frame(payload))

        recv_ok = 0
        contains_ok = 0
        start = time.time()
        expected_opcode = 2 if expect_binary else 1

        def should_continue() -> bool:
            if read_seconds > 0:
                return (time.time() - start) < read_seconds
            if expect_text_contains:
                return contains_ok < max(1, expect_text_count)
            if expect_close:
                return True
            return recv_ok < messages

        while should_continue() and (time.time() - start) < timeout_s:
            try:
                _, opcode, _payload = _ws_read_frame(bsock, timeout_s)
            except (ConnectionError, OSError):
                if expect_close:
                    return
                raise

            if opcode == expected_opcode:
                recv_ok += 1
                if expect_text_contains and expected_opcode == 1:
                    try:
                        txt = _payload.decode("utf-8", errors="replace")
                    except Exception:
                        txt = ""
                    if expect_text_contains in txt:
                        contains_ok += 1
            elif opcode == 8:
                if expect_close:
                    return
                break
            elif opcode == 9:
                pong = bytearray([0x8A, 0x80 | len(_payload)])
                mask_key = os.urandom(4)
                pong.extend(mask_key)
                pong_payload = bytes(b ^ mask_key[i % 4] for i, b in enumerate(_payload))
                bsock.sendall(bytes(pong) + pong_payload)

        if expect_text_contains:
            if contains_ok < max(1, expect_text_count):
                raise RuntimeError(
                    f"expected text containing {expect_text_contains!r} x{expect_text_count}, got {contains_ok}"
                )
        elif expect_close:
            raise RuntimeError("expected close, but connection kept alive")
        else:
            # 默认逻辑：按 messages 计数
            if messages > 0 and recv_ok < messages:
                raise RuntimeError(f"expected {messages} messages, got {recv_ok}")

        try:
            bsock.sendall(_ws_make_client_close_frame(1000))
        except Exception:
            pass
    finally:
        try:
            sock.close()
        except Exception:
            pass


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--proxy-host", required=True)
    ap.add_argument("--proxy-port", type=int, required=True)
    ap.add_argument("--host-header", required=True)
    ap.add_argument("--path", default="/ws")
    ap.add_argument("--message", default='{"type":"ping"}')
    ap.add_argument("--messages", type=int, default=1000)
    ap.add_argument("--timeout", type=float, default=30.0)
    ap.add_argument("--expect-binary", action="store_true")
    ap.add_argument("--extensions", default="")
    ap.add_argument("--protocol", default="")
    ap.add_argument("--expect-protocol", default="")
    ap.add_argument("--expect-extensions", default="")
    ap.add_argument("--sleep-before-send", type=float, default=0.0)
    ap.add_argument("--no-send", action="store_true")
    ap.add_argument("--read-seconds", type=float, default=0.0)
    ap.add_argument("--expect-text-contains", default="")
    ap.add_argument("--expect-text-count", type=int, default=1)
    ap.add_argument("--send-invalid-control-fin0", action="store_true")
    ap.add_argument("--send-oversize-len", type=int, default=0)
    ap.add_argument("--expect-close", action="store_true")
    args = ap.parse_args()

    run(
        proxy_host=args.proxy_host,
        proxy_port=args.proxy_port,
        request_host_header=args.host_header,
        path=args.path,
        message_text=args.message,
        messages=args.messages,
        timeout_s=args.timeout,
        expect_binary=args.expect_binary,
        extensions=args.extensions,
        protocol=args.protocol,
        expect_protocol=args.expect_protocol,
        expect_extensions=args.expect_extensions,
        sleep_before_send_s=args.sleep_before_send,
        no_send=args.no_send,
        read_seconds=args.read_seconds,
        expect_text_contains=args.expect_text_contains,
        expect_text_count=args.expect_text_count,
        send_invalid_control_fin0=args.send_invalid_control_fin0,
        send_oversize_len=args.send_oversize_len,
        expect_close=args.expect_close,
    )


if __name__ == "__main__":
    main()
