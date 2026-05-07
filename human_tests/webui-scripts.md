# Web UI Scripts 页面测试用例

## 功能模块说明

Scripts 页面用于管理 Bifrost 的脚本功能，支持四种类型脚本：请求脚本（Request）、响应脚本（Response）、解码脚本（Decode）、协议解析脚本（Parser）。页面采用左右分栏布局，左侧为脚本列表（树状视图，支持文件夹分组），右侧为代码编辑器（Monaco Editor）和测试结果面板。

## 前置条件

1. 启动 Bifrost 服务（使用临时数据目录避免污染正式环境）：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- start -p 8800 --unsafe-ssl --no-system-proxy
   ```
2. 确保端口 8800 未被其他进程占用
3. 浏览器已打开，可访问 `http://127.0.0.1:8800/_bifrost/`

---

## 测试用例

### TC-WSC-01：访问 Scripts 页面

**操作步骤**：
1. 在浏览器中打开 `http://127.0.0.1:8800/_bifrost/scripts`

**预期结果**：
- 页面成功加载，左侧显示脚本列表面板（data-testid: `scripts-list-panel`）
- 左侧面板标题为 "Scripts"
- 左侧列表顶部只显示两个主操作按钮：`+` 创建菜单（data-testid: `scripts-create-menu-button`）和 `...` 更多菜单（data-testid: `scripts-more-menu-button`）
- 左侧列表顶部不再直接平铺 New Request / New Response / New Decode / Sandbox / Export All / Import 多个按钮
- 右侧显示空状态提示 "Select a script or create a new one"
- 右侧空状态区域包含 "How to use Scripts" 说明，列出 Request scripts、Response scripts 等用法
- 底部显示脚本统计信息（如 "0 scripts"）

---

### TC-WSC-02：通过 + 下拉菜单创建请求脚本

**操作步骤**：
1. 在 Scripts 页面，点击左侧面板头部的 `+` 创建菜单（data-testid: `scripts-create-menu-button`）
2. 在下拉菜单中点击 "Request Script"（data-testid: `scripts-create-request-item`）
3. 右侧编辑器面板加载后，在编辑器中输入脚本内容：
   ```javascript
   request.headers["X-Test-Header"] = "test-value";
   ```
4. 点击右上角 "Save" 按钮（data-testid: `scripts-save-button`）
5. 在弹出的 "Save New Script" 对话框中，输入脚本名称 `test-request-script`
6. 点击对话框中的 "Save" 按钮

**预期结果**：
- 点击 Request Script 菜单项后，右侧编辑器显示 Request Script 模板代码
- 编辑器面板标题显示 "New Script"，类型标签为蓝色 "request"
- 保存对话框中有 placeholder 提示 "Enter script name (e.g., api/add-auth-header)"
- 保存成功后显示 Toast 消息 "Script created"
- 左侧列表中出现新脚本项，显示名称 `test-request-script`，类型标签为蓝色 "REQ"

---

### TC-WSC-03：通过 + 下拉菜单创建响应脚本

**操作步骤**：
1. 点击左侧面板头部的 `+` 创建菜单
2. 在下拉菜单中点击 "Response Script"（data-testid: `scripts-create-response-item`）
3. 在编辑器中输入脚本内容：
   ```javascript
   response.headers["X-Response-Modified"] = "true";
   ```
4. 点击 "Save" 按钮
5. 在对话框中输入脚本名称 `test-response-script`
6. 点击 "Save"

**预期结果**：
- 点击 Response Script 菜单项后，右侧编辑器显示 Response Script 模板代码
- 编辑器面板标题显示 "New Script"，类型标签为绿色 "response"
- 保存成功后显示 Toast 消息 "Script created"
- 左侧列表中出现新脚本项，类型标签为绿色 "RES"

---

### TC-WSC-04：通过 + 下拉菜单创建解码脚本

**操作步骤**：
1. 点击左侧面板头部的 `+` 创建菜单
2. 在下拉菜单中点击 "Decode Script"（data-testid: `scripts-create-decode-item`）
3. 在编辑器中输入脚本内容：
   ```javascript
   ctx.output = { data: "decoded", code: "ok", msg: "" };
   ```
4. 点击 "Save" 按钮
5. 在对话框中输入脚本名称 `test-decode-script`
6. 点击 "Save"

**预期结果**：
- 点击 Decode Script 菜单项后，右侧编辑器显示 Decode Script 模板代码
- 编辑器面板标题显示 "New Script"，类型标签为紫色 "decode"
- 保存成功后显示 Toast 消息 "Script created"
- 左侧列表中出现新脚本项，类型标签为紫色 "DEC"

---

### TC-WSC-04A：通过 + 下拉菜单创建 Parser 脚本

**操作步骤**：
1. 点击左侧面板头部的 `+` 创建菜单
2. 确认菜单中同时存在 "Request Script"、"Response Script"、"Decode Script"、"Parser Script" 四个创建入口
3. 点击 "Parser Script"（data-testid: `scripts-create-parser-item`）
4. 在编辑器中输入脚本内容：
   ```javascript
   ctx.output = { data: "parser-output", code: "0", msg: "" };
   ```
5. 点击 "Save" 按钮
6. 在对话框中输入脚本名称 `test-parser-script`
7. 点击 "Save"

**预期结果**：
- `+` 创建菜单明确展示四种脚本类型，而不是只有 request / response / decode 三种
- 点击 Parser Script 菜单项后，右侧编辑器显示新脚本，类型标签显示 "Parser"
- 保存成功后显示 Toast 消息 "Script created"
- 左侧列表中出现新脚本项，类型标签为 "PAR"

---

### TC-WSC-04B：通过 ... 下拉菜单访问非创建操作

**操作步骤**：
1. 点击左侧面板头部的 `...` 更多菜单（data-testid: `scripts-more-menu-button`）
2. 查看菜单项
3. 点击 "Sandbox Settings"（data-testid: `scripts-more-sandbox-item`）
4. 关闭 Sandbox Settings 对话框
5. 再次打开 `...` 更多菜单

**预期结果**：
- 更多菜单包含 "Sandbox Settings"、"Export All"、"Import"
- 无脚本时 "Export All" 为禁用状态；已有脚本时 "Export All" 可点击
- 点击 "Sandbox Settings" 后打开 Sandbox Settings 对话框
- Import 操作仍通过 `.bifrost` 文件选择器触发，不再作为顶部平铺按钮展示

---

### TC-WSC-04C：通过 ... 下拉菜单触发真实 Import 文件选择器

**操作步骤**：
1. 在 Scripts 页面，切换到暗色主题。
2. 点击左侧面板头部的 `...` 更多菜单（data-testid: `scripts-more-menu-button`）。
3. 点击 "Import"（data-testid: `scripts-more-import-item`）。
4. 使用浏览器自动化等待 `filechooser` 事件。

**预期结果**：
- 更多菜单中的 Import 项可见且可点击。
- 点击 Import 后浏览器真实触发文件选择器，而不仅是菜单项可见。
- 文件选择器支持选择 `.bifrost` 文件，且当前入口允许多文件选择。

---

### TC-WSC-05：在代码编辑器中编辑脚本

**前置条件**：已通过 TC-WSC-02 创建 `test-request-script`

**操作步骤**：
1. 在左侧列表中点击 `test-request-script`
2. 在右侧 Monaco 编辑器中修改脚本内容，添加一行：
   ```javascript
   log.info("Script executed");
   ```

**预期结果**：
- 点击脚本后，编辑器加载该脚本的完整内容
- 编辑器面板标题显示脚本名称 `test-request-script`
- 编辑器支持 TypeScript 语法高亮
- 编辑器提供代码补全（输入 `request.` 时弹出属性建议）
- 编辑器提供 Bifrost 类型定义提示（BifrostRequest、BifrostContext 等）

---

### TC-WSC-05A：Parser 脚本编辑器识别 bp parser 运行时上下文字段

**操作步骤**：
1. 在 Scripts 页面，点击左侧面板头部的 `+` 创建菜单。
2. 点击 "Parser Script"。
3. 在 Monaco 编辑器中粘贴包含以下访问模式的脚本：
   - `ctx.phase === "websocket_send"`
   - `ctx.phase === "websocket_recv"`
   - `request.bodyBase64`
   - `response.bodyBase64`
   - `response.request.path`
   - `req.method` / `req.host` / `req.path` / `req.url`
4. 等待 Monaco diagnostics 刷新。

**预期结果**：
- 编辑器类型系统识别 `ctx.phase` 的 request / response / websocket_send / websocket_recv 四种阶段。
- Parser/Decode 脚本中 `request.bodyBase64`、`response.bodyBase64` 和 `response.request` 请求快照字段都有补全提示。
- 类似 `build_in_bp` 的 `currentRequest()` 写法不会因为 `req.method`、`req.host`、`req.path`、`req.url` 被误报类型错误。

---

### TC-WSC-06：保存已有脚本

**前置条件**：已在 TC-WSC-05 中修改了脚本内容

**操作步骤**：
1. 点击右上角 "Save" 按钮（data-testid: `scripts-save-button`）

**预期结果**：
- 保存成功后显示 Toast 消息 "Script saved"
- 不会弹出命名对话框（因为是已有脚本，非新建）
- 编辑器内容保持修改后的版本

---

### TC-WSC-07：使用快捷键保存脚本

**前置条件**：已有脚本处于编辑状态

**操作步骤**：
1. 在编辑器中进行修改
2. 按下 `Cmd+S`（macOS）快捷键

**预期结果**：
- 脚本保存成功，显示 Toast 消息 "Script saved"
- 快捷键与点击 Save 按钮效果一致

---

### TC-WSC-08：脚本名称校验 — 不允许特殊字符

**操作步骤**：
1. 通过 `+` 创建菜单点击 "Request Script" 创建新脚本
2. 点击 "Save" 按钮
3. 在保存对话框中输入名称 `test@script!`
4. 点击 "Save"

**预期结果**：
- 显示错误消息 "Script name can only contain letters, numbers, hyphens, underscores and slashes"
- 脚本未被保存
- 对话框保持打开状态

---

### TC-WSC-09：脚本名称校验 — 空名称

**操作步骤**：
1. 通过 `+` 创建菜单点击 "Request Script" 创建新脚本
2. 点击 "Save" 按钮
3. 在保存对话框中不输入任何名称（留空）
4. 点击 "Save"

**预期结果**：
- 显示错误消息 "Please enter a script name"
- 脚本未被保存

---

### TC-WSC-10：创建带目录的脚本（名称中使用 /）

**操作步骤**：
1. 通过 `+` 创建菜单点击 "Request Script" 创建新脚本
2. 在编辑器中输入内容：
   ```javascript
   request.headers["X-Folder-Test"] = "ok";
   ```
3. 点击 "Save" 按钮
4. 在对话框中输入名称 `api/auth/add-token`
5. 点击 "Save"

**预期结果**：
- 保存成功，显示 Toast 消息 "Script created"
- 左侧列表中出现文件夹结构：文件夹 `api` > 文件夹 `auth` > 脚本 `add-token`
- 文件夹图标显示为 FolderOutlined/FolderOpenOutlined
- 脚本项图标显示为 FileOutlined
- 文件夹节点显示脚本计数标签（如 "1"）

---

### TC-WSC-11：脚本树状视图文件夹展开/折叠

**前置条件**：已通过 TC-WSC-10 创建带目录结构的脚本

**操作步骤**：
1. 在左侧列表中点击文件夹 `api`
2. 再次点击文件夹 `api`

**预期结果**：
- 首次点击文件夹时，文件夹折叠，子项不可见，图标变为 FolderOutlined
- 再次点击文件夹时，文件夹展开，子项重新可见，图标变为 FolderOpenOutlined

---

### TC-WSC-12：测试/运行脚本

**前置条件**：已创建 `test-request-script` 且编辑器中已加载该脚本

**操作步骤**：
1. 在左侧列表中点击 `test-request-script` 加载到编辑器
2. 点击右上角 "Run" 按钮（data-testid: `scripts-test-button`）

**预期结果**：
- 下方自动展开测试结果面板（data-testid: `scripts-test-result-panel`）
- 测试结果面板显示执行结果：
  - 成功时：绿色 ✓ 图标 + "Test Result (Xms)" 标题
  - 如有 Request Modifications，显示修改的 headers 等内容（JSON 格式）
  - Logs 区域显示脚本中 `log.info()` 输出的日志条目
- 每条日志显示级别标签（INFO/DEBUG/WARN/ERROR）和时间戳

---

### TC-WSC-13：查看脚本测试日志输出

**前置条件**：已有包含日志输出的脚本

**操作步骤**：
1. 创建一个新的 Request 脚本，内容为：
   ```javascript
   log.info("Info message");
   log.warn("Warning message");
   log.error("Error message");
   log.debug("Debug message");
   request.headers["X-Log-Test"] = "done";
   ```
2. 保存脚本为 `log-test-script`
3. 点击 "Run" 按钮执行测试

**预期结果**：
- 测试结果面板的 Logs 区域显示 4 条日志条目
- 各条日志分别带有蓝色 "INFO"、橙色 "WARN"、红色 "ERROR"、灰色 "DEBUG" 级别标签
- 每条日志包含时间戳和消息内容
- Request Modifications 区域显示 `"X-Log-Test": "done"`

---

### TC-WSC-14：删除脚本

**前置条件**：已创建 `test-request-script`

**操作步骤**：
1. 在左侧列表中点击 `test-request-script`
2. 点击右上角 "Delete" 按钮（data-testid: `scripts-delete-button`）
3. 在确认弹窗 "Delete Script" 中点击 "Delete"

**预期结果**：
- 弹窗内容包含 "Are you sure you want to delete"
- 确认删除后显示 Toast 消息 "Script deleted"
- 左侧列表中不再显示 `test-request-script`
- 右侧编辑器恢复到空状态

---

### TC-WSC-15：通过右键菜单删除脚本

**前置条件**：已创建至少一个脚本

**操作步骤**：
1. 在左侧列表中右键点击目标脚本项
2. 在弹出的上下文菜单中点击 "Delete"
3. 在确认弹窗中点击 "Delete"

**预期结果**：
- 右键菜单包含 "Rename"、"Export"、"Delete" 等选项
- Delete 选项显示为红色（danger）
- 确认删除后脚本从列表中移除，显示 Toast 消息 "Script deleted"

---

### TC-WSC-16：脚本模板自动填充

**操作步骤**：
1. 通过 `+` 创建菜单点击 "Request Script"
2. 检查编辑器中自动填充的模板内容
3. 关闭编辑器（选择其他脚本或创建新脚本）
4. 通过 `+` 创建菜单点击 "Response Script"
5. 检查编辑器中自动填充的模板内容
6. 关闭编辑器
7. 通过 `+` 创建菜单点击 "Decode Script"
8. 检查编辑器中自动填充的模板内容

**预期结果**：
- Request 模板包含 "Bifrost Request Script Template" 注释头，包含 `request.headers["X-Custom-Header"]` 示例
- Response 模板包含 "Bifrost Response Script Template" 注释头，包含 `response.headers["X-Processed-By"]` 示例
- Decode 模板包含 "Bifrost Decode Script Template" 注释头，包含 `ctx.output = { code: "0", data: text, msg: "" }` 示例
- 每种模板都包含 `log`、`ctx`、`file`、`net` 等可用对象的说明注释

---

### TC-WSC-17：脚本搜索过滤

**前置条件**：已创建多个脚本（如 `test-request-script`、`api/auth/add-token`、`log-test-script`）

**操作步骤**：
1. 在左侧搜索框（data-testid: `scripts-search-input`）中输入 `auth`

**预期结果**：
- 列表仅显示名称包含 "auth" 的脚本（如 `api/auth/add-token`）
- 对应的父文件夹自动展开
- 搜索关键词在脚本名称中高亮显示（黄色背景）
- 清除搜索内容后恢复显示所有脚本

---

### TC-WSC-18：脚本重命名

**前置条件**：已创建 `test-response-script`

**操作步骤**：
1. 在左侧列表中右键点击 `test-response-script`
2. 在上下文菜单中点击 "Rename"
3. 在弹出的 "Rename Script" 对话框中修改名称为 `test-response-renamed`
4. 点击 "Rename" 按钮

**预期结果**：
- Rename 对话框中预填充当前脚本名称
- 重命名成功后显示 Toast 消息 "Script renamed"
- 列表中脚本名称更新为 `test-response-renamed`

---

### TC-WSC-19：键盘上下键切换选中脚本

**前置条件**：已创建至少 2 个脚本

**操作步骤**：
1. 点击列表中的第一个脚本
2. 点击列表区域使其获得焦点
3. 按下键盘 ↓ 方向键
4. 按下键盘 ↑ 方向键

**预期结果**：
- 按 ↓ 键后，选中项切换到下一个脚本，右侧编辑器加载该脚本内容
- 按 ↑ 键后，选中项切换回上一个脚本
- 选中项的 `aria-selected` 属性为 `true`，非选中项为 `false`

---

### TC-WSC-20：桌面端 Scripts 编辑器支持基础编辑快捷键

**前置条件**：通过桌面客户端打开 Scripts 页面，并已选中一个脚本

**操作步骤**：
1. 在右侧编辑器中输入一些代码
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

### TC-WSC-21：桌面端 Undo 回原文后保存，macOS 窗口黄点消失

**前置条件**：
1. 使用 Bifrost 桌面客户端打开 Scripts 页面
2. 已选中一个已有脚本，且当前内容可明确识别原文

**操作步骤**：
1. 在编辑器末尾追加一个字符，使内容从 `A` 变为 `AB`
2. 确认 macOS 窗口左上角关闭按钮黄点出现
3. 执行一次 `Cmd+Z`，使内容回到原文 `A`
4. 点击 "Save" 按钮，或按 `Cmd+S`

**预期结果**：
- 第 2 步后，macOS 窗口左上角关闭按钮出现黄点
- 第 3 步后，内容恢复原文
- 第 4 步后，显示 Toast 消息 "Script saved"
- 保存完成后，macOS 窗口左上角关闭按钮黄点消失
- 编辑器保持当前脚本内容，不弹出新建命名对话框

---

## 清理

测试完成后清理临时数据：
```bash
rm -rf .bifrost-test
```
