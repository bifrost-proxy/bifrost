#!/usr/bin/env python3
"""
HTTP Echo Server - 用于验证代理服务对请求的处理

启动方式:
    python3 http_echo_server.py [port]
    python3 http_echo_server.py 3000

功能:
    - 详细回显所有收到的请求信息
    - 返回 JSON 格式，便于断言验证
    - 支持所有 HTTP 方法
    - 支持自定义响应头和状态码
"""

import http.server
import json
import socketserver
import sys
import urllib.parse
import gzip
import io
from datetime import datetime


def print_banner(unicode_banner, ascii_banner):
    """在不支持 Unicode 输出的终端中回退到 ASCII banner。"""
    encoding = getattr(sys.stdout, 'encoding', None) or 'utf-8'
    try:
        unicode_banner.encode(encoding)
    except UnicodeEncodeError:
        print(ascii_banner)
    else:
        print(unicode_banner)


class EchoHandler(http.server.BaseHTTPRequestHandler):
    """回显所有请求信息的 Handler"""

    protocol_version = 'HTTP/1.1'

    def log_message(self, format, *args):
        """自定义日志格式，包含更多调试信息"""
        timestamp = datetime.now().strftime('%Y-%m-%d %H:%M:%S.%f')[:-3]
        test_id = getattr(self, '_test_id', 'UNKNOWN')
        print(f"[{timestamp}] [{test_id}] {self.client_address[0]}:{self.client_address[1]} - {format % args}")

    def _log_request_start(self, method):
        """打印请求开始日志，包含测试标识"""
        self._test_id = self.headers.get('X-Test-ID', 'UNKNOWN')
        timestamp = datetime.now().strftime('%Y-%m-%d %H:%M:%S.%f')[:-3]
        print(f"\n{'='*60}")
        print(f"[{timestamp}] [{self._test_id}] {method} {self.path}")

    def _build_response(self, body_content=None):
        """构建统一的响应数据结构"""
        headers_dict = {}
        headers_list = []
        for key, value in self.headers.items():
            headers_dict[key] = value
            headers_list.append(f"{key}: {value}")

        cookies_dict = {}
        cookie_header = self.headers.get('Cookie', '')
        if cookie_header:
            for part in cookie_header.split(';'):
                part = part.strip()
                if '=' in part:
                    key, value = part.split('=', 1)
                    cookies_dict[key.strip()] = value.strip()

        parsed_path = urllib.parse.urlparse(self.path)
        query_params = urllib.parse.parse_qs(parsed_path.query)

        response_data = {
            "timestamp": datetime.now().isoformat(),
            "server": {
                "type": "http_echo_server",
                "protocol": "http",
                "port": self.server.server_address[1],
                "address": f"{self.server.server_address[0]}:{self.server.server_address[1]}"
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
                "cookies": cookies_dict,
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
                response_data["request"]["body_encoding_error"] = True

        return response_data

    def _get_content_type_and_body(self, response_data):
        """根据请求路径确定 Content-Type 和响应体"""
        path = self.path.lower()

        if path.endswith('.html') or path.endswith('.htm'):
            body = f"""<!DOCTYPE html>
<html>
<head><title>Echo Response</title></head>
<body>
<h1>Echo Response</h1>
<pre>{json.dumps(response_data, indent=2, ensure_ascii=False)}</pre>
</body>
</html>"""
            return 'text/html; charset=utf-8', body
        elif path.endswith('.js'):
            body = f"""// Echo Response
var echoData = {json.dumps(response_data, ensure_ascii=False)};
console.log('Echo Server Response:', echoData);"""
            return 'application/javascript; charset=utf-8', body
        elif path.endswith('.css'):
            body = f"""/* Echo Response */
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
</request>
</echo>"""
            return 'application/xml; charset=utf-8', body
        elif path.endswith('.txt'):
            body = f"Echo Response\n\n{json.dumps(response_data, indent=2, ensure_ascii=False)}"
            return 'text/plain; charset=utf-8', body
        else:
            return 'application/json; charset=utf-8', json.dumps(response_data, indent=2, ensure_ascii=False)

    def _send_response(self, status_code=200, extra_headers=None, body_content=None):
        """发送响应"""
        response_data = self._build_response(body_content)
        content_type, response_body = self._get_content_type_and_body(response_data)

        self.send_response(status_code)
        self.send_header('Content-Type', content_type)
        self.send_header('Content-Length', str(len(response_body.encode('utf-8'))))
        self.send_header('X-Echo-Server', 'bifrost-test')
        self.send_header('X-Request-Method', self.command)
        self.send_header('X-Request-Path', self.path)
        self.send_header('Connection', 'keep-alive')

        if extra_headers:
            for key, value in extra_headers.items():
                self.send_header(key, value)

        self.end_headers()
        self.wfile.write(response_body.encode('utf-8'))

        print(f"  -> Response: {status_code}, Content-Type: {content_type.split(';')[0]}, Body size: {len(response_body)} bytes")

    def _read_body(self):
        """读取请求体"""
        content_length = self.headers.get('Content-Length')
        if content_length:
            try:
                length = int(content_length)
                return self.rfile.read(length)
            except (ValueError, Exception) as e:
                print(f"  -> Error reading body: {e}")
                return None
        return None

    def _handle_large_response(self):
        """处理大响应体请求 /large-response?size=BYTES&marker=MARKER&encoding=gzip"""
        parsed_path = urllib.parse.urlparse(self.path)
        query_params = urllib.parse.parse_qs(parsed_path.query)
        
        size = int(query_params.get('size', ['1024'])[0])
        marker = query_params.get('marker', ['MARKER'])[0]
        encoding = query_params.get('encoding', [''])[0].lower()
        
        print(f"  -> Generating large response: size={size}, marker={marker}, encoding={encoding or 'none'}")
        
        marker_len = len(marker)
        if size < marker_len * 2:
            size = marker_len * 2
        
        padding_size = size - marker_len * 2
        body = marker + ('X' * padding_size) + marker
        
        resp_bytes = body.encode('utf-8')
        extra_headers = {}
        if encoding == 'gzip':
            buf = io.BytesIO()
            with gzip.GzipFile(fileobj=buf, mode='wb') as f:
                f.write(resp_bytes)
            resp_bytes = buf.getvalue()
            extra_headers['Content-Encoding'] = 'gzip'

        self.send_response(200)
        self.send_header('Content-Type', 'text/plain; charset=utf-8')
        self.send_header('Content-Length', str(len(resp_bytes)))
        self.send_header('X-Echo-Server', 'bifrost-test')
        self.send_header('X-Response-Size', str(size))
        self.send_header('X-Response-Marker', marker)
        self.send_header('Connection', 'keep-alive')
        for k, v in extra_headers.items():
            self.send_header(k, v)
        self.end_headers()
        self.wfile.write(resp_bytes)
        
        print(f"  -> Sent large response: raw={size} bytes, wire={len(resp_bytes)} bytes")

    def do_GET(self):
        """处理 GET 请求"""
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
            self.send_header('X-Echo-Server', 'bifrost-test')
            self.send_header('Connection', 'close')
            self.end_headers()
            self.wfile.write(body.encode('utf-8'))
            return
        if parsed_path.path == '/static-value':
            body = "REMOTE_VALUE"
            self.send_response(200)
            self.send_header('Content-Type', 'text/plain; charset=utf-8')
            self.send_header('Content-Length', str(len(body)))
            self.send_header('X-Echo-Server', 'bifrost-test')
            self.send_header('Connection', 'keep-alive')
            self.end_headers()
            self.wfile.write(body.encode('utf-8'))
            return
        if parsed_path.path == '/large-response':
            self._handle_large_response()
        else:
            self._send_response()

    def do_POST(self):
        """处理 POST 请求"""
        self._log_request_start("POST")
        print(f"Headers:")
        for key, value in self.headers.items():
            print(f"  {key}: {value}")

        body = self._read_body()
        if body:
            print(f"Body ({len(body)} bytes):")
            try:
                print(f"  {body.decode('utf-8')[:500]}...")
            except UnicodeDecodeError:
                print(f"  [Binary data: {body.hex()[:100]}...]")
        parsed_path = urllib.parse.urlparse(self.path)
        if parsed_path.path == '/large-response':
            self._handle_large_response()
        else:
            self._send_response(body_content=body)

    def do_PUT(self):
        """处理 PUT 请求"""
        self._log_request_start("PUT")
        print(f"Headers:")
        for key, value in self.headers.items():
            print(f"  {key}: {value}")

        body = self._read_body()
        self._send_response(body_content=body)

    def do_DELETE(self):
        """处理 DELETE 请求"""
        self._log_request_start("DELETE")
        print(f"Headers:")
        for key, value in self.headers.items():
            print(f"  {key}: {value}")
        self._send_response()

    def do_PATCH(self):
        """处理 PATCH 请求"""
        self._log_request_start("PATCH")
        body = self._read_body()
        self._send_response(body_content=body)

    def do_HEAD(self):
        """处理 HEAD 请求"""
        self._log_request_start("HEAD")
        self.send_response(200)
        self.send_header('Content-Type', 'application/json')
        self.send_header('X-Echo-Server', 'bifrost-test')
        self.end_headers()

    def do_OPTIONS(self):
        """处理 OPTIONS 请求，支持 CORS preflight"""
        self._log_request_start("OPTIONS")
        print(f"Headers:")
        for key, value in self.headers.items():
            print(f"  {key}: {value}")

        self.send_response(200)
        self.send_header('Access-Control-Allow-Origin', '*')
        self.send_header('Access-Control-Allow-Methods', 'GET, POST, PUT, DELETE, PATCH, OPTIONS, HEAD')
        self.send_header('Access-Control-Allow-Headers', '*')
        self.send_header('Access-Control-Max-Age', '86400')
        self.send_header('Content-Length', '0')
        self.end_headers()


class ThreadedHTTPServer(socketserver.ThreadingMixIn, http.server.HTTPServer):
    """支持多线程的 HTTP Server"""
    allow_reuse_address = True
    daemon_threads = True


def main():
    port = int(sys.argv[1]) if len(sys.argv) > 1 else 3000
    host = '127.0.0.1'

    unicode_banner = f"""
╔══════════════════════════════════════════════════════════════╗
║           HTTP Echo Server - Bifrost E2E Testing             ║
╠══════════════════════════════════════════════════════════════╣
║  Address: http://{host}:{port:<5}                              ║
║  Purpose: Echo all request details for verification          ║
║                                                              ║
║  Response format: JSON with complete request info            ║
║  Supported methods: GET, POST, PUT, DELETE, PATCH, OPTIONS   ║
╚══════════════════════════════════════════════════════════════╝
"""
    ascii_banner = (
        "+------------------------------------------------------------+\n"
        "|           HTTP Echo Server - Bifrost E2E Testing          |\n"
        "+------------------------------------------------------------+\n"
        f"|  Address: http://{host}:{port:<5}                             |\n"
        "|  Purpose: Echo all request details for verification       |\n"
        "|                                                            |\n"
        "|  Response format: JSON with complete request info         |\n"
        "|  Supported methods: GET, POST, PUT, DELETE, PATCH, OPTIONS|\n"
        "+------------------------------------------------------------+"
    )
    print_banner(unicode_banner, ascii_banner)

    with ThreadedHTTPServer((host, port), EchoHandler) as httpd:
        print(f"Starting HTTP Echo Server on {host}:{port}...")
        print("Press Ctrl+C to stop\n")
        try:
            httpd.serve_forever()
        except KeyboardInterrupt:
            print("\nShutting down server...")
            httpd.shutdown()


if __name__ == '__main__':
    main()
