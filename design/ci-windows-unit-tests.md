# CI Windows Unit Tests 稳定化设计方案

## 背景

Bifrost 需要在 Parallels Windows VM 与 GitHub Actions `windows-latest` runner 上跑通完整 `cargo test --workspace --all-features` 工作流；目标平台为 `x86_64-pc-windows-msvc`。历史多类失败只在 Windows 上暴露：

- `bifrost-agent` 的 `exec_command` 交互式 TTY stdin 在 Windows 高负载下不稳定，短命令 P1 E2E 断言直接失败。
- `bifrost-admin` IM Gateway external CLI 测试用 Unix-only delayed command，Windows `taskkill` 遇到 PID 已消失时被判失败。
- HTTPS interception 的 H2 body reset fallback 到 HTTP/1.1 时，body 未按 `Content-Length` 有界读取，客户端把 fallback body 判定为 decode error。
- `skills` registry watcher 的 remove 事件路径在 Windows 上已被删除，无法 `canonicalize`，导致缓存未清理。
- 平台专用测试 helper（iOS cfgutil、Android CA status）在 Windows `--all-targets -D warnings` 下产生 unused import。
- GitHub Actions `Windows Unit Tests (x86_64)` job 主体通过后，`Post Run Swatinem/rust-cache@v2` 在 Windows tar/zstd 保存阶段红灯，把已通过的 job 拖成失败。

本设计把上述失败一次性收敛，并明确 `Windows Unit Tests (x86_64)` job 的 rust-cache 只 restore 不 save。

## 用户目标验证清单

### 必须实现

**运行时 & 单测修复**

- `bifrost-agent` `exec_command` 允许 Windows 高负载下短命令先返回 running session，再通过 `write_stdin` / poll 累积输出到最终 exit code；Windows 上不再把交互式 TTY stdin 作为 P1 E2E 硬性断言。
- `bifrost-admin` IM Gateway external CLI 测试使用平台化 delayed command；Windows `taskkill` 遇到 PID 已消失时视为停止成功。
- HTTPS interception 的 H2 body reset fallback 到 HTTP/1.1 后，先按 `Content-Length` 有界读取响应体；需要跳过 body processing 时仍规范化响应头，避免客户端把 fallback body 判定为 decode error。
- `skills` registry watcher 同时保存 raw root 与 canonical root；Windows remove event 中已删除路径无法 canonicalize 时，仍可从 raw root 计算 slug 并删除缓存项。
- 平台专用测试 helper 随 `cfg(target_os = ...)` 一起收敛，Unix-only fixture import 不得在 Windows `--all-targets -D warnings` 下产生 unused warning。

**CI job 配置**

- `.github/workflows/ci.yml` 的 `test-windows-tray` job（`Windows Unit Tests (x86_64)`，`runs-on: windows-latest`，`timeout-minutes: 60`）显式使用 `Swatinem/rust-cache@v2` 且 `save-if: ${{ false }}`——仅 restore，不在 post-step 保存 cache，避免 Windows tar/zstd 打包超时/失败让 job 尾巴红灯。
- Job 直接执行：

  ```bash
  cargo test --workspace --all-features --target x86_64-pc-windows-msvc \
    -- --skip test_https_interception_websocket_applies_request_and_response_header_rules
  cargo test -p bifrost-tests --test https_proxy_test --all-features \
    --target x86_64-pc-windows-msvc \
    test_https_interception_websocket_applies_request_and_response_header_rules \
    -- --test-threads=1
  ```

  两条命令合起来覆盖整个 workspace，同时把 flakey WebSocket header 用例单独 `--test-threads=1` 跑。
- `aarch64-pc-windows-msvc` 当前只在 `build-cli-windows` / `build-desktop-windows` 编译类 job 里覆盖，没有专门的 Windows aarch64 单元测试 job；本设计不新增。

**本地 VM 验证**

- Windows VM 真实仓库路径：`C:\Users\eden\github\bifrost`。
- 本地 full workspace 验证用 `SKIP_FRONTEND_BUILD=1`，避免前端构建掩盖 Rust Windows 单测问题。
- 至少一次完整执行 `cargo test --workspace --all-features -j1`（本地 VM），记录在 `human_tests/ci-windows-unit-tests.md`。

### 必须不破坏

- 非 Windows 平台行为不变；跨平台差异全部通过 `cfg(target_os = ...)` 缩到 Windows 分支。
- `bifrost-tests` HTTPS fallback 用例继续使用本地 mock TLS server，不访问外网，不启动正式 Bifrost 服务，不修改系统代理。
- macOS / Linux `test` job 保留原 `Swatinem/rust-cache@v2 { save-if: always() }`——那些平台上 tar/zstd 稳定。
- `--skip test_https_interception_websocket_applies_request_and_response_header_rules` 加在第一条命令的 filter，是为了把该用例挪到 `--test-threads=1` 独立执行，不允许直接完全跳过。

### 必须真实验证

- Windows VM 本地跑 `cargo test --workspace --all-features -j1`，输出全绿并记录到 `human_tests/ci-windows-unit-tests.md`。
- 受影响 targeted tests 在 VM 内单独复跑（`bifrost-admin` IM Gateway、`skills` watcher、`bifrost-core` launchd parser、`bifrost-device` iOS/Android helper 编译）。
- `cargo clippy --workspace --all-targets --all-features -j1 -- -D warnings` 在 Windows 上通过。
- GitHub Actions `Windows Unit Tests (x86_64)` 在 CI 上：
  - 主体测试全绿。
  - `Post Run Swatinem/rust-cache@v2` 不再出现因保存 cache 失败让 job 红灯的情况。
- E2E / 集成兜底：`cargo test -p bifrost-tests --test https_proxy_test` 在 Windows VM 内通过。

## 产品语义

本设计不改变 Bifrost 面向最终用户的行为；只把 Windows 平台的稳定性拉平到 macOS / Linux。

- Skills registry watcher 在 Windows 上删除路径时，缓存立即清理，与 macOS/Linux 一致；用户不会看到"已删除的 skill 仍出现在列表"的过时状态。
- HTTPS H2 → HTTP/1.1 fallback 后，客户端拿到的响应体完整且 header 规范；不会因 body decode 错误影响 Chat/IM 场景。

## 技术细节

### CI job 结构（`.github/workflows/ci.yml` L1341–1374）

```yaml
test-windows-tray:
  name: Windows Unit Tests (x86_64)
  runs-on: windows-latest
  timeout-minutes: 60
  steps:
    - uses: actions/checkout@v4
    - uses: pnpm/action-setup@v4
    - uses: actions/setup-node@v4
      with:
        node-version: "22"
        cache: "pnpm"
        cache-dependency-path: |
          pnpm-lock.yaml
          web/pnpm-lock.yaml
    - name: Install root dependencies
      run: pnpm install --frozen-lockfile
    - name: Install frontend dependencies
      run: pnpm install --frozen-lockfile
      working-directory: web
    - uses: dtolnay/rust-toolchain@stable
      with:
        targets: x86_64-pc-windows-msvc
    - uses: Swatinem/rust-cache@v2
      with:
        key: test-windows
        save-if: ${{ false }}          # <-- 仅 restore
    - name: Run Windows unit tests
      # This intentionally verifies Windows can compile/test the workspace
      # without ASR native runtime assets; bifrost-asr full local engines are
      # target-gated to macOS aarch64 and must stay unavailable here.
      shell: bash
      run: |
        cargo test --workspace --all-features --target x86_64-pc-windows-msvc -- --skip test_https_interception_websocket_applies_request_and_response_header_rules
        cargo test -p bifrost-tests --test https_proxy_test --all-features --target x86_64-pc-windows-msvc test_https_interception_websocket_applies_request_and_response_header_rules -- --test-threads=1
```

关键点：

- `save-if: ${{ false }}`：Windows 上 rust-cache 的 tar/zstd 保存阶段会因权限锁 / 时长超时红灯。仅 restore 命中缓存加速，不再落盘。
- 两条 cargo test 命令覆盖所有 workspace crate；WebSocket header 用例强制 `--test-threads=1`。
- 注释明确 `bifrost-asr` 的 `full-local-asr` feature 只在 macOS aarch64 启用，Windows 不构建 ASR native runtime——避免有人误增 dependency。

### 平台专用改动落点

| Crate | 改动 |
| --- | --- |
| `bifrost-agent` | `exec_command` 交互式 stdin 走 platform-gated 分支；Windows 允许 running session + poll。 |
| `bifrost-admin` | IM Gateway external CLI 测试用 delayed command；Windows `taskkill` PID 消失容错。 |
| `bifrost-tests` `https_proxy_test` | H2 body reset fallback 时按 `Content-Length` 有界读；跳过 body processing 前规范化 header。 |
| `skills` | Watcher 同时保存 raw + canonical root；remove event canonicalize 失败时从 raw root 算 slug。 |
| `bifrost-core` `launchd` parser | Windows 编译路径的 cfg 收敛。 |
| `bifrost-cli` upgrade/main | Windows 编译差异用 cfg 缩小。 |
| `bifrost-device` | iOS cfgutil / Android CA status 平台专用 helper 随 target_os cfg 一起 gated，Windows 无 unused import。 |

### 本地 VM

- Path：`C:\Users\eden\github\bifrost`（Parallels VM，Windows 11 x64）。
- 依赖：Visual Studio MSVC environment、rustup stable、Git Bash、LLVM `lld-link`。
- 环境变量：`SKIP_FRONTEND_BUILD=1`；`RUSTC_WRAPPER` 不设置。
- 运行：`cargo test --workspace --all-features -j1`。

## CLI / Web / Admin API / Sync 边界

- CLI：无新增子命令 / flag。
- Web / Admin API：无变化。
- Sync：不影响 sync payload / API 契约。
- README：不需要更新（无用户可见新功能）。

## 实现切分

### Phase 1 — Rust 代码 Windows 修复

- `bifrost-agent` `exec_command` 分支化。
- `bifrost-admin` IM Gateway external CLI 平台化 + `taskkill` 容错。
- `bifrost-tests` HTTPS H2 fallback body 有界读 + header 规范化。
- `skills` watcher raw + canonical root。
- 平台专用测试 helper cfg gate。

### Phase 2 — CI job 配置

- `test-windows-tray` job 加 `save-if: ${{ false }}`。
- Run step 拆成两条 cargo test：workspace skip WebSocket 用例 + WebSocket 用例 `--test-threads=1`。

### Phase 3 — 本地 VM 验证

- Windows VM 跑 `cargo fmt --all -- --check` / `cargo clippy --workspace --all-targets --all-features -j1 -- -D warnings` / `cargo test --workspace --all-features -j1`。
- 结果记录到 `human_tests/ci-windows-unit-tests.md`。

### Phase 4 — 索引

- `human_tests/readme.md` 更新对应索引行。

## 测试方案

### 单元测试

覆盖修复涉及的所有 crate：

- `bifrost-admin`：IM Gateway external CLI Windows `taskkill` PID 消失分支。
- `bifrost-tests`：`https_proxy_test` H2 body reset fallback 用例。
- `skills`：registry watcher raw/canonical remove 事件。
- `bifrost-core`：launchd parser Windows 编译。
- `bifrost-cli`：upgrade/main Windows 编译。
- `bifrost-device`：iOS cfgutil / Android CA status Windows `--all-targets` 编译。

### 集成 & E2E 测试

- `cargo test -p bifrost-tests --test https_proxy_test` 在 Windows VM 通过。
- CI `test-windows-tray` job 在 GitHub Actions 通过。
- Workspace 兜底：至少一次完整 `cargo test --workspace --all-features -j1`（本地 VM）。

### human_tests

- 更新 `human_tests/ci-windows-unit-tests.md`，覆盖：
  1. 五类 Windows 修复的复跑步骤（agent / admin / https / skills / device helper cfg）。
  2. CI `save-if: false` 断言。
  3. 两条 cargo test 命令实际输出片段。
  4. Windows VM 全 workspace `-j1` 通过截图或日志摘要。
- 更新 `human_tests/readme.md` 索引行。

## Review / Fix / Test 闭环

1. 第 1 轮：复核当前 diff / Windows 失败归因 / targeted tests；发现新失败先补最小修复，再跑对应过滤用例。
2. 第 2 轮：复查跨平台 cfg、HTTP header/body 语义、watcher path 匹配；复跑 targeted tests + workspace full test。
3. 第 3 轮：若 clippy / fmt / CI / Windows full test 仍失败，按失败日志追加新轮次，不削弱断言。

## 风险与决策

- 决策：Windows `Swatinem/rust-cache@v2` `save-if: false`——tar/zstd 保存在 Windows hosted runner 上历史多次失败，禁用后损失编译缓存换来 job 稳定性。
- 决策：WebSocket header 用例强制 `--test-threads=1` 而非拆到独立 job——保持同一 job 内一次性覆盖 workspace，减少 CI 复杂度。
- 决策：不新增 `aarch64-pc-windows-msvc` 单元测试 job——arm64 runner 稀缺，且当前 `build-cli-windows` / `build-desktop-windows` 已覆盖编译路径。
- 风险：Skills watcher 双 root 需要在所有 add/modify/remove 事件路径都保持同步；未来新增事件类型必须更新。缓解：在 watcher 测试中覆盖 raw + canonical 两条路径。
- 风险：HTTPS H2 fallback 的 `Content-Length` 有界读若遇到 chunked 响应会退化；本设计只针对 H2 reset → HTTP/1.1 fallback path，chunked 走原逻辑。

## 依赖项

- Windows VM：Visual Studio MSVC environment、rustup stable、Git Bash、LLVM `lld-link`
- 环境变量：`SKIP_FRONTEND_BUILD=1`
- `.github/workflows/ci.yml`（`test-windows-tray` job）
- Rust crates：`bifrost-agent`、`bifrost-admin`、`bifrost-tests`、`skills`、`bifrost-core`、`bifrost-cli`、`bifrost-device`
- 本地 mock TLS server（`bifrost-tests` HTTPS fallback 用例内嵌）

## 校验要求

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -j1 -- -D warnings`
- `cargo test --workspace --all-features -j1`
- 受影响 targeted tests 必须在 Windows VM 仓库内执行
- 静态断言 `test-windows-tray` job 的 `save-if: ${{ false }}` 与两条 cargo test 命令
- GitHub Actions `Windows Unit Tests (x86_64)` job 全绿

## 文档更新要求

- 更新 `human_tests/ci-windows-unit-tests.md`
- 更新 `human_tests/readme.md`
- 本修复不新增 CLI/API 配置项，不需要更新 README
