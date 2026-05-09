# WebUI Layout Navigation

## 背景

WebUI 左侧一级导航在窗口高度较小时会超过可视区域。当前导航项和底部主题切换按钮处于同一列，导航区没有独立滚动能力，导致靠下的 Tab 可能不可见或被状态栏挤压。

## 实现方案

1. `web/src/components/Layout/index.tsx` 的 sidebar 保持固定宽度和整高布局。
2. 在 sidebar 内新增导航滚动容器：
   - `flex: 1`
   - `minHeight: 0`
   - `overflowY: auto`
   - `overflowX: hidden`
3. 每个一级导航 tab item 设置稳定最小高度：
   - `minHeight: 64`
   - `flexShrink: 0`
4. 底部主题切换按钮不放入滚动容器，保持固定在 sidebar 底部。
5. 颜色继续使用 Ant Design token 或现有主题变量，保持亮色和暗色主题兼容。

## 测试方案

- E2E：使用较矮 viewport 打开 WebUI，确认 sidebar 导航容器 `scrollHeight > clientHeight`，向下滚动后 Settings/Notify 等靠下导航项仍可点击。
- human_tests：更新 `human_tests/webui-layout-navigation.md`，新增侧边栏滚动用例，覆盖小窗口、最小 tab 高度、底部主题按钮固定和亮/暗主题可读性。
- 最终执行 rust-project-validate 要求的 fmt、clippy、测试与 workspace all-features 测试。

## 文档影响

- 更新 `human_tests/webui-layout-navigation.md` 和 `human_tests/readme.md`。
- 不涉及 README 用户命令或 API 文档更新。

## 版本弹窗 Upgrade Command 复制回归

### 问题

版本更新弹窗中的 `Copy` 按钮复用了公共 `copyToClipboard()`，但没有检查返回的布尔结果。公共复制工具在失败时返回 `false`，不会抛异常，导致弹窗可能在实际未写入剪贴板时仍提示 `Command copied to clipboard`。

公共复制工具的同步 fallback 也不能只信任 `document.execCommand('copy')` 的返回值；嵌入式浏览器里可能出现 API 返回成功但系统剪贴板为空的假阳性。

### 实现方案

1. `web/src/utils/clipboard.ts` 保持桌面壳原生 `write_clipboard` 优先。
2. Web fallback 使用真实 `copy` 事件写入 `event.clipboardData.setData('text/plain', text)`。
3. 只有 `execCommand('copy')` 返回成功且 copy 事件实际写入数据时，才返回 `true`。
4. 版本弹窗 `handleCopyCommand` 按 Rules/Replay 等模块的模式检查 `copyToClipboard()` 返回值，成功才显示成功提示，否则显示失败提示。

### 验证计划

- 单元测试：`web/src/utils/clipboard.test.ts` 覆盖同步 copy 事件写入、`execCommand` 假成功、async Clipboard API fallback。
- E2E 测试：`web/tests/ui/admin-settings.spec.ts` 覆盖版本更新弹窗点击 `Copy` 后浏览器剪贴板内容为 `bifrost upgrade`。
- 真实场景测试：更新并执行 `human_tests/webui-layout-navigation.md` 中版本弹窗 Copy 回归用例，验证真实剪贴板内容。
