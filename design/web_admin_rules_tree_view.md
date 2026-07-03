# Web Admin Rules 列表树状视图（按 `/` 分组）

## 背景

Web Admin 的 Rules 页面左侧列表最初为扁平列表。当用户按 `A/B/C` 这种带 `/` 的规则名管理规则时，扁平列表滚动长、层级不清晰、批量识别所属模块困难。

Bifrost 从 rules 树状视图落地起，把规则名按 `/` 拆分为“文件夹 + 叶子节点”的树形结构在左侧展示，同时保留原有的选中、上下键导航、搜索、启停、右键菜单、新建、刷新、拖拽等交互。规则名本身仍以 flat string 存储，不新增 rename/move 语义。

## 用户目标验证清单

### 必须实现

- 规则名按 `/` 分段：`teamA/serviceA/login` 展示为文件夹 `teamA` → 文件夹 `serviceA` → 叶子 `login`。
- 空 segment 归一化：`A//B` 视为 `A/B`。
- 每个文件夹行可点击折叠/展开。
- 选中某个叶子时自动展开所有父路径，确保叶子可见。
- 输入搜索关键词时自动展开所有文件夹，避免匹配结果被折叠隐藏。
- 键盘上下键仍能在“当前可见叶子”序列内切换。
- 右键菜单（导出/删除/重命名）、启用/禁用 Switch、新建、刷新在树状结构下都能命中正确叶子。
- 首次进入 Rules 页面时默认展开所有一级/多级文件夹，避免用户第一眼看到空列表。
- 单一规则名不含 `/` 时仍作为顶层叶子渲染，行为与旧扁平列表一致。

### 必须不破坏

- Rules 列表的排序策略（sort_order、search 权重、Default 置顶）：树构建从 `filteredRules` 顺序生成，不改变叶子顺序。
- 原有 `listbox` 语义与可访问性：keyboard 导航目标改为 flatten 后的可见叶子序列。
- Group 规则、系统 Default 规则的展示与保护语义。
- Rules push 更新链路：树只在渲染层拼接，来源数据仍由 `useRulesStore` 提供。
- Rules 页面右侧 RuleEditor 的选中联动、复制、导出、分享逻辑。

### 必须真实验证

- Playwright E2E：能创建带 `/` 的多个规则，folder 行可见、可折叠/展开、折叠后子叶子不可见、搜索时自动展开。
- 手工验证：常见路径深度（1、2、3 层）都能正常渲染；`A//B` 等特殊输入不产生空 folder；键盘上下键遇到折叠 folder 时跳过其子叶子。

## 产品语义

### 树只是展示层，不是新的存储模型

- 规则名仍是 flat string，`.bifrost` 文件、Admin API、CLI 均维持不变。
- 树在前端 `RuleList` 渲染前根据 `filteredRules` 构造，等价于对现有排序结果做一次 group-by-path 的可视化包装。
- 拖拽、rename、批量移动**不改变** flat 规则名；后续如要引入“按目录移动”，需要单独设计。

### 展开状态是运行时前端状态

- 展开集合 `expandedFolders` 保存在 `RuleList` 组件 state 中；首次进入或数据刷新后默认展开全部一级 folder 路径，交由用户按需折叠。
- 搜索关键词非空时，构建后的 tree 会强制展开全部 folder；用户清空搜索后恢复到用户手动折叠/展开的状态。
- 选中叶子时，`getRuleParentPaths(rule.name)` 计算出所有父路径并加入展开集合，保证选中项一定可见。

### Default、Group 规则的展示

- 系统 `Default` 规则名不含 `/`，仍作为置顶叶子直接渲染，不进入任何 folder。
- Group 规则名如含 `/` 也按同样规则分组；folder 行本身不承载启停/编辑语义，只承担折叠。

## 前端实现

### 数据结构与构建

新增 `web/src/pages/Rules/RuleList/ruleTree.ts`（138 行），导出：

```ts
export type RuleTreeNode = RuleTreeFolderNode | RuleTreeLeafNode;

export interface RuleTreeFolderNode {
  type: 'folder';
  name: string;   // 当前 segment 显示名，如 'serviceA'
  path: string;   // 从根拼出的完整路径，如 'teamA/serviceA'
  children: RuleTreeNode[];
}

export interface RuleTreeLeafNode {
  type: 'rule';
  name: string;   // 规则完整 flat 名，如 'teamA/serviceA/login'
  label: string;  // 最后一段 segment 显示名，如 'login'
  rule: RuleFile;
}

export function splitRulePath(name: string): string[];
export function getRuleParentPaths(ruleName: string): string[];
export function buildRuleTree(rules: RuleFile[]): RuleTreeFolderNode;
export function collectFolderPaths(tree: RuleTreeFolderNode): string[];
export function getTopFolderPrefix(ruleName: string): string | null;
export function flattenVisibleRuleNames(
  tree: RuleTreeFolderNode,
  expandedFolders: Set<string>
): string[];
```

关键行为：

- `splitRulePath`：按 `/` 切分，`trim` 每段，过滤空 segment。因此 `'A//B'`、`'A/ /B'` 都会归一为 `['A','B']`。
- `buildRuleTree`：按输入 `rules` 顺序遍历，`folderIndex` map 复用同名文件夹节点，父节点 `children` 追加顺序即列表顺序。因此排序策略（sort_order / search）继续由 `filteredRules` 决定。
- `collectFolderPaths`：深度优先收集所有 folder 路径，用于“默认展开全部”的初始化。
- `flattenVisibleRuleNames`：DFS 遍历 tree，遇到 folder 若在 `expandedFolders` 集合内则递归下潜，否则跳过所有子叶子；返回值供键盘上下键导航使用。

### 列表渲染与交互

`web/src/pages/Rules/RuleList/index.tsx`（1249 行）内改动要点：

- 引入 `buildRuleTree`、`collectFolderPaths`、`flattenVisibleRuleNames`。
- `const ruleTree = useMemo(() => buildRuleTree(filteredRules), [filteredRules])`。
- `expandedFolders` 存放于 `useState<string[]>([])`，用 `useMemo` 派生 `expandedFolderSet`。
- 数据首次装载或 `allFolderPaths` 增加时，用 `allFolderPaths = collectFolderPaths(ruleTree)` 补齐默认展开状态，避免新出现的 folder 默认折叠。
- 搜索关键词非空时把展开集合替换为 `new Set(allFolderPaths)`，保证结果全部可见。
- Folder 行 dom 使用 `data-testid="rule-folder-item"`、`data-folder-expanded="true|false"`，供 Playwright 断言。
- 叶子节点保留原 `role="option"`，`aria-selected` 与旧扁平列表一致。
- 右键菜单、Switch、拖拽 handler 只挂在叶子节点上，folder 行不触发规则级操作。

### 键盘导航

- 上下键前的选中候选序列从 `filteredRules.map(r => r.name)` 改为 `flattenVisibleRuleNames(ruleTree, expandedFolderSet)`。
- 折叠 folder 时子叶子从序列中消失，键盘导航自然跳过。
- 选中一个叶子后 `getRuleParentPaths(ruleName)` 补齐父路径展开状态。

## 影响范围

- 修改：`web/src/pages/Rules/RuleList/index.tsx`、`web/src/pages/Rules/RuleList/index.module.css`。
- 新增：`web/src/pages/Rules/RuleList/ruleTree.ts`。
- 依赖类型：`web/src/types.ts` 中 `RuleFile`。
- E2E：`web/tests/ui/admin-rules-values.spec.ts` 增加“Rules 列表支持按 / 分组的树状展开/折叠”用例。
- 无后端 / Admin API / CLI 改动，无 sync/import 路径改动。

## 非目标

- 不改变规则名本身的语义或存储格式；`A/B` 仍是一个字符串规则名。
- 不新增基于文件夹的 rename/move/批量删除；folder 行只承担折叠交互。
- 不改变 Default 规则置顶、`can_delete/can_disable/can_rename/can_reorder` 语义。
- 不改变 Group 规则的展示、启停、生效链路。
- 不在树上支持拖拽跨 folder 移动叶子；后续如需，需单独设计。

## Sync / 导入 / 分享边界

- 树只在渲染层，`.bifrost` 文件、`GET /api/rules`、导入导出、Share URL 都不受影响。
- Sync 场景仍按 flat 规则名 diff；用户在多设备使用相同 `A/B/C` 名字时，前端各自渲染出等价树。

## Phase 1：数据结构与展示

- 新增 `ruleTree.ts`，覆盖 `splitRulePath / buildRuleTree / collectFolderPaths / getRuleParentPaths / getTopFolderPrefix / flattenVisibleRuleNames`。
- 手工构造 `RuleFile[]` mock 验证树结构。

## Phase 2：列表渲染与折叠

- `RuleList` 递归渲染 folder / leaf 两类节点。
- 默认展开全部 folder；用户点击 folder 行切换展开状态。
- 折叠状态与 `filteredRules` 变化解耦；新增 folder 自动补齐展开。

## Phase 3：选中、键盘导航、搜索联动

- 叶子选中时自动展开父路径。
- 上下键导航 target 改为 `flattenVisibleRuleNames`。
- 搜索关键词非空时强制展开全部 folder。

## Phase 4：E2E 与手工验证

- Playwright 用例覆盖 folder 显示、折叠、展开、搜索、上下键。
- 手工在 macOS 桌面端与浏览器端验证长/深路径命名。

## 测试方案

### 单元/组件测试

`web/` 子项目当前未接入 Vitest/Jest，本设计沿用现状：`ruleTree.ts` 的行为通过 E2E + 手工覆盖，`splitRulePath` 与 `buildRuleTree` 的分支通过实际 Playwright 场景覆盖到。

### Playwright E2E

`web/tests/ui/admin-rules-values.spec.ts` 已落地：

- UI-RULES-TREE-01（`Rules 列表支持按 / 分组的树状展开/折叠`，spec 中约 867 行起）：
  - 通过 Admin API 创建 `{folder}/a-child`、`{folder}/b-child` 与 `top`
  - 断言 `data-testid="rule-folder-item"` 中包含 folder 名的行可见
  - 通过 `data-folder-expanded` 属性判定当前展开状态，必要时点击折叠
  - 折叠后断言两个子叶子不可见，再次点击后可见
- UI-RULES-KBD-02（`Rules 列表在获得焦点后支持上下键切换选中项`，同 spec 约 912 行）：验证键盘 flatten 序列在折叠/展开变化下的正确性。

### 真实场景手工验证

- 依次创建：`teamA/serviceA/login`、`teamA/serviceA/logout`、`teamB/serviceB/*`、`top-level`。
- 验证默认展开、折叠/展开切换、点击叶子右侧 RuleEditor 联动、右键菜单（导出/删除/重命名）、启停 Switch 可用。
- 在搜索框输入 `serviceA`，验证树自动展开且过滤后仍有正确路径。
- 键盘上下键遍历，折叠 folder 时跳过其内叶子；重新展开后可继续遍历。

### 环境约束

所有服务启动必须使用临时 `BIFROST_DATA_DIR`、非 9900 端口、`BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`、`BIFROST_DISABLE_TRAY=1` 和 `--no-system-proxy`；Playwright 由 `web/tests/ui/helpers/test-env.ts` 的 `allocateUiTestEnv()` 分配端口。

### 覆盖率与项目校验

- `pnpm -C web lint`
- `pnpm -C web test:ui -- admin-rules-values.spec.ts`
- `cargo fmt --all -- --check` / `cargo test --workspace --all-features`（若涉及 Rust 改动）
- `rust-project-validate`

本机 no-local-coverage 约定不跑 `make coverage` / `make coverage-unit`；交付说明依赖远端 CI。

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核用户目标：树状渲染、默认展开、折叠、选中/搜索自动展开、键盘导航。
- 复核 diff：`ruleTree.ts` 全部导出被消费，`RuleList` 只在渲染层引用。
- 重点 review：`splitRulePath` 的空 segment 归一化、`buildRuleTree` 是否稳定保序、`collectFolderPaths` 是否遗漏嵌套 folder、`flattenVisibleRuleNames` 是否跳过折叠子树。
- 复测：Playwright `admin-rules-values.spec.ts`；手工验证长/深/含特殊字符规则名。

### 第 2 轮

- 复核第 1 轮问题修复；`git status --short` 与 `git diff` 均无遗漏。
- 重点 review：树状结构对 Default 置顶、Group 规则、批量选择、拖拽是否有副作用；搜索清空后展开状态是否正确恢复。
- 复测：失败路径重跑，必要时补 mac 桌面端 UI 手测。

## 风险与决策

- **规则名冲突**：出现 `A` 和 `A/B` 时，`A` 是顶层叶子、`A/B` 会在名为 `A` 的 folder 下。UI 将 folder 与顶层叶子并排渲染，folder 行图标区分；后续若造成困惑可考虑给 folder 加 badge。
- **深路径体验**：极深路径（≥ 6 层）会占用较多水平空间，缩进用固定像素而不是相对宽度，滚动可继续查看；如后期问题突出再引入水平滚动或路径省略。
- **默认全展开的性能**：在超大规则量下默认展开会导致 DOM 节点较多；`RuleList` 已使用虚拟化的场景下，folder 节点也需要计入高度。当前规模在 100~500 规则内表现良好。
- **折叠状态未持久化**：刷新页面后回到默认展开状态，避免与 push 更新叠加时状态漂移。若后续需要，可将折叠状态存 `localStorage`。
- **未来若上马 rename/move**：应先在 storage 层增加 `move`/`rename-under` 语义并给 Default/系统规则加保护；本设计不做。
