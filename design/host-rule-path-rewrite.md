# Host Rule Path Rewrite

## 问题描述

当规则同时包含源路径前缀和目标路径前缀时，例如：

```text
https://ejt9lgzgu9.feishu-boe.cn/labor_cost/static/ http://localhost:9000/labor_cost/static/
```

请求：

```text
https://ejt9lgzgu9.feishu-boe.cn/labor_cost/static/07c1d7e1fb3e13436b958af5f90ec9c8.svg
```

预期应转发到：

```text
http://localhost:9000/labor_cost/static/07c1d7e1fb3e13436b958af5f90ec9c8.svg
```

不能错误转成：

```text
http://localhost:9000/labor_cost/static/labor_cost/static/07c1d7e1fb3e13436b958af5f90ec9c8.svg
```

## 根因

现有实现只拿到了目标 host rule 中的 target path，但没有稳定拿到“真正生效的源 pattern path”。

- 如果直接把 target path 当作 base path 再拼原请求 path，会导致重复前缀
- 如果从 `resolved_rules.rules` 中倒序取最后一条 host 类规则，又可能拿到并未实际生效的另一条规则，导致错误裁剪 path

## 修复方案

1. 在 `crates/bifrost-proxy/src/utils/url.rs` 中提供 path 重写辅助函数：
   - 从 pattern 提取 source path
   - 从 host rule 提取 target path
   - 用 source path 去裁剪请求 path，再和 target path 合并
2. source path 必须基于**当前真正生效的 host rule**反查：
   - 按 `host_protocol + host_rule value` 在 `resolved_rules.rules` 中从前向后定位第一条匹配规则
   - 不再使用“最后一条 host 类规则”的近似逻辑
3. 将该逻辑同时接入：
   - 普通 HTTP 转发链路
   - HTTPS TLS 拦截转发链路
   - WebSocket 的 host rewrite 握手路径

## 测试方案

### 单元测试

- `utils/url.rs`
  - `test_rewrite_path_same_source_target`
  - `test_rewrite_path_with_query_string`
  - `test_find_host_rule_source_path_uses_selected_rule_not_last_host_rule`

### E2E 脚本

- `e2e-tests/tests/test_host_rule_path_rewrite.sh`
  - 构造 `https://.../labor_cost/static/ -> http://127.0.0.1:<echo>/labor_cost/static/`
  - 通过真实代理请求 `.svg`
  - 断言上游 `parsed_path` 等于单份 `/labor_cost/static/<file>`
  - 断言 query string 保持不变

### Human Tests

- 更新 `human_tests/proxy-http-https.md`
  - 新增 host rule 路径前缀回归用例
  - 覆盖 HTTPS + TLS 拦截 + 静态资源路径前缀保留

## 校验要求

- 先执行新增 E2E 脚本
- 再执行 Rust 相关单元测试
- 最后执行 `cargo test --workspace --all-features`

## 文档更新要求

- 更新 `human_tests/proxy-http-https.md`
- 更新 `human_tests/readme.md`
