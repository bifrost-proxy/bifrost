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

进度文件 `<data_dir>/upgrade-progress.json` 采用“目标目录内唯一临时文件 + flush/sync + 平台原子 replace”写入。Admin、CLI、Windows helper 与重启后的 desktop core 在所有权交接窗口可能短暂并发，禁止共用固定 `.tmp` 文件名；否则一个 writer 会 rename 另一个 writer 的临时文件，令 Web UI 看到旧状态或误退化为 `Idle`。读方(托盘/admin/web)对解析失败或缺文件一律退化为 `Idle`。文件天然跨代理重启存活,让 web 在重连后仍能读到 `Completed` 终态。

`UpgradePhase`:

```
Idle → Checking → Downloading → Installing → Restarting → Completed / Failed
```

只有 `Downloading` 阶段的 `percent` 有意义;`Installing` / `Restarting` 走步骤态。

### 故障推演与终态约束

| 故障窗口 | 禁止出现的假成功/异常状态 | 收敛方式 |
| --- | --- | --- |
| version-check 声称可升级但没有 target | CLI/App 各自重新追随不同的 latest | Admin 拒绝启动；所有安装器只接受同一个 pinned target |
| Web、Tray 或多个浏览器同时点击 | 两个安装器交叉替换、两个重启器争抢端口，loser 留在 Checking | Admin 进程锁保护 check→claim→spawn，`upgrade.lock` 保护跨进程安装与重启；锁竞争 loser 立即写 terminal `Failed` |
| Admin、CLI、desktop helper 在 handoff 边界并发写 progress | 固定临时文件互相覆盖，状态停留旧 phase 或短暂读成 `Idle` | 每次写使用同目录唯一临时文件并原子替换目标；PowerShell helper 同样使用 PID + GUID 临时名并在 finally 清理 |
| 下载、解包、安装器或 Homebrew 卡住 | progress 超过 stale 门限后先失败、随后又变成功 | 所有长等待有超时并每 30 秒写心跳；超时终止 child 并写 `Failed` |
| 安装命令退出 0，但磁盘 CLI/App 仍是旧版 | UI 写 `Completed`，实际运行仍显示旧版本 | 在任何 companion 更新和重启前执行 `--version` / bundle version 精确 target 核验 |
| CLI 成功但已安装 App 更新失败，或反之 | 两个组件永久漂移但整体宣告完成 | 后台统一升级把 companion 失败视为整体失败；部分成功仍保留更新入口供重试 |
| macOS App 在 target→backup→staging rename 窗口中断 | 下一进程使用不同 PID backup 名，无法恢复唯一旧 App | 使用跨进程稳定 backup 名；下一次尝试先恢复 backup，staging 完整复制并通过版本核验后才交换 |
| Windows 运行中 CLI 延迟替换中断/版本错误 | App 立即探测旧 exe 并误报失败，或错误 exe 被当作成功 | App 在 CLI child 退出后有界重试 `--version`，等待 deferred helper 完成；PowerShell/外层验证失败都恢复旧 exe |
| 安装包可运行但版本不是 pinned target | 手动/script 路径清掉 backup 后留下错误 CLI | exact target 核验通过前保留 binary backup；不匹配时恢复旧 CLI 并写 `Failed` |
| 浏览器连接 desktop-owned core 后点击更新 | App 安装完成但没有 Tauri handoff，页面 reload 后假成功 | desktop-owned core 只接受带 Tauri 一次性 origin token 的 `channel=desktop`；普通浏览器请求返回 409 并提示从桌面 App 发起 |
| CLI-owned core 停止后新 daemon 启动失败 | 进度仍 completed 或端口被双重占用 | 精确 PID/port 所有权校验、等待端口释放、启动失败写 `Failed` |
| App handoff 无法拉起新 App/新 core | WebView 普通 reload 掩盖失败 | Tauri/helper 写持久化 `Failed`；只有新 managed core ready 且 deferred App 版本等于 pinned target 才写最终 `Completed` |
| Desktop shell 复用 CLI-owned core | CLI 的 `Restarting` 被误当作 App handoff，两个 owner 相互抢重启 | Web UI 只对 `source=desktop` 的 `Restarting` 调用 Tauri handoff；`source=admin` 保持 CLI restart 轮询 |
| macOS 同时安装系统级与用户级 App | Admin 选择第一个候选目录，更新非当前运行副本 | desktop Admin 不传 `--app-dir`；bundled core 从自身 executable 向上解析当前 `Bifrost.app` |
| Windows App/sidecar 仍运行时执行 MSI/EXE | 安装器因已加载文件锁失败，或清理用户传入的本地包 | App updater 写含 package ownership 的 pending-install marker；Tauri handoff 等 App/core 退出后再有界运行安装器，仅删除 updater 下载包并拉起新 App |
| Windows deferred installer 返回成功但装入错误 App/core | 新 App 写 `Failed` 后错误版本留在磁盘，下一次启动仍失败 | helper 安装前在目标目录外保留完整快照，等待新 App 编译版本与 managed core 共同确认；不匹配/启动失败时让新 App 正常退出，恢复旧目录并重启旧 App，成功后才清理 guard/快照/自有安装包 |

### 升级发起方与自恢复

- `source ∈ { "tray", "admin", "cli" }`:仅用于诊断。
- **stale active**:`updated_at` 超过 120 秒仍未更新且仍处 active,`GET /api/system/upgrade/progress` 归一化为 `Failed`,避免 UI 卡死在 Working。
- **磁盘二进制已是 latest**:`upgrade` 交互路径会“already latest”直接退出;`self-update` 必须绕过该短路,始终对运行中的旧 daemon 触发 `maybe_restart_running_proxy`。
- **runtime marker 缺失**:Admin 子进程参数携带发起请求的 PID/端口。已有且 PID/port 匹配的 restartable daemon marker 可直接作为权威依据，不依赖可选的 `lsof`；只有 marker 缺失或需要把 foreground 规范化为 daemon 时才要求端口 owner 与 PID 完全一致。无 host 可恢复时使用 `127.0.0.1`，禁止扩大为 `0.0.0.0`。
- **进度所有权**:最外层 `self-update` 是 CLI channel 唯一 terminal progress writer。CLI 联动的 App 安装仍输出诊断日志,但 `source=cli-upgrade` 时不触碰 `upgrade-progress.json`,避免 Web UI 在 daemon 重启前提前观察到 `completed`。
- **单升级器所有权**:Admin 进程用互斥锁串行化 `check → claim → spawn`,实际 `self-update` 再持有 `<data_dir>/upgrade.lock` 跨进程排他锁。受控 CLI/App child 只有同时携带内部 parent-lock marker 才复用父事务的锁；可见的 `source=cli-upgrade` 本身不能绕过锁。CLI-owned orchestrator 启动 App companion 时，无论 caller-managed 即时安装还是 Windows desktop handoff，都必须把 parent-lock marker 显式传给 child；否则 child 会争用父锁并在 core 重启前终止统一升级。Windows pending desktop handoff 是锁释放后的延续 guard，竞争者不得覆盖它已有的 `Restarting` progress。Web UI 双击、两个浏览器、Tray 与 Web UI 同时发起时最多一个升级器能进入安装/重启阶段。
- **进度心跳与有界等待**:active progress 的 stale 门限为 120 秒,所有可能等待 600 秒的 CLI/App 子进程必须至少每 30 秒刷新一次 progress,并在自身超时后终止子进程、写 `Failed`;禁止 UI 先显示“无响应”,稍后又跳回成功。

### 运行时所有权与统一升级编排

GET version-check 仍按 Web UI query 展示 CLI 或 App bundle 的 `current_version`,但 `has_update` 是 CLI 与已安装 App 的并集：主组件已到 target、伴随组件仍旧时也必须保留更新入口。POST upgrade 的版本门禁与实际 orchestrator 都由当前 core 的真实启动模式决定,避免过期或冲突 query 用错误组件做“无更新”判断:

- **CLI-owned**:`BIFROST_DESKTOP_CORE` 未启用。Admin 强制选择 CLI orchestrator,传递当前 PID/port 给 `self-update`;外层依次更新 CLI、已安装 App,最后重启 CLI core。原本由 `start -d` 启动的 daemon 直接按 runtime snapshot 重启；直接执行 `bifrost upgrade` 遇到另一个 foreground CLI core 时也归 CLI updater 接续，不能因其不可 daemon-restart 就误判为 desktop-owned。Web UI 位于 CLI 前台 core 时，精确 PID/port owner 校验通过后先把该 snapshot 转为 detached-daemon 接续契约，再停止旧前台进程并启动新 daemon。后台链路中 App 伴随更新是完成门禁,失败时整体 progress 为 `Failed`,禁止 CLI 已更新却向 UI 宣告整体完成。
- **App-owned**:`BIFROST_DESKTOP_CORE=1`。只有桌面 shell 先通过 Tauri command 签发短时、一次性 origin token，再发送带该 token 的 `channel=desktop` 请求，才能启动 desktop orchestrator；普通浏览器或仅伪造 query/header 的 REST 调用没有共享数据目录中的 token 文件，Admin 返回 409，避免没有 Tauri handoff 的调用者安装 App 后无法重启。App orchestrator 先调用独立 CLI 的 `upgrade -y`,同时注入 `BIFROST_DESKTOP_MANAGED_UPGRADE_SKIP_APP=1`、`BIFROST_DESKTOP_MANAGED_UPGRADE_SKIP_RESTART=1`、parent-lock marker 与同一个目标版本,保证 CLI 子流程复用父锁、只更新精确目标 CLI,不会递归安装 App、停止 desktop core 或抢占重启。Windows deferred helper 从事务对象直接读取 pinned target，不从可缺失或陈旧的 progress 文件反推；父 App 收口时 helper 不发布 terminal progress，App 持续重试版本探针并统一决定成功或失败。desktop Admin 不覆盖 `--app-dir`，由 bundled core 的 `current_exe()` 解析正在运行的 App 副本。macOS 完成原子 App swap 后进入 `Restarting`；Windows 把 MSI/EXE 与“是否由 updater 下载”一起登记为 pending install，同样进入 `Restarting`。Web UI 只在 progress 同时满足 `source=desktop` 与 `phase=Restarting` 时调用 Tauri handoff；helper 等旧 App/core 退出后执行 Windows installer，仅清理 updater 自有包。新 App/core ready 后还要核对 App 编译版本与 pinned target，一致才写 `Completed`，不一致写 `Failed`。
- Windows deferred helper 的安装事务跨越旧 App、installer、新 App 三个进程阶段：旧 App/core 退出后先把安装目录快照到目标外并把 rollback metadata 原子写入 relaunch marker；installer 成功不等于提交，pending guard、快照与 updater 自有包继续保留。新 App 编译版本与新 managed core 都 ready 后才提交清理；任一不匹配、提前退出、120 秒验证超时或 core 启动失败都会触发新 App 正常关停，helper 仅终止安装目录内残留进程、恢复完整旧目录、保持 `Failed` 并重新拉起旧 App。
- desktop channel 的 unified version-check 以已安装 App 为 primary，并从与 CLI updater 相同的 PATH、`BIFROST_INSTALL_DIR`、用户安装目录和平台默认路径解析独立 CLI；通过有界 `bifrost --version` 探针比较 companion。即使 App/core 已到 target，只要独立 CLI 仍旧，WebUI 仍显示更新并进入同一个 pinned-target orchestrator；缺失、非零退出或超时才回退到 bundled core 版本。
- macOS desktop-owned core 的 App primary 版本必须先从当前 sidecar executable 向上解析实际 `Bifrost.app` bundle，再回退 `/Applications` 与 `~/Applications` 候选。系统级与用户级副本并存时，不能用第一个全局候选掩盖正在运行的旧 App；`BIFROST_APP_INSTALL_DIR` 显式测试/管理覆盖仍保持最高优先级。
- **硬边界**:App-owned 编排不传 restart hint，CLI updater 读取到 `RuntimeStartMode::Desktop` 时直接跳过 proxy restart。CLI-owned Web UI 编排必须携带并校验当前 PID/port；验证通过的 foreground runtime 会被规范化为 restartable daemon，验证失败则终止升级。
- **共同结果**:两种入口都以同一个 pinned target 同时升级 CLI + 已安装 App；直接 `app upgrade --version` 也以 skip-App/skip-restart 行为把该 target 传给 CLI 引擎，不能递归编排或重新观察 `latest` 后发生版本分叉。所有顶层 CLI/App updater 共用 `upgrade.lock`，只有携带 parent-lock marker 的内部 companion 跳过重复加锁。重启所有权互斥：CLI-owned 由 CLI updater 重启 daemon，App-owned 由 Tauri handoff 重启 App/core，不允许两个重启器同时操作同一监听端口。
- **发起界面与运行时 owner 解耦**:Admin 不把 caller-controlled `channel=desktop` 当作 WebView 证明；桌面进程通过 Tauri command 在共享数据目录签发 UUID origin token，Web UI 放入专用 header，Admin 原子消费且校验 30 秒有效期后才向 CLI-owned orchestrator 传递私有 origin marker。不能仅凭 `RuntimeStartMode::Desktop` 推断 WebView 正在等待 handoff。已安装桌面 shell 正在运行且请求携带真实、未消费的 WebView token 时，CLI companion 才发布 `source=desktop/Restarting` 交给 Tauri；CLI-owned core 仍由 CLI updater 重启，Tauri marker 不包含未托管的 core child。终端或普通浏览器发起时不允许留下等待 WebView 的 pending install：macOS/Windows companion 先通过 single-instance 内部 shutdown 参数请求当前 shell 安全退出；旧 App 不认识该参数时，回退 macOS 系统 Quit 或 Windows process-tree termination request。只有确认目标 App executable 已全部释放后才执行 caller-managed 安装并重新拉起；退出超时则拒绝覆盖，不能冒险替换锁定文件。多 App 副本并存时 companion 优先选择实际正在运行的候选。
- progress 写权限跟随 `upgrade.lock` owner：任何 contended loser 都只记录诊断并退出，不能写共享 `Failed` 覆盖 owner 的 Checking/Downloading/Installing/Restarting；pending desktop handoff 同样由 marker owner 保留状态。只有成功持锁的 updater 或无法打开锁文件的独立启动失败路径可以发布自己的终态。

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
pub fn write_progress(&Path, &UpgradeProgress); // 唯一同目录 tmp + sync + 原子 replace
pub fn clear_progress(&Path);
pub fn is_stale(&UpgradeProgress, max_age_secs) -> bool;
pub const DEFAULT_STALE_SECS: i64;
```

### 升级引擎旁路进度上报(bifrost-cli)

升级引擎按职责拆分，所有文件保持在 1500 行以内：`upgrade.rs` 负责主编排与安装，`upgrade/download.rs` 负责镜像/下载/Homebrew，`upgrade/restart.rs` 负责 runtime marker、deferred replacement 与重启，`upgrade/tests.rs` 承载回归测试。

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
  - query 只表达请求目标，不作为调用 surface 证明：CLI-owned core 始终使用 CLI orchestrator；desktop-owned core 只接受桌面 shell 通过 Tauri 签发的一次性 token + `channel=desktop`，无 token 的浏览器/REST 请求返回 409。版本门禁仍由真实 runtime owner 决定，并用 CLI/App 更新并集避免部分漂移后入口消失。
  - 读 `read_progress`,若非 stale 的 active 升级 → 409。
  - 读 `VersionChecker` 最新结果,若无可用更新 → 409。
  - `has_update=true` 但缺失 target version 时直接失败；禁止退回未固定的 `latest`。写入初始 `UpgradeProgress { phase: Checking, source, target_version }`。
  - CLI-owned:spawn detached `bifrost self-update --target <v> --source admin --running-proxy-pid <pid> --running-proxy-port <port>`;PID/port 参数为隐藏内部协议且必须成对出现。
  - App-owned:spawn detached `bifrost app upgrade --version <v> --source desktop -y`；不传候选目录，避免多副本时覆盖非当前运行 App。App 命令从 bundled core executable 解析活跃 App 路径，并联动带 skip-app/skip-restart 所有权标记的独立 CLI。
  - binary 定位:`std::env::current_exe()` 优先(admin 与 bifrost core 同进程),fallback `PATH` 中 `bifrost`。
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
- **runtime marker 被其它流程清理**:完整且匹配的 restartable daemon marker 不依赖 `lsof`；marker 缺失/foreground 接续时，Admin 必须传递精确 PID/port,updater 必须做 listener owner 校验再恢复 marker，且缺失 host 只能按 loopback 恢复。校验失败时终止并保留诊断，不能误杀未知监听进程或扩大网络暴露面。
- **嵌套 App 更新提前宣告成功**:CLI 联动 App 的子流程不拥有共享 terminal progress;外层 CLI 完成二进制替换、App 必须更新与 daemon 重启后才写 `completed`。
- **App 与 CLI 相互递归/争抢重启**:App-owned 编排给独立 CLI 注入 skip-app/skip-restart,CLI updater同时拒绝接管 Desktop runtime；desktop-owned core 拒绝普通浏览器 channel，三层边界共同阻断双重安装和无 handoff 安装。
- **同时点击或 Tray/Web UI 并发升级**:Admin 请求锁只解决同一服务内的并发,`upgrade.lock` 再覆盖不同入口/进程；竞争失败方不安装、不停止服务，并立即把自己预写的 Checking 收敛为 terminal failure。
- **latest 在升级中变化**:Admin 选定的 target 是本次事务的一致性边界,CLI 和 App 都必须使用该 target,不得在子流程重新解析新的 latest。
- **安装渠道绕过 target**:`~/.bifrost/bin` 的 script 安装改走内置 target-aware 原子替换,不再重新执行永远跟随最新版本的在线脚本；Homebrew 命令有心跳/超时,重启与核验使用稳定的 `bin/bifrost` launcher 而不是可能被 reinstall 删除的 Cellar 版本路径。所有非 deferred CLI 安装完成后都执行 `--version == target` 门禁。
- **子命令退出 0 但没有升级**:App-owned 路径以 `bifrost --version == target` 作为 CLI 完成门禁；CLI-owned 后台无法识别安装方式或无法解析目标时返回失败,不得沿用交互命令“提示手动安装后返回 0”的软失败语义。
- **长时间安装被误判 stale**:等待独立 CLI 或 App 安装时持续刷新 Installing 心跳,并保留 600 秒硬超时；超时后杀掉子进程并发布失败,状态不会从 stale failed 反跳 completed。
- **App 覆盖到一半失败**:macOS bundle 先复制到同目录 staging、核验目标版本,再用 rename 切换；backup 使用跨 PID 稳定路径，旧 App 在 staging 校验通过前保持不动，切换失败或下一进程重试时都可恢复。
- **App 已安装但 Tauri handoff 启动失败**:native command 在 marker/helper 任一步失败时先把共享 progress 从预完成状态覆写为 `Failed` 并清理无效 marker；Web UI 同时显示真实错误并允许重试，不得 fallback 到普通 reload。
- **App 模块失控增长**:`app.rs` 只保留升级编排，平台安装/原子替换位于 `app/installer.rs`，测试位于 `app/tests.rs`；三个文件都受 1500 行门禁约束。
- **skill 安装失败**:提示手动重试即可,不回滚新二进制,避免让升级变成“成功又不成功”的中间态。
- **Windows / 无头 CI 上的原生 tray**:tray helper 无法长驻,继续保留“log-only 降级模式”,不把 tray 缺失误判为升级失败。
- **`稍后提示` 覆盖范围**:仅本会话记住 `latest`,下次进程重启仍会弹;避免用户永久错过关键升级。
