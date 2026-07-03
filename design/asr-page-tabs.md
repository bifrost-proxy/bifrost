# ASR 页面 Tab 化交互改造

## 背景

ASR 工具页原来在 `/_bifrost/ai?aiSection=tools-asr` 首页一次性展示模型管理、声纹识别、声纹唤醒、定时任务和转写工作台。这个布局把不同频率、不同交互密度、不同权限层次的能力压在同一屏，用户进入后信息密度过高，首屏首次渲染要同时初始化模型状态卡、Speech Workbench、Directory Tasks 列表、Diarization/Voiceprint 设置和 Voice Wake 表单，造成主线程卡顿和视觉噪声。

同时深链场景（`asrTask=<task_id>` 打开任务详情、`asrFile` 打开文件详情、`asrDay` 打开某日 daily、`asrDailyReport` 打开 Daily Agent 报告）继续存在，任何 Tab 化改造都不能破坏这些既有 URL。

本次改造把 ASR 首页拆成顶部三个 Tab，只在激活 Tab 下挂载对应子模块，压低首屏噪声与初始化开销，同时保留全部深链和亮暗主题兼容。

## 用户目标验证清单

### 必须实现

- ASR 首页顶部渲染三个 Tab：`Scheduled Tasks`（定时任务）、`ASR Management`（ASR 管理）、`Voiceprint & Wake`（声纹识别与唤醒）。
- Tab 顺序固定：Scheduled Tasks 第一，ASR Management 第二，Voiceprint & Wake 第三。
- 每个 Tab 只在激活时挂载对应子组件，切换时旧 Tab 卸载或至少停止拉取。
- URL 参数 `asrTab` 保存首页 Tab 状态，合法值集合固定，非法值降级为默认值。
- 默认 Tab 为 `scheduled`，兼容原入口 `/_bifrost/ai?aiSection=tools-asr`。
- 任务详情深链 `asrTask=<task_id>` 保持既有行为，进入独立详情页，不嵌入首页 Tab。
- `asrTaskTab`、`asrFile`、`asrDay`、`asrDailyReport` 深链继续工作。
- Playwright UI 用例、TypeScript 类型检查覆盖 Tab 切换、URL 持久化、深链穿透。
- 亮色/暗色主题下 Tab 标签、卡片背景、边框、文字均可读，不新增硬编码颜色。

### 必须不破坏

- 现有 ASR 页面所有子模块的业务能力不变：模型管理、Speech Workbench 上传/麦克风、Directory Tasks CRUD/运行/暂停/外接设备导入/删除、Diarization 录入/识别、Voiceprint 录入/识别、Voice Wake Actions 配置。
- 现有 URL 参数 `aiSection`、`asrTask`、`asrTaskTab`、`asrFile`、`asrDay`、`asrDailyReport` 保持向后兼容。
- Rust/CLI/Admin API 不受影响，本次仅调整 WebUI 布局与路由状态。
- Playwright 已有的 ASR 相关用例（麦克风计量、Voice Wake 等）继续通过。

### 必须真实验证

- 真实浏览器打开三个 Tab，验证内容与 URL `asrTab` 同步。
- 刷新 `asrTab=voice` 后 Tab 仍为声纹识别与唤醒。
- 从 Home 或其他 Tools 导航跳入 ASR 时进入默认 `scheduled` Tab。
- 任务详情深链 `asrTask=<id>` 直接进入详情页，不显示三 Tab 结构。
- 亮色/暗色主题人工目检 Tab 头、Card 背景、Tag/Button 可读。

## 产品语义

### Tab 归属

| Tab | key | 内容 | 数据源 |
| --- | --- | --- | --- |
| Scheduled Tasks | `scheduled` | ASR Directory Task 列表、创建/编辑弹窗、Run/Pause/Resume 按钮、外接设备 Import 按钮、删除危险确认 | `DirectoryTasksPanel` + Directory Tasks API |
| ASR Management | `management` | 模型初始化/下载进度/状态、Speech Workbench（上传文件+麦克风+离线转写） | `SpeechTab` 模型管理板块 + `SpeechWorkbench` |
| Voiceprint & Wake | `voice` | Speaker Diarization 配置、声纹录入/识别、Voice Wake Actions 触发规则 | `DiarizationSetupCard` + `VoiceWakeActionsCard` |

任务详情页不属于任何 Tab，通过 `asrTask` 深链进入独立视图。任务详情内部的 Tab（`asrTaskTab`）与首页 `asrTab` 独立，命名空间不冲突。

### Tab 标签文案

首版 Tab 标签使用英文常量（`Scheduled Tasks` / `ASR Management` / `Voiceprint & Wake`），与组件常量一致。Playwright 已有中文标题用例（`定时任务` / `ASR 管理` / `声纹识别与唤醒`）需要在本次改造中同步调整为英文断言，或组件同步提供中文标题；两种方案二选一后必须在 diff 中保持一致，不允许组件用英文、Playwright 断中文导致误绿或误红。

### 默认 Tab 与 URL 语义

- 首次访问 `/_bifrost/ai?aiSection=tools-asr` 未带 `asrTab` 时进入默认 Tab。
- 用户点击 Tab 立即写入 `asrTab=<key>`，使用 `history.replaceState` 或路由库的 `replace` 语义，避免每次切换 Tab 都新增 history entry。
- 从任务详情返回首页时，`asrTab` 保持上一次 Tab；如果 URL 中没有 `asrTab`，回退到默认。
- 非法值（例如 `asrTab=foo`）视为无效，前端降级为默认 `scheduled` 并写回合法值。

### 深链兼容

- `asrTask=<task_id>` 出现时页面直接进入任务详情页，跳过三 Tab 首屏，避免闪烁。
- `asrTaskTab` 只在任务详情页内生效。
- `asrFile`、`asrDay`、`asrDailyReport` 只在任务详情下有意义，不影响 `asrTab`。

## 技术细节

### 组件与文件

- 新增 `web/src/pages/ASR/components/ASRHomeTabs.tsx`：接受当前 `asrTab`、setter 与三个已挂载 slot；负责渲染 Ant Design `Tabs`、只挂载激活 Tab 的 children。
- 新增 `web/src/pages/ASR/components/asrHomeRoute.ts`：导出 `ASR_HOME_TAB_KEYS = ["scheduled", "management", "voice"] as const`、类型守卫 `isAsrHomeTab(x: unknown): x is AsrHomeTab`、URL 读写函数 `readAsrHomeTab(search)` / `writeAsrHomeTab(next)`。
- 修改 `web/src/pages/ASR/index.tsx`：
  - 判断是否处于任务详情（`asrTask` 存在），若是则渲染既有详情视图，否则渲染 `ASRHomeTabs`。
  - 从 URL 读入 `asrTab`，非法值降级并回写。
  - 把 `DirectoryTasksPanel`、`SpeechTab`（模型管理板块）+ `SpeechWorkbench`、`DiarizationSetupCard`+`VoiceWakeActionsCard` 分别塞到三个 slot。
- 复用 `web/src/pages/Settings/tabs/SpeechTab.tsx` 的模型管理板块；如果耦合过深需要拆出一个 `AsrModelManagementSection` 供 ASR 页面复用，禁止跨页面直接 mount `SpeechTab` 整个 tab。

### URL 参数

- URL 结构：`/_bifrost/ai?aiSection=tools-asr&asrTab=<scheduled|management|voice>`。
- 使用与项目现有 URL 参数库一致的接口，避免混用 `URLSearchParams` 和 router hook 两套写法。
- Tab 切换只 replace 当前 entry；点击浏览器后退应回到进入 ASR 前的页面，而不是在三个 Tab 之间循环。

### 挂载策略

- Ant Design `Tabs` 使用 `destroyInactiveTabPane` 或手动条件渲染，非激活 Tab 卸载子组件。
- 卸载后子组件的 SSE/WebSocket/interval 必须在 cleanup 中释放；本次改造要 audit `SpeechWorkbench` 和 `DirectoryTasksPanel` 的 cleanup 是否满足。
- 首次进入某 Tab 时子组件重新 mount 并按现有逻辑拉取数据；不引入额外的全局缓存层。

### 主题

- Tab 头颜色、背景、下划线、hover 状态使用 Ant Design token 或现有 CSS 变量，不引入十六进制硬编码。
- 组件内部的 Card/Alert/Tag 样式沿用现有约定，与 Home、Rules、Traffic 等页面视觉一致。

## CLI + Web + Admin API 边界

- CLI：本次改造不涉及 CLI，`bifrost ai asr` 相关命令不变。
- Web：仅新增/修改 WebUI 组件与路由，无新样式主题。
- Admin API：本次改造不涉及 Admin API 变更，`/api/asr/*` 保持既有形状。

## Sync、导入导出与分享

- Rules/Values sync 与本次改造无关。
- 用户 ASR 任务、声纹库、模型下载状态由现有 API 管理，不通过 Tab 化改造引入新的持久化字段。

## 实现切分

### Phase 1：组件与路由

- 新增 `ASRHomeTabs.tsx` 与 `asrHomeRoute.ts`。
- 修改 `ASR/index.tsx` 使用新组件，接入 URL 参数。
- 保证非任务详情场景下三 Tab 正常渲染。

### Phase 2：兼容与拆分

- 若 `SpeechTab` 无法直接复用则抽出 `AsrModelManagementSection`。
- 校验 Tab 切换后旧 Tab cleanup（SSE、WS、interval）无泄漏。
- 处理 `asrTab` 非法值兜底和 `asrTask` 深链穿透。

### Phase 3：测试与主题

- 新增 Playwright 用例 `web/tests/ui/asr-home-tabs.spec.ts`。
- 补齐 `tsc -b` 类型检查。
- 亮色/暗色目检并调整 token。

### Phase 4：human_tests 与文档

- 在 `human_tests/asr-scheduled-task-plan-b.md` 增加 `TC-ASPB-42 ASR 首页三 Tab 交互改造`。
- 同步 `human_tests/readme.md` 用例数、说明。
- 更新 `design/asr-page-tabs.md`（本文档）。

## 测试方案

### 单元/类型测试

- `pnpm --dir web exec tsc -b --pretty false` 校验 `ASRHomeTabs` props、`asrHomeRoute` 类型守卫和 `ASR/index.tsx` 迁移。
- 若已有 vitest 覆盖 URL 读写，补一个 `readAsrHomeTab` 非法值降级用例；无需额外基础设施。

### E2E 测试

`web/tests/ui/asr-home-tabs.spec.ts` 覆盖：

1. 打开 `/_bifrost/ai?aiSection=tools-asr`，默认显示 Scheduled Tasks 的 Directory Tasks 列表，未看到 Speech Workbench 或 Voiceprint 标题。
2. 点击 `ASR Management`，URL 出现 `asrTab=management`，Speech Workbench 与模型管理卡片渲染，Scheduled Tasks 卸载。
3. 点击 `Voiceprint & Wake`，URL 出现 `asrTab=voice`，声纹与 Voice Wake 卡片渲染。
4. 刷新页面 `asrTab=voice` 保持第三个 Tab 激活。
5. 打开深链 `asrTask=<id>` 直接进入任务详情，不显示三 Tab。
6. 非法值 `asrTab=foo` 降级为 `scheduled` 并在 URL 中回写。

### 真实场景测试 human_tests

`human_tests/asr-scheduled-task-plan-b.md` 新增 `TC-ASPB-42`，覆盖：

- 首屏默认 Tab。
- 三 Tab 切换、URL 持久化、刷新恢复。
- 任务详情深链穿透。
- 亮色/暗色主题目检。
- 从 Home 侧栏 Tools 导航跳入 ASR 页面时不带 `asrTab` 参数，进入默认 Tab。

`human_tests/readme.md` 更新对应用例编号和描述。

启动 Bifrost 使用临时 `BIFROST_DATA_DIR`、非 9900 端口、`BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`、`BIFROST_DISABLE_TRAY=1`、`--no-system-proxy`。

### 覆盖率与项目校验

- `pnpm --dir web exec tsc -b --pretty false`
- `pnpm --dir web test -- asr-home-tabs`（或 Playwright 单文件运行）
- `rust-project-validate` 覆盖 Rust 部分；本次改动无 Rust 变更时可跳过 workspace 全量。
- 本机若沿用 no-local-coverage 约定，则不跑 `make coverage`；交付时说明依赖 CI。

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核用户目标：三 Tab、顺序、默认值、URL 参数、深链兼容。
- 复核 diff：新增文件、`ASR/index.tsx` 修改点、Playwright 用例、human_tests。
- 重点 review：Tab 切换是否卸载子组件、SSE/WS cleanup 是否补齐、`asrTaskTab` 与 `asrTab` 是否解耦。
- 复测：`tsc -b`、`asr-home-tabs.spec.ts`、已有 ASR 相关 Playwright。

### 第 2 轮

- 复核第 1 轮修复后的 diff。
- 重点 review：亮色/暗色主题、非法值降级、`aiSection=tools-asr` 之外的入口（如 Home 卡片点击）是否仍能进入 ASR 页面。
- 复测：真实浏览器目检 + Playwright 复跑。

## 风险与决策点

- Tab 标签中英文一致性：文档、组件常量、Playwright 断言必须统一，否则出现"组件用英文、E2E 断中文"的假象；建议本次统一保留英文，将来单独做 i18n 处理。
- `SpeechTab` 复用边界：模型管理板块目前和 Settings 页面共用同一组件；如果需要拆出 `AsrModelManagementSection`，要同步保证 Settings 端仍能正常渲染。
- 深链穿透：`asrTask` 存在时跳过三 Tab；未来若增加"首页 Tab + 详情侧栏"混合布局，需要重新设计路由，本文档只承诺当前行为。
- Playwright 中文断言修改：如果他人正在同时改这批用例，可能出现 rebase 冲突；建议本次 diff 包含用例改动，避免拆分成两个 PR 时出现"组件英文、E2E 中文"的过渡态。
- 组件 lazy load：如需按 Tab 做 code split，本版本不做，避免打包/预加载策略引入额外测试复杂度；后续独立优化。
