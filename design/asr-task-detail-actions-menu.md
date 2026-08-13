# ASR 任务详情操作菜单

## 背景

ASR Directory Task 详情页把刷新、音频压缩、原文件清理、失败分片重试、运行、暂停和强制暂停同时放在 Card 顶栏右侧。窗口宽度不足或任务状态增加可用操作时，右侧按钮组会挤压左侧返回按钮和任务标题，造成导航入口被遮挡。

## 用户目标验证清单

### 必须实现

- 顶栏右侧始终只显示一个省略号“更多操作”按钮。
- 当前任务运行状态、批量重试状态和源音频压缩状态在下拉菜单顶部展示。
- Refresh、Compress/Cancel compression、Clean originals、Retry all failed chunks、Run、Pause/Resume 和 Force Pause 全部进入下拉菜单。
- 保留既有禁用条件、暂停模式子菜单、危险操作确认和 Clean originals 双重确认。

### 必须不破坏

- 左侧返回按钮与任务标题仍保持可见、可点击。
- Overview、Files、Daily Docs、Daily Agent 和 Daily Agent Records Tab 行为不变。
- 亮色和暗色主题均使用 Ant Design 主题 token，不新增硬编码颜色。
- 不修改 ASR API、Rust 代码或代理运行逻辑。

### 必须真实验证

- Playwright 在窄视口打开任务详情，断言顶栏只有一个右侧更多按钮且返回按钮可见。
- 点击更多按钮后能看到状态和全部操作。
- 从菜单触发 Clean originals、Retry all failed chunks 和 Compress WAVs 时，既有确认与请求链路继续工作。
- 亮色、暗色主题下菜单均可见且布局不溢出。

## 交互设计

- 使用 Ant Design `Dropdown` + `Menu`，触发器为 icon-only `MoreOutlined` 工具按钮，提供 `aria-label="More task actions"` 和原生 `title`；菜单展开时不保留额外 Tooltip 浮层。
- 菜单第一组为只读状态：Task 始终显示；Bulk retry 和 Compression 在存在对应状态时显示。
- 操作按“通用 → 音频维护 → 任务运行控制”排列，并用 divider 分组。
- Pause 保留二级菜单：`Pause until next schedule` 与 `Pause indefinitely`。
- 需要确认的操作改由 `Modal.confirm` 承接，避免在 Dropdown 内嵌多个 Popconfirm 造成浮层嵌套和菜单提前卸载。

## 验证方案

- TypeScript：`pnpm --dir web exec tsc -b --pretty false`。
- Playwright：运行 ASR 详情相关的定向用例，覆盖菜单布局与现有压缩、清理、批量重试链路。
- human_tests：更新 `human_tests/asr-scheduled-task-plan-b.md`，创建窄窗口、亮暗主题和操作菜单回归用例后立即执行。
- Rust 构建、Rust workspace 测试和 coverage changed-lines：本次没有生产 Rust 变更，按用户要求不执行。
