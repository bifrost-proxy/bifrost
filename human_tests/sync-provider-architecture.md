# Sync Provider Architecture 真实场景测试

## 功能模块说明

本用例用于审查 `design/sync-provider-architecture.md` 是否完整覆盖 Sync Provider 方案。当前阶段是设计文档落地, 不是功能实现; 因此本用例通过静态审查命令和人工检查清单验证方案是否可继续评审和拆解。

## 前置条件

1. 当前工作区位于 Bifrost 仓库根目录。
2. 已存在 `design/sync-provider-architecture.md`。
3. 不启动 Bifrost 服务, 不修改系统代理, 不连接真实远端同步服务。

## 测试用例列表

### TC-SPA-01: 方案覆盖 provider/capability/lane 基础模型

**操作步骤**:

1. 执行:
   ```bash
   rg -n "Provider Type|Provider Instance|Capability|Sync Lane|Capability Matrix" design/sync-provider-architecture.md
   ```
2. 阅读匹配段落。

**预期结果**:

- 文档明确区分 Provider Type 与 Provider Instance。
- 文档包含 capability 能力矩阵。
- 文档包含 `personal_rules`, `group_rules`, `remote_invoke` lane。
- 文档说明多个 provider instance 可以同时 enabled。

### TC-SPA-02: GitHub Gist 边界清晰

**操作步骤**:

1. 执行:
   ```bash
   rg -n "GitHub Gist|Rules only|encrypted|Group rules|Remote Invoke|secret gist" design/sync-provider-architecture.md
   ```
2. 阅读 GitHub Gist provider 章节和 capability matrix。

**预期结果**:

- GitHub Gist 只支持个人规则配置同步。
- GitHub Gist 不支持小组规则。
- GitHub Gist 不支持 Remote Invoke relay。
- 文档要求 GitHub Gist payload 默认客户端加密。
- 文档说明 secret gist 本身不是安全边界。

### TC-SPA-03: ByteDance 内网 provider 自动检测与不覆盖用户选择

**操作步骤**:

1. 执行:
   ```bash
   rg -n "ByteDance|auto|detect|bifrost.bytedance.net|do not override|managed" design/sync-provider-architecture.md
   ```
2. 阅读 ByteDance Internal Provider 章节和 Product Defaults。

**预期结果**:

- 文档指定探测 `https://bifrost.bytedance.net/v4/sso/check`。
- 文档指定 200/401 算可达, DNS/TLS/timeout/5xx 算不可用。
- 文档说明仅当用户尚未选择 provider 时自动推荐或创建 managed provider。
- 文档明确已选 GitHub 或其他 provider 时不能自动覆盖。

### TC-SPA-04: CLI/UI/API 具备可扩展操作面

**操作步骤**:

1. 执行:
   ```bash
   rg -n "Admin API|bifrost sync provider|bifrost sync lane|Provider Catalog|Capability Routing|Implementation Phases" design/sync-provider-architecture.md
   ```
2. 阅读 Admin API, CLI, Web UI, Implementation Phases 章节。

**预期结果**:

- 文档包含 provider catalog/list/add/update/delete/auth/logout API。
- 文档包含 CLI provider 子命令和 lane 子命令。
- 文档包含 UI 的 Sync Targets, Capability Routing, Provider Catalog。
- 文档包含分阶段落地计划。

## 清理步骤

本用例只读文档, 无需清理临时服务或数据目录。

## 执行记录

- 2026-07-06: PASS - 在 `codex/sync-provider-design` worktree 中执行 TC-SPA-01 至 TC-SPA-04 的 `rg` 静态审查命令, 均能命中对应章节; 人工复核确认设计覆盖多 provider 同启、capability 区分、GitHub Gist rules-only、ByteDance 自动检测、UI/CLI/API 和分阶段落地。
