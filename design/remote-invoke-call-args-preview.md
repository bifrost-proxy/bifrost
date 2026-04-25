# Remote Invoke Recent Calls 参数预览回退

## 背景

Remote Invoke 的 `openCall` 已升级到密文链路，relay 不再持久化明文 `command_summary`。当前 client 侧 `Recent Calls` 标题区域仍优先依赖 `command_summary.masked_args_json` 渲染参数预览，导致加密链路下即使本地已经解密并保存了 `command.args_json`，Web UI 依然显示不出命令参数详情。

用户可见现象：

- `Recent Calls` 只显示命令名与状态，不显示参数预览
- hover Tooltip 也没有完整参数 JSON
- `bytes_out` 仍能显示，说明并非整条调用记录缺失，而是参数摘要字段为空

## 目标

- `Recent Calls` 在加密链路下继续展示可读参数预览
- 优先复用已有 `command_summary.masked_args_json`
- 若 relay 未下发该字段，则 client 本地使用已解密的 `command.args_json` 补齐
- 不改变 connect 等无参数命令的展示行为
- `Recent Calls` 本地落盘，Bifrost 重启后仍能恢复最近记录
- 支持一键清理当前客户端的全部 Recent Calls
- 本地记录默认保留 7 天，单个 relay/client 最多保留 100000 条
- 命令相关文本超过 200 字符时直接截断，只保留前 200 字符用于展示和落盘，避免超长命令撑爆本地历史文件

## 实现方案

### 1. Client Worker 本地补齐参数摘要

文件：`crates/bifrost-admin/src/remote_invoke/worker.rs`

- 在 `build_call_command_summary()` 中保留现有 `command_preview` 回退逻辑
- 新增 `masked_args_json` 回退：
  - 若 relay 下发的 `masked_args_json` 非空，则保持原值
  - 若为空，则使用本地解密后的 `RemoteCommand.args_json`
  - 空字符串按缺失处理，避免写入无意义占位

这样 `GET /api/remote-invoke/calls` 返回的本地调用历史可以稳定包含参数摘要，不再依赖 relay 明文字段。

### 1.1 Recent Calls 本地落盘与清理

文件：`crates/bifrost-admin/src/remote_invoke/worker.rs`

- 在 worker 内维护 `CallHistoryStore`，落盘文件为 `BIFROST_DATA_DIR/admin/remote_invoke_call_history.json`
- 存储维度为 `relay_url + client_instance_id + call_id`
- 收到 `call_open` 并完成本地解密后立即写入记录
- completed / failed / cancelled 终态更新同一条记录
- worker 启动和 relay_url 切换时按当前 relay/client 恢复历史
- 非终态历史在恢复时收敛为 `failed`，避免重启后永久显示 streaming
- `remote_invoke.retention_days` 默认 7 天；`remote_invoke.max_records` 默认 100000 条
- `DELETE /_bifrost/api/remote-invoke/calls` 清理当前 relay/client 的全部本地 Recent Calls
- 写入内存历史和本地落盘前统一执行 200 字符截断：
  - `command_summary.command_preview`
  - `command_summary.masked_args_json`
  - `command.command`
  - `command.args_json`
  - `command.query` 中的字符串字段
  - `command.argv`、`command.shell`、`command.command_text`、`command.cwd`、`command.env`
  - `policy_id`
- 截断逻辑不添加省略号，严格保留原始前 200 个 Unicode 字符；API、Web UI、落盘文件看到同一份截断后的内容。
- 对 `args_json` / `masked_args_json` 这类 JSON 字符串，截断发生在 JSON 内部的字符串值上，序列化后的 JSON 仍保持合法，避免参数预览因为硬截断变成不可解析文本。

### 2. Web UI 再做一层展示回退

文件：`web/src/api/remoteInvoke.ts`
文件：`web/src/pages/Settings/tabs/RemoteInvokeTab.tsx`

- 抽出 `Recent Calls` 参数预览来源函数
- 展示顺序：
  1. `call.command_summary.masked_args_json`
  2. `call.command.args_json`
- `RemoteInvokeTab` 的标题预览与 Tooltip 共用同一个来源，避免标题和 hover 内容不一致
- Recent Calls 卡片右上角增加清理按钮，调用 `DELETE /remote-invoke/calls` 后清空页面列表

### 2.1 Recent Calls 长内容布局与详情弹窗

文件：`web/src/pages/Settings/tabs/RemoteInvokeTab.tsx`

- 每条 Recent Calls 记录改为稳定的三段布局：
  1. 左侧为命令摘要和参数预览，占据剩余宽度并允许收缩
  2. 中间为状态、caller、policy、exec mode 等短标签，不被长命令挤压
  3. 右侧保留详情按钮，点击后打开完整记录弹窗
- 命令摘要、参数预览、caller、policy、exec mode 均单行显示并自动 `ellipsis` 截断，避免长 shell 文本把列表撑高或把右侧列压成竖排。
- 点击记录或详情按钮时调用 `GET /remote-invoke/calls/{call_id}` 拉取最新详情；失败时回退当前列表记录，保持详情可读。
- 详情弹窗展示服务端已保存的命令、参数 JSON、调用 ID、grant/client/caller、状态、耗时、流量、policy/exec mode 与命令详情 JSON；其中命令相关长文本最多为前 200 字符，不再暴露或保存完整超长原文。

## 测试方案

### 单元测试

- `crates/bifrost-admin/src/remote_invoke/worker.rs`
  - 验证 `command_summary.masked_args_json` 缺失时，会回退到 `command.args_json`
  - 验证已有 `masked_args_json` 时不会被本地 `args_json` 覆盖
  - 验证 call history store 按 7 天保留期和 max_records 裁剪
  - 验证 clear_for_client 只清理当前 relay/client
  - 验证重启恢复时 streaming 记录收敛为 failed
  - 验证 call history store 写入前会把命令相关字符串截断到 200 字符，原始长片段不会出现在落盘 JSON 中
- `web/src/api/remoteInvoke.test.ts`
  - 验证 Recent Calls 参数预览来源优先使用 `masked_args_json`
  - 验证缺失时回退到 `command.args_json`

### E2E 测试

- 更新 `e2e-tests/tests/test_remote_invoke_e2e.sh`
- 新增断言：
  - `remote search` 执行后，`/api/remote-invoke/calls` 中对应记录的 `command_summary.masked_args_json` 非空
  - 其中包含 `query`、`max_results`、`max_scan`
- 更新 `e2e-tests/tests/test_remote_invoke_recent_calls_args_preview_e2e.sh`
- 新增断言：
  - Recent Calls 写入 `remote_invoke_call_history.json`
  - 超长搜索参数在 Recent Calls API 中只返回前 200 字符，且 `masked_args_json` 仍是合法 JSON
  - `remote_invoke_call_history.json` 不包含完整超长参数原文
  - 保留同一 `BIFROST_DATA_DIR` 重启 Bifrost 后，同一 `call_id` 仍可从 Recent Calls API 读取
  - DELETE Recent Calls 后 API 返回空列表
- Web UI 回归验证：
  - 构造包含超长 `shell.exec` 文本的 Recent Calls 记录
  - 在浏览器中打开 Remote Invoke Tab，确认列表行保持单行摘要、右侧标签不换成竖排
  - 点击该记录后确认详情弹窗展示完整命令和参数

### 真实场景测试（human_tests）

- 更新 `human_tests/remote-invoke.md`
- 新增回归用例：加密链路下 `Recent Calls` 必须展示参数预览与 Tooltip 完整 JSON，重启后不丢失，并支持清理全部记录
- 新增回归用例：超长命令不会撑乱 Recent Calls 布局，且 API / 详情 / 落盘文件都只保留前 200 字符
- 同步更新 `human_tests/readme.md` 索引与用例数

## 校验要求

- `pnpm --dir web test:unit -- src/api/remoteInvoke.test.ts`
- `cargo test -p bifrost-admin build_call_command_summary -- --nocapture`
- `bash e2e-tests/tests/test_remote_invoke_e2e.sh`
- `bash scripts/ci/local-ci.sh --e2e-only platform`
- `cargo test --workspace --all-features`
- `rust-project-validate`

## 文档更新要求

- 本次改动仅修复 Remote Invoke Recent Calls 展示回退逻辑，不涉及 README / 外部 API 文档变更
- 必须更新 `human_tests/remote-invoke.md`
- 必须更新 `human_tests/readme.md`
