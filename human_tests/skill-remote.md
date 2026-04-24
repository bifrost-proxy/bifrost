# Bifrost Remote Skill 真实场景测试

## 功能模块说明

验证用户通过 `bifrost install-skill` 安装技能后，能够获得独立的 `bifrost-remote` skill，并且该 skill 正确表达 Remote Invoke 的远程设备控制能力、目标端默认启动方式、只读查询与远程控制两类操作的前置准备、当前 relay-backed 子命令边界，以及 `remote command exec` 的授权操作路径。

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

1. 在安装产物中检查 description：
   ```bash
   rg -n '远程设备控制能力|remote command exec 操作目标设备|开启系统代理' "$tmpdir/skills/bifrost-remote/SKILL.md"
   ```

预期结果：

- description 明确 remote 用于远程设备控制能力。
- description 明确可通过 `remote command exec` 操作目标设备。
- description 明确目标端启动应开启系统代理。

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

### TC-SR-04 当前子命令边界指向 remote command exec 替代路径

操作步骤：

1. 检查 remote skill 不提供 `bifrost remote traffic clear` 可执行示例：
   ```bash
   ! rg -n 'bifrost remote traffic clear' "$tmpdir/skills/bifrost-remote/SKILL.md"
   ```
2. 检查写操作被引导到 `remote command exec`：
   ```bash
   rg -n '不代表 Agent 不能操作目标设备|remote command exec.*目标机命令|traffic clear.*remote command exec' "$tmpdir/skills/bifrost-remote/SKILL.md"
   ```
3. 检查本地管理命令边界：
   ```bash
   rg -n 'remote shell .*当前机器本地管理命令|remote grant .*当前机器本地管理命令|caller 要管理目标设备' "$tmpdir/skills/bifrost-remote/SKILL.md"
   ```

预期结果：

- 文档不提供不可用的 `bifrost remote traffic clear` 直接执行示例。
- 对 rule/config/script/value/CA/系统代理等目标设备操作，文档引导走已授权的 `remote command exec`。
- 文档明确 caller 本地 `remote shell` / `remote grant` 不是 relay-backed 管理 API，管理目标设备时应通过 `remote command exec`。

### TC-SR-05 两类操作的前置准备清晰可执行

操作步骤：

1. 检查 remote skill 明确区分两类操作：
   ```bash
   rg -n '两类操作的前置准备工作|只读查询类|远程设备控制类' "$tmpdir/skills/bifrost-remote/SKILL.md"
   ```
2. 检查只读查询类写明目标端如何启用 Remote Invoke 授权：
   ```bash
   rg -n 'SSH key' "$tmpdir/skills/bifrost-remote/SKILL.md"
   rg -n 'Enter Discovery Mode' "$tmpdir/skills/bifrost-remote/SKILL.md"
   rg -n 'query.*访问模式' "$tmpdir/skills/bifrost-remote/SKILL.md"
   rg -n 'bifrost remote status' "$tmpdir/skills/bifrost-remote/SKILL.md"
   ```
3. 检查远程设备控制类写明目标端如何启用 Shell Access policy/profile：
   ```bash
   rg -n 'Shell Access policy' "$tmpdir/skills/bifrost-remote/SKILL.md"
   rg -n 'remote shell profile add' "$tmpdir/skills/bifrost-remote/SKILL.md"
   rg -n 'remote shell policy add' "$tmpdir/skills/bifrost-remote/SKILL.md"
   rg -n 'selected' "$tmpdir/skills/bifrost-remote/SKILL.md"
   rg -n 'all' "$tmpdir/skills/bifrost-remote/SKILL.md"
   ```
4. 检查 caller 侧验证命令：
   ```bash
   rg -n 'bifrost remote command exec --shell-text "bifrost status"' "$tmpdir/skills/bifrost-remote/SKILL.md"
   ```

预期结果：

- 文档把只读查询和远程设备控制分成两类前置准备。
- 只读查询类包含 SSH key、配对码、`Enter Discovery Mode`、`query` 访问模式和 `remote status` 验证。
- 远程设备控制类包含 Shell Access profile/policy 配置示例，并说明需要 `selected` 或 `all` 授权。
- caller 侧有可执行的 `remote command exec --shell-text "bifrost status"` 验证命令。

## 清理步骤

```bash
rm -rf "$tmpdir"
```
