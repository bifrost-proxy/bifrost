# WebUI Scripts actions menu

## 背景

Scripts 页面左侧列表顶部原来同时展示 Sandbox Settings、New Request、New Response、New Decode、Export All、Import 多个图标按钮。按钮数量过多，且创建入口没有覆盖已经支持的 `parser` 脚本类型。

## 目标

- 左侧列表 header 保持紧凑，只保留两个主要入口：
  - `+`：创建脚本下拉菜单。
  - `...`：更多操作下拉菜单。
- `+` 创建菜单必须覆盖四类脚本：
  - Request Script
  - Response Script
  - Decode Script
  - Parser Script
- `...` 更多菜单承载非创建操作：
  - Sandbox Settings
  - Export All
  - Import
- 保留现有脚本列表、右键菜单、重命名、删除、导入导出和搜索行为。
- 新增/修改 UI 必须兼容亮色和暗色主题，新增颜色使用 Ant Design token 或 CSS 变量，不新增硬编码颜色。

## 实现方案

1. 在 `web/src/pages/Scripts/index.tsx` 中把 header action 拆分为两个 `Dropdown`：
   - `createMenuItems` 通过 `onNewScript(type)` 调用现有 `createNewScript` store action。
   - `moreMenuItems` 调用现有 `onOpenSandboxSettings`、`onExportAll`，并通过隐藏 `ImportBifrostButton` 的 click 触发导入文件选择。
2. 为 `ScriptType = 'parser'` 补齐 UI 展示：
   - 创建菜单展示 Parser Script。
   - 列表标签展示 `PAR`。
   - 编辑器 type tag 和 Monaco extra lib 走 decode/parser 共用的 parser 输出上下文定义。
3. 给新顶部按钮增加稳定 `data-testid`：
   - `scripts-create-menu-button`
   - `scripts-more-menu-button`
   - `scripts-create-request-item`
   - `scripts-create-response-item`
   - `scripts-create-decode-item`
   - `scripts-create-parser-item`
   - `scripts-more-sandbox-item`
   - `scripts-more-export-all-item`
   - `scripts-more-import-item`
4. 导入仍复用 `ImportBifrostButton` 的现有校验与导入逻辑，避免复制 bifrost 文件类型检测逻辑。

## 测试方案

- 单元/静态检查：
  - `cd web && pnpm lint` 或项目现有 Web 检查命令。
  - `cargo fmt --all -- --check`。
  - `cargo clippy --workspace --all-targets --all-features -- -D warnings`。
  - `cargo test --workspace --all-features`。
- E2E/真实场景：
  - 在 `human_tests/webui-scripts.md` 新增/更新 Scripts header 菜单用例。
  - 使用临时 `BIFROST_DATA_DIR` 和非 9900 端口启动 Bifrost，必须携带 `--no-system-proxy`。
  - 在真实浏览器中打开 Scripts 页面，验证亮色/暗色主题下 header 只显示两个入口。
  - 点击 `+` 下拉，确认四类创建项存在，并逐项至少触发到对应新脚本模板/类型标签。
  - 点击 `...` 下拉，确认 Sandbox Settings、Export All、Import 三类操作可发现；无脚本时 Export All 为禁用状态，有脚本后可用。

## 文档更新

- 更新 `human_tests/webui-scripts.md` 和 `human_tests/readme.md`。
- 如后续用户手册描述 Scripts 页面操作入口，需要同步 `docs/scripts.md`。
