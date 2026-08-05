# IM Codex Fast 模式切换

> 状态：已实现

## 背景

Bifrost 不应替用户决定 Codex 的 service tier。未显式配置或执行 session 命令时，
Exec 与 App Server 请求都必须省略 `service_tier`，让 Codex 使用自身默认模式。
Runner 静态配置仍可通过 `configOverrides` 显式设置该值；IM 通道也提供 `/fast`
session 命令，让用户主动开启、关闭或查询快速模式。

Codex CLI 的交互语义支持 Fast 模式开启、关闭和状态查询。IM Gateway 需要在
Bifrost 侧实现等价的命令状态机，不能依赖外部 Runner 把 prompt 文本解释成
CLI slash command。

## 用户目标验证清单

### 必须实现

- Codex Runner 支持 `/fast`、`/fast on`、`/fast off`、`/fast status`。
- 无参数 `/fast` 在当前 session 的有效模式上执行切换。
- `on` 写入 `service_tier="fast"`；`off` 写入 `service_tier="default"`。
- session override 在下一轮 Codex Exec 和 App Server 请求中优先于 Runner 静态配置。
- `/fast status` 展示当前有效模式、service tier 和来源。
- 未显式配置时 `/fast status` 明确说明 Bifrost 没有设置 tier、将使用 Codex 自身默认模式。
- 未显式配置时普通 turn 的 Exec 参数和 App Server thread/turn 请求均不携带 service tier。
- 运行中发送 `/fast ...` 作为系统命令处理，不进入实时 guide 或消息队列。

### 必须不破坏

- `/model`、`/effort`、`/cwd`、`/runner`、`/q`、`/rq` 等既有命令语义不变。
- 普通未知 slash 仍按现有 prompt passthrough / guide 语义处理。
- Runner 切换后，各 adapter/runner 的 session override 相互隔离。
- 清空 session 时与模型、effort override 一样清除 Fast override。
- Runner 静态 `service_tier` 配置继续生效；只有 session 命令覆盖它。

### 必须真实验证

- 通过 mock Codex Runner 的实际 argv 捕获验证 `fast` 与 `default` 确实进入
  下一轮命令参数。
- 验证 `/fast` 命令本身不进入 mock Runner stdin。
- 切换到非 Codex Runner 后发送 `/fast`，收到“不支持此命令”，且 Runner 不执行。
- 群聊 slash 分类和任务运行中命令路径都把 `/fast` 识别为系统命令。

## 命令语义

| 命令 | 行为 |
| --- | --- |
| `/fast` | 在当前有效 `fast` / `default` 之间切换 |
| `/fast on` | 将当前 Codex session 切换到快速模式 |
| `/fast off` | 将当前 Codex session 切换到标准模式 |
| `/fast status` | 查询当前有效模式、tier 与配置来源 |

除 `codex` adapter 外，任何 Runner 收到以上命令都返回：

```text
当前 Runner 不支持 `/fast` 命令；该命令仅支持 Codex Runner。
```

命令参数大小写不敏感；其它参数返回明确用法错误。命令仅改变后续 turn。

## 状态与优先级

`ImAgentSessionState` 新增：

- `service_tier_override`
- `service_tier_override_source`

有效值优先级：

1. 当前 session slash command override；
2. Runner `adapterConfig.configOverrides` 中的 `service_tier`；
3. 未显式配置时不传 `service_tier`，使用 Codex 自身默认模式。

应用 session override 时先移除请求中已有的 `service_tier` config override，再写入
session 值，避免 App Server 与 Exec 使用列表中较早的 Runner 值。

## 验证计划

### 单元测试

- parser：toggle/on/off/status、大小写、非法参数和 `/fastish` 边界。
- session override：静态 `fast` 被 session `default` 覆盖，来源正确。
- command spec：默认 Exec 不注入 tier；显式 session tier 正确生效。
- App Server：默认启动参数、thread/start、thread/resume、turn/start 均省略 tier；显式 session tier 正确生效。
- group classification：`/fast` 合法与非法形式均走系统命令路径。
- help：仅 Codex Runner 展示 `/fast`。

### E2E

扩展 IM Gateway 外部 Runner shell E2E：

1. 配置 mock Codex adapter，发送 `/fast status`，确认提示“未显式设置”且命令不执行 Runner。
2. 发送普通消息，从 mock Runner argv 确认没有 `service_tier`。
3. 发送 `/fast off` 后发送普通消息，从 mock Runner argv 确认
   `service_tier="default"`。
4. 发送 `/fast on` 后再次运行，确认 `service_tier="fast"`。
5. 在任务运行中发送 `/fast off` 并用 `/q` 排队下一轮，确认命令不进入 guide，
   且排队轮读取最新 session 状态并使用 `service_tier="default"`。
6. 切换非 Codex mock Runner，确认 `/fast` 返回不支持且 Runner 不执行。

### human_tests

在 `human_tests/im-gateway-external-cli-chat-gateway.md` 增加 Codex Fast
session 切换回归，并同步 `human_tests/readme.md` 对应模块索引。文档写入后立即按
用例逐条执行。

## Coverage 与交付

本地不运行 `make coverage` 或 `scripts/ci/coverage-all.sh`。业务代码覆盖率由远端
CI 的 `bash scripts/ci/coverage-all.sh --json --gate` 门禁验证。
