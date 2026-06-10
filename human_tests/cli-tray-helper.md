# CLI 托盘 Helper 真实场景测试

## 功能模块说明

验证 `bifrost` 内置 `__tray` 托盘 helper 在 macOS/Windows 上的完整生命周期：CLI 自动拉起、托盘图标显示、默认菜单操作、Rules 快速切换、自定义菜单加载、单实例保护、服务停止后状态变化、状态轮询刷新、可靠性回归，以及 `--no-tray` / `BIFROST_DISABLE_TRAY=1` 的禁用行为。

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
- 点击图标本身不会立即产生 `tray icon and menu updated` 日志；菜单重建只应由状态变化、菜单动作、显式 reload 或规则轮询变化触发

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

### TC-TH-13: Rules 菜单仅个人规则时展示两级并支持单选

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
7. 通过 Admin API 删除 `tray-personal-a`，等待最多 2 秒后再次展开 Rules 子菜单

**预期结果：**
- 无规则时 Rules 入口不会消失，而是显示 `Rules: None` + `No rules available`
- 顶层菜单存在 `Rules: tray-personal-a`
- 只有个人规则时，Rules 下一级直接展示 `tray-personal-a` 和 `tray-personal-b`，不出现 `My Rules` 或组名层级
- `tray-personal-a` 初始带原生勾选标记，`tray-personal-b` 初始不勾选
- 点击 `tray-personal-b` 后，顶层文案更新为 `Rules: tray-personal-b`
- `active-summary` 只包含 `tray-personal-b`，不包含 `tray-personal-a`
- 删除 `tray-personal-a` 后，Rules 子菜单不再包含 `tray-personal-a`，仍包含 `tray-personal-b`
- 准备、读取和切换均通过 Admin API 完成，没有直接编辑规则文件

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
- 脚本自行构建 `bifrost` release binary
- 主服务 Admin API `/_bifrost/api/proxy/address` 在临时端口 ready，响应包含本次端口
- `runtime.json` 存在，且其中的 `port` 等于本次临时端口、`pid` 为有效进程 ID
- 数据目录优先生成 `tray.pid`，且对应 helper 进程存活；Windows runner 若 `tray.pid` 缺失或 helper 进程短暂启动后退出，但 `logs/tray.log*` 已包含启动标记，可按 log-only fallback 通过
- `logs/tray.log*` 包含 `bifrost-tray starting`
- 脚本结束时停止主服务、杀掉 helper，并清理临时数据目录

## 本次执行记录

| 日期 | 用例 | 执行方式 | 结果 |
| --- | --- | --- | --- |
| 2026-06-11 | TC-TH-02-REG-01 / TC-TH-21 | 针对 PR CI run `27305425195` 的 macOS shell shard 1 超时补充验证：失败 artifact 显示 `test_cli_foreground_ctrlc_no_enter.sh` 已输出 `PASS: foreground Ctrl-C stops without an extra Enter`，`test_cli_tray_menu_click_regression.sh` 卡在 shard 内自行 `cargo test -p bifrost-cli pure_tray_icon_event_does_not_rebuild_native_menu` 的冷编译/下载阶段。修复后脚本在 `SKIP_BUILD=true` 时跳过该 unit guard，并复用 `BIFROST_BIN` 或 `target/release/bifrost`，保留真实 macOS tray helper 启动、`tray.pid`、`tray.log` 和纯图标点击不重建菜单的日志断言。 | 本地执行 `SKIP_BUILD=true BIFROST_BIN=/Users/eden/work/github/bifrost-tray-helper-design/target/debug/bifrost bash e2e-tests/tests/test_cli_tray_menu_click_regression.sh` 通过，输出 `PASS: tray helper launched and pure icon interaction rebuild guard is active`；CI 待重跑确认 |

## 清理步骤

```bash
cargo run --bin bifrost -- stop
rm -rf ./.bifrost-tray-test
```
