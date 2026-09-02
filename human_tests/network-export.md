# Network 导出生效规则快照

## 功能模块说明

验证 WebUI Traffic/Network 右键导出 `.bifrost` 请求文件时，导出内容不仅包含选中的请求，还包含该请求进入 Bifrost 时对应端口正在生效的规则快照。默认代理端口和自定义临时端口必须明确区分，避免用户反馈问题时排查方拿到错误的规则上下文。压缩或二进制 Body 必须可逆保存，预览应展示可解码的明文，不能用 UTF-8 lossy 字符串破坏原始字节。

## 前置条件

```bash
source ~/.zshrc
cd /Users/eden/work/github/bifrost-network-empty-bifrost-export
```

执行真实场景时必须使用临时数据目录，并且启动 Bifrost 时必须带 `--no-system-proxy`，避免影响本机系统代理。

## 测试用例列表

### TC-NE-01：默认端口 Network 导出包含默认端口生效规则快照

**操作步骤**：
1. 启动真实 Bifrost 服务，使用临时 `BIFROST_DATA_DIR` 和随机主端口。
2. 新增并启用默认规则 `main-default`，规则内容包含 `main-only.test status://209`。
3. 通过默认代理端口请求 `http://main-only.test/main-port`，等待 Traffic DB 生成记录。
4. 调用 `POST /_bifrost/api/bifrost-file/export/network`，请求体包含该 Traffic record id。
5. 解析导出的 `.bifrost` 文件中 `---` 后面的 JSON 内容。

**预期结果**：
- 导出的 record 包含 `listener_port`，值为默认主端口。
- `active_rules.source` 为 `default_port`。
- `active_rules.listener_port` 等于默认主端口。
- `active_rules.merged_content` 和 `active_rules.rules[].content` 包含 `main-only.test status://209`。
- 导出内容不包含自定义端口规则 `temp-only.test status://210`。

### TC-NE-02：自定义端口 Network 导出包含该自定义端口绑定规则快照

**操作步骤**：
1. 在同一真实 Bifrost 服务中新增临时端口，并绑定规则 `temp-bound`。
2. 通过临时端口请求 `http://temp-only.test/temp-port`，等待 Traffic DB 生成记录。
3. 调用 `POST /_bifrost/api/bifrost-file/export/network`，请求体包含该临时端口 Traffic record id。
4. 解析导出的 `.bifrost` 文件中 `---` 后面的 JSON 内容。

**预期结果**：
- 导出的 record 包含 `listener_port`，值为临时端口。
- `active_rules.source` 为 `custom_port`。
- `active_rules.listener_port` 等于临时端口。
- `active_rules.merged_content` 和 `active_rules.rules[].content` 包含 `temp-only.test status://210`。
- 导出内容不包含默认端口规则 `main-only.test status://209`。

### TC-NE-03：Network 空选择导出仍被前后端拦截

**操作步骤**：
1. 调用前端导出 helper 或后端 `POST /_bifrost/api/bifrost-file/export/network`，传入空的 `record_ids`。

**预期结果**：
- 前端 helper 返回 `Select at least one Network record before exporting a .bifrost file`。
- 后端返回 400，错误信息同样提示必须至少选择一条 Network record。
- 不生成空的 Network `.bifrost` 包。

### TC-NE-04：旧 Network 导出文件仍可解析导入

**操作步骤**：
1. 使用缺少 `listener_port` 和 `active_rules` 字段的旧版 Network `.bifrost` 文件。
2. 调用 Network import 或 parser 解析该文件。

**预期结果**：
- 旧文件解析不失败。
- 缺失的 `listener_port` 在导入恢复为 Traffic record 时按默认值 `0` 处理。
- 缺失的 `active_rules` 按 `None` 处理，不影响旧文件导入。

### TC-NE-05：标准 HTTP 压缩 Body 在请求和响应侧均展示明文

**操作步骤**：
1. 启动真实 Bifrost 服务，使用临时 `BIFROST_DATA_DIR`、随机主端口和 `--no-system-proxy`。
2. 通过代理发送 `Content-Type: application/json`、`Content-Encoding: gzip, deflate` 的双层压缩 POST 请求，并让上游响应返回相同编码链。
3. 等待 Traffic DB 生成记录，调用 Network 导出接口并解析 `.bifrost`。
4. 调用 Network 预览接口查看同一个导出文件。

**预期结果**：
- `request_body` 和 `response_body` 都是解压后的 JSON 明文，不包含替换字符 `�`。
- `Content-Encoding` 中多个编码（包括重复 header 字段）按应用顺序的逆序解码；内置支持 `gzip`（含 `x-gzip` 兼容别名）、`deflate`、`br`、`zstd` 和 `identity`。
- 对仍持有原始压缩引用的流式记录，`request_body_base64` 可解码为原始双层压缩字节，再解压后与 JSON 明文完全一致。
- 单条记录预览的 Request/Response Body 面板均展示 JSON 明文，并恢复两侧 `application/json` 内容类型。
- 遇到未知或自定义编码时不做部分解码，保留完整原始字节交给自定义 decoder。
- 对旧版本已经 lossy 导出的压缩 Body，预览隐藏乱码并提示需要使用新版本重新导出。

## 执行记录

- 2026-05-20：已执行 `cargo test -p bifrost-admin network_export -- --nocapture`，6 个后端导出用例通过，覆盖空选择拦截、默认端口快照、默认规则目录缺失空快照和自定义端口快照。
- 2026-05-20：已执行 `cargo test -p bifrost-core parse_network_accepts_legacy_record_without_active_rules -- --nocapture`，旧 Network record 缺少 `listener_port` / `active_rules` 时解析通过。
- 2026-05-20：已执行 `cd web && pnpm vitest run src/api/bifrost-file.test.ts`，5 个前端 import/export helper 用例通过，覆盖空 Network 导出提示。
- 2026-05-20：已执行 `e2e-tests/tests/test_temporary_port_bindings.sh`，55/55 通过；其中新增断言确认默认端口导出包含 `default_port` 生效规则快照，自定义端口导出包含 `custom_port` 生效规则快照，且两者不互相混入。
- 2026-09-02：已执行 `cargo test -p bifrost-admin handlers::bifrost_file::tests:: -- --nocapture`（22/22 通过）及 `cargo test -p bifrost-admin handlers::network_body::tests:: -- --nocapture`（6/6 通过），覆盖所有内置 HTTP 压缩算法、双层编码、未知编码透传、原始字节可恢复、新格式预览解压、旧 lossy 文件警告和旧格式兼容。
- 2026-09-02：已执行 `cargo test -p bifrost-proxy transform::decompress::tests::test_ -- --nocapture`（7/7 通过），覆盖完整 Content-Encoding 链逆序解码和自定义编码原样保留。
- 2026-09-02：已执行 `e2e-tests/tests/test_temporary_port_bindings.sh`，62/62 通过；真实 `gzip, deflate` 双层压缩 POST 及响应经代理录制、Network 导出和预览后，两侧明文 JSON、内容类型和原始请求字节断言通过。
- 2026-09-02：已按 TC-NE-05 独立人工执行 release 二进制，使用临时数据目录、动态端口 `62396/62397`、`--no-system-proxy` 及禁用托盘/登录提示环境变量；Traffic 请求/响应、Network 导出及预览的双层压缩明文断言通过，`x-company-codec` 请求/响应二进制字节保持不变，服务按精确 PID 清理。

## 清理步骤

```bash
rm -rf ./.bifrost-e2e-network-export-* /tmp/bifrost-network-export-*
```
