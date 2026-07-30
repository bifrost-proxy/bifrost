# CLI Values 与 Scripts 管理测试用例

## 功能模块说明

本测试用例覆盖 Bifrost CLI 的 `value`（别名 `val`）和 `script` 子命令，用于验证变量管理和脚本管理的完整生命周期，包括增删改查、导入、运行、重命名等操作。

## 前置条件

1. 启动 Bifrost 服务（使用临时数据目录避免污染正式环境）：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- start -p 8800 --unsafe-ssl
   ```
2. 确保 `.bifrost-test` 目录为干净状态（如有残留先执行 `rm -rf .bifrost-test`）
3. 后续所有 CLI 命令均需附带 `BIFROST_DATA_DIR=./.bifrost-test` 环境变量前缀

---

## Values 测试用例

### TC-CVS-01：空状态下列出所有值

**操作步骤**：
1. 执行命令：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- value list
   ```

**预期结果**：
- 输出 `No values defined.`
- 输出 `Values directory:` 后跟 `.bifrost-test/values` 相关路径

---

### TC-CVS-02：添加一个值（value add）

**操作步骤**：
1. 执行命令：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- value add MY_SERVER 127.0.0.1:3000
   ```

**预期结果**：
- 输出 `Value 'MY_SERVER' added successfully.`

---

### TC-CVS-03：使用别名 set 添加值

**操作步骤**：
1. 执行命令：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- value set API_TOKEN abc123xyz
   ```

**预期结果**：
- 输出 `Value 'API_TOKEN' added successfully.`

---

### TC-CVS-04：列出已添加的值（value list）

**前置条件**：已通过 TC-CVS-02 和 TC-CVS-03 添加了 `MY_SERVER` 和 `API_TOKEN`

**操作步骤**：
1. 执行命令：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- value list
   ```

**预期结果**：
- 输出 `Values (2):` 和分隔线 `====================`
- 列表中包含 `MY_SERVER = 127.0.0.1:3000`
- 列表中包含 `API_TOKEN = abc123xyz`
- 输出 `Values directory:` 后跟对应路径

---

### TC-CVS-05：查看指定值（value show）

**前置条件**：已通过 TC-CVS-02 添加了 `MY_SERVER`

**操作步骤**：
1. 执行命令：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- value show MY_SERVER
   ```

**预期结果**：
- 输出 `127.0.0.1:3000`（仅输出值内容，无其他前缀）

---

### TC-CVS-06：使用别名 get 查看值

**前置条件**：已通过 TC-CVS-03 添加了 `API_TOKEN`

**操作步骤**：
1. 执行命令：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- value get API_TOKEN
   ```

**预期结果**：
- 输出 `abc123xyz`

---

### TC-CVS-07：查看不存在的值（value show 错误路径）

**操作步骤**：
1. 执行命令：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- value show NON_EXISTENT
   ```

**预期结果**：
- 命令返回非零退出码
- 错误信息中包含 `Value 'NON_EXISTENT' not found`

---

### TC-CVS-08：更新已有的值（value update）

**前置条件**：已通过 TC-CVS-02 添加了 `MY_SERVER`

**操作步骤**：
1. 执行命令：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- value update MY_SERVER 192.168.1.100:8080
   ```
2. 执行命令验证更新结果：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- value show MY_SERVER
   ```

**预期结果**：
- 步骤 1 输出 `Value 'MY_SERVER' updated successfully.`
- 步骤 2 输出 `192.168.1.100:8080`（新值）

---

### TC-CVS-09：删除一个值（value delete）

**前置条件**：已通过 TC-CVS-03 添加了 `API_TOKEN`

**操作步骤**：
1. 执行命令：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- value delete API_TOKEN
   ```
2. 执行命令验证删除结果：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- value show API_TOKEN
   ```

**预期结果**：
- 步骤 1 输出 `Value 'API_TOKEN' deleted successfully.`
- 步骤 2 返回非零退出码，错误信息中包含 `Value 'API_TOKEN' not found`

---

### TC-CVS-10：删除不存在的值（value delete 错误路径）

**操作步骤**：
1. 执行命令：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- value delete NON_EXISTENT
   ```

**预期结果**：
- 命令返回非零退出码
- 错误信息中包含 `Value 'NON_EXISTENT' not found`

---

### TC-CVS-11：从 JSON 文件导入值（value import）

**操作步骤**：
1. 创建临时导入文件 `/tmp/bifrost-test-values.json`，内容如下：
   ```json
   {
     "HOST_A": "10.0.0.1:80",
     "HOST_B": "10.0.0.2:443",
     "SECRET_KEY": "s3cret"
   }
   ```
2. 执行命令：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- value import /tmp/bifrost-test-values.json
   ```
3. 执行命令验证导入结果：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- value list
   ```

**预期结果**：
- 步骤 2 输出 `Imported 3 value(s) from '/tmp/bifrost-test-values.json'.`
- 步骤 3 列表中包含 `HOST_A = 10.0.0.1:80`、`HOST_B = 10.0.0.2:443`、`SECRET_KEY = s3cret`

---

### TC-CVS-12：从不存在的文件导入值（value import 错误路径）

**操作步骤**：
1. 执行命令：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- value import /tmp/no-such-file.json
   ```

**预期结果**：
- 命令返回非零退出码
- 错误信息中包含 `File not found`

---

### TC-CVS-13：使用别名 val 执行子命令

**操作步骤**：
1. 执行命令：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- val list
   ```

**预期结果**：
- 与 `value list` 行为完全一致，能正常列出已有的值

---

## Scripts 测试用例

### TC-CVS-14：空状态下列出所有脚本（script list）

**操作步骤**：
1. 执行命令：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- script list
   ```

**预期结果**：
- 输出 `No scripts found.`
- 输出 `Scripts directory:` 后跟 `.bifrost-test/scripts` 相关路径

---

### TC-CVS-15：添加 request 类型脚本（script add request --content）

**操作步骤**：
1. 执行命令：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- script add request add-header --content 'function onRequest(context, request) { request.headers["X-Test"] = "hello"; return request; }'
   ```

**预期结果**：
- 输出 `Script 'add-header' (request) saved successfully.`

---

### TC-CVS-16：添加 response 类型脚本（script add response --content）

**操作步骤**：
1. 执行命令：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- script add response modify-status --content 'function onResponse(context, request, response) { response.status = 201; return response; }'
   ```

**预期结果**：
- 输出 `Script 'modify-status' (response) saved successfully.`

---

### TC-CVS-17：列出所有脚本（script list 全类型）

**前置条件**：已通过 TC-CVS-15 和 TC-CVS-16 添加了脚本

**操作步骤**：
1. 执行命令：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- script list
   ```

**预期结果**：
- 输出 `request scripts (1):` 及其下列出 `add-header`
- 输出 `response scripts (1):` 及其下列出 `modify-status`
- 输出 `Scripts directory:` 后跟对应路径

---

### TC-CVS-18：按类型筛选列出脚本（script list -t request）

**前置条件**：已通过 TC-CVS-15 和 TC-CVS-16 添加了脚本

**操作步骤**：
1. 执行命令：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- script list -t request
   ```

**预期结果**：
- 输出 `request scripts (1):` 及其下列出 `add-header`
- 不输出 response 类型脚本
- 输出 `Scripts directory:` 后跟对应路径

---

### TC-CVS-19：查看脚本内容（script show <type> <name>）

**前置条件**：已通过 TC-CVS-15 添加了 request 类型的 `add-header` 脚本

**操作步骤**：
1. 执行命令：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- script show request add-header
   ```

**预期结果**：
- 输出 `Script: add-header (request)`
- 输出 `Content:`
- 输出脚本内容，包含 `function onRequest`

---

### TC-CVS-20：仅用名称模糊查看脚本（script show <name>）

**前置条件**：已通过 TC-CVS-15 添加了 `add-header` 脚本，且该名称在所有类型中唯一

**操作步骤**：
1. 执行命令：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- script show add-header
   ```

**预期结果**：
- 自动匹配到 request 类型的 `add-header`
- 输出 `Script: add-header (request)`
- 输出 `Content:` 和脚本内容

---

### TC-CVS-21：使用别名 get 查看脚本

**前置条件**：已通过 TC-CVS-15 添加了 `add-header` 脚本

**操作步骤**：
1. 执行命令：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- script get request add-header
   ```

**预期结果**：
- 与 `script show request add-header` 输出完全一致

---

### TC-CVS-22：更新已有脚本（script update）

**前置条件**：已通过 TC-CVS-15 添加了 request 类型的 `add-header` 脚本

**操作步骤**：
1. 执行命令：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- script update request add-header --content 'function onRequest(context, request) { request.headers["X-Updated"] = "true"; return request; }'
   ```
2. 执行命令验证更新结果：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- script show request add-header
   ```

**预期结果**：
- 步骤 1 输出 `Script 'add-header' (request) updated successfully.`
- 步骤 2 输出的脚本内容包含 `X-Updated`（新内容），不再包含 `X-Test`（旧内容）

---

### TC-CVS-23：运行脚本测试（script run <type> <name>）

**前置条件**：已通过 TC-CVS-22 更新了 request 类型的 `add-header` 脚本

**操作步骤**：
1. 执行命令：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- script run request add-header
   ```

**预期结果**：
- 输出 `Script: add-header (request)`
- 输出 `Success: true`
- 输出 `Duration: <N> ms`（N 为非负整数）
- 输出 `Output:` 后跟 JSON 格式的请求修改结果
- 输出 `Logs:` 部分

---

### TC-CVS-24：仅用名称运行脚本（script run <name>）

**前置条件**：`add-header` 名称在所有脚本类型中唯一

**操作步骤**：
1. 执行命令：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- script run add-header
   ```

**预期结果**：
- 自动匹配到 request 类型的 `add-header`
- 输出与 TC-CVS-23 一致，包含 `Script: add-header (request)` 和 `Success: true`

---

### TC-CVS-25：重命名脚本（script rename）

**前置条件**：Bifrost 服务正在运行（端口 8800），已添加 request 类型的 `add-header` 脚本

**操作步骤**：
1. 执行命令：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- script rename request add-header inject-header
   ```
2. 执行命令验证重命名结果：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- script show request inject-header
   ```
3. 执行命令验证旧名称已不可用：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- script show request add-header
   ```

**预期结果**：
- 步骤 1 输出 `Script 'request/add-header' renamed to 'inject-header'.`
- 步骤 2 成功显示脚本内容，标题为 `Script: inject-header (request)`
- 步骤 3 返回错误，提示脚本未找到

---

### TC-CVS-26：删除脚本（script delete）

**前置条件**：已通过 TC-CVS-16 添加了 response 类型的 `modify-status` 脚本

**操作步骤**：
1. 执行命令：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- script delete response modify-status
   ```
2. 执行命令验证删除结果：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- script show response modify-status
   ```

**预期结果**：
- 步骤 1 输出 `Script 'modify-status' (response) deleted successfully.`
- 步骤 2 返回错误，提示脚本未找到

---

### TC-CVS-27：通过文件添加脚本（script add --file）

**操作步骤**：
1. 创建临时脚本文件 `/tmp/bifrost-test-decode.js`，内容如下：
   ```javascript
   function onDecode(context, request, response) {
     return { decoded: true, body: response.body };
   }
   ```
2. 执行命令：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- script add decode my-decoder --file /tmp/bifrost-test-decode.js
   ```
3. 执行命令验证：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- script show decode my-decoder
   ```

**预期结果**：
- 步骤 2 输出 `Script 'my-decoder' (decode) saved successfully.`
- 步骤 3 输出 `Script: my-decoder (decode)` 和 `Content:`，脚本内容包含 `function onDecode`

---

### TC-CVS-28：查看不存在的脚本（错误路径）

**操作步骤**：
1. 执行命令：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- script show request non-existent-script
   ```

**预期结果**：
- 命令返回非零退出码
- 错误信息中包含脚本加载失败的提示

---

### TC-CVS-29：按 decode 类型筛选列出脚本

**前置条件**：已通过 TC-CVS-27 添加了 decode 类型的 `my-decoder` 脚本

**操作步骤**：
1. 执行命令：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- script list -t decode
   ```

**预期结果**：
- 输出 `decode scripts (1):` 及其下列出 `my-decoder`
- 不输出 request 或 response 类型脚本

---

### TC-CVS-30：运行 decode 类型脚本

**前置条件**：已通过 TC-CVS-27 添加了 decode 类型的 `my-decoder` 脚本

**操作步骤**：
1. 执行命令：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- script run decode my-decoder
   ```

**预期结果**：
- 输出 `Script: my-decoder (decode)`
- 输出 `Success: true`
- 输出 `Duration: <N> ms`
- 输出 `Output:` 后跟 JSON 格式的解码输出结果

---

### TC-CVS-31：运行中 daemon 的 CLI 写入通过 API 即时同步

**操作步骤**：
1. 在仓库根目录执行专项真实链路脚本：
   ```bash
   BIFROST_BIN="$PWD/target/debug/bifrost" SKIP_BUILD=1 \
     bash e2e-tests/tests/test_cli_resource_api_live_sync.sh
   ```
2. 观察脚本依次验证 Values / Scripts 的 active push、tab 重订阅快照、
   update/delete/rename/import、runtime 异常和离线回退。

**预期结果**：
- 真实隔离 daemon 启动，PID、监听端口和 `/_bifrost/api/system/overview` 三层就绪。
- `value add/update/delete/import` 与 `script add/update/rename/delete` 执行后，
  Admin API 无等待即可读到最新结果。
- 已订阅客户端收到 CLI 写入触发的 `values_update` / `scripts_update`。
- 客户端先关闭订阅、CLI 写入、再开启订阅时立即收到包含新资源的全量快照。
- runtime PID 仍存活但 Admin API 无法验证时，CLI 返回非零并包含
  `refusing direct file writes`，`SHOULD_NOT_WRITE.txt` 不存在。
- daemon 停止后，CLI Values / Scripts 仍可离线写入当前测试数据目录。
- 脚本最终输出 `[cli-resource-live-sync] PASS`。

---

### TC-CVS-32：在线同步无明显常驻资源回归

**操作步骤**：
1. 执行 TC-CVS-31 的专项脚本。
2. 查看脚本输出中的 `RSS before=... after=... delta=...`。
3. 检查本次 diff 不包含新增后台轮询、文件 watcher、timer 或无界 channel。

**预期结果**：
- 完成 CLI 写入、active push 和 tab 重订阅后，daemon RSS 增量不超过
  `32768 KiB`。
- 实现只增加按用户动作触发的 loopback API 请求和 `false -> true` 时的单次快照；
  空闲态没有新增 CPU 工作。

---

## 本次执行记录

| 日期 | 用例 | 结果 | 实际结果 |
| --- | --- | --- | --- |
| 2026-07-30 | TC-CVS-31 | 通过 | 使用当前分支 `target/debug/bifrost` 在动态端口和 `mktemp` 数据目录执行专项脚本；Values / Scripts 的 active push、热订阅快照、CRUD、批量 import、fail-closed 与离线写入全部通过，脚本输出 `PASS`。 |
| 2026-07-30 | TC-CVS-32 | 通过 | 同一真实链路观测 daemon RSS 从 `94064 KiB` 到 `100704 KiB`，增量 `6640 KiB`，低于 `32768 KiB` 门禁；代码检查确认无新增 timer、watcher 或轮询。 |

## 清理

测试完成后清理临时数据和文件：
```bash
rm -rf .bifrost-test
rm -f /tmp/bifrost-test-values.json
rm -f /tmp/bifrost-test-decode.js
```
