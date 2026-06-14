# CLI 导入/导出、指标、同步、升级、补全命令测试用例

## 功能模块说明

测试 Bifrost CLI 中的导入/导出（`import`/`export`）、指标查看（`metrics`）、远程同步（`sync`）、版本升级（`upgrade`/`version-check`）、Shell 补全（`completions`）以及技能安装（`install-skill`）等命令的完整功能，包含 install-skill 更多 agent 兼容回归。

## 前置条件

1. 启动 Bifrost 服务（使用临时数据目录避免污染正式环境）：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- start -p 8800 --unsafe-ssl
   ```
2. 确保已创建至少一条规则、一个 value 和一个 script，以便测试导出功能：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- -p 8800 rule add export-test -c "example.com host://127.0.0.1:3000"
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- -p 8800 value set MY_VAR "hello_world"
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- -p 8800 script add request export-test-script -c "module.exports = function(ctx) { return ctx; }"
   ```

---

## 测试用例

### TC-CIE-01：导出规则到 .bifrost 文件

**操作步骤**：
1. 执行命令：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- -p 8800 export rules export-test -d "测试导出" -o /tmp/test-export-rules.bifrost
   ```

**预期结果**：
- 输出包含 `Exported rules to: /tmp/test-export-rules.bifrost`
- 文件 `/tmp/test-export-rules.bifrost` 已创建且非空
- 命令退出码为 0

---

### TC-CIE-02：导出 values 到 .bifrost 文件

**操作步骤**：
1. 执行命令：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- -p 8800 export values MY_VAR -d "测试导出 values" -o /tmp/test-export-values.bifrost
   ```

**预期结果**：
- 输出包含 `Exported values to: /tmp/test-export-values.bifrost`
- 文件 `/tmp/test-export-values.bifrost` 已创建且非空
- 命令退出码为 0

---

### TC-CIE-03：导出 values 不指定名称（导出全部）

**操作步骤**：
1. 执行命令：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- -p 8800 export values -o /tmp/test-export-all-values.bifrost
   ```

**预期结果**：
- 输出包含 `Exported values to: /tmp/test-export-all-values.bifrost`
- 文件包含所有已定义的 values
- 命令退出码为 0

---

### TC-CIE-04：导出 scripts 到 .bifrost 文件

**操作步骤**：
1. 执行命令：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- -p 8800 export scripts request/export-test-script -d "测试导出脚本" -o /tmp/test-export-scripts.bifrost
   ```

**预期结果**：
- 输出包含 `Exported scripts to: /tmp/test-export-scripts.bifrost`
- 文件 `/tmp/test-export-scripts.bifrost` 已创建且非空
- 命令退出码为 0

---

### TC-CIE-05：导出规则到 stdout（不指定 -o）

**操作步骤**：
1. 执行命令：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- -p 8800 export rules export-test
   ```

**预期结果**：
- 直接输出 .bifrost 文件内容到终端（标准输出）
- 内容为可解析的 .bifrost 格式
- 命令退出码为 0

---

### TC-CIE-06：导入 .bifrost 文件

**前置条件**：已通过 TC-CIE-01 创建 `/tmp/test-export-rules.bifrost`

**操作步骤**：
1. 执行命令：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- -p 8800 import /tmp/test-export-rules.bifrost
   ```

**预期结果**：
- 输出包含 `Import Result`
- 输出包含 `Success: true`
- 输出包含 `Type:` 字段标识文件类型
- 输出包含已导入的资源数量（如 `Rules imported: 1`）
- 命令退出码为 0

---

### TC-CIE-07：仅检测文件类型（--detect-only）

**前置条件**：已通过 TC-CIE-01 创建 `/tmp/test-export-rules.bifrost`

**操作步骤**：
1. 执行命令：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- -p 8800 import /tmp/test-export-rules.bifrost --detect-only
   ```

**预期结果**：
- 输出包含 `File Type Detection`
- 输出包含 `Type:` 字段标识文件类型
- 输出包含 `Meta:` 字段展示文件元数据（JSON 格式）
- 不会实际执行导入操作
- 命令退出码为 0

---

### TC-CIE-08：查看指标摘要（metrics summary）

**操作步骤**：
1. 执行命令：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- -p 8800 metrics summary
   ```

**预期结果**：
- 输出包含 `Bifrost Metrics Summary` 标题
- 输出包含 `Version:` 显示当前版本
- 输出包含 `Uptime:` 显示运行时间（格式 `Xh Xm Xs`）
- 输出包含 `Port: 8800`
- 输出包含 `Traffic:` 部分，包括 `Total requests:`、`Active connections:` 等字段
- 命令退出码为 0

---

### TC-CIE-09：查看应用维度指标（metrics apps）

**操作步骤**：
1. 执行命令：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- -p 8800 metrics apps
   ```

**预期结果**：
- 如有流量数据，输出表头 `APPLICATION  REQUESTS  BYTES IN  BYTES OUT` 以及对应的应用数据
- 输出底部显示 `Total: N applications`
- 如无流量数据，输出 `No application metrics available.`
- 命令退出码为 0

---

### TC-CIE-10：查看主机维度指标（metrics hosts）

**操作步骤**：
1. 执行命令：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- -p 8800 metrics hosts
   ```

**预期结果**：
- 如有流量数据，输出表头 `HOST  REQUESTS  BYTES IN  BYTES OUT` 以及对应的主机数据
- 输出底部显示 `Total: N hosts`
- 如无流量数据，输出 `No host metrics available.`
- 命令退出码为 0

---

### TC-CIE-11：查看指标历史（metrics history --limit 200）

**操作步骤**：
1. 执行命令：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- -p 8800 metrics history --limit 200
   ```

**预期结果**：
- 如有历史数据，输出表头 `TIMESTAMP  REQUESTS  ACTIVE  BYTES IN  BYTES OUT` 以及历史快照数据
- 输出底部显示 `Showing N snapshots`，N 不超过 200
- 如无历史数据，输出 `No metrics history available.`
- 命令退出码为 0

---

### TC-CIE-12：查看同步状态（sync status）

**操作步骤**：
1. 执行命令：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- -p 8800 sync status
   ```

**预期结果**：
- 输出包含 `Sync Status` 标题
- 输出包含 `Enabled:` 字段（`true` 或 `false`）
- 输出包含 `Auto sync:` 字段
- 输出包含 `Remote URL:` 字段
- 输出包含 `Has session:` 字段
- 输出包含 `Reachable:` 字段
- 输出包含 `Authorized:` 字段
- 输出包含 `Syncing:` 字段
- 输出包含 `Reason:` 字段
- 命令退出码为 0

---

### TC-CIE-13：查看同步配置（sync config）

**操作步骤**：
1. 执行命令：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- -p 8800 sync config
   ```

**预期结果**：
- 输出包含 `Sync Configuration` 标题
- 输出包含 `Enabled:` 字段
- 输出包含 `Auto sync:` 字段
- 输出包含 `Remote URL:` 字段
- 命令退出码为 0

---

### TC-CIE-14：检查新版本（version-check）

**操作步骤**：
1. 执行命令：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- version-check
   ```

**预期结果**：
- 命令标准输出不为空，这是本次回归修复的核心校验点
- 输出至少包含 `Current version:` 当前版本信息
- 如果当前已是最新版本，输出包含 `You are running the latest version.` 或等价提示
- 如果有新版本可用，输出包含最新版本号，以及 `Run 'bifrost upgrade' to update.` 提示
- 如果暂时无法获取最新版本，输出包含 `Could not determine the latest version` 或等价网络提示
- 命令退出码为 0

---

### TC-CIE-15：升级命令行为描述（upgrade）

**操作步骤**：
1. 执行命令：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- upgrade
   ```

**预期结果**：
- 输出 `Checking for updates... (current: vX.Y.Z)` 显示当前版本
- 如果已是最新版本，输出 `✓ You're already on the latest version (vX.Y.Z)`
- 如果有新版本可用：
  - 显示 `📦 New version available!` 及版本对比信息
  - 显示安装方式（Homebrew / Install script / Manual）
  - 提示 `Do you want to upgrade now? [y/N]`
  - 输入 `n` 取消升级，输出 `Upgrade cancelled.`
- 如果网络不可用，输出 `⚠ Could not check for updates. Check your network connection.`
- 命令退出码为 0

---

### TC-CIE-16：生成 Bash 补全脚本（completions bash）

**操作步骤**：
1. 执行命令：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- completions bash
   ```

**预期结果**：
- 输出一段 Bash 补全脚本内容
- 脚本内容包含 `bifrost` 字符串
- 脚本格式为合法的 Bash 语法
- 命令退出码为 0

---

### TC-CIE-17：生成 Zsh 补全脚本（completions zsh）

**操作步骤**：
1. 执行命令：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- completions zsh
   ```

**预期结果**：
- 输出一段 Zsh 补全脚本内容
- 脚本内容包含 `bifrost` 和 `#compdef` 等 Zsh 补全特征标记
- 命令退出码为 0

---

### TC-CIE-18：生成 Fish 补全脚本（completions fish）

**操作步骤**：
1. 执行命令：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- completions fish
   ```

**预期结果**：
- 输出一段 Fish 补全脚本内容
- 脚本内容包含 `bifrost` 和 `complete` 等 Fish 补全特征关键字
- 命令退出码为 0

---

### TC-CIE-19：使用别名生成补全脚本（comp）

**操作步骤**：
1. 执行命令：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- comp bash
   ```

**预期结果**：
- 输出与 TC-CIE-16 一致，`comp` 别名正常工作
- 命令退出码为 0

---

### TC-CIE-20：安装 SKILL.md 到 AI 工具（install-skill -y）

**操作步骤**：
1. 执行命令：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- install-skill -y
   ```

**预期结果**：
- 输出包含 `🔧 Bifrost SKILL.md Installer` 标题
- 输出包含 `Source: GitHub main branch (latest)`
- 输出包含 `Target tools:` 列出所有目标工具（Claude Code, Codex, Trae, Cursor）
- 输出包含 `Install mode: global`
- 输出包含 `Target paths:` 列出每个工具的安装目标路径
- 下载并安装成功后输出 `✓ Downloaded N bytes`
- 每个工具安装后输出 `✓ <path> (N bytes)`
- 最终输出 `✓ Successfully installed to N tools!`
- 命令退出码为 0

---

### TC-CIE-21：安装 SKILL.md 到指定工具

**操作步骤**：
1. 执行命令：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- install-skill -t claude-code -y
   ```

**预期结果**：
- `Target tools:` 仅显示 `Claude Code`
- 仅安装到 Claude Code 对应路径
- 输出 `✓ Successfully installed to 1 tool!`
- 命令退出码为 0

---

### TC-CIE-22：安装 SKILL.md 到 GitHub Copilot

**操作步骤**：
1. 执行命令：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- install-skill -t github-copilot -y
   ```

**预期结果**：
- `Target tools:` 显示 `GitHub Copilot`
- 输出的目标路径包含 `~/.copilot/skills/bifrost/SKILL.md` 或等价的 Copilot skills 目录
- 最终输出 `✓ Successfully installed to 1 tool!`
- 命令退出码为 0

---

### TC-CIE-23：安装 SKILL.md 到通用 Agent Skills 目录

**操作步骤**：
1. 执行命令：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- install-skill -t universal -y
   ```

**预期结果**：
- `Target tools:` 显示 `Universal Agent Skills`
- 输出的目标路径包含 `~/.agents/skills/bifrost/SKILL.md` 或等价的项目级 `.agents/skills` 目录
- 最终输出 `✓ Successfully installed to 1 tool!`
- 命令退出码为 0

---

### TC-CIE-24：项目级安装到 Codex 时同时写入 `.codex` 和 `.agents`

**操作步骤**：
1. 执行命令：
   ```bash
   TOOLCHAIN_BIN="$(dirname "$(rustup which cargo)")"
   TEST_ROOT="$(mktemp -d)"
   (
     cd "$TEST_ROOT" && PATH="$TOOLCHAIN_BIN:$PATH" \
     BIFROST_INSTALL_SKILL_SOURCE=embedded \
     BIFROST_DATA_DIR=./.bifrost-test \
     cargo run --manifest-path <REPO_ROOT>/Cargo.toml --bin bifrost -- install-skill -t codex --cwd -y
   )
   ```
2. 检查以下文件都存在且非空：
   ```bash
   test -s "$TEST_ROOT/.codex/skills/bifrost/SKILL.md"
   test -s "$TEST_ROOT/.agents/skills/bifrost/SKILL.md"
   ```

**预期结果**：
- `Target tools:` 显示 `Codex`
- `Install mode:` 显示 `project-local (current directory)`
- 输出的目标路径同时包含 `.codex/skills/bifrost/SKILL.md` 和 `.agents/skills/bifrost/SKILL.md`
- 上述两个 `SKILL.md` 文件都被创建，且内容非空
- 命令退出码为 0

---

### TC-CIE-25：导入不存在的文件报错

**操作步骤**：
1. 执行命令：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- -p 8800 import /tmp/non-existent-file.bifrost
   ```

**预期结果**：
- 输出错误信息包含 `Failed to read file`
- 命令退出码非 0

---

### TC-CIE-26：version-check redirect 优先验证（upgrade 命令）

**操作步骤**：
1. 清除 version check 缓存以强制重新请求：
   ```bash
   rm -rf .bifrost-test
   ```
2. 使用 debug 日志级别执行 upgrade 观察请求路径：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test RUST_LOG=bifrost_core::version_check=debug ./target/debug/bifrost upgrade 2>&1
   ```

**预期结果**：
- debug 日志中出现 `fetching latest version via redirect` 表明走了 redirect 路径
- debug 日志中出现 `redirect followed, extracting version from final URL` 且 final_url 包含 `/tag/vX.Y.Z`
- debug 日志中出现 `extracted version from redirect` 表明版本号成功提取
- 标准输出不包含 `GitHub API rate limit` 错误
- 命令退出码为 0

---

### TC-CIE-27：highlights 获取失败不阻塞版本检测

**操作步骤**：
1. 执行 upgrade 验证整体流程在 API 限流时仍然正常工作：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test RUST_LOG=bifrost_core::version_check=debug ./target/debug/bifrost upgrade 2>&1
   ```
2. 确认代码中 `fetch_latest_release_sync` 的降级逻辑：API highlights 失败 → HTML 兜底 → 空 vec（不影响版本号返回）

**预期结果**：
- 无论 highlights 获取路径如何，版本号都能成功获取
- 如果 API highlights 为空，debug 日志出现 `API highlights empty or rate limited, trying HTML page fallback`
- 所有 highlights 获取函数的错误都不会阻塞版本号返回（仅返回空 vec）
- 命令退出码为 0

---

### TC-CIE-28：升级校验子进程超时不阻塞 upgrade

**操作步骤**：
1. 执行聚焦单元测试，验证 upgrade 用临时文件原子替换最终二进制：
   ```bash
   SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-cli --lib upgrade_install_binary_atomically_replaces_existing_target
   ```
2. 执行聚焦单元测试，模拟子进程长时间不退出：
   ```bash
   SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-cli --lib upgrade_command_status_with_timeout_does_not_block_on_hung_child
   ```
3. 执行聚焦单元测试，确认子进程正常退出和失败退出仍被正确区分：
   ```bash
   SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-cli --lib upgrade_command_status_with_timeout_reports_success_and_failure
   ```
4. 验证安装脚本使用临时文件原子替换目标二进制：
   ```bash
   BIFROST_INSTALL_BINARY_SKIP_MAIN=1 bash -c 'source ./install-binary.sh; d="$(mktemp -d)"; printf old > "$d/bifrost"; printf new > "$d/source"; install_binary_atomically "$d/source" "$d/bifrost"; test "$(cat "$d/bifrost")" = new; test ! -e "$d/bifrost.tmp.$$"'
   ```
5. 验证安装脚本 post-install 子步骤具备超时 watchdog：
   ```bash
   bash -n install-binary.sh
   BIFROST_INSTALL_BINARY_SKIP_MAIN=1 bash -c 'source ./install-binary.sh; set +e; BIFROST_INSTALL_POST_INSTALL_TIMEOUT=1 run_bifrost_post_install_command sh -c "sleep 5"; rc="$?"; test "$rc" -eq 124'
   ```

**预期结果**：
- 第 1 条命令输出 `test result: ok`，确认 upgrade 不再直接复制到最终可执行路径
- 第 2 条命令在 1 秒内完成，输出 `test result: ok`，不会等待 `sleep 5` 完整结束
- 第 3 条命令输出 `test result: ok`，成功退出映射为 success，非 0 退出映射为 failure
- 第 4 条命令完成后目标文件内容为 `new`，临时文件不存在
- 第 5 条命令返回 124，并输出 post-install command timed out 相关 warning
- 任一子步骤卡住时，`bifrost upgrade` 或一键安装脚本都不会无限等待该子进程

---

## 清理

测试完成后清理临时数据和测试文件：
```bash
rm -f /tmp/test-export-rules.bifrost
rm -f /tmp/test-export-values.bifrost
rm -f /tmp/test-export-all-values.bifrost
rm -f /tmp/test-export-scripts.bifrost
rm -rf .bifrost-test
```
