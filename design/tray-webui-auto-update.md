# Tray / Web UI 后台自动更新

## 背景

Bifrost 早期升级只有一条路径:用户在终端里手动跑 `bifrost upgrade`,回车、看进度、必要时再手动 `bifrost start` 重启。桌面用户在托盘里看到红点或版本号变了,还得切到 iTerm,失去了 desktop app 的“一键更新”体验;Admin Web UI 用户则完全没有反馈——检测到新版本只弹了一个提示框,没有下一步动作。

本方案把升级从纯 CLI 操作抬升为**托盘一键更新 + Web UI 弹窗更新**两条可视化入口,同时通过一个跨进程的进度文件 `<data_dir>/upgrade-progress.json` 让托盘、Admin、Web UI 三端共同观察同一次升级。

代码入口:

- `crates/bifrost-core/src/upgrade_progress.rs`(共享进度通道)
- `crates/bifrost-cli/src/commands/upgrade.rs`(现有升级引擎,只加最小改动)
- `crates/bifrost-cli/src/commands/upgrade_background.rs`(后台升级 + ProgressSink)
- `crates/bifrost-cli/src/cli.rs`(隐藏子命令 `self-update`)
- `crates/bifrost-cli/src/commands/tray/tray.rs`(托盘菜单项 + OP_UPGRADING 状态)
- `crates/bifrost-admin/src/handlers/system.rs`(`/api/system/upgrade` + `/api/system/upgrade/progress`)
- `web/src/stores/useVersionStore.ts` + `web/src/api/version.ts` + `VersionModal`(前端进度轮询与自动 reload)

## 用户目标验证清单

### 必须实现

- 有新版本时,托盘菜单出现 `Update to vX.Y.Z`,点击后进入 `Bifrost: Updating…` 状态,升级完成后托盘自动恢复 Running 且版本号刷新。
- Web UI 检测到新版本弹版本管理弹窗,右下角提供 `立即更新` 与 `稍后提示` 两个按钮。
- 点击 `立即更新` 后弹窗切进度态:先下载进度(百分比),再安装/重启进度(步骤态),无法手动关闭。
- 升级重启后前端 disconnect → reconnect,自动 `window.location.reload()` 一次,`sessionStorage` 标志位防止刷新风暴。
- 隐藏 CLI `bifrost self-update --target <v> --source (tray|admin|cli)` 由托盘/admin spawn,不走用户可见 help。
- Admin 端 `POST /api/system/upgrade` 只 spawn 子进程,不在自身进程里执行升级(避免自杀式重启)。
- Admin 发起 CLI 更新时把当前 Admin 进程 PID 与真实监听端口作为隐藏 hint 交给 `self-update`;即使 `runtime.json` / `bifrost.pid` 丢失,updater 也必须在校验“PID 存活且确实持有该端口”后恢复 marker 并重启旧 daemon。
- CLI 升级联动桌面 App 时,嵌套 `app upgrade --source cli-upgrade` 不得写共享终态;`completed` 只能由最外层 CLI 更新在 daemon 重启完成后发布。

### 必须不破坏

- `bifrost upgrade` CLI 交互路径 stdout/stderr 行为保持不变——只有当 ProgressSink 显式安装时才写进度文件。
- crate 依赖方向 `bifrost-cli → bifrost-admin → bifrost-core` 不被打破:进度状态类型下沉到 `bifrost-core`,admin 只 spawn 子进程。
- 老的 `version-check` 弹窗、`markVersionSeen(latest)` 语义、`稍后提示` 后本会话不再弹窗的行为保持。
- 托盘现有 `spawn_start` / `spawn_stop` / OP 状态机(OP_STARTING/OP_STOPPING/OP_RESTARTING/OP_STOPPING_ON_EXIT)不受影响,`OP_UPGRADING` / `OP_UPGRADE_FAILED` 与它们错开。
- 升级失败时旧版本继续可用,不删除任何用户数据,skill 安装失败也不回滚新二进制。

### 必须真实验证

- 托盘出现 `Update to vX.Y.Z` → 点击 → 完成后托盘 Running + 版本号刷新。
- Web UI 弹窗 → `立即更新` → 下载 + 安装进度实时刷新 → 自动 reload 一次。
- `POST /api/system/upgrade` 在无更新时 409,在已有 active 升级时 409。
- 磁盘上 `bifrost` 已被替换成 latest 但 9900 端口仍是旧版时,`self-update` 仍强制重启旧 daemon(修复现场故障)。
- 双主题(Light/Dark)下弹窗与进度条样式正确。

## 产品语义

### 升级进度是跨进程状态,不是 turn 内信号

进度文件 `<data_dir>/upgrade-progress.json` 采用 `write tmp + rename` 原子写。读方(托盘/admin/web)对解析失败或缺文件一律退化为 `Idle`。文件天然跨代理重启存活,让 web 在重连后仍能读到 `Completed` 终态。

`UpgradePhase`:

```
Idle → Checking → Downloading → Installing → Restarting → Completed / Failed
```

只有 `Downloading` 阶段的 `percent` 有意义;`Installing` / `Restarting` 走步骤态。

### 升级发起方与自恢复

- `source ∈ { "tray", "admin", "cli" }`:仅用于诊断。
- **stale active**:`updated_at` 超过 120 秒仍未更新且仍处 active,`GET /api/system/upgrade/progress` 归一化为 `Failed`,避免 UI 卡死在 Working。
- **磁盘二进制已是 latest**:`upgrade` 交互路径会“already latest”直接退出;`self-update` 必须绕过该短路,始终对运行中的旧 daemon 触发 `maybe_restart_running_proxy`。
- **runtime marker 缺失**:Admin 子进程参数携带发起请求的 PID/端口。`self-update` 只在进程仍存活且端口 owner 与 PID 完全一致时合成 daemon runtime snapshot;任一校验失败都忽略 hint,禁止按端口模糊匹配或误停其它服务。
- **进度所有权**:最外层 `self-update` 是 CLI channel 唯一 terminal progress writer。CLI 联动的 App 安装仍输出诊断日志,但 `source=cli-upgrade` 时不触碰 `upgrade-progress.json`,避免 Web UI 在 daemon 重启前提前观察到 `completed`。

### “立即更新” vs “稍后提示”

- `立即更新`:调 `POST /api/system/upgrade`,进入 Web UI 进度态。
- `稍后提示`:`markVersionSeen(latest)` + 关闭弹窗,本会话不再自动弹。
- 弹窗一旦进入进度态,右上角关闭按钮 + `稍后提示` 都隐藏,避免用户在下载/安装中间关掉弹窗又误以为升级中止。

### 托盘自主维护 version_cache

托盘 helper 启动后延迟 30 秒执行一次后台 version check,之后每 6 小时最多一次;`version_cache.json` 仍新鲜时跳过联网。网络失败时保留旧缓存不隐藏更新入口。这样 tray helper 与 admin `VersionChecker`、`bifrost upgrade` 共用同一个缓存源,`update_now` 菜单项在离线时仍可展示。

## 技术细节

### 共享进度通道(bifrost-core)

`crates/bifrost-core/src/upgrade_progress.rs`(已存在):

```rust
pub enum UpgradePhase { Idle, Checking, Downloading, Installing, Restarting, Completed, Failed }
pub struct UpgradeProgress {
    pub phase: UpgradePhase,
    pub percent: Option<f64>,
    pub message: String,
    pub target_version: Option<String>,
    pub source: Option<String>,
    pub error: Option<String>,
    pub updated_at: String,
}
impl UpgradeProgress {
    pub fn idle() -> Self;
    pub fn new(phase, message) -> Self;
    pub fn with_target/with_source/with_percent/with_error(self, …) -> Self;
    pub fn is_active(&self) -> bool;
}
pub fn progress_file_path(&Path) -> PathBuf;   // data_dir/upgrade-progress.json
pub fn read_progress(&Path) -> UpgradeProgress; // 解析失败 → idle()
pub fn write_progress(&Path, &UpgradeProgress); // 原子 tmp + rename
pub fn clear_progress(&Path);
pub fn is_stale(&UpgradeProgress, max_age_secs) -> bool;
pub const DEFAULT_STALE_SECS: i64;
```

### 升级引擎旁路进度上报(bifrost-cli)

`upgrade.rs` 保留原有下载/安装/重启逻辑,仅新增:

- `download_file_once_with_progress` 的渲染节流点(250ms)直接调用 `super::upgrade_background::report_download(downloaded, total, started)`。
- 安装/重启阶段调用 `report_installing()` / `report_restarting()`。
- 未安装 sink 时全部为 no-op,CLI 交互路径完全不变(不需要给 `download_file_with_progress` 增加 `on_progress` 闭包参数)。

`upgrade_background.rs`:

- 全局 `SINK: OnceLock<Mutex<Option<ProgressSink>>>`;
- `install_sink` / `take_sink` / `emit(build)`;
- `report_download(u64, Option<u64>, Instant)` / `report_installing()` / `report_restarting()`;
- `handle_upgrade_background(target, source, running_proxy_pid, running_proxy_port)`:构造 sink 与可选的 Admin restart hint,在 `Checking → Downloading → Installing → Restarting → Completed/Failed` 各阶段写入进度文件。

### 隐藏 CLI 入口

`crates/bifrost-cli/src/cli.rs`:

```rust
#[command(hide = true, about = "Run an unattended background upgrade (used by tray/admin)")]
SelfUpdate {
    #[arg(long)] target: Option<String>,
    #[arg(long, default_value = "cli")] source: String,
}
```

`main.rs` 路由到 `handle_upgrade_background(target, source, running_proxy_pid, running_proxy_port)`。两个 restart hint 参数隐藏且必须成对出现,用户 `bifrost --help` 不可见。

### Admin API

`crates/bifrost-admin/src/handlers/system.rs`:

- `POST /api/system/upgrade`
  - 读 `read_progress`,若非 stale 的 active 升级 → 409。
  - 读 `VersionChecker` 最新结果,若无可用更新 → 409。
  - 写入初始 `UpgradeProgress { phase: Checking, source: "admin", target_version }`。
  - Spawn detached `bifrost self-update --target <v> --source admin --running-proxy-pid <pid> --running-proxy-port <port>`,binary 定位:`std::env::current_exe()` 优先(admin 与 bifrost 同进程),fallback `PATH` 中 `bifrost`。PID/port 参数为隐藏内部协议,且必须成对出现。
  - stdout/stderr 追加到 `logs/upgrade-background.log`,父进程仍存活时 wait 子进程,避免下载 100% / 安装 0% 空档没日志。
  - 返回 `202 Accepted` + 当前进度快照。
- `GET /api/system/upgrade/progress`
  - `read_progress → normalize_progress` → stale active 归一化为 `Failed`。
  - 返回 JSON。

安全:沿用 admin 现有同源/鉴权中间件,与 `version-check` / `system/*` 一致,不新增公网面。

### 托盘 UI

`crates/bifrost-cli/src/commands/tray/tray.rs`:

- 常量 `OP_UPGRADING`、`OP_UPGRADE_FAILED` 与现有 OP 常量错开(现有 0/1/2/4/5)。
- `operation_status_label(OP_UPGRADING) = Some("Bifrost: Updating…")`,`operation_busy(OP_UPGRADING) = true`。
- `upgrade_status_label(op, percent)`:`Downloading` 阶段附带 `{percent}%`,其它 phase 返回不带百分比的稳定文案,`UPGRADE_PERCENT_NONE` 时返回 None。
- 菜单项 `update_now`:仅当 `version_cache.json` 显示存在更新时可点击;升级进行中时禁用,label 变为进行态。
- 菜单固定展示 `Version v{current}` 信息行(不可点击)与 `Update to v{latest}`,避免用户混淆当前版本与可更新版本。
- `MenuItemAction::StartUpgrade { target_version }` → `spawn_tray_task(bifrost self-update --target <v> --source tray)`。
- 状态轮询线程额外读 `upgrade-progress.json`,把 `Downloading/percent` 映射到 tray 标题;`Completed` 清操作态回 `OP_IDLE`;`Failed` 转 `OP_UPGRADE_FAILED`。
- Tray helper 首次 version check 延迟由 `test_cli_tray_startup_ci.sh` 压到 0,以便 CI 断言。

### Web UI(Ant Design)

- `web/src/api/version.ts`:`startUpgrade()` / `getUpgradeProgress()`。
- `web/src/types/index.ts`:`UpgradePhase` / `UpgradeProgress`,snake_case 与后端对齐。
- `web/src/stores/useVersionStore.ts`:`upgradePhase / upgradePercent / upgradeMessage / upgradeError / startUpgrade() / pollUpgradeProgress()`。
- `VersionModal`:
  - 未升级:footer 右下角渲染 `稍后提示`(次)+ `立即更新`(主)。
  - 升级中:antd `Progress` 展示 `percent` + 步骤态,禁止手动关闭。
  - 轮询每 ~1s;`Restarting` 后监听 `pushService.onConnectionChange` 的 disconnected→reconnected;或轮询直接读到 `Completed`。
  - 检测到 reconnected 或 Completed → `window.location.reload()` 一次,`sessionStorage['bifrost-upgrade-reload-pending']` 防止刷新风暴。
- WebView 在 active 状态读到 `Idle`(其它端已经 clear terminal progress)时,不能卡在 Working:若观察到连接断开→reload;否则强制刷新 version-check 恢复弹窗状态。
- `useGlobalDataSync` 初始化时检查 sessionStorage 升级标志,已消费则清理。

## Sync 边界

- 升级触发/进度是本机行为,不进入 rule/group sync 通道。
- `version_cache.json` 与 `upgrade-progress.json` 均在本机 `data_dir`,不上传云端。
- 团队/组织不能通过 sync 强制其它成员升级或降级。

## Phase 1 - 4

### Phase 1:共享进度通道

- 新增 `upgrade_progress.rs`(core)+ `pub mod` 导出。
- 单元测试:round-trip、损坏文件 → idle、`is_active`、`is_stale`。

### Phase 2:升级引擎接入

- `upgrade.rs` 在下载/安装/重启节点调用 `upgrade_background::report_*`。
- 新增 `upgrade_background.rs`:`ProgressSink`、`handle_upgrade_background`。
- 新增隐藏 `self-update` 子命令,`main.rs` 路由。
- 单元测试:各 phase 写入、失败 phase 写 `error`、无 sink 时 no-op、磁盘 latest 时仍 restart 旧 daemon。

### Phase 3:Admin API + 托盘

- `POST /api/system/upgrade` + `GET /api/system/upgrade/progress`(stale 归一化)。
- 托盘菜单 `Version v{current}` 信息行 + `Update to v{latest}` + `OP_UPGRADING/OP_UPGRADE_FAILED` 状态。
- 单元测试:409 分支、版本信息行、`operation_status_label`、`upgrade_status_label` 百分比。

### Phase 4:Web UI + human_tests + 文档

- `VersionModal` 双按钮 + 进度态、`useVersionStore` 轮询、`sessionStorage` 去重刷新。
- `human_tests/tray-webui-auto-update.md` 新增。
- README/skill 加“Update to vX.Y.Z / 立即更新 / 稍后提示”截图与故障排查。

## 测试方案

### 单元测试

- `bifrost-core / upgrade_progress`:
  - `write_read_round_trip_preserves_all_fields`
  - JSON 损坏 / 文件不存在 → `read_progress` 返回 `idle()`
  - `is_active` 覆盖各 phase;`is_stale` 边界
- `bifrost-cli / upgrade_background`:
  - `download_report_writes_percent_and_phase`
  - `installing_and_restarting_report_phases`
  - `reporting_without_sink_is_noop`
  - `background_upgrade_source_delegates_windows_deferred_terminal_progress`
- `bifrost-cli / tray` (`tray_tests.rs`):
  - `test_upgrade_operation_status_and_busy`(OP_UPGRADING label + busy)
  - `test_upgrade_status_label_includes_percent_only_while_downloading`
  - `test_tray_update_cache_missing_or_stale_requires_fetch`
  - `test_detect_update_available_uses_tray_cache_without_network`
  - `test_tray_start_service_uses_detached_daemon`
- `bifrost-cli / upgrade skills`:
  - `upgrade_post_install_skill_args_cover_all_supported_tools`(固定 `install-skill --tool all -y`)
  - `upgrade_post_install_skill_messages_cover_all_statuses`(失败不回滚)
- `bifrost-admin / system handler`:无更新 409、已有 active 409、正常 202 + 初始进度、`GET progress` 返回写入值、stale active → Failed。

### 接口化 E2E

- `e2e-tests/tests/test_upgrade_admin_api_restart_e2e.sh`:构造 fake latest release,POST `/api/system/upgrade` → 轮询 progress → 断言 `Checking → Downloading → Installing → Restarting → Completed`;无更新时 409;直接 `bifrost self-update --source admin` 且磁盘 latest 时仍重启旧 daemon,进度文件 `completed`。
- `e2e-tests/tests/test_upgrade_cli.sh`:在临时 data dir 拉起 daemon,删除 `runtime.json` / `bifrost.pid` 但保持监听存活,再用 Admin PID/port hint 执行 already-latest `self-update`;断言 marker 被恢复、PID 变化、Admin API 恢复、terminal progress 为 `completed`。
- `test_cli_tray_startup_ci.sh`:预置新鲜 `version_cache.json`,压首次 check 延迟到 0,断言 tray 启动后从 `tray.log` 读到“缓存新鲜跳过联网”。
- Web:`web/tests/ui` 补 VersionModal 双按钮 + 进度态 + 双主题快照。

### 真实场景测试 human_tests

`human_tests/tray-webui-auto-update.md`(已在仓库中):

- TC-Upd-01:托盘出现 `Update to vX.Y.Z`,点击 → `Bifrost: Updating…` → 完成后 Running + 新版本号。
- TC-Upd-02:Web UI 弹窗右下角 `立即更新 / 稍后提示` 位置正确。
- TC-Upd-03:点 `稍后提示` 后本会话不再自动弹。
- TC-Upd-04:点 `立即更新` → 下载 + 安装进度 → 自动重启 → 前端 reload 一次且只一次。
- TC-Upd-05:active 状态下 `GET progress` 返回 `idle`,弹窗不卡 Working:无断连时强制 refresh version-check,有断连时 reload。
- TC-Upd-06:CLI `bifrost upgrade` 与托盘/Admin 后台升级都在新二进制安装后自动 `install-skill --tool all -y` 并重启;测试使用临时 HOME/USERPROFILE、`BIFROST_INSTALL_SKILL_SOURCE=embedded`、`BIFROST_DISABLE_TRAY=1` 等。
- TC-Upd-07:升级失败(断网/坏归档)托盘 + Web 双端都给失败提示可重试。
- TC-Upd-08:Light/Dark 双主题弹窗与进度条样式正确。

## Review/Fix/Test 闭环

### 第 1 轮

- 复核用户目标:两个可视化入口 + 单一进度通道 + 自动 reload 一次。
- 复核修改:core 进度模块、upgrade_background、SelfUpdate 隐藏命令、admin handler、tray 菜单、Web VersionModal。
- 执行 `cargo fmt`、`cargo clippy --workspace --all-features -D warnings`、`cargo test --workspace --all-features`、`pnpm lint && pnpm typecheck && pnpm build`、`bash e2e-tests/tests/test_upgrade_admin_api_restart_e2e.sh`、`bash e2e-tests/tests/test_cli_tray_startup_ci.sh`。

### 第 2 轮

- 复查 diff:进度文件并发写、stale 归一化、升级失败回退、reload 去重、双主题、crate 依赖方向未被打破。
- 磁盘 latest / 内存旧版本组合场景 self-update 仍重启旧 daemon。
- 复跑受影响单元 + E2E + 关键 human_tests。

## 风险与决策

- **Admin 内嵌升级导致自杀重启**:强制走 detached `self-update` 子进程,admin 不承担二进制替换与重启,只做协议边界与进度可读。
- **magic string `UPGRADE_PROGRESS:`**:早期方案用 stdout 信号解析,已被否决;改成文件通道,天然支持跨代理重启存活与多端并发读取。
- **`bifrost upgrade` 交互路径“already latest”短路**:后台路径必须绕过,否则磁盘 latest 但旧 daemon 仍跑的场景下升级看似成功但服务未更新。
- **runtime marker 被其它流程清理**:不能仅依赖 marker 判断“是否运行”。Admin 必须传递精确 PID/port,updater 必须先做 owner 校验再恢复 marker;校验失败时宁可不重启并保留诊断,不能误杀未知监听进程。
- **嵌套 App 更新提前宣告成功**:CLI 联动 App 的子流程不拥有共享 terminal progress;外层 CLI 完成二进制替换、App best-effort 更新与 daemon 重启后才写 `completed`。
- **skill 安装失败**:提示手动重试即可,不回滚新二进制,避免让升级变成“成功又不成功”的中间态。
- **Windows / 无头 CI 上的原生 tray**:tray helper 无法长驻,继续保留“log-only 降级模式”,不把 tray 缺失误判为升级失败。
- **`稍后提示` 覆盖范围**:仅本会话记住 `latest`,下次进程重启仍会弹;避免用户永久错过关键升级。
