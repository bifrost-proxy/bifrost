# 移除 ChatGPT Web Runner

内置 `chatgpt_web`（显示名 ChatGPT Web）已停止支持。`chatgpt-web`、`gpt-web`、`gpt_web` 等旧 adapter 名称会明确报错，不能通过自定义 executable、legacy execution mode 或 browser worker 恢复执行。

## 迁移

- 旧 Runner 配置仍可读取，旧会话、history、运行产物、凭据和 browser profile 保留。不会自动选择其他 provider，也不会自动改变权限或产生其他 Runner 的付费请求。
- 在 Agent Runners 编辑旧 Runner，将 Adapter 明确改为 Codex、Traex、Claude Code 或已授权的自定义 CLI，并确认权限、模型和费用后保存；也可以在 JSON 配置中显式修改。检查默认 Runner、各 IM channel、Daily Agent、research fan-out 与 schedule 的绑定。
- 如要删除旧 Runner 配置，请先明确更新所有引用；保留历史数据不影响退役。
- 删除 research fan-out 的 `chatgpt_project_url`。旧字段仍可反序列化以给出迁移错误；已不再提供项目归档、Chat/Pro 浏览器模式或登录操作。
- 旧登录 API `chat/adapters/chatgpt-web/auth/*` 返回 HTTP 410 和移除说明，普通聊天/执行请求返回明确错误。WebUI 不再提供该 Adapter 或项目配置；CLI help 不再推荐它。

## 范围与验证

删除专属浏览器/CDP 实现、登录、worker dispatch、环境变量、日报/调度特殊分支及专属测试。共享 WebSocket、HTTP、ZIP、浏览器相关工具与 OpenAI API/Codex 能力保留。`WorkerKind::Browser` 仅用于读取历史 job 元数据，不能通过 worker API 解析、启动或运行。

回归覆盖旧别名拒绝、自定义 executable 不可绕过、旧配置保留与显式迁移、历史文件不变、登录 API 410、Runner 选项消失、其他 Runner 的执行与会话回归。必须执行仓库 changed-lines 90% 预检；完整跨平台 CI 在独立 PR 验证。旧 `WorkerKind::Browser` 的反序列化仅用于历史元数据，worker 启动/解析入口已删除。


研究 fan-out 会在读取缓存产物之前检查所有绑定，不能通过旧缓存把退役配置标记为可继续运行。界面删除仍被绑定的退役 Runner 时明确拒绝，不自动替换默认 Runner 或 channel。已有配置可通过显式编辑 Adapter 或 JSON 绑定迁移。

验证记录见 [退役真实场景用例](../human_tests/remove-chatgpt-web-runner.md)。本地完整 clippy 使用 `mise.toml` 固定的 Rust 1.95.0，而非浮动 stable。


本地回环 mock 测试需要显式设置 `BIFROST_E2E_ALLOW_FEISHU_LOOPBACK_BASE_URL=1`（仅限测试子进程）。未设置时，三个既有 group/debug-inbound 测试在 main 与本分支均超时；设置后三个隔离复测全部通过。该开关仅使用 debug 构建的回环 mock，不修改生产 allowlist 或云端网络策略。

2026-10-30 本地验证：Rust 1.95.0 workspace clippy、Rust fmt、前端生产构建、类型检查和定向 ESLint 通过；前端单元测试 511/511 通过。带显式回环 mock 开关的管理端为 3507 通过、1 忽略；CLI lib/bin 分别为 1456/1472 通过、各 2 忽略，相关 CLI 和 worker 集成测试通过。完整 workspace 与 `make coverage-changed` 在未修改的 daemon shutdown 集成测试处，因只读 home 无法创建 CLI 代理 profile 锁而停止（2 项失败）；未修改 HOME、用户 profile 或生产代理设置。补采真实退役 E2E 与 CLI/API 黑盒覆盖率后，同源 changed-lines 门禁为 90.97%（131/144），超过 90% 下限。完整跨平台 CI 仍需在 draft PR 核验。
