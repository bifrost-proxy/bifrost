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

## 实现方案

### 1. Client Worker 本地补齐参数摘要

文件：`crates/bifrost-admin/src/remote_invoke/worker.rs`

- 在 `build_call_command_summary()` 中保留现有 `command_preview` 回退逻辑
- 新增 `masked_args_json` 回退：
  - 若 relay 下发的 `masked_args_json` 非空，则保持原值
  - 若为空，则使用本地解密后的 `RemoteCommand.args_json`
  - 空字符串按缺失处理，避免写入无意义占位

这样 `GET /api/remote-invoke/calls` 返回的本地调用历史可以稳定包含参数摘要，不再依赖 relay 明文字段。

### 2. Web UI 再做一层展示回退

文件：`web/src/api/remoteInvoke.ts`
文件：`web/src/pages/Settings/tabs/RemoteInvokeTab.tsx`

- 抽出 `Recent Calls` 参数预览来源函数
- 展示顺序：
  1. `call.command_summary.masked_args_json`
  2. `call.command.args_json`
- `RemoteInvokeTab` 的标题预览与 Tooltip 共用同一个来源，避免标题和 hover 内容不一致

## 测试方案

### 单元测试

- `crates/bifrost-admin/src/remote_invoke/worker.rs`
  - 验证 `command_summary.masked_args_json` 缺失时，会回退到 `command.args_json`
  - 验证已有 `masked_args_json` 时不会被本地 `args_json` 覆盖
- `web/src/api/remoteInvoke.test.ts`
  - 验证 Recent Calls 参数预览来源优先使用 `masked_args_json`
  - 验证缺失时回退到 `command.args_json`

### E2E 测试

- 更新 `e2e-tests/tests/test_remote_invoke_e2e.sh`
- 新增断言：
  - `remote search` 执行后，`/api/remote-invoke/calls` 中对应记录的 `command_summary.masked_args_json` 非空
  - 其中包含 `query`、`max_results`、`max_scan`

### 真实场景测试（human_tests）

- 更新 `human_tests/remote-invoke.md`
- 新增回归用例：加密链路下 `Recent Calls` 必须展示参数预览与 Tooltip 完整 JSON
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
