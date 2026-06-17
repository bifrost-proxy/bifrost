# ASR 页面 Tab 化交互改造

## 功能模块说明

ASR 工具页原来在 `/_bifrost/ai?aiSection=tools-asr` 首页一次性展示模型管理、声纹识别、声纹唤醒、定时任务和转写工作台，用户进入后信息密度过高。本次改造把 ASR 首页拆成顶部三个 Tab：

1. `Scheduled Tasks`（定时任务）：展示 ASR Directory Task 列表、创建/编辑、运行、暂停、恢复、外接设备导入和删除。
2. `ASR Management`（ASR 管理）：展示 ASR 模型初始化/状态管理（来自 `Settings/tabs/SpeechTab`），以及文件上传、麦克风实时转写、离线转写工作台。
3. `Voiceprint & Wake`（声纹识别与唤醒）：展示 Speaker Diarization、声纹录入/识别，以及 Voice Wake Actions 配置。

> Tab 标签当前在 `ASRHomeTabs.tsx` 中为英文常量（`Scheduled Tasks` / `ASR Management` / `Voiceprint & Wake`），与早期文案 `定时任务` / `ASR 管理` / `声纹识别与唤醒` 对应；Playwright 用例仍按中文文案断言，如需统一文案应同步调整组件与用例。

任务详情页继续通过 `asrTask=<task_id>` 深链进入，不嵌在首页 Tab 内，避免破坏现有文件详情、Daily Docs 和 Daily Agent 路由。

## 实现逻辑

- 新增 `ASRHomeTabs` 组件封装首页三 Tab，只在当前激活 Tab 下挂载对应子模块，降低首页首屏噪声。
- 新增 URL 参数 `asrTab` 保存首页 Tab：合法值由 `web/src/pages/ASR/components/asrHomeRoute.ts` 中的 `ASR_HOME_TAB_KEYS = ["scheduled", "management", "voice"]` 定义；缺省值为 `scheduled`，`management` 和 `voice` 通过 URL 持久化，刷新后恢复。
- 保持旧入口 `/_bifrost/ai?aiSection=tools-asr` 默认进入 `Scheduled Tasks`，符合“第一个 tab 就是定时任务”的预期。
- 保持任务详情深链 `asrTask`、`asrTaskTab`、`asrFile`、`asrDay`、`asrDailyReport` 行为不变。
- 所有样式使用 Ant Design/Tabs 默认主题能力和现有 Card 组件，不新增硬编码颜色，继续兼容亮色/暗色主题。

## 依赖项

- WebUI：`web/src/pages/ASR/index.tsx`、`web/src/pages/ASR/components/ASRHomeTabs.tsx`、`web/src/pages/ASR/components/asrHomeRoute.ts`。
- 现有子组件：`DirectoryTasksPanel`、`SpeechWorkbench`、`DiarizationSetupCard`、`VoiceWakeActionsCard`（位于 `web/src/pages/ASR/components/`），以及从 `web/src/pages/Settings/tabs/SpeechTab.tsx` 引入的模型管理面板。
- 测试：Playwright UI 测试、TypeScript 构建检查、真实场景 `human_tests/asr-scheduled-task-plan-b.md`。

## 测试方案

### 单元/类型测试

- `pnpm --dir web exec tsc -b --pretty false` 验证新增组件 props、URL Tab 类型守卫和现有 ASR 页面类型正确。

### E2E 测试

- 新增 Playwright 用例 `web/tests/ui/asr-home-tabs.spec.ts`：
  - 打开 `/_bifrost/ai?aiSection=tools-asr` 默认显示 `定时任务`，不显示 ASR 管理和声纹模块内容。
  - 点击 `ASR 管理` 后 URL 写入 `asrTab=management`，显示模型管理和转写工作台。
  - 点击 `声纹识别与唤醒` 后 URL 写入 `asrTab=voice`，显示声纹识别和唤醒模块。
  - 刷新 `asrTab=voice` 后仍保持声纹 Tab。
  - 任务详情深链 `asrTask=<id>` 仍绕过首页 Tab 进入详情页。

### 真实场景测试

- 在 `human_tests/asr-scheduled-task-plan-b.md` 增加 `TC-ASPB-42 ASR 首页三 Tab 交互改造`。
- 同步 `human_tests/readme.md` 中该文档用例数和说明。
- 按用例真实执行，记录默认 Tab、Tab 切换、URL 持久化、详情深链和亮色/暗色主题可读性。

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核用户目标：顶部三 Tab、Tab 顺序、每个 Tab 的内容归属和深链兼容。
- 复核 diff：新增组件、ASR index 路由状态、E2E 和 human_tests。
- 执行 `pnpm --dir web exec tsc -b --pretty false` 与新增 Playwright 用例。
- 修复 TypeScript、路由状态或用例断言问题后复测。

### 第 2 轮

- 复查第 1 轮修复后的 diff，确认没有把详情页错误嵌入首页 Tab。
- 复查亮色/暗色主题不新增硬编码颜色，URL 参数不破坏 `asrTaskTab`。
- 复跑受影响 WebUI 验证和 human_tests 对应用例。

## 校验要求

- WebUI 类型检查必须通过。
- 新增 Playwright 用例必须通过。
- 本次只修改 WebUI/文档/测试，不修改 Rust 逻辑；Rust 全量测试如未执行需在最终验证矩阵说明。

## 文档更新要求

- 更新 `design/asr-page-tabs.md`。
- 更新 `human_tests/asr-scheduled-task-plan-b.md` 和 `human_tests/readme.md`。
