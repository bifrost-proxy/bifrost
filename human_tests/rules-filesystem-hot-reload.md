# 规则文件系统热更新真实场景测试

## 功能模块说明

验证运行中的 Bifrost 在未登录远端 Sync 的本地使用场景下，可以感知 CLI 本地规则命令和直接文件修改，并让代理运行时规则、Rules active summary、页面小圆点使用的 Badge 规则快照保持一致。同时验证 Group 规则同步在多端读取和纯同步元信息变化时不会重复写盘、不会制造 runtime reload 日志风暴。

## 前置条件

- 在仓库根目录执行。
- 已编译最新 `target/debug/bifrost`，或允许测试脚本自动执行 `cargo build --bin bifrost`。
- 测试启动 Bifrost 时必须使用临时 `BIFROST_DATA_DIR`，并携带 `--no-system-proxy`。
- 测试默认设置 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`，避免未登录 Sync 时打开登录浏览器。

## 测试用例列表

### TC-RFHR-01 CLI 本地规则新增后运行时自动生效

操作步骤：
1. 执行 `BIFROST_BIN=target/debug/bifrost e2e-tests/tests/test_rules_filesystem_hot_reload.sh`。
2. 脚本启动真实 HTTP echo server 和真实 Bifrost 代理。
3. 脚本在同一个 `BIFROST_DATA_DIR` 下执行 `bifrost rule add <name> --content "127.0.0.1 statusCode://203"`。
4. 脚本不调用 WebUI 保存或 Admin API 创建规则，直接通过代理请求 echo server。
5. 脚本读取 `/_bifrost/api/rules/active-summary`。

预期结果：
- CLI 命令成功写入规则文件。
- 代理请求状态码自动从 `200` 变为 `203`。
- active summary 的 `merged_content` 包含 `statusCode://203`。

### TC-RFHR-02 直接编辑 `.bifrost` 文件后运行时自动更新

操作步骤：
1. 继续执行同一个脚本。
2. 脚本先执行 `bifrost rule update <name> --content "127.0.0.1 statusCode://204"`。
3. 脚本不调用 WebUI 保存或 Admin API 更新规则，直接通过代理请求 echo server。
4. 脚本读取 `/_bifrost/api/rules/active-summary`。
5. 脚本直接修改 `rules/<name>.bifrost`，将 `statusCode://204` 替换为 `statusCode://205`。
6. 脚本不调用 WebUI 保存或 Admin API 更新规则，直接通过代理请求 echo server。
7. 脚本读取 `/_bifrost/api/rules/active-summary`。

预期结果：
- CLI update 后代理请求状态码自动变为 `204`。
- CLI update 后 active summary 的 `merged_content` 包含 `statusCode://204`。
- 直接文件编辑后代理请求状态码自动变为 `205`。
- 直接文件编辑后 active summary 的 `merged_content` 包含 `statusCode://205`。
- 说明页面小圆点依赖的 Badge 规则快照已随磁盘变化刷新。

### TC-RFHR-03 CLI 本地规则删除后运行时自动清理

操作步骤：
1. 继续执行同一个脚本。
2. 脚本执行 `bifrost rule delete <name>`。
3. 脚本不调用 WebUI 保存或 Admin API 删除规则，直接通过代理请求 echo server。
4. 脚本读取 `/_bifrost/api/rules/active-summary`。

预期结果：
- 代理请求状态码恢复为原始 `200`。
- active summary 不再包含被删除的规则名。

### TC-RFHR-04 直接删除 `.bifrost` 文件后运行时自动清理

操作步骤：
1. 继续执行同一个脚本。
2. 脚本重新执行 `bifrost rule add <name> --content "127.0.0.1 statusCode://206"`。
3. 脚本直接删除 `rules/<name>.bifrost`。
4. 脚本不调用 WebUI 保存或 Admin API 删除规则，直接通过代理请求 echo server。
5. 脚本读取 `/_bifrost/api/rules/active-summary`。

预期结果：
- 重新添加后代理请求状态码自动变为 `206`。
- 直接删除文件后代理请求状态码恢复为原始 `200`。
- active summary 不再包含被删除的规则名。

### TC-RFHR-05 Group 规则无变化同步不得重复写盘（回归）

操作步骤：
1. 执行 focused 回归测试：
   ```bash
   cargo test -p bifrost-admin group_rules::tests:: -- --nocapture
   ```
2. 测试使用临时 `RulesStorage` 创建远端 Group env，并执行第一次本地同步。
3. 测试记录生成的 `.bifrost` 文件内容、mtime 和 `last_synced_at`。
4. 等待短暂时间后，使用完全相同的远端 env 再执行一次同步。
5. 测试再次读取文件内容、mtime 和 `last_synced_at`。
6. 测试覆盖远端 legacy rule 内容需要本地 canonical normalization 的场景，例如 `ignore://host|rule` 转成本地规则正文。
7. 测试确认同一 group 的同步落盘复用同一把锁，避免多个端同时读取同一 group 时重复写入。
8. 补充执行真实热更新脚本，确认无变化写盘修复没有破坏真实文件变化 reload：
   ```bash
   BIFROST_BIN=target/debug/bifrost e2e-tests/tests/test_rules_filesystem_hot_reload.sh
   ```

预期结果：
- 第二次 Group env 同步返回未改变 active rules。
- 第二次同步不改写 `.bifrost` 文件内容。
- 第二次同步不推进 `.bifrost` 文件 mtime。
- 第二次同步不刷新 `last_synced_at`。
- 远端 legacy 格式内容经过本地 canonical normalization 后，重复同步仍不写盘。
- 同一 group 在单进程内复用同步锁，多端并发读取不会放大本地写盘。
- 真实 CLI 新增、更新、直接编辑和删除 `.bifrost` 文件仍能触发运行中代理与 active summary 热更新。

### TC-RFHR-06 同步元信息变化不得触发 runtime reload，真实规则内容变化必须触发

操作步骤：
1. 执行 focused watcher 回归测试：
   ```bash
   cargo test -p bifrost-cli rules_filesystem_snapshot -- --nocapture
   ```
2. 测试创建临时 rules 目录和已同步 `.bifrost` 文件。
3. 测试只修改 `last_synced_at`、`remote_updated_at` 等 sync metadata 后重新采集 rules filesystem snapshot。
4. 测试修改真实规则正文后重新采集 rules filesystem snapshot。
5. 补充执行真实热更新脚本，确认真实文件正文变化仍能刷新运行中代理：
   ```bash
   BIFROST_BIN=target/debug/bifrost e2e-tests/tests/test_rules_filesystem_hot_reload.sh
   ```

预期结果：
- sync metadata-only 变化前后的 runtime snapshot 相同，不触发 runtime reload。
- 真实规则正文变化前后的 runtime snapshot 不同，会触发 runtime reload。
- 真实 CLI 新增、更新、直接编辑和删除 `.bifrost` 文件仍能触发运行中代理与 active summary 热更新。

执行记录：
- 2026-06-12 执行 `cargo test -p bifrost-admin sync_envs_to_local_skips_unchanged_remote_rule_write -- --nocapture` 通过，确认第二次相同 Group env 同步不写盘、不推进 mtime、不刷新 `last_synced_at`。
- 2026-06-12 执行 `BIFROST_BIN=target/release/bifrost e2e-tests/tests/test_rules_filesystem_hot_reload.sh` 通过。真实脚本启动临时 Bifrost 和 HTTP echo server，覆盖 CLI add/update/delete、直接编辑 `.bifrost`、直接删除 `.bifrost`，所有代理状态码与 active summary 断言通过，全流程使用临时数据目录并完成清理。
- 2026-06-23 执行 `cargo test -p bifrost-admin group_rules::tests:: -- --nocapture` 通过，确认重复 Group env 同步不写盘，legacy/canonical 内容重复同步不刷新 `last_synced_at`，同 group 同步锁复用。
- 2026-06-23 执行 `cargo test -p bifrost-cli rules_filesystem_snapshot -- --nocapture` 通过，确认 sync metadata-only 变化不改变 runtime snapshot，真实规则内容变化会改变 runtime snapshot。
- 2026-06-23 执行 `BIFROST_DISABLE_TRAY=1 BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 BIFROST_BIN=target/debug/bifrost e2e-tests/tests/test_rules_filesystem_hot_reload.sh` 通过。真实脚本启动临时 Bifrost 和 HTTP echo server，覆盖 CLI add/update/delete、直接编辑 `.bifrost`、直接删除 `.bifrost`，所有代理状态码与 active summary 断言通过，全流程使用临时数据目录、`--no-system-proxy` 并完成清理。

## 清理步骤

- 脚本退出时停止 Bifrost 代理进程。
- 脚本退出时停止 HTTP echo server。
- 脚本退出时删除临时 `BIFROST_DATA_DIR`。
