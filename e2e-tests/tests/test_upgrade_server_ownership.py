#!/usr/bin/env python3
"""Real macOS Desktop + CLI upgrade handoff, isolated ports/data, no system proxy."""
import json
import os
from pathlib import Path
import shutil
import socket
import subprocess
import tempfile
import time
import urllib.request

ROOT = Path(__file__).resolve().parents[2]
CLI = Path(os.environ.get('BIFROST_BIN', ROOT / 'target/debug/bifrost')).resolve()
APP = Path(os.environ.get('BIFROST_DESKTOP_APP_BIN', ROOT / 'desktop/src-tauri/target/debug/bifrost-desktop')).resolve()
HTTP = urllib.request.build_opener(urllib.request.ProxyHandler({}))


def free_port():
    with socket.socket() as sock:
        sock.bind(('127.0.0.1', 0))
        return sock.getsockname()[1]


def wait_until(check, seconds=30):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        if check():
            return
        time.sleep(.1)
    raise AssertionError('timed out waiting for scenario assertion')


def system(port):
    try:
        with HTTP.open(f'http://127.0.0.1:{port}/_bifrost/api/system', timeout=.5) as response:
            return json.load(response)
    except (OSError, ValueError):
        return None


def scenario(name, cli_running, target_matches, desktop_owned=False, late_cli=False):
    root = Path(tempfile.mkdtemp(prefix='.bifrost-e2e-upgrade-owner-', dir=ROOT))
    data = root / 'data'
    data.mkdir()
    port = free_port()
    env = dict(os.environ, BIFROST_DATA_DIR=str(data), BIFROST_DESKTOP_BIN=str(CLI),
               BIFROST_DESKTOP_TEST_ALLOW_MULTIPLE_INSTANCES='1',
               BIFROST_DESKTOP_NO_SYSTEM_PROXY='1', BIFROST_DESKTOP_SKIP_CERT_PREFLIGHT='1',
               BIFROST_DISABLE_TRAY='1', BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT='1',
               BIFROST_SYSTEM_PROXY_DISABLE_LAUNCHD_INSTALL='1',
               BIFROST_SYSTEM_PROXY_DISABLE_LIFECYCLE_HELPER='1', BIFROST_EXTERNAL_CLI_WORKER='0')
    env.pop('BIFROST_DETACHED_DAEMON_CHILD', None)
    app = None
    log_handle = (root / 'app.log').open('w')
    try:
        version = subprocess.check_output([str(CLI), '--version'], env=env, text=True).split()[-1]
        old_pid = None
        if cli_running:
            subprocess.run([str(CLI), 'start', '-d', '-p', str(port), '--host', '127.0.0.1',
                            '--skip-cert-check', '--no-system-proxy', '--no-tray', '-y'],
                           env=env, stdout=log_handle, stderr=log_handle, check=True, timeout=90)
            wait_until(lambda: system(port))
            old_pid = system(port)['pid']
        marker = dict(schema_version=1, created_at_ms=int(time.time()*1000), old_app_pid=0,
                      old_core_pid=99999999 if desktop_owned else None,
                      observed_external_core_pid=old_pid, proxy_port=port, app_target=str(APP),
                      target_version=version if target_matches else '99.99.99')
        (data / 'desktop-upgrade-relaunch.json').write_text(json.dumps(marker))
        # Deliberately different preference: upgrade must honor the runtime snapshot.
        (data / 'desktop-config.json').write_text(json.dumps(dict(proxy_port=free_port())))
        app = subprocess.Popen([str(APP)], env=env, stdout=log_handle, stderr=log_handle)
        bootstrap = data / 'logs/desktop-bootstrap.log'
        def logs():
            return bootstrap.read_text() if bootstrap.exists() else ''
        if target_matches and not late_cli:
            wait_until(lambda: 'desktop backend start succeeded' in logs(), 45)
            runtime = json.loads((data / 'runtime.json').read_text())
            assert runtime['port'] == port, runtime
            assert system(port)['pid'] == runtime['pid']
            if desktop_owned:
                assert runtime['runtime_start_mode'] == 'desktop', runtime
                assert logs().count('starting sidecar;') == 1
            else:
                assert runtime['pid'] == old_pid, runtime
                assert runtime['runtime_start_mode'] == 'daemon', runtime
                assert 'starting sidecar;' not in logs()
        else:
            wait_until(lambda: 'CLI-owned backend did not restart' in logs(), 50)
            assert 'starting sidecar;' not in logs()
            if cli_running:
                assert system(port)['pid'] == old_pid
            else:
                assert system(port) is None
        if late_cli:
            subprocess.run([str(CLI), 'start', '-d', '-p', str(port), '--host', '127.0.0.1',
                            '--skip-cert-check', '--no-system-proxy', '--no-tray', '-y'],
                           env=env, stdout=log_handle, stderr=log_handle, check=True, timeout=90)
            wait_until(lambda: 'recovered CLI-owned upgrade handoff from healthy target backend' in logs(), 45)
            assert not (data / 'desktop-upgrade-relaunch.json').exists()
            assert 'starting sidecar;' not in logs()
            assert json.loads((data / 'runtime.json').read_text())['runtime_start_mode'] == 'daemon'
        assert app.poll() is None, 'Desktop must remain available to show status'
        print(f'PASS {name}: original_port={port}, cli_pid={old_pid}', flush=True)
    except Exception:
        print(f'FAIL {name}; evidence retained at {root}', flush=True)
        raise
    finally:
        if app and app.poll() is None:
            app.terminate()
            try:
                app.wait(timeout=10)
            except subprocess.TimeoutExpired:
                app.kill()
                app.wait()
        subprocess.run([str(CLI), 'stop'], env=dict(env, BIFROST_DESKTOP_AUTHORIZED_STOP_INTERNAL='1'),
                       stdout=log_handle, stderr=log_handle, timeout=45)
        log_handle.close()
    shutil.rmtree(root)


if __name__ == '__main__':
    if os.uname().sysname != 'Darwin':
        raise SystemExit('macOS Desktop scenario requires macOS')
    scenario('CLI-owned ready server is reused without restart', True, True)
    scenario('Desktop-owned relaunch preserves custom runtime port', False, True, True)
    scenario('CLI restart failure on free port does not transfer ownership', False, False)
    scenario('CLI wrong-version server is preserved', True, False)

    scenario('Late CLI readiness recovers automatically on original port', False, True, late_cli=True)
