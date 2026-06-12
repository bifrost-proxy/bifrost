# Rule Share Query 真实场景测试

## 功能模块说明

Rule Share Query 允许 Web UI 或 CLI 把规则编码到任意 HTTP/HTTPS URL 的 `__bifrost_rule` query 中。Bifrost 代理劫持到该请求后，会导入并启用这条个人规则，禁用其他个人规则，并在 `GET` / `HEAD` 请求上重定向到移除私有 query 的 clean URL。

导入后的本地规则统一使用 `share/` 命名空间，例如 payload 名称 `cli-share-test` 会落为 `share/cli-share-test`。再次分享这类导入规则时，生成的协议 payload 必须剥掉 `share/` 前缀，并优先使用导入元数据中的原始分享名。

## 前置条件

- 使用独立临时数据目录，避免污染真实配置。
- 启动 Bifrost 时必须设置 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`，并使用 `--no-system-proxy`。
- 使用 `cargo run --bin bifrost -- start -p <PORT> --host 127.0.0.1 --no-system-proxy --no-intercept` 启动测试服务。
- 使用一个本地 HTTP fixture 作为目标网站，例如 `python3 -m http.server <TARGET_PORT>`。

## 测试用例列表

### TC-RSQ-01 CLI 生成分享链接

操作步骤：
1. 设置临时 `BIFROST_DATA_DIR`。
2. 执行 `cargo run --bin bifrost -- rule share cli-share-test http://127.0.0.1:<TARGET_PORT>/hello --content "share.test bp://127.0.0.1:3000"`。
3. 执行 `cargo run --bin bifrost -- rule share cli-share-test-bare a.com --content "bare.test direct"`。
4. 检查标准输出。

预期结果：
- 输出是一个 HTTP URL。
- URL 保留目标网站地址。
- URL query 中包含 `__bifrost_rule=`。
- 裸域名输入 `a.com` 不报错，输出规范成 `https://a.com/...`。
- 命令不需要运行中的 Bifrost 服务。

### TC-RSQ-02 代理 GET 导入并重定向 clean URL

操作步骤：
1. 使用临时数据目录启动 Bifrost 测试服务。
2. 生成一条分享链接，目标 URL 指向本地 HTTP fixture。
3. 通过 `curl -x http://127.0.0.1:<PROXY_PORT> -I <分享链接>` 发起请求。
4. 查看响应头和本地规则列表。

预期结果：
- HTTP 响应为 `302`。
- `Location` 指向不含 `__bifrost_rule` 的 clean URL。
- 规则列表中出现 `share/<分享 payload 规则名>`。
- 该规则处于 enabled 状态。

### TC-RSQ-03 同名同内容重复访问复用已有规则

操作步骤：
1. 在 TC-RSQ-02 的数据目录中再次访问同一个分享链接。
2. 执行 `cargo run --bin bifrost -- rule list`。

预期结果：
- 规则列表中只有一条对应规则。
- 没有创建 `规则名 2` 之类的重复规则。

### TC-RSQ-04 同名不同内容创建后缀规则并禁用旧个人规则

操作步骤：
1. 生成第二条分享链接，规则名称与 TC-RSQ-02 相同，但规则内容不同。
2. 通过代理访问第二条分享链接。
3. 再次通过代理访问第二条分享链接。
4. 执行 `cargo run --bin bifrost -- rule list`。

预期结果：
- 规则列表中同时存在 `share/规则名` 和 `share/规则名 2`。
- 原规则处于 disabled 状态。
- `share/规则名 2` 处于 enabled 状态。
- 再次访问第二条分享链接后仍只有 `share/规则名` 和 `share/规则名 2`，不会创建 `share/规则名 3`。

### TC-RSQ-05 已导入 share 规则再次分享时剥掉命名空间

操作步骤：
1. 在 TC-RSQ-04 后，对 `share/规则名 2` 执行 `cargo run --bin bifrost -- rule share "share/规则名 2" http://127.0.0.1:<TARGET_PORT>/hello`。
2. 通过代理访问生成的新分享链接。
3. 执行 `cargo run --bin bifrost -- rule list`。

预期结果：
- CLI 能成功生成分享链接，说明 payload 名称没有携带协议不允许的 `/`。
- 再次访问该链接后规则列表仍只有 `share/规则名` 和 `share/规则名 2`。
- `share/规则名 2` 仍处于 enabled 状态，不会创建 `share/规则名 2 2`。

### TC-RSQ-06 Admin API 创建分享链接

操作步骤：
1. 在临时数据目录中已有一条个人规则或已导入的 `share/...` 规则。
2. 向 `http://127.0.0.1:<PROXY_PORT>/_bifrost/api/rules/share-link` 发送 `POST` JSON：`{"name":"<规则名>","target_url":"http://127.0.0.1:<TARGET_PORT>/from-api"}`。
3. 再发送 `POST` JSON：`{"name":"<规则名>","target_url":"a.com"}`。
4. 检查响应 JSON。

预期结果：
- 响应包含 `url`、`query_param`、`payload_version`、`rule_name` 和 `content_hash`。
- `query_param` 为 `__bifrost_rule`。
- 如果请求分享的是 `share/...` 规则，响应 `rule_name` 为原始分享名，不包含 `share/` 前缀。
- 裸域名 `a.com` 不返回 400，响应 URL 规范成 `https://a.com/...`。
- `url` 可被代理导入。

### TC-RSQ-07 Web UI 分享入口

操作步骤：
1. 打开 Rules 页面。
2. 在个人规则名称上右键。
3. 点击 Share。
4. 输入目标 URL 并点击 Create Link。
5. 点击 Copy。

预期结果：
- 右键菜单显示 Share。
- 弹窗生成的链接包含 `__bifrost_rule`。
- Copy 操作成功提示。

## 清理步骤

- 停止测试 Bifrost 进程。
- 停止本地 HTTP fixture。
- 删除临时 `BIFROST_DATA_DIR`。

## 执行记录

执行时间：2026-06-12

测试环境：
- `BIFROST_DATA_DIR=/tmp/bifrost-rule-share.Xlfaov`
- Bifrost 测试服务：`127.0.0.1:18080`
- HTTP fixture：`127.0.0.1:18081`

结果：
- TC-RSQ-01：通过。`bifrost rule share` 输出保留目标 URL 且包含 `__bifrost_rule=`；裸域名 `a.com` 输出规范成 `https://a.com/...`。
- TC-RSQ-02：通过。通过代理访问分享链接返回 `302 Found`，`Location` 为不含 `__bifrost_rule` 的 clean URL；规则 `share/rsq-demo` 导入并启用。
- TC-RSQ-03：通过。重复访问同一个分享链接后仍只有一条 `share/rsq-demo`，未创建后缀重复规则。
- TC-RSQ-04：通过。使用同名不同有效内容导入后生成 `share/rsq-demo 2 [enabled]`，原 `share/rsq-demo [disabled]`；重复访问第二条分享链接不会继续创建 `share/rsq-demo 3`。测试中额外确认无效规则内容会被拒绝导入，但仍会重定向到 clean URL。
- TC-RSQ-05：通过。对 `share/rsq-demo 2` 再次生成分享链接后，重新导入仍复用 `share/rsq-demo 2`，不会创建 `share/rsq-demo 2 2`。
- TC-RSQ-06：通过。`POST /_bifrost/api/rules/share-link` 返回 `query_param="__bifrost_rule"`、`payload_version=1` 和可导入 URL；分享 `share/...` 规则时响应 `rule_name` 为原始分享名；裸域名 `a.com` 不返回 400，响应 URL 规范成 `https://a.com/...`。
- TC-RSQ-07：通过。真实浏览器中 Rules 右键菜单显示 Share；弹窗可生成包含 `__bifrost_rule` 的链接；Copy 按钮显示 `Copied`。
