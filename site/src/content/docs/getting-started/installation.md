---
title: "安装与启动"
description: "按照 docs/getting-started.md 的最新方式安装和启动 Bifrost。"
editLink: false
---

> 此页面由 `docs/getting-started.md` 自动同步生成。
# 安装与启动

本文档汇总 Bifrost 的安装、启动、管理端入口与卸载方式。

## 选择安装方式

| 场景 | 推荐方式 | 说明 |
| --- | --- | --- |
| 想最快开始抓包、查看流量和编辑规则 | 桌面端 App | 从 Releases 下载 `.dmg` 或 `.msi`，启动后内置代理后端和 Web UI 一起工作 |
| 想在终端、脚本或 CI 中使用 | CLI | 使用一键安装脚本、Homebrew 或 npm |
| 想本地调试 Bifrost 源码 | 从源码构建 | 使用 `./install.sh` 或手动构建 Rust/Web/Tauri 产物 |

## 安装 CLI

### 一键安装

```bash
curl -fsSL https://raw.githubusercontent.com/bifrost-proxy/bifrost/main/install-binary.sh | bash
```

默认安装完成后，脚本会自动完成一键体验初始化：

- 安装并信任 Bifrost CA 证书。
- 安装所有支持 AI 工具的 Bifrost skills。
- 以后台服务启动 Bifrost。

Bash 与 PowerShell 安装脚本都会自动探测 GitHub 直连和内置镜像源，优先使用最快可用的 release 下载地址；受限网络中可以设置 `BIFROST_GITHUB_MIRROR` 指定优先镜像，也可以用 `BIFROST_MIRROR_PROBE_TIMEOUT` 调整镜像探测超时。

可选参数：

```bash
# 指定安装目录
curl -fsSL https://raw.githubusercontent.com/bifrost-proxy/bifrost/main/install-binary.sh | bash -s -- --dir /usr/local/bin

# 安装指定版本
curl -fsSL https://raw.githubusercontent.com/bifrost-proxy/bifrost/main/install-binary.sh | bash -s -- --version v0.2.0

# 仅安装二进制，不自动安装证书、skills 或启动服务
curl -fsSL https://raw.githubusercontent.com/bifrost-proxy/bifrost/main/install-binary.sh | bash -s -- --no-post-install
```

### Homebrew（macOS）

```bash
brew tap bifrost-proxy/bifrost
brew install bifrost
```

### 使用 npm 安装

```bash
npm i @bifrost-proxy/bifrost
```

### 从源码构建

环境要求：

- Rust 1.70+
- Cargo
- Node.js 22+
- pnpm

构建步骤：

```bash
git clone https://github.com/bifrost-proxy/bifrost.git
cd bifrost

./install.sh

# 或手动构建
cd web && pnpm install && pnpm build && cd ..
cargo build --release
```

### 手动下载

可直接从 [Releases](https://github.com/bifrost-proxy/bifrost/releases) 下载预编译二进制。

## 安装桌面端 App

桌面端基于 Tauri 构建，安装包内包含 Web UI 和 `bifrost` 代理后端。适合不想先使用终端命令、但需要本机流量调试、规则编辑、Replay 和自动更新的用户。

### 直接下载最新版

点击与你设备匹配的安装包，页面会自动解析最新版本并直接开始下载。

<div class="desktop-download-grid" data-vp-download-status="loading">
  <a class="desktop-download-card" href="#" aria-disabled="true" data-vp-download-target="mac-arm">
    <img src="https://p1-hera.feishucdn.com/tos-cn-i-jbbdkfciu3/e20a102ae0da492baf4d3b81a9a03c22.png~tplv-jbbdkfciu3-png:0:0.png" alt="" width="52" height="52" loading="lazy" decoding="async" />
    <strong>macOS Apple Silicon</strong>
    <span>M1 / M2 / M3 / M4</span>
  </a>
  <a class="desktop-download-card" href="#" aria-disabled="true" data-vp-download-target="mac-intel">
    <img src="https://p1-hera.feishucdn.com/tos-cn-i-jbbdkfciu3/e20a102ae0da492baf4d3b81a9a03c22.png~tplv-jbbdkfciu3-png:0:0.png" alt="" width="52" height="52" loading="lazy" decoding="async" />
    <strong>macOS Intel</strong>
    <span>Intel Mac</span>
  </a>
  <a class="desktop-download-card" href="#" aria-disabled="true" data-vp-download-target="win-x64">
    <img src="https://p1-hera.feishucdn.com/tos-cn-i-jbbdkfciu3/62baaf31cd8147699516852865782600.png~tplv-jbbdkfciu3-png:0:0.png" alt="" width="52" height="52" loading="lazy" decoding="async" />
    <strong>Windows</strong>
    <span>x64 安装包</span>
  </a>
  <a class="desktop-download-card" href="#" aria-disabled="true" data-vp-download-target="win-arm">
    <img src="https://p1-hera.feishucdn.com/tos-cn-i-jbbdkfciu3/62baaf31cd8147699516852865782600.png~tplv-jbbdkfciu3-png:0:0.png" alt="" width="52" height="52" loading="lazy" decoding="async" />
    <strong>Windows ARM64</strong>
    <span>ARM 安装包</span>
  </a>
</div>

<p class="desktop-download-status" data-vp-download-message>正在解析最新版本...</p>

### macOS 安装

1. 双击打开下载的 `.dmg`。
2. 将 `Bifrost.app` 拖入 `Applications`。
3. 从 `Applications` 或 Launchpad 打开 Bifrost。
4. 如果系统提示来自互联网下载，确认打开；如果 Gatekeeper 拦截未签名版本，在 `System Settings -> Privacy & Security` 中允许打开。

### Windows 安装

1. 双击下载的 `.msi`。
2. 按安装向导完成安装。
3. 从 Start Menu 打开 `Bifrost`。
4. 如果 Windows SmartScreen 提示未知发布者，确认来源是 `bifrost-proxy/bifrost` 的 GitHub Releases 后再选择继续。

### 首次启动后检查

桌面端启动后会在应用内部启动打包的 `bifrost` 后端，并打开本机管理界面。首次启动会异步检查并安装 Bifrost CA；需要 HTTPS 抓包时，确认系统已信任该 CA。默认数据目录仍为 `~/.bifrost`。

如果后续需要在终端或 AI 工具中使用 Bifrost，打开桌面端 Settings，在 `Desktop Proxy Core` 卡片点击 `Install CLI & Skills`。这会安装命令行 `bifrost` 和 Bifrost AI skills。

更完整的桌面端安装、更新、卸载和源码构建说明见 [`desktop.md`](./desktop)。

## 检查安装

```bash
command -v bifrost
bifrost --version
```

如果尚未加入 `PATH`，也可以在源码仓库中执行：

```bash
cargo run -p bifrost-cli -- --version
```

## 启动代理

```bash
# 后台启动，默认监听 0.0.0.0:9900
bifrost start -d

# 自定义端口和监听地址
bifrost -p 9000 -H 127.0.0.1 start

# 启用 HTTP + SOCKS5
bifrost -p 9900 --socks5-port 1080 start

# 按域名启用 TLS 抓包
bifrost start --intercept-include "*.api.local"

# 按应用启用 TLS 抓包
bifrost start --app-intercept-include "*Chrome,*curl"

# 前台启动，适合查看实时日志
bifrost start
```

`bifrost start -d` 会以后台服务启动 Bifrost，并按默认配置启用系统代理，浏览器和桌面应用通常无需额外配置就会进入 Bifrost。TLS 抓包按需开启，不建议默认全局 `--intercept`；优先使用域名白名单、应用白名单或规则级 `tlsIntercept://`。遇到 SSL pinning 应用时，应使用 `--app-intercept-exclude`、`--intercept-exclude` 或规则级 `tlsPassthrough://` 排除。需要 TLS 抓包时，启动流程会自动生成并安装 Bifrost CA；`bifrost ca generate` / `bifrost ca install` 主要用于手动修复或诊断证书状态。默认访问控制模式为 `interactive`：本机 loopback 直接允许，非本机地址需要管理端审批、白名单或显式 `--allow-lan` / `--access-mode` 放行。`--no-system-proxy` 只用于 CI、测试沙箱或你明确不希望修改系统网络的诊断场景，不作为普通用户的首次启动路径。

## 管理端入口

服务启动后，在浏览器访问：

```text
http://127.0.0.1:<port>/_bifrost/
```

默认端口示例：

```text
http://127.0.0.1:9900/_bifrost/
```

常用入口：

| 路径 | 说明 |
| --- | --- |
| `/_bifrost/` | Web UI |
| `/_bifrost/api/rules/*` | 规则管理 API |
| `/_bifrost/api/values/*` | Values API |
| `/_bifrost/api/traffic/*` | 流量 API |
| `/_bifrost/api/scripts/*` | Scripts API |
| `/_bifrost/api/replay/*` | 请求重放 API |

说明：

- 管理端默认仅允许通过 `127.0.0.1` 或 `localhost` 访问
- SSE 增量订阅接口为 `/_bifrost/api/traffic/{id}/sse/stream?from=begin`

### 设备可用性检查

当手机、平板、另一台电脑或电视浏览器配置 Bifrost 后仍然无法抓包时，优先使用 `Settings -> Certificate -> Availability Check`，不要先猜是证书问题还是代理授权问题。

使用步骤：

1. 在 Certificate 页面顶部的 `Availability Check` 中选择当前电脑的局域网 IP。
2. 点击 `Generate Availability Check`，把页面展示的链接发给目标设备，或让目标设备扫码。
3. 目标设备打开检测页后会自动检查代理访问授权、探针端口可达性，以及 Bifrost CA 的 HTTPS 信任状态。
4. 如果代理访问显示待批准，回到 Bifrost 管理端批准该设备，或调整访问控制/局域网访问配置。
5. 如果 HTTPS 信任失败，先安装 Bifrost CA；iOS 还必须进入 `设置 > 通用 > 关于本机 > 证书信任设置`，打开 Bifrost CA 的完全信任。
6. 检查成功后，点击页面上的代理地址复制 `<局域网 IP>:<端口>`，或打开代理二维码继续配置。

这个检查验证的是“当前设备当前浏览器能否真实完成 Bifrost CA 签发证书的 HTTPS 握手”。它不能保证所有 App 都能被解密，因为个别 App 可能使用证书固定、自定义 TLS 栈，Android App 也可能默认不信任用户安装的 CA。

## 环境变量

| 环境变量 | 说明 | 默认值 |
| --- | --- | --- |
| `BIFROST_DATA_DIR` | 数据目录路径 | `~/.bifrost` |
| `RUST_LOG` | 日志级别和过滤器 | `info` |
| `WEB_PORT` | Web UI 开发服务端口 | `3000` |

示例：

```bash
RUST_LOG=debug bifrost start
RUST_LOG=bifrost_proxy=debug,info bifrost start
```

日常使用不建议为了临时排障创建新的 `BIFROST_DATA_DIR`。优先启动一次主服务，再使用 `bifrost port bind ...` 创建独立调试端口；只有自动化测试、并发实验或破坏性配置验证需要完全隔离状态时，才设置临时数据目录。

## 卸载

```bash
# 卸载 CLI 和桌面应用
./uninstall.sh

# 连同数据一起清理
./uninstall.sh --purge
```
