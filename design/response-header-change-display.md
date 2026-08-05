# 响应头变化来源展示

## 背景与目标

Traffic 详情此前以 `Current` / `Original` 对比响应头，并把所有缺失字段统一渲染为红色删除。代理转发时，Bifrost 会按 HTTP 规范自动移除 `Connection` 等 hop-by-hop header；这不是规则、脚本或断点操作，却会被用户理解为 Bifrost 主动篡改了响应。

本改动需要同时满足：

- 明确两个快照的传输方向：`Sent to client` 与 `Upstream original`。
- 把协议转发处理与配置修改分开计数、分开配色。
- `Connection` 及其声明的 hop-by-hop header 使用中性信息色和 `Protocol handling` 标识，不使用危险红色。
- 普通新增、修改、删除继续作为 `Configured changes` 展示，保留现有差异能力。
- network `.bifrost` 导出同时保存上游原始和最终发送响应头，重新导入后仍能解释差异。
- 兼容只含旧 `response_headers` 字段的历史 network 文件。

## 实现逻辑

### WebUI 差异分类

`web/src/components/TrafficDetail/panes/Header/diff.ts` 负责纯数据计算：

1. 按 header 名称（大小写不敏感）和值对比两个快照；仅顺序或名称大小写不同不算变化，重复 header 仍逐项保留。
2. 标记 `added`、`modified`、`deleted`、`unchanged`。
3. 以下名称归为协议处理：`Connection`、`Keep-Alive`、`Proxy-Authenticate`、`Proxy-Authorization`、`Proxy-Connection`、`TE`、`Trailer`、`Transfer-Encoding`、`Upgrade`。
4. 解析上游 `Connection` 的逗号分隔 token；token 指定的扩展 header 同样归为协议处理。
5. 其余差异归为配置修改。

协议处理行使用 Ant Design theme token 的 info 色，保持亮色和暗色主题一致；实际配置删除仍使用 error token 和删除线。提示文案明确说明协议处理不代表规则、脚本或断点发生了修改。

### 导出和导入

`NetworkRecord` 新增可选字段 `original_response_headers`：

- 新导出：`response_headers` 保存发送给客户端的最终快照，`original_response_headers` 保存上游原始快照。
- 新文件导入：恢复两个快照；两者相同则沿用 TrafficRecord 的无差异表示。
- 旧文件导入：当 `original_response_headers` 缺失时，继续把旧 `response_headers` 解释为上游原始快照，避免历史文件出现虚假的变化。

该字段为可选 JSON 字段，不改变 network 文件头版本，旧读取端可忽略新增字段。

## 依赖与边界

- UI 使用现有 Ant Design `Tag`、`Tooltip`、theme token，不新增依赖或硬编码颜色。
- 本次仅解释响应头快照差异，不改变真实 HTTP 转发与 header 清理行为。
- 来源分类只把 HTTP hop-by-hop header 归为协议处理；业务 header 不做猜测。
- request header 复用方向明确的 `Sent upstream` / `Client original` 文案，但不改变其数据来源。

## 测试方案

### 单元测试

- Web：`Connection` 删除、`Connection` token 指定字段、普通配置差异、最终空 header 集合。
- Rust：新 network 文件保存两个响应头快照；新格式导入恢复差异；旧格式缺少新增字段时继续按旧语义导入。

### E2E 测试

- 导入无规则的确定性双快照记录：`Connection` 显示 `Protocol handling`，`Configured changes` 为 0。
- 带 `resHeaders` 规则的请求：配置变化计数增加，仍与协议处理分开。
- 验证 `Sent to client` / `Upstream original` 页签文案。
- 亮色和暗色主题下信息色、危险色及文字均可辨识。

### 真实场景测试

创建 `human_tests/response-header-change-display.md`，覆盖无规则协议处理、规则修改、network 文件导出导入、亮暗主题，并在文档创建后立即逐条执行。

## Review/Fix/Test 闭环

### 第 1 轮

- 对照用户目标检查是否仍有“删除即用户操作”的文案或红色语义。
- 检查重复 header、空最终集合、动态 `Connection` token 和旧文件兼容。
- 执行相关 Web/Rust 单元测试和 Traffic UI E2E；发现问题立即修复并复跑。

### 第 2 轮

- 复查最新 `git status` / `git diff`，确认设计、实现、E2E、human_tests 与索引一致。
- 检查亮暗主题、导出后重导入，以及请求头详情无回归。
- 复跑受影响测试；若仍有用户可感知问题，继续追加闭环。

## 校验要求

- 先按 `.agents/skills/e2e-test/` 执行目标 E2E。
- 再按 `.agents/skills/rust-project-validate/` 执行格式、Clippy、构建与测试校验。
- 执行 `cargo test --workspace --all-features`。
- 本地不运行全量覆盖率；由远端 CI 的 `bash scripts/ci/coverage-all.sh --json --gate` 执行 90% 门禁。
