# WebUI Scripts 头部动作菜单

## 背景

Scripts 页面左侧列表 header 原来同时展示 Sandbox Settings、New Request、New Response、New Decode、Export All、Import 多个图标按钮。按钮堆叠拥挤，且随着运行时脚本类型扩展到 `parser`（bp parser 运行时），旧的"三个新建按钮"入口无法覆盖第四类脚本。

设计目标是把 header 收敛为 `+`（创建）与 `...`（更多操作）两个 Dropdown，覆盖当前实际支持的四种脚本类型（Request/Response/Decode/Parser），并保留导入/导出/沙盒设置等非创建操作。所有变更必须兼容亮色和暗色主题，颜色使用 Ant Design token，不引入硬编码颜色，且保留稳定的 `data-testid` 供 Playwright 用例断言。

## 用户目标验证清单

### 必须实现

- 左侧列表 header 只显示两个入口按钮：`scripts-create-menu-button` 与 `scripts-more-menu-button`。
- 点击 `+` 按钮弹出创建下拉菜单，含四项：Request Script、Response Script、Decode Script、Parser Script。
- 点击 `...` 按钮弹出更多操作菜单，含三项：Sandbox Settings、Export All、Import。
- 创建菜单每一项都通过 `createNewScript(type)` store action 打开一个空模板脚本，模板文件里包含对应 type tag（req/res/decode/parser）。
- Import 项复用 `ImportBifrostButton`，通过隐藏 `hiddenImportRef` 触发原生文件选择器，避免复制 `.bifrost` 文件类型检测逻辑。
- Export All 项在没有任何脚本时保持 disabled；有脚本时可用，导出多脚本 `.bifrost` 包。
- Sandbox Settings 项点击后弹出现有 Sandbox Modal（file/net/limits 三段配置），提交时调用 `updateSandboxConfig`。
- 每个入口和菜单项都带稳定 `data-testid`，供 Playwright 用例复用。
- 亮色主题（`data-theme="light"`）和暗色主题（`data-theme="dark"`）下 header 视觉一致，没有硬编码颜色。

### 必须不破坏

- Scripts 列表本身：搜索、右键菜单、重命名、删除、拖拽排序、单击选中、方向键上下切换选中项行为保持。
- Push 同步、协同编辑广播不受 header 重构影响。
- Monaco 编辑器 extra lib 类型提示（含 `parser` 类型的 ctx/request/response 定义）保留。
- 现有 `ImportBifrostButton` 交互（多选、bifrost 文件类型校验、导入 toast）不重复实现。

### 必须真实验证

- 真实浏览器打开 Scripts 页面，亮色/暗色两次断言 header 只有两个按钮，旧的 `scripts-new-request/response/decode-button` 已经不存在。
- 打开创建菜单验证四项存在并可点击各自触发一次新脚本创建。
- 打开更多菜单验证三项存在，点击 Import 能触发系统 filechooser 事件。
- 创建脚本后 Export All 从 disabled 变 enabled，导出得到含新脚本的 `.bifrost`。

## 产品语义

### 两级入口按钮

`+` 按钮承载"新增"语义。所有 script 创建（含未来可能扩展的类型）都收敛到该菜单，避免 header 再次膨胀。当前四类脚本对应的运行时 hook 点：

- Request Script：在 request 阶段执行，`ctx.request` 可改；模板 tag `REQ`。
- Response Script：在 response 阶段执行，`ctx.response` 可改；模板 tag `RES`。
- Decode Script：body 解码阶段执行，用于自定义解码器；模板 tag `DEC`。
- Parser Script：bp parser 运行时使用，处理二进制/自定义协议 request/response；模板 tag `PAR`。

`...` 按钮承载"非新增操作"语义，当前三项：

- Sandbox Settings：进程内 Sandbox 的 file/net/limits 三段配置，改动需要一次 `updateSandboxConfig` 回写。
- Export All：把当前所有脚本打成一个 `.bifrost` 包。
- Import：唤起 `ImportBifrostButton` 的隐藏 file input，支持多选。

菜单项顺序固定，不随脚本数量或 sandbox 状态动态重排，保证肌肉记忆稳定。

### data-testid 规范

前端和 Playwright 通过下列稳定 testid 交互：

- 按钮：`scripts-create-menu-button`、`scripts-more-menu-button`
- 创建菜单项：`scripts-create-request-item`、`scripts-create-response-item`、`scripts-create-decode-item`、`scripts-create-parser-item`
- 更多菜单项：`scripts-more-sandbox-item`、`scripts-more-export-all-item`、`scripts-more-import-item`

Playwright 用例只允许通过 testid 定位，不允许通过按钮文本，避免国际化改动破坏用例。

## 技术细节

### 前端实现

`web/src/pages/Scripts/index.tsx` 中 header 由两个 `antd` `Dropdown` 组成：

```tsx
const createMenuItems = useMemo<MenuProps["items"]>(
  () => [
    { key: "request",  label: <span data-testid="scripts-create-request-item">Request Script</span>,  onClick: () => onNewScript("request")  },
    { key: "response", label: <span data-testid="scripts-create-response-item">Response Script</span>, onClick: () => onNewScript("response") },
    { key: "decode",   label: <span data-testid="scripts-create-decode-item">Decode Script</span>,     onClick: () => onNewScript("decode")   },
    { key: "parser",   label: <span data-testid="scripts-create-parser-item">Parser Script</span>,     onClick: () => onNewScript("parser")   },
  ],
  [onNewScript],
);
```

`moreMenuItems` 结构类似，其中 `Import` 通过 `hiddenImportRef.current?.querySelector("button")?.click()` 触发隐藏的 `ImportBifrostButton`。`Export All` 的 `disabled` 由 `hasScripts` 决定。

`onNewScript(type)` 桥接到 store `createNewScript(type)`，仍然复用原本 Scripts store 的模板生成逻辑（decode 与 parser 共享 parser 输出上下文的 Monaco extra lib）。

### 隐藏 ImportBifrostButton

Import 项没有直接调 `openFileDialog`，而是保留一个不可见的 `<ImportBifrostButton>`，通过 CSS class `styles.hiddenImport` 隐藏但保留在 DOM 中，`hiddenImportRef` 触发内部 `<button>` 的原生 click。这样保留了 `.bifrost` 文件类型校验、多文件校验和导入 toast，全部无需在 header 里重复实现。

### 主题兼容

Scripts header 不再使用任何 hex 颜色。图标、hover 背景、Dropdown 弹层背景都走 `antd` v5 的 token（`token.colorText`, `token.colorFillSecondary`, `token.colorBgElevated` 等）。因此 `data-theme="light"` / `data-theme="dark"` 切换后视觉自动跟随。

## CLI + Web + Admin API

- Web：`web/src/pages/Scripts/index.tsx` header 内两个 Dropdown。
- Admin API：不新增 endpoint。创建/导入/导出/Sandbox settings 分别复用：
  - `POST /api/scripts` 创建。
  - `POST /api/scripts/import` + `GET /api/scripts/export` 走 `ImportBifrostButton` 与 `exportAllScripts`。
  - `GET/POST /api/sandbox/config`（`web/src/api/config.ts` 的 `getSandboxConfig` / `updateSandboxConfig`）。
- CLI：本次重构不涉及 CLI；`bifrost script` 系列命令的模板/导入/导出与 Web 共享底层 store，无需同步修改。

## Sync 边界

本次改动纯前端 header，不修改任何脚本存储字段，不影响多设备 Sync：脚本导入/导出仍然复用 `.bifrost` 打包格式，Sandbox 配置仍走 Admin API 而不参与 rule/group sync。

## Phase 拆分

### Phase 1：header 结构收敛

- 移除旧的 `scripts-new-request-button` 等硬按钮。
- 新增 `createMenuItems` / `moreMenuItems` 两个 Dropdown，接入现有 `onNewScript` / `onExportAll` / `onOpenSandboxSettings` / `hiddenImportRef` 触发方式。
- 保留稳定 testid。

### Phase 2：Parser 类型补齐

- 创建菜单加入 Parser Script 项。
- 列表 tag 增加 `PAR`。
- Monaco extra lib 里 decode/parser 共享 parser 输出上下文类型定义。

### Phase 3：主题与视觉

- 移除硬编码颜色，改用 antd token。
- 亮色/暗色两次视觉走查。

### Phase 4：Playwright 与 human_tests

- `web/tests/ui/admin-scripts.spec.ts` 用例断言两个入口、四类创建、三类更多操作、Import filechooser 触发。
- `human_tests/webui-scripts.md` 新增/更新对应真实浏览器步骤。

## 测试方案

### Playwright（`web/tests/ui/admin-scripts.spec.ts`）

- `Scripts 左侧头部在亮色和暗色主题下只保留创建菜单与更多菜单`（第 29 行）：断言 light + dark 主题下 header 只有两个按钮，旧按钮 count = 0；打开创建菜单四项可见；打开更多菜单三项可见；点击 Import 触发 `filechooser` 事件且 `isMultiple()` 为 true。
- `Scripts parser 编辑器识别 bp parser 运行时 ctx/request/response 字段`（第 74 行）：验证 Parser Script 模板的 Monaco extra lib 类型提示。
- `Scripts 页面完成创建、测试、push 同步，并让请求脚本真实作用于代理流量`（第 143 行）：从 header 创建 Request Script，写入代码，push 同步并对真实代理请求生效；同时打开更多菜单验证三项可见并 Escape 关闭。
- `Scripts 列表在获得焦点后支持上下键切换选中项`（第 290 行）：保证 header 重构不破坏列表键盘导航。

### human_tests（`human_tests/webui-scripts.md`）

- 使用临时 `BIFROST_DATA_DIR`、非 9900 端口、`--no-system-proxy` 启动真实 Bifrost。
- 打开 Scripts 页面，切换亮/暗主题各验证一次 header 只有两个入口。
- 依次点击创建菜单四项，观察四类脚本模板正确落地，且列表 tag 分别为 REQ/RES/DEC/PAR。
- 打开更多菜单点击 Sandbox Settings，改一次 file `max_bytes` 保存成功；点击 Export All 得到 `.bifrost`；点击 Import 选择该文件二次导入无冲突。

### 静态检查

- `pnpm --dir web lint`
- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`

## Review/Fix/Test 闭环

### 第 1 轮

- 复核用户目标：两个入口、四类创建、三类更多、亮/暗主题、testid 稳定、无硬编码颜色。
- 复核 diff：header dropdown 结构、Parser 项接入、hiddenImportRef 触发、`hasScripts` 控制 Export All disabled。
- 复跑：`admin-scripts.spec.ts` 中 header 相关 3 个用例。

### 第 2 轮

- 复核 `git status --short` / `git diff` 无残留旧按钮。
- 主题 token 覆盖：确认没有 `#` 前缀色值残留。
- 复跑 Playwright 全量 Scripts 用例，检查列表键盘导航、Push 同步用例回归。

## 风险与决策点

- 隐藏 `ImportBifrostButton` 触发方式依赖内部 `<button>` DOM 结构：若上游组件把 button 换成非 button 元素，`hiddenImportRef.current?.querySelector("button")?.click()` 会失效。建议在 Playwright 用例中保留 filechooser 断言以尽早发现。
- Parser 脚本类型属于 bp parser 运行时新形态，Monaco extra lib 若未同步扩展 parser 输出上下文，用户会看到 `any`。因此 Parser 项与 Monaco 扩展要一起上线，避免入口暴露而类型不全。
- Dropdown 项数量未来若继续增加（如 Runtime Log Viewer），需要考虑改为 SubMenu 或搜索型 command palette，而不是继续在两个 Dropdown 中塞入 5+ 项。
