---

name: "bifrost-remote"
description: "通过 Bifrost Remote Invoke 远程操作另一台电脑：连接、状态/流量查询、shell 命令执行；以及对目标仓库做 coding-agent 级的文件读写/搜索/原子 edit/批量 patch（受 FileAccessPolicy 约束）。触发词包括：连接另一台电脑、远程执行命令、远程改代码、远端仓库编辑/重构/批量修改文件、在远端机器上跑 coding agent、远程 grep/search/read/write/edit/patch。重要：修改远端文件必须优先使用 bifrost remote file 子命令，禁止用 shell + base64 + cat/echo 的方式改代码。"

---

# Bifrost Remote

本技能指导 Agent 通过 `bifrost remote` 与远端 Bifrost 建立连接，并完成三类操作：**远端查询**（status / traffic / search）、**远端 shell 执行**（shell.exec）、**远端文件编程**（`remote file` 子命令，coding-agent 友好）。

> **CLI 命名空间约定（重要）**
>
> - `bifrost remote ...` — 所有子命令都在**已连接的远端设备**上执行（relay-backed）。
> - `bifrost setting ...` — 在**当前本机**上管理 Shell Access policy / 本地 grant。
>
> 旧写法 `bifrost remote shell` / `bifrost remote grant` 已被标记为 deprecated（仍可用，一次 release cycle 内会被移除），请统一切换到 `bifrost setting shell` / `bifrost setting grant`。
> 关键判断：看这个命令是否改变远端状态。改**本机配置**一律走 `bifrost setting`。

---

## 黄金法则：修改远端文件用 `remote file`，不要用 `remote command exec + base64`

这是本技能的第一原则。Agent 经常会踩这个坑——用 `remote command exec --shell-text "echo '$B64' | base64 -d > /path/to/file.rs"` 去改代码，这种做法有 6 个已知缺陷：

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
| 看一下远端某文件 | `remote file read <path>` | `remote command exec --shell-text "cat <path>"` |
| 列目录树、找文件名 | `remote file list` / `remote file glob` | `shell-text "find ..." / "ls -R"` |
| 正则搜代码、定位符号 | `remote file search <regex>` | `shell-text "grep -rn ..."` |
| 写一个新文件 | `remote file write --content-file ./local.txt --create-parents` | `shell-text "echo ... > file"` |
| 改已有文件的几行 | `remote file edit --base-sha256 <sha> --edits '[...]'` | `sed -i` / `echo`+重定向 |
| 多文件统一 patch | `remote file apply-patch --patch-file ./diff.patch` | 循环 shell-text 的 sed |
| 创建目录 | `remote file mkdir --parents` | `shell-text "mkdir -p ..."` |
| 移动/删除 | `remote file mv` / `remote file rm --recursive` | `shell-text "mv" / "rm -rf"` |
| 传输大文件 / 二进制 / 特殊字符 | `remote file write --content-b64 "$(base64 < ./blob.bin)"` | echo 管道 base64 |
| 校验文件哈希 | `remote file hash <path> --algo sha256` | `shell-text "shasum ..."` |

**只有下列场景才回落到 `remote command exec`**：
- 跑测试 / 构建 / 启动脚本（`cargo test`、`npm run build`、`python app.py`）。
- `chmod` / `chown` / `ln -s` / `git` 这种文件元信息或 VCS 操作（`remote file` 不覆盖）。
- 需要 streaming 观察 stdout 的长任务。

---

## 一、适用场景

触发本技能的典型表述：

- 操作另一台电脑上的 bifrost：连接、查询状态/流量、远程执行命令。
- 远程改代码、远端仓库重构、在远端机器上跑 coding agent。
- 读远端文件、远程 grep、远程 glob、远程原子 edit、远程批量 patch。
- 用户表述中出现「另一台电脑的项目」「远端文件」「远程 read/write/edit/search」。
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

检查是否已有实例在跑；有就复用，不要启动第二个：

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
| 只读查询 | Access = `query` | `remote_query` | `status` / `search` / `traffic list/get/search` |
| 远端 shell | Access = `selected` 或 `all` | `remote_shell_exec`（加 stdin/interactive 则升级为 `remote_shell_interactive`） | `command exec` |
| 远端文件 | File Access = read / read-write | `remote_file_read` / `remote_file_write` | 所有 `file <cmd>` |

三类 scope **互相独立**，请用户在目标端 Web UI 的 Remote Invoke 授权请求里按需勾选。比如 Agent 只能调用 `remote file read` 但没勾 File Access=read-write，`remote file write` 会以 `file.permission_denied` 拒绝。

### 3.1 建议用 SSH key（长期）

1. 目标端 Web UI `http://127.0.0.1:9900/_bifrost/settings?tab=remote-invoke` → 创建/导出 SSH key。
2. 将 key 文件安全拷到 caller 机器，推荐 `~/.bifrost/remote-device.key`。
3. caller 侧：`bifrost remote connect --ssh-key ~/.bifrost/remote-device.key`。
4. 回收：目标端 Web UI revoke key 即可。

### 3.2 或用 6 位配对码（首次）

1. 目标端 Web UI → Enter Discovery Mode → 显示 6 位码。
2. caller 侧：`bifrost remote connect <pair-code>`。
3. 目标端 Web UI 批准请求，勾选需要的能力和时长。
4. 配对码一次性，复用已保存连接。

### 3.3 远端 Shell Access policy（启用 `command exec` 必读）

caller 的 grant 有 `shell_exec` scope 还不够，**目标端还要配一条匹配的 Shell Access policy**。目标端用户用 `bifrost setting shell` 管理本机 policy（旧版为 `bifrost remote shell`，已 deprecated）：

```bash
# 在目标机上执行
bifrost setting shell profile add \
  --id default --name "Default" \
  --cwd "$HOME" --env PATH --env HOME \
  --default-cwd "$HOME" --timeout-ms 30000 --inherit-env

bifrost setting shell policy add \
  --id allow-bifrost-cli --name "Allow Bifrost CLI" \
  --mode shell_text --pattern '^bifrost\s+' \
  --shell /bin/bash --profile default
```

如果希望 Agent 有广泛 shell 能力，目标端可以创建更宽的 policy，并在授权请求里选 `all`。**能力开放程度由目标端用户决定，caller 不能绕过。**

### 3.4 远端 FileAccessPolicy（启用 `remote file` 必读）

目标端的 `~/.bifrost/file-access.toml` 控制 Agent 能访问哪些目录。新版（`feat/remote-file-api` 后）支持多条 `[[grant]]`，按 `ssh_fingerprint` / `caller_fingerprint` / `grant_id` 匹配，再 fallback 到 `[default]`。示例：

```toml
# 绑定当前 caller 的 ssh key（推荐）
# 字段名以代码里的 serde 为准：写入/读取策略是 `ops`（不是 `allow`），
# 值用下划线命名：read / list / stat / glob / search / hash /
# write / edit / mkdir / move / delete / apply_patch。
[[grant]]
match.ssh_fingerprint = "5f02477634441d5d..."
roots = ["/Users/eden/work/github/bifrost"]
ops = ["read", "list", "stat", "glob", "search", "hash",
       "write", "edit", "mkdir", "move", "delete", "apply_patch"]
# write_denies 只在写类操作（write/edit/mkdir/move/delete/apply_patch）上
# 生效，读类仍可访问。适合「能读 Cargo.lock 做分析，但不让 agent 写坏它」。
write_denies = ["**/Cargo.lock", "**/package-lock.json", "**/pnpm-lock.yaml"]

# 默认策略：其他设备连上来只能只读 $HOME，并且默认把敏感路径拉黑。
# default_denies() 内置会再叠加一层（.git/.ssh/.aws/.env*/id_rsa*/*.pfx/*.p12 等），
# 这里的 denies 是在此基础上额外追加的项目级规则。
[default]
roots = ["/Users/eden"]
ops = ["read", "list", "stat", "glob", "search", "hash"]
denies = ["**/secrets/**", "**/*.secret.toml"]
```

改完文件后无需重启 Bifrost，下次请求会自动热加载。

---

## 四、Agent 工作流

### 4.1 连接

先看本地有没有已保存连接；有就直接查询。没有再走 SSH key / pair code：

```bash
bifrost remote status                                   # 已有连接？
bifrost remote connect --ssh-key ~/.bifrost/remote-device.key   # 或
bifrost remote connect <pair-code>
```

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
3. **检查已有连接**：先执行 `bifrost remote status` 确认是否已有可用连接。如果已经连接到同一设备，跳过 connect 步骤。
4. **发起连接**：执行 `bifrost remote connect --ssh-key ~/.bifrost/remote-device.key`。
5. **确认连接成功**：连接成功后执行 `bifrost remote status` 确认远端可达。
6. **告知用户**：向用户简要报告连接结果（成功/失败及原因）。

**注意事项**：
- **不要让用户手动保存文件**：Agent 应自动完成密钥文件的保存，用户粘贴即代表授权。
- **密钥块中的 `Device-Code` 是 Header 元数据**，标识目标设备，无需单独解析——`bifrost remote connect` 会自动处理。
- **如果连接失败**，按以下顺序排查：密钥是否完整（BEGIN/END 行是否被截断）→ 目标设备是否在线 → 目标设备是否已 revoke 该 key → 网络连通性。
- **不要将密钥内容输出到日志或回复中**，保存后即可丢弃明文。

### 4.2 远端查询（`remote_query` scope）

```bash
bifrost remote status
bifrost remote search <keyword> --max-results 50 --max-scan 200 \
    [--url|--headers|--body|--req-header|--res-body] \
    [--method GET --status 2xx --host example.com --protocol HTTPS]
bifrost remote traffic list  --limit 50 [--method --status --protocol --host --path ...]
bifrost remote traffic get   <id> [--request-body --response-body]
bifrost remote traffic search <keyword> --max-results 50
```

输出格式统一：`--format table|compact|json|json-pretty`，`--no-color` 适合非交互。

### 4.3 远端 shell 执行（`remote_shell_exec` scope）

```bash
bifrost remote command exec --shell-text "ls -la /tmp"
bifrost remote command exec -- ls -la /tmp
bifrost remote command exec --cwd /Users/eden/work/repo --env FOO=bar --shell-text "echo $FOO"
bifrost remote command exec --timeout-ms 10000 --shell-text "cargo test 2>&1 | tail -30"
```

注意：`$FOO` 是远端 shell 展开，不是 caller 的 env。caller 的 env 要通过 `--env` 显式传入。

### 4.4 远端文件编程（`remote_file_*` scope）

**这是本技能的主要价值点。**`remote file` 的完整子命令集：

```bash
# —— 只读（需 remote_file_read）——
bifrost remote file read   <path> [--max-bytes N] [--allow-binary] \
                                   [--offset LINE] [--limit N]
bifrost remote file list   [path] [--depth N] [--no-ignore] [--exclude NAME]...
bifrost remote file stat   <path>
bifrost remote file glob   '<pattern>'  [--max-matches N] [--no-ignore]
bifrost remote file search '<regex>'    [--path <sub>] [--max-matches N] \
                                         [-B N] [-A N] [-i] [--glob '<pat>']
bifrost remote file hash   <path> [--algo sha256]

# —— 读写（需 remote_file_write）——
bifrost remote file write  <path> (--content-file <local|->) | (--content-b64 <b64>) \
                                   [--base-sha256 SHA] [--allow-overwrite true|false] \
                                   [--create-parents]
bifrost remote file edit   <path> --edits '<json>' [--base-sha256 SHA]
bifrost remote file mkdir  <path> [--parents]
bifrost remote file mv     <from> <to>
bifrost remote file rm     <path> [--recursive]
bifrost remote file apply-patch (--patch-file <local|->) | (--patch-b64 <b64>)
```

所有子命令共享 `--cwd <path>`、`--output human|json`、`--relay-url`、`--client-id`。

#### 行为要点

- **gitignore 默认打开**：`list` / `glob` / `search` 默认跳 `.gitignore` 命中路径。要扫被忽略文件加 `--no-ignore`。
- **`truncated`**：`list` / `glob` / `search` / `read` 超限时响应带 `"truncated": true`。Agent 应据此分片（`--offset`+`--limit`、收窄 `--path` / `--glob`）。
- **整文件 sha256**：`read` 当 `truncated=true` 时响应额外带 `file_sha256`（整文件），可用于 resume 或乐观锁一致性校验。
- **原子写**：`write` / `edit` 采用 tmp+rename，失败自动回滚。
- **乐观锁**：`write` / `edit` 传 `--base-sha256` 后，文件已被改动会返回 `file.sha_mismatch`；Agent 应重新 `read` 再重试，不要盲目覆盖。
- **EOL 保留**：`edit` 自动识别并保留 LF / CRLF 风格。
- **`--content-b64` / `--patch-b64`**：由 caller 本地 base64、目标端解码；适合二进制、含 CRLF、含特殊字符的文本。远比 echo 管道 base64 + shell 重定向安全。
- **`--create-parents`**：`write` 自带 `mkdir -p`，一次 round-trip 搞定。
- **Symlink lstat 语义**：`stat` / `list` 不跟随软链；Windows 自动去 `\\?\` / `\\?\UNC\` 前缀。

#### 错误码契约

| 错误码 | 含义 | Agent 应对 |
|---|---|---|
| `file.out_of_scope` | 路径在 `roots` 之外 | **不要擅自改 `--cwd`**，请用户在目标端更新 FileAccessPolicy |
| `file.permission_denied` | 命中 denies，或缺 write scope | 如果是 denies（比如 `.ssh`、`target`），说明用户不想给；如果是缺 scope，请用户重新授权 |
| `file.binary_not_allowed` | 是二进制但没加 `--allow-binary` | 显式加 `--allow-binary`，或改用 `hash` + 分片 |
| `file.sha_mismatch` | 乐观锁失败 | 重新 `read` + 重算 sha + 重试 |
| `file.not_found` | 路径不存在 | 视任务决定 `mkdir` / `write --create-parents` |
| `file.is_a_directory` / `file.not_a_directory` | 类型不匹配 | 切换子命令 |

#### Coding agent 典型 workflow

```bash
# 1. 侦察
bifrost remote file list src --depth 2 --output json
bifrost remote file glob 'src/**/*.rs' --max-matches 200 --output json

# 2. 定位符号
bifrost remote file search 'fn handle_file_\w+' --path src --glob '*.rs' \
  -B 2 -A 2 --output json

# 3. 读 + 拿 sha256
bifrost remote file read src/lib.rs --output json        # 响应含 sha256

# 4. 乐观锁 edit
bifrost remote file edit src/lib.rs \
  --base-sha256 <sha-from-step-3> \
  --edits '[{"start_line":10,"end_line":12,"replacement":"// new impl\n"}]' \
  --output json

# 5. 新建 / 覆盖写（二进制、特殊字符统一走 b64）
bifrost remote file write docs/changelog.md \
  --content-b64 "$(base64 < ./local-notes.md)" \
  --create-parents --output json

# 6. 多文件 patch
bifrost remote file apply-patch --patch-file ./refactor.diff --output json

# 7. 跑测试（这一步才用 shell.exec）
bifrost remote command exec --cwd /Users/eden/work/github/repo \
  --timeout-ms 300000 --shell-text "cargo test 2>&1 | tail -30"
```

### 4.5 断开与回收

```bash
bifrost remote disconnect                    # 撤销当前 client 的 grants
bifrost remote disconnect --all              # 所有 client
bifrost remote disconnect --grant-id <gid>   # 指定 grant
```

---

## 五、当前 relay-backed 命令清单

| Scope | 子命令 |
|---|---|
| `remote_query` | `status` · `search.stream` · `traffic.list` · `traffic.get` |
| `remote_shell_exec` / `remote_shell_interactive` | `shell.exec` |
| `remote_file_read` | `file.read/list/stat/glob/search/hash` |
| `remote_file_write` | `file.write/edit/mkdir/mv/rm/apply_patch` |

不在此清单内的管理面（rule / config / script / value / CA / 系统代理 / traffic clear）**没有**专用 relay-backed 子命令。caller 想远程管理这些模块，应通过已授权的 `remote command exec` 在目标端跑等价的本机 CLI。

---

## 六、本地管理 vs 远端操作

| 你想做 | 在哪里执行 | 命令 |
|---|---|---|
| 管理**本机** Shell Access policy/profile | 本机 | `bifrost setting shell ...`（旧：`bifrost remote shell`，deprecated） |
| 管理**本机** remote-invoke grants | 本机 | `bifrost setting grant ...`（旧：`bifrost remote grant`，deprecated） |
| 操作**已连接的远端设备** | caller | `bifrost remote <connect/disconnect/status/command/file/search/traffic>` |
| 给远端改 Shell Access policy | 远端 | `bifrost remote command exec --shell-text "bifrost setting shell policy add ..."` |
| 给远端改 FileAccessPolicy | 远端 | 请用户在目标端编辑 `~/.bifrost/file-access.toml`（会热加载）；或在 shell 授权允许的情况下用 `remote command exec` 辅助 |

> **不要**把 `bifrost setting ...` 或 `bifrost remote shell/grant`（deprecated 别名）当成 relay-backed 管理 API 直接调，它们只作用于**执行该命令的那台机器**。

---

## 七、Agent 执行约束（强制）

按优先级阅读：

1. **先用 `remote file`，再考虑 `remote command exec`**。任何修改远端文件内容的操作，先看是否能用 `remote file write/edit/mv/rm/mkdir/apply-patch` 完成。**严禁**用 `command exec --shell-text "echo '$B64' | base64 -d > ..."` 这类 shell 拼接写文件。违反此条 = 违反本技能。
2. **不要重复 `remote connect`**：先跑 `bifrost remote status` 看已有连接是否可用。
3. **SSH key 优于 pair code**：有 key 先用 key，一次连接永久复用（直到 key 被 reset/revoke）。
4. **多 client 场景**：显式 `--client-id <prefix>`，不要依赖交互式选择。
5. **失败分类**：
   - `file.out_of_scope` / `file.permission_denied` → 告诉用户需要调 FileAccessPolicy，不自作主张改 `--cwd` 绕过。
   - `file.sha_mismatch` → 重 read 重算 sha 再重试。
   - connect 失败 → 检查 SSH key 有效性、pair code 是否过期、目标是否在线、Web UI 是否授权。grant 失效就重新 connect，**不要**伪造本地连接文件。
6. **本机 vs 远端不要混**：改本机走 `bifrost setting`；改远端走 `bifrost remote`。
7. **不要承诺 OS 级 sandbox**：`shell.exec` 是 Shell Access policy 级限制，不是 sandbox。
8. **长任务超时**：构建、测试类 `command exec` 记得 `--timeout-ms 300000`（默认 30s 不够用）。
9. **大文件/二进制传输**：`--content-file -` 从 stdin，`--content-b64` / `--patch-b64` 适合非交互；避免 echo 管道 base64。
10. **只读先行**：在做 write 之前，至少 `list` + `read` 侦察一次，别盲写。

---

## 八、FAQ

**Q: 为什么我 `remote file read` 返回 `file.out_of_scope`？**
A: 目标端 `~/.bifrost/file-access.toml` 里没有把该路径加入 `roots`。让用户追加一条 `[[grant]]` 或扩大 `[default].roots`。

**Q: 我想改远端 `Cargo.toml` 的 version，该用哪个命令？**
A: `remote file read` 拿到 sha → `remote file edit --base-sha256 <sha> --edits '[...]'`。不要用 `shell-text "sed -i ..."`。

**Q: 我要把本地一个 500KB 的二进制部署到远端？**
A: `bifrost remote file write <remote-path> --content-b64 "$(base64 -w0 < ./local.bin)" --allow-overwrite true --create-parents`。

**Q: 远端上已有一个 git 仓库，我想 `git pull` 再跑测试？**
A: `remote command exec`：`--cwd /path/to/repo --shell-text "git pull --ff-only && cargo test"`。git / 测试用 shell，代码改动用 file。

**Q: `bifrost remote shell` 和 `bifrost setting shell` 到底有什么差别？**
A: 完全一样的管理能力，都改**本机**数据目录。`bifrost remote shell` 是历史遗留别名，已 deprecated，运行时会打印 warning；请统一改用 `bifrost setting shell`。
