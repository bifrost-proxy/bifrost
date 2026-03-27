#!/usr/bin/env python3
"""
WebSocket Echo Server - 用于验证代理服务的 WebSocket 处理能力

启动方式:
    python3 ws_echo_server.py [port] [--ssl]
    python3 ws_echo_server.py 3020           # ws://
    python3 ws_echo_server.py 3021 --ssl     # wss://

功能:
    - 支持 ws:// 和 wss:// 连接
    - 详细打印连接信息、收到的消息
    - 消息回显功能
    - 返回连接时的 headers 信息

测试端点:
    /ws           - 标准 echo 模式
    /ws/broadcast - 连接后自动推送 5 条消息
    /ws/binary    - 接收 text 返回 binary
    /ws/ping      - 立即发送 ping 帧
    /ws/close     - 发送带状态码的 close 帧
"""

import asyncio
import base64
import hashlib
import json
import os
import ssl
import struct
import sys
import tempfile
import urllib.parse
import zlib
from datetime import datetime, timezone

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


def generate_self_signed_cert():
    """生成临时自签名证书"""
    cert_dir = tempfile.mkdtemp(prefix='bifrost_ws_test_')
    cert_path = os.path.join(cert_dir, 'cert.pem')
    key_path = os.path.join(cert_dir, 'key.pem')

    try:
        from cryptography import x509
        from cryptography.x509.oid import NameOID
        from cryptography.hazmat.primitives import hashes, serialization
        from cryptography.hazmat.primitives.asymmetric import rsa
        from datetime import timedelta
        import ipaddress

        key = rsa.generate_private_key(public_exponent=65537, key_size=2048)

        subject = issuer = x509.Name([
            x509.NameAttribute(NameOID.COUNTRY_NAME, "CN"),
            x509.NameAttribute(NameOID.ORGANIZATION_NAME, "Bifrost Test"),
            x509.NameAttribute(NameOID.COMMON_NAME, "localhost"),
        ])

        cert = (
            x509.CertificateBuilder()
            .subject_name(subject)
            .issuer_name(issuer)
            .public_key(key.public_key())
            .serial_number(x509.random_serial_number())
            .not_valid_before(datetime.now(timezone.utc))
            .not_valid_after(datetime.now(timezone.utc) + timedelta(days=365))
            .add_extension(
                x509.SubjectAlternativeName([
                    x509.DNSName("localhost"),
                    x509.DNSName("*.local"),
                    x509.IPAddress(ipaddress.IPv4Address("127.0.0.1")),
                ]),
                critical=False,
            )
            .sign(key, hashes.SHA256())
        )

        with open(key_path, 'wb') as f:
            f.write(key.private_bytes(
                encoding=serialization.Encoding.PEM,
                format=serialization.PrivateFormat.TraditionalOpenSSL,
                encryption_algorithm=serialization.NoEncryption()
            ))

        with open(cert_path, 'wb') as f:
            f.write(cert.public_bytes(serialization.Encoding.PEM))

        return cert_path, key_path, cert_dir

    except ImportError:
        import subprocess
        subprocess.run([
            'openssl', 'req', '-x509', '-newkey', 'rsa:2048',
            '-keyout', key_path, '-out', cert_path,
            '-days', '365', '-nodes',
            '-subj', '/CN=localhost/O=Bifrost Test'
        ], capture_output=True)
        return cert_path, key_path, cert_dir


def parse_http_request(data):
    """解析 HTTP 请求头"""
    try:
        request_str = data.decode('utf-8', errors='ignore')
        lines = request_str.split('\r\n')

        if not lines:
            return None, None, None

        request_line = lines[0]
        parts = request_line.split(' ')
        method = parts[0] if len(parts) > 0 else ''
        path = parts[1] if len(parts) > 1 else '/'

        headers = {}
        for line in lines[1:]:
            if ':' in line:
                key, value = line.split(':', 1)
                headers[key.strip()] = value.strip()

        return method, path, headers
    except Exception as e:
        log(f"Error parsing request: {e}")
        return None, None, None


def compute_accept_key(key):
    """计算 WebSocket accept key"""
    sha1 = hashlib.sha1((key + WS_MAGIC_KEY).encode()).digest()
    return base64.b64encode(sha1).decode()


def create_ws_frame(payload, opcode=1, mask=False, fin=True, rsv1=False):
    """创建 WebSocket 帧"""
    if isinstance(payload, str):
        payload = payload.encode('utf-8')

    length = len(payload)
    first_byte = opcode
    if fin:
        first_byte |= 0x80
    if rsv1:
        first_byte |= 0x40
    header = bytes([first_byte])

    if length <= 125:
        header += bytes([length])
    elif length <= 65535:
        header += bytes([126]) + struct.pack('>H', length)
    else:
        header += bytes([127]) + struct.pack('>Q', length)

    return header + payload


def get_header_ci(headers: dict, name: str):
    for k, v in headers.items():
        if k.lower() == name.lower():
            return v
    return None


def parse_path_and_query(raw_path: str):
    try:
        u = urllib.parse.urlparse(raw_path)
        return u.path or raw_path, urllib.parse.parse_qs(u.query)
    except Exception:
        return raw_path, {}


def make_permessage_deflate_raw(payload: bytes) -> bytes:
    """生成 permessage-deflate 格式的 raw DEFLATE + SYNC_FLUSH（去掉 trailer）。"""
    c = zlib.compressobj(wbits=-15)
    out = c.compress(payload) + c.flush(zlib.Z_SYNC_FLUSH)
    # RFC 7692：消息以 SYNC_FLUSH 结束，但不携带 0x00 0x00 0xff 0xff trailer
    if out.endswith(b"\x00\x00\xff\xff"):
        out = out[:-4]
    return out


def create_close_frame(code=1000, reason=""):
    """创建带状态码的 close 帧"""
    payload = struct.pack('>H', code) + reason.encode('utf-8')
    return create_ws_frame(payload, opcode=8)


def parse_ws_frame(data):
    """解析 WebSocket 帧"""
    if len(data) < 2:
        return None, None, data

    first_byte = data[0]
    second_byte = data[1]

    fin = (first_byte & 0x80) != 0
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


class WebSocketConnection:
    def __init__(self, reader, writer, client_addr, use_ssl, server_port):
        self.reader = reader
        self.writer = writer
        self.client_addr = client_addr
        self.use_ssl = use_ssl
        self.server_port = server_port
        self.headers = {}
        self.path = '/'

    async def handle(self):
        try:
            data = await self.reader.read(4096)
            if not data:
                return

            method, path, headers = parse_http_request(data)
            self.headers = headers or {}
            self.path = path or '/'

            path_only, query = parse_path_and_query(self.path)

            log(f"{'='*60}")
            log(f"New connection from {self.client_addr}")
            log(f"Request: {method} {path}")
            log(f"Headers:")
            for key, value in self.headers.items():
                log(f"  {key}: {value}")

            ws_key = self.headers.get('Sec-WebSocket-Key')
            if not ws_key:
                log("No Sec-WebSocket-Key found, not a WebSocket request")
                self.writer.close()
                return

            accept_key = compute_accept_key(ws_key)
            protocol = 'wss' if self.use_ssl else 'ws'

            ext_offer = get_header_ci(self.headers, 'Sec-WebSocket-Extensions') or ""
            protocol_offer = get_header_ci(self.headers, 'Sec-WebSocket-Protocol')
            selected_protocol = None
            if protocol_offer:
                # 选择第一个子协议，便于测试 replay 的协议协商透传
                selected_protocol = protocol_offer.split(',', 1)[0].strip() or None

            accept_extensions = None
            if "permessage-deflate" in ext_offer.lower():
                accept_extensions = "permessage-deflate"

            response_lines = [
                'HTTP/1.1 101 Switching Protocols\r\n',
                'Upgrade: websocket\r\n',
                'Connection: Upgrade\r\n',
                f'Sec-WebSocket-Accept: {accept_key}\r\n',
            ]
            if selected_protocol:
                response_lines.append(f'Sec-WebSocket-Protocol: {selected_protocol}\r\n')
            if accept_extensions:
                response_lines.append(f'Sec-WebSocket-Extensions: {accept_extensions}\r\n')
            response_lines.extend(
                [
                    'X-Echo-Server: bifrost-ws-test\r\n',
                    f'X-Protocol: {protocol}\r\n',
                    '\r\n',
                ]
            )
            response = ''.join(response_lines)
            self.writer.write(response.encode())
            await self.writer.drain()

            log(f"WebSocket handshake completed ({protocol}://)")

            welcome_msg = json.dumps({
                "type": "connection_info",
                "server": {
                    "type": "ws_echo_server",
                    "protocol": protocol,
                    "port": self.server_port,
                    "address": f"127.0.0.1:{self.server_port}"
                },
                "connection": {
                    "path": self.path,
                    "headers": self.headers,
                    "client_address": self.client_addr
                },
                "timestamp": datetime.now().isoformat()
            })

            # 某些用例需要避免额外的 welcome 干扰（如长连接/压缩分片测试）
            if not (path_only.startswith("/ws/idle") or path_only.startswith("/ws/deflate_frag")):
                self.writer.write(create_ws_frame(welcome_msg))
                await self.writer.drain()
                log(f"Sent welcome message with connection info")

            if path_only.startswith("/ws/broadcast"):
                await self.handle_broadcast_mode()
            elif path_only.startswith("/ws/ping"):
                await self.handle_ping_mode()
            elif path_only.startswith("/ws/close"):
                await self.handle_close_mode()
            elif path_only.startswith("/ws/binary"):
                await self.handle_binary_mode()
            elif path_only.startswith("/ws/idle"):
                await self.handle_idle_mode(query)
            elif path_only.startswith("/ws/deflate_frag"):
                await self.handle_deflate_frag_mode()
            else:
                await self.message_loop()

        except Exception as e:
            log(f"Error handling connection: {e}")
        finally:
            self.writer.close()
            await self.writer.wait_closed()
            log(f"Connection closed: {self.client_addr}")

    async def message_loop(self):
        """消息处理循环"""
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
                                log(f"Received binary (as hex): {message[:100]}")
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

    async def handle_broadcast_mode(self):
        """广播模式：自动推送 5 条消息"""
        log("Broadcast mode: sending 5 messages")
        for i in range(5):
            msg = json.dumps({
                "type": "broadcast",
                "index": i + 1,
                "total": 5,
                "message": f"Broadcast message {i + 1}",
                "timestamp": datetime.now().isoformat()
            })
            self.writer.write(create_ws_frame(msg))
            await self.writer.drain()
            log(f"Sent broadcast message {i + 1}/5")
            await asyncio.sleep(0.3)

        self.writer.write(create_close_frame(1000, "Broadcast complete"))
        await self.writer.drain()
        log("Broadcast complete, sent close frame")

    async def handle_ping_mode(self):
        """Ping 模式：立即发送 ping 帧"""
        log("Ping mode: sending ping frame")
        self.writer.write(create_ws_frame(b'server_ping', opcode=9))
        await self.writer.drain()
        await self.message_loop()

    async def handle_close_mode(self):
        """Close 模式：发送带状态码的 close 帧"""
        log("Close mode: sending close frame with code 1001")
        await asyncio.sleep(0.5)
        self.writer.write(create_close_frame(1001, "Going away"))
        await self.writer.drain()
        log("Sent close frame")

    async def handle_binary_mode(self):
        """Binary 模式：接收 text 返回 binary"""
        log("Binary mode: will respond with binary frames")
        buffer = b''

        while True:
            try:
                data = await asyncio.wait_for(self.reader.read(4096), timeout=60)
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

                    elif opcode in (1, 2):
                        binary_response = payload if opcode == 2 else payload
                        self.writer.write(create_ws_frame(binary_response, opcode=2))
                        await self.writer.drain()
                        log(f"Sent binary response: {len(binary_response)} bytes")

            except asyncio.TimeoutError:
                break

    async def handle_idle_mode(self, query: dict):
        """长连接模式：延迟发送消息，用于验证 replay 长连接不会被 30s 强制断开。"""
        delay = 35
        msg = "late_message"
        try:
            if "delay" in query and query["delay"]:
                delay = int(query["delay"][0])
            if "msg" in query and query["msg"]:
                msg = query["msg"][0]
        except Exception:
            pass

        log(f"Idle mode: waiting {delay}s then sending message")
        await asyncio.sleep(max(delay, 0))
        self.writer.write(create_ws_frame(msg, opcode=1))
        await self.writer.drain()
        log("Idle mode: message sent")
        # 保持连接一小段时间，给客户端/代理捕获
        await asyncio.sleep(1)
        self.writer.write(create_close_frame(1000, "idle complete"))
        await self.writer.drain()

    async def handle_deflate_frag_mode(self):
        """发送 permessage-deflate + fragmentation 的压缩消息。"""
        text = b"hello-deflate-frag"
        compressed = make_permessage_deflate_raw(text)
        # 分成两帧：首帧 opcode=Text + rsv1=1 + fin=0；续帧 opcode=Continuation + fin=1
        mid = max(1, len(compressed) // 2)
        part1 = compressed[:mid]
        part2 = compressed[mid:]

        log(f"Deflate-frag mode: sending compressed fragmented message ({len(compressed)} bytes)")
        self.writer.write(create_ws_frame(part1, opcode=1, fin=False, rsv1=True))
        await self.writer.drain()
        await asyncio.sleep(0.05)
        self.writer.write(create_ws_frame(part2, opcode=0, fin=True, rsv1=False))
        await self.writer.drain()

        await asyncio.sleep(0.2)
        self.writer.write(create_close_frame(1000, "deflate frag complete"))
        await self.writer.drain()


async def start_server(host, port, use_ssl=False):
    ssl_context = None
    cert_dir = None

    if use_ssl:
        log("Generating self-signed certificate...")
        cert_path, key_path, cert_dir = generate_self_signed_cert()
        ssl_context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        ssl_context.load_cert_chain(cert_path, key_path)
        log(f"Certificate loaded: {cert_path}")

    protocol = 'wss' if use_ssl else 'ws'

    async def client_handler(reader, writer):
        addr = writer.get_extra_info('peername')
        client_addr = f"{addr[0]}:{addr[1]}" if addr else "unknown"
        conn = WebSocketConnection(reader, writer, client_addr, use_ssl, port)
        await conn.handle()

    server = await asyncio.start_server(
        client_handler,
        host,
        port,
        ssl=ssl_context
    )

    unicode_banner = f"""
╔══════════════════════════════════════════════════════════════╗
║         WebSocket Echo Server - Bifrost E2E Testing          ║
╠══════════════════════════════════════════════════════════════╣
║  Address: {protocol}://{host}:{port:<5}                               ║
║  Protocol: {protocol.upper():<4}                                           ║
║  Purpose: Test WebSocket forwarding                          ║
║                                                              ║
║  Features:                                                   ║
║    - Echo all received messages                              ║
║    - Return connection headers info                          ║
║    - Ping/Pong support                                       ║
╚══════════════════════════════════════════════════════════════╝
"""
    ascii_banner = (
        "+------------------------------------------------------------+\n"
        "|         WebSocket Echo Server - Bifrost E2E Testing       |\n"
        "+------------------------------------------------------------+\n"
        f"|  Address: {protocol}://{host}:{port:<5}                              |\n"
        f"|  Protocol: {protocol.upper():<4}                                        |\n"
        "|  Purpose: Test WebSocket forwarding                      |\n"
        "|                                                            |\n"
        "|  Features: Echo messages, headers info, and ping/pong     |\n"
        "+------------------------------------------------------------+"
    )
    print_banner(unicode_banner, ascii_banner)

    log(f"Starting WebSocket Echo Server on {protocol}://{host}:{port}...")
    log("Press Ctrl+C to stop\n")

    try:
        async with server:
            await server.serve_forever()
    except KeyboardInterrupt:
        log("Shutting down server...")
    finally:
        if cert_dir:
            import shutil
            shutil.rmtree(cert_dir, ignore_errors=True)


def main():
    port = 3020
    use_ssl = False

    args = sys.argv[1:]
    for arg in args:
        if arg == '--ssl':
            use_ssl = True
        elif arg.isdigit():
            port = int(arg)

    asyncio.run(start_server('127.0.0.1', port, use_ssl))


if __name__ == '__main__':
    main()
