# ChatGPT Web Runner 退役验证

所有操作使用新编译的云端二进制、独立临时数据目录和动态端口，不使用真实账号、既有浏览器档案或正式服务。

## TC-RWR-01：旧配置、CLI 与 API 明确拒绝

执行 `BIFROST_BIN=./target/debug/bifrost bash e2e-tests/tests/test_retired_web_runner.sh`。脚本预置旧 `chatgpt_web` 配置，验证 GET 仍可读取，未修改配置可原样保存；新增退役配置与普通/流式执行返回迁移错误，CLI 非零退出并展示同一错误，旧登录 API 返回 410。worker 模式列表与启动 API 不再提供 browser；CLI help 不再推荐旧 Runner。

## TC-RWR-02：数据保留与显式迁移

同一脚本预置历史、凭据和 profile 标记文件；只有显式把旧 Runner 修改为受控的 Python custom CLI 后才执行成功。断言三种旧文件内容不变，结果包含 `SUPPORTED_RUNNER_OK`。

## TC-RWR-03：界面入口消失与其他 Runner 可选

使用本地 Chromium 运行 `web/tests/ui/admin-settings.spec.ts` 的“Agent Runners 新增弹窗只展示当前支持的 Adapter”，以及 `web/tests/ui/asr-daily-agent-runner.spec.ts` 的 Runner dropdown 和退役 Project setting 用例。验证旧记录显示移除提示、Add Runner 无 ChatGPT Web、ASR 无 Project URL 字段；Codex/Traex/Claude Code 仍可选，选择支持的 Runner 后保存绑定。

## 执行记录

2026-10-30，在独立云端工作区使用 Rust 1.95.0 编译的真实二进制执行：

- TC-RWR-01/02：`test_retired_web_runner.sh` 通过，覆盖 API、真实 CLI、显式迁移后的 custom CLI 执行及三个文件内容不变。
- TC-RWR-03：系统 Chromium 151 和 FFmpeg 运行三项 Playwright 用例，3/3 通过；随后补测删除绑定旧 Runner 的场景，1/1 通过且配置 PATCH 次数为零。没有下载浏览器、复用个人 profile 或调整网络策略。
- 相关回归：五 Agent ASR 研究流水线 E2E 通过；auxiliary worker 核心隔离 E2E 通过；Rust 旧配置保留集成用例 1/1 通过。完整 ASR/Weixin/Remote Invoke companion 套件不属于本次退役用例。

## 评审增量回归

多条已绑定旧 Runner 同时存在时，新增受支持的替代 Runner、逐条迁移均应成功，其他旧记录与绑定原样保留；新增退役记录/绑定失败。删除仅被 Daily Agent 引用的任意命名旧 Runner 后，普通执行与缓存研究均须明确失败，不能调用 Codex。多个 Daily Agent 留有废弃项目 URL（含禁用记录）时，页面“清除废弃项目设置”仅移除该字段，随后可逐条编辑 Runner；API 使用显式 `remove_retired_project_urls: true`，不能自动切换提供商。

评审增量：11 项退役回归、93 项 Daily Agent 与 176 项 External CLI 测试通过；真实多旧配置 API/CLI 黑盒、更新后的 Rust 退役 E2E（1/1）、显式项目设置清理组件测试通过。完整 workspace 与原始覆盖率命令的只读 HOME 限制仍保留，不以增量通过替代。

两项评审增量浏览器用例通过（逐条迁移 Runner、显式清除多个废弃项目设置），并检查未自动改选 Runner。最终同源变更行门禁为 92.22%（166/180），阈值 90%；这是补采结果，不把原始失败命令改称通过。Rust 1.95 全目标/全特性 clippy 与 workspace/desktop fmt 通过。
