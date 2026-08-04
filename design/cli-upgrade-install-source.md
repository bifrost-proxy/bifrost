# CLI upgrade 安装来源识别

## 背景

`bifrost upgrade` 原先只区分 Homebrew、旧的 `~/.bifrost/bin` 脚本路径和手动二进制。npm/pnpm 安装实际通过 Node 包装器启动平台原生包；原生进程因此落入 `Manual`，会直接覆盖包管理器拥有的二进制，既破坏包状态，也可能在 pnpm 的版本化 store 中重启到旧路径。

## 用户目标验证清单

### 必须实现

- `bifrost update` 作为 `bifrost upgrade` 的可见别名，复用相同参数解析和唯一升级执行路径。
- 自动识别 Homebrew、npm、pnpm、官方安装脚本和手动二进制来源。
- Homebrew、npm、pnpm 分别沿原包管理器升级，并把目标版本固定为本次 version-check 的版本。
- npm/pnpm 升级后重新解析全局包中的平台二进制，再执行版本校验、skills 安装、Desktop companion 更新和 proxy restart。
- 官方 Bash/PowerShell 安装器写入二进制同目录来源标记，使自定义安装目录仍可识别为脚本安装。

### 必须不破坏

- Homebrew 判断不能把 Homebrew Node 的 `/opt/homebrew/.../node_modules` npm 全局包误识别为 formula。
- npm/pnpm 来源环境标记只有在当前可执行文件确实位于 `@bifrost-proxy` Node 包结构内时才生效。
- 手动安装和脚本安装继续使用既有的固定目标 release、原子替换、校验/恢复和 Windows deferred handoff。
- 包管理器失败必须返回原始原因和可重试命令，禁止静默回退为直接覆盖包内二进制。
- 既有运行时 ownership、Desktop companion、skills 安装和 restart 流程保持单一实现。

## 设计

### CLI 别名

- `update` 通过 Clap `visible_alias` 直接绑定到 `Commands::Upgrade`；不新增 command variant、handler 或升级分支。
- `bifrost update` 与 `bifrost upgrade` 接受相同参数并输出相同帮助，顶层 `--help` 显示该别名。

### 来源识别优先级

1. 测试专用 install target override（仅 debug/显式测试开关）。
2. `node_modules/@bifrost-proxy` 平台包或主包 fallback 二进制：Node 包结构优先于 Homebrew 前缀；包装器传入 `BIFROST_CLI_INSTALL_SOURCE=npm|pnpm`，没有标记时用 `.pnpm`/pnpm global 路径兜底。
3. Homebrew Cellar/Linuxbrew 路径。
4. `<bifrost executable>.install-source` 内容为 `script`，或兼容旧 `~/.bifrost/bin`、默认 `~/.local/bin` 路径。
5. 其他现存路径视为 manual；无法取得 current executable 才是 unknown。

### npm/pnpm 升级事务

- npm：`npm install --global --no-audit --progress=false @bifrost-proxy/bifrost@<version>`。
- pnpm：`pnpm add --global @bifrost-proxy/bifrost@<version>`。
- Windows 显式选择 `npm.cmd` / `pnpm.cmd`；目标版本在进入批处理 shim 前只允许 ASCII 字母数字、`.`、`-`、`+` 且长度不超过 128，拒绝 shell 元字符。
- 两个命令都使用 10 分钟有界执行和现有进度 heartbeat；非零退出、spawn 失败或超时直接失败。
- 成功后执行 `<manager> root --global`，再由 Node 加载全局主包的 `lib/index.js#getBinaryPath()`，取得新平台二进制。这样 npm hoist、主包内 optional dependency、pnpm content-addressed store 和 `install.js` downloaded fallback 共用发布包自己的解析规则。
- 新解析出的二进制必须存在，并继续通过现有 `bifrost --version` 目标版本门禁；后续所有动作只使用该新路径。npm/pnpm 路径只做版本校验，不调用手动安装的 backup 恢复/清理函数，不直接修改任何包文件。

### 脚本来源标记

- `install-binary.sh`、`install-binary.ps1` 与源码 `install.sh` 在替换 CLI 后，同目录原子写入 `bifrost[.exe].install-source`，内容固定为 `script`；`uninstall.sh` 同时删除二进制和 marker，也会清理只剩 marker 的残余状态。
- CLI 只接受规范化后的精确值 `script`；标记缺失时保持旧路径兼容和 manual fallback。

## 验证

- Rust 单元测试：`update`/`upgrade` 解析等价、别名可发现性、来源矩阵、Homebrew Node 路径冲突、伪造环境变量、脚本 marker、固定版本命令、global root 二进制解析。
- Node 单元测试：npm 与 pnpm 包装器路径识别。
- E2E：`test_upgrade_install_source_e2e.sh` 断言 `update`/`upgrade` help 一致，并用隔离全局目录和 fake npm/pnpm 分别真实运行两个拼写，断言来源输出、固定参数、升级后新二进制校验和失败不回退。
- Installer E2E：Bash/PowerShell 原子安装函数必须生成来源 marker。
- Human test：`human_tests/cli-upgrade-install-source.md`。
- 覆盖率：本地不执行全量 coverage；由 CI `scripts/ci/coverage-all.sh --json --gate` 和 changed-lines 门禁兜底。

## 风险与边界

- `npx`/`pnpm dlx` 的临时包不是全局安装；如果无法从 global root 解析主包，upgrade 会安全失败并提示当前包管理器问题，不会擅自创建全局安装或覆盖缓存。
- npm/pnpm registry、认证、代理和自定义 global dir 完全沿用用户现有包管理器配置。
- Windows 包文件锁由 npm/pnpm 自身事务处理；若包管理器不能替换运行中的平台包，CLI 保留原错误并失败，不回退到非包管理器写入。
