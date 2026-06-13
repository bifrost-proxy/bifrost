# App Icon 内存优化真实场景测试

## 功能模块说明

验证管理端 App Icon API 在 macOS 上请求多个真实应用图标时，不会把 `NSWorkspace` / AppKit / ImageIO / Metal 等高成本图标提取栈常驻加载到 Bifrost 主进程，避免主进程内存从约 80 MiB 膨胀到数百 MiB 或更高。

## 前置条件

- 运行平台为 macOS。
- 当前仓库已构建可执行文件 `target/debug/bifrost`，或允许测试脚本自动执行：

```bash
SKIP_FRONTEND_BUILD=1 cargo build --bin bifrost
```

- 测试必须使用临时数据目录，并启动时显式禁用系统代理和 Sync 自动登录弹窗：

```bash
BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1
BIFROST_DATA_DIR="$(mktemp -d)"
bifrost start --skip-cert-check --unsafe-ssl --no-system-proxy -y
```

## 测试用例列表

### TC-AIMO-01 多 App 图标请求不显著抬高主进程 RSS

操作步骤：

1. 在仓库根目录执行：

```bash
e2e-tests/tests/test_app_icon_memory_guard.sh
```

2. 脚本会自动分配本地端口，使用临时 `BIFROST_DATA_DIR` 启动 Bifrost。
3. 脚本会请求以下真实系统 App 图标：

```text
Calculator, Calendar, Contacts, FaceTime, Freeform, Maps, Photos, Preview, Reminders, Shortcuts, Stickies, TextEdit
```

4. 脚本会读取请求前后的 Bifrost 主进程 RSS。

预期结果：

- 至少 3 个真实 App 图标返回 HTTP 200。
- 其他 App 如在当前系统不存在，可以返回 HTTP 404。
- 不出现非 200/404 的异常状态码。
- 多图标请求后主进程 RSS 增量小于 150000 KB。
- 测试服务使用 `--no-system-proxy`，不会修改系统代理。

本次实际结果：

- 2026-06-13 已执行通过。
- 请求成功图标数：12。
- 请求前 RSS：79456 KB。
- 请求后 RSS：86736 KB。
- RSS 增量：7280 KB。

### TC-AIMO-02 非 App bundle 进程路径不触发 macOS 图标提取

操作步骤：

1. 在仓库根目录执行：

```bash
cargo test -p bifrost-admin app_icon::tests::macos_icon_path_rejects_non_app_process_paths
```

2. 测试输入模拟 `target/debug/deps/some-test-binary` 这类非用户 App bundle 路径。

预期结果：

- 路径解析返回 `None`。
- 该路径不会进入 macOS AppKit 图标提取 fallback。
- 测试通过。

本次实际结果：

- 2026-06-13 已通过 `cargo test -p bifrost-admin app_icon::tests::` 覆盖。

### TC-AIMO-03 图标内存缓存按字节驱逐

操作步骤：

1. 在仓库根目录执行：

```bash
cargo test -p bifrost-admin app_icon::tests::memory_cache_evicts_by_total_bytes
```

2. 测试构造低字节上限的内存缓存，依次写入多个图标。

预期结果：

- 超出总字节上限时驱逐旧图标。
- 当前缓存字节数保持在上限内。
- 测试通过。

本次实际结果：

- 2026-06-13 已通过 `cargo test -p bifrost-admin app_icon::tests::` 覆盖。

## 清理步骤

- `test_app_icon_memory_guard.sh` 会在退出时停止测试 Bifrost 进程，清理临时 `BIFROST_DATA_DIR`，并释放测试端口。
- 如脚本被强制中断，可执行：

```bash
BIFROST_DATA_DIR=<测试临时目录> target/debug/bifrost stop
```
