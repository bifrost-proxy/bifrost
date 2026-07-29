# ASR Daily Agent 可靠性测试用例

## 功能模块说明

本用例验证 ASR Daily Agent 在重启、重复请求、部分失败和历史补录场景下的持久化边界。覆盖微信发送就绪状态、幂等投递、研究问题级复用、产物失效、导入完成屏障和日期水位线。

## 前置条件

- 在仓库根目录执行命令前先运行 `source ~/.zshrc`。
- 自动化测试使用临时数据目录，不读取或修改用户真实 ASR 数据。
- 如需启动测试服务，必须设置 `BIFROST_DISABLE_TRAY=1`、`BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`，使用非 9900 端口并带 `--no-system-proxy`。
- 不移动、删除或重新复制任何真实录音。

## 测试用例列表

### TC-ADR-01 微信 context token 加密持久化

操作步骤：

1. 在临时数据目录创建微信 provider context store。
2. 写入测试 context token，读取落盘文件并检查权限与明文泄漏。
3. 重新创建 provider，查询同一用户的 `send_ready`。
4. 注入损坏密文与持久化失败。

预期结果：

- 文件中不出现 token 明文，Unix 权限为 `0600`。
- 重启后 `send_ready=true`。
- 损坏密文返回 `send_ready=false`。
- 持久化失败时 token 不会只发布到内存。

### TC-ADR-02 微信状态区分 connected 与 send_ready

操作步骤：

1. 构造已配置但尚无入站 context token 的微信 provider。
2. 查询 provider status。
3. 写入有效 context token 后再次查询。

预期结果：

- 首次响应包含 `send_ready=false` 和可理解的原因。
- context token 可用后响应包含 `send_ready=true`。
- API 不返回 token 内容。

### TC-ADR-03 重复投递只调用 provider 一次

操作步骤：

1. 用固定 `idempotency_key`、目标和纯文本提交第一次发送。
2. 记录成功 message ID。
3. 用完全相同请求再次提交。
4. 用同一 key 提交不同正文。

预期结果：

- 第二次相同请求直接返回持久化回执，不再次调用 provider。
- 不同正文返回冲突。
- outbox 文件权限为 `0600`，服务重启后仍可复用成功回执。
- provider 失败后的重试使用相同 client ID。

### TC-ADR-04 provider 回执后的本地故障可安全恢复

操作步骤：

1. 注入 outbox 最终提交失败。
2. 让 provider 返回成功。
3. 再次使用同一 idempotency key 发送。

预期结果：

- 首次请求明确报告“provider 已确认但本地提交失败”。
- 重试沿用相同 provider client ID，不生成新的逻辑消息。
- 持久化失败的状态不会错误地只发布到进程内存。

### TC-ADR-05 研究问题按指纹复用

操作步骤：

1. 为多个研究问题生成成功子结果和元数据。
2. 不改变问题再次运行 fanout。
3. 只修改一个问题，再次运行。
4. 篡改一个结果文件，再次运行。

预期结果：

- 未改变且哈希完整的成功问题直接复用。
- 只重跑指纹变化或结果损坏的问题。
- 已成功且完整的问题不会重复调用研究 runner。

### TC-ADR-06 产物版本与上游变化触发失效

操作步骤：

1. 生成 report 及 processed state v2。
2. 保持输入、配置和上游产物不变再次运行。
3. 分别修改 report、生成契约、agent 配置和上游报告。
4. 加载缺少 v2 artifact 字段的旧状态。

预期结果：

- 完全一致时标记 unchanged。
- 任一哈希或契约变化只使受影响的 agent/date 失效。
- 旧状态保守地重新生成一次并迁移。

### TC-ADR-07 日期水位线阻止意外历史扫荡

操作步骤：

1. 在无状态目录放入多个历史日期输入。
2. 执行一次无日期范围的自动运行。
3. 在水位线之前添加从未跟踪的历史文件后再次自动运行。
4. 显式指定该历史日期运行。

预期结果：

- 首次无范围运行只处理最新日期。
- 新出现但早于水位线的未跟踪日期不会被自动回填。
- 显式日期请求可执行受控 backfill。

### TC-ADR-08 导入完成屏障只消费一次

操作步骤：

1. 写入 importing 状态并尝试消费 completion token。
2. 写入 completed 状态、最终计数和 token，再次消费。
3. 重复消费同一 token。
4. 注入 ASR 调度失败并释放消费状态。

预期结果：

- 未完成或读回校验失败时不调度 ASR。
- 完整 token 只允许一次调度。
- 调度失败后 token 回到可重试状态。
- 不会创建并发或重复 ASR run。

## 清理步骤

- 删除自动化测试创建的临时目录。
- 如启动过测试服务，确认进程已退出。
- 不改动系统代理状态。

## 执行记录

| 日期 | 用例 | 命令 / 证据 | 结果 |
| --- | --- | --- | --- |
| 2026-07-28 | TC-ADR-01、TC-ADR-02 | 微信 context store、重启 `send_ready`、损坏密文和持久化失败单元测试；隔离端口运行 `test_weixin_provider_e2e.sh` | PASS |
| 2026-07-28 | TC-ADR-03、TC-ADR-04 | outbox 重复请求、正文冲突、provider 失败重试、回执后本地提交故障注入测试 | PASS |
| 2026-07-28 | TC-ADR-05 | `daily_agent_research_reuses_only_matching_untampered_child` 及问题指纹变化测试 | PASS |
| 2026-07-28 | TC-ADR-06、TC-ADR-07 | 产物哈希/契约/配置/上游失效测试；日期 watermark 与显式 backfill 测试 | PASS |
| 2026-07-28 | TC-ADR-08 | external import completion barrier、单次消费和调度失败释放测试 | PASS |
| 2026-07-28 | 全量回归 | 串行 `cargo test --workspace --all-features -- --test-threads=1`；严格 clippy；`local-ci.sh --skip-e2e --skip-deps-audit`；7/27 生产批次导入、ASR、Agent 与三条微信投递闭环 | PASS |
