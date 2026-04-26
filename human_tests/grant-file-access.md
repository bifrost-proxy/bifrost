# Grant File Access 正交权限模型 — 测试用例

## 功能模块说明

验证 Grant 权限系统中 `file_access` 独立于 `grant_scope` 的正交权限模型，包括：
- WebUI 预设策略模式（Full Access / Shell Only / File Only / Query Only / Custom）
- PairingRequestModal 审批流程中正确传递 `file_access`
- Grant Editor 中正确修改 `file_access`
- CLI `remote grant update --file-access` 参数
- 后端权限检查：`scope_allows_command()` 正确组合判断

## 前置条件

1. 启动本地 Bifrost 服务（带临时数据目录）：
```bash
BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- start -p 8800 --unsafe-ssl --no-system-proxy
```
2. WebUI 可通过 `http://localhost:8800/_bifrost/` 访问

---

## 测试用例

### TC-GFA-01: API — approve pairing 携带 file_access

**操作步骤**：
1. 通过 API 发送 approve 请求，body 包含 `file_access: "read_write"`
```bash
curl -s http://localhost:8800/api/remote-invoke/pairings/<pairing_id>/approve \
  -H 'Content-Type: application/json' \
  -d '{"grant_mode":"permanent","grant_scope":"remote_shell_interactive","file_access":"read_write","policy_binding":{"mode":"all"},"interactive_allowed":true,"stdin_allowed":true}'
```
2. 查询 grant 列表，确认新 grant 包含 `file_access: "read_write"`

**预期结果**：
- approve 返回 200
- grant 列表中对应 grant 的 `file_access` 字段为 `"read_write"`

### TC-GFA-02: API — update grant 修改 file_access

**操作步骤**：
1. 查询 grant 列表获取一个 grant_id
2. 通过 PATCH 更新 grant 的 file_access
```bash
curl -s -X PATCH http://localhost:8800/api/remote-invoke/grants/<grant_id> \
  -H 'Content-Type: application/json' \
  -d '{"file_access":"read"}'
```
3. 再次查询 grant 列表确认更新生效

**预期结果**：
- PATCH 返回 200
- grant 的 `file_access` 已变为 `"read"`

### TC-GFA-03: API — update grant 通过 file_access=none 撤销文件权限

**操作步骤**：
1. 使用 PATCH 将 grant 的 file_access 设为 none
```bash
curl -s -X PATCH http://localhost:8800/api/remote-invoke/grants/<grant_id> \
  -H 'Content-Type: application/json' \
  -d '{"file_access":"none"}'
```

**预期结果**：
- PATCH 返回 200
- grant 的 `file_access` 为 `"none"`

### TC-GFA-04: WebUI PairingRequestModal — Full Access 预设

**操作步骤**：
1. 打开 WebUI，进入 Settings → Remote Invoke 页面
2. 在配对审批弹窗中选择 "Full Access" 预设
3. 确认预设描述为 "Shell + File + Interactive"

**预期结果**：
- Full Access 预设同时启用 shell 和 file 访问
- 审批后生成的 grant 应包含 `grant_scope: "remote_shell_interactive"` 和 `file_access: "read_write"`

### TC-GFA-05: WebUI PairingRequestModal — Shell Only 预设

**操作步骤**：
1. 在配对审批弹窗中选择 "Shell Only" 预设
2. 确认预设描述为 "Execute commands, no file access"

**预期结果**：
- 审批后生成的 grant 包含 `grant_scope: "remote_shell_interactive"` 和 `file_access: "none"`

### TC-GFA-06: WebUI PairingRequestModal — File Only 预设

**操作步骤**：
1. 在配对审批弹窗中选择 "File Only" 预设
2. 确认预设描述为 "Read & write files, no shell"

**预期结果**：
- 审批后生成的 grant 包含 `grant_scope: "remote_query"` 和 `file_access: "read_write"`

### TC-GFA-07: WebUI Grant Editor — 预设选择器

**操作步骤**：
1. 打开 WebUI，进入 Settings → Remote Invoke → Grants 列表
2. 点击某个 grant 的编辑按钮
3. 确认 Grant Editor 弹窗显示 Radio.Group 预设选择器
4. 选择不同的预设，确认能切换

**预期结果**：
- Grant Editor 显示 5 个预设选项（Full Access / Shell Only / File Only / Query Only / Custom）
- 切换预设后保存，grant 属性正确更新

### TC-GFA-08: WebUI Grant Editor — Custom 模式展开详细面板

**操作步骤**：
1. 在 Grant Editor 中选择 "Custom" 预设
2. 确认显示详细配置面板（Shell Access select + File Access select）

**预期结果**：
- Custom 模式下显示 Shell Access 和 File Access 独立选择器
- 可以独立设置 shell 和 file 权限组合

### TC-GFA-09: WebUI Grant 列表 — file_access Tag 显示

**操作步骤**：
1. 打开 Grants 列表
2. 查看有 file_access 的 grant

**预期结果**：
- 有 `file_access: "read_write"` 的 grant 显示 `File: R/W` 蓝色标签
- 有 `file_access: "read"` 的 grant 显示 `File: Read` 蓝色标签
- 无 file_access 或 `none` 的 grant 不显示 file 标签

### TC-GFA-10: CLI — remote grant update --file-access

**操作步骤**：
```bash
cargo run --bin bifrost -- remote grant update <grant_id> --file-access read_write
```

**预期结果**：
- 命令成功执行，返回更新后的 grant JSON
- grant 的 `file_access` 字段为 `"read_write"`

### TC-GFA-11: CLI — remote grant update --scope 不再支持 remote_file_read/write

**操作步骤**：
```bash
cargo run --bin bifrost -- remote grant update <grant_id> --scope remote_file_read
```

**预期结果**：
- 命令报错，提示不支持的 scope 值
- 错误信息中不包含 `remote_file_read`/`remote_file_write` 作为可选值

### TC-GFA-12: 权限检查 — file_access=none 阻止文件操作

**操作步骤**：
1. 设置 grant 为 `grant_scope: "remote_shell_interactive"` + `file_access: "none"`
2. 尝试执行 remote file list 操作

**预期结果**：
- 文件操作被拒绝，返回权限错误
- shell 操作（如 remote shell exec）仍然正常工作

### TC-GFA-13: 权限检查 — file_access=read 阻止写操作

**操作步骤**：
1. 设置 grant 为 `file_access: "read"`
2. 尝试 `remote file list`（读操作）→ 应成功
3. 尝试 `remote file write`（写操作）→ 应失败

**预期结果**：
- 读操作（list/read/search/glob）成功
- 写操作（write/edit/mkdir/remove 等）被拒绝

### TC-GFA-14: 回归 — SSE grant_created 事件包含 file_access

**操作步骤**：
1. 搭建完整本地测试环境（Relay + TARGET + CALLER）
2. 发起配对，在 TARGET 用 Full Access 审批
3. 检查 TARGET 的 `remote_invoke_grant_info.json` 文件

**预期结果**：
- grant_info 中 `file_access` 为 `"read_write"`（非 `"none"`）
- TARGET 日志中不出现 `file_access None` 相关的权限拒绝

### TC-GFA-15: 回归 — approve_pairing 持久化 grant_info

**操作步骤**：
1. 完成配对审批后，立即检查 TARGET 的 `remote_invoke_grant_info.json`
2. 对比 `remote_invoke_grant_policy.json` 中的 `file_access`

**预期结果**：
- 两个文件的 `file_access` 值一致
- `grant_info.json` 中 `file_access` 不为 `"none"`（消除竞态条件）

### TC-GFA-16: 端到端 — Full Access 授权后执行文件操作

**操作步骤**：
1. 搭建本地 Relay（端口 13579，启用 remote-invoke）
2. 启动 TARGET（端口 18800），连接 Relay
3. 从 CALLER 发起 `remote connect`，TARGET 用 Full Access 审批
4. 配置 `file-access.toml`，添加目标目录为 root
5. 执行 `remote file mkdir` 创建目录
6. 执行 `remote file write` 写入文件（stdin 管道）
7. 执行 `remote file list` 验证文件存在
8. 执行 `remote file read --offset 1 --limit 5` 验证内容

**预期结果**：
- mkdir 返回 `created: true`
- write 返回正确的 `bytes_written` 和 `sha256`
- list 返回写入的文件条目
- read 返回正确的 `content_b64`、`total_lines`、`start_line`、`end_line`

### TC-GFA-17: 端到端 — 三策略动态切换验证（自动化脚本）

**操作步骤**：
1. 运行自动化测试脚本 `/tmp/test_three_policies.sh`，脚本自动搭建完整环境（Relay + TARGET + CALLER）
2. 策略一（`file_access=read_write`）：审批后验证 file.read、file.write、file.mkdir、file.list 均成功
3. 策略二（`file_access=read`）：通过 PATCH API 降级为 read-only，验证 file.read/list 成功，file.write/mkdir 被拒绝
4. 策略三（`file_access=none`）：通过 PATCH API 降级为 none，验证 file.read/list/write/mkdir 均被拒绝
5. 恢复验证：通过 PATCH API 恢复为 read_write，验证 file.read/write 恢复正常

**预期结果**（14 个子用例）：
- TC-P1-01~04: read_write 下 read ✅、write ✅、mkdir ✅、list ✅
- TC-P2-01~04: read 下 read ✅、list ✅、write 被拒绝（`grant file_access=Read does not allow write`）、mkdir 被拒绝
- TC-P3-01~04: none 下 read/list/write/mkdir 全部被拒绝（`scope does not allow command kind File`）
- TC-P4-01~02: 恢复 read_write 后 read ✅、write ✅

**实际测试结果**：2026-04-25 全部 14/14 通过 ✅

### TC-GFA-18: 回归 — file_access=read 时 executor 阻止写操作

**操作步骤**：
1. 配对连接后，设置 grant 为 `file_access: "read"`
2. 发送 `remote file write` 命令
3. 检查错误消息

**预期结果**：
- 错误包含 `grant file_access=Read does not allow write operation`
- 写操作在 executor 层被拒绝，不会到达文件系统

**实际测试结果**：2026-04-25 通过 ✅（修复了 executor 中缺少 grant 级别写权限检查的 bug）

---

## 清理步骤

```bash
rm -rf ./.bifrost-test
```
