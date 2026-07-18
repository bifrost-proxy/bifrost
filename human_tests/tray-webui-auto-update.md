# Tray 与 WebView 后台升级真实场景测试

## 功能模块说明

验证托盘/Admin/WebView 触发的后台升级链路，重点覆盖下载已完成、磁盘二进制已更新但运行中的 daemon 仍是旧版本时，后台 `self-update` 必须重启旧进程，WebView 也不能在进度被清理后一直卡在 Working 状态。

## 前置条件

1. 使用当前工作区编译出的 Bifrost 二进制，所有测试必须使用临时 `BIFROST_DATA_DIR`，避免污染正式服务数据。
2. 启动测试服务时设置：
   ```bash
   BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1
   BIFROST_DISABLE_TRAY=1
   ```
3. 除非用例明确验证系统代理，否则启动服务必须带 `--no-system-proxy`。
4. 对 9900 现场排查只允许读取状态、日志和 API 响应；不得重启或停止用户当前 9900 服务。

## 测试用例

### TC-TWA-01：9900 现场版本错位诊断

**操作步骤**：
1. 查询 9900 端口监听进程，记录 PID 和启动命令。
2. 请求 `http://127.0.0.1:9900/_bifrost/api/system/overview`，记录运行中服务版本。
3. 执行 `command -v bifrost` 确认实际安装路径，再执行 `bifrost --version` 记录磁盘二进制版本。
4. 请求 `http://127.0.0.1:9900/_bifrost/api/system/version-check?refresh=true`，记录 `current_version`、`latest_version`、`has_update`。
5. 查询 `http://127.0.0.1:9900/_bifrost/api/system/upgrade/progress`，并检查 `~/.bifrost/logs/bifrost.log` 中最近一次 `admin upgrade: spawned self-update subprocess` 记录和对应子进程状态。

**预期结果**：
- 9900 服务仍可响应 Admin API。
- 若运行中版本小于磁盘版本，结论必须明确标注为“磁盘二进制已更新但旧 daemon 未重启”。
- 若 self-update 子进程已退出且旧 daemon 仍在，结论必须指向后台 latest 分支未重启，而不是下载卡住。
- 不修改 9900 服务状态。

### TC-TWA-02：后台 self-update 在磁盘二进制已是 latest 时仍重启 daemon

**操作步骤**：
1. 使用临时目录复制当前 `target/release/bifrost` 或 `target/debug/bifrost` 到测试安装路径。
2. 使用该安装路径启动 daemon：
   ```bash
   BIFROST_DATA_DIR=<tmp>/data \
   BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 \
   BIFROST_DISABLE_TRAY=1 \
   <tmp>/install/bifrost start -p <free-port> --host 127.0.0.1 --daemon --access-mode allow_all --skip-cert-check --no-system-proxy --no-intercept -y
   ```
3. 记录旧 PID。
4. 删除测试 data dir 中的 `runtime.json` 与 `bifrost.pid`，确认旧 PID 仍存活且 Admin API 仍可访问。
5. 将 `BIFROST_UPGRADE_TEST_LATEST_VERSION` 设置为当前磁盘二进制版本，并把旧 PID 与真实监听端口作为成对的 Admin hint 执行：
   ```bash
   BIFROST_DATA_DIR=<tmp>/data \
   BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 \
   BIFROST_DISABLE_TRAY=1 \
   BIFROST_UPGRADE_TEST_ALLOW_RELEASE_OVERRIDES=1 \
   BIFROST_UPGRADE_TEST_LATEST_VERSION=<current-version> \
   <tmp>/install/bifrost self-update --target <current-version> --source admin \
     --running-proxy-pid <old-pid> --running-proxy-port <free-port>
   ```
6. 等待 Admin API 恢复，读取新 PID、新的 runtime marker 和 `<tmp>/data/upgrade-progress.json`。

**预期结果**：
- `self-update` 命令退出码为 0。
- updater 输出明确记录已从 live Admin listener 恢复缺失的 runtime marker。
- 新 PID 与旧 PID 不同，且新 PID 存活。
- `upgrade-progress.json` 的 `phase` 为 `completed`。
- 测试服务可以正常 stop 并释放端口。

### TC-TWA-03：Admin API 后台升级完整 download/install/restart 链路

**操作步骤**：
1. 执行 `bash e2e-tests/tests/test_upgrade_admin_api_restart_e2e.sh`。
2. 观察 `POST /_bifrost/api/system/upgrade` 是否返回 active progress。
3. 观察 progress 是否到达 `completed`。
4. 观察重启后 PID 是否变化，运行路径是否仍指向测试安装路径。

**预期结果**：
- Admin POST upgrade 被接受，source 为 `admin`。
- progress 最终为 `completed`。
- 重启后 PID 变化，Admin API 恢复可访问。
- 新增的 already-latest 子用例也通过。

### TC-TWA-04：WebView active upgrade 遇到 idle progress 后退出 Working 状态

**操作步骤**：
1. 将 `useVersionStore` 置为 `upgrading=true` 且 `upgradePhase=installing`。
2. 模拟 `GET /system/upgrade/progress` 返回 `phase=idle`。
3. 运行升级轮询。
4. 检查 store 状态和 version-check 调用。

**预期结果**：
- `upgrading` 变为 `false`。
- `upgradePhase` 变为 `idle`。
- 无连接断开时，触发一次 `checkVersion({ forceRefresh: true, skipCache: true })`。
- 页面不会继续显示 Updating/Working 的升级弹窗。

### TC-TWA-05：后台升级子进程诊断日志与退出回收

**操作步骤**：
1. 触发一次 Admin 或 `self-update --source admin` 后台升级。
2. 检查测试数据目录下 `logs/upgrade-background.log`。
3. 查询后台升级子进程退出后是否仍有由 daemon 持有的 defunct child。

**预期结果**：
- 能在 `upgrade-background.log` 中看到后台升级输出或错误。
- 父进程存活且子进程退出时，不留下长期 defunct child。

### TC-TWA-06：Tray helper 自主低频后台检查更新与当前版本展示

**操作步骤**：
1. 使用临时数据目录，不启动 Web UI version-check，直接写入/读取 tray 共享的 `version_cache.json`。
2. 执行 tray 单元回归：
   ```bash
   cargo test -p bifrost-cli test_menu_running_state
   cargo test -p bifrost-cli test_tray_update_cache_missing_or_stale_requires_fetch
   cargo test -p bifrost-cli test_detect_update_available_uses_tray_cache_without_network
   ```
3. 执行真实 tray helper 启动回归：
   ```bash
   BIFROST_BIN="$PWD/target/debug/bifrost" SKIP_BUILD=true bash e2e-tests/tests/test_cli_tray_startup_ci.sh
   ```
4. 检查测试断言是否覆盖：菜单展示不可点击的当前版本号、缓存缺失需要后台检查、新鲜缓存不联网、过期缓存需要后台检查、缓存中存在更高版本时 tray 能识别为可更新。

**预期结果**：
- 测试命令退出码为 0。
- tray 菜单状态行下方展示当前版本号，例如 `Version v0.0.110`，且该信息行不可点击。
- tray 更新提示不再依赖 Web UI 先写缓存。
- tray 对新鲜缓存不发起高频 GitHub 请求；过期或缺失缓存才会进入后台检查路径。
- Windows CI 显式 log-only 降级模式只要求 `bifrost-tray starting` 启动标记，不要求后台线程日志，因为该模式表示无交互 runner 上 helper 可能无法长驻。

### TC-TWA-07：CLI 联动 App 更新不得提前发布 completed

**操作步骤**：
1. 执行 `cargo test -p bifrost-cli nested_cli_upgrade_does_not_publish_terminal_app_progress`。
2. 检查 `bifrost upgrade` 联动 App 时使用的内部 source 仍为 `cli-upgrade`。
3. 检查 `write_app_progress` 对普通 `desktop` / `cli` source 仍写共享进度，但对 `cli-upgrade` source 不写入。
4. 执行 marker 恢复 E2E，确认最终 `upgrade-progress.json` 由外层 self-update 写为 `completed`：
   ```bash
   BIFROST_BIN="$PWD/target/debug/bifrost" \
     bash e2e-tests/tests/test_upgrade_cli.sh --only-runtime-marker
   ```

**预期结果**：
- 单元测试退出码为 0。
- 嵌套 App 更新不会在 daemon 重启前让 Web UI 观察到 terminal `completed`。
- marker 恢复 E2E 最终仍得到 `phase=completed`，且新 daemon PID 与旧 PID 不同。

### TC-TWA-08：CLI-owned 与 App-owned core 升级所有权互斥且都更新 CLI + App

**操作步骤**：
1. 构建当前 debug CLI：
   ```bash
   SKIP_FRONTEND_BUILD=1 cargo build -p bifrost-cli --bin bifrost
   ```
2. 执行双 runtime 所有权回归：
   ```bash
   BIFROST_BIN="$PWD/target/debug/bifrost" \
     bash e2e-tests/tests/test_upgrade_cli.sh --only-runtime-ownership
   ```
3. 在 macOS 执行 App-owned Admin 实链路：
   ```bash
   BIFROST_BIN="$PWD/target/debug/bifrost" \
     bash e2e-tests/tests/test_upgrade_app_owned_core_e2e.sh
   ```
4. 执行桌面重启 handoff 合约：
   ```bash
   bash e2e-tests/tests/test_desktop_upgrade_handoff_contract.sh
   ```

**预期结果**：
- CLI-owned daemon 即使 runtime marker 丢失，也由 `self-update` 精确校验 PID/端口、恢复 marker 并从旧 PID 重启到新 PID。
- CLI-owned foreground core 不会被误判成 App-owned；验证 PID/端口后接续为新版本 detached daemon。
- App-owned core 的 runtime marker 保持 `runtime_start_mode=desktop`；CLI updater 即使被直接调用也不得停止或重启该 PID。
- App-owned core 收到普通浏览器的 `channel=cli` 请求时返回 409 和桌面 App 引导，不安装任何组件；只有桌面 shell 的 `channel=desktop` 才启动 desktop orchestrator。
- desktop orchestrator 先调用独立 CLI 的 `upgrade -y`，并设置内部 `skip_app=1`、`skip_restart=1`，禁止递归更新 App 或抢占 core 重启；随后真实替换 App bundle。
- App-owned upgrade 达到 `completed` 时 core 仍存活，随后仅由 Tauri upgrade handoff 负责停止旧 App/core 并拉起新 App/core。
- 任一路径只有在 CLI 与已安装 App 的伴随更新都成功后才写 `completed`；伴随更新失败必须写 `failed`，不得部分成功却对 UI 宣告完成。
- CLI-owned 路径的 App 伴随更新失败时，旧 daemon 必须保持原 PID 和可用状态，不得在组件未齐备时提前重启。

### TC-TWA-09：升级状态机并发、版本漂移、软失败与超时回归

**操作步骤**：
1. 执行 Admin 实链路；脚本会同时发出两个 POST：
   ```bash
   BIFROST_BIN="$PWD/target/debug/bifrost" \
     bash e2e-tests/tests/test_upgrade_admin_api_restart_e2e.sh
   ```
2. 执行跨进程升级锁和 App-owned CLI 门禁单测：
   ```bash
   cargo test -p bifrost-cli cross_process_upgrade_lock_allows_only_one_owner --lib -- --nocapture
   cargo test -p bifrost-cli desktop_managed_cli_upgrade_cannot_reenter_app_or_restart_its_core --lib -- --nocapture
   cargo test -p bifrost-cli installed_cli_version_must_match_the_pinned_target --lib -- --nocapture
   cargo test -p bifrost-cli script_installs_use_the_target_aware_atomic_upgrade_path --lib -- --nocapture
   cargo test -p bifrost-cli homebrew_restart_uses_stable_launcher_outside_versioned_cellar --lib -- --nocapture
   cargo test -p bifrost-cli homebrew_upgrade_commands_are_bounded_and_verify_formula_target --lib -- --nocapture
   cargo test -p bifrost-cli macos_app_swap_preserves_old_bundle_when_staging_is_invalid --lib -- --nocapture
   cargo test -p bifrost-cli windows_deferred_install_pins_target_and_respects_parent_progress_ownership --lib -- --nocapture
   ```
3. 执行 App-owned 实链路，检查独立 CLI 收到与 App 相同的 pinned target，并在 `--version` 核验后才安装 App：
   ```bash
   BIFROST_BIN="$PWD/target/debug/bifrost" \
     bash e2e-tests/tests/test_upgrade_app_owned_core_e2e.sh
   ```
4. 执行 Web UI 状态回归：
   ```bash
   pnpm --dir web exec vitest run src/stores/useVersionStore.test.ts
   python3 -m unittest scripts/ci/tests/test_coverage_diff.py
   ```

**预期结果**：
- 两个并发 POST 中恰好一个返回 202，另一个返回 409；最终只发生一次安装与一次 daemon 重启。
- 主组件已是 target、伴随组件仍旧时，version-check 仍返回 `has_update=true`，避免 CLI/App 部分升级后更新入口消失。
- 跨进程 `upgrade.lock` 同时只允许一个 owner，释放后可再次获取。
- App-owned 独立 CLI 同时收到 `skip_app=1`、`skip_restart=1` 和与 App 一致的 target；退出 0 但 `--version` 不等于 target 时整体失败。
- CLI-owned 的 Script/Homebrew/Manual 非 deferred 安装也必须在 App 更新和 core 重启前核验实际 CLI target；Script 渠道不得重新追随变化后的 latest，Homebrew 重启不得依赖会被 reinstall 删除的 Cellar 路径。
- 独立 CLI 卡住时在有界超时后被终止，等待期间持续写 Installing 心跳，不会跨过 120 秒 stale 门限。
- App 包先在 staging 中完成复制和版本核验；staging 无版本或版本错误时旧 App bundle 仍保持原版本可启动。
- 模拟 App 已被移到 backup 后中断时，下一次尝试先恢复旧 App；Windows deferred 路径在调度替换前完成 App 同版本门禁，并在 CLI target 核验失败时保留/恢复旧 exe。
- Tauri restart invoke 失败时 Web UI 显示 `failed` 和真实错误，不执行普通 reload、不保留“已成功”状态。
- 点击 Retry 只重试 Tauri handoff，不重新请求已经因 App 到达 target 而返回“无更新”的安装 API。
- helper 无法打开新 App 或新 App 无法拉起 managed core 时，持久化 progress 必须从预完成状态改为 `failed`；只有新 core ready 后才刷新最终 `completed`。

### TC-TWA-10：PR review 升级恢复与调用边界回归

**操作步骤**：
1. 构建当前 CLI，并运行升级评论对应的定向单元测试：
   ```bash
   SKIP_FRONTEND_BUILD=1 cargo build -p bifrost-cli --bin bifrost
   SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-cli --lib upgrade_ --no-fail-fast
   SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-cli --lib commands::app::tests --no-fail-fast
   SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin handlers::system::tests --lib --no-fail-fast
   cargo test --manifest-path desktop/src-tauri/Cargo.toml restart_handoff_setup_failure -- --nocapture
   cargo test --manifest-path desktop/src-tauri/Cargo.toml deferred_desktop_installer_marker -- --nocapture
   cargo test --manifest-path desktop/src-tauri/Cargo.toml deferred_desktop_install_completion -- --nocapture
   pnpm --dir web exec vitest run src/stores/useVersionStore.test.ts
   SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-cli top_level_app_upgrade_owns_the_shared_lock_but_nested_companion_skips_it --lib -- --nocapture
   SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-cli background_upgrade_preserves_progress_owned_by_pending_desktop_handoff --lib -- --nocapture
   SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-cli windows_deferred_install_pins_target_and_respects_parent_progress_ownership --lib -- --nocapture
   python3 -m unittest scripts/ci/tests/test_coverage_diff.py
   ```
2. 执行 App-owned 真实 Admin 链路，验证浏览器请求被拒绝、桌面请求仍完成 CLI + App 安装：
   ```bash
   BIFROST_BIN="$PWD/target/debug/bifrost" \
     bash e2e-tests/tests/test_upgrade_app_owned_core_e2e.sh
   ```
3. 执行升级恢复与 desktop handoff 合约：
   ```bash
   BIFROST_BIN="$PWD/target/debug/bifrost" \
     bash e2e-tests/tests/test_upgrade_restart_e2e.sh
   bash e2e-tests/tests/test_desktop_upgrade_handoff_contract.sh
   ```
4. 执行 direct desktop CLI 终态和 WebView owner 分流回归：
   ```bash
   BIFROST_BIN="$PWD/target/debug/bifrost" \
     bash e2e-tests/tests/test_desktop_app_update_cli.sh
   pnpm --dir web exec vitest run src/stores/useVersionStore.test.ts
   ```
5. 检查模块行数和文档可移植性：
   ```bash
   test "$(wc -l < crates/bifrost-cli/src/commands/app.rs)" -le 1500
   test "$(wc -l < crates/bifrost-cli/src/commands/app/installer.rs)" -le 1500
   test "$(wc -l < crates/bifrost-cli/src/commands/app/tests.rs)" -le 1500
   test "$(wc -l < crates/bifrost-cli/src/commands/upgrade.rs)" -le 1500
   test "$(wc -l < crates/bifrost-cli/src/commands/upgrade/download.rs)" -le 1500
   test "$(wc -l < crates/bifrost-cli/src/commands/upgrade/restart.rs)" -le 1500
   test "$(wc -l < crates/bifrost-cli/src/commands/upgrade/tests.rs)" -le 1500
   local_home='/Users/'eden_studio
   ! grep -q "$local_home" human_tests/tray-webui-auto-update.md
   ```

**预期结果**：
- 跨进程锁 loser 立即得到 `Failed` progress，不会停留在 Checking。
- 匹配的 restartable runtime marker 在 `lsof` 不可用时仍可升级；marker 恢复的缺省 host 是 `127.0.0.1`。
- macOS App 使用稳定 backup 名跨 updater PID 恢复；Windows App-owned CLI 版本探针等待 deferred replacement。
- CLI 安装后的 exact target 不匹配时恢复旧 binary backup。
- CLI-owned companion 可显式传递检测到的 App parent；Admin desktop orchestrator 不传 `--app-dir`，让 bundled core 从自身 executable 解析当前运行 App，避免多副本时更新错误副本。
- foreground CLI runtime 与 daemon 一样归 CLI updater 重启，只有 `RuntimeStartMode::Desktop` 会交给 Tauri handoff。
- Windows App-owned MSI/EXE 不在 App/sidecar 仍运行时执行；pending marker 交给 Tauri helper，在旧 App/core 退出后有界安装，失败写 `Failed`，成功才拉起新 App。pending handoff 拒绝后台竞争者时不得把原有 `Restarting/source=desktop` 覆盖成 `Failed/source=admin`。
- Windows deferred 安装只在新 App 的编译版本等于 pinned target 时写 `Completed`；安装器成功但拉起旧/错误版本时写 `Failed`。
- pending marker 区分 updater 下载包与调用者传入包；handoff 成功后只删除前者，保留用户的 `--package` 文件。
- Desktop shell 复用 CLI-owned core 时，`source=admin` 的 `Restarting` 继续由 CLI owner 收口，不触发 Tauri App handoff。
- Windows deferred installer 在旧 App/core 退出后的整个安装窗口继续持有 pending-marker guard，CLI、tray 与 App updater 都不能取得共享升级锁；成功或失败后 guard 都会释放，陈旧 marker 不会永久阻塞后续更新。
- 只有 Admin/Tauri 发起且带 handoff 标记的 `source=desktop` 才停在 `Restarting` 交给当前 App；用户直接执行 `bifrost app upgrade --source desktop --no-cli` 会自行写 `Completed`。Desktop shell 观察到 CLI-owned `source=admin` 的 `Completed` 时只刷新 WebView，不重启 App。
- changed-lines 95% 门禁只排除从原文件真实消失、至少 8 行且至少 4 行实质代码的机械搬移块；仍保留在原文件的 copy-paste 必须继续计入门禁，小样板和真实修改行也仍计入，报告显示排除行数。
- 顶层 App updater 与 self-update 共用跨进程 `upgrade.lock`，并发 App/CLI updater 只能有一个 owner；`source=cli-upgrade` 只有同时携带父事务私有 marker 才能绕过父锁，用户仅伪造可见 source 不能绕过。App 管理的 CLI child 必须固定 target、禁止递归更新 App/重启 core；Windows deferred helper 直接使用该固定 target，且由 App 父事务收口时不得提前发布 `Completed`。
- native desktop restart marker/helper 失败会持久化 `Failed`，刷新后不会重新显示旧 `Completed`。
- 普通浏览器不能启动 desktop-owned 安装；桌面 shell 请求仍把 CLI 与 App 一起升级。
- `app.rs`、`app/installer.rs`、`app/tests.rs` 以及 `upgrade.rs`、`upgrade/download.rs`、`upgrade/restart.rs`、`upgrade/tests.rs` 均不超过 1500 行，测试文档不包含本机绝对路径。

## 清理步骤

1. 停止测试数据目录中的 Bifrost 服务：
   ```bash
   BIFROST_DATA_DIR=<tmp>/data <tmp>/install/bifrost stop
   ```
2. 删除临时测试目录。
3. 不清理、不停止、不重启用户正在运行的 9900 服务。

## 执行记录

2026-07-18 本次状态机审计已执行（最终复测）：

- TC-TWA-08/09/10（第五轮完整复测）：通过。App-owned 真实 Admin 链路 `17/17`，CLI runtime ownership `4/4`，CLI restart `21/21`，Admin 真实下载→安装→版本核验→daemon 重启 `15/15`，direct desktop CLI `36/36`，native handoff 为 `5/5 + 1/1 + 1/1 + 1/1`。CLI 全量 lib `1233 passed / 2 ignored`；strict clippy、all-targets build、fmt 全绿；workspace 首轮唯一失败是无关 rule-share 测试并发创建默认 RulesStorage 命中 `AlreadyExists`，精确复跑 `1/1` 后以 `--test-threads=1` 复跑整个 workspace 退出码 0。第 1 轮 review 修正 child marker 误加到错误 helper、pending handoff 二次检查竞态和 coverage 短片段误排除；第 2 轮将 CLI lock bypass 收紧为 marker + skip-App + skip-restart + pinned-target 缺一不可，并复跑定向单测、coverage 工具 `11/11` 与 restart E2E `21/21`。所有真实链路使用临时目录和随机端口，already-latest fixture 显式隔离本机 App 安装目录；未操作 9900 或真实 `/Applications/Bifrost.app`。
- TC-TWA-09/10（PR comments 第五轮定向回归）：通过。父锁/私有 child marker、pending desktop handoff progress 保留、Windows deferred pinned target 与 progress owner、direct App 固定 target 四个定向单测均为 `1/1`；coverage-diff 工具测试为 `11/11`，新增断言证明原位置仍保留的 copy-paste 代码不会被 changed-lines 门禁排除，保留块切分出的短片段也不能伪装成搬移。App-managed child 只有同时携带 skip-App、skip-restart、pinned-target 和 parent-lock marker 才复用父锁；仅伪造 `source=cli-upgrade` 或 marker 会被共享锁拒绝。全部测试使用临时目录，未启动或修改 9900 服务。
- TC-TWA-10（CI fixture 版本核验）：通过。Linux Shell CI 首轮 158/159，唯一失败是 Admin API E2E 将当前 `0.0.156` 二进制直接放进命名为 `0.0.157` 的归档，新的安装后版本门禁正确拒绝该假 fixture。fixture 改为只在临时二进制副本中等长替换编译版本字节，打包前真实执行 `--version` 校验；macOS 临时副本重新 ad-hoc codesign。随后真实 Admin POST 升级、原子替换、版本核验、daemon 重启与 already-latest 路径复测为 `15/15`，使用临时目录和随机端口，未操作 9900。
- TC-TWA-10（PR comments 第四轮）：通过。Windows deferred pending marker 的 active/stale guard 定向测试 `1/1`，App-owned handoff transaction `1/1`，CLI interactive wrapper/shared lock `1/1`，Web owner 分流 `5/5`，desktop PowerShell guard 清理合约 `1/1`。真实 `test_desktop_app_update_cli.sh` 为 `36/36`，证明 direct `app upgrade --source desktop --no-cli` 安装后写 `completed` 而非永久停在 `restarting`；CLI-owned `source=admin` 的 `completed` 在 desktop shell 中只 reload WebView，不调用 Tauri App handoff。pending marker 在 process lock 释放后继续拒绝 CLI/tray owner，成功与失败路径均移除 guard，10 分钟外的陈旧 marker 不再阻塞。全部使用临时目录和随机端口，未操作 9900。
- TC-TWA-10（PR comments 第三轮）：通过。App 定向单测 `23/23`；Tauri deferred marker/版本核验 `2/2`，desktop handoff 合约为既有 marker `5/5` + setup failure `1/1` + deferred marker `1/1` + deferred target verification `1/1`；Web 状态机 `4/4`，证明 desktop shell 观察到 CLI-owned `source=admin` 的 `Restarting` 时不会调用 Tauri handoff。App-owned Admin 实链路 `17/17`，CLI restart E2E 首轮 `20/21` 暴露测试合约仍受 1500 行门禁和旧静态断言约束，收窄调用格式并补 package ownership、deferred target verification、source-gated handoff 断言后复跑 `21/21`。Windows pending marker 以 `package_owned_by_updater` 区分下载包和调用者 `--package`，PowerShell 只清理前者；新 managed core ready 后，Tauri 还会比较 relaunched App 编译版本与 pinned target，不一致时写 `Failed` 而不是假 `Completed`。全部实链路使用临时目录与随机端口，未操作 9900。
- TC-TWA-10（新增 review comments）：通过。CLI upgrade `53/53`、App 最终 `23/23`、Admin `16/16`、Tauri handoff 定向 `2/2`、Web 状态机 `3/3`；新增 App 单测证明顶层 App/self-update 争用同一个 lock 时只有一个 owner、内部 companion 不死锁，并用伪造的后续 `latest=99.0.0` 证明直接 App upgrade 的 CLI 仍严格使用已解析 target。App-owned Admin 实链路 `17/17`，终态在旧 App/core 仍存活时保持 `restarting`，等待 Tauri 独占 handoff；CLI restart E2E 首轮因模块拆分后的 shell 测试仍只扫描旧单文件而出现 3 个测试缺陷，修正为按职责扫描 root/restart 子模块后复跑 `21/21`；desktop handoff 为既有 `5/5` + failure `1/1` + deferred installer `1/1`。第一轮 review 还发现若复用第二个 desktop executable 作为 Windows helper，会继续持有 App 文件锁，现已改为独立 PowerShell handoff；其 MSI 参数显式引用含空格路径，并接受 0、1641、3010 成功码。macOS desktop Admin 不再传候选 `--app-dir`，foreground runtime 归 CLI updater，Windows pending MSI/EXE 由 helper 在旧 App/core 退出后执行。App 与 upgrade 的 7 个相关模块最终分别为 1500、199、735、1500、539、639、1332 行，均小于等于 1500；shell 语法和文档本机路径检查通过。所有实链路均使用临时数据目录与随机端口，未操作 9900。
- TC-TWA-10：通过。定向单测为 CLI upgrade `51/51`、App installer `20/20`、Admin system handler `16/16`、native restart failure `1/1`；`test_upgrade_app_owned_core_e2e.sh` 为 `17/17`，证明普通浏览器请求 desktop-owned core 返回 409 且不修改 App/CLI，桌面 shell 请求随后同时完成 CLI 与 App 的 pinned-target 更新；`test_upgrade_restart_e2e.sh` 为 `21/21`，覆盖 lock loser 终态、无 `lsof` marker 复用、loopback 恢复、CLI mismatch 回滚、稳定 App backup 和 app-dir 传递合约；`test_desktop_upgrade_handoff_contract.sh` 的既有 handoff 测试 `5/5` 与新增失败持久化测试 `1/1` 均通过。覆盖率门禁反馈后将纯机械搬移收窄为 installer command 子模块，并为 Linux 非测试构建增加平台 cfg；三个 App 模块最终分别为 1485、94、623 行。定向 App 单测 `20/20`、CLI restart E2E `21/21`、fmt 与 bifrost-cli clippy 再次通过。文档本机路径检查与三个 shell 语法检查均通过。所有服务均使用临时数据目录和随机端口，未操作用户正在运行的 9900 服务。
- TC-TWA-09 progress owner 隔离回归：通过。执行 `SKIP_FRONTEND_BUILD=1 cargo build -p bifrost-cli --bin bifrost` 后，以 `BIFROST_BIN="$PWD/target/debug/bifrost" SKIP_BUILD=true bash e2e-tests/tests/test_upgrade_admin_api_restart_e2e.sh` 真实验证；临时 daemon 在端口 52152 从 PID 17754 升级重启为 PID 18197，并发 POST 为 `409 + 202` 且只启动一个 updater，终态保持 `completed`；already-latest 从 PID 18433 重启为 PID 18570。最终 `15/15` 通过，证明非 owner 线程的 Installing/Restarting 事件不会覆盖当前 upgrade transaction 的终态。
- TC-TWA-09：通过。最终构建后 `test_upgrade_admin_api_restart_e2e.sh` 为 `15/15`，两个并发 POST 得到 `202 + 409`，只启动一个 updater，临时 daemon 从 PID 40482 重启为 PID 41110；already-latest 从 PID 41351 重启为 PID 41489。`test_upgrade_cli.sh --only-runtime-ownership` 为 `4/4`，`test_upgrade_app_owned_core_e2e.sh` 为 `13/13`，`test_desktop_upgrade_handoff_contract.sh` 为 `5/5`。跨进程锁、desktop-managed CLI pinned target/版本门禁、macOS App staging 失败和 interrupted-backup 恢复、Windows deferred App 门禁/CLI target 核验/失败回滚的定向测试全部通过。独立 CLI 收到与 App 相同的 `target=99.0.1`，App bundle 真实替换且 core 在 Tauri handoff 前保持原 owner。`useVersionStore.test.ts` 为 `3/3`；全量 Web 单测 `173/173`、lint（仅 14 个既有 warning）和 production build 均通过。

2026-07-17 本次修复已执行：

- TC-TWA-01：通过。9900 监听进程为 PID 22956，Admin overview 返回运行版本 `0.0.155`；`command -v bifrost` 返回 `~/.local/bin/bifrost`，磁盘 CLI 返回 `0.0.156`；强制 version-check 返回 `current_version=0.0.155`、`latest_version=0.0.156`、`has_update=true`；upgrade progress 为 `idle`，且 `~/.bifrost/runtime.json`、`~/.bifrost/bifrost.pid` 均缺失。结论为磁盘 CLI 已完成替换，但旧 daemon 因运行时标记缺失没有重启。全程未修改 9900 服务状态。
- TC-TWA-02：通过。执行 `BIFROST_BIN="$PWD/target/debug/bifrost" bash e2e-tests/tests/test_upgrade_cli.sh --only-runtime-marker`，删除临时 daemon 的 runtime marker 后，updater 用 Admin 传入的精确 PID/端口恢复标记并完成重启；测试摘要 `Total: 1, Passed: 1, Failed: 0`。
- TC-TWA-03：通过。执行 `BIFROST_BIN="$PWD/target/debug/bifrost" bash e2e-tests/tests/test_upgrade_admin_api_restart_e2e.sh`；完整安装链路从 PID 96962 重启为 PID 97494，already-latest 链路从 PID 97754 重启为 PID 97891；测试摘要 `Total: 14, Passed: 14, Failed: 0`。
- TC-TWA-04：通过。执行 `pnpm --dir web exec vitest run src/stores/useVersionStore.test.ts`，`1` 个测试文件、`2` 个用例全部通过。
- TC-TWA-05：通过。Admin E2E 断言 `logs/upgrade-background.log` 非空；测试结束后扫描未发现属于本次临时安装路径的残留 bifrost 或 defunct 子进程。
- TC-TWA-06：通过。三个 tray 定向单元测试分别在 `src/lib.rs` 和 `src/main.rs` 目标中通过；执行 `BIFROST_BIN="$PWD/target/debug/bifrost" SKIP_BUILD=true bash e2e-tests/tests/test_cli_tray_startup_ci.sh`，Darwin tray helper 在临时端口 13904 启动并完成断言，随后由脚本清理。
- TC-TWA-07：通过。`nested_cli_upgrade_does_not_publish_terminal_app_progress` 在 `src/lib.rs` 和 `src/main.rs` 目标中通过；TC-TWA-02 的 marker 恢复 E2E 同时证明最终 `completed` 由外层 self-update 在 daemon 重启成功后写入。
- TC-TWA-08：通过。`test_upgrade_cli.sh --only-runtime-ownership` 在 macOS 为 `4/4`，CLI-owned marker 恢复后旧 daemon PID 被替换、CLI foreground core 被接续为新 daemon、App 伴随更新失败时 progress 为 `failed` 且旧 daemon 保持原 PID 可用、App-owned core 在直接 self-update 后保持原 PID 与 `runtime_start_mode=desktop`；`test_upgrade_app_owned_core_e2e.sh` 为 `12/12`，CLI/App 版本比较仍按请求组件分别返回、冲突的 CLI 执行请求被实际 runtime owner 派发到 desktop orchestrator、独立 CLI 收到 `upgrade -y` 且 `skip_app=1/skip_restart=1`、App bundle 从 `0.0.1` 真实替换为 `99.0.1`、core 保持存活；准备 debug sidecar 与 `web/dist-desktop` 后，`test_desktop_upgrade_handoff_contract.sh` 为 `5/5 PASS`，验证 fresh/stale marker、禁止复用旧 backend 与 helper 环境清理。

2026-06-17 本次修复已执行：

- TC-TWA-01：通过。9900 监听进程为 PID 84203，启动命令仍是 `/Users/eden/.local/bin/bifrost start ... -p 9900 ... --system-proxy`；`/_bifrost/api/system/overview` 返回运行版本 `0.0.104`；`/Users/eden/.local/bin/bifrost --version` 返回 `0.0.105`；`version-check?refresh=true` 返回 `current_version=0.0.104`、`latest_version=0.0.105`、`has_update=true`；`upgrade/progress` 返回 `idle`；日志 `bifrost.2026-06-17.log` 记录 `child_pid=53186` 的 self-update 子进程，`ps` 显示该子进程为 `<defunct>`。结论：现场是磁盘二进制已更新但旧 daemon 未重启，WebView 读到 idle 后未恢复 UI。
- TC-TWA-02：通过。`BIFROST_BIN=/Users/eden/work/github/bifrost/target/debug/bifrost bash e2e-tests/tests/test_upgrade_admin_api_restart_e2e.sh` 中 already-latest 子用例验证旧 PID 30641 被重启为新 PID 31380，并写入 `phase=completed`；放宽二进制校验超时后重跑，旧 PID 30641 类似路径稳定通过。
- TC-TWA-03：通过。完整 Admin POST 升级链路最终 `phase=completed`，source 为 `admin`，后台子进程诊断日志非空，daemon 从 PID 8392 重启为 PID 10397，并继续从测试安装路径运行；新增 already-latest 子用例同次通过，旧 PID 18373 重启为新 PID 19013；测试摘要 `Total: 14, Passed: 14, Failed: 0`。
- TC-TWA-04：通过。`pnpm --dir web exec vitest run src/stores/useVersionStore.test.ts` 通过，验证 active `installing` 状态读到 `idle` 后 `upgrading=false`、`upgradePhase=idle`，并触发强制 version-check。
- TC-TWA-05：通过。E2E 已断言测试数据目录下 `logs/upgrade-background.log` 非空；现场旧版本中已确认 child 53186 defunct，修复后 Admin spawn 会等待仍由父进程持有的子进程退出并保留 stdout/stderr 到诊断日志。

2026-06-19 本次优化已执行：

- TC-TWA-06：通过。执行 `cargo test -p bifrost-cli test_menu_running_state`、`cargo test -p bifrost-cli test_tray_update_cache_missing_or_stale_requires_fetch` 与 `cargo test -p bifrost-cli test_detect_update_available_uses_tray_cache_without_network`，过滤用例均在 `src/lib.rs` 和 `src/main.rs` 单测目标中通过；验证菜单包含不可点击的当前版本号、缓存缺失/过期会进入后台检查路径、新鲜缓存跳过联网、缓存中存在更高版本时 tray 识别 `Update to v999.0.0` 的前置状态。随后执行 `SKIP_FRONTEND_BUILD=1 cargo build --bin bifrost` 生成当前 debug 二进制，再执行 `BIFROST_BIN="$PWD/target/debug/bifrost" SKIP_BUILD=true bash e2e-tests/tests/test_cli_tray_startup_ci.sh`，真实拉起 Darwin tray helper（port 16172，tray_pid 34284），脚本通过并断言 tray 日志包含 `tray update check skipped; cached version is still fresh`。GitHub Actions run `27835478800` 暴露 Windows ARM log-only 模式下 helper 可重复启动但不长驻，已将脚本调整为该显式降级模式只验证启动标记，后台更新日志断言保留给有存活 helper 的平台。
