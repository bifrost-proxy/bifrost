---

name: "bifrost-remote"
description: "通过 Bifrost Remote Invoke 远程操作另一台电脑：连接管理、状态/流量查询、远端 shell 执行；以及对目标仓库做 coding-agent 级的文件读写/搜索/原子 edit/批量 patch（受 FileAccessPolicy 约束）。触发词包括：连接另一台电脑、远程执行命令、远程改代码、远端仓库编辑/重构/批量修改文件、在远端机器上跑 coding agent、远程 grep/find/read/write/edit/patch。重要：修改远端文件必须优先使用 bifrost remote file 子命令。"

---

# Bifrost Remote

本技能指导 Agent 通过 `bifrost remote` 与远端 Bifrost 建立连接，并完成五类操作：**连接管理**（`conn`）、**远端 shell 执行**（`exec`）、**远端长任务续接**（`job`）、**远端文件编程**（`file`，coding-agent 友好）、**远端流量查询**（`traffic`）。

---

## 黄金法则：修改远端文件用 `remote file`，不要用 `remote exec + base64`

这是本技能的第一原则。Agent 经常会踩这个坑——用 `remote exec --shell-text "echo '$B64' | base64 -d > /path/to/file.rs"` 去改代码，这种做法有 6 个已知缺陷：

1. 没有原子写，失败后留下半文件。
2. 没有乐观锁，覆盖其他人并发修改时静默丢失。
3. 要经过 Shell Access policy 审计，污染审计日志。
4. 被目标端 `.gitignore` / FileAccessPolicy `denies` 命中时，shell 能写但违背用户预期。
5. 跨平台 quoting 噩梦（Windows CRLF、特殊字符、`$` 展开）。
6. 无法带 `base_sha256` 一致性校验，不适合多 agent 协作。

**正确做法**：直接使用 `bifrost remote file` 子命令。它带原子 tmp+rename、sha256 乐观锁、gitignore 感知、错误码契约、binary/text 自动识别、CRLF 保留、base64 传输等全部能力，由服务端实现而非客户端拼 shell。

下面是 Agent 在各种"改远端文件"任务下应该选的子命令：

| Agent 的意图 | 选哪个命令 | 不要这么做 |
|---|---|---|
| 看一下远端某文件 | `remote file read <path>` | `remote exec --shell-text "cat <path>"` |
| 一次读多个文件 | 先试 `remote file read-many --path a --path b ...`；若 policy deny，回落多次 `read` | `shell-text "cat a b c"` |
| 列目录树、找文件名 | `remote file list` / `remote file glob` | `shell-text "find ..." / "ls -R"` |
| 正则搜代码、定位符号 | `remote file find <regex>` | `shell-text "grep -rn ..."` |
| 看一个源文件有哪些符号（函数/类/结构体） | `remote file outline <path>` | 人肉 `read` 整文件再扫一遍 |
| 多关键词 OR 搜 / 字面量 / 整词 | `remote file find -e p1 -e p2 [--fixed-strings] [--word]` | 多次 grep 后人肉合并 |
| 搜到点想看上下文 | `remote file find <regex> --around 3` | 再单独 `read` 那几行 |
| 写一个短文本文件 | `remote file write <path> --content "..." --create-parents` | `shell-text "echo ... > file"` |
| 从本地文件写过去 | `remote file write <path> --content-file ./local.txt --create-parents` | `scp` / `echo`+重定向 |
| 放临时脚本 / 日志 | `remote file scratch-dir [--name .bifrost-tmp]` 后写入返回目录 | 写 `/tmp/...` 或 `target/...` 反复撞 FileAccessPolicy |
| 改已有文件的几行（按行号） | `remote file edit --base-sha256 <sha> --edits '[{"start_line":..}]'` | `sed -i` / `echo`+重定向 |
| 按内容锚点改（不数行号） | `remote file edit --edits '[{"old_string":"..","new_string":".."}]'` | `sed`/正则替换 |
| 多文件统一 patch（含改名/复制） | `remote file patch --patch-file ./diff.patch` | 循环 shell-text 的 sed |
| 创建目录 | `remote file mkdir --parents` | `shell-text "mkdir -p ..."` |
| 移动/删除 | `remote file move` / `remote file delete --recursive` | `shell-text "mv" / "rm -rf"` |
| 传输大文件 / 二进制 / 特殊字符 | `remote file write --content-b64 "$(base64 < ./blob.bin)"` | echo 管道 base64 |
| 校验文件哈希 | `remote file hash <path> --algo sha256` | `shell-text "shasum ..."` |

**只有下列场景才回落到 `remote exec`**：
- 跑测试 / 构建 / 启动脚本（`cargo test`、`npm run build`、`python app.py`）。
- `chmod` / `chown` / `ln -s` / `git` 这种文件元信息或 VCS 操作（`remote file` 不覆盖）。
- 需要运行远端进程本身。长任务优先用 `exec --detach` + `remote job watch`，不要把构建/测试押在一条长期 streaming 连接上。

---

## 一、适用场景

触发本技能的典型表述：

- 操作另一台电脑上的 bifrost：连接、查询状态/流量、远端执行命令。
- 远端改代码、远端仓库重构、在远端机器上跑 coding agent。
- 读远端文件、远程 grep、远程 glob、远程原子 edit、远程批量 patch。
- 用户表述中出现「另一台电脑的项目」「远端文件」「远程 read/write/edit/find」。
- 用户粘贴了 `-----BEGIN BIFROST KEY-----` ... `-----END BIFROST KEY-----` 格式的密钥块，说明用户要 Agent 连接远端 Bifrost（详见 4.1.1 自动连接流程）。

---

## 二、目标终端如何安装并启动 Bifrost

在**目标机**上确认二进制：

```bash
command -v bifrost
bifrost --version
```

若不存在，走官方脚本：

```bash
curl -fsSL https://raw.githubusercontent.com/bifrost-proxy/bifrost/main/install-binary.sh | bash
```

安装脚本默认会自动安装并信任 CA、安装 Bifrost skills，并启动后台服务。随后检查是否已有实例在跑；有就复用，不要启动第二个：

```bash
bifrost status
```

没有则启动（前台运行，系统代理默认开）：

```bash
bifrost start
```

只有测试场景才用临时数据目录、非 9900 端口或 `--no-system-proxy`。

---

## 三、授权准备（三类能力，三套 scope）

| 能力 | UI 勾选项 | 底层 scope | 覆盖的 relay-backed 命令 |
|---|---|---|---|
| 只读查询 | Access = `query` | `remote_query` | `conn status` / `traffic list/get/search` |
| 远端 shell | Access = `selected` 或 `all` | `remote_shell_exec`（加 stdin/interactive 则升级为 `remote_shell_interactive`） | `exec` |
| 远端文件 | File Access = read / read-write | `remote_file_read` / `remote_file_write` | 所有 `file <cmd>` |

三类 scope **互相独立**，请用户在目标端 Web UI 的 Remote Invoke 授权请求里按需勾选。比如 Agent 只能调用 `remote file read` 但没勾 File Access=read-write，`remote file write` 会以 `file.permission_denied` 拒绝。

Shell Access 的 CLI 配置入口是 `bifrost setting shell profile add` 和 `bifrost setting shell policy add`。只有需要执行 `remote exec` 时才配置 Shell Access；纯文件读写优先配置 FileAccessPolicy 并使用 `bifrost remote file`。

`bifrost setting ...` 改的是当前本机配置；给远端改 Shell Access policy、rule、config、script、value、CA 或系统代理等目标设备管理项时，如果没有专用 relay-backed 子命令，应通过已授权的 `remote exec` 在目标设备上执行等价本机命令或 API。

远端流量查询只提供 `bifrost remote traffic {list,get,search}`，不提供清理类写操作。

当 Access 设为 `selected` 时，目标端需要预先配置 shell profile 和 policy 来限定可执行的命令范围：

```bash
# 创建一个 shell profile（定义允许使用的 shell）
bifrost setting shell profile add --id default --name Default \
    --shell /bin/bash --shell /bin/sh

# 创建一个 shell policy（定义命令匹配规则）
bifrost setting shell policy add --id allow-bifrost-cli \
    --name "Allow bifrost CLI" \
    --pattern '^bifrost\s+' --shell /bin/bash --profile default
```

Access = `all` 则跳过策略检查，允许执行任意命令。

### 3.1 建议用 SSH key（长期）

1. 目标端 Web UI `http://127.0.0.1:9900/_bifrost/settings?tab=remote-invoke` → 创建/导出 SSH key。
2. 将 key 文件安全拷到 caller 机器，推荐 `~/.bifrost/remote-device.key`。
3. caller 侧：`bifrost remote conn up --ssh-key ~/.bifrost/remote-device.key`。
4. 回收：目标端 Web UI revoke key 即可。

### 3.2 或用 6 位配对码（首次）

1. 目标端 Web UI → Enter Discovery Mode → 显示 6 位码。
2. caller 侧：`bifrost remote conn up <pair-code>`。
3. 目标端 Web UI 批准请求，勾选需要的能力和时长。
4. 配对码一次性，复用已保存连接。

## 四、Agent 工作流

### 4.0 Caller 侧前置自检（本机 bifrost 就绪）

在执行任何 `bifrost remote` 命令之前，Agent **必须**先确认本机（caller 侧）的 `bifrost` CLI 可用。流程如下：

```bash
# 1. 检查 bifrost 是否已安装
command -v bifrost && bifrost --version
```

- **如果已安装**：直接进入 4.1 连接流程。
- **如果未安装**（`command not found`）：自动执行安装，无需询问用户：

```bash
# 2. 自动安装（macOS / Linux 通用）
curl -fsSL https://raw.githubusercontent.com/nicholasgasior/gbin/master/install.sh | bash -s -- bifrost-proxy/bifrost bifrost

# 3. 验证安装成功
bifrost --version
```

- **如果安装失败**（网络不通、权限不足等）：向用户报告错误，建议手动安装或排查网络。不要在 bifrost 不可用的情况下继续后续步骤。

> **注意**：此自检仅针对 caller（本机），不涉及目标机。目标机的安装由用户自行完成（参见第二节）。

### 4.0.1 调用异常时主动获取远端最新技能

如果执行 `bifrost remote` 相关调用时遇到异常（例如子命令不存在、参数不兼容、协议错误、返回行为与本文不一致、工具调用失败且本地 skill 可能过旧），Agent **必须**主动获取远端最新的 Bifrost Remote skill，再继续排查或重试。

- 权威入口：[skill_remote.md](https://github.com/bifrost-proxy/bifrost/blob/main/skill_remote.md)。
- 需要直接读取原文时，可使用 raw 地址：
  ```bash
  curl -fsSL https://raw.githubusercontent.com/bifrost-proxy/bifrost/main/skill_remote.md
  ```
- 刷新后先核对最新命令面、参数、错误码和恢复流程；如果远端最新文档与本地 skill 冲突，以远端最新文档为准，并在交付中说明本地 skill 可能滞后。
- 不要把“本地 skill 没写”当作功能不存在的证据；调用异常应先确认是否已有远端最新技能更新。

### 4.1 连接（`conn`）

先看本地有没有已保存连接；有就直接查询。没有再走 SSH key / pair code：

```bash
bifrost remote conn status                                      # 已有连接？
bifrost remote conn up --ssh-key ~/.bifrost/remote-device.key   # 或
export BIFROST_REMOTE_SSH_KEY="$(cat ~/.bifrost/remote-device.key)"
bifrost remote conn up --ssh-key                                # CI/自动化固定 env，或
bifrost remote conn up <pair-code>
bifrost remote conn up --ssh-key ~/.bifrost/remote-device.key --label "my-ci-bot"  # 自定义标签
```

**`--label <name>`**：为本次连接指定自定义标签（如 `my-ci-bot`、`eden-macbook`）。标签会显示在远端管理端的 Grants 列表标题区域，方便识别来源。未指定时默认使用本机 hostname。

多连接场景：`--client-id <prefix>` 显式指定目标；非交互环境下必传。

#### 4.1.1 用户粘贴 Bifrost Key 时的自动连接流程

当用户消息中包含如下格式的密钥块时：

```
-----BEGIN BIFROST KEY-----
Device-Code: xxxxxx
<base64 encoded key data>
-----END BIFROST KEY-----
```

Agent **必须**将其视为「用户想要连接到该密钥对应的远端设备」，并按以下步骤自动执行：

1. **提取密钥内容**：将用户消息中 `-----BEGIN BIFROST KEY-----` 到 `-----END BIFROST KEY-----`（含首尾行）之间的完整文本提取出来。
2. **保存为本地密钥文件**：将完整密钥文本（含 BEGIN/END 行）写入 `~/.bifrost/remote-device.key`（如目录不存在则创建）。文件权限建议 `chmod 600`。
3. **检查已有连接**：先执行 `bifrost remote conn status` 确认是否已有可用连接。如果已经连接到同一设备，跳过 connect 步骤。
4. **发起连接**：执行 `bifrost remote conn up --ssh-key ~/.bifrost/remote-device.key`。
5. **确认连接成功**：连接成功后再执行 `bifrost remote conn status` 确认远端可达。
6. **告知用户**：向用户简要报告连接结果（成功/失败及原因）。

**注意事项**：
- **不要让用户手动保存文件**：Agent 应自动完成密钥文件的保存，用户粘贴即代表授权。
- **密钥块中的 `Device-Code` 是 Header 元数据**，标识目标设备，无需单独解析——`bifrost remote conn up` 会自动处理。
- **如果连接失败**，按以下顺序排查：密钥是否完整（BEGIN/END 行是否被截断）→ 目标设备是否在线 → 目标设备是否已 revoke 该 key → 网络连通性。
- **不要将密钥内容输出到日志或回复中**，保存后即可丢弃明文。

### 4.2 远端流量查询（`traffic`，`remote_query` scope）

```bash
bifrost remote conn status
bifrost remote traffic list   --limit 50 [--cursor <c>] [--direction backward|forward] \
    [--method GET] [--status 200] [--status-min 200 --status-max 299] \
    [--protocol http|https|ws|wss|h3] [--host <substr>] [--url <substr>] [--path <substr>] \
    [--content-type <ct>] [--client-ip <ip>] [--client-app <app>] \
    [--has-rule-hit true|false] [--is-websocket true|false] [--is-sse true|false] [--is-tunnel true|false] \
    [-f|--format table|compact|json|json-pretty] [--no-color]
bifrost remote traffic get    <id> [--request-body --response-body]
bifrost remote traffic search <keyword> --max-results 50 --max-scan 200 \
    [--url|--headers|--body|--req-header|--res-body] \
    [--method GET --status 2xx --host example.com --protocol HTTPS]
```

输出格式统一：`--output human|json|json-pretty`，`--no-color` 适合非交互。

总结：`remote traffic {list,get,search}` 涵盖了所有只读流量查询能力。

清理目标设备流量记录属于写操作，不提供对应的 `bifrost remote traffic` 子命令。确需清理时，必须先取得 shell 授权，再用 `bifrost remote exec` 在目标设备上执行本机命令或 API。

### 4.3 远端 shell 执行（`exec`，`remote_shell_exec` scope）

```bash
bifrost remote exec --shell-text "ls -la /tmp"
bifrost remote exec -- ls -la /tmp
bifrost remote exec --cwd <USER_HOME>/work/repo --env FOO=bar --shell-text "echo \$FOO"
bifrost remote exec --timeout-ms 10000 --shell-text "cargo test 2>&1 | tail -30"

# 长任务 / 大输出：一等公民 job 模型
bifrost remote exec --detach --cwd <USER_HOME>/work/repo \
  --timeout-ms 1800000 --shell-text "cargo build --release 2>&1"
bifrost remote job list
bifrost remote job status <call_id>
bifrost remote job logs <call_id> --output-file ./build.log
bifrost remote job watch <call_id> --output-file ./build.log

# 需要用户 PATH / login shell 环境时，显式开启 login shell；CLI 会注入 BIFROST_REMOTE=1、TERM=dumb 抑制常见 shell integration 噪声
bifrost remote exec --login --cwd <USER_HOME>/work/repo \
  --shell-text "cargo --version && pwd"
```

注意：
- `$FOO` 是远端 shell 展开，不是 caller 的 env。caller 的 env 要通过 `--env` 显式传入。
- `--detach` 会立即返回 `call_id`，并把本次 call 的 relay token 加密写入 caller 本地 `remote-jobs.json`；后续只用 `call_id` 执行 `remote job status/logs/watch` 查真实远端进程状态和 exit code，不要手工复制 token。
- `remote job list` 列出 caller 本地已知的 detached job、最近状态、exit code、设备与命令摘要；断开、切线程或重启 CLI 后先用它找回 `call_id`。
- `remote job logs --output-file` / `remote job watch --output-file` 默认做流 digest 校验；遇到 digest mismatch 时先重新 watch/logs 或查看远端日志，不要第一反应加 `--no-verify-digest`。
- `--stream` 仍可用于短命令或临时观察；构建、测试、CI、迁移等长任务默认走 `--detach`。
- `--timeout-ms` 会受到目标端 policy 上限约束；若被 cap，错误信息会包含 requested/policy/capped_by_policy，不能把会话超时信号当作远端进程真实 exit code。
- `--login` 是显式 opt-in。需要 `~/.cargo/bin` / `mise` / `nvm` 等用户 PATH 时使用；`--cwd` 会在 shell rc 之后再次生效，避免 rc 里的 `cd` 覆盖工作目录。

### 4.4 远端文件编程（`file`，`remote_file_*` scope）

**这是本技能的主要价值点。**`remote file` 的完整子命令集：

```bash
# —— 只读（需 remote_file_read）——
bifrost remote file read   <path> [--max-bytes N] [--allow-binary] \
                                   [--offset LINE] [--limit N]
bifrost remote file read-many --path <p1> --path <p2> ... \
                                   [--max-bytes N] [--allow-binary]
bifrost remote file scratch-dir [--cwd <repo>] [--name .bifrost-tmp] \
                                   [--output human|json]
bifrost remote file list   [path] [--depth N] [--no-ignore] [--exclude NAME]...
bifrost remote file stat   <path>
bifrost remote file glob   '<pattern>'  [--max-matches N] [--no-ignore]
bifrost remote file find   ['<regex>']  [-e '<regex>']... \
                                         [--fixed-strings] [--word] \
                                         [--around N] \
                                         [--path <sub>] [--max-matches N] \
                                         [--max-scan N] \
                                         [-B N] [-A N] [-i] [--glob '<pat>'] \
                                         [--no-ignore] [--exclude NAME]...
bifrost remote file hash   <path> [--algo sha256]
bifrost remote file outline <path> [--max-symbols N] [--max-bytes N]

# —— 读写（需 remote_file_write）——
bifrost remote file write  <path> (--content <text>) | (--content-file <local|->) | (--content-b64 <b64>) \
                                   [--base-sha256 SHA] [--allow-overwrite true|false] \
                                   [--create-parents]
bifrost remote file edit   <path> --edits '<json>' [--base-sha256 SHA]
bifrost remote file mkdir  <path> [--parents]
bifrost remote file move   <from> <to>
bifrost remote file delete <path> [--recursive]
bifrost remote file patch  (--patch-file <local|->) | (--patch-b64 <b64>)
```

所有子命令共享 `--cwd <path>`、`--output human|json`、`--relay-url`、`--client-id`。

#### `read-many`：一次往返并发读多个文件

需要同时看一组文件（比如某模块的 5 个源文件）时，不要循环调用 `read`，用 `read-many` 一次取回：

```bash
bifrost remote file read-many \
  --path src/lib.rs --path src/main.rs --path Cargo.toml
```

- 重复 `--path` 指定每个文件，**至少传一个**。
- 服务端**并发**读取，单个文件失败（不存在 / 越权 / 二进制未允许）**不会**中断其余文件——该文件返回单独的错误项，其余正常返回。
- json 模式返回 `{files:[...], count, ok_count}`：`count` 是请求数，`ok_count` 是成功数；每个 `files[i]` 要么带 `content_b64`/`sha256`/`size` 等正文字段，要么带该文件的 `error` 码。
- 每个文件仍受 `--max-bytes` / `--allow-binary` 约束（对全体生效）。
- 如果当前授权 policy 阻断 `read-many`（例如返回 `file.op_not_permitted`），不要卡住：直接回落为多次 `remote file read`，并在交付里说明 policy 未开放批量读。

#### `scratch-dir`：拿一个 policy 内的临时落点

需要在远端放 smoke 脚本、临时日志或小型中间文件时，先创建授权根内的暂存目录，不要猜 `/tmp`、`.git`、`target`：

```bash
bifrost remote file scratch-dir --cwd <USER_HOME>/work/repo
bifrost remote file write .bifrost-tmp/smoke.js --content-file ./smoke.js --create-parents
```

- 默认目录名是 `.bifrost-tmp`，也可以用 `--name <dir>` 指定；它走 `remote file mkdir --parents` 同一套 FileAccessPolicy。
- macOS `/tmp` 常是 `/private/tmp` 的 symlink，容易触发 `file.symlink_escape` 或 `file.out_of_scope`；`target/` 常被 deny pattern 拒绝。
- 任务结束前按需 `remote file delete .bifrost-tmp --recursive` 清理。

#### `outline`：快速拿到一个源文件的符号地图

进入一个陌生源文件前，不要先把整文件 `read` 回来再人肉扫一遍函数/类定义。用 `outline` 一次拿到顶层符号清单（函数、结构体、类、方法、枚举、trait/interface、常量等），按行号定位：

```bash
bifrost remote file outline crates/bifrost-cli/src/cli/remote.rs --max-symbols 50
```

- 服务端**纯解析、零依赖**（基于多语言正则/启发式抽取，不需要远端装 tree-sitter / LSP），跨平台稳定。
- 自动按扩展名识别语言：rust / typescript / javascript / python / go / java / kotlin / c / cpp / ruby / swift / csharp / php；无法识别的语言返回空符号集（`language=unknown`），不报错。
- 默认最多抽取 2000 个符号、扫描前 4 MiB（且不超过授权的 `max_read_bytes`）；超出时 `truncated=true`。`--max-symbols` 收紧上限。
- 二进制文件按 `file.binary_not_allowed` 拒绝（符号抽取无意义）。
- human 模式按 `行号 | 类型 | 签名` 逐行输出，footer 给出 `N symbols (lang[, truncated])`；json 模式返回 `{language, symbols:[{kind,name,line,signature}], count, truncated, total_size, total_lines}`。
- 典型用法：`outline` 拿到符号 + 行号 → 直接 `read --offset <line> --limit N` 精读那一段，或 `edit --base-sha256 ... --edits '[{"start_line":...}]'` 定点改，省掉整文件往返。

#### 按内容锚定编辑（`edit` 的第二种形态）

`edit` 的 `--edits` 现支持两种**互斥**的 item 形态，单次调用只能用其中一种：

1. **行号区间**（原有）：`{"start_line":10,"end_line":12,"replacement":"new\n"}`，1-based 闭区间。
2. **内容锚定**（新增）：`{"old_string":"foo","new_string":"bar","expected_count":1}`，按**字面子串**定位替换，不必数行号。`expected_count` 省略时默认 1；实际命中数与之不符会报错回滚，避免误改。锚定文本被 EOL 归一化到源文件风格。

```bash
# 锚定改：把唯一一处 "old_token" 换成 "new_token"
bifrost remote file edit src/config.rs \
  --edits '[{"old_string":"old_token","new_string":"new_token"}]'

# 一个文件里预期出现 3 次，全部替换
bifrost remote file edit src/x.rs \
  --edits '[{"old_string":"v1","new_string":"v2","expected_count":3}]'
```

判定规则：只要 item 带 `old_string` 即视为锚定模式；不能在同一次调用里混用行号区间和锚定 item。

#### `find` 增强：多模式 OR / 字面量 / 整词 / 上下文 snippet

```bash
# 多关键词 OR（命中任一即返回）：位置参数 + 可重复的 -e
bifrost remote file find 'TODO' -e 'FIXME' -e 'XXX' --glob '*.rs'

# 字面量搜索（pattern 里的正则元字符当普通字符）
bifrost remote file find -e 'a.b(c)' --fixed-strings

# 整词匹配（加词边界 \b...\b，避免 sub-word 误命中）
bifrost remote file find -e 'log' --word

# 对称上下文窗口：每个命中额外回带前后 N 行，合成 snippet 字段
bifrost remote file find 'fn main' --around 3
```

- 位置参数 `<regex>` 与可重复的 `-e/--regex` 可同时给，全部合并为一条**非捕获交替** `(?:p1|p2|...)`。
- `--fixed-strings`：对每个模式做 `regex::escape`，按字面量匹配。
- `--word`：每个模式包成 `\b(?:..)\b`。
- `--around N`：等价同时设置前后上下文，命中处额外返回 `snippet`（含上下文的连续片段）。`--around` 与 `-B/-A` 同时给时以更大的窗口为准。

#### 输出渲染：默认 `human`，结构化字段才加 `--output json`

`remote file` 所有子命令的输出默认就是 **`human`** 格式，开箱就是人类/agent 可读，不必每次都加 `--output json`。

- **`read` 默认直接打印明文正文到 stdout**，可以直接 pipe、直接读、直接 diff，不再需要拿 `content_b64` 自己 base64 解码。文件的 **行数 / 字节数 / sha256 footer** 打印在 **stderr**，所以管道里拿到的 stdout 是干净的文件内容。
- **`list` / `glob` / `find`** 默认逐行打印路径 / `path:line:col: 预览`，末尾在 stderr 给条数统计。
- **`write` / `edit` / `mkdir` / `move` / `delete` / `patch`** 默认打印一行人类可读的结果（如 `Wrote <path> (123 bytes)`），sha256 走 stderr footer。
- **乐观锁 sha 哪里拿**：`read` 的 human 输出会在 stderr footer 打印 `sha256=<...>`，直接抄它喂给后续 `edit --base-sha256` / `write --base-sha256` 即可，不必再切 json。
- **什么时候才需要 `--output json`**：当你要程序化消费结构化字段（如脚本里解析 `total_lines` / `truncated` / `byte_column` / `applied_edits`，或要拿 base64 原文做二进制处理）时，才显式加 `--output json`。json 模式输出原始 JSON，关键字段见下方「行为要点」里 `read --output json` 一条。

#### 行为要点

- **gitignore 默认打开**：`list` / `glob` / `find` 默认跳 `.gitignore` 命中路径。要扫被忽略文件加 `--no-ignore`。
- **`truncated` 自动提示**：`list` / `glob` / `find` / `read` 超限时，human 输出会在 stderr 直接给出**截断的下一步建议**（如"用 --offset/--limit 取更多""收紧 pattern / 提高 --max-matches"）。Agent 据此分片即可，不必自己猜。json 模式下对应字段为 `"truncated": true`。
- **整文件 sha256**：`read` 当 `truncated=true` 时额外带整文件 `file_sha256`，可用于 resume 或乐观锁一致性校验。
- **原子写**：`write` / `edit` / `patch` 采用 tmp+rename，失败自动回滚。`patch` 多文件级原子（任一文件失败全部回滚，已新建的文件也会被 unlink）。
- **`patch` 支持改名 / 复制**：除新增、修改、删除外，`patch` 现支持 `rename from/to`（改名）与 `copy from/to`（复制）形态的 diff，整批同样原子提交、失败整体回滚。
- **乐观锁**：`write` / `edit` 传 `--base-sha256` 后，文件已被改动会返回 `file.sha_mismatch`；Agent 应重新 `read` 再重试，不要盲目覆盖。
- **EOL 保留**：`edit` 自动识别并保留 LF / CRLF 风格；跨风格 replacement 会被归一到目标文件风格。
- **字符列定位**：`find` 返回的 `column` 是**字符列（char-based）**；`byte_column` 是字节偏移，二者在 CJK / 多字节场景会不同。
- **写文件三种入口**：`--content <text>` 内联短 UTF-8 文本（最简单，适合一两行字符串）；`--content-file <local|->` 从本地文件或 stdin；`--content-b64 <b64>` 由 caller 本地 base64、目标端解码。优先级：`--content-b64` > `--content` > `--content-file`。二进制、含 CRLF、含特殊字符的文本走 `--content-b64` 最安全，远比 echo 管道 base64 + shell 重定向可靠。
- **`--create-parents`**：`write` 自带 `mkdir -p`，一次 round-trip 搞定。
- **`edit` 的 `--edits` 严格校验**：传非法 JSON 或非数组会**立即报错并给出示例**（不再静默吞错发 null）。两种形态：行号区间 `[{"start_line":10,"end_line":12,"replacement":"new text\n"}]`（1-based 闭区间），或内容锚定 `[{"old_string":"foo","new_string":"bar","expected_count":1}]`（按字面子串定位，`expected_count` 默认 1，命中数不符则报错）；单次调用两种形态不可混用。
- **错误自带修复建议**：file 操作失败时，CLI 会根据服务端的 `[file.xxx]` 错误码在 stderr 自动追加一行可操作的 `→` 提示（如 sha 不匹配→重新 read 取最新 sha；out_of_scope→用 `scratch-dir` 或让目标端加白名单而非改 cwd）。错误码含义见下方"错误码契约"。
- **Symlink lstat 语义**：`stat` / `list` 不跟随软链；`stat` 额外返回 `symlink_target`。Windows 自动去 `\\?\` / `\\?\UNC\` 前缀。
- **`read --output json` 的关键字段**：`content_b64`（base64 编码正文）、`sha256`（返回切片的 sha）、`size`（返回切片字节数）、`total_size` / `total_lines`（整文件）、`truncated`（是否截断）、`file_sha256`（仅 truncated=true 时出现，整文件 sha，用于后续 `--base-sha256` 乐观锁）、`start_line` / `end_line`（使用 `--offset`/`--limit` 时的范围）、`mtime_unix`。注意 human 模式下这些都已替你渲染好（正文走 stdout、sha/行数/字节数走 stderr footer），只有要程序化解析时才需要 json。

#### 错误码契约

无论 human 还是 json 模式，服务端都会带结构化错误码；human 模式下 CLI 还会自动补一行 `→` 修复建议。

| 错误码 | 含义 | Agent 应对 |
|---|---|---|
| `file.out_of_scope` | 路径在 `roots` 之外 | 临时文件先试 `remote file scratch-dir`；否则请用户在目标端更新 FileAccessPolicy，**不要擅自改 `--cwd`** |
| `file.permission_denied` | 命中 denies，或缺 write scope | 如果是 denies（比如 `.ssh`、`target`），说明用户不想给；如果是缺 scope，请用户重新授权 |
| `file.op_not_permitted` | 当前 grant/policy 未开放该 file op | 按提示降级：如 `read-many` 被 deny，回落多次 `remote file read`；需要长期使用则请用户重授权 |
| `file.binary_not_allowed` | 是二进制但没加 `--allow-binary` | 显式加 `--allow-binary`，或改用 `hash` + 分片 |
| `file.sha_mismatch` | 乐观锁失败 | 重新 `read` + 重算 sha + 重试 |
| `file.not_found` | 路径不存在 | 视任务决定 `mkdir` / `write --create-parents` |
| `file.is_a_directory` / `file.not_a_directory` | 类型不匹配 | 切换子命令 |
| `file.invalid_args` | 参数非法（如 `edit` 行号越界） | 重新 `read` 拿当前行号；`edit` 行号是 1-based 闭区间 |
| `file.invalid_regex` / `file.invalid_glob` | 搜索 / glob 模式非法 | 检查模式语法 |
| `file.precondition_failed` | patch 上下文与当前文件不匹配 | 重新 `read` 目标，针对最新内容重新生成 diff |

#### Coding agent 典型 workflow

默认就走 human 模式，又快又干净；只有需要程序化字段时才在那一步加 `--output json`。

```bash
# 0. 读取目标工程约束信息：先读工作目录下的 AGENTS.md/agents.md，
#    再读取 .agents/skills/*/SKILL.md 的元信息；skill 详细内容按需加载。

# 1. 侦察（human：逐行路径 + 末尾条数；要程序化解析才加 --output json）
bifrost remote file list src --depth 2
bifrost remote file glob 'src/**/*.rs' --max-matches 200

# 2. 定位符号（human：path:line:col: 预览 + 上下文；多关键词用可重复 -e）
bifrost remote file find 'fn handle_file_\w+' --path src --glob '*.rs' --around 2

# 2b. 进陌生文件先看符号地图（函数/类/结构体 + 行号），再按行号精读
bifrost remote file outline src/lib.rs --max-symbols 50

# 3. 读文件（human：明文走 stdout，sha256/行数/字节数走 stderr footer）
bifrost remote file read src/lib.rs            # 直接看到明文；footer 里有 sha256
bifrost remote file read-many --path src/lib.rs --path src/main.rs  # 一次读多个

# 4a. 乐观锁 edit（按行号；sha 抄第 3 步 footer 里的 sha256）
bifrost remote file edit src/lib.rs \
  --base-sha256 <sha-from-step-3> \
  --edits '[{"start_line":10,"end_line":12,"replacement":"// new impl\n"}]'

# 4b. 锚定 edit（按内容，不数行号）
bifrost remote file edit src/lib.rs \
  --edits '[{"old_string":"old_impl()","new_string":"new_impl()"}]'

# 5a. 新建 / 覆盖写——短文本直接内联
bifrost remote file write docs/changelog.md \
  --content "## v1.2.3\n- fix something\n" --create-parents

# 5b. 新建 / 覆盖写——二进制 / 特殊字符统一走 b64
bifrost remote file write assets/logo.png \
  --content-b64 "$(base64 < ./local-logo.png)" \
  --allow-overwrite true --create-parents

# 6. 多文件 patch
bifrost remote file patch --patch-file ./refactor.diff

# 7. 跑测试（这一步才用 exec）
bifrost remote exec --cwd <USER_HOME>/work/github/repo \
  --detach --timeout-ms 1800000 --shell-text "cargo test 2>&1"
bifrost remote job watch <call_id> --output-file ./test.log
```

### 4.5 断开与回收（`conn down`）

```bash
bifrost remote conn down                     # 撤销当前 client 的 grants
bifrost remote conn down --all               # 所有 client
bifrost remote conn down --grant-id <gid>    # 指定 grant
```

### 4.6 连接断开、长任务续接与连接漂移恢复

长任务（build/test/CI watch/迁移）必须默认使用 `exec --detach`。这会把远端进程生命周期和 caller 当前这条网络连接解耦：即使本地终端、SSE stream 或 relay 短连接断开，只要远端 call 仍在，caller 都可以凭本地 job cache 中的 `call_id` 重新接上。

```bash
bifrost remote exec --detach --cwd <repo> \
  --timeout-ms 1800000 --shell-text "cargo test 2>&1"

# 断开/切线程/重启 CLI 后，优先用 job 命令恢复观察
bifrost remote job list
bifrost remote job status <call_id>
bifrost remote job logs <call_id> --output-file ./test.log
bifrost remote job watch <call_id> --output-file ./test.log
```

恢复判断：

- `list` 能看到 caller 本地记住的 detached jobs；如果不知道 `call_id`，先查 `list`。
- `status` 能返回 `running/exited + exit_code` 时，不要重新启动同一长任务；继续 `logs` / `watch`。
- `watch` 会跟到远端终态，并用真实远端 exit code 作为本地退出码。
- `logs` / `watch` 遇到 digest mismatch 时，先重新执行同一个 `job logs/watch` 或查看目标端日志；只有你已独立校验输出完整性时，才考虑 `--no-verify-digest`。
- 只有 `grant revoked`、`authorization expired`、transport identity 变化、relay reusable authorization 变化等连接身份问题，才走下面的 `conn down/up` 重建连接流程。

长会话或服务端重启后，caller 侧有时会看到这类错误：

- `Config error: saved connection transport no longer matches relay reusable authorization; reconnect required`（relay reusable authorization 变了）
- `stream ingest error: OffsetAhead`（流式 session 的 offset 对不上）
- 其他形如 `grant revoked` / `authorization expired` 的错误

统一恢复流程：

```bash
bifrost remote conn down --all
rm -f ~/.bifrost/remote-connections.*
bifrost remote conn up --ssh-key ~/.bifrost/remote-device.key
bifrost remote conn status
```

这一步会清掉本地缓存的 relay token 和 per-connection 状态，用 SSH key 重新建立长连接。不要手工修改 `remote-connections.*` 文件。



## 五、Agent 执行约束（强制）

按优先级阅读：

1. **先读取目标工程约束信息**：执行任何远端工程任务前，必须先阅读工作目录下的 `AGENTS.md` / `agents.md` 手册；然后读取 `.agents/skills/` 下所有 skill 的元信息（如 frontmatter、名称、描述、触发条件和路径）。skill 详细正文只在任务实际命中或需要其流程时按需加载，避免把无关细节塞进上下文。
2. **先用 `remote file`，再考虑 `remote exec`**。任何修改远端文件内容的操作，先看是否能用 `remote file write/edit/move/delete/mkdir/patch` 完成。**严禁**用 `exec --shell-text "echo '$B64' | base64 -d > ..."` 这类 shell 拼接写文件。违反此条 = 违反本技能。
3. **默认 human 输出，不要无脑加 `--output json`**：`remote file` 默认就是人类可读，`read` 直接给明文、错误自带修复建议、截断自带下一步。只有要在脚本里解析结构化字段（`total_lines` / `truncated` / `byte_column` / `applied_edits` 等）或处理二进制原文时才加 `--output json`。
4. **不要重复 `remote conn up`**：先跑 `bifrost remote conn status` 看已有连接是否可用。
5. **SSH key 优于 pair code**：有 key 先用 key，一次连接永久复用（直到 key 被 reset/revoke）。
6. **多 client 场景**：显式 `--client-id <prefix>`，不要依赖交互式选择。
7. **失败分类**（错误信息里 CLI 已自动带 `→` 修复建议，先读它）：
   - `file.out_of_scope` / `file.permission_denied` → 告诉用户需要调 FileAccessPolicy，不自作主张改 `--cwd` 绕过。
   - `file.sha_mismatch` → 重 read 重算 sha 再重试。
   - connect 失败 → 检查 SSH key 有效性、pair code 是否过期、目标是否在线、Web UI 是否授权。grant 失效就重新 `conn up`，**不要**伪造本地连接文件。
8. **本机 vs 远端不要混**：改本机走 `bifrost setting`；改远端走 `bifrost remote`。
9. **不要承诺 OS 级 sandbox**：`exec` 是 Shell Access policy 级限制，不是 sandbox。
10. **长任务必须优先 detach**：构建、测试、CI watch、数据库迁移等用 `remote exec --detach`，再用 `remote job list` 找到本地记录的 call，并用 `remote job watch/logs/status <call_id>` 追踪真实 exit code。`--stream --output-file` 只适合短观察；出现 digest mismatch / 143 / wall-clock timeout 时，不要盲目重试，改走 job 模型。
11. **写文件入口选择**：短文本用 `--content`；本地文件 / stdin 用 `--content-file`；二进制 / 特殊字符 / 大文件用 `--content-b64`。避免 echo 管道 base64。
12. **临时文件先 scratch-dir**：不要写 `/tmp`、`.git` 或 `target` 试运气；用 `remote file scratch-dir` 获取 policy 内落点。
13. **只读先行**：在做 write 之前，至少 `list` + `read` 侦察一次，别盲写。

---

## 六、FAQ

**Q: 为什么我 `remote file read` 返回 `file.out_of_scope`？**
A: 目标端 `~/.bifrost/file-access.toml` 里没有把该路径加入 `roots`。让用户追加一条 `[[grant]]` 或扩大 `[default].roots`。

**Q: 我 `read` 一个文件，怎么直接看内容、又怎么拿乐观锁要用的 sha？**
A: 直接 `remote file read <path>`——明文打在 stdout，`sha256=<...>` 在 stderr footer，抄过去喂 `edit --base-sha256` 即可。不需要 `--output json` 再解 base64。

**Q: 我想改远端 `Cargo.toml` 的 version，该用哪个命令？**
A: `remote file read` 看内容并从 footer 拿 sha → `remote file edit --base-sha256 <sha> --edits '[...]'`。不要用 `shell-text "sed -i ..."`。

**Q: 我要写一个就两三行的小文件，还得先在本地建临时文件吗？**
A: 不用。直接 `remote file write <path> --content "第一行\n第二行\n" --create-parents`。

**Q: 我要把本地一个 500KB 的二进制部署到远端？**
A: `bifrost remote file write <remote-path> --content-b64 "$(base64 -w0 < ./local.bin)" --allow-overwrite true --create-parents`。

**Q: 远端上已有一个 git 仓库，我想 `git pull` 再跑测试？**
A: `remote exec --detach --cwd /path/to/repo --shell-text "git pull --ff-only && cargo test"`，再用 `remote job watch <call_id>`。忘了 call id 时先跑 `remote job list`。git / 测试用 shell，代码改动用 file。

**Q: 我要一次看某模块的好几个文件，怎么少跑几趟？**
A: 先用 `remote file read-many --path a --path b --path c`，一次往返并发取回；某个文件读失败不影响其余文件，json 模式里看 `ok_count`。如果 policy 阻断 `read-many`，立即回落为多次 `remote file read`。

**Q: 我要放一个临时 smoke 脚本，能写 `/tmp` 吗？**
A: 默认不要。先 `remote file scratch-dir --cwd /path/to/repo`，把脚本写到 `.bifrost-tmp/` 下；任务结束后按需 `remote file delete .bifrost-tmp --recursive`。

**Q: 我要进一个几千行的陌生源文件改东西，怎么先快速摸清结构？**
A: 先 `remote file outline <path>` 拿到函数/类/结构体清单 + 行号，再针对目标符号 `read --offset <line> --limit N` 精读那一段，或直接 `edit` 定点改。不用把整文件拉回来人肉扫。

**Q: 我想替换某个符号但懒得数行号，会不会一定要先 read 算行号？**
A: 不用。用锚定编辑：`remote file edit <path> --edits '[{"old_string":"旧串","new_string":"新串"}]'`，按字面子串定位。担心多处命中误改就加 `expected_count` 兜底。

**Q: 我要在一批文件里搜 TODO / FIXME / XXX 三个词，还要看命中处上下文？**
A: `remote file find 'TODO' -e 'FIXME' -e 'XXX' --around 3`——多模式 OR + 每个命中回带前后 3 行 snippet。要按字面量搜带正则元字符的串再加 `--fixed-strings`，要整词匹配加 `--word`。

**Q: 我的 diff 里有文件改名 / 复制，`patch` 能直接吃吗？**
A: 能。`patch` 现支持 `rename from/to` 与 `copy from/to` 形态，和新增 / 修改 / 删除一起整批原子提交，失败整体回滚。
