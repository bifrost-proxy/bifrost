# Desktop 无签名自动更新（Tauri 自定义 Updater）

## 背景

Tauri 官方 updater 插件强制要求使用 `.sig` 对发布产物进行签名校验。为了在不提供签名文件的情况下仍能从 GitHub Releases 自动下载并安装最新桌面端版本，本模块改为在 Rust 后端实现自定义更新逻辑，并在前端提供进度提示。

## 目标

- 不依赖 `.sig` 签名文件。
- 从 GitHub Releases 下载对应平台产物并触发安装。
- 更新过程向前端持续推送状态与下载进度。

## 实现概述

### 版本检测

- 远端版本号来源：拉取 `main` 分支的 `desktop/src-tauri/Cargo.toml`，解析 `package.version`。
- 本地版本号来源：`env!("CARGO_PKG_VERSION")`（即当前桌面壳 `bifrost-desktop` 的版本）。
- 对比方式：优先按 `semver` 比较，解析失败则退化为字符串不等比较。

### Release 产物选择

按照 Release workflow 的产物命名约定拼接下载链接：

- Windows：`bifrost-desktop-v{version}-{target}.msi`
- macOS：`bifrost-desktop-v{version}-{target}.dmg`

其中 `target` 由当前架构映射：

- Windows：`x86_64-pc-windows-msvc` / `aarch64-pc-windows-msvc`
- macOS：`x86_64-apple-darwin` / `aarch64-apple-darwin`

### 下载与进度推送

- 下载使用 `reqwest` blocking client，写入 `std::env::temp_dir()`。
- 通过 `desktop://update-status` 事件向前端推送阶段变化与下载进度（含 percent）。

### 安装

- Windows：启动 `msiexec.exe /i <msi> /passive /norestart`，随后退出当前应用。
- macOS：挂载 dmg，生成一个临时脚本等待当前进程退出后复制 `.app` 覆盖原应用目录并重启，然后退出当前应用。

## 测试方案

### 单元测试

- `test_parse_version_from_manifest`：验证 Cargo.toml 版本提取。
- `test_build_release_asset_url_windows/macos`：验证下载 URL 拼接与后缀。

### 端到端（Updater 逻辑）

- `RUN_DESKTOP_UPDATE_E2E=1 cargo test -p bifrost-desktop`：
  - 拉取远端 `Cargo.toml` 版本。
  - 以 `platform_override=windows` 生成下载链接并下载 msi 到临时目录（install 阶段使用 dry-run）。

### 真实场景测试

- 将本地版本号调低（`desktop/src-tauri/Cargo.toml` 与 `desktop/src-tauri/tauri.conf.json`）。
- 启动桌面端（desktop 模式），观察 UI 是否出现“应用更新”弹窗与进度。
- 验证下载链接与 GitHub Releases 资产匹配。
- Windows / macOS 上验证是否能成功拉起安装流程并退出应用。
