# Long-term Memory 真实场景测试

## 功能模块说明

验证 Bifrost Agent 长期记忆系统的跨 session 显式记忆、自动抽取、召回注入、Admin API、WebUI 管理、JSONL 导入导出、GC 与隐私脱敏能力。

## 前置条件

- 工作目录：`/Users/eden/work/github/bifrost`
- 测试端口禁止使用 9900。
- 服务启动必须使用临时数据目录并携带 `--no-system-proxy`：

```bash
export BIFROST_DATA_DIR="$(mktemp -d)"
export BIFROST_AGENT_HOME="$BIFROST_DATA_DIR/agent"
cargo build --bin bifrost
./target/debug/bifrost start -p 18881 --unsafe-ssl --no-system-proxy
```

## 测试用例列表

### TC-LTM-01 `/remember` 显式写入

操作步骤：
1. 运行专项 E2E：`bash e2e-tests/tests/test_long_term_memory_remember_recall.sh`
2. 观察 `/remember 用户偏好...` 返回内容。

预期结果：
- 命令返回 `已记住长期记忆`。
- `$BIFROST_AGENT_HOME/memory/memories.sqlite` 中存在新记录。

实际结果：
- 已执行 `bash e2e-tests/tests/test_long_term_memory_remember_recall.sh`，但当前 Codex 沙箱在 mock Chat Completions 启动阶段阻塞：`TcpListener::bind("127.0.0.1:0")` 返回 `Operation not permitted (os error 1)`，业务链路未进入。补充执行 `cargo test -p bifrost-agent memory_runtime::tests::generate_memories_disabled_short_circuits -- --nocapture`，确认显式 `remember_explicit` 在 `generate_memories=false` 时仍能写入 SQLite。

### TC-LTM-02 跨 session 召回注入

操作步骤：
1. 运行 `bash e2e-tests/tests/test_long_term_memory_remember_recall.sh`。
2. 检查 mock Chat Completions 收到的第二个 session 请求。

预期结果：
- messages 中存在紧跟主 system prompt 的 `# Long-term memories (per user)`。
- 注入内容包含第一轮 `/remember` 写入的中文偏好。

实际结果：
- 已执行专项 E2E，当前沙箱因 socket bind `Operation not permitted` 未能进入第二 session mock 请求验证。召回排序与 system message 格式已由 `cargo test -p memory recall::tests:: -- --nocapture` 覆盖；需在允许本地监听端口的环境复跑脚本完成最终 E2E 断言。

### TC-LTM-03 关闭召回开关

操作步骤：
1. 运行 `cargo test -p bifrost-agent memory_runtime::tests::use_memories_disabled_short_circuits -- --nocapture`。

预期结果：
- `MemoriesConfig.use_memories = Some(false)` 时不打开 DB、不注入记忆。

实际结果：
- 通过。`cargo test -p bifrost-agent memory_runtime::tests::use_memories_disabled_short_circuits -- --nocapture` 结果：1 passed。

### TC-LTM-04 自动抽取配置开关

操作步骤：
1. 运行 `cargo test -p bifrost-agent memory_runtime::tests::generate_memories_disabled_short_circuits -- --nocapture`。

预期结果：
- `generate_memories = Some(false)` 时自动抽取短路。
- 显式 `/remember` 不受该开关影响。

实际结果：
- 通过。`cargo test -p bifrost-agent memory_runtime::tests::generate_memories_disabled_short_circuits -- --nocapture` 结果：1 passed，且显式写入路径可用。

### TC-LTM-05 隐私脱敏

操作步骤：
1. 运行 `cargo test -p memory redact::tests:: -- --nocapture`。

预期结果：
- `sk-...`、`ghp_...`、`AIza...`、`Bearer ...`、长 base64、`password=`、`token=`、`api_key`、`BF-...` 全部替换为 `<REDACTED>`。

实际结果：
- 通过。`cargo test -p memory redact::tests:: -- --nocapture` 结果：10 passed。

### TC-LTM-06 SQLite 存储与 FTS 搜索

操作步骤：
1. 运行 `cargo test -p memory store::tests::search_ -- --nocapture`。

预期结果：
- 英文关键词通过 FTS 命中。
- 中文关键词通过 LIKE fallback 命中。

实际结果：
- 通过。`cargo test -p memory store::tests::search_ -- --nocapture` 结果：2 passed。

### TC-LTM-07 去重与导入导出

操作步骤：
1. 运行 `cargo test -p memory store::tests::insert_dedupes_within_scope -- --nocapture`。
2. 运行 `cargo test -p memory store::tests::export_import_jsonl_round_trip -- --nocapture`。

预期结果：
- 相同 scope 的重复内容不新增记录，只更新 use_count。
- JSONL 导出后导入新 store 能恢复记录。

实际结果：
- 通过。`cargo test -p memory store::tests::insert_dedupes_within_scope -- --nocapture` 与 `cargo test -p memory store::tests::export_import_jsonl_round_trip -- --nocapture` 均通过。

### TC-LTM-08 GC 与 pinned 保护

操作步骤：
1. 运行 `cargo test -p memory store::tests::gc_ -- --nocapture`。

预期结果：
- 超过 `max_unused_days` 的非 pinned 记忆被软删除。
- pinned 记忆不被 GC 删除。

实际结果：
- 通过。`cargo test -p memory store::tests::gc_ -- --nocapture` 结果：2 passed。

### TC-LTM-09 Admin API 增删改查

操作步骤：
1. 启动临时服务：
   `BIFROST_DATA_DIR="$(mktemp -d)" cargo run --bin bifrost -- start --host 127.0.0.1 -p 18881 --unsafe-ssl --no-system-proxy`
2. `curl -sS -X POST http://127.0.0.1:18881/_bifrost/api/agent/memories -H 'Content-Type: application/json' -d '{"content":"api memory","kind":"fact","tags":["api"],"scope":{"type":"global"}}'`
3. `curl -sS http://127.0.0.1:18881/_bifrost/api/agent/memories?query=api`
4. `curl -sS http://127.0.0.1:18881/_bifrost/api/agent/memories/stats`

预期结果：
- POST 返回 201 与记录 ID。
- GET 列表包含 `api memory`。
- stats.total >= 1。

实际结果：
- 已尝试以临时 `BIFROST_DATA_DIR`、`--host 127.0.0.1`、`--no-system-proxy` 启动服务；当前沙箱启动后无法对测试端口完成监听/ready 探测，`READY=0`，因此 API curl 未执行到业务断言。Rust 层 `cargo check -p bifrost-admin` 已验证 handler/router 编译通过。

### TC-LTM-10 WebUI 管理页面

操作步骤：
1. 启动临时服务并打开 `http://127.0.0.1:18881/_bifrost/`。
2. 进入 Settings -> Agent -> Memory Records。
3. 新建、编辑、pin、搜索、删除一条记忆。

预期结果：
- 列表刷新展示新记录。
- pin 按钮状态切换。
- 搜索能过滤到记录。
- 删除后列表不再展示该记录。
- 亮色与暗色主题下文字、按钮、标签均可读。

实际结果：
- 受 TC-LTM-09 同一端口监听限制影响，当前沙箱无法打开真实 WebUI 页面完成浏览器交互；前端构建已在 `cargo check -p bifrost-admin` 中通过。需在允许本地监听端口和浏览器访问的环境复测。

### TC-LTM-11 JSONL API 导入导出

操作步骤：
1. 调用 `GET /_bifrost/api/agent/memories/export`。
2. 将返回 `content` 再 POST 到 `/_bifrost/api/agent/memories/import`。

预期结果：
- export 返回 JSON，包含 `content` 与 `count`。
- import 返回 `inserted/deduped/failed` 报告。
- 导入路径仍会脱敏和去重。

实际结果：
- API 方式受 TC-LTM-09 端口监听限制影响未执行到 curl 断言；等价存储导入导出链路已由 `cargo test -p memory store::tests::export_import_jsonl_round_trip -- --nocapture` 验证通过。

### TC-LTM-12 compaction 事件持久化

操作步骤：
1. 运行 `cargo test -p bifrost-agent persistence::tests::record_compaction_event_round_trip -- --nocapture`。

预期结果：
- session JSONL 中出现 `event_type = "compaction"`。
- 事件能被加载扫描识别，不再只是定义常量。

实际结果：
- 通过。`cargo test -p bifrost-agent persistence::tests::record_compaction_event_round_trip -- --nocapture` 结果：1 passed。

## 清理步骤

```bash
pkill -f "bifrost start -p 18881" || true
rm -rf ./.bifrost-ltm-human-test /tmp/bifrost_ltm_human_test*
```
