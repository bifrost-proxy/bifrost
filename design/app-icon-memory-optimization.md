# App 图标内存优化设计方案

## 背景

管理端 Traffic 列表通过 `/_bifrost/api/app-icon/:app` 获取应用图标。当前 macOS 图标提取链路优先调用 `NSWorkspace::iconForFile`，这个路径会在 Bifrost 主进程内加载 Foundation、ImageIO、QuartzCore、Metal、IconServices 等系统框架与缓存。真实观察中，连续请求多个应用图标后，主进程 physical footprint 可从约 80 MiB 上升到 400 MiB 以上，甚至超过 1 GiB，即使停止请求，AppKit / Metal / IconServices 的常驻缓存也不会释放。

这类内存不是来自流量 body store 或 WebSocket payload 缓存，而是 AppKit 图标提取在长生命周期主进程内留下的系统框架与图像缓存。优化目标：保留 App 图标功能，同时把高风险图标提取成本移出主进程，并限制主进程内图标缓存的常驻体积。

## 用户目标验证清单

### 必须实现

- macOS 主进程优先走纯文件解析路径（`.icns` + `icns` crate），不加载 AppKit。
- `.icns` 解析失败或缺失时，通过短生命周期 `bifrost app-icon-worker` 子进程执行 NSWorkspace fallback；子进程退出后 AppKit/Metal 常驻成本随之释放。
- 主进程内不再常驻正向图标字节；正向图标写入磁盘缓存 `<data_dir>/app_info/*.png`，命中直接返回文件字节。
- 主进程仅保留负缓存 `AppIconNegativeCache`，FIFO 驱逐，容量上限 `NEGATIVE_CACHE_MAX_ENTRIES = 1024`。
- 单个图标字节数上限 `MEMORY_CACHE_MAX_SINGLE_ICON_BYTES = 2 MiB`；超出的 worker 输出被拒绝、不落盘也不返回。
- 路径归一：`.app/Contents/MacOS/...` 归一到最外层 `.app` bundle；无 `.app` ancestor 的路径不走 macOS AppKit 提取。
- 前端 `AppIcon` 组件懒加载并做全局请求限流。
- 非 macOS 平台继续使用原 Windows/Linux 图标查找逻辑。
- 不新增分发产物，不改动 release artifact 列表。

### 必须不破坏

- Traffic 列表和 Traffic Detail 抽屉的 App 图标展示继续正常。
- `/_bifrost/api/app-icon/:app` 响应仍返回 `image/png` 与 CORS 头。
- Windows / Linux 图标查找路径行为不变。
- 已有磁盘缓存命中时仍直接返回 PNG，不需要重新提取。
- 系统代理不被测试或图标提取路径修改；helper 子进程不常驻。

### 必须真实验证

- macOS 上真实请求 12 个系统 App 图标（Calculator/Calendar/Contacts/FaceTime/Freeform/Maps/Photos/Preview/Reminders/Shortcuts/Stickies/TextEdit）：至少 3 个 200，其余可 404，主进程 RSS 增量 < 150 000 KB。
- `app_info/*.png` 磁盘缓存真实生成并被下次请求命中。
- helper 子进程真实以 `bifrost app-icon-worker --path ... --output ...` 形式执行并退出。
- 负缓存达到 1024 后按 FIFO 驱逐最旧 key；`remove` 可清除已记录 miss。
- 非 macOS 平台图标行为不变（Windows/Linux CI 或本地手动验证）。

## 产品语义

### 主进程优先纯文件解析，AppKit 只在子进程

macOS 上先定位 `.app` bundle，再从 `Contents/Info.plist` 找到 `CFBundleIconFile` 或 `CFBundleIconName`，读取 `Contents/Resources/<icon>.icns` 并用 Rust `icns` crate 转成 PNG（`extract_icon_via_icns`，优先挑选 `RGBA32_32x32_2x/32x32/64x64/128x128/16x16_2x/16x16` 图层）。该路径不需要 AppKit，不会把 NSWorkspace / Metal 图像栈加载进主进程。

只有 `.icns` 缺失、Info.plist 未声明图标、或 icns crate 解析失败时，才进入 helper 子进程 fallback。

### AppKit fallback 走同二进制的 `app-icon-worker` 子命令

不新增单独分发产物。CLI 增加隐藏内部命令：

```text
bifrost app-icon-worker --path <app-path> --output <temp-png-path>
```

由当前 `bifrost` 可执行文件 re-exec 启动。父进程创建临时 PNG 输出文件，设置超时并校验输出大小；子进程内可以调用 `NSWorkspace::iconForFile`、`NSBitmapImageRep` 转 PNG（`objc2_app_kit`）。子进程结束后 AppKit/Metal/Foundation 的常驻成本随子进程退出释放，不污染主进程 footprint。

使用临时文件而不是 stdout 可以避免大 PNG 写入时被 pipe buffer 阻塞。helper 命令通过 clap `hide=true` 隐藏在 help 之外，不改变用户公开 CLI 面。

### 主进程内只有负缓存，正向图标走磁盘

原缓存把每个 app 的完整 `Vec<u8>` PNG 都按条目数裁剪后挂在主进程堆上，是 footprint 增长的另一个来源。优化后：

- 正向图标只写入磁盘缓存 `<data_dir>/app_info/<key>.png`（`save_to_disk`）；命中时通过 `get_from_disk` 直接从文件读出返回，热图标常驻成本落在可回收的 OS page cache，不计入进程 footprint。
- 主进程仅保留一个负缓存 `AppIconNegativeCache`，记录已知抓取失败的 app key，避免重复触发昂贵的 fallback。负缓存按 FIFO 驱逐，容量上限 `NEGATIVE_CACHE_MAX_ENTRIES = 1024`。
- 单个图标字节数有 `MEMORY_CACHE_MAX_SINGLE_ICON_BYTES = 2 MiB` 上限；超过的 worker 输出被拒绝、不落盘也不返回，同时写入负缓存。

### 路径归一与过滤

从 traffic DB 取到的 client path 可能是：

- `.app/Contents/MacOS/...` 内部可执行文件。
- `/System/Applications/*.app` 系统级应用。
- `target/debug/deps/...`、单元测试二进制、helper、临时进程。
- `.xpc`、`.appex` 或没有用户可感知 App bundle 的系统组件。

优化后先把路径归一到最外层 `.app` bundle；没有 `.app` bundle 的路径不走 macOS AppKit 提取，也不进入 helper。这样避免 UI 中某些短生命周期进程、测试二进制或系统 helper 触发昂贵的图标提取。

### 前端懒加载与并发限流

`AppIcon` 组件通过 IntersectionObserver 只在可见后请求图标，并对全局同时发起的图标请求做限流。列表快速滚动或大量记录渲染时不会一次性打满后端提取队列，也不会同时唤起大量 helper 子进程。

## 技术细节

### 关键文件

- `crates/bifrost-admin/src/app_icon.rs`：常量、`AppIconNegativeCache`、缓存服务、`extract_app_icon`、`extract_app_icon_macos`、`extract_icon_via_icns`、`extract_icon_via_nsworkspace`、`extract_icon_via_worker`、Windows/Linux 分支，以及单元测试。
- `crates/bifrost-admin/src/handlers/app_icon.rs`：`handle_app_icon` HTTP handler；返回 `image/png` + CORS 头 + Cache-Control。
- `crates/bifrost-cli/src/main.rs`：注册隐藏子命令 `app-icon-worker`。
- `crates/bifrost-cli/src/commands/start.rs`：启动主代理时初始化 `AppIconCacheService`（数据目录路径）。
- `web/src/components/AppIcon/...`：前端懒加载与限流（复用现有组件）。

### 常量

```rust
const MEMORY_CACHE_MAX_SINGLE_ICON_BYTES: usize = 2 * 1024 * 1024;
const NEGATIVE_CACHE_MAX_ENTRIES: usize = 1024;
```

### AppIconNegativeCache

FIFO 有界容量，`insert(key)`、`contains(key)`、`remove(key)`、`clear()`。达到容量时 pop_front 旧 key。仅存 key，不存 payload，避免任何 payload 计入 footprint。

### 磁盘缓存布局

```
<data_dir>/app_info/
  <sha_or_normalized_key>.png
```

`get_from_disk` 通过 `tokio::fs::read`（或同步 `std::fs::read`）返回字节；`save_to_disk` 通过 `tokio::fs::write` + tmp file rename 落地，避免部分写入。清理入口 `clear_all_disk_cache` 用于 `data_dir` 重置或用户手动 purge。

### macOS 提取主流程

```
extract_app_icon(path)
  ├─ normalize path → find `.app` ancestor
  ├─ if not `.app` bundle → return None
  └─ extract_app_icon_macos(path)
       ├─ extract_icon_via_icns(path)   # 优先
       └─ extract_icon_via_worker(path) # fallback
```

`extract_icon_via_worker` 生成临时 PNG 输出路径，`std::process::Command::new(current_exe).arg("app-icon-worker").arg("--path").arg(app).arg("--output").arg(tmp)`，设置超时；读取输出文件字节；校验大小 > 0 且 <= `MEMORY_CACHE_MAX_SINGLE_ICON_BYTES`；否则返回 None 并把 key 入负缓存。

### 子进程 `app-icon-worker`

- clap 隐藏命令；参数 `--path` `--output`；不打印 stdout，只写文件。
- 内部用 `objc2_app_kit::NSWorkspace::sharedWorkspace().icon_for_file(...)` 取 NSImage，`NSBitmapImageRep` 转 PNG bytes，写入 output 文件。
- 完成后进程退出，AppKit/Metal 常驻栈随之释放。

### HTTP Handler

`handle_app_icon`：

- 从 path 参数解析 app key / normalize。
- 命中磁盘缓存 → `image/png` + CORS + Cache-Control 返回。
- 命中负缓存 → 404。
- 否则触发提取；成功写盘 + 返回；失败入负缓存 + 404。
- CORS 头：`Access-Control-Allow-Origin: *` 等，`app_icon_success_response_allows_cors` 单测覆盖。

## CLI 与 Admin API

- CLI 新增 hidden 命令 `bifrost app-icon-worker --path <app-path> --output <temp-png-path>`，不出现在 `bifrost --help` 中。
- Admin API `GET /_bifrost/api/app-icon/:app` 保持路径与响应契约：`image/png`、CORS、Cache-Control；404 表示无图标（已 negative-cached）。
- 不新增其他 CLI 或 API。

## Sync / 导入导出 / 分享边界

App 图标是本机数据目录级缓存，不参与 rule sync、group sync、rule share 或规则导入导出。`<data_dir>/app_info/*.png` 属于纯本地衍生数据，可与 traffic DB 独立清理。

## 实现切分

### Phase 1：主进程路径归一与纯文件解析

- 引入 `AppIconNegativeCache` 与常量。
- 实现 `extract_icon_via_icns` 优先路径。
- HTTP handler 命中负缓存直接 404。

### Phase 2：AppKit 隔离到 helper 子进程

- CLI 注册隐藏 `app-icon-worker` 子命令。
- `extract_icon_via_worker` 通过 `current_exe` re-exec 调用；tmp 文件 + rename；超时；大小校验。

### Phase 3：磁盘缓存

- `AppIconCacheService`：磁盘目录初始化、`get_from_disk`、`save_to_disk`、`clear_all_disk_cache`。
- HTTP handler 优先磁盘命中；miss 才触发提取。
- 移除主进程正向 `Vec<u8>` 缓存。

### Phase 4：前端与真实场景验证

- 前端 `AppIcon` 组件懒加载 + 全局请求限流。
- `human_tests/app-icon-memory-optimization.md` 逐条执行。
- `e2e-tests/tests/test_app_icon_memory_guard.sh` 覆盖 RSS 增量断言。

## 测试方案

### 单元测试

- 路径归一：`.app/Contents/MacOS/bin` 归一到 `.app` bundle。
- 路径过滤：`target/debug/deps/foo` 与无 `.app` ancestor 的路径不触发 macOS 图标提取（`macos_icon_path_rejects_non_app_process_paths`）。
- 负缓存：达到 `NEGATIVE_CACHE_MAX_ENTRIES` 后 FIFO 驱逐最旧 key；`remove` 可清除已记录 miss；`clear` 清空。
- Handler：已有缓存命中返回 `image/png` 与 CORS 头（`app_icon_success_response_allows_cors`）。
- 大小上限：模拟 > 2 MiB 输出被拒绝、不落盘、进入负缓存。
- 磁盘缓存：`get_from_disk` / `save_to_disk` 往返一致；tmp rename 原子。

### E2E 测试

`e2e-tests/tests/test_app_icon_memory_guard.sh`：

- 使用临时 `BIFROST_DATA_DIR`、`--no-system-proxy`、`BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1` 启动 Bifrost。
- 分配本地端口。
- 请求 Calculator、Calendar、Contacts、FaceTime、Freeform、Maps、Photos、Preview、Reminders、Shortcuts、Stickies、TextEdit 12 个真实系统 App 图标。
- 至少 3 个 200，其余可 404；不允许 5xx。
- macOS 上采集主进程 RSS：请求前 vs 请求后，增量 < 150 000 KB。
- 退出时停止 Bifrost、清理临时数据目录、释放端口。

### 真实场景测试 human_tests

`human_tests/app-icon-memory-optimization.md` 维护三条用例：

- TC-AIMO-01：多 App 图标请求不显著抬高主进程 RSS。执行 `e2e-tests/tests/test_app_icon_memory_guard.sh`。
- TC-AIMO-02：非 App bundle 路径不触发 macOS 图标提取。执行 `cargo test -p bifrost-admin app_icon::tests::macos_icon_path_rejects_non_app_process_paths`。
- TC-AIMO-03：图标内存缓存按字节驱逐。执行 `cargo test -p bifrost-admin app_icon::tests::memory_cache_evicts_by_total_bytes`。

`human_tests/readme.md` 同步索引。执行时使用 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`、`BIFROST_DATA_DIR=$(mktemp -d)`、`--no-system-proxy`、`--skip-cert-check`、`--unsafe-ssl`、`-y`。

### 覆盖率与项目校验

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test -p bifrost-admin app_icon`
- `bash e2e-tests/tests/test_app_icon_memory_guard.sh`
- `cargo test --workspace --all-features`
- `cargo build --workspace --all-features`
- `rust-project-validate`

本机 no-local-coverage 时不跑 `make coverage`；交付说明豁免并依赖远端 CI。

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核用户目标：内存根因、helper 隔离、磁盘缓存、负缓存 FIFO、前端限流、分发兼容。
- 复核 diff：`git status --short` / `git diff` 是否覆盖 `bifrost-admin` `bifrost-cli` `web` `human_tests` `e2e-tests`。
- 重点 review：`.app` 归一是否覆盖 `MacOS/`、`Contents/Resources` 等分支；helper 子进程超时是否有 upper bound；磁盘缓存 key 是否稳定；负缓存是否与 `data_dir` 生命周期同进程。
- 复测：`cargo test -p bifrost-admin app_icon`；执行 `test_app_icon_memory_guard.sh`。

### 第 2 轮

- 复查第 1 轮修复：多次请求同一图标是否只触发一次 helper；helper 输出超过 2 MiB 是否被拒；负缓存持久性只在内存（不上磁盘）。
- 再次执行 `git status --short` / `git diff` 与 human_tests 索引更新检查。
- 复跑受影响单测、E2E 与 human_tests；出现新问题继续追加轮次。

## 风险与决策点

- helper 子进程启动开销：每次 fallback 都要 fork + re-exec，最坏路径下延迟明显；通过纯文件 `.icns` 优先 + 负缓存去重把 fallback 命中率控制在低位。
- 磁盘缓存持久性：`<data_dir>/app_info/*.png` 属于本地衍生数据，用户 `bifrost stop` + 删除数据目录会一并清空；不影响 traffic DB。
- 负缓存条目上限 1024：在极端多样应用场景下可能不足，可提升到 4096；但需要评估内存增量与命中率。
- 单个图标 2 MiB 上限：覆盖 macOS 常见系统 App，超大 icns（罕见）会被拒绝并入负缓存；产品接受该行为。
- 平台差异：Windows/Linux 图标查找不走 helper 子进程，仅共享磁盘/负缓存策略；如果后续 Windows 也出现类似 footprint 问题，可复用相同 helper 模式。
- 打包边界：不新增分发产物；`app-icon-worker` 由同一 `bifrost` 二进制承担，release 打包脚本无需改动。
- 安全边界：helper 子进程仅接收父进程传入的 `<app-path>`，输出到父进程创建的临时文件；不接收网络输入，不解析用户 payload。
