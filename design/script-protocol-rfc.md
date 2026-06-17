# Script 协议与脚本系统

## 现状结论

旧 RFC 仍停留在提案态，与当前实现已经有明显差距。现在脚本系统不只包含 `reqScript` / `resScript`，还包含 `decode` 脚本、脚本日志、脚本管理页，以及可配置的 sandbox file/net API。

## 当前实现

- 规则语法层面已经支持：
  - `reqScript://...`
  - `resScript://...`
  - `decode://...`
- 管理端已经有独立的 `Scripts` 页面，使用 Monaco 编辑器提供类型定义与脚本测试体验。
- Traffic Detail 已经能展示请求/响应脚本执行结果与脚本日志。
- 前端类型定义里已经暴露：
  - `request` / `response` / `ctx` / `log`
  - `file` / `net` sandbox API
- sandbox 相关配置不是“完全固定不可配”，而是有 `getSandboxConfig` / `updateSandboxConfig` 管理入口。

## 与旧 RFC 的偏差

- 文档把范围限定在 request / response 两类脚本，已经过时。
- 文档假定的“只提供极简安全对象、禁止文件和网络”也不再准确；当前实现支持受控 file/net 能力。
- 引擎选型、运行时细节若要继续保留，必须以实际 `bifrost_script` 集成为准重新核对，不能继续把旧提案写成事实。

## 建议 (planned, not yet shipped as of 2026-06-17)

- 这份文档应该重写为“已实现能力说明”，而不是继续维护 RFC 形态。
- 后续如果补文档，建议拆成：
  - 规则协议语义；
  - 脚本运行时与 sandbox；
  - 管理端脚本编辑 / 调试 / 日志体验。

## 参考实现 (verified 2026-06-17)

- 协议常量：`crates/bifrost-core/src/protocol.rs` (`Protocol::ReqScript` / `ResScript` / `Decode`)
- 规则解析：`crates/bifrost-core/src/rule/parser/mod.rs`
- 脚本引擎：`crates/bifrost-script/src/engine.rs`、`crates/bifrost-script/src/sandbox.rs` (`SandboxConfig` 暴露 `file_root` / `file_allowed_dirs` / `allow_network` / `network_timeout_ms` / `max_file_bytes` / `max_net_request_bytes` / `max_net_response_bytes`)
- 内置 parser 脚本：`crates/bifrost-script/src/builtins.rs` (`build_in_bp`)
- Sandbox 管理 API：`crates/bifrost-admin/src/handlers/config.rs` (`get_sandbox_config` / `update_sandbox_config`)
