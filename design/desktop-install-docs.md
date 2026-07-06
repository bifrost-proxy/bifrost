# Desktop Installation Documentation

## 用户目标验证清单

### 必须实现

- README 和 README.en.md 给出桌面端安装入口，不再只写“去 releases 下载”。
- 中文/英文安装手册把 CLI 与 Desktop 两条路径并列说明。
- 中文/英文 desktop 手册覆盖安装包选择、macOS 安装、Windows 安装、首次启动、CLI/Skills 联动、更新、卸载和常见问题。
- 官网首页在保持 “AI-era proxy / Coding Agent ready” 定位的同时，增加桌面端作为上手路径的入口。

### 必须不破坏

- CLI 一键安装命令保持现有权威写法。
- `site/src/content/docs` 继续由 `docs/` 和 `docs-en/` 同步生成，不能只手写生成内容。
- 首页继续保持手写静态 HTML/CSS/JS，不引入 VitePress/Vue runtime。
- 官网默认首次启动示例仍使用 `bifrost start -d`，不把测试隔离参数作为普通用户入口。

### 必须真实验证

- 执行 site docs sync 和 build，确认中文/英文 desktop 文档进入站点产物。
- 执行首页静态验证，确认桌面端入口、文档入口、语言切换和首屏定位共存。
- 按 human_tests 用例逐条检查 README、安装手册和官网可发现性。

### 必须交付

- 更新 human_tests 并同步索引。
- 完成至少两轮 Review/Fix/Test。
- 提交、推送、创建或更新 PR，并跟进远端 CI。

## 技术方案

1. 源文档只改 `README.md`、`README.en.md`、`docs/getting-started.md`、`docs-en/getting-started.md`、`docs/desktop.md`、`docs-en/desktop.md`。
2. 站点文档页通过 `pnpm --dir site run docs:sync` 自动生成到 `site/src/content/docs`。
3. 官网首页直接改 `site/home/index.html`、`site/home/home.js`、`site/home/styles.css`，新增“CLI or Desktop”安装路径区块，视觉上沿用现有低饱和背景、8px 卡片、紧凑信息密度和 AI/Coding Agent 叙事。
4. E2E 使用现有 `e2e-tests/tests/test_site_docs_sync.sh` 覆盖 docs sync、VitePress 构建、静态首页断言和链接校验。

## 风险与边界

- 不改安装脚本、Tauri 代码、CLI 行为或 release 产物命名逻辑。
- 不直接修改 `bifrost-proxy.github.io` 部署仓库。
- 文档中涉及未签名安装包的说明只作为当前风险提示，不引入绕过安全校验的默认建议。
