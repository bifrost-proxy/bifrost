# Tray 系统状态展示

## 功能模块详细描述

Tray 菜单新增全系统状态展示能力，默认开启，在原生托盘菜单顶部展示两排只读信息：

- `System: CPU <percent> | Memory <used> / <total>`
- `Network: Up <rate> | Down <rate>`

这些指标表示整台机器的状态，不表示 Bifrost 进程自身资源消耗，也不表示 Bifrost 代理流量聚合。Settings 新增独立 `Tray` tab，用于管理原生 Tray helper 与系统状态展示开关。

## 实现逻辑

### 配置模型

在 `bifrost-storage::TrayConfig` 中增加：

- `enabled: bool`：是否启用 tray helper，沿用既有语义。
- `show_system_stats: bool`：是否在 tray 菜单展示系统状态，默认 `true`。

Admin API `/api/config/tray` 返回并更新 `show_system_stats`；`PUT` 支持只更新 `enabled` 或只更新 `show_system_stats`，避免 Settings 中两个开关互相覆盖。

### Tray helper 本地采样

系统状态由 `bifrost __tray` helper 本地采样，不经过 Admin API：

- Tray helper 在独立进程运行，即使 Bifrost 主服务繁忙、停止或 Admin API 无响应，也可以继续更新机器状态。
- 采样线程固定低频刷新，当前为 3 秒。
- 使用同一个 `sysinfo::System` 与 `sysinfo::Networks` 实例循环刷新，避免 CPU 与网络差值类指标在首帧或重建实例时失真。
- 网络速率汇总非 loopback 接口的 `received()` 与 `transmitted()` 差值，转换为 bytes/sec 后格式化。

### 性能与实时性权衡

刷新频率不做成用户可见配置，默认固定为 3 秒。理由：

- 托盘菜单是按需查看的信息面板，不是监控曲线；3 秒内的数据新鲜度足够判断当前系统是否繁忙。
- 采样只在 tray helper 本地执行，不访问 Admin API，不阻塞主代理进程。
- 当 `show_system_stats=false` 时，线程只低频读取配置并清空菜单状态，不执行 `sysinfo` CPU/内存/网络采样。
- Mac 本地 release microbench 使用 `sysinfo 0.31.4` 连续执行 500 次采样，单次平均 1.1796ms、最大 1.8934ms；按采样耗时估算，1s/2s/3s 刷新分别约为 0.1180% / 0.0590% / 0.0393% 单核 CPU。
- Mac debug tray helper 真实进程采样 20 秒：3 秒刷新开启系统状态时平均 CPU 0.4650%、RSS 64,332KB；关闭系统状态时平均 CPU 0.1300%、RSS 63,514KB。增量约 0.335% CPU 与 818KB RSS，处在可接受范围。

若后续真实用户机器上看到明显 CPU 增高，优先把固定刷新间隔调高到 5 秒；只有存在明确用户需求时再增加 Settings 中的刷新频率选项。

### 菜单更新

系统状态开启时菜单结构固定包含 `_system_stats` 与 `_network_stats` 两个 disabled item；初始显示 collecting 文案，后续只更新 item 文本。

现有 `NativeMenuState::refresh_in_place` 会对同结构菜单调用原生 item `set_text`，不会每 3 秒替换整个 native menu，避免 macOS/Windows 上展开菜单时被后台刷新关闭。

## 依赖项

- `bifrost-cli` 在非 Linux tray 构建下新增 `sysinfo = "0.31"`，与 `bifrost-admin` 已使用版本一致。
- 不新增 Tauri tray 依赖；仍复用现有 `tao` + `tray-icon` helper。

## Windows / macOS 兼容调研

- `tray-icon` 的 `MenuItem`/`CheckMenuItem` 支持 `set_text`，满足高频文本原地更新需求。
- `sysinfo 0.31.4` 支持 Windows 与 macOS 的 CPU、内存和网络接口统计；CPU 使用率与网络速率都依赖同一实例的连续刷新。
- 如果后续 Windows VM 发现 `sysinfo::Networks` 对某些虚拟网卡漏报，fallback 方案是 Windows `GetIfTable2Ex` 汇总 `MIB_IF_ROW2` 字节计数。
- 如果后续 macOS 发现内存或 CPU 数值与系统 Activity Monitor 口径偏差不可接受，fallback 方案是 macOS `host_statistics64` / `sysctl` 平台封装。

## 测试方案

### 单元测试

- `bifrost-storage`：`TrayConfig::default()` 默认 `enabled=true`、`show_system_stats=true`，序列化 round-trip 保留字段。
- `bifrost-admin`：`GET /api/config/tray` 与 `GET /api/config` 返回 `show_system_stats`；`PUT /api/config/tray` 支持仅更新系统状态开关。
- `bifrost-cli`：
  - 系统状态格式化输出稳定单位。
  - Tray 菜单开启系统状态时展示两排 disabled item。
  - 系统状态关闭时隐藏两排 item。
  - 同结构状态文本更新不改变 native menu shape。
- `web`：Settings Tray tab 展示两个开关，并通过 API 保存/回读。

### E2E 测试

- 新增 shell E2E 覆盖 `/api/config/tray` 默认值、关闭/开启 `show_system_stats`、关闭/开启 `enabled` 后 config 持久化。
- 新增/更新 Playwright 覆盖 `Settings -> Tray` tab、两个开关可见与保存。
- Tray helper smoke 继续使用 `test_cli_tray_startup_ci.sh`；新增系统状态日志或菜单结构断言时必须避免要求无头 Windows runner 长驻原生 tray。

### 真实场景测试

更新 `human_tests/cli-tray-helper.md`：

- macOS：真实启动 tray helper，验证默认开启、Settings Tray tab 开关、Admin API 持久化和性能基线；原生菜单两排结构由跨平台 menu 单测断言。
- Windows VM：在隔离 worktree 中构建并真实启动 tray helper，验证默认开启、Admin API 持久化和 tray helper 日志；原生 notification area 文本读取若受交互会话或辅助功能限制，不作为唯一验收依据。

## Review/Fix/Test 闭环方案

第 1 轮：

- 复核用户目标：默认开启、两排展示、全系统指标、Settings 独立 Tray tab、macOS/Windows 兼容。
- Review 修改文件：配置/API、tray helper、Settings UI、测试与文档。
- 运行最小测试：相关 Rust 单元测试、web UI 单测/Playwright、shell E2E。

第 2 轮：

- 复查第 1 轮修复后的 diff，确认没有把 Bifrost 进程指标误用为系统指标。
- 复跑受影响测试，执行 human_tests，补齐 coverage 门禁。

## 校验要求

- 必须按仓库规则执行 human_tests。
- 必须执行 `make coverage`；若 E2E coverage 环境不可用，执行 `make coverage-unit` 并说明原因。
- 收尾前按 `rust-project-validate` 执行格式、clippy、build、`cargo test --workspace --all-features`。
- 默认提交、推送、创建/更新 PR，并使用 GitHub Actions PAT fail-fast 看护 CI。
