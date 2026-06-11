# CLI 分组管理命令测试用例

## 功能模块说明

测试 `bifrost group` 子命令的完整功能，包括分组列表查询（含关键字过滤和分页）、分组详情查看，以及分组规则的增删改查、启用/禁用操作。

## 前置条件

1. 启动 Bifrost 服务（使用临时数据目录避免污染正式环境）：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- start -p 8800 --unsafe-ssl
   ```
2. 确保已通过 `bifrost sync login` 登录同步服务，或已有可用的分组数据
3. 记录一个可用的 `<group_id>`（可通过 `bifrost group list` 获取）

---

## 测试用例

### TC-CGR-01：列出所有分组

**操作步骤**：
1. 执行命令：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- -p 8800 group list
   ```

**预期结果**：
- 输出 `Groups (N/M):` 格式的标题，N 为当前页数量，M 为总数
- 每个分组显示为 `  <id> <name> [public|private] - <description>` 格式
- 输出底部显示分页信息 `Page X/Y (total: Z, limit: 50, offset: 0)`
- 如果无分组，输出 `No groups found.`
- 命令退出码为 0

---

### TC-CGR-02：使用关键字和限制数量列出分组

**操作步骤**：
1. 执行命令：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- -p 8800 group list --keyword "team" --limit 20
   ```

**预期结果**：
- 仅返回名称或描述中包含 "team" 的分组
- 最多返回 20 条记录
- 分页信息中 `limit` 显示为 `20`
- URL 中包含 `keyword=team` 和 `limit=20` 参数
- 命令退出码为 0

---

### TC-CGR-03：查看分组详情

**前置条件**：通过 TC-CGR-01 获取一个有效的 `<group_id>`

**操作步骤**：
1. 执行命令（将 `<group_id>` 替换为实际 ID）：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- -p 8800 group show <group_id>
   ```

**预期结果**：
- 输出包含 `Group: <name>` 显示分组名称
- 输出包含 `ID: <group_id>` 显示分组 ID
- 输出包含 `Visibility: public` 或 `Visibility: private`
- 如有描述，输出包含 `Description: <what>`
- 输出包含 `Created: <create_time>` 显示创建时间
- 命令退出码为 0

---

### TC-CGR-04：查看不存在的分组

**操作步骤**：
1. 执行命令：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- -p 8800 group show non-existent-id-12345
   ```

**预期结果**：
- 输出错误信息，提示分组不存在或请求失败
- 命令退出码非 0

---

### TC-CGR-05：列出分组内的规则

**前置条件**：通过 TC-CGR-01 获取一个有效的 `<group_id>`

**操作步骤**：
1. 执行命令：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- -p 8800 group rule list <group_id>
   ```

**预期结果**：
- 输出包含 `Group: <name> (<group_id>)` 标题
- 输出包含 `Writable: yes` 或 `Writable: no`
- 如有规则，输出 `Rules (N):` 后逐行显示 `  <name> [enabled|disabled] (M rules, updated: <time>)`
- 如无规则，输出 `No rules found.`
- 命令退出码为 0

---

### TC-CGR-06：查看分组规则详情

**前置条件**：通过 TC-CGR-05 获取一个有效的 `<group_id>` 和 `<rule_name>`

**操作步骤**：
1. 执行命令：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- -p 8800 group rule show <group_id> <rule_name>
   ```

**预期结果**：
- 输出包含 `Rule: <name>`
- 输出包含 `Status: enabled` 或 `Status: disabled`
- 输出包含 `Sync: <status>`（如 `synced`、`local_only`）
- 如有远程 ID，输出包含 `Remote ID: <id>`
- 输出包含 `Created: <time>` 和 `Updated: <time>`
- 输出包含 `Content:` 后面跟规则内容
- 命令退出码为 0

---

### TC-CGR-07：通过 --content 添加分组规则

**前置条件**：拥有一个可写的 `<group_id>`

**操作步骤**：
1. 执行命令：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- -p 8800 group rule add <group_id> test-rule-content -c "example.com host://127.0.0.1:3000"
   ```

**预期结果**：
- 输出包含 `Rule 'test-rule-content' added to group successfully.`
- 命令退出码为 0
- 执行 `group rule show <group_id> test-rule-content` 确认规则已创建，内容为 `example.com host://127.0.0.1:3000`

---

### TC-CGR-08：通过 --file 添加分组规则

**前置条件**：拥有一个可写的 `<group_id>`

**操作步骤**：
1. 创建规则文件：
   ```bash
   echo "*.test.com host://127.0.0.1:8080" > /tmp/test-rule.txt
   ```
2. 执行命令：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- -p 8800 group rule add <group_id> test-rule-file -f /tmp/test-rule.txt
   ```

**预期结果**：
- 输出包含 `Rule 'test-rule-file' added to group successfully.`
- 命令退出码为 0
- 执行 `group rule show <group_id> test-rule-file` 确认规则内容为 `*.test.com host://127.0.0.1:8080`

---

### TC-CGR-09：更新分组规则

**前置条件**：已通过 TC-CGR-07 创建了 `test-rule-content` 规则

**操作步骤**：
1. 执行命令：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- -p 8800 group rule update <group_id> test-rule-content -c "example.com host://127.0.0.1:4000"
   ```

**预期结果**：
- 输出包含 `Rule 'test-rule-content' updated successfully.`
- 命令退出码为 0
- 执行 `group rule show <group_id> test-rule-content` 确认内容已更新为 `example.com host://127.0.0.1:4000`

---

### TC-CGR-10：启用分组规则

**前置条件**：已有一个处于禁用状态的规则，或使用已创建的 `test-rule-content`

**操作步骤**：
1. 执行命令：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- -p 8800 group rule enable <group_id> test-rule-content
   ```

**预期结果**：
- 输出包含 `Rule 'test-rule-content' enabled` 或类似的成功消息
- 命令退出码为 0
- 执行 `group rule show <group_id> test-rule-content` 确认 `Status: enabled`

---

### TC-CGR-11：禁用分组规则

**前置条件**：已有一个处于启用状态的规则

**操作步骤**：
1. 执行命令：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- -p 8800 group rule disable <group_id> test-rule-content
   ```

**预期结果**：
- 输出包含 `Rule 'test-rule-content' disabled` 或类似的成功消息
- 命令退出码为 0
- 执行 `group rule show <group_id> test-rule-content` 确认 `Status: disabled`

---

### TC-CGR-12：删除分组规则

**前置条件**：已创建 `test-rule-content` 和 `test-rule-file` 规则

**操作步骤**：
1. 执行命令：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- -p 8800 group rule delete <group_id> test-rule-content
   ```
2. 再执行：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- -p 8800 group rule delete <group_id> test-rule-file
   ```

**预期结果**：
- 每次输出包含 `Rule '<name>' deleted successfully.` 或类似的成功消息
- 命令退出码为 0
- 执行 `group rule list <group_id>` 确认已删除的规则不再出现

---

### TC-CGR-13：更新规则时未提供 --content 和 --file 报错

**操作步骤**：
1. 执行命令：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- -p 8800 group rule update <group_id> some-rule
   ```

**预期结果**：
- 输出错误信息包含 `--content` 或 `--file must be provided`
- 命令退出码非 0

---

### TC-CGR-14：服务未启动时执行分组命令报错

**前置条件**：确保 Bifrost 服务未运行

**操作步骤**：
1. 停止服务后执行命令：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- -p 8800 group list
   ```

**预期结果**：
- 输出错误信息包含 `Failed to connect to Bifrost admin API`
- 提示 `Is the proxy server running?`
- 提示 `Hint: Start the proxy with: bifrost start`
- 命令退出码非 0

---

### TC-CGR-15：Group CLI mock 单测并发稳定性回归

**操作步骤**：
1. 执行高并发 group 命令单测：
   ```bash
   cargo test -p bifrost-cli commands::group::tests:: -- --test-threads=16
   ```
2. 执行 workspace 聚合测试：
   ```bash
   cargo test --workspace --all-features
   ```

**预期结果**：
- `test_group_rule_add_with_file`、`test_group_rule_update_with_content` 等相邻 add/update 用例在高并发下全部通过。
- workspace 聚合测试不再因 group mock HTTP server 时序互相干扰而失败。

**执行记录（2026-05-02）**：
- `cargo test -p bifrost-cli commands::group::tests:: -- --test-threads=16`：PASS，lib 与 bin 两套 group tests 共 66 个用例通过。

---

### TC-CGR-16：Group Sync E2E 不被本地 sync-server 全局限流误伤

**背景**：完整 Group Sync E2E 会在一分钟内覆盖 sync-server 直连 API、Bifrost Admin 代理、规则转发效果和 CLI Group 操作。修复前，macOS CI shard 在 Step 12 之后触发本地 mock sync-server 的 `429 too many requests`，导致后续 group rules 与 CLI 断言连锁失败。

**操作步骤**：
1. 执行 sync-server 类型检查：
   ```bash
   pnpm --dir packages/bifrost-sync-server lint
   ```
2. 执行全局限流配置单测：
   ```bash
   pnpm --dir packages/bifrost-sync-server test src/__tests__/rate-limit.test.ts
   ```
3. 执行 sync-server 全量单元测试：
   ```bash
   pnpm --dir packages/bifrost-sync-server test
   ```
4. 使用独立端口执行完整 Group Sync E2E：
   ```bash
   ADMIN_PORT=18123 SYNC_PORT=18131 ECHO_PORT=18132 e2e-tests/tests/test_group_sync_e2e.sh
   ```

**预期结果**：
- sync-server 类型检查通过。
- `rate-limit.test.ts` 验证 `server.rate_limit_per_ip` 会控制非 Remote Invoke 路径的全局 IP 限流，`server.auth_rate_limit_per_ip` 会控制登录/注册路径的独立限流。
- sync-server 全量单元测试通过，完整 Group API 单测不会被本地测试配额误伤。
- `test_group_sync_e2e.sh` 启动本地 sync-server 时使用 `--rate-limit-per-ip 1000 --auth-rate-limit-per-ip 1000`，完整执行 93 个断言，`Failed: 0`，日志中不再出现因本地测试配额导致的 `too many requests` 连锁失败。

**执行记录（2026-06-11）**：
- `pnpm --dir packages/bifrost-sync-server lint`：PASS。
- `pnpm --dir packages/bifrost-sync-server test src/__tests__/rate-limit.test.ts`：PASS，2 个测试通过。
- `pnpm --dir packages/bifrost-sync-server test`：PASS，9 个 test file / 160 个测试通过。
- `ADMIN_PORT=18123 SYNC_PORT=18131 ECHO_PORT=18132 e2e-tests/tests/test_group_sync_e2e.sh`：PASS，`Total: 93 / Passed: 93 / Failed: 0`。

---

## 清理

测试完成后清理临时数据和测试文件：
```bash
rm -f /tmp/test-rule.txt
rm -rf .bifrost-test
```
