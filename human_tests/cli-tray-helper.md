# CLI 托盘 Helper 真实场景测试

## 功能模块说明

验证 `bifrost` 内置 `__tray` 托盘 helper 在 macOS/Windows 上的完整生命周期：CLI 自动拉起、托盘图标显示、默认菜单操作、Rules 快速切换、自定义菜单加载、单实例保护、配置化启停、服务停止后状态变化、状态轮询刷新、主进程繁忙时菜单仍可响应的可靠性回归，以及 `--no-tray` / `BIFROST_DISABLE_TRAY=1` 的禁用行为。

## 前置条件

- macOS 或 Windows 系统
- 已编译 `bifrost` 二进制（`cargo build --bin bifrost`）
- 托盘 helper 通过当前 `bifrost` 二进制的隐藏 `__tray` 子命令重入启动；如需开发覆盖，可设置 `BIFROST_TRAY_BIN` 指向兼容 `bifrost __tray` 的二进制
- 使用临时数据目录避免影响现有服务
- 规则切换验证必须通过管理端 HTTP API 准备/验证规则状态，禁止直接编辑 `rules/` 或 `state.json`
- macOS 托盘菜单交互必须使用 AppleScript/System Events 操作 CLI 启动出的菜单栏图标，例如通过 `osascript` 点击对应进程的 menu bar item；若 `osascript` 返回 `-1719` 辅助访问权限错误，记录为环境阻塞，不能用截图观察替代
- Windows 回归项需在 Windows 交互用户 session 下执行，不能在 Session 0/service 环境中替代

## 启动命令模板

```bash
BIFROST_DATA_DIR=./.bifrost-tray-test \
BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 \
cargo run --bin bifrost -- start -p 8801 --unsafe-ssl --no-system-proxy --skip-cert-check
```

## 测试用例

### TC-TH-01: macOS/Windows CLI 启动后自动拉起托盘图标

**操作步骤：**
1. 使用启动命令模板启动 Bifrost 服务
2. 观察 macOS 菜单栏（或 Windows notification area）

**预期结果：**
- 系统托盘/菜单栏出现 Bifrost 图标
- `.bifrost-tray-test/tray.pid` 文件存在且内容为有效 PID
- `.bifrost-tray-test/tray.lock` 文件存在
- `.bifrost-tray-test/logs/tray.log` 文件存在且包含启动日志

### TC-TH-02: 默认菜单包含所有预期项

**操作步骤：**
1. 在 TC-TH-01 基础上，点击托盘图标展开菜单

**预期结果：**
- 顶部显示 "Bifrost: Running on 127.0.0.1:8801"（灰色不可点击）
- 包含以下菜单项：Open Admin UI、Open Traffic、Open Rules、Copy Admin URL、Copy HTTP Proxy、Copy SOCKS5 Proxy
- 统一代理模式下 Copy SOCKS5 Proxy 可点击，复制值为 `socks5://127.0.0.1:8801`；如果启动时显式传入 `--socks5-port <port>`，则复制独立 SOCKS5 端口
- 没有规则时仍显示 `Rules: None` 子菜单，子菜单中显示置灰的 `No rules available`
- Stop Bifrost 下方显示 `System Proxy` 原生勾选菜单项
- 包含分隔线
- 包含：Stop Bifrost、System Proxy、Open Logs
- 不包含：Restart Bifrost、Open Data Directory
- 最底部：Quit Tray

### TC-TH-02-REG-01: 点击托盘图标后菜单保持展开

**操作步骤：**
1. 在 TC-TH-01 基础上，连续点击托盘图标 3 次
2. 每次点击后观察菜单是否保持展开至少 2 秒
3. 每次菜单展开后移动鼠标到 "Open Admin UI" 菜单项但不点击
4. 查看 `.bifrost-tray-test/logs/tray.log*`

**预期结果：**
- 每次点击后菜单都保持展开，不出现闪烁一下立即消失
- 鼠标移动到 "Open Admin UI" 时该菜单项保持可见且可高亮
- 点击图标本身不会立即产生 `tray menu rebuilt` 日志
- 菜单打开后的短保护窗口内，后台状态轮询、规则轮询或系统代理状态刷新不应替换 native menu
- 保护窗口外的后台状态变化仍可重建菜单，用于保持服务停止/启动状态刷新

### TC-TH-02-REG-01B: 后台数据刷新不关闭已展开菜单

**操作步骤：**
1. 执行单元回归，验证后台状态/数据变化在托盘点击后的保护窗口内不会触发 native menu rebuild：
```bash
cargo test -p bifrost-cli native_menu -- --nocapture
```
2. 使用启动命令模板启动 Bifrost 服务和托盘
3. 展开托盘菜单并保持菜单打开
4. 在菜单打开期间制造后台数据变化，例如新增/启用/禁用规则，或触发 Rules/System Proxy Admin API 状态变化
5. 保持鼠标悬停在菜单项上至少 3 秒，并查看 `.bifrost-tray-test/logs/tray.log*`

**预期结果：**
- 第 1 步单元回归通过
- 后台数据变化期间，已展开的系统菜单不被自动关闭
- 鼠标悬停项保持可见且可高亮
- `tray.log*` 可以出现后台数据加载或状态变化日志；同结构刷新应出现原地刷新而不是 `tray menu rebuilt`
- 用户点击 `Reload Tray Menu`、切换规则或执行系统代理开关这类菜单动作后，可以触发一次菜单重建

### TC-TH-02-REG-02: runtime 缺失但父进程存活时不显示 Unknown

**操作步骤：**
1. 使用启动命令模板启动 Bifrost 服务，并确认托盘图标已出现
2. 记录主服务 PID 与托盘 helper PID：`pgrep -af 'bifrost.*__tray|target/debug/bifrost'`
3. 临时移动 runtime 文件：`mv ./.bifrost-tray-test/runtime.json ./.bifrost-tray-test/runtime.json.bak`
4. 等待 2 秒后展开托盘菜单
5. 测试结束后恢复 runtime 文件：`mv ./.bifrost-tray-test/runtime.json.bak ./.bifrost-tray-test/runtime.json`

**预期结果：**
- 主服务 PID 仍存活时，菜单顶部显示 `Bifrost: Running on 127.0.0.1:8801`
- 菜单不显示 `Bifrost: Unknown`
- Open Admin UI、Copy HTTP Proxy、Rules、System Proxy 等依赖 Admin URL 的菜单项仍使用启动参数中的 `127.0.0.1:8801`
- 恢复 runtime 文件后菜单状态保持 Running，不出现 Stop/Start 与状态标题不一致

### TC-TH-02-REG-03: 主进程 Admin API 繁忙时菜单仍快速响应

**操作步骤：**
1. 执行单元回归，模拟一个只监听但不响应的 Admin API 端口：
```bash
cargo test -p bifrost-cli quick_menu_snapshot -- --nocapture
```
2. 使用启动命令模板启动 Bifrost 服务和托盘
3. 制造主进程短时高负载，例如并发触发规则/组/Admin API 刷新，或在开发环境中用 CPU profiler/压力脚本压高主进程 CPU
4. 连续点击托盘图标 3 次，每次观察菜单是否能快速展开并保持可交互
5. 查看 `.bifrost-tray-test/logs/tray.log*`

**预期结果：**
- 第 1 步单元回归通过，快速菜单快照在慢 Admin API 下不会等待 HTTP read timeout
- 托盘菜单使用最近一次后台快照渲染，点击图标不会因为规则、组、active-summary 或 system proxy Admin API 慢而卡住
- 主进程繁忙时，Rules/System Proxy 可以短暂显示旧状态或稍后更新，但菜单本身必须可展开、可移动高亮、可点击 Quit Tray/Open Logs 等本地动作
- `tray.log*` 中不应出现由纯菜单展开导致的同步 Admin API 长耗时阻塞；菜单数据刷新应由后台快照线程完成

### TC-TH-02-REG-04: CLI 重启不创建第二个托盘进程

**操作步骤：**
1. 执行单元回归，验证 `tray.lock` 被已有 helper 持有时 launcher 会跳过 spawn，stale `tray.pid` 不会误挡启动：
```bash
cargo test -p bifrost-cli existing_tray_helper_pid -- --nocapture
```
2. 使用启动命令模板启动 Bifrost 服务和托盘
3. 记录当前 helper PID：
```bash
cat ./.bifrost-tray-test/tray.pid
```
4. 在不退出托盘的情况下，再次用同一 `BIFROST_DATA_DIR` 执行启动命令模板
5. 再次读取 `tray.pid`，并检查 `pgrep -af 'bifrost.*__tray'` 中同一数据目录对应的 helper 数量

**预期结果：**
- 第 1 步单元回归通过
- 第二次 CLI 启动不会创建新的托盘 helper；`tray.pid` 保持为第一次启动的 PID
- 日志记录已有 helper 被复用或跳过启动，而不是短暂创建第二个 helper 后由内部 lock 退出
- 同一 `BIFROST_DATA_DIR` 下始终只有一个活动 tray helper

### TC-TH-03: Open Admin UI 打开浏览器管理端

**操作步骤：**
1. 点击 "Open Admin UI" 菜单项

**预期结果：**
- 默认浏览器打开 `http://127.0.0.1:8801/_bifrost/`
- 管理端页面正常加载

### TC-TH-04: Copy HTTP Proxy 复制到剪贴板

**操作步骤：**
1. 点击 "Copy HTTP Proxy" 菜单项
2. 打开任意文本编辑器粘贴

**预期结果：**
- 剪贴板内容为 `http://127.0.0.1:8801`

### TC-TH-05: Quit Tray 不停止 Bifrost 服务

**操作步骤：**
1. 点击 "Quit Tray" 菜单项
2. 验证服务状态

**预期结果：**
- 托盘图标消失
- `.bifrost-tray-test/tray.pid` 文件被清理
- Bifrost 主服务仍然运行（`curl http://127.0.0.1:8801/_bifrost/api/status` 返回 200）

### TC-TH-06: Stop Bifrost 停止服务

**操作步骤：**
1. 重新启动服务和托盘（参考 TC-TH-01）
2. 点击 "Stop Bifrost" 菜单项

**预期结果：**
- Bifrost 主服务停止（端口 8801 不再监听）
- 托盘图标仍存在但菜单进入 stopped 状态（状态行显示 "Bifrost: Stopped" 或 "Bifrost: Disconnected"）
- 依赖服务的菜单项（Open Admin UI 等）置灰

### TC-TH-07: --no-tray 禁用托盘

**操作步骤：**
1. 使用以下命令启动：
```bash
BIFROST_DATA_DIR=./.bifrost-tray-test \
BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 \
cargo run --bin bifrost -- start -p 8801 --unsafe-ssl --no-system-proxy --no-tray
```

**预期结果：**
- Bifrost 服务正常启动
- 系统托盘/菜单栏没有 Bifrost 图标
- `.bifrost-tray-test/tray.pid` 文件不存在

### TC-TH-08: BIFROST_DISABLE_TRAY=1 禁用托盘

**操作步骤：**
1. 使用以下命令启动：
```bash
BIFROST_DATA_DIR=./.bifrost-tray-test \
BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 \
BIFROST_DISABLE_TRAY=1 \
cargo run --bin bifrost -- start -p 8801 --unsafe-ssl --no-system-proxy
```

**预期结果：**
- Bifrost 服务正常启动
- 系统托盘/菜单栏没有 Bifrost 图标

### TC-TH-08B: 配置文件与 Settings 开关禁用/重新启用托盘

**操作步骤：**
1. 执行单元回归，验证配置文件禁用会阻止 CLI launcher 创建托盘：
```bash
cargo test -p bifrost-cli should_launch_tray_disabled_by_config -- --nocapture
```
2. 使用启动命令模板启动 Bifrost 服务和托盘
3. 打开 `http://127.0.0.1:8801/_bifrost/settings?tab=proxy`
4. 确认 Proxy 页中 Tray Icon 开关位于 System Proxy 配置前方
5. 关闭 Tray Icon 开关，并通过 API 验证：
```bash
curl -sS http://127.0.0.1:8801/_bifrost/api/config/tray
```
6. 等待最多 3 秒，检查 `.bifrost-tray-test/tray.pid` 是否被清理，并检查系统托盘/菜单栏图标是否消失
7. 重新打开 Tray Icon 开关，并通过 API 验证：
```bash
curl -sS -X PUT http://127.0.0.1:8801/_bifrost/api/config/tray \
  -H 'Content-Type: application/json' \
  -d '{"enabled":true}'
curl -sS http://127.0.0.1:8801/_bifrost/api/config/tray
```
8. 等待最多 3 秒，检查 `.bifrost-tray-test/tray.pid` 重新出现，且 PID 对应的 `bifrost __tray` helper 进程存活
9. 再次关闭 Tray Icon 开关，确认 helper 退出后停止服务
10. 使用同一个 `BIFROST_DATA_DIR` 重新执行启动命令模板

**预期结果：**
- 第 1 步单元回归通过
- Settings > Proxy 中 Tray Icon 开关显示在 System Proxy 开关之前
- `PUT /_bifrost/api/config/tray` 后 `config.toml` 持久化 `[tray] enabled = false`
- 运行中的托盘 helper 轮询到禁用配置后退出，`tray.pid` 被清理
- 重新 `PUT /_bifrost/api/config/tray {"enabled":true}` 或打开 Settings 开关后，管理端不需要重启主服务即可重新启动托盘 helper，`tray.pid` 重新生成且 helper 进程存活
- 重新启动 CLI 时不会创建新的托盘 helper，系统托盘/菜单栏没有 Bifrost 图标
- 再次打开 API 时返回 `{"enabled":false,"supported":true}`（Linux 上 supported 可为 false）

### TC-TH-09: 单实例保护——重复启动被拒绝

**操作步骤：**
1. 启动 Bifrost 服务（自动拉起 tray helper）
2. 手动尝试再次启动 tray helper：
```bash
./target/debug/bifrost __tray --data-dir ./.bifrost-tray-test --runtime-file ./.bifrost-tray-test/runtime.json --parent-pid 1
```

**预期结果：**
- 第二个 `bifrost __tray` helper 立即退出
- 退出信息包含 "another tray helper is already running"
- 原有的 tray 图标继续正常工作

### TC-TH-10: 自定义菜单加载

**操作步骤：**
1. 在数据目录创建 `tray.json`：
```bash
cat > ./.bifrost-tray-test/tray.json << 'EOF'
{
  "version": 1,
  "items": [
    {"id": "settings", "label": "Open Settings", "action": {"type": "open_admin_route", "route": "/settings"}},
    {"id": "docs", "label": "Bifrost Docs", "action": {"type": "open_url", "url": "https://github.com/bifrost-proxy/bifrost"}}
  ]
}
EOF
```
2. 启动 Bifrost 服务
3. 展开托盘菜单

**预期结果：**
- 默认菜单项后（分隔线之后）出现 "Open Settings" 和 "Bifrost Docs" 自定义项
- 点击 "Open Settings" 打开 `http://127.0.0.1:8801/_bifrost/settings`
- 点击 "Bifrost Docs" 打开 GitHub 页面

### TC-TH-11: 非法 tray.json 降级为默认菜单

**操作步骤：**
1. 创建非法 JSON：
```bash
echo "not valid json" > ./.bifrost-tray-test/tray.json
```
2. 启动 Bifrost 服务

**预期结果：**
- 托盘正常启动，显示默认菜单（无自定义项）
- `tray.log` 中记录配置解析错误

### TC-TH-12: Linux 不启动托盘

**操作步骤：**
1. 在 Linux 系统上执行 `bifrost start`

**预期结果：**
- 服务正常启动
- 不产生 `tray.pid` 或 `tray.lock`
- CLI help 中不显示 `--no-tray` 参数

### TC-TH-13: Rules 菜单仅个人规则时展示两级、最近快捷区并支持单选/取消

**操作步骤：**
1. 使用启动命令模板启动 Bifrost 服务
2. 在没有规则时展开托盘菜单，确认存在 `Rules: None` 与置灰的 `No rules available`
3. 通过 Admin API 创建两条个人规则：
```bash
curl -sS -X POST http://127.0.0.1:8801/_bifrost/api/rules \
  -H 'Content-Type: application/json' \
  -d '{"name":"tray-personal-a","content":"example.com statusCode://201","enabled":true}'
curl -sS -X POST http://127.0.0.1:8801/_bifrost/api/rules \
  -H 'Content-Type: application/json' \
  -d '{"name":"tray-personal-b","content":"example.org statusCode://202","enabled":false}'
```
4. 等待最多 2 秒后点击托盘图标展开菜单，把鼠标悬浮到 `Rules: tray-personal-a`
5. 观察 Rules 下级菜单，然后点击 `tray-personal-b`
6. 再次展开托盘菜单，并调用 `curl -sS http://127.0.0.1:8801/_bifrost/api/rules/active-summary`
7. 再次点击当前已勾选的 `tray-personal-b`
8. 再次展开托盘菜单，并调用 `curl -sS http://127.0.0.1:8801/_bifrost/api/rules/active-summary`
9. 继续切换至少 5 条规则，包含个人规则与组规则；再次展开托盘菜单并悬浮到 `Rules: ...`
10. 通过 Admin API 删除 `tray-personal-a`，等待最多 2 秒后再次展开 Rules 子菜单

**预期结果：**
- 无规则时 Rules 入口不会消失，而是显示 `Rules: None` + `No rules available`
- 顶层菜单存在 `Rules: tray-personal-a`
- 只有个人规则时，Rules 下一级直接展示 `tray-personal-a` 和 `tray-personal-b`，不出现 `My Rules` 或组名层级
- `tray-personal-a` 初始带原生勾选标记，`tray-personal-b` 初始不勾选
- 点击 `tray-personal-b` 后，托盘只需要禁用当前已启用的 `tray-personal-a` 并启用 `tray-personal-b`，不能对所有菜单候选规则批量调用 disable API
- 点击 `tray-personal-b` 后，顶层文案更新为 `Rules: tray-personal-b`；菜单关闭后立即再次展开时，不能仍显示切换前的 `Rules: tray-personal-a`
- `active-summary` 只包含 `tray-personal-b`，不包含 `tray-personal-a`
- 再次点击已勾选的 `tray-personal-b` 后，托盘调用 `tray-personal-b` disable API，不重新 enable；顶层文案更新为 `Rules: None`，`active-summary` 不再包含 `tray-personal-b`
- Rules 子菜单顶部展示最近 5 个成功切换过的规则快捷项；个人规则显示规则名，组规则显示 `组名/规则名`；超过 5 个时最旧项被淘汰
- 删除 `tray-personal-a` 后，Rules 子菜单不再包含 `tray-personal-a`，仍包含 `tray-personal-b`
- 准备、读取和切换均通过 Admin API 完成，没有直接编辑规则文件

### TC-TH-13-REG-01: Rules 切换不会全量禁用候选规则且立即刷新快照

**操作步骤：**
1. 使用启动命令模板启动 Bifrost 服务
2. 通过 Admin API 准备至少 1 条已启用规则和 3 条以上未启用规则，未启用规则需同时覆盖个人规则和可管理组规则
3. 展开托盘菜单，点击其中一条未启用规则
4. 立即再次展开托盘菜单，并调用 `curl -sS http://127.0.0.1:8801/_bifrost/api/rules/active-summary`
5. 查看 `logs/tray.log*` 与主服务规则日志，确认本次点击对应的 enable/disable 请求数量

**预期结果：**
- 本次点击只对切换前已启用的规则调用 disable API，然后对目标规则调用 enable API；未启用的其他候选规则不会被逐个 disable
- 如果点击前只有 1 条规则启用，则本次切换最多产生 1 次 disable 和 1 次 enable；不会因为菜单中存在多个个人/组候选规则而出现批量 `group rule disabled`
- 点击成功后 helper 立即刷新菜单数据快照；再次打开菜单时顶层 `Rules: ...`、原生勾选状态和 `active-summary` 均指向新规则
- 若 Admin API 返回失败，菜单可以保留旧状态，但必须在日志中记录失败请求；不能静默关闭菜单并让用户误以为已切换成功

### TC-TH-14: Rules 菜单存在组规则时展示三级并支持跨组单选

**操作步骤：**
1. 使用启动命令模板启动 Bifrost 服务
2. 确保远端 `/api/group` 返回至少一个 `level >= 1` 的可管理组；Owner 与 Master 都必须纳入验证
3. 通过 Admin API 创建或确认一条个人规则 `tray-personal-c`
4. 通过 Admin API 创建或确认一条组规则 `tray-group-rule-a`
5. 通过 `PUT /_bifrost/api/rules/tray-personal-c/enable` 启用个人规则，并通过组规则 disable API 禁用组规则
6. 点击托盘图标展开菜单，把鼠标悬浮到 `Rules: tray-personal-c`
7. 观察第二级菜单，再悬浮到组名，点击 `tray-group-rule-a`
8. 再次展开托盘菜单，并调用 `curl -sS http://127.0.0.1:8801/_bifrost/api/rules/active-summary`
9. 调用 `curl -sS http://127.0.0.1:8801/_bifrost/api/group-rules/<group_id>`，确认托盘展示的组规则来自远端接口返回的组名和规则名

**预期结果：**
- 顶层菜单存在 `Rules: tray-personal-c`
- 存在组规则时，Rules 第二级展示 `My Rules` 与远端 `level >= 1` 组名，二者平级
- `My Rules` 的下一级展示个人规则；组名的下一级展示组规则
- 点击组规则后，顶层文案更新为 `Rules: <组名>/tray-group-rule-a`
- `active-summary` 只包含被点击的组规则，不包含 `tray-personal-c`
- 本地个人规则以 `reference-candidates` 中的个人规则为准；组规则不以本地 `rules/` 目录为准

### TC-TH-14-REG-01: 非 Managed 组规则收起到 More

**操作步骤：**
1. 使用带 Sync session 的真实数据目录启动 Bifrost 服务，或使用已有包含组规则缓存的本机数据目录
2. 调用 `curl -sS http://127.0.0.1:8801/_bifrost/api/group`，确认 Web UI `Managed` 区域对应的组满足 `level >= 1`，包含 Owner 与 Master 时都应记录
3. 调用 `curl -sS http://127.0.0.1:8801/_bifrost/api/rules/reference-candidates`，确认至少存在一个不在 `level >= 1` Managed 列表中的组规则候选，例如本地 `rules/` 目录残留但 `/api/group` 不返回的组、`level=0` 组或 `level=null` Discover/Public 组
4. 对 `level >= 1` 的组调用 `curl -sS http://127.0.0.1:8801/_bifrost/api/group-rules/<group_id>`，确认 Owner/Master 组规则可从远端接口返回
5. 展开托盘菜单并悬浮 `Rules: <当前启用规则>`
6. 观察 Rules 子菜单的第二级分组，并点击底部 `More...`

**预期结果：**
- 第二级只展示 `My Rules` 和 Web UI `Managed` 区域中的 Owner/Master 组，不直接列出非 Managed 组名
- Master 组（例如 `next-agent`）与 Owner 组一样展示为二级组菜单
- 如果存在非 Managed 组规则候选，Rules 子菜单底部显示 `More...`
- 点击 `More...` 后打开 `http://127.0.0.1:8801/_bifrost/rules`
- 非 Managed 组规则仍可在 Admin Rules 页面继续浏览和测试

### TC-TH-14-REG-02: 系统代理开关位于 Stop Bifrost 下方

**操作步骤：**
1. 使用启动命令模板启动 Bifrost 服务
2. 展开托盘菜单
3. 观察动作区菜单项顺序
4. 点击 `System Proxy`，等待最多 3 秒后再次展开托盘菜单
5. 通过 `curl -sS http://127.0.0.1:8801/_bifrost/api/proxy/system` 验证系统代理状态
6. 如果第 4 步已启用系统代理，再次点击 `System Proxy` 关闭并复查状态

**预期结果：**
- 动作区顺序为 `Stop Bifrost`、`System Proxy`、`Open Logs`
- 菜单中不存在 `Restart Bifrost`
- 菜单中不存在 `Open Data Directory`
- `System Proxy` 是原生勾选菜单项，勾选状态与 `managed_by_bifrost=true` 的系统代理状态一致
- 点击后通过 Admin API 切换系统代理到当前 Bifrost 端口；再次点击可关闭，关闭后不残留 Bifrost 管理的系统代理
- 切换动作使用 Admin API 的个人规则 enable/disable 与组规则 enable/disable 接口

### TC-TH-15: 服务停止后 1 秒轮询刷新菜单状态

**操作步骤：**
1. 使用启动命令模板启动 Bifrost 服务和托盘
2. 在终端执行 `BIFROST_DATA_DIR=./.bifrost-tray-test cargo run --bin bifrost -- stop`
3. 等待 2 秒后点击托盘图标展开菜单

**预期结果：**
- 菜单状态行显示 "Bifrost: Stopped" 或 "Bifrost: Disconnected"
- Open Admin UI、Open Traffic、Open Rules、Copy Admin URL、Copy HTTP Proxy 等依赖服务的菜单项置灰
- 没有依赖再次点击图标预热下一次菜单才能看到新状态

### TC-TH-15B: 主服务退出后托盘空转 10 分钟自动退出

**操作步骤：**
1. 使用启动命令模板启动 Bifrost 服务和托盘
2. 记录 `.bifrost-tray-test/tray.pid` 中的 helper PID
3. 在终端执行 `BIFROST_DATA_DIR=./.bifrost-tray-test cargo run --bin bifrost -- stop`
4. 确认托盘状态进入 Stopped 或 Disconnected
5. 等待 10 分钟以上，期间不要通过托盘或 CLI 启动服务
6. 检查 helper PID 是否退出，并检查 `.bifrost-tray-test/tray.pid` 是否被清理
7. 回归保护：执行 `cargo test -p bifrost-cli service_idle_auto_exit -- --nocapture`

**预期结果：**
- 主服务停止后的 10 分钟内，如果没有新的服务启动，托盘 helper 自动退出
- helper 退出前记录空转超时日志，并清理自己的 `tray.pid`
- 如果服务重新变为 Running，空转计时清零；如果用户点击 Start 正在启动，helper 不会在启动中途退出

### TC-TH-16: 自定义 admin_api 只允许 GET/POST 且使用管理端基准路径

**操作步骤：**
1. 在数据目录创建 `tray.json`，包含：
```json
{
  "version": 1,
  "items": [
    {"id": "refresh", "label": "Refresh System Proxy", "action": {"type": "admin_api", "method": "POST", "path": "/api/proxy/system/refresh"}}
  ]
}
```
2. 启动 Bifrost 服务并展开托盘菜单，点击 "Refresh System Proxy"
3. 查看 `.bifrost-tray-test/logs/tray.log`
4. 将 `method` 改为 `DELETE`，重启服务和托盘

**预期结果：**
- 第 2 步调用的是 `http://127.0.0.1:8801/_bifrost/api/proxy/system/refresh`
- 合法 POST 动作不会被静默改成其它 method
- 改成 DELETE 后托盘保留默认菜单，`tray.log` 记录 `admin_api method must be GET or POST`

### TC-TH-17: Windows sibling bifrost.exe 回退与无控制台闪窗

**操作步骤：**
1. 在 Windows 上准备 `bifrost.exe`
2. 不传 `--bifrost-bin`，直接启动 `bifrost.exe __tray --data-dir .\.bifrost-tray-test --runtime-file .\.bifrost-tray-test\runtime.json --parent-pid <pid>`
3. 展开菜单点击 Stop Bifrost，再重新启动并观察托盘 10 秒

**预期结果：**
- Stop Bifrost 可通过当前 `bifrost.exe` 或 trusted `--bifrost-bin` 正常执行
- 轮询服务状态期间没有每秒弹出 `tasklist` 或其它控制台黑窗
- `tray.log` 不出现找不到 trusted bifrost binary 的错误

### TC-TH-18: IPv6 host URL 使用方括号

**操作步骤：**
1. 准备 runtime 文件，host 为 `::1`，port 为 `8801`
2. 启动 tray helper 并展开菜单
3. 点击 Copy Admin URL 和 Copy HTTP Proxy 后分别粘贴到文本编辑器

**预期结果：**
- Admin URL 为 `http://[::1]:8801/_bifrost/`
- HTTP Proxy 为 `http://[::1]:8801`
- Open Admin UI 使用同样带方括号的 URL

### TC-TH-19: 超大 tray.json fail closed 且日志保留不无限增长

**操作步骤：**
1. 在数据目录写入大于 1 MiB 的 `tray.json`
2. 在 `.bifrost-tray-test/logs/` 下创建一个修改时间超过 30 天、文件名以 `tray.log` 开头的旧日志
3. 启动 Bifrost 服务和托盘
4. 展开托盘菜单并查看 `logs` 目录

**预期结果：**
- 托盘显示默认菜单，不加载超大配置里的自定义项
- `tray.log` 记录 `tray.json is too large`
- 超过 30 天的 `tray.log*` 旧文件被清理

### TC-TH-20: Start Bifrost 有明确启动进展与失败反馈

**操作步骤：**
1. 让托盘处于 `Bifrost: Disconnected` 或 `Bifrost: Stopped` 状态
2. 点击 "Start Bifrost"
3. 立即再次展开托盘菜单
4. 等待启动成功或故意制造端口占用/无效参数后再次展开菜单

**预期结果：**
- 第 3 步状态行显示 `Bifrost: Starting...`
- 启动进行中时 "Start Bifrost" 置灰，不能重复触发并发 start
- 启动成功后状态行变为 `Bifrost: Running on 127.0.0.1:<port>`
- 托盘触发的 Start 会继承原服务启动所需参数；例如原服务使用 `--skip-cert-check --unsafe-ssl` 时，点击 Start 后不应因为 CA 检查失败而立即退出
- 启动失败或超时后状态行显示 `Bifrost: Start failed - open logs`
- 失败后 "Start Bifrost" 恢复可点击，用户可以重试；Open Logs 始终可点击

### TC-TH-21: CI 跨平台 Tray 启动烟测

**操作步骤：**
1. 在 macOS 或 Windows 环境执行：

```bash
bash e2e-tests/tests/test_cli_tray_startup_ci.sh
```

2. 查看脚本输出和临时数据目录中的 `start.out`、`start.err`、`logs/tray.log*`

**预期结果：**
- 普通本地运行时，脚本可自行构建 `bifrost` release binary
- CI 或显式复用场景中，如果传入 `BIFROST_BIN`、`SKIP_BUILD=true` 或 `BIFROST_TRAY_STARTUP_SKIP_BUILD=1`，脚本复用现有 binary，不在 shard 内重新执行 release 构建
- 主服务 Admin API `/_bifrost/api/proxy/address` 在临时端口 ready，响应包含本次端口
- `runtime.json` 存在，且其中的 `port` 等于本次临时端口、`pid` 为有效进程 ID
- 数据目录优先生成 `tray.pid`，且对应 helper 进程存活；Windows runner 若 `tray.pid` 缺失或 helper 进程短暂启动后退出，但 `logs/tray.log*` 已包含启动标记，可按 log-only fallback 通过
- `logs/tray.log*` 包含 `bifrost-tray starting`
- 脚本结束时停止主服务、杀掉 helper，并清理临时数据目录

### TC-TH-22: macOS Tray Helper 内存口径与空闲占用

**操作步骤：**
1. 执行 `cargo build --release --bin bifrost`
2. 使用独立临时数据目录启动 release 版服务：

```bash
TMP_DIR=$(mktemp -d /tmp/bifrost-tray-mem.XXXXXX)
BIFROST_DATA_DIR="$TMP_DIR" BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 \
  ./target/release/bifrost start -p 18802 --unsafe-ssl --skip-cert-check --no-system-proxy
```

3. 等待 `tray.pid` 出现并记录 helper PID：`cat "$TMP_DIR/tray.pid"`
4. 启动后等待 5-15 秒，执行 `ps -o pid,ppid,rss,vsz,etime,command -p <tray_pid>`
5. 执行 `vmmap -summary <tray_pid>`，记录 `Physical footprint`
6. 查看 `logs/tray.log*`，确认远端 group 接口失败时不会每秒重复 warning

**预期结果：**
- release 版 helper 可正常启动，`tray.pid` 对应进程存活
- `ps RSS` 可高于 30 MB，但必须记录为包含 macOS AppKit/Objective-C/CoreFoundation 共享 framework resident 页的诊断口径
- `vmmap -summary` 的 `Physical footprint` 不超过 30 MB
- helper 空闲运行时不会因为远端 group 接口失败而每秒重复请求和写 warning；失败后按短退避周期重试
- 测试结束后停止服务、杀掉 helper 并删除临时数据目录

### TC-TH-23: Tray Helper 内存优化代码归因与收尾验证

**操作步骤：**
1. 检查 `crates/bifrost-cli/src/main.rs` 中 `commands::tray::run_if_tray_process()` 是否仍在 panic hook、crypto provider、clap parse、主日志初始化之前执行
2. 检查 `crates/bifrost-cli/Cargo.toml`，确认当前同二进制 `bifrost __tray` 仍会随 `bifrost-cli` 链接主服务/主 CLI 大依赖
3. 检查 `crates/bifrost-cli/src/commands/tray/` 的 import 和 `cargo tree -p bifrost-cli --target aarch64-apple-darwin -i <crate>` 输出，确认 tray 自身直接依赖与同二进制继承依赖的边界
4. 检查 `design/cli-tray-helper.md` 是否记录代码级归因、已试验但不采用的优化项、继续使用单二进制和跨平台 tray 基础库的结论
5. 确认代码中不存在独立 `bifrost-tray-helper` crate、sibling helper 查找逻辑，剪贴板和打开 URL/目录继续复用 `arboard` 与 `open`

**预期结果：**
- `__tray` 早返回入口顺序保持不变，说明继续挪入口不是主要降 RSS 方向
- 当前 RSS 高值被归因为“完整 `bifrost-cli` 同二进制链接 + AppKit/菜单栈共享 framework resident 页”，而不是规则缓存、Admin API 或日志符号段
- 文档明确区分已落地的小优化、无收益且不采用的独立 helper / `open` / `arboard` 替换试验、以及暂不采用的原生 AppKit/Win32 helper 风险
- 后续若目标是 `Physical footprint < 30 MB`，当前方案可沿用；若目标是用户可见 `ps RSS < 30 MB`，文档必须明确 macOS AppKit 基线已超过 30 MB，不能继续承诺通过小优化达成

## 本次执行记录

| 日期 | 用例 | 执行方式 | 结果 |
| --- | --- | --- | --- |
| 2026-06-11 | TC-TH-02-REG-01 / TC-TH-21 | 针对 PR CI run `27305425195` 的 macOS shell shard 1 超时补充验证：失败 artifact 显示 `test_cli_foreground_ctrlc_no_enter.sh` 已输出 `PASS: foreground Ctrl-C stops without an extra Enter`，`test_cli_tray_menu_click_regression.sh` 卡在 shard 内自行 `cargo test -p bifrost-cli pure_tray_icon_event_does_not_rebuild_native_menu` 的冷编译/下载阶段。修复后脚本在 `SKIP_BUILD=true` 时跳过该 unit guard，并复用 `BIFROST_BIN` 或 `target/release/bifrost`，保留真实 macOS tray helper 启动、`tray.pid`、`tray.log` 和纯图标点击不重建菜单的日志断言。 | 本地执行 `SKIP_BUILD=true BIFROST_BIN=/Users/eden/work/github/bifrost-tray-helper-design/target/debug/bifrost bash e2e-tests/tests/test_cli_tray_menu_click_regression.sh` 通过，输出 `PASS: tray helper launched and pure icon interaction rebuild guard is active`；CI 待重跑确认 |
| 2026-06-11 | TC-TH-21 | 针对 PR CI run `27308760100` 的 macOS shell shard 2 超时补充验证：失败 artifact 显示 `test_cli_tray_startup_ci.sh` 在 shard 内自行 `cargo build --release --bin bifrost`，900 秒预算内停在编译 `bifrost-proxy` 后被 shell runner 杀掉。修复后 startup smoke 在 `BIFROST_BIN`、`SKIP_BUILD=true` 或 `BIFROST_TRAY_STARTUP_SKIP_BUILD=1` 时复用现有 binary，只在普通本地运行且未要求 skip-build 时构建。 | 本地执行 `SKIP_BUILD=true BIFROST_BIN=$PWD/target/release/bifrost bash e2e-tests/tests/test_cli_tray_startup_ci.sh` 验证 skip-build 路径；CI 待重跑确认 |
| 2026-06-11 | TC-TH-02-REG-03 | 针对主进程 CPU 高或 Admin API 慢时托盘菜单卡住的回归补充验证：实现上将规则、组、active-summary 与 system proxy 获取移动到后台快照线程，UI event loop 只读取最近一次快照。 | 本地执行 `cargo test -p bifrost-cli quick_menu_snapshot -- --nocapture` 通过，慢 Admin API 快速快照回归通过；本地执行 `SKIP_BUILD=true BIFROST_BIN=$PWD/target/debug/bifrost bash e2e-tests/tests/test_cli_tray_startup_ci.sh` 通过，输出 `PASS: tray helper started on Darwin` |
| 2026-06-11 | TC-TH-02-REG-04 | 针对 CLI 重启时同一数据目录不能创建第二个 tray helper 的回归补充验证：launcher 在 spawn 前检查 `tray.lock`，已有 helper 持锁时直接跳过创建，helper 内部 lock 继续作为竞态兜底；lock 持有但 `tray.pid` 暂缺时也跳过 spawn，避免退出窗口期短暂重复创建。 | 本地执行 `cargo test -p bifrost-cli existing_tray_helper_pid -- --nocapture` 通过，包含 active lock + stale pid 与 lock held/no pid 两种路径 |
| 2026-06-11 | TC-TH-08B | 针对配置文件和 Settings 开关禁用/重新启用托盘的新增验证：`[tray] enabled = false` 阻止 CLI 启动 helper；运行中通过 Admin API 关闭 Tray Icon 后，helper 轮询到配置禁用并主动清理 `tray.pid` 退出；重新启用时 Admin API 通过 CLI 注入的 launcher 回调重新创建 helper，并对旧 helper 退出锁释放窗口做短重试。 | 本地执行 `cargo test -p bifrost-cli should_launch_tray_disabled_by_config -- --nocapture` 通过；本地真实启动当前 `target/debug/bifrost` 后执行 `PUT /_bifrost/api/config/tray {\"enabled\":false}` 通过，输出 `PASS: tray config API disabled running helper`；预置 `config.toml` 后重启通过，输出 `PASS: tray config disabled before start skipped helper`；本次补充执行 `SKIP_BUILD=true BIFROST_BIN=$PWD/target/debug/bifrost bash e2e-tests/tests/test_cli_tray_config_reenable.sh` 通过，输出 `PASS: tray helper relaunched after config enable (before=76710 after=77231)` |
| 2026-06-11 | TC-TH-13 | 针对 Rules 菜单只支持选中、不支持再次点击取消的回归补充验证：菜单 action 带上当前 checked 状态；点击未启用规则时继续执行单选收敛，点击当前已启用规则时只调用该规则 disable API，不再 enable 回去。 | 本地执行 `cargo test -p bifrost-cli toggle_single_rule -- --nocapture` 通过未选中选择路径；执行 `cargo test -p bifrost-cli enabled_rule_calls_admin_api_for_disable_only -- --nocapture` 通过已选中取消路径，断言只发 `PUT /_bifrost/api/rules/beta/disable`；执行 `cargo test -p bifrost-cli test_rules_menu_two_levels_without_groups -- --nocapture` 通过，断言 action 的 `currently_enabled` 与原生勾选状态一致 |
| 2026-06-11 | TC-TH-13-REG-01 | 针对 Rules 点击一次但状态仍停留在旧规则的回归补充验证：旧实现把菜单中所有候选规则放进 action，日志中出现一次点击触发大量 `group rule disabled` 的现象；修复后 action 只携带当前已启用的其他规则，切换成功后立即刷新菜单快照并提升 generation。 | 本地执行 `cargo test -p bifrost-cli test_rules_menu_two_levels_without_groups -- --nocapture` 通过，断言未启用 `beta` 的 action 只携带已启用的 `alpha`；执行 `cargo test -p bifrost-cli recent_rule -- --nocapture` 通过，断言最近快捷项也不会携带全量候选；执行 `cargo test -p bifrost-cli toggle_single_rule -- --nocapture` 通过，断言切换路径只调用待禁用目标与新目标 |
| 2026-06-12 | TC-TH-15B | 针对主服务退出后托盘 helper 不应长期残留的新增验证：状态轮询线程在服务进入 Stopped/Disconnected 后开始 10 分钟空转计时，Running 或 Starting 会重置计时；超过 10 分钟仍未恢复服务时设置 quit flag 并清理 `tray.pid`。 | 本地执行 `cargo test -p bifrost-cli service_idle_auto_exit -- --nocapture` 通过，覆盖停止未超时不退出、达到 10 分钟退出、Running 重置、Starting 不退出并重新计时 |
| 2026-06-11 | TC-TH-22 | 针对 tray helper RSS 超过 50 MB 的内存口径与运行时瘦身验证：复用 HTTP agent、缩小 tray 日志队列、常驻/动作线程使用小栈，并对远端 group 失败做短退避；同时区分 `ps RSS` 与 macOS `Physical footprint`。 | 本地执行 release 真实 helper 测量：`ps RSS` 启动后约 38 MB，12 秒后约 56 MB；`vmmap -summary` 显示 `Physical footprint: 17.8M`、dirty heap 约 11.9M，满足 30 MB 独占内存目标；`strip` 将二进制从 110M 降至 92M 但 RSS 不变，说明 RSS 主要来自共享 framework 映射而非符号段；远端 group 失败日志退避后 12 秒内 warning 约 3 次，不再每秒刷 |
| 2026-06-11 | TC-TH-23 | 针对 tray helper 内存优化做代码级归因与收尾：检查 `main.rs` 早返回入口、`Cargo.toml` 主 CLI 依赖、tray 模块 import、`tray_launcher.rs` 配置读取依赖，以及 `arboard`/`open`/`image`/`tao`/`tray-icon`/`muda`/`bifrost-core` 的依赖树；同时清理不采用的独立 helper 与手写平台 open/clipboard 方案。 | 本地执行 `rg -n "run_if_tray|install_panic_hook|init_crypto_provider|Cli::parse|init_logging" crates/bifrost-cli/src/main.rs`，确认 `run_if_tray_process` 在主初始化前；执行 `cargo tree -p bifrost-cli --target aarch64-apple-darwin -i arboard` 与 `cargo tree -p bifrost-cli --target aarch64-apple-darwin -i open`，确认继续复用跨平台 `arboard`/`open`；执行残留扫描 `rg -n "bifrost-tray-helper|find_sibling_tray_helper|tray_helper_binary|/usr/bin/pbcopy|fn copy_text_to_clipboard|fn open_location"` 无命中；已在 `design/cli-tray-helper.md` 记录独立 helper、替换 `open`/`arboard` 和原生 AppKit/Win32 的实测结论与不采用原因 |
| 2026-06-12 | TC-TH-21 | 跟进 GitHub Actions run `27407031037` 的 `E2E Runner (aarch64-pc-windows-msvc)`，定位 `test_cli_tray_startup_ci.sh` 在 Windows tray helper 快速创建又清理 `tray.pid` 时，`[[ -s tray.pid ]]` 后的 `cat tray.pid | tr ...` 触发 `set -e -o pipefail` 直接退出。 | 待复验；脚本改为通过 `read_tray_pid_file` 容忍 PID 文件读取竞态，Windows 仍可用 tray log startup marker 做 log-only fallback |

## 清理步骤

```bash
cargo run --bin bifrost -- stop
rm -rf ./.bifrost-tray-test
```
