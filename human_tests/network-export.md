# Network 导出生效规则快照

## 功能模块说明

验证 WebUI Traffic/Network 右键导出 `.bifrost` 请求文件时，导出内容不仅包含选中的请求，还包含该请求进入 Bifrost 时对应端口正在生效的规则快照。默认代理端口和自定义临时端口必须明确区分，避免用户反馈问题时排查方拿到错误的规则上下文。压缩或二进制 Body 必须可逆保存，预览应展示可解码的明文，不能用 UTF-8 lossy 字符串破坏原始字节。

## 前置条件

```bash
source ~/.zshrc
cd /Users/eden/work/github/bifrost-network-empty-bifrost-export
```

执行真实场景时必须使用临时数据目录，并且启动 Bifrost 时必须带 `--no-system-proxy`，避免影响本机系统代理。

## 测试用例列表

### TC-NE-01：默认端口 Network 导出包含默认端口生效规则快照

**操作步骤**：
1. 启动真实 Bifrost 服务，使用临时 `BIFROST_DATA_DIR` 和随机主端口。
2. 新增并启用默认规则 `main-default`，规则内容包含 `main-only.test status://209`。
3. 通过默认代理端口请求 `http://main-only.test/main-port`，等待 Traffic DB 生成记录。
4. 调用 `POST /_bifrost/api/bifrost-file/export/network`，请求体包含该 Traffic record id。
5. 解析导出的 `.bifrost` 文件中 `---` 后面的 JSON 内容。

**预期结果**：
- 导出的 record 包含 `listener_port`，值为默认主端口。
- `active_rules.source` 为 `default_port`。
- `active_rules.listener_port` 等于默认主端口。
- `active_rules.merged_content` 和 `active_rules.rules[].content` 包含 `main-only.test status://209`。
- 导出内容不包含自定义端口规则 `temp-only.test status://210`。

### TC-NE-02：自定义端口 Network 导出包含该自定义端口绑定规则快照

**操作步骤**：
1. 在同一真实 Bifrost 服务中新增临时端口，并绑定规则 `temp-bound`。
2. 通过临时端口请求 `http://temp-only.test/temp-port`，等待 Traffic DB 生成记录。
3. 调用 `POST /_bifrost/api/bifrost-file/export/network`，请求体包含该临时端口 Traffic record id。
4. 解析导出的 `.bifrost` 文件中 `---` 后面的 JSON 内容。

**预期结果**：
- 导出的 record 包含 `listener_port`，值为临时端口。
- `active_rules.source` 为 `custom_port`。
- `active_rules.listener_port` 等于临时端口。
- `active_rules.merged_content` 和 `active_rules.rules[].content` 包含 `temp-only.test status://210`。
- 导出内容不包含默认端口规则 `main-only.test status://209`。

### TC-NE-03：Network 空选择导出仍被前后端拦截

**操作步骤**：
1. 调用前端导出 helper 或后端 `POST /_bifrost/api/bifrost-file/export/network`，传入空的 `record_ids`。

**预期结果**：
- 前端 helper 返回 `Select at least one Network record before exporting a .bifrost file`。
- 后端返回 400，错误信息同样提示必须至少选择一条 Network record。
- 不生成空的 Network `.bifrost` 包。

### TC-NE-04：旧 Network 导出文件仍可解析导入

**操作步骤**：
1. 使用缺少 `listener_port` 和 `active_rules` 字段的旧版 Network `.bifrost` 文件。
2. 调用 Network import 或 parser 解析该文件。

**预期结果**：
- 旧文件解析不失败。
- 缺失的 `listener_port` 在导入恢复为 Traffic record 时按默认值 `0` 处理。
- 缺失的 `active_rules` 按 `None` 处理，不影响旧文件导入。

### TC-NE-05：标准 HTTP 压缩 Body 在请求和响应侧均展示明文

**操作步骤**：
1. 启动真实 Bifrost 服务，使用临时 `BIFROST_DATA_DIR`、随机主端口和 `--no-system-proxy`。
2. 通过代理发送 `Content-Type: application/json`、`Content-Encoding: gzip, deflate` 的双层压缩 POST 请求，并让上游响应返回相同编码链。
3. 再发送一个 `Content-Type: application/gzip` 请求：应用 payload 自身为 gzip，HTTP `Content-Encoding: gzip` 在外层再压一层，并让上游原样响应。
4. 等待 Traffic DB 生成记录，调用 Network 导出接口并解析 `.bifrost`。
5. 调用 Network 预览接口查看同一个导出文件。
6. 确认导入该文件，打开 `OUT-` 前缀的导入记录，并分别读取默认 Body 与 `raw=1` Body。
7. 使用 `bifrost traffic get --request-body --response-body`、批量 Body API 和 `bifrost traffic search --req-body --res-json ... --include bodies` 读取同一条流式压缩记录。
8. 发送由两个相邻 gzip member 组成的请求，并让上游原样响应；同时保持一条 gzip SSE 连接，在连接未关闭时读取事件流。

**预期结果**：
- `request_body` 和 `response_body` 都是解压后的 JSON 明文，不包含替换字符 `�`。
- `Content-Encoding` 中多个编码（包括重复 header 字段）按应用顺序的逆序解码；内置支持 `gzip`（含 `x-gzip` 兼容别名）、`deflate`、`br`、`zstd` 和 `identity`。
- canonical request/response Body 引用只保存 wire bytes，encoding 只写 Traffic DB metadata，不创建新的 `.content-encoding` sidecar 或重复 raw Body；`raw=1` / `request_body_base64` 可恢复原始双层压缩字节。
- 单条记录预览的 Request/Response Body 面板均展示 JSON 明文，并恢复两侧 `application/json` 内容类型。
- 遇到未知或自定义编码时不做部分解码，保留完整原始字节交给自定义 decoder。
- `application/gzip` 双层场景只移除 HTTP 外层 gzip，应用 payload 自身的 gzip 字节保持完整；`raw=1` 仍返回落盘的 wire body。
- 导入记录的请求/响应 Body 均可打开并显示明文，`raw=1` 可恢复包内保存的原始压缩字节；多记录旧包也展示 lossy Body 警告。
- `traffic get`、批量 Body API、关键词搜索、JSON Body 条件过滤及搜索结果附带 Body 均使用解压后的请求/响应内容。
- gzip SSE 在连接仍打开时可以恢复全部明文事件；Traffic Body 读取遵循运行时配置的解压输出上限。
- 相邻的多个 gzip member 会全部解码并拼接，不会只展示第一个 member。
- 对旧版本已经 lossy 导出的压缩 Body，预览隐藏乱码并提示需要使用新版本重新导出。

## 执行记录

- 2026-05-20：已执行 `cargo test -p bifrost-admin network_export -- --nocapture`，6 个后端导出用例通过，覆盖空选择拦截、默认端口快照、默认规则目录缺失空快照和自定义端口快照。
- 2026-05-20：已执行 `cargo test -p bifrost-core parse_network_accepts_legacy_record_without_active_rules -- --nocapture`，旧 Network record 缺少 `listener_port` / `active_rules` 时解析通过。
- 2026-05-20：已执行 `cd web && pnpm vitest run src/api/bifrost-file.test.ts`，5 个前端 import/export helper 用例通过，覆盖空 Network 导出提示。
- 2026-05-20：已执行 `e2e-tests/tests/test_temporary_port_bindings.sh`，55/55 通过；其中新增断言确认默认端口导出包含 `default_port` 生效规则快照，自定义端口导出包含 `custom_port` 生效规则快照，且两者不互相混入。
- 2026-09-02：已执行 `cargo test -p bifrost-admin handlers::bifrost_file::tests:: -- --nocapture`（30/30 通过）及 `cargo test -p bifrost-admin handlers::network_body::tests:: -- --nocapture`（10/10 通过），覆盖所有内置 HTTP 压缩算法、双层编码、相邻 gzip member、共享解压预算、未知编码透传、原始字节可恢复、导入 Body 持久化、非法 base64 在写入任何记录前整体拒绝、新格式预览解压、多记录包无批量解压、旧 lossy 文件警告和旧格式兼容。
- 2026-09-02：已执行 `cargo test -p bifrost-admin query_service::tests`（5/5 通过）、`cargo test -p bifrost-admin search::engine::tests`（13/13 通过）和 `cargo test -p bifrost-admin decode_replay_body`（13/13 通过），覆盖 `traffic get`、关键词搜索、JSONPath 条件、include body 与 Replay 响应的标准 HTTP 编码解码，以及缺失/非 JSON Body 的回退行为；未知自定义编码仍透传。
- 2026-09-02：已执行 `cargo test -p bifrost-proxy transform::decompress::tests`（9/9 通过），覆盖完整 Content-Encoding 链逆序解码、相邻 gzip member、共享解压预算和自定义编码原样保留。
- 2026-09-02：已执行 `cargo test -p bifrost-admin handlers::traffic::sse_stream_tests::`（6/6 通过）、`handlers::traffic::stored_body_tests::`（2/2 通过）和 `handlers::traffic::batch_query_tests::`（8/8 通过），覆盖压缩 SSE 事件恢复、配置化解压上限与批量 Body 解码。
- 2026-09-02：已按仓库门禁无 filter 执行 `RUST_TEST_THREADS=1 SKIP_FRONTEND_BUILD=1 make coverage-changed`，最终变更生产 Rust 行覆盖率为 91.22%（291/319），通过 90% 门禁。
- 2026-09-03：已执行 `e2e-tests/tests/test_temporary_port_bindings.sh`，73/73 通过；真实 `gzip, deflate` 双层压缩 POST 及响应经代理录制、`traffic get`、批量 Body API、正文搜索、响应 JSONPath 过滤、include body、Network 导出、预览和重新导入后，两侧明文 JSON、内容类型、导入 Body 与原始请求字节断言通过；另验证多个相邻 gzip member 全部解码、打开状态的 gzip SSE 恢复全部事件且不会在完整 member 后提前关闭，以及 `application/gzip` payload 外叠 HTTP gzip 时请求/响应只移除 HTTP 外层编码。
- 2026-09-03：canonical wire-only 改造后再次执行 `BIFROST_BIN=target/debug/bifrost bash e2e-tests/tests/test_temporary_port_bindings.sh`，73/73 通过；真实规则绑定与应用、双层压缩 Traffic/CLI/Batch/Search/Network 导入导出、`raw=1` wire 恢复、相邻 gzip member 和打开状态 gzip SSE 均通过。随后执行 `BIFROST_BIN=target/debug/bifrost bash e2e-tests/tests/test_response_stream_script.sh`，response stream script HTTP/HTTPS 真实链路通过。
- 2026-09-02：已按 TC-NE-05 独立人工执行 release 二进制，使用临时数据目录、动态端口 `62396/62397`、`--no-system-proxy` 及禁用托盘/登录提示环境变量；Traffic 请求/响应、Network 导出及预览的双层压缩明文断言通过，`x-company-codec` 请求/响应二进制字节保持不变，服务按精确 PID 清理。

## 清理步骤

```bash
rm -rf ./.bifrost-e2e-network-export-* /tmp/bifrost-network-export-*
```
