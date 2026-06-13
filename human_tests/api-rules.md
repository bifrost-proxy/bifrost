# Rules 管理 API 测试用例

## 功能模块说明

验证 Bifrost Rules 管理 API 的完整功能，包括规则文件的增删改查、启用/禁用、列表查询等操作。所有 API 均以 `http://127.0.0.1:8800/_bifrost/api/rules` 为基础路径。

## 前置条件

1. 启动 Bifrost 服务（使用临时数据目录避免污染正式环境）：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- start -p 8800 --unsafe-ssl
   ```
2. 确保端口 8800 可用且服务已正常启动
3. 确保当前无已有规则（可先调用 DELETE 清理）

---

## 测试用例

### TC-ARU-01：获取规则列表（空列表）

**操作步骤**：
1. 执行以下命令：
   ```bash
   curl -s http://127.0.0.1:8800/_bifrost/api/rules | jq .
   ```

**预期结果**：
- HTTP 状态码 200
- 返回 JSON 数组 `[]`（空数组）

---

### TC-ARU-02：创建规则（包含 name/content/enabled）

**操作步骤**：
1. 执行以下命令：
   ```bash
   curl -s -X POST http://127.0.0.1:8800/_bifrost/api/rules \
     -H "Content-Type: application/json" \
     -d '{"name": "test-rule-1", "content": "example.com 127.0.0.1:3000", "enabled": true}' | jq .
   ```

**预期结果**：
- HTTP 状态码 200
- 返回 JSON 包含 `"message"` 字段，内容为 `"Rule 'test-rule-1' created successfully"`

---

### TC-ARU-03：获取规则列表（含已创建规则）

**前置条件**：已通过 TC-ARU-02 创建规则

**操作步骤**：
1. 执行以下命令：
   ```bash
   curl -s http://127.0.0.1:8800/_bifrost/api/rules | jq .
   ```

**预期结果**：
- HTTP 状态码 200
- 返回 JSON 数组，长度为 1
- 数组元素包含以下字段：
  - `name`: `"test-rule-1"`
  - `enabled`: `true`
  - `rule_count`: 大于 0 的整数（表示该规则文件解析出的规则条数）
  - `sort_order`: 整数
  - `created_at`: 非空字符串（ISO 时间格式）
  - `updated_at`: 非空字符串（ISO 时间格式）

---

### TC-ARU-04：获取规则详情

**前置条件**：已通过 TC-ARU-02 创建规则

**操作步骤**：
1. 执行以下命令：
   ```bash
   curl -s http://127.0.0.1:8800/_bifrost/api/rules/test-rule-1 | jq .
   ```

**预期结果**：
- HTTP 状态码 200
- 返回 JSON 对象包含：
  - `name`: `"test-rule-1"`
  - `content`: `"example.com 127.0.0.1:3000"`
  - `enabled`: `true`
  - `sort_order`: 整数
  - `created_at`: 非空字符串
  - `updated_at`: 非空字符串
  - `sync`: 对象，包含 `status` 字段（值为 `"local_only"`）

---

### TC-ARU-05：更新规则内容（PUT 更新 content）

**前置条件**：已通过 TC-ARU-02 创建规则

**操作步骤**：
1. 执行以下命令：
   ```bash
   curl -s -X PUT http://127.0.0.1:8800/_bifrost/api/rules/test-rule-1 \
     -H "Content-Type: application/json" \
     -d '{"content": "example.com 127.0.0.1:4000"}' | jq .
   ```

**预期结果**：
- HTTP 状态码 200
- 返回 JSON 包含 `"message"` 字段，内容为 `"Rule 'test-rule-1' updated successfully"`

**验证步骤**：
1. 再次获取规则详情：
   ```bash
   curl -s http://127.0.0.1:8800/_bifrost/api/rules/test-rule-1 | jq .content
   ```
2. 确认 `content` 已更新为 `"example.com 127.0.0.1:4000"`

---

### TC-ARU-06：更新规则启用状态（PUT 更新 enabled）

**前置条件**：已通过 TC-ARU-02 创建规则

**操作步骤**：
1. 执行以下命令：
   ```bash
   curl -s -X PUT http://127.0.0.1:8800/_bifrost/api/rules/test-rule-1 \
     -H "Content-Type: application/json" \
     -d '{"enabled": false}' | jq .
   ```

**预期结果**：
- HTTP 状态码 200
- 返回 JSON 包含 `"message"` 字段，内容为 `"Rule 'test-rule-1' updated successfully"`

**验证步骤**：
1. 再次获取规则详情：
   ```bash
   curl -s http://127.0.0.1:8800/_bifrost/api/rules/test-rule-1 | jq .enabled
   ```
2. 确认 `enabled` 为 `false`

---

### TC-ARU-07：通过专用接口启用规则

**前置条件**：规则 `test-rule-1` 当前为禁用状态（TC-ARU-06 已禁用）

**操作步骤**：
1. 执行以下命令：
   ```bash
   curl -s -X PUT http://127.0.0.1:8800/_bifrost/api/rules/test-rule-1/enable | jq .
   ```

**预期结果**：
- HTTP 状态码 200
- 返回 JSON 包含 `"message"` 字段，内容为 `"Rule 'test-rule-1' enabled successfully"`

**验证步骤**：
1. 获取规则详情确认 `enabled` 为 `true`：
   ```bash
   curl -s http://127.0.0.1:8800/_bifrost/api/rules/test-rule-1 | jq .enabled
   ```

---

### TC-ARU-08：通过专用接口禁用规则

**前置条件**：规则 `test-rule-1` 当前为启用状态（TC-ARU-07 已启用）

**操作步骤**：
1. 执行以下命令：
   ```bash
   curl -s -X PUT http://127.0.0.1:8800/_bifrost/api/rules/test-rule-1/disable | jq .
   ```

**预期结果**：
- HTTP 状态码 200
- 返回 JSON 包含 `"message"` 字段，内容为 `"Rule 'test-rule-1' disabled successfully"`

**验证步骤**：
1. 获取规则详情确认 `enabled` 为 `false`：
   ```bash
   curl -s http://127.0.0.1:8800/_bifrost/api/rules/test-rule-1 | jq .enabled
   ```

---

### TC-ARU-09：创建包含特殊字符名称的规则

**操作步骤**：
1. 执行以下命令（名称包含中文和特殊字符）：
   ```bash
   curl -s -X POST http://127.0.0.1:8800/_bifrost/api/rules \
     -H "Content-Type: application/json" \
     -d '{"name": "测试规则-v2.0", "content": "api.example.com 127.0.0.1:5000", "enabled": false}' | jq .
   ```

**预期结果**：
- HTTP 状态码 200
- 返回 JSON 包含 `"message"` 字段，内容为 `"Rule '测试规则-v2.0' created successfully"`

**验证步骤**：
1. 通过 URL 编码的名称获取规则详情：
   ```bash
   curl -s "http://127.0.0.1:8800/_bifrost/api/rules/%E6%B5%8B%E8%AF%95%E8%A7%84%E5%88%99-v2.0" | jq .name
   ```
2. 确认返回 `"测试规则-v2.0"`

---

### TC-ARU-10：创建重复名称的规则返回错误

**前置条件**：已通过 TC-ARU-02 创建名为 `test-rule-1` 的规则

**操作步骤**：
1. 执行以下命令：
   ```bash
   curl -s -w "\n%{http_code}" -X POST http://127.0.0.1:8800/_bifrost/api/rules \
     -H "Content-Type: application/json" \
     -d '{"name": "test-rule-1", "content": "duplicate.com 127.0.0.1:6000", "enabled": true}'
   ```

**预期结果**：
- HTTP 状态码 409（Conflict）
- 返回 JSON 包含错误信息 `"Rule with this name already exists"`

---

### TC-ARU-11：验证列表响应中的 rule_count 字段

**前置条件**：已创建至少一个包含多条规则的规则文件

**操作步骤**：
1. 创建一个包含多条规则的规则文件：
   ```bash
   curl -s -X POST http://127.0.0.1:8800/_bifrost/api/rules \
     -H "Content-Type: application/json" \
     -d '{"name": "multi-rules", "content": "a.com 127.0.0.1:3001\nb.com 127.0.0.1:3002\nc.com 127.0.0.1:3003", "enabled": true}' | jq .
   ```
2. 获取规则列表：
   ```bash
   curl -s http://127.0.0.1:8800/_bifrost/api/rules | jq '.[] | select(.name == "multi-rules") | .rule_count'
   ```

**预期结果**：
- 创建成功，HTTP 状态码 200
- 列表中 `multi-rules` 的 `rule_count` 值为 `3`（对应三条规则）

---

### TC-ARU-12：删除规则

**前置条件**：已通过 TC-ARU-02 创建规则 `test-rule-1`

**操作步骤**：
1. 执行以下命令：
   ```bash
   curl -s -X DELETE http://127.0.0.1:8800/_bifrost/api/rules/test-rule-1 | jq .
   ```

**预期结果**：
- HTTP 状态码 200
- 返回 JSON 包含 `"message"` 字段，内容为 `"Rule 'test-rule-1' deleted successfully"`

**验证步骤**：
1. 尝试获取已删除的规则：
   ```bash
   curl -s -w "\n%{http_code}" http://127.0.0.1:8800/_bifrost/api/rules/test-rule-1
   ```
2. 确认 HTTP 状态码为 404，返回错误信息 `"Rule 'test-rule-1' not found"`

---

### TC-ARU-13：删除不存在的规则返回 404

**操作步骤**：
1. 执行以下命令：
   ```bash
   curl -s -w "\n%{http_code}" -X DELETE http://127.0.0.1:8800/_bifrost/api/rules/nonexistent-rule
   ```

**预期结果**：
- HTTP 状态码 404
- 返回 JSON 包含错误信息 `"Rule 'nonexistent-rule' not found"`

---

### TC-ARU-14：创建有效规则返回语法检查报告并保存

**操作步骤**：
1. 执行以下命令：
   ```bash
   curl -s -X POST http://127.0.0.1:8800/_bifrost/api/rules \
     -H "Content-Type: application/json" \
     -d '{"name": "syntax-valid-api", "content": "example.com host://127.0.0.1:3000", "enabled": true}' | jq .
   ```

**预期结果**：
- HTTP 状态码 200
- 返回 JSON 包含 `"success": true`
- 返回 JSON 包含 `"saved": true`
- 返回 JSON 包含 `"syntax.valid": true`
- 返回 JSON 包含 `"syntax.rule_count": 1`

---

### TC-ARU-15：创建缺失引用规则返回 422 且不保存

**操作步骤**：
1. 执行以下命令：
   ```bash
   curl -s -w "\n%{http_code}" -X POST http://127.0.0.1:8800/_bifrost/api/rules \
     -H "Content-Type: application/json" \
     -d '{"name": "syntax-invalid-api", "content": "@missing-shared-rule", "enabled": true}'
   ```
2. 再次获取该规则：
   ```bash
   curl -s -w "\n%{http_code}" http://127.0.0.1:8800/_bifrost/api/rules/syntax-invalid-api
   ```

**预期结果**：
- 第 1 步 HTTP 状态码为 422
- 返回 JSON 包含 `"success": false`
- 返回 JSON 包含 `"saved": false`
- 返回 JSON 包含 `"syntax.valid": false`
- 返回 JSON 包含 `"syntax.errors[0].code": "E020"`
- 第 2 步 HTTP 状态码为 404，说明无效规则没有落盘

---

### TC-ARU-16：更新为缺失引用规则返回 422 且保留旧内容

**前置条件**：已创建规则 `syntax-preserve-api`，内容为 `preserve.example.com host://127.0.0.1:3001`

**操作步骤**：
1. 执行以下命令：
   ```bash
   curl -s -w "\n%{http_code}" -X PUT http://127.0.0.1:8800/_bifrost/api/rules/syntax-preserve-api \
     -H "Content-Type: application/json" \
     -d '{"content": "@missing-update-reference", "enabled": true}'
   ```
2. 获取规则详情：
   ```bash
   curl -s http://127.0.0.1:8800/_bifrost/api/rules/syntax-preserve-api | jq -r .content
   ```

**预期结果**：
- 第 1 步 HTTP 状态码为 422
- 返回 JSON 包含 `"saved": false`
- 返回 JSON 包含 `"syntax.errors[0].code": "E020"`
- 第 2 步仍输出 `preserve.example.com host://127.0.0.1:3001`

---

### TC-ARU-17：allow_invalid 显式保存无效规则但返回语法错误

**操作步骤**：
1. 执行以下命令：
   ```bash
   curl -s -X POST http://127.0.0.1:8800/_bifrost/api/rules \
     -H "Content-Type: application/json" \
     -d '{"name": "syntax-allowed-api", "content": "@missing-allowed-reference", "enabled": true, "allow_invalid": true}' | jq .
   ```
2. 获取规则详情：
   ```bash
   curl -s http://127.0.0.1:8800/_bifrost/api/rules/syntax-allowed-api | jq -r .content
   ```

**预期结果**：
- 第 1 步 HTTP 状态码为 200
- 返回 JSON 包含 `"success": true`
- 返回 JSON 包含 `"saved": true`
- 返回 JSON 包含 `"syntax.valid": false`
- 返回 JSON 包含 `"syntax.errors[0].code": "E020"`
- 第 2 步输出 `@missing-allowed-reference`

**执行结果（2026-06-12，本地开发分支）**：
- ✅ PASS：执行 `BIFROST_BIN="$PWD/target/debug/bifrost" bash e2e-tests/tests/test_rule_syntax_save_api.sh`。脚本使用临时数据目录和动态端口启动真实 Bifrost 服务，验证有效创建返回 `saved=true` / `syntax.valid=true`，缺失 `@` 引用创建返回 HTTP 422 且 GET 为 404，缺失引用更新返回 HTTP 422 且旧内容保持不变，`allow_invalid=true` 可保存无效规则但返回 `syntax.valid=false` / `E020`。

---

## 清理

测试完成后清理临时数据：
```bash
rm -rf .bifrost-test
```
