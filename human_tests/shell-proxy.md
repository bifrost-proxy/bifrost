# Shell Proxy 持久化配置真实场景测试

## 功能模块说明

验证 `ShellProxyManager` 的运行期配置以及 `bifrost cli-proxy enable/disable` 的独立环境安装只操作各自的 Bifrost 管理块，不使用整文件备份覆盖恢复；同时验证 Bash、Zsh、Fish、PowerShell 的代理变量和常见工具链 CA 变量完整、可卸载且不破坏系统根证书信任。

## 前置条件

- 在仓库根目录执行。
- 使用临时目录模拟用户 HOME 和 Bifrost 数据目录，禁止直接修改真实 `~/.zshrc`、`~/.zprofile`、`~/.bashrc` 或 `~/.bash_profile`。
- Bifrost 管理块以 `# >>> Bifrost proxy start >>>` 和 `# <<< Bifrost proxy end <<<` 为边界；测试只能断言这两个 marker 内的内容被新增、替换或删除。
- 独立 `cli-proxy` 命令使用 `# >>> Bifrost CLI proxy environment start >>>` 和对应 end marker；两个管理块必须互相隔离。

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

### TC-SP-06: Enable 按当前或指定 shell 安装完整代理与 CA 环境

**操作步骤：**
1. 创建临时 HOME 和 `BIFROST_DATA_DIR`。
2. 依次执行 `bifrost cli-proxy enable --shell bash|zsh|fish|powershell --port 18888`。
3. 检查 Bash/Zsh/Fish/PowerShell 对应 profile 文件和语法。
4. 从 Bash 测试脚本中设置一个过期的 `SHELL=/bin/zsh`，不传 `--shell` 再执行一次 Enable。

**预期结果：**
- 父进程识别出的实际当前 Bash 优先于过期的 `SHELL=/bin/zsh`；只写入当前或显式指定 shell 的 profile，不修改其它 shell 文件。
- profile 包含大小写代理变量以及 `NO_PROXY`。
- profile 包含 Node、Python/Requests/pip、npm、Git、AWS、gRPC、Cargo、Composer、OpenSSL/Go/Bifrost 的 CA 文件或目录变量。
- `--shell` 可用值为 `bash`、`zsh`、`fish`、`powershell`，无法探测时明确提示显式指定。

### TC-SP-07: 覆盖型 CA 变量使用系统根证书与 Bifrost CA 合并 bundle

**操作步骤：**
1. 在临时数据目录执行 `bifrost cli-proxy enable --shell zsh`。
2. 读取 `<BIFROST_DATA_DIR>/certs/cli-proxy-ca-bundle.pem`。
3. 对比 profile 中 `NODE_EXTRA_CA_CERTS`、`REQUESTS_CA_BUNDLE`、`DENO_CERT` 和 `SSL_CERT_FILE` 的路径。

**预期结果：**
- 合并 bundle 至少包含一个系统根证书和 Bifrost CA。
- `NODE_EXTRA_CA_CERTS` 指向原始 Bifrost CA，保持 Node 的追加信任语义。
- `REQUESTS_CA_BUNDLE`、`SSL_CERT_FILE` 等覆盖型变量指向合并 bundle，不会只保留 Bifrost CA 而破坏普通 HTTPS 信任。
- 不写入任何关闭证书校验的变量。

### TC-SP-08: Enable 幂等且与 start --cli-proxy 生命周期块隔离

**操作步骤：**
1. 在模拟 profile 中预置旧 `# >>> Bifrost proxy start >>>` 管理块。
2. 连续两次执行 `bifrost cli-proxy enable --shell zsh`。
3. 执行 `bifrost cli-proxy disable --shell zsh`。

**预期结果：**
- 独立环境管理块始终只有一个。
- Disable 只删除 `Bifrost CLI proxy environment` 块。
- 旧 `Bifrost proxy start` 块及其内容完整保留。

### TC-SP-09: Disable 精准卸载并保留用户配置

**操作步骤：**
1. 在目标 profile 写入用户 alias/PATH 注释。
2. 执行 Enable 后再执行 `bifrost cli-proxy disable --shell <shell>`。
3. 对 Bash、Zsh、Fish、PowerShell 分别检查目标文件。

**预期结果：**
- 所有 Bifrost 独立代理和 CA 变量随管理块移除。
- 用户原有内容逐字保留。
- 卸载后为空的 profile 保留为空文件，避免误删启用前已存在的空文件。
- 重复 Disable 返回成功并提示已经禁用。

### TC-SP-10: Shell 元字符按字面量写入而不执行

**操作步骤：**
1. 把包含单引号、分号和 `touch <sentinel>` 的字符串作为 `--no-proxy` 传给 Bash Enable。
2. 在隔离 HOME 中 source 生成的 `.bashrc`。
3. 检查 sentinel 文件。

**预期结果：**
- profile 可以成功 source。
- `NO_PROXY` 保留原始字面值。
- sentinel 不存在，参数内容没有被当作 shell 命令执行。

### TC-SP-11: 自动安装或卸载失败时输出完整手工恢复信息

**操作步骤：**
1. 把 HOME 指向不可创建 profile 的路径，执行 `bifrost cli-proxy enable --shell bash`。
2. 检查失败输出和退出码。
3. 把 `.bashrc` 构造成目录以触发读取失败，执行 `bifrost cli-proxy disable --shell bash`。
4. 检查失败输出和退出码。

**预期结果：**
- 两条命令均保持非零退出码并显示原始失败原因。
- Enable 输出目标 profile、可直接复制的完整管理块、代理变量和全部 CA 文件/目录变量。
- 合并 bundle 未生成时明确告警，不把只含 Bifrost CA 的替代路径伪装成完整系统信任。
- Disable 输出需要检查的 profile，以及包含起止 marker 的精准手工删除范围。
- 手工指引不会声称自动操作已经成功。

### TC-SP-12: 残缺 marker 和多 profile 写入失败不会破坏既有配置

**操作步骤：**
1. 在 `.bashrc` 中写入用户配置和独立环境块 start marker，但不写 end marker，再执行 Enable。
2. 检查失败输出与 `.bashrc` 内容。
3. 在单元测试临时目录中准备可写 `.bashrc`，并让第二个 profile 的父路径是普通文件以触发写入失败。
4. 检查第一个 profile 是否回滚。
5. 把换行和管理 marker 作为 `--no-proxy` 值传入，检查退出码、手工回退输出和目标 profile。

**预期结果：**
- 残缺 marker 导致非零退出，输出明确提示 marker 不完整并给出手工删除范围。
- Enable 不追加第二个管理块，用户配置逐字保留。
- 第二个 profile 写入失败后，第一个 profile 恢复为执行前原文，不留下半安装状态。
- 含换行/marker 的值被拒绝，手工回退也不渲染可注入的配置块，目标 profile 不创建。

### TC-SP-13: 父进程 shell 检测依赖在所有平台可用

**操作步骤：**
1. 执行 `cargo metadata --no-deps --format-version 1`，读取 `bifrost-cli` 的 `sysinfo` 依赖声明。
2. 确认该依赖的 `target` 为 `null`，而不是仅限 macOS 或 Windows。
3. 执行 `cargo check -p bifrost-cli --all-targets --all-features`。

**预期结果：**
- Linux、macOS 和 Windows 构建都会解析到父进程 shell 检测所需的 `sysinfo`。
- `bifrost-cli` 全 target、全 feature 编译通过。

### TC-SP-14: 主程序退出时自动卸载独立 CLI proxy 配置

**操作步骤：**
1. 使用隔离 HOME 和 `BIFROST_DATA_DIR` 启动未启用系统代理的 daemon，再执行 `bifrost cli-proxy enable --shell zsh`；确认服务启动后再 Enable 的管理块已经写入。
2. 执行 `bifrost restart`，等待新 listener 就绪并检查管理块。
3. 执行 `bifrost stop`，轮询检查管理块是否移除。
4. 再次 Enable，并以前台模式启动服务；确认 listener 就绪后对主进程发送 `SIGKILL`。
5. 最长等待 20 秒，轮询检查 `.zshrc` 中独立环境管理块；同时确认旧 `# >>> Bifrost proxy start >>>` 块不受独立卸载逻辑影响。

**预期结果：**
- lifecycle helper 在服务启动时创建，因此服务运行后再 Enable 也受退出清理保护；Linux 未启用系统代理时同样有效。
- restart 交接期间独立环境管理块持续存在，不出现代理/CA 环境配置空窗。
- 正常 stop 在终止服务前移除独立环境管理块。
- 前台主进程被强制终止后，独立 helper 确认父进程消失并自动移除 Bash、Zsh、Fish、PowerShell 所有支持 profile 中的独立管理块。
- 清理幂等且仅删除 `Bifrost CLI proxy environment` 块；用户内容和旧运行期 marker 保持不变。

## 执行记录

| 日期 | 用例 | 执行记录 | 结果 |
| --- | --- | --- | --- |
| 2026-06-21 | TC-SP-01 ~ TC-SP-05 | 执行 `cargo test -p bifrost-core shell_proxy --lib -- --nocapture`，覆盖启用、禁用、restore、crash recovery、旧 backup 兼容和无 marker 文件 no-op。 | 通过。24 个 shell_proxy 单元/文件系统回归全部通过；测试全部使用临时目录 rc 文件，没有修改真实用户 shell 配置。 |
| 2026-08-06 | TC-SP-01 ~ TC-SP-13 | 执行 `cargo test -p bifrost-core shell_proxy --lib -- --nocapture`（24/24）、`cargo test -p bifrost-core cli_proxy_env --lib -- --nocapture`（17/17）、CLI parse/help 与父进程 shell 单测，以及 `BIFROST_BIN="$PWD/target/debug/bifrost" SKIP_BUILD=true bash e2e-tests/tests/test_cli_proxy_environment_e2e.sh`；执行 startup UI 静态守卫和 coverage pipeline contract；通过 `cargo metadata` 断言 `sysinfo` 为跨平台依赖并执行 `cargo check -p bifrost-cli --all-targets --all-features`；最后执行完整 unit + proxy E2E 覆盖率门禁并以 `origin/main` 为基准运行 95% 增量覆盖率检查。 | 通过。真实 CLI 在隔离 HOME/BIFROST_DATA_DIR 中覆盖四种 shell、161 证书合并 bundle、当前父 shell优先、幂等、旧 marker 隔离、字面量转义、Deno/Node/Python/Go/Cargo 等 CA 变量、残缺 marker 拒绝、写入回滚、回滚失败诊断和自动失败后的完整手工指引；新 shell 用例显式禁止 Sync 自动登录弹窗和 Tray，proxy coverage manifest 15 项契约一致；`sysinfo` 依赖 target 为 `null` 且 CLI 全 target/all-features 编译通过；rules 68/68、shell 15/15、runner 373/373、绝对覆盖率门禁通过，变更生产 Rust 行覆盖率 `95.08% (618/650)`；临时 profile 仅位于测试沙箱，没有修改真实用户配置。 |
| 2026-08-06 | TC-SP-14 | 使用 release binary 执行 `PROXY_PORT=19891 bash e2e-tests/tests/test_cli_proxy_environment_e2e.sh`，在隔离 HOME/BIFROST_DATA_DIR 中先启动 daemon 后 Enable，真实执行 restart、stop，再以前台模式启动并对主进程发送 `SIGKILL`。 | 通过。restart 期间 marker 保留，stop 后 marker 删除；强杀主进程后独立 lifecycle helper 在轮询窗口内删除 marker。测试未启用系统代理，证明 Linux/无系统代理路径同样受保护；所有进程和临时目录均由测试清理。 |

## 清理步骤

- 删除测试用临时目录。
- 若测试中启动过 Bifrost 服务，执行 `BIFROST_DATA_DIR=<临时目录> target/debug/bifrost stop`。
