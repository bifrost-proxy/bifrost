# Web UI Values 页面测试用例

## 功能模块说明

Values 页面用于管理 Bifrost 的可复用变量，支持在规则中通过 `{name}` 语法引用。页面采用左右分栏布局，左侧为 Value 列表（支持搜索、排序、多选），右侧为 Monaco Editor 编辑器（自动检测 JSON/XML 并语法高亮）。Values 可用于存储 API Key、Token 等敏感数据或常用内容。

## 前置条件

1. 启动 Bifrost 服务（使用临时数据目录避免污染正式环境）：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- start -p 8800 --unsafe-ssl
   ```
2. 确保端口 8800 未被其他进程占用
3. 浏览器已打开，可访问 `http://127.0.0.1:8800/_bifrost/`

---

## 测试用例

### TC-WVA-01：访问 Values 页面

**操作步骤**：
1. 在浏览器中打开 `http://127.0.0.1:8800/_bifrost/values`

**预期结果**：
- 页面成功加载，左侧显示 Value 列表面板（data-testid: `values-list`）
- 左侧面板标题为 "Values"
- 右侧显示空状态提示 "Select a value to edit"
- 右侧空状态区域包含 "What are Values?" 说明和 "How to Use" 使用指南
- 使用指南中说明了 `{name}` 引用语法
- 底部显示统计信息（如 "0 values"）

---

### TC-WVA-02：Value 列表显示与排序

**前置条件**：已创建多个 Value（可通过 API 预创建或通过 UI 逐个创建）

**操作步骤**：
1. 打开 Values 页面
2. 观察左侧列表中的 Value 展示
3. 点击搜索框旁的排序选择器（data-testid: `value-sort-select`），选择 "Newest"
4. 切换排序为 "Updated"
5. 切换排序为 "Name"

**预期结果**：
- 列表中每个 Value 项显示名称
- 选中的 Value 项有高亮样式
- "Newest" 排序：按创建时间倒序（最新创建的在最前面）
- "Updated" 排序：按更新时间倒序（最近修改的在最前面）
- "Name" 排序：按名称字母升序排列
- 底部统计信息显示正确的 Value 总数

---

### TC-WVA-03：创建新 Value

**操作步骤**：
1. 点击左侧面板头部的 "New Value" 按钮（+ 图标，data-testid: `value-new-button`）
2. 在弹出的 "New Value" 对话框中，输入名称 `api_key`
3. 点击 "Create" 按钮

**预期结果**：
- 对话框中有 placeholder 提示 "Value name (e.g., api_key, auth_token)"
- 创建成功后显示 Toast 消息 "Value created"
- 左侧列表中出现新 Value 项 `api_key`
- 新创建的 Value 自动被选中
- 右侧编辑器加载该 Value（内容为空）
- 编辑器标题显示 `api_key`

---

### TC-WVA-04：创建 Value — 名称为空校验

**操作步骤**：
1. 点击 "New Value" 按钮
2. 不输入任何名称（留空）
3. 点击 "Create"

**预期结果**：
- 显示错误消息 "Value name is required"
- Value 未被创建
- 对话框保持打开状态

---

### TC-WVA-05：在编辑器中编辑 Value

**前置条件**：已通过 TC-WVA-03 创建 `api_key`

**操作步骤**：
1. 在左侧列表中点击 `api_key`
2. 在右侧 Monaco 编辑器中输入内容：`sk-1234567890abcdef`

**预期结果**：
- 编辑器加载成功，标题区域（data-testid: `value-editor-title`）显示 `api_key`
- 编辑器底部状态栏显示 `PLAINTEXT`（纯文本检测）
- 编辑器底部状态栏显示引用提示 `Use {api_key} to reference this value in rules`
- 输入内容后，左侧列表中该 Value 项显示未保存标记（橙色圆点 Tooltip: "Unsaved changes"）
- Save 按钮从禁用状态变为可用状态

---

### TC-WVA-06：保存 Value 变更

**前置条件**：已在 TC-WVA-05 中修改了 Value 内容

**操作步骤**：
1. 点击右上角 "Save" 按钮（data-testid: `value-save-button`）

**预期结果**：
- 保存成功后显示 Toast 消息 "Saved"
- 左侧列表中未保存标记消失
- Save 按钮恢复为禁用状态（无新更改）

---

### TC-WVA-07：使用快捷键保存 Value

**前置条件**：Value 处于编辑状态

**操作步骤**：
1. 在编辑器中修改内容
2. 按下 `Cmd+S`（macOS）快捷键

**预期结果**：
- Value 保存成功，显示 Toast 消息 "Saved"
- 与点击 Save 按钮效果一致

---

### TC-WVA-08：编辑器自动检测 JSON 并语法高亮

**操作步骤**：
1. 创建一个新 Value `json_config`
2. 在编辑器中输入 JSON 内容：
   ```json
   {"server": "api.example.com", "port": 8080, "ssl": true}
   ```

**预期结果**：
- 编辑器自动检测为 JSON 格式
- 底部状态栏语言标识显示 "JSON"
- 编辑器对 JSON 内容进行语法高亮（key、value、数字、布尔值颜色区分）
- 工具栏出现 "Format" 按钮（data-testid: `value-format-button`）
- 点击 Format 按钮后，JSON 被格式化为缩进形式，显示 Toast 消息 "Formatted"

---

### TC-WVA-09：删除 Value

**前置条件**：已创建 `api_key`

**操作步骤**：
1. 在左侧列表中点击 `api_key`
2. 点击右上角 "Delete" 按钮（data-testid: `value-delete-button`）
3. 在确认弹窗 "Delete Value" 中点击 "Delete"

**预期结果**：
- 弹窗内容包含 "Are you sure to delete"
- 确认删除后显示 Toast 消息 "Value deleted"
- 左侧列表中不再显示 `api_key`
- 如果列表中还有其他 Value，自动选中第一个

---

### TC-WVA-10：通过右键菜单删除 Value

**前置条件**：已创建至少一个 Value

**操作步骤**：
1. 在左侧列表中右键点击目标 Value 项
2. 在弹出的上下文菜单中点击 "Delete"
3. 在确认弹窗中点击 "Delete"

**预期结果**：
- 右键菜单包含 "Copy Value"、"Rename"、"Export"、"Delete" 等选项
- Delete 选项显示为红色（danger）
- 确认删除后 Value 从列表中移除

---

### TC-WVA-11：重命名 Value

**前置条件**：已创建 Value `json_config`

**操作步骤**：
1. 在左侧列表中右键点击 `json_config`（或点击 Value 项右侧的 ⋮ 菜单按钮，data-testid: `value-item-menu`）
2. 在菜单中点击 "Rename"
3. 在弹出的 "Rename Value" 对话框中修改名称为 `app_config`
4. 点击 "Rename" 按钮

**预期结果**：
- Rename 对话框中预填充当前 Value 名称
- 重命名成功后显示 Toast 消息 "Value renamed"
- 列表中 Value 名称更新为 `app_config`
- 如果该 Value 当前被选中，编辑器标题同步更新

---

### TC-WVA-12：在规则中通过 {name} 引用 Value

**前置条件**：
- 已创建 Value `test_host`，内容为 `httpbin.org`
- 保存该 Value

**操作步骤**：
1. 打开 Rules 页面 `http://127.0.0.1:8800/_bifrost/rules`
2. 创建新规则，内容为：
   ```
   example.com redirect://{test_host}/get
   ```
3. 保存规则
4. 通过代理访问 `http://example.com`，验证 `{test_host}` 是否被替换为 `httpbin.org`

**预期结果**：
- 规则中的 `{test_host}` 在运行时被替换为 Value 的实际值 `httpbin.org`
- 请求被正确重定向到 `httpbin.org/get`

---

### TC-WVA-13：导出所有 Values 为 .bifrost 文件

**前置条件**：已创建至少一个 Value

**操作步骤**：
1. 点击左侧面板头部的 "Export All" 按钮（data-testid: `value-export-all-button`）

**预期结果**：
- 浏览器下载一个 `.bifrost` 文件
- 文件内容包含 "values" 标识
- 文件内容包含所有 Value 的名称和值

---

### TC-WVA-14：从 .bifrost 文件导入 Values

**前置条件**：
- 已通过 TC-WVA-13 导出了 `.bifrost` 文件
- 已删除导出前创建的所有 Value

**操作步骤**：
1. 点击左侧面板头部的导入按钮（data-testid: `value-import-button`）
2. 选择之前导出的 `.bifrost` 文件

**预期结果**：
- 导入成功后显示 Toast 消息 "导入成功"
- 左侧列表中恢复之前导出的所有 Value
- 每个 Value 的名称和内容与导出前一致

---

### TC-WVA-15：搜索过滤 Values

**前置条件**：已创建多个 Value（如 `api_key`、`app_config`、`db_password`）

**操作步骤**：
1. 在左侧搜索框（data-testid: `value-search-input`）中输入 `api`

**预期结果**：
- 列表仅显示名称或内容包含 "api" 的 Value
- 不匹配的 Value 被隐藏
- 清除搜索内容后恢复显示所有 Value
- 搜索结果为空时显示 "No matching values"

---

### TC-WVA-16：复制 Value 内容

**前置条件**：已创建 Value `api_key`，内容为 `sk-1234567890abcdef`

**操作步骤**：
1. 在左侧列表中点击 `api_key`
2. 点击右上角 "Copy" 按钮（data-testid: `value-copy-button`）

**预期结果**：
- 显示 Toast 消息 "Copied"
- 系统剪贴板中包含 Value 的完整内容 `sk-1234567890abcdef`

---

### TC-WVA-17：键盘上下键切换选中 Value

**前置条件**：已创建至少 2 个 Value

**操作步骤**：
1. 点击列表中的第一个 Value
2. 点击列表区域使其获得焦点
3. 按下键盘 ↓ 方向键
4. 按下键盘 ↑ 方向键

**预期结果**：
- 按 ↓ 键后，选中项切换到下一个 Value，右侧编辑器加载该 Value 内容
- 按 ↑ 键后，选中项切换回上一个 Value
- 选中项的 `aria-selected` 属性为 `true`，非选中项为 `false`

---

### TC-WVA-18：刷新 Values 列表

**操作步骤**：
1. 点击左侧面板头部的 "Refresh" 按钮（data-testid: `value-refresh-button`）

**预期结果**：
- 列表重新加载最新数据
- 如果外部（如通过 API）新增了 Value，刷新后可见
- 刷新过程中可能短暂显示加载状态

---

### TC-WVA-19：桌面端 Values 编辑器支持基础编辑快捷键

**前置条件**：通过桌面客户端打开 Values 页面，并已选中一条 Value

**操作步骤**：
1. 在右侧编辑器中输入一些文本
2. 按 `Cmd+A`
3. 按 `Cmd+C`
4. 按 `Cmd+V`
5. 按 `Cmd+X`
6. 按 `Cmd+Z` 与 `Shift+Cmd+Z`

**预期结果**：
- `Cmd+A` 可全选编辑器文本
- `Cmd+C` 可复制选中文本
- `Cmd+V` 可粘贴文本
- `Cmd+X` 可剪切选中文本
- `Cmd+Z` / `Shift+Cmd+Z` 可撤销与重做

---

### TC-WVA-20：桌面端 Undo 回原文后保存，macOS 窗口黄点消失

**前置条件**：
1. 使用 Bifrost 桌面客户端打开 Values 页面
2. 已选中一个已有 Value，且当前内容可明确识别原文

**操作步骤**：
1. 在编辑器末尾追加一个字符，使内容从 `A` 变为 `AB`
2. 确认 macOS 窗口左上角关闭按钮黄点出现
3. 执行一次 `Cmd+Z`，使内容回到原文 `A`
4. 点击 "Save" 按钮，或按 `Cmd+S`

**预期结果**：
- 第 2 步后，macOS 窗口左上角关闭按钮出现黄点
- 第 3 步后，内容恢复原文，列表中的未保存圆点仍可保留
- 第 4 步后，显示 Toast 消息 "Saved"
- 保存完成后，macOS 窗口左上角关闭按钮黄点消失
- 列表未保存圆点消失，Save 按钮恢复禁用

---

## 清理

测试完成后清理临时数据：
```bash
rm -rf .bifrost-test
```
