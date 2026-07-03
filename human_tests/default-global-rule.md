# 全局 Default 规则真实场景测试

## 功能模块说明

验证 `Default` 全局默认规则的初始化、保护、Web UI 交互、CLI 交互、Admin API 行为，以及它在主代理端口和临时端口上的兜底生效能力。

## 前置条件

- 使用隔离数据目录：`export BIFROST_DATA_DIR="$(mktemp -d)"`。
- 启动服务必须禁用系统代理、托盘和 Sync 自动登录弹窗：
  - `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`
  - `BIFROST_DISABLE_TRAY=1`
  - `cargo run -p bifrost-cli --bin bifrost -- start --port <MAIN_PORT> --no-system-proxy --skip-cert-check`
- Web UI 打开 `http://127.0.0.1:<MAIN_PORT>/_bifrost/`。
- 所有测试结束后执行 `bifrost stop` 并删除临时数据目录。

## 测试用例列表

### TC-DGR-01 CLI 自动初始化与保护展示

操作步骤：
1. 在隔离数据目录下执行 `bifrost rule list`。
2. 执行 `bifrost rule show Default`。
3. 执行 `bifrost rule disable Default`。
4. 执行 `bifrost rule delete Default`。
5. 执行 `bifrost rule add Default --content "example.test status://200"`。
6. 执行 `bifrost rule add default --content "example.test status://200"`。
7. 删除磁盘上的 `<BIFROST_DATA_DIR>/rules/Default.bifrost` 后重启服务或再次执行 `bifrost rule list`。

预期结果：
- `rule list` 输出包含 `Default [enabled, global, protected]`，且 Default 位于列表第一项。
- `rule show Default` 输出包含 `Scope: global default` 和保护说明。
- disable/delete/add 均返回非 0，错误说明 Default 不能被停用、删除或作为普通规则创建；`default`、`DEFAULT` 等大小写变体也被视为保留名。
- `Default.bifrost` 被删除后会自动恢复，恢复后的 Default 仍为 enabled 且仍置顶。

### TC-DGR-02 Admin API 能力字段与内容可编辑

操作步骤：
1. `GET /_bifrost/api/rules`。
2. `GET /_bifrost/api/rules/Default`。
3. `PUT /_bifrost/api/rules/Default`，body 为 `{"content":"global-default.test status://218 resBody://(global-default)"}`。
4. `PUT /_bifrost/api/rules/Default`，body 为 `{"enabled":false}`。
5. `DELETE /_bifrost/api/rules/Default`。
6. 尝试创建或重命名为 `default`。

预期结果：
- 列表第一项为 `Default`，`enabled=true`，`is_global_default=true`，`can_delete=false`，`can_disable=false`，`can_rename=false`，`can_reorder=false`。
- 更新内容成功，重新读取详情时内容包含 `global-default.test`，且仍保持 enabled true。
- 设置 `enabled=false` 和删除均返回 400。
- 创建或重命名为 `default` 返回错误，不产生同名规则文件。

### TC-DGR-03 Web UI 交互保护与编辑

操作步骤：
1. 打开 Rules 页面。
2. 确认左侧第一条规则为 `Default`。
3. 尝试点击 Default 的启用开关、双击 Default 行、右键 Default 行。
4. 打开 Default 编辑器，修改内容为 `web-default.test status://218 resBody://(web-default)` 并保存。

预期结果：
- Default 始终置顶，开关禁用，右键菜单不显示 Disable、Rename、Delete。
- 编辑器不显示 Delete 按钮，但显示 Save 按钮。
- 保存后成功提示出现，重新读取 API 可看到新内容。

### TC-DGR-04 Web UI 从 Default 深链进入后可切换其它规则

操作步骤：
1. 新增普通规则 `SwitchTest`，内容为 `switch-test.local status://219`。
2. 在浏览器打开 `http://127.0.0.1:<MAIN_PORT>/_bifrost/rules?rule=Default`。
3. 确认编辑器标题为 `Default`。
4. 点击左侧规则列表中的 `SwitchTest`。
5. 观察编辑器标题、左侧选中态和浏览器地址栏。

预期结果：
- 页面初始按 URL 参数选中 `Default`。
- 点击 `SwitchTest` 后编辑器标题切换为 `SwitchTest`，左侧选中态也切换到 `SwitchTest`。
- 浏览器地址栏中的 query 更新为 `rule=SwitchTest`。
- 页面不会被旧的 `rule=Default` 参数重新拉回 Default。

### TC-DGR-05 主端口全局生效

操作步骤：
1. 将 Default 内容设置为 `global-default.test status://218 resBody://(global-default)`。
2. 启动主代理端口。
3. 通过主代理请求 `http://global-default.test/main`。
4. 新增普通规则 `main-only.test status://219 resBody://(main-only)`，通过主代理请求 `http://main-only.test/main`。
5. 查询主端口 active rules 或对应网络导出。

预期结果：
- `global-default.test` 请求返回 body 包含 `global-default`。
- 普通规则仍按原逻辑生效，返回 body 包含 `main-only`。
- 合并后的 active rules 中 `Default` 始终排在普通规则之前。

### TC-DGR-06 临时端口全局生效且不可显式绑定

操作步骤：
1. 将 Default 内容设置为 `global-default.test status://218 resBody://(global-default)`。
2. 新增普通规则 `temp-only.test status://220 resBody://(temp-only)` 并禁用它。
3. 执行 `bifrost port bind --port <TEMP_PORT> --rule temp-only`。
4. 通过临时端口请求 `http://global-default.test/temp`。
5. 通过临时端口请求 `http://temp-only.test/temp`。
6. 执行 `bifrost port active <TEMP_PORT>`。
7. 执行 `bifrost port bind --port 0 --rule Default`。
8. 执行 `bifrost port bind --port 0 --rule default`。

预期结果：
- 临时端口请求 `global-default.test` 返回 body 包含 `global-default`。
- 临时端口请求 `temp-only.test` 返回 body 包含 `temp-only`，证明端口显式绑定仍可加载 disabled 普通规则。
- `port active` 输出中 `Default [global]` 位于 `temp-only` 之前。
- 显式绑定 `Default` 或 `default` 返回非 0，错误说明 Default 会自动应用到每个临时端口。

### TC-DGR-07 Sync 与 Share 环境不破坏 Default

操作步骤：
1. 确认 `GET /_bifrost/api/rules/Default` 中 sync 状态为 `local_only` 且无 remote id。
2. 进入 Rule Share 独占导入流程或执行对应 E2E，观察其它普通规则可被独占流程停用。
3. 再次读取 Default。

预期结果：
- Default 不进入远端 Sync 规则集合。
- 独占启用 Share 规则时，Default 不被停用，仍保持 `enabled=true`。

## 清理步骤

1. 执行 `bifrost port destroy <TEMP_PORT>`。
2. 执行 `bifrost stop`。
3. 删除 `BIFROST_DATA_DIR` 临时目录。
