# App 图标内存优化方案

## 背景

管理端 Traffic 列表会通过 `/_bifrost/api/app-icon/:app` 获取应用图标。当前 macOS 图标提取链路优先调用 `NSWorkspace::iconForFile`，这个路径会在 Bifrost 主进程内加载 Foundation、ImageIO、QuartzCore、Metal、IconServices 等系统框架与缓存。真实观察中，连续请求多个应用图标后，主进程 physical footprint 可从约 80 MiB 上升到 400 MiB 以上，甚至超过 1 GiB。

这类内存并不来自流量 body store 或 WebSocket payload 缓存，而是 AppKit 图标提取在长生命周期主进程内留下的系统框架与图像缓存。优化目标是保留 App 图标功能，同时把高风险图标提取成本移出主进程，并限制主进程内图标缓存的常驻体积。

## 实现方案

### 1. 主进程优先走纯文件解析路径

macOS 上先定位 `.app` bundle，再从 `Contents/Info.plist` 找到 `CFBundleIconFile` 或 `CFBundleIconName`，读取 `Contents/Resources/*.icns` 并用 Rust `icns` crate 转成 PNG。该路径不需要 AppKit，不会把 NSWorkspace/Metal 图像栈加载进主进程。

如果 `.icns` 文件不存在、Info.plist 未声明图标、或 `.icns` 无法解析，再进入隔离 helper fallback。

### 2. AppKit fallback 放入同一个二进制的短生命周期 helper

不新增单独分发产物。CLI 增加隐藏内部命令 `app-icon-worker`，由当前 `bifrost` 可执行文件 re-exec 启动：

```text
bifrost app-icon-worker --path <app-path> --output <temp-png-path>
```

父进程创建临时 PNG 输出文件，设置超时并校验输出大小。子进程内可以调用 `NSWorkspace`，结束后 AppKit/Metal/Foundation 的常驻成本随子进程退出释放，不污染主进程 footprint。使用临时文件而不是 stdout 可以避免大 PNG 写入时被 pipe buffer 阻塞。

helper 命令隐藏在 clap help 之外，不改变用户公开 CLI 面，也不影响现有打包分发流程。

### 3. 路径归一与过滤

从 traffic DB 取到的 client path 可能是：

- `.app/Contents/MacOS/...` 内部可执行文件
- `/System/Applications/*.app`
- `target/debug/deps/...`、测试二进制、helper、临时进程
- `.xpc`、`.appex` 或没有用户可感知 App bundle 的系统组件

优化后先把路径归一到最外层 `.app` bundle；没有 `.app` bundle 的路径不走 macOS AppKit 提取。这样避免 UI 中某些短生命周期进程、测试二进制或系统 helper 触发昂贵的图标提取。

### 4. 主进程不再常驻正向图标，仅缓存负查询

原缓存把每个 app 的完整 `Vec<u8>` PNG 都按条目数裁剪后挂在主进程堆上，是 footprint 增长的另一个来源。优化后主进程内不再常驻任何正向图标字节：

- 正向图标只写入磁盘缓存 `<data_dir>/app_info/*.png`，命中时直接从文件读出返回，热图标常驻成本落在可回收的 OS page cache，不计入进程 footprint。
- 主进程仅保留一个负缓存 (`AppIconNegativeCache`)，记录已知抓取失败的 app key，避免重复触发昂贵的 fallback。负缓存按 FIFO 驱逐，容量上限 `NEGATIVE_CACHE_MAX_ENTRIES = 1024`。
- 单个图标字节数有 2 MiB 上限 (`MEMORY_CACHE_MAX_SINGLE_ICON_BYTES`)，超过的 worker 输出会被拒绝、不落盘也不返回。

### 5. 前端懒加载与并发限流

`AppIcon` 组件只在可见后请求图标，且全局限制同时请求数。列表快速滚动或大量记录渲染时，不会一次性打满后端图标提取队列。

### 6. 兼容性边界

- 已有磁盘缓存命中时仍直接返回 PNG。
- 标准 `.app` 图标功能保持可用。
- 非 macOS 平台继续使用原有 Windows/Linux 图标查找逻辑，仅共享内存缓存封顶。
- 不新增打包产物，不修改 release artifact 列表。

## 测试方案

### 单元测试

- 路径归一：`.app/Contents/MacOS/bin` 归一到 `.app` bundle。
- 路径过滤：`target/debug/deps/foo` 和无 `.app` ancestor 的路径不触发 macOS 图标提取。
- 负缓存：达到 `NEGATIVE_CACHE_MAX_ENTRIES` 后按 FIFO 驱逐最旧 key，`remove` 可清除已记录的 miss。
- handler：已有缓存命中仍返回 `image/png` 与 CORS 头。

### E2E 测试

新增 app icon memory guard 脚本：

- 使用临时 `BIFROST_DATA_DIR` 和 `--no-system-proxy` 启动 Bifrost。
- 请求多个真实系统 App 图标。
- 验证请求成功或合理返回 404。
- macOS 上采集主进程 footprint/RSS，断言多图标请求后主进程不出现旧链路那种数百 MiB 级增长。

### human_tests

新增 `human_tests/app-icon-memory-optimization.md`：

- 真实启动临时 Bifrost。
- 调用 `/_bifrost/api/app-icon/*` 请求多个系统 App 图标。
- 通过 `footprint`/`ps` 验证主进程内存保持在合理范围。
- 确认系统代理不被测试服务修改。

## Review/Fix/Test 闭环

第 1 轮：

- 对照用户目标复核：内存根因、helper 隔离、缓存封顶、前端限流、分发兼容。
- review `bifrost-admin`、`bifrost-cli`、`web`、`human_tests` 和 E2E 变更。
- 运行 `cargo test -p bifrost-admin app_icon` 与前端静态检查。

第 2 轮：

- 复查第 1 轮修复后的 diff 和真实 human_tests 输出。
- 复跑受影响测试与 app icon E2E。
- 确认不需要新增分发产物且 release 打包流程无改动。

## 校验要求

- `cargo fmt --all`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test -p bifrost-admin app_icon`
- app icon E2E 脚本
- `cargo test --workspace --all-features`
- `cargo build --workspace --all-features`
- 覆盖率门禁优先执行 `make coverage`

## 文档更新

- 新增本设计文档。
- 新增 `human_tests/app-icon-memory-optimization.md`。
- 更新 `human_tests/readme.md` 索引。
