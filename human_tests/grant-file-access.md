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

---

## 清理步骤

```bash
rm -rf ./.bifrost-test
```
