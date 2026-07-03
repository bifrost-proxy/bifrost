# bifrost-cli 命令行代理（环境变量注入）

## 背景

Bifrost 已经通过 `bifrost system-proxy` / `--system-proxy` 管理系统级 HTTP 代理设置（macOS 走 `networksetup`，Windows 走注册表，Linux 走各桌面环境接口）。这条路径对 GUI 应用和大部分系统网络栈生效，但对**从终端启动的开发工具链**（git、curl、cargo、pip、npm、go、maven、docker CLI、kubectl …）不完全适用：这些工具优先读取 `http_proxy` / `https_proxy` / `all_proxy` / `no_proxy` 环境变量，系统代理设置未必被继承（尤其是 macOS/Linux 上从 shell 启动的进程）。

CI 脚本、IDE 集成 Terminal、Agent 派发的 shell 命令、临时排障场景都需要一种「shell 环境级」的代理注入：**在代理运行期间稳定地把标准 proxy 环境变量注入到用户当前 shell 的启动脚本**，并在代理进程退出时自动撤销，避免代理下线后所有终端网络卡死。

本方案在 `bifrost-cli` 增加 `--cli-proxy` 启动参数，自动检测当前 shell 类型（zsh / bash / fish 支持持久化，其它 shell 走临时命令输出），通过「受控块（managed block）」实现幂等写入与安全删除，同时提供 Admin API 与 CLI/TUI 状态入口。

## 用户目标验证清单

### 必须实现

- 新增全局启动参数 `--cli-proxy [--cli-proxy-no-proxy <LIST>]`。
- 代理启动时向当前 shell 的启动脚本写入受控块，包含大小写两套 8 个变量：`http_proxy`/`https_proxy`/`all_proxy`/`no_proxy` 和 `HTTP_PROXY`/`HTTPS_PROXY`/`ALL_PROXY`/`NO_PROXY`。
- 自动检测 shell 类型（按 `SHELL` basename 判断 zsh/bash/fish；否则按 `PSModulePath`/`PROFILE` 判 PowerShell；Windows 上按 `COMSPEC` 判 Cmd；都不匹配 → `Unknown`）。
- 写入策略：zsh 写 `~/.zshrc` + `~/.zprofile`；bash 写 `~/.bashrc` + `~/.bash_profile`；fish 写 `~/.config/fish/config.fish`；其它 shell 不写文件，只打印临时命令。
- 受控块使用固定标记：`# >>> Bifrost proxy start >>>` / `# <<< Bifrost proxy end <<<`（常量在 `crates/bifrost-core/src/shell_proxy.rs`）。
- 幂等：重复启用时替换标记块内容而不是追加多份；禁用时只删除标记块之间的内容。
- 崩溃恢复：下次 `bifrost start` 启动时调用 `ShellProxyManager::recover_from_crash(data_dir)`，只利用旧 `shell_proxy_backup.json` 定位待清理 rc 文件，不覆盖 `original_content`。
- 状态入口：前台 `bifrost start` 输出 `🖥️  CLI PROXY (ENV)` 块；`bifrost status -t` TUI 通过 Admin push (`SETTINGS_SCOPE_CLI_PROXY`) 显示；`GET /api/proxy/cli` 返回 `{ enabled, shell, config_files, proxy_url }`。
- 代理进程退出时自动撤销：正常退出走 `disable_persistent`；异常退出下次启动时 `recover_from_crash` 清理。

### 必须不破坏

- 不改动 `bifrost system-proxy` 现有行为；两条路径并存。
- 不改动 `.zshrc`/`.bashrc`/`.config/fish/config.fish` 中受控块以外的任何字节。
- 用户手动追加到 rc 文件的其它内容在启用/禁用/恢复三条路径下都保留。
- 不做整文件备份恢复：整文件备份会形成写入空窗期，用户或其它工具在这段时间修改 rc 后用旧快照覆盖会造成配置丢失。当前实现的安全边界是 marker-block-only。
- PowerShell / Cmd / Unknown 分支不写任何 rc 文件；只打印 `CLI proxy persistent config not available for <shell> shell (use temporary commands instead)`，避免误写 `~/.profile`。
- POSIX `sh` / `dash` / `ksh` 不在支持列表（planned, not yet shipped as of 2026-06-16），落到 `Unknown` 分支。
- GUI 应用（Finder/LaunchAgent 启动）代理生效不保证 —— 此类需求应使用 `system-proxy`。

### 必须真实验证

- 真实执行 `bifrost -p 18888 start --cli-proxy`（临时 `HOME`），验证 rc 文件中出现受控块，8 个变量齐全。
- 停止代理，验证受控块被完整移除，其它内容不变。
- 手动把 daemon kill -9，下次 `bifrost start` 触发 `recover_from_crash` 清理残留受控块。
- `bifrost status -t` TUI 中 CLI Proxy 面板显示 enabled=true / 正确 shell / 正确 config_files。
- `curl -s http://127.0.0.1:8080/api/proxy/cli` 返回一致 JSON。
- 新开 zsh/bash/fish 终端立即 `echo $https_proxy` 命中代理地址。

## 产品语义

### cli-proxy vs system-proxy

- `system-proxy`：面向 GUI 与系统网络栈，可能涉及管理员权限，覆盖面广但对 shell-only 工具链未必生效。
- `cli-proxy`：面向终端与开发脚本，不需要管理员权限，只影响读取环境变量的进程，轻量；仅在 Bifrost 代理运行期间生效。

两者可共存：`bifrost start --cli-proxy` 会同时启用 system-proxy（默认）与 cli-proxy（显式）。

### 受控块（Managed Block）不变式

- Bifrost 只拥有两个 marker 之间的字节；marker 之外的所有字节在启用、禁用、恢复三条路径下都保持不变。
- 启用时：文件中已存在 marker 块 → 替换块内容；不存在 → 追加新块到文件末尾。
- 禁用时：只删除 marker 块之间的内容与 marker 本身；没有 marker 的文件保持原样；删除后文件为空则移除该空文件。
- 恢复时（崩溃场景）：与禁用同语义 —— 只删除 marker 块，保留用户在代理启用期间追加或修改的其它内容。**不使用整文件快照覆盖**。

### 大小写两套变量

- 部分工具只读大写（Java `-D` 系？Windows 派生工具），部分只读小写（很多 POSIX 工具默认小写）。同时写两套稳定性最好。
- `no_proxy` 默认 `localhost,127.0.0.1,::1`；用户 `--cli-proxy-no-proxy` 覆盖时应包含 loopback，否则 Bifrost 自己的 admin/callback 请求都会走代理造成回环。

### 状态是「rc 文件里有没有 marker 块」

- `enabled` 字段由「当前 shell 目标 rc 文件是否包含 `START_MARKER`」决定，而不是靠内存标志位。这样即使进程重启、崩溃恢复未完成，UI 也能反映真实磁盘状态。
- `config_files` 返回当前 shell 对应的 rc 路径数组，供用户排查「为什么我新开的终端没生效」。

## 技术细节

### 常量

```rust
// crates/bifrost-core/src/shell_proxy.rs
const BACKUP_FILE_NAME: &str = "shell_proxy_backup.json";
const START_MARKER: &str = "# >>> Bifrost proxy start >>>";
const END_MARKER: &str = "# <<< Bifrost proxy end <<<";
```

> 历史文档曾写作 `# >>> bifrost cli-proxy >>>` / `# <<< bifrost cli-proxy <<<`；**实际实现以上述常量为准**。

### 类型

```rust
pub enum ShellType {
    Zsh,
    Bash,
    Fish,
    PowerShell,
    Cmd,
    Unknown,
}

pub struct CliProxyStatus {
    pub enabled: bool,
    pub shell: ShellType,
    pub config_files: Vec<PathBuf>,
    pub proxy_url: String,
}

pub struct ShellProxyManager { /* ... */ }

impl ShellProxyManager {
    pub fn detect_shell() -> ShellType;
    pub fn enable_persistent(&mut self, host: &str, port: u16, bypass: &str) -> Result<()>;
    pub fn disable_persistent(&mut self) -> Result<()>;
    pub fn recover_from_crash(data_dir: &Path) -> Result<()>;
    pub fn enable_temporary(&self, host: &str, port: u16, bypass: &str) -> Vec<String>;
}
```

### CLI 参数

```rust
// crates/bifrost-cli/src/cli.rs（start 子命令）
#[arg(long = "cli-proxy", help = "Persist proxy env vars in this shell's rc file until proxy stops")]
pub cli_proxy: bool,

#[arg(long = "cli-proxy-no-proxy", value_name = "LIST", default_value = "localhost,127.0.0.1,::1")]
pub cli_proxy_no_proxy: String,
```

### Shell 检测优先级

`ShellProxyManager::detect_shell`（`crates/bifrost-core/src/shell_proxy.rs::76`）：

1. `SHELL` env 的 basename 包含 `zsh` / `bash` / `fish` → 对应 shell。
2. 存在 `PSModulePath` 或 `PROFILE` → `PowerShell`。
3. Windows 上存在 `COMSPEC` → `Cmd`。
4. 都不匹配 → `Unknown`。

**Unknown 不会回退到 `sh` / `~/.profile`**；持久化直接跳过，只打印临时命令。

### 写入文件策略

| Shell | 写入文件 |
|-------|---------|
| zsh | `~/.zshrc`、`~/.zprofile` |
| bash | `~/.bashrc`、`~/.bash_profile` |
| fish | `~/.config/fish/config.fish` |
| PowerShell / Cmd / Unknown | 不写任何文件 |

zsh/bash 双文件写入原因：不同启动模式（login shell vs interactive shell）读取不同 rc；双写提升「新开终端必生效」稳定性。

### 相关文件

- `crates/bifrost-core/src/shell_proxy.rs`：`ShellProxyManager` 核心实现（764 行）。
- `crates/bifrost-core/src/lib.rs`：re-export。
- `crates/bifrost-cli/src/cli.rs`：`--cli-proxy` 参数定义。
- `crates/bifrost-cli/src/commands/start.rs`：启动时调用 `enable_persistent` + `recover_from_crash`。
- `crates/bifrost-cli/src/commands/stop.rs`：正常停止时调用 `disable_persistent`。
- `crates/bifrost-cli/src/commands/status_tui.rs`：TUI 显示 CLI Proxy 面板。
- `crates/bifrost-cli/src/main.rs`：signal handler / drop guard 触发禁用。
- `crates/bifrost-admin/src/handlers/proxy.rs`：`GET /api/proxy/cli` 处理。
- `crates/bifrost-admin/src/push.rs`：`SETTINGS_SCOPE_CLI_PROXY` push。
- `web/src/services/pushService.ts` / `web/src/stores/useProxyStore.ts` / `web/src/pages/Settings/tabs/ProxyTab.tsx` / `SystemProxySection.tsx` / `web/src/api/proxy.ts`：Web UI 消费状态。
- `e2e-tests/tests/test_cli_proxy_start_e2e.sh`：E2E。
- `human_tests/shell-proxy.md`：human_tests 用例。

## CLI + Web + Admin API

### CLI

```bash
# 启用（同时启用 system-proxy）
bifrost -p 9900 start --cli-proxy

# 自定义 bypass 列表
bifrost -p 9900 start --cli-proxy --cli-proxy-no-proxy "localhost,127.0.0.1,::1,*.corp.internal"

# 仅 cli-proxy，不动 system-proxy
bifrost -p 9900 start --cli-proxy --no-system-proxy
```

前台启动输出块（示例）：

```text
🖥️  CLI PROXY (ENV)
   Status:    ✓ Enabled
   Shell:     zsh
   Rc files:  /Users/eden/.zshrc, /Users/eden/.zprofile
   Proxy:     http://127.0.0.1:9900
   No proxy:  localhost,127.0.0.1,::1
   Note:      New terminals will inherit these vars until proxy stops.
```

不支持 shell 分支输出：

```text
🖥️  CLI PROXY (ENV)
   Status:    ⚠ Requested but not enabled
   Shell:     powershell
   Note:      CLI proxy persistent config not available for powershell shell.
              Use temporary commands instead:
                $env:HTTP_PROXY  = "http://127.0.0.1:9900"
                $env:HTTPS_PROXY = "http://127.0.0.1:9900"
                $env:NO_PROXY    = "localhost,127.0.0.1,::1"
```

### Admin API

- `GET /api/proxy/cli`：
  ```json
  {
    "enabled": true,
    "shell": "zsh",
    "config_files": ["/Users/eden/.zshrc", "/Users/eden/.zprofile"],
    "proxy_url": "http://127.0.0.1:9900",
    "no_proxy": "localhost,127.0.0.1,::1"
  }
  ```
- Admin push scope `SETTINGS_SCOPE_CLI_PROXY`：状态变更时广播（enable / disable / recover）。

### Web UI

Settings → Proxy tab → System Proxy section 下方增加 CLI Proxy 只读面板：显示 enabled / shell / config_files / proxy_url / no_proxy；提供「Copy temporary commands」按钮（针对不支持的 shell）。

## Sync 边界

- 不同步：`--cli-proxy` 是本机 shell 环境注入，跨机器语义无意义。
- rc 文件是每台机器本地文件；也不参与 Bifrost 配置同步。
- 状态（enabled / shell / config_files）不参与远端同步；仅通过 Admin push 在本机 UI/TUI 之间同步。

## 实现切分

### Phase 1：ShellProxyManager 核心

- `crates/bifrost-core/src/shell_proxy.rs`：`ShellType`、`CliProxyStatus`、`detect_shell`、`enable_persistent`、`disable_persistent`、`enable_temporary`、marker 常量。
- 单元测试覆盖 detect_shell 各分支、enable/disable 幂等、非 marker 内容保留。

### Phase 2：CLI 集成

- `cli.rs` 新增 `--cli-proxy` / `--cli-proxy-no-proxy` 参数。
- `commands/start.rs` 启动时调用 `enable_persistent` + `recover_from_crash`。
- `commands/stop.rs` 停止时 `disable_persistent`。
- `main.rs` 增加 drop guard / signal handler，在异常退出前尽力 `disable_persistent`。
- 前台启动输出 CLI PROXY (ENV) 面板。

### Phase 3：Admin + TUI + Web

- `crates/bifrost-admin/src/handlers/proxy.rs` 新增 `GET /api/proxy/cli`。
- `crates/bifrost-admin/src/push.rs` 新增 `SETTINGS_SCOPE_CLI_PROXY`。
- `crates/bifrost-cli/src/commands/status_tui.rs` 加 CLI Proxy 面板。
- Web UI：`api/proxy.ts` 新 endpoint；`stores/useProxyStore.ts` 消费 push；`Settings/tabs/ProxyTab.tsx` / `SystemProxySection.tsx` 渲染。

### Phase 4：E2E + human_tests + 文档

- `e2e-tests/tests/test_cli_proxy_start_e2e.sh`：临时 `HOME`、`bifrost start --cli-proxy`、断言 rc marker 块；stop 后断言清理。
- 更新 `human_tests/shell-proxy.md`。
- 更新 `docs/cli.md` / `docs/cli-quick-start.md` 中的 cli-proxy 章节。

## 测试方案

### 单元测试

- `detect_shell_zsh_bash_fish_powershell_cmd_unknown`
- `enable_persistent_zsh_writes_zshrc_and_zprofile_with_managed_block`
- `enable_persistent_bash_writes_bashrc_and_bash_profile`
- `enable_persistent_fish_writes_config_fish`
- `enable_persistent_powershell_skips_files_and_prints_temporary`
- `enable_persistent_replaces_existing_managed_block_idempotent`
- `disable_persistent_removes_only_managed_block_leaves_other_content`
- `disable_persistent_removes_empty_rc_files_after_cleanup`
- `recover_from_crash_uses_backup_paths_but_never_overwrites_original_content`
- `no_proxy_defaults_include_loopback`
- `custom_no_proxy_overrides_default`

### E2E 测试

`e2e-tests/tests/test_cli_proxy_start_e2e.sh`：

1. 临时 `HOME` 目录，避免污染真实用户环境。
2. 启动 `bifrost -p <port> start --cli-proxy`，等待前台面板显示 Enabled。
3. 断言 `$HOME/.zshrc`（或对应 shell rc）包含 `START_MARKER` 与 8 个 export 语句。
4. 通过 `bash -c 'source $HOME/.bashrc; env | grep -i proxy'`（或 zsh 对应）验证变量注入。
5. `bifrost stop`。
6. 断言 rc 文件不再包含 marker 块；用户预置的其它行保留。
7. 崩溃恢复分支：手动 `kill -9` daemon → 二次 `bifrost start`（可加 `--no-system-proxy` 避免副作用）→ 断言 marker 块被清理。

### 真实场景测试（human_tests）

更新 `human_tests/shell-proxy.md`：

- TC-SP-01：zsh 用户 `bifrost start --cli-proxy`，新开 Terminal 立即 `curl https://httpbin.org/get` 命中代理。
- TC-SP-02：bash 用户同上。
- TC-SP-03：fish 用户同上。
- TC-SP-04：`bifrost stop` 后新开 Terminal，`echo $https_proxy` 为空。
- TC-SP-05：崩溃恢复：`kill -9` daemon 后 `bifrost start` 清理残留 marker。
- TC-SP-06：PowerShell / Cmd / Unknown shell 分支：不写文件，打印临时命令。
- TC-SP-07：`bifrost status -t` 与 `GET /api/proxy/cli` 状态一致。

`human_tests/readme.md` 索引同步。

### 覆盖率与项目校验

- `cargo test -p bifrost-core --lib shell_proxy`
- `cargo test -p bifrost-cli --lib cli_proxy`
- `cargo test --workspace --all-features`
- `bash e2e-tests/tests/test_cli_proxy_start_e2e.sh`
- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- rust-project-validate
- no-local-coverage 约定下不跑 `make coverage`

## Review/Fix/Test 闭环

### 第 1 轮

- 复核用户目标：`--cli-proxy` 启动/停止/崩溃恢复语义、8 变量注入、支持/不支持 shell 分支、状态入口。
- 复核 diff：`shell_proxy.rs`、CLI 参数、start/stop/main drop guard、Admin API、Web UI、E2E、human_tests。
- 重点 review：**是否有整文件备份恢复的代码路径残留**（禁止）；marker 常量与文档一致；`recover_from_crash` 只用路径不用 original_content；no_proxy 默认包含 loopback。
- 复测：focused 单测 + E2E。

### 第 2 轮

- 复查第 1 轮修复；再次运行 workspace test 与 E2E。
- 手动验证真实 zsh/bash/fish 场景（macOS 真机）；PowerShell 分支至少代码路径验证。
- human_tests 索引一致；rust-project-validate 通过或记录豁免。
- 文档同步：`docs/cli.md` 中 cli-proxy 段落更新，`docs/cli-quick-start.md` 「让流量进入代理」场景包含 cli-proxy 路径。

## 风险与决策

- 整文件备份恢复（**已否决**）：整文件快照在启用→禁用之间形成写入空窗期，任何在此期间修改 rc 的操作会被旧快照覆盖，最坏可能清空用户 `.zshrc`。当前实现严格 marker-block-only；`shell_proxy_backup.json` 只保留路径用于 crash recovery 定位，不保留 original_content。
- Shell 检测错判：`SHELL` env 可能被人为改成不匹配的值。当前策略是「找不到就打临时命令」，不做危险默认（不写 `~/.profile`）。
- Fish 的 `set -gx` 语法与 zsh/bash 的 `export` 不同：`enable_persistent` 针对 fish 分支单独生成 `set -gx VAR VALUE` 行。
- POSIX `sh` / `dash` / `ksh` 未支持：planned；当前用户需要走临时命令。
- 无 tty 环境（cron / launchd）：`SHELL` 可能为空，落到 Unknown 分支，只打临时命令。这是安全默认。
- Bifrost 自身请求走代理形成回环：`no_proxy` 默认包含 `localhost,127.0.0.1,::1`；用户自定义时必须包含 loopback，Admin 端做校验并在缺失时插入。
- GUI 应用不生效：文档明确「cli-proxy 只覆盖读取 shell env 的进程；GUI/系统栈请用 `system-proxy`」。
- 多 Bifrost 实例并存：多个 bifrost 进程同时启用 `--cli-proxy` 会互相覆盖 marker 块内容（只有一个 proxy_url 生效）。第一版接受此语义；后续如有需求可在 marker 中加实例 ID。
