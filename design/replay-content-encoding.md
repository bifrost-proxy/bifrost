# Replay 请求正文编码一致性

## 问题与目标

Traffic 持久化会保留请求的 wire bytes 和 `Content-Encoding`，正文读取接口则默认返回解压后的明文。Replay 有两种输入模型：

- `traffic replay` 直接读取历史请求的 wire bytes。
- WebUI Unified Replay 通过 JSON API 传递字符串正文，只能表达明文。

修复目标是保证发送正文与 `Content-Encoding` 始终一致，并保持未修改请求的 wire replay 行为不变。

## 实现逻辑

### Legacy Traffic Replay

无 JSON Patch 时，继续原样发送历史 wire bytes，并保留原始 `Content-Encoding`。

有 JSON Patch 时：

1. 合并所有 `Content-Encoding` header，按逆序严格解压。
2. 使用解压后的明文解析 JSON 并应用 Patch。
3. 按原编码顺序重新压缩。
4. 删除旧 `Content-Length`，由 HTTP client 根据新 wire body 重算。

支持 `identity`、`gzip`/`x-gzip`、`deflate`、`br`、`zstd` 和组合编码。未知编码、损坏数据或解压后超过 10 MiB 安全上限时明确失败，禁止将不一致数据发送到上游。

### Unified Replay

Unified Replay 的请求 DTO 使用字符串表示 body，因此正文语义固定为明文。WebUI 从 Traffic 导入已解压正文时删除原始 `Content-Encoding` 和 `Content-Length`。后端在请求规则和请求脚本执行后再次删除这两个 wire-level header，防止手工输入、规则或脚本重新制造“压缩头 + 明文 body”的错配。

该归一化不改变 `Content-Type` 等实体语义头，由 HTTP client 根据最终正文生成长度。

## 验证方案

- Rust 单元测试覆盖 gzip、组合编码、未知编码、损坏压缩流、无 Patch 原样透传，以及 Unified Replay 明文 header 归一化。
- Web 单元测试覆盖 Traffic 导入时大小写不敏感地删除编码和长度头，并保留正文与其他 header。
- Replay E2E 覆盖真实代理捕获 gzip JSON、CLI Patch 后上游成功解压并收到修改值，以及 WebUI 导入后上游收到明文且不带压缩头。
- `human_tests/traffic-replay.md` 记录并真实执行对应回归场景。
- Rust 生产代码变更执行相关 crate 测试、fmt、clippy、workspace tests 和 `make coverage-changed`。

## Review/Fix/Test

第 1 轮重点检查编码链顺序、严格失败行为、header 大小写、多值 header 和未 Patch 兼容性，并执行最小相关测试。第 2 轮复查最新 diff、文档与真实链路结果，复跑受影响测试并执行最终项目校验。
