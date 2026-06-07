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

### TC-IEC-21: Codex IM Runner 服务重启后默认续接上一次 thread

操作步骤：
1. 使用临时数据目录启动 Bifrost，端口为 `$MAIN_PORT`，启动参数必须包含 `--no-system-proxy`。
2. 配置一个 mock Codex runner，mock 第一次输出：
   ```json
   {"type":"thread.started","thread_id":"thread-human-1"}
   {"type":"assistant_final","content":"FIRST_OK"}
   ```
3. 通过 Chat Gateway 或真实 IM 入站发送第一条消息，指定 `providerId:"provider-a"`、`sessionKey:"provider-a:user-a"`、runner `codex`。
4. 确认 `$BIFROST_DATA_DIR/agent/im_gateway/session_state.json` 中存在该 session 的 `externalThreadId:"thread-human-1"`。
5. 停止服务并用同一个 `BIFROST_DATA_DIR` 重新启动 Bifrost。
6. 发送第二条消息，仍使用同一 provider/user/channel，不显式传 `threadId`。
7. 读取第二次 run 的 `runtime_snapshot.json`。

预期结果：
1. 第二次 run 使用 `codex exec resume ... thread-human-1 -`，而不是 `codex exec --json ... -` 新建线程。
2. 第二次响应来自同一 session 的续接结果。
3. `session_state.json` 的 `updatedAt` 更新；IM Agent loop 会记录 `historyPath`，Chat Gateway 直接调用至少记录 `externalThreadId` 并保留 run artifacts。
4. 未出现跨 provider/user/runner 的状态串用。

### TC-IEC-22: `/reset` 后清理持久状态并允许新建 thread

操作步骤：
1. 延续 TC-IEC-21 的临时数据目录，确认 `session_state.json` 中已有 `externalThreadId:"thread-human-1"`。
2. 从同一 IM session 发送 `/reset` 或 `/clear`。
3. 再发送一条普通消息，mock Codex 输出新的：
   ```json
   {"type":"thread.started","thread_id":"thread-human-2"}
   {"type":"assistant_final","content":"RESET_OK"}
   ```
4. 读取本次 run 的 `runtime_snapshot.json` 和 `session_state.json`。

预期结果：
1. `/reset` 回复会话已重置。
2. `/reset` 后的下一次 run 不携带 `thread-human-1`。
3. 新 run 记录 `externalThreadId:"thread-human-2"`。
4. 清理范围仅限当前 adapter/runner；同一 `sessionKey` 下其它 adapter/runner 的状态不被误删。

### TC-IEC-23: ChatGPT Web Runner 服务重启后恢复 conversationId 且 reset 后不复活

操作步骤：
1. 使用临时数据目录启动 Bifrost，端口为 `$MAIN_PORT`，启动参数必须包含 `--no-system-proxy`。
2. 配置 Chat Gateway runner `chatgpt-web-resume-e2e`，adapter 为 `chatgpt_web` 且 enabled。
3. 在 `$BIFROST_DATA_DIR/agent/im_gateway/session_state.json` 预置同一 `sessionKey + adapter + runnerId` 的 `externalConversationId:"conv-chatgpt-web-e2e-1"`。
4. 停止服务并用同一个 `BIFROST_DATA_DIR` 重新启动 Bifrost。
5. 调用 `/_bifrost/api/im-gateway/chat`，传 `sessionKey`、`runnerId`、`adapter:"chatgpt_web"`，不显式传 `conversationId`。
6. 读取最新 chat run 的 `runtime_snapshot.json`。
7. 调用同一 endpoint 发送 `/reset`。
8. 再次重启服务后发送普通消息，并读取最新 `runtime_snapshot.json`。

预期结果：
1. 第一次普通消息在显式 E2E mock ChatGPT Web adapter 下成功返回，`runtime_snapshot.params.conversationId` 为 `conv-chatgpt-web-e2e-1`，证明重启后默认恢复旧 conversation。
2. `/reset` 返回 `{"success":true,"cleared":true}`，不触发真实 ChatGPT Web run。
3. `/reset` 后的下一次普通消息不再携带 `conv-chatgpt-web-e2e-1`。
4. 同一 `sessionKey` 下非 `chatgpt_web/chatgpt-web-resume-e2e` 的状态不被清理。

### TC-IEC-24: `/stop` 同时停止 external runner，避免状态显示已停但进程仍跑

操作步骤：
1. 使用临时 runs root 启动一个 external-cli mock runner，命令先 `sleep 2` 再输出 `assistant_final`。
2. 该 run 绑定明确 `session_key:"external-stop-status-deadlock"`。
3. 在 run 仍处于 sleep 阶段时调用共享 stop helper，传入同一 session key。
4. 等待 run 结束并读取返回状态。

预期结果：
1. stop helper 返回 `true`，即使内置 Agent manager 中没有 active stop signal。
2. external runner 通过 session key stop marker 收敛为 `status:"stopped"`。
3. 不会等待 sleep 完成后输出迟到的 `assistant_final`。
4. 该行为覆盖 IM 忙碌态 `/stop`、空闲态 `/stop` 和 `/agent/chat` `/stop` 共用入口。

### TC-IEC-25: Trae Runner Web Chat 实时过程与 Web History 过程默认可见

操作步骤：
1. 使用临时数据目录启动 Bifrost，启动命令必须包含 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1` 和 `--no-system-proxy`：
   ```bash
   BIFROST_DATA_DIR=/tmp/bifrost-traex-runner-e2e \
   BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 \
   cargo run --bin bifrost -- start -p 18890 --unsafe-ssl --no-system-proxy --skip-cert-check
   ```
2. 配置 `traex` runner，adapter 为 `traex`，executable 为本机真实 Trae CLI，sandbox 为 `read-only`，`skipGitRepoCheck=true`，delivery mode 为 `progress_card` 或 `final_reply`：
   ```bash
   curl -sS -X PATCH http://127.0.0.1:18890/_bifrost/api/im-gateway/chat/config \
     -H 'content-type: application/json' \
     -d '{"version":1,"defaultRunnerId":"traex","runners":{"traex":{"enabled":true,"adapter":"traex","adapterConfig":{"executable":"/Users/eden/.local/bin/traex","sandbox":"read-only","skipGitRepoCheck":true},"injectBifrostTools":false,"skillPaths":[],"deliveryMode":"progress_card"}},"channels":{}}'
   ```
3. 打开 `http://127.0.0.1:18890/_bifrost/ai?aiSection=agent-chat&agentSection=chat&view=active`。
4. 选择或确认当前 Runner 为 `traex`，发送 `Reply exactly BIFROST_TRAEX_WEB_UI_STREAM_OK`。
5. 运行中观察消息气泡的过程区域；完成后刷新或打开历史 session，检查最终消息。

预期结果：
1. run 使用 `adapter:"traex"`、`runner_id:"traex"`，run detail artifact 中 `runtime_snapshot.args` 包含 `--cd <work_dir> exec --json --output-last-message <last_message.md> -`。
2. 运行中可以看到 Trae JSONL 归一化后的 process event；完成后最终消息包含 `BIFROST_TRAEX_WEB_UI_STREAM_OK`。
3. Web History 中完成后的过程块默认可见，页面不需要用户再次点开才能看到过程信息；飞书 progress card 的完成态折叠规则以 TC-IEC-29 为准。
4. timeline 中不出现外层 `traex` wrapper 的单次工具调用噪音；只展示 Trae 自己输出的状态、工具或最终事件。

### TC-IEC-26: Trae Runner 飞书 IM Progress Card 与 Web History 展示一致

操作步骤：
1. 延续 TC-IEC-25 的临时服务和 `traex` runner 配置。
2. 将 `feishu-main` Provider 的 Agent Runner 配置为 `traex`，工作目录为当前测试 worktree。
3. 从飞书 IM 通道发送一条普通消息，例如 `你好` 或 `你是谁`。
4. 观察飞书 IM 中的 progress card 更新和最终结果。
5. 打开对应 session history URL，检查 Web Chat 历史渲染。

预期结果：
1. 飞书消息命中 `traex` runner，session JSONL 中记录 `adapter:"traex"`、`runner_id:"traex"`。
2. progress card 在运行中展示 Trae 状态或工具过程，完成后收敛为最终结果，不停留在 running。
3. Web history 中同一 turn 显示 `Runner: traex`，过程块默认展开，最终答案可见。
4. 飞书通道与 Web Chat 历史使用同一 timeline 语义，不出现 Web 有过程但 IM 无过程，或 IM 有 wrapper 工具噪音的分裂表现。

### TC-IEC-27: Trae Permission Mode 默认值不会传递为非法 exec 参数

操作步骤：
1. 配置 `traex` runner 时将 WebUI Permission Mode 保持为 `Headless default`，或 API 中省略 `permissionMode` / 传空值。
2. 触发一次 Trae Chat Gateway run。
3. 读取本次 run 的 `runtime_snapshot.json` 和最终状态。

预期结果：
1. `runtime_snapshot.args` 不包含 `--permission-mode default`。
2. Trae 不再报错 `permission_mode = "default" is not supported in exec mode`。
3. 如果用户显式选择 `plan`、`bypass_permissions`、`auto` 或 `custom`，后端才生成对应 `--permission-mode <value>`。

### TC-IEC-28: Codex Runner Web Chat 实时工具过程与最终结果展示

操作步骤：
1. 确认本机真实 Codex CLI 可用：
   ```bash
   /opt/homebrew/bin/codex --version
   /opt/homebrew/bin/codex exec --help
   ```
2. 直接运行真实 Codex CLI，要求它执行一次 `pwd` 并输出固定最终答案，同时记录 stdout JSONL 每行到达时间。
3. 使用临时数据目录启动 Bifrost，必须禁用系统代理和 Sync 自动登录弹窗：
   ```bash
   BIFROST_DATA_DIR=/tmp/bifrost-codex-runner-e2e \
   BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 \
   cargo run --bin bifrost -- start -p 18891 --unsafe-ssl --no-system-proxy --skip-cert-check
   ```
4. 配置 `codex` runner，adapter 为 `codex`，executable 为 `/opt/homebrew/bin/codex`，sandbox 为 `read-only`，`skipGitRepoCheck=true`。
5. 在 WebUI Agent Chat 中选择 Codex runner，发送：
   ```text
   Run exactly one shell command using your shell tool: pwd. Then reply exactly: BIFROST_CODEX_WEB_UI_STREAM_OK
   ```
6. 打开同一 session 的历史页，检查过程块和最终消息。

预期结果：
1. 直接 CLI 调用在进程结束前输出 `thread.started`、`turn.started`、`item.started command_execution`、`item.completed command_execution`、`item.completed agent_message`、`turn.completed`；`item.started command_execution` 早于最终答案到达。
2. Web Chat 运行中可见 `exec_command` 工具过程，完成后最终消息包含 `BIFROST_CODEX_WEB_UI_STREAM_OK`。
3. 历史 timeline 中存在 `tool_call/tool_result`，`tool_name` 为 `exec_command`，arguments 包含 Codex 输出的 shell command，result 包含 `pwd` 输出。
4. 不出现外层 `codex` wrapper 的单次工具调用噪音；只展示 Codex JSONL 归一化后的状态、工具或最终事件。
5. Codex CLI 当前不暴露隐藏 chain-of-thought；页面不能伪造思考文本，只能展示公开输出的状态、工具过程、结果和最终答案。

### TC-IEC-29: Codex Runner 飞书 IM Progress Card 与 Web History 展示一致

操作步骤：
1. 使用 TC-IEC-28 的临时服务和 `codex` runner 配置，确保 delivery mode 为 `progress_card`。
2. 从飞书 IM 通道或 Chat Gateway 模拟同一 provider/session 触发 Codex runner，并要求执行 `pwd` 后输出固定文本。
3. 打开 WebUI history 页面查看该 session。

预期结果：
1. progress card 在运行中展开“执行过程”，按时间顺序展示公开模型 content 和工具调用；连续多个工具调用默认合并成“已运行 N 条命令”的一级折叠组。
2. progress card 的全局状态位于卡片顶部，执行过程信息位于中间，完成后最终结论位于卡片底部；过程和状态面板默认折叠，但可以手动展开过程，再展开工具分组和单条工具详情查看完整信息，卡片不再停留在 running。
3. progress card 的状态面板展示 `Runner: codex`、`Adapter: codex`、模型标签、外部会话、队列/引导状态和工作路径。
4. 如果 runner 显式配置了 `adapterConfig.model`，模型标签展示该模型名；如果没有显式配置，只展示 `Codex 默认模型（未显式配置）`，不能猜测具体模型。
5. progress card 不展示内置 Bifrost Agent 专属的空指标，例如 `Loop 0/0`、`Context ~0 / N/A`、`Token N/A`、`压缩 0 次`。
6. Web history 中显示 `Runner: codex`、过程 timeline、工具参数/结果和最终答案。
7. Web history 与 IM progress card 使用同一 canonical timeline：工具名称、参数、结果、成功状态一致。
8. 若本次只通过 Chat Gateway 做基础验证，也必须至少确认 timeline 中的 `tool_call/tool_result` 可被 IM progress card 复用；真实飞书发送可作为人工补充验收。

### TC-IEC-30: 外部 Runner 模型、Token、Context 与当前状态展示

操作步骤：
1. 使用当前源码启动临时 Bifrost 服务：
   ```bash
   BIFROST_DATA_DIR=/tmp/bifrost-runner-status-model \
   BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 \
   cargo run --bin bifrost -- start -p 18892 --unsafe-ssl --no-system-proxy --skip-cert-check
   ```
2. 配置 `codex` 或 `traex` runner，显式设置 `adapterConfig.model`，delivery mode 为 `progress_card`，work_dir 指向当前仓库。
3. 通过 WebUI Agent Chat 选择该 runner，发送一个会触发公开状态或工具过程的请求，例如要求执行一次 `pwd` 并输出固定文本。
4. 打开 WebUI history/session detail，检查 session detail JSON、过程块和最终消息。
5. 如果有飞书 IM 通道可用，从飞书触发同一 runner；否则通过 Chat Gateway/run detail 验证 progress card 使用同一 metadata。

预期结果：
1. 运行中 `/status` 或 Web active status 文本包含当前公开状态，例如 `turn started`、`running`、工具状态或 runner event 状态。
2. IM progress card 状态面板显示 Runner、Adapter、模型、外部会话、工作路径、队列/引导状态和最新工具；内置 Agent 卡片状态面板显示模型但仍保留自身 Context/Token/压缩指标。
3. 外部 runner 完成后，run metadata 包含 `model`/`modelSource`/`modelLabel`；显式配置模型时展示配置模型，未显式配置时只展示默认模型标签，不猜测真实模型。
4. Codex/Trae JSONL 中的 `turn.completed.usage` 被归一化为 `usageInputTokens`、`usageOutputTokens`、`usageTotalTokens` 等 metadata；WebUI/session detail 与 progress card 能显示 token usage。
5. 外部 runner 的 Context 行展示最近一轮 `input_tokens` 作为最近输入 context，例如 `Context：最近输入 59.6K / N/A`；由于 CLI 未暴露 context window、压缩阈值或压缩次数，Bifrost 不展示伪造的百分比或压缩次数。
6. Codex/Trae 明确输出的 `reasoning_summary` 或等价公开进展文本会进入过程 timeline；如果 CLI 没有输出该文本，只展示状态、工具过程和最终答案，不伪造隐藏 chain-of-thought。

### TC-IEC-31: Trae agent_message 过程展示与超时失败收敛

操作步骤：
1. 将飞书默认 IM 通道配置为 `traex` runner，delivery mode 为 `progress_card`，work_dir 指向当前分支 worktree。
2. 从飞书发送一个需要 Trae 多次读取 diff 的 review 请求，例如：
   ```text
   对当前工作区当前分支做代码 review，仅 review，不做修改
   ```
3. 在运行中观察 progress card 的执行过程区域。
4. 如果运行超过 runner `timeoutSecs`，等待卡片最终收敛。
5. 检查本地 run artifacts 的 `normalized_events.jsonl` 和 `result.json`。

预期结果：
1. Trae 输出的公开 `agent_message` 不提前占用底部最终结论，而是作为执行过程中的模型 content/思考信息展示。
2. 执行过程按时间顺序展示模型 content 和工具调用；连续工具调用默认折叠为一组，展开组后再展开单条工具查看输入、耗时和输出。
3. 底部最终结论只来自 turn 结束时的最终结果；运行中不应只因为早期 `agent_message` 就展示最终结论。
4. 若 run 超时，卡片状态为失败，最终结论明确显示 `Runner failed: external CLI timed out...`，不能显示为已完成，也不能把早期 `agent_message` 当作成功结果。
5. `result.json.status` 为 `timed_out` 时，session 状态和 IM progress card 都按失败路径处理。

### TC-IEC-32: Trae 长任务默认无超时且过程卡片稳定刷新

操作步骤：
1. 将 `traex` runner 的 `adapterConfig.timeoutSecs` 清空或省略，并确认 `feishu-main` channel override 为 `runnerId=traex`、`deliveryMode=progress_card`。
2. 从飞书发送一个需要多轮 review 和多次工具调用的请求。
3. 观察运行中的 progress card 执行过程区域。
4. 检查对应 run 的 `runtime_snapshot.json` 和 `normalized_events.jsonl`。

预期结果：
1. `runtime_snapshot.json` 中不再出现 180 秒默认超时；未显式配置 timeout 时，Bifrost 不主动按固定秒数杀掉外部 runner。
2. Trae 重复输出同一条 `command_execution item.started` 时，执行过程不重复插入相同的运行中工具行。
3. 执行过程中持续展示模型公开 content 和工具调用；连续工具调用默认折叠为一组，组内单条工具详情默认折叠，展开后展示输入与输出预览，完整输出仍保存在 run artifacts。
4. 大输出工具不会导致后续飞书卡片更新丢失，最终结论仍位于卡片底部。

### TC-IEC-33: Trae Web Chat 长任务实时过程展示

操作步骤：
1. 启动当前分支的本地 Bifrost 服务，端口使用 `9900`，必须携带 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1` 与 `--no-system-proxy`。
2. 确认 Web Chat 默认 runner 或当前会话 runner 为 `traex`，work_dir 指向当前分支 worktree。
3. 在 WebUI Agent Chat 选择 Trae runner，发送一个较长 review 请求，例如：
   ```text
   对当前工作区当前分支 codex/traex-runner-streaming 做一次代码 review，仅 review，不做修改。请重点检查 Trae/external runner streaming、Web Chat/飞书进度卡片、默认 timeout、过程信息流式展示、工具调用摘要、最终结果位置、以及最近几个提交的回归风险。你需要真实读取 git diff、相关 Rust/TS 代码、E2E 和 human_tests 文档后再给结论。
   ```
4. 运行中观察 WebView 消息流与本地 session history 文件。
5. 等待任务完成后刷新同一 history 页面，再观察最终布局。

预期结果：
1. 运行中 WebView 不只显示命令数量汇总行；过程块默认展开，并能持续看到模型公开 content/status 与工具调用。
2. 工具调用行展示 `exec_command: <命令片段>`，而不是只重复显示一串 `exec_command`。
3. Trae/Codex 重复输出同一 `call_id` 的 `item.started` 时，WebView 不重复插入相同工具行，active commands 不会持续虚高。
4. 单条工具详情默认折叠，点击工具行后可查看输入和输出；输出过长时只在 WebView 中展示预览，完整输出保留在 run artifacts。
5. 完成后过程块默认折叠，最终 review 结论显示在该轮消息底部；用户仍可以手动展开过程块查看历史过程。

### TC-IEC-34: Feishu progress card 过程元素 ID 合法性回归

操作步骤：
1. 使用当前分支运行 progress card 单元回归：
   ```bash
   SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin feishu_progress_card_process_element_ids_stay_within_feishu_limits -- --nocapture
   ```
2. 构造包含 18 轮模型 content + 工具调用的 progress card，确认卡片 JSON 中包含两位数工具元素 ID。
3. 递归检查卡片中所有 `element_id`。
4. 复查最近飞书真实运行日志中是否存在 `code=300301` / `elementID format error`。

预期结果：
1. 过程工具元素使用短 ID（如 `ap_t_35` / `ap_td_35`），不会再生成 `agent_process_tool_10` 或 `agent_process_tool_10_detail` 这类超过飞书限制的 ID。
2. 所有 `element_id` 均以字母开头，只包含字母、数字和下划线，且长度不超过 20。
3. 飞书 progress card 后续 patch 不会因为元素 ID 格式错误被拒绝，长任务中间过程可以持续刷新。
4. 执行过程不展示 `Loop 1`、`Pipeline`、`工具摘要`、`[模型]`、run id、`turn started` 或 `model rerouted` 等内部/噪音提示。
5. 连续多个工具调用默认合并为“已运行 N 条命令”的一级折叠组；展开该组后，单条工具仍保持折叠，用户可继续展开查看输入/输出。
6. 模型公开 content 直接按时间顺序展示，不额外添加 `1.`、`2.`、`3.` 这类编号前缀。

### TC-IEC-35: 外部 Runner 运行中消息默认排队并续接原生 thread

操作步骤：
1. 启动一个长时间运行的 Codex 或 Trae external runner session，确保同一 `sessionKey` 处于 active 状态。
2. 在该 session 运行期间，从同一 IM 会话或 Web Chat session 再发送一条普通用户消息，不使用 `/stop`。
3. 读取即时响应或 progress card 队列状态。
4. 当前 run 完成后，读取下一轮 run 的 `runtime_snapshot.json`。
5. 分别对 Codex 和 Trae runner 执行上述检查。

预期结果：
1. Codex 和 Trae 这类 external/custom runner 运行中不做 guide 注入；普通新消息默认进入排队队列。
2. `/stop` 仍作为控制命令立即尝试停止当前外部 runner，不作为普通排队消息。
3. 当前 run 完成后自动处理下一条排队消息。
4. Codex 下一轮使用已保存的 `threadId` 构造 `codex exec resume ... <threadId> -`。
5. Trae 下一轮使用已保存的 `threadId` 构造 `traex exec resume ... <threadId> -`，不能退化为新建 `traex exec --json ... -`。

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
- 2026-05-24：执行 TC-IEC-21/22，临时服务端口 `18894`，`BIFROST_DATA_DIR=/tmp/bifrost-im-session-human.BK5xH7`，启动命令使用 `cargo run --bin bifrost -- start -p 18894 --unsafe-ssl --no-system-proxy`；mock Codex 第一次写入 `thread-human-1`，重启服务后第二次 Chat Gateway run `1779614415676-e874b7c3-6450-407e-9b27-98400a5de4b5` 的 `runtime_snapshot.args` 为 `exec --cd ... resume --json --output-last-message ... thread-human-1 -`，证明默认续接；随后 `/reset` 返回 `{"success":true,"cleared":true}`，第三次 run `1779614617852-f6cfc201-8263-436c-8f1a-7f2a537de88a` 的 `runtime_snapshot.args` 不包含 `resume` 或 `thread-human-1`，并写入 `externalThreadId:"thread-human-2"`，证明主动重建后不会复活旧线程。
- 2026-05-24：新增 TC-IEC-23，自动 E2E 通过仅 debug/dev 构建启用的 `BIFROST_CHATGPT_WEB_E2E_MOCK=1` 覆盖 ChatGPT Web Chat Gateway 重启恢复 `conversationId` 与 `/reset` 后不复活旧 conversation；本轮以 `cargo run -p bifrost-e2e -- --test im_gateway_chatgpt_web_restores_conversation_after_service_restart --test-timeout 180` 作为执行证据。
- 2026-05-25：执行 TC-IEC-24，命令 `source ~/.zshrc && cargo test -p bifrost-admin request_agent_stop_stops_external_runner_by_session_key --lib` 通过；mock runner 在 `sleep 2` 阶段收到 session key stop marker，最终状态为 `Stopped`。
- 2026-06-07：执行 TC-IEC-25/26/27，临时服务端口 `18890`，`BIFROST_DATA_DIR=/tmp/bifrost-traex-runner-e2e`，启动命令使用 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 cargo run --bin bifrost -- start -p 18890 --unsafe-ssl --no-system-proxy --skip-cert-check`；WebUI Trae run 修复 `permissionMode=default` 后可完成，Feishu session `feishu-main:ou_64f88363f262c64aba91f0b9e1aaed81` 的 history 显示 `Traex`、`Runner: traex`、`Ready`，过程块默认展开并显示 Trae 状态事件，最终答案可见；确认 Trae JSONL 简单问答只输出状态/最终消息时，Bifrost 不伪造工具事件，也不再显示外层 wrapper 工具调用。自动 E2E `RUN_REAL_TRAEX_E2E=1 SKIP_BUILD=true BIFROST_TRAEX_BIN=/Users/eden/.local/bin/traex e2e-tests/tests/test_im_gateway_traex_runner_streaming.sh` 通过，run `1780796012473-3e6b1d8a-51e5-45e5-a7c2-fef5d35b44f0` 返回 `BIFROST_TRAEX_E2E_STREAM_OK`，并断言 snapshot 不包含 `--permission-mode default`。
- 2026-06-07：执行 TC-IEC-28/29 基础链路，真实 Codex CLI 为 `/opt/homebrew/bin/codex`，版本 `codex-cli 0.136.0`；直接 CLI 计时验证 `thread.started`/`turn.started` 在进程结束前输出，`item.started command_execution` 与 `item.completed command_execution` 早于最终答案。自动 E2E `RUN_REAL_CODEX_E2E=1 BIFROST_CODEX_BIN=/opt/homebrew/bin/codex e2e-tests/tests/test_im_gateway_codex_runner_streaming.sh` 通过，run `1780806229800-fef7a0fa-5e06-4c30-b3b2-7604fec9751e` 返回 `BIFROST_CODEX_E2E_STREAM_OK`，并断言 `tool_started` 早于 `run_finished`。临时 WebUI 服务端口 `18891`，`BIFROST_DATA_DIR=/tmp/bifrost-codex-runner-ui`，Chat Gateway run `1780806481288-d1be0aaa-a68e-4279-be45-22fb9293bdd2` 返回 `BIFROST_CODEX_WEB_UI_STREAM_OK2`；Web history 显示 `Runner: codex`、`Ran 1 command`、单条 `exec_command` 和最终答案。飞书 IM 未做真实发送，本轮通过同一 session timeline 的 `tool_call/tool_result` 验证 progress card 可复用数据；真实飞书发送保留为人工补充验收。
- 2026-06-07：补充执行 TC-IEC-29 的飞书 progress card 状态/model 回归验证，命令 `SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin progress_card --lib -- --nocapture` 通过 18 个测试，新增断言外部 runner 卡片展示 `Runner: codex`、`Adapter: codex`、模型标签、外部会话、队列/引导、工作路径和最新工具，且不展示 `Loop`、`Context`、`Token`、`压缩`、`N/A` 等内置 Agent 空指标；命令 `SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin codex_ --lib -- --nocapture` 通过 14 个测试，确认 Codex run metadata 写入显式模型或默认模型标签；真实 Codex E2E `RUN_REAL_CODEX_E2E=1 BIFROST_CODEX_BIN=/opt/homebrew/bin/codex e2e-tests/tests/test_im_gateway_codex_runner_streaming.sh` 通过，run `1780809274058-4e032b02-5d7b-4793-9f15-5eee8d80d695`。
- 2026-06-07：补充执行 TC-IEC-29/30 的飞书 progress card 过程折叠回归验证，命令 `SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin progress_card -- --nocapture` 通过 20 个测试，断言全局状态在顶部、执行 Pipeline 在中间、最终结论在底部；运行中 Pipeline 默认展开，完成后 Pipeline 默认折叠；Pipeline 内按 Loop 先展示模型输出，再展示工具摘要，单条工具详情默认折叠且展开后展示输入/输出。命令 `SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin codex_cli_parser_maps_reasoning_summary_to_assistant_delta -- --nocapture` 通过，确认公开 reasoning summary 进入过程 timeline。真实 Codex E2E `RUN_REAL_CODEX_E2E=1 BIFROST_CODEX_BIN=/opt/homebrew/bin/codex e2e-tests/tests/test_im_gateway_codex_runner_streaming.sh` 通过，run `1780814063688-a2641c31-d00d-4906-9f01-a19940194cef`；真实 Trae E2E `RUN_REAL_TRAEX_E2E=1 SKIP_BUILD=true BIFROST_TRAEX_BIN=/Users/eden/.local/bin/traex e2e-tests/tests/test_im_gateway_traex_runner_streaming.sh` 通过，run `1780814083507-7b776ac0-6448-43ce-8ca8-7b90406ce14a`。真实飞书发送待本地服务重启后由飞书链路人工触发验证。
- 2026-06-07：执行 TC-IEC-31 的本地回归分析，真实飞书消息 `om_x100b6d659c70dd00b1ae9657765ca2a` 对应 Trae run `1780817198147-3a72a280-b4bc-44b0-9dfe-789f5f48d1c2`，`runtime_snapshot.json` 确认 executable 为 `/Users/eden/.local/bin/traex`、workDir 为 `/Users/eden/work/github/bifrost-traex-runner`；`normalized_events.jsonl` 有 59 条事件，包含多个 `agent_message`、`tool_started`、`tool_finished`，但 `result.json.status` 为 `timed_out`。补充回归测试 `SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin progress_card -- --nocapture` 通过 21 个测试，新增断言运行中的 `AssistantFinal/agent_message` 进入 Pipeline 且不提前写底部最终结论；`SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin timed_out_external_cli_result_reports_failure_reply -- --nocapture` 通过，确认 `TimedOut` 生成失败文案而不是使用早期 agent message。
- 2026-06-07：执行 TC-IEC-32 的问题复核，真实 Trae run `1780819082852-cfa6cdf8-b605-4ad0-a213-2bb778000d49` 有 55 条 normalized events（4 条 `assistant_final`、32 条 `tool_started`、16 条 `tool_finished`），说明 Trae 过程事件已经实时读到；问题在于配置仍带 180 秒超时以及卡片中重复 tool started 和超长输出详情导致中间更新不稳定。补充回归测试覆盖 Trae 默认无 timeout、重复 running tool 去重和工具详情输出预览限长；真实 Trae E2E `RUN_REAL_TRAEX_E2E=1 SKIP_BUILD=true BIFROST_TRAEX_BIN=/Users/eden/.local/bin/traex e2e-tests/tests/test_im_gateway_traex_runner_streaming.sh` 通过，run `1780821058530-058460b9-a487-4184-a01e-8eadeaf347f8`，并断言 `runtime_snapshot.timeoutSecs` 为空。
- 2026-06-07：执行 TC-IEC-33 的真实 Web Chat 长任务验证，当前分支服务使用 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 cargo run --bin bifrost -- start -p 9900 -H 0.0.0.0 --allow-lan --daemon --unsafe-ssl --skip-cert-check --no-system-proxy` 启动，最终 PID `59573`，系统代理保持禁用；Web Chat 通过 `runnerId=traex`、`adapter=traex`、workDir `/Users/eden/work/github/bifrost-traex-runner` 触发真实 Trae review，session `admin-chat-webview-trae2-1780826332`，history `/Users/eden/.bifrost/agent/sessions/2026/06/07/session-admin-chat-webview-trae2-1780826332-1780826332.jsonl`。流式响应自然结束为 `status:"succeeded"`，过程中出现多段 `assistant_final/agent_message` 与 `tool_started/tool_finished`，WebView 运行中可看到 `I'll review the current branch...`、`Let me read the key files...`、`Now let me check...` 等公开 content 穿插在 `exec_command: <命令>` 工具摘要之间。完成后刷新 history 页面，过程块默认折叠且不再显示 `我先执行一步检查。`，摘要按钮显示 `Ran 22 commands · 6m 57s`；点击摘要后可展开看到 content -> tool 摘要的交叉过程，且最终 review 结论位于过程块下方。点击首条 `exec_command` 工具行后显示 `Input:` 与 `Output:` 预览，长输出保留 `展开更多` 控件。
- 2026-06-07：执行 TC-IEC-34 的 Feishu progress card 卡住问题回归分析，真实飞书消息对应 Trae run `1780827766302-6a2c209c-84f6-4e64-b487-dcaaa8837361` 已成功完成，canonical history 包含大量 `assistant_delta`、`assistant_message`、`tool_call` 和 `tool_result`，但飞书日志出现 `code=300301` 与 `ElementID agent_process_tool_11_detail: Code 1002: elementID format error. Only alphabets, numbers, and underscores are allowed. It must start with an alphabet and not exceed 20 characters.`；确认根因是 progress card 第 10 条以后动态过程元素 ID 超过飞书 20 字符限制。修复后命令 `SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin progress_card -- --nocapture` 通过 23 个测试，新增用例覆盖 `ap_t_35` / `ap_td_35` 两位数工具 ID，并递归断言所有 `element_id` 符合飞书格式限制。
- 2026-06-07：根据真实飞书截图继续执行 TC-IEC-34 文案回归，去除执行过程中的 `Loop 1/2`、`Pipeline`、`工具摘要`、`[模型]`、纯 run id、`turn started` 与 `model rerouted` 等内部提示；过程区域改为从上到下展示公开模型内容和工具调用，工具详情仍保持可展开。命令 `SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin progress_card -- --nocapture` 与 `SKIP_FRONTEND_BUILD=1 cargo run -p bifrost-e2e -- --test im_gateway_agent_streaming_progress_card_renderer --jobs 1 --timeout 120` 均通过。
- 2026-06-07：继续执行 TC-IEC-34 的工具密集场景回归，飞书 progress card 将连续 3 条工具调用默认合并为 `已运行 3 条命令` 一级折叠组，展开后每条工具仍是独立折叠项；同时把过程 Markdown 从双空行压缩为单换行，减少行高和竖向占用。新增 `consecutive_process_tools_are_grouped_by_default` 回归测试。
- 2026-06-07：继续执行 TC-IEC-34 的过程文案回归，飞书 progress card 的模型公开 content 不再添加 `1.`、`2.`、`3.` 编号前缀，按时间顺序直接展示原文；新增断言覆盖 `1. 我先看分支差异` / `1. 我会先检查代码路径` 不应出现在卡片 JSON 中。
- 2026-06-07：执行 TC-IEC-35 的代码路径复核与单元回归，确认 IM event loop 和 Web Chat `/stream` 对 external/custom runner 的运行中新消息默认走 `SessionQueueManager` 排队，不做 guide 注入；`/stop` 仍单独尝试停止当前外部进程。补充 Trae 排队续跑回归，修复 `apply_external_cli_resume_metadata` 只给 Codex 注入 `threadId` 的问题，确保 Trae queued continuation 也能走 `traex exec resume ... <threadId> -`。命令 `SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin busy_message_mode -- --nocapture` 通过 9 个测试，`SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin 'adapter_builds_resume_command_from_thread_id' -- --nocapture` 通过 Codex/Trae 2 个 resume 命令构造测试。

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
     /tmp/bifrost-im-external-cli-dark.png \
     /tmp/bifrost-traex-runner-e2e
   ```
