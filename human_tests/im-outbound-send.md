# IM 主动外发真实场景测试

## 功能模块说明

验证 `bifrost im send` 通过指定 provider 向 owner、已配置 target 或飞书直达群聊发送有序内容包，并覆盖文本、Markdown、图片、文件和飞书原生卡片。所有自动验证均连接隔离的本地假飞书 OpenAPI；没有用户明确给出的真实 provider 与目的地时，禁止向真实个人或群聊试发。

## 前置条件

- 位于 Bifrost 仓库根目录。
- 已安装 Rust、Python 3 与 `curl`。
- 使用 debug 构建，以便启用仅测试环境可用的飞书 loopback fixture。
- 不配置任何真实飞书或微信凭证。

```bash
SKIP_FRONTEND_BUILD=1 cargo build --bin bifrost
export BIFROST_BIN="$PWD/target/debug/bifrost"
```

## 测试用例列表

### TC-IOS-01：离线 help 与严格参数校验

- **操作步骤**：执行 `"$BIFROST_BIN" im send --help`，再由 `test_im_outbound_send_e2e.sh` 传入未知参数 `--typo`。
- **预期结果**：help 不要求服务已启动或 provider 已配置，输出位置 provider 和所有内容参数；未知参数返回非零并明确报告 `unknown im send option '--typo'`，不会静默忽略。

### TC-IOS-02：运行时 provider 能力发现

- **操作步骤**：执行 `BIFROST_BIN="$BIFROST_BIN" bash e2e-tests/tests/test_im_outbound_send_e2e.sh`，观察脚本对飞书和微信 `provider capabilities --format json` 的断言。
- **预期结果**：飞书声明 file/native_card 为 `native`；微信声明 Markdown 为 `degraded`、file 为 `unsupported`、`requires_context=true`。

### TC-IOS-03：按 provider 名称发送给 owner

- **操作步骤**：由同一 E2E 脚本执行 `bifrost im send feishu-main --text 'owner hello' --idempotency-key owner-e2e --format json`。
- **预期结果**：bundle `status=success`、`destination=owner`；假飞书收到 `open_id=ou_e2e_owner` 的交互式 Markdown 卡片，并携带稳定 UUID。

### TC-IOS-04：通过 target 别名发送原生飞书卡片

- **操作步骤**：由 E2E 脚本创建 `oncall` target，再执行 `--bot-name 'Feishu Main' --target oncall --card-file ...`。
- **预期结果**：机器人名称被服务端精确解析为 `feishu-main`，bundle 发送到 `target:oncall`；假飞书收到 `chat_id=oc_oncall`，原生卡片的 `header.title.content` 保持为 `Target card`，未被兼容渲染器删除。

### TC-IOS-05：直达群聊发送有序 Markdown、图片与文件

- **操作步骤**：由 E2E 脚本执行 `--bot-id cli_e2e --chat-id oc_direct --markdown-file ... --image ... --file ... --format json`，不传 provider 名称。
- **预期结果**：图片和文件先通过 raw binary upload 上传；随后 receipt 顺序严格为 `markdown,image,file`，飞书消息类型严格为 `interactive,image,file`，三项均发往 `oc_direct`。

### TC-IOS-06：逐项 receipt 与幂等 UUID

- **操作步骤**：检查 TC-IOS-03～05 的 JSON 输出和假飞书请求记录。
- **预期结果**：每个内容项均有独立 receipt、message ID 和成功状态；同一 bundle 的每项都有非空且互不相同的稳定 UUID，可安全重试而不把整个多内容包视作一个不可区分请求。

### TC-IOS-07：安装后的全局 Skill 可发现外发能力

- **操作步骤**：由 `test_im_outbound_send_e2e.sh` 使用 `BIFROST_INSTALL_SKILL_SOURCE=embedded bifrost install-skill --tool codex --dir <临时目录> -y` 安装内嵌 Skill，并检查安装产物。
- **预期结果**：安装到临时 Codex 目录的 `SKILL.md` frontmatter 包含飞书/微信外发触发语义，正文包含 capabilities、`--chat-id`、`--bot-id` / `--bot-name`、授权确认与 `partial_success` 处理规则。

### TC-IOS-08：真实外发安全边界

- **操作步骤**：检查本轮测试的 provider base URL、凭证和请求日志；确认所有请求仅访问 `127.0.0.1` 假服务。
- **预期结果**：没有访问真实飞书/微信 OpenAPI，没有向真实 owner、用户或群聊发送消息；测试目录在退出时清理。

### TC-IOS-09：机器人选择器的唯一性与安全返回

- **操作步骤**：执行 Admin 单元测试 `provider_resolve_by_feishu_bot_id_and_name_is_exact_and_unambiguous`，并由 E2E 分别使用 `--bot-name 'Feishu Main'` 和 `--bot-id cli_e2e` 发送。
- **预期结果**：只有 enabled Feishu provider 可命中；ID 与名称组合做交集匹配；名称重名返回 409；解析响应仅包含 provider ID、类型和显示名称，不回传完整 App ID。

### TC-IOS-10：附件文件名拒绝跨平台路径分隔符

- **操作步骤**：由 E2E 脚本向 raw upload API 传入 URL 编码后的 `..\\secret.txt` 文件名和单字节正文。
- **预期结果**：服务端返回 HTTP 400，错误正文包含 `plain file name`；请求不会进入飞书文件上传接口，Unix 与 Windows 风格的路径组件都会被拒绝。

## 清理步骤

E2E 脚本通过 `trap` 自动停止 Bifrost 与假飞书进程并删除临时目录。如脚本被强制终止，定向结束其日志中记录的测试 PID，并只删除仓库根目录下 `.bifrost-e2e-im-send.*` 临时目录。
