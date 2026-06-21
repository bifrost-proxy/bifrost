# bifrost-cli 命令行代理（环境变量）方案

## 背景

Bifrost 已支持通过 `--system-proxy` / `bifrost system-proxy` 管理系统代理，但在大量开发/自动化场景中，仍需要**命令行层面的代理**：

- CI / 脚本 / 终端内运行的工具（git、curl、cargo、pip、npm、go、maven…）通常优先读取 `http_proxy/https_proxy/all_proxy/no_proxy` 等环境变量。
- 某些工具（如 git/cargo/maven）也支持工具自身的代理配置；但环境变量仍是最通用的“一键生效”方式。

因此需要在 `bifrost-cli` 增加启动参数：**仅在代理运行期间稳定地设置/撤销 shell 环境变量代理**，并可自动识别用户当前 shell 类型。

## 目标

- 启动参数 `--cli-proxy` 控制是否在代理运行期间写入 shell 启动脚本。
- 自动检测 shell 类型（zsh/bash/fish/sh）。
- 稳定、可重复执行：
  - 写入 shell 启动脚本（持久化），确保新开终端即可生效。
  - 通过“受控块（managed block）”实现幂等写入与安全删除，不污染用户其他内容。
  - 代理进程退出时自动撤销配置，避免代理退出后仍保留代理环境导致网络异常。
- 同时设置大小写两套变量（兼容更多程序）。

## 非目标

- 不保证对 GUI 应用（Finder/LaunchAgent 启动）生效。GUI 应用通常不读取 shell rc 文件；此类需求应使用系统代理或平台级环境变量机制。
- 不在本次方案中处理系统级（root/systemd/launchctl）全局环境注入，仅在文档中给出建议。
- 不试图对所有工具写入其私有配置（如 `.npmrc`、`~/.cargo/config.toml`）；本次以环境变量为核心，私有配置作为调研补充。

## CLI 设计

新增启动参数：

```bash
bifrost start --cli-proxy [--cli-proxy-no-proxy <LIST>]
```

### 启用

用途：代理启动时写入 shell 启动脚本（持久化），代理退出时自动撤销。

```bash
bifrost -p <PORT> start --cli-proxy
bifrost -p <PORT> start --cli-proxy --cli-proxy-no-proxy "localhost,127.0.0.1,::1,*.local"
```

写入变量集合：

- `http_proxy` / `https_proxy` / `all_proxy` / `no_proxy`
- `HTTP_PROXY` / `HTTPS_PROXY` / `ALL_PROXY` / `NO_PROXY`

### 状态

通过以下入口展示当前 CLI 代理状态：

- 前台 `bifrost start` 输出的 `🖥️  CLI PROXY (ENV)` 块：实时显示 `Status`（`✓ Enabled` / `⚠ Requested but not enabled` / `Disabled`）、`Proxy` 地址、`No Proxy` 列表（默认 `localhost,127.0.0.1,::1,*.local`）。
- `bifrost status -t` TUI：通过 Admin push (`SETTINGS_SCOPE_CLI_PROXY`) 拿到 `CliProxyStatus { enabled, shell, config_files, proxy_url }` 并渲染。
- Web UI / 程序化查询：`GET /api/proxy/cli`，返回 `{ enabled, shell, config_files, proxy_url }`；其中 `enabled` 由 rc 文件中是否包含 `START_MARKER` 决定，`config_files` 为当前 shell 对应的目标 rc 文件路径数组。

## Shell 自动检测与写入文件选择

检测优先级（实现位于 `crates/bifrost-core/src/shell_proxy.rs::ShellProxyManager::detect_shell`）：

1. 环境变量 `SHELL` 的 basename 包含 `zsh` / `bash` / `fish` → 对应 shell。
2. 否则若存在 `PSModulePath` 或 `PROFILE` → `PowerShell`。
3. 否则在 Windows 上若存在 `COMSPEC` → `Cmd`。
4. 都不匹配 → `Unknown`（不会回退到 `sh`/`~/.profile`，持久化配置直接跳过）。

写入文件策略（仅以下三类 shell 支持持久化；其它 shell 只能用 `enable_temporary` 的临时命令输出）：

- zsh：`~/.zshrc` 与 `~/.zprofile`
- bash：`~/.bashrc` 与 `~/.bash_profile`
- fish：`~/.config/fish/config.fish`
- PowerShell / Cmd / Unknown：**不写任何 rc 文件**，启动时打印 `CLI proxy persistent config not available for <shell> shell (use temporary commands instead)`。

说明：

- zsh、bash 在不同启动模式会读取不同文件；同时写入两类常见文件可提升“新开终端必生效”的稳定性。
- fish 的配置文件是固定路径。
- POSIX `sh` / `dash` / `ksh` 当前**不在支持列表**（planned, not yet shipped as of 2026-06-16）；落到 `Unknown` 分支后不会写 `~/.profile`。

## 受控块（Managed Block）与幂等

采用固定标记，确保可重复执行且可安全删除（常量定义于 `crates/bifrost-core/src/shell_proxy.rs`）：

- 开始标记：`# >>> Bifrost proxy start >>>`
- 结束标记：`# <<< Bifrost proxy end <<<`

（早期文档曾写作 `# >>> bifrost cli-proxy >>>` / `# <<< bifrost cli-proxy <<<`，实际实现并未使用该名称，以代码中的常量 `START_MARKER` / `END_MARKER` 为准。）

实现策略（`ShellProxyManager::enable_persistent` / `disable_persistent` / `restore`）：

1. 启用时：逐个读取当前 rc 文件；若文件中已存在标记块则只替换标记块内容，否则追加一个新的 Bifrost 管理块到文件末尾。
2. 禁用时：逐个读取当前 rc 文件，只删除开始/结束标记之间的 Bifrost 管理块；没有 marker 的文件保持原样。
3. 恢复时：仍然只删除当前文件里的 Bifrost 管理块，保留用户在代理启用期间追加或修改的其它内容；如果删除管理块后文件为空，则移除该空文件。
4. 崩溃恢复：下次 `bifrost start` 启动时调用 `ShellProxyManager::recover_from_crash(data_dir)`；若发现旧版本残留的 `shell_proxy_backup.json`，只使用其中的路径定位待清理 rc 文件，不把 `original_content` 覆盖写回。

设计约束：禁止依赖整文件备份恢复 rc 文件。整文件备份会在代理启用到恢复之间形成空窗期，用户或其它工具在这段时间写入 `.zshrc` / `.bashrc` 后，再用旧快照覆盖会造成配置丢失甚至清空。当前实现的安全边界是 marker-block-only：Bifrost 只拥有两个 marker 中间的内容。

## 软件代理设置调研（常见场景）

### 1) 通用环境变量（推荐）

大多数 CLI 工具会读取：

- `http_proxy` / `https_proxy` / `all_proxy`：代理地址
- `no_proxy`：直连列表（逗号分隔，常见包含 `localhost,127.0.0.1,::1` 与内网域名）

补充：

- 大小写两套变量兼容性更好（部分程序仅读取大写或仅读取小写）。
- SOCKS：常见用 `all_proxy=socks5://...` 或 `socks5h://...`（`h` 表示由代理侧解析 DNS，避免本地 DNS 泄露）。

### 2) git

- 环境变量：继承 `http_proxy/https_proxy`（很多场景够用）
- git 配置（更显式）：
  - `git config --global http.proxy http://127.0.0.1:9900`
  - `git config --global https.proxy http://127.0.0.1:9900`
  - 取消：`git config --global --unset http.proxy` / `https.proxy`

### 3) curl / wget

- curl：
  - 环境变量：读取 `http_proxy/https_proxy/all_proxy/no_proxy`
  - 参数：`curl -x http://127.0.0.1:9900 https://example.com`
- wget：
  - 环境变量：读取 `http_proxy/https_proxy/no_proxy`
  - 配置文件：`~/.wgetrc` 支持 `http_proxy=...`

### 4) Node.js 生态（npm/yarn/pnpm）

- 环境变量：多数包管理器/脚本会继承系统环境
- npm：
  - `npm config set proxy http://127.0.0.1:9900`
  - `npm config set https-proxy http://127.0.0.1:9900`
  - `npm config set noproxy "localhost,127.0.0.1,*.local"`
- yarn（classic/berry）与 pnpm：通常也支持 config 级别的 proxy，但环境变量仍是最通用方式。

### 5) Python（pip / requests / poetry）

- pip：
  - `pip config set global.proxy http://127.0.0.1:9900`
  - 环境变量同样生效：`HTTPS_PROXY=...`
- Python requests：默认读取 `HTTP(S)_PROXY` / `NO_PROXY`
- Poetry：多数情况下继承环境变量，也可通过其 config 设置。

### 6) Rust（cargo）

- 环境变量：`HTTP(S)_PROXY`、`ALL_PROXY`
- cargo 配置（更可控）：
  - `~/.cargo/config.toml`：
    - `[http] proxy = "http://127.0.0.1:9900"`

### 7) Go

- Go 下载模块一般会继承 `HTTP(S)_PROXY`。
- 常见还会配合：
  - `GOPROXY`（模块代理，与 HTTP 代理不同概念）
  - `GONOSUMDB` / `GOPRIVATE`（私有模块）

### 8) Java（Maven / Gradle）

- JVM 参数（对 Java 程序通用）：
  - `-Dhttp.proxyHost=127.0.0.1 -Dhttp.proxyPort=9900`
  - `-Dhttps.proxyHost=127.0.0.1 -Dhttps.proxyPort=9900`
  - `-Dhttp.nonProxyHosts="localhost|127.0.0.1|*.local"`
- Maven：`~/.m2/settings.xml` 支持 `<proxies>` 配置
- Gradle：`~/.gradle/gradle.properties` 支持 `systemProp.http.proxyHost=...`

### 9) Docker / Kubernetes

- Docker daemon / systemd service：需要在服务环境中配置（不同于 shell rc）。
- Kubernetes 工具（kubectl/helm）通常继承环境变量；但集群内部代理需要按集群组件配置。

## 与 Bifrost 其他能力的关系

- `system-proxy`：用于 GUI/全系统网络栈的代理路由（更广），可能涉及管理员权限。
- `cli-proxy`：用于终端/脚本环境的统一代理（更轻量），不需要管理员权限，适合开发工具链，且仅在代理运行期间生效。

## 验证方案

- 增加 e2e 脚本：
  - 使用临时 `HOME` 目录，避免污染真实用户环境。
  - 启动代理携带 `--cli-proxy`，验证 rc 文件中受控块写入；停止代理后验证自动撤销。
