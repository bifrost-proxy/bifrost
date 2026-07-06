# Bifrost 能力地图文档真实场景测试

## 功能模块说明

本用例验证 `docs/bifrost-capability-map.md` 能够作为 Bifrost CLI、GUI 与 AI 能力地图的本地交付物，并确认该文档已经图文写入目标飞书文档：

`https://bytedance.larkoffice.com/docx/KqpqdoYcPoaeWKx81UbchJi1nkd`

该测试覆盖本地文档、截图资产、CLI/GUI/AI 事实锚点、飞书写回验证和变更边界，不涉及业务代码行为修改。

## 前置条件

- 仓库位于 `/Users/bytedance/main/codes/bifrost`。
- 本机可执行 `bifrost`，且 `bifrost --version` 返回当前 CLI 版本。
- 本机可执行 `agent-browser`，用于访问 Bifrost Web UI 并生成截图。
- 本机可执行 `lark-cli`，且已通过 user 身份授权目标飞书文档读写。
- 本机可执行 `feishu-cli`，且 user token 可用于 `doc import` 上传图片和导入 Mermaid 图表。
- 本次任务只修改文档与截图资产，不修改 Rust、WebUI、脚本、协议或运行行为。

## 测试用例列表

### TC-BCM-01：本地能力地图文档存在并纳入 docs 索引

操作步骤：

1. 执行 `test -f docs/bifrost-capability-map.md`。
2. 执行 `rg -n "CLI、GUI 与 AI 能力地图" docs/README.md docs/bifrost-capability-map.md`。
3. 执行 `rg -n "## AI 能力详解|## GUI 功能地图|## CLI 功能地图|## 源码证据索引" docs/bifrost-capability-map.md`。

预期结果：

- `docs/bifrost-capability-map.md` 存在。
- `docs/README.md` 包含 `CLI、GUI 与 AI 能力地图` 链接。
- 本地能力地图包含 GUI、CLI、AI 和源码证据索引章节。

### TC-BCM-02：GUI 截图资产存在且被文档引用

操作步骤：

1. 执行 `find docs/assets/bifrost-capability-map -maxdepth 1 -type f -name '*.png' -print | sort`。
2. 执行 `rg -n "assets/bifrost-capability-map" docs/bifrost-capability-map.md`。
3. 打开或预览 `docs/assets/bifrost-capability-map/gui-network.png` 和 `docs/assets/bifrost-capability-map/gui-activity.png`，确认截图来自干净临时实例，不包含真实业务域名、业务规则名或请求详情。

预期结果：

- 存在 7 张截图：Activity、Network、AI General、AI Chat、AI Runners、AI ASR、IM Schedules。
- 文档中引用了这 7 张截图。
- Network 截图为空流量状态，Activity 截图仅展示默认规则和 127.0.0.1:9911，不暴露真实业务数据。

### TC-BCM-03：CLI 能力事实与当前 CLI 帮助一致

操作步骤：

1. 执行 `bifrost --log-dir /tmp/bifrost-codex-log --version`。
2. 执行 `bifrost --log-dir /tmp/bifrost-codex-log --help`。
3. 执行 `bifrost --log-dir /tmp/bifrost-codex-log ai asr --help`。
4. 执行 `bifrost --log-dir /tmp/bifrost-codex-log agent run --help`。
5. 执行 `bifrost --log-dir /tmp/bifrost-codex-log im --help`。
6. 执行 `bifrost --log-dir /tmp/bifrost-codex-log remote file --help`。

预期结果：

- CLI 版本可读。
- 顶层 help 包含 `start`、`rule`、`group`、`traffic`、`search`、`ai`、`agent`、`im`、`remote`、`install-skill` 等命令族。
- AI/Agent/IM/Remote 子命令 help 与本地文档中的能力描述一致。

### TC-BCM-04：GUI 页面截图来自真实 Web UI

操作步骤：

1. 确认临时截图采集实例已按如下安全配置启动过：`BIFROST_DATA_DIR=/private/tmp/bifrost-doc-gui-data`、`127.0.0.1:9911`、`--no-system-proxy`、`--no-tray`。
2. 使用 `agent-browser` 打开 `http://127.0.0.1:9911/_bifrost/`。
3. 依次采集 Activity、Network、AI General、AI Chat、AI Runners、ASR、IM Gateway Schedules 截图。
4. 采集完成后停止临时 9911 实例。

预期结果：

- 截图文件均来自真实 Web UI，而不是手工绘制。
- 截图采集过程不接管系统代理，不污染用户当前 9900 实例。
- 临时 9911 实例最终停止，不残留监听进程。

### TC-BCM-05：飞书目标文档已写入图文内容并可读回

操作步骤：

1. 执行 `lark-cli docs +fetch --api-version v2 --as user --doc "https://bytedance.larkoffice.com/docx/KqpqdoYcPoaeWKx81UbchJi1nkd" --scope outline --max-depth 3 --detail with-ids --format json`。
2. 执行 `lark-cli docs +fetch --api-version v2 --as user --doc "https://bytedance.larkoffice.com/docx/KqpqdoYcPoaeWKx81UbchJi1nkd" --scope keyword --keyword "AI Runners|IM Gateway Schedules|内置 Bifrost Agent runtime|源码证据索引" --context-before 1 --context-after 1 --detail with-ids --format json`。
3. 执行 `feishu-cli doc get KqpqdoYcPoaeWKx81UbchJi1nkd -o json`。

预期结果：

- 目标文档 outline 包含 `Bifrost CLI、GUI 与 AI 能力地图`、`GUI 功能地图`、`CLI 功能地图`、`AI 能力详解`、`源码证据索引` 等章节。
- 关键词读回包含 `IM Gateway Schedules` 图片标签、`内置 Bifrost Agent runtime` 章节、源码证据表格。
- `feishu-cli doc get` 显示 revision 已高于写入前 revision 5。

### TC-BCM-06：变更边界仅限文档、截图与 human_tests

操作步骤：

1. 执行 `git status --short`。
2. 执行 `git diff --stat`。
3. 执行 `git diff -- docs/README.md docs/bifrost-capability-map.md human_tests/readme.md human_tests/bifrost-capability-map.md`。

预期结果：

- 变更范围只包含文档、截图资产和 human_tests 索引/用例。
- 不出现 Rust、WebUI、脚本、配置、协议或运行时代码改动。
- 因为本任务只改文档和截图资产，`cargo test`、自动 E2E、`make coverage` 和 local-ci 可标记为不适用，但最终交付必须说明原因。

## 本次执行记录

执行日期：2026-07-06

| 用例 | 执行结果 | 证据摘要 |
| --- | --- | --- |
| TC-BCM-01 | 通过 | `test -f docs/bifrost-capability-map.md` 成功；`rg` 确认 `docs/README.md` 已链接能力地图，文档包含 GUI、CLI、AI、源码证据索引章节 |
| TC-BCM-02 | 通过 | `find docs/assets/bifrost-capability-map -name '*.png'` 返回 7 张截图；`rg` 确认文档引用 7 张截图；人工预览确认 Network/Activity 截图来自干净临时实例且无真实业务数据 |
| TC-BCM-03 | 通过 | `bifrost --version` 返回 `0.0.141`；顶层 help 和 `ai asr`、`agent run`、`im`、`remote file` help 均与文档中的命令族说明一致 |
| TC-BCM-04 | 通过 | 已用 `agent-browser` 访问临时 `127.0.0.1:9911/_bifrost/` 并覆盖采集 7 张 GUI 截图；临时实例使用 `--no-system-proxy --no-tray`，采集后已 Ctrl-C 停止；`lsof -nP -iTCP:9911 -sTCP:LISTEN` 无监听 |
| TC-BCM-05 | 通过 | `feishu-cli doc import` 成功导入 123 个块、6 个 Mermaid 画板、5 个表格、7 张图片；`lark-cli docs +fetch --scope outline` 读回主要章节；关键词读回包含图片标签、AI runtime 段落和源码证据表；`feishu-cli doc get` 显示 revision `109` |
| TC-BCM-06 | 通过 | `git status --short --branch` 仅显示文档、截图资产和 human_tests 变更；`git ls-files --others` 的新增文件均位于 `docs/` 或 `human_tests/`；未修改 Rust、WebUI、脚本、配置、协议或运行时代码 |

## 清理步骤

- 确认临时 9911 Bifrost 实例已经停止。
- 本次执行已删除任务专用临时目录 `/private/tmp/bifrost-doc-gui-data` 与 `/private/tmp/bifrost-doc-gui-log`。
- 不删除用户当前 `http://127.0.0.1:9900/` 实例的任何数据。
