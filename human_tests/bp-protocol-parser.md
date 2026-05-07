# BP 协议脚本解析真实场景测试

## 功能模块说明

验证 `bp://<script>` 绑定 parser 脚本并配合 `decode://bp` 对请求/响应体做落库前解析的用户可感知行为。覆盖本地 parser 脚本、远程 parser 脚本下载缓存、Traffic 详情 decoded body 展示、raw body 保留，以及 `bifrost search` 搜索解析后的内容。

## 前置条件

- 在仓库根目录 `/Users/eden/work/github/bifrost` 执行。
- 每条 shell 命令先执行 `source ~/.zshrc`。
- 不使用 9900 端口，不修改系统代理；启动 Bifrost 必须使用临时 `BIFROST_DATA_DIR` 和 `--no-system-proxy`。
- 使用最新源码构建的 `target/release/bifrost`。

## 测试用例列表

### TC-BP-01 本地 parser 脚本解析响应体并在 Traffic 详情展示 decoded body

操作步骤：
1. 执行 `source ~/.zshrc && cargo build --release --bin bifrost`。
2. 执行 `source ~/.zshrc && SKIP_BUILD=true bash e2e-tests/tests/test_bp_parser_e2e.sh`。
3. 观察脚本输出中的本地 parser 断言。

预期结果：
- 输出包含 `client should still receive upstream body for local parser` 通过。
- 输出包含 `decode://bp must not rewrite client response` 通过。
- 输出包含 `response-body should expose local decoded parser output` 通过。
- 输出包含 `raw response body should remain upstream body` 通过。

### TC-BP-02 远程 parser 脚本自动下载、校验并缓存后执行

操作步骤：
1. 继续观察 `e2e-tests/tests/test_bp_parser_e2e.sh` 的远程 parser 断言。
2. 确认脚本使用 `bp://http://127.0.0.1:<port>/remote_echo.js?sha256=<sha>` 规则。

预期结果：
- 输出包含 `client should still receive upstream body for remote parser` 通过。
- 输出包含 `response-body should expose remote decoded parser output` 通过。
- 输出包含 `remote parser should be cached after first execution` 通过。

### TC-BP-02A 远程 parser 下载超时后返回明确错误且不污染缓存

操作步骤：
1. 执行 `source ~/.zshrc && cargo test -p bifrost-script test_remote_parser_download_timeout_uses_network_timeout -- --nocapture`。
2. 观察测试输出，确认测试在短时间内失败返回，而不是长时间挂起。
3. 确认断言覆盖了两点：
   - 错误消息包含 `remote parser download failed`，表示超时发生在远程下载阶段。
   - 临时 `scripts/_remote-cache/parser` 目录下没有遗留 `.js` 缓存文件。

预期结果：
- 单测通过。
- 超时路径返回明确的远程下载失败错误，而不是静默卡住或返回无关错误。
- 下载超时后不会写入远程 parser 缓存文件，避免污染后续请求。

### TC-BP-02B 远程 parser 响应体超过脚本大小上限时拒绝并且不污染缓存

操作步骤：
1. 执行 `source ~/.zshrc && cargo test -p bifrost-script test_remote_parser_download_rejects_body_over_script_limit -- --nocapture`。
2. 观察测试输出，确认 mock 远端返回没有 `Content-Length` 的超大响应体。
3. 确认断言覆盖了两点：
   - 错误消息包含 `remote parser script too large`，表示超限发生在远程下载读取阶段。
   - 临时 `scripts/_remote-cache/parser` 目录下没有遗留 `.js` 缓存文件。

预期结果：
- 单测通过。
- 远程 parser 下载复用本地脚本 8 MiB 上限，不能通过无 `Content-Length` 响应绕过。
- 响应体超过大小上限时不会写入远程 parser 缓存文件，避免污染后续请求。

### TC-BP-03 `bifrost search` 可以搜索解析后的响应体内容

操作步骤：
1. 继续观察 `e2e-tests/tests/test_bp_parser_e2e.sh` 中 search 断言。
2. 确认测试脚本通过 `bifrost search <decoded-marker> --res-body --format json` 查询。

预期结果：
- 输出包含 `bifrost search should find local decoded bp response body` 通过。
- 输出包含 `bifrost search should find remote decoded bp response body` 通过。
- search JSON 的 `total_matched` 大于 0。

### TC-BP-03A WebUI Search SSE 不丢失 decoded search 结果和完成事件

操作步骤：
1. 继续观察 `e2e-tests/tests/test_bp_parser_e2e.sh` 中 Search SSE 断言。
2. 确认测试脚本通过 `POST /_bifrost/api/search/stream` 搜索本地 parser decoded marker。
3. 检查 SSE 输出同时包含 `event: result`、decoded marker 和 `event: done`。

预期结果：
- 输出包含 `Search SSE should stream decoded bp response result and done event` 通过。
- 慢消费或 channel 满时，Search SSE 不应因为非阻塞发送失败而静默丢弃 decoded result、done 或 error。

### TC-BP-04 规则层保持 `bp://` 脚本引用语义并支持 profile 风格配置

操作步骤：
1. 执行 `source ~/.zshrc && cargo test -q -p bifrost-core bp_`。
2. 执行 `source ~/.zshrc && cargo test -q -p bifrost-cli bp_`。

预期结果：
- core 测试通过，确认 `bp://http://...` 不会被核心 resolver 提前下载为脚本内容。
- core 测试通过，确认 `bp://{profile}` 可以展开为脚本配置字符串。
- CLI 测试通过，确认 `bp://` 和 `decode://bp` 可以累积到 proxy 规则解析结果。

### TC-BP-04A 本地 parser 名称拒绝路径穿越

操作步骤：
1. 执行 `source ~/.zshrc && cargo test -p bifrost-script test_local_parser_ref_rejects_path_traversal_name -- --nocapture`。
2. 执行 `source ~/.zshrc && SKIP_BUILD=true bash e2e-tests/tests/test_bp_parser_e2e.sh`。
3. 观察 E2E 中 `bp-invalid.test bp://../local_echo decode://bp` 的断言。

预期结果：
- 单测返回 `InvalidName`，错误信息包含 `cannot contain '..'`。
- E2E 输出包含 `invalid local parser name should be rejected in traffic decode result` 通过。
- 客户端仍收到上游原始响应，Traffic 详情中 parser 结果标记失败，不能加载 `local_echo` 或其他 `scripts/parser` 外的 `.js` 文件。

### TC-BP-05 `docs/rules` 使用手册覆盖 bp 配置、IDL/PSM 与搜索说明

操作步骤：
1. 执行 `source ~/.zshrc && grep -n "## bp" docs/rules/scripts.md`。
2. 执行 `source ~/.zshrc && grep -n "bp://build_in_bp?idl=" docs/rules/scripts.md`。
3. 执行 `source ~/.zshrc && grep -n "psm=foo.bar.order" docs/rules/scripts.md`。
4. 执行 `source ~/.zshrc && grep -n "bifrost search" docs/rules/scripts.md`。
5. 执行 `source ~/.zshrc && grep -n "bp.*decode://bp" docs/rules/README.md`。
6. 执行 `source ~/.zshrc && test -f assets/scripts/parser/build_in_bp.js && grep -n "Bifrost BP parser adapter" assets/scripts/parser/build_in_bp.js`。

预期结果：
- `docs/rules/scripts.md` 包含独立 `bp` 手册章节。
- 手册包含本地 IDL 文件、PSM + BAM、远程 parser、Values 收敛长配置、`bifrost search --res-body` 示例。
- `docs/rules/README.md` 在脚本与 decode 分类中索引 `bp` + `decode://bp`。
- 仓库保留 `assets/scripts/parser/build_in_bp.js` 作为可复用的独立 parser adapter。

### TC-BP-06 使用 next_agent PSM 验证 `build_in_bp.js` 的 BAM 调用路径

操作步骤：
1. 执行 `source ~/.zshrc && grep -n "namespace go\\|service NextAgentService\\|Healthz" /Users/eden/work/code/nextoncall/next_agent/idl/flow.devops.next_agent.thrift`，确认 IDL 中存在 `flow.devops.next_agent` 命名空间和 `Healthz` 接口。
2. 使用 `HealthzRequest` 的空 Thrift struct body（单字节 `0x00`，base64 为 `AA==`），执行 Node sandbox harness 调用 `assets/scripts/parser/build_in_bp.js`：

```bash
source ~/.zshrc && node <<'NODE'
const fs = require('fs');
const cp = require('child_process');
const script = fs.readFileSync('assets/scripts/parser/build_in_bp.js', 'utf8');
global.ctx = {
  phase: 'request',
  scriptName: 'build_in_bp?psm=flow.devops.next_agent&pattern=%2Fapi%2Fv1%2Fhealthz&type=request&timeoutMs=10000'
};
global.request = { path: '/api/v1/healthz', bodyBase64: 'AA==' };
global.response = {};
global.file = { exists: () => false, readText: () => { throw new Error('unexpected file read'); } };
global.net = {
  fetch(url, optionsJson) {
    const options = JSON.parse(optionsJson || '{}');
    const args = ['-sS', '-i', '-X', options.method || 'GET'];
    for (const [key, value] of Object.entries(options.headers || {})) args.push('-H', `${key}: ${value}`);
    if (options.bodyBase64) {
      args.push('--data-binary', '@-');
      const body = Buffer.from(options.bodyBase64, 'base64');
      const output = cp.execFileSync('curl', args.concat(url), { input: body, encoding: 'utf8', timeout: options.timeoutMs || 10000 });
      const split = output.indexOf('\r\n\r\n');
      const header = split >= 0 ? output.slice(0, split) : '';
      return JSON.stringify({ ok: /^HTTP\\/\\S+ 2\\d\\d/m.test(header), status: Number((header.match(/^HTTP\\/\\S+ (\\d+)/m) || [])[1] || 0), body: split >= 0 ? output.slice(split + 4) : output });
    }
    const output = cp.execFileSync('curl', args.concat(url), { encoding: 'utf8', timeout: options.timeoutMs || 10000 });
    return JSON.stringify({ ok: true, status: 200, body: output });
  }
};
eval(script);
console.log(JSON.stringify(ctx.output));
NODE
```

预期结果：
- 第 1 步确认 `/Users/eden/work/code/nextoncall/next_agent/idl/flow.devops.next_agent.thrift` 是 Thrift IDL，包含 `namespace go flow.devops.next_agent`、`service NextAgentService` 和 `Healthz`。
- 因 `build_in_bp.js` 的本地 IDL 分支只实现 protobuf wire decode，next_agent Thrift IDL 必须通过 `psm=flow.devops.next_agent` 走 BAM binary_tools parse。
- 若未配置 `bamToken` 或 `bamTokenFile`，BAM 调用应返回明确的登录态错误（例如 `401 需要登陆`），不能假装解析成功。
- 配置有效 BAM cookie 后，同一 harness 应进入 `code: "0"` 并返回 BAM 的 `parse_result`。

### TC-BP-06A `build_in_bp.js` 与相关文档不暴露明文默认域名或历史依赖描述

操作步骤：
1. 执行以下检查命令：

```bash
source ~/.zshrc && node <<'NODE'
const fs = require('fs');
const files = [
  'assets/scripts/parser/build_in_bp.js',
  'docs/rules/scripts.md',
  'docs/scripts.md',
  'design/bp-protocol-parser.md',
  'human_tests/bp-protocol-parser.md',
];
const forbidden = [
  'w' + 'histle',
  'byte' + 'api.',
  'mag' + 'w.',
  'byted' + 'ance.',
  'paas-' + 'gw',
  'cloud.' + 'byted',
];
let failed = false;
for (const file of files) {
  const text = fs.readFileSync(file, 'utf8');
  for (const token of forbidden) {
    if (text.includes(token)) {
      console.error(`${file} still contains ${token}`);
      failed = true;
    }
  }
}
process.exit(failed ? 1 : 0);
NODE
```

预期结果：
- 检查命令退出码为 0。
- `assets/scripts/parser/build_in_bp.js` 仍保留默认地址能力，但默认地址只以 base64 常量形式出现在源码中。
- 相关使用手册与设计文档不再描述历史依赖包，也不出现明文默认地址。

### TC-BP-07 使用默认 Bifrost sync token 换取 server `bam_token`

操作步骤：
1. 创建临时 `BIFROST_DATA_DIR`，不要使用 9900 端口，不修改系统代理。
2. 将默认数据目录 `~/.bifrost/db/config.json` 中的同步登录 `token` 复制到临时数据目录的 `scripts/_sandbox/db/config.json`，只保留 `token` 字段。
3. 不要手动复制 `assets/scripts/parser/build_in_bp.js`，直接启动最新源码构建的 Bifrost：`BIFROST_DATA_DIR=<临时目录> target/release/bifrost -p <非9900端口> start -y --access-mode allow_all --skip-cert-check --unsafe-ssl --no-system-proxy --rules-file <规则文件>`。
4. 确认启动后临时数据目录出现 `scripts/parser/build_in_bp.js`。
5. 规则文件配置：

```txt
bp-real.test host://127.0.0.1:<upstream-port>
bp-real.test bp://build_in_bp?psm=flow.devops.next_agent&pattern=%2Fapi%2Fv1%2Fhealthz&type=request&timeoutMs=10000
bp-real.test decode://bp
```

6. 通过代理向 `http://bp-real.test/api/v1/healthz` 发送 body 为单字节 `0x00` 的 POST 请求。
7. 读取 Traffic 详情，检查 `decode_req_script_results`。

预期结果：
- `build_in_bp` 默认读取 `db/config.json` 的 `token`，调用默认同步鉴权信息接口换取 `data.bam_token`。
- Bifrost 自动释放的 `scripts/parser/build_in_bp.js` 被实际执行，不需要用户手工复制脚本。
- BAM parse 不再返回 `401 需要登陆`。
- 对 `flow.devops.next_agent` 当前返回 `参数错误, only support pb idl`，说明鉴权已通过但 `binary_tools/parse` 入口只支持 pb idl，不支持 next_agent 的 Thrift IDL。
- 客户端请求和原始 request body 仍保留，不因 parser 失败被改写。

### TC-BP-08 使用真实 next_agent Thrift RPC bytes 验证解包和搜索

操作步骤：
1. 使用 BAM Open API 文档中的只读接口确认测试服务元数据可用：
   - `/api/service/idl?psm=flow.devops.next_agent&version=1.0.77`
   - `/api/service/refschema?psm=flow.devops.next_agent&version=1.0.77&raw_field=1`
   - `/api/endpoint/info/example_code?psm=flow.devops.next_agent&rpc_method=Healthz&schema_type=resp&version=1.0.77&custom_protocol=rpc`
2. 执行 `source ~/.zshrc && cargo test -p bifrost-script test_build_in_bp_decodes_real_next_agent_thrift_rpc_bytes -- --nocapture`。
3. 执行 `source ~/.zshrc && SKIP_BUILD=true bash e2e-tests/tests/test_bp_parser_e2e.sh`。
4. 在 E2E 中确认 `bp-thrift.test` 用例发送的 request body 是真实 `flow.devops.next_agent.Healthz` Thrift CALL frame：`80010001000000074865616c74687a000000070c00010000`。
5. 在 E2E 中确认上游返回的 response body 是真实 `flow.devops.next_agent.Healthz` Thrift REPLY frame：`gAEAAgAAAAdIZWFsdGh6AAAABwwAAAsAAQAAAAJvawwA/wAAAA==`。
6. 读取 Traffic 的 `/request-body`，确认 decoded body 包含 `protocol=thrift-binary`、`schema_type=request`、`method=Healthz`。
7. 读取 Traffic 的 `/response-body`，确认 decoded body 包含 `protocol=thrift-binary`、`schema_type=response`、`method=Healthz`、`status=ok`。
8. 执行 `bifrost search Healthz --req-body --format json` 和 `bifrost search ok --res-body --format json`，确认均命中该记录。

预期结果：
- `build_in_bp?protocol=thrift` 不调用 pb-only `binary_tools/parse`，而是通过 BAM endpoint/refschema 元数据在 JS 内完成 Thrift binary 解包。
- 真实 Healthz RPC CALL/REPLY frame 被解析为 `method=Healthz`，响应数据包含 `status=ok`。
- 解码后的 request body 与 response body 都可以被 `bifrost search` 搜索到。
- 记录中 raw request body 与 raw response body 仍保留原始二进制，不改写客户端真实流量。
- 由于 Bifrost 当前 SOCKS5 / CONNECT 原始 tunnel 不落 TCP payload，真实 Kitex TCP tunnel 本身只验证为已知边界，不作为 decoded 搜索验收路径。

### TC-BP-09 使用二进制 request/response body 验证 WebUI raw 与 decoded 展示

操作步骤：
1. 使用线上 Bifrost sync token 换取 BAM token，调用真实 BAM `endpoint/list`、`endpoint/info?schema=ref`、`service/refschema?raw_field=1`，把 `flow.devops.next_agent.Healthz` 元数据落到临时数据目录的 `scripts/_sandbox/`。
2. 启动临时 Bifrost，必须使用非 9900 端口、临时 `BIFROST_DATA_DIR`、`--no-system-proxy` 和当前工作区代码。
3. Bifrost 规则配置：

```txt
bp-real-next-agent-binary.test host://127.0.0.1:<UPSTREAM_PORT>
bp-real-next-agent-binary.test bp://build_in_bp?protocol=thrift&psm=flow.devops.next_agent&version=1.0.77&method=Healthz&autoBamToken=false&endpointInfoFile=endpoint-info-healthz.json&refschemaFile=service-refschema.json
bp-real-next-agent-binary.test decode://bp
```

4. 通过临时 Bifrost 代理端口发起 `POST http://bp-real-next-agent-binary.test/thrift/healthz`，request body 必须是 `Healthz` Thrift CALL binary frame。
5. 上游 fixture 只能返回 `Healthz` Thrift REPLY binary frame，不返回 JSON wrapper。
6. 在 WebUI Traffic 详情中打开该记录，检查默认 request/response body 面板展示 decoded JSON。
7. 在 request Body 与 response Body 面板内切换 `Decoded / Raw`，确认 `Raw` 使用 Hex 展示解码前的原始二进制。
8. 使用 raw body ref/API 或落盘文件检查原始 request/response body 仍为二进制，request raw size 为 24 bytes，response raw size 为 37 bytes。
9. 执行 `bifrost search Healthz --req-body --format json` 和 `bifrost search ok --res-body --format json`。

预期结果：
- Traffic 命中 `bp://build_in_bp?protocol=thrift...` 和 `decode://bp`。
- `decode_req_script_results` 与 `decode_res_script_results` 中 `build_in_bp` 均执行成功。
- 默认 request body 展示 `schema_type=request`、`message.type=1`、`method=Healthz`。
- 默认 response body 展示 `schema_type=response`、`message.type=2`、`method=Healthz`、`status=ok`。
- Body 面板的 `Raw` 视图展示原始 Thrift CALL / REPLY bytes，不展示 decoded JSON，也不经过 UTF-8 lossy 重新编码；request hex 以 `80 01 00 01 ... 48 65 61 6c 74 68 7a` 开头，response hex 以 `80 01 00 02 ... 48 65 61 6c 74 68 7a` 开头。
- raw request body 为原始 Thrift CALL bytes，raw response body 为原始 Thrift REPLY bytes。
- `bifrost search` 可以分别搜索 decoded request body 中的 `Healthz` 和 decoded response body 中的 `ok`。

### TC-BP-09A 回归：raw body API 精确返回解码前二进制

操作步骤：
1. 复用 TC-BP-09 生成的二进制 Thrift 流量记录 ID。
2. 执行 `source ~/.zshrc && curl -s "http://127.0.0.1:<PORT>/_bifrost/api/traffic/<ID>/request-body?raw=1&encoding=base64" | jq -r '.data_base64'`。
3. 执行 `source ~/.zshrc && curl -s "http://127.0.0.1:<PORT>/_bifrost/api/traffic/<ID>/response-body?raw=1&encoding=base64" | jq -r '.data_base64'`。
4. 对比返回值与发起请求/fixture 响应的原始 base64。

预期结果：
- request raw base64 等于 `gAEAAQAAAAdIZWFsdGh6AAAABwwAAQAA`。
- response raw base64 等于 `gAEAAgAAAAdIZWFsdGh6AAAABwwAAAsAAQAAAAJvawwA/wAAAA==`。
- 不带 `raw=1` 的 `/request-body` 和 `/response-body` 仍返回 decoded JSON，用于 WebUI 默认展示和 decoded body 搜索。

### TC-BP-10 内置 `build_in_bp` 自动释放覆盖并在规则编辑器中提示成对写法

操作步骤：
1. 执行 `source ~/.zshrc && cargo test -p bifrost-script build_in_bp_parser_script -- --nocapture`。
2. 执行 `source ~/.zshrc && pnpm --dir web test:unit -- bpSnippets.test.ts protocol-docs.test.ts`。
3. 执行 `source ~/.zshrc && bash e2e-tests/tests/test_bp_parser_e2e.sh`。
4. 检查 E2E 输出中内置脚本释放和 syntax 提示断言。

预期结果：
- `test_init_releases_build_in_bp_parser_script` 通过，确认新数据目录会自动生成 `scripts/parser/build_in_bp.js`。
- `test_init_overwrites_stale_build_in_bp_parser_script_and_cache` 通过，确认已有旧内容会在初始化时被内置版本覆盖，且脚本缓存失效后重新读取新内容。
- Web 单测通过，确认 `bp://` 智能提示会优先生成 `bp://build_in_bp?psm=<psm>&method=<method> decode://bp`，自定义 parser 也会提示 `bp://<parser> decode://bp`。
- Web 单测通过，确认 Rules 编辑器的协议文档系统包含 `bp://` 与 `decode://`，hover/补全文档会解释 `decode://bp` 的成对使用方式和 decoded body 可展示、可搜索语义。
- BP E2E 输出包含 `build_in_bp parser script should be auto released to data dir`、`syntax endpoint should expose auto released build_in_bp parser` 和 `syntax endpoint should expose decode://bp smart hint` 通过。

### TC-BP-11 规则校验与编辑器补全兼容 parser 列表、远端 URL 和绝对路径

操作步骤：
1. 执行 `source ~/.zshrc && cargo test -p bifrost-admin builtin_decode_script_references -- --nocapture`。
2. 执行 `source ~/.zshrc && cargo test -p bifrost-admin missing_decode_script_warning -- --nocapture`。
3. 执行 `source ~/.zshrc && pnpm --dir web test:unit -- bpSnippets.test.ts protocol-docs.test.ts hover.test.ts tokenizer.test.ts`。
4. 执行 `source ~/.zshrc && bash e2e-tests/tests/test_bp_parser_e2e.sh`。
5. 在 Rules 编辑器输入 `bp://build_in_bp?psm=psm&method=method decode://bp`，检查 query 参数高亮不会污染后面的 `decode://bp` 协议。
6. 在 Rules 编辑器输入 `bp://`，触发补全并检查候选项来自 `/api/syntax` 的 `scripts.parser_scripts`。
7. 在 Rules 编辑器中对 `bp://build_in_bp?protocol=thrift` 触发定义跳转，检查页面跳转到 `/scripts?type=parser&name=build_in_bp` 并选中 parser 脚本。
8. 通过 `/_bifrost/api/rules/validate` 校验包含以下规则的内容：

```txt
bp-validate-local.test bp://build_in_bp?protocol=thrift decode://bp
bp-validate-remote.test bp://https://example.com/parser/build_in_bp.js?sha256=0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef decode://bp
bp-validate-file-url.test bp://file:///Users/eden/parser/build_in_bp.js decode://bp
bp-validate-abs.test bp:///Users/eden/parser/build_in_bp.js decode://bp
```

预期结果：
- `decode://bp`、`decode://utf8`、`decode://default` 均作为内置 decoder 通过规则校验，不提示 `Script 'bp' not found`。
- 缺失的普通 decode script 仍会提示 `Available decode scripts`，且候选列表来自 decode 脚本集合，不混入 response/request 脚本。
- `bp://...?...` 的 query 参数被当作当前 protocol value 的一部分，后续 `decode://bp` 继续按协议渲染。
- `bp://` 补全会刷新 syntax 信息，展示 parser 类型脚本，并对 parser 脚本自动补齐 `decode://bp`。
- `bp://build_in_bp?protocol=...` 和其他本地 parser 脚本名支持跳转到 Scripts 页面对应 parser 脚本。
- `bp://https://...`、`bp://file:///...`、`bp:///绝对路径...` 不会被当成本地 parser 脚本名做缺失脚本报警。

## 清理步骤

- `e2e-tests/tests/test_bp_parser_e2e.sh` 通过 trap 自动停止 Bifrost、mock server、远程脚本 HTTP server，并删除临时数据目录。
- 如执行中断，使用 `lsof -nP -iTCP:<测试端口> -sTCP:LISTEN` 查找残留进程并手动停止。
