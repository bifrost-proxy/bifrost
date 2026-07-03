# E2E Script Startup

## 背景

Bifrost 的 Sync 模块（`crates/bifrost-sync`）在进程首次启动时会检查是否已登录，如果本地缺乏有效 credential，会尝试打开系统浏览器完成 OAuth 登录，以便设备间同步规则、Group、traffic history。这在真实用户桌面场景是合理的默认体验，但在 E2E 测试或 CI 场景带来三个问题：

1. 本地 E2E 每次执行都会弹出登录页，浪费用户时间并可能污染开发者浏览器 cookie；
2. GitHub Actions runner 上如果调用启动命令，会尝试打开 headless 浏览器或阻塞在无 UI 的登录环节，导致 job 挂死；
3. CI 中并发 E2E 大量启动 Bifrost 时，同一时间发出多个登录请求，还会污染 Sync 服务端登录日志。

Bifrost 提供了 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1` 环境变量，用于在启动时明确抑制自动登录弹窗。用户既有失误是：新加的 E2E 脚本或 Rust integration test 忘记设置这个变量，导致某次 CI 忽然出现登录弹窗污染。本文档定义"启动 Bifrost 的 E2E 入口默认禁用 Sync 自动登录弹窗"的实现契约以及静态守卫脚本。

## 用户目标验证清单

### 必须实现

- `e2e-tests/test_utils/process.sh` 与 `e2e-tests/test_utils/admin_client.sh` 默认导出 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`，使用 `: "${VAR:=1}"` 语法允许父环境覆盖。
- 所有 `e2e-tests/**/*.sh`、`scripts/**/*.sh`、`tests/**/*.sh` 中直接启动 Bifrost（命中 `BIFROST_BIN`、`target/{debug,release}/bifrost`、`cargo run --bin bifrost` 等启动 hint）的脚本，要么 source 了 `process.sh`/`admin_client.sh`，要么在文件开头显式导出 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`。
- 所有 `crates/**/*.rs` 与 `tests/**/*.rs` 中通过 `CARGO_BIN_EXE_bifrost` 调用 `.arg("start")` 的 Rust integration test，必须在 `Command::env(...)` 中显式设置 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`。
- 同样约束适用于 tray 抑制：脚本必须要么 source helper（helper 内 default `BIFROST_DISABLE_TRAY=1`），要么显式设置 `BIFROST_DISABLE_TRAY=1` 或 `--no-tray`，避免 CI headless 环境弹出 tray icon 或 GUI 依赖初始化失败。
- 提供静态守卫脚本 `e2e-tests/tests/test_e2e_scripts_disable_sync_login_prompt.sh`，一次性扫描全部启动入口并在缺口出现时以非 0 退出。

### 必须不破坏

- `e2e-tests/tests/test_sync_startup_login_preflight_e2e.sh` 是唯一需要真实验证 Sync 启动登录预检行为的脚本，必须留在 `ALLOWLIST` 中，由脚本自身按 case 显式控制 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT` 值。
- 现有 E2E 场景（rules、traffic、agent、im-gateway、admin API 等）继续通过公共 helper 启动，运行行为不变。
- 静态守卫脚本不要引入 rust toolchain 依赖，只做纯 python 文本扫描，避免 CI 需要预热 cargo。
- 用户仍可在自己的开发终端手动执行 `bifrost start` 走登录流程，本方案只约束测试入口。

### 必须真实验证

- 在临时 shell 里跑一次 `bash e2e-tests/tests/test_e2e_scripts_disable_sync_login_prompt.sh`，输出必须为 `All Bifrost startup tests/scripts disable Sync auto-login prompt by default.`。
- 至少运行一个真实使用 helper 的 E2E 脚本（例如 rules 或 admin），确认启动过程中未出现 sync auto-login 请求（观察 stderr / sync 服务端 log）。
- Sync 登录预检专用脚本 `test_sync_startup_login_preflight_e2e.sh` 在父环境设置 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1` 时仍能覆盖所有 case（脚本自身按 case 显式覆盖变量）。

## 产品语义

### 默认关闭，helper 兜底

E2E 环境的默认行为是"不弹登录"。任何"通过公共 helper 启动 Bifrost 的路径"必须自动获得该默认；直接绕过 helper 启动的路径（包括新增脚本或 Rust integration test）必须自己显式声明。守卫脚本负责把两条路径的缺口都识别出来，避免"新脚本忘了声明"这一类容易漏掉的回归。

### Sync 登录预检脚本是唯一例外

`test_sync_startup_login_preflight_e2e.sh` 存在的目的是主动验证 Sync 启动登录预检和禁用开关行为，需要在 case 内切换该变量的值。该脚本被列入 `ALLOWLIST`，守卫脚本不再检查其内部是否声明默认值。

### 静态扫描比运行时探测更可靠

Sync 弹窗触发依赖账号状态、网络状况和 credential 缓存，端到端探测不稳定；用文本扫描可以在最短时间内给出确定判断，且与 CI runner 环境解耦。

## 技术细节

### helper 层默认

```bash
# e2e-tests/test_utils/process.sh
: "${BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT:=1}"
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT

: "${BIFROST_DISABLE_TRAY:=1}"
export BIFROST_DISABLE_TRAY
```

`admin_client.sh` 同步保留 default，避免仅 source `admin_client.sh` 的脚本漏声明。

### 守卫脚本

`e2e-tests/tests/test_e2e_scripts_disable_sync_login_prompt.sh` 用一段 python 完成扫描：

- 遍历 `e2e-tests/`、`scripts/`、`tests/` 下所有 `.sh`，若命中启动 hint 且既未包含 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT`，也未 source `process.sh` / `admin_client.sh`，判定为缺口；tray 缺口同理，接受 `BIFROST_DISABLE_TRAY` 或 `--no-tray`。
- 校验 `process.sh` 自身保留 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT:=1` 与 `BIFROST_DISABLE_TRAY:=1` 的默认值。
- 遍历 `crates/**/*.rs` 与 `tests/**/*.rs`，找出 `CARGO_BIN_EXE_bifrost` + `.arg("start")` 的调用，若未在同一文件设置 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT`，加入缺口。
- 若存在任何缺口，打印分类清单并 `sys.exit(1)`；否则打印 `All Bifrost startup tests/scripts disable Sync auto-login prompt by default.`。

### 启动 hint 列表

守卫脚本识别以下调用形态：

- `BIFROST_BIN`
- `target/debug/bifrost` / `target/release/bifrost`
- `cargo run --bin bifrost`（含 `--release`）
- `"$BIN" start` / `"$BIFROST_BIN" start` / `$BIFROST_BIN start`
- `./target/debug/bifrost start` / `./target/release/bifrost start`

新增其他真实启动路径时同步更新 `START_HINTS`。

### Rust integration test 处理

`crates/bifrost-cli/tests/daemon_shutdown.rs` 与后续任意直接调用 `bifrost start` 的 Rust test 必须显式：

```rust
Command::new(env!("CARGO_BIN_EXE_bifrost"))
    .env("BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT", "1")
    .env("BIFROST_DISABLE_TRAY", "1")
    .arg("start")
```

守卫脚本按 `.arg("start")` 严格匹配，不误报只做 `bifrost status` / `bifrost stop` 的 test。

## CLI / 脚本 / CI 集成

### 本地开发

- 运行任意 E2E 前无需手动设置该变量。
- 新增脚本时优先 `source "${SCRIPT_DIR}/../test_utils/process.sh"`；无法 source 时在文件开头显式声明 default。
- 新增 Rust integration test 时在测试模块内建 helper 函数统一注入 env。

### CI

- `scripts/run_all_e2e.sh` 已 source helper，无需额外声明。
- CI YAML 不需要单独为该变量赋值；守卫脚本作为独立 job 或 rules E2E 前置步骤运行。
- 守卫脚本失败输出中列出所有缺口路径，便于快速修复。

### 手工验证

```bash
bash e2e-tests/tests/test_e2e_scripts_disable_sync_login_prompt.sh
```

期望输出结尾为：

```
All Bifrost startup tests/scripts disable Sync auto-login prompt by default.
```

任何异常输出必须列出具体路径。

## Sync 边界

守卫脚本本身不触发 Sync 服务端调用；它是纯静态扫描。真实运行时若某个 case 出现 sync 登录请求，通常是：

- helper 未 source；或
- Rust test 未设置 env；或
- 有新的启动 hint 未被 `START_HINTS` 收录。

三种情况都由本方案覆盖或明确扩展点。

## 实现切分

### Phase 1：helper 默认与守卫脚本

- 更新 `process.sh` / `admin_client.sh` 默认变量。
- 引入 `test_e2e_scripts_disable_sync_login_prompt.sh`。
- 本地执行一次，确认无缺口。

### Phase 2：清理已有缺口

- 修复 shell 侧缺口（顶层 `e2e-tests/*.sh`、`scripts/**/*.sh` 中直接启动 Bifrost 的入口）。
- 修复 Rust integration test 缺口，覆盖 daemon 相关 test。
- 每修一处，重跑守卫脚本。

### Phase 3：CI 集成

- 在 rules / 全量 E2E 前调用守卫脚本，作为 fast-fail 步骤。
- 若守卫失败，直接跳过后续昂贵的 rules/agent E2E，避免浪费 runner 资源。

### Phase 4：文档与守卫扩展

- 更新 `human_tests/e2e-script-startup.md` 与 `human_tests/readme.md` 索引。
- 记录 `START_HINTS` 与 `ALLOWLIST` 更新流程。
- 后续新增 Sync 相关环境变量时，同一守卫脚本可扩展 tray、system proxy、data dir 等静态守卫。

## 测试方案

### 单元测试

- 无 Rust 单元测试直接对应；行为通过 Rust integration test 与 shell 静态扫描共同覆盖。

### E2E 测试

- `e2e-tests/tests/test_e2e_scripts_disable_sync_login_prompt.sh`：静态扫描全部 shell 和 Rust 启动入口。
- `e2e-tests/tests/test_sync_startup_login_preflight_e2e.sh`：验证 Sync 登录预检脚本自身在父环境设置默认值时仍能覆盖所有 case。
- 至少运行一个使用 helper 的 E2E 脚本（例如 `test_rules_basic.sh`），从 stderr 与 Sync mock 服务端 log 确认没有触发登录请求。

### 真实场景测试 human_tests

- 更新 `human_tests/e2e-script-startup.md`，包含 TC-ESS-01 守卫脚本执行、TC-ESS-02 Sync 登录预检 case 兼容性。
- 每次执行需使用临时 `BIFROST_DATA_DIR`、非 9900 端口、`BIFROST_DISABLE_TRAY=1`、`--no-system-proxy`；`BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT` 保持默认或按 case 显式覆盖。
- 更新 `human_tests/readme.md` 索引。

### 覆盖率与项目校验

实现阶段应运行：

- 守卫脚本自身：`bash e2e-tests/tests/test_e2e_scripts_disable_sync_login_prompt.sh`
- `bash -n` 校验所有修改过的 shell 脚本
- 相关 Rust integration test 编译：`cargo test -p bifrost-cli daemon_shutdown --no-run`
- `cargo fmt --all -- --check`
- `rust-project-validate`

本机 no-local-coverage 约定下不运行 `make coverage` / `make coverage-unit`；交付时说明 coverage 本地豁免。

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核用户目标：helper 默认、shell 缺口清理、Rust integration test 缺口清理、守卫脚本能落到 CI。
- 复核 diff：helper 是否保留双默认、守卫脚本是否覆盖 shell/rust 双侧、`ALLOWLIST` 是否只保留 Sync 登录预检脚本。
- 重点 review：是否存在新增启动 hint 未加入 `START_HINTS`；是否有脚本 source 了不同 helper 但绕过默认。
- 复测：跑守卫脚本、跑一个 helper-based E2E、跑 Sync 登录预检 case。

### 第 2 轮

- 复核第 1 轮修复。
- 重点 review：错误消息稳定性、CI job 集成方式、human_tests 与 readme 索引同步。
- 复测：改动过的 shell 脚本用 `bash -n` 复检、守卫脚本再跑一次、抽查一次 rules E2E。

## 风险与决策点

- **新增启动方式漏检**：如果未来引入新的 wrapper（例如 `bifrost-launcher`），需要显式加入 `START_HINTS`，否则守卫失效。留下扩展点即可。
- **父环境覆盖**：允许父环境覆盖是必要的，Sync 登录预检脚本以及少量调试场景依赖这一能力。方案通过 `: "${VAR:=1}"` 语法保留覆盖能力。
- **性能**：静态扫描一次约扫描千级文件，纯 python 遍历秒级完成，可作为 CI fast-fail 前置。
- **旧 test 迁移**：迁移过程中会短暂看到守卫失败，属于预期；一次性修复所有历史缺口后守卫稳定。
