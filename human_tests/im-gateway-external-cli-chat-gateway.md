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
2. 检查 AI -> Agent -> General 中存在 `Default Runner` 控件，选项只包含已配置的外部 runner ID（如 `codex`、`abc`），不出现已删除的内置 runner。
3. 打开 `http://127.0.0.1:18882/_bifrost/ai?agentSection=runners`。
4. 检查 Runners 页面默认展示 runner 列表，而不是直接展示大表单；页面内不再出现 `Default Custom Runner` 或默认 runner 下拉。
5. 点击 `Add Runner`，弹窗填写自定义 Runner ID、Adapter、Executable、Arguments、Skill Paths、Instructions 等配置；Adapter 下拉只展示 `Codex CLI`、`Trae CLI`、`ChatGPT Web`，不展示内部/未来扩展项 `Custom`、`Mock`；保存后列表中出现新 runner。
6. 编辑已有 runner，确认仍通过弹窗修改配置；每个 runner 都可独立选择 adapter。
7. 打开 `http://127.0.0.1:18882/_bifrost/ai?imGatewaySection=connections`。
8. 编辑一个 IM Provider，检查弹窗内 `Agent Runner` 控件选项包含 `Inherit global default` 和各自定义 runner ID。
9. 分别选择 `Inherit global default`、一个自定义 runner 并保存。
10. 通过 API 读取 Agent 配置、Provider 配置和 Runner config：
   ```bash
   curl -sS http://127.0.0.1:18882/_bifrost/api/im-gateway/agent
   curl -sS http://127.0.0.1:18882/_bifrost/api/im-gateway/providers
   curl -sS http://127.0.0.1:18882/_bifrost/api/im-gateway/chat/config
   ```

预期结果：
1. 全局 Agent General 是默认 runner 的唯一入口；Runners 页面只管理 runner 实体，不提供第二个默认值入口。
3. 选择自定义 runner（如 `abc`）时 Agent/Provider 配置直接保存为 `runner:"abc"`；Runner registry 的 `defaultRunnerId` 仅表示全局默认自定义 runner，IM channel 的 `runnerId` 仅作为通道覆盖。
4. IM Provider 可以通过 `agent_config.runner` 覆盖全局 runner，也可以清空为继承全局默认。
5. Provider 列表卡片展示当前覆盖状态；未覆盖时显示 `Global default`，自定义 runner 显示 runner ID。
6. 亮色和暗色主题下控件文案、下拉菜单和说明文字清晰可见。
7. Agent Runners 新建/编辑弹窗不向普通用户暴露 `Custom`、`Mock` adapter；历史配置或自动化测试使用这些 adapter 时仍由协议层兼容。

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
1. stop helper 在 external runner worker 接受停止请求时返回 `true`。
2. external runner 通过 session key stop marker 收敛为 `status:"stopped"`。
3. 不会等待 sleep 完成后输出迟到的 `assistant_final`。
4. 该行为覆盖 IM 忙碌态 `/stop` 与空闲态 `/stop` 共用入口。

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

### TC-IEC-27: Codex/Trae Permission 默认 Headless Full Access

操作步骤：
1. 配置 `traex` runner 时将 WebUI Permission Mode 保持为 `Headless default`，或 API 中省略 `permissionMode` / 传空值 / 传历史值 `default`。
2. 触发一次 Trae Chat Gateway run，读取本次 run 的 `runtime_snapshot.json` 和最终状态。
3. 再显式选择 `plan`、`auto` 或 `custom` 中任一非 bypass 模式，触发一次 run 并读取 `runtime_snapshot.json`。
4. 配置 `codex` runner 时保持默认 adapterConfig，不显式设置 `dangerFullAccess`、`sandbox` 或 `approvalPolicy`。
5. 触发一次 Codex Chat Gateway run，读取本次 run 的 `runtime_snapshot.json`。
6. 再显式为 Codex 配置 `sandbox` 或 `approvalPolicy`，触发一次 run 并读取 `runtime_snapshot.json`。

预期结果：
1. `runtime_snapshot.args` 不包含 `--permission-mode default`。
2. Trae 不再报错 `permission_mode = "default" is not supported in exec mode`。
3. 默认/空值/历史 `default` 会生成 `--dangerously-bypass-approvals-and-sandbox`，且不同时生成 `--permission-mode bypass_permissions`，避免 Trae CLI 报 `sandbox_mode` 与 `permission_mode` override 冲突，同时保持 IM/Web 无人值守 full access。
4. 如果用户显式选择 `plan`、`auto` 或 `custom`，后端生成对应 `--permission-mode <value>`，且不默认追加 full access；显式选择 `bypass_permissions` 时默认视为 full access，并只生成 dangerous full access 参数。
5. Codex 默认空配置会生成 `--dangerously-bypass-approvals-and-sandbox`，避免 IM/Web 无人值守链路等待二次授权。
6. Codex 显式配置 `sandbox` 或 `approvalPolicy` 时不被默认 full access 覆盖，除非同时显式设置 `dangerFullAccess=true`。

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
1. progress card 在运行中展开“执行过程”，按时间顺序展示公开模型 content 和工具调用；连续多个工具调用默认合并成“已执行 N 个步骤”的一级折叠组。
2. progress card 的全局状态位于卡片顶部，执行过程信息位于中间，完成后最终结论位于卡片底部；过程和状态面板默认折叠，但可以手动展开过程，再展开工具分组和单条工具详情查看完整信息，卡片不再停留在 running。
3. progress card 的状态面板展示 `Runner: codex`、`Adapter: codex`、模型标签、外部会话、队列/引导状态和工作路径。
4. 如果 runner 显式配置了 `adapterConfig.model`，模型标签展示该模型名；如果没有显式配置，只展示 `Codex 默认模型（未显式配置）`，不能猜测具体模型。
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
2. IM progress card 状态面板显示 Runner、Adapter、模型、外部会话、工作路径、队列/引导状态和最新工具。
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
5. 连续多个工具调用默认合并为“已执行 N 个步骤”的一级折叠组；展开该组后，单条工具仍保持折叠，用户可继续展开查看输入/输出。
6. 模型公开 content 直接按时间顺序展示，不额外添加 `1.`、`2.`、`3.` 这类编号前缀。

### TC-IEC-35: IM 默认 Queue、Web 默认 Guide，并续接原生 thread

操作步骤：
1. 启动一个长时间运行的 Codex 或 Trae external runner session，确保同一 `sessionKey` 处于 active 状态。
2. 在该 session 运行期间，从同一 IM 会话发送一条普通用户消息和一条 `/g <引导>`，并从 Web Chat Guide 模式发送一条消息。
3. 确认普通 IM 消息未触发 steer，而两条显式 Guide 路径让 app-server 收到当前 `threadId` / `turnId` 对应的 `turn/steer`；再发送 `/q <消息>` 或在 WebUI 选择 Queue。
4. 读取即时响应或 progress card 队列状态；当前 run 完成后，读取显式排队消息下一轮 run 的 `runtime_snapshot.json`。
5. 分别对 Codex 和 Trae runner 执行上述检查。

预期结果：
1. Codex 和 Trae 这类 app-server runner 的普通 IM busy 文本默认进入排队队列；显式 `/g` 和 WebUI Guide 才注入当前 turn。
2. `/stop` 仍作为控制命令立即尝试停止当前外部 runner，不作为普通排队消息。
3. 当前 run 完成后只自动处理显式排队或 Guide 失败降级的消息，成功注入的 Guide 不得重复执行下一轮。
4. Codex/Traex 排队下一轮复用已保存的 `threadId`；app-server transport 使用 `thread/resume` 后再 `turn/start`，不能退化为新建 thread。

### TC-IEC-36: Agent Chat 外部 Runner 运行中交互与 Threads 面板回归

操作步骤：
1. 使用 WebUI Agent Chat 选择或默认进入 Trae/Codex external runner。
2. 启动一个长时间运行的 external runner session。
3. 运行中在输入框输入一条普通追加消息。
4. 观察输入框上方运行中工具栏和实际 stream 请求。
5. 点击右侧 Threads 标题右侧的折叠按钮，刷新页面，再点击悬浮展开按钮。
6. 准备一个缺少 `runner_id` 但 `source` 或 `title` 包含 Trae/Traex 的 thread 摘要。

预期结果：
1. 除 ChatGPT Web 外，运行中 external runner 展示 Guide/Queue，默认选中 Guide；ChatGPT Web 只展示 Queue。
2. Guide 通过 `/g <message>` 请求注入当前 turn；切换 Queue 后通过 `/q <message>` 进入下一轮队列。
4. Threads 面板标题右侧按钮向右收起；收起后面板消失，只显示右上悬浮向左展开按钮。
5. 折叠状态写入 `localStorage`，刷新页面后仍保持；展开后写回未折叠状态。
6. Trae/Traex thread 的 runner 标记显示 Trae 短标（`Tr`），不误显示其它 runner 标记。
7. 如果 thread 摘要仍是 stale running，但 history timeline 已写入 `run_state_changed: completed` 和最终 assistant message，Web Chat 必须以 completed timeline 为准收敛为 Ready/Send，不再显示额外 `Thinking...`。

### TC-IEC-37: Agent Chat 外部 Runner 历史恢复状态唯一真源回归

操作步骤：
1. 使用默认 9900 端口和默认数据目录启动当前源码 Bifrost，启动命令必须包含 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1` 与 `--no-system-proxy`。
2. 打开一个历史上绑定 ChatGPT Web Runner 的会话，刷新 history URL，并检查顶部 Runner 标签、Threads 短标和详情 API。
3. 在该历史会话继续发送一条新消息，等待 runner 完成。
4. 重启 Bifrost 服务后再次打开同一个会话，继续发送一条短消息。
5. 打开一个历史上绑定 Trae Runner 的 Feishu/Web 会话，其 JSONL 中已包含 completed `run_state_changed` 和最终 `assistant_message`，但 session state 可能仍有 stale running。
6. 刷新该 history URL，展开已完成轮次的过程块。
7. 对一个新建 Web Chat 会话发送消息，观察运行中的 ChatUI 过程块。

预期结果：
2. 服务重启后，session detail API 仍从 history/session binding 恢复 `runner_id`、`runner_type` 和 external conversation/thread id；无法恢复原生线程时才显式降级为同 runner 的新线程。
4. 已完成的 history 以 completed timeline 或 thread summary 为唯一真源收敛为 Ready/Send，不继续显示 Running、Thinking 或 running placeholder；刷新页面后状态保持一致。
5. 已完成轮次的过程块默认折叠，展开后不重复展示最终 assistant 内容。
6. ChatUI 过程块不展示 `Run state: Running` 内部状态行；运行中只展示模型公开 content、工具组/工具摘要和必要的 Thinking 尾部提示。
7. 未来由 runner-call 子线程产生的用户可见消息会写回父 session canonical JSONL，刷新父会话不丢最后一轮消息；历史旧数据不做兼容回填。

### TC-IEC-38: Trae/Codex stdout turn.completed 不提前结束 Web Chat

操作步骤：
1. 执行 focused 单元回归：
   ```bash
   cargo test -p bifrost-admin external_runner_progress_run_finished_does_not_complete_before_final_response --lib -- --nocapture
   ```
2. 执行最终写入顺序回归：
   ```bash
   cargo test -p bifrost-admin external_runner_final_result_records_message_before_completed_state --lib -- --nocapture
   ```
3. 如果需要真实 WebUI 复核，启动当前源码 Bifrost，命令必须包含 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`、临时 `BIFROST_DATA_DIR` 和 `--no-system-proxy`，选择 Trae 或 Codex external runner，发送会触发延迟最终答案的请求，观察 history timeline。
4. 执行真实服务 E2E：
   ```bash
   SKIP_BUILD=true BIFROST_BIN=target/debug/bifrost e2e-tests/tests/test_im_gateway_external_runner_delayed_final_state.sh
   ```

预期结果：
1. stdout progress 中出现 `RunFinished` / `turn.completed` 后，canonical JSONL 不写入 `run_state_changed: completed`，session summary 仍保持 `running`。
2. 最终 `ExternalCliRunResult` 写入时，canonical JSONL 中最终 `assistant_message` 位于 `run_state_changed: completed` 之前。
3. Web Chat 在最终答案落入 history 前不提前显示 Ready；最终答案出现后才收敛为 Ready/Send。
4. 失败、停止、超时路径仍按最终 result status 收敛，不因忽略 progress `RunFinished` 而永久 running。
5. 自动 E2E 在临时数据目录和随机端口启动当前源码 Bifrost，构造先输出 `turn.completed`、延迟 5 秒再输出 `agent_message` 的 external runner；中途 `/agent/sessions/all` 必须返回 `running:true`、`status:"active"`、`state:"running"`、`run_state:"running"`，最终必须返回 `ended/completed`，且 JSONL 中最终 `assistant_message` 早于 `run_state_changed: completed`。

### TC-IEC-39: 飞书 Codex/Trae Runner 默认启用 progress card

操作步骤：
1. 执行 focused 单元回归：
   ```bash
   cargo test -p bifrost-admin feishu_codex_like_external_runner_defaults_to_progress_card_without_channel_override --lib -- --nocapture
   ```
2. 检查测试构造的 effective config：Provider 类型为 Feishu，adapter 分别为 `traex` 和 `codex`，runner 级 `deliveryMode` 为 `final_reply`，`sources.deliveryMode` 为 `runner`。
3. 检查测试中的显式覆盖分支：`sources.deliveryMode=channel` 时保留 `final_reply`；input override 为 `no_im` 时保留 `no_im`；非 Feishu Provider 保留 runner 默认值。

预期结果：
1. Feishu + Trae/Codex external runner 在没有 channel 显式 delivery override 时解析为 `ProgressCard`。
2. `final_reply` 作为 runner 默认值不会再让飞书通道跳过卡片，只输出最终文本。
3. 显式 channel deliveryMode 和 route/input delivery override 优先级不变，用户仍可主动选择 `final_reply` 或 `no_im`。
4. Weixin、ChatGPT Web、自定义非 Codex-like adapter 不受该默认策略影响。

### TC-IEC-40: Web History 最终回复展开保留 external runner 过程

操作步骤：
1. 执行前端 timeline 回归：
   ```bash
   pnpm --dir web exec vitest run src/pages/AI/AgentChatSection.timeline.test.ts
   ```
2. 重点检查 `attaches external runner process steps to the final assistant message` 用例：构造 Feishu + `traex` 的 history，事件顺序包含 `assistant_delta`、`tool_call`、`tool_result`、`run_state_changed: completed` 和最终 `assistant_message`。
3. 如需真实 WebUI 复核，打开对应 history 深链，展开已完成轮次的 `已处理 <duration>` 过程块，观察最终 assistant 回复下方是否显示 thinking/tool 过程。

预期结果：
1. history 转换后只有同一轮的用户消息和最终 assistant 消息，不生成额外 `Agent is running...` 占位 assistant。
2. 最终 assistant 消息的 `processSteps` 包含 thinking 和工具执行结果，展开后可看到过程信息。
3. 最终 assistant 内容仍是真实最终回复，不被过程占位文案替代。
4. 非 external runner 的普通 assistant delta 仍作为普通 assistant 内容展示，不被误收进过程块。

### TC-IEC-41: Windows stop marker 与 taskkill missing pid 幂等回归

操作步骤：
1. 执行 Windows CI 失败用例对应的本地窄测：
   ```bash
   SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin \
     external_cli_runtime_marks_stopped_run_before_late_stdout --lib -- --nocapture
   SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin \
     external_cli_runtime_stops_active_run_by_session_key --lib -- --nocapture
   ```
2. 执行新增的 missing active pid 与 `taskkill` 文案分类回归：
   ```bash
   SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin \
     request_run_stop_treats_missing_active_pid_as_stopped --lib -- --nocapture
   SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin \
     taskkill_missing_process_messages_are_idempotent --lib -- --nocapture
   ```
3. 推送后检查 GitHub Actions Windows Unit Tests，确认 `im_gateway::external_cli::tests::external_cli_runtime_marks_stopped_run_before_late_stdout` 和 `im_gateway::external_cli::tests::external_cli_runtime_stops_active_run_by_session_key` 不再因 `taskkill /PID ... /T /F exited with status exit code: 255` 失败。

预期结果：
1. stop marker 已写入但 active pid 已由另一路径终止或已退出时，停止请求仍返回成功并清理 active run 表。
2. Windows `taskkill` 输出进程不存在或 `no running instance of the task` 时按幂等停止处理。
3. `Access is denied` 等非 missing-process 错误不会被误判为成功。
4. 两个原 CI 失败用例最终仍收敛为 `status:"stopped"`，`response:"External CLI run was stopped by request."`。

### TC-IEC-42: Traex progress card 保留模型公开输出并隐藏机器状态

操作步骤：
1. 使用临时数据目录启动当前源码 Bifrost，必须包含 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1` 和 `--no-system-proxy`：
   ```bash
   BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 \
   BIFROST_DATA_DIR=/tmp/bifrost-traex-visible-<ts> \
   target/debug/bifrost start --host 127.0.0.1 -p <port> --unsafe-ssl --skip-cert-check --no-system-proxy
   ```
2. 通过 Chat Gateway defaults API 配置 `traex` runner，adapter 为 `traex`，executable 指向本机真实 Traex CLI，`sandbox=read-only`、`skipGitRepoCheck=true`、`deliveryMode=progress_card`。
3. 调用 `/chat/stream`，要求 Traex 先说明要检查什么，再执行版本检查命令，最后给出结论：
   ```bash
   curl -sS -N "http://127.0.0.1:<port>/_bifrost/api/im-gateway/chat/stream" \
     -H 'content-type: application/json' \
     -d '{"message":"检查一下 Traex 的版本是否需要更新。请先简短说明你准备检查什么，然后运行必要命令，最后给出结论。","sessionKey":"traex-visible-regression-<ts>","providerId":"feishu-main","runnerId":"traex","workDir":"/Users/eden/work/github/bifrost"}'
   ```
4. 读取本次 run 的 `cli.stdout.log`、`normalized_events.jsonl`、run detail 和 session JSONL，检查事件顺序。
5. 执行 focused card 回归：
   ```bash
   SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin im_gateway::progress_card --lib -- --nocapture
   ```

预期结果：
1. Traex 原始 stdout 和 `normalized_events.jsonl` 至少包含一条工具前的 `agent_message`/`assistant_final`，例如“检查当前 Traex 版本并与最新可用版本对比”。
2. 工具事件仍按 `tool_started`/`tool_finished` 进入 timeline，工具前后的公开模型 content 与工具调用保持交叉顺序。
3. `tool_calls`、`waiting_on_session`、`model_request`、`model_response`、`turn started`、`model rerouted:*` 等机器状态不进入 progress card 过程区域。
4. 完成态 progress card 底部展示最终结论；如果最后一条运行中模型 content 与最终输出完全相同，过程区域去重该终态重复项，但保留更早的公开模型 content 和工具过程。

### TC-IEC-43: Web Chat 外部 Runner 图片保存到 session 附件并以绝对路径注入 prompt

操作步骤：
1. 使用临时数据目录启动当前源码 Bifrost，必须包含 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`、`BIFROST_DISABLE_TRAY=1` 和 `--no-system-proxy`：
   ```bash
   BIFROST_DATA_DIR=/tmp/bifrost-runner-image-real-<ts> \
   BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 \
   BIFROST_DISABLE_TRAY=1 \
   cargo run --bin bifrost -- start --host 127.0.0.1 -p <port> --unsafe-ssl --skip-cert-check --no-system-proxy
   ```
2. 配置一个 mock external runner，executable 从 stdin 读取 prompt 并输出 `{"type":"assistant_final","content":"BIFROST_IMAGE_PATH_OK"}`。
3. 调用 `/_bifrost/api/im-gateway/chat/stream`，请求体包含 `runnerId`、`sessionKey` 和 `images:[{"mimeType":"image/png","data":"<base64>","name":"hello.png"}]`。
4. 使用同一个 `sessionKey` 再调用一次 `/_bifrost/api/im-gateway/chat/stream`，传入不同字节的第二张图片。
5. 读取两次返回的 `runId`，检查 `agent/im_gateway/chat_runs/<runId>/prompt.md`、`result.json` 和 session 附件目录。

预期结果：
1. stream 返回 `eventType:"run_finished"`、`status:"succeeded"`，response 为 `BIFROST_IMAGE_PATH_OK`。
2. `prompt.md` 包含 `## Attached Images`，并列出本地绝对图片路径。
3. 图片文件位于 `agent/sessions/by-key/attachments/<session-file-stem>/<runId>/images/image-1.png`，文件字节与请求图片一致。
4. `result.json.metadata["attachments.images"]` 记录图片 path、mimeType、sizeBytes 和 name。
5. session timeline 中该 user message 带图片内容，Web History 回放能感知这一轮包含图片。
6. 同一个 `sessionKey` 的第二轮图片路径与第一轮不同，第一轮 `result.json.metadata["attachments.images"]` 中记录的旧路径仍存在且文件字节未被第二轮覆盖。

### TC-IEC-44: Web Slash Runner Call 支持 image-only 图片输入

操作步骤：
1. 延续 TC-IEC-43 的临时服务和 mock runner 配置。
2. 调用 `/_bifrost/api/im-gateway/chat/runner-calls/stream`：
   ```bash
   curl -sS -N "http://127.0.0.1:<port>/_bifrost/api/im-gateway/chat/runner-calls/stream" \
     -H 'content-type: application/json' \
   ```
3. 读取返回的 `runId`、目标 child session 和 run artifacts。

预期结果：
1. image-only 请求不再返回 `message or images are required`。
2. stream 返回 `runner_call_started`、`assistant_final`、`runner_call_finished`，最终 status 为 `succeeded`。
3. 目标 runner prompt 包含 `## Attached Images` 和绝对图片路径。
4. 图片保存到目标 child session 独立的 `attachments/<session-file-stem>/<runId>/images/` 目录，不覆盖 TC-IEC-43 的普通 Web Chat 图片。
5. 父会话可见用户触发了带图 runner-call，空文本时显示 `Attached 1 image` 语义。

### TC-IEC-47: 外部 Runner 图片附件目录拒绝 API 调用方覆盖

操作步骤：
1. 启动临时 Bifrost 服务并配置 mock Codex/Traex-compatible runner。
2. 调用 `/_bifrost/api/im-gateway/chat/stream`，请求体在 `params` 中故意传入 `attachmentBaseDir` 指向服务数据目录外的可写路径：
   ```bash
   curl -sS -N "http://127.0.0.1:<port>/_bifrost/api/im-gateway/chat/stream" \
     -H 'content-type: application/json' \
     -d '{"runnerId":"mock-image","sessionKey":"human-malicious-attachment-base","message":"describe this","images":[{"mimeType":"image/png","data":"<base64>","name":"evil.png"}],"params":{"attachmentBaseDir":"/tmp/bifrost-evil-attachments"}}'
   ```
3. 读取 run `result.json.metadata["attachments.images"]` 和 prompt。
4. 检查调用方指定的恶意目录是否被创建。

预期结果：
1. 恶意 `attachmentBaseDir` 不被使用，目录不会被创建。
2. 图片落盘路径位于服务端 session attachment dir 的 `<run-id>/images/image-1.png` 下，或在无法取得 session recorder 时降级到 run dir 内部附件目录。
3. prompt 中的 `## Attached Images` 只引用服务端生成的绝对路径。
4. run metadata 与 session detail 中的附件审计路径一致。


操作步骤：
2. 等待 runner-call 返回成功 run id。
3. 调用 `/_bifrost/api/im-gateway/agent/sessions/<callerSessionKey>`。

预期结果：
1. 父 session detail 的 `metadata.runner.adapter`、`metadata.attachments.count`、`metadata.cli.*` 与目标外部 run 的 `result.json.metadata` 一致。
2. 父 session 历史包含用户可见的 `Run with <target>` 触发消息。
3. 不需要重新打开或重建父 session，也能通过 `latest_run_id` 合并目标 run metadata。

### TC-IEC-45: Feishu 图片消息进入 Codex/Trae 外部 Runner

操作步骤：
1. 配置 Feishu provider，将该通道默认 Runner 设为 Codex 或 Traex external runner，delivery mode 使用 `progress_card` 或 `final_reply`。
2. 从飞书给该 bot 发送一条带图片的消息，正文可为空或为“请看这张图”。
3. 在本地数据目录中查找对应 session JSONL、chat run 目录和 runner prompt。
4. 再紧接着发送一条纯文本消息，验证 queued/下一轮不会复用上一轮图片。
5. 再发送第二条带不同图片的消息，验证同一飞书会话下两次带图消息分别保存到不同 run id 子目录。

预期结果：
1. event loop 调用 provider resolver 下载图片后，将图片转成 external CLI request 的 `images[]`。
2. Codex/Trae prompt 包含 `## Attached Images` 和 session 附件绝对路径；附件文件字节可读取。
3. progress card 或 final reply 正常发送到原飞书会话，不出现“看不到图片”这类因未传图导致的失败。
4. 后续纯文本消息的 runner prompt 不再包含上一轮图片路径。
5. 第二条带图消息不会覆盖第一条带图消息的附件文件；第一条 run 的 metadata 路径仍可读取原始图片字节。

### TC-IEC-48: Feishu/IM 文件附件进入外部 Runner prompt

操作步骤：
1. 使用临时数据目录启动当前源码 Bifrost，必须包含 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`、`BIFROST_DISABLE_TRAY=1` 和 `--no-system-proxy`。
2. 配置一个 Codex-compatible mock external runner，executable 从 stdin 读取 prompt 到捕获文件，并输出 `{"type":"assistant_final","content":"BIFROST_IMAGE_PATH_OK"}`。
3. 创建一个 Feishu provider，`enabled=true`、`event_connection_enabled=false`，并在 `agent_config.runner` 中绑定该 mock runner。
4. 调用 `/_bifrost/api/im-gateway/debug/mock-inbound`，向该 provider 注入一条纯文件消息，payload 至少包含：
   ```json
   {
     "providerId": "file-inbound-provider",
     "userId": "file-inbound-owner",
     "chatId": "chat-file-inbound-provider",
     "text": "",
     "files": [
       {
         "fileKey": "mock-file-report",
         "name": "../report final.md",
         "mimeType": "text/markdown",
         "data": "<base64 of # Report\\n\\nhello from file\\n>"
       }
     ]
   }
   ```
5. 等待 `agent/im_gateway/chat_runs/<runId>/result.json` 写入，读取同一 run 的 `prompt.md`、`result.json.metadata` 和附件目录。

预期结果：
1. debug inbound API 返回 `success:true`，纯文件消息不再因正文为空被拒绝。
2. external runner 成功执行，`prompt.md` 包含 `## Attached Files`，并列出本地绝对文件路径。
3. 文件落盘在本次 run 的 `attachments/files/` 或 session 附件子目录下，文件名经过安全净化，例如 `1-report_final.md`，且文件字节与注入的 base64 内容一致。
4. `result.json.metadata["attachments.files"]` 记录 path、mimeType、sizeBytes、name；`attachments.fileCount=1`、`attachments.imageCount=0`、`attachments.count=1`。
5. 捕获到的 runner stdin prompt 中包含同一个附件路径，说明 Agent 实际能在 prompt 里看到用户发送的文档路径。

### TC-IEC-46: Codex/Traex Runner diagnostics 采集并在 Web UI 展示

操作步骤：
1. 使用当前源码编译产物启动 9900 或临时端口服务，必须包含 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`、`BIFROST_DISABLE_TRAY=1` 和 `--no-system-proxy`。
2. 配置一个 Codex-compatible mock runner 和一个 Traex-compatible mock runner，二者都从 stdin 读取 prompt，输出包含 tool started、tool completed、assistant final 的 JSONL，并让 executable 支持 `--version`。
3. 分别调用 `/_bifrost/api/im-gateway/chat/stream`，请求体包含对应 `runnerId`、`sessionKey`、文本和一张图片。
4. 读取两个 run 的 `result.json.metadata`，并调用 `/_bifrost/api/agent/sessions/detail?session_key=<sessionKey>`。
5. 打开 Web UI Agent Chat 对应 session 的状态弹窗，查看 Context 卡片下方的 `Runner diagnostics`。

预期结果：
1. Codex 与 Traex run metadata 均包含 `cli.executable`、`cli.args`、`cli.version`、`runner.adapter`、`prompt.estimatedTokens`、`attachments.count`、`attachments.totalBytes`、`io.stdoutBytes`、`io.stderrBytes`、`timing.totalDurationMs`、`tools.count`、`tools.totalDurationMs`、`resume.requested`。
2. session detail 响应包含同一组 metadata，Web UI 可从 `SessionDetail.metadata` 展示 diagnostics，不依赖仅存在于 run artifact 的本地文件读取。
3. Web UI 显示 CLI、Prompt、Attachments、I/O、Tools、Tool time、Run time、First event、Resume 等行；缺失字段显示 `-`，不伪造 context window、剩余 context 或 billing token。
4. `normalized_events.jsonl` 中 tool completed raw event 带 `observedAtMs`，同一 tool 有 started/completed 时带 `durationMs`。

### TC-IEC-49: Codex/Traex Runner 模型与思考配置在飞书卡片和 Web UI 状态栏展示

操作步骤：
1. 准备隔离的 Codex 与 Traex 配置目录，分别写入默认模型和思考配置：
   ```bash
   codex_home=$(mktemp -d)
   trae_home=$(mktemp -d)
   cat >"$codex_home/config.toml" <<'TOML'
   model = "gpt-5.1-codex"
   model_provider = "codex-provider"
   model_reasoning_effort = "high"
   model_reasoning_summary = "auto"
   TOML
   cat >"$trae_home/traecli.toml" <<'TOML'
   model = "trae-default-model"
   model_provider = "trae-provider"
   reasoning_effort = "medium"
   reasoning_summary = "concise"
   TOML
   CODEX_HOME="$codex_home" TRAE_HOME="$trae_home" cargo test -p bifrost-admin codex_and_traex_model_config_resolves_user_defaults_and_overrides -- --nocapture
   ```
2. 执行飞书卡片摘要回归：
   ```bash
   cargo test -p bifrost-admin progress_card --lib -- --nocapture
   cargo test -p bifrost-admin online_notification --lib -- --nocapture
   ```
3. 执行线上通知 E2E：
   ```bash
   SKIP_BUILD=true BIFROST_BIN=target/debug/bifrost e2e-tests/tests/test_im_online_notification_runner_context.sh
   ```
4. 执行 Web UI 状态栏回归：
   ```bash
   pnpm --dir web exec vitest run src/pages/AI/AgentChatSection.timeline.test.ts
   pnpm --dir web exec playwright test tests/ui/agent-chat.spec.ts -g "keeps running history token HUD synced with live status" --reporter=line
   ```

预期结果：
1. Codex resolver 从 `CODEX_HOME/config.toml` 读取 `model`、`model_provider`、`model_reasoning_effort`、`model_reasoning_summary`；Traex resolver 从 `TRAE_HOME/traecli.toml` 读取 `model`、`model_provider`、`reasoning_effort`、`reasoning_summary`。
2. Runner 配置中的显式 `model/reasoningEffort/reasoningSummary` 覆盖默认配置，并在 run metadata、active status 和 session detail 中保持一致。
3. 飞书 progress card 的摘要区域展示模型来源和思考配置，例如 `模型：gpt-5.1-codex（Codex 配置） · 思考：high · 摘要：auto`。
4. Feishu online notification 摘要包含 `Model`、`Reasoning Effort`、`Reasoning Summary` 三行；未知值显示 `N/A`，不省略字段。
5. Web UI Agent Chat 状态弹窗的 Context 区域展示 `Model` 和 `Reasoning`，运行中 live status 优先于陈旧历史快照。

### TC-IEC-50: Codex/Traex/Claude Code `/models` 与 `/model` session 模型切换

操作步骤：
1. 启动临时 Bifrost 服务，必须设置 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`、`BIFROST_DISABLE_TRAY=1`，并使用 `--no-system-proxy`。
2. 配置 Codex-compatible runner 和 Traex-compatible runner，分别让 `adapterConfig.executable` 指向 mock `codex` / mock `traecli`；两个 mock 的 `debug models` 都返回包含 `Doubao-Unit`、`visibility:"list"`、`base_instructions` 的模型 catalog，同时包含一个 `visibility:"hidden"` 的模型。
3. 配置 Claude Code runner，adapter 为 `claude_code`；无需 mock `debug models`，因为 Claude Code CLI 当前没有稳定模型枚举命令，Bifrost 使用内置 alias catalog。
4. 通过 `/_bifrost/api/im-gateway/chat/stream` 分别对 Codex、Traex 与 Claude Code session 发送 `/models`。
5. 通过同一接口对 Codex/Traex 发送 `/model definitely-not-a-real-model`，再发送 `/model Doubao-Unit`；对 Claude Code 发送 `/model bad model`、`/model sonnet` 与 `/model claude-opus-4-5-20251101`。
6. 再发送普通消息，读取该 run 的 `runtime_snapshot.json` 和 `session_state.json`。
7. 打开 Web UI Agent Chat，选中 Codex、Traex 或 Claude Code runner 时在 composer 输入 `/`，验证 slash 面板包含 `/models` 和 `/model`；`/models` 在输入过程中默认选中，回车直接发送；`/model` 回车或 Tab 只补齐命令并把光标放在尾部，方便继续输入模型名。
8. 在已有历史对话线程中发送 `/models`、`/model Doubao-Unit` 或 `/model sonnet` 和一条普通消息，然后刷新页面并继续发送下一条消息。

预期结果：
1. Codex/Traex `/models` 返回 `Doubao-Unit`、reasoning/tier/visibility 等白名单信息，不包含 `base_instructions` 或隐藏模型；Claude Code `/models` 返回 `sonnet`、`opus`、`fable` alias，并说明 Claude Code 可接受 alias 或完整模型名。
2. Codex/Traex 非法模型返回可见拒绝消息，不写入 `modelOverride`，下一条普通 run 不使用非法模型；Claude Code 包含空格等非法 slug 时同样拒绝。
3. `/model Doubao-Unit` 或 `/model sonnet` 将当前 `sessionKey + adapter + runnerId` 的 `modelOverride` 持久化为对应模型，来源为 `session slash command`。
4. 下一条普通 Codex/Traex run 的启动参数包含 `--model Doubao-Unit`；下一条普通 Claude Code run 的启动参数包含 `--model sonnet` 或 `--model claude-opus-4-5-20251101`；session slash override 覆盖 runner 默认模型。
5. Web UI 仅在当前 runner adapter 为 `codex`、`traex` 或 `claude_code` 时展示 model slash 命令；其它 adapter 不展示这两个入口。
6. slash 命令和系统回执刷新后仍在消息列表中，发送下一条普通消息后不消失，但不会作为用户 prompt 注入 runner 上下文。
7. 飞书 IM 空闲状态下 `/models`、`/model` 使用同一 session override；运行中发送 `/model` 明确提示等待当前任务结束，不把 `/model` 当普通 prompt 送进 Codex/Traex/Claude Code。
8. Agent Chat 输入框上方 token HUD 在刷新、发送下一条消息和 run 完成后持续展示当前模型、token 与 context，不因 history summary 或运行中空 status 快照退回 `Tokens -`、`Context 0%` 或隐藏模型名。
9. Web UI 已有历史 assistant 回复后再次发送 `/model <name>`，页面立即追加独立居中的系统行 `切换模型为 <name>`，不替换、不隐藏、不合并最后一条 assistant 回复；刷新后历史显示与即时显示一致，且该系统行不作为 user/assistant 消息注入 runner prompt。

### TC-IEC-51: Agent Chat 多轮历史分页与运行中刷新稳定性

操作步骤：
1. 用当前编译版本启动 9900，必须设置 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`、`BIFROST_DISABLE_TRAY=1`，并使用 `--no-system-proxy`、`--no-tray`、`--no-intercept`。
2. 打开一个已有多轮 external runner 会话，例如 `/_bifrost/ai?aiSection=agent-chat&agentSection=chat&session=admin-chat-1782407491650&view=active`。
3. 确认初始页面默认只展示最近若干轮对话，而不是只展示最后一条 running/assistant 消息；页面顶部存在 `Load older`。
4. 点击 `Load older`，等待 2 秒以上，确认旧消息仍留在页面中，不被 timeline 刷新重新裁掉。
5. 刷新页面，确认页面恢复为默认最近若干轮窗口，并继续显示 `Load older`。
6. 在同一会话发送一条真实 external runner 消息，分别在发送后立即、运行中 5 秒、运行完成后和再次刷新后检查消息列表。

预期结果：
1. 后端 session detail 中的 user/assistant/system 聊天主干是 Web UI 消息列表的稳定锚点；timeline tail 只补充最后一轮过程信息，不能替换整段聊天历史。
2. 默认窗口按“人类消息轮次”裁剪最近若干轮，而不是按 timeline event 数量裁剪；一个噪声很多的 run 不能把页面压缩成最后一条消息。
3. `Load older` 点击后展开的旧消息在等待、运行中刷新、SSE catch-up 和完成后刷新期间不消失。
4. 发送新消息运行中，消息列表仍保留多轮上下文，`Load older` 不因 timeline `has_more=false` 临时消失。
5. 运行完成和刷新页面后，最新用户消息和 assistant 回复仍显示，旧消息窗口与 `Load older` 状态一致。

### TC-IEC-52: Agent Chat 任务计划浮层 hover 与复制稳定性

操作步骤：
1. 用当前编译版本启动 9900，必须设置 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`、`BIFROST_DISABLE_TRAY=1`，并使用 `--no-system-proxy`、`--no-tray`、`--no-intercept`。
2. 打开一个存在 external runner plan/todo 的 Agent Chat 会话，例如 `/_bifrost/ai?aiSection=agent-chat&agentSection=chat&session=admin-chat-1782407491650&view=active`。
3. 将鼠标移动到输入框上方的任务计划胶囊，例如 `Step 2/4 ... +3`。
4. 确认浮层显示 `Task progress` 和完整任务列表。
5. 将鼠标从胶囊慢速移动到胶囊上方的浮层，经过胶囊与浮层之间的空隙后停留在浮层上。
6. 在浮层内拖选一条任务文本，例如 `审查未提交 diff`。
7. 将鼠标移出浮层和胶囊区域。

预期结果：
1. 鼠标停留在胶囊、胶囊与浮层之间的透明桥接区域、浮层本身任一区域时，浮层保持展开，不闪烁、不消失。
2. 鼠标进入浮层并停留超过关闭延迟后，浮层仍展示完整 `Task progress` 内容。
3. 浮层内任务文本可以被选中，便于复制。
4. 鼠标离开整组区域后，浮层正常关闭。

### TC-IEC-53: Agent Chat Web UI 图文用户消息气泡刷新后保留图片

操作步骤：
1. 使用当前编译版本重启 `9900` 服务，启动参数包含 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`、`BIFROST_DISABLE_TRAY=1`、`--no-system-proxy`、`--no-tray`、`--no-intercept`。
2. 打开真实 Web UI 会话：
   ```text
   http://127.0.0.1:9900/_bifrost/ai?aiSection=agent-chat&agentSection=chat&session=admin-chat-1782407491650&view=active
   ```
3. 在 composer 中粘贴一张 PNG 图片，并输入文本 `图片气泡展示回归：这张测试图里有什么？`。
4. 发送消息后等待用户消息出现在消息列表。
5. 检查该用户消息的同一个气泡内同时展示文本和图片预览。
6. 刷新页面，重新检查同一条用户消息。
7. 查询 session detail API，确认最后一条用户消息包含 text 和 `image_url` 两类 `content_parts`。

预期结果：
1. 发送后用户气泡内有文本和图片预览，图片不被拆到独立消息，也不只出现在发送前预览区。
2. 刷新后该用户气泡仍显示同一张图片，timeline/history 恢复不会丢弃 `content.images`。
3. session detail 中该用户消息的 `content_parts` 包含文本 part 与 `data:image/png;base64,...` 图片 part。
4. Runner 能读取图片并完成回复；本用例重点验收 Web UI 消息气泡展示，不要求模型回复内容固定。

### TC-IEC-54: Feishu Runner 进度卡 card_id 瞬时失效恢复

操作步骤：
1. 使用当前源码执行 Feishu progress card mock 回归：
   ```bash
   SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin start_feishu_after_finished_card_recovers_from_invalid_card_id_send --lib -- --nocapture
   ```
2. mock Feishu 服务先创建并发送第一轮进度卡，随后将该轮标记完成并关闭 streaming。
3. 模拟同一 session 立即开始下一轮外部 Runner 消息；mock 服务让下一轮第一次 `send card entity` 返回：
   ```json
   {"code":230099,"msg":"Failed to create card content, ext=ErrCode: 11310; ErrMsg: cardid is invalid;"}
   ```
4. 检查测试断言和请求计数。
5. 在真实飞书环境中，上一轮 Runner 结束后立即发送下一轮消息，观察 IM 消息流。

预期结果：
1. progress session 识别 `cardid is invalid` 为可恢复错误，重新创建新的 CardKit card entity 并重试发送一次。
2. 第二轮进度卡发送成功后，registry handle 指向重试后的新 `card_id`，后续进展和最终结论更新同一张新进度卡。
3. 旧的已完成卡片不被撤回、不被改写；新一轮只在最新用户消息之后创建新进度卡。
4. 即使进度卡启动仍失败，外部 Runner `ProgressCard` delivery 也不得先发送 `已开始处理 Runner 任务。` 占位消息，避免最终结果之外多出一张无实时过程的卡片。

### TC-IEC-55: Agent Chat 已有对话记录中模型切换系统消息刷新后保留

操作步骤：
1. 使用当前编译版本重启 `9900` 服务，启动参数包含 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`、`BIFROST_DISABLE_TRAY=1`、`--no-system-proxy`、`--no-tray`、`--no-intercept`。
2. 打开一个已有多轮 external runner 对话记录的 Web UI 会话，例如：
   ```text
   http://127.0.0.1:9900/_bifrost/ai?aiSection=agent-chat&agentSection=chat&session=admin-chat-direct-system-check-1782409300&view=active
   ```
3. 在该会话内执行 `/model <可用模型>`，确认页面不出现空白系统气泡，完成后立即显示独立居中的系统提示 `切换模型为 <可用模型>`。
4. 查询 session detail API：
   ```bash
   curl -sS 'http://127.0.0.1:9900/_bifrost/api/im-gateway/agent/sessions/<sessionKey>'
   ```
5. 刷新 Web UI 页面，重新检查消息列表。
6. 确认普通 user/assistant 对话记录仍完整显示，系统提示没有被折叠进某条 user/assistant 气泡，也没有替换最后一条 assistant 回复。

预期结果：
1. session detail 的 `messages` 同时包含 canonical timeline 的 user/assistant 消息和 `role:"system"` 的模型切换展示消息。
2. 刷新后 Web UI 仍展示独立居中的 `切换模型为 <可用模型>` 系统行，系统行有 `agent-chat-message-system` 与 `agent-chat-message-bubble-system`。
3. user/assistant 正常对话仍按轮次展示，不会因为系统消息合并而消失、塌缩或只剩最后一轮。
4. 模型切换系统消息仅用于 Web UI display/detail，不作为正式 user/assistant prompt 污染后续 runner 上下文。
5. 模型切换系统行是轻量提示样式：文字和时间同一行、字体与时间一致、无边框、无背景，不展示成正式消息气泡。
6. `/model` 命令运行期间不插入 `content` 为空的 system message；DOM 中 `agent-chat-message-system` 的文本内容不能为空。

### TC-IEC-56: 飞书 External CLI Runner 多图输入不丢失

操作步骤：
1. 执行 Feishu 富文本多图解析单元测试：
   ```bash
   SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin test_normalize_feishu_post_extracts_text_and_images --lib -- --nocapture
   ```
2. 执行外部 Runner 多图附件写入单元测试：
   ```bash
   SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin external_cli_run_writes_image_attachments_and_injects_prompt_paths --lib -- --nocapture
   ```
3. 执行 IM event loop route 纯图片回归：
   ```bash
   SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin im_event_loop_external_cli_route_processes_image_only_message --lib -- --nocapture
   ```
4. 执行忙碌外部 Runner 队列图片保留回归：
   ```bash
   SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin test_queue_preserves_image_attachments --lib -- --nocapture
   ```
5. 执行队列图片出队后转 External CLI 输入的多图转换回归：
   ```bash
   SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin external_cli_images_from_chat_images_preserves_payloads --lib -- --nocapture
   ```
6. 执行真实临时服务 E2E，覆盖 Web Chat external runner 首轮两张图片、后续单图、Traex 兼容 runner 和 runner-call 图片输入：
   ```bash
   SKIP_BUILD=true BIFROST_BIN=target/debug/bifrost e2e-tests/tests/test_im_gateway_external_runner_image_input.sh
   ```

预期结果：
1. Feishu `post` 内容中两张 `image_key` 都进入 `ImEventMessage.images`。
2. External CLI runtime 在 prompt 中注入 `## Attached Images`，并同时列出 `image-1.png` 与 `image-2.jpg`。
3. `result.metadata["attachments.images"]` 包含两张图片，MIME 分别为 `image/png` 和 `image/jpeg`。
4. `ExternalCliAgentChat` route 对纯图片消息不跳过，真实创建 mock runner run。
5. 外部 Runner 忙碌时排队的图片保存在 `QueueItem.images`，后续出队执行时不会只剩 `[图片消息]` 文本占位。
6. `ChatImageInput` 转 `ExternalCliImageInput` 时保留多图顺序、MIME 和 base64 数据。
7. E2E 首轮 run 的 `attachments.images` 同时包含 `hello.png` 与 `hello-two.jpg`，后续 run 不覆盖首轮图片文件。

### TC-IEC-57: Codex/Traex 运行中向当前 turn 注入引导消息

前置条件：

1. 当前 worktree 已编译 `target/debug/bifrost`。
2. Python 3 与 curl 可用；测试不依赖真实模型账号，使用实现完整 JSON-RPC 握手、`turn/steer` 与 token usage 通知的临时 app-server。

操作步骤：

1. 执行真实临时服务 E2E：
   ```bash
   SKIP_BUILD=true BIFROST_BIN=target/debug/bifrost \
     e2e-tests/tests/test_external_runner_live_guide.sh
   ```
2. 脚本分别启动 Codex 与 Traex session，并在各自 `turn/start` 后执行：
   ```bash
   bifrost agent guide --session <session-key> --json "focus-<runner>"
   ```
3. 脚本读取 mock app-server 收到的 `turn/steer`、Chat stream 的 `run_finished`、run detail 的 snapshot/metadata，并对不存在的 session 调用 guide API。

预期结果：

1. Codex 和 Traex 默认或显式 `transport=app_server` 均通过独立 `bifrost-runner` worker 启动 stdio app-server；Codex 使用 `app-server --stdio`，Traex 使用其版本支持的 `app-server --listen stdio://`。
2. CLI 返回 `delivery=steered`，`threadId`、`turnId` 与当前 app-server turn 一致。
3. app-server 收到 `turn/steer`，包含当前 `threadId`、`expectedTurnId`、唯一 `clientUserMessageId` 和引导文本；引导不会启动第二个 CLI 进程或新 turn。
4. 原 stream 在同一 turn 内返回引导后的最终答案，run 状态为 `succeeded`。
5. run detail 的 snapshot 为对应 CLI 的 stdio app-server 参数，metadata 持久化 `threadId` 和 input/output/total token usage。
6. 非活动 session 返回 HTTP 409 和 `delivery=rejected`，不会静默创建任务。
7. 脚本退出时停止临时 Bifrost 并删除临时数据目录、mock executable 和日志。

### TC-IEC-58: IM 与 WebUI 统一运行中 Guide/Queue 语义

前置条件：

1. 当前 worktree 已编译 `target/debug/bifrost`。
2. Python 3、curl、pnpm 与 Playwright 浏览器可用；真实服务 E2E 使用隔离数据目录和 mock app-server，不修改本机 `9900` 服务。

操作步骤：

1. 执行 busy mode、IM help 和 Guide 失败降级不丢消息的 Rust 回归：
   ```bash
   cargo test -p bifrost-admin busy_message_mode --lib
   cargo test -p bifrost-admin im_help_for_ --lib
   cargo test -p bifrost-admin guide_stream_falls_back_to_queue_without_losing_message --lib
   ```
2. 执行真实临时服务 E2E：
   ```bash
   SKIP_BUILD=true BIFROST_BIN=target/debug/bifrost \
     e2e-tests/tests/test_external_runner_live_guide.sh
   ```
3. 执行 WebUI 运行中输入回归：
   ```bash
   pnpm --dir web exec playwright test tests/ui/agent-chat.spec.ts \
     -g "(defaults external runners to Guide|keeps ChatGPT Web running input|supports running stop, guide)"
   ```

预期结果：

1. 飞书/微信 IM 的 Codex、Traex、Claude Code 与其他 runner 在 session 忙碌时，普通消息默认加入下一轮 FIFO 队列；只有 `/g <消息>` 明确请求当前 turn 引导。
2. 显式 `/g` 时 Codex/Traex app-server 收到 `turn/steer`，引导文本只进入当前 turn，不会因等待 steer ACK 阻塞 runner 控制循环，也不会在成功 steer 后再次作为下一轮执行。
3. `/q queue-explicit` 不进入 `turn/steer`；当前 turn 结束后只执行一次排队消息，文本和顺序保持不变。
4. runner 拒绝 Guide、控制通道失败或不支持 live guide 时，原消息明确降级排队，队列中仍保留完整文本。
5. Agent Chat WebUI 在 Codex/Traex/Claude Code 与自定义 runner 运行中默认选中 Guide，发送 `/g <消息>`；切换 Queue 后发送 `/q <消息>`，控制回执不会错误结束主 turn 或被陈旧 thread summary 覆盖。

### TC-IEC-59: 同一 session 替代运行不会被旧 run 清理引导通道

操作步骤：

1. 执行 app-server 会话所有权回归：
   ```bash
   SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin \
     'im_gateway::external_cli::app_server::tests::stale_app_server_cleanup_preserves_replacement_session_owner' \
     --lib -- --nocapture
   SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin \
     'im_gateway::external_cli::app_server::tests::app_server_registration_rejects_stale_run_ownership' \
     --lib -- --nocapture
   SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin \
     'im_gateway::external_cli::tests::stale_run_session_cleanup_preserves_replacement_owner' \
     --lib -- --nocapture
   ```
2. 复跑真实临时服务 live-guide E2E：
   ```bash
   SKIP_BUILD=true BIFROST_BIN=target/debug/bifrost \
     e2e-tests/tests/test_external_runner_live_guide.sh
   ```

预期结果：

1. 同一 session 的新 run 注册后，旧 run 的 cleanup 只能按自身 `run_id` 条件删除，不能移除新 run 的 app-server handle。
2. 已失去 session 所有权的旧 run 不能晚到注册 handle。
3. `ACTIVE_SESSIONS` 的旧 owner 清理不能删除同 session 的替代 owner。
4. Codex/Traex 当前 turn 的真实 guide、拒绝和 exec queue fallback 链路继续通过。

### TC-IEC-60: Guide 控制通道饱和时有界降级

操作步骤：

1. 执行 worker control channel 饱和回归：
   ```bash
   SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin \
     worker_guide_rejects_saturated_control_channel_without_waiting \
     --lib -- --nocapture
   ```
2. 复跑 Guide 失败降级不丢消息回归：
   ```bash
   SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin \
     guide_stream_falls_back_to_queue_without_losing_message \
     --lib -- --nocapture
   ```

预期结果：

1. worker control channel 最多缓存 32 条控制请求，单 run 最多保留 32 条未确认 Guide。
2. 通道已满时新 Guide 立即返回 `too many pending guide requests`，不等待 20 秒 ACK timeout，也不继续增长内存。
3. 上层收到该错误后把完整原消息降级到 FIFO Queue；Stop 在饱和 channel 上仍有直接终止 worker 的兜底。

### TC-IEC-61: Codex app-server 容量错误有限自动重试

操作步骤：

1. 执行容量错误分类与副作用门禁单测：
   ```bash
   SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin \
     capacity_retry_classification_is_strict_and_side_effect_aware \
     --lib -- --nocapture
   ```
2. 执行同一 thread 容量失败后恢复单测：
   ```bash
   SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin \
     mock_app_server_retries_capacity_error_on_same_thread \
     --lib -- --nocapture
   ```
3. 使用当前源码二进制执行隔离黑盒 E2E：
   ```bash
   SKIP_BUILD=true BIFROST_BIN=target/debug/bifrost \
     bash e2e-tests/tests/test_im_gateway_codex_capacity_retry.sh
   ```

预期结果：

1. `codexErrorInfo=serverOverloaded` 在没有助手输出、工具调用和 pending guide 时，
   同一 thread 最多重试 3 次，退避为 1s、2s、4s。
2. 首次容量失败、第二次成功的场景只产生一个最终成功结果，不向调用方输出中间
   `run_failed`，metadata 为 `runner.capacityRetryCount=1`。
3. 持续容量失败总共启动 4 个 turn，最终保留原始容量错误并失败，metadata 为
   `runner.capacityRetryCount=3`。
4. 普通错误以及已经产生 assistant 输出后的容量错误都只启动一个 turn，不自动重试。
5. 测试只使用临时 `BIFROST_DATA_DIR` 和随机端口，不影响正式 9900 服务或真实 provider。

### TC-IEC-62: Claude Code stream-json LiveGuideChannel

操作步骤：

1. 执行 transport、帧格式与同进程 guide 单元回归：
   ```bash
   SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin external_cli --lib -- --nocapture
   ```
2. 通过真实 Bifrost 临时服务和 mock Claude CLI 执行 CLI/API 黑盒链路：
   ```bash
   SKIP_BUILD=true BIFROST_BIN="$PWD/target/debug/bifrost" \
     bash e2e-tests/tests/test_external_runner_live_guide.sh
   ```

预期结果：

1. Claude Code 无 custom args 时选择 `stream_json` transport；custom args（包括 `--input-format text`）与显式 `transport=exec` 继续使用一次性 exec 并在 guide 时诚实排队。
2. stream-json 首条 user frame 启动 turn；guide 先在同一 stdin 发送 Claude `interrupt` control request，收到匹配的成功 control response 后再发送 guide user frame。mock 的 start/interrupt/guide 记录 PID 相同且只出现一次 start。
3. `--replay-user-messages` 回显匹配 guide frame 后返回 `accepted=true`；interrupt 被拒绝、result 先到、回显超时、session 被替换或 stdin 关闭时返回 rejected，交由上层 queue fallback。
4. Claude `session_id` 映射为 `threadId`，没有虚构 `turnId`；assistant/tool/result 继续映射现有 progress events，下一轮仍使用既有 `--resume <session_id>`。

执行记录（2026-07-12）：PASS — `external_cli` focused suite 89 个测试全部通过（含 stream-json frame、同进程 guide、transport fallback 与既有 app-server 回归）；真实临时服务 E2E 输出 `[external-runner-live-guide] PASS`，Claude、Codex、Traex 三条 steer 链路以及 reject/exec/inactive fallback 全部通过。

### TC-IEC-63: Claude Code stream-json mock 不等待 stdin EOF 回归

操作步骤：

1. 确认当前分支已构建最新 Bifrost debug binary：
   ```bash
   SKIP_FRONTEND_BUILD=1 cargo build --bin bifrost
   ```
2. 使用真实 Bifrost 进程运行模型与思考等级 shell E2E：
   ```bash
   SKIP_BUILD=true \
   BIFROST_BIN="$PWD/target/debug/bifrost" \
   bash e2e-tests/tests/test_im_gateway_traex_model_slash.sh
   ```
3. 检查测试用 mock Claude Code 的输入处理：它只消费一条初始 user JSONL frame，随后输出 assistant 与 result frame；不等待 stdin EOF。

预期结果：

1. 脚本输出 `[im-gateway-model-slash] PASS` 并以 0 退出。
2. Claude Code 的 model 与 effort 两次运行均返回 `BIFROST_CLAUDE_MODEL_SLASH_OK`。
3. 不再出现 `stream-json runner timed out after 30 seconds`。
4. Codex 与 Traex 的既有 model/effort 断言继续通过。

### TC-IEC-64: Claude Code 显式 exec 保持文本 stdin 回归

操作步骤：

1. 运行命令参数单元回归：
   ```bash
   SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin \
     claude_code_explicit_exec_transport_keeps_text_stdin \
     --lib -- --nocapture
   ```
2. 使用真实 Bifrost 进程运行 external runner live-guide E2E：
   ```bash
   SKIP_BUILD=true BIFROST_BIN="$PWD/target/debug/bifrost" \
     bash e2e-tests/tests/test_external_runner_live_guide.sh
   ```
3. 检查 mock 记录的两次 `claude-exec` 进程参数和输出：首轮运行中发送 guide 后应诚实降级为 Queue，第二轮应只执行排队消息一次。

预期结果：

1. Claude Code 显式 `adapterConfig.transport=exec` 且没有 custom args 时生成 `--input-format text`，不包含 `--replay-user-messages`。
2. mock 两次都能从 stdin 读取完整原始 prompt 文本并返回 `EXEC_claude-exec`，不会进入 stream-json 解析分支或因非 JSON 输入失败。
3. guide 响应为 `delivery=queued` 且 reason 包含 `exec transport`；两轮 `run_finished` 都成功，排队消息没有丢失或重复。

### TC-IEC-65: Codex 可重试断流不提前终止且失败回复不泄露协议

操作步骤：

1. 从仓库根目录执行确定性的 mock app-server 回归：
   ```bash
   SKIP_BUILD=true BIFROST_BIN=target/debug/bifrost \
     e2e-tests/tests/test_im_gateway_codex_retryable_error.sh
   ```
2. 脚本使用动态非正式端口和临时数据目录启动当前源码构建的 Bifrost，不连接真实 IM Provider。
3. 第一轮 mock Codex 依次发送 `error(willRetry=true)`、assistant final、`turn/completed`；第二轮发送 `error(willRetry=false)`。

预期结果：

1. 第一轮状态为 `succeeded`，回复为 `BIFROST_RETRY_RECOVERED`；重连提示归一化为 `status`，不存在 `run_failed`。
2. 第二轮状态为 `failed`，用户可见回复精确为 `permanent request failure`。
3. 两轮回复都不以 app-server 初始化 JSON（如 `{"id":1`）开头；原始 JSON-RPC stdout 只保存在 run artifact 中。
4. 脚本输出 `[im-gateway-codex-retryable-error] PASS` 并清理其临时进程和数据。

## 最近执行记录

- 2026-07-13：合并微信默认排队/引用与 Daily Research 功能线后执行 TC-IEC-65。先以 `SKIP_FRONTEND_BUILD=1 cargo build --bin bifrost` 构建当前源码，再以隔离临时数据目录和动态端口运行 `SKIP_BUILD=true BIFROST_BIN=target/debug/bifrost bash e2e-tests/tests/test_im_gateway_codex_retryable_error.sh`，输出 `[im-gateway-codex-retryable-error] PASS`。可重试 error 继续同一 turn 至 `BIFROST_RETRY_RECOVERED`，不可重试 error 收敛为 `permanent request failure`，两条用户回复均未泄露 JSON-RPC 握手 stdout。

### TC-IEC-66: GPT Web 搜索来源 favicon 紧凑引用渲染回归

操作步骤：

1. 执行 GPT Web 引用结构单元回归：
   ```bash
   pnpm --dir web exec vitest run src/pages/AI/AgentChatSection.markdown.test.ts
   ```
2. 执行 Agent Chat 真实浏览器 E2E：
   ```bash
   pnpm --dir web exec playwright test tests/ui/agent-chat.spec.ts \
     -g "renders GPT Web source favicons as compact citations" --reporter=line
   ```
3. E2E 消息使用真实 GPT Web 结构：
   ```markdown
   [![](https://www.google.com/s2/favicons?domain=https://www.reuters.com&sz=128)Reuters+2![](https://www.google.com/s2/favicons?domain=https://apnews.com&sz=128)AP News+2](https://www.reuters.com/world/china/example?utm_source=chatgpt.com)
   ```
   同一条 assistant 消息另放一张普通 Markdown 正文图片。
4. 检查 DOM：来源引用只有一个外链、包含两个来源名称和两个 14px favicon；favicon 不带 `data-agent-chat-image-id`，普通正文图片仍带预览标识。
5. 点击普通正文图片，确认灯箱只有这一张图片；关闭灯箱后切换暗色主题，确认来源引用仍清晰可见。
6. 启动当前 worktree 的 Vite 前端并代理现有 9900 后端，在浏览器打开用户提供的真实 `session` / `historyPath`；如果旧 `view=chat` 参数未直接选中当前 Agent Chat 路由，则补齐当前 canonical 参数 `aiSection=agent-chat&agentSection=chat&view=active`。检查 Reuters/AP、新华网、卫报等来源引用均不再显示为正文大图。

预期结果：

1. GPT Web 的 Google favicon + 来源名称序列被识别为紧凑来源引用，图标位于对应来源文字前方，整组仍跳转到原文章 URL。
2. 来源 favicon 固定为 14px，不获得正文图片的 120px 最小宽度/90px 最小高度，也不进入图片灯箱计数。
3. 普通 Markdown 图片、图片附件、普通外链和带描述文字的链接图片维持原有渲染与灯箱行为。
4. 亮色和暗色主题均使用 Ant Design token，边框、背景、文字和 focus/hover 状态可读。

### TC-IEC-67: Linux app-server `ETXTBSY` 有界重试

操作步骤：

1. 执行注入瞬态 `ETXTBSY`、持续 `ETXTBSY` 与非瞬态 `NotFound` 的确定性回归，并验证缺失 executable 的用户错误：
   ```bash
   SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin \
     app_server_spawn_ --lib -- --nocapture
   ```
2. 在 Linux 执行故意保持 mock app-server 可执行文件写句柄、延迟释放的真实 spawn 回归：
   ```bash
   SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin \
     app_server_spawn_retries_linux_text_file_busy \
     --lib -- --nocapture
   ```
3. 执行触发原 CI 失败的 app-server Live Guide 单测：
   ```bash
   SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin \
     mock_app_server_accepts_live_guide_and_completes_same_turn \
     --lib -- --nocapture
   ```
4. 使用当前源码二进制执行隔离 app-server 黑盒链路：
   ```bash
   SKIP_BUILD=true BIFROST_BIN="$PWD/target/debug/bifrost" \
     bash e2e-tests/tests/test_external_runner_live_guide.sh
   ```
5. 推送后确认 PR 的 `Coverage (Unit + Proxy E2E) & 90% Gate` job 通过。

预期结果：

1. Unix `ETXTBSY` 前两次失败、第三次成功时返回成功；持续 `ETXTBSY` 时恰好尝试 8 次后返回；Linux 真实 spawn 在写句柄释放后同样成功。启动层以 5ms 线性递增退避，最多尝试 8 次、总等待不超过 140ms。
2. 非 `ETXTBSY` 错误只尝试一次；缺失 executable 的用户错误包含 adapter 与实际路径；成功 spawn 后不做第二次启动，避免重复 thread 或副作用。
3. Live Guide 与真实临时服务 E2E 保持通过，运行结束后不残留 active run/session。
4. coverage job 不再因该 mock app-server 测试的 `ETXTBSY` 瞬态失败而中断，并继续执行 90% gate。

### TC-IEC-67: 绑定 Session 的 WebUI 与飞书双向进展同步

操作步骤：

1. 执行绑定目标与主动卡片发送回归：
   ```bash
   SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin web_im_progress_target_ --lib -- --nocapture
   SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin group_binding_can_be_resolved --lib -- --nocapture
   SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin proactive_progress_card_ --lib -- --nocapture
   SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin \
     bound_web_turn_sends_direct_progress_card_and_final_output --lib -- --nocapture
   SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin \
     web_guide_queue_fallback_updates_bound_im_progress_card --lib -- --nocapture
   ```
2. 构建当前源码二进制并执行跨通道 Live Guide E2E：
   ```bash
   SKIP_FRONTEND_BUILD=1 cargo build --bin bifrost
   SKIP_BUILD=true BIFROST_BIN="$PWD/target/debug/bifrost" \
     bash e2e-tests/tests/test_external_runner_live_guide.sh
   ```
3. 检查 focused 测试的 mock Feishu HTTP 请求记录：首个消息路径必须为 `/open-apis/im/v1/messages`，接收目标为绑定的 `chat_id/open_id`，最终 CardKit update payload 包含 WebUI turn 的最终输出。
4. 检查 Live Guide E2E 的 `codex-cross-channel` 场景：Web stream 先启动 canonical 私聊 Session，随后 mock IM inbound 发送 `guide-from-im-to-web`，同一个 stream 返回 `GUIDED_codex-cross-channel`。

如当前测试账号有不会影响其他成员的专用飞书群，可补充真实飞书观察；这不是自动回归的前置条件，禁止为测试向生产群或真实用户发送未获授权的消息。

预期结果：

1. 群聊 Session 只从持久化 `im_group_bindings` 解析原 `provider_id + chat_id`；机器人私聊只解析启用 Provider 的 canonical `provider_id:owner_open_id`，伪造或无绑定 Session 不产生 IM 目标。
2. WebUI 启动的绑定 Session 在飞书直接发送一张进度卡，接口路径不是 `/<message_id>/reply`，payload 包含正确 `receive_id`，因没有飞书源消息而不携带引用消息 ID。
3. WebUI 与飞书看到同一批运行进展和最终回复；飞书在 WebUI 启动的活跃 turn 中继续发送消息时，支持 steer 的 Runner 在同一 thread/turn 接收 Guide，失败或不支持时按原 Session 排队。
4. 纯 WebUI Session 保持 WebUI-only；`deliveryMode=no_im` 仍禁止跨通道发送。

### TC-IEC-68: Desktop Service 精简 PATH 下默认 Traex Runner 可启动

前置条件：

1. 当前源码已构建为 `target/debug/bifrost`。
2. 不停止、不替换用户当前 9900 Service；测试使用临时数据目录和动态端口。

操作步骤：

1. 执行确定性 Desktop PATH 回归：
   ```bash
   SKIP_BUILD=true BIFROST_BIN="$PWD/target/debug/bifrost" \
     bash e2e-tests/tests/test_im_gateway_desktop_path_traex.sh
   ```
2. 脚本在临时 `HOME/.local/bin/traex` 创建 mock Traex app-server，以 `PATH=/usr/bin:/bin` 启动隔离 Bifrost Service，并配置不含 `adapterConfig.executable` 与 `adapterConfig.env.PATH` 的默认 Traex Runner。
3. 检查 Chat Gateway stream、run detail 和 mock 进程记录。
4. 可选真实链路补充：
   ```bash
   RUN_REAL_TRAEX_E2E=1 SKIP_BUILD=true \
     BIFROST_BIN="$PWD/target/debug/bifrost" \
     bash e2e-tests/tests/test_im_gateway_traex_runner_streaming.sh
   ```
   若真实模型响应超时，必须检查隔离 Service 是否已成功创建 `traex app-server --listen stdio://` 子进程，并把模型超时与 executable spawn 失败分开归因。

预期结果：

1. 确定性脚本输出 `[im-gateway-desktop-path-traex] PASS`。
2. run snapshot 的 executable 为裸 `traex`，参数前三项为 `app-server --listen stdio://`，配置 env keys 不含 `PATH`。
3. mock 进程实际 PATH 包含临时 `HOME/.local/bin`，版本探测返回 `traex 0.0.0-desktop-path-mock`，最终响应为 `BIFROST_DESKTOP_PATH_TRAEX_OK`。
4. 测试结束后临时 Service、mock 进程和临时目录全部清理，用户现有 CLI-owned 或 Desktop-owned Service 均不受影响。

### TC-IEC-69: 空指令原样透传与 Base 首条生命周期

前置条件：

1. 当前源码已构建为 `target/debug/bifrost`。
2. 测试只使用临时数据目录、动态端口、mock Feishu Provider 和捕获 stdin 的 mock Runner，不停止或替换用户当前 Bifrost Service。

操作步骤：

1. 执行真实隔离链路：
   ```bash
   SKIP_BUILD=true BIFROST_BIN="$PWD/target/debug/bifrost" \
     bash e2e-tests/tests/test_im_gateway_prompt_passthrough.sh
   ```
2. 脚本先把 Agent Base / Developer / User Instructions 与 Runner Instructions 全部保存为空，同时故意保留旧配置 `injectBifrostTools:true`，再通过 mock IM inbound 发送 `原样透传消息`。
3. 脚本随后配置 `BASE_INSTRUCTION`、`DEVELOPER_INSTRUCTION`、`USER_INSTRUCTION`、`RUNNER_INSTRUCTION`，发送 `/clear` 后连续发送 `首条分层消息` 与 `后续分层消息`。
4. 检查 mock Runner 捕获的三条 stdin JSONL，逐字比较 prompt。
5. 执行 focused 单元回归：
   ```bash
   cargo test -p bifrost-admin compose_message_instructions
   cargo test -p bifrost-admin build_prompt_does_not_inject_legacy_bifrost_tool_context
   cargo test -p bifrost-admin chat_gateway_agent_instructions_include_base_only_on_first_message
   ```
6. 执行 Settings focused UI 回归：
   ```bash
   pnpm --dir web exec playwright test tests/ui/admin-settings.spec.ts \
     -g "Settings Agent 三层 instructions|Agent Runners 新增弹窗" \
     --reporter=line
   ```

预期结果：

1. 隔离脚本输出 `[im-prompt-passthrough] PASS`，并清理临时 Service 与数据目录。
2. 第一条 stdin 精确等于 `原样透传消息\n`，不存在 `Bifrost Tool Context` 或其他 Bifrost 自动前缀。
3. `/clear` 后首条 stdin 精确按 `Base -> Developer -> User -> Runner -> 原消息` 排列。
4. 同会话第二条 stdin 不再包含 Base，但 Developer / User / Runner 仍按消息传入。
5. Settings 不再展示 `Inject Bifrost Tools`，并明确说明 Base 只用于新会话首条、其他三层是消息级；空值不增加内容。
6. focused Rust 与 Playwright 回归全部通过。

### TC-IEC-70: IM Codex Fast 模式按 Session 切换

前置条件：

1. 当前源码已构建为 `target/debug/bifrost`。
2. 测试只使用临时数据目录、动态端口、mock Feishu Provider 和 mock Codex/Traex Runner，不停止或替换用户当前 Bifrost Service。

操作步骤：

1. 执行真实隔离链路：
   ```bash
   SKIP_BUILD=true BIFROST_BIN="$PWD/target/debug/bifrost" \
     bash e2e-tests/tests/test_im_gateway_codex_fast_slash.sh
   ```
2. mock IM 对 Codex Runner 依次发送 `/fast off`、普通消息、裸 `/fast`、`/fast status` 和第二条普通消息；随后启动慢任务，在任务运行中发送 `/fast off` 并用 `/q` 排队下一条消息。
3. 检查 `session_state.json`、mock Codex argv 和 IM outbound message log。
4. mock IM 对 Traex Runner 发送 `/fast off` 与 `/fast invalid`，检查两条 outbound message log 和 Traex argv。
5. 执行 focused 单元回归：
   ```bash
   cargo test -p bifrost-admin fast_slash -- --nocapture
   cargo test -p bifrost-admin codex_session_fast_override_replaces_runner_service_tier -- --nocapture
   cargo test -p bifrost-admin service_tier_resolution_uses_last_runner_override_then_codex_default -- --nocapture
   cargo test -p bifrost-admin im_help_for_codex_runner_lists_fast_command -- --nocapture
   cargo test -p bifrost-admin slash_classification_matches_direct_message_command_boundaries -- --nocapture
   ```

预期结果：

1. 隔离脚本输出 `[im-codex-fast] PASS`，并清理临时 Service 与数据目录。
2. `/fast off` 和裸 `/fast` 本身不执行 Codex；随后两条普通消息的 Runner argv 分别只包含一个 `service_tier="default"` 和 `service_tier="fast"`。
3. `/fast status` 回复当前使用快速模式；运行中 `/fast off` 不进入 live guide，排队消息读取最新 session 状态并使用 `service_tier="default"`；session 最终持久化 `serviceTierOverride:"default"` 与 `serviceTierOverrideSource:"session slash command"`。
4. Traex 收到合法 `/fast off` 或非法参数 `/fast invalid` 后都优先明确回复“当前 Runner 不支持 `/fast` 命令”，且 Traex Runner 不执行。
5. 合法和非法 `/fast` 在群聊与忙碌链路均作为系统命令分类，不进入普通 prompt 或 live guide。

## 最近执行记录

- 2026-07-31：PASS — 新增并立即执行 TC-IEC-70。`SKIP_BUILD=true BIFROST_BIN="$PWD/target/debug/bifrost" bash e2e-tests/tests/test_im_gateway_codex_fast_slash.sh` 输出 `[im-codex-fast] PASS`；真实隔离 IM mock-inbound 链路确认 `/fast off` 与裸 `/fast` 不执行 Runner，随后两次 Codex `exec` 分别只携带 `service_tier="default"` 与 `service_tier="fast"`，`/fast status` 回复当前快速模式；扩展忙碌链路后，当前任务仍使用切换前的 `fast`，运行中 `/fast off` 不进入 stdin/live guide，`/q` 排队的下一轮读取最新 session 并只携带 `service_tier="default"`，session 最终持久化 `serviceTierOverride:"default"` 和来源 `session slash command`；Traex 收到合法 `/fast off` 与非法参数 `/fast invalid` 都优先明确返回不支持且未执行。五条 focused Rust 回归逐条通过，覆盖 parser、session 覆盖 Runner 静态配置、Codex 默认 tier、帮助文案和群聊 slash 系统命令分类。E2E 首轮仅因断言把 mock Runner 的 `--version` 探测计作业务运行而失败，修正为只统计 `exec`；扩展非支持 Runner 双消息后又发现最终断言早于第二条异步 outbound log，改为等待计数达到 2 后完整复跑通过。
- 2026-07-30：PASS — 新增并立即执行 TC-IEC-69。`SKIP_BUILD=true BIFROST_BIN="$PWD/target/debug/bifrost" bash e2e-tests/tests/test_im_gateway_prompt_passthrough.sh` 输出 `[im-prompt-passthrough] PASS`；mock IM + mock Runner 捕获的三条 stdin 分别精确为原始消息、首条 `Base -> Developer -> User -> Runner -> 消息`、后续 `Developer -> User -> Runner -> 消息`，遗留 `injectBifrostTools:true` 未生成 `Bifrost Tool Context`。三组 focused Rust 回归分别通过 `2/2`、`1/1`、`1/1`。首次 Settings Playwright 暴露旧用例仍把只读 `default_base_instructions` 当成已配置 Base，已按“空值不注入”语义修正为显式填写后才保存；完整重跑 `Settings Agent 三层 instructions|Agent Runners 新增弹窗` 通过 `2/2`，确认遗留开关不再显示且生命周期说明可见。
- 2026-07-25：PASS — 新增并立即执行 TC-IEC-68。先构建当前源码二进制，再执行 `SKIP_BUILD=true BIFROST_BIN="$PWD/target/debug/bifrost" bash e2e-tests/tests/test_im_gateway_desktop_path_traex.sh`，输出 `[im-gateway-desktop-path-traex] PASS`；隔离 Service 以 `PATH=/usr/bin:/bin`、临时 HOME 和动态端口启动，Runner 配置未设置 executable/PATH，仍从临时 `HOME/.local/bin/traex` 启动 `app-server --listen stdio://`，run snapshot executable 为裸 `traex`，版本为 `traex 0.0.0-desktop-path-mock`，最终响应为 `BIFROST_DESKTOP_PATH_TRAEX_OK`。补充真实 Traex 用例时，隔离 Service 成功创建 `traex app-server --listen stdio://` 子进程 PID `26233`，确认原 `No such file or directory` 不再出现；外部模型在 240 秒内未完成而由 Python HTTP 客户端超时，按外部模型延迟记录，不视为 PATH 回归失败。测试未停止或替换用户 9900 Service。
- 2026-07-24：PASS — 新增并立即执行 TC-IEC-67。绑定解析 focused 单测 `2/2 + 1/1` 通过，主动卡片、完整 Web→IM bridge、Web Guide 降级排队后刷新绑定卡片三项单测各 `1/1` 通过；mock Feishu 记录首个消息路径为 `/open-apis/im/v1/messages`、`receive_id=ou_owner`，没有 reply message 路径，最终 CardKit update 包含 `final from WebUI`，Guide 降级时卡片包含 `WebUI 引导无法实时注入` 和原引导摘要。重新构建当前源码二进制后执行 `SKIP_BUILD=true BIFROST_BIN="$PWD/target/debug/bifrost" bash e2e-tests/tests/test_external_runner_live_guide.sh`，输出 `[external-runner-live-guide] PASS`；新增 `codex-cross-channel` 场景由 Web stream 启动 `cross-channel-provider:cross-channel-owner`，随后 mock IM inbound 的 `guide-from-im-to-web` 在同一 active turn 被 steer，Web stream 最终返回 `GUIDED_codex-cross-channel`。未向真实生产群或用户发送测试消息。
- 2026-07-24：PASS — 复跑 TC-IEC-48 与同脚本图片/runner-call 回归。先执行 `SKIP_FRONTEND_BUILD=1 cargo build --bin bifrost` 构建当前源码二进制，再执行 `SKIP_BUILD=true BIFROST_BIN="$PWD/target/debug/bifrost" bash e2e-tests/tests/test_im_gateway_external_runner_image_input.sh`；脚本使用隔离临时数据目录和 mock external runner 注入纯文件消息，debug inbound 接受空正文文件消息并生成 run `1784881047460-3b7fa88e-5eb4-456d-a636-93a1a2c72a87`。验证 `prompt.md` 和 runner stdin 均包含 `## Attached Files` 及本地文件路径，附件落盘为 `.bifrost-e2e-runner-image.Phu6KP/agent/sessions/by-key/attachments/session-7c3700696145216f4803a299a940daf92873f6b8ad58099d11991c3169bf44d9/1784881047460-3b7fa88e-5eb4-456d-a636-93a1a2c72a87/files/1-report_final.md`，文件内容与注入的 markdown base64 一致，metadata 记录 `attachments.fileCount=1`、`attachments.imageCount=0`、`attachments.count=1` 和 `attachments.files[0]` 的 path/mimeType/sizeBytes/name。
- 2026-07-24：PASS — 新增并立即执行 TC-IEC-48。先以 `SKIP_FRONTEND_BUILD=1 cargo build --bin bifrost` 构建当前源码二进制，再执行 `SKIP_BUILD=true BIFROST_BIN="$PWD/target/debug/bifrost" bash e2e-tests/tests/test_im_gateway_external_runner_image_input.sh`；脚本使用隔离临时数据目录和 mock external runner 注入纯文件消息，debug inbound 接受空正文文件消息并生成 run `1784873185896-cf83955e-4ac4-4a3e-b95f-9782f807947b`。验证 `prompt.md` 和 runner stdin 均包含 `## Attached Files` 及本地文件路径，附件落盘为 `.bifrost-e2e-runner-image.qLXnna/agent/sessions/by-key/attachments/session-7c3700696145216f4803a299a940daf92873f6b8ad58099d11991c3169bf44d9/1784873185896-cf83955e-4ac4-4a3e-b95f-9782f807947b/files/1-report_final.md`，文件内容与注入的 markdown base64 一致，metadata 记录 `attachments.fileCount=1`、`attachments.imageCount=0`、`attachments.count=1` 和 `attachments.files[0]` 的 path/mimeType/sizeBytes/name。
- 2026-07-21：PASS — 按更新后的 TC-IEC-43/44 执行 `SKIP_BUILD=true BIFROST_BIN=target/debug/bifrost bash e2e-tests/tests/test_im_gateway_external_runner_image_input.sh`；同 key 两轮 Web Chat、Trae 及 runner-call 图片均写入 `agent/sessions/by-key/attachments/session-{sha256}/<runId>/images/`，不同 run 目录隔离且旧图片未被覆盖。
- 2026-07-15：针对 PR #394 coverage job 的 Linux `Text file busy (os error 26)` 新增并立即执行 TC-IEC-66。注入回归验证 Unix `ETXTBSY` 前两次失败后第三次成功、持续占用恰好尝试 8 次后返回、`NotFound` 仅尝试一次，并验证缺失 executable 的用户错误保留 adapter 与实际路径；本机 macOS 按平台预期过滤 Linux 真实 spawn 用例，原失败用例 `mock_app_server_accepts_live_guide_and_completes_same_turn` 输出 `1 passed`；重新构建当前源码二进制后，`SKIP_BUILD=true BIFROST_BIN="$PWD/target/debug/bifrost" bash e2e-tests/tests/test_external_runner_live_guide.sh` 输出 `[external-runner-live-guide] PASS`。Linux 持有写句柄的真实回归与 95% changed-lines / 90% workspace coverage gate 由 PR CI 在目标平台执行并作为本用例最终门禁。
- 2026-07-15：新增并立即执行 TC-IEC-66。引用结构单元回归 `6/6` 通过，覆盖双来源、单来源、非法/混合节点、普通图片，以及独立/描述性 favicon 仍可预览；Playwright focused E2E `1/1` 通过，确认双来源 favicon 均为 `14px × 14px`、无图片预览 ID，普通正文图片仍是唯一灯箱图片，暗色主题可见。随后以 `BACKEND_PORT=9900 WEB_PORT=3015 pnpm --dir web exec vite --host 127.0.0.1 --port 3015` 代理现有正式后端，在 Codex 内置浏览器打开用户真实 session/history；当前 canonical Agent Chat 参数下共识别 14 组来源引用、15 个 favicon，首组 `Reuters+2AP News+2` 高度 24px、两个图标均为 `14px × 14px`，页面没有正文预览图片。暗色主题下引用背景为 `rgba(255, 255, 255, 0.04)`、边框为 `rgb(48, 48, 48)`，组件持续可见。未停止或替换 9900 服务，未修改系统代理。
- 2026-07-13：针对 PR #377 自动 Review 的 Claude 显式 exec stdin 兼容问题新增并立即执行 TC-IEC-64。单元回归输出 `1 passed`；真实临时 Bifrost E2E 输出 `[external-runner-live-guide] PASS`。mock 记录两次 `claude-exec` 均带 `--input-format text` 且不含 `--replay-user-messages`，首轮 guide 返回 `delivery=queued`，两轮均返回 `EXEC_claude-exec`。
- 2026-07-13：针对 PR #377 Linux `E2E Shell` 失败新增并立即执行 TC-IEC-63。修复前同一命令稳定在 Python 第 246 行收到 `stream-json runner timed out after 30 seconds`；修复 mock Claude 从等待 stdin EOF 改为消费一条初始 user JSONL frame 后，`SKIP_BUILD=true BIFROST_BIN=target/debug/bifrost bash e2e-tests/tests/test_im_gateway_traex_model_slash.sh` 输出 `[im-gateway-model-slash] PASS`。Codex、Traex、Claude Code 的 model 与 effort 六次真实临时服务运行全部通过，Claude run 为 `1783925191553-f1ad8e07-13b0-44c3-b237-c05cedac9305`、effort run 为 `1783925198233-c9920e47-70ea-4976-bbae-d38932762e75`，无 30 秒超时。
- 2026-07-12：新增并立即执行 TC-IEC-61。两个 focused Rust 单测分别输出 `1 passed`；`SKIP_BUILD=true BIFROST_BIN=target/debug/bifrost bash e2e-tests/tests/test_im_gateway_codex_capacity_retry.sh` 输出 `[codex-capacity-retry] PASS`。隔离 mock app-server 验证首次 `serverOverloaded` 后在同一 thread 第二次成功、持续容量错误 3 次重试后失败、普通错误不重试、已有 assistant delta 后不重试；成功 run 未输出中间 `run_failed`，并持久化 `runner.capacityRetryCount=1`。
- 2026-07-10：第 4 轮性能 review 新增并立即执行 TC-IEC-60。`worker_guide_rejects_saturated_control_channel_without_waiting` 与 `guide_stream_falls_back_to_queue_without_losing_message` 均输出 `1 passed`；同时复跑 `external_worker_`，陈旧 channel 不误杀 PID 与已 ACK Stop 不追加 kill 两项均通过。确认控制通道饱和时 Guide 快速返回上限错误，上层仍保留完整消息进入 Queue，且 Stop 的 PID reuse 安全边界未被有界通道改造破坏。
- 2026-07-10：第 3 轮全面 review 新增并立即执行 TC-IEC-59。`stale_app_server_cleanup_preserves_replacement_session_owner`、`app_server_registration_rejects_stale_run_ownership` 与 `stale_run_session_cleanup_preserves_replacement_owner` 均通过，app-server 模块合计 `9 passed`；确认旧 run cleanup 按 `run_id` 条件删除、失去所有权的 run 不能晚到注册，且 `ACTIVE_SESSIONS` 的旧 owner 不能删除替代 owner。随后用当前源码重新构建 `target/debug/bifrost`，执行 `SKIP_BUILD=true BIFROST_BIN=target/debug/bifrost e2e-tests/tests/test_external_runner_live_guide.sh` 输出 `[external-runner-live-guide] PASS`。
- 2026-07-10：新增并立即执行 TC-IEC-58。三个 focused Rust 命令分别通过 `10/10`、`2/2`、`1/1`；`SKIP_BUILD=true BIFROST_BIN=target/debug/bifrost e2e-tests/tests/test_external_runner_live_guide.sh` 通过，真实临时服务验证 Codex/Traex Web steer、飞书 IM 普通消息默认 steer、`/q queue-explicit` 只在当前 turn 完成后执行一次、`/g release-queue` 注入当前 turn、拒绝/exec transport 明确降级队列且原消息不丢失；Playwright 三组运行中输入用例 `3/3` 通过，覆盖非 ChatGPT Web runner 默认 Guide、显式 Queue、ChatGPT Web 仅 Queue，以及外部 Runner Stop/Guide/Queue/移除队列回归。定位并修复了 IM handler 等待 Guide ACK 时暂停 runner control future 导致 20 秒自锁、成功 steer 后又错误排队重复执行的问题。
- 2026-07-10：第 2 轮 review 后执行真实本机 CLI 兼容验证。`RUN_REAL_CODEX_E2E=1 SKIP_BUILD=true BIFROST_BIN=target/debug/bifrost BIFROST_CODEX_BIN=$(command -v codex) e2e-tests/tests/test_im_gateway_codex_runner_streaming.sh` 通过，run `1783652710170-bb55ada8-78f5-42a3-9c21-8f695aa8dbfc` 使用 Codex stdio app-server 并持久化真实 thread/token usage；`RUN_REAL_TRAEX_E2E=1 SKIP_BUILD=true BIFROST_BIN=target/debug/bifrost BIFROST_TRAEX_BIN=$(command -v traex) e2e-tests/tests/test_im_gateway_traex_runner_streaming.sh` 通过，run `1783652721440-28607fc3-acd3-46a4-b79e-a67aed22ef06` 验证当前 Traex 需要 `app-server --listen stdio://` 且真实返回最终答案、thread/token usage。Codex 真实模型本轮没有执行提示要求的 `pwd` 工具，因此真实脚本只在模型实际产生 tool event 时断言 started/finished 成对与顺序；无条件工具事件映射由 deterministic mock app-server E2E 断言。
- 2026-07-10：新增并立即执行 TC-IEC-57；第 2 轮 review 扩展拒绝与 exec 回退后再次执行。命令 `SKIP_BUILD=true BIFROST_BIN=target/debug/bifrost e2e-tests/tests/test_external_runner_live_guide.sh` 通过；脚本用隔离数据目录启动真实 Bifrost 服务与 mock JSON-RPC/exec 进程，Codex/Traex 的 `bifrost agent guide` 都返回 `delivery=steered`，mock 收到带正确 `expectedTurnId`、唯一 `clientUserMessageId` 和 guide text 的 `turn/steer`，原 stream 在同一 turn 返回 `GUIDED_codex` / `GUIDED_traex`。run detail 断言 Codex snapshot 为 `app-server --stdio`、Traex 为 `app-server --listen stdio://`，metadata 持久化 thread id 与 `11/7/18` input/output/total token；app-server 明确拒绝 steer 和显式 `transport=exec` 两种场景都返回 `delivery=queued` 并在首轮后执行第二轮；不存在的 session 返回 HTTP 409 `delivery=rejected`，脚本清理临时服务和数据。
- 2026-07-08：新增并执行 TC-IEC-56 的本地回归。执行 `SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin test_normalize_feishu_post_extracts_text_and_images --lib -- --nocapture`、`external_cli_run_writes_image_attachments_and_injects_prompt_paths`、`im_event_loop_external_cli_route_processes_image_only_message`、`test_queue_preserves_image_attachments`、`external_cli_images_from_chat_images_preserves_payloads` 均通过；执行 `SKIP_BUILD=true BIFROST_BIN=target/debug/bifrost e2e-tests/tests/test_im_gateway_external_runner_image_input.sh` 通过，临时服务端口 `65211`，首轮 run `1783470749961-9783d1b6-20b8-4561-bd6e-fe7a486cec7b` 同时写入 `image-1.png` 与 `image-2.jpg`，后续单图 run `1783470750022-a7838ec2-5729-4fc0-8770-27c13c784918` 未覆盖首轮图片，Traex 兼容 run `1783470750145-e362365a-a1b6-4a44-bd8d-ea7b8f49f7ea` 与 runner-call run `1783470750275-68429b1e-41eb-45d4-b830-5b6b02623dcd` 均保留图片附件。
- 2026-06-26：追加执行 TC-IEC-55 的真实 Web UI 即时切模型回归。修复前在未刷新页面直接发送 `/model` 会先插入 `content:""` 的 system 占位，DOM 中出现空白系统胶囊，并且系统提示是有边框/背景的大号气泡。修复后执行 `pnpm --dir web run build`、`pnpm --dir web run test:unit -- AgentChatSection --run`、`cargo build --bin bifrost` 均通过；覆盖重启 `9900`，PID `87418`，启动命令包含 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`、`BIFROST_DISABLE_TRAY=1`、`--no-system-proxy`、`--no-tray`、`--no-intercept`。真实浏览器打开 `admin-chat-direct-system-check-1782409300`，先刷新确认新 bundle 生效，再发送 `/model GPT-5.5`：DOM 返回 `emptySystemCount=0`，最后一条 system 文本为 `切换模型为 GPT-5.5`，`agent-chat-message-bubble-system` 样式为 `borderWidth:0px`、`borderStyle:none`、`backgroundColor:rgba(0, 0, 0, 0)`、`fontSize:11px`、`lineHeight:18px`、`flexDirection:row`、`gap:8px`。刷新后再次确认 `emptySystemCount=0`、`hasLatestSwitch=true`、普通对话文本仍存在，HUD 显示 `Model GPT-5.5 (trae)`。
- 2026-06-26：执行 TC-IEC-55 的真实 9900 回归。先确认问题根因：`session_state.json` 中 `admin-chat-direct-system-check-1782409300::traex::Traex` 已持久化多条 `role:"system"` 模型切换消息，但 session detail 主路径先从 canonical JSONL 读到 user/assistant timeline 后，只合并 external runner metadata，没有把 external state 的 system display messages 合并回 `messages`，导致刷新后系统提示丢失。修复后执行 `cargo test -p bifrost-admin handlers::im_gateway::agent_api::tests::session_detail_metadata_merge_preserves_external_system_display_messages -- --nocapture` 通过；执行 `cargo build --bin bifrost` 后覆盖重启 `9900`，PID `46889`，启动命令包含 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`、`BIFROST_DISABLE_TRAY=1`、`--no-system-proxy`、`--no-tray`、`--no-intercept`。API 验证 `/_bifrost/api/im-gateway/agent/sessions/admin-chat-direct-system-check-1782409300` 返回 `message_count=11`，包含 5 条 `role:"system"` 的 `切换模型为 ...` 消息，同时保留 `你好`、`你是谁` 等 user/assistant 对话。真实 Edge/Playwright 打开同一 URL 并刷新，DOM 中 `agent-chat-message-system` 数量为 5，页面文本同时包含 `切换模型为 Kimi-K2.6`、`切换模型为 GPT-5.5`、`你好`、`你是谁`，HUD 显示 `Model Kimi-K2.6 (trae)`。
- 2026-06-26：追加执行 TC-IEC-50 的真实 9900 slash 键盘回归。先发现 `/model` 后按 Tab 会错误选中 `/models` 并提交，输入框被清空且后端返回 `runner 'codex' is not enabled`；修复后重新执行 `pnpm --dir web build` 与 `cargo build --bin bifrost`，覆盖重启 9900，PID `83008`，启动命令包含 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`、`BIFROST_DISABLE_TRAY=1`、`--no-system-proxy`、`--no-tray`、`--no-intercept`。Playwright 打开 `/_bifrost/ai?aiSection=agent-chat&agentSection=chat&session=codex-ui-slash-smoke-1782430796490&view=active`：输入 `/mo` 后按 Enter，页面发送 `/models` 并收到后端响应；再次输入 `/model` 后按 Tab，输入框值保持为 `/model `，焦点仍在 `agent-chat-input`，未发送请求。截图保存为 `/tmp/bifrost-9900-slash-smoke-fixed.png`。同时通过真实 `/chat/stream` API 验证 Traex `/models` 返回 Traex 模型列表、Traex 非法 `/model definitely-not-a-real-model-for-smoke` 返回“未切换模型”、Traex `/model Kimi-K2.6` 在 session detail 中写入 `messages[0].role="system"`、`content="切换模型为 Kimi-K2.6"` 与 `metadata.modelOverride="Kimi-K2.6"`；Codex `/models` 返回 Codex 模型列表，Codex 非法 `/model definitely-not-a-real-codex-model-for-smoke` 返回“未切换模型”。
- 2026-06-26：执行 TC-IEC-54 的 Feishu progress card mock 回归。命令 `SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin start_feishu_after_finished_card_recovers_from_invalid_card_id_send --lib -- --nocapture` 通过；mock 服务先发送第一轮 `card_1/om_1` 并 finish，第二轮第一次 `send card entity` 返回 `code=230099`、`cardid is invalid`，实现重新创建 `card_3` 并发送成功，最终 handle 指向 `card_3/om_3`，请求计数为 `card_counter=3`、`message_counter=3`、`card_update_counter=1`、`settings_update_counter=1`、`recall_counter=0`。同时执行旧语义回归 `SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin queue_state_rollover_send_failure_keeps_previous_running_handle --lib -- --nocapture` 通过，确认非 `cardid is invalid` 的普通发送失败仍不切换 handle；执行 `SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin progress_card --lib -- --nocapture`，35 个 progress_card 相关测试全部通过。
- 2026-06-26：执行 TC-IEC-53 的真实 Web UI 回归。当前编译版本 `cargo build --bin bifrost` 后覆盖重启 `9900`，PID `23043`，启动命令包含 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`、`BIFROST_DISABLE_TRAY=1`、`--no-system-proxy`、`--no-tray`、`--no-intercept`。Playwright 打开 `http://127.0.0.1:9900/_bifrost/ai?aiSection=agent-chat&agentSection=chat&session=admin-chat-1782407491650&view=active`，在 `agent-chat-input` 粘贴 PNG 并发送 `图片气泡展示回归：这张测试图里有什么？`。发送后 DOM 中包含该文本的 `agent-chat-message-bubble-user` 同时包含 1 个 `agent-chat-previewable-image`，图片 `src` 前缀为 `data:image/png;base64,iVBORw0K`；刷新页面后同一文本气泡仍包含 1 张图片。session detail API 返回最后一条用户消息 `content_parts` 同时包含 text 和 image_url。截图保存为 `/tmp/bifrost-user-image-bubble-sent.png`、`/tmp/bifrost-user-image-bubble-reload.png`。
- 2026-06-26：执行 TC-IEC-52 的真实 Web UI 回归。当前编译版本 `cargo build --bin bifrost` 后覆盖重启 `9900`，PID `55951`，启动命令包含 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`、`BIFROST_DISABLE_TRAY=1`、`--no-system-proxy`、`--no-tray`、`--no-intercept`。Playwright 打开 `http://127.0.0.1:9900/_bifrost/ai?aiSection=agent-chat&agentSection=chat&session=admin-chat-1782407491650&view=active`，确认存在任务计划胶囊 `Step 2/4审查未提交 diff+3`。鼠标 hover 胶囊后浮层出现，DOM 同时存在 `agent-chat-plan-hover-bridge` 与 `agent-chat-plan-popover`；鼠标移动到 8px 透明桥接区域并停留 `260ms` 后，浮层仍存在；继续移动到浮层中心并停留 `500ms` 后，浮层仍存在且文本包含 `Task progress1/4 completed...`；鼠标移到页面左上角 `260ms` 后浮层关闭。随后在浮层内拖选第二条任务文本，`window.getSelection()` 返回 `未提交 diff`，且浮层仍存在。截图保存为 `/tmp/bifrost-plan-hover-popover-open.png`、`/tmp/bifrost-plan-hover-popover-held.png`、`/tmp/bifrost-plan-hover-text-selection.png`。
- 2026-06-26：执行 TC-IEC-51 的真实 Web UI 回归。当前编译版本 `cargo build --bin bifrost` 后覆盖重启 `9900`，PID `73886`，启动命令包含 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`、`BIFROST_DISABLE_TRAY=1`、`--no-system-proxy`、`--no-tray`、`--no-intercept`。API 确认 `admin-chat-1782407491650` 的 session detail 有 `message_count=112`、`timeline_event_count=208`。真实浏览器打开 `http://127.0.0.1:9900/_bifrost/ai?aiSection=agent-chat&agentSection=chat&session=admin-chat-1782407491650&view=active`：初始页面 `Load older=true`，同时可见 `你好`、`你是干啥的`、`图片里面说的啥？` 和最新回复，不再只剩最后一条；点击 `Load older` 后 textLength 从 `2681` 增至 `3053`，等待 2.5 秒仍保持 `3053` 且旧首轮 `你是谁` 可见；刷新后恢复默认窗口且 `Load older=true`。随后在同一 Web UI 发送 `请用一句话回复：列表稳定验证二`，发送立即、运行 5 秒、完成后和最终刷新四个阶段均保持多轮历史可见，且 `Load older=true`；最终回复 `列表稳定验证二应同时检查会话列表 fallback title...` 可见。截图保存为 `/tmp/bifrost-chat-window-fixed-initial.png`、`/tmp/bifrost-chat-window-fixed-after-load-older-wait.png`、`/tmp/bifrost-chat-window-fixed-after-send-immediate.png`、`/tmp/bifrost-chat-window-fixed-after-final-reload.png`。
- 2026-06-25：新增并执行 TC-IEC-50 的自动真实服务回归。命令 `SKIP_FRONTEND_BUILD=1 cargo build --bin bifrost` 后运行 `SKIP_BUILD=true BIFROST_BIN=target/debug/bifrost e2e-tests/tests/test_im_gateway_traex_model_slash.sh`；脚本启动临时 Bifrost、配置 mock `traecli`，真实调用 `/chat/stream` 发送 `/models`、`/model Doubao-Unit` 和普通消息。断言 `/models` 响应包含 `Doubao-Unit` 与 `fast` tier、不包含 `SHOULD_NOT_LEAK` 或 hidden model；普通 run 的 `runtime_snapshot.args` 包含 `--model Doubao-Unit`；`session_state.json` 写入 `modelOverride:"Doubao-Unit"` 和 `modelOverrideSource:"session slash command"`。随后执行真实 Web UI 验证：`pnpm --dir web run build` 与 `cargo build --bin bifrost` 均通过；临时服务端口 `65110`，数据目录 `/tmp/bifrost-traex-webui-models.mNgvpy`，当前 Runner 为 `Traex`；浏览器打开 `/_bifrost/ai?aiSection=agent-chat&agentSection=chat`，输入 `/` 后 slash 面板显示 `/models` 和 `/model`；点击 `/models` 后页面返回 `Doubao-UI`、`fast`、`visibility: list`，不包含 `WEB_UI_SHOULD_NOT_LEAK` 或 `hidden-ui`，且未展示“上下文正在自动压缩”；通过 Web UI 发送 `/model Doubao-UI` 后 `session_state.json` 写入 `modelOverride:"Doubao-UI"`；再发送普通消息 `hello after ui model switch`，页面返回 `BIFROST_TRAEX_WEB_UI_MODEL_OK`，run `1782399832370-576febaa-a224-4455-9d8c-13bf49bede39` 的 `runtime_snapshot.args` 包含 `--model Doubao-UI`。
- 2026-06-25：继续执行 TC-IEC-50 的回归。当前编译版本通过 `cargo build --bin bifrost` 后重启 `9900`，PID `69582`，启动命令包含 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`、`BIFROST_DISABLE_TRAY=1`、`--no-system-proxy`、`--no-tray`、`--no-intercept`。API 验证 `/_bifrost/api/im-gateway/agent/sessions/admin-chat-1782405107022` 与 `/sessions/all` 均返回 `model:"Kimi-K2.6"`、`model_provider:"trae"`、`total_tokens_used/tokens:103688`、`estimated_tokens:103590`。真实浏览器打开 `http://127.0.0.1:9900/_bifrost/ai?aiSection=agent-chat&agentSection=chat&session=admin-chat-1782405107022&view=active`，刷新后 HUD 显示 `Tokens 77.6K · Context 31% · Model Kimi-K2.6 (trae)`；发送 `请用一句话回复：HUD 检查` 后 1 秒内 HUD 未清空，仍显示同一模型与 token/context；运行完成后 API usage 更新为 `103688/103590`，再次刷新页面 HUD 显示 `Tokens 103.7K · Context 41.4% · Model Kimi-K2.6 (trae)`，消息列表保留本轮用户消息。
- 2026-06-25：重新执行自动 E2E。命令 `SKIP_BUILD=true BIFROST_BIN=target/debug/bifrost e2e-tests/tests/test_im_gateway_traex_model_slash.sh` 通过，临时服务端口 `52213`，Codex run `1782406247186-39e421b5-1d0c-4332-969e-9d9bde18dbcd`、Traex run `1782406250690-3a71f7d6-af55-4799-bad0-ffe912fae72f` 均验证下一轮启动参数包含目标 `--model`。
- 2026-06-25：执行 TC-IEC-50 的 Web UI system message 回归。当前编译版本重启 `9900`，PID `85396`，启动命令包含 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`、`BIFROST_DISABLE_TRAY=1`、`--no-system-proxy`、`--no-tray`、`--no-intercept`。先通过 `/_bifrost/api/im-gateway/chat/stream` 对隔离 Traex session `admin-chat-direct-system-check-1782409300` 发送 `/model Kimi-K2.6`，确认 session detail 返回 `messages[0].role="system"`、`content="切换模型为 Kimi-K2.6"`。随后用真实 Web UI 打开该 session，刷新后消息区仍显示独立系统行 `切换模型为 Kimi-K2.6`；再在 Web UI composer 发送 `/model GPT-5.5`，刷新后消息区显示两条独立系统行 `切换模型为 Kimi-K2.6` 和 `切换模型为 GPT-5.5`，detail 中两条消息均为 `role:"system"`，不会作为正式 user/assistant 消息进入 runner prompt。截图保存为 `/tmp/bifrost-webui-system-row-reload-1782409728674.png` 与 `/tmp/bifrost-webui-system-row-webui-gpt55-reload-1782409835380.png`。
- 2026-06-25：针对“刷新后系统信息丢失且应独立居中显示”再次执行 TC-IEC-50 回归。重新编译 `cargo build --bin bifrost` 并覆盖重启 `9900`，PID `8181`，启动命令包含 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`、`BIFROST_DISABLE_TRAY=1`、`--no-system-proxy`、`--no-tray`、`--no-intercept`。通过 API 对隔离 session `admin-chat-system-row-final-1782410162512` 发送 `/model Kimi-K2.6`，Web UI 再真实输入并发送 `/model GPT-5.5`；刷新页面后 DOM 中两条 `agent-chat-message-bubble-system` 分别为 `切换模型为 Kimi-K2.6` 和 `切换模型为 GPT-5.5`，两条 bubble 的中心与整行中心偏差均为 `0px`。session detail 返回两条 `role:"system"` 消息，且无 user/assistant slash 消息；当前模型为 `GPT-5.5`。截图保存为 `/tmp/bifrost-system-row-final-centered-after-reload.png`。
- 2026-06-25：执行 TC-IEC-49 的本地回归。`cargo test -p bifrost-admin codex_and_traex_model_config_resolves_user_defaults_and_overrides -- --nocapture` 通过，确认 Codex 从 `CODEX_HOME/config.toml`、Traex 从 `TRAE_HOME/traecli.toml` 读取默认模型和思考配置，runner 显式配置可覆盖默认值。`cargo test -p bifrost-admin progress_card --lib -- --nocapture` 通过 34 个用例，飞书 progress card 外部 runner 摘要包含模型来源、思考强度与思考摘要。`cargo test -p bifrost-admin online_notification --lib -- --nocapture` 通过 4 个用例，线上通知摘要包含 Model / Reasoning Effort / Reasoning Summary。`SKIP_BUILD=true BIFROST_BIN=target/debug/bifrost e2e-tests/tests/test_im_online_notification_runner_context.sh` 通过，真实临时 Bifrost + mock Feishu API 发送的 interactive card content 包含新增三行，未知值稳定展示 `N/A`。`pnpm --dir web exec vitest run src/pages/AI/AgentChatSection.timeline.test.ts` 通过 16 个用例，`pnpm --dir web exec playwright test tests/ui/agent-chat.spec.ts -g "keeps running history token HUD synced with live status" --reporter=line` 通过，真实浏览器打开 Agent Chat 状态弹窗后 Context 区域可见 `gpt-5.1-codex (codex config)` 与 `high / auto`。
- 2026-06-25：执行 TC-IEC-43/44/46 的自动真实服务回归。先编译当前源码 `cargo build --bin bifrost`，再运行 `SKIP_BUILD=true BIFROST_BIN=/Users/eden_studio/work/github/bifrost/target/debug/bifrost e2e-tests/tests/test_im_gateway_external_runner_image_input.sh`。脚本启动临时服务端口 `53049`，配置 `adapter=codex` 的 mock runner 和 `adapter=traex` 的 mock runner；普通 Web Chat 第一轮 run `1782381743199-a7e58df0-386b-412c-899c-3c9b7a1ad7d4` 图片路径为 `.../attachments/session-web-image-session-e2e-1782381743/1782381743199-a7e58df0-386b-412c-899c-3c9b7a1ad7d4/images/image-1.png`，同 session 第二轮 run `1782381743258-ce882dbb-b5df-4e94-8b8b-b13902fae270` 图片路径为 `.../1782381743258-ce882dbb-b5df-4e94-8b8b-b13902fae270/images/image-1.png`，断言第一轮字节仍为 `hello-image`。Traex-compatible run `1782381743368-d14c605e-391a-4b83-80fd-f2b1b779104f` 图片路径为 `.../attachments/session-web-image-session-traex-e2e-1782381743/1782381743368-d14c605e-391a-4b83-80fd-f2b1b779104f/images/image-1.png`；runner-call image-only run `1782381743490-e375c12b-f907-41ce-a5cf-8f66873787b9` 成功。脚本同时断言 Codex/Traex `result.json.metadata` 和 `/_bifrost/api/im-gateway/agent/sessions/<sessionKey>` 均包含 `cli.executable`、`cli.args`、`cli.version`、`runner.adapter`、prompt/attachment/io/timing/tool/resume/usage 字段，`normalized_events.jsonl` 的 tool completed raw event 包含 `durationMs`。
- 2026-06-25：执行 TC-IEC-46 的 9900 实服务 Web UI 回归。使用当前编译产物重启 `9900`，命令包含 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`、`BIFROST_DISABLE_TRAY=1`、`--no-system-proxy`、`--no-tray`、`--no-intercept`，服务 PID `89308`。通过 `/_bifrost/api/im-gateway/chat/runner-calls/stream` 向真实 `Traex` runner 发送 `latest-9900-api.png`，父会话 `admin-chat-web-ui-traex-metadata-1782384031439` 成功返回 run `1782384031460-4af24711-839e-4c9b-8246-f4047298e0e3`，图片保存到 `.../attachments/session-runner-call_admin-chat-web-ui-traex-metadata-1782384031439_Traex-1782384031/1782384031460-4af24711-839e-4c9b-8246-f4047298e0e3/images/image-1.png`。随后用浏览器打开 `http://127.0.0.1:9900/_bifrost/ai?aiSection=agent-chat&agentSection=chat&session=admin-chat-web-ui-traex-metadata-1782384031439`，会话正文可见 `Run with Traex` 和绝对图片路径；点击 `Status` 后确认 `Runner diagnostics` 展示 `traex · traecli 0.200.12`、`Attachments 1 · 79 bytes`、I/O、Tools、Run time `5.9s`、First event `1.9s`、Resume `false`。
- 2026-06-25：执行 TC-IEC-43/44 的真实服务回归。临时服务端口 `18948`，数据目录 `/tmp/bifrost-runner-image-real2.LOZvE5`，启动命令使用 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`、`BIFROST_DISABLE_TRAY=1`、`cargo run --bin bifrost -- start --host 127.0.0.1 -p 18948 --unsafe-ssl --skip-cert-check --no-system-proxy`。配置 mock runner `mock-image` 后，`/_bifrost/api/im-gateway/chat/stream` 带 `hello.png` 返回 run `1782377507822-55608d18-baf4-4923-beab-8a731a8f6dde`，`/_bifrost/api/im-gateway/chat/runner-calls/stream` image-only 返回 run `1782377530402-a904849d-f79b-4ce8-a362-ecf26fe4f6be`。两个 run 均返回 `BIFROST_IMAGE_PATH_OK`，prompt 均包含 `## Attached Images` 和 `/attachments/session-.../images/image-1.png`；普通 Web Chat 图片路径为 `.../attachments/session-web-image-session-1-1782377498/images/image-1.png`，字节为 `hello-image`；runner-call 图片路径为 `.../attachments/session-runner-call_human-runner-call-parent-real2_mock-image-1782377530/images/image-1.png`，字节为 `runner-call-image`，两者路径不同且互不覆盖。随后将该真实链路固化为自动 E2E，命令 `SKIP_BUILD=true BIFROST_BIN=/Users/eden_studio/work/github/bifrost/target/debug/bifrost e2e-tests/tests/test_im_gateway_external_runner_image_input.sh` 通过，自动脚本再次验证普通 Web Chat run `1782377882467-f3974976-5584-4e96-a4a3-b155207e24d0` 与 runner-call run `1782377882519-f15c3c1a-90cc-40da-b21a-2e8b3756778c` 的图片路径位于不同 session 附件目录。TC-IEC-45 的 Feishu 真实发送本轮未执行，使用代码级 resolver/传递回归和 Web 真实链路覆盖修复主体，仍需后续接入真实 Feishu bot 做人工验收。
- 2026-06-25：追加执行 TC-IEC-43 同 session 多轮图片覆盖复现。当前 9900 服务运行 `/Users/eden_studio/work/github/bifrost/target/debug/bifrost`，临时配置 mock runner 后，对同一个 `sessionKey=overwrite-live-same-session-1782380510` 连续调用两次 `/chat/stream`。修复前两次 run `1782380510858-b6eaf17e-04b4-4a2a-8e97-419a18819d93` 与 `1782380510912-6bc96012-83dd-4461-a528-9ffb145534c7` 的图片路径相同：`.../attachments/session-overwrite-live-same-session-1782380510-1782380510/images/image-1.png`，第二轮后第一轮路径文件字节从 `FIRST_IMAGE_BYTES_UNIQUE` 变成 `SECOND_IMAGE_BYTES_DIFFERENT`，确认 P1 覆盖问题真实存在。修复后自动 E2E `test_im_gateway_external_runner_image_input.sh` 必须验证同一 `sessionKey` 第二轮图片路径按 run id 分目录且第一轮字节保持 `hello-image`。
- 2026-06-14：执行 TC-IEC-42。先在 `main` 启动临时服务，端口 `51754`，数据目录 `/tmp/bifrost-traex-visible-1781401190`，命令包含 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1` 与 `--no-system-proxy`；配置真实 Traex runner `/Users/eden/.local/bin/traex` 后调用 `/chat/stream`，run `1781401268378-9439e402-6e4c-4b12-85a9-09e6f889b792`。`cli.stdout.log` 与 `normalized_events.jsonl` 显示 Traex 先输出 `agent_message`“检查当前 Traex 版本并与最新可用版本对比。”，再交叉输出 `command_execution` 工具事件，最后输出 `agent_message`“**结论：** 当前 Traex 版本为 `0.200.9`，已是最新版本，无需更新。”；session JSONL 同步记录工具前 `assistant_delta`、工具调用、最终 `assistant_delta` 和 `assistant_message`。补充单测 `traex_model_messages_stay_visible_while_machine_statuses_are_hidden` 后执行 `SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin im_gateway::progress_card --lib` 通过，29 个 progress card 用例全部通过，确认机器状态隐藏、工具前模型公开 content 保留、终态重复结论不再留在过程区。
- 2026-06-12：执行 TC-IEC-41 的本地回归步骤，命令 `SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin external_cli_runtime_marks_stopped_run_before_late_stdout --lib -- --nocapture`、`SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin external_cli_runtime_stops_active_run_by_session_key --lib -- --nocapture`、`SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin request_run_stop_treats_missing_active_pid_as_stopped --lib -- --nocapture`、`SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin taskkill_missing_process_messages_are_idempotent --lib -- --nocapture` 均通过。Windows Unit Tests 的真实 `taskkill` missing pid 路径待 PR GitHub Actions 验证。
- 2026-06-08：执行 TC-IEC-39/40 的本轮回归验证。命令 `SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin feishu_codex_like_external_runner_defaults_to_progress_card_without_channel_override --lib -- --nocapture` 通过，确认 Feishu + Trae/Codex external runner 在没有 channel delivery override 时解析为 `ProgressCard`，且显式 channel/input override 与非 Feishu Provider 行为不变。命令 `pnpm --dir web exec vitest run src/pages/AI/AgentChatSection.timeline.test.ts` 通过，1 个测试文件 10 个用例全部通过，确认 external runner 的 thinking/tool `processSteps` 挂在最终 assistant message 上，不再生成额外 `Agent is running...` 占位消息。
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
- 2026-06-07：补充执行 TC-IEC-29 的飞书 progress card 状态/model 回归验证，命令 `SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin progress_card --lib -- --nocapture` 通过，外部 runner 卡片展示 `Runner: codex`、`Adapter: codex`、模型标签、外部会话、队列/引导、工作路径和最新工具；真实 Codex E2E `RUN_REAL_CODEX_E2E=1 BIFROST_CODEX_BIN=/opt/homebrew/bin/codex e2e-tests/tests/test_im_gateway_codex_runner_streaming.sh` 通过。
- 2026-06-07：补充执行 TC-IEC-29/30 的飞书 progress card 过程折叠回归验证，命令 `SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin progress_card -- --nocapture` 通过 20 个测试，断言全局状态在顶部、执行 Pipeline 在中间、最终结论在底部；运行中 Pipeline 默认展开，完成后 Pipeline 默认折叠；Pipeline 内按 Loop 先展示模型输出，再展示工具摘要，单条工具详情默认折叠且展开后展示输入/输出。命令 `SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin codex_cli_parser_maps_reasoning_summary_to_assistant_delta -- --nocapture` 通过，确认公开 reasoning summary 进入过程 timeline。真实 Codex E2E `RUN_REAL_CODEX_E2E=1 BIFROST_CODEX_BIN=/opt/homebrew/bin/codex e2e-tests/tests/test_im_gateway_codex_runner_streaming.sh` 通过，run `1780814063688-a2641c31-d00d-4906-9f01-a19940194cef`；真实 Trae E2E `RUN_REAL_TRAEX_E2E=1 SKIP_BUILD=true BIFROST_TRAEX_BIN=/Users/eden/.local/bin/traex e2e-tests/tests/test_im_gateway_traex_runner_streaming.sh` 通过，run `1780814083507-7b776ac0-6448-43ce-8ca8-7b90406ce14a`。真实飞书发送待本地服务重启后由飞书链路人工触发验证。
- 2026-06-07：执行 TC-IEC-31 的本地回归分析，真实飞书消息 `om_x100b6d659c70dd00b1ae9657765ca2a` 对应 Trae run `1780817198147-3a72a280-b4bc-44b0-9dfe-789f5f48d1c2`，`runtime_snapshot.json` 确认 executable 为 `/Users/eden/.local/bin/traex`、workDir 为 `/Users/eden/work/github/bifrost-traex-runner`；`normalized_events.jsonl` 有 59 条事件，包含多个 `agent_message`、`tool_started`、`tool_finished`，但 `result.json.status` 为 `timed_out`。补充回归测试 `SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin progress_card -- --nocapture` 通过 21 个测试，新增断言运行中的 `AssistantFinal/agent_message` 进入 Pipeline 且不提前写底部最终结论；`SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin timed_out_external_cli_result_reports_failure_reply -- --nocapture` 通过，确认 `TimedOut` 生成失败文案而不是使用早期 agent message。
- 2026-06-07：执行 TC-IEC-32 的问题复核，真实 Trae run `1780819082852-cfa6cdf8-b605-4ad0-a213-2bb778000d49` 有 55 条 normalized events（4 条 `assistant_final`、32 条 `tool_started`、16 条 `tool_finished`），说明 Trae 过程事件已经实时读到；问题在于配置仍带 180 秒超时以及卡片中重复 tool started 和超长输出详情导致中间更新不稳定。补充回归测试覆盖 Trae 默认无 timeout、重复 running tool 去重和工具详情输出预览限长；真实 Trae E2E `RUN_REAL_TRAEX_E2E=1 SKIP_BUILD=true BIFROST_TRAEX_BIN=/Users/eden/.local/bin/traex e2e-tests/tests/test_im_gateway_traex_runner_streaming.sh` 通过，run `1780821058530-058460b9-a487-4184-a01e-8eadeaf347f8`，并断言 `runtime_snapshot.timeoutSecs` 为空。
- 2026-06-07：执行 TC-IEC-33 的真实 Web Chat 长任务验证，当前分支服务使用 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 cargo run --bin bifrost -- start -p 9900 -H 0.0.0.0 --allow-lan --daemon --unsafe-ssl --skip-cert-check --no-system-proxy` 启动，最终 PID `59573`，系统代理保持禁用；Web Chat 通过 `runnerId=traex`、`adapter=traex`、workDir `/Users/eden/work/github/bifrost-traex-runner` 触发真实 Trae review，session `admin-chat-webview-trae2-1780826332`，history `/Users/eden/.bifrost/agent/sessions/2026/06/07/session-admin-chat-webview-trae2-1780826332-1780826332.jsonl`。流式响应自然结束为 `status:"succeeded"`，过程中出现多段 `assistant_final/agent_message` 与 `tool_started/tool_finished`，WebView 运行中可看到 `I'll review the current branch...`、`Let me read the key files...`、`Now let me check...` 等公开 content 穿插在 `exec_command: <命令>` 工具摘要之间。完成后刷新 history 页面，过程块默认折叠且不再显示 `我先执行一步检查。`，摘要按钮显示 `Ran 22 commands · 6m 57s`；点击摘要后可展开看到 content -> tool 摘要的交叉过程，且最终 review 结论位于过程块下方。点击首条 `exec_command` 工具行后显示 `Input:` 与 `Output:` 预览，长输出保留 `展开更多` 控件。
- 2026-06-07：执行 TC-IEC-34 的 Feishu progress card 卡住问题回归分析，真实飞书消息对应 Trae run `1780827766302-6a2c209c-84f6-4e64-b487-dcaaa8837361` 已成功完成，canonical history 包含大量 `assistant_delta`、`assistant_message`、`tool_call` 和 `tool_result`，但飞书日志出现 `code=300301` 与 `ElementID agent_process_tool_11_detail: Code 1002: elementID format error. Only alphabets, numbers, and underscores are allowed. It must start with an alphabet and not exceed 20 characters.`；确认根因是 progress card 第 10 条以后动态过程元素 ID 超过飞书 20 字符限制。修复后命令 `SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin progress_card -- --nocapture` 通过 23 个测试，新增用例覆盖 `ap_t_35` / `ap_td_35` 两位数工具 ID，并递归断言所有 `element_id` 符合飞书格式限制。
- 2026-06-07：根据真实飞书截图继续执行 TC-IEC-34 文案回归，去除执行过程中的 `Loop 1/2`、`Pipeline`、`工具摘要`、`[模型]`、纯 run id、`turn started` 与 `model rerouted` 等内部提示；过程区域改为从上到下展示公开模型内容和工具调用，工具详情仍保持可展开。命令 `SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin progress_card -- --nocapture` 与 `SKIP_FRONTEND_BUILD=1 cargo run -p bifrost-e2e -- --test im_gateway_agent_streaming_progress_card_renderer --jobs 1 --timeout 120` 均通过。
- 2026-06-07：继续执行 TC-IEC-34 的工具密集场景回归，飞书 progress card 将连续 3 条工具调用默认合并为 `已运行 3 条命令` 一级折叠组，展开后每条工具仍是独立折叠项；同时把过程 Markdown 从双空行压缩为单换行，减少行高和竖向占用。新增 `consecutive_process_tools_are_grouped_by_default` 回归测试。
- 2026-07-13：PASS — 按更新后的 TC-IEC-34 执行 `feishu_progress_card_process_element_ids_stay_within_feishu_limits` 与 `consecutive_process_tools_are_grouped_by_default`，各 1 项测试均通过；当前 CardKit payload 使用“已执行 N 个步骤”分组文案，元素 ID 限制和折叠行为保持不变。随后复跑 `file_change_` 9 项单测及 `im_gateway_file_change_progress_card_renderer` E2E，全部通过。
- 2026-06-07：继续执行 TC-IEC-34 的过程文案回归，飞书 progress card 的模型公开 content 不再添加 `1.`、`2.`、`3.` 编号前缀，按时间顺序直接展示原文；新增断言覆盖 `1. 我先看分支差异` / `1. 我会先检查代码路径` 不应出现在卡片 JSON 中。
- 2026-06-07：执行 TC-IEC-35 的代码路径复核与单元回归，确认 IM event loop 和 Web Chat `/stream` 对 external/custom runner 的运行中新消息默认走 `SessionQueueManager` 排队，不做 guide 注入；`/stop` 仍单独尝试停止当前外部进程。补充 Trae 排队续跑回归，修复 `apply_external_cli_resume_metadata` 只给 Codex 注入 `threadId` 的问题，确保 Trae queued continuation 也能走 `traex exec resume ... <threadId> -`。命令 `SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin busy_message_mode -- --nocapture` 通过 9 个测试，`SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin 'adapter_builds_resume_command_from_thread_id' -- --nocapture` 通过 Codex/Trae 2 个 resume 命令构造测试。
- 2026-06-07：补充执行 TC-IEC-18 的 Agent Runners Adapter 菜单回归，命令 `pnpm --dir web exec playwright test tests/ui/admin-settings.spec.ts -g "Agent Runners 新增弹窗只展示当前支持的 Adapter" --reporter=line` 通过 1 个测试；截图级验证 Add Runner 弹窗 Adapter 下拉只展示 `Codex CLI`、`Trae CLI`、`ChatGPT Web`，不再展示 `Custom`、`Mock`。
- 2026-06-07：执行 TC-IEC-37 的状态唯一真源回归，命令 `cargo test -p bifrost-agent scan_session_summary_tracks_external_runner_metadata -- --nocapture`、`cargo test -p bifrost-admin active_history_detail_uses_chatgpt_web_binding_from_history_state -- --nocapture`、`cargo test -p bifrost-admin runner_call_visible_messages_are_recorded_in_parent_history -- --nocapture`、`pnpm --dir web exec tsc -b`、`pnpm --dir web exec playwright test tests/ui/agent-chat.spec.ts -g "initial load|stops running history polling|completed history override|running history timeline" --reporter=line` 均通过。随后用默认 9900 服务真实验证：`admin-chat-1779725144963` 顶部显示 `Runner: web` 且续聊返回 `OK`，JSONL 追加 `agent_kind=web` running/completed 与 `assistant_message OK`；`admin-chat-1780845051758` 刷新后顶部为 Ready，消息区 `thinkingTail=0`、`runningGroups=0`、`Run state: Running` 不可见，running placeholder 不可见。
- 2026-06-08：补充执行 TC-IEC-27 的 Trae CLI 参数冲突回归。真实飞书 IM 会话 `feishu-main:ou_64f88363f262c64aba91f0b9e1aaed81` 的 run `1780879951643-4487fc45-367c-4e2b-9c4d-be987ef0c370` 失败，`cli.stderr.log` 为 `Error: sandbox_mode and permission_mode overrides cannot both be set`，`runtime_snapshot.args` 同时包含 `--permission-mode bypass_permissions` 和 `--dangerously-bypass-approvals-and-sandbox`。修复后 focused 单测 `SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin traex_adapter_ --lib -- --nocapture` 通过 5 个测试，断言默认/default/resume full access 只生成 dangerous full access，不再同时生成 `--permission-mode`；真实 Trae E2E `RUN_REAL_TRAEX_E2E=1 BIFROST_TRAEX_BIN=/Users/eden/.local/bin/traex e2e-tests/tests/test_im_gateway_traex_runner_streaming.sh` 通过，run `1780880340108-0246e9c7-cb2f-4244-a667-c7002d4f1dba` 成功完成，并断言 `runtime_snapshot.args` 不同时包含 `--permission-mode` 与 `--dangerously-bypass-approvals-and-sandbox`。
- 2026-06-08：执行 TC-IEC-38 的 Trae/Codex stdout `turn.completed` 不提前结束回归。focused 单测 `cargo test -p bifrost-admin external_runner_ --lib -- --nocapture` 通过 20 个 external runner 相关测试。随后使用临时服务端口 `18938`、`BIFROST_DATA_DIR=/tmp/bifrost-traex-delayed-final-verify`、`BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 cargo run --bin bifrost -- start -p 18938 --unsafe-ssl --skip-cert-check --no-system-proxy` 启动当前源码服务，配置 mock runner `traex-delayed-final` 先输出 `turn.completed`、睡眠 5 秒再输出 `agent_message: BIFROST_DELAYED_FINAL_OK`；早期 `turn.completed` 后查询 `/agent/sessions/all` 返回 `running:true`、`status:"active"`、`state:"running"`、`run_state:"running"`，最终 stream 返回 `BIFROST_DELAYED_FINAL_OK` 且 session 收敛为 `status:"ended"`、`run_state:"completed"`，JSONL 顺序为 `assistant_message` 早于 `run_state_changed: completed`。自动 E2E `SKIP_BUILD=true BIFROST_BIN=/Users/eden/work/github/bifrost/target/debug/bifrost e2e-tests/tests/test_im_gateway_external_runner_delayed_final_state.sh` 通过，覆盖同一真实服务链路。

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
