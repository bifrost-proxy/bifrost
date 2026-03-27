#!/usr/bin/env python3
"""
HTTPS Echo Server - 用于验证代理服务的 TLS 处理能力

启动方式:
    python3 https_echo_server.py [port]
    python3 https_echo_server.py 3443

功能:
    - 自签名证书的 HTTPS 服务
    - 详细回显所有收到的请求信息
    - 返回 JSON 格式，便于断言验证
    - 用于测试 https→http, http→https 转发场景
"""

import http.server
import json
import os
import socketserver
import ssl
import sys
import tempfile
import urllib.parse
import gzip
import io
import time
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


def print_banner(unicode_banner, ascii_banner):
    """在不支持 Unicode 输出的终端中回退到 ASCII banner。"""
    encoding = getattr(sys.stdout, 'encoding', None) or 'utf-8'
    try:
        unicode_banner.encode(encoding)
    except UnicodeEncodeError:
        print(ascii_banner)
    else:
        print(unicode_banner)


def generate_self_signed_cert():
    """生成临时自签名证书"""
    cert_dir = tempfile.mkdtemp(prefix='bifrost_test_')
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
            x509.NameAttribute(NameOID.STATE_OR_PROVINCE_NAME, "Beijing"),
            x509.NameAttribute(NameOID.LOCALITY_NAME, "Beijing"),
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

        print(f"  Certificate generated: {cert_path}")
        return cert_path, key_path, cert_dir

    except ImportError:
        print("  Warning: cryptography not installed, using openssl command")
        import subprocess
        subprocess.run([
            'openssl', 'req', '-x509', '-newkey', 'rsa:2048',
            '-keyout', key_path, '-out', cert_path,
            '-days', '365', '-nodes',
            '-subj', '/CN=localhost/O=Bifrost Test'
        ], capture_output=True)
        return cert_path, key_path, cert_dir


class EchoHandler(http.server.BaseHTTPRequestHandler):
    """回显所有请求信息的 Handler"""

    protocol_version = 'HTTP/1.1'

    def log_message(self, format, *args):
        timestamp = datetime.now().strftime('%Y-%m-%d %H:%M:%S.%f')[:-3]
        test_id = getattr(self, '_test_id', 'UNKNOWN')
        print(f"[{timestamp}] [{test_id}] {self.client_address[0]}:{self.client_address[1]} - {format % args}")

    def _log_request_start(self, method):
        """打印请求开始日志，包含测试标识"""
        self._test_id = self.headers.get('X-Test-ID', 'UNKNOWN')
        timestamp = datetime.now().strftime('%Y-%m-%d %H:%M:%S.%f')[:-3]
        print(f"\n{'='*60}")
        print(f"[{timestamp}] [{self._test_id}] [HTTPS] {method} {self.path}")

    def _build_response(self, body_content=None):
        headers_dict = {}
        headers_list = []
        for key, value in self.headers.items():
            headers_dict[key] = value
            headers_list.append(f"{key}: {value}")

        parsed_path = urllib.parse.urlparse(self.path)
        query_params = urllib.parse.parse_qs(parsed_path.query)

        tls_info = {}
        if hasattr(self.connection, 'getpeercert'):
            try:
                tls_info['peer_cert'] = str(self.connection.getpeercert())
                tls_info['cipher'] = self.connection.cipher()
                tls_info['version'] = self.connection.version()
            except Exception:
                pass

        response_data = {
            "timestamp": datetime.now().isoformat(),
            "server": {
                "type": "https_echo_server",
                "protocol": "https",
                "port": self.server.server_address[1],
                "address": f"{self.server.server_address[0]}:{self.server.server_address[1]}",
                "tls": tls_info
            },
            "request": {
                "method": self.command,
                "path": self.path,
                "parsed_path": parsed_path.path,
                "query_string": parsed_path.query,
                "query_params": query_params,
                "http_version": self.request_version,
                "headers": headers_dict,
                "headers_raw": headers_list,
                "client_address": f"{self.client_address[0]}:{self.client_address[1]}"
            }
        }

        if body_content is not None:
            try:
                response_data["request"]["body"] = body_content.decode('utf-8')
                response_data["request"]["body_raw"] = body_content.hex()
            except UnicodeDecodeError:
                response_data["request"]["body"] = None
                response_data["request"]["body_raw"] = body_content.hex()

        return response_data

    def _get_content_type_and_body(self, response_data):
        """根据请求路径确定 Content-Type 和响应体"""
        path = self.path.lower()
        
        if path.endswith('.html') or path.endswith('.htm'):
            body = f"""<!DOCTYPE html>
<html>
<head><title>HTTPS Echo Response</title></head>
<body>
<h1>HTTPS Echo Response</h1>
<pre>{json.dumps(response_data, indent=2, ensure_ascii=False)}</pre>
</body>
</html>"""
            return 'text/html; charset=utf-8', body
        elif path.endswith('.js'):
            body = f"""// HTTPS Echo Response
var echoData = {json.dumps(response_data, ensure_ascii=False)};
console.log('HTTPS Echo Server Response:', echoData);"""
            return 'application/javascript; charset=utf-8', body
        elif path.endswith('.css'):
            body = f"""/* HTTPS Echo Response */
/* Request Info: {response_data.get('request', {}).get('path', '')} */
body {{ color: #333; }}
.echo-data {{ display: block; }}"""
            return 'text/css; charset=utf-8', body
        elif path.endswith('.xml'):
            body = f"""<?xml version="1.0" encoding="UTF-8"?>
<echo>
<request>
<method>{response_data.get('request', {}).get('method', '')}</method>
<path>{response_data.get('request', {}).get('path', '')}</path>
<protocol>https</protocol>
</request>
</echo>"""
            return 'application/xml; charset=utf-8', body
        elif path.endswith('.txt'):
            body = f"HTTPS Echo Response\n\n{json.dumps(response_data, indent=2, ensure_ascii=False)}"
            return 'text/plain; charset=utf-8', body
        else:
            return 'application/json; charset=utf-8', json.dumps(response_data, indent=2, ensure_ascii=False)

    def _send_response(self, status_code=200, extra_headers=None, body_content=None):
        response_data = self._build_response(body_content)
        content_type, response_body = self._get_content_type_and_body(response_data)

        self.send_response(status_code)
        self.send_header('Content-Type', content_type)
        self.send_header('Content-Length', str(len(response_body.encode('utf-8'))))
        self.send_header('X-Echo-Server', 'bifrost-test-https')
        self.send_header('X-Request-Method', self.command)
        self.send_header('X-Request-Path', self.path)
        self.send_header('X-Protocol', 'https')
        self.send_header('Connection', 'keep-alive')

        if extra_headers:
            for key, value in extra_headers.items():
                self.send_header(key, value)

        self.end_headers()
        self.wfile.write(response_body.encode('utf-8'))

        print(f"  -> Response: {status_code}, Content-Type: {content_type.split(';')[0]}, Body size: {len(response_body)} bytes")

    def _read_body(self):
        content_length = self.headers.get('Content-Length')
        if content_length:
            try:
                return self.rfile.read(int(content_length))
            except Exception:
                return None
        return None

    def _query_params(self):
        parsed_path = urllib.parse.urlparse(self.path)
        return parsed_path, urllib.parse.parse_qs(parsed_path.query)

    def _httpbin_headers(self):
        return {k.title(): v for k, v in self.headers.items()}

    def _httpbin_url(self, parsed_path):
        host = self.headers.get('Host', f'127.0.0.1:{self.server.server_address[1]}')
        if parsed_path.query:
            return f"https://{host}{parsed_path.path}?{parsed_path.query}"
        return f"https://{host}{parsed_path.path}"

    def _httpbin_cookies(self):
        cookies = {}
        cookie_header = self.headers.get('Cookie', '')
        for part in cookie_header.split(';'):
            part = part.strip()
            if '=' in part:
                key, value = part.split('=', 1)
                cookies[key.strip()] = value.strip()
        return cookies

    def _send_bytes_response(self, status_code, content_type, body_bytes, extra_headers=None):
        self.send_response(status_code)
        self.send_header('Content-Type', content_type)
        self.send_header('Content-Length', str(len(body_bytes)))
        self.send_header('X-Echo-Server', 'bifrost-test-https')
        self.send_header('X-Protocol', 'https')
        self.send_header('Connection', 'keep-alive')
        if extra_headers:
            for key, value in extra_headers.items():
                self.send_header(key, value)
        self.end_headers()
        self.wfile.write(body_bytes)

    def _send_json_bytes(self, payload, status_code=200, extra_headers=None):
        self._send_bytes_response(
            status_code,
            'application/json; charset=utf-8',
            json.dumps(payload, ensure_ascii=False).encode('utf-8'),
            extra_headers=extra_headers,
        )

    def _handle_httpbin_common(self, method, body=None):
        parsed_path, query_params = self._query_params()
        path = parsed_path.path
        url = self._httpbin_url(parsed_path)
        headers = self._httpbin_headers()

        if path == '/ip':
            self._send_json_bytes({"origin": "127.0.0.1"})
            return True
        if path == '/headers':
            self._send_json_bytes({"headers": headers})
            return True
        if path == '/user-agent':
            self._send_json_bytes({"user-agent": self.headers.get('User-Agent', '')})
            return True
        if path == '/cookies':
            self._send_json_bytes({"cookies": self._httpbin_cookies()})
            return True
        if path.startswith('/status/'):
            status_segment = path[len('/status/'):].split('/', 1)[0] or '200'
            status_code = int(status_segment)
            self._send_bytes_response(status_code, 'application/json; charset=utf-8', b'{}')
            return True
        if path.startswith('/bytes/'):
            size = max(0, int(path.split('/')[-1] or '0'))
            payload = ('0123456789abcdef' * ((size // 16) + 1)).encode('utf-8')[:size]
            self._send_bytes_response(200, 'application/octet-stream', payload)
            return True
        if path.startswith('/redirect/'):
            remaining = max(0, int(path.split('/')[-1] or '0'))
            location = '/get' if remaining <= 1 else f'/redirect/{remaining - 1}'
            self._send_bytes_response(302, 'text/plain; charset=utf-8', b'', extra_headers={'Location': location})
            return True
        if path == '/drip':
            duration = float(query_params.get('duration', ['0'])[0])
            numbytes = max(0, int(query_params.get('numbytes', ['0'])[0]))
            delay = float(query_params.get('delay', ['0'])[0])
            if delay > 0:
                time.sleep(delay)
            if duration > 0:
                time.sleep(duration)
            payload = (b'd' * numbytes) or b'drip'
            self._send_bytes_response(200, 'application/octet-stream', payload)
            return True
        if path == '/sse':
            count = max(1, int(query_params.get('count', ['3'])[0]))
            delay = float(query_params.get('delay', ['0'])[0])
            chunks = []
            for idx in range(count):
                chunks.append(f"event: message\ndata: {{\"id\": {idx}, \"host\": \"httpbin.org\"}}\n\n")
            if delay > 0:
                time.sleep(delay * count)
            self._send_bytes_response(200, 'text/event-stream; charset=utf-8', ''.join(chunks).encode('utf-8'))
            return True
        if path == '/gzip':
            payload = json.dumps({
                "gzipped": True,
                "headers": headers,
                "method": method,
            }, ensure_ascii=False).encode('utf-8')
            buf = io.BytesIO()
            with gzip.GzipFile(fileobj=buf, mode='wb') as gz:
                gz.write(payload)
            self._send_bytes_response(
                200,
                'application/json; charset=utf-8',
                buf.getvalue(),
                extra_headers={'Content-Encoding': 'gzip'},
            )
            return True
        if path == '/html':
            html = "<html><body><h1>httpbin mock html</h1><p>forwarded</p></body></html>"
            self._send_bytes_response(200, 'text/html; charset=utf-8', html.encode('utf-8'))
            return True
        if path == '/get' or path == '/anything':
            self._send_json_bytes({
                "args": {k: v[-1] for k, v in query_params.items()},
                "headers": headers,
                "origin": "127.0.0.1",
                "url": url,
            })
            return True
        if path in ('/post', '/put', '/patch', '/delete'):
            text_body = body.decode('utf-8', errors='replace') if body else ""
            try:
                json_body = json.loads(text_body) if text_body else None
            except json.JSONDecodeError:
                json_body = None
            self._send_json_bytes({
                "args": {k: v[-1] for k, v in query_params.items()},
                "data": text_body,
                "headers": headers,
                "json": json_body,
                "method": method,
                "origin": "127.0.0.1",
                "url": url,
            })
            return True
        return False

    def _handle_large_response(self):
        parsed_path = urllib.parse.urlparse(self.path)
        query_params = urllib.parse.parse_qs(parsed_path.query)

        size = int(query_params.get('size', ['1024'])[0])
        marker = query_params.get('marker', ['MARKER'])[0]

        marker_len = len(marker)
        if size < marker_len * 2:
            size = marker_len * 2

        padding_size = size - marker_len * 2
        body = marker + ('X' * padding_size) + marker

        self.send_response(200)
        self.send_header('Content-Type', 'text/plain; charset=utf-8')
        self.send_header('Content-Length', str(len(body)))
        self.send_header('X-Echo-Server', 'bifrost-test-https')
        self.send_header('X-Response-Size', str(size))
        self.send_header('X-Response-Marker', marker)
        self.send_header('Connection', 'keep-alive')
        self.end_headers()
        self.wfile.write(body.encode('utf-8'))

    def do_GET(self):
        self._log_request_start("GET")
        print(f"Headers:")
        for key, value in self.headers.items():
            print(f"  {key}: {value}")
        parsed_path = urllib.parse.urlparse(self.path)
        if parsed_path.path == '/health':
            body = "ok"
            self.send_response(200)
            self.send_header('Content-Type', 'text/plain; charset=utf-8')
            self.send_header('Content-Length', str(len(body)))
            self.send_header('X-Echo-Server', 'bifrost-test-https')
            self.send_header('Connection', 'close')
            self.end_headers()
            self.wfile.write(body.encode('utf-8'))
            return
        if self._handle_httpbin_common("GET"):
            return
        if parsed_path.path == '/large-response':
            self._handle_large_response()
        else:
            self._send_response()

    def do_POST(self):
        self._log_request_start("POST")
        body = self._read_body()
        if self._handle_httpbin_common("POST", body):
            return
        parsed_path = urllib.parse.urlparse(self.path)
        if parsed_path.path == '/large-response':
            self._handle_large_response()
        else:
            self._send_response(body_content=body)

    def do_PUT(self):
        self._log_request_start("PUT")
        body = self._read_body()
        if self._handle_httpbin_common("PUT", body):
            return
        self._send_response(body_content=body)

    def do_DELETE(self):
        self._log_request_start("DELETE")
        if self._handle_httpbin_common("DELETE"):
            return
        self._send_response()

    def do_PATCH(self):
        self._log_request_start("PATCH")
        body = self._read_body()
        if self._handle_httpbin_common("PATCH", body):
            return
        self._send_response(body_content=body)

    def do_HEAD(self):
        self._log_request_start("HEAD")
        self.send_response(200)
        self.send_header('Content-Type', 'application/json')
        self.send_header('X-Echo-Server', 'bifrost-test-https')
        self.end_headers()

    def do_OPTIONS(self):
        self._log_request_start("OPTIONS")
        self.send_response(200)
        self.send_header('Access-Control-Allow-Origin', '*')
        self.send_header('Access-Control-Allow-Methods', 'GET, POST, PUT, DELETE, PATCH, OPTIONS, HEAD')
        self.send_header('Access-Control-Allow-Headers', '*')
        self.send_header('Content-Length', '0')
        self.end_headers()


class ThreadedHTTPServer(socketserver.ThreadingMixIn, http.server.HTTPServer):
    allow_reuse_address = True
    daemon_threads = True


def main():
    port = int(sys.argv[1]) if len(sys.argv) > 1 else 3443
    host = '127.0.0.1'

    unicode_banner = f"""
╔══════════════════════════════════════════════════════════════╗
║          HTTPS Echo Server - Bifrost E2E Testing             ║
╠══════════════════════════════════════════════════════════════╣
║  Address: https://{host}:{port:<5}                             ║
║  Purpose: Test TLS termination and https forwarding          ║
║                                                              ║
║  Note: Uses self-signed certificate                          ║
╚══════════════════════════════════════════════════════════════╝
"""
    ascii_banner = (
        "+------------------------------------------------------------+\n"
        "|          HTTPS Echo Server - Bifrost E2E Testing          |\n"
        "+------------------------------------------------------------+\n"
        f"|  Address: https://{host}:{port:<5}                            |\n"
        "|  Purpose: Test TLS termination and https forwarding       |\n"
        "|                                                            |\n"
        "|  Note: Uses self-signed certificate                        |\n"
        "+------------------------------------------------------------+"
    )
    print_banner(unicode_banner, ascii_banner)

    print("Generating self-signed certificate...")
    cert_path, key_path, cert_dir = generate_self_signed_cert()

    context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    context.load_cert_chain(cert_path, key_path)

    with ThreadedHTTPServer((host, port), EchoHandler) as httpd:
        httpd.socket = context.wrap_socket(httpd.socket, server_side=True)
        print(f"Starting HTTPS Echo Server on {host}:{port}...")
        print("Press Ctrl+C to stop\n")
        try:
            httpd.serve_forever()
        except KeyboardInterrupt:
            print("\nShutting down server...")
            httpd.shutdown()
        finally:
            import shutil
            shutil.rmtree(cert_dir, ignore_errors=True)


if __name__ == '__main__':
    main()
