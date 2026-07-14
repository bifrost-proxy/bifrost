# WebKit 左侧导航兼容方案

## 背景与问题

Bifrost Web UI 的一级导航栏固定为 50px，内部导航项需要在有限宽度内稳定展示图标和 9px 英文标签。当前导航滚动容器同时使用 `overflow-y: auto`、`overflow-x: hidden` 与 `scrollbar-gutter: stable`。

Chromium 在 macOS overlay scrollbar 环境下不会为该容器持续占用横向空间；Safari/WebKit 会为 `stable` gutter 保留滚动条宽度，使滚动容器的实际内容宽度小于 50px。固定 50px 的导航项在缩窄后的 flex 容器内居中，再被父容器的 `overflow-x: hidden` 裁切，最终表现为 `Network`、`DevTools`、`Settings` 等标签右侧缺字。Tauri macOS 使用系统 WebView，因此会复现同一问题。

## 用户目标验证清单

### 必须实现

- Safari 中左侧导航的所有可见图标和标签完整显示，不再横向裁切。
- Tauri macOS WebView 中使用同一布局契约，不再复现 Safari 的裁切。
- 小窗口下仍允许导航列表纵向滚动，底部 OpenAPI 与主题切换保持固定。

### 必须不破坏

- 侧栏继续保持 50px，不通过扩大导航栏改变现有高密度工作台布局。
- Chromium 中导航位置、点击区域、激活背景和路由行为不变。
- 亮色、暗色主题继续使用现有 Ant Design token，修复不引入硬编码主题颜色。
- macOS/Windows 桌面自定义 chrome 的顶部 spacer、拖拽区域和窗口按钮边界不变。

### 必须真实验证

- Playwright WebKit 与 Chromium 都逐项验证导航标签、图标、OpenAPI、主题按钮位于 50px 侧栏横向边界内。
- 在小高度 viewport 触发纵向溢出后，WebKit 仍可滚动到 Settings，且横向边界不退化。
- 亮色和暗色主题都执行横向边界断言。
- 使用 Safari 打开真实隔离服务，并使用 macOS Tauri App 复查用户截图中的导航区域。

### 必须交付

- 更新布局实现、单元契约测试、Playwright E2E 与 `human_tests/webui-layout-navigation.md`。
- 完成两轮 Review/Fix/Test、项目终检、提交、PR 与远端 CI（含 coverage 门禁）看护。

## 设计约束

- 遵循根目录 `DESIGN.md`：导航保持紧凑、稳定、可扫描，激活状态同时保留颜色与背景。
- 使用现有 `token.colorTextSecondary`、`token.colorPrimary`、`token.colorPrimaryBg`，不增加新的视觉 token。
- 纵向滚动条不应改变固定窄栏的内容宽度；横向裁切只用于阻止内容越过侧栏边界，不能裁掉本应位于侧栏内的内容。

## 实现方案

1. 把侧栏宽度、滚动容器和导航项的几何样式抽为可单元测试的布局契约。
2. 将滚动容器的 `scrollbar-gutter` 从 `stable` 改为 `auto`：
   - 无滚动条时不预留永久 gutter；
   - 出现滚动条时遵循平台原生 overlay/非 overlay 行为；
   - 不支持该属性的旧 WebKit 会忽略声明并使用等价初始行为。
3. 窄侧栏不展示原生滚动条轨道：标准属性使用 `scrollbar-width: none`，WebKit 通过局部 `::-webkit-scrollbar` 将宽高设为 0；滚轮、触控板、键盘与程序化滚动能力不变。这样即使平台使用非 overlay 滚动条，也不会再次压缩 49px 的侧栏内部可用宽度。
4. 保留 `overflow-y: auto` 与每项 64px 最小高度，确保小窗口纵向滚动能力不回归。
5. 为导航图标和标签补稳定的 `data-testid`，Playwright 直接检查元素几何边界，不使用截图像素猜测作为唯一断言。

不采用以下方案：

- 扩大侧栏：会改变 Chrome 和桌面端整体工作台密度，且没有消除 scrollbar gutter 的根因。
- 缩小字体或压缩文案：会降低可读性，只掩盖较长标签的裁切。
- 取消 `overflow-x: hidden`：会让标签越过侧栏覆盖内容区，破坏固定导航边界。

## 测试方案

### 单元测试

- 验证侧栏宽度与导航项宽度均为 50px。
- 验证滚动容器保持 `overflow-y: auto`、`overflow-x: hidden`，且 `scrollbar-gutter` 为 `auto`，防止后续重新引入稳定 gutter。
- 验证导航项保持 64px 高度和禁止 flex shrink。

### E2E

- 新增 `web/tests/ui/sidebar-webkit-compatibility.spec.ts`。
- 将该用例纳入 `scripts/ci/run-ui-critical.sh`，使 PR 的 Chromium critical matrix 必跑几何与滚动回归。
- 以 Chromium 和 WebKit 分别执行：
  - 桌面高度下检查所有可见标签/图标完整落在侧栏边界内；
  - 360px 高度下确认导航容器可纵向滚动，滚动后的 Settings 可见且可点击；
  - 亮色和暗色主题各执行一次几何断言；
  - OpenAPI 与主题按钮保持在侧栏边界内。

### 真实场景测试

- 在 `human_tests/webui-layout-navigation.md` 新增 Safari、Tauri macOS 与 Chrome 对照回归用例。
- 使用独立 `BIFROST_DATA_DIR` 和 `--no-system-proxy` 启动最新服务；先确认监听进程与 Admin API ready，再在 Safari/Chrome/Tauri 中执行。

## Review/Fix/Test 闭环

### 第 1 轮

- 对照用户截图、本文目标清单和当前 diff，检查 WebKit 根因是否被消除而非仅调整文案。
- 检查 50px 侧栏、64px 导航项、纵向滚动、桌面拖拽区、OpenAPI 与主题按钮边界。
- 运行单元测试、Chromium/WebKit E2E 和新增 human_tests；发现问题立即修复并复跑。

### 第 2 轮

- 基于第 1 轮修复后的最新 diff，复查亮/暗主题、正常/窄高度、浏览器/桌面壳四组边界。
- 再次执行 `git status --short`、`git diff` 和必要的 `git diff --cached`。
- 复跑受影响测试，并确认文档、索引和实现一致；如仍发现问题则追加后续轮次。

## Coverage 与项目终检

- 本次修改 TypeScript/React 业务代码，远端 CI 必须通过 `bash scripts/ci/coverage-all.sh --json --gate` 及各 crate 棘轮阈值；默认不在本地执行高成本全量 coverage。
- E2E 完成后按 `rust-project-validate` 执行 Rust fmt、desktop fmt、clippy、受影响构建与 `cargo test --workspace --all-features`，并按变更范围决定是否运行 local-ci。
