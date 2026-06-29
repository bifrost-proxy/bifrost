# Rule Share Query 真实场景测试

## 功能模块说明

Rule Share Query 允许 Web UI 或 CLI 把规则编码到任意 HTTP/HTTPS URL 的 `__bifrost_rule` query 中。Bifrost 代理劫持到该请求后，会先跳到本机确认页；用户点击 Apply Rule 后才导入并启用这条个人规则，禁用其他个人规则，并重定向到移除私有 query 的 clean URL。

导入后的本地规则统一使用 `share/` 命名空间，例如 payload 名称 `cli-share-test` 会落为 `share/cli-share-test`。再次分享这类导入规则时，生成的协议 payload 必须剥掉 `share/` 前缀，并优先使用导入元数据中的原始分享名。

## 前置条件

- 使用独立临时数据目录，避免污染真实配置。
- 启动 Bifrost 时必须设置 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`，并使用 `--no-system-proxy`。
- 使用 `cargo run --bin bifrost -- start -p <PORT> --host 127.0.0.1 --no-system-proxy --no-intercept --intercept-include a.com` 启动测试服务。
- 临时数据目录的 `config.toml` 必须显式设置 `[sync] enabled = false` 和 `auto_sync = false`，避免测试导入同步到真实远端规则。
- 使用一个本地 HTTP fixture 作为目标网站，例如 `python3 -m http.server <TARGET_PORT>`。

## 测试用例列表

### TC-RSQ-01 CLI 生成分享链接

操作步骤：
1. 设置临时 `BIFROST_DATA_DIR`。
2. 执行 `cargo run --bin bifrost -- rule share cli-share-test http://127.0.0.1:<TARGET_PORT>/hello --content "share.test bp://127.0.0.1:3000"`。
3. 执行 `cargo run --bin bifrost -- rule share cli-share-test-bare a.com --content "bare.test bp://127.0.0.1:3000"`。
4. 检查标准输出。

预期结果：
- 输出是一个 HTTP URL。
- URL 保留目标网站地址。
- URL query 中包含 `__bifrost_rule=`。
- 裸域名输入 `a.com` 不报错，输出规范成 `http://a.com/...`。
- 通过 Bifrost 代理打开 `http://a.com/...__bifrost_rule=...` 后，会导入并启用 `share/cli-share-test-bare`，随后重定向到不含私有 query 的 `http://a.com/`。
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
- 裸域名 `a.com` 不返回 400，响应 URL 规范成 `http://a.com/...`。
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

### TC-RSQ-08 HTTPS 浏览器代理导入含规则引用的分享链接

操作步骤：
1. 使用临时数据目录启动 Bifrost 测试服务，启动参数包含 `--intercept-include a.com`。
2. 在临时规则集中创建一条 enabled 的普通个人规则，例如 `d`，内容为 `a.com status://200 resBody://(shadowed)`，确保如果分享 query 未被优先捕获，请求会走普通规则匹配。
3. 生成一条 `https://a.com/?__bifrost_rule=...` 分享链接，payload 内容包含独立的规则引用行，例如 `@a`，以及至少一条有效转发规则。
4. 使用真实 Chromium/Playwright 浏览器配置 HTTP 代理 `127.0.0.1:<PROXY_PORT>`，并开启忽略 HTTPS 证书错误，然后访问该分享链接。
5. 查看浏览器响应、Bifrost 临时数据目录规则列表和导入规则正文。

预期结果：
- 浏览器收到 `302` 响应。
- `Location` 为不含 `__bifrost_rule` 的 `https://a.com/` clean URL，页面 JavaScript 不会看到私有 query。
- 临时规则集中出现 `share/<payload name> [enabled]`。
- 原 enabled 个人规则被禁用，说明导入后的 exclusive scope 生效。
- 导入规则正文保留 `@a` 规则引用行，不因校验失败而拒绝导入。
- 真实用户默认数据目录中不会新增 `share/<payload name>`，也不会修改真实 `a` / `d` 规则。

### TC-RSQ-09 管理端页面防嵌入与分享确认 API

操作步骤：
1. 使用临时数据目录启动 Bifrost 测试服务。
2. 请求普通管理端页面 `http://127.0.0.1:<PROXY_PORT>/_bifrost/`，检查响应头。
3. 生成一条分享链接，目标 URL 指向本地 HTTP fixture。
4. 通过代理访问分享链接，读取 302 `Location` 指向的 `/_bifrost/share/rule?...` 确认页。
5. 对确认页执行 `curl -D -`，检查响应头和 HTML。
6. 直接向 `POST /_bifrost/api/rules/share-confirm` 发送三类请求：跨站请求、同源但缺 CSRF 请求、同源且带 CSRF 但不携带 `confirmation` 字段的请求。
7. 查看规则列表。

预期结果：
- 普通管理端 HTML 页面包含 `X-Frame-Options: DENY` 和包含 `frame-ancestors 'none'` 的 `Content-Security-Policy`。
- 确认页响应包含 `Cache-Control: no-store`、`Referrer-Policy: no-referrer`、`X-Content-Type-Options: nosniff`、`X-Frame-Options: DENY`。
- 确认页响应的 `Content-Security-Policy` 包含 `frame-ancestors 'none'`、`connect-src 'self'`、`base-uri 'none'` 和 `form-action 'none'`。
- HTML 展示完整 content hash 供人工核对，但不展示 hash 输入框；Apply Rule 按钮默认可点击。
- 跨站请求返回 `403`，同源缺 CSRF 返回 `403`。
- 同源、带 CSRF 且不携带 `confirmation` 的请求返回 `200`，导入并启用 `share/<payload name>`。

### TC-RSQ-10 真实浏览器 Apply Rule 不需要填写页面 hash

操作步骤：
1. 使用临时数据目录启动 Bifrost 测试服务，并生成一条目标指向本地 HTTP fixture 的分享链接。
2. 通过 Bifrost 代理访问分享链接，取得 `/_bifrost/share/rule?...` 确认页地址。
3. 使用真实 Chromium/Chrome 打开确认页。
4. 不填写任何页面 hash，直接点击 Apply Rule。
5. 等待页面跳转回 clean target URL，并查看本地规则列表。
6. 在浏览器控制台或自动化执行结果中检查没有 `Failed to fetch` 报错。

预期结果：
- 页面展示规则名、content hash、返回目标和完整规则内容。
- 页面没有 hash 输入框或“Type the full content hash to apply”文案。
- Apply Rule 按钮无需输入即可点击。
- 点击后不出现 `Failed to fetch`；规则导入成功并启用 `share/<payload name>`。
- 浏览器跳转到不含 `__bifrost_rule` 的 clean target URL。

## 清理步骤

- 停止测试 Bifrost 进程。
- 停止本地 HTTP fixture。
- 删除临时 `BIFROST_DATA_DIR`。

## 执行记录

执行时间：2026-06-12

测试环境：
- `BIFROST_DATA_DIR=/tmp/bifrost-rule-share.*`，临时 `config.toml` 已设置 `[sync] enabled = false` / `auto_sync = false`
- Bifrost 测试服务：`127.0.0.1:<随机端口>`
- HTTP fixture：`127.0.0.1:18081`

结果：
- TC-RSQ-01：通过。`bifrost rule share` 输出保留目标 URL 且包含 `__bifrost_rule=`；裸域名 `a.com` 输出规范成 `http://a.com/...`，经代理打开后可导入 `share/rsq-e2e-bare` 并重定向 clean URL。
- TC-RSQ-02：通过。通过代理访问分享链接返回 `302 Found`，`Location` 为不含 `__bifrost_rule` 的 clean URL；规则 `share/rsq-demo` 导入并启用。
- TC-RSQ-03：通过。重复访问同一个分享链接后仍只有一条 `share/rsq-demo`，未创建后缀重复规则。
- TC-RSQ-04：通过。使用同名不同有效内容导入后生成 `share/rsq-demo 2 [enabled]`，原 `share/rsq-demo [disabled]`；重复访问第二条分享链接不会继续创建 `share/rsq-demo 3`。测试中额外确认无效规则内容会被拒绝导入，但仍会重定向到 clean URL。
- TC-RSQ-05：通过。对 `share/rsq-demo 2` 再次生成分享链接后，重新导入仍复用 `share/rsq-demo 2`，不会创建 `share/rsq-demo 2 2`。
- TC-RSQ-06：通过。`POST /_bifrost/api/rules/share-link` 返回 `query_param="__bifrost_rule"`、`payload_version=1` 和可导入 URL；分享 `share/...` 规则时响应 `rule_name` 为原始分享名；裸域名 `a.com` 不返回 400，响应 URL 规范成 `http://a.com/...`。
- TC-RSQ-07：通过。真实浏览器中 Rules 右键菜单显示 Share；弹窗可生成包含 `__bifrost_rule` 的链接；Copy 按钮显示 `Copied`。
- TC-RSQ-08：通过。使用用户提供的精确 `https://a.com/?__bifrost_rule=...` payload，通过真实 Chromium/Playwright 浏览器和隔离 Bifrost 代理访问后，浏览器响应为 `302` 且 `Location=https://a.com/`；临时规则列表为 `share/d [enabled]`、`d [disabled]`、`a [disabled]`；通过 Admin API 读取 `share/d` 正文，正文保留 `@a`，内容 SHA-256 为 `d6409597953c58aa91ca4679ee9c5f8064f09a6c1adca45d17b537118903bf68`；复核真实默认数据目录仍为 `a [disabled]`、`d [enabled]`、`NextOncall双前端本地开发 [enabled]`，未新增 `share/d`。

执行时间：2026-06-24

测试环境：
- `BIFROST_DATA_DIR=/tmp/bifrost-admin-security.*`，临时 `config.toml` 已设置 `[sync] enabled = false` / `auto_sync = false`
- Bifrost 测试服务：`127.0.0.1:<随机端口>`

结果：
- TC-RSQ-09：通过。普通管理端 HTML 页面真实响应包含 `X-Frame-Options: DENY` 和包含 `frame-ancestors 'none'` 的 CSP；确认页真实响应包含 `Cache-Control: no-store`、`Referrer-Policy: no-referrer`、`X-Content-Type-Options: nosniff`、`X-Frame-Options: DENY` 和包含 `frame-ancestors 'none'` / `base-uri 'none'` / `form-action 'none'` 的 CSP；页面要求完整 content hash 后才能 Apply；跨站确认请求返回 `403`，缺 CSRF 返回 `403`，有 CSRF 但空 confirmation 返回 `400`，带完整 hash 返回 `200` 并导入 `share/shared-security [enabled]`。

执行时间：2026-06-29

测试环境：
- `BIFROST_DATA_DIR=/tmp/bifrost-rule-share-*` 与 `/tmp/bifrost-admin-security-*`，临时 `config.toml` 已设置 `[sync] enabled = false` / `auto_sync = false`
- Bifrost 测试服务：`127.0.0.1:<随机端口>`，启动参数包含 `--no-system-proxy`，并设置 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1` / `BIFROST_DISABLE_TRAY=1`
- 真实 Chrome headless：`/Applications/Google Chrome.app/Contents/MacOS/Google Chrome`

执行命令：
- `BIFROST_BIN="$PWD/target/debug/bifrost" bash e2e-tests/tests/test_rule_share_query.sh`
- `BIFROST_BIN="$PWD/target/debug/bifrost" bash e2e-tests/tests/test_rule_share_confirm_browser.sh`
- `BIFROST_BIN="$PWD/target/debug/bifrost" bash e2e-tests/tests/test_admin_cross_site_security.sh`

结果：
- TC-RSQ-09：通过。确认页真实响应包含 `connect-src 'self'`、`frame-ancestors 'none'`、`base-uri 'none'` 和 `form-action 'none'`；HTML 展示 `Content hash` 但不包含 `id="confirmation"` 或 `Type the full content hash to apply`；Apply Rule 按钮默认可点击；跨站确认请求返回 `403`，同源缺 CSRF 返回 `403`，同源带 CSRF 且不携带 `confirmation` 返回 `200` 并导入 `share/shared-security [enabled]`。
- TC-RSQ-10：通过。真实 Chrome 打开 `/_bifrost/share/rule?...` 后不需要填写页面 hash，直接点击 Apply Rule 输出 `browser apply succeeded without hash`；浏览器未捕获 `Failed to fetch`、`Refused to connect` 或 CSP 报错；规则列表出现 `share/rsq-browser [enabled]`，页面跳转到 clean target URL。

执行时间：2026-06-29

测试环境：
- `BIFROST_DATA_DIR=/tmp/bifrost-rule-share-*`，临时 `config.toml` 已设置 `[sync] enabled = false` / `auto_sync = false`
- Bifrost 测试服务：`127.0.0.1:<随机端口>`，启动参数包含 `--no-system-proxy`，并设置 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1` / `BIFROST_DISABLE_TRAY=1`
- 本地 HTTP fixture：`python3 -m http.server <随机端口> --bind 127.0.0.1`

执行命令：
- `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 BIFROST_DISABLE_TRAY=1 BIFROST_BIN="$PWD/target/release/bifrost" bash e2e-tests/tests/test_rule_share_query.sh`
- `NO_PROXY=127.0.0.1 no_proxy=127.0.0.1 BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 BIFROST_DISABLE_TRAY=1 BIFROST_BIN="$PWD/target/release/bifrost" bash e2e-tests/tests/test_rule_share_query.sh`

结果：
- TC-RSQ-01 至 TC-RSQ-06：通过。脚本输出 `rule share confirmation E2E passed`；启动阶段先等待本地 HTTP fixture 和 Bifrost 代理 ready，避免 CI 并发环境下目标站点尚未监听时空日志退出；显式代理访问均设置 `--noproxy ""`，在 `NO_PROXY=127.0.0.1` 环境下仍会经过 Bifrost 代理并得到 `302` 确认页跳转；裸域名、代理导入、重复导入、同名不同内容后缀、reshare 与 share-link API 均通过真实 CLI/API 链路验证。
