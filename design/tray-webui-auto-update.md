# 托盘 / Web UI 后台自动更新

## 功能模块详细描述

当前 Bifrost 的升级只能通过手动执行 `bifrost upgrade` 命令完成，用户必须切到终端、复制命令、确认、再手动重启代理。本模块把“升级”从纯 CLI 操作升级为**桌面托盘**与 **Admin Web UI** 两条可视化入口，核心目标：

1. **托盘一键更新**：当检测到新版本时，托盘菜单出现“Update to vX.Y.Z”入口。点击后后台静默下载、安装，托盘状态实时显示“正在更新”，安装完成后自动重启代理。
2. **Web UI 版本管理弹窗**：检测到新版本时弹出版本管理弹窗，右下角提供两个按钮——“立即更新”和“稍后提示”。
3. **Web UI 进度展示**：点击“立即更新”后，Web UI 实时展示**下载进度**与**安装进度**两段进度；安装完成后代理自动重启；重启完成、前端重新连上后端后，Web UI 自动刷新一次。

本模块**不重写**现有升级引擎（`crates/bifrost-cli/src/commands/upgrade.rs`），而是复用其下载/原子安装/重启能力，新增一条**结构化进度通道**与**后台触发入口**，让托盘和 Admin 两端都能驱动同一套升级逻辑并观察进度。

### 关键架构约束

- crate 依赖方向为 `bifrost-cli → bifrost-admin → bifrost-core`。升级引擎位于 `bifrost-cli`，而 `bifrost-admin` **不能**反向依赖 `bifrost-cli`。
- 因此升级进度状态类型与读写逻辑必须下沉到 **`bifrost-core`**（admin 与 cli 都依赖 core），作为跨进程共享通道。
- Admin 触发升级时不能在自身进程内执行升级（升级会停止并重启包含 admin 的代理进程），必须 **spawn 独立的 `bifrost upgrade` 子进程**，由该子进程停止旧代理、安装新二进制并重启。这与托盘现有 `spawn_start` / `spawn_stop` 的进程模型一致。
- 升级进度通过 **`<data_dir>/upgrade-progress.json`** 文件落盘传递：升级子进程为写方，托盘与 admin 为读方。文件落盘可跨“代理重启”存活，使前端在重连后仍能读到 `Completed` 终态。

## 数据结构

新增 `crates/bifrost-core/src/upgrade_progress.rs`：

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpgradePhase {
    Idle,         // 无进行中的升级
    Checking,     // 正在校验版本/准备
    Downloading,  // 下载归档中
    Installing,   // 解压 + 原子替换二进制
    Restarting,   // 停止旧代理 / 等待端口释放 / 拉起新代理
    Completed,    // 升级成功（新进程已起或等待前端刷新）
    Failed,       // 升级失败
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpgradeProgress {
    pub phase: UpgradePhase,
    /// 0.0 - 100.0，仅 Downloading 阶段有意义；其余阶段为 None
    pub percent: Option<f64>,
    /// 人类可读的状态文案（英文，复用 download_progress_line 等）
    pub message: String,
    /// 目标版本，例如 "0.0.104"
    pub target_version: Option<String>,
    /// 升级发起方，用于诊断："tray" / "admin" / "cli"
    pub source: Option<String>,
    /// 失败原因（phase == Failed 时）
    pub error: Option<String>,
    /// 最近一次更新时间（RFC3339）
    pub updated_at: String,
}

impl UpgradeProgress {
    pub fn idle() -> Self { /* phase = Idle */ }
    pub fn is_active(&self) -> bool {
        matches!(self.phase, UpgradePhase::Checking | UpgradePhase::Downloading
            | UpgradePhase::Installing | UpgradePhase::Restarting)
    }
}

pub fn progress_file_path(data_dir: &Path) -> PathBuf; // data_dir/upgrade-progress.json
pub fn read_progress(data_dir: &Path) -> UpgradeProgress; // 读不到/损坏 → idle()
pub fn write_progress(data_dir: &Path, progress: &UpgradeProgress); // 原子写（tmp + rename）
pub fn clear_progress(data_dir: &Path); // 删除文件
/// 判定 stale：updated_at 超过 N 秒未更新且仍处 active，视为异常残留
pub fn is_stale(progress: &UpgradeProgress, max_age_secs: i64) -> bool;
```

> 写入采用 `write tmp + rename` 原子替换，避免读方读到半截 JSON。读方对解析失败/缺文件一律退化为 `idle()`，保证升级通道不会因脏文件阻塞。

## 实现逻辑

### 1. 共享进度通道（bifrost-core）

- 新增 `upgrade_progress` 模块（约 150 行，独立文件，不触碰其它 core 文件的行数预算）。
- 在 `crates/bifrost-core/src/lib.rs` 中 `pub mod upgrade_progress;`。
- 单元测试覆盖：原子写读 round-trip、损坏文件退化为 idle、`is_active` / `is_stale` 边界。

### 2. 升级引擎接入进度回调（bifrost-cli/upgrade.rs）

升级引擎已有的下载/安装/重启逻辑保持不变，仅新增一条“进度上报”旁路，**默认关闭**，避免影响普通 CLI 交互体验：

- `handle_upgrade` 现有签名 `(force, restart)` 扩展为内部统一入口，新增 `progress_sink: Option<ProgressSink>`。对外保留：
  - `pub fn handle_upgrade(force, restart)`：CLI 交互路径，`progress_sink = None`，行为完全不变。
  - `pub fn handle_upgrade_background(target_version, source)`：后台路径，构造一个写文件的 `ProgressSink`，等价于 `force = true, restart = true`，并在各阶段写 `UpgradeProgress`。
- `ProgressSink` 是对 `data_dir` 的轻封装：
  - 阶段切换时写 `Checking/Downloading/Installing/Restarting/Completed/Failed`。
  - 下载阶段：在 `download_file_once_with_progress` 的渲染节流点（已有 250ms 节流）回调 `percent`，文案复用 `download_progress_line`。
- 为了不让 `upgrade.rs`（现已 2607 行，超过 1500 行限制）继续膨胀，新增逻辑放入 **新文件** `crates/bifrost-cli/src/commands/upgrade_background.rs`：
  - 封装 `ProgressSink`、`handle_upgrade_background`、阶段编排。
  - 通过对 `upgrade.rs` 暴露的少量 `pub(crate)` 函数（`download_and_install`、`maybe_restart_running_proxy`、版本获取）进行编排；必要时把这几个函数从私有改为 `pub(crate)`，不改动其实现体。
- 下载进度回调改造：给 `download_file_with_progress` / `download_file_once_with_progress` 增加一个 `on_progress: &mut dyn FnMut(u64, Option<u64>, Instant)` 参数，CLI 交互路径传入“打印到 stdout”的闭包（保持原行为），后台路径传入“写进度文件”的闭包。函数体只在原渲染点多调一次回调，逻辑零行为变化。

### 3. 后台升级 CLI 入口

- 在 `Commands` 枚举（`crates/bifrost-cli/src/cli.rs`）新增隐藏子命令：
  ```
  #[command(hide = true, about = "Run an unattended background upgrade (used by tray/admin)")]
  SelfUpdate { #[arg(long)] target: Option<String>, #[arg(long, default_value = "cli")] source: String }
  ```
  - 隐藏命令（`hide = true`），不污染用户可见帮助，仅供托盘/admin 内部 spawn。
  - `main.rs` 路由到 `handle_upgrade_background(target, source)`。
- 该命令等价于 `bifrost upgrade --yes --restart`，但全程写进度文件，无交互、无 stdin 阻塞。

### 4. 托盘后台更新入口（bifrost-cli/tray）

- **菜单项**：`build_menu` 新增条件项 `update_now`，仅当存在新版本时显示，label = `Update to v{latest}`。
  - 新版本判定：托盘读取 `data_dir/version_cache.json`（已有，admin `VersionChecker` 与 update_check 共用该缓存）与当前版本比对 `is_newer_version`。
  - 升级进行中（`read_progress().is_active()`）时该项禁用并显示进行态。
- **新动作** `MenuItemAction::StartUpgrade { target_version }`：
  - `execute_action` 中新增分支：检查 `operation` 未忙 → 置新操作态 `OP_UPGRADING` → `spawn_tray_task` 中 `spawn` 出 `bifrost self-update --target <v> --source tray`（detached，复用 `resolve_bifrost_binary` + `configure_service_command`）。
- **状态指示**：
  - 新增 `OP_UPGRADING = 3`（与现有 OP 常量错开，注意现状已占用 0/1/2/4/5）。
  - `operation_status_label(OP_UPGRADING)` → `Some("Bifrost: Updating…")`。
  - `operation_busy` 纳入 `OP_UPGRADING`，升级期间禁用 start/stop/update。
  - 托盘状态轮询线程额外读取 `upgrade-progress.json`：
    - `Downloading` → 状态文案 `Bifrost: Updating… {percent}%`。
    - `Installing` / `Restarting` → `Bifrost: Updating…`。
    - `Completed` → 清操作态回 `OP_IDLE`，清进度文件，恢复正常状态展示（升级子进程会重启代理，托盘 state 轮询自然恢复 Running）。
    - `Failed` → `OP_UPGRADE_FAILED`，状态 `Bifrost: Update failed - open logs`，允许重试。
- 升级子进程会停止旧代理→替换二进制→拉起新代理。托盘自身是独立进程不受影响，仅通过文件观察进度。

### 5. Admin 升级 API（bifrost-admin）

`crates/bifrost-admin/src/handlers/system.rs` 在现有 `/api/system/*` 下新增：

- `POST /api/system/upgrade`：
  - 读取 `version_checker` 最新结果；无可用更新返回 `409 Conflict`。
  - 若 `read_progress().is_active()` 且非 stale，返回 `409`（已有升级在进行）。
  - 写入初始 `UpgradeProgress { phase: Checking, source: "admin", target_version }`。
  - **spawn** `bifrost self-update --target <v> --source admin`（detached）。定位二进制：优先 `std::env::current_exe()`（admin 运行在 bifrost 进程内，current_exe 即 bifrost 本体），回退 `PATH` 中 `bifrost`。
  - 返回 `202 Accepted` + 当前进度快照。
- `GET /api/system/upgrade/progress`：
  - 返回 `read_progress(data_dir)`。stale active（默认 120s 未更新）归一化为 `Failed`（提示“升级进程无响应”）。
- 复用 `state.rs` 已有 `data_dir`（与 `version_check` 同源 `bifrost_storage::data_dir`）。
- Admin 不直接执行升级，规避“自杀式重启”问题；进度文件在重启前由子进程持续更新，重启后端口恢复，前端 `GET progress` 读到 `Completed`。

> 安全：升级触发是高权限操作，沿用 admin 现有同源/鉴权中间件（与 `version-check`、`proxy/system` 一致），不新增公网暴露面。

### 6. Web UI 版本管理弹窗 + 进度（web/）

技术栈为 **Ant Design**（`antd`），`VersionModal` 已存在，沿用 antd `Modal` / `Progress` / `Button`，遵循双主题（`theme.useToken()` 变量，禁止硬编码颜色）。

- `web/src/api/version.ts` 新增：
  - `startUpgrade(): Promise<UpgradeProgress>` → `POST /system/upgrade`。
  - `getUpgradeProgress(): Promise<UpgradeProgress>` → `GET /system/upgrade/progress`。
- `web/src/types/index.ts` 新增 `UpgradePhase` / `UpgradeProgress` 类型，与后端字段对齐（snake_case）。
- `useVersionStore` 新增升级态：`upgradePhase`、`upgradePercent`、`upgradeMessage`、`upgradeError`、`startUpgrade()`、`pollUpgradeProgress()`。
- `VersionModal`：
  - 升级可用且未在升级时，footer 右下角渲染两个按钮：**“稍后提示”**（次要，点击 `markVersionSeen(latest)` + 关闭）与 **“立即更新”**（主按钮，调 `startUpgrade()`）。
  - 点击“立即更新”后弹窗进入**进度态**（弹窗不可手动关闭，隐藏右上角关闭与“稍后提示”）：
    - **下载进度**：`antd Progress`，百分比取 `upgradePercent`（`Downloading` 阶段）。
    - **安装进度**：`Installing`/`Restarting` 阶段以步骤态展示（Progress 切 `status="active"` 或步骤文案“Installing… / Restarting…”）。
  - 轮询：进入升级态后每 ~1s `getUpgradeProgress()`，直至 `Completed` / `Failed`。
- **自动刷新**：
  - 升级进入 `Restarting` 后，后端会断开（代理重启）。前端 `pushService` 现有 `onConnectionChange` 会先 disconnected 再 reconnected。
  - 监听“在升级态下检测到一次 disconnected→reconnected”后，或轮询读到 `Completed` 后，执行**一次** `window.location.reload()`。用 sessionStorage 标志位（`bifrost-upgrade-reload-pending`）防止重复刷新与刷新风暴。
  - 刷新后清理标志位与进度（`Completed` 由后续 `version-check` 自然归零；前端不再展示更新入口）。
- `useGlobalDataSync` 在初始化时检查 `sessionStorage` 升级标志，若存在且后端已恢复，确保只刷新一次。

### 7. 文件行数与边界

- 新增逻辑尽量放新文件：`upgrade_progress.rs`（core）、`upgrade_background.rs`（cli）、web 侧新增 hook/组件片段，避免触碰超长文件。
- `upgrade.rs` 仅做“函数签名加回调参数 / 可见性放宽”的最小改动，不新增大段逻辑。

## 测试方案

### 单元测试

- **bifrost-core / upgrade_progress**：
  - `write_progress` + `read_progress` round-trip 还原所有字段。
  - 文件不存在 / JSON 损坏 → `read_progress` 返回 `idle()`。
  - `is_active` 覆盖各 phase；`is_stale` 在超时/未超时边界正确。
- **bifrost-cli / tray**：
  - `build_menu` 在有新版本时包含 `update_now`，label 含目标版本；无更新时不含。
  - 升级进行中 `update_now` 被禁用。
  - `operation_status_label(OP_UPGRADING)` / `operation_busy(OP_UPGRADING)` / `OP_UPGRADE_FAILED` 行为。
- **bifrost-cli / upgrade_background**：
  - `ProgressSink` 在各阶段写出预期 `phase`；下载回调把 `percent` 写入文件。
  - 失败路径写 `Failed` + `error`。
- **bifrost-admin / system handler**：
  - 无更新时 `POST /api/system/upgrade` → 409。
  - 已有 active 升级 → 409。
  - 正常 → 202 + 初始进度；`GET progress` 返回写入值；stale active 归一化为 Failed。

### 接口化 E2E 测试（先编译、再启动 Bifrost、再用真实接口验证）

按 `.trae/skills/e2e-test` 流程：

1. 编译最新 Bifrost 二进制。
2. 临时数据目录启动服务（`--no-system-proxy`、避开 9900 端口、`BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`）。
3. `GET /_bifrost/api/system/version-check`：构造/注入一个“有新版本”的 `version_cache.json`，断言 `has_update = true`。
4. `POST /_bifrost/api/system/upgrade`（用测试版本覆盖 env 走 `test_upgrade_latest_version_override`，避免真实下载 GitHub），断言 202。
5. 轮询 `GET /_bifrost/api/system/upgrade/progress`，断言 phase 由 `Checking`→…→ 终态推进，字段结构正确。
6. 无更新时 `POST upgrade` 断言 409。

> E2E 用测试钩子避免真实联网下载，验证的是 API 协议、进度文件读写、状态机推进，而非真实二进制替换。

### 真实场景测试（human_tests）

新增 `human_tests/tray-webui-auto-update.md`，逐条真实执行并记录：

- 托盘出现“Update to vX.Y.Z”，点击后状态变“Updating…”，下载/安装/重启后托盘恢复 Running 且版本号更新。
- Web UI 弹出版本管理弹窗，右下角“立即更新 / 稍后提示”两个按钮存在且位置正确。
- 点“稍后提示”后本会话不再自动弹窗。
- 点“立即更新”展示下载进度 + 安装进度；安装后代理自动重启；重启后 Web UI 自动刷新一次且只刷新一次。
- 升级失败（断网/坏归档）时托盘与 Web UI 都给出失败提示并可重试。
- 双主题（Light/Dark）下弹窗与进度条样式正确。

## 两轮 Review / Fix / Test 计划

按 AGENTS.md「至少两轮 Review/Fix/Test」：

- **第 1 轮**：自审 + 跑 `cargo fmt`、`cargo clippy --workspace --all-features -D warnings`、`cargo test --workspace --all-features`、web `pnpm lint && pnpm typecheck && pnpm build`；e2e-test 通过后跑 `rust-project-validate`；修正首轮问题。
- **第 2 轮**：聚焦边界——进度文件并发写、stale 归一化、升级失败回退、自动刷新去重（不刷新风暴）、双主题、crate 依赖方向未被破坏；再次全量校验 + coverage 90% 闸门（`make coverage`）。
- 全绿后 commit / push / 开 PR，经 `github-actions-pat` 守 CI 到绿。

## 行为预期

1. 检测到新版本 → 托盘出现“Update to vX.Y.Z”，Web UI 弹出版本管理弹窗（右下角“立即更新 / 稍后提示”）。
2. 托盘点击更新 → 后台下载安装、状态“Updating…”、完成后自动重启，托盘恢复 Running 且版本已更新。
3. Web UI 点“立即更新” → 展示下载进度 + 安装进度 → 自动重启 → 前端重连后自动刷新一次。
4. 点“稍后提示” → 记住该版本，本会话不再自动弹窗。
5. 升级失败 → 两端均给出失败提示并可重试，旧版本继续可用。
