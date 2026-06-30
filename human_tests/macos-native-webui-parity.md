# macOS Native WebUI Parity 真实场景测试

## 功能模块说明

验证 macOS Native App 不能停留在 scaffold 或局部 Network 仿制，而必须按 `design/macos-native-webui-parity.md` 逐页对齐 WebUI 的真实能力、交互和 Admin API 数据流。

本文件分两类用例：

- 方案验收用例：当前即可执行，确认 WebUI parity 范围、信息源和门禁已落文档。
- 实现验收用例：后续每个页面完成后必须逐条执行，不能用截图替代 API 和交互验证。

## 前置条件

- 仓库根目录为当前工作目录。
- 当前分支包含 `apps/macos` Native App。
- 默认 Bifrost 服务可通过 `http://127.0.0.1:9900/_bifrost/` 打开 WebUI。
- 执行 Native UI 验证时必须启动 `.app`：
  ```bash
  open -n apps/macos/.build/Bifrost.app
  ```
  禁止启动裸 `apps/macos/.build/.../debug/Bifrost` Mach-O。

## 测试用例

### TC-MWUP-01：WebUI parity 方案覆盖全部顶层页面

**操作步骤：**
1. 执行：
   ```bash
   for key in Network Replay Rules Values Scripts AI DevTools Groups Notify Settings; do
     rg -n "$key" design/macos-native-webui-parity.md >/dev/null || exit 1
   done
   ```
2. 执行：
   ```bash
   rg -n "P0: Native Shell|P1: Network|P1: Rules / Values / Scripts|P1: Replay|P2: Settings|P2: Notifications / Groups / DevTools|P3: AI" design/macos-native-webui-parity.md
   ```

**预期结果：**
- 所有 WebUI 顶层菜单页面都能在 parity 方案中找到。
- 方案按 P0/P1/P2/P3 明确实现阶段，且没有把未实现页面写成已完成。

### TC-MWUP-02：方案明确 WebUI 源码、API 和 store 信息源

**操作步骤：**
1. 执行：
   ```bash
   for path in \
     web/src/App.tsx \
     web/src/components/Layout/index.tsx \
     web/src/components/Toolbar/index.tsx \
     web/src/components/StatusBar/index.tsx \
     web/src/api/traffic.ts \
     web/src/api/rules.ts \
     web/src/api/replay.ts \
     web/src/api/config.ts \
     web/src/services/pushService.ts; do
     test -e "$path" || exit 1
     rg -n "$path" design/macos-native-webui-parity.md >/dev/null || exit 1
   done
   ```

**预期结果：**
- 每个关键 WebUI 源码/API/store 文件都存在。
- parity 方案显式引用这些文件，后续实现必须以真实源码和接口为准。

### TC-MWUP-03：方案明确禁止假数据和裸 Mach-O 启动

**操作步骤：**
1. 执行：
   ```bash
   rg -n "不使用假数据|禁止用 sample rows|不启动裸 Mach-O|真实 \\.app" design/macos-native-webui-parity.md human_tests/macos-native-webui-parity.md
   ```

**预期结果：**
- 方案和 human test 均明确禁止 Native UI 使用假数据。
- 方案和 human test 均明确 Native 验证必须打开 `.app`，不能启动 terminal 可执行文件。

### TC-MWUP-04：Shell 与 Network 完成后必须做 WebUI/Native 对照截图

**操作步骤：**
1. 打开 WebUI：
   ```bash
   open http://127.0.0.1:9900/_bifrost/
   ```
2. 截取 WebUI Network 参考图：
   ```bash
   screencapture -x /tmp/bifrost-webui-network-default.png
   ```
3. 构建并打开 Native app：
   ```bash
   scripts/build-macos-native.sh --skip-sidecar --test
   open -n apps/macos/.build/Bifrost.app
   ```
4. 截取 Native 展开态、折叠态、暗色态：
   ```bash
   screencapture -x /tmp/bifrost-native-network-expanded.png
   screencapture -x /tmp/bifrost-native-network-collapsed.png
   screencapture -x /tmp/bifrost-native-network-dark.png
   ```

**预期结果：**
- Native 默认是左右布局，左侧菜单展开，底部主题切换可见。
- 顶部标题栏空间承载 Network 工具按钮和 Breakpoint/TLS/System Proxy 开关。
- 中间区域为 Filter panel、Traffic table、Traffic detail 三栏。
- 底部状态栏为矮状态条，并展示 Proxy/Sync/TLS/速率/资源/版本等信息。
- 截图中不出现 Dashboard scaffold、假 Traffic 行或 placeholder 文案。

### TC-MWUP-05：每个页面完成后必须通过真实 API 双向验证

**操作步骤：**
1. 对正在验收的页面执行 Native 写操作，例如创建 Rules/Values/Scripts/Replay saved request。
2. 在 WebUI 刷新对应页面，确认 Native 写入对象可见。
3. 在 WebUI 修改同一对象。
4. 返回 Native，确认 push 或 polling 后状态更新。
5. 通过 Admin API 或 CLI 查询最终状态。

**预期结果：**
- Native 与 WebUI 消费同一个默认数据目录和同一个 Admin API 服务。
- 双向写入都能在另一端可见。
- 最终状态与 Admin API/CLI 输出一致。
- 测试对象在清理步骤中删除，不污染用户长期数据。

### TC-MWUP-06：页面完成门禁必须覆盖状态、主题、窗口和错误态

**操作步骤：**
1. 对正在验收的页面分别执行：
   - 浅色主题截图。
   - 深色主题截图。
   - sidebar 展开截图。
   - sidebar 折叠截图。
   - 窄窗口截图。
   - 空态或无数据状态验证。
   - API 错误态验证。
2. 将截图保存为：
   ```text
   /tmp/bifrost-native-<page>-light.png
   /tmp/bifrost-native-<page>-dark.png
   /tmp/bifrost-native-<page>-collapsed.png
   /tmp/bifrost-native-<page>-narrow.png
   /tmp/bifrost-native-<page>-empty-or-error.png
   ```

**预期结果：**
- 所有状态下布局不重叠、不溢出、不出现无功能按钮。
- 深色主题不是局部反色，表格、编辑器、弹窗、状态栏都可读。
- sidebar 折叠后仍能定位页面、切换主题、显示 Notify 状态。
- API 错误态有明确用户反馈，并允许重试或恢复。

## 清理步骤

- 删除测试创建的 Rules、Values、Scripts、Replay saved requests、Groups 或 Remote Invoke grants。
- 关闭 Native app：
  ```bash
  pkill -f 'Bifrost.app/Contents/MacOS/Bifrost' || true
  ```
- 不停止用户已有默认 `bifrost` daemon，除非本次测试明确由当前用例启动并记录了 pid。
