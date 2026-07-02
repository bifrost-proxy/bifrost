# Web UI 规则管理页面测试用例

## 前置条件

1. 启动 Bifrost 服务（使用临时数据目录避免污染正式环境）：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- start -p 8800 --unsafe-ssl --no-system-proxy
   ```
2. 使用 Chrome 浏览器访问 `http://127.0.0.1:8800/_bifrost/`
3. 确保服务正常运行且无其他规则文件残留（如有必要可先清理 `.bifrost-test` 目录重新启动）

---

## 测试用例

### TC-WRU-01：访问规则页面

**操作步骤**：
1. 在浏览器中打开 `http://127.0.0.1:8800/_bifrost/rules`

**预期结果**：
- 页面正常加载，URL 为 `http://127.0.0.1:8800/_bifrost/rules`
- 左侧显示规则列表面板（标题为 "Rules"），右侧显示规则编辑器面板
- 左侧面板包含顶部工具栏（New Rule 按钮、Refresh 按钮、Import 按钮）
- 左侧面板包含搜索框和排序选择器
- 底部状态栏显示 "0 rules, 0 enabled"（初始状态无规则时）
- 右侧编辑器区域显示 "Select a rule to edit" 空状态提示

---

### TC-WRU-02：创建新规则文件

**操作步骤**：
1. 点击左侧面板顶部的 "+" (New Rule) 按钮
2. 在弹出的 "New Rule" 对话框中，输入规则名称 `test-rule-01`
3. 点击 "Create" 按钮

**预期结果**：
- 弹出 Modal 对话框，标题为 "New Rule"，包含规则名称输入框
- 创建成功后显示 Toast 消息 "Rule created"
- 左侧规则列表中出现 `test-rule-01`
- 右侧编辑器自动加载该规则的内容（默认内容为 `# New rule`）
- 编辑器标题显示 `test-rule-01`
- 底部状态栏更新为 "1 rules, 0 enabled"

---

### TC-WRU-03：规则列表展示

**前置条件**：已通过 TC-WRU-02 创建 `test-rule-01`，再创建 `test-rule-02` 和 `demo/sub-rule`

**操作步骤**：
1. 观察左侧规则列表面板

**预期结果**：
- 列表中按顺序显示所有已创建的规则文件
- 每个规则项显示规则名称
- 每个规则项右侧有 Switch 开关（默认关闭状态）
- 已启用的规则项显示绿色勾号图标
- 底部状态栏显示 "3 rules, 0 enabled"
- 点击某一规则项时，该项高亮选中，右侧编辑器加载对应内容

---

### TC-WRU-04：树形视图展示（文件夹结构）

**前置条件**：已创建名称包含 `/` 的规则，如 `demo/sub-rule`

**操作步骤**：
1. 观察左侧规则列表中的文件夹结构

**预期结果**：
- 名称带 `/` 的规则自动按路径分组为树形结构
- 显示文件夹图标和文件夹名称（如 `demo`）
- 文件夹左侧显示展开/折叠箭头（CaretDown / CaretRight）
- 点击文件夹行可展开/折叠子项
- 展开时文件夹图标变为打开状态（FolderOpen），折叠时为关闭状态（Folder）
- 子规则项（如 `sub-rule`）缩进显示在文件夹内
- 选中子规则时，父文件夹自动展开

---

### TC-WRU-05：编辑规则内容

**前置条件**：已选中 `test-rule-01`

**操作步骤**：
1. 在右侧 BifrostEditor 编辑器中，清除默认内容
2. 输入以下规则内容：
   ```
   example.com host://127.0.0.1:3000
   ```

**预期结果**：
- 编辑器支持正常文本输入和编辑
- 输入内容后，规则列表中 `test-rule-01` 旁出现未保存标记（橙色小圆点）
- 编辑器顶部 "Save" 按钮变为可点击状态（非 disabled）
- 编辑器标题区域显示规则元信息（Created 时间、Updated 时间等）

---

### TC-WRU-06：保存规则变更

**前置条件**：已在 TC-WRU-05 中编辑了规则内容

**操作步骤**：
1. 点击编辑器顶部的 "Save" 按钮

**预期结果**：
- 显示 Toast 消息 "Saved"
- 未保存标记（橙色小圆点）消失
- "Save" 按钮恢复为 disabled 状态
- 规则内容已持久化保存

---

### TC-WRU-07：使用快捷键保存

**前置条件**：已选中规则并修改了内容

**操作步骤**：
1. 在编辑器中输入任意新内容
2. 按下 `Cmd+S`（macOS）或 `Ctrl+S`（Windows/Linux）

**预期结果**：
- 显示 Toast 消息 "Saved"
- 规则内容保存成功
- 未保存标记消失

---

### TC-WRU-08：桌面端 Undo 回原文后保存，macOS 窗口黄点消失

**前置条件**：
1. 使用 Bifrost 桌面客户端打开 Rules 页面（不是普通浏览器 Web 页面）
2. 已选中一个已有规则，且当前内容可明确识别为 `A`（例如仅一行 `example.com host://127.0.0.1:3000` 也可，关键是能确认“回到原文”）

**操作步骤**：
1. 在右侧编辑器末尾追加一个字符，使内容从 `A` 变为 `AB`
2. 观察 macOS 窗口左上角关闭按钮黄点出现
3. 执行一次 `Cmd+Z`，使编辑器内容回到原始内容 `A`
4. 点击编辑器顶部 "Save" 按钮，或按 `Cmd+S`

**预期结果**：
- 第 2 步后，macOS 窗口左上角关闭按钮出现黄点
- 第 3 步后，编辑器内容恢复为原文，Rules 列表中的未保存圆点仍可保留
- 第 4 步后，显示 Toast 消息 "Saved"
- 保存完成后，macOS 窗口左上角关闭按钮黄点消失
- Rules 列表中的未保存圆点消失，Save 按钮恢复禁用

---

### TC-WRU-09：启用/禁用规则文件

**前置条件**：已创建 `test-rule-01` 并保存了有效规则内容

**操作步骤**：
1. 在规则列表中，点击 `test-rule-01` 右侧的 Switch 开关（开启）
2. 观察变化
3. 再次点击 Switch 开关（关闭）

**预期结果**：
- 开启时：
  - 显示 Toast 消息 "Rule enabled"
  - Switch 切换为开启状态
  - 规则名称旁显示绿色勾号图标
  - 底部状态栏中 enabled 计数增加
- 关闭时：
  - 显示 Toast 消息 "Rule disabled"
  - Switch 切换为关闭状态
  - 绿色勾号图标消失
  - 底部状态栏中 enabled 计数减少

---

### TC-WRU-10：通过右键菜单启用/禁用规则

**操作步骤**：
1. 右键点击规则列表中的 `test-rule-01`
2. 在右键菜单中选择 "Enable"（或 "Disable"，取决于当前状态）

**预期结果**：
- 右键菜单包含 Enable/Disable、Rename、Export、Delete 选项
- 选择 Enable/Disable 后，规则状态正确切换
- 显示对应的 Toast 消息

---

### TC-WRU-11：通过双击启用/禁用规则

**操作步骤**：
1. 双击规则列表中的 `test-rule-01`

**预期结果**：
- 规则启用状态切换（启用变禁用 / 禁用变启用）
- 显示对应的 Toast 消息 "Rule enabled" 或 "Rule disabled"

---

### TC-WRU-12：删除规则文件

**前置条件**：已创建 `test-rule-02`

**操作步骤**：
1. 选中 `test-rule-02`
2. 点击编辑器顶部的 "Delete" 按钮

**预期结果**：
- 弹出确认对话框，内容为 `Are you sure to delete "test-rule-02"?`
- 点击 "Delete" 确认后，显示 Toast 消息 "Rule deleted"
- 规则从列表中移除
- 底部状态栏更新规则总数

---

### TC-WRU-13：通过右键菜单删除规则

**操作步骤**：
1. 右键点击要删除的规则
2. 选择 "Delete"

**预期结果**：
- 弹出确认对话框
- 确认后规则被删除，显示 Toast 消息 "Rule deleted"

---

### TC-WRU-14：编辑器语法高亮

**操作步骤**：
1. 选中一个规则文件
2. 在编辑器中输入以下内容：
   ```
   # 这是一条注释
   example.com host://127.0.0.1:3000
   /api/** statusCode://200
   ```

**预期结果**：
- 以 `#` 开头的注释行使用注释颜色高亮
- 协议关键字（如 `host`、`statusCode`）使用特定颜色高亮
- `://` 分隔符正确着色
- URL 模式（如 `example.com`、`/api/**`）使用独立颜色
- 值部分（如 `127.0.0.1:3000`、`200`）有对应颜色
- 编辑器支持 `bifrost` 自定义语言的 Monarch 语法高亮规则

---

### TC-WRU-15：编辑器自动补全 — 协议补全

**操作步骤**：
1. 在编辑器中新起一行
2. 输入 `example.com ` 后，继续输入 `host`

**预期结果**：
- 输入协议名称时弹出自动补全下拉列表
- 补全列表中包含已注册的协议（如 `host`、`xHost`、`statusCode`、`method`、`replaceStatus` 等）
- 每个补全项显示协议名称和简要描述
- 选择一个补全项后，自动插入完整的协议 snippet（如 `host://(key=value)` 格式）
- 支持多种 snippet 变体：inline 参数格式、block 变量引用格式

---

### TC-WRU-16：编辑器自动补全 — 变量引用补全

**前置条件**：在 Values 页面已创建一个名为 `my_target` 的全局变量

**操作步骤**：
1. 在编辑器中输入 `{`
2. 观察补全列表

**预期结果**：
- 弹出补全列表，包含全局变量（标识为 "Global Value"）和本地变量（如有）
- 全局变量如 `{my_target}` 出现在补全候选中
- 选择后自动插入 `{my_target}` 完整引用

---

### TC-WRU-17：编辑器语法校验 — 错误提示

**操作步骤**：
1. 在编辑器中输入不合法的规则语法，如：
   ```
   example.com unknownProtocol://value
   ```
2. 等待编辑器校验完成（约 500ms 防抖延迟）

**预期结果**：
- 编辑器在不合法的行下方显示红色波浪线标记
- 鼠标悬停在错误标记上时，显示错误信息 tooltip
- 错误信息说明具体的语法问题（如未知协议）

---

### TC-WRU-18：规则内容使用 inline values（``` blocks）

**操作步骤**：
1. 在编辑器中输入包含 inline value 的规则：
   ```
   example.com resBody://{my_body}
   ```my_body
   {"code": 200, "message": "hello"}
   ```
   ```

**预期结果**：
- ``` 代码块区域被正确识别并高亮
- 变量名（如 `my_body`）在代码块头部和引用处均有高亮
- 代码块内的内容不受外部语法规则影响
- 保存后规则生效，inline value 可被正常引用

---

### TC-WRU-19：Dynamic Island 显示活跃规则摘要

**前置条件**：已创建并启用至少 2 个包含有效规则的规则文件

**操作步骤**：
1. 在 Rules 页面，观察页面顶部中央位置的 Dynamic Island 胶囊组件

**预期结果**：
- Dynamic Island 显示为浮动胶囊样式，位于页面顶部居中
- 胶囊中显示闪电图标和 "{N} active" 文字（N 为活跃规则文件数量）
- 闪电图标旁有绿色徽标显示活跃规则数
- 若无活跃规则，图标和数字显示为灰色

---

### TC-WRU-20：Dynamic Island 展开查看详情

**前置条件**：已启用至少 1 个规则文件

**操作步骤**：
1. 点击 Dynamic Island 胶囊

**预期结果**：
- 胶囊展开，下方弹出详情面板
- 面板中列出所有活跃规则文件，每个条目包含：
  - 闪电图标（绿色）
  - 规则文件名称
  - 规则数量（如 "2 rules"）
- 标题区域显示 "My Rules"
- 点击某个规则条目可跳转选中该规则文件
- 底部有 "Show Merged Rules" 链接，点击可展开查看合并后的完整规则文本
- 在面板外部点击可收起面板

---

### TC-WRU-21：Dynamic Island 拖拽移动

**操作步骤**：
1. 鼠标按住 Dynamic Island 胶囊不松开
2. 拖动至页面的其他位置

**预期结果**：
- 胶囊跟随鼠标移动，光标变为 grabbing 样式
- 松开鼠标后胶囊停留在新位置
- 胶囊移动范围被限制在父容器内部
- 短距离点击（未超过拖拽阈值 4px）视为点击而非拖拽，触发展开/收起

---

### TC-WRU-22：Dynamic Island 变量冲突警告

**前置条件**：在两个不同的启用规则文件中定义同名 inline value（如都包含 ``` block 定义 `my_var`）

**操作步骤**：
1. 观察 Dynamic Island 胶囊

**预期结果**：
- 胶囊中出现黄色警告图标（WarningOutlined）
- 鼠标悬停警告图标时显示 tooltip，如 "1 variable conflict"
- 展开详情面板后，顶部显示 "Variable Conflicts" 黄色警告区域
- 列出冲突的变量名及来源规则文件和值预览

---

### TC-WRU-23：通过 Import 按钮导入 .bifrost 文件

**前置条件**：准备一个包含规则的 `.bifrost` 格式文件

**操作步骤**：
1. 点击左侧面板顶部的 Import 按钮（ImportOutlined 图标）
2. 在文件选择对话框中选择一个 `.bifrost` 文件
3. 确认导入

**预期结果**：
- 文件选择对话框默认过滤 `.bifrost` 格式文件
- 导入成功后显示 Toast 消息 "导入成功"
- 规则列表自动刷新，显示新导入的规则文件
- 若文件类型不匹配（非 rules 类型），显示错误消息 "文件类型不匹配"

---

### TC-WRU-24：拖拽 .bifrost 文件导入规则

**前置条件**：准备一个包含规则的 `.bifrost` 格式文件

**操作步骤**：
1. 从文件管理器中拖拽一个 `.bifrost` 文件到 Bifrost Web UI 页面上

**预期结果**：
- 拖入文件时页面显示全屏蒙层，提示 "释放以导入 .bifrost 文件"（带上传图标）
- 释放文件后蒙层消失，显示导入进度 Modal（"正在导入..." 加载中状态）
- 导入成功后显示 Toast 消息 "导入 {文件名} 成功"
- 自动刷新规则列表并跳转到 Rules 页面
- 若拖入的文件非 `.bifrost` 格式，显示警告消息 "请拖入 .bifrost 格式的文件"

---

### TC-WRU-25：多规则文件管理 — 批量选择

**前置条件**：已创建至少 3 个规则文件

**操作步骤**：
1. 点击第一个规则
2. 按住 `Ctrl`（macOS 为 `Cmd`）点击第三个规则
3. 按住 `Shift` 点击第二个规则

**预期结果**：
- `Ctrl/Cmd+Click` 可逐个添加或取消选择
- `Shift+Click` 选择连续范围内的所有规则
- 被多选的规则项有独立的高亮样式（multiSelected）
- 右键点击任意一个被选中的规则时，右键菜单中的 Export 和 Delete 显示选中数量（如 "Export (3)"、"Delete (3)"）

---

### TC-WRU-26：多规则文件管理 — 批量删除

**前置条件**：已通过 TC-WRU-25 多选了若干规则

**操作步骤**：
1. 右键点击被选中的规则之一
2. 选择 "Delete (N)"

**预期结果**：
- 弹出确认对话框，内容为 `Are you sure to delete N rules?`
- 确认后逐一删除，显示 Toast 消息 "{N} rules deleted"
- 所有被选中的规则从列表中移除
- 底部状态栏更新规则总数

---

### TC-WRU-27：多规则文件管理 — 批量导出

**前置条件**：已多选若干规则

**操作步骤**：
1. 右键点击被选中的规则之一
2. 选择 "Export (N)"

**预期结果**：
- 浏览器开始下载一个 `.bifrost` 文件
- 文件名格式为 `bifrost-rules-{N}.bifrost`（多个规则时）或 `{规则名}.bifrost`（单个规则时）

---

### TC-WRU-28：规则列表搜索过滤

**前置条件**：已创建多个不同名称的规则文件

**操作步骤**：
1. 在左侧面板搜索框中输入 `test`

**预期结果**：
- 规则列表实时过滤，仅显示名称包含 "test" 的规则
- 搜索不区分大小写
- 文件夹节点在搜索时自动展开
- 搜索框支持清除按钮，点击后恢复完整列表
- 无匹配结果时显示 "No matching rules"

---

### TC-WRU-29：规则列表排序

**操作步骤**：
1. 在左侧面板搜索框旁的排序选择器中切换为 `Updated`
2. 刷新 Rules 页面
3. 再将排序选择器切换为 `Name`
4. 再次刷新 Rules 页面
5. 最后切换回 `Manual`

**预期结果**：
- 支持三种排序模式：Manual（手动）、Updated（按更新时间倒序）、Name（按名称升序）
- Manual 模式下可拖拽排序（显示拖拽手柄 HolderOutlined）
- Updated 模式下最近更新的规则排在最前
- Name 模式下按字母顺序排列
- 切换排序模式后列表立即重新排列
- 第 2 步刷新后排序选择器仍显示 `Updated`，列表仍按更新时间倒序展示
- 第 4 步刷新后排序选择器仍显示 `Name`，列表仍按名称升序展示
- 排序方式写入 UI 配置信息，刷新页面不丢失状态

**本次执行记录（2026-05-09）**：
- 通过。使用临时数据目录 `.bifrost-human-ui` 启动 `./.bifrost-ui-target/debug/bifrost start -p 8800 --unsafe-ssl --no-system-proxy`。
- 创建 `aaa-human-sort` 与 `zzz-human-sort`，更新 `aaa-human-sort` 后在浏览器中验证：切换 `Updated` 后刷新仍显示 `Updated` 且 `aaa-human-sort` 排第一；切换 `Name` 后刷新仍显示 `Name` 且 `aaa-human-sort` 排第一；最后切回 `Manual`。

---

### TC-WRU-30：规则拖拽排序

**前置条件**：排序模式为 Manual，已创建多个规则

**操作步骤**：
1. 通过左侧拖拽手柄图标按住一个规则
2. 向上或向下拖动到目标位置
3. 松开鼠标

**预期结果**：
- 拖拽过程中出现目标位置指示线（before/after）
- 拖拽到列表边缘时自动滚动
- 松开后规则移动到目标位置
- 显示 Toast 消息 "Rule order updated"
- 新顺序持久化保存

---

### TC-WRU-31：规则重命名

**操作步骤**：
1. 右键点击规则列表中的 `test-rule-01`
2. 选择 "Rename"
3. 在弹出的 "Rename Rule" 对话框中输入新名称 `renamed-rule`
4. 点击 "Rename" 按钮

**预期结果**：
- 弹出 Modal 对话框，标题为 "Rename Rule"
- 输入框预填充当前规则名称
- 重命名成功后显示 Toast 消息 "Rule renamed"
- 列表中规则名称更新为 `renamed-rule`
- 编辑器标题同步更新

---

### TC-WRU-32：编辑器 Copy 功能

**前置条件**：已选中一个有内容的规则

**操作步骤**：
1. 点击编辑器顶部的 "Copy" 按钮

**预期结果**：
- 显示 Toast 消息 "Copied"
- 规则完整内容已复制到系统剪贴板
- 可在其他文本编辑器中粘贴验证

---

### TC-WRU-33：编辑器 Hover 提示

**操作步骤**：
1. 在编辑器中输入 `example.com host://127.0.0.1:3000`
2. 将鼠标悬停在 `host` 协议关键字上

**预期结果**：
- 弹出 Hover 信息面板
- 显示协议名称、描述、值类型、示例等文档信息
- 悬停在变量引用（如 `{my_var}`）上时显示变量来源信息

---

### TC-WRU-34：编辑器键盘导航

**操作步骤**：
1. 点击左侧规则列表使其获得焦点
2. 按 `↓` 键和 `↑` 键

**预期结果**：
- 使用上下箭头键可在规则列表中导航
- 当前高亮项自动滚动到可视区域
- 选中规则时右侧编辑器同步加载对应内容
- 多选状态在键盘导航时被清除

---

### TC-WRU-35：编辑器元信息展示

**前置条件**：已选中一个规则

**操作步骤**：
1. 观察编辑器标题区域下方的元信息行

**预期结果**：
- 显示以下元信息项（以分隔格式排列）：
  - `Sync Local only`（同步状态，本地规则显示为 "Local only"）
  - `Created YYYY-MM-DD HH:mm:ss`（创建时间）
  - `Updated YYYY-MM-DD HH:mm:ss`（更新时间）
  - `Last sync --`（最近同步时间，未同步时显示 "--"）

---

### TC-WRU-36：Dynamic Island 显示合并规则内容

**前置条件**：已启用至少 1 个规则文件且规则内容非空

**操作步骤**：
1. 点击 Dynamic Island 胶囊展开详情
2. 点击底部的 "Show Merged Rules" 链接

**预期结果**：
- 展开一个 `<pre>` 代码块，显示所有活跃规则合并后的完整文本内容
- 代码块使用等宽字体、灰色背景、圆角边框
- 再次点击变为 "Hide Merged Rules" 可收起
- 合并内容反映所有启用规则文件中的规则条目

---

### TC-WRU-37：无活跃规则时 Dynamic Island 状态

**前置条件**：所有规则文件均处于禁用状态

**操作步骤**：
1. 点击 Dynamic Island 胶囊

**预期结果**：
- 胶囊显示 "0 active"，图标为灰色
- 展开后显示 "No active rules" 提示文本
- 无规则列表和合并内容区域

---

### TC-WRU-38：桌面端 Rules 编辑器支持基础编辑快捷键

**前置条件**：通过桌面客户端打开 Rules 页面，并已选中一条规则

**操作步骤**：
1. 在右侧编辑器中输入三行文本
2. 按 `Cmd+A`
3. 按 `Cmd+C`
4. 按 `Cmd+X`
5. 按 `Shift+Cmd+Z` 与 `Cmd+Z`

**预期结果**：
- `Cmd+A` 可全选编辑器文本
- `Cmd+C` 可复制选中文本
- `Cmd+X` 可剪切选中文本
- `Cmd+Z` / `Shift+Cmd+Z` 可撤销与重做

---

### TC-WRU-39：Dynamic Island Merged Rules 一键复制

**前置条件**：已启用至少 1 个规则文件且规则内容非空

**操作步骤**：
1. 点击 Dynamic Island 胶囊展开详情
2. 点击底部的 "Show Merged Rules" 链接
3. 点击 Merged Rules 代码框右上角的复制按钮
4. 在文本编辑器或可输入区域粘贴剪贴板内容

**预期结果**：
- Merged Rules 代码框右上角显示复制按钮
- 点击复制按钮后显示 "Merged rules copied" Toast
- 粘贴出的内容与代码框内展示的合并规则文本一致
- 复制按钮不会触发 Dynamic Island 收起或跳转规则详情

---

### TC-WRU-40-回归：编辑器内容恢复原文后 Save 按钮应禁用

**前置条件**：已选中一个已有内容的规则（如 `test-rule-01`，内容非空）

**操作步骤**：
1. 在编辑器中追加一个空格（例如在现有内容末尾键入空格键）
2. 按 `Backspace` 删除刚输入的空格，使编辑器内容恢复为原始值
3. 观察编辑器顶部的 Save 按钮状态
4. 在编辑器中输入实际变更内容（例如在末尾添加 `xyz`）
5. 再次观察 Save 按钮状态
6. 点击 Save 按钮保存

**预期结果**：
- 第 2 步后编辑器内容恢复为原文，Save 按钮应为 **disabled（禁用）** 状态（因为内容未实际变更）
- 第 4 步后内容发生实际变更，Save 按钮应变为 **enabled（可点击）** 状态
- 第 6 步保存成功，显示 Toast 消息 "Saved"，规则内容已持久化

**回归目的**：验证修复后，打字再回退到原文时 Save 按钮正确禁用，避免误启用的"无操作保存"导致用户困惑。

---

### TC-WRU-41-回归：Group 规则 active summary 不依赖远端 group 信息

**前置条件**：
1. 使用临时数据目录和非 9900 端口启动 Bifrost，带 `--no-system-proxy`。
2. 准备一个本地 Group 规则目录，目录下至少有一条启用规则。
3. 断开或模拟远端 group/peer 接口不可用。

**操作步骤**：
1. 打开 `http://127.0.0.1:8800/_bifrost/rules?group={group_id}&rule={rule_name}`。
2. 观察页面顶部 Dynamic Island。
3. 点击 Dynamic Island 胶囊展开详情。
4. 直接请求 `http://127.0.0.1:8800/_bifrost/api/rules/active-summary`。

**预期结果**：
- Dynamic Island 不应长时间卡在 `0 active`。
- 展开面板中显示本地已启用的 Group 规则。
- API 返回 200，`rules` 数组包含该 Group 规则，`merged_content` 包含该规则文本。
- 即使远端 group/peer 接口不可用，active summary 仍以本地目录名作为 fallback 展示规则，不把本地启用规则清空或删除。
- 代理运行时规则加载同样包含该本地 Group 目录；在 Web UI 中启用/禁用该 Group 规则后，代理无需等待远端刷新即可通过规则 hot reload 使用最新本地状态。

**回归目的**：防止 Rules 预览链路或代理处理链路因远端 group cache 解析失败而把已启用规则展示为 0 或跳过本地 Group 规则。

---

### TC-WRU-42-稳定性：远端失败和快速本地变更下 active summary 保持可靠

**前置条件**：
1. 使用临时数据目录和非 9900 端口启动 Bifrost，带 `--no-system-proxy --enable-badge-injection`。
2. 准备一个未缓存 group id 的本地 Group 规则目录，目录下有一条启用规则。
3. 准备一个已缓存 group id 的 Group 规则 `rapid-toggle-rule`，初始为 disabled。
4. 让 sync session 缺失、失效或远端 group/peer 接口返回错误，模拟临时登录态失效或网络抖动。

**操作步骤**：
1. 连续请求 `/_bifrost/api/rules/active-summary` 两次，第二次请求应在第一次后台 group cache 解析失败后执行。
2. 检查本地 Group 规则目录仍存在，没有被 active summary 当作 orphan 删除。
3. 对 `rapid-toggle-rule` 连续执行 3 轮 enable/disable。
4. 每次 enable/disable 后轮询 `/_bifrost/api/rules/active-summary` 和代理页面中注入的 Badge 内联数据，最长等待 2 秒。

**预期结果**：
- 未缓存 Group 在远端解析失败时仍出现在 active summary 中，`merged_content` 包含规则内容。
- 远端解析失败或请求短暂卡住后不会把 cache-resolved 状态永久卡住；后续请求仍可继续尝试补全远端映射。
- active summary 不删除本地 Group 规则目录。
- 快速 enable 后，active summary 与 Badge 都包含该规则；快速 disable 后，二者都移除该规则。
- 代理 runtime 使用的规则状态与 active summary 保持一致，不依赖远端 group 刷新。

**回归目的**：把网络抖动、临时登录态失效、系统短暂卡顿和本地修改延迟作为一等稳定性风险覆盖，避免再次出现页面和代理链路长期显示 `0 active` 或旧规则残留。

**执行结果（2026-05-20，CI Runner 本地 sync-server 回归）**：
- ✅ PASS：执行 `BIFROST_DATA_DIR=$(mktemp -d) BIFROST_E2E_REPORT_DIR=$(mktemp -d) BIFROST_E2E_RUNNER_JOBS=8 BIFROST_E2E_RETRY_FAILED_ONCE=1 BIFROST_E2E_HTTP_RETRIES=2 TIMEOUT=90 bash scripts/ci/run-e2e-runner.sh`。
- 脚本启动仓库内置 `packages/bifrost-sync-server` 本地测试服务并注册测试用户，Rust E2E 通过 `BIFROST_E2E_SYNC_BASE_URL` / `BIFROST_E2E_SYNC_TOKEN` 使用该服务。
- `bifrost-e2e` 共运行 363 个用例，结果 `358 passed / 0 failed / 5 skipped`，覆盖 `group_rules_active_summary_survives_remote_cache_resolution_failure` 和快速启停相关回归。

---

### TC-WRU-43-回归：Group URL 参数为本地组名时不返回 502

**前置条件**：
1. 已登录 Bifrost Sync，且本地存在一个 Group 规则目录，例如 `next-agent`。
2. `group_name_cache` 中存在或曾经存在该 Group 的 id/name 映射；如果映射暂时缺失，本地目录仍存在。
3. 该 Group 下存在一条规则，例如 `NextOncall双前端本地开发`。

**操作步骤**：
1. 在浏览器打开：
   ```text
   http://127.0.0.1:9900/_bifrost/rules?group=next-agent&rule=NextOncall%E5%8F%8C%E5%89%8D%E7%AB%AF%E6%9C%AC%E5%9C%B0%E5%BC%80%E5%8F%91
   ```
2. 观察 Rules 页面是否能进入 Group 模式。
3. 检查 Network 中的 `/_bifrost/api/group-rules/next-agent` 和规则详情请求。
4. 在同一页面尝试启用/停用该 Group 规则。

**预期结果**：
- 页面不显示 502 错误。
- `/_bifrost/api/group-rules/next-agent` 返回 200，并返回本地 Group 规则列表。
- 规则详情请求返回 200，编辑器展示目标规则内容。
- 如果 cache 已能反查真实 group id，响应里的 `group_id` 为真实 id；如果远端或 cache 临时不可用，也至少返回本地目录规则并保持只读/本地可启停能力。
- 启用/停用后 active summary、Badge 和代理运行时按本地最新状态刷新。

**回归目的**：覆盖 Badge/Rules 深链历史契约中 `group` 参数使用本地 group name 的情况，防止后端把该值误当远端 group id 而请求失败。

---

### TC-WRU-44-回归：退出登录再重新登录后 Group 规则跳转仍使用真实 ID

**前置条件**：
1. 已登录 Bifrost Sync。
2. 本地存在一个有远端 ID 映射的 Group，例如 `next-agent`。
3. Group 下存在规则 `NextOncall双前端本地开发`，并可在 Rules 页面启用/停用。

**操作步骤**：
1. 打开 Rules 页面，切到目标 Group，启用 `NextOncall双前端本地开发`。
2. 展开 Rules 页 Dynamic Island，点击该 Group 规则，确认 URL 的 `group` 参数为真实 group ID。
3. 打开 Settings / Sync，执行退出登录。
4. 重新登录 Sync。
5. 回到 Rules 页面，再次启用或确认该 Group 规则处于启用状态。
6. 再次展开 Dynamic Island 并点击该 Group 规则。
7. 打开一个被代理注入 Bifrost Badge 的页面，展开 Badge 并点击同一 Group 规则。

**预期结果**：
- 退出登录不会删除本地 `.group_cache.json` 中的 group id/name 映射。
- 退出登录后，Group 规则文件仍可作为本地缓存查看，但不会出现在 active summary、注入 Badge active 列表，也不会被代理规则引擎命中。
- 重新登录后 active summary 返回真实 `group_id`，不是本地 group name。
- Dynamic Island 点击后的 URL 为 `/_bifrost/rules?group=<真实group_id>&rule=...`。
- 注入 Badge 保持历史 name/id 反向映射契约，点击后同样跳转到真实 group ID。
- 启用/停用后代理运行时立即热更新，规则命中状态与本地 UI 一致，不依赖远端周期刷新。

**回归目的**：覆盖 logout/login 循环导致 group cache 被清空、进而 Dynamic Island 或 Badge 回退为 `group={group_name}` 的问题。

---

### TC-WRU-45：`@规则名称` / `@组名称/规则名称` 引用解析、补全与编辑器原位展开

**前置条件**：
1. 使用临时数据目录启动 Bifrost，必须设置 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1` 并携带 `--no-system-proxy`。
2. 准备三个个人私有规则：
   - `at-ref-shared-human`：内容为 `at-rule-e2e.test reqHeaders://X-At-Rule=ok`，状态为 disabled。
   - `commented-at-ref-shared-human`：内容为 `at-rule-e2e.test reqHeaders://X-Commented-At-Rule=ok`，状态为 disabled。
   - `at-ref-entry-human`：内容为：
     ```text
     @at-ref-shared-human	# tab comment should still resolve
     @commented-at-ref-shared-human
     @at-ref-team-human/at-ref-group-shared-human	# tab comment should still resolve
     # @at-ref-comment-only-human should stay a comment
     at-rule-e2e.test host://127.0.0.1:{MOCK_PORT}
     ```
     状态为 enabled。
   - `at-ref-missing-entry-human`：内容为：
     ```text
     @missing-runtime-reference
     at-rule-missing-e2e.test host://127.0.0.1:{MOCK_PORT}
     ```
     状态为 enabled。
3. 准备一个本地缓存组 `at-ref-team-human`，其中规则 `at-ref-group-shared-human` 内容为 `at-rule-e2e.test reqHeaders://X-Group-At-Rule=ok`，状态为 disabled。
4. 准备一个本地 HTTP mock 服务，返回请求头 `X-At-Rule`、`X-Commented-At-Rule` 与 `X-Group-At-Rule` 的值。

**操作步骤**：
1. 请求 `POST /_bifrost/api/rules/validate`，body 中传入 `current_rule_name=at-ref-entry-human` 和 entry 规则内容。
2. 通过 Bifrost 代理访问 `http://at-rule-e2e.test/check`。
3. 在浏览器打开 `http://127.0.0.1:{PORT}/_bifrost/rules`，选中 `at-ref-entry-human`。
4. 请求 `GET /_bifrost/api/rules/reference-candidates`。
5. 在亮色主题下点击编辑器第一行的 `@at-ref-shared-human`。
6. 再次点击同一 `@at-ref-shared-human`。
7. 切换到暗色主题，重复第 5 步。
8. 点击第二行 `@at-ref-team-human/at-ref-group-shared-human`。
9. 在编辑器中输入 `@` 加组名/规则名的部分字符，触发 Monaco 补全。
10. 请求 `POST /_bifrost/api/rules/validate`，body 中传入 `content="@missing-rule"`。
11. 通过 Bifrost 代理访问 `http://at-rule-missing-e2e.test/check`。
12. 在 Rules 页面选中包含 `@missing-rule` 的规则，鼠标悬浮缺失引用 token。
13. 检查 entry 规则中的 `# @at-ref-comment-only-human` 注释行。

**预期结果**：
- validate API 对 entry 规则返回 `valid=true`，`rule_count=4`。
- 代理请求到达 mock 服务，mock 服务看到 `X-At-Rule: ok`、`X-Commented-At-Rule: ok` 和 `X-Group-At-Rule: ok`，说明 disabled 本地 shared、`commented-*` shared 与 disabled 组 shared 规则都只通过 entry 的 `@` 引用生效。
- reference candidates API 返回个人规则短名 `at-ref-shared-human`、`commented-at-ref-shared-human`，并返回组规则 qualified name `at-ref-team-human/at-ref-group-shared-human`。
- 亮色主题下，`@at-ref-shared-human` 有可点击样式；点击后在当前行下方展开只读详情，详情内容包含 shared 规则文本。
- 再次点击同一 `@` 引用后，展开详情收起。
- 暗色主题下，展开详情的文字、背景、边框和关闭按钮均清晰可读，不与编辑器内容重叠。
- 点击组规则引用后，当前行下方展开组规则详情，详情内容包含 `X-Group-At-Rule=ok`。
- Monaco 补全基于自动检索候选做 fuzzy 搜索，输入部分字符也能提示完整 `@at-ref-team-human/at-ref-group-shared-human`。
- 缺失引用返回 `valid=false`，错误 code 为 `E020`，错误信息包含 `missing-rule`。
- 运行时解析包含缺失引用的 enabled 规则时跳过缺失引用行，后续 `at-rule-missing-e2e.test host://127.0.0.1:{MOCK_PORT}` 仍生效，代理请求到达 mock 服务并返回 `/check`。
- Rules 编辑器中缺失引用 token 使用错误色标红；鼠标悬浮时出现错误提示，包含缺失引用名称和 `was not found`。
- `# @at-ref-comment-only-human` 保持普通注释，不被标记为规则引用，不出现红色错误 decoration，也不会触发规则引用展开。

**回归目的**：覆盖规则引用语义、disabled shared 规则复用、`commented-*` 规则名、tab 行内注释、注释内 `@` 不误报、组规则 qualified 引用、候选自动检索、模糊补全、编辑器原位展开详情、亮暗主题可读性、运行时缺失引用跳过和 UI 缺失引用诊断，防止运行时解析和 WebUI 编辑体验不一致。

**执行结果（2026-06-10，本地开发分支）**：
- ✅ PASS：执行 `bash e2e-tests/tests/test_rule_references.sh`，脚本先编译当前分支 debug 二进制，预置 disabled 组 shared 规则，再通过真实 Admin API 创建 disabled 私有 shared、`commented-*` shared、enabled entry，并用 `allow_invalid=true` 创建包含缺失引用的 enabled 规则；validate API 对 entry 返回 `valid=true` / `rule_count=4`，对缺失引用返回 `E020`，真实代理请求确认 mock 服务收到 `X-At-Rule: ok`、`X-Commented-At-Rule: ok` 与 `X-Group-At-Rule: ok`，并确认 tab 行内注释可解析、注释内 `@` 不作为缺失引用、运行时缺失引用行被跳过后 `at-rule-missing-e2e.test` 仍代理到 mock 服务。
- ✅ PASS：执行 `npm --prefix web run test:ui -- web/tests/ui/admin-rules-values.spec.ts -g "@规则引用"`，验证缺失引用返回 `E020`，缺失引用 token 标红并 hover 出现错误提示，reference candidates 返回私有短名、`commented-*` 私有短名与组 qualified name，Rules 编辑器在亮色主题下点击 `@规则` 展开/收起详情，在暗色主题下详情内容仍可读，点击 `@组名/规则名` 展开组规则详情，验证 Monaco fuzzy 补全提示完整组规则引用，并确认注释行 `# @...` 不带任何规则引用 decoration 且点击不会展开详情区。

---

### TC-WRU-46：保存缺失引用规则时展示后端语法错误且不清除未保存状态

**前置条件**：
1. 使用临时数据目录启动 Bifrost，必须设置 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1` 并携带 `--no-system-proxy`。
2. 浏览器打开 `http://127.0.0.1:{PORT}/_bifrost/rules`。
3. 已创建规则 `syntax-webui-invalid`，初始内容为 `example.com host://127.0.0.1:3000`。

**操作步骤**：
1. 在 Rules 页面选中 `syntax-webui-invalid`。
2. 将编辑器内容替换为：
   ```text
   @missing-webui-reference
   ```
3. 点击 Save 按钮，或按 `Cmd+S` / `Ctrl+S`。
4. 观察 Toast 或页面错误提示。
5. 刷新页面或重新选中 `syntax-webui-invalid`，检查编辑器内容。

**预期结果**：
- 保存请求返回失败，页面展示的错误信息包含 `E020` 或缺失引用名称 `missing-webui-reference`。
- 错误信息包含后端返回的行列位置或 suggestion，能指导 Agent/用户修复。
- Rules 列表中的未保存标记不会被错误清除，Save 按钮仍可继续用于修复后重试。
- 刷新或重新选中规则后，持久化内容仍为 `example.com host://127.0.0.1:3000`，无效内容没有落盘。
- 将内容修复为有效规则后再次保存成功，Toast 显示 `Saved`，未保存标记消失。

**回归目的**：覆盖 WebUI 保存接口不再只吞掉后端异常，而是把保存前语法检查的结构化错误反馈给调用方，并保持不落盘和可重试状态。

**执行结果（2026-06-12，本地开发分支）**：
- ✅ PASS：使用临时数据目录启动真实 Bifrost：`BIFROST_DATA_DIR=$(mktemp -d) BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 target/debug/bifrost -H 127.0.0.1 -p 61450 start -y --access-mode allow_all --skip-cert-check --no-system-proxy --unsafe-ssl`。随后通过 Playwright 打开 `http://127.0.0.1:61450/_bifrost/rules`，预置 `syntax-webui-invalid` 有效规则，编辑为 `@missing-webui-reference` 后点击 Save；保存请求返回 HTTP 422，响应体 `saved=false`、`syntax.errors[0].code=E020`，页面正文出现 `E020` 或缺失引用名，Save 按钮保持可用；通过 API 确认持久化内容仍为原有效规则；修复为有效规则后再次保存成功并出现 `Saved`。

---

### TC-WRU-47：全局 Rules 状态胶囊跨页面显示、拖拽与跳转

**前置条件**：
1. 使用临时数据目录或当前开发服务启动 Bifrost 后端，至少启用 1 条规则。
2. 启动纯前端开发服务：`cd web && WEB_PORT=3000 pnpm dev`，默认代理后端 `127.0.0.1:9900`。
3. 使用 Chrome 打开 `http://127.0.0.1:3000/_bifrost/traffic`。

**操作步骤**：
1. 在 Traffic 页面观察 Rules 状态胶囊。
2. 拖拽胶囊到页面右下方，再松开鼠标。
3. 刷新当前页面，观察胶囊位置。
4. 再次点击胶囊展开详情面板。
5. 点击详情面板中的一条活跃规则。
6. 从 Rules 页面切换到 Settings、Values 或 Traffic 页面，观察胶囊可见性。

**预期结果**：
- 胶囊在非 Rules 页面也可见，显示当前活跃规则数量，例如 `1 active`。
- 拖拽后胶囊位置随鼠标移动，松开后停留在新位置，且不会被拖出视口。
- 刷新页面后胶囊回到默认顶部居中位置，不保留上一次拖拽位置。
- 点击胶囊能展开详情面板，详情中展示 My Rules / Group Rules 分组和 Merged Rules 入口。
- 点击活跃规则后跳转到 `/_bifrost/rules?rule=<规则名>`；Group 规则跳转时 URL 包含 `group=<真实group_id>`。
- 跳转到 Rules 页面后对应规则被选中，编辑器展示该规则详情。
- 切换到其他主页面后胶囊仍保持可见。

**回归目的**：覆盖 Rules 状态胶囊从 Rules 页面内组件提升为全局 AppLayout 组件后的可见性、单页会话内拖拽、刷新回原位、详情展开和深链跳转行为，防止仅 Rules 页面可用、刷新后位置丢失到不可见区域或跨页面跳转不选中规则。

**执行结果（2026-07-02，本地纯前端 smoke）**：
- ✅ PASS：执行 `WEB_PORT=3107 PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH='/Applications/Google Chrome.app/Contents/MacOS/Google Chrome' pnpm --dir web exec playwright test tests/ui/rules-dynamic-island-global.spec.ts --config=playwright.frontend.config.ts`。测试使用 Vite 纯前端服务和 mock Admin API，不启动 Rust 后端；验证 Traffic 页面出现 `2 active` 全局胶囊、拖拽后位置变化、刷新后回到默认位置、点击展开详情、点击 `global-active-rule` 后跳转到 `/_bifrost/rules?rule=global-active-rule`，并在 Rules 编辑器中显示目标规则。

---

## 清理

测试完成后清理临时数据：
```bash
rm -rf .bifrost-test
```
