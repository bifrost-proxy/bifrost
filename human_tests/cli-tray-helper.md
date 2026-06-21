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
- 包含以下菜单项：Open Traffic、Open Rules、Open Settings、Copy HTTP Proxy、Copy SOCKS5 Proxy
- 点击 Open Settings 打开 `http://127.0.0.1:8801/_bifrost/settings`
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
3. 每次菜单展开后移动鼠标到 "Open Traffic" 菜单项但不点击
4. 查看 `.bifrost-tray-test/logs/tray.log*`

**预期结果：**
- 每次点击后菜单都保持展开，不出现闪烁一下立即消失
- 鼠标移动到 "Open Traffic" 时该菜单项保持可见且可高亮
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
- Open Traffic、Copy HTTP Proxy、Rules、System Proxy 等依赖 Admin URL 的菜单项仍使用启动参数中的 `127.0.0.1:8801`
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

### TC-TH-03: Open Traffic 打开浏览器管理端 Traffic 页面

**操作步骤：**
1. 点击 "Open Traffic" 菜单项

**预期结果：**
- 默认浏览器打开 `http://127.0.0.1:8801/_bifrost/traffic`
- 管理端 Traffic 页面正常加载

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
- 依赖服务的菜单项（Open Traffic 等）置灰

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
- Open Traffic、Open Rules、Open Settings、Copy HTTP Proxy 等依赖服务的菜单项置灰
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
3. 点击 Open Settings 并确认打开 Settings 页面；点击 Copy HTTP Proxy 后粘贴到文本编辑器

**预期结果：**
- Open Settings 打开 `http://[::1]:8801/_bifrost/settings`
- HTTP Proxy 为 `http://[::1]:8801`
- Open Settings 和 Open Traffic 使用同样带方括号的 URL

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
- 数据目录必须生成 `tray.pid`，且对应 helper 进程存活；Windows runner 只有在显式设置 `BIFROST_TRAY_STARTUP_ALLOW_LOG_ONLY=1` 的诊断模式下，才允许把 `logs/tray.log*` 启动标记作为降级信号。常规回归中，只有 `bifrost-tray starting` 但没有活 helper 必须判为失败
- `logs/tray.log*` 包含 `bifrost-tray starting`
- 脚本结束时停止主服务、杀掉 helper，并清理临时数据目录

### TC-TH-24: Windows 前台启动托盘 helper 必须在主线程保持存活（回归）

**操作步骤：**
1. 在 Windows 11 交互用户 session 中准备当前版本 `bifrost.exe`
2. 使用临时数据目录启动前台服务：

```powershell
$env:BIFROST_DATA_DIR="$env:TEMP\bifrost-tray-win-main-thread"
$env:BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT="1"
bifrost.exe start -p 18895 --unsafe-ssl --skip-cert-check --no-system-proxy
```

3. 等待 Admin API ready 后检查：

```powershell
Test-Path "$env:BIFROST_DATA_DIR\tray.pid"
Get-Content "$env:BIFROST_DATA_DIR\tray.pid"
Get-Process bifrost | Select-Object Id,Path,CommandLine
Get-Content "$env:BIFROST_DATA_DIR\logs\tray.log*" -Tail 80
```

4. 观察 Windows notification area 是否出现 Bifrost 图标

**预期结果：**
- 前台 `bifrost start` 进程保持运行，Admin API ready
- `tray.pid` 存在且 PID 对应 `bifrost.exe __tray` helper 进程仍存活
- `logs/tray.log*` 包含 `bifrost-tray starting`，并且不只是重复 starting 后退出
- Windows notification area 显示 Bifrost 图标
- 如果只有 `tray.lock` 或 `tray.log*`，但没有 `tray.pid` / live helper / notification area 图标，判定为托盘启动回归

### TC-TH-25: Windows 托盘 Stop 后可从同一托盘重新 Start（回归）

**操作步骤：**
1. 在 Windows 11 交互用户 session 中准备当前版本 `bifrost.exe`
2. 使用临时数据目录启动服务并等待 notification area 出现 Bifrost 托盘图标：

```powershell
$env:BIFROST_DATA_DIR="$env:TEMP\bifrost-tray-win-stop-start"
$env:BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT="1"
bifrost.exe start -p 18896 --unsafe-ssl --skip-cert-check --no-system-proxy
```

3. 点击托盘菜单中的 `Stop Bifrost`
4. 等待菜单状态变为 `Bifrost: Stopped` 或 `Bifrost: Disconnected`
5. 不退出托盘 helper，继续点击同一托盘菜单中的 `Start Bifrost`
6. 等待最多 10 秒后检查：

```powershell
Invoke-WebRequest -UseBasicParsing http://127.0.0.1:18896/_bifrost/api/proxy/address
Get-Content "$env:BIFROST_DATA_DIR\runtime.json"
Get-Content "$env:BIFROST_DATA_DIR\tray.pid"
Get-Process bifrost | Select-Object Id,Path,CommandLine
Get-Content "$env:BIFROST_DATA_DIR\logs\tray.log*" -Tail 120
```

**预期结果：**
- Stop 后主服务进程退出，托盘 helper 在空转超时窗口内保持存在并显示 stopped/disconnected 状态
- 点击 `Start Bifrost` 后菜单先显示 `Bifrost: Starting...`，且 `Start Bifrost` 暂时置灰
- 托盘触发的 Start 使用 detached daemon 启动主服务，不依赖 tray helper 的前台子进程生命周期
- detached daemon child 看到 `BIFROST_DETACHED_DAEMON_CHILD=1` 后必须直接进入长期 runtime，不能再次递归执行 daemon parent 启动器
- `runtime.json` 重新生成并指向活的主服务 PID，`runtime_start_mode` 为 `daemon`
- Admin API 重新 ready，托盘菜单恢复 `Bifrost: Running on 127.0.0.1:18896`
- `tray.pid` 仍指向同一个存活的 `bifrost.exe __tray` helper；不会创建第二个托盘 helper，也不会因为只有 foreground child 退出而显示 `Start failed`

### TC-TH-26: 托盘后台空闲不高频查询系统代理且 Windows 不弹终端窗口（回归）

**操作步骤：**
1. 在 Windows 11 交互用户 session 中使用当前版本 `bifrost.exe` 启动服务和托盘：

```powershell
$env:BIFROST_DATA_DIR="$env:TEMP\bifrost-tray-win-system-proxy-cache"
$env:BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT="1"
bifrost.exe start -p 18897 --unsafe-ssl --skip-cert-check --no-system-proxy
```

2. 在不展开托盘菜单的情况下等待 30 秒，并用进程采样确认是否有 Bifrost 父进程反复创建系统命令：

```powershell
Get-CimInstance Win32_Process |
  Where-Object { $_.Name -in @("reg.exe", "powershell.exe", "cmd.exe", "conhost.exe", "WindowsTerminal.exe", "OpenConsole.exe") } |
  Select-Object ProcessId,ParentProcessId,Name,CommandLine
```

3. 展开托盘菜单一次，允许托盘按需刷新 System Proxy 缓存
4. 再等待 30 秒，重复第 2 步采样
5. 查看 `logs/tray.log*` 与 `logs/bifrost*.log`

**预期结果：**
- 托盘后台空闲轮询不会每秒请求 `/api/proxy/system`
- 后台空闲时，Bifrost 主服务不会反复创建 `reg.exe` / `powershell.exe` / `cmd.exe` / `conhost.exe` / `WindowsTerminal.exe`
- Windows 上即使用户展开托盘菜单触发一次 System Proxy 按需刷新，也不会弹出可见终端窗口
- Windows 上点击 `Open Traffic` / `Open Settings` / `Open Logs` 这类打开 URL 或目录的菜单项时，托盘 helper 也不应通过 `cmd /c start` 创建可见 console 子进程；应使用无控制台的系统打开 API
- Windows 上 Sync 启动自动登录提示打开浏览器时，也不应通过 `cmd /c start` 创建可见 console 子进程；应使用无控制台的系统打开 API
- Windows 上 `bifrost start -d` 后，daemon child 必须自动拉起 tray helper；不能只启动后台主服务而没有托盘图标
- System Proxy 菜单项使用最近一次缓存状态渲染；缓存只在托盘交互、System Proxy 开关操作或显式菜单动作后刷新
- macOS/Windows 都不允许因为托盘常驻而高频调用系统代理检测命令；相关检测必须是按需或低频缓存路径

### TC-TH-28: Windows Tray self-update 释放托盘锁并准确上报失败（回归）

**操作步骤：**
1. 在 Parallels Windows 11 交互用户 session 中确认当前用户 PATH 优先命中待升级的 CLI 安装位，例如：

```powershell
$env:BIFROST_DATA_DIR="$env:USERPROFILE\.bifrost"
Get-Command bifrost -All
& "$env:USERPROFILE\.local\bin\bifrost.exe" --version
```

2. 通过该二进制启动服务和托盘，确保 `tray.log*` 记录 `tray helper launched tray_bin=...\.local\bin\bifrost.exe`：

```powershell
$env:BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT="1"
& "$env:USERPROFILE\.local\bin\bifrost.exe" start -d -p 9900 --unsafe-ssl --skip-cert-check --no-system-proxy
Get-Content "$env:BIFROST_DATA_DIR\logs\bifrost*.log" -Tail 200
Get-Content "$env:BIFROST_DATA_DIR\logs\tray.log*" -Tail 120
```

3. 从托盘菜单点击 `Update to v<latest>`，等待最多 3 分钟。
4. 检查升级结果、helper 日志和目标二进制版本：

```powershell
Get-Content "$env:BIFROST_DATA_DIR\upgrade-progress.json"
Get-ChildItem "$env:USERPROFILE\.local\bin" -Force -Filter ".bifrost-upgrade-*" |
  Sort-Object LastWriteTime |
  Select-Object Name,Length,LastWriteTime
Get-ChildItem "$env:USERPROFILE\.local\bin" -Force -Filter ".bifrost-upgrade-*.log" |
  ForEach-Object { $_.FullName; Get-Content $_.FullName }
& "$env:USERPROFILE\.local\bin\bifrost.exe" --version
Get-Process bifrost -ErrorAction SilentlyContinue |
  Select-Object Id,Path,StartTime
```

**预期结果：**
- Tray 触发更新后，Rust `self-update` 调度 Windows helper 前会停止同一 `data_dir` 的 tray helper，避免 `bifrost.exe __tray` 持有目标 exe 锁。
- Windows helper 日志包含 `waiting for target binary to become writable`，随后替换目标 exe；不应出现 `Access is denied` / `访问被拒绝`。
- 如果目标 exe 因其他进程长期锁定导致替换失败，`upgrade-progress.json` 必须为 `phase: "failed"` 且 `error` 包含失败原因；禁止显示 `phase: "completed"`。
- 替换成功后，`upgrade-progress.json` 为 `phase: "completed"`，`.local\bin\bifrost.exe --version` 返回目标版本，pending 文件不再残留。
- 如果重启参数存在，helper 使用替换后的目标 exe 执行 `start -d`，新 `Get-Process bifrost` 输出的 `Path` 指向已更新的目标 exe。

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

### TC-TH-29: Tray 常驻展示系统 CPU/内存/磁盘/上下行网速并可在 Settings 关闭

**操作步骤：**
1. 执行菜单结构与系统状态格式化单元回归：
```bash
cargo test -p bifrost-cli system_stats --lib
cargo test -p bifrost-cli menu_bar_stats_bitmap --lib
```
2. 使用当前 debug 二进制执行配置 E2E。macOS 验证默认开启、系统状态总开关独立关闭/开启、CPU/Memory/Disk/Upload/Download 子开关默认全开、单项关闭/开启与配置持久化；Windows/Linux 仅验证 `system_stats_supported=false`、系统状态全部 mask off，且系统状态更新被拒绝：
```bash
BIFROST_BIN="$PWD/target/debug/bifrost" bash e2e-tests/tests/test_tray_system_stats_config.sh
```
3. 使用临时数据目录真实启动 macOS 或 Windows tray helper：
```bash
BIFROST_BIN="$PWD/target/debug/bifrost" SKIP_BUILD=true bash e2e-tests/tests/test_cli_tray_startup_ci.sh
```
4. 打开 `http://127.0.0.1:<port>/_bifrost/settings?tab=tray`。macOS 确认 Settings 中存在独立 Tray tab，`Tray Icon` 和 `Show System Stats` 默认打开，CPU、Memory、Disk、Upload、Download 五个子开关默认打开；分别在桌面和移动端 viewport 截图确认图标、文案、开关位置没有遮挡、错位或横向溢出。Windows 确认 Settings Tray tab 只展示 `Tray Icon` 一个配置项，不展示 `System Status`、`Show System Stats` 或任意 CPU/Memory/Disk/Upload/Download 子开关。
5. 在 macOS 上直接观察当前 `bifrost` 菜单栏 status item；Windows 不做资源状态可视验收。
6. macOS 连续截取至少 3 张菜单栏截图，每张间隔 1 秒，确认无需点击下拉菜单即可看到常驻状态：左侧 Bifrost 图标视觉大小与未开启系统状态时一致，右侧文本为单行布局，展示 `C... | M... | D... | ↑... ↓...`；上传/下载网速是一个整体字段，中间仅用小空格，不用竖线隔开。
7. 对比连续截图，确认字体粗细和高度接近参考系统监控菜单但不过粗，列宽稳定且不依赖左侧补零，上行/下行网速在 1 秒间隔截图中能随系统流量变化刷新。
8. 在 macOS 上执行 `route -n get default`，确认默认出站接口（如 `en0`）；观察 tray 展示值应来自默认出站接口或最可信的活跃物理接口，不应把 Parallels/bridge/VPN/tunnel/Docker 等虚拟接口简单累加导致几十 MB/s 的离谱尖峰。
9. 展开 macOS 原生 tray 菜单，确认菜单中不再重复展示 `System:` 与 `Network:` 两排资源信息。
10. 在 Settings 中关闭 `Download` 子开关，确认 `GET /_bifrost/api/config/tray` 返回 `system_stats_items.download=false`，macOS 菜单栏不再展示 `↓...`，其它已启用项仍展示；重新打开 `Download` 后下行字段恢复。
11. 关闭 `Show System Stats` 后再次观察 macOS 菜单栏与托盘菜单，并截图确认常驻状态恢复普通 Bifrost 图标、菜单仍不展示系统状态两排。
12. 再次打开 `GET /_bifrost/api/config/tray`。macOS 确认 `show_system_stats` 与 Settings 开关一致，且各 `system_stats_items` 子开关与 Settings 一致；Windows 确认 `system_stats_supported=false`、`show_system_stats=false`，且所有 `system_stats_items` 为 `false`。
13. 在 Windows 上仅执行 API/UI unsupported 验证：`PUT /_bifrost/api/config/tray {"show_system_stats":true}` 和 `PUT /_bifrost/api/config/tray {"system_stats_items":{"cpu":true}}` 均返回 400 且错误为 `tray system stats are not supported on this platform`；同时查看 `tray.log*`，确认没有系统状态采样线程启动日志。
14. 在 macOS 上对 `bifrost __tray` helper 做至少 60 秒 CPU/RSS 采样，确认空闲平均 CPU <1%，RSS 相比本功能改动前后没有显著增长；若本机同时有截图、编译或其它重负载，必须记录为环境干扰并重新采样。

**预期结果：**
- macOS 默认 `GET /api/config/tray` 返回 `enabled: true`、`system_stats_supported: true`、`show_system_stats: true`，且 `system_stats_items.cpu/memory/disk/upload/download` 全为 `true`。
- macOS 菜单栏无需点击即可常驻展示系统状态；左侧 Bifrost 图标大小不因启用系统状态而缩小或变形，右侧状态文本使用常规字重系统字体、单行 `C/M/D/↑/↓` 布局和稳定列宽，上传/下载作为一个网络字段展示，数据来源是整机状态，不是 Bifrost 进程自身指标。
- 网速优先按默认出站接口的累计字节差值计算；找不到默认接口时才回退到活跃物理接口，并通过 hysteresis、虚拟接口过滤和指数平滑避免双算、跳接口和单样本尖峰。
- macOS 下拉菜单不重复展示系统状态详情；Windows 托盘菜单也不展示 CPU/Memory/Disk/Up/Down 系统状态详情。
- Windows notification area 原生不支持 macOS 这种横向常驻文本；Windows 产品行为是不支持 Tray 系统信息，不采样、不展示、不暴露配置项。
- Settings 中有独立 `Tray` tab；macOS `Show System Stats` 可独立于 `Tray Icon` 开关关闭/开启，CPU、Memory、Disk、Upload、Download 每一项都可单独启用/禁用且默认全部启用；Windows 只展示 `Tray Icon` 一个配置项。
- Web UI 截图中 Tray tab 的两个卡片、图标、说明文案和开关在桌面与移动端均完整可见；移动端没有横向溢出，开关仍保持右侧对齐。
- macOS 桌面截图中必须看到菜单栏常驻状态；连续 1 秒间隔截图中的网络速率应随系统流量变化刷新，菜单不应闪退或被刷新关闭。
- 关闭单个子项后，只移除该子项展示，不影响其它系统状态项；关闭 `Show System Stats` 后，macOS 菜单栏恢复普通 Bifrost 图标。
- Windows `GET /api/config/tray` 返回 `system_stats_supported=false`、`show_system_stats=false`、所有 `system_stats_items=false`；任何系统状态字段更新都返回 400；真实 tray helper 可启动但不启动系统状态采样线程，且不修改系统代理。
- macOS tray helper 空闲平均 CPU 目标为 <1%，内存不能显著增长；系统状态线程不得因高频网卡列表刷新或重复位图重绘造成 2-3% 常驻占用。

### TC-TH-30: macOS Native 两行菜单栏系统状态像素级还原回归

**操作步骤：**
1. 执行 native 菜单栏状态位图与行转换单元回归：
```bash
cargo test -p bifrost-cli menu_bar_stats --lib --all-features -- --nocapture
```
2. 构建当前 release 二进制：
```bash
SKIP_FRONTEND_BUILD=1 cargo build --release --bin bifrost
```
3. 使用临时数据目录启动真实 macOS Bifrost；macOS native 两行状态项为默认路径，无需额外打开实验开关：
```bash
DATA_DIR=$(mktemp -d /tmp/bifrost-native-stats.XXXXXX)
BIFROST_DATA_DIR="$DATA_DIR" \
BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 \
target/release/bifrost start -d -y --skip-cert-check -p 62144 --host 127.0.0.1 --no-system-proxy
```
4. 等待 `tray.pid` 生成，确认 `GET /_bifrost/api/config/tray` 返回系统状态支持且全项开启。
5. 查看 `logs/tray.log.*`，确认出现 `native macOS tray stats view enabled`。
6. 连续截取 3 张完整顶部菜单栏截图，每张间隔 1 秒；同时生成局部 2x 对照图，要求截图中包含 Bifrost native 单一状态项和右侧参考状态项。
7. 对 3 张截图逐张目视检查：CPU 与上下行网速是否刷新、两行布局是否稳定、列间竖线是否贯穿上下两行、网络上下两行字号是否一致、图形箭头是否与参考方案接近、状态项是否明显比旧 18pt 位图方案更接近参考产品。
8. 使用 `PUT /_bifrost/api/config/tray` 分别关闭 CPU、Memory、Disk、Upload、Download 和 `show_system_stats`，每次等待至少 1 秒后读取 `System Events` 中 `bifrost` 进程 menu bar item 的 `description`，确认对应字段从状态栏 description 中消失，其它字段保持存在。
9. 对同一个 menu bar item 执行 `AXPress`，读取展开菜单中的 `Open Traffic`、`Open Rules`、`Open Settings`、`Stop Bifrost`、`Quit Tray` 等菜单项，确认图标、状态和菜单合并在同一个可点击状态项里。
10. 对 tray helper 做 60 秒空闲 CPU/RSS 采样，确认平均 CPU <1%，RSS 无显著增长。

**预期结果：**
- native 状态项无需展开下拉菜单即可展示两行系统状态：第一行展示 CPU/MEM/SSD/上行速率数值，第二行展示 CPU/MEM/SSD 标签和下行速率。
- CPU/MEM/SSD 数值行字体大于标签行；网络上下行属于同一列且字体一致。
- 列间分隔线为连续竖线，不是上下两行各自的 `|` 字符。
- Bifrost 图标、系统状态和菜单必须是同一个 `NSStatusItem`；点击状态块本身能打开 Bifrost 菜单，不允许出现“状态文字不可点击、旁边另有 Bifrost 图标”的分离形态。
- 网络列使用图形箭头，不使用文本箭头字符；箭头在固定槽位内左对齐，数值文本右对齐，允许箭头和数值之间留出空白。
- accessibility description 必须与当前开关配置一致：全部开启时包含 `C/M/D/↑/↓`，关闭单项后移除对应字段，关闭 `show_system_stats` 后退回 `Bifrost`。
- 不设置 `BIFROST_TRAY_NATIVE_STATS_VIEW` 时，macOS 默认走 native `NSStatusItem` 路径；显式设置 `BIFROST_TRAY_NATIVE_STATS_VIEW=0` 时回退既有 `tray-icon` 位图路径。
- 如果 native `NSStatusItem` 创建失败，应回退既有位图路径，不出现菜单栏空白状态。
- 平均 CPU 低于 1%，RSS 不出现显著持续增长。

## 本次执行记录

| 日期 | 用例 | 执行方式 | 结果 |
| --- | --- | --- | --- |
| 2026-06-21 | TC-TH-30 | 针对 review 提出的三项 macOS native tray 性能/稳定性风险执行代码审查和回归修复。确认 `poll_system_stats` 原实现每 1 秒调用 `load_tray_system_stats_config` 同步读取并解析 `config.toml`，存在高频 I/O 与配置写入瞬间 partial-read 风险；确认默认路由刷新原实现每 60 秒在采样线程同步执行 `route -n get default` / IPv6 fallback，存在网络切换时卡住状态刷新风险；确认 `NativeStatsStatusItem` 持有 AppKit `NSStatusItem` 但没有显式 `!Send/!Sync` 约束，未来误移动到采样线程会让 Drop 在非主线程触发 AppKit cleanup。修复后，config 改为 `notify` watcher 事件驱动并保留 30 秒兜底重读，读取/解析失败保留上一份配置；默认路由检测改为后台 `bifrost-tray-route-detect` worker，`route` 子进程使用 750ms timeout，采样线程只非阻塞消费 channel 结果；native status item 通过 `PhantomData<Rc<()>>` 标记 main-thread only，并在 Drop 中检查主线程。执行 `cargo fmt --all` 通过；执行 `SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-cli system_stats --lib --all-features -- --nocapture` 通过 27/27，覆盖 parse error 保留旧 config、route timeout、resolver 结果无需等下一次 60 秒刷新即可应用；执行 `SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-cli native_ --lib --all-features -- --nocapture` 通过 13/13；执行 `SKIP_FRONTEND_BUILD=1 cargo check -p bifrost-cli --all-features` 通过；执行 `SKIP_FRONTEND_BUILD=1 cargo clippy -p bifrost-cli --all-targets --all-features -- -D warnings` 通过；执行 `BIFROST_BIN="$PWD/target/debug/bifrost" BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 bash e2e-tests/tests/test_tray_system_stats_config.sh` 通过 63/63，覆盖 Settings/API 总开关与 CPU/Memory/Disk/Upload/Download 子项持久化链路。 | 通过。三条 review 均为真实或部分真实风险，本轮已修复到“不影响 1 秒展示刷新”的方案：配置变化仍可通过 watcher 尽快反映，watcher 失效时最多 30 秒兜底；默认路由查询不会再阻塞采样线程，route 超时或失败时保留上一轮接口/活跃接口 fallback；native AppKit 状态项的跨线程误用被编译期约束挡住。 |
| 2026-06-21 | TC-TH-30 | 针对 macOS native tray 系统状态 CPU 占用偏高执行性能回归和优化验证。先使用当前 debug 二进制启动真实临时实例 `/tmp/bifrost-tray-perf.bzuPmW`，端口 `59646`，主进程 PID `98433`，tray PID `98451`，启动参数包含 `--no-system-proxy --skip-cert-check` 且设置 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`；60 个 1 秒 `ps` 样本记录 `/tmp/bifrost-tray-perf.bzuPmW/tray-ps-sample-debug.csv`，汇总 `avg_cpu=1.978 p50=1.100 p90=5.100 p95=6.000 max_cpu=6.500 avg_rss_mb=84.10`。执行 `sample 98451 10`，样本 `/tmp/bifrost-tray-perf.bzuPmW/tray-debug.sample.txt` 明确热点集中在 `encode_menu_bar_stats_png`、`png::Writer::write_image_data` 和 `NSImage initWithData`，系统指标采样本身占比很低。随后将 native AppKit 路径从每秒 PNG encode/decode 改为 `NSBitmapImageRep` raw RGBA buffer，并在宽高不变时复用同一个 `NSImage` / bitmap representation，只更新像素内容。执行 `cargo fmt --all` 与 `SKIP_FRONTEND_BUILD=1 cargo check -p bifrost-cli --all-features` 通过；执行 `SKIP_FRONTEND_BUILD=1 cargo build --release --bin bifrost` 通过。最终 release 实例使用临时目录 `/tmp/bifrost-tray-perf-cache.zqeaw6`、端口 `60807`、主进程 PID `22080`、tray PID `22098` 启动；AX 读取同一个 menu bar item description 为 `Bifrost: C15% | M55% | D65% | ↑5.7 K/s ↓9.9 K/s`。60 个 1 秒样本记录 `/tmp/bifrost-tray-perf-cache.zqeaw6/tray-ps-sample-release-cache.csv`，汇总 `avg_cpu=0.437 p50=0.300 p90=1.000 p95=1.200 max_cpu=2.300 avg_rss_mb=66.43 max_rss_mb=67.16`。关闭系统状态后 release 基线记录 `/tmp/bifrost-tray-perf-release.93wKvJ/tray-ps-sample-release-disabled.csv`，汇总 `avg_cpu=0.077 p95=0.600 max_cpu=0.600 avg_rss_mb=69.63`。尝试截图 `/tmp/bifrost-tray-perf-cache.zqeaw6/menu-bar-cache.png` 生成 5120x2880 PNG 但为黑帧，`screencapture -R` 返回 `could not create image from rect`，因此本轮没有把黑帧当视觉通过证据。 | 通过。优化后保持 1 秒状态刷新和同一个 native `NSStatusItem` 展示/菜单语义不变，平均 CPU 从 debug 基线 `1.978%` 降到 release `0.437%`，低于 1% 目标；关闭系统状态后的基线为 `0.077%`，说明系统状态展示本身新增平均开销约 `0.36%`。RSS 稳定在约 66-67 MB，无显著增长。`sample` 已确认 PNG encode/decode 热点消失，剩余开销主要是每秒 AppKit 状态项重绘；个别 1 秒 `ps` 样本仍可能出现 1.x% 瞬时峰值，如需把 P95 也压到 1% 以下，需要把可见刷新频率降到 1.5-2 秒，会牺牲用户要求的网速实时性，本轮选择保留 1 秒刷新。 |
| 2026-06-21 | TC-TH-02 / TC-TH-03 / TC-TH-14-REG-02 / TC-TH-30 | 针对 native macOS 菜单点击不触发 action、默认菜单重复入口和 System Proxy 菜单打开前刷新回归重新执行。执行 `cargo test -p bifrost-core shell_proxy --lib -- --nocapture` 通过 24/24；执行 `cargo test -p bifrost-cli commands::tray::menu::tests --lib --all-features -- --nocapture` 通过 20/20，断言 `open_admin_ui` 不存在且 `Open Settings` 指向 `/_bifrost/settings`；执行 `cargo test -p bifrost-cli native_ --lib --all-features -- --nocapture` 通过 13/13，覆盖 native `menuWillOpen` 刷新 System Proxy 缓存。执行 `SKIP_FRONTEND_BUILD=1 cargo build --bin bifrost` 后使用临时目录 `/tmp/bifrost-open-settings-fixed.g1hwHz/data`、端口 `61804` 启动真实 macOS daemon，tray PID `26957`，启动参数包含 `--no-system-proxy --skip-cert-check`，并设置 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`。通过 `System Events` 读取同一个 menu bar item 的 description 为 `Bifrost: C15% | M55% | D65% | ↑9.3 K/s ↓17.7 K/s`；执行 AXPress 后菜单项为 `Bifrost: Running on 127.0.0.1:61804`、`Version v0.0.112`、`Open Traffic`、`Open Rules`、`Open Settings`、`Copy HTTP Proxy`、`Copy SOCKS5 Proxy`、`Rules: None`、`Stop Bifrost`、`System Proxy`、`Open Logs`、`Quit Tray`，不包含 `Open Admin UI`。把 Microsoft Edge 当前 tab 先设为 `about:blank`，再通过 tray 菜单点击 `Open Settings`；`tray.log` 中 `native menu action triggered` 计数从 1 增加到 2，Edge active tab URL 变为 `http://127.0.0.1:61804/_bifrost/settings`。截图保存为 `/tmp/bifrost-open-settings-fixed-menu.png`，可见完整菜单、Settings 页面和同一个 native 状态项。 | 通过。native `NSStatusItem.setMenu(...)` 路径已补显式 target/action 派发，不再依赖不会触发的 `muda::MenuEvent`；`Open Settings` 会真实打开管理端 Settings 页面；默认菜单删除重复的 `Open Admin UI`，保留 `Open Traffic` 作为 Traffic 页入口；System Proxy 勾选状态在 native 菜单 `menuWillOpen` 时触发按需刷新，避免只在旧 TrayIconEvent 点击路径更新造成 stale 状态。 |
| 2026-06-21 | TC-TH-02 | 根据最新反馈将默认菜单中的 `Copy Admin URL` 替换为 `Open Settings`。执行 `cargo test -p bifrost-cli test_menu_running_state --lib --all-features -- --nocapture` 通过 1/1，断言 `open_settings` 菜单项 label 为 `Open Settings` 且 action 为 `OpenUrl("http://127.0.0.1:8800/_bifrost/settings")`；执行 `cargo fmt --all -- --check` 通过；执行 `cargo test -p bifrost-cli commands::tray::menu::tests --lib --all-features -- --nocapture` 通过 20/20；执行 `BIFROST_BIN="$PWD/target/debug/bifrost" bash e2e-tests/tests/test_tray_system_stats_config.sh` 通过 63/63。随后执行 `SKIP_FRONTEND_BUILD=1 cargo build --bin bifrost` 重建真实 CLI 二进制，清理旧临时实例后使用临时目录 `/tmp/bifrost-open-settings.IaCn4B/data`、端口 `56622` 启动真实 macOS daemon，tray PID `34043`，启动参数包含 `--no-system-proxy --skip-cert-check`，并设置 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`。`tray.log.2026-06-21` 记录 `native macOS tray stats view enabled as primary status item`；通过 `System Events` 针对 PID `34043` 执行 AXPress，展开同一个 native status item 的菜单，菜单项依次包含 `Open Admin UI`、`Open Traffic`、`Open Rules`、`Open Settings`、`Copy HTTP Proxy`、`Copy SOCKS5 Proxy`、`Rules: None`、`Stop Bifrost`、`System Proxy`、`Open Logs`、`Quit Tray`。验证后执行 `BIFROST_DATA_DIR=/tmp/bifrost-open-settings.IaCn4B/data target/debug/bifrost stop` 停止临时实例。 | 通过。默认菜单红框位置现在是 `Open Settings`，不再显示 `Copy Admin URL`；行为不是只改文案，菜单 action 指向 Settings 路由。验证中发现如果只跑 lib 单测而不重建 `target/debug/bifrost`，真实托盘会继续使用旧二进制；本轮已重建 binary 后再做 AX 菜单回归，避免旧实例和旧二进制串台。 |
| 2026-06-21 | TC-TH-30 | 修正 macOS native 路线默认行为后补充回归。执行 `cargo fmt --all -- --check` 通过；执行 `cargo test -p bifrost-cli native_stats_view --lib --all-features -- --nocapture` 通过 1/1，确认不设置 `BIFROST_TRAY_NATIVE_STATS_VIEW` 时默认启用 native，显式设置 `0/false/no/off` 时才回退旧路径；执行 `cargo test -p bifrost-cli native_ --lib --all-features -- --nocapture` 通过 12/12。执行 `SKIP_FRONTEND_BUILD=1 cargo build --release --bin bifrost` 通过；复制 release 二进制到 `/tmp/bifrost-native-default.8UAgca/bifrost-native-default`，不设置 `BIFROST_TRAY_NATIVE_STATS_VIEW`，仅设置 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`，使用临时数据目录 `/tmp/bifrost-native-default.8UAgca/data`、端口 `50021`、主进程 PID `73098`、tray PID `73116` 启动真实 macOS daemon，启动参数包含 `--no-system-proxy --skip-cert-check`。`GET /_bifrost/api/config/tray` 返回系统状态支持且全项开启；`tray.log.2026-06-21` 记录 `native macOS tray stats view enabled as primary status item`；`System Events` 读取 menu bar item description 为 `Bifrost: C5% | M80% | D65% | ↑5.1 K/s ↓4.6 K/s`。测试后停止临时实例并确认主进程、tray 进程退出。 | 通过。macOS native 两行状态项现在是默认路径，不需要额外环境变量；环境变量只保留为显式回退开关。该修复避免用户安装正常版本后仍落到旧 `tray-icon` 位图路径。 |
| 2026-06-21 | TC-TH-30 | 按用户要求重新执行所有 Tray 系统状态开关回归。执行 `cargo test -p bifrost-cli native_ --lib --all-features -- --nocapture` 通过 11/11，覆盖 native 单状态项、两行布局、网络图形箭头、固定槽位、可访问性标签和子项过滤；执行 `cargo test -p bifrost-cli menu_bar_stats --lib --all-features -- --nocapture` 通过 6/6；执行 `cargo test -p bifrost-cli system_stats --lib --all-features -- --nocapture` 通过 24/24；执行 `BIFROST_BIN="$PWD/target/debug/bifrost" bash e2e-tests/tests/test_tray_system_stats_config.sh` 通过 63/63，覆盖 CPU/Memory/Disk/Upload/Download 与 `show_system_stats` 的逐项关闭、持久化和恢复。随后执行 `SKIP_FRONTEND_BUILD=1 cargo build --release --bin bifrost` 通过，复制 release 二进制到 `/tmp/bifrost-native-regression3.sm0Nsa/bifrost-native-regression3` 并以 daemon 方式启动真实 macOS 实例，数据目录 `/tmp/bifrost-native-regression3.sm0Nsa/data`，端口 `49518`，主进程 PID `64402`，tray PID `64421`，环境包含 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1` 与 `BIFROST_TRAY_NATIVE_STATS_VIEW=1`，启动参数包含 `--no-system-proxy --skip-cert-check`。`tray.log.2026-06-21` 记录 `native macOS tray stats view enabled as primary status item`。通过 `PUT /_bifrost/api/config/tray` 逐项切换后用 `System Events` 读取同一个 menu bar item description：全开为 `Bifrost: C5% | M80% | D65% | ↑3.4 K/s ↓8.1 K/s`；`cpu=false` 为 `Bifrost: M80% | D65% | ↑4.0 K/s ↓3.2 K/s`；`memory=false` 为 `Bifrost: C0% | D65% | ↑1.6 K/s ↓1.7 K/s`；`disk=false` 为 `Bifrost: C5% | M80% | ↑5.0 K/s ↓6.3 K/s`；`upload=false` 为 `Bifrost: C0% | M80% | D65% | ↓25.1 K/s`；`download=false` 为 `Bifrost: C5% | M80% | D65% | ↑4.2 K/s`；`show_system_stats=false` 为 `Bifrost`；恢复全开为 `Bifrost: C5% | M80% | D65% | ↑12.6 K/s ↓12.4 K/s`，原始记录保存于 `/tmp/bifrost-native-regression3.sm0Nsa/tray-switch-ax.txt`。对同一状态项执行 AXPress 后菜单项包含 `Open Admin UI`、`Open Traffic`、`Open Rules`、`Stop Bifrost`、`System Proxy`、`Open Logs`、`Quit Tray`。对 tray PID `64421` 采集 60 个 1 秒 CPU/RSS 样本，记录 `/tmp/bifrost-native-regression3.sm0Nsa/tray-perf-60s.txt`。尝试 `screencapture -x -R0,0,1800,140` 返回 `could not create image from rect`；整屏 `screencapture -x` 可生成 5120x2880 PNG，但顶部裁剪为黑帧；Computer Use 读取 Finder 桌面截图成功但只暴露左侧系统菜单，未显示右侧 menu extras。 | 通过，截图通道受限。配置 E2E、真实 macOS daemon 实例、状态栏 AX description 和 AXPress 菜单验证共同证明 Settings/Tray 每个开关都会反映到同一个可点击 Bifrost native status item：关闭单项只隐藏对应 CPU/MEM/SSD/上行/下行字段，关闭总开关恢复纯 `Bifrost`，恢复全开后全部字段回到状态栏；点击状态块本身打开 Bifrost 原菜单，不再是分离的不可点击状态块。性能汇总为 `samples=60 avg_cpu=0.6583 max_cpu=2.0000 avg_rss_kb=79212 min_rss_kb=79136 max_rss_kb=79216 rss_delta_kb=80`，平均 CPU <1%，RSS 无显著增长。当前自动截图链路仍无法稳定捕获右侧菜单栏状态项，因此没有把黑帧当作视觉通过证据；视觉部分仍保留为 AX 可验证和后续人工目视复核。 |
| 2026-06-21 | TC-TH-30 | 根据最新反馈将 native 方案从“额外状态项”改为单个 AppKit `NSStatusItem`：同一状态项内渲染 Bifrost 图标、两行系统状态和菜单；网络列改为图形箭头，箭头左对齐、数值右对齐；native Bifrost 图标使用 36px/18pt 目标高度，避免图标变小；状态项写入 accessibility description 供真实状态栏回归。执行 `cargo fmt --all -- --check`、`cargo check -p bifrost-cli --all-features`、`cargo test -p bifrost-cli native_stats_accessibility_label --lib --all-features -- --nocapture`、`cargo test -p bifrost-cli native_menu_bar_stats_rows_follow_each_tray_switch --lib --all-features -- --nocapture`、`cargo test -p bifrost-cli native_network --lib --all-features -- --nocapture` 均通过；执行 `BIFROST_BIN="$PWD/target/debug/bifrost" bash e2e-tests/tests/test_tray_system_stats_config.sh` 通过 63/63，覆盖 CPU/Memory/Disk/Upload/Download 逐项关闭、持久化和恢复。执行 `SKIP_FRONTEND_BUILD=1 cargo build --release --bin bifrost` 通过；使用临时数据目录 `/tmp/bifrost-native-ax.Flkijl`、端口 `62147` 启动真实 release 服务，主进程 PID `48471`、tray PID `48489`，启动参数包含 `--no-system-proxy --skip-cert-check`，并设置 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1` 与 `BIFROST_TRAY_NATIVE_STATS_VIEW=1`；`tray.log.2026-06-21` 记录 `native macOS tray stats view enabled as primary status item`。`System Events` 读取同一个 menu bar item 的 description 为 `Bifrost: C5% | M80% | D65% | ↑... ↓...`；执行 AXPress 后读取菜单项包含 `Open Admin UI`、`Open Traffic`、`Open Rules`、`Stop Bifrost`、`Quit Tray`。逐项 `PUT /_bifrost/api/config/tray` 后读取 description：`cpu=false` -> `Bifrost: M80% | D65% | ↑... ↓...`；`memory=false` -> `Bifrost: C5% | D65% | ↑... ↓...`；`disk=false` -> `Bifrost: C5% | M80% | ↑... ↓...`；`upload=false` -> `Bifrost: C5% | M80% | D65% | ↓...`；`download=false` -> `Bifrost: C5% | M80% | D65% | ↑...`；`show_system_stats=false` -> `Bifrost`。有效顶部截图保存为 `/tmp/bifrost-native-ax-shots/topbar-direct.png`，包含左侧 Bifrost 单 native 状态项与右侧旧参考状态项。对 tray PID `48489` 做 60 秒空闲采样，记录 `/tmp/bifrost-native-ax-shots/tray-perf-60s.txt`。 | 通过。状态栏形态已合并为单个可点击 native `NSStatusItem`，不再存在“状态文字不可点击、旁边另有 Bifrost 图标”的分离问题；AXPress 证明点击同一状态块能打开 Bifrost 菜单。逐项开关回归证明 Settings/API 配置会真实反映到菜单栏状态项内容；Upload/Download 关闭时只隐藏对应方向，两个方向仍作为同一个网络列处理。截图方面，`/tmp/bifrost-native-ax-shots/topbar-direct.png` 是有效视觉证据；后续批量 `screencapture` 在本机再次出现间歇黑帧或 `could not create image from rect`，因此逐项切换的截图证据以 AX description 为机器可验证结果，不能把黑帧截图当作通过证据。60 秒性能汇总为 `samples=60 avg_cpu=0.6950 max_cpu=2.3000 avg_rss_kb=81537 min_rss_kb=81520 max_rss_kb=81552 rss_delta_kb=32`，平均 CPU <1%，RSS 无显著增长。 |
| 2026-06-21 | TC-TH-30 | 执行 `cargo fmt --all -- --check` 通过；执行 `cargo test -p bifrost-cli menu_bar_stats --lib --all-features -- --nocapture` 通过 5/5，覆盖 native 单行到参考两行转换、网络上下行同字号、48px native bitmap 非空和贯穿竖线。执行 `SKIP_FRONTEND_BUILD=1 cargo build --release --bin bifrost` 通过。使用临时数据目录 `/tmp/bifrost-native-stats.dOukVt`、端口 `62144` 启动真实 release 服务，主进程 PID `5871`、tray PID `5889`，启动参数包含 `--no-system-proxy --skip-cert-check`，并设置 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1` 与 `BIFROST_TRAY_NATIVE_STATS_VIEW=1`。`GET /_bifrost/api/config/tray` 返回系统状态支持且 CPU/Memory/Disk/Upload/Download 全部启用；`tray.log.2026-06-21` 记录 `native macOS tray stats view enabled`。连续截取 3 张完整顶部菜单栏截图 `/tmp/bifrost-native-stats-shots/topbar-1.png`、`topbar-2.png`、`topbar-3.png`，并生成局部 2x 对照图 `/tmp/bifrost-native-stats-shots/status-compare-1-2x.png`、`status-compare-2-2x.png`、`status-compare-3-2x.png`。对 tray helper 采集 60 秒空闲 CPU/RSS，记录 `/tmp/bifrost-native-stats-shots/tray-perf-60s.txt`。 | 通过。截图显示 native 两行状态项在完整菜单栏中常驻可见，和右侧参考项同屏对照；CPU 从 `10%` 刷新到 `5%`，网速从 `↑9.5 K/s ↓13.3 K/s` 刷新到 `↑8.3 K/s ↓18.8 K/s`、`↑8.9 K/s ↓15.3 K/s`，证明 1 秒刷新链路有效。两行布局稳定，列间竖线贯穿上下行，网络上下行位于同一列且字号一致，整体高度明显突破旧 `tray-icon` 18pt 位图限制。已知差异：实验路径当前保留原 Bifrost 菜单图标，因此 native stats 与菜单图标是两个相邻 `NSStatusItem`；点击 stats 文本本身不打开菜单。60 秒性能汇总为 `samples=60 avg_cpu=0.3200 max_cpu=1.4000 avg_rss_kb=71766 min_rss_kb=71536 max_rss_kb=71792 rss_delta_kb=256`，平均 CPU <1%，RSS 无显著增长。 |
| 2026-06-21 | TC-TH-29 | 对 Settings Tray 系统信息的 CPU、Memory、Disk、Upload、Download 子项逐项执行切换验证。先保持真实 release 实例 `/tmp/bifrost-tray-toggle.NX9uab`、端口 `62143`、主进程 PID `26574`、tray PID `26595` 运行；通过 `PUT /_bifrost/api/config/tray` 依次写入 `00_all_on`、`01_cpu_off`、`02_memory_off`、`03_disk_off`、`04_upload_off`、`05_download_off`、`06_restored_all_on`，每次写入后等待刷新并执行整条菜单栏截图，截图保存于 `/tmp/bifrost-tray-final-shots/toggles/00_all_on.png` 到 `/tmp/bifrost-tray-final-shots/toggles/06_restored_all_on.png`。 | 通过。截图显示菜单栏 status item 跟随配置变化：`01_cpu_off` 去掉 `C...`，`02_memory_off` 去掉 `M...`，`03_disk_off` 去掉 `D...`，`04_upload_off` 网络字段只剩 `↓...`，`05_download_off` 网络字段只剩 `↑...`，`06_restored_all_on` 恢复 `C... | M... | D... | ↑... ↓...`。Upload/Download 作为同一个网络字段渲染，内部仅空格分隔，没有额外竖线。当前浏览器 Settings 直连路由在重开 tab 时返回 426 导致表单区域白屏，因此本轮截图证据重点是菜单栏实际状态变化；配置侧以每次 `PUT` 返回的 `system_stats_items` JSON 与前序 E2E 配置脚本结果作为验证。 |
| 2026-06-21 | TC-TH-29 | 根据最新视觉反馈，将 macOS 菜单栏单行状态调整为 28px 常规字重系统字体，仅保留轻量横向叠画；列分隔线左右 gap 从 12px 降到 6px；Upload/Download 合并为一个网络字段，例如 `↑1.5 M/s ↓512 K/s`，中间只用小空格，不再用竖线分隔。执行 `git diff --check`、`cargo fmt --all -- --check`、`cargo test -p bifrost-cli menu_bar_stats --lib --all-features -- --nocapture`、`cargo test -p bifrost-cli system_stats --lib --all-features -- --nocapture`、`cargo clippy -p bifrost-cli --all-targets --all-features -- -D warnings` 均通过；执行 `cargo build --release --bin bifrost` 通过。使用临时目录 `/tmp/bifrost-tray-toggle.NX9uab`、端口 `62143` 启动真实 release 实例，主进程 PID `26574`、tray PID `26595`，启动参数包含 `--no-system-proxy --skip-cert-check` 且设置 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`。通过 `PUT /_bifrost/api/config/tray` 逐项切换 CPU、Memory、Disk、Upload、Download：`00-all-on`、`01-cpu-off`、`02-memory-off`、`03-disk-off`、`04-upload-off`、`05-download-off`、`06-restored-all-on`，每次 API 返回的 `system_stats_items` 均与目标一致。 | 自动化与配置链路通过；代码断言确认菜单栏字段会按子项过滤，且 Upload/Download 作为同一个网络列渲染，网络列内部不再产生竖线。尝试对每次切换执行真实桌面截图，输出 `/tmp/bifrost-tray-toggle-shots/00-all-on-top.png` 到 `/tmp/bifrost-tray-toggle-shots/06-restored-all-on-top.png`；但本轮 `screencapture` 返回黑帧，Computer Use 只能看到 Finder 左侧菜单栏、看不到右侧 status items，System Events 也无法读取 `SystemUIServer` menu bar items。因此这 7 张截图文件不可作为有效视觉证据，需要用户在当前保留运行的本地实例上人工目视确认，或重新授权/恢复终端屏幕捕捉后补截。warm-up 后 `ps` 显示主进程 CPU `0.1%`、RSS `50752KB`，tray 进程 CPU `0.1%`、RSS `69936KB`。 |
| 2026-06-21 | TC-TH-29 | 根据最新视觉反馈，将 macOS 菜单栏单行常驻状态字体从 24px 继续增大到 28px，基线调整到 31px，单行分隔线纵向范围调整为 3..33，尽量吃满 36px 透明模板图高度。执行 `cargo fmt --all -- --check` 通过；执行 `cargo test -p bifrost-cli menu_bar_stats --lib --all-features -- --nocapture` 通过 3/3；执行 `cargo build --release --bin bifrost` 通过。真实 release 实例使用临时目录 `/tmp/bifrost-single.senf14`、端口 `62142`、主进程 PID `1919`、tray PID `1944` 启动，启动参数包含 `--no-system-proxy --skip-cert-check` 且设置 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`。执行 `screencapture -x -R0,0,5120,260` 截取整条菜单栏顶部区域，截图为 `/tmp/bifrost-28px-shots/menu-top-1.png`；截图后 `ps -p 1919,1944 -o pid,ppid,%cpu,rss,command` 显示主进程 CPU `0.1%`、RSS `50960KB`，tray 进程 CPU `0.5%`、RSS `69728KB`。 | 通过。截图中左侧 Bifrost 单行状态项无需点击下拉即可直接展示 `C...% | M...% | D...% | ↑... ↓...`，字体比上一轮明显变大并接近右侧旧版参考项，未观察到文字裁剪；CPU/Memory/Disk 继续使用 `C/M/D` 前缀，Upload/Download 继续使用上下箭头且不左侧补零。当前本地实例保留运行，供用户回到 Mac 后继续人工目视确认。 |
| 2026-06-21 | TC-TH-29 | 根据最新视觉反馈，将 macOS 菜单栏常驻状态从两行布局回归为单行高字号布局：CPU/Memory/Disk 分别显示为 `C/M/D` 前缀，Upload/Download 使用 `↑/↓`，例如 `C20% | M55% | D55% | ↑1.5 M/s ↓512 K/s`；去除百分比和网速的左侧补零，稳定宽度改由渲染器按 `100%` 与 `999.9 M/s` 预留列宽负责。执行 `cargo fmt --all -- --check` 通过；执行 `cargo test -p bifrost-cli system_stats --lib` 通过 24/24；执行 `cargo test -p bifrost-cli menu_bar_stats --lib` 通过 3/3；执行 `cargo build --release --bin bifrost` 通过。真实 release 实例使用临时目录 `/tmp/bifrost-single.senf14`、端口 `62142`、主进程 PID `22322`、tray PID `22342` 启动，启动参数包含 `--no-system-proxy --skip-cert-check` 且设置 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`。 | 代码与自动化验证通过。单行方案避免两行文本在 macOS 18pt status item 高度中被压缩，网速使用真实接口字节差分格式化，不再因量化显示成 `0B/s`。本轮 `screencapture` 在授权后仍出现间歇黑帧，computer-use 只能看到应用窗口/桌面缩略图，无法稳定截取右侧系统状态栏细节；因此桌面视觉仍需用户在当前保留的本地实例上人工目视确认，或后续重新授权/重启截屏服务后补齐连续 1 秒截图证据。 |
| 2026-06-21 | TC-TH-29 | 针对 macOS 菜单栏常驻资源状态的最终视觉与性能要求重新执行。先停止上一轮测试实例，执行 `cargo build --release --bin bifrost` 通过；使用临时数据目录 `/tmp/bifrost-final-sep.mbZloF` 启动真实 release 服务，端口 `62141`，主进程 PID `20614`，tray PID `20652`，启动参数包含 `--no-system-proxy --skip-cert-check` 且设置 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`。本机同时存在用户旧软件 `/Users/eden_studio/work/github/bifrost-videos-tool/.bifrost-ui-target/debug/bifrost`，因此截图时按左侧带 Bifrost `b` 图标的状态项裁剪，排除右侧旧软件干扰。执行 `screencapture` 连续截取 3 张 1 秒间隔菜单栏截图并放大裁剪：`/tmp/bifrost-final-sep-shots/bifrost-1-3x.png`、`/tmp/bifrost-final-sep-shots/bifrost-2-3x.png`、`/tmp/bifrost-final-sep-shots/bifrost-3-3x.png`；截图期间通过 `curl https://speed.cloudflare.com/__down?bytes=12000000` 产生真实系统网络流量。执行 60 个 1 秒 `ps` 样本采集，记录 `/tmp/bifrost-final-sep-shots/tray-perf-60s.txt`。 | 通过。正确的左侧 Bifrost 菜单栏项无需点击下拉即可常驻展示两行状态：第一行展示 `CPU/MEM/SSD/↑.../s` 数值，第二行展示 `CPU/MEM/SSD/↓.../s` 缩写与下行网速；网络流量下截图从 `↑000B/s ↓000B/s` 刷新到 `↑512K/s`、`↑006M/s ↓256K/s`，证明 1 秒刷新链路与上下行两排展示有效。列间分隔线由渲染器绘制为贯穿两行的连续竖线，不再是上下两行独立 `|` 字符；字体粗细接近参考系统监控项。60 秒性能汇总为 `samples=60 avg_cpu=0.1767 max_cpu=2.0000 avg_rss_kb=70457 min_rss_kb=70288 max_rss_kb=70688 rss_delta_kb=400`，平均 CPU 低于 1%，RSS 无显著增长。 |
| 2026-06-21 | TC-TH-29 | 针对 CPU/Memory 采样实时性从 3 秒提高到 2 秒重新执行全启用性能采样。执行 `SKIP_FRONTEND_BUILD=1 cargo build --release --bin bifrost` 构建当前 release 二进制；使用临时数据目录 `/tmp/bifrost-tray-cpu-2s-all.sSz1Bp` 启动真实 macOS tray helper，端口 `52047`，主进程 PID `94814`，tray PID `94863`；启动参数包含 `--no-system-proxy --skip-cert-check` 且设置 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`，保留 Tray 启用。通过 `GET /_bifrost/api/config/tray` 确认返回 `{"enabled":true,"supported":true,"system_stats_supported":true,"show_system_stats":true,"system_stats_items":{"cpu":true,"memory":true,"disk":true,"upload":true,"download":true}}`，即 CPU/Memory/Disk/Upload/Download 全部启用。warm-up 15 秒后对 tray helper 采集 120 个 1 秒 `ps` CPU/RSS 样本，样本文件为 `/tmp/bifrost-tray-cpu-2s-all.sSz1Bp/cpu_samples.txt`，汇总文件为 `/tmp/bifrost-tray-cpu-2s-all.sSz1Bp/summary.txt`。 | 通过。CPU/Memory 内部刷新间隔提高到 2 秒后，全启用真实 tray helper 性能为 `samples=120 avg_cpu=0.0625 min_cpu=0.0000 max_cpu=0.7000 over_1=0 over_1_5=0 nonzero=28 avg_rss_kb=61342`；平均 CPU 仍远低于 1%，没有任何样本超过 1% 或 1.5%，说明 2 秒 CPU/Memory 实时性提升没有破坏既定性能目标。 |
| 2026-06-21 | TC-TH-29 | 针对“全部系统信息展示启用”状态重新执行最终性能采样。先构建当前 release 二进制并启动真实 macOS tray helper，临时 artifact 目录为 `/tmp/bifrost-tray-cpu-full-staticicon.Rh9qaE`，端口 `57210`，主进程 PID `88952`，tray PID `88988`；通过 `GET /_bifrost/api/config/tray` 确认返回 `{"enabled":true,"supported":true,"system_stats_supported":true,"show_system_stats":true,"system_stats_items":{"cpu":true,"memory":true,"disk":true,"upload":true,"download":true}}`，即 CPU/Memory/Disk/Upload/Download 全部启用。warm-up 15 秒后对 tray helper 执行 120 个 1 秒 `ps` CPU/RSS 样本采集，采样结果为 `samples=120 avg_cpu=0.0467 min_cpu=0.0000 max_cpu=0.7000 over_1=0 over_1_5=0 nonzero=25 avg_rss_kb=61388`。本轮代码同时确认系统状态文本变化不再周期性调用 macOS `set_icon`，菜单栏图标只在启动、服务状态变化和 show/hide 切换时更新，以避免 AppKit status item 位图重设造成 2-3% 瞬时 CPU 峰值。 | 通过。所有系统信息展示均启用时，真实 tray helper 平均 CPU 为 `0.0467%`，最大 CPU 为 `0.7000%`，没有任何样本超过 `1%` 或 `1.5%`；满足用户要求的 “1% 以内，不能超过 1.5%” 性能目标。系统状态采样保持约 3 秒菜单层刷新，CPU/Memory 内部缓存提高到最多每 2 秒刷新，Disk/Upload/Download 数据继续更新到菜单数据层；为达成稳定低 CPU，周期性系统状态文本不再驱动 macOS status item icon 重绘，避免性能尖峰。 |
| 2026-06-21 | TC-TH-29 | 针对网速计算方式重新评估并补充回归。读取当前实现确认网速来源为 `sysinfo::Networks` 的接口累计 `total_received()` / `total_transmitted()`，按 `Instant` 单调时间做差分；执行 `route -n get default` 得到默认路由接口 `en1`；执行 `netstat -ibn` 与 `ifconfig` 发现本机同时存在 `utun5`、`bridge100/101`、`vmenet0/1`、`awdl0`、`llw0` 等 VPN/虚拟机/本地链路接口，验证“所有接口累加”会混入非用户直觉的流量。执行 `nettop -m route -t external -d -x -L 3 -s 1` 与 `netstat -ibn -I en1; sleep 3; netstat -ibn -I en1` 对照，确认系统工具也以累计字节计数和 delta 口径展示单位时间吞吐。代码补充 IPv6 默认路由 fallback，并在默认路由接口变化时重置网络累计基线和平滑值。执行 `cargo fmt --all` 通过；执行 `cargo test -p bifrost-cli system_stats --lib` 通过 23/23，新增覆盖默认路由解析过滤虚拟接口、首选接口变化重置网络基线。 | 通过。最终网速算法选择“默认路由接口优先的内核累计字节差分 / 单调时间”，找不到默认路由时才回退到最活跃非虚拟物理接口；不用 Bifrost 代理流量、不用 per-process 统计、不累加所有接口。3 秒采样窗口配合 60/40 EMA 平滑，更符合菜单栏人眼观察稳定性；接口变化时丢弃旧平滑值，避免 Wi-Fi/有线/VPN 切换后的残留速度。 |
| 2026-06-21 | TC-TH-29 | 针对性能极致化和字体加粗补充验证。实现上将 CPU/Memory 采样限定为对应子项启用时才刷新，Disk 采样从 3 秒降频到 30 秒，默认路由/网卡列表刷新从 30 秒降频到 60 秒，Upload/Download 均关闭时继续跳过网络采样并重置基线；macOS 菜单栏文字从 2-pass faux-bold 增加到 3-pass faux-bold。执行 `cargo fmt --all -- --check` 通过；执行 `cargo test -p bifrost-cli system_stats --lib` 通过 21/21，新增覆盖只启用网络时不刷新 CPU/Memory/Disk、Disk 未到 30 秒窗口不刷新且到期后刷新；执行 `cargo test -p bifrost-cli menu_bar_stats --lib` 通过 3/3；执行 `cargo build --release --bin bifrost` 通过。真实 release 性能采样使用临时目录 `/tmp/bifrost-tray-perf.JzmLJs`、端口 `59659`、主进程 `81783`、tray PID `81816`；warm-up 10 秒后采样 60 秒，记录 `/tmp/bifrost-tray-perf-cpu-1781978487.txt`，结果 `samples=60 avg=0.5167 max=3.0000 min=0.0000`，采样结束时 `ps` 当前 CPU `0.7%`、RSS `68064KB`。 | 通过。release 真实 tray helper 平均 CPU 明确低于 1%，符合系统状态常驻展示的性能目标；短瞬时峰值仍可能出现，但平均占用稳定在 0.5% 左右。字体已进一步加粗，实时性保持为菜单栏系统状态每 3 秒更新，网速仍每 3 秒按默认路由接口累计字节差分刷新；低频的磁盘和网卡列表刷新被降频以减少 I/O 和路由查询开销。 |
| 2026-06-21 | TC-TH-29 | 针对菜单栏高度、下行宽度、稳定网速算法和系统状态逐项开关重新执行。执行 `cargo fmt --all -- --check` 通过；执行 `cargo test -p bifrost-storage unified_config --lib` 通过 7/7，覆盖 `system_stats_items` 默认全开和部分 TOML 配置未声明字段仍默认开启；执行 `cargo test -p bifrost-admin tray_config --lib` 通过 5/5，覆盖 `GET /api/config/tray`、空 payload 拒绝、`show_system_stats` 单独更新和 `system_stats_items.download` 单独更新；执行 `cargo test -p bifrost-cli system_stats --lib` 通过 20/20，覆盖默认路由优先、活跃接口 hysteresis、虚拟/VPN/bridge 接口过滤、短采样窗口抑制、EMA 平滑、逐项菜单栏过滤、等宽字段和 Upload/Download 均关闭时重置网络累计基线；执行 `cargo test -p bifrost-cli menu_bar_stats --lib` 通过 3/3，覆盖 78px bitmap、50px 等宽字体和 Running-only 标题；执行 `cargo test -p bifrost-admin openapi --lib` 通过 2/2；执行 `pnpm --dir web exec tsc -b` 通过；首次执行 `pnpm --dir web exec playwright test tests/ui/admin-settings.spec.ts -g "Settings Tray tab" --reporter=line` 因本机缺少 Playwright Chromium 失败，执行 `pnpm --dir web exec playwright install chromium` 后重跑通过 1/1，覆盖 Tray tab 总开关、Download 子开关和移动端无横向溢出；执行 `cargo build --bin bifrost` 通过；执行 `BIFROST_BIN="$PWD/target/debug/bifrost" bash e2e-tests/tests/test_tray_system_stats_config.sh` 通过 22/22；执行 `BIFROST_BIN="$PWD/target/debug/bifrost" SKIP_BUILD=true bash e2e-tests/tests/test_cli_tray_startup_ci.sh` 通过，输出 `PASS: tray helper started on Darwin; port=18703 tray_pid=47093`。真实 macOS 可视验证使用临时目录 `/tmp/bifrost-menubar-final3.cvZBVm`、端口 `62105`、tray PID `49170`，默认 API 返回 `{"enabled":true,"supported":true,"show_system_stats":true,"system_stats_items":{"cpu":true,"memory":true,"disk":true,"upload":true,"download":true}}`，默认路由接口 `en1`；截图 `/tmp/bifrost-menubar-final3-1.png` 显示 78px/50px 菜单栏状态图标和文字高度，下行字段完整可见；`PUT {"system_stats_items":{"download":false}}` 返回 `download=false` 后截图 `/tmp/bifrost-menubar-final3-download-off.png` 确认菜单栏移除 `↓...`；恢复 `download=true` 后截图 `/tmp/bifrost-menubar-final3-download-on.png` 确认下行字段恢复且完整；空闲 CPU 30 秒采样记录 `/tmp/bifrost-menubar-final3-cpu-idle.txt`，汇总 `samples=30 avg=0.4533 max=3.5000`。 | 通过。当前实现菜单栏 bitmap 高度和字体高度已明显增加，图标不因启用系统状态而缩小，`↓nnnU/s` 下行字段完整显示；系统状态总开关与 CPU/Memory/Disk/Upload/Download 子开关均可独立配置且默认全开；网速使用默认路由接口 `en1` 的累计字节差分，结合虚拟接口过滤、最小采样间隔、接口 hysteresis 和 60/40 EMA 平滑，避免汇总所有网卡造成 VPN/bridge/虚拟网卡双算和尖峰；关闭单个子项只移除对应字段，不影响其它指标。临时前台实例和 orphan tray helper 已清理。Windows VM 真实图形验收本轮未执行，仍需后续接 Windows 环境验证 notification area 降级详情。 |
| 2026-06-21 | TC-TH-29 | 接手挂起的 TRAY 任务后补齐固定宽度、等宽字体、磁盘百分比、图标大小和 CPU <1% 验证。执行 `cargo fmt --all -- --check` 通过；执行 `cargo test -p bifrost-cli system_stats --lib` 通过 13/13，覆盖固定宽度 `C05% | M09% | D09% | ↑008K/s↓026K/s`、磁盘百分比、虚拟/VPN/bridge 接口过滤、短采样窗口抑制和未知值兜底；执行 `cargo test -p bifrost-cli menu_bar_stats --lib` 通过 3/3，覆盖 macOS Running-only 标题、菜单栏 bitmap 非空和 `000/111/888` 等宽数字宽度；执行 `BIFROST_BIN="$PWD/target/debug/bifrost" bash e2e-tests/tests/test_tray_system_stats_config.sh` 通过 11/11；执行 `BIFROST_BIN="$PWD/target/debug/bifrost" SKIP_BUILD=true bash e2e-tests/tests/test_cli_tray_startup_ci.sh` 通过，输出 `PASS: tray helper started on Darwin`。重建等宽字体验收实例：数据目录 `/tmp/bifrost-menubar-mono.42um5h`，端口 `50839`，daemon PID `7930`，tray PID `7967`，`GET /_bifrost/api/config/tray` 返回 `{"enabled":true,"supported":true,"show_system_stats":true}`；连续截图保存到 `/tmp/bifrost-menubar-mono-1.png`、`/tmp/bifrost-menubar-mono-2.png`、`/tmp/bifrost-menubar-mono-3.png`。纯空闲 CPU 采样 40 秒记录在 `/tmp/bifrost-menubar-mono-cpu-idle2.txt`。 | 通过。当前实现使用等宽字体渲染右侧状态文本，数字宽度稳定；百分比固定两/三位空间，网速固定三位数字加单位；macOS bitmap 左侧 Bifrost 图标占满模板高度，避免启用系统状态后图标显著变小。纯空闲 CPU 40 个 1 秒采样平均 `0.8100%`、最大 `4.2000%`，满足 <1% 平均目标；截图并发采样平均 `1.7167%` 被记录为 `screencapture`/WindowServer 干扰，不作为空闲指标。Windows VM 真实图形验收尚未在本轮执行，需后续接 Windows 环境验证 notification area 菜单详情。 |
| 2026-06-20 | TC-TH-29 | 针对系统状态从下拉菜单改为 macOS 菜单栏常驻展示，执行 `cargo test -p bifrost-cli system_stats --lib` 通过 11/11，覆盖 `menu_bar` 两行紧凑文案、网络 collecting 文案、未知内存总量兜底、macOS Running-only 状态、macOS 下拉菜单隐藏资源两行和菜单构建；执行 `cargo test -p bifrost-cli menu_bar_stats_bitmap --lib` 通过 1/1，确认小字号 bitmap 高度 36px 且非空；执行 `BIFROST_BIN="$PWD/target/debug/bifrost" bash e2e-tests/tests/test_tray_system_stats_config.sh` 通过 11/11，确认默认开启、独立关闭/开启和配置持久化；执行 `BIFROST_BIN="$PWD/target/debug/bifrost" SKIP_BUILD=true bash e2e-tests/tests/test_cli_tray_startup_ci.sh` 通过，输出 `PASS: tray helper started on Darwin`。按验收要求重建并保留小字号 daemon 实例：数据目录 `/tmp/bifrost-menubar-small.qG5J2N`，端口 `61460`，daemon PID `49547`，tray PID `49587`。 | 部分通过。当前代码把 macOS menu bar status item icon 渲染为小字号两行模板图像：第一行 `CPU% | MEM% | ↑上行速率`，第二行 `CPU | MEM | ↓下行速率`，符合无需展开下拉菜单即可看数值和类型缩写的验收方向；Mac 下拉菜单不再重复展示资源信息；daemon 与 tray helper 已保留供人工目视验收。当前 System Events 查询在本机超时，PNG 截图仍受 macOS Screen Recording/TCC 权限阻塞，需要用户目视确认或授权后补截连续截图。 |
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
| 2026-06-14 | TC-TH-21 / TC-TH-24 | Parallels Windows 11 真实现状排查：`bifrost --version` 为 `0.0.100`，`config.toml` 中 `[tray] enabled = true`；前台 `bifrost.exe start` 进程存活且 `runtime_start_mode` 为 `foreground`，但数据目录只有 `tray.lock`、没有 `tray.pid`，系统中没有 `bifrost.exe __tray` helper。日志多次出现主进程 `tray helper launched` 与 helper `bifrost-tray starting data_dir=... parent_pid=...`，随后 helper 退出。代码路径确认 Windows `main()` 原先把 `run_if_tray_process()` 放在 `bifrost-cli-main` worker 线程里，导致原生 tray event loop 没有在进程主线程运行。 | 已修复代码入口：Windows `main()` 最早阶段先执行 `commands::tray::run_if_tray_process()`，普通 CLI 再进入大栈 worker；同时收紧 `test_cli_tray_startup_ci.sh`，默认不再允许 Windows log-only fallback。固定后 Windows VM 本地编译验证暂阻塞：该 VM 缺少 MSVC linker，`cargo build --bin bifrost` 报 `linker lld-link not found` / `link.exe was not found`，需要安装 Visual Studio Build Tools 或由 Windows CI 验证。 |
| 2026-06-14 | TC-TH-25 | 用户在 Windows 11 上真实复现：托盘图标已经出现，但从托盘菜单 Stop 主服务后，再从同一托盘点击 Start 无法成功启动。代码路径确认托盘 `StartService` 原先执行 `bifrost start --no-tray --no-system-proxy` 前台启动，并由 tray helper 线程等待子进程；在 Windows tray helper 发起的服务控制路径中，前台子进程生命周期和 helper 会话绑定过紧，容易在 Stop 后 Start 被判失败或无法维持服务。 | 已修复为托盘 Start 使用 `bifrost start --daemon --no-tray --no-system-proxy`，并允许 daemon parent 成功退出后继续等待 `runtime.json` 指向活主服务；本地执行 `cargo test -p bifrost-cli commands::tray::tray::tests::test_tray_start_service_uses_detached_daemon -- --nocapture` 通过。 |
| 2026-06-15 | TC-TH-26 | Windows 11 真实复现：运行中的 Bifrost 主服务 PID `4480` 和托盘 helper PID `2500` 存活时，进程采样抓到主服务每秒左右创建 `reg.exe query HKCU\Software\Microsoft\Windows\CurrentVersion\Internet Settings /v ProxyEnable|ProxyServer|ProxyOverride`，随后 Windows 自动创建 `conhost.exe`、`OpenConsole.exe`、`WindowsTerminal.exe`，表现为终端窗口反复弹出又关闭。代码路径确认托盘后台 `poll_menu_data` 每 1 秒刷新菜单快照并请求 `/api/proxy/system`，Admin handler 经 `SystemProxyManager::get_current()` 读取系统代理。修复后补充采样发现用户点击 `Open Logs` / `Open Admin UI` 时 `open::that()` 仍会通过 `cmd /c start` 拉起 console 子进程；代码复核发现 Sync 启动自动登录提示也存在同类 `cmd /C start` 偶发路径。 | 已修复为托盘后台刷新不再请求 System Proxy；仅在托盘交互或 System Proxy 开关后按需刷新并缓存该状态。Windows `parse_windows_proxy()` 改用 HKCU registry API 读取 `ProxyEnable` / `ProxyServer` / `ProxyOverride`，不再 spawn `reg.exe`。Windows 托盘打开 URL/目录和 Sync 自动登录打开浏览器均改为 Win32 `ShellExecuteW`，不再通过 `cmd /c start`。本地执行 `cargo test -p bifrost-cli tray -- --nocapture` 通过，包含 `test_background_menu_refresh_preserves_system_proxy_cache`；Windows VM 待替换二进制后执行进程采样复验。 |
| 2026-06-15 | TC-TH-27 | Windows 11 真实复现：最新二进制执行 `target\debug\bifrost.exe start -d -p 9900 --unsafe-ssl --skip-cert-check --no-system-proxy` 后，后台主服务 PID `5148` 存活，但没有自动出现 `bifrost.exe __tray` helper；必须手动 `Start-Process ... __tray ...` 才能看到托盘。代码路径确认 foreground 初始化完成后会调用 `tray_launch_callback()`，但该 callback 在 daemon child 中由 `no_tray || detached_daemon_child` 构造，导致 `BIFROST_DETACHED_DAEMON_CHILD=1` 的长期主服务进程永远跳过 tray。 | 已修复为 daemon child 不再抑制启动 tray；只有显式 `--no-tray` 或配置禁用 tray 才跳过。新增单元测试 `daemon_child_does_not_suppress_startup_tray` 覆盖该行为；Windows VM 待替换二进制后执行 `start -d` 自动托盘复验。 |
| 2026-06-15 | TC-TH-25 | Windows 11 真实复现：用户从托盘 Stop 主服务后，同一托盘点击 `Start Bifrost` 多次只短暂创建 `bifrost.exe` 子进程，随后 exit code 1；`tray.log.2026-06-15` 记录 `bifrost service started pid=...` 后 `bifrost start exited before service became ready status=exit code: 1`。用同参直接执行 `target\debug\bifrost.exe start --daemon --no-tray --no-system-proxy -p 9900 --skip-cert-check --unsafe-ssl` 复现 `Daemon exited before the proxy listener became ready`。代码路径确认 Windows/macOS exec daemon child 继承原始 `--daemon` 参数后没有识别 `BIFROST_DETACHED_DAEMON_CHILD=1`，会再次进入 daemon parent 启动器，而不是进入长期 runtime。 | 已修复为 detached daemon child 不再 spawn 新 daemon parent，而是直接执行 runtime；新增单元测试 `detached_daemon_child_runs_runtime_instead_of_spawning_again` 覆盖该分支。Windows VM 待替换二进制后复验托盘 Stop -> Start。 |
| 2026-06-20 | TC-TH-28 | Parallels Windows 11 真实复现：`tray.log.2026-06-20` 显示 05:42 与 05:43 两次从托盘触发 `bifrost self-update spawned target=0.0.111`，当时 `tray helper launched tray_bin=C:\Users\eden_studio\.local\bin\bifrost.exe`；`.local\bin\.bifrost-upgrade-3244.log` 和 `.bifrost-upgrade-8220.log` 均记录 `replacing C:\Users\eden_studio\.local\bin\bifrost.exe` 后 `Access is denied`，且 `.bifrost.exe.pending.*` 残留。与此同时 `upgrade-progress.json` 错误显示 `phase: completed`，而 `.local\bin\bifrost.exe --version` 仍为 `0.0.110`。 | 已定位根因：Windows 延迟替换 helper 只等待 self-update 父进程退出，没有停止/等待同一 exe 上运行的 tray helper，导致目标 exe 被锁；同时 Rust 父进程在 helper 实际替换前写入 completed，造成假成功。已更新实现：调度 helper 前停止 tray helper，helper 等目标 exe 可写，成功/失败由 helper 写入 terminal progress。已执行日志/版本/残留文件只读验证；修复后二进制待 Windows VM 重建后按本用例复验。 |
| 2026-06-20 | TC-TH-29 | 历史下拉菜单方案执行记录：Tray 系统状态两排展示与 Settings 开关新增验证。macOS 本地执行 `cargo test -p bifrost-cli system_stats --lib` 通过 8/8，覆盖 system/network 两个 disabled menu rows、关闭后隐藏、CPU/Memory/Up/Down 文案格式、Windows/macOS loopback 过滤、网络首帧 collecting、按累计计数和实际 elapsed 计算速率、计数回退跳过；执行 `BIFROST_BIN="$PWD/target/debug/bifrost" bash e2e-tests/tests/test_tray_system_stats_config.sh` 通过 11/11，验证 `enabled=true`、`show_system_stats=true` 默认值、独立关闭/开启与 `config.toml` 持久化；执行 `BIFROST_BIN="$PWD/target/debug/bifrost" SKIP_BUILD=true bash e2e-tests/tests/test_cli_tray_startup_ci.sh` 真实拉起 Darwin tray helper（最新执行 port 16751，tray_pid 74797）。Web UI 截图验证先在桌面与 390px 移动端发现移动端 `Show System Stats` 开关换行偏左，已改为稳定 flex 布局；复跑临时截图验证后桌面/移动端 `scrollWidth` 分别等于 viewport，移动端两个开关 `left=317` 且默认 `aria-checked=true`，随后正式 Playwright 用例通过。macOS 桌面 tray 使用唯一进程名 `/tmp/bifrost-tray-visual-bin` 真实启动（port 62832，tray_pid 69739），System Events 连续读取原生菜单：17:35:04 `Network: Up 162 KB/s | Down 621 KB/s`，17:35:05 `Network: Up 425 KB/s | Down 1.1 MB/s`，17:35:07 `Network: Up 64.5 KB/s | Down 113 KB/s`，17:35:08 `Network: Up 300 KB/s | Down 770 KB/s`，证明菜单两排存在且网络 1 秒级刷新。性能基线：网速每 1 秒刷新，CPU/内存每 3 秒刷新；`sysinfo 0.31.4` release microbench 连续 500 次采样平均 1.1796ms、最大 1.8934ms；Mac debug tray helper warm-up 5 秒后采样 20 秒，开启系统状态平均 CPU 0.4700%、RSS 67,989KB，关闭系统状态平均 CPU 0.0600%、RSS 67,076KB。随后用 Parallels Windows 11 原 repo 外的隔离目录 `C:\Users\eden_studio\work\github\bifrost-tray-system-stats-win` 验证，不污染 VM 原脏目录；执行 `cargo test -p bifrost-cli system_stats --lib` 通过 8/8；`SKIP_FRONTEND_BUILD=1` 重新编译 `target\debug\bifrost.exe` 成功；真实交互用户 session 启动临时实例（port 61038，main pid 10012，tray_pid 4056），`GET /api/config/tray` 默认 `show_system_stats=true`。 | 部分通过，剩余桌面 PNG 截图被系统权限阻塞。网速默认 1 秒刷新，CPU/内存默认 3 秒刷新，在实时性和性能之间取低开销方案；网速用系统原生累计字节计数按接口计算差值，避免首帧假 0、接口重建或计数回退造成虚高速率。Web UI 截图验证通过并修复了移动端布局问题；macOS 原生菜单辅助功能文本验证证明 1 秒刷新正确。未完成项：macOS `screencapture` 无法创建图片，ScreenCaptureKit 返回 TCC 拒绝屏幕捕捉；Windows VM 截图被 Windows Defender Firewall 弹窗遮挡，未在未授权情况下点击 Allow。需要用户授予 macOS Screen Recording 权限，并确认 Windows VM 允许点击防火墙 Allow 后，才能补齐桌面 tray 多张 PNG 截图验收。 |

## 清理步骤

```bash
cargo run --bin bifrost -- stop
rm -rf ./.bifrost-tray-test
```
