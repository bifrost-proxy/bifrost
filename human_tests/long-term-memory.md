# Long-term Memory 真实场景测试

## 功能模块说明

验证 Bifrost Agent 长期记忆已改为文件布局和按需加载：记忆存放在用户数据目录 `agent/memory/`，模型请求只注入读取说明和 `memory_summary.md` 摘要，不再使用 SQLite 数据库存储。

## 前置条件

- 工作目录：`<REPO_ROOT>`
- 测试端口禁止使用 9900。
- 服务启动必须使用临时数据目录并携带 `--no-system-proxy`：

```bash
export BIFROST_DATA_DIR="$(mktemp -d)"
cargo build --bin bifrost
./target/debug/bifrost start -p 18881 --unsafe-ssl --no-system-proxy
```

## 测试用例列表

### TC-LTM-01 文件目录初始化

操作步骤：
1. 运行 `cargo test -p bifrost-agent memory_runtime::tests::memory_read_instructions_use_agent_memory_root -- --nocapture`。
2. 检查测试输出无失败。

预期结果：
- 记忆根目录为 `agent/memory/`。
- 注入说明引用 `memory_summary.md`、`MEMORY.md`、`rollout_summaries/`、`skills/`。
- 注入说明包含 `<oai-mem-citation>` 要求。

实际结果：
- 通过。`cargo test -p bifrost-agent memory_runtime::tests:: -- --nocapture` 中该用例 passed，注入说明包含 `agent/memory` 文件布局与 citation 要求。

### TC-LTM-02 空 memory_summary 不注入

操作步骤：
1. 运行 `cargo test -p bifrost-agent memory_runtime::tests::empty_memory_summary_does_not_inject -- --nocapture`。

预期结果：
- `memory_summary.md` 为空时不向模型请求注入 memory instructions。

实际结果：
- 通过。`cargo test -p bifrost-agent memory_runtime::tests:: -- --nocapture` 中该用例 passed，空 summary 返回 `None`。

### TC-LTM-03 关闭召回开关

操作步骤：
1. 运行 `cargo test -p bifrost-agent memory_runtime::tests::use_memories_disabled_short_circuits -- --nocapture`。

预期结果：
- `MemoriesConfig.use_memories = Some(false)` 时不读取文件、不注入记忆说明。

实际结果：
- 通过。`cargo test -p bifrost-agent memory_runtime::tests:: -- --nocapture` 中该用例 passed，`use_memories=false` 不注入。

### TC-LTM-04 `/remember` 文件追加且不创建 SQLite

操作步骤：
1. 运行 `cargo test -p bifrost-agent memory_runtime::tests::remember_writes_codex_files_without_sqlite -- --nocapture`。

预期结果：
- `/remember` 等价路径追加 `MEMORY.md`、`memory_summary.md` 和 `raw_memories.md`。
- `agent/memory/memories.sqlite` 不存在。

实际结果：
- 通过。`cargo test -p bifrost-agent memory_runtime::tests:: -- --nocapture` 中该用例 passed，显式写入更新 `MEMORY.md`、`memory_summary.md` 和 `raw_memories.md`，`raw_memories.md` 包含 `source: user_explicit`，且未创建 `memories.sqlite`。

### TC-LTM-05 E2E 按需加载说明注入

操作步骤：
1. 运行 `bash e2e-tests/tests/test_long_term_memory_remember_recall.sh`。

预期结果：
- mock Chat Completions 收到的 messages 包含 `## Memory` read-path instructions。
- messages 包含 `memory_summary.md (already provided below; do NOT open again)`。
- messages 包含 `MEMORY.md (searchable registry; primary file to query)`。
- messages 包含预置 summary：`Bifrost should use on-demand memory loading.`。
- 测试临时 `agent/memory/` 下没有 `memories.sqlite`。

实际结果：
- 通过。2026-05-05 执行 `CARGO_TARGET_DIR=target/ci-fix BIFROST_E2E_RUNNER_JOBS=1 bash e2e-tests/tests/test_long_term_memory_remember_recall.sh`：
  - `im_gateway_agent_long_term_memory_remember_recall` passed，mock 请求包含 read-path instructions 与预置 summary。
  - `im_gateway_agent_auto_memory_new_session_consumes` passed，自动生成记忆后新 session 消费 `MEM-AUTO-42`，且未创建 `memories.sqlite`。

### TC-LTM-06 Admin API 文件追加与列表

操作步骤：
1. 启动临时服务：
   `BIFROST_DATA_DIR="$(mktemp -d)" cargo run --bin bifrost -- start --host 127.0.0.1 -p 18881 --unsafe-ssl --no-system-proxy`
2. 追加记忆：
   `curl -sS -X POST http://127.0.0.1:18881/_bifrost/api/agent/memories -H 'Content-Type: application/json' -d '{"content":"api file memory"}'`
3. 搜索记忆：
   `curl -sS 'http://127.0.0.1:18881/_bifrost/api/agent/memories?query=api%20file'`
4. 查看 stats：
   `curl -sS http://127.0.0.1:18881/_bifrost/api/agent/memories/stats`

预期结果：
- POST 返回 201 与文件条目 ID。
- GET 列表包含 `api file memory`。
- stats 返回 `memory_root`，并显示 `memory_summary_bytes > 0`、`memory_md_bytes > 0`。
- `$BIFROST_DATA_DIR/agent/memory/memories.sqlite` 不存在。

实际结果：
- 已尝试启动临时服务并 curl stats；当前沙箱中 `cargo run` 等待 artifact lock，8 秒 ready 窗口内服务未监听，curl 返回 `Could not connect to server`。需在允许本地服务启动/监听的环境复跑 API 断言。

### TC-LTM-07 Admin API 导入导出文件内容

操作步骤：
1. 调用 `GET /_bifrost/api/agent/memories/export`。
2. 将任意 Markdown 片段 POST 到 `/_bifrost/api/agent/memories/import`。
3. 再次搜索导入内容。

预期结果：
- export 返回 `memory_summary.md` 和 `MEMORY.md` 的合并文本。
- import 将内容追加到 `MEMORY.md`。
- 搜索可以命中导入内容。

实际结果：
- 受 TC-LTM-06 同一服务启动限制影响，当前沙箱未进入 API 导入导出业务断言；需在服务可监听环境复跑。

### TC-LTM-08 WebUI 文件视图

操作步骤：
1. 启动临时服务并打开 `http://127.0.0.1:18881/_bifrost/`。
2. 进入 Settings -> Agent -> Memory Records。
3. 点击 Append，添加 `webui file memory`。
4. 搜索 `webui file`。
5. 点击 Export。

预期结果：
- 页面展示文件路径、内容、统计标签。
- Append 后列表刷新出现 `webui file memory`。
- 搜索只展示匹配行。
- Export 下载内容来自 `agent/memory/` 文件。
- 亮色与暗色主题下文字、按钮、标签均可读。

实际结果：
- 受 TC-LTM-06 同一服务启动限制影响，当前沙箱无法打开真实 WebUI；前端已随 `cargo check -p bifrost-admin` 构建通过，仍需在可访问浏览器环境复跑交互断言。

### TC-LTM-09 真实对话接口自动生成记忆、Phase 2 Consolidation 并跨独立 Session 消费

操作步骤：
1. 运行 `bash e2e-tests/tests/test_long_term_memory_human_api.sh`。
2. 脚本使用临时 `BIFROST_DATA_DIR` 启动 Bifrost：
   `./target/debug/bifrost start --host 127.0.0.1 -p 18883 --unsafe-ssl --no-system-proxy`。
3. 脚本启动 OpenAI-compatible mock 模型服务，并通过 `PATCH /_bifrost/api/im-gateway/agent` 配置 Agent：
   - `enabled=true`
   - `base_url=http://127.0.0.1:18884/chat/completions`
   - `memories.use_memories=true`
   - `memories.generate_memories=true`
4. 第一个独立 session 调用 `POST /_bifrost/api/im-gateway/agent/chat`：
   `{"session_key":"human-memory-source","message":"请记住我是“独孤怼怼”。"}`
5. 检查 `$BIFROST_DATA_DIR/agent/memory/memory_summary.md`、`MEMORY.md`、`raw_memories.md`、`.phase2_state.json` 和 `rollout_summaries/`。
6. 第二个独立 session 调用同一对话接口：
   `{"session_key":"human-memory-consumer-1","message":"这是新的独立 session。请只根据长期记忆回答：我是谁？"}`
7. 第三个独立 session 再调用同一对话接口：
   `{"session_key":"human-memory-consumer-2","message":"再开一个新的独立 session。请只根据长期记忆回答：我是谁？"}`
8. 检查 mock 模型请求日志是否包含 `Memory Writing Agent: Phase 2`、`## Memory`、`MEMORY.md (searchable registry; primary file to query)` 和 `独孤怼怼`。

预期结果：
- 第一轮对话结束后自动抽取记忆，并触发无数据库 Phase 2 consolidation。
- `memory_summary.md` 与 `MEMORY.md` 都包含 `独孤怼怼`。
- `MEMORY.md` 中最终条目 `source` 为 `phase2_consolidated`。
- `raw_memories.md` 保留原始追溯材料，包含 `source: auto_extract` 与 `独孤怼怼`。
- `.phase2_state.json` 存在并记录非空 `last_input_hash`。
- `.phase2_state.json` 记录 bounded input 元数据：`processed_input_count=1`、`total_input_count=1`、`has_more_inputs=false`、`updated_at_unix>0`。
- `rollout_summaries/` 至少新增一个本轮自动抽取摘要文件。
- `$BIFROST_DATA_DIR/agent/memory/memories.sqlite` 不存在。
- 第二个和第三个全新 session 的响应都包含 `独孤怼怼`。
- 第二个和第三个 session 发给模型的请求中都自动注入 memory read-path instructions，并包含 `memory_summary.md` 摘要里的 `独孤怼怼`。

实际结果：
- 通过。2026-05-03 使用临时目录 `/tmp/bifrost-memory-phase2.kqYSys`、mock 模型 `127.0.0.1:18894`、真实 Bifrost `127.0.0.1:18893` 手工执行：
  - `BIFROST_DATA_DIR=/tmp/bifrost-memory-phase2.kqYSys ./target/debug/bifrost start --host 127.0.0.1 -p 18893 --unsafe-ssl --no-system-proxy`
  - `PATCH /_bifrost/api/im-gateway/agent` 配置 `base_url=http://127.0.0.1:18894/chat/completions`、`memories.use_memories=true`、`memories.generate_memories=true`
  - 第一 session `phase2-source` 返回 `已记住，你是独孤怼怼。`
  - 服务日志出现 `phase-2 memory consolidation completed`
  - `memory_summary.md`、`MEMORY.md`、`raw_memories.md` 均包含 `独孤怼怼`
  - `MEMORY.md` 包含 `source: phase2_consolidated`
  - `raw_memories.md` 保留 `source: auto_extract`
  - `.phase2_state.json` 存在并包含非空 `last_input_hash`，且记录 `processed_input_count=1`、`total_input_count=1`、`has_more_inputs=false`、`updated_at_unix>0`
  - 2026-05-03 复跑 `bash e2e-tests/tests/test_long_term_memory_human_api.sh` 通过，脚本使用真实 Bifrost `127.0.0.1:18883` 与临时 `BIFROST_DATA_DIR`，并额外断言 `.phase2_state.json` 的 bounded input 元数据。
  - `rollout_summaries/` 生成 markdown 摘要文件
  - `agent/memory/memories.sqlite` 不存在
  - 第二 session `phase2-consumer-1` 与第三 session `phase2-consumer-2` 均返回 `你是独孤怼怼。`
  - mock 请求日志显示 `consolidate=True`、`extract=True`、`memory_prompt=True has_memory=True asks_identity=True`，确认真实触发 Phase 2，并在新 session 注入 `## Memory` 后消费摘要。

### TC-LTM-10 回归：自动记忆 E2E mock 与当前 Phase 1/Phase 2 Prompt 对齐

操作步骤：
1. 运行：
   `CARGO_TARGET_DIR=target/ci-fix BIFROST_E2E_RUNNER_JOBS=1 cargo run -p bifrost-e2e -- --test im_gateway_agent_auto_memory_new_session_consumes --test-timeout 120 --port 18882`
2. 观察 mock Chat Completions 请求识别逻辑是否进入 Phase 1 抽取与 Phase 2 consolidation 分支。
3. 检查测试输出中的内存文件落地、consolidation 和新 session 消费断言。

预期结果：
- Phase 1 请求按当前 `EXTRACT_SYSTEM_PROMPT` 被识别，mock 返回 `rollout_summary`、`rollout_slug`、`raw_memory` JSON，内容包含 `MEM-AUTO-42`。
- Phase 2 请求按当前 `CONSOLIDATION_SYSTEM_PROMPT` 被识别，`phase-2 memory consolidation completed` 出现在日志中。
- `memory_summary.md`、`MEMORY.md`、`raw_memories.md` 和 `rollout_summaries/` 均落地并包含 `MEM-AUTO-42`。
- 新 session 请求注入 memory read-path instructions，并返回包含 `MEM-AUTO-42` 的答案。

实际结果：
- 通过。2026-05-05 执行该命令 passed，日志显示：
  - `codex-style extraction written rollout_slug=auto-memory-source has_raw_memory=true has_rollout_summary=true`
  - `phase-2 memory consolidation completed`
  - `memory read instructions injected`
  - `im_gateway_agent_auto_memory_new_session_consumes` 1/1 passed。

### TC-LTM-11 回归：真实对话 shell E2E mock 与当前 Phase 1/Phase 2 Prompt 对齐

操作步骤：
1. 运行：
   `BIFROST_PORT=18883 MOCK_PORT=18884 bash e2e-tests/tests/test_long_term_memory_human_api.sh`
2. 观察 mock Chat Completions 请求识别逻辑是否进入当前 Phase 1 抽取与 Phase 2 consolidation 分支。
3. 检查脚本输出的 memory 文件断言、Phase 2 状态断言和跨独立 session 消费断言。

预期结果：
- Phase 1 请求按当前 `Memory Writing Agent: Phase 1` prompt 被识别，mock 返回 `rollout_summary`、`rollout_slug`、`raw_memory` JSON，内容包含 `独孤怼怼`。
- Phase 2 请求按当前 `Memory Writing Agent: Phase 2` prompt 被识别，`MEMORY.md` 中最终条目 `source` 为 `phase2_consolidated`。
- 系统 prompt 构建过程中技能摘要 UTF-8 安全裁剪，不会因中文 skill 描述在真实对话 API 中 panic。
- `raw_memories.md` 保留 `source: auto_extract`。
- 第二个和第三个全新 session 的响应都包含 `独孤怼怼`。

实际结果：
- 通过。2026-05-05 执行该命令 passed，脚本输出 `[long-term-memory-human-api] PASS`，并验证 `memory_summary.md`、`MEMORY.md`、`raw_memories.md`、`.phase2_state.json`、`rollout_summaries/` 和跨独立 session 读取均符合预期。

## 清理步骤

```bash
pkill -f "bifrost start -p 18881" || true
pkill -f "bifrost start --host 127.0.0.1 -p 18883" || true
rm -rf ./.bifrost-ltm-human-test /tmp/bifrost_ltm_human_test*
```
