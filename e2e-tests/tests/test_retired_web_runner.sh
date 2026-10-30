#!/usr/bin/env bash
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
BIFROST_BIN="${BIFROST_BIN:-$REPO_DIR/target/debug/bifrost}"
TEST_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/bifrost-retired-runner.XXXXXX")"
export BIFROST_DATA_DIR="$TEST_ROOT/data"
source "$REPO_DIR/e2e-tests/test_utils/process.sh"
mark_e2e_data_root "$BIFROST_DATA_DIR"
BIFROST_PID=""
cleanup() {
  local result=$?
  if [[ -n "$BIFROST_PID" ]]; then kill "$BIFROST_PID" 2>/dev/null || true; wait "$BIFROST_PID" 2>/dev/null || true; fi
  if [[ $result -ne 0 ]]; then tail -60 "$TEST_ROOT/service.log" >&2 || true; fi
  rm -rf "$TEST_ROOT"
}
trap cleanup EXIT
PORT="$(python3 - <<'PY'
import socket
with socket.socket() as sock:
    sock.bind(('127.0.0.1', 0))
    print(sock.getsockname()[1])
PY
)"
python3 - "$BIFROST_DATA_DIR" <<'PY'
import json,pathlib,sys
root=pathlib.Path(sys.argv[1]); config=root/'admin/im_gateway_external_cli_agent.json'
config.parent.mkdir(parents=True,exist_ok=True)
config.write_text(json.dumps({'version':2,'defaultRunnerId':'web','runners':{'web':{'enabled':True,'adapter':'chatgpt_web','adapterConfig':{}},'web-b':{'enabled':True,'adapter':'gpt-web','adapterConfig':{}}},'channels':{'alpha':{'runnerId':'web'},'beta':{'runnerId':'web-b'}}}))
legacy=root/'agent/im_gateway/chatgpt_web';legacy.mkdir(parents=True,exist_ok=True)
for name in ['auth_state.json','history.jsonl','browser_profile/sentinel']:
    file=legacy/name;file.parent.mkdir(parents=True,exist_ok=True);file.write_text('preserve '+name)
PY
BIFROST_E2E=1 "$BIFROST_BIN" start --host 127.0.0.1 -p "$PORT" --no-system-proxy --skip-cert-check --unsafe-ssl >"$TEST_ROOT/service.log" 2>&1 &
BIFROST_PID=$!
python3 - "$PORT" "$BIFROST_BIN" "$BIFROST_DATA_DIR" <<'PY'
import json,pathlib,subprocess,sys,time,urllib.request,urllib.error
port,binary,data=sys.argv[1:];base=f'http://127.0.0.1:{port}/_bifrost/api'
client=urllib.request.build_opener(urllib.request.ProxyHandler({}))
def request(path, method='GET', body=None, expected=200):
    req=urllib.request.Request(base+path,data=json.dumps(body).encode() if body is not None else None,method=method,headers={'Content-Type':'application/json','X-Bifrost-CSRF':'1'})
    try: response=client.open(req,timeout=60)
    except urllib.error.HTTPError as error: response=error
    raw=response.read().decode();assert response.code==expected,(path,response.code,raw)
    return json.loads(raw)
for _ in range(200):
    try: request('/proxy/address');break
    except Exception: time.sleep(.1)
else: raise AssertionError('service failed to become ready')
config=request('/im-gateway/chat/config');assert config['runners']['web']['adapter']=='chatgpt_web'
request('/im-gateway/chat/config','PATCH',config)
invalid=json.loads(json.dumps(config));invalid['runners']['new-web']=config['runners']['web']
request('/im-gateway/chat/config','PATCH',invalid,400)
supported={'enabled':True,'adapter':'custom_cli','adapterConfig':{'executable':sys.executable,'args':['-c','import sys; sys.stdin.read(); print("SUPPORTED_RUNNER_OK")']},'deliveryMode':'no_im'}
config['runners']['replacement']=supported
request('/im-gateway/chat/config','PATCH',config)
assert request('/im-gateway/chat','POST',{'runnerId':'replacement','message':'hello'})['status']=='succeeded'
for suffix in ['', '/stream']:
    error=request('/im-gateway/chat'+suffix,'POST',{'runnerId':'web','message':'do not run','sessionKey':'old-session'},400)
    assert 'removed' in error['error'] and 'explicitly' in error['error'],error
for action in ['status','open','stop']:
    request('/im-gateway/chat/adapters/chatgpt-web/auth/'+action,'POST',{},410)
request('/workers/browser/start','POST',{},404)
assert all(mode['workerKind']!='browser' for mode in request('/workers/modes'))
help_output=subprocess.run([binary,'agent','run','--help'],text=True,capture_output=True,check=True).stdout
assert 'chatgpt-web' not in help_output and 'ChatGPT Web' not in help_output
# Global proxy port flag must be before the subcommand.
run=subprocess.run([binary,'-p',port,'agent','run','--runner','web','--json','hello'],text=True,capture_output=True,timeout=30)
assert run.returncode!=0 and 'removed' in run.stderr,(run.returncode,run.stdout,run.stderr)
# Explicit migration to a supported custom executable; no automatic provider change.
config['runners']['web']={'enabled':True,'adapter':'custom_cli','adapterConfig':{'executable':sys.executable,'args':['-c','import sys; sys.stdin.read(); print("SUPPORTED_RUNNER_OK")']},'deliveryMode':'no_im'}
request('/im-gateway/chat/config','PATCH',config)
assert request('/im-gateway/chat/config')['runners']['web-b']['adapter']=='gpt-web'
assert 'removed' in request('/im-gateway/chat','POST',{'runnerId':'web-b','message':'no'},400)['error']
assert request('/im-gateway/chat/config')['channels']['beta']['runnerId']=='web-b'
# A deleted arbitrary ID must fail explicitly, even if its spelling is not a retired adapter alias.
config['runners'].pop('web-b')
request('/im-gateway/chat/config','PATCH',config)
assert 'does not exist' in request('/im-gateway/chat','POST',{'runnerId':'web-b','message':'no'},400)['error']
config['runners']['web-b']=supported
request('/im-gateway/chat/config','PATCH',config)
result=request('/im-gateway/chat','POST',{'runnerId':'web','message':'hello','sessionKey':'supported-runner'})
assert result['status']=='succeeded' and 'SUPPORTED_RUNNER_OK' in result['response'],result
legacy=pathlib.Path(data)/'agent/im_gateway/chatgpt_web'
for name in ['auth_state.json','history.jsonl','browser_profile/sentinel']:
    assert (legacy/name).read_text()=='preserve '+name,name
print('[retired-web-runner] PASS: API, CLI, explicit migration, supported execution, data preservation')
PY
