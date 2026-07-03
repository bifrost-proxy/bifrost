# 桌面安装脚本改为直接复制 App Bundle

## 背景

`./install.sh` 是源码构建路径下的一键安装脚本，负责把 `bifrost` CLI 装到 `~/.local/bin`、把桌面端装到 macOS Applications 目录。历史实现依赖 Tauri DMG 打包产物：

- 先跑 `pnpm run desktop:build`（等价于 `tauri build`，默认包含 `dmg` bundle）。
- 再 `hdiutil attach` 挂载 DMG，从里面 `cp -R Bifrost.app`。

DMG 路径的问题：

- `dmg` bundle 依赖 `create-dmg` + `hdiutil`，在受限 CI / 开发机上偶发失败或超时（尤其 Apple Silicon 首次 sign）。
- 用户源码安装的语义就是“我要一个本地 `.app`”，中间过 DMG 是白白多花 1~3 分钟。
- DMG 挂载失败会让整个 install.sh 中断，用户看到的是 `hdiutil` 报错，不知道其实 `.app` 已经在 target 目录下可用。

本方案把 macOS 桌面安装改成“只构建 `.app`、直接复制到 `/Applications`”，同时提供 `--app-dir` 与 `BIFROST_APP_INSTALL_DIR` 让用户改到用户目录。

## 用户目标验证清单

### 必须实现

- `package.json` 提供 `desktop:build:app` 脚本，构建时向 Tauri 传 `--bundles app`（不生成 dmg/deb/msi）。
- `install.sh` 桌面安装路径调用 `pnpm run desktop:build:app`，不再依赖 dmg。
- `install.sh` 默认 `APP_INSTALL_DIR=/Applications`；`BIFROST_APP_INSTALL_DIR` 环境变量或 `--app-dir` 参数可覆盖。
- 复制前检查目标目录可写；不可写则打印明确提示（含 `sudo` / `--app-dir $HOME/Applications` 建议）并退出。
- 复制优先使用 `ditto`（保留 xattr / codesign），退化时使用 `cp -R`。
- 复制前 `rm -rf` 目标 `Bifrost.app`，保证升级干净。
- 复制后对 target 执行 `clear_xattr` 移除 `com.apple.quarantine`，避免首次打开 Gatekeeper 拦截。
- `find_desktop_bundle` 优先在 `desktop/src-tauri/target/release/bundle/macos/Bifrost.app` 命中，未命中则 `find ... -name 'Bifrost.app'` 兜底。

### 必须不破坏

- CLI 安装路径（`~/.local/bin/bifrost`）逻辑不变。
- `--cli-only` / `--desktop-only` / `--no-desktop` 参数继续工作。
- Linux / Windows 桌面构建路径（当前 install.sh 只面向 macOS 桌面）不受影响，Linux 侧仍走各自打包脚本。
- 已装了旧 DMG 版本的用户升级时，新的 `.app` 覆盖旧位置，不残留旧 bundle。

### 必须真实验证

- `pnpm run desktop:build:app` 成功产出 `desktop/src-tauri/target/release/bundle/macos/Bifrost.app`，无 `bundle/dmg/` 目录。
- `./install.sh --desktop-only` 默认写入 `/Applications/Bifrost.app`（需 sudo 或 `/Applications` 可写）。
- `./install.sh --desktop-only --app-dir /tmp/bifrost-install-test` 写入指定目录，不误装到 `/Applications`。
- 目标不可写时脚本提示明确、退出码非 0。
- 双击装好的 `.app` 首次能打开（无 Gatekeeper 阻断，因 `clear_xattr` 已跑）。

## 产品语义

### 源码安装 = “直接给 .app”

用户跑 `install.sh` 的期望是“我 clone 仓库 → 得到可用的桌面版本”。中间过 DMG 只是分发场景的封装。移除 DMG 依赖后：

- 构建更快（省 sign + hdiutil + create-dmg）。
- 失败面收窄（只可能因 `tauri build --bundles app` 或 `cp` 失败）。
- 用户可控性更高（想装到别处直接 `--app-dir`）。

### 默认目录 = `/Applications`

选择系统级 Applications 是因为：

- macOS 用户直觉：Launchpad / Spotlight 都识别。
- 支持多用户共享，一个用户装完其他用户能用。
- 与官方 DMG 分发版本落点一致，切换渠道无残留。

不可写时提示 `sudo ./install.sh` 或 `--app-dir $HOME/Applications`，用户可自选。

### 打包语义与官方 release 保持分离

`desktop:build:app` 仅供源码安装使用；官方 release 仍走 `desktop:build`（包含 dmg + notarize）。两条脚本并存，不互相替代。

## 技术细节

### package.json

```json
{
  "scripts": {
    "desktop:build:app": "node scripts/ensure-root-dev-deps.mjs && pnpm --dir web run build:desktop && cargo build -p bifrost-cli --release && node scripts/prepare-tauri-sidecar.mjs release && pnpm exec tauri build --config desktop/src-tauri/tauri.conf.json --bundles app"
  }
}
```

保持与 `desktop:build` 相同的前置：确保根 dev deps、web build:desktop、release sidecar 二进制、prepare-tauri-sidecar。差别只在 `--bundles app`。

### install.sh 关键片段

```bash
DEFAULT_APP_INSTALL_DIR="/Applications"
APP_INSTALL_DIR="${BIFROST_APP_INSTALL_DIR:-$DEFAULT_APP_INSTALL_DIR}"

install_desktop() {
    local installed_app_path="$APP_INSTALL_DIR/Bifrost.app"
    ensure_desktop_dist
    print_step "Building desktop app bundle for macOS..."
    (cd "$SCRIPT_DIR" && pnpm run desktop:build:app)

    desktop_bundle_path="$(find_desktop_bundle)" || {
        print_error "Desktop bundle not found after build"; exit 1;
    }

    print_step "Installing desktop app..."
    if [[ ! -w "$APP_INSTALL_DIR" ]]; then
        print_error "No write permission for $APP_INSTALL_DIR"
        echo "  Try re-running with sudo, or pass --app-dir \$HOME/Applications"
        exit 1
    fi
    copy_app_bundle "$desktop_bundle_path" "$installed_app_path"
    clear_xattr "$installed_app_path"
    print_success "Desktop app installed: $installed_app_path"
}

copy_app_bundle() {
    local source_app="$1" target_app="$2"
    rm -rf "$target_app"
    if command -v ditto &>/dev/null; then
        ditto "$source_app" "$target_app"
    else
        cp -R "$source_app" "$target_app"
    fi
}

find_desktop_bundle() {
    local bundle_root="$SCRIPT_DIR/desktop/src-tauri/target/release/bundle"
    if [[ -d "$bundle_root/macos/Bifrost.app" ]]; then
        printf '%s\n' "$bundle_root/macos/Bifrost.app"; return 0
    fi
    local app_path
    app_path="$(find "$bundle_root" -maxdepth 3 -type d -name 'Bifrost.app' 2>/dev/null | head -n 1)"
    [[ -n "$app_path" && -d "$app_path" ]] && { printf '%s\n' "$app_path"; return 0; }
    return 1
}
```

### Gatekeeper 处理

`clear_xattr` 执行 `xattr -dr com.apple.quarantine <path>`。因为源码构建的 `.app` 没有开发者签名，首次打开会被 Gatekeeper 拦截。移除隔离属性后可直接打开；用户可自行 codesign 增强安全。

### 目录切换语义

- 环境变量 `BIFROST_APP_INSTALL_DIR=/Applications` 优先级低于 `--app-dir`。
- `--app-dir` 支持相对路径，脚本内不做 `realpath`（Bash 3 兼容）。
- 目标目录必须已存在；不存在时报错，用户手工 `mkdir`。

## CLI 与 Admin API

不引入 CLI 参数或 Admin API 变化。仅 `install.sh` 与 `package.json` 层的变更。

## 实现切分

### Phase 1：package.json 脚本

- 新增 `desktop:build:app`，参数只多 `--bundles app`。
- 手工跑一次验证 `bundle/macos/Bifrost.app` 存在、`bundle/dmg/` 不存在。

### Phase 2：install.sh 改造

- 桌面路径改用 `pnpm run desktop:build:app`。
- 默认 `APP_INSTALL_DIR=/Applications`。
- 新增 `--app-dir` 参数与 `BIFROST_APP_INSTALL_DIR` 环境变量。
- `copy_app_bundle` 支持 `ditto` 优先、`cp -R` 兜底。
- 写权限预检 + 明确错误提示。
- `find_desktop_bundle` 逻辑保留 fallback。

### Phase 3：清理旧 DMG 路径

- 删除 install.sh 中 `hdiutil attach/detach` 相关代码。
- 保留 `release.sh` / CI 中的 DMG 打包（那是 release 用途，不由 install.sh 触发）。

### Phase 4：文档

- `README.md` 源码安装章节：更新桌面安装说明，明确默认 `/Applications`、`--app-dir` 用法、需要 sudo 时的建议。
- `docs/desktop.md` 与 `docs-en/desktop.md` 同步更新桌面构建命令为 `pnpm run desktop:build:app`。
- `human_tests/desktop-install-app-copy.md` 新增/更新用例。

## 测试方案

### 单元测试

- `install.sh` / `package.json` 无 Rust 单元测试覆盖；靠脚本级手工验证 + CI job。

### E2E 测试

- CI `.github/workflows/ci.yml` 新增 macOS job：
  - `./install.sh --desktop-only --app-dir /tmp/bifrost-install-test-apps`
  - 断言 `/tmp/bifrost-install-test-apps/Bifrost.app/Contents/MacOS/Bifrost` 可执行。
  - 断言 build 目录下无 `bundle/dmg/`。
- 现有 release job 不受影响，继续跑 `desktop:build` + 签名 + notarize。

### 真实场景测试

`human_tests/desktop-install-app-copy.md`：

- TC-DIA-01：`./install.sh` 默认路径 → CLI + 桌面装到 `/Applications`（可能需 sudo）。
- TC-DIA-02：`./install.sh --desktop-only --app-dir /tmp/test` 装到指定目录，`/Applications` 无残留。
- TC-DIA-03：`BIFROST_APP_INSTALL_DIR=~/Applications ./install.sh` 生效。
- TC-DIA-04：`--app-dir /System` 之类不可写目录 → 明确错误 + 建议 sudo。
- TC-DIA-05：安装后双击 `.app` 首次可开（Gatekeeper 不阻断）。
- TC-DIA-06：先装旧 DMG 版本再跑新脚本，`.app` 被干净覆盖，无版本混合。
- TC-DIA-07：构建产物中不存在 `bundle/dmg/`。

### 覆盖率与项目校验

- 无新 Rust 单测；`cargo` 校验按常规执行：
  - `cargo fmt --all -- --check`
  - `cargo clippy --all-targets --all-features -- -D warnings`
  - `cargo build -p bifrost-cli --release`
- `pnpm --dir web run build:desktop` 单跑验证 web 构建 OK。
- `pnpm run desktop:build:app` 手工跑验证 Tauri 只出 `.app`。
- 本地按 `rust-project-validate` 约定豁免 `make coverage`。

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核用户目标：`--bundles app`、默认 `/Applications`、`--app-dir`、写权限提示、Gatekeeper 处理。
- 复核 diff：`install.sh`、`package.json`、`README.md`、`docs/desktop.md`。
- 重点 review：
  - `find_desktop_bundle` 兜底 `find` 是否处理路径含空格？
  - `copy_app_bundle` 是否在 `ditto` 缺席环境降级正确？
  - `--app-dir` 与环境变量优先级顺序在 help 中一致？
- 复测：`--desktop-only --app-dir /tmp/xx`、`--app-dir /System`、默认 `/Applications`。

### 第 2 轮

- 检查 `git status --short`、`git diff` 无遗漏；`release.sh` / CI 未被误改。
- 重点 review：`README.md` 与 `docs/desktop.md` 命令一致；旧 `desktop:build` 文档仍指向 release 场景。
- 复测：升级路径（旧 DMG 装 → 新脚本装）、Gatekeeper 首次打开、xattr 清理。

## 风险与决策点

- **未签名 `.app` 的用户体验**：源码安装用户默认拿到未签名 `.app`，`clear_xattr` 只能绕过 Gatekeeper 首次拦截。若产品未来要求签名，可增加 `--codesign` 参数用本地开发者证书 sign。
- **`/Applications` 需要 sudo**：默认路径需权限，用户体感偶尔不便。选择保留 `/Applications` 是为了与官方 release 路径一致；不可写时提示明确。
- **DMG 从 install.sh 完全移除的兼容性**：老的 wiki 或 README 可能提及“install.sh 会生成 DMG”，需扫一遍改掉。
- **Windows / Linux 桌面安装**：本方案只处理 macOS 分支；Linux 若需类似“只出 deb / appimage”应另开子任务，不在本方案范围。
- **`--bundles app` 的 Tauri 版本要求**：Tauri v1 / v2 参数一致；若未来升级 Tauri，需回归验证。
