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
2. 执行聚焦单元测试，验证 upgrade 校验失败时能从 backup 恢复旧二进制：
   ```bash
   SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-cli --lib upgrade_restore_binary_backup_restores_previous_target
   ```
3. 执行聚焦单元测试，验证 upgrade restart 会等待 runtime.json 中的 HTTP 端口和 separate SOCKS5 端口：
   ```bash
   SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-cli --lib upgrade_restart_ports_from_runtime_uses_runtime_ports
   ```
4. 执行聚焦单元测试，模拟子进程长时间不退出：
   ```bash
   SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-cli --lib upgrade_command_status_with_timeout_does_not_block_on_hung_child
   ```
5. 执行聚焦单元测试，确认子进程正常退出和失败退出仍被正确区分：
   ```bash
   SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-cli --lib upgrade_command_status_with_timeout_reports_success_and_failure
   ```
6. 验证升级重启 E2E 的源码门禁包含多端口释放等待、restart executable 固定和系统代理恢复：
   ```bash
   bash e2e-tests/tests/test_upgrade_restart_e2e.sh
   ```
7. 验证安装脚本使用临时文件原子替换目标二进制：
   ```bash
   BIFROST_INSTALL_BINARY_SKIP_MAIN=1 bash -c 'source ./install-binary.sh; d="$(mktemp -d)"; printf old > "$d/bifrost"; printf new > "$d/source"; install_binary_atomically "$d/source" "$d/bifrost"; test "$(cat "$d/bifrost")" = new; test ! -e "$d/bifrost.tmp.$$"'
   ```
8. 验证安装脚本 post-install 子步骤具备超时 watchdog：
   ```bash
   bash -n install-binary.sh
   BIFROST_INSTALL_BINARY_SKIP_MAIN=1 bash -c 'source ./install-binary.sh; set +e; BIFROST_INSTALL_POST_INSTALL_TIMEOUT=1 run_bifrost_post_install_command sh -c "sleep 5"; rc="$?"; test "$rc" -eq 124'
   ```

**预期结果**：
- 第 1 条命令输出 `test result: ok`，确认 upgrade 不再直接复制到最终可执行路径，旧二进制 backup 保留到最终校验阶段
- 第 2 条命令输出 `test result: ok`，确认新二进制校验失败或 fallback 失败时旧二进制可恢复
- 第 3 条命令输出 `test result: ok`，确认 upgrade restart 不只等待 HTTP 端口，也等待 separate SOCKS5 端口
- 第 4 条命令在 1 秒内完成，输出 `test result: ok`，不会等待 `sleep 5` 完整结束
- 第 5 条命令输出 `test result: ok`，成功退出映射为 success，非 0 退出映射为 failure
- 第 6 条命令通过 19 个断言，确认 upgrade restart 使用多端口释放门禁、端口占用诊断和系统代理恢复；源码门禁覆盖替换后不再用 `current_exe()` 推断新二进制路径
- 第 7 条命令完成后目标文件内容为 `new`，临时文件不存在
- 第 8 条命令返回 124，并输出 post-install command timed out 相关 warning
- 任一子步骤卡住时，`bifrost upgrade` 或一键安装脚本都不会无限等待该子进程
- Windows 上如果手动安装目标就是当前运行的 `bifrost.exe`，`bifrost upgrade` 应提前给出明确错误，避免在文件锁定的 `remove_file` 阶段失败

---

### TC-CIE-29：显式 GitHub mirror 在并行测试负载下保持第一优先级

**操作步骤**：
1. 连续 5 轮并行运行 CLI upgrade 回归：
   ```bash
   for round in 1 2 3 4 5; do
     SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-cli --lib upgrade_ -- --test-threads=16 || exit 1
   done
   ```
2. 确认 `download_selection_success_and_free_restart_port_are_exercised` 每轮均通过，且未出现 mutex poison 连锁失败。

**预期结果**：
- `BIFROST_GITHUB_MIRROR` 指向随机端口 fixture 时，`ordered_download_bases` 的首项始终是该显式地址，不受内置 GitHub 探测线程调度影响。
- 5 轮 upgrade 回归全部通过；fixture 使用系统分配的随机端口，不与并发用例碰撞，环境变量在互斥区内恢复。

**2026-07-20 执行结果**：5 轮均为 61/61 通过；显式 mirror 每轮保持首项，随机端口 fixture 无碰撞，未出现环境变量断言失败或 mutex poison 连锁失败。

---

### TC-CIE-30：独立资源 release 不得成为 Bifrost upgrade 目标

**操作步骤**：
1. 执行 `SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-core version_check::tests:: --lib -- --test-threads=1`。
2. 执行 `bash e2e-tests/tests/test_asr_moss_release_contract.sh`，确认 MOSS runtime workflow 固定 `make_latest: false`，version-check 使用 published releases 而不是裸 tags 回退。
3. 查询 GitHub `releases/latest` API 与 redirect，确认两者都指向严格的 `v<major>.<minor>.<patch>` Bifrost release，而不是 `moss-runtime-v*`。
4. 按 `per_page=100` 遍历 GitHub published Releases API 直到空页，统计稳定产品、prerelease、draft、独立资源 Release，并抽查当前稳定版 CLI、desktop、`bifrost-v<version>-checksums.txt` 和 MOSS runtime 资产下载返回 HTTP 200。
5. 执行已安装的 `bifrost upgrade -y`，观察 latest/current、CLI 下载资产与 desktop 后置更新目标。
6. 执行 `node scripts/npm-publish.mjs 0.0.159 --print-tag` 与 `node scripts/npm-publish.mjs 0.0.159-beta.1 --print-tag`，并检查 Release workflow 的 Homebrew job 条件。

**预期结果**：
- `moss-runtime-v1.0.0`、`vmoss-runtime-v1.0.0`、draft、prerelease、非三段数字 tag 都不能成为稳定 Bifrost update target。
- `/releases/latest` 被资源 release 污染时，sync/async 检查均从 published release 列表选择最新稳定 Bifrost release；不会回退到可能没有安装资产的裸 git tag。
- published release fallback 显式分页，每页 100 条并持续到空页；即使未来累计超过 1000 个独立资源 Release，也不会因固定页数上限漏掉稳定产品 Release。
- `bifrost upgrade` 不再拼接 `bifrost-desktop-vmoss-runtime-*.dmg`，当前稳定版用户不会因 MOSS runtime release 收到虚假桌面更新。
- 稳定 npm 版本使用 `latest` dist-tag，prerelease 使用 `next`；prerelease Release 跳过 Homebrew 默认 formula/cask 更新，因此 beta 演练不会改变稳定渠道的默认安装目标。

**2026-07-21 执行记录**：
- 42 个 version-check 单元测试全部通过，MOSS release contract 与 runtime packager fixture 通过。
- GitHub latest API 与 redirect 均返回稳定产品 release `v0.0.158`，且 `draft=false`、`prerelease=false`。
- 全量遍历到 159 个 published Releases：第 1 页 100 条（97 个稳定产品、2 个 prerelease、1 个 `moss-runtime-v1.0.0`），第 2 页 59 条历史 alpha/beta，第 3 页为空；当前稳定产品资产矩阵完整。
- `v0.0.158` macOS ARM64 CLI、desktop DMG、`bifrost-v0.0.158-checksums.txt`，以及 MOSS runtime zip / sha256 的真实下载请求均返回 HTTP 200。
- 已安装的 `bifrost v0.0.156` 执行 `bifrost upgrade -y` 后使用 `v0.0.158` 产品 Release 的 CLI tarball 完成升级，并正确更新 `/Applications/Bifrost.app`；输出中没有 `moss-runtime` 或 `vmoss-runtime`，升级后正式服务按原端口重新启动。
- npm dist-tag 探针分别返回稳定版 `latest`、beta 版 `next`；Release contract 验证 prerelease Homebrew job 被条件跳过，预发布验证不会覆盖稳定安装渠道。
- PR 两条 P2 review 均纳入回归：fallback 已显式分页，并移除 10 页固定上限；高页码 URL 单测与 Release contract 通过。

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
