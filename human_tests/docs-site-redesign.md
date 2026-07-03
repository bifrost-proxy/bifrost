# Docs Site Redesign 方案验证

## 功能模块说明

验证文档站重构技术方案是否满足“调研更好方案、首页纯 HTML 极致性能、参考 BigModel 首页体验、文档区继续稳定可维护”的设计目标。

本文件验证方案文档，不验证尚未实现的运行时代码。

## 前置条件

1. 当前工作目录位于仓库根目录。
2. 已完成参考站真实浏览器观察：`https://bigmodel.cn/glm-coding`。
3. 方案文档存在：`design/docs-site-redesign.md`。
4. 本用例文档存在：`human_tests/docs-site-redesign.md`。

## 测试用例列表

### TC-DSR-01 方案覆盖用户目标

操作步骤：
1. 执行 `rg -n "必须实现|必须不破坏|必须真实验证|必须交付" design/docs-site-redesign.md`。
2. 检查方案是否明确写出首页不使用框架生成页面。
3. 检查方案是否明确写出首页使用手写 HTML、CSS 和极少量原生 JavaScript。
4. 检查方案是否明确写出文档区继续保留 Astro/Starlight/Pagefind。
5. 检查方案是否明确写出已从独立 worktree 交付，不污染当前并行分支。

预期结果：
- 四类目标清单均存在。
- 首页纯 HTML 和非框架生成约束明确。
- 文档区能力保留明确。
- worktree 隔离在最终交付证据中可对应。

### TC-DSR-02 外部调研与选型结论完整

操作步骤：
1. 执行 `rg -n "Astro|Starlight|Pagefind|Docusaurus|VitePress|Cloudflare|GitHub Pages" design/docs-site-redesign.md`。
2. 检查方案是否分别说明保留 Starlight、保留 Pagefind、不迁移 Docusaurus、不迁移 VitePress 的理由。
3. 检查方案是否包含短期 GitHub Pages 与中期 Cloudflare Pages 的部署策略。
4. 检查方案末尾是否列出调研来源 URL。

预期结果：
- 每个候选方案都有结论。
- 结论围绕首页性能、文档区维护成本、搜索和部署能力展开。
- 调研来源可追溯。

### TC-DSR-03 BigModel 首页体验参考被正确吸收

操作步骤：
1. 执行 `rg -n "BigModel|GLM Coding|终端|IDE|首屏|如何开始" design/docs-site-redesign.md`。
2. 检查方案是否记录参考站首屏模式：极简导航、超大产品名、短副标题、单主 CTA、演示面板前置。
3. 检查方案是否记录参考站轻量 Tab 交互。
4. 检查方案是否记录滚动叙事顺序。
5. 检查方案是否明确禁止照搬 BigModel 的素材、文案和品牌。

预期结果：
- 方案提炼体验模式，而不是复制页面。
- Bifrost 首页结构能映射到产品真实场景：CLI、Web UI、Desktop、抓包、改写、回放。

### TC-DSR-04 首页纯 HTML 架构可执行

操作步骤：
1. 执行 `rg -n "site/home|build-home|verify-home|dist/index.html|astro build" design/docs-site-redesign.md`。
2. 检查方案是否说明 `astro build` 后由 `build-home.mjs` 覆盖 `site/dist/index.html`。
3. 检查方案是否说明 `verify-site-links` 必须在首页覆盖后执行。
4. 检查方案是否包含 base path 处理要求。
5. 检查方案是否包含回滚到 `site/src/pages/index.astro` 的方式。

预期结果：
- 首页与文档区构建边界明确。
- 构建顺序不会让 Astro 重新覆盖手写首页。
- GitHub Pages `/bifrost/` base path 被纳入设计。
- 回滚路径明确且不影响文档区。

### TC-DSR-05 性能预算与可访问性约束明确

操作步骤：
1. 执行 `rg -n "性能预算|HTML|CSS|JS|首屏|LCP|CLS|第三方请求|system font|ARIA|no-JS" design/docs-site-redesign.md`。
2. 检查方案是否给出 HTML、CSS、JS、图片和首屏总传输预算。
3. 检查方案是否要求第三方请求为 0。
4. 检查方案是否要求图片宽高或 aspect-ratio。
5. 检查方案是否要求 Tab 使用 `aria-selected` 且 no-JS fallback 可用。

预期结果：
- 性能预算可被未来脚本验证。
- 可访问性和 no-JS 不是口头目标，而是进入 `verify-home.mjs` 约束。

### TC-DSR-06 测试、Review/Fix/Test 和不适用项完整

操作步骤：
1. 执行 `rg -n "测试方案|单元测试|E2E|真实场景测试|Review/Fix/Test|make coverage|cargo test|local-ci" design/docs-site-redesign.md`。
2. 检查方案是否列出未来实现阶段的单元测试和 E2E 测试名称。
3. 检查方案是否把本次文档-only 的 Rust、Web、E2E、coverage 和 local-ci 标记为不适用并说明原因。
4. 检查方案是否包含两轮 Review/Fix/Test 范围。

预期结果：
- 当前方案阶段与未来实现阶段测试边界清楚。
- 不适用项有具体原因。
- 两轮闭环可执行。

## 清理步骤

本用例不创建临时文件，无需清理。
