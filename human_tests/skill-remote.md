# Bifrost Remote Skill 真实场景测试

## 功能模块说明

验证用户通过 `bifrost install-skill` 安装技能后，能够获得独立的 `bifrost-remote` skill，并且该 skill 正确表达 Remote Invoke 的远程设备控制能力、目标端默认启动方式、查询/shell/文件三类 scope 的前置准备、当前 relay-backed 子命令边界、`remote exec` 的授权操作路径、远端工程任务开始前必须读取工程约束信息的要求，不包含历史版本迁移文案，并且不提供 `remote traffic clear` 写操作命令。

## 前置条件

- 当前工作目录为 Bifrost 仓库根目录。
- 不启动 Bifrost 代理服务，不修改系统代理。
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
   rg -n 'exec --detach|remote job status|remote job logs|remote job watch|真实远端 exit code' "$tmpdir/skills/bifrost-remote/SKILL.md"
   ```
2. 检查断线恢复优先 job 续接，连接身份失效才重建 conn：
   ```bash
   rg -n '断开/切线程/重启 CLI|优先用 job|grant revoked|authorization expired|conn down/up' "$tmpdir/skills/bifrost-remote/SKILL.md"
   ```
3. 检查 shell 环境和 cwd 语义：
   ```bash
   rg -n -- '--login|BIFROST_REMOTE=1|TERM=dumb|--cwd.*shell rc' "$tmpdir/skills/bifrost-remote/SKILL.md"
   ```
4. 检查 file UX 新能力：
   ```bash
   rg -n 'remote file scratch-dir|file.op_not_permitted|read-many.*policy|/private/tmp|mtime_unix' "$tmpdir/skills/bifrost-remote/SKILL.md"
   ```

预期结果：

- 文档指导 build/test/CI watch 等长任务使用 `remote exec --detach`，再用 `remote job status/logs/watch` 续接和获取真实 exit code。
- 文档说明 caller stream 断开、digest mismatch 或本地切线程后不要重启同一长任务，应凭 `call_id`/`relay_token` 续接。
- 文档说明 `--login` 仅在需要用户 PATH/rc 时启用，默认路径保持 stdout 干净，`--cwd` 在 shell rc 后仍生效。
- 文档说明临时脚本走 `scratch-dir`，`read-many` 被 policy deny 时降级多次 `read`。

执行记录（2026-06-16）：

- PASS：对仓库源文件 `skill_remote.md` 执行上述关键字检查，命中 `exec --detach`、`remote job status/logs/watch`、`真实远端 exit code`、`断开/切线程/重启 CLI`、`grant revoked`、`authorization expired`、`--login`、`BIFROST_REMOTE=1`、`TERM=dumb`、`remote file scratch-dir`、`file.op_not_permitted`、`/private/tmp` 与 `mtime_unix`。

## 清理步骤

```bash
rm -rf "$tmpdir"
```
