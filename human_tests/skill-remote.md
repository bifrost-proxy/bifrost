# Bifrost Remote Skill 真实场景测试

## 功能模块说明

验证用户通过 `bifrost install-skill` 安装技能后，能够获得独立的 `bifrost-remote` skill，并且该 skill 正确表达 Remote Invoke 的远程设备控制能力、目标端默认启动方式、查询/shell/文件三类 scope 的前置准备、当前 relay-backed 子命令边界、`remote exec` 的授权操作路径、远端工程任务开始前必须读取工程约束信息的要求、调用异常时主动获取远端最新技能的要求，不包含历史版本迁移文案，并且不提供 `remote traffic clear` 写操作命令。

## 前置条件

- 当前工作目录为 Bifrost 仓库根目录。
- TC-SR-01 到 TC-SR-12 为文档/CLI 静态验证，不启动 Bifrost 代理服务，不修改系统代理。
- TC-SR-13 为真实端到端验证，必须启动仓库内 relay server、target Bifrost 与 caller Bifrost CLI，且 target 启动必须禁用系统代理、自动登录弹窗和托盘。
- 使用临时目录验证 skill 安装输出。
- 所有命令显式设置：
  ```bash
  HTTP_PROXY=http://127.0.0.1:9900
  HTTPS_PROXY=http://127.0.0.1:9900
  BIFROST_INSTALL_SKILL_SOURCE=embedded
  ```

## 测试用例列表

### TC-SR-01 安装后 remote skill 可发现

操作步骤：

1. 创建临时目录：
   ```bash
   tmpdir=$(mktemp -d /tmp/bifrost-skill-remote-human.XXXXXX)
   ```
2. 执行安装：
   ```bash
   HTTP_PROXY=http://127.0.0.1:9900 HTTPS_PROXY=http://127.0.0.1:9900 BIFROST_INSTALL_SKILL_SOURCE=embedded \
     cargo run -p bifrost-cli -- install-skill --tool codex --dir "$tmpdir/skills/bifrost" -y
   ```
3. 检查文件：
   ```bash
   test -f "$tmpdir/skills/bifrost/SKILL.md"
   test -f "$tmpdir/skills/bifrost-remote/SKILL.md"
   rg -n 'name: "bifrost-remote"' "$tmpdir/skills/bifrost-remote/SKILL.md"
   ```

预期结果：

- 主 skill 写入 `bifrost/SKILL.md`。
- remote skill 写入 sibling 目录 `bifrost-remote/SKILL.md`。
- remote skill frontmatter 包含 `name: "bifrost-remote"`。

### TC-SR-02 description 表达远程设备控制能力

操作步骤：

1. 在安装产物中检查 description 和启动指引：
   ```bash
   rg -n '远程操作另一台电脑' "$tmpdir/skills/bifrost-remote/SKILL.md"
   rg -n '远端 shell 执行|remote exec' "$tmpdir/skills/bifrost-remote/SKILL.md"
   rg -n '系统代理默认开' "$tmpdir/skills/bifrost-remote/SKILL.md"
   ```

预期结果：

- description 明确 remote 用于远程操作另一台电脑。
- description 明确可通过 `remote exec` 操作目标设备。
- description 明确目标端启动时系统代理默认开启。

### TC-SR-03 目标端启动指引默认使用正式实例

操作步骤：

1. 检查目标端启动指引：
   ```bash
   rg -n '^bifrost start$|^bifrost status$|http://127\\.0\\.0\\.1:9900/_bifrost/settings\\?tab=remote-invoke' "$tmpdir/skills/bifrost-remote/SKILL.md"
   ```
2. 检查没有把正式用户启动写成临时端口：
   ```bash
   ! rg -n '9899|\\.bifrost-remote-target' "$tmpdir/skills/bifrost-remote/SKILL.md"
   ```

预期结果：

- 用户正式场景指引为默认 `bifrost start` 和 `bifrost status`。
- Web UI 默认 URL 为 `127.0.0.1:9900`。
- 文档不再推荐正式用户使用 `9899` 或 `.bifrost-remote-target`。

### TC-SR-04 当前子命令边界与 remote exec 替代路径

操作步骤：

1. 检查 remote skill 只列出只读 traffic 子命令：
   ```bash
   rg -n 'remote traffic \\{list,get,search\\}' "$tmpdir/skills/bifrost-remote/SKILL.md"
   ! rg -n 'remote traffic \\{list,get,search,clear\\}|bifrost remote traffic clear|traffic\\.clear' "$tmpdir/skills/bifrost-remote/SKILL.md"
   ```
2. 检查没有专门 remote 子命令的管理面被引导到 `remote exec`：
   ```bash
   rg -n '没有.*专用 relay-backed 子命令|应通过已授权的 `remote exec`' "$tmpdir/skills/bifrost-remote/SKILL.md"
   ```
3. 检查本地管理命令边界：
   ```bash
   rg -n 'bifrost setting .*当前本机|改.*本机配置.*bifrost setting|给远端改 Shell Access policy' "$tmpdir/skills/bifrost-remote/SKILL.md"
   ```

预期结果：

- 文档只把 `list/get/search` 列为 remote traffic 查询能力。
- 文档不提供 `bifrost remote traffic clear` 写操作命令。
- 对 rule/config/script/value/CA/系统代理等目标设备操作，文档引导走已授权的 `remote exec`。
- 文档明确当前本机 Shell Access policy / grant 管理使用 `bifrost setting ...`，管理目标设备时应通过 `remote exec`。

### TC-SR-05 三类能力的前置准备清晰可执行

操作步骤：

1. 检查 remote skill 明确区分三类 scope：
   ```bash
   rg -n '三类能力，三套 scope|只读查询|远端 shell|远端文件' "$tmpdir/skills/bifrost-remote/SKILL.md"
   ```
2. 检查查询类写明目标端如何启用 Remote Invoke 授权：
   ```bash
   rg -n 'SSH key' "$tmpdir/skills/bifrost-remote/SKILL.md"
   rg -n 'Enter Discovery Mode' "$tmpdir/skills/bifrost-remote/SKILL.md"
   rg -n 'Access = `query`|remote_query' "$tmpdir/skills/bifrost-remote/SKILL.md"
   rg -n 'bifrost remote conn status' "$tmpdir/skills/bifrost-remote/SKILL.md"
   ```
3. 检查远程设备控制类写明目标端如何启用 Shell Access policy/profile：
   ```bash
   rg -n 'Shell Access policy' "$tmpdir/skills/bifrost-remote/SKILL.md"
   rg -n 'bifrost setting shell profile add' "$tmpdir/skills/bifrost-remote/SKILL.md"
   rg -n 'bifrost setting shell policy add' "$tmpdir/skills/bifrost-remote/SKILL.md"
   rg -n 'selected' "$tmpdir/skills/bifrost-remote/SKILL.md"
   rg -n 'all' "$tmpdir/skills/bifrost-remote/SKILL.md"
   ```
4. 检查 caller 侧远端 shell 执行示例：
   ```bash
   rg -n 'bifrost remote exec --shell-text "ls -la /tmp"' "$tmpdir/skills/bifrost-remote/SKILL.md"
   ```

预期结果：

- 文档把查询、shell、文件分成三类能力和 scope。
- 查询类包含 SSH key、配对码、`Enter Discovery Mode`、`query` 访问模式和 `remote conn status` 验证。
- 远程设备控制类包含 Shell Access profile/policy 配置示例，并说明需要 `selected` 或 `all` 授权。
- caller 侧有可执行的 `remote exec --shell-text ...` 示例命令。

### TC-SR-06 不包含历史版本迁移文案

操作步骤：

1. 检查安装产物不包含历史别名迁移文案：
   ```bash
   ! rg -n 'deprecated|old `bifrost remote|历史版本|迁移文案' "$tmpdir/skills/bifrost-remote/SKILL.md"
   ```

预期结果：

- remote skill 只描述当前命令面，不要求用户阅读 `deprecated` 或历史别名迁移信息。

### TC-SR-07 CLI 不暴露 remote traffic clear

操作步骤：

1. 检查 CLI help 中不暴露 clear 子命令：
   ```bash
   HTTP_PROXY=http://127.0.0.1:9900 HTTPS_PROXY=http://127.0.0.1:9900 \
     cargo run --bin bifrost -- remote traffic --help
   ```

预期结果：

- help 中包含 `list`、`get`、`search`。
- help 中不包含 `clear`。

### TC-SR-08 远端工程任务先读取工程约束信息

操作步骤：

1. 检查安装产物要求先阅读工作目录下的 AGENTS 手册：
   ```bash
   rg -n '执行任何远端工程任务前|先阅读目标工程约束信息' "$tmpdir/skills/bifrost-remote/SKILL.md"
   rg -n 'AGENTS\\.md|agents\\.md' "$tmpdir/skills/bifrost-remote/SKILL.md"
   ```
2. 检查安装产物要求读取 `.agents/skills/` 下所有 skill 元信息：
   ```bash
   rg -n '\\.agents/skills/.*所有 skill 的元信息|\\.agents/skills/\\*/SKILL\\.md.*元信息' "$tmpdir/skills/bifrost-remote/SKILL.md"
   ```
3. 检查安装产物要求 skill 详细内容按需加载：
   ```bash
   rg -n 'skill 详细.*按需加载|详细正文.*按需加载' "$tmpdir/skills/bifrost-remote/SKILL.md"
   ```

预期结果：

- 文档明确执行任何远端工程任务前必须先读取工作目录下的 `AGENTS.md` / `agents.md`。
- 文档明确必须读取 `.agents/skills/` 下所有 skill 元信息。
- 文档明确 skill 详细正文只在任务命中或实际需要流程时按需加载。

### TC-SR-09 新远程能力与长任务恢复路径可被 coding agent 理解

操作步骤：

1. 检查 skill 明确把长任务从 stream 主路径切到 detach/job：
   ```bash
   rg -n 'exec --detach|remote job list|remote job status|remote job logs|remote job watch|真实远端 exit code' "$tmpdir/skills/bifrost-remote/SKILL.md"
   ```
2. 检查断线恢复优先 job cache 续接，连接身份失效才重建 conn：
   ```bash
   rg -n '断开/切线程/重启 CLI|remote job list|job cache|grant revoked|authorization expired|conn down/up' "$tmpdir/skills/bifrost-remote/SKILL.md"
   ```
3. 检查文档不要求用户手工复制 relay token：
   ```bash
   ! rg -n -- 'remote job .*--relay-token <token>|call_id.*relay_token' "$tmpdir/skills/bifrost-remote/SKILL.md"
   ```
4. 检查 shell 环境和 cwd 语义：
   ```bash
   rg -n -- '--login|BIFROST_REMOTE=1|TERM=dumb|--cwd.*shell rc' "$tmpdir/skills/bifrost-remote/SKILL.md"
   ```
5. 检查 file UX 新能力：
   ```bash
   rg -n 'remote file scratch-dir|file.op_not_permitted|read-many.*policy|/private/tmp|mtime_unix' "$tmpdir/skills/bifrost-remote/SKILL.md"
   ```

预期结果：

- 文档指导 build/test/CI watch 等长任务使用 `remote exec --detach`，再用 `remote job list/status/logs/watch` 续接和获取真实 exit code。
- 文档说明 caller stream 断开、digest mismatch 或本地切线程后不要重启同一长任务，应先用 `remote job list` 找回 `call_id`，再用 call_id-only job 命令续接；不要求用户手工复制 relay token。
- 文档说明 `--login` 仅在需要用户 PATH/rc 时启用，默认路径保持 stdout 干净，`--cwd` 在 shell rc 后仍生效。
- 文档说明临时脚本走 `scratch-dir`，`read-many` 被 policy deny 时降级多次 `read`。

执行记录（2026-06-16）：

- PASS：对仓库源文件 `skill_remote.md` 执行上述关键字检查，命中 `exec --detach`、`remote job status/logs/watch`、`真实远端 exit code`、`断开/切线程/重启 CLI`、`grant revoked`、`authorization expired`、`--login`、`BIFROST_REMOTE=1`、`TERM=dumb`、`remote file scratch-dir`、`file.op_not_permitted`、`/private/tmp` 与 `mtime_unix`。

### TC-SR-10 调用异常时主动获取远端最新技能

操作步骤：

1. 检查 skill 明确要求调用异常时主动获取远端最新技能：
   ```bash
   rg -n '调用异常时主动获取远端最新技能|工具调用失败且本地 skill 可能过旧|以远端最新文档为准' "$tmpdir/skills/bifrost-remote/SKILL.md"
   ```
2. 检查 skill 写入 GitHub 权威入口链接：
   ```bash
   rg -n 'https://github.com/bifrost-proxy/bifrost/blob/main/skill_remote\\.md' "$tmpdir/skills/bifrost-remote/SKILL.md"
   ```
3. 检查 skill 提供可直接读取原文的 raw 地址：
   ```bash
   rg -n 'https://raw\\.githubusercontent\\.com/bifrost-proxy/bifrost/main/skill_remote\\.md' "$tmpdir/skills/bifrost-remote/SKILL.md"
   ```

预期结果：

- 文档要求 Agent 在 `bifrost remote` 相关调用异常、本地 skill 可能过旧时，主动获取远端最新 `skill_remote.md`。
- 文档包含用户提供的 GitHub 权威入口链接。
- 文档包含可直接读取最新原文的 raw 地址，便于无浏览器环境下刷新技能内容。
- PASS（2026-06-17）：对仓库源文件 `skill_remote.md` 执行更新后的关键字检查，命中 `remote job list`、`job cache`、call_id-only `status/logs/watch`，且未命中 `remote job ... --relay-token <token>`。

### TC-SR-11 `--from-local` 本地 payload 能力出现在安装后的 remote skill

操作步骤：

1. 检查安装后的 remote skill 说明 `write/edit/patch` 都支持 caller 本地 payload：
   ```bash
   rg -n -- '--from-local.*write|write.*--from-local|edit.*--from-local|patch.*--from-local' "$tmpdir/skills/bifrost-remote/SKILL.md"
   ```
2. 检查文档明确 `mkdir/move/delete` 不使用 `--from-local`：
   ```bash
   rg -n 'mkdir.*move.*delete.*没有 caller 本地 payload|mkdir.*move.*delete.*不使用 `--from-local`' "$tmpdir/skills/bifrost-remote/SKILL.md"
   ```
3. 检查仓库源文件也包含同样说明：
   ```bash
   rg -n -- '--from-local.*write|write.*--from-local|edit.*--from-local|patch.*--from-local' skill_remote.md
   rg -n 'mkdir.*move.*delete.*没有 caller 本地 payload|mkdir.*move.*delete.*不使用 `--from-local`' skill_remote.md
   ```

预期结果：

- 安装后的 `bifrost-remote/SKILL.md` 明确告诉 Agent：`write --from-local` 读取本地文件内容，`edit --from-local` 读取 edits JSON，`patch --from-local` 读取 unified diff。
- 文档明确 `mkdir` / `move` / `delete` 没有 caller 本地 payload，避免 Agent 误以为所有写操作都需要或支持本地路径。

执行记录（2026-06-27）：

- PASS：执行 `tmpdir=$(mktemp -d); BIFROST_INSTALL_SKILL_SOURCE=embedded cargo run -p bifrost-cli -- install-skill --tool codex --dir "$tmpdir/skills/bifrost" -y` 后，`$tmpdir/skills/bifrost-remote/SKILL.md` 存在；`rg -n -- '--from-local.*write|write.*--from-local|edit.*--from-local|patch.*--from-local' "$tmpdir/skills/bifrost-remote/SKILL.md"` 命中 `write/edit/patch` 命令面、行为要点和入口选择；`rg -n 'mkdir.*move.*delete.*没有 caller 本地 payload|mkdir.*move.*delete.*不使用 `--from-local`' "$tmpdir/skills/bifrost-remote/SKILL.md"` 命中本地 payload 边界说明。

### TC-SR-12 remote skill 文档与当前 CLI 命令面保持一致

操作步骤：

1. 检查 traffic 输出格式文档使用 `--format` 而不是 `--output`：
   ```bash
   rg -n '输出格式走 `-f\\|--format`|list/get.*table\\|compact\\|json\\|json-pretty|search.*ndjson' skill_remote.md
   ! rg -n '输出格式统一：`--output human\\|json\\|json-pretty`' skill_remote.md
   target/debug/bifrost remote traffic list --help | rg -- '-f, --format <FORMAT>'
   target/debug/bifrost remote traffic search --help | rg -- 'ndjson'
   ```
2. 检查 `remote traffic search` 只承诺核心过滤项，并明确 `--include` / `--max-body` 需要 `remote exec`：
   ```bash
   rg -n '核心过滤项|`--include` / `--max-body`' skill_remote.md
   target/debug/bifrost search --help | rg -- '--include|--max-body'
   ! target/debug/bifrost remote traffic search --help | rg -- '--include|--max-body'
   ```
3. 检查 file 子命令 synopsis 包含已实现参数：
   ```bash
   rg -n 'file list.*--max-matches.*--cursor|file glob.*--max-matches.*--cursor|file find.*--cursor|file move.*--base-sha256|file patch.*--base-sha' skill_remote.md
   target/debug/bifrost remote file list --help | rg -- '--max-matches|--cursor'
   target/debug/bifrost remote file glob --help | rg -- '--cursor|--exclude'
   target/debug/bifrost remote file find --help | rg -- '--cursor'
   target/debug/bifrost remote file move --help | rg -- '--base-sha256|--allow-overwrite'
   target/debug/bifrost remote file patch --help | rg -- '--base-sha'
   ```
4. 检查 `--around` 文档与实现一致，明确覆盖 `-A/-B`：
   ```bash
   rg -n '`--around`.*覆盖.*-B/-A|需要非对称上下文时只用 `-B/-A`' skill_remote.md
   target/debug/bifrost remote file find --help | rg -- 'takes precedence over -A/-B'
   ```
5. 检查错误码表覆盖 FileAccess、anchored edit、patch 特殊错误和 IO 错误：
   ```bash
   rg -n 'file.deny_pattern|file.symlink_escape|file.ignored_by_gitignore|file.anchor_not_found|file.anchor_not_unique|file.binary_patch_unsupported|file.unsupported_diff|file.io_error' skill_remote.md
   ```
6. 检查二进制 / 本地大文件推荐 `--from-local`，且不再推荐 macOS 不兼容的 `base64 -w0`：
   ```bash
   rg -n '本地大文件 / 二进制 / 特殊字符优先用 `--from-local`|write <remote-path> --from-local ./local.bin' skill_remote.md
   ! rg -n 'base64 -w0' skill_remote.md
   ```

预期结果：

- 文档中的 traffic 输出参数与当前 CLI help 一致，使用 `-f|--format`。
- 文档不再过度承诺 remote search 支持 `--include` / `--max-body`。
- 文档列出的 remote file 参数覆盖当前 help 中的分页、乐观锁和 overwrite 保护参数。
- `--around` 语义与实现一致：存在时覆盖 `-A/-B`。
- 错误码表包含当前实现会返回、且 Agent 需要处理的主要 file 错误码。
- 本地大文件和二进制写入推荐 `--from-local`，不依赖平台特定 `base64 -w0`。

执行记录（2026-06-28）：

- PASS：执行 TC-SR-12 的静态文档断言，确认 `skill_remote.md` 使用 `-f|--format` 描述 remote traffic 输出，未再出现旧的 `--output human|json|json-pretty` 说法；remote search 只承诺核心过滤项并明确 `--include` / `--max-body` 需走 `remote exec`；file synopsis 包含 list/glob/find 分页、move 乐观锁/overwrite、patch `--base-sha`；`--around` 文档写明覆盖 `-A/-B`；错误码表包含 deny/symlink/gitignore/anchor/binary patch/unsupported diff/io；文档中不再出现 `base64 -w0`。
- PASS：执行当前源码构建出的 `target/debug/bifrost --version`，输出 `bifrost 0.0.124`；随后执行 `remote traffic list/search --help`、本地 `search --help`、`remote file list/glob/find/move/patch/write/edit --help`，确认 help 中暴露 `--format`、search `ndjson`、本地 search `--include/--max-body` 而 remote search 不暴露、file 分页/乐观锁/`--from-local`/`--base-sha` 参数与文档一致。
- PASS：执行离线参数解析验证，在临时 `BIFROST_DATA_DIR` 下依次运行 `remote traffic list --format json`、`remote traffic search --format ndjson`、`remote file list --max-matches --cursor`、`remote file glob --cursor --exclude`、`remote file find --around -A --cursor`、`remote file write --from-local`、`remote file edit --from-local`、`remote file patch --from-local --base-sha`、`remote file move --base-sha256 --allow-overwrite`。所有命令都通过 clap 参数解析并在无远端连接阶段失败，未出现 `unexpected argument` / `unrecognized subcommand` / `invalid value` / 缺 required argument 等文档参数错误。
- PASS：执行 `BIFROST_INSTALL_SKILL_SOURCE=embedded cargo run -q -p bifrost-cli --bin bifrost -- install-skill --tool codex --dir "$tmpdir/skills/bifrost" -y` 安装到临时目录，确认 `$tmpdir/skills/bifrost-remote/SKILL.md` 包含更新后的 `--format`、`--include` / `--max-body` 边界、`--around` 覆盖语义、`file.binary_patch_unsupported`、patch `--base-sha` 与 `write --from-local ./local.bin` 推荐；同时确认安装产物不含 `base64 -w0` 和旧 `--output human|json|json-pretty` 说法。测试完成后删除临时目录。

### TC-SR-13 remote skill 全量用法真实端到端验收

操作步骤：

1. 使用当前仓库源码编译出的 `target/debug/bifrost`，通过仓库内 relay server 启动真实三端链路：
   - relay：`packages/bifrost-sync-server`
   - target：临时 `BIFROST_DATA_DIR`、`--no-system-proxy`、`BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`、`BIFROST_DISABLE_TRAY=1`
   - caller：独立临时 `BIFROST_DATA_DIR`
2. 执行 remote file relay E2E，逐项覆盖 `skill_remote.md` 推荐的 file surface：
   ```bash
   NODE_BIN="$(command -v node)" \
   BIFROST_BIN="$PWD/target/debug/bifrost" \
   SKIP_BUILD=true \
   REMOTE_FILE_CMD_TIMEOUT_SECS=45 \
   BIFROST_DISABLE_TRAY=1 \
   BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 \
   bash e2e-tests/tests/test_remote_file_relay_e2e.sh
   ```
3. 执行 remote invoke E2E，覆盖 `conn up --label/status/down`、`traffic list/get/search`、取消、断连清理和 relay token 安全：
   ```bash
   NODE_BIN="$(command -v node)" \
   BIFROST_BIN="$PWD/target/debug/bifrost" \
   SKIP_BUILD=true \
   BIFROST_DISABLE_TRAY=1 \
   BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 \
   bash e2e-tests/tests/test_remote_invoke_e2e.sh
   ```
4. 执行 SSH remote invoke E2E，覆盖 `--ssh-key` 文件、`BIFROST_REMOTE_SSH_KEY`、SSH grant 权限升降级、同一 SSH key 多 caller identity 隔离，以及通过 `remote exec` 调目标端本机 Bifrost CLI 的 fallback：
   ```bash
   NODE_BIN="$(command -v node)" \
   BIFROST_BIN="$PWD/target/debug/bifrost" \
   SKIP_BUILD=true \
   BIFROST_DISABLE_TRAY=1 \
   BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 \
   bash e2e-tests/tests/test_remote_invoke_ssh_e2e.sh
   ```
5. 执行 shell streaming E2E，覆盖 `remote exec --shell-text`、argv、stdin、`--cwd`、`--env`、`--login`、`--timeout-ms`、`BIFROST_REMOTE=1`、`TERM=dumb` 与 Recent Calls 元数据：
   ```bash
   NODE_BIN="$(command -v node)" \
   BIFROST_BIN="$PWD/target/debug/bifrost" \
   SKIP_BUILD=true \
   BIFROST_DISABLE_TRAY=1 \
   BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 \
   bash e2e-tests/tests/test_remote_shell_exec_streaming_e2e.sh
   ```
6. 执行 remote job / run E2E，覆盖 `remote exec --detach`、`remote job list/status/logs/watch --output-file`、call_id-only job cache、真实 exit code，以及 `remote run --script-file --interpreter --cwd --env --detach -- <args>`：
   ```bash
   NODE_BIN="$(command -v node)" \
   BIFROST_BIN="$PWD/target/debug/bifrost" \
   SKIP_BUILD=true \
   BIFROST_DISABLE_TRAY=1 \
   BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 \
   bash e2e-tests/tests/test_remote_job_real_e2e.sh
   ```
7. 执行 CLI tooling E2E，覆盖 `remote run` help、`BIFROST_REMOTE_CLIENT_ID`、`--client-id`、`file write --path` 兼容和 `write/edit/patch --from-local` 参数解析：
   ```bash
   NODE_BIN="$(command -v node)" \
   BIFROST_BIN="$PWD/target/debug/bifrost" \
   SKIP_BUILD=true \
   BIFROST_DISABLE_TRAY=1 \
   BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 \
   bash e2e-tests/tests/test_remote_cli_tooling_e2e.sh
   ```
8. 对 `read-many` 回归运行最小 Rust 测试，确认服务端 handler 仍支持部分失败和总量限制：
   ```bash
   cargo test -p bifrost-admin read_many --lib -- --nocapture
   ```

预期结果：

- remote file E2E 启动真实 relay/target/caller 后全部通过，且至少包含：
  - `read/list/stat/hash/write/mkdir/move/delete/glob/find/edit/patch`
  - `write/edit/patch --from-local` 后通过 remote read 和 target disk `cmp -s` 做 byte-for-byte 校验
  - `read-many`、`scratch-dir`、`outline`
  - `find -B/-A`、`--exclude`、`.gitignore`、CRLF、CJK、sha mismatch、readonly scope 等边界
- remote invoke E2E 全部通过，说明文档中的连接、label、traffic 和断连恢复推荐用法可用。
- SSH E2E 全部通过，且通过 `remote exec` 在目标端本机执行并校验：
  - `bifrost status --format json`
  - `bifrost search <marker> --include bodies,headers --max-body 32768 --format ndjson`
  - `bifrost traffic get --ids <id> --max-body 32768 --format ndjson`
  - `bifrost traffic auth-status <id> --format json`
  - `bifrost traffic export <id> --as curl`
  - `bifrost traffic replay <id> --patch /body/x=1 --refresh-auth --format json`
  - `bifrost capture wait --host ... --timeout 1s --format json`，并确认远端 124 timeout exit code 经 `remote exec --timeout-ms` 保留
- shell streaming E2E 全部通过，说明文档中的 shell stdout streaming、stdin、`--cwd`、`--env`、`--login`、`--timeout-ms` 和远端环境变量说明可用。
- remote job / run E2E 全部通过，说明文档中的 detach/job 续接和本地脚本上传到远端执行路径可用。
- CLI tooling E2E 全部通过，说明文档中的本地参数写法没有 clap/命令面漂移。
- `read-many` 不再触发目标端 stack overflow。

执行记录（2026-06-28）：

- FAIL → FIX → PASS：新增 `read-many/scratch-dir/outline` 真实用例后，首次发现 `read-many` 在真实 target tokio worker 中触发 `thread 'tokio-rt-worker' has overflowed its stack` 并导致目标端 `Abort trap: 6`；修复 `handle_file_read_many`，将 per-file `handle_file_read` future 与 outer future boxing，重新编译 `target/debug/bifrost` 后复跑通过。
- PASS：`cargo test -p bifrost-admin read_many --lib -- --nocapture`，3 passed。
- PASS：`cargo build -p bifrost-cli --bin bifrost` 成功生成新 `target/debug/bifrost`。
- PASS：`e2e-tests/tests/test_remote_file_relay_e2e.sh` 使用真实 relay/target/caller 通过，汇总 `Total: 89, Passed: 89, Failed: 0`；其中 `TC-FILE-13B` 对 `write/edit/patch --from-local` 均通过 remote read 与目标端磁盘文件逐字节比较，`TC-FILE-21/22/23` 覆盖 `read-many/scratch-dir/outline`。
- PASS：`e2e-tests/tests/test_remote_invoke_e2e.sh` 使用真实 relay/target/caller 通过，汇总 `total=73 passed=73 failed=0`；新增 `TC-RI-01A` 验证 `remote conn up --label` 真实到达目标端 pending pairing。
- PASS：`e2e-tests/tests/test_remote_invoke_ssh_e2e.sh` 使用真实 relay/target/caller 和本地 mock target 通过；验证 SSH key 文件路径、`BIFROST_REMOTE_SSH_KEY`、grant 权限升降级、两个 caller fingerprint 隔离、remote traffic search/get，并通过 `remote exec` 调目标端本机 CLI 验证 `status/search --include/traffic get --ids/auth-status/export/replay --patch --refresh-auth/capture wait` 的真实输出和 marker 数据；其中 replay 用 POST JSON 流量验证 `--patch /body/x=1` 后目标端 echo 的 body 变为 `x=1`。
- PASS：`e2e-tests/tests/test_remote_shell_exec_streaming_e2e.sh` 使用真实 relay/target/caller 通过，汇总 `Total: 51, Passed: 51, Failed: 0`；验证 shell_text/argv/stdin streaming，以及 `--cwd` 在 login shell rc 后仍生效、`--env` 到达远端进程、`BIFROST_REMOTE=1`、`TERM=dumb` 与 Recent Calls policy/exec_mode/stdout_digest。
- PASS：`e2e-tests/tests/test_remote_job_real_e2e.sh` 使用真实 relay/target/caller 通过，汇总 `Total: 76, Passed: 76, Failed: 0`；验证 detach job 的 call_id-only status/logs/watch、`--output-file`、真实 exit code、local job cache 加密 token，以及 `remote run` 本地脚本上传、`--cwd`、`--env`、`--detach` 和 `-- <args>`。
- PASS：`e2e-tests/tests/test_remote_cli_tooling_e2e.sh` 通过，确认 `remote run`、client-id/env fallback、`file write --path` 兼容与 `write/edit/patch --from-local` 参数面可用。

## 清理步骤

```bash
rm -rf "$tmpdir"
```
