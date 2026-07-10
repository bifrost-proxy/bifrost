# Network Account Name Filter

## 功能模块说明

验证代理账号名称在 Network 流量链路中的记录、展示和筛选行为：

- 已认证代理请求会把匹配到的账号名写入 traffic record。
- Network 列表数据源暴露账号名，详情接口保留 `account_name`。
- Network 左侧筛选在存在账号数据时显示 Accounts 分组，并支持按账号筛选。
- 账号管理卡片将启用状态和最近连接状态压缩到右上角，减少列表纵向占用。

## 前置条件

- 当前工作目录：`/Users/eden_studio/work/github/bifrost-account-network-name-filter`
- 使用当前分支构建出的二进制：`target/debug/bifrost`
- 测试必须使用临时 `BIFROST_DATA_DIR` 和动态端口，不使用默认 `~/.bifrost` 或默认 `9900`
- 需要 `curl`、`jq`、`python3`

## 测试用例列表

### TC-NAF-01 真实代理请求记录账号名

操作步骤：

1. 执行：

   ```bash
   BIFROST_BIN="$PWD/target/debug/bifrost" e2e-tests/tests/test_network_account_name_e2e.sh
   ```

2. 测试脚本会动态启动 Bifrost、配置两个代理账号、启动本地 upstream，并通过 `alice-account` 发送真实 HTTP 代理请求。
3. 脚本会查询 `/api/traffic` 和 `/api/traffic/{id}`。

预期结果：

- 代理请求返回 HTTP `200`。
- `/api/traffic` compact record 包含 `acct: "alice-account"`。
- `/api/traffic/{id}` detail 包含 `account_name: "alice-account"`。
- 测试脚本结束时显示 `Tests Passed: 2` 和 `All tests passed!`。

### TC-NAF-02 account_name 精确筛选隔离账号

操作步骤：

1. 执行 TC-NAF-01 中同一脚本。
2. 脚本会分别通过 `alice-account` 和 `bob-account` 发送真实代理请求。
3. 脚本会查询：
   - `GET /api/traffic?account_name=alice-account&account_name_match=equals`
   - `POST /api/traffic/query`，body 为 `{"account_name":"bob-account","account_name_match":"equals","limit":100}`

预期结果：

- Alice 筛选结果包含 Alice 请求 marker，不包含 Bob 请求 marker。
- Bob 筛选结果包含 Bob 请求 marker，不包含 Alice 请求 marker。
- 两个账号的请求记录都包含对应 `acct` 值。

### TC-NAF-03 WebUI Network 账号筛选数据源

操作步骤：

1. 运行 Web store 单元测试：

   ```bash
   pnpm --dir web run test:unit -- useTrafficStore
   ```

2. 检查测试输出中 `useTrafficStore` 相关用例全部通过。

预期结果：

- Store 能从 compact traffic record 中收集账号 catalog。
- Panel filter 的账号选择能过滤 Network records。
- 清空 traffic 时账号 catalog 同步清空。

### TC-NAF-04 账号管理卡片布局紧凑化

操作步骤：

1. 运行前端构建或类型检查链路：

   ```bash
   pnpm --dir web run test:unit -- useTrafficStore
   ```

2. 代码复核 `web/src/pages/Settings/tabs/AccessControlTab.tsx` 中账号卡片布局。

预期结果：

- 启用状态 switch 位于账号卡片右上角。
- 最近连接状态与右上角操作区同组展示。
- 删除操作为图标按钮，不占据底部状态行。
- 账号名、密码输入和启用状态在亮/暗主题下继续使用 Ant Design token，不新增硬编码主题色。

## 清理步骤

- `test_network_account_name_e2e.sh` 通过 `trap` 停止本次 Bifrost 测试进程、本地 upstream，并删除临时数据目录。
- 不需要清理默认 `~/.bifrost` 或 `9900` 服务，因为测试不使用它们。

## 本轮执行记录

- 2026-07-10 18:16 CST 执行 `BIFROST_BIN="$PWD/target/debug/bifrost" e2e-tests/tests/test_network_account_name_e2e.sh`：2 个用例全部通过，输出 `Tests Passed: 2` 和 `All tests passed!`。
- 2026-07-10 18:16 CST 执行 `pnpm --dir web run test:unit -- useTrafficStore`：33 个 test files / 160 个 tests 全部通过。
- 2026-07-10 18:16 CST 复核 `web/src/pages/Settings/tabs/AccessControlTab.tsx`：账号卡片右上角包含 small switch、Last Connected 文案和删除图标按钮，底部状态行已移除，未新增硬编码主题色。
