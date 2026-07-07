# Rule Effectiveness Highlights

## 用户目标验证清单

### 必须实现

- 在 Activity 的 Merged Rules 详情中自动标注真正生效、部分生效和被覆盖的规则行。
- 在全局 Rules 胶囊的 Merged Rules 详情中复用同一套标注逻辑。
- 鼠标悬浮生效规则时解释为什么生效；悬浮不生效规则时解释被哪一行覆盖或替换。
- 判断逻辑必须贴近 Bifrost 当前规则解析特性：matcher priority、非 multi-match 协议首条获胜、multi-match 协议并存、`reqHeaders` 后写同名 header 覆盖前写 header。

### 必须不破坏

- 不改变后端规则解析、代理运行时或 active summary API 语义。
- 不影响 Merged Rules 一键复制和选区复制。
- 不改变全局 Rules 胶囊的拖拽、刷新回原位、详情展开和 Rules 深链跳转行为。
- 亮色和暗色主题下都保持可读，长规则行不因为标注而挤压文本。

### 必须真实验证

- 用单元测试覆盖同 matcher / 同协议覆盖、`reqHeaders` 同名 header 后写覆盖、不同协议共存、注释/空行中立。
- 用纯前端 Playwright smoke 覆盖胶囊 Merged Rules 展开、行级高亮和 hover 解释。
- 更新并执行 `human_tests/webui-rules.md` 与 `human_tests/admin-activity-tab.md` 中对应真实场景用例。

## 设计方案

本次不修改后端 API。Activity 和 Rules 胶囊当前都只消费 `merged_content`，因此 WebUI 新增共享的 `RuleEffectivenessCode` 组件，在渲染 merged rules 时做轻量解释分析。

分析范围是“静态可证明”的覆盖关系，而不是对任意未来请求做完整命中模拟：

- 对同 matcher、同 protocol 的非 multi-match 规则，按估算 matcher priority 和解析顺序选出获胜行，后续同类行标记为 covered。
- 对 multi-match 协议默认保留为 effective，因为 resolver 会允许它们并存。
- 对 `reqHeaders` 追加 field-level 判断：同 matcher 下相同 header name 后续再次写入时，前面的 header 变为部分生效或 covered。
- 对 forwarding 决策类协议（`http` / `https` / `ws` / `wss` / `host` / `xhost` / `passthrough`），同 matcher 下第一个已选中的 forwarding 决策获胜，后续 forwarding 决策标记为 covered。
- 注释、空行、value block 和暂不识别的复杂语法保持 neutral，避免过度宣称。

## UI 方案

- 生效行：左侧绿色边线与柔和背景，hover 显示 resolver 选择原因。
- 部分生效行：左侧 amber 边线，hover 显示哪些操作被后续替换。
- 被覆盖行：左侧灰色边线和弱化文字，hover 显示覆盖来源行号。
- 所有规则行展示稳定行号 gutter；hover 中提到 `line N` 时，用户可以在同一代码面板内直接定位。
- 长 URL、长 JSON header 和其他不可分隔 token 必须在面板宽度内自动换行，禁止撑出 Activity 面板或 Dynamic Island 弹层。
- 所有标注使用 Ant Design token 注入 CSS 变量，保持亮暗主题一致。
- Activity 中的 active rule sets 展示在 Merged Rules 标题下方，以小字号标签从左到右、从上到下平铺。标签不使用额外圆点，单击直接跳转到对应 Rules 详情页。
- Activity 的 Temporary Ports 按端口拆成独立全宽卡片，从上到下排列。每张卡片内的 Merged Rules 随内容自然撑高，不在小框内二次滚动。

## 测试方案

- `web/src/utils/ruleEffectiveness.test.ts`
  - 注释/空行 neutral。
  - nextoncall 样例中后续同 matcher `passthrough://` 被首条 passthrough 覆盖。
  - nextoncall 样例中旧 `x-tt-env` 被后续同 matcher `reqHeaders` 替换。
  - 同 matcher 不同协议可共存。
  - 同 matcher `statusCode` duplicate 由首条获胜。
- `web/tests/ui/rules-dynamic-island-global.spec.ts`
  - 胶囊保持全局可见、拖拽、刷新回原位、跳转契约。
  - 展开 Merged Rules 后可看到 active / covered 行。
  - hover covered `reqHeaders` 行出现替换解释。

## 边界

- 当前前端解释器只标注静态可证明的覆盖关系。涉及 include/exclude filter、skip、运行时请求 URL、响应状态或响应 header 的动态条件时，UI 不做强判定。
- matcher priority 是前端按核心规则模型同步估算；后续若后端 active summary 提供权威逐行解析元数据，应优先切换到后端字段作为数据源。
