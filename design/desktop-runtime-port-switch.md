# Desktop Runtime Port Switch

## 功能模块描述

桌面端修改代理端口时，当前实现会优先尝试让内嵌 core 在运行时重绑监听端口，而不是默认走整进程 `stop -> start`。如果后端没有确认热切成功，桌面壳层才回退到重启 sidecar。

这意味着当前真实语义是：

- 首选热切 listener
- 失败时由桌面端兜底重启 core
- Web UI 保持当前窗口与路由，只刷新后端连接和实时订阅

## 当前实现

### 1. 端口切换入口

- Tauri 命令入口是 `desktop/src-tauri/src/main.rs` 的 `update_desktop_proxy_port`。
- 它会先读取当前桌面 runtime 里的 `expected_port` / `port`。
- 若目标端口与当前期望端口一致，直接返回当前 runtime 信息，不重复触发切换。

### 2. 优先走后端运行时重绑

- 桌面端会调用当前 core 的：
  - `PUT /_bifrost/api/config/server`
- 请求体为：

```json
{ "port": 9901 }
```

- 后端若支持运行时重绑，会返回可解析的端口切换结果；桌面端据此更新：
  - `desktop-config.json` 中保存的期望端口
  - Tauri runtime 中的 `expected_port`
  - 当前实际端口 `port`

### 3. 回退路径仍存在

- 如果后端返回的是普通 server config（说明该 core 还不支持运行时 rebind），则会进入 `RestartRequired` 分支。
- 如果响应既不是端口切换结果也不是普通 server config，桌面端会再用 `wait_for_rebound_backend_port` 在目标端口及其顺延端口上做最多约 2s 的健康探测；探测成功仍按 `Rebound` 处理，失败才返回错误。
- `RestartRequired` 分支下桌面壳层会：
  - 调用 `terminate_child` 停掉当前 managed child
  - 调用 `stop_backend_with_binary`，即 `<embedded core binary> stop`（带 `BIFROST_DATA_DIR`，等价于直接 `bifrost stop`）
  - 通过 `wait_for_backend_shutdown` 等待旧端口断开（约 3s）
  - 通过 `launch_backend_on_available_port` 按首选端口重新拉起内嵌 core
  - 若目标端口不可用，继续沿用桌面端现有的 `MAX_PORT_INCREMENT_ATTEMPTS = 64` 端口顺延策略

因此它不是“纯热切、绝不重启”的实现，而是“热切优先，重启兜底”。

### 4. 前端连接恢复

- `web/src/pages/Settings/tabs/ProxyTab.tsx` 的说明文案已经改成 rebind 语义。
- 前端在收到桌面 runtime 返回后会：
  - 更新期望端口和实际端口
  - 轮询新端口健康状态
  - 刷新 overview / proxy 状态
  - 断开并重连 push 通道
- `web/src/desktop/tauri.ts` 还会缓存最近一次可用的 `invoke` 与窗口句柄，降低切换窗口期 `window.__TAURI__` 短暂缺失带来的误报概率。

### 5. 当前仍保留的旧文案

- Settings 卡片标题和说明已经体现 “rebind”。
- 操作按钮文本仍然是 `Apply & Restart`，属于尚未完全收敛的 UI 文案，不代表主路径一定会重启进程。

## 依赖项

- `desktop/src-tauri/src/main.rs`
- `web/src/desktop/tauri.ts`
- `web/src/runtime.ts`
- `web/src/stores/useDesktopCoreStore.ts`
- `web/src/pages/Settings/tabs/ProxyTab.tsx`
- `crates/bifrost-admin/src/handlers/config.rs`
- `crates/bifrost-admin/src/port_rebind.rs`
- `crates/bifrost-proxy/src/server.rs`

## 测试方案

1. 桌面端启动后确认当前端口可正常访问 admin 与代理功能。
2. 在 Settings 页改成新端口，确认桌面窗口不会整体退出。
3. 观察端口切换完成后：
   - `expectedProxyPort` 更新为用户输入值
   - `proxyPort` 更新为实际监听值
   - overview / push / system proxy / cli proxy 请求都命中新端口
4. 构造目标端口占用场景，确认仍能回退到下一个可用端口。
5. 构造后端无法确认 rebind 的场景，确认桌面端会进入重启兜底路径而不是静默失败。

## 校验要求

- 先执行相关桌面端端到端验证
- 再执行 `rust-project-validate` 要求的格式、lint、测试和构建校验

## 文档更新要求

- `crates/bifrost-admin/ADMIN_API.md` 需以 `PUT /_bifrost/api/config/server` 的当前语义为准
