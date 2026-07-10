# 开发与测试服务隔离

## 背景与问题

Bifrost 的正式本机服务通常监听 `9900`，数据目录为 `~/.bifrost`。开发和 E2E 过去存在三条会破坏该服务的路径：

1. `scripts/run_all_e2e.sh` 对部分串行测试执行 `kill_all_bifrost`，Unix 实现是 `pkill -f bifrost`，会停止机器上所有 Bifrost 实例。
2. 直接执行测试、`make run`、`make dev` 或 human test 时，如果没有显式设置 `BIFROST_DATA_DIR`，源码二进制会读取 `~/.bifrost/runtime.json`。携带 `--yes` 时会先停止该 PID，再由测试实例覆盖 `runtime.json` / `bifrost.pid`。
3. IM provider store 使用非原子 `std::fs::write`，读取到半写 JSON 或不兼容结构时会直接删除唯一配置文件，使短暂的并发/崩溃问题变成永久配置丢失。

## 目标

- E2E 清理只能停止本次测试沙箱拥有的 Bifrost PID。
- `9900` 默认为测试清理保护端口，任何按端口回收都必须拒绝该端口。
- 直接执行 E2E 脚本、`make run`、`make run-release`、`make dev` 和 `scripts/dev-run.sh` 默认使用仓库内独立数据目录、端口 `8800`，并禁用系统代理、托盘和 Sync 登录弹窗。
- IM provider 配置使用同目录原子替换，并维护 last-known-good 备份；主文件损坏时自动恢复，主文件和备份都损坏时保留原文件，不做删除式 reset。
- 保持 CI 对测试残留进程的回收能力，不让泄漏进程占用后续端口。

## 实现逻辑

### 测试进程所有权

测试沙箱根目录写入 `.bifrost-e2e-owned` 标记。`kill_bifrost_in_data_root` 仅扫描该目录下的 `runtime.json`、`bifrost.pid`、`tray.pid`，读取 PID 后再次核验进程名为 `bifrost` / `bifrost.exe`，最后定向 TERM/KILL。未带所有权标记的目录直接拒绝清理。

禁止再按进程名称做 host-wide 清理。`check-e2e-launch-guard.sh` 同时检查：

- Tray 护栏；
- Sync 登录护栏；
- `BIFROST_DATA_DIR` 隔离；
- 不存在裸 `pkill -f bifrost` / `killall bifrost` / 全映像 `taskkill`。

### 端口与数据目录

- `kill_bifrost_on_port` 默认保护 `9900`，可用 `BIFROST_E2E_PROTECTED_PORTS` 设置更多保护端口。
- `process.sh` 在调用方未设置数据目录，或继承到 `~/.bifrost` 及其子目录时，自动改道到 `.bifrost-e2e-runs/direct/data-*`。ownership marker 和清理函数也都会先拒绝整个正式目录树。
- Make 开发入口使用 `.bifrost-dev` 和 `8800`，可通过 `BIFROST_DEV_DATA_DIR` / `BIFROST_DEV_PORT` 显式覆盖；`scripts/dev-run.sh` 使用同样的仓库根目录默认值，并允许通过 `BIFROST_DATA_DIR` / `BIFROST_DEV_PORT` 覆盖。

### IM provider 持久化

每次保存先验证当前主文件：

- 当前主文件有效：原子写入 `.bak` 作为 last-known-good；
- 首次保存：用新内容初始化 `.bak`；
- 当前主文件无效：保留现有 `.bak`，禁止用坏内容覆盖恢复点；
- 主文件通过同目录临时文件 `write + fsync + persist/rename` 原子替换。

启动读取顺序为主文件、备份。主文件缺失或无效而备份有效时，内存加载备份并原子恢复主文件；两者都无效时记录 warning，保留两份原始文件并以内存空列表降级。此时 store 进入只读保护，`add/update/delete` 全部拒绝，防止空内存状态覆盖恢复证据；人工修复文件并重启后才恢复写入。

## 依赖项

- 使用 `bifrost-admin` 已有的 workspace `tempfile` 依赖，不引入新 crate。
- E2E 使用现有 `process.sh`、`assert.sh`、随机端口和 release Bifrost 二进制。

## 测试方案

### 单元测试

- `atomic_save_seeds_valid_recovery_backup`：首次保存同时得到有效主文件和备份，无残留临时文件。
- `truncated_primary_recovers_provider_from_backup`：截断主文件后重建 store，provider 自动恢复且主文件重新合法。
- `invalid_primary_without_backup_is_preserved`：无备份时坏主文件原字节保留。
- `invalid_backup_does_not_delete_invalid_primary`：双损坏时两份文件均不删除。
- `unsupported_version_without_backup_is_preserved`：不兼容版本也不会触发删除式 reset。

### E2E

`test_e2e_process_cleanup_isolation.sh` 先断言继承的正式数据目录被自动改道，且正式根目录即使存在 marker 也不允许扫描清理。然后启动两个随机端口实例：一个位于带 ownership marker 的测试沙箱，一个位于沙箱外模拟正式服务。执行测试清理后断言：

- 沙箱实例退出；
- 沙箱外实例仍可通过 Admin API；
- 受保护端口清理被拒绝；
- 沙箱外 IM provider 文件哈希不变。

### 真实场景测试

`human_tests/development-service-isolation.md` 在真实 9900 服务上只做只读快照，执行隔离 E2E 后复核 PID、版本、数据目录、provider 列表和 `im_gateway_providers.json` 哈希均不变。

## Review/Fix/Test 闭环

1. 第 1 轮重点检查 shell 跨平台、PID 复用、ownership marker、9900 保护、原子写错误路径；运行 provider store 单测、launch guard 和隔离 E2E。
2. 第 2 轮复查修复后的 diff、文档/human_tests 索引和真实 9900 不变量；复跑定向测试后再执行 `rust-project-validate` 与 workspace all-features。
3. CI 闭环复查 Shell E2E 的串行 cleanup 作用域；cleanup 必须在 `run_shell_test_isolated` 内使用函数自身的 `shell_data_dir`，不得在调用层引用已离开作用域的局部变量。

## 校验要求

- `bash scripts/ci/check-e2e-launch-guard.sh`
- `cargo test -p bifrost-admin im_gateway::provider_store::tests --lib`
- `SKIP_BUILD=true bash e2e-tests/tests/test_e2e_process_cleanup_isolation.sh`
- `cargo fmt --all -- --check`
- `cargo fmt --manifest-path desktop/src-tauri/Cargo.toml --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo build --all-targets --all-features`
- `cargo test --workspace --all-features`
- 远端 CI `bash scripts/ci/coverage-all.sh --json --gate`

## 文档更新

- `AGENTS.md`：把数据目录、9900 和 host-wide kill 加入测试启动红线。
- `.agents/skills/e2e-test/07-真实人类测试.md`：真实数据验证改为复用已运行主服务，不再用源码重启正式目录。
- `e2e-tests/readme.md`：删除全机 pkill 排障建议。
- `human_tests/development-service-isolation.md` 与 `human_tests/readme.md`：新增真实回归和索引。
