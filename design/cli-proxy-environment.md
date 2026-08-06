# CLI Proxy 环境安装与卸载

## 背景

`bifrost start --cli-proxy` 当前只在代理进程运行期间写入通用代理变量，并在代理停止时移除。它没有独立的安装/卸载入口，也没有为 Node.js、Python、Go、Git、Cargo、AWS CLI、gRPC 等常见工具链配置 Bifrost CA。

本设计新增独立命令：

```bash
bifrost cli-proxy enable
bifrost cli-proxy disable
```

该命令负责把代理与 CA 环境配置安装到当前 shell profile。配置在 Bifrost 主程序运行期间持续有效；主程序正常退出或异常消失后，由独立 lifecycle helper 自动移除。用户也可以提前执行 `disable` 手动卸载。

## 用户目标

- `enable` 一次完成代理环境变量和常见 CA 环境变量安装。
- `disable` 只删除 Bifrost 管理块，保留用户原有 shell 配置。
- 根据当前 shell 选择配置文件，并允许 `--shell` 显式覆盖。
- 支持 Bash、Zsh、Fish、PowerShell；无法识别的 shell 明确报错。
- 重复执行 `enable` 只替换现有管理块，不重复追加。
- 与 `start --cli-proxy` 的运行期管理块完全隔离，互不删除。
- 主程序退出时自动清理独立管理块；`restart` 交接期间保留，避免新进程接管前出现配置空窗。
- CA bundle 保留系统根证书，避免覆盖型 CA 变量破坏普通 HTTPS 信任。

## 命令语义

```text
bifrost cli-proxy enable [--host HOST] [--port PORT] [--no-proxy LIST]
                         [--shell bash|zsh|fish|powershell]
                         [--ca-file PATH] [--ca-dir PATH]
bifrost cli-proxy disable [--shell bash|zsh|fish|powershell]
```

- `host` 默认 `127.0.0.1`。
- `port` 未传时优先使用当前运行实例端口，否则使用全局 `-p`（默认 `9900`）。
- `no-proxy` 默认 `localhost,127.0.0.1,::1`。
- `ca-file` 默认 `<BIFROST_DATA_DIR>/certs/ca.crt`；缺失时生成 Bifrost CA。
- `ca-dir` 默认 CA 文件所在目录。
- `shell` 未传时先沿父进程链识别实际执行命令的 shell，再回退到 `SHELL` / PowerShell 环境变量；显式参数优先。这样从 Zsh 进入 Bash 后执行命令时不会误写 `.zshrc`。识别到 sh/dash/ksh/csh/tcsh/Nushell/Xonsh/Elvish 等尚不支持安全自动写入的 shell 时直接报错，不猜测成另一种 shell。

## Shell 与配置文件

| Shell | 配置文件 |
| --- | --- |
| Bash | `~/.bashrc`，以及 Bash 已使用的首个登录 profile（按 `.bash_profile`、`.bash_login`、`.profile` 优先级；均不存在时才创建 `.bash_profile`） |
| Zsh | `~/.zshrc`、`~/.zprofile` |
| Fish | `~/.config/fish/config.fish` |
| PowerShell | Unix: `~/.config/powershell/Microsoft.PowerShell_profile.ps1`；Windows: Documents 下 PowerShell profile |

独立命令使用以下 marker：

```text
# >>> Bifrost CLI proxy environment start >>>
# <<< Bifrost CLI proxy environment end <<<
```

marker 只有独占完整行时才视为管理边界；普通 `echo`、注释或文档行中出现相同子串不会被替换或删除。生成块始终以换行结束，避免用户后续追加配置时被 end marker 注释吞掉。

旧 `start --cli-proxy` 继续使用 `# >>> Bifrost proxy start >>>`，两个管理块仍保持隔离：`disable` 只移除独立安装块，旧运行期清理只移除运行期块；主程序退出的统一清理流程会分别移除两者。

## 生命周期与异常退出

- Bifrost 主程序在每次启动时创建独立 lifecycle helper，helper 以主进程 PID 和启动时间双因子识别父进程，并位于独立进程组中。
- 正常 `stop` 会在终止服务前移除独立 CLI proxy 管理块；直接前台退出也会在 shutdown 阶段清理。
- 主程序崩溃、被强制终止或 PID 被复用时，helper 确认父进程消失后遍历 Bash、Zsh、Fish、PowerShell 的所有支持 profile，移除独立管理块。因此清理不依赖 helper 自己运行在哪一种 shell 中。
- `restart` 写入 `PreserveForRestart` marker，旧主进程和旧 helper 跳过清理；新主进程接管后创建新的 helper。若新进程最终不能继续运行，新的退出路径仍会执行清理。E2E 必须观察 runtime PID 发生变化后再断言，不能把旧 listener 仍可连接误判为新进程接管成功。
- 手动 `bifrost cli-proxy disable` 始终可用，并且只提前移除当前选择 shell 的独立管理块；helper 后续再次清理是幂等操作。
- 为了让“服务运行中再执行 enable”也获得退出保护，helper 在 Bifrost 启动时始终创建，不要求当时已经启用系统代理或 CLI proxy；Linux 等没有系统代理集成的平台也会创建。

## 环境变量覆盖

### 代理变量

- `HTTP_PROXY`、`HTTPS_PROXY`、`ALL_PROXY`、`NO_PROXY`
- `http_proxy`、`https_proxy`、`all_proxy`、`no_proxy`

### 追加型 CA

- `NODE_EXTRA_CA_CERTS`：指向原始 Bifrost CA PEM，让 Node.js 在内置/系统根证书之外追加信任。
- `BIFROST_CA_BUNDLE`：指向原始 Bifrost CA PEM；Bifrost 自身会在系统根证书基础上追加。

### Bundle 文件型 CA

以下变量指向由系统根证书和 Bifrost CA 合成的托管 PEM bundle，避免替换系统信任后导致普通 HTTPS 失败：

- `SSL_CERT_FILE`
- `REQUESTS_CA_BUNDLE`、`CURL_CA_BUNDLE`
- `PIP_CERT`
- `NPM_CONFIG_CAFILE`、`npm_config_cafile`
- `GIT_SSL_CAINFO`
- `AWS_CA_BUNDLE`
- `GRPC_DEFAULT_SSL_ROOTS_FILE_PATH`
- `CARGO_HTTP_CAINFO`、`CARGO_HTTP_PROXY_CAINFO`
- `COMPOSER_CAFILE`
- `DENO_CERT`

### 目录型 CA

- `BIFROST_CA_DIR`
- `SSL_CERT_DIR`

目录变量是文件变量之外的补充。OpenSSL/Requests 对目录可能要求 `c_rehash` 布局，因此可靠主路径仍是合并 bundle；Go 和 Bifrost 可使用目录变量覆盖其目录型证书读取路径。

## 安全与兼容边界

- 所有写入值按目标 shell 做字面量引用，路径、空格和 shell 元字符不得转化为命令执行。
- `enable` 校验自定义 CA 文件和目录真实存在且类型正确。
- 写入前先检查所有目标 profile；多文件写入中途失败时回滚已经写入的文件，避免只安装一半。
- profile 中已有残缺、逆序或重复的 marker-only 行时拒绝自动覆盖，并输出精准手工处理指引；普通行内 marker 子串按用户内容保留；环境变量值中的换行或 marker 文本同样拒绝写入，避免配置块边界注入。
- `disable` 只移除管理块，不删除卸载后为空的 profile，避免误删启用前就存在的空文件。
- 不设置 `*_NO_VERIFY`、`NODE_TLS_REJECT_UNAUTHORIZED=0` 等关闭校验变量。
- Java truststore、Docker registry CA、浏览器 NSS DB 等不能安全地通过单个 PEM 环境变量统一配置，本命令不静默改写这些独立信任库；系统层继续使用 `bifrost ca install`。
- 当前 shell 已启动的进程不会被反向修改；命令输出明确提示重新打开 shell 或 source 对应配置文件。

## 验证计划

- 单元测试：shell 路径、shell 语法转义、变量矩阵、bundle 含系统根和 Bifrost CA、幂等替换、精确卸载、与旧 marker 隔离。
- E2E：临时 HOME 和 `BIFROST_DATA_DIR` 中运行真实 CLI，覆盖 Bash/Zsh/Fish/PowerShell、自动生成 CA、重复 enable、disable、旧 marker 共存，以及主进程异常退出后的 helper 自动清理。
- human_tests：按真实用户命令逐条检查帮助、启用结果、CA 变量、卸载保留用户配置。
- 远端 CI 执行 workspace 测试、E2E 和 90% coverage gate。
