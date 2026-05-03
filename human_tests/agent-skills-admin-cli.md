# Agent Skills Admin and CLI 真实场景测试用例

## 功能模块说明

覆盖 Agent Skills 管理端导入接口与 IM CLI secret 解析的 review 修复：导入接口不再接收客户端本机 PathBuf，错误码按错误类别分层，CLI secret 缺失不再静默变成空串。

## 前置条件

```bash
cd <REPO_ROOT>
export CARGO_TARGET_DIR=./.codex-target/fixpass
```

## 测试用例列表

### TC-ASAC-01: multipart package 字段解析回归

- **操作步骤**:
  ```bash
  CARGO_TARGET_DIR=./.codex-target/fixpass cargo test -p bifrost-admin multipart_import_extracts_package_field_bytes --quiet
  ```
- **预期结果**: 测试通过；multipart/form-data 中名为 `package` 的字段能提取原始 zip bytes。
- **本次执行结果**: 2026-05-03 通过，结果 `1 passed, 0 failed`。

### TC-ASAC-02: AgentSkillError 冲突错误码回归

- **操作步骤**:
  ```bash
  CARGO_TARGET_DIR=./.codex-target/fixpass cargo test -p bifrost-admin agent_skill_error_maps_conflict_to_409 --quiet
  ```
- **预期结果**: 测试通过；冲突错误映射为 HTTP 409。
- **本次执行结果**: 2026-05-03 通过，结果 `1 passed, 0 failed`。

### TC-ASAC-03: IM CLI secret 缺失错误回归

- **操作步骤**:
  ```bash
  CARGO_TARGET_DIR=./.codex-target/fixpass cargo test -p bifrost-cli resolve_secret_missing --quiet
  ```
- **预期结果**: 测试通过；缺失 `env:` 返回 `ResolveSecretError::Missing`，缺失 `file:` 返回 `ResolveSecretError::Io`，不再返回空字符串。
- **本次执行结果**: 2026-05-03 通过，lib/bin 目标共执行匹配测试，结果均为 `2 passed, 0 failed`。

## 清理步骤

测试使用临时路径或纯内存 bytes，无需手动清理。
