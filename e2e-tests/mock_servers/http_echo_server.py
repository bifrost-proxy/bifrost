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
import base64
import json
import socketserver
import subprocess
import sys
import urllib.parse
import gzip
import io
import os
import time
from datetime import datetime

PNG_1X1_BYTES = base64.b64decode(
    b'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAusB9p1+tH8AAAAASUVORK5CYII='
)
JPEG_1X1_BYTES = base64.b64decode(
    b'/9j/4AAQSkZJRgABAQAAAQABAAD/2wCEAAkGBxAQEBAQEA8QDw8QEA8PDw8QEA8QFREWFhURFRUYHSggGBolGxUV'
    b'ITEhJSkrLi4uFx8zODMsNygtLisBCgoKDg0OGhAQGi0fHyUtLS0tLS0tLS0tLS0tLS0tLS0tLS0tLS0tLS0tLS0t'
    b'LS0tLS0tLS0tLS0tLS0tLf/AABEIAAEAAgMBIgACEQEDEQH/xAAXAAADAQAAAAAAAAAAAAAAAAAAAQMC/8QAFBAB'
    b'AAAAAAAAAAAAAAAAAAAAAP/aAAwDAQACEAMQAAAByA//xAAZEAEAAwEBAAAAAAAAAAAAAAABAAIRITH/2gAIAQEA'
    b'AT8AqWORo//EABQRAQAAAAAAAAAAAAAAAAAAABD/2gAIAQIBAT8Af//EABQRAQAAAAAAAAAAAAAAAAAAABD/2gAI'
    b'AQMBAT8Af//Z'
)

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

    def _query_params(self):
        parsed_path = urllib.parse.urlparse(self.path)
        return parsed_path, urllib.parse.parse_qs(parsed_path.query)

    def _httpbin_headers(self):
        return {k.title(): v for k, v in self.headers.items()}

    def _httpbin_cookies(self):
        cookies = {}
        cookie_header = self.headers.get('Cookie', '')
        for part in cookie_header.split(';'):
            part = part.strip()
            if '=' in part:
                key, value = part.split('=', 1)
                cookies[key.strip()] = value.strip()
        return cookies

    def _httpbin_url(self, parsed_path):
        host = self.headers.get('Host', f'127.0.0.1:{self.server.server_address[1]}')
        if parsed_path.query:
            return f"http://{host}{parsed_path.path}?{parsed_path.query}"
        return f"http://{host}{parsed_path.path}"

    def _httpbin_origin(self):
        forwarded_for = self.headers.get('X-Forwarded-For')
        if forwarded_for:
            return forwarded_for.split(',', 1)[0].strip()
        return self.client_address[0]

    def _httpbin_single_args(self, query_params):
        return {k: v[-1] for k, v in query_params.items()}

    def _safe_int(self, value, default=0, minimum=None, maximum=None):
        try:
            parsed = int(value)
        except (TypeError, ValueError):
            parsed = default
        if minimum is not None:
            parsed = max(minimum, parsed)
        if maximum is not None:
            parsed = min(maximum, parsed)
        return parsed

    def _safe_float(self, value, default=0.0, minimum=None, maximum=None):
        try:
            parsed = float(value)
        except (TypeError, ValueError):
            parsed = default
        if minimum is not None:
            parsed = max(minimum, parsed)
        if maximum is not None:
            parsed = min(maximum, parsed)
        return parsed

    def _decode_text_body(self, body):
        return body.decode('utf-8', errors='replace') if body else ""

    def _parse_form_data(self, body):
        content_type = self.headers.get('Content-Type', '')
        if not body or 'application/x-www-form-urlencoded' not in content_type:
            return {}
        return {
            key: values[-1]
            for key, values in urllib.parse.parse_qs(
                self._decode_text_body(body),
                keep_blank_values=True,
            ).items()
        }

    def _parse_multipart_data(self, body):
        content_type = self.headers.get('Content-Type', '')
        if not body or 'multipart/form-data' not in content_type:
            return {}, {}

        boundary = None
        for part in content_type.split(';'):
            part = part.strip()
            if part.startswith('boundary='):
                boundary = part[len('boundary='):].strip().strip('"')
                break
        if not boundary:
            return {}, {}

        fields = {}
        files = {}
        delimiter = ('--' + boundary).encode('utf-8')
        end_delimiter = ('--' + boundary + '--').encode('utf-8')

        parts = body.split(delimiter)
        for raw_part in parts:
            if not raw_part or raw_part.strip() == b'--' or raw_part == b'\r\n':
                continue
            if raw_part.startswith(end_delimiter) or raw_part.strip() == b'':
                continue

            if raw_part.startswith(b'\r\n'):
                raw_part = raw_part[2:]
            if raw_part.endswith(b'\r\n'):
                raw_part = raw_part[:-2]

            header_end = raw_part.find(b'\r\n\r\n')
            if header_end == -1:
                continue
            header_block = raw_part[:header_end].decode('utf-8', errors='replace')
            part_body = raw_part[header_end + 4:]

            name = None
            filename = None
            for line in header_block.split('\r\n'):
                lower_line = line.lower()
                if lower_line.startswith('content-disposition:'):
                    for param in line.split(';'):
                        param = param.strip()
                        if param.startswith('name='):
                            name = param[len('name='):].strip().strip('"')
                        elif param.startswith('filename='):
                            filename = param[len('filename='):].strip().strip('"')

            if name is None:
                continue

            if filename is not None:
                try:
                    files[name] = part_body.decode('utf-8')
                except UnicodeDecodeError:
                    files[name] = base64.b64encode(part_body).decode('ascii')
            else:
                fields[name] = part_body.decode('utf-8', errors='replace')
        return fields, files

    def _parse_authorization(self):
        header = self.headers.get('Authorization', '')
        if not header or ' ' not in header:
            return '', ''
        scheme, value = header.split(' ', 1)
        return scheme.lower(), value.strip()

    def _compress_brotli(self, payload):
        try:
            proc = subprocess.run(
                ['brotli', '--stdout', '--quality=4'],
                input=payload,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                check=True,
            )
            return proc.stdout
        except Exception as exc:
            print(f"  -> Brotli compression unavailable: {exc}")
            return None

    def _build_httpbin_body_response(self, method, parsed_path, query_params, headers, body):
        text_body = self._decode_text_body(body)
        form_data = self._parse_form_data(body)
        multipart_form, multipart_files = self._parse_multipart_data(body)
        try:
            json_body = json.loads(text_body) if text_body else None
        except json.JSONDecodeError:
            json_body = None
        if multipart_form:
            form_data.update(multipart_form)
        return {
            "args": self._httpbin_single_args(query_params),
            "data": text_body,
            "files": multipart_files,
            "form": form_data,
            "headers": headers,
            "json": json_body,
            "method": method,
            "origin": self._httpbin_origin(),
            "url": self._httpbin_url(parsed_path),
        }

    def _build_httpbin_request_response(self, method, parsed_path, query_params, headers, body=None):
        payload = {
            "args": self._httpbin_single_args(query_params),
            "headers": headers,
            "method": method,
            "origin": self._httpbin_origin(),
            "url": self._httpbin_url(parsed_path),
        }
        if body is not None:
            payload.update(self._build_httpbin_body_response(method, parsed_path, query_params, headers, body))
        return payload

    def _httpbin_redirect_location(self, target):
        host = self.headers.get('Host', f'127.0.0.1:{self.server.server_address[1]}')
        if target.startswith(('http://', 'https://')):
            return target
        if target.startswith('/'):
            return target
        return f"http://{host}/{target}"

    def _send_bytes_response(self, status_code, content_type, body_bytes, extra_headers=None):
        self.send_response(status_code)
        self.send_header('Content-Type', content_type)
        self.send_header('Content-Length', str(len(body_bytes)))
        self.send_header('X-Echo-Server', 'bifrost-test')
        self.send_header('Connection', 'keep-alive')
        if extra_headers:
            header_items = extra_headers.items() if hasattr(extra_headers, 'items') else extra_headers
            for key, value in header_items:
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

    def _send_chunked_response(self, status_code, content_type, chunks, extra_headers=None):
        self.send_response(status_code)
        self.send_header('Content-Type', content_type)
        self.send_header('Transfer-Encoding', 'chunked')
        self.send_header('X-Echo-Server', 'bifrost-test')
        self.send_header('Connection', 'keep-alive')
        if extra_headers:
            header_items = extra_headers.items() if hasattr(extra_headers, 'items') else extra_headers
            for key, value in header_items:
                self.send_header(key, value)
        self.end_headers()

        for chunk in chunks:
            if not chunk:
                continue
            self.wfile.write(f"{len(chunk):X}\r\n".encode('ascii'))
            self.wfile.write(chunk)
            self.wfile.write(b"\r\n")
            self.wfile.flush()
        self.wfile.write(b"0\r\n\r\n")
        self.wfile.flush()

    def _handle_httpbin_common(self, method, body=None):
        parsed_path, query_params = self._query_params()
        path = parsed_path.path
        url = self._httpbin_url(parsed_path)
        headers = self._httpbin_headers()
        args = self._httpbin_single_args(query_params)

        if path == '/ip':
            self._send_json_bytes({"origin": self._httpbin_origin()})
            return True
        if path == '/headers':
            self._send_json_bytes({"headers": headers})
            return True
        if path == '/user-agent':
            self._send_json_bytes({"user-agent": self.headers.get('User-Agent', '')})
            return True
        if path.startswith('/basic-auth/'):
            _, _, username, password = path.split('/', 3)
            scheme, value = self._parse_authorization()
            expected = base64.b64encode(f'{username}:{password}'.encode('utf-8')).decode('ascii')
            if scheme == 'basic' and value == expected:
                self._send_json_bytes({"authenticated": True, "user": username})
            else:
                self._send_json_bytes(
                    {"authenticated": False, "user": username},
                    status_code=401,
                    extra_headers={'WWW-Authenticate': 'Basic realm="Fake Realm"'},
                )
            return True
        if path == '/bearer':
            scheme, value = self._parse_authorization()
            if scheme == 'bearer' and value:
                self._send_json_bytes({"authenticated": True, "token": value})
            else:
                self._send_json_bytes(
                    {"authenticated": False, "token": ""},
                    status_code=401,
                    extra_headers={'WWW-Authenticate': 'Bearer'},
                )
            return True
        if path == '/cookies':
            self._send_json_bytes({"cookies": self._httpbin_cookies()})
            return True
        if path.startswith('/cookies/set'):
            cookies_to_set = []
            for key, value in args.items():
                cookies_to_set.append(('Set-Cookie', f'{key}={value}; Path=/'))
            redirect_target = query_params.get('url', ['/cookies'])[0]
            self._send_bytes_response(
                302,
                'application/json; charset=utf-8',
                b'',
                extra_headers=[('Location', redirect_target), *cookies_to_set],
            )
            return True
        if path.startswith('/cookies/delete'):
            cookies_to_clear = []
            for key in args.keys():
                cookies_to_clear.append(('Set-Cookie', f'{key}=; Path=/; Max-Age=0'))
            redirect_target = query_params.get('url', ['/cookies'])[0]
            self._send_bytes_response(
                302,
                'application/json; charset=utf-8',
                b'',
                extra_headers=[('Location', redirect_target), *cookies_to_clear],
            )
            return True
        if path.startswith('/status/'):
            status_segment = path[len('/status/'):].split('/', 1)[0] or '200'
            if ',' in status_segment:
                status_segment = status_segment.split(',', 1)[0]
            status_code = self._safe_int(status_segment, default=200, minimum=100, maximum=599)
            self._send_bytes_response(status_code, 'application/json; charset=utf-8', b'{}')
            return True
        if path.startswith('/bytes/'):
            size = self._safe_int(path.split('/')[-1] or '0', minimum=0, maximum=1024 * 1024)
            payload = ('0123456789abcdef' * ((size // 16) + 1)).encode('utf-8')[:size]
            self._send_bytes_response(200, 'application/octet-stream', payload)
            return True
        if path.startswith('/stream-bytes/'):
            size = self._safe_int(path.split('/')[-1] or '0', minimum=0, maximum=1024 * 1024)
            chunk_size = self._safe_int(query_params.get('chunk_size', ['1024'])[0], default=1024, minimum=1)
            payload = ('0123456789abcdef' * ((size // 16) + 1)).encode('utf-8')[:size]
            chunks = [payload[i:i + chunk_size] for i in range(0, len(payload), chunk_size)] or [b'']
            self._send_chunked_response(200, 'application/octet-stream', chunks, extra_headers={'X-Chunk-Size': str(chunk_size)})
            return True
        if path.startswith('/delay/'):
            delay = self._safe_float(path.split('/')[-1] or '0', default=0.0, minimum=0.0, maximum=10.0)
            if delay > 0:
                time.sleep(delay)
            self._send_json_bytes(self._build_httpbin_request_response(method, parsed_path, query_params, headers, body))
            return True
        if path.startswith('/redirect/'):
            remaining = self._safe_int(path.split('/')[-1] or '0', minimum=0)
            location = '/get' if remaining <= 1 else f'/redirect/{remaining - 1}'
            self._send_bytes_response(302, 'text/plain; charset=utf-8', b'', extra_headers={'Location': location})
            return True
        if path.startswith('/relative-redirect/'):
            remaining = self._safe_int(path.split('/')[-1] or '0', minimum=0)
            location = '/get' if remaining <= 1 else f'/relative-redirect/{remaining - 1}'
            self._send_bytes_response(302, 'text/plain; charset=utf-8', b'', extra_headers={'Location': location})
            return True
        if path.startswith('/absolute-redirect/'):
            remaining = self._safe_int(path.split('/')[-1] or '0', minimum=0)
            location = self._httpbin_redirect_location('/get' if remaining <= 1 else f'/absolute-redirect/{remaining - 1}')
            self._send_bytes_response(302, 'text/plain; charset=utf-8', b'', extra_headers={'Location': location})
            return True
        if path == '/redirect-to':
            status_code = self._safe_int(query_params.get('status_code', ['302'])[0], default=302, minimum=300, maximum=399)
            location = self._httpbin_redirect_location(query_params.get('url', ['/get'])[0])
            self._send_bytes_response(status_code, 'text/plain; charset=utf-8', b'', extra_headers={'Location': location})
            return True
        if path == '/gzip':
            payload = json.dumps({
                "gzipped": True,
                "headers": headers,
                "method": method,
                "origin": self._httpbin_origin(),
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
        if path == '/brotli':
            payload = json.dumps({
                "brotli": True,
                "headers": headers,
                "method": method,
                "origin": self._httpbin_origin(),
            }, ensure_ascii=False).encode('utf-8')
            compressed = self._compress_brotli(payload)
            if compressed is None:
                self._send_json_bytes(
                    {"brotli": False, "fallback": "brotli command unavailable"},
                    status_code=501,
                )
            else:
                self._send_bytes_response(
                    200,
                    'application/json; charset=utf-8',
                    compressed,
                    extra_headers={'Content-Encoding': 'br'},
                )
            return True
        if path == '/deflate':
            import zlib
            payload = json.dumps({
                "deflated": True,
                "headers": headers,
                "method": method,
                "origin": self._httpbin_origin(),
            }, ensure_ascii=False).encode('utf-8')
            self._send_bytes_response(
                200,
                'application/json; charset=utf-8',
                zlib.compress(payload),
                extra_headers={'Content-Encoding': 'deflate'},
            )
            return True
        if path == '/html':
            html = "<html><body><h1>httpbin mock html</h1><p>forwarded</p></body></html>"
            self._send_bytes_response(200, 'text/html; charset=utf-8', html.encode('utf-8'))
            return True
        if path == '/image':
            accept = self.headers.get('Accept', '').lower()
            if 'image/jpeg' in accept:
                self._send_bytes_response(200, 'image/jpeg', JPEG_1X1_BYTES)
            else:
                self._send_bytes_response(200, 'image/png', PNG_1X1_BYTES)
            return True
        if path == '/image/png':
            self._send_bytes_response(200, 'image/png', PNG_1X1_BYTES)
            return True
        if path == '/image/jpeg':
            self._send_bytes_response(200, 'image/jpeg', JPEG_1X1_BYTES)
            return True
        if path == '/xml':
            xml = """<?xml version="1.0" encoding="utf-8"?>
<slideshow title="Sample Slide Show" date="2026-03-27" author="Bifrost">
  <slide type="all">
    <title>httpbin mock xml</title>
  </slide>
</slideshow>"""
            self._send_bytes_response(200, 'application/xml; charset=utf-8', xml.encode('utf-8'))
            return True
        if path == '/robots.txt':
            self._send_bytes_response(200, 'text/plain; charset=utf-8', b'User-agent: *\nDisallow: /deny\n')
            return True
        if path == '/deny':
            self._send_bytes_response(200, 'text/plain; charset=utf-8', b'YOU SHOULD NOT BE HERE')
            return True
        if path == '/uuid':
            self._send_json_bytes({"uuid": "00000000-0000-4000-8000-000000000000"})
            return True
        if path.startswith('/base64/'):
            encoded = path[len('/base64/'):]
            padding = '=' * (-len(encoded) % 4)
            try:
                decoded = base64.urlsafe_b64decode((encoded + padding).encode('ascii'))
            except Exception:
                self._send_bytes_response(400, 'text/plain; charset=utf-8', b'invalid base64 input')
                return True
            self._send_bytes_response(200, 'text/plain; charset=utf-8', decoded)
            return True
        if path == '/response-headers':
            body_payload = {}
            extra_headers = []
            for key, value in args.items():
                extra_headers.append((key, value))
                body_payload[key] = value
            self._send_json_bytes(body_payload, extra_headers=extra_headers)
            return True
        if path.startswith('/cache/'):
            ttl = self._safe_int(path.split('/')[-1] or '0', default=60, minimum=0)
            self._send_json_bytes(
                {"args": args, "cache": ttl, "url": url},
                extra_headers={'Cache-Control': f'public, max-age={ttl}'},
            )
            return True
        if path.startswith('/range/'):
            size = self._safe_int(path.split('/')[-1] or '0', default=0, minimum=0, maximum=1024 * 1024)
            payload = ('abcdefghijklmnopqrstuvwxyz' * ((size // 26) + 1)).encode('utf-8')[:size]
            range_header = self.headers.get('Range', '')
            if range_header.startswith('bytes=') and payload:
                start_s, _, end_s = range_header[len('bytes='):].partition('-')
                start = self._safe_int(start_s, default=0, minimum=0, maximum=max(0, size - 1))
                end = self._safe_int(end_s, default=size - 1, minimum=start, maximum=max(0, size - 1))
                partial = payload[start:end + 1]
                self._send_bytes_response(
                    206,
                    'application/octet-stream',
                    partial,
                    extra_headers={
                        'Accept-Ranges': 'bytes',
                        'Content-Range': f'bytes {start}-{end}/{size}',
                    },
                )
            else:
                self._send_bytes_response(
                    200,
                    'application/octet-stream',
                    payload,
                    extra_headers={'Accept-Ranges': 'bytes'},
                )
            return True
        if path.startswith('/etag/'):
            etag = path[len('/etag/'):] or 'mock-etag'
            normalized = f'"{etag}"'
            if self.headers.get('If-None-Match') == normalized:
                self._send_bytes_response(304, 'application/json; charset=utf-8', b'', extra_headers={'ETag': normalized})
            else:
                self._send_json_bytes({"etag": etag}, extra_headers={'ETag': normalized})
            return True
        if path == '/cache':
            self._send_json_bytes(
                {"args": args, "headers": headers, "url": url},
                extra_headers={'Cache-Control': 'public, max-age=60', 'ETag': '"mock-cache"'},
            )
            return True
        if path == '/drip':
            duration = self._safe_float(query_params.get('duration', ['0'])[0], default=0.0, minimum=0.0, maximum=10.0)
            numbytes = self._safe_int(query_params.get('numbytes', ['0'])[0], default=0, minimum=0, maximum=1024 * 1024)
            delay = self._safe_float(query_params.get('delay', ['0'])[0], default=0.0, minimum=0.0, maximum=10.0)
            if delay > 0:
                time.sleep(delay)
            payload = (b'd' * numbytes) or b'drip'
            chunk_count = max(1, min(len(payload), self._safe_int(query_params.get('numbytes', ['1'])[0], default=1, minimum=1)))
            chunk_size = max(1, len(payload) // chunk_count)
            chunks = []
            for idx in range(0, len(payload), chunk_size):
                chunks.append(payload[idx:idx + chunk_size])
                if duration > 0:
                    time.sleep(duration / max(1, len(payload[idx:]) // chunk_size + 1))
            self._send_chunked_response(200, 'application/octet-stream', chunks)
            return True
        if path == '/stream' or path.startswith('/stream/'):
            count_source = path.split('/')[-1] if path.startswith('/stream/') else query_params.get('n', ['20'])[0]
            line_count = self._safe_int(count_source, default=20, minimum=1, maximum=100)
            lines = []
            for idx in range(line_count):
                lines.append(json.dumps({
                    "id": idx,
                    "args": args,
                    "headers": headers,
                    "origin": self._httpbin_origin(),
                    "url": url,
                }, ensure_ascii=False))
            self._send_chunked_response(
                200,
                'application/jsonl; charset=utf-8',
                [(line + '\n').encode('utf-8') for line in lines],
            )
            return True
        if path == '/sse':
            count = self._safe_int(query_params.get('count', ['3'])[0], default=3, minimum=1, maximum=100)
            delay = self._safe_float(query_params.get('delay', ['0'])[0], default=0.0, minimum=0.0, maximum=10.0)
            chunks = []
            for idx in range(count):
                chunks.append(f"event: message\ndata: {{\"id\": {idx}, \"host\": \"httpbin.org\"}}\n\n".encode('utf-8'))
                if delay > 0:
                    time.sleep(delay)
            self._send_chunked_response(200, 'text/event-stream; charset=utf-8', chunks)
            return True
        if path == '/get':
            self._send_json_bytes(self._build_httpbin_request_response(method, parsed_path, query_params, headers))
            return True
        if path == '/anything' or path.startswith('/anything/'):
            self._send_json_bytes(self._build_httpbin_body_response(method, parsed_path, query_params, headers, body))
            return True
        if path in ('/post', '/put', '/patch', '/delete'):
            self._send_json_bytes(self._build_httpbin_body_response(method, parsed_path, query_params, headers, body))
            return True
        return False

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
        if self._handle_httpbin_common("GET"):
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
        if self._handle_httpbin_common("POST", body):
            return
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
        if self._handle_httpbin_common("PUT", body):
            return
        self._send_response(body_content=body)

    def do_DELETE(self):
        """处理 DELETE 请求"""
        self._log_request_start("DELETE")
        print(f"Headers:")
        for key, value in self.headers.items():
            print(f"  {key}: {value}")
        if self._handle_httpbin_common("DELETE"):
            return
        self._send_response()

    def do_PATCH(self):
        """处理 PATCH 请求"""
        self._log_request_start("PATCH")
        body = self._read_body()
        if self._handle_httpbin_common("PATCH", body):
            return
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
