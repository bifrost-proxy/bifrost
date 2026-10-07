#!/usr/bin/env bash
set -euo pipefail
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
BIFROST_BIN="${BIFROST_BIN:-$ROOT_DIR/target/debug/bifrost}"
python3 - "$BIFROST_BIN" "$ROOT_DIR" <<'PY'
import base64
import hashlib
import json
import os
from pathlib import Path
import re
import socket
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.parse
import urllib.request

binary, root = sys.argv[1:]
with tempfile.TemporaryDirectory(prefix='.bifrost-e2e-share-verify-', dir=root) as temporary:
    data = Path(temporary)
    env = dict(os.environ, BIFROST_DATA_DIR=str(data), BIFROST_DISABLE_TRAY='1',
               BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT='1')
    def cli(*args):
        return subprocess.run([binary, *args], env=env, capture_output=True, text=True, timeout=30)
    def share(content=None, file=None):
        args = ['rule', 'share', 'verify-e2e', 'https://example.test/app?site=1']
        args += ['--content', content] if content is not None else ['--file', file]
        result = cli(*args)
        assert result.returncode == 0, result.stderr
        return result.stdout.strip()
    # Verify is read-only even before any local rule storage exists.
    result = cli('rule', 'verify', 'https://example.test/', '--json')
    assert result.returncode != 0 and not list(data.iterdir())
    assert json.loads(result.stdout)['valid'] is False
    result = cli('rule', 'share', 'broken', 'https://example.test/', '--file',
                 str(Path(root) / 'e2e-tests/rules/share/verify_invalid.txt'))
    assert result.returncode != 0 and not result.stdout.strip()
    assert 'line 2:' in result.stderr and 'Suggestion:' in result.stderr
    url = share(file=str(Path(root) / 'e2e-tests/rules/share/verify_valid.txt'))
    result = cli('rule', 'verify', url, '--json')
    verified = json.loads(result.stdout)
    assert result.returncode == 0 and verified['valid'] is True
    assert verified['target_url'] == 'https://example.test/app?site=1'
    result = cli('rule', 'verify', url)
    assert result.returncode == 0 and 'Valid rule share link' in result.stdout
    encoded = urllib.parse.parse_qs(urllib.parse.urlsplit(url).query)['__bifrost_rule'][0]
    payload = json.loads(base64.urlsafe_b64decode(encoded + '=' * (-len(encoded) % 4)))
    def encode(value):
        return base64.urlsafe_b64encode(json.dumps(value).encode()).decode().rstrip('=')
    bad_hash = dict(payload, content=payload['content'] + '\nchanged.test status://201')
    invalid_syntax = dict(payload, content='example.test unknownProtocol://x')
    invalid_syntax['content_hash'] = hashlib.sha256(invalid_syntax['content'].encode()).hexdigest()
    failures = [(encode(bad_hash), 'hash mismatch'), (encode(invalid_syntax), 'line 1:'),
                ('!', 'base64'), ('e30', 'missing field')]
    for value, expected in failures:
        broken = 'https://example.test/app?' + urllib.parse.urlencode({'__bifrost_rule': value})
        result = cli('rule', 'verify', broken, '--json')
        report = json.loads(result.stdout)
        assert result.returncode != 0 and not report['valid']
        assert expected in report['error'] and report['next_action']
    print('PASS generation diagnostics and offline verify JSON/text/read-only checks')
    (data / 'config.toml').write_text('[sync]\nenabled = false\nauto_sync = false\n')
    result = cli('rule', 'add', 'stored-invalid', '--file',
                 str(Path(root) / 'e2e-tests/rules/share/verify_invalid.txt'), '--allow-invalid')
    assert result.returncode == 0, result.stderr
    with socket.socket() as sock:
        sock.bind(('127.0.0.1', 0))
        port = sock.getsockname()[1]
    with (data / 'proxy.log').open('w') as log:
        process = subprocess.Popen([binary, 'start', '-p', str(port), '--host', '127.0.0.1',
            '--skip-cert-check', '--no-system-proxy', '--no-intercept',
            '--intercept-include', 'example.test', '-y'], env=env, stdout=log, stderr=log)
        try:
            admin = f'http://127.0.0.1:{port}/_bifrost'
            opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
            for _ in range(160):
                try:
                    before = opener.open(admin + '/api/rules', timeout=1).read()
                    break
                except (OSError, urllib.error.URLError):
                    assert process.poll() is None, (data / 'proxy.log').read_text()[-3000:]
                    time.sleep(.25)
            else:
                raise AssertionError('proxy did not become ready')
            ca = next(data.rglob('ca.crt'))
            for scheme in ['http', 'https']:
                for value, expected in failures:
                    broken = f'{scheme}://example.test/app?' + urllib.parse.urlencode({'__bifrost_rule': value})
                    result = subprocess.run(['curl', '-sS', '--max-time', '15', '--noproxy', '',
                        '-x', f'http://127.0.0.1:{port}', '--cacert', str(ca),
                        '-w', '\n%{http_code}', broken], capture_output=True, text=True, timeout=20)
                    assert result.returncode == 0, result.stderr
                    body, status = result.stdout.rsplit('\n', 1)
                    assert status == '400' and expected in body, (scheme, status, body)
                    assert 'Unable to apply shared Bifrost rule' in body
                    assert 'bifrost rule verify' in body
            encoded_key_url = 'http://example.test/app?%5F%5Fbifrost_rule=!'
            result = subprocess.run(['curl', '-sS', '--max-time', '15', '--noproxy', '',
                '-x', f'http://127.0.0.1:{port}', '-w', '\n%{http_code}', encoded_key_url],
                capture_output=True, text=True, timeout=20)
            assert result.returncode == 0 and result.stdout.endswith('400')
            assert 'Unable to apply shared Bifrost rule' in result.stdout
            assert opener.open(admin + '/api/rules').read() == before, 'invalid consumption changed rules'
            confirm = admin + '/share/rule?' + urllib.parse.urlencode({
                'payload': encode(bad_hash), 'target': 'https://example.test/app'})
            try:
                opener.open(confirm)
                raise AssertionError('bad confirmation page accepted')
            except urllib.error.HTTPError as error:
                assert error.code == 400 and b'hash mismatch' in error.read()
            # Apply must revalidate even when callers bypass the confirmation page.
            good_confirm = admin + '/share/rule?' + urllib.parse.urlencode({
                'payload': encoded, 'target': 'https://example.test/app'})
            page = opener.open(good_confirm).read().decode()
            csrf = json.loads(re.search(r'const csrfToken = (.*);', page).group(1))
            request = urllib.request.Request(admin + '/api/rules/share-confirm', method='POST',
                data=json.dumps({'payload': encode(bad_hash), 'target_url': 'https://example.test/app'}).encode(),
                headers={'Content-Type': 'application/json', 'X-Bifrost-CSRF': csrf})
            try:
                opener.open(request)
                raise AssertionError('bad Apply accepted')
            except urllib.error.HTTPError as error:
                assert error.code == 400 and b'hash mismatch' in error.read()
            assert opener.open(admin + '/api/rules').read() == before
            request = urllib.request.Request(admin + '/api/rules/share-link', method='POST',
                data=json.dumps({'name': 'stored-invalid', 'target_url': 'https://example.test/app'}).encode(),
                headers={'Content-Type': 'application/json', 'X-Bifrost-CSRF': csrf})
            try:
                opener.open(request)
                raise AssertionError('generation API accepted invalid syntax')
            except urllib.error.HTTPError as error:
                assert error.code == 400 and b'line 2:' in error.read()
            assert opener.open(admin + '/api/rules').read() == before
            print('PASS HTTP/HTTPS visible error pages, confirmation and Apply revalidation; rules unchanged')
        finally:
            process.terminate()
            try:
                process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=10)
print('rule share verify E2E passed')
PY
