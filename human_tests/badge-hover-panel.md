# Bifrost Badge Hover 规则详情面板测试用例

## 功能模块说明

在被代理页面中注入的 Bifrost Badge（左下角圆点），hover 时向上展开一个面板展示当前启用的规则详情。

- 面板数据在 HTML 注入时内联（非跨域 fetch），避免跨站安全风险
- 规则数据通过 `AdminState.badge_rules_cache` 缓存，规则变更时自动刷新（启动/API 操作/热重载）
- 面板展示：规则列表（My Rules + 分组规则）、合并规则内容（可折叠）
- 规则行可点击在新窗口中打开对应的规则编辑页面

## 前置条件

```bash
BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- start -p 8800 --unsafe-ssl --intercept --enable-badge-injection
```

---

## 测试用例

### TC-BHP-01：Badge 注入包含面板 HTML、内联数据和脚本

**操作步骤**：
```bash
curl -x http://127.0.0.1:8800 http://httpbin.org/html -s
```

**预期结果**：
- HTML 中包含 `__bifrost_badge__`、`__bb_panel__`（面板容器）
- HTML 中包含 `<script>` 标签及 `merged_content`、`admin_port` 内联数据
- **不**包含 `fetch(` 调用（数据完全内联，无跨域请求）

---

### TC-BHP-02：Hover 展开面板展示规则列表

**操作步骤**：
1. 创建规则：
   ```bash
   curl -X POST http://127.0.0.1:8800/_bifrost/api/rules \
     -H "Content-Type: application/json" \
     -d '{"name":"test-badge-rule","content":"example.com mock 200\nhttpbin.org mock 302 https://google.com","enabled":true}' -s
   ```
2. 通过代理获取 HTML 并在浏览器中打开
3. 鼠标悬浮到左下角 Badge 圆点上

**预期结果**：
- 面板从 Badge 上方向上展开
- 面板标题显示 "Active Rules" + 活跃规则数
- "My Rules" 分区显示 `test-badge-rule`（2 rules）
- 面板滚动条仅在最外层卡片上，无内部嵌套滚动条

---

### TC-BHP-03：规则行点击跳转到规则编辑页

**操作步骤**：
1. Hover 展开面板后，检查规则行的 HTML

**预期结果**：
- 私有规则链接格式：`http://127.0.0.1:8800/_bifrost/rules?rule=test-badge-rule`
- 小组规则链接格式：`http://127.0.0.1:8800/_bifrost/rules?group={group_id}&rule={name}`
- 链接带 `target="_blank" rel="noopener"`，点击在新窗口打开

---

### TC-BHP-04：Merged Rules 折叠展开

**操作步骤**：
1. Hover 展开面板
2. 点击 "▾ Merged Rules" 标题

**预期结果**：
- 折叠区域展开，显示合并后的规则文本
- 内容为等宽字体，保留换行格式
- 再次点击可折叠收起

---

### TC-BHP-05：Merged Rules 一键复制

**操作步骤**：
1. 使用临时数据目录和非 9900 端口启动 Bifrost，带 `--no-system-proxy --enable-badge-injection`
2. 创建并启用一条包含多行规则内容的规则
3. 通过代理在浏览器中打开测试 HTML 页面
4. 鼠标悬浮到左下角 Badge 圆点上
5. 点击 "▾ Merged Rules" 标题展开代码框
6. 点击代码框右上角的 "Copy" 按钮
7. 读取系统剪贴板内容，或粘贴到可编辑输入框中检查内容，禁止只看按钮状态

**预期结果**：
- Merged Rules 代码框右上角显示复制按钮
- 点击复制后按钮短暂显示 "Copied"
- 剪贴板内容等于当前展开的合并规则文本，包含规则换行与缩进
- 如果浏览器拒绝写入剪贴板，按钮显示 "Failed"，不能在剪贴板为空时显示 "Copied"

---

### TC-BHP-06：Badge 弹窗层级高于页面高 z-index 浮层

**操作步骤**：
1. 使用临时数据目录和非 9900 端口启动 Bifrost，带 `--no-system-proxy --enable-badge-injection`
2. 通过代理打开一个包含 `z-index: 2147483646` 固定定位覆盖层的测试 HTML 页面
3. 鼠标悬浮到左下角 Badge 圆点上
4. 观察 Badge 和展开后的面板是否被覆盖层遮挡

**预期结果**：
- Badge 圆点显示在页面覆盖层上方
- Hover 后面板显示在页面覆盖层上方
- 面板中的规则列表和 Merged Rules 区域可正常点击

---

### TC-BHP-07：暗色模式适配

**操作步骤**：
1. 系统切换到暗色模式
2. Hover 展开面板

**预期结果**：
- 面板背景为深色（#1f1f1f）
- 文字颜色适配暗色主题
- Merged Rules 代码块背景也为深色

---

### TC-BHP-08：高性能缓存验证

**操作步骤**：
1. 查看启动日志确认初始缓存加载
2. 通过 API 创建/删除规则，然后请求代理 HTML 查看面板数据

**预期结果**：
- 面板数据在规则变更后自动更新（无需重启服务）
- 高并发下 badge 注入不会触发文件系统 IO（使用缓存读取）

---

### TC-BHP-09：禁用 badge 后面板不注入

**操作步骤**：
```bash
curl -X PUT http://127.0.0.1:8800/_bifrost/api/config/performance \
  -H "Content-Type: application/json" -d '{"inject_bifrost_badge":false}' -s
curl -x http://127.0.0.1:8800 http://httpbin.org/html -s | grep "__bb_panel__"
```

**预期结果**：
- 返回的 HTML 中不包含 `__bifrost_badge__` 和 `__bb_panel__`

---

## 清理步骤

```bash
curl -X DELETE http://127.0.0.1:8800/_bifrost/api/rules/test-badge-rule -s
curl -X PUT http://127.0.0.1:8800/_bifrost/api/config/performance \
  -H "Content-Type: application/json" -d '{"inject_bifrost_badge":true}' -s
```
