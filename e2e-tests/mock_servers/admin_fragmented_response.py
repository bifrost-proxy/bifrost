"""One real loopback config response, split after headers to exercise body reads."""
import argparse
import json
import socket
import time
from pathlib import Path

parser = argparse.ArgumentParser()
parser.add_argument('--port-file', required=True)
parser.add_argument('--request-file', required=True)
args = parser.parse_args()
body = json.dumps({
    'timeout_secs': 30, 'http1_max_header_size': 8192,
    'http2_max_header_list_size': 16384, 'websocket_handshake_max_header_size': 4096,
}).encode()
with socket.socket() as server:
    server.bind(('127.0.0.1', 0))
    server.listen(1)
    server.settimeout(10)
    Path(args.port_file).write_text(str(server.getsockname()[1]))
    connection, _ = server.accept()
    with connection:
        connection.settimeout(10)
        request = b''
        while b'\r\n\r\n' not in request:
            chunk = connection.recv(4096)
            if not chunk:
                raise RuntimeError('request closed before complete headers')
            request += chunk
            if len(request) > 65536:
                raise RuntimeError('request headers exceed fixture limit')
        Path(args.request_file).write_text(request.split(b'\r\n', 1)[0].decode())
        connection.sendall(f'HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {len(body)}\r\n\r\n'.encode())
        time.sleep(0.05)
        connection.sendall(body)
