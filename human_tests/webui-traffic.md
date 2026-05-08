# Web UI Traffic 页面测试用例

## 功能模块说明

Bifrost Web UI 的 Traffic 页面是核心功能页面，用于实时展示和分析通过代理的所有 HTTP/HTTPS/WebSocket/SSE 流量。主要功能包括：

- 流量列表表格（虚拟滚动，支持大量记录）
- 流量筛选与过滤
- 流量详情面板（Overview、Header、Query、Cookie、Body、Raw、Messages、Script 等 Tab）
- 右键上下文菜单（复制 URL、复制 cURL、下载 HAR、Replay、导出等）
- 清空流量
- 搜索功能

## 前置条件

1. 启动 Bifrost 服务（使用临时数据目录避免污染正式环境）：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- start -p 8800 --unsafe-ssl
   ```
2. 生成测试流量（在另一个终端执行）：
   ```bash
   # 普通 GET 请求
   curl -x http://127.0.0.1:8800 http://httpbin.org/get

   # 带 JSON 响应的请求
   curl -x http://127.0.0.1:8800 http://httpbin.org/json

   # POST 请求带 JSON Body
   curl -x http://127.0.0.1:8800 -X POST http://httpbin.org/post \
     -H "Content-Type: application/json" \
     -d '{"name":"bifrost","version":"1.0"}'

   # 带 Query 参数的请求
   curl -x http://127.0.0.1:8800 "http://httpbin.org/get?foo=bar&lang=zh"

   # 带 Cookie 的请求
   curl -x http://127.0.0.1:8800 -b "session=abc123;token=xyz" http://httpbin.org/cookies

   # 不同状态码的请求
   curl -x http://127.0.0.1:8800 http://httpbin.org/status/404
   curl -x http://127.0.0.1:8800 http://httpbin.org/status/500
   curl -x http://127.0.0.1:8800 http://httpbin.org/status/301

   # 不同 HTTP 方法
   curl -x http://127.0.0.1:8800 -X PUT http://httpbin.org/put -d "data=test"
   curl -x http://127.0.0.1:8800 -X DELETE http://httpbin.org/delete
   curl -x http://127.0.0.1:8800 -X PATCH http://httpbin.org/patch -d "data=patch"

   # 生成较大响应体
   curl -x http://127.0.0.1:8800 http://httpbin.org/bytes/10240

   # HTML 响应
   curl -x http://127.0.0.1:8800 http://httpbin.org/html
   ```
3. 确保浏览器可访问 `http://127.0.0.1:8800/_bifrost/traffic`

---

## 测试用例

### TC-WTR-01：访问 Traffic 页面

**操作步骤**：
1. 在浏览器中打开 `http://127.0.0.1:8800/_bifrost/traffic`

**预期结果**：
- 页面正常加载，显示 Traffic 页面
- 左侧导航栏高亮 "Traffic" 菜单项
- 页面上方显示工具栏（含清空按钮、过滤按钮等）
- 页面主体显示流量表格

---

### TC-WTR-02：流量表格列显示

**前置条件**：已通过前置条件生成测试流量

**操作步骤**：
1. 在浏览器中打开 `http://127.0.0.1:8800/_bifrost/traffic`
2. 观察流量表格的表头列

**预期结果**：
- 表格包含以下列（从左到右）：`#`（序号）、状态圆点、`Protocol`、`Method`、`Status`、`Client`、`Host`、`Path`、`Type`、`Size`、`Time`、`Start Time`、`End Time`、`Rules`
- `#` 列显示 5 位序号（如 `00001`）
- 状态圆点列显示彩色圆点，代表请求状态
- `Protocol` 列显示协议标签（如 `http`、`https`）
- `Method` 列显示彩色 HTTP 方法标签（如 `GET`、`POST`）
- `Status` 列显示彩色状态码标签（如绿色 `200`、橙色 `404`、红色 `500`）
- `Host` 列显示请求目标主机名
- `Path` 列显示请求路径
- `Type` 列显示响应内容类型简写
- `Size` 列显示响应大小
- `Time` 列显示请求耗时
- `Start Time` 列显示请求开始时间（等宽字体）
- `End Time` 列显示请求结束时间
- `Rules` 列显示命中规则数（无命中时显示 `-`）

---

### TC-WTR-03：点击流量记录打开详情面板

**操作步骤**：
1. 在流量表格中点击任意一条 GET 请求记录

**预期结果**：
- 页面右侧（或下方）显示详情面板
- 详情面板顶部显示请求 URL、Method 标签和状态码
- 详情面板分为上下两个区域：Request（请求）和 Response（响应）
- 两个区域之间有可拖拽的分隔条

---

### TC-WTR-04：详情面板 Request 区域 - Overview Tab

**前置条件**：已点击一条流量记录打开详情面板

**操作步骤**：
1. 在 Request 区域点击 "Overview" Tab

**预期结果**：
- 显示请求概览信息
- 包含 General 区域，展示 URL、Method、Status、Protocol 等基本信息
- 包含 Timing 区域（如果有 timing 数据），展示时间分布条形图：DNS lookup、Connection established、TLS handshake、Request sent、Waiting (TTFB)、Content download
- Timing 表格中每项显示毫秒数
- 包含 Total 耗时

---

### TC-WTR-05：详情面板 Request 区域 - Header Tab

**操作步骤**：
1. 在 Request 区域点击 "Header" Tab

**预期结果**：
- 显示请求头列表，以键值对形式展示
- 包含常见请求头如 `Host`、`User-Agent`、`Accept` 等
- 头部名称和值清晰可读

---

### TC-WTR-06：详情面板 Request 区域 - Query Tab

**前置条件**：已通过 `curl -x http://127.0.0.1:8800 "http://httpbin.org/get?foo=bar&lang=zh"` 生成带 Query 参数的请求

**操作步骤**：
1. 点击该带 Query 参数的请求记录
2. 在 Request 区域点击 "Query" Tab

**预期结果**：
- Query Tab 可见（仅当 URL 含查询参数时显示）
- 以键值对形式展示查询参数：`foo = bar`、`lang = zh`

---

### TC-WTR-07：详情面板 Request 区域 - Cookie Tab

**前置条件**：已通过 `curl -x http://127.0.0.1:8800 -b "session=abc123;token=xyz" http://httpbin.org/cookies` 生成带 Cookie 的请求

**操作步骤**：
1. 点击该带 Cookie 的请求记录
2. 在 Request 区域点击 "Cookie" Tab

**预期结果**：
- Cookie Tab 可见（仅当请求头包含 Cookie 时显示）
- 以键值对形式展示 Cookie：`session = abc123`、`token = xyz`

---

### TC-WTR-08：详情面板 Request 区域 - Body Tab

**前置条件**：已通过 POST 请求生成带 Body 的流量

**操作步骤**：
1. 点击该 POST 请求记录
2. 在 Request 区域点击 "Body" Tab

**预期结果**：
- Body Tab 可见（仅当请求有 Body 时显示）
- 显示请求体内容 `{"name":"bifrost","version":"1.0"}`
- JSON 内容有语法高亮

---

### TC-WTR-09：详情面板 Request 区域 - Raw Tab

**操作步骤**：
1. 在 Request 区域点击 "Raw" Tab

**预期结果**：
- 显示原始 HTTP 请求文本，包含请求行（如 `GET /get HTTP/1.1`）和所有请求头
- 如果有请求体，也一并显示在头部之后

---

### TC-WTR-10：详情面板 Response 区域 - Header Tab

**操作步骤**：
1. 在 Response 区域点击 "Header" Tab

**预期结果**：
- 显示响应头列表，以键值对形式展示
- 包含常见响应头如 `Content-Type`、`Content-Length`、`Server` 等

---

### TC-WTR-11：详情面板 Response 区域 - Body Tab

**前置条件**：已点击一条返回 JSON 的请求（如 `http://httpbin.org/json`）

**操作步骤**：
1. 在 Response 区域点击 "Body" Tab

**预期结果**：
- Body Tab 可见（仅当响应有 Body 时显示）
- 显示响应体内容
- JSON 内容有语法高亮

---

### TC-WTR-12：详情面板 Response 区域 - Set-Cookie Tab

**前置条件**：请求的响应中包含 `Set-Cookie` 头

**操作步骤**：
1. 点击该请求记录
2. 在 Response 区域查看是否有 "Set-Cookie" Tab

**预期结果**：
- 当响应头包含 `Set-Cookie` 时，显示 "Set-Cookie" Tab
- 点击后以结构化形式展示 Set-Cookie 内容

---

### TC-WTR-13：详情面板 Response 区域 - Raw Tab

**操作步骤**：
1. 在 Response 区域点击 "Raw" Tab

**预期结果**：
- 显示原始 HTTP 响应文本，包含状态行（如 `HTTP/1.1 200 OK`）和所有响应头
- 如果有响应体，也一并显示在头部之后

---

### TC-WTR-14：详情面板 Response 区域 - Messages Tab（WebSocket 流量）

**前置条件**：生成 WebSocket 流量（需要 WebSocket 服务端支持）

**操作步骤**：
1. 在流量表格中找到 WebSocket 类型的流量记录
2. 点击该记录
3. 在 Response 区域查看 "Messages" Tab

**预期结果**：
- Messages Tab 可见，标签显示消息计数，如 `Messages (5)`
- 消息列表展示发送（Send）和接收（Receive）方向的消息帧
- 每条消息显示帧类型（Text / Binary）、方向标识、内容和时间戳

---

### TC-WTR-15：详情面板 Response 区域 - Script Tab

**前置条件**：请求经过了脚本处理（配置了 req-script 或 res-script 规则）

**操作步骤**：
1. 点击有脚本执行记录的请求
2. 在 Request 区域或 Response 区域查看 "Script" Tab

**预期结果**：
- Script Tab 可见（仅当有脚本执行结果时显示）
- 显示脚本执行日志和结果

---

### TC-WTR-16：Overview 显示 Timing 信息

**操作步骤**：
1. 点击一条已完成的 HTTP 请求记录
2. 在 Request 区域的 "Overview" Tab 查看 Timing 区域

**预期结果**：
- 显示 Timing 条形图，各阶段用不同颜色区分：
  - DNS lookup（紫色）
  - Connection established（绿色）
  - TLS handshake（黄色，仅 HTTPS）
  - Request sent（橙色）
  - Waiting (TTFB)（蓝色）
  - Content download（青色）
- 条形图下方表格列出每个阶段的毫秒数
- 最后一行显示 Total 总耗时

---

### TC-WTR-17：Overview 显示命中规则信息

**前置条件**：配置一条规则（如 `httpbin.org status://201`）并发起匹配请求

**操作步骤**：
1. 通过代理访问匹配规则的 URL
2. 在流量表格中点击该请求记录
3. 在 Request 区域的 "Overview" Tab 查看

**预期结果**：
- Overview 中显示命中的规则信息
- 每条命中规则显示：匹配模式（Pattern）、协议（Protocol）、目标值（Value）
- 如果规则有名称，也一并显示

---

### TC-WTR-18：Body 视图 JSON Pretty Print

**前置条件**：已点击一条返回 JSON 的请求

**操作步骤**：
1. 在 Response 区域点击 "Body" Tab
2. 观察 Body 视图的显示格式下拉菜单

**预期结果**：
- 默认以 JSON 高亮模式显示，JSON 内容格式化缩进
- 格式下拉菜单显示 "JSON"
- JSON 的键、字符串值、数字值、布尔值等分别以不同颜色高亮
- 可切换到 "Tree" 模式，以树形结构展示 JSON 对象

---

### TC-WTR-19：Body 视图 Hex 模式

**操作步骤**：
1. 在 Body Tab 中，点击格式下拉菜单
2. 选择 "Hex"

**预期结果**：
- Body 内容以十六进制视图显示
- 左侧显示偏移地址
- 中间显示十六进制字节值
- 右侧显示对应的 ASCII 可打印字符（不可打印字符显示为 `.`）

---

### TC-WTR-20：流量表格按 Method 筛选

**操作步骤**：
1. 通过 URL 参数访问：`http://127.0.0.1:8800/_bifrost/traffic` 页面
2. 在过滤面板中选择 Method 为 `POST`，或通过 API 验证：
   ```bash
   curl "http://127.0.0.1:8800/_bifrost/api/traffic?method=POST"
   ```

**预期结果**：
- 仅显示 HTTP 方法为 POST 的请求
- 其他方法（GET、PUT、DELETE 等）的请求不显示
- API 返回的 `records` 数组中所有记录的 `m` 字段值为 `POST`

---

### TC-WTR-21：流量表格按状态码筛选

**操作步骤**：
1. 通过 API 验证按状态码筛选：
   ```bash
   curl "http://127.0.0.1:8800/_bifrost/api/traffic?status=404"
   ```
2. 通过状态码范围筛选：
   ```bash
   curl "http://127.0.0.1:8800/_bifrost/api/traffic?status_min=400&status_max=499"
   ```

**预期结果**：
- 精确筛选：仅返回状态码为 404 的请求
- 范围筛选：仅返回状态码在 400-499 之间的请求（如 404）
- 不匹配的状态码不出现在结果中

---

### TC-WTR-22：流量表格按 Host 筛选

**操作步骤**：
1. 通过 API 验证按 Host 筛选：
   ```bash
   curl "http://127.0.0.1:8800/_bifrost/api/traffic?host=httpbin"
   ```

**预期结果**：
- 仅返回 Host 包含 "httpbin" 的请求
- 匹配方式为模糊匹配（LIKE %keyword%）

---

### TC-WTR-23：流量表格按 Content-Type 筛选

**操作步骤**：
1. 通过 API 验证按 Content-Type 筛选：
   ```bash
   curl "http://127.0.0.1:8800/_bifrost/api/traffic?content_type=json"
   ```

**预期结果**：
- 仅返回响应 Content-Type 包含 "json" 的请求
- 如 `application/json` 类型的响应会被匹配

---

### TC-WTR-23B：主筛选器按代理端口筛选

**操作步骤**：
1. 使用隔离数据目录启动 Bifrost，端口为 `$MAIN_PORT`，启动参数必须包含 `--no-system-proxy`。
2. 通过 `$MAIN_PORT` 代理发起请求 `/port-filter-main`，再准备另一条 `listener_port` 不同的 traffic 记录 `/port-filter-other`。
3. 打开 Traffic 页面：
   ```text
   http://127.0.0.1:$MAIN_PORT/_bifrost/traffic
   ```
4. 点击主筛选器 `Add Filter`，在字段下拉中选择 `Port`。
5. 确认操作符自动变为 `Equals`，输入 `$MAIN_PORT`。
6. 使用 API 验证服务端筛选：
   ```bash
   curl "http://127.0.0.1:$MAIN_PORT/_bifrost/api/traffic?listener_port=$MAIN_PORT&limit=50"
   ```
7. 切换到 Fuzzy Search，使用 condition `listener_port equals $MAIN_PORT` 验证搜索筛选。

**预期结果**：
- 主筛选器字段下拉包含 `Port`。
- 选择 `Port` 后，输入框提示为代理端口，操作符默认为 `Equals`。
- 表格只展示 `listener_port=$MAIN_PORT` 的记录，隐藏其他端口记录。
- API 返回 records 中每条记录的 `lp` 都等于 `$MAIN_PORT`。
- Fuzzy Search 使用相同 port condition 时只返回对应端口记录。

---

### TC-WTR-24：右键上下文菜单 - Copy URL

**操作步骤**：
1. 在流量表格中右键点击一条请求记录
2. 在弹出的上下文菜单中点击 "Copy URL"

**预期结果**：
- 显示上下文菜单，包含 "Copy URL" 选项
- 点击后 URL 被复制到剪贴板
- 显示 Toast 消息 "URL copied to clipboard"
- 菜单自动关闭

---

### TC-WTR-25：右键上下文菜单 - Copy as cURL

**操作步骤**：
1. 在流量表格中右键点击一条请求记录
2. 在弹出的上下文菜单中点击 "Copy as cURL"

**预期结果**：
- 点击后生成 cURL 命令并复制到剪贴板
- cURL 命令包含请求方法、URL、请求头和请求体（如有）
- 显示 Toast 消息 "cURL command copied to clipboard"

---

### TC-WTR-26：右键上下文菜单 - Replay

**操作步骤**：
1. 在流量表格中右键点击一条非 CONNECT（非 Tunnel）请求记录
2. 在弹出的上下文菜单中点击 "Replay"

**预期结果**：
- 菜单中显示 "Replay" 选项（仅对非 Tunnel 请求显示）
- 点击后页面跳转到 `/replay` 页面
- Replay 页面自动填充该请求的 URL、方法、请求头等信息

---

### TC-WTR-27：右键上下文菜单 - Download as HAR

**操作步骤**：
1. 在流量表格中右键点击一条请求记录
2. 在弹出的上下文菜单中点击 "Download as HAR"

**预期结果**：
- 显示 loading 提示 "Generating HAR file..."
- 浏览器下载一个 .har 文件
- 显示 Toast 消息 "Downloaded 1 request(s) as HAR"
- HAR 文件内容符合 HAR 1.2 规范

---

### TC-WTR-28：右键上下文菜单 - Export as .bifrost

**操作步骤**：
1. 在流量表格中右键点击一条请求记录
2. 在弹出的上下文菜单中点击 "Export as .bifrost"

**预期结果**：
- 浏览器下载一个 .bifrost 文件
- 文件包含该请求的完整信息

---

### TC-WTR-29：清空所有流量

**操作步骤**：
1. 确认当前流量表格中有流量记录
2. 点击工具栏中的清空按钮（垃圾桶图标）
3. 通过 API 验证：
   ```bash
   curl -X DELETE "http://127.0.0.1:8800/_bifrost/api/traffic"
   ```

**预期结果**：
- 流量表格中的所有记录被清空（活跃连接除外）
- API 返回 "All traffic data cleared successfully"
- 清空后表格显示为空

---

### TC-WTR-30：过滤面板显示与交互

**操作步骤**：
1. 在工具栏中点击过滤按钮打开过滤面板
2. 观察过滤面板的可用选项

**预期结果**：
- 过滤面板显示在流量表格上方或侧边
- 提供以下过滤维度：
  - Method（GET、POST、PUT、DELETE 等）
  - Status（状态码范围或精确值）
  - Protocol（http、https 等）
  - Host（模糊搜索）
  - Content-Type
  - 特殊类型：WebSocket、SSE、H3、Tunnel、Has Rule Hit
  - Client App / Client IP
- 选择过滤条件后，流量表格实时更新，仅显示匹配的记录
- 过滤条件可以组合使用（AND 逻辑）

---

### TC-WTR-31：固定过滤器（Pinned Filters）

**操作步骤**：
1. 在过滤面板中设置一个过滤条件（如 Method = GET）
2. 将该过滤条件固定（Pin）
3. 切换到其他页面后再回到 Traffic 页面

**预期结果**：
- 固定的过滤条件在页面切换后仍然保留
- 流量表格仍按照固定的过滤条件显示
- 工具栏或过滤面板中显示已固定的过滤器标识

---

### TC-WTR-32：虚拟滚动 - 大量记录

**前置条件**：生成大量流量（至少 200 条以上）：
```bash
for i in $(seq 1 200); do
  curl -s -x http://127.0.0.1:8800 http://httpbin.org/get > /dev/null &
done
wait
```

**操作步骤**：
1. 打开 Traffic 页面
2. 快速滚动流量表格到底部
3. 再快速滚动回顶部

**预期结果**：
- 表格采用虚拟滚动，仅渲染可视区域内的行
- 快速滚动时页面不卡顿，滚动流畅
- 滚动到底部后如有更多记录，自动加载更多
- 所有记录按序号有序排列

---

### TC-WTR-33：WebSocket 流量显示帧计数

**前置条件**：生成 WebSocket 流量

**操作步骤**：
1. 在流量表格中找到 WebSocket 类型的记录
2. 观察该记录的显示

**预期结果**：
- WebSocket 记录的 Method 列显示 `GET`（升级请求的方法）
- 流量表格行中显示帧计数信息
- `data-frame-count` 属性包含当前帧数量
- 点击该记录后，Response 区域的 Messages Tab 标签显示帧数，如 `Messages (10)`

---

### TC-WTR-34：SSE 流量显示事件计数

**前置条件**：生成 SSE 流量（需要 SSE 服务端支持，或使用支持 SSE 的 API）

**操作步骤**：
1. 在流量表格中找到 SSE 类型的记录
2. 点击该记录

**预期结果**：
- SSE 记录在表格中正常显示
- 点击后 Response 区域的 Messages Tab 标签显示事件计数，如 `Messages (15)`
- Messages Tab 中按时间顺序展示 SSE 事件
- 每个事件显示 event 类型、data 内容

---

### TC-WTR-35：详情面板搜索功能

**操作步骤**：
1. 点击一条流量记录打开详情面板
2. 在 Request 区域的搜索框中输入关键词（如某个请求头的名称）

**预期结果**：
- 搜索框位于面板内 Tab 区域
- 输入关键词后，当前 Tab 内容中匹配的文本被高亮显示
- 搜索支持在 Overview、Header、Body、Raw 等多个 Tab 中使用

---

### TC-WTR-36：Body 文本选择

**操作步骤**：
1. 点击一条返回 JSON 的请求记录
2. 在 Response 区域的 Body Tab 中，用鼠标拖选一段文本

**预期结果**：
- 文本可以正常选中，选中区域有高亮背景
- 可以通过 Ctrl+C / Cmd+C 复制选中的文本
- 粘贴后内容与选中内容一致

---

### TC-WTR-37：流量表格 Rules 列徽章显示

**前置条件**：已配置规则并生成命中规则的流量

**操作步骤**：
1. 在流量表格中找到命中规则的请求记录
2. 观察 Rules 列

**预期结果**：
- 命中规则的记录在 Rules 列显示蓝色闪电图标
- 图标旁边有蓝色数字徽章，显示命中规则数
- 鼠标悬停闪电图标时，Tooltip 显示 "X rule(s) matched" 和命中的协议列表
- 未命中规则的记录 Rules 列显示 "-"

---

### TC-WTR-38：右键上下文菜单 - 多选批量导出

**操作步骤**：
1. 在流量表格中按住 Ctrl（Mac 为 Cmd）或 Shift 键多选多条记录
2. 右键点击其中一条选中的记录

**预期结果**：
- 上下文菜单仅显示批量操作选项："Export X requests as .bifrost"
- 菜单中不显示单条记录的操作（Copy URL、Copy as cURL、Replay 等）
- 点击导出后，生成包含所有选中记录的 .bifrost 文件

---

### TC-WTR-39：详情面板 Request/Response 区域折叠

**操作步骤**：
1. 点击一条流量记录打开详情面板
2. 点击 Request 区域的折叠按钮
3. 再点击展开按钮恢复

**预期结果**：
- 点击 Request 区域折叠按钮后，Request 区域缩小为标题栏高度
- Response 区域自动占满剩余空间
- 再次点击展开按钮后，Request 区域恢复原始大小
- 同理，Response 区域也可独立折叠和展开
- Request 和 Response 不能同时折叠

---

### TC-WTR-40：右键上下文菜单 - TLS 拦截操作

**前置条件**：流量表格中有 CONNECT（Tunnel）类型的请求

**操作步骤**：
1. 在流量表格中右键点击一条 Tunnel 请求记录

**预期结果**：
- 上下文菜单中显示 TLS 拦截相关选项：
  - "Intercept {域名}" —— 将域名加入 TLS 拦截列表
  - "Intercept {应用名}" —— 将客户端应用加入拦截列表（如有 client_app）
  - "Intercept IP {IP}" —— 将客户端 IP 加入拦截列表（如有 client_ip）
- 对于 Tunnel 请求，不显示 "Replay" 选项
- 如果域名已在拦截列表中，则不显示对应的拦截选项

---

### TC-WTR-41：流量表格自动滚动到底部

**操作步骤**：
1. 打开 Traffic 页面，确保有持续的流量产生
2. 滚动表格到底部
3. 等待新的流量记录产生

**预期结果**：
- 当表格滚动位置在底部时，新记录产生后表格自动滚动以显示最新记录
- 当用户手动向上滚动离开底部后，不再自动滚动
- 有新记录未显示时，页面提示新记录数量，可点击滚动到底部

---

### TC-WTR-42：流量详情中请求序号跳转搜索

**操作步骤**：
1. 点击一条流量记录打开详情面板
2. 在详情面板顶部找到序号区域
3. 点击序号区域触发搜索
4. 输入目标序号（如 "5"）

**预期结果**：
- 显示序号搜索输入框
- 输入数字后，下拉列表显示匹配的请求记录（序号包含输入数字的记录）
- 每个选项显示 `#序号`、Method、Status、Host、Path
- 选中后跳转到对应的请求详情

---

### TC-WTR-43：双击流量记录在新窗口打开

**操作步骤**：
1. 在流量表格中双击一条请求记录

**预期结果**：
- 该请求的详情在新窗口/新标签页中打开
- 新窗口完整显示该请求的详情（Header、Body 等所有 Tab）

---

### TC-WTR-44：流量按协议类型筛选

**操作步骤**：
1. 通过 API 验证按协议筛选：
   ```bash
   curl "http://127.0.0.1:8800/_bifrost/api/traffic?protocol=http"
   ```
2. 验证 WebSocket 筛选：
   ```bash
   curl "http://127.0.0.1:8800/_bifrost/api/traffic?is_websocket=true"
   ```
3. 验证 SSE 筛选：
   ```bash
   curl "http://127.0.0.1:8800/_bifrost/api/traffic?is_sse=true"
   ```
4. 验证 Tunnel 筛选：
   ```bash
   curl "http://127.0.0.1:8800/_bifrost/api/traffic?is_tunnel=true"
   ```

**预期结果**：
- 各筛选条件正确过滤结果
- `protocol=http` 仅返回 HTTP 协议的请求
- `is_websocket=true` 仅返回 WebSocket 类型的请求
- `is_sse=true` 仅返回 SSE 类型的请求
- `is_tunnel=true` 仅返回 CONNECT Tunnel 类型的请求

---

### TC-WTR-45：流量全局搜索功能

**操作步骤**：
1. 在 Traffic 页面工具栏中找到搜索入口
2. 输入搜索关键词（如 "httpbin"）
3. 执行搜索

**预期结果**：
- 搜索引擎在所有流量记录中进行全文检索
- 搜索范围包括 URL、请求头、响应头、请求体、响应体
- 搜索结果高亮匹配的关键词
- 搜索支持通过 SSE 流式返回结果（实时显示匹配进度）

---

### TC-WTR-回归-01：file:// 规则响应的请求在 Traffic 中可见

**背景**：修复 Bug——使用 file:// 规则（如 `a.com file://xxxx`）响应请求时，该请求不在 network traffic 中显示。

**前置条件**：
1. 创建一个临时文件作为 mock 响应：
   ```bash
   echo '{"mock":"from_file"}' > /tmp/bifrost-mock-test.json
   ```
2. 启动 Bifrost 服务并配置 file:// 规则：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- start -p 8800 --unsafe-ssl -r "test.local file:///tmp/bifrost-mock-test.json"
   ```

**操作步骤**：
1. 通过代理发起请求到匹配 file:// 规则的域名：
   ```bash
   curl -x http://127.0.0.1:8800 http://test.local/any-path
   ```
2. 在浏览器中打开 `http://127.0.0.1:8800/_bifrost/traffic`
3. 通过 API 验证 traffic 记录：
   ```bash
   curl "http://127.0.0.1:8800/_bifrost/api/traffic?limit=10"
   ```

**预期结果**：
- curl 请求返回 mock 文件内容 `{"mock":"from_file"}`
- Traffic 页面中可以看到该请求记录
- 请求记录 Host 为 `test.local`，Status 为 `200`
- 请求记录的 Rules 列显示规则命中标识（蓝色闪电图标 + 数字）
- API 返回的 records 数组中包含该请求记录，`h` 字段为 `test.local`，`s` 字段为 `200`，`rc` > 0

---

### TC-WTR-回归-02：redirect:// 规则响应的请求在 Traffic 中可见

**背景**：与 file:// 同属 HTTPS tunnel mock 响应路径，修复前 redirect:// 规则响应也不会被录制。

**操作步骤**：
1. 启动 Bifrost 服务并配置 redirect:// 规则：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- start -p 8800 --unsafe-ssl -r "test.local/old redirect://https://example.com/new"
   ```
2. 通过代理发起请求：
   ```bash
   curl -x http://127.0.0.1:8800 http://test.local/old -v
   ```
3. 通过 API 验证 traffic 记录：
   ```bash
   curl "http://127.0.0.1:8800/_bifrost/api/traffic?limit=10"
   ```

**预期结果**：
- curl 返回 302 重定向响应，`Location` 头为 `https://example.com/new`
- API 返回的 records 中包含该请求记录，`h` 字段为 `test.local`，`s` 字段为 `302`

---

### TC-WTR-回归-03：tpl:// 和 rawfile:// 规则响应的请求在 Traffic 中可见

**操作步骤**：
1. 启动 Bifrost 服务并配置规则：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- start -p 8800 --unsafe-ssl \
     -r "test.local/raw rawfile://(raw-content-test)" \
     -r "test.local/tpl tpl://(Template for {{url}})"
   ```
2. 分别发起请求：
   ```bash
   curl -x http://127.0.0.1:8800 http://test.local/raw
   curl -x http://127.0.0.1:8800 http://test.local/tpl
   ```
3. 通过 API 验证：
   ```bash
   curl "http://127.0.0.1:8800/_bifrost/api/traffic?limit=10"
   ```

**预期结果**：
- 两个请求都返回对应的 mock 内容
- API 返回的 records 中包含两条请求记录，Host 均为 `test.local`
- 两条记录的 Rules 计数均 > 0

---

## 清理

测试完成后清理临时数据：
```bash
rm -rf .bifrost-test
rm -f /tmp/bifrost-mock-test.json
```

## 执行记录

2026-05-08 Traffic 主筛选器端口过滤执行记录：

- 已执行命令：`source ~/.zshrc; pnpm --dir web test:ui traffic.spec.ts -g "主筛选器支持按代理端口过滤 Traffic"`
- 使用隔离数据目录：Playwright UI 全局 setup 自动分配独立 `BIFROST_DATA_DIR` 与独立后端端口，启动 Bifrost 时包含 `--no-system-proxy`。
- 端口要求：UI 测试动态分配后端端口，未使用 `9900`。
- 实际结果：Playwright 本次执行 `1 passed`，Traffic 主筛选器可选择 `Port`，输入临时端口后列表只保留对应入口端口记录。
- 结论：`TC-WTR-23B` 已按文档完成执行并通过，本次无环境阻塞。
