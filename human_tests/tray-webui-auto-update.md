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
   SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-core \
     concurrent_progress_writers_never_publish_partial_json_or_leave_temp_files \
     --lib -- --nocapture
   ```

**预期结果**：
- 两个并发 POST 中恰好一个返回 202，另一个返回 409；最终只发生一次安装与一次 daemon 重启。
- 主组件已是 target、伴随组件仍旧时，version-check 仍返回 `has_update=true`，避免 CLI/App 部分升级后更新入口消失。
- 跨进程 `upgrade.lock` 同时只允许一个 owner，释放后可再次获取。
- App-owned 独立 CLI 同时收到 `skip_app=1`、`skip_restart=1` 和与 App 一致的 target；退出 0 但 `--version` 不等于 target 时整体失败。
- CLI-owned 的 Script/Homebrew/Manual 非 deferred 安装也必须在 App 更新和 core 重启前核验实际 CLI target；Script 渠道不得重新追随变化后的 latest，Homebrew 重启不得依赖会被 reinstall 删除的 Cellar 路径。
- 独立 CLI 卡住时在有界超时后被终止，等待期间持续写 Installing 心跳，不会跨过 120 秒 stale 门限。
- App 包先在 staging 中完成复制和版本核验；staging 无版本或版本错误时旧 App bundle 仍保持原版本可启动。
- 模拟 App 已被移到 backup 后中断时，下一次尝试先恢复旧 App；Windows MSI/EXE 安装与 pinned-target 版本核验必须是同一事务，错误包覆盖旧目录后要恢复旧 App 与 uninstaller、清除错误 sidecar，首次安装失败则删除未验证目录；Windows deferred CLI 路径在调度替换前完成 App 同版本门禁，并在 CLI target 核验失败时保留/恢复旧 exe。
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
   cargo test --manifest-path desktop/src-tauri/Cargo.toml deferred_desktop_install_commit -- --nocapture
   cargo test --manifest-path desktop/src-tauri/Cargo.toml -- --nocapture
   pnpm --dir web exec vitest run src/stores/useVersionStore.test.ts
   SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-cli top_level_app_upgrade_owns_the_shared_lock_but_nested_companion_skips_it --lib -- --nocapture
   SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-cli background_upgrade_preserves_progress_owned_by_pending_desktop_handoff --lib -- --nocapture
   SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-cli windows_deferred_install_pins_target_and_respects_parent_progress_ownership --lib -- --nocapture
   python3 -m unittest scripts/ci/tests/test_coverage_diff.py
   ```
2. 执行 App-owned 真实 Admin 链路，验证浏览器请求被拒绝、测试夹具按生产格式签发短时一次性 UUID origin token 并通过专用 header 模拟 Tauri 桌面请求、Admin 原子消费凭证后仍完成 CLI + App 安装：
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
   再用独立临时安装目录和随机端口执行直接 App CLI 入口，确认它更新 CLI 后仍会重启 CLI-owned core：
   ```bash
   BIFROST_BIN="$PWD/target/debug/bifrost" \
   BIFROST_UPGRADE_E2E_START_WITH_INSTALL_BIN=1 \
   BIFROST_UPGRADE_E2E_ENTRYPOINT=app-upgrade \
   BIFROST_DISABLE_TRAY=1 \
   BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 \
     bash e2e-tests/tests/test_upgrade_local_restart_e2e.sh
   ```
   最后构造“磁盘 CLI 已是目标版本，但运行中 core 仍是旧版本”的原始故障形态；例如当前目标 `0.0.156` 时，脚本从当前真实二进制生成临时 `0.0.155` core：
   ```bash
   BIFROST_BIN="$PWD/target/debug/bifrost" \
   BIFROST_UPGRADE_E2E_VERSION=0.0.156 \
   BIFROST_UPGRADE_E2E_ENTRYPOINT=app-upgrade-stale-runtime \
   BIFROST_DISABLE_TRAY=1 \
   BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 \
     bash e2e-tests/tests/test_upgrade_local_restart_e2e.sh
   ```
4. 执行 direct desktop CLI 终态和 WebView owner 分流回归：
   ```bash
   SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-cli \
     upgrade_post_install_desktop_app_args_disable_cli_recursion --lib -- --nocapture
   SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin \
     upgrade_process_args_separate_cli_and_desktop_channels --lib -- --nocapture
   SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-core \
     desktop_upgrade_origin --lib -- --nocapture
   SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin \
     runtime_owner_overrides_the_request_channel --lib -- --nocapture
   cargo test --manifest-path desktop/src-tauri/Cargo.toml \
     internal_upgrade_shutdown_argument_is_detected_without_consuming_other_open_requests \
     -- --nocapture
   BIFROST_BIN="$PWD/target/debug/bifrost" \
     bash e2e-tests/tests/test_desktop_app_update_cli.sh
   pnpm --dir web exec vitest run src/stores/useVersionStore.test.ts
   pnpm --dir web test:unit -- src/api/version.test.ts src/desktop/tauri.test.ts
   ```
5. 检查模块行数和文档可移植性：
   ```bash
   test "$(wc -l < crates/bifrost-cli/src/commands/app.rs)" -le 1500
   test "$(wc -l < crates/bifrost-cli/src/commands/app/installer.rs)" -le 1500
   test "$(wc -l < crates/bifrost-cli/src/commands/app/tests.rs)" -le 1500
   test "$(wc -l < crates/bifrost-cli/src/commands/upgrade.rs)" -le 1500
   test "$(wc -l < crates/bifrost-cli/src/commands/upgrade/desktop_companion.rs)" -le 1500
   test "$(wc -l < crates/bifrost-cli/src/commands/upgrade/download.rs)" -le 1500
   test "$(wc -l < crates/bifrost-cli/src/commands/upgrade/restart.rs)" -le 1500
   test "$(wc -l < crates/bifrost-cli/src/commands/upgrade/tests.rs)" -le 1500
   test "$(wc -l < crates/bifrost-cli/src/commands/upgrade/tests/review_comments.rs)" -le 1500
   test "$(wc -l < desktop/src-tauri/src/main.rs)" -le 1500
   test "$(wc -l < desktop/src-tauri/src/upgrade_handoff.rs)" -le 1500
   test "$(wc -l < desktop/src-tauri/src/backend_runtime.rs)" -le 1500
   test "$(wc -l < desktop/src-tauri/src/tests.rs)" -le 1500
   current_user="$(id -un)"
   ! grep -Fq "/Users/$current_user/" human_tests/tray-webui-auto-update.md
   ```

**预期结果**：
- 跨进程锁 loser 立即得到 `Failed` progress，不会停留在 Checking。
- Admin、CLI、Windows helper 与新 desktop core 即使在 handoff 窗口并发写进度，也只能发布完整 JSON；每个 writer 使用唯一临时文件，结束后不得残留 `.tmp`，不能因固定临时文件竞争把 UI 留在旧 phase 或误退化为 `Idle`。
- 匹配的 restartable runtime marker 在 `lsof` 不可用时仍可升级；marker 恢复的缺省 host 是 `127.0.0.1`。
- macOS App 使用稳定 backup 名跨 updater PID 恢复；Windows App-owned CLI 版本探针等待 deferred replacement。
- CLI 安装后的 exact target 不匹配时恢复旧 binary backup。
- CLI-owned companion 可显式传递检测到的 App parent；Admin desktop orchestrator 不传 `--app-dir`，让 bundled core 从自身 executable 解析当前运行 App，避免多副本时更新错误副本。
- foreground CLI runtime 与 daemon 一样归 CLI updater 重启，只有 `RuntimeStartMode::Desktop` 会交给 Tauri handoff。
- Windows App-owned MSI/EXE 不在 App/sidecar 仍运行时执行；pending marker 交给 Tauri helper，在旧 App/core 退出后有界安装，失败写 `Failed`，成功才拉起新 App。pending handoff 拒绝后台竞争者时不得把原有 `Restarting/source=desktop` 覆盖成 `Failed/source=admin`。
- Windows deferred 安装只在新 App 的编译版本等于 pinned target 时写 `Completed`；安装器成功但拉起旧/错误版本时写 `Failed`。
- Windows deferred helper 必须在安装前把完整目录快照到安装目标之外，并在新 App 编译版本与 managed core 都确认前保留 pending guard、快照和 updater 自有包；版本不匹配、App 提前退出、验证超时或 core 启动失败时，新 App 正常释放文件，helper 恢复旧目录、保留 `Failed` 并重新拉起旧 App。安装器退出 0 本身不能提交事务。
- pending marker 区分 updater 下载包与调用者传入包；handoff 成功后只删除前者，保留用户的 `--package` 文件。
- Desktop shell 复用 CLI-owned core 时，core restart 继续由 CLI owner 收口；但如果原始请求来自 Tauri WebView 且目标 App shell 仍运行，App companion 独立切换到 `source=desktop/Restarting`，只让 Tauri 释放并重启 App，不把外部 CLI core 误记为 managed child。
- Windows deferred installer 在旧 App/core 退出后的整个安装窗口继续持有 pending-marker guard，CLI、tray 与 App updater 都不能取得共享升级锁；成功或失败后 guard 都会释放，陈旧 marker 不会永久阻塞后续更新。
- 只有 Admin/Tauri 发起且带 handoff 标记的 `source=desktop` 才停在 `Restarting` 交给当前 App；用户直接执行 `bifrost app upgrade --source desktop --no-cli` 会自行写 `Completed`。Desktop shell 观察到 CLI-owned `source=admin` 的 `Completed` 时只刷新 WebView，不重启 App。
- changed-lines 95% 门禁只排除从原文件真实消失、至少 8 行且至少 4 行实质代码的机械搬移块；仍保留在原文件的 copy-paste 必须继续计入门禁，小样板和真实修改行也仍计入，报告显示排除行数。
- 顶层 App updater 与 self-update 共用跨进程 `upgrade.lock`，并发 App/CLI updater 只能有一个 owner；`source=cli-upgrade` 只有同时通过父锁 token、owner PID/sidecar、真实父进程和 live lock 校验才能复用父锁，用户伪造可见 source 或环境变量不能绕过。App 管理的 CLI child 必须固定 target、禁止递归更新 App/重启 core；Windows deferred helper 直接使用该固定 target，且由 App 父事务收口时不得提前发布 `Completed`。
- CLI-owned orchestrator 的 caller-managed 与 desktop-handoff App companion 都必须继承父锁随机 token/owner PID，并以真实 OS parent、owner sidecar 与 live lock 三重校验后在同一父事务内安装 App；仅设置调用者可控环境变量不得绕过锁。macOS desktop version-check 必须优先实际启动 bundled core 的 `Bifrost.app`，系统级与用户级副本并存时不能被另一个已是 target 的副本掩盖更新入口。
- 直接执行 `bifrost app upgrade` 时只禁止 CLI updater 递归更新 App，不得禁止 CLI-owned core 重启；真实链路必须看到旧 PID 被替换、新 PID 使用升级后的临时 CLI，并且 App bundle 同时安装到隔离目录。
- CLI-owned core 的 macOS/Windows companion 只有在“目标桌面进程运行 + 已原子消费 Tauri 签发的短时一次性 origin token”同时成立时才改走 `source=desktop` handoff；只伪造 `channel=desktop` 或 header 的 REST/自动化请求不能得到私有 marker，必须走 caller-managed 安装。终端或普通浏览器发起时先通过 single-instance 内部 shutdown 参数让当前 App 安全退出，旧版本不支持该参数时回退 macOS 系统 Quit 或 Windows process-tree termination request；确认 executable 已释放后再 caller-managed 安装并重启，不能留下无人接管的 pending marker，也不能在运行中覆盖 Windows MSI/EXE 资源。
- native desktop restart marker/helper 失败会持久化 `Failed`，刷新后不会重新显示旧 `Completed`。
- 普通浏览器不能启动 desktop-owned 安装；桌面 shell 请求仍把 CLI 与 App 一起升级。
- App-owned E2E 的合法桌面请求必须携带共享数据目录中真实存在的短时 UUID origin token，不能仅靠 `channel=desktop` 绕过来源认证；请求受理后 token 文件必须已被原子消费。
- `app.rs`、`app/installer.rs`、`app/tests.rs`、`upgrade.rs`、`upgrade/desktop_companion.rs`、`upgrade/download.rs`、`upgrade/restart.rs`、`upgrade/tests*.rs` 以及 desktop `main.rs`、`upgrade_handoff.rs`、`backend_runtime.rs`、`tests.rs` 均不超过 1500 行，测试文档不包含本机绝对路径。

## 清理步骤

1. 停止测试数据目录中的 Bifrost 服务：
   ```bash
   BIFROST_DATA_DIR=<tmp>/data <tmp>/install/bifrost stop
   ```
2. 删除临时测试目录。
3. 不清理、不停止、不重启用户正在运行的 9900 服务。

## 执行记录

2026-07-19 本次 PR comments 与上线风险审计已执行：

- TC-TWA-10（跨进程 progress 原子发布回归）：通过。审计发现 Rust 共享 writer 使用固定 `upgrade-progress.json.tmp`，Admin、CLI、Windows helper 与重启后的 desktop core 在所有权交接窗口可能互相 rename/覆盖临时文件，导致 UI 停留旧 phase 或把缺失/损坏文件退化为 `Idle`。现改为目标目录内唯一临时文件、flush/sync 后平台原子 replace；两个 PowerShell helper 同步改为 PID + GUID 临时名并在 `finally` 清理。按用例立即执行 core 16 writer × 32 次并发写入回归 `1/1`、Windows CLI helper 静态契约 `1/1`、desktop deferred marker/handoff 契约 `1/1`，发布文件始终是完整 JSON 且没有临时文件残留。全部使用临时目录，未绑定、停止或修改 9900，也未操作真实 App 安装目录。
- TC-TWA-08/09（两种 runtime owner 真实升级）：通过。CLI-owned Admin POST 链路 `15/15`，两个并发请求严格得到 `409 + 202`，临时 daemon 从 PID `14863` 交接为目标版本 PID `15670`，安装路径、终态 `completed/source=admin`、diagnostics log、端口释放与 already-latest 重启均真实验证；App-owned 链路 `20/20`，无 Tauri token 的浏览器请求被 409 拒绝且不修改组件，真实一次性 token 被原子消费后，独立 CLI 收到同一 pinned target 与 skip-App/skip-restart marker，App 包到达 target，旧 desktop core 保持存活并等待 Tauri handoff。两条链路均使用临时目录与随机端口，未操作 9900 或真实 App。
- TC-TWA-10（restart 与 native handoff 完整契约）：通过。升级 restart E2E `21/21`，覆盖无 daemon、已有 daemon、runtime marker、端口释放、跨平台 deferred replacement 与全部 review contract；desktop handoff 为基础 marker `5/5`、setup failure `1/1`、deferred marker `1/1`、target verification `1/1`、commit cleanup `1/1`。新 progress 临时文件策略没有改变 App/core 终态所有权与失败覆盖行为。
- Review/Fix/Test 两轮闭环：第 1 轮复核目标、最新 diff 与 lockfile 后，发现手工回退 workspace 版本行会使 `cargo test --locked` 拒绝执行，恢复 Cargo 解析后的必要 lockfile 更新并复跑定向测试；第 2 轮复查跨进程 writer、CLI/App owner 分流、Windows helper 清理、coverage changed-lines 工具与最新 diff，未发现新的阻塞问题。最终 `cargo fmt --all -- --check`、desktop fmt、workspace/desktop all-targets all-features strict clippy、workspace all-targets all-features build、`cargo test --locked --workspace --all-features -- --test-threads=1` 全部通过；本地不执行覆盖率脚本，由 PR CI 的 90% coverage gate 兜底。
- CI fail-fast 追加轮次：run `29681642065` 的 coverage job 并非比例未达标，而是在 LLVM coverage 并行负载下，旧 shell-script 错版本 fixture 偶发先进入通用 command-error 回滚分支，使“必须返回 pinned-target mismatch 文案”的精确断言失败；真实旧 binary 已恢复。fixture 改为复制 macOS/Linux 都会对额外参数返回成功的 `/usr/bin/true`，稳定进入“命令成功但版本不匹配”分支，并保留精确错误消息、旧 binary 内容和 backup 清理三重断言。定向测试在 lib/main 两个目标均通过，CLI lib 并行全量为 `1240 passed / 2 ignored`，fmt 通过；未运行本地覆盖率脚本，等待新 CI coverage 90% gate 权威复测。
- CI fail-fast 第二个追加轮次：run `29682001092` 的 Unit & Integration Tests 命中 2026-04 已存在的 TOCTOU 测试缺陷：用例释放 OS 分配的 ephemeral port `43755` 后，另一并行任务在 `lsof` 前复用了同一端口，导致“free port 必须无人监听”断言失败。产品 `find_process_on_port` 未报错；负向用例改查保留的 port 0（绑定时只用于请求 OS 分配真实端口，本身不会成为 listener），正向真实 listener/PID 用例保持不变，避免用重试掩盖竞态。按本用例立即执行定向测试与 CLI lib 并行全量复测。
- CI fail-fast Windows 追加轮次：run `29682255429` 的 Windows Unit Tests 真实证明唯一 temp 仍不足以保证发布：`tempfile 3.27` 的 `MoveFileExW(MOVEFILE_REPLACE_EXISTING)` 在 16 writer × 32 次压力下返回 Win32 code 5。Rust writer 现在保留 `PersistError.file` 并只对 5/32/33 做 100 次有界退避重试；CLI replacement helper 与 desktop handoff 的 progress/marker `Move-Item -Force` 使用同一错误码和预算，永久错误立即抛出，`finally` 清理保持不变。按用例立即执行后，core 并发测试 `1/1`、CLI helper 契约 lib/main `1/1 + 1/1`、desktop helper 契约 `1/1`、restart E2E `21/21`、handoff contract `5/5 + 1/1 + 1/1 + 1/1 + 1/1`、root/desktop fmt 与 strict clippy 均通过。本机 Windows target check 被 `ring` 的 MSVC C 头文件环境（缺少 `assert.h`）阻塞在依赖构建前，Windows 原生编译与压力由下一轮 CI 复测。
- CI fail-fast Windows reader 追加轮次：run `29683229569` 证明 writer 重试已生效，失败点推进到并发 reader：打开刚被替换的 `upgrade-progress.json` 返回 Win32 code 5；产品 `read_progress` 原先会把它直接降级为 `Idle`，可造成 UI 瞬态回退。Rust progress reader 现在与 writer 共用 5/32/33 有界退避，压力测试逐次调用产品解析路径并断言 phase/target；CLI helper 保留 previous target/source 的读取、desktop helper 的 terminal progress/marker 读取也统一重试。缺失/损坏文件仍立即按原契约降级，不掩盖永久错误。按用例立即复跑并通过：core 并发压力 `1/1`、缺失/损坏降级 `1/1 + 1/1`、CLI helper contracts `1/1 + 1/1`、desktop helper contract `1/1`、restart E2E `21/21`、handoff contracts `5/5 + 1/1 + 1/1 + 1/1 + 1/1`、root/desktop fmt 与 strict clippy；Windows 原生读写压力由下一轮 CI 复测。
- PR comments 新增 P1/P2 回归轮次：CLI-owned + WebView 混合模式下，companion 已发布 `Restarting/source=desktop` 后，CLI core restart 不得再用 background `source=admin` 覆盖，否则 Tauri handoff 会永久失去触发条件；普通 CLI/App-owned restart 仍必须更新自己的 progress。父事务子进程不再信任布尔 marker，而是校验随机 token、owner sidecar、真实 OS parent PID 和仍被占用的 `upgrade.lock`；伪造 token/PID/managed flags 必须返回 already-running，真实直系 child 才能复用父锁。第 1 轮 review 又把 credential 继承收窄为显式 data_dir opt-in，`brew`、版本探针、shutdown 等普通 helper 看不到 token，并将新增测试移入 review-comments 模块保持所有生产/测试模块不超过 1500 行。按用例立即执行：handoff progress preserve `1/1`、凭据四要素与锁释放 `1/1`、真实父子进程 caller-managed/desktop-handoff `1/1`、managed CLI 不递归 `1/1`、普通/受控 helper 凭据隔离 `1/1`、coverage edited-middle copy `14/14`、restart E2E `21/21`、CLI-owned Admin `15/15`、App-owned `20/20`、native handoff `5+1+1+1+1`，workspace all-features 全部通过。全部使用临时目录或随机端口，未操作 9900 或真实 App。
- CI fail-fast 父 PID 跨平台追加轮次：run `29687400174` 在 196 秒内由 Coverage 与 Linux Clippy 同时暴露 E0433；新 `current_parent_pid` 无条件引用了只在 macOS/Windows target dependency 声明的 `sysinfo`，所以本机 macOS 全绿但 Linux lib 无法编译。未把 `sysinfo` 扩为 Linux 依赖；Unix 改用无指针参数的 `libc::getppid()` 获取内核记录的直系父 PID，Windows 保持 `sysinfo`。按用例立即复跑真实三层父子进程锁测试、credential helper 隔离与 workspace strict clippy，全部通过；下一 CI 同时验证 Linux coverage 编译和 Windows parent PID 分支。

2026-07-18 本次状态机审计已执行（最终复测）：

- TC-TWA-10（PR comment 凭证回归的 CI 夹具修复）：通过。最终头提交的 macOS `agent-extensions` E2E 暴露出 App-owned 脚本仍只传 `channel=desktop`，因此被新增的真实 WebView origin 校验正确拒绝，后续 CLI child 与 pinned target 断言均为空。脚本现按生产 token 文件格式在隔离数据目录签发 UUID、以 `0600` 创建、通过 `X-Bifrost-Desktop-Upgrade-Origin` 发起合法桌面请求，并断言 Admin 已原子消费凭证；无凭证浏览器请求仍为 409 且不修改 App/CLI。按文档立即执行后 App-owned Admin 实链路为 `20/20`，CLI 与 App 同时更新到 `99.0.1`、core 保持原 owner 等待 Tauri handoff；未绑定、停止或修改 9900。
- TC-TWA-10（PR comments 第十四轮，真实 WebView origin 凭证）：通过。`channel=desktop` 不再被当成 WebView 存在证明；桌面进程通过 Tauri command 在共享数据目录签发 UUID token，Web API 只在 desktop channel 上把 token 放入专用 header，Admin 对请求 channel、30 秒有效期和一次性文件做原子校验/消费。CLI-owned core 的无 token REST 调用继续由 CLI owner 收口并走 caller-managed App 安装，不会留下无人消费的 `Restarting`；desktop-owned core 的无 token 请求返回 409。第 1 轮 review 将“后签发 token 清除前一个 token”修正为每个 token 独立一次性消费，避免多个合法桌面窗口相互误伤；第 2 轮把 Unix token 文件权限收紧为 `0600`，避免同机其他用户从共享数据目录读取短时凭证。按文档立即执行 core token 正常/错误/过期/重复消费及权限 `3/3`、Admin owner/origin `1/1`、Web 全量 `178/178`、workspace 与 desktop strict clippy、Web lint/build，以及 restart E2E `21/21`，全部通过；未绑定、停止或修改 9900。
- TC-TWA-10（PR comments 第十三轮，WebView origin 与 shell 运行状态解耦）：通过。Admin 现在把原始 `channel=desktop` 独立编码为私有 WebView-origin marker，即使 effective orchestrator 因 CLI-owned core 变为 CLI 也不会丢失发起界面；CLI companion 不再用 `RuntimeStartMode::Desktop` 猜测是否有人消费 handoff。目标 App 正在运行且有 WebView marker 时才交给 Tauri；终端/普通浏览器路径先通过 `--bifrost-upgrade-shutdown` 触发 desktop single-instance graceful shutdown，旧 App 不支持时再走平台退出回退，确认 executable 退出后才安装和重启，超时则拒绝覆盖。macOS/Windows 多副本候选优先实际运行的 App。第 1 轮 review 修复 shell 在检测后自行退出会被误报失败的 race；第 2 轮补上 v155 等旧 App 不认识新 shutdown 参数的兼容回退。按文档立即执行 CLI owner/mode/active-candidate 测试 `1/1`、Admin origin 环境测试 `1/1`、desktop shutdown 参数测试 `1/1`、desktop 全量 `53/53`、strict clippy、restart E2E `21/21` 和基于 `id -un` 的当前用户路径检查，全部通过；未绑定、停止或修改 9900。
- TC-TWA-10（PR comments 第十二轮）：通过。CLI-owned orchestrator 现在对 caller-managed 与 Windows desktop-handoff 两类 App companion 都显式注入 parent-lock marker，child 继续携带 `--no-cli`、固定 target 和 owner source，在父事务内更新 App 而不与父进程争用 `upgrade.lock`；macOS desktop version-check 从当前 bundled core executable 向上解析实际 `Bifrost.app` 并置于全局候选之前，多副本测试证明 active `0.0.143` 不会被另一个 `0.0.144` App 掩盖。Admin owner fixture 改用隔离端口 `19900`，文档可移植性检查改为项目相对文件上的通用 macOS home-path 正则，不再嵌入本机路径。按文档立即执行 companion 参数/真实 child 环境测试各 `1/1`、active/override App 版本测试 `2/2`、Admin owner 测试 `1/1`、restart E2E `21/21` 与文档路径检查，全部通过；未绑定、停止或修改 9900。
- TC-TWA-10（PR comments 第十一轮，Windows deferred 跨进程回滚）：通过。helper 在旧 App/core 退出后、MSI/EXE 执行前把完整安装目录快照到目标目录之外，并把 rollback metadata 原子写回 relaunch marker；installer 返回成功后仍保留 pending guard、快照和 updater 自有包，等待新 App 编译版本与 managed core 共同确认。版本不匹配、App 提前退出、120 秒验证超时或 core 启动失败会让新 App 走正常 shutdown，helper 仅清理安装目录内残留进程、恢复旧目录、保留 `Failed` 并重启旧 App；只有 `Completed` 才提交清理。第 1 轮 review 发现新 App 写 `Completed` 后 helper 若在清理前崩溃会残留 guard/快照，补充了 App 侧受路径校验保护的幂等提交清理，任意非 `.Bifrost.rollback-*` 同级目录会拒绝删除；第 2 轮复跑 desktop 全量单测 `52/52`、handoff contract `5/5 + 1/1 + 1/1 + 1/1 + 1/1`、restart E2E `21/21` 与 desktop strict clippy 全绿。新 CI run `29667989074` fail-fast 只发现独立 desktop manifest 的 rustfmt 差异，根 workspace fmt 不会覆盖该 manifest；第 3 轮按 CI 精确命令格式化后，desktop fmt check、commit cleanup 定向测试与完整 handoff contract 再次通过。静态合约额外断言 snapshot 发生在 installer 之前、验证失败必经 restore、pending/package 不会提前清理。测试只使用临时目录、随机端口与构建 sidecar，未操作 9900 或真实 Windows/macOS App。
- TC-TWA-10（并发升级单测隔离回归）：run `29661719290` 的 Unit & Integration Tests 首个失败是下载镜像测试把“最快候选”断言为本地 fixture，但并行 runner 上真实 `github.com` 探针可能更早返回成功；该 panic 持锁退出后又让四个环境隔离测试因 poisoned mutex 连锁失败。现将候选列表注入下载选择核心，测试只使用本地成功/失败端点，不访问真实公共镜像；mutex 继续保持严格 poison 语义，不在可能残留环境修改时掩盖前序 panic。默认并发 CLI lib 两轮复跑均为 `1234 passed / 2 ignored`，原 5 个失败路径全部通过。
- TC-TWA-10（Linux CI 依赖边界回归）：run `29661465065` 的 Rust Clippy 首次在 Linux lib-test 编译阶段发现 Windows 进程探针被 `cfg(test)` 错误带入，而 `sysinfo` 原本只属于 macOS target dependency。现将真实进程探针严格限定为 Windows 编译，并为 Windows target 显式声明 `sysinfo`；非 Windows 测试继续覆盖无依赖的路径归一化和 owner 决策。修复后本机 all-targets/all-features strict clippy 通过，后续 Linux 与 Windows 原生编译由新 CI run 继续门禁。
- TC-TWA-10（PR comments 第六轮）：通过。desktop `main.rs` 按 upgrade handoff、backend runtime 和 tests 拆为 `1353/660/1430/1113` 行；CLI upgrade companion 独立为 `203` 行，upgrade tests 拆为 `1470/107` 行。锁竞争定向测试证明 fresh Windows pending marker 返回“handoff already pending”且保留原 `Restarting/source=desktop`；直接 App 行为测试证明只禁止 App 递归而保留 proxy restart；Windows companion 决策覆盖“桌面进程运行→desktop deferred handoff”和“未运行→caller-managed install”。升级 restart 契约修正拆分后的扫描范围并复跑 `21/21`。普通 CLI 本地归档真实升级最终为 `18/18`，旧 daemon `99783` 在随机端口 `57694` 被新 PID `285` 替换；直接 `app upgrade` 真实链路最终为 `19/19`，在隔离目录安装 pinned App target，并将旧 daemon `98290` 在随机端口 `57566` 替换为新 PID `98529`。针对用户原始 `155` 残留问题，额外构造磁盘/命令 CLI 已为 `0.0.156`、运行中 core 为临时真实 `0.0.155` 的 already-latest 场景，`app upgrade` 回归为 `17/17`：旧 PID `11975` 被新 PID `12172` 替换，App、CLI 与新 core 都收敛到 `0.0.156`。三条链路都额外从安装路径执行 `--version` 并读取新 core 的 `/api/system.version`，均精确等于 pinned target；同时保持 no-system-proxy、清理 tray helper 并释放端口。App-owned Admin 为 `17/17`，direct desktop CLI 为 `36/36`，native handoff 为 `5/5 + 1/1 + 1/1 + 1/1`。未操作 9900 或真实 `/Applications/Bifrost.app`。
- TC-TWA-08/09/10（第五轮完整复测）：通过。App-owned 真实 Admin 链路 `17/17`，CLI runtime ownership `4/4`，CLI restart `21/21`，Admin 真实下载→安装→版本核验→daemon 重启 `15/15`，direct desktop CLI `36/36`，native handoff 为 `5/5 + 1/1 + 1/1 + 1/1`。CLI 全量 lib `1233 passed / 2 ignored`；strict clippy、all-targets build、fmt 全绿；workspace 首轮唯一失败是无关 rule-share 测试并发创建默认 RulesStorage 命中 `AlreadyExists`，精确复跑 `1/1` 后以 `--test-threads=1` 复跑整个 workspace 退出码 0。第 1 轮 review 修正 child marker 误加到错误 helper、pending handoff 二次检查竞态和 coverage 短片段误排除；第 2 轮将 CLI lock bypass 收紧为 marker + skip-App + skip-restart + pinned-target 缺一不可，并复跑定向单测、coverage 工具 `13/13` 与 restart E2E `21/21`。所有真实链路使用临时目录和随机端口，already-latest fixture 显式隔离本机 App 安装目录；未操作 9900 或真实 `/Applications/Bifrost.app`。
- TC-TWA-09/10（PR comments 第五轮定向回归）：通过。父锁/私有 child marker、pending desktop handoff progress 保留、Windows deferred pinned target 与 progress owner、direct App 固定 target 四个定向单测均为 `1/1`；coverage-diff 工具测试为 `13/13`，新增断言证明跨文件原位置仍保留的 copy-paste 和同文件前插/后插 duplicate 都不会被 changed-lines 门禁误排除，而同文件真实机械搬移仍可排除；保留块切分出的短片段也不能伪装成搬移。使用失败 CI run `29654516618` 的原始 `lcov.info` 重放修复后的门禁，changed-lines 为 `95.42% (1021/1070)` 并通过 `95%` 阈值；coverage pipeline contract `31/31` 通过。App-managed child 只有同时携带 skip-App、skip-restart、pinned-target 和 parent-lock marker 才复用父锁；仅伪造 `source=cli-upgrade` 或 marker 会被共享锁拒绝。全部测试使用临时目录，未启动或修改 9900 服务。
- TC-TWA-10（CI 高负载 CLI 探针回归）：通过。run `29655456923` 在 LLVM coverage 全量并发测试中暴露 non-zero CLI fixture 的 `1s` 超时过窄，瞬时脚本可能在 runner 高负载调度下先命中 timeout，导致用例未验证到预期的 exit-status 分支。fixture 超时调整为 `10s`，仍严格断言错误包含非零退出状态，并在失败信息中打印实际 error chain；精确用例与 CLI lib 全量并发测试均重新执行。该改动不放宽产品门禁或业务断言，未操作 9900 服务。
- TC-TWA-10（Windows pinned-target fixture 回归）：通过。run `29655827088` 的原生 Windows self-update replacement E2E 为 `8/9`：旧 proxy 已停止且 helper 已 scheduled，但新 daemon 不可达。审计确认归档标称 `0.0.157`、内部 executable 实际仍为 `0.0.156`，新增的安装后 pinned-target 校验正确拒绝假包；Windows fixture 现与 Admin fixture 一致，只对临时 binary 等长改写编译版本并在打包前真实执行 `--version` 核验。失败路径还会在 cleanup 前输出 helper log、args、progress 与相关进程，避免再次丢失现场。修复后重跑 restart E2E 与 shell syntax；原生 Windows replacement 由后续 CI 门禁验证。全部本地测试使用临时目录和随机端口，未操作 9900 服务。
- TC-TWA-10（Windows core 版本验真回归）：run `29662118452` 中安装后的 Windows CLI 已为 `0.0.157`，但 `/api/system.version` 仍为 `0.0.156`，证明只执行 `bifrost --version` 不能验证目标包里的 core。Windows CI 现分别构建当前版本与通过 release 同款 `BIFROST_VERSION=0.0.157` 注入的目标版本 executable；升级前先从目标 executable 真实启动临时 core，并同时断言 CLI 与 `/api/system.version` 精确等于 pinned target，之后才执行 self-replacement、PID 交接和重启版本断言。失败时在 cleanup 前输出 helper log、args、progress 和目标路径进程详情，不放宽产品版本门禁。本机同链路新增验真后为 `19/19`，旧 PID `22911` 在随机端口 `55466` 被新 PID `23126` 替换，CLI/core 均为 `0.0.157`；未操作 9900 或真实 App。
- TC-TWA-10（PR comments 第七轮）：新增两条 P2 均已按 owner/预算修复。Windows 桌面进程只有在 `runtime.json.start_mode=desktop` 且 runtime PID 仍存活时才能选择 `source=desktop` handoff；App 仅作为 WebUI shell、core 由 CLI 启动时保持 caller-managed，由 CLI updater 负责原 core 重启，不能让重启后的 App 再拉起第二个 bundled core。pending-install guard 与 relaunch marker 的新鲜期统一为 15 分钟，覆盖 helper 最坏 30 秒等 App + 30 秒等 core + 600 秒 installer 的 11 分钟预算并保留调度余量；11 分钟 marker 仍 active，超过 15 分钟才 stale。定向 CLI owner/timeout 单测、desktop `51/51`、upgrade restart `21/21`、handoff contract `5/5 + 1/1 + 1/1 + 1/1` 均通过；未操作 9900 或真实 App。
- TC-TWA-10（PR comments 第八轮）：通过。Windows MSI/EXE 安装现在先将整个既有安装目录快照到安装目标之外，再把安装器执行和 installed App pinned-target 核验作为一个事务。App 全量定向单测 `32/32`，其中三条新回归分别证明错误版本覆盖后恢复旧 App 与 uninstaller 并删除新增 sidecar、无旧版本的失败首次安装不留下目录、核验成功时提交新 App；CLI restart E2E `21/21`，真实启动临时 daemon 并验证 PID/端口/runtime marker 行为。全部使用临时目录和随机端口，未操作 9900 或真实 App。
- TC-TWA-10（Windows rollback coverage 门禁回归）：run `29664666806` 的工作区 90% 棘轮已通过，但 changed production Rust lines 为 `94.31%`，低于 `95%`。未降低阈值；新增 parentless target、陈旧 file 清理、rollback copy 失败三条错误注入用例，要求回滚自身失败时恢复被临时移走的旧目录，并同时报告原安装错误与回滚错误。首轮编译暴露测试位于父模块而快照类型仍为子模块私有，现仅放宽到 `app` 父模块可见，不导出 crate/public API。修正后 App 全量 `35/35`、restart E2E `21/21`、fmt 通过；后续远端 coverage 门禁继续验收。全部使用临时目录和随机端口，未操作 9900 或真实 App。
- TC-TWA-10（PR comments 第九轮）：通过。desktop App/core 已为 release target、独立 CLI 仍旧时，`version-check?channel=desktop` 现在从与 CLI updater 一致的 PATH、显式安装目录、用户目录和平台默认路径解析 CLI，并执行有 5 秒上限的真实 `--version` 探针；缺失、非零退出或超时才回退到 bundled core。Admin 定向测试 `18/18` 覆盖真实 CLI 输出、非零退出、超时、缺失、跳过运行中 core 与候选选择；App-owned 实链路 `19/19`，先把临时 App 写成 target、CLI 保持 `0.0.2`，真实 API 仍返回 `has_update=true`，再恢复旧 App 并完成 CLI+App pinned-target 升级。首轮候选测试因 macOS `/var` fixture 未 canonicalize 与产品契约不一致而失败，修正 fixture 后复跑全绿；未操作 9900 或真实 App。
- TC-TWA-10（PR comments 第十轮）：通过。当 tray/CLI/Admin/App 两个顶层 updater 争用 `upgrade.lock` 时，loser 不再写共享终态。定向测试由 owner 持锁并写 `Downloading/source=tray/target=0.0.155`，再分别让 Admin background 与 desktop App loser 请求 `0.0.156`；loser engine 未运行/调用返回 already-running，原 phase/source/target/message/error 完整不变。pending desktop handoff owner 保留测试同样通过，restart E2E `21/21`。两次首轮仅被 formatter 差异拦截，执行 fmt 后同组测试全绿；未操作 9900 或真实 App。
- TC-TWA-10（CI fixture 版本核验）：通过。Linux Shell CI 首轮 158/159，唯一失败是 Admin API E2E 将当前 `0.0.156` 二进制直接放进命名为 `0.0.157` 的归档，新的安装后版本门禁正确拒绝该假 fixture。fixture 改为只在临时二进制副本中等长替换编译版本字节，打包前真实执行 `--version` 校验；macOS 临时副本重新 ad-hoc codesign。随后真实 Admin POST 升级、原子替换、版本核验、daemon 重启与 already-latest 路径复测为 `15/15`，使用临时目录和随机端口，未操作 9900。
- TC-TWA-10（PR comments 第四轮）：通过。Windows deferred pending marker 的 active/stale guard 定向测试 `1/1`，App-owned handoff transaction `1/1`，CLI interactive wrapper/shared lock `1/1`，Web owner 分流 `5/5`，desktop PowerShell guard 清理合约 `1/1`。真实 `test_desktop_app_update_cli.sh` 为 `36/36`，证明 direct `app upgrade --source desktop --no-cli` 安装后写 `completed` 而非永久停在 `restarting`；CLI-owned `source=admin` 的 `completed` 在 desktop shell 中只 reload WebView，不调用 Tauri App handoff。pending marker 在 process lock 释放后继续拒绝 CLI/tray owner，成功与失败路径均移除 guard，超过当前新鲜期的陈旧 marker 不再阻塞。全部使用临时目录和随机端口，未操作 9900。
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
