# Shell Proxy 持久化配置真实场景测试

## 功能模块说明

验证 `ShellProxyManager` 对 `.zshrc`、`.zprofile`、`.bashrc`、`.bash_profile` 等 shell rc 文件的持久化代理配置只操作 Bifrost 管理块，不使用整文件备份覆盖恢复，避免清空或覆盖用户在代理启用期间新增的配置。

## 前置条件

- 在仓库根目录执行。
- 使用临时目录模拟用户 HOME 和 Bifrost 数据目录，禁止直接修改真实 `~/.zshrc`、`~/.zprofile`、`~/.bashrc` 或 `~/.bash_profile`。
- Bifrost 管理块以 `# >>> Bifrost proxy start >>>` 和 `# <<< Bifrost proxy end <<<` 为边界；测试只能断言这两个 marker 内的内容被新增、替换或删除。

## 测试用例列表

### TC-SP-01: 启用持久化代理只追加或替换 Bifrost 管理块

**操作步骤：**
1. 创建临时目录，并在其中创建模拟 `.zshrc`，内容包含用户已有 alias、PATH 或注释。
2. 构造 `ShellProxyManager` 指向该临时 `.zshrc`。
3. 调用 `enable_persistent("127.0.0.1", 7890, "localhost")`。
4. 读取 `.zshrc` 内容。

**预期结果：**
- 用户原有内容保持不变。
- 文件中只新增一个 Bifrost 管理块。
- 管理块内包含 `HTTP_PROXY=http://127.0.0.1:7890` 等代理配置。
- 数据目录下不生成依赖整文件恢复的 `shell_proxy_backup.json`。

### TC-SP-02: 禁用持久化代理只删除 Bifrost 管理块

**操作步骤：**
1. 创建模拟 `.zprofile`，内容为 `before`、Bifrost 管理块、`after`。
2. 创建另一个模拟 `.zshrc`，内容只包含普通用户配置且没有 Bifrost marker。
3. 调用 `disable_persistent()`。
4. 分别读取 `.zprofile` 和 `.zshrc`。

**预期结果：**
- `.zprofile` 中 Bifrost 管理块被删除，`before` 与 `after` 保留。
- `.zshrc` 没有 marker 时完全保持原样。
- 禁用流程不写入或依赖 `shell_proxy_backup.json`。

### TC-SP-03: 恢复流程保留代理启用期间的用户编辑

**操作步骤：**
1. 创建模拟 `.zshrc`，内容为 `ORIGINAL`。
2. 调用 `enable_persistent()` 写入 Bifrost 管理块。
3. 模拟用户在代理启用期间追加 `# user edited while proxy active`。
4. 调用 `restore()`。
5. 读取 `.zshrc`。

**预期结果：**
- Bifrost 管理块被删除。
- `ORIGINAL` 保留。
- `# user edited while proxy active` 保留。
- 不会因为存在旧备份或恢复流程而把文件覆盖回启用前快照。

### TC-SP-04: 仅由 Bifrost 创建的空 rc 文件在恢复后删除

**操作步骤：**
1. 准备一个不存在的模拟 `.bashrc` 路径。
2. 调用 `enable_persistent()`，让 Bifrost 创建只包含管理块的 `.bashrc`。
3. 调用 `restore()`。
4. 检查 `.bashrc` 是否存在。

**预期结果：**
- 恢复时删除 Bifrost 管理块后文件为空。
- 该空文件被移除，避免留下无意义的 rc 文件。

### TC-SP-05: 旧版本 backup 只用于定位路径，不恢复 stale 内容

**操作步骤：**
1. 在临时数据目录写入旧格式 `shell_proxy_backup.json`，其中 `original_content` 为 `STALE BACKUP`，路径指向模拟 `.zshrc`。
2. 当前 `.zshrc` 内容为 `USER BEFORE`、Bifrost 管理块、`USER AFTER`。
3. 调用 `ShellProxyManager::recover_from_crash(data_dir)`。
4. 读取 `.zshrc` 与数据目录。

**预期结果：**
- `.zshrc` 中只删除 Bifrost 管理块。
- `USER BEFORE` 与 `USER AFTER` 保留。
- `STALE BACKUP` 不会写回 `.zshrc`。
- 恢复完成后旧 `shell_proxy_backup.json` 被清理。

## 执行记录

| 日期 | 用例 | 执行记录 | 结果 |
| --- | --- | --- | --- |
| 2026-06-21 | TC-SP-01 ~ TC-SP-05 | 执行 `cargo test -p bifrost-core shell_proxy --lib -- --nocapture`，覆盖启用、禁用、restore、crash recovery、旧 backup 兼容和无 marker 文件 no-op。 | 通过。24 个 shell_proxy 单元/文件系统回归全部通过；测试全部使用临时目录 rc 文件，没有修改真实用户 shell 配置。 |

## 清理步骤

- 删除测试用临时目录。
- 若测试中启动过 Bifrost 服务，执行 `BIFROST_DATA_DIR=<临时目录> target/debug/bifrost stop`。
