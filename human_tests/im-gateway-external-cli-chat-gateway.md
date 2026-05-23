# Agent Custom Runner / Chat Gateway 真实场景测试

## 功能模块说明

验证 Agent Custom Runner 与 Chat Gateway API：可以不经过真实 IM 通道，直接通过 HTTP 请求触发自定义 CLI Runner；Runtime 将输入消息写入 CLI stdin，收集 stdout/stderr，归一化 JSONL progress events，并提供 run detail 供 WebUI 与后续 IM 消息更新复用。同时覆盖全局默认配置、单 Provider/IM 通道覆盖配置、effective config 来源预览、NDJSON stream、stop marker、工作目录继承、ExternalCliAgentChat route action 和 WebUI 配置面。

## 前置条件

1. 在独立 worktree 中执行，避免污染主工作区。
2. 启动 Bifrost 时必须使用临时数据目录并加 `--no-system-proxy`：
   ```bash
   BIFROST_DATA_DIR=/tmp/bifrost-im-external-cli-test cargo run --bin bifrost -- start -p 18880 --unsafe-ssl --no-system-proxy
   ```
3. 准备 mock 外部 Agent：
   ```bash
   cat > /tmp/mock-external-agent.sh <<'SH'
   #!/usr/bin/env sh
   cat >/dev/null
   printf '%s\n' \
     '{"type":"run_started","content":"started"}' \
     '{"type":"assistant_delta","delta":"working"}' \
     '{"type":"tool_started","tool_name":"exec_command","content":"checking"}' \
     '{"type":"assistant_final","content":"mock final answer"}'
   SH
   chmod +x /tmp/mock-external-agent.sh
   ```

## 测试用例列表

### TC-IEC-01: Chat Gateway 直接触发 mock CLI Agent

操作步骤：
1. 调用：
   ```bash
   curl -sS http://127.0.0.1:18880/_bifrost/api/im-gateway/chat \
     -H 'content-type: application/json' \
     -d '{
       "message":"hello from chat gateway",
       "sessionKey":"human-test-chat",
       "runtime":"external_cli",
       "adapter":"mock",
       "adapterConfig":{
         "executable":"/tmp/mock-external-agent.sh",
         "args":[]
       }
     }'
   ```
2. 检查响应 JSON。

预期结果：
1. 响应包含 `runId`、`status:"succeeded"`、`response:"mock final answer"`。
2. 响应包含至少 4 条 `events`，其中包含 `assistant_delta`、`tool_started`、`assistant_final` 的归一化事件。
3. 响应包含 `artifacts.runDir`、`artifacts.stdout`、`artifacts.normalizedEvents`。

### TC-IEC-02: Run Detail API 可读取本次运行 artifacts

操作步骤：
1. 取 TC-IEC-01 响应中的 `runId`。
2. 调用：
   ```bash
   curl -sS http://127.0.0.1:18880/_bifrost/api/im-gateway/chat/runs/<runId>
   ```

预期结果：
1. 响应 `runId` 与请求一致。
2. `snapshot.executable` 为 `/tmp/mock-external-agent.sh`。
3. `stdout` 包含 mock 输出的 JSONL。
4. `events` 与 TC-IEC-01 返回的事件一致。

### TC-IEC-03: 非法 runId 被拒绝

操作步骤：
1. 调用：
   ```bash
   curl --path-as-is -i http://127.0.0.1:18880/_bifrost/api/im-gateway/chat/runs/../secret
   ```

预期结果：
1. 返回 400 或 404。
2. 不读取 run 根目录之外的文件。

### TC-IEC-04: 空消息请求被拒绝

操作步骤：
1. 调用：
   ```bash
   curl -i http://127.0.0.1:18880/_bifrost/api/im-gateway/chat \
     -H 'content-type: application/json' \
     -d '{"message":"","runtime":"external_cli","adapter":"mock","adapterConfig":{"executable":"/tmp/mock-external-agent.sh","args":[]}}'
   ```

预期结果：
1. 返回 400。
2. 响应错误包含 `message cannot be empty`。

### TC-IEC-05: Codex adapter 默认命令契约

操作步骤：
1. 执行单元测试：
   ```bash
   cargo test -p bifrost-admin codex_adapter_builds_exec_command_with_prompt_stdin -- --nocapture
   ```

预期结果：
1. 测试通过。
2. Codex adapter 默认命令包含 `codex exec --json --output-last-message <path> -`。
3. 当配置 `workDir/profile/model/sandbox/enableFeatures/ephemeral` 时，这些字段被映射为当前 Codex CLI 参数；历史 `search:true` 仅作为兼容入口映射为 `--enable web_search`，不生成已废弃的 `--search`。
4. 当前本机 Codex CLI `0.130.0` 不再支持旧 `--ask-for-approval` 参数，默认 Codex adapter 不应生成该参数。

### TC-IEC-06: Runtime 真实执行 mock CLI 并写入 artifacts

操作步骤：
1. 执行单元测试：
   ```bash
   cargo test -p bifrost-admin external_cli_runtime_runs_mock_command_and_writes_artifacts -- --nocapture
   ```

预期结果：
1. 测试通过。
2. 临时 run 目录中存在 `runtime_snapshot.json` 与 `normalized_events.jsonl`。
3. 最终 response 来自归一化后的 assistant final event。

### TC-IEC-07: 默认不发送真实 IM 消息

操作步骤：
1. 执行 TC-IEC-01。
2. 打开 IM Gateway History 页面或查询消息历史 API。

预期结果：
1. 本次 Chat Gateway 运行不会创建 outbound IM message log。
2. 后续 IM 通道接入时，发送/更新 IM 消息必须通过独立的 delivery policy 显式开启。

### TC-IEC-08: 全局默认配置与通道覆盖配置合并

操作步骤：
1. 保存全局默认配置：
   ```bash
   curl -sS -X PATCH http://127.0.0.1:18880/_bifrost/api/im-gateway/chat/config/defaults \
     -H 'content-type: application/json' \
     -d '{"enabled":true,"adapter":"mock","adapterConfig":{"executable":"/tmp/mock-external-agent.sh","args":[]},"allowWorkDirs":["/tmp"],"injectBifrostTools":true,"skillPaths":[],"deliveryMode":"final_reply"}'
   ```
2. 保存通道覆盖：
   ```bash
   curl -sS -X PATCH http://127.0.0.1:18880/_bifrost/api/im-gateway/chat/config/channels/provider-a \
     -H 'content-type: application/json' \
     -d '{"adapter":"mock","injectBifrostTools":false}'
   ```
3. 查询 effective config：
   ```bash
   curl -sS http://127.0.0.1:18880/_bifrost/api/im-gateway/chat/config/channels/provider-a
   ```

预期结果：
1. `settings.enabled` 来自全局默认配置且为 `true`。
2. `settings.adapter` 为 `mock`。
3. `sources.adapter` 为 `channel`，`sources.allowWorkDirs` 为 `global`。
4. `settings.injectBifrostTools` 为 `false` 且来源为 `channel`。

### TC-IEC-09: Chat Gateway 使用通道配置触发外部 CLI

操作步骤：
1. 在 TC-IEC-08 后调用：
   ```bash
   curl -sS http://127.0.0.1:18880/_bifrost/api/im-gateway/chat \
     -H 'content-type: application/json' \
     -d '{"message":"hello provider config","providerId":"provider-a","sessionKey":"human-provider-a"}'
   ```

预期结果：
1. 响应 `adapter` 为 `mock`，证明请求继承了通道 effective config。
2. 响应 `status` 为 `succeeded`。
3. 响应 `response` 为 `mock final answer`。

### TC-IEC-10: Chat Gateway NDJSON stream

操作步骤：
1. 调用：
   ```bash
   curl -sS -N http://127.0.0.1:18880/_bifrost/api/im-gateway/chat/stream \
     -H 'content-type: application/json' \
     -d '{"message":"hello stream","providerId":"provider-a","sessionKey":"human-provider-a"}'
   ```

预期结果：
1. 第一行包含 `eventType:"run_started"`。
2. 中间包含归一化的 `assistant_delta`、`tool_started`、`assistant_final`。
3. 最后一行包含 `eventType:"run_finished"`、`runId`、`response:"mock final answer"`。

### TC-IEC-11: work_dir allowlist 拒绝越界目录

操作步骤：
1. 在全局配置 `allowWorkDirs:["/tmp"]` 后调用：
   ```bash
   curl -i http://127.0.0.1:18880/_bifrost/api/im-gateway/chat \
     -H 'content-type: application/json' \
     -d '{"message":"bad workdir","providerId":"provider-a","workDir":"/Users"}'
   ```

预期结果：
1. 返回 400。
2. 响应包含 `outside allowWorkDirs`。

### TC-IEC-12: Run stop marker API

操作步骤：
1. 取任一真实 runId。
2. 调用：
   ```bash
   curl -i -X POST http://127.0.0.1:18880/_bifrost/api/im-gateway/chat/runs/<runId>/stop
   ```

预期结果：
1. 返回 200。
2. 响应包含 `success:true` 和原 `runId`。
3. 如果 run 仍在执行，External CLI active process 会收到终止信号；Unix 下只有在确认子进程已进入独立 process group 时才终止同组子进程，避免误伤 Bifrost 或 CI runner 进程组；即使 shell 在信号后继续吐出迟到 stdout，最终 run 也必须以 `status:"stopped"` 收敛，`response` 为停止提示，不残留测试进程。

### TC-IEC-13: ExternalCliAgentChat route action 可保存

操作步骤：
1. 调用：
   ```bash
   curl -sS http://127.0.0.1:18880/_bifrost/api/im-gateway/routes \
     -H 'content-type: application/json' \
     -d '{"id":"external-cli-route","provider_id":"provider-a","name":"External CLI Route","enabled":true,"event_type":"message_receive","matcher":{"keyword":"codex"},"action":{"type":"external_cli_agent_chat","delivery_mode":"no_im","reply_target":"original_chat"},"timeout_ms":30000,"max_output_bytes":1048576}'
   ```

预期结果：
1. 返回 `success:true`。
2. Route store 接受 `external_cli_agent_chat` action，后续真实 IM 入站命中该 route 时会使用 External CLI runtime。

### TC-IEC-14: WebUI 配置预览和测试抽屉

操作步骤：
1. 打开 WebUI 的 IM Gateway 配置页。
2. 进入 External CLI section。
3. 检查全局默认配置与单个 IM 通道覆盖配置。
4. 检查 External CLI Runtime 的字段：runner id、default runner、adapter、executable、args、instructions、skills、tools、delivery policy。
5. 确认页面不再把 Working Directory 设计为 CLI runner 字段，而是提示继承 IM Provider Agent settings / global Agent settings。
4. 分别切换亮色与暗色主题。

预期结果：
1. 页面可以展示多个 CLI runners，并支持选择 default runner。
2. 通道配置只覆盖 runner、enabled 和 delivery，不重复暴露 CLI 可执行文件或 working directory。
3. “测试运行”可以调用 Chat Gateway，不需要真实 IM 入站消息。
4. 亮色与暗色主题下文字、表单、按钮、事件流、artifact 链接均清晰可辨。

### TC-IEC-15: Chat Gateway 真实调用本地 Codex CLI

操作步骤：
1. 确认本机真实 Codex CLI 可执行文件：
   ```bash
   /opt/homebrew/bin/codex --version
   /opt/homebrew/bin/codex exec --help
   ```
2. 先直接调用 Codex CLI 验证认证和基础运行：
   ```bash
   printf '%s\n' 'Reply exactly: BIFROST_REAL_CODEX_OK. Do not run shell commands. Do not edit files.' \
     | /opt/homebrew/bin/codex exec --json \
       --cd ~/work/github/bifrost-im-external-cli-agent \
       --sandbox read-only \
       --output-last-message /tmp/bifrost-real-codex-direct-last.md -
   ```
3. 通过 Chat Gateway 真实调用 Codex adapter：
   ```bash
   curl -sS -X POST http://127.0.0.1:18880/_bifrost/api/im-gateway/chat \
     -H 'content-type: application/json' \
     -d '{
       "message":"Reply exactly: BIFROST_CHAT_GATEWAY_REAL_CODEX_OK. Do not run shell commands. Do not edit files.",
       "sessionKey":"real-codex-chat-gateway-test",
       "runtime":"external_cli",
       "adapter":"codex",
       "workDir":"~/work/github/bifrost-im-external-cli-agent",
       "adapterConfig":{
         "executable":"/opt/homebrew/bin/codex",
         "sandbox":"read-only",
         "timeoutSecs":180
       },
       "allowWorkDirs":["~/work/github/bifrost-im-external-cli-agent"],
       "injectBifrostTools":false,
       "skillPaths":[]
     }'
   ```
4. 使用同等 payload 调用 `/chat/stream`。

预期结果：
1. 直接 Codex CLI 调用输出真实 Codex JSONL，包含 `thread.started`、`item.completed`、`turn.completed`，并写出 last message。
2. Chat Gateway 响应 `adapter:"codex"`、`status:"succeeded"`，`response` 包含 `BIFROST_CHAT_GATEWAY_REAL_CODEX_OK`。
3. run detail 可读取 stdout、last message、normalized events；stdout 保留真实 Codex JSONL。
4. normalized events 包含 `run_started`、`assistant_final`、`run_finished`。
5. `/chat/stream` 返回 NDJSON，包含 `BIFROST_STREAM_REAL_CODEX_OK`，不依赖 mock CLI。

### TC-IEC-16: Codex CLI 非致命 warning 不被误判为 run_failed

操作步骤：
1. 使用本机真实 Codex CLI 与临时 Bifrost 服务，调用：
   ```bash
   curl -sS --json '{
     "message":"Reply exactly: BIFROST_REVIEW_REAL_CODEX_CHAT_OK3. Do not run shell commands. Do not edit files.",
     "adapter":"codex",
     "workDir":"~/work/github/bifrost",
     "allowWorkDirs":["~/work/github/bifrost"],
     "injectBifrostTools":false,
     "adapterConfig":{"executable":"codex","sandbox":"read-only","timeoutSecs":180}
   }' http://127.0.0.1:18882/_bifrost/api/im-gateway/chat
   ```
2. 读取响应中的 `events` 和 run detail 中的 `cli.stdout.log`。

预期结果：
1. HTTP 响应 `status:"succeeded"`，`response` 精确包含 `BIFROST_REVIEW_REAL_CODEX_CHAT_OK3`。
2. `cli.stdout.log` 是真实 Codex JSONL，允许出现 Codex 本地配置 warning。
3. warning 归一化为 `status` 事件，不出现 `run_failed` 事件；真实失败仍由进程 exit status 判断。

### TC-IEC-17: Chat Gateway stream 不重复发送 start/finish

操作步骤：
1. 使用本机真实 Codex CLI 与临时 Bifrost 服务，调用：
   ```bash
   curl -sS -N --json '{
     "message":"Reply exactly: BIFROST_REVIEW_REAL_CODEX_STREAM_OK2. Do not run shell commands. Do not edit files.",
     "adapter":"codex",
     "workDir":"~/work/github/bifrost",
     "allowWorkDirs":["~/work/github/bifrost"],
     "injectBifrostTools":false,
     "adapterConfig":{"executable":"codex","sandbox":"read-only","timeoutSecs":180}
   }' http://127.0.0.1:18882/_bifrost/api/im-gateway/chat/stream
   ```
2. 逐行解析返回的 NDJSON。

预期结果：
1. NDJSON 包含 `BIFROST_REVIEW_REAL_CODEX_STREAM_OK2`。
2. `run_started` 恰好出现 1 次，`run_finished` 恰好出现 1 次。

### TC-IEC-18: WebUI Agent Runners 管理与 Provider Runner 绑定

操作步骤：
1. 打开 `http://127.0.0.1:18882/_bifrost/ai?agentSection=general`。
2. 检查 AI -> Agent -> General 中存在 `Default Runner` 控件，选项包含 `Bifrost Agent` 和已配置的自定义 runner ID（如 `codex`、`abc`），不出现 `External CLI (Codex)` 这类旧固定文案。
3. 打开 `http://127.0.0.1:18882/_bifrost/ai?agentSection=runners`。
4. 检查 Runners 页面默认展示 runner 列表，而不是直接展示大表单；页面内不再出现 `Default Custom Runner` 或默认 runner 下拉。
5. 点击 `Add Runner`，弹窗填写自定义 Runner ID、Adapter、Executable、Arguments、Skill Paths、Instructions 等配置；保存后列表中出现新 runner。
6. 编辑已有 runner，确认仍通过弹窗修改配置；每个 runner 都可独立选择 adapter。
7. 打开 `http://127.0.0.1:18882/_bifrost/ai?imGatewaySection=connections`。
8. 编辑一个 IM Provider，检查弹窗内 `Agent Runner` 控件选项包含 `Inherit global default`、`Bifrost Agent` 和各自定义 runner ID。
9. 分别选择 `Inherit global default`、`Bifrost Agent`、一个自定义 runner 并保存。
10. 通过 API 读取 Agent 配置、Provider 配置和 Runner config：
   ```bash
   curl -sS http://127.0.0.1:18882/_bifrost/api/im-gateway/agent
   curl -sS http://127.0.0.1:18882/_bifrost/api/im-gateway/providers
   curl -sS http://127.0.0.1:18882/_bifrost/api/im-gateway/chat/config
   ```

预期结果：
1. 全局 Agent General 是默认 runner 的唯一入口；Runners 页面只管理 runner 实体，不提供第二个默认值入口。
2. 选择 `Bifrost Agent` 时 Agent/Provider 配置保存为 `runner:"bifrost_agent"`。
3. 选择自定义 runner（如 `abc`）时 Agent/Provider 配置直接保存为 `runner:"abc"`；Runner registry 的 `defaultRunnerId` 仅表示全局默认自定义 runner，IM channel 的 `runnerId` 仅作为通道覆盖。
4. IM Provider 可以通过 `agent_config.runner` 覆盖全局 runner，也可以清空为继承全局默认。
5. Provider 列表卡片展示当前覆盖状态；未覆盖时显示 `Global default`，自定义 runner 显示 runner ID。
6. 亮色和暗色主题下控件文案、下拉菜单和说明文字清晰可见。

### TC-IEC-19: Codex Runner 工作目录按 Provider -> Global 降级并显式传给 CLI

操作步骤：
1. 启动临时 Bifrost 服务：
   ```bash
   BIFROST_DATA_DIR=/tmp/bifrost-runner-workdir-real cargo run --bin bifrost -- start -p 18884 --unsafe-ssl --no-system-proxy
   ```
2. 配置全局 Agent：
   ```bash
   curl -sS -X PATCH http://127.0.0.1:18884/_bifrost/api/im-gateway/agent \
     -H 'content-type: application/json' \
     -d '{"enabled":true,"runner":"codex","work_dir":"~/work/github/bifrost"}'
   ```
3. 配置默认 Codex runner，故意使用自定义 `args:["exec","--json","-"]`，验证运行时仍会注入 Codex 所需工作目录与 last message 参数：
   ```bash
   curl -sS -X PATCH http://127.0.0.1:18884/_bifrost/api/im-gateway/chat/config \
     -H 'content-type: application/json' \
     -d '{"version":1,"defaultRunnerId":"codex","runners":{"codex":{"enabled":true,"adapter":"codex","adapterConfig":{"executable":"codex","args":["exec","--json","-"]},"injectBifrostTools":false,"skillPaths":[],"deliveryMode":"final_reply"}},"channels":{}}'
   ```
4. 创建或更新 Provider `workdir-provider`，配置 `agent_config.runner="codex"` 与 `agent_config.work_dir="~/work/github/bifrost/crates/bifrost-admin"`。
5. 带 providerId 调用 `/chat`，要求 Codex 执行 `pwd` 并返回 `WORKDIR_CHECK:<pwd>`。
6. 不带 providerId 调用 `/chat`，要求 Codex 执行 `pwd` 并返回 `GLOBAL_WORKDIR_CHECK:<pwd>`。
7. 分别读取两个 run 的 `runtime_snapshot.json` 与 `last_message.md`。

预期结果：
1. Provider run 的响应为 `WORKDIR_CHECK:~/work/github/bifrost/crates/bifrost-admin`。
2. Provider run 的 `runtime_snapshot.args` 包含 `--cd ~/work/github/bifrost/crates/bifrost-admin` 与 `--output-last-message <run>/last_message.md`，`workDir` 为 Provider 工作目录。
3. 无 providerId 的 Chat Gateway run 降级使用全局 Agent 工作目录，响应为 `GLOBAL_WORKDIR_CHECK:~/work/github/bifrost`。
4. Global run 的 `runtime_snapshot.args` 包含 `--cd ~/work/github/bifrost` 与 `--output-last-message <run>/last_message.md`，`workDir` 为全局 Agent 工作目录。
5. Codex session 在 Codex Desktop 中归属到对应 `--cd` 项目目录，而不是 Bifrost 服务启动时的偶然 cwd。

### TC-IEC-20: Schedule Agent 支持当前 Codex CLI 参数覆盖且保留 Runner 命令配置

操作步骤：
1. 执行单元测试，验证 Codex adapter 把当前 CLI 参数映射成真实 argv：
   ```bash
   cargo test -p bifrost-admin codex_adapter_ --lib -- --nocapture
   ```
2. 执行 CLI 参数解析测试，验证 schedule add/update 可把 agent 专用参数写入 `agent.adapter_config`：
   ```bash
   cargo test -p bifrost-cli parse_schedule_ --lib -- --nocapture
   ```
3. 执行 Schedule Runner 覆盖回归测试，验证 schedule 级 adapter_config 与 runner 默认 adapter_config 字段级合并，不会因为仅覆盖 model/env 等字段丢失 runner 的 executable/args：
   ```bash
   cargo test -p bifrost-admin schedule_agent_adapter_config_overrides_runner_without_dropping_command --lib -- --nocapture
   ```
4. 使用临时服务创建一个不会立即触发的 Agent schedule（避免实际调用真实 Codex）：
   ```bash
   BIFROST_DATA_DIR=/tmp/bifrost-schedule-agent-codex-args cargo run --bin bifrost -- start -p 18886 --unsafe-ssl --no-system-proxy
   cargo run --bin bifrost -- im schedule add codex-args-human \
     --target oc_human_test \
     --cron '0 0 1 1 *' \
     --agent-prompt 'Human test only' \
     --agent-runner-id codex \
     --agent-model gpt-5 \
     --agent-profile-v2 team \
     --agent-reasoning-effort high \
     --agent-reasoning-summary auto \
     --agent-add-dir /tmp/extra \
     --agent-config 'shell_environment_policy.inherit=all' \
     --agent-enable web_search \
     --agent-danger-full-access
   ```
5. 调用 schedules API 读取 `codex-args-human`。

预期结果：
1. `codex_adapter_` 相关测试通过；其中 `codex_adapter_builds_current_cli_config_flags` 断言包含 `--profile-v2`、`--config model_reasoning_effort="high"`、`--config model_reasoning_summary="auto"`、`--skip-git-repo-check`、`--ignore-user-config`、`--ignore-rules`、`--add-dir`、`--enable`、`--disable`，且不生成当前 Codex CLI 不支持的 `--search`；`codex_adapter_applies_config_flags_to_custom_args` 断言 Runner 自定义 `args` 场景仍会注入 schedule 级 `--model`、reasoning `--config`、`--enable web_search`，并在 `--dangerously-bypass-approvals-and-sandbox` 存在时移除模板里的 `--sandbox`。
2. `codex_adapter_danger_full_access_suppresses_sandbox` 通过，断言 `--dangerously-bypass-approvals-and-sandbox` 存在且不再同时生成 `--sandbox`。
3. `parse_schedule_add_args_supports_agent_codex_flags` 与 `parse_schedule_update_args_supports_agent_codex_flags` 通过，断言 CLI 写入 `agent.runner_id="codex"` 与 `agent.adapter_config` 的 model/profileV2/reasoning/addDirs/configOverrides/enableFeatures/disableFeatures/dangerFullAccess/skipGitRepoCheck。
4. `schedule_agent_adapter_config_overrides_runner_without_dropping_command` 通过；测试中的 mock runner 仍使用 runner 默认 `executable/args` 执行，同时能读取 schedule 覆盖的 env/model 相关配置，避免出现只传 schedule 专用参数导致 runner 命令配置被清空的回归。
5. API 返回的 schedule 中 `task_type:"agent"`，`message_channel.provider_id:"feishu-main"`、`message_channel.target_id:"oc_human_test"`、`message_channel.target_mode:"configured_target"`，且 `agent.adapter_config` 保留上述字段，说明定时任务可覆盖 Runner 默认 Codex 参数并绑定明确投递通道。

## 最近执行记录

- 2026-05-13：执行 TC-IEC-01/02/03/04/08/09/10/11/12/13，临时服务端口 `18880`，`BIFROST_DATA_DIR=/tmp/bifrost-im-external-cli-test`，均通过；TC-IEC-12 使用 `sleep 23` 慢进程验证 active run 可停止，run 收敛为 `status:"stopped"`，`response:"External CLI run was stopped by request."`，且未残留 `sleep 23` 测试进程。
- 2026-05-13：执行 TC-IEC-05/06 及 stop 迟到输出回归单测，`cargo test -p bifrost-admin external_cli -- --nocapture`，5 个测试通过。
- 2026-05-13：执行 TC-IEC-14，使用 Playwright 打开 `http://127.0.0.1:18880/_bifrost/ai?imGatewaySection=external-cli`，亮色与暗色两种 `colorScheme` 下均能看到 External CLI、Global Defaults、Channel Override，截图保存到 `/tmp/bifrost-im-external-cli-light.png` 与 `/tmp/bifrost-im-external-cli-dark.png`。
- 2026-05-13：执行 TC-IEC-12 CI 回归验证，`cargo test -p bifrost-admin external_cli_runtime_marks_stopped_run_before_late_stdout --lib`、`cargo test -p bifrost-admin external_cli_runtime_stops_active_run_by_session_key --lib`、`cargo test -p bifrost-admin external_cli_runtime_runs_mock_command_and_writes_artifacts --lib` 均通过；补充 `terminate_process_rejects_pid_zero` 防止 pid 0 退化为当前进程组信号。
- 2026-05-13：执行 TC-IEC-15，真实 Codex CLI 为 `/opt/homebrew/bin/codex`，版本 `codex-cli 0.130.0`；直接 CLI 返回 `BIFROST_REAL_CODEX_OK`；Chat Gateway `/chat` 真实 run `1778652465502-adf6d351-7824-48cd-bcb2-d5401202c483` 返回 `BIFROST_CHAT_GATEWAY_REAL_CODEX_OK`；`/chat/stream` 真实 run `1778652498219-6548feb4-c476-45da-b699-c3bd78eb2af9` 返回 `BIFROST_STREAM_REAL_CODEX_OK`。同时确认 PATH 第一位 `~/.local/bin/codex` 缺少 `@openai/codex-darwin-arm64`，真实测试显式使用 `/opt/homebrew/bin/codex`。
- 2026-05-13：执行 TC-IEC-16/17，临时服务端口 `18882`，`BIFROST_DATA_DIR=/tmp/bifrost-real-codex-review`，真实 Codex CLI 通过 PATH 中 `codex-cli 0.130.0` 执行；`/chat` 真实 run `1778660318463-f4c70869-226b-46ae-9bc2-dc53a4bd02ba` 返回 `BIFROST_REVIEW_REAL_CODEX_CHAT_OK3`，events 为 `run_started/status/status/assistant_final/run_finished`，未出现 `run_failed`；`/chat/stream` 返回 `BIFROST_REVIEW_REAL_CODEX_STREAM_OK2`，`run_started` 与 `run_finished` 均仅出现 1 次。
- 2026-05-13：新增 TC-IEC-18，覆盖用户指出的 WebUI 缺口：AI -> Agent -> General 的 Default Runner、AI -> Agent -> Runners 的 runner 列表/新建/编辑弹窗、IM Provider 弹窗绑定具体 runner。随后将配置从单一 Global Defaults 重构为 `defaultRunnerId + runners{}`，Codex 只是默认自定义 runner，IM channel 只选择 runner；Working Directory 从 runner 配置中移除，改为继承 IM Provider Agent/global Agent 配置。
- 2026-05-13：执行 TC-IEC-19，临时服务端口 `18884`，`BIFROST_DATA_DIR=/tmp/bifrost-runner-workdir-real-3`；真实 Codex CLI provider run `1778669065397-60bc3fca-0b58-4aea-a6ba-48a59dc0ffbd` 返回 `WORKDIR_CHECK:~/work/github/bifrost/crates/bifrost-admin`，snapshot 显式包含 `--cd ~/work/github/bifrost/crates/bifrost-admin`；global fallback run `1778669086562-dfcbc5ec-e4d4-4e6a-9ed4-129a6d27e4aa` 返回 `GLOBAL_WORKDIR_CHECK:~/work/github/bifrost`，snapshot 显式包含 `--cd ~/work/github/bifrost`。
- 2026-05-13：复测 TC-IEC-18，临时服务端口 `18884`，`BIFROST_DATA_DIR=/tmp/bifrost-runner-ui-real`；Playwright 打开 `/_bifrost/ai?agentSection=runners`，确认 Runners 在 Agent 板块、列表展示 `codex` runner、无 `Default Custom Runner`、无 runner 表格 `default` 标签、Channel Runner 文案为 `Inherit global runner`，`Add Runner` 弹窗包含 Runner ID / Adapter / Executable / Arguments / Skill Paths / Instructions；亮色截图 `/tmp/bifrost-runner-ui-modal-updated.png`，暗色截图 `/tmp/bifrost-runner-ui-runners-dark.png`。
- 2026-05-13：WebUI build 已随 `cargo test -p bifrost-admin external_cli -- --nocapture`、`cargo clippy -p bifrost-admin --all-targets --all-features -- -D warnings` 和 `pnpm --dir web run build` 通过。
- 2026-05-23：更新 TC-IEC-20，覆盖 Schedule Agent 对当前 Codex CLI 参数的独立配置：`model/profileV2/reasoningEffort/reasoningSummary/dangerFullAccess/skipGitRepoCheck/ignoreUserConfig/ignoreRules/addDirs/configOverrides/enableFeatures/disableFeatures`；同时新增字段级合并回归，确保 schedule 覆盖 model/env 等字段时不会丢失 runner 默认 executable/args，并确认历史 `search:true` 兼容映射为 `--enable web_search` 而不再生成废弃 `--search`。
- 2026-05-23：执行 TC-IEC-20 的 CLI 解析与真实临时服务创建 schedule 链路，命令 `cargo test -p bifrost-cli parse_schedule_ --lib -- --nocapture` 与 `./e2e-tests/tests/test_im_schedule_agent_cli_args.sh` 均通过；确认 `bifrost im schedule add ... --target oc_human_test ...` 写入明确 `message_channel`，且 agent Codex adapter 参数完整保留。
- 2026-05-23：复测 TC-IEC-20 的自定义 Runner args 覆盖回归，命令 `cargo test -p bifrost-admin codex_adapter_ --lib -- --nocapture`、`cargo test -p bifrost-cli parse_schedule_ --lib -- --nocapture`、`cargo test -p bifrost-admin schedule_agent_adapter_config_overrides_runner_without_dropping_command --lib -- --nocapture` 均通过；确认 `codex_adapter_applies_config_flags_to_custom_args` 覆盖自定义 `args` 仍注入 schedule 级 Codex 参数，并在 danger full access 下移除 `--sandbox`。

## 清理步骤

1. 停止 Bifrost 测试进程。
2. 删除临时数据和 mock 脚本：
   ```bash
   rm -rf /tmp/bifrost-im-external-cli-test \
     /tmp/mock-external-agent.sh \
     /tmp/defaults.json \
     /tmp/channel.json \
     /tmp/slow-channel.json \
     /tmp/im_chat_run.json \
     /tmp/im_chat_stream.ndjson \
     /tmp/im_workdir_reject.json \
     /tmp/slow-response.json \
     /tmp/slow-stop.json \
     /tmp/slow-leftover.txt \
     /tmp/real-codex-request.json \
     /tmp/real-codex-stream-request.json \
     /tmp/real-codex-chat-result.json \
     /tmp/real-codex-chat-detail.json \
     /tmp/real-codex-stream.ndjson \
     /tmp/bifrost-real-codex-direct-last.md \
     /tmp/bifrost-im-external-cli-light.png \
     /tmp/bifrost-im-external-cli-dark.png
   ```
