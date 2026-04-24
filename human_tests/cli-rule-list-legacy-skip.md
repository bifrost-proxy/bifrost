# CLI `rule list` `.bifrost` 文件过滤测试

## 功能模块说明

本文档验证规则解析模块只会扫描 `.bifrost` 结尾的规则文件，像 `.group_cache` 这样的普通文件会被自动忽略；同时确认 group 子目录的存在不会被误当成规则文件，避免目录扫描阶段误伤小组规则存储结构。

## 前置条件

1. 在仓库根目录执行以下命令，确保使用独立临时数据目录：
   ```bash
   rm -rf ./.bifrost-test
   ```
2. 后续命令统一使用：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost --
   ```
   为简化描述，后续以 `bifrost` 代指该完整命令前缀。
3. 确认测试期间不使用 9900 端口；本用例不需要启动代理服务，也不修改系统代理。

## 测试用例

### TC-CRL-01：`rule list` 自动忽略非 `.bifrost` 普通文件

**操作步骤**：
1. 执行 `bifrost rule add valid-local -c "example.com host://127.0.0.1:3000"`
2. 执行以下命令写入一个非规则缓存文件：
   ```bash
   mkdir -p ./.bifrost-test/rules
   cat > ./.bifrost-test/rules/.group_cache <<'EOF'
   {"group_id":"debug2","rules":["cached-only"]}
   EOF
   ```
3. 执行 `bifrost rule list`

**预期结果**：
- 第 1 步输出 `Rule 'valid-local' added successfully.`
- 第 3 步命令执行成功，不输出 `Error:`
- 第 3 步输出包含 `Rules (1):`
- 第 3 步输出包含 `valid-local [enabled]`
- 第 3 步输出不包含 `.group_cache`
- 第 3 步输出中不出现 `Failed to load rule file` 或 `missing field 'name'`

### TC-CRL-02：group 子目录存在时本地规则扫描仍然稳定

**操作步骤**：
1. 执行以下命令创建一个 group 子目录规则文件：
   ```bash
   mkdir -p ./.bifrost-test/rules/demo-group
   cat > ./.bifrost-test/rules/demo-group/group-rule.bifrost <<'EOF'
   01 rules
   [meta]
   name = "group-rule"
   enabled = true
   sort_order = 0
   version = "1.0.0"
   created_at = "2026-04-23T00:00:00Z"
   updated_at = "2026-04-23T00:00:00Z"
   [meta.sync]
   rule_id = "rl_demo_group_rule"
   status = "local_only"
   [options]
   rule_count = 1
   ---
   group.example.com host://127.0.0.1:4100
   EOF
   ```
2. 执行 `bifrost rule list`

**预期结果**：
- 第 2 步命令执行成功，不输出 `Error:`
- 第 2 步输出仍然包含 `valid-local [enabled]`
- 第 2 步输出不包含 `demo-group`
- 第 2 步输出不受根目录 `.group_cache` 文件影响，也不出现目录被当成规则文件后的解析告警

## 清理步骤

测试结束后执行：

```bash
rm -rf ./.bifrost-test
```
