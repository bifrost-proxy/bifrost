# 移动端连接 Bifrost 使用教程

## 背景

Bifrost 的代理服务可以运行在 PC、macOS 或 Linux 上，但移动端用户需要同时理解局域网可达性、代理监听端口、访问控制和 HTTPS CA 信任。现有 README 和 WebUI 已有 Availability Check 与移动设备信任能力，但正式文档缺少一篇从启动服务到手机 Wi-Fi 代理配置的完整教程。

本次只补充用户文档与文档站同步，不修改代理、WebUI 或设备安装实现。

## 用户目标验证清单

### 必须实现

- 新增中文移动端教程，说明 iPhone/iPad/Android 连接 PC、Mac、Linux 上 Bifrost 的完整流程。
- 解释管理端地址与移动端代理地址的区别，明确不能把 `127.0.0.1` 填入手机代理设置。
- 覆盖 CLI、桌面端和 Linux 启动方式，HTTP 代理与可选 SOCKS5 端口。
- 覆盖同网段、防火墙、AP isolation、访问控制和 Availability Check。
- 覆盖 iOS profile 安装与完全信任、Android 用户 CA 与 App 证书固定边界。
- 新增英文镜像，并让站点自动生成中文和 `/en/` 页面。

### 必须不破坏

- 不修改代理监听、访问控制、证书或 WebUI 运行行为。
- 不绕过 `docs/`、`docs-en/` 到 `site/src/content/docs/` 的单向同步边界。
- 不推荐把 Bifrost 暴露到公网，也不把 `allow_all` 描述为默认安全配置。
- 不在文档中写入本地绝对路径或固定测试端口。
- 保留现有 README、安装、桌面和移动设备信任文档的有效链接。

### 必须真实验证

- 文档内的启动命令、管理端路径、Availability Check 路径和移动端代理格式与当前 CLI/WebUI 实现一致。
- `docs/mobile-proxy.md` 与 `docs-en/mobile-proxy.md` 均被 docs sync 发现并生成稳定站点页面。
- 站点构建、文档同步校验和站内链接校验通过。
- human_tests 用例逐条执行，至少验证源文档、站点生成页面、关键安全提示和相对链接。

### 必须交付

- `docs/mobile-proxy.md`
- `docs-en/mobile-proxy.md`
- `docs/README.md`、`docs-en/README.md` 的入口
- `site/scripts/docs-sync-lib.mjs` 的 Getting Started 稳定路由映射
- `human_tests/mobile-proxy-usage.md` 与 `human_tests/readme.md` 索引
- 完成两轮 Review/Fix/Test，提交、推送、PR 和远端 CI 看护

## 放置与路由决策

教程属于“从安装后第一次使用到跨设备连接”的入门任务，不属于 CLI 参数参考或规则协议，因此源文档放在 `docs/` 根目录，站点页面放在 Getting Started 分组。

稳定映射：

| 源文档 | 站点路由 |
| --- | --- |
| `docs/mobile-proxy.md` | `/getting-started/mobile-proxy` |
| `docs-en/mobile-proxy.md` | `/en/getting-started/mobile-proxy` |

`site/scripts/docs-sync-lib.mjs` 的 known page 映射负责标题、描述和顺序；页面正文仍只维护在 `docs/` 与 `docs-en/`。

## 内容结构

1. 地址模型：管理端、HTTP 代理、SOCKS5。
2. PC/Mac/Linux 启动。
3. 同网段与防火墙前置条件。
4. Availability Check。
5. iOS/Android Wi-Fi HTTP Proxy 设置。
6. HTTPS CA 信任与平台差异。
7. 常见故障和安全收尾。

## 测试方案

### 单元/静态验证

- `pnpm --dir site run docs:test`
- `pnpm --dir site run docs:sync`
- `pnpm --dir site run docs:verify`
- 相对 Markdown 链接扫描和关键命令/路径检索

本次不修改 Rust、WebUI 或运行时脚本，不新增 Rust 单元测试。

### E2E 验证

- `pnpm --dir site run build`：真实同步中文/英文源文档并生成部署产物。
- `pnpm --dir site run site:verify-links`：验证生成页面的站内链接和静态资源。
- `bash e2e-tests/tests/test_site_docs_sync.sh`：确认新文档可被站点同步链路发现；若脚本不接受固定 probe，则用 docs sync/build 作为等价真实站点链路。

### 真实场景验证

新增 `human_tests/mobile-proxy-usage.md`，覆盖：

- 文档入口、双语入口和稳定路由。
- PC/Mac/Linux 启动命令与 `127.0.0.1` 安全边界。
- HTTP/SOCKS5 端口、Availability Check、访问控制和移动端设置步骤。
- iOS/Android CA 信任边界、证书固定提示和公网暴露警告。
- 生成后的中文/英文站点页面内容。

## 覆盖率与项目校验

- 本次仅修改 Markdown、站点同步元数据和 human_tests，不修改业务代码，coverage 90% CI 门禁对业务代码不适用；仍由远端 CI 的统一 workflow 负责仓库级门禁。
- Rust 单元测试、`cargo test --workspace --all-features`、Rust fmt/clippy 和 `rust-project-validate` 对本次文档变更不适用。
- `scripts/ci/local-ci.sh` 对本次文档变更不适用；站点专用测试和构建是有效验证。

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核源文档是否覆盖用户从启动到手机配置的完整链路。
- 执行 `git status --short`、`git diff`，检查文档是否含本地绝对路径、错误端口或公网不安全建议。
- 运行 docs 单测、sync、verify、关键链接扫描和 site build。
- 修复文档内容、英文镜像、索引或生成路由问题后复测。

### 第 2 轮

- 基于第 1 轮最新 diff，再次检查源文档与生成页面是否一致。
- 复查中文/英文导航、页面标题顺序、关键路径和人类测试索引。
- 复跑 docs 单测、verify、站点构建、站内链接和 human_tests 中的全部静态用例。
- 若任一项失败，追加 Review/Fix/Test 轮次直至关闭。

## 文档更新要求

- `docs/README.md` 与 `docs-en/README.md` 增加入门教程入口。
- 不直接提交 `site/src/content/docs/` 生成文件；构建时由 `docs-sync` 生成。
- `human_tests/readme.md` 只新增移动端教程索引行，不更新全局计数。
