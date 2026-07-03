# Web ESLint Cleanup 设计方案

## 背景

`web/` 前端历史积累了一批 lint warning：DevTools 组件里从 `.tsx` 里 export 了非组件 helper 触发 `react-refresh/only-export-components`；一些 React Hooks 有依赖数组缺项与 `set-state-in-effect` 报告；有若干空 `catch` 块与未使用变量；Storage 面板的自研虚拟列表 hook 与 React Compiler 严格模式不兼容。

本模块用于约束 `web/` 目录 ESLint 清理任务。目标：让 `pnpm --dir web lint` 全量零错误、零警告通过；`pnpm --dir web build` TypeScript + Vite 打包成功。同时不降级 lint 配置、不引入新功能、不触碰 Rust 代码，也不新增运行时依赖。

真实清理涉及 `web/src/pages/DevTools/components/` 下的 `ConsolePanel.tsx`、`ElementsPanel.tsx`、`NetworkPanel.tsx`、`StoragePanel.tsx`、`shared.tsx` 以及新拆出的 `consoleValueUtils.ts`、`domUtils.ts`、`sharedUtils.ts` 三个非组件工具模块。

## 用户目标验证清单

### 必须实现

- `cd web && pnpm run lint` 零 error、零 warning，`--max-warnings=0` 语义生效。
- `cd web && pnpm run build` 成功：TypeScript 无编译错误、Vite 打包成功。
- 保留原有 React DevTools 面板可见行为：Console/Elements/Network/Storage 三个 tab 都能正常渲染并处理事件。
- Storage 面板的虚拟列表退化为普通滚动列表时，仍保留分页/交互能力，用户在实际使用中感知不到功能倒退。
- 拆出的工具模块（`consoleValueUtils.ts`、`domUtils.ts`、`sharedUtils.ts`）承接原 `.tsx` 中的 helper，避免 `react-refresh/only-export-components` 触发。

### 必须不破坏

- 不降级 `.eslintrc` 规则：不禁用 `react-hooks/exhaustive-deps`、`react-refresh/only-export-components`、`react-compiler` 相关规则。
- 不新增运行时依赖，不改 `pnpm-lock.yaml`。
- 不新增 lint disable 注释（除非上游库 bug 需临时豁免，且必须写出 issue 链接）。
- 不改 Rust 代码；后端 handler、CLI、E2E 脚本保持原状。
- Storage 面板 UX 主要保留：即使把 hook 拆掉，用户不应感觉到功能倒退。

### 必须真实验证

- `pnpm --dir web lint` 输出无 warning，无 error。
- `pnpm --dir web build` 输出 dist / dist-desktop。
- `pnpm --dir web test` 若已配置则一起跑；未配置则以 lint + build 为准。
- 相关 Rust 校验 `cargo fmt / cargo clippy -- -D warnings / cargo check` 顺跑不受影响。

## 产品语义

### `react-refresh/only-export-components` 的产品含义

Vite React Refresh 要求 `.tsx` 只 export React 组件。历史代码里把 `formatConsoleValue` / `renderDomNode` / `useSharedNetworkFilter` 之类 helper 与组件一起 export，会导致 HMR 时整个模块重新执行、丢失面板 state。用户在 DevTools 里改动网络过滤后再改代码就会「面板重置」。

清理后：所有非组件 helper 迁移到同目录 `.ts` 文件（`consoleValueUtils.ts`、`domUtils.ts`、`sharedUtils.ts`）；组件文件只 export React 组件本身。

### `react-hooks/exhaustive-deps` 与 `set-state-in-effect`

保留原有用户可见行为的前提下补齐依赖数组、消除 `useEffect` 里的立即 `setState` 循环。这类修复不应改变 UI 语义；如果发现依赖数组补齐后带来循环 render，必须重新设计副作用触发时机，而不是加 disable。

### Storage 面板虚拟列表退化

原自研虚拟列表 hook 依赖手动切片和 `scrollTop` 侦听，React Compiler 无法证明其无副作用，报告 warning。清理策略：

- 保留表格样式、列选择、行操作。
- 用普通 `overflow-y: auto` 滚动 + 上限行数替代虚拟化；如果条目非常多，改为分页控件。
- 不引入 `react-window` 之类新依赖，避免包体膨胀。

## 技术细节

### 目录调整

```
web/src/pages/DevTools/components/
├── ConsolePanel.tsx        # 只 export ConsolePanel
├── ElementsPanel.tsx       # 只 export ElementsPanel
├── NetworkPanel.tsx        # 只 export NetworkPanel
├── StoragePanel.tsx        # 只 export StoragePanel（退化虚拟列表）
├── shared.tsx              # 共享 React 组件（Header/Row/Cell）
├── consoleValueUtils.ts    # formatConsoleValue / previewJson
├── domUtils.ts             # renderDomNode / walkDom
└── sharedUtils.ts          # useSharedNetworkFilter / useSharedSelection
```

Hook 名称是原文件已有的；本文件不引入新名字，只承接迁移。

### 空 catch 与未使用变量

- 空 `catch {}` 全部改成 `catch (error) { console.debug(...) }` 或显式忽略：`catch { /* intentionally ignored */ }` 并保留 comment，让 lint 允许。
- 未使用变量删除，或用 `_` 前缀（TS 允许并 lint 兼容）。

### React Compiler 规则

- 保留 `react-compiler/react-compiler` 规则；虚拟列表 hook 无法通过时改为常规实现，而不是关规则。
- 组件内 `useMemo` / `useCallback` 只用于必要的引用稳定；React Compiler 会自动记忆纯计算，不再堆多余的 memo。

## CLI + Web + Admin API

- CLI：无变化。
- Web：仅前端 lint / 目录调整；无 API 调用变化，无 UI 语义变化（除 Storage 面板虚拟化退化）。
- Admin API：无变化。

## Sync 边界

不涉及 Rust Sync；本模块是纯前端 lint 治理。

## Phase 1-4

### Phase 1：拆分非组件 helper

- 把 `ConsolePanel.tsx` / `ElementsPanel.tsx` / `NetworkPanel.tsx` / `shared.tsx` 内的 helper 迁移到 `consoleValueUtils.ts` / `domUtils.ts` / `sharedUtils.ts`。
- 原组件文件保持只 export 组件；调整所有 import 引用。

### Phase 2：Hooks 依赖与 effect 修复

- 补 `react-hooks/exhaustive-deps` 缺项；对不该纳入依赖的常量用 `useRef` 或提取到组件外。
- 消除 `set-state-in-effect` 报告：等值判断 + 早退，避免立即回写。

### Phase 3：Storage 面板降级

- 移除自研虚拟列表 hook 与相关 scrollTop 侦听。
- 保留列/操作/筛选交互；改为普通滚动或分页。
- 手工验证 Storage 面板功能未破坏。

### Phase 4：Lint 收尾 + 文档

- 删除空 catch、未使用变量。
- 确保 `pnpm --dir web lint` 零 warning、`pnpm --dir web build` 成功。
- 新增 `human_tests/web-lint-cleanup.md`（TC-WLF-01/02）；同步 `human_tests/readme.md` 索引（不维护「总用例数」）。

## 测试方案

### 单元测试

- 不新增单元测试：本任务不修改可单测业务算法，核心验证由 lint/build 和现有回归覆盖。
- 若发现虚拟列表退化后关键交互（选择/排序）容易回归，可在 `web/src/pages/DevTools/components/__tests__/` 补最小 vitest；不强制。

### E2E 测试

- 不新增 Playwright；`pnpm --dir web build` 保证类型与打包路径可用。

### 真实场景测试 human_tests

`human_tests/web-lint-cleanup.md`：

- **TC-WLF-01** `cd web && pnpm run lint`，期望 exit code 0、无 error、无 warning。
- **TC-WLF-02** `cd web && pnpm run build`，期望 TypeScript 通过、Vite 打包成功、`dist/` 生成 index.html + 资源文件。
- （可选）**TC-WLF-03** 手工打开 DevTools 面板确认 Storage/Network/Console/Elements 均正常渲染。

## Review/Fix/Test 闭环

### 第 1 轮

- 复核 lint 输出：`pnpm --dir web lint` 必须显示 `0 problems`。
- 复核目录：所有 helper 迁到 `.ts`；组件文件不再 export 非组件。
- 复核 Storage 面板功能是否退化到用户不可感知的程度。
- 校验：
  - `pnpm --dir web lint`
  - `pnpm --dir web build`
  - `cargo fmt --all -- --check`
  - `cargo clippy --workspace --all-targets --all-features -- -D warnings`
  - `cargo check --workspace`
  - `cargo test -p memory -p bifrost-agent`

### 第 2 轮

- 复查第 1 轮修改：确认没有引入 `eslint-disable`、没有新增运行时依赖。
- 复跑 lint + build。
- 若 CI 上 lint 阈值调整过，同步更新 CI 配置。

## 风险与决策

- **hook 依赖补齐可能带来意外重渲染**：需要逐个手工验证 DevTools 面板交互是否稳定；如果补齐后出现循环，宁可重构副作用触发方式，不添加 disable。
- **Storage 面板退化虚拟列表**：极端情况下（几十万条 key）滚动会卡顿；文档中说明限制，并在后续版本引入受控分页或第三方库。
- **拆分 helper 引入的循环 import**：`consoleValueUtils.ts` 若引用 `shared.tsx`，可能出现循环；应确保工具模块只依赖标准库和其它 `.ts`，不反向依赖组件。
- **未来 lint 规则升级**：本次不冻结 ESLint 版本，仅确保当前 lock 下零 warning；如果后续升级引入新规则，需要开新任务再清理，避免本次 PR 变大。
- **禁止降级规则**：任何「加 disable / 关规则」的诱惑都必须走 review 讨论；本方案默认拒绝任何 lint 关闭手段。
