#!/usr/bin/env python3
"""
HTTP + WebSocket Echo Server - 用于验证代理服务的 HTTP 和 WebSocket 处理能力

启动方式:
    python3 http_ws_echo_server.py [port]
    python3 http_ws_echo_server.py 8000

功能:
    - 同一端口同时支持 HTTP 和 WebSocket
    - 详细打印所有请求信息
    - HTTP: 返回 JSON 格式的请求详情
    - WebSocket: 消息回显 + 连接信息
"""

import asyncio
import base64
import hashlib
import json
import struct
import sys
from datetime import datetime

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


WS_MAGIC_KEY = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11"


def print_banner(unicode_banner, ascii_banner):
    """在不支持 Unicode 输出的终端中回退到 ASCII banner。"""
    encoding = getattr(sys.stdout, 'encoding', None) or 'utf-8'
    try:
        unicode_banner.encode(encoding)
    except UnicodeEncodeError:
        print(ascii_banner)
    else:
        print(unicode_banner)


def log(message):
    timestamp = datetime.now().strftime('%Y-%m-%d %H:%M:%S.%f')[:-3]
    print(f"[{timestamp}] {message}")


def parse_http_request(data):
    """解析 HTTP 请求"""
    try:
        request_str = data.decode('utf-8', errors='ignore')
        lines = request_str.split('\r\n')

        if not lines:
            return None, None, None, None

        request_line = lines[0]
        parts = request_line.split(' ')
        method = parts[0] if len(parts) > 0 else ''
        path = parts[1] if len(parts) > 1 else '/'

        headers = {}
        body_start = 0
        for i, line in enumerate(lines[1:], 1):
            if line == '':
                body_start = i + 1
                break
            if ':' in line:
                key, value = line.split(':', 1)
                headers[key.strip()] = value.strip()

        body = '\r\n'.join(lines[body_start:]) if body_start > 0 else ''

        return method, path, headers, body
    except Exception as e:
        log(f"Error parsing request: {e}")
        return None, None, None, None


def compute_accept_key(key):
    """计算 WebSocket accept key"""
    sha1 = hashlib.sha1((key + WS_MAGIC_KEY).encode()).digest()
    return base64.b64encode(sha1).decode()


def create_ws_frame(payload, opcode=1, fin=True):
    """创建 WebSocket 帧"""
    if isinstance(payload, str):
        payload = payload.encode('utf-8')

    length = len(payload)
    first_byte = opcode
    if fin:
        first_byte |= 0x80
    header = bytes([first_byte])

    if length <= 125:
        header += bytes([length])
    elif length <= 65535:
        header += bytes([126]) + struct.pack('>H', length)
    else:
        header += bytes([127]) + struct.pack('>Q', length)

    return header + payload


def parse_ws_frame(data):
    """解析 WebSocket 帧"""
    if len(data) < 2:
        return None, None, data

    first_byte = data[0]
    second_byte = data[1]

    opcode = first_byte & 0x0F
    masked = (second_byte & 0x80) != 0
    payload_len = second_byte & 0x7F

    offset = 2

    if payload_len == 126:
        if len(data) < 4:
            return None, None, data
        payload_len = struct.unpack('>H', data[2:4])[0]
        offset = 4
    elif payload_len == 127:
        if len(data) < 10:
            return None, None, data
        payload_len = struct.unpack('>Q', data[2:10])[0]
        offset = 10

    if masked:
        if len(data) < offset + 4 + payload_len:
            return None, None, data
        mask_key = data[offset:offset + 4]
        offset += 4
        payload = bytes(b ^ mask_key[i % 4] for i, b in enumerate(data[offset:offset + payload_len]))
    else:
        if len(data) < offset + payload_len:
            return None, None, data
        payload = data[offset:offset + payload_len]

    remaining = data[offset + payload_len:]
    return opcode, payload, remaining


class Connection:
    def __init__(self, reader, writer, client_addr, server_port):
        self.reader = reader
        self.writer = writer
        self.client_addr = client_addr
        self.server_port = server_port

    async def handle(self):
        try:
            data = await asyncio.wait_for(self.reader.read(8192), timeout=30)
            if not data:
                return

            method, path, headers, body = parse_http_request(data)
            if not method:
                return

            log(f"{'='*60}")
            log(f"New connection from {self.client_addr}")
            log(f"Request: {method} {path}")
            log(f"Headers:")
            for key, value in headers.items():
                log(f"  {key}: {value}")

            ws_key = headers.get('Sec-WebSocket-Key')
            upgrade = headers.get('Upgrade', '').lower()

            if ws_key and upgrade == 'websocket':
                await self.handle_websocket(path, headers, ws_key)
            else:
                await self.handle_http(method, path, headers, body)

        except asyncio.TimeoutError:
            log(f"Connection timeout: {self.client_addr}")
        except Exception as e:
            log(f"Error handling connection: {e}")
        finally:
            self.writer.close()
            try:
                await self.writer.wait_closed()
            except:
                pass
            log(f"Connection closed: {self.client_addr}")

    async def handle_http(self, method, path, headers, body):
        """处理 HTTP 请求"""
        log(f"Handling HTTP {method} request")

        response_data = {
            "type": "http_echo",
            "timestamp": datetime.now().isoformat(),
            "server": {
                "type": "http_ws_echo_server",
                "protocol": "http",
                "port": self.server_port,
                "address": f"127.0.0.1:{self.server_port}"
            },
            "request": {
                "method": method,
                "path": path,
                "headers": headers,
                "body": body if body else None,
                "client_address": self.client_addr
            }
        }

        response_body = json.dumps(response_data, indent=2, ensure_ascii=False)
        response_bytes = response_body.encode('utf-8')

        response = (
            'HTTP/1.1 200 OK\r\n'
            'Content-Type: application/json; charset=utf-8\r\n'
            f'Content-Length: {len(response_bytes)}\r\n'
            'X-Echo-Server: bifrost-http-ws-test\r\n'
            f'X-Request-Method: {method}\r\n'
            f'X-Request-Path: {path}\r\n'
            'Connection: close\r\n'
            '\r\n'
        )

        self.writer.write(response.encode() + response_bytes)
        await self.writer.drain()
        log(f"Sent HTTP response: 200 OK, {len(response_bytes)} bytes")

    async def handle_websocket(self, path, headers, ws_key):
        """处理 WebSocket 连接"""
        log(f"Handling WebSocket upgrade")

        accept_key = compute_accept_key(ws_key)

        response = (
            'HTTP/1.1 101 Switching Protocols\r\n'
            'Upgrade: websocket\r\n'
            'Connection: Upgrade\r\n'
            f'Sec-WebSocket-Accept: {accept_key}\r\n'
            'X-Echo-Server: bifrost-http-ws-test\r\n'
            '\r\n'
        )
        self.writer.write(response.encode())
        await self.writer.drain()
        log(f"WebSocket handshake completed")

        welcome_msg = json.dumps({
            "type": "connection_info",
            "server": {
                "type": "http_ws_echo_server",
                "protocol": "ws",
                "port": self.server_port,
                "address": f"127.0.0.1:{self.server_port}"
            },
            "connection": {
                "path": path,
                "headers": headers,
                "client_address": self.client_addr
            },
            "timestamp": datetime.now().isoformat()
        })

        self.writer.write(create_ws_frame(welcome_msg))
        await self.writer.drain()
        log(f"Sent welcome message")

        await self.ws_message_loop()

    async def ws_message_loop(self):
        """WebSocket 消息处理循环"""
        buffer = b''

        while True:
            try:
                data = await asyncio.wait_for(self.reader.read(4096), timeout=300)
                if not data:
                    break

                buffer += data

                while buffer:
                    opcode, payload, buffer = parse_ws_frame(buffer)
                    if opcode is None:
                        break

                    if opcode == 8:
                        log("Received close frame")
                        self.writer.write(create_ws_frame(b'', opcode=8))
                        await self.writer.drain()
                        return

                    elif opcode == 9:
                        log("Received ping, sending pong")
                        self.writer.write(create_ws_frame(payload, opcode=10))
                        await self.writer.drain()

                    elif opcode == 10:
                        log("Received pong")

                    elif opcode in (1, 2):
                        if opcode == 1:
                            try:
                                message = payload.decode('utf-8')
                                log(f"Received text: {message[:200]}{'...' if len(message) > 200 else ''}")
                            except:
                                message = payload.hex()
                        else:
                            log(f"Received binary: {len(payload)} bytes")
                            message = payload.hex()

                        echo_response = json.dumps({
                            "type": "echo",
                            "opcode": opcode,
                            "message": message if opcode == 1 else None,
                            "binary_hex": payload.hex() if opcode == 2 else None,
                            "length": len(payload),
                            "timestamp": datetime.now().isoformat()
                        })

                        self.writer.write(create_ws_frame(echo_response))
                        await self.writer.drain()
                        log(f"Sent echo response")

            except asyncio.TimeoutError:
                log("Connection timeout, sending ping")
                self.writer.write(create_ws_frame(b'ping', opcode=9))
                await self.writer.drain()
            except Exception as e:
                log(f"Error in message loop: {e}")
                break


async def start_server(host, port):
    async def client_handler(reader, writer):
        addr = writer.get_extra_info('peername')
        client_addr = f"{addr[0]}:{addr[1]}" if addr else "unknown"
        conn = Connection(reader, writer, client_addr, port)
        await conn.handle()

    server = await asyncio.start_server(client_handler, host, port)

    unicode_banner = f"""
╔══════════════════════════════════════════════════════════════╗
║       HTTP + WebSocket Echo Server - Bifrost E2E Testing     ║
╠══════════════════════════════════════════════════════════════╣
║  Address: http://{host}:{port:<5}                              ║
║  WebSocket: ws://{host}:{port:<5}                              ║
║  Purpose: Test HTTP and WebSocket forwarding                 ║
║                                                              ║
║  Features:                                                   ║
║    - HTTP: Echo all request details as JSON                  ║
║    - WebSocket: Echo messages + connection info              ║
╚══════════════════════════════════════════════════════════════╝
"""
    ascii_banner = (
        "+------------------------------------------------------------+\n"
        "|       HTTP + WebSocket Echo Server - Bifrost E2E Testing  |\n"
        "+------------------------------------------------------------+\n"
        f"|  Address: http://{host}:{port:<5}                             |\n"
        f"|  WebSocket: ws://{host}:{port:<5}                             |\n"
        "|  Purpose: Test HTTP and WebSocket forwarding              |\n"
        "|                                                            |\n"
        "|  Features: HTTP echo + WebSocket echo                     |\n"
        "+------------------------------------------------------------+"
    )
    print_banner(unicode_banner, ascii_banner)

    log(f"Starting HTTP+WS Echo Server on {host}:{port}...")
    log("Press Ctrl+C to stop\n")

    try:
        async with server:
            await server.serve_forever()
    except KeyboardInterrupt:
        log("Shutting down server...")


def main():
    port = int(sys.argv[1]) if len(sys.argv) > 1 else 8000
    host = '127.0.0.1'
    asyncio.run(start_server(host, port))


if __name__ == '__main__':
    main()
