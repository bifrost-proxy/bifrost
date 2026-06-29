# 规则合并策略 - 全量协议合并行为验证

## 功能模块说明

验证所有协议的合并策略是否符合设计文档（`design/rule-merge-strategy.md`）中标记为"正确"的行为。涵盖：
- 转发类协议 first-match-wins
- Mock 类协议 first-match-wins（non-multi_match）
- 修改类标量值 single-match
- 修改类纯累积型 accumulate
- Body/Prepend/Append last-wins
- CORS/HTML/JS/CSS 注入 last-wins
- KV 集合累积
- 特殊协议
- 控制类协议

## 前置条件

1. 启动 Bifrost 服务（使用临时数据目录）：
```bash
BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- start -p 8800 --unsafe-ssl
```

## 测试用例列表

### TC-RMS-01: 单元测试 - 转发类协议 first-match-wins

**操作步骤**：
```bash
cargo test -p bifrost-cli -- test_merge_host_first_match_wins test_merge_xhost_first_match_wins test_merge_http_first_match_wins test_merge_https_first_match_wins test_merge_ws_first_match_wins test_merge_wss_first_match_wins test_merge_tunnel_assigns_host test_merge_host_passthrough_blocks_host
```

**预期结果**：
- 所有 8 个测试通过
- host/xhost/http/https/ws/wss 两条同域名规则时，第一条匹配的值生效（first-match-wins）
- tunnel 直接赋值到 host 字段
- passthrough 设置 ignored.host=true 后阻断后续 host 规则

### TC-RMS-02: 单元测试 - Mock 类协议 non-multi_match

**操作步骤**：
```bash
cargo test -p bifrost-cli -- test_merge_file_non_multi_match test_merge_tpl_non_multi_match test_merge_rawfile_non_multi_match test_merge_redirect_non_multi_match
```

**预期结果**：
- 所有 4 个测试通过
- file/tpl/rawfile/redirect 两条同域名规则时，只有第一条匹配到的规则生效

**执行记录（2026-06-29 CI 覆盖率回归）**：
- 执行命令：`cargo test -p bifrost-cli parsing::rules::tests::test_merge_redirect_non_multi_match -- --nocapture`
- 实际结果：通过。redirect non-multi_match 用例使用 `redirect://(...)` 内联目标值，避免 CI 覆盖率任务把 `http://target-a.com` 当作远程内容加载并引入外部 HTML。

### TC-RMS-03: 单元测试 - 修改类标量值 single-match

**操作步骤**：
```bash
cargo test -p bifrost-cli -- test_merge_status_code_single_match test_merge_replace_status_single_match test_merge_method_single_match test_merge_ua_single_match test_merge_referer_single_match test_merge_proxy_single_match test_merge_auth_single_match test_merge_req_delay_single_match test_merge_res_delay_single_match test_merge_req_speed_single_match test_merge_res_speed_single_match test_merge_req_type_single_match test_merge_res_type_single_match test_merge_req_charset_single_match test_merge_res_charset_single_match test_merge_cache_single_match test_merge_attachment_single_match test_merge_http3_flag
```

**预期结果**：
- 所有 18 个测试通过
- 标量值类型（statusCode/replaceStatus/method/ua/referer/proxy/auth/reqDelay/resDelay/reqSpeed/resSpeed/reqType/resType/reqCharset/resCharset/cache/attachment/http3）均为 non-multi_match，只有第一条规则生效

### TC-RMS-04: 单元测试 - Body/Prepend/Append last-wins

**操作步骤**：
```bash
cargo test -p bifrost-cli -- test_merge_res_body_last_wins test_merge_req_body_last_wins test_merge_req_prepend_last_wins test_merge_req_append_last_wins test_merge_res_prepend_last_wins test_merge_res_append_last_wins
```

**预期结果**：
- 所有 6 个测试通过
- Body/Prepend/Append 是 multi_match 类型，多条规则匹配时后面的覆盖前面的（last-wins）

### TC-RMS-05: 单元测试 - CORS last-wins

**操作步骤**：
```bash
cargo test -p bifrost-cli -- test_merge_res_cors_last_wins test_merge_req_cors_last_wins
```

**预期结果**：
- 所有 2 个测试通过
- CORS 是整体替换语义，multi_match 时后面的覆盖前面的

### TC-RMS-06: 单元测试 - 纯累积型

**操作步骤**：
```bash
cargo test -p bifrost-cli -- test_merge_req_replace_accumulate test_merge_res_replace_accumulate test_merge_url_replace_accumulate test_merge_req_script_accumulate test_merge_res_script_accumulate test_merge_decode_accumulate test_merge_dns_accumulate test_merge_delete_accumulate test_merge_header_replace_accumulate
```

**预期结果**：
- 所有 9 个测试通过
- 累积型协议（reqReplace/resReplace/urlReplace/reqScript/resScript/decode/dns/delete/headerReplace）多条规则全部累积，不覆盖

### TC-RMS-07: 单元测试 - KV 集合累积

**操作步骤**：
```bash
cargo test -p bifrost-cli -- test_merge_req_cookies_accumulate test_merge_res_cookies_accumulate test_merge_url_params_accumulate test_merge_trailers_accumulate
```

**预期结果**：
- 所有 4 个测试通过
- reqCookies/resCookies/urlParams/trailers 多条规则的 KV 对全部累积

### TC-RMS-08: 单元测试 - HTML/JS/CSS 注入 last-wins

**操作步骤**：
```bash
cargo test -p bifrost-cli -- test_merge_html_append_last_wins test_merge_html_prepend_last_wins test_merge_html_body_last_wins test_merge_js_append_last_wins test_merge_js_prepend_last_wins test_merge_js_body_last_wins test_merge_css_append_last_wins test_merge_css_prepend_last_wins test_merge_css_body_last_wins
```

**预期结果**：
- 所有 9 个测试通过
- HTML/JS/CSS 注入类协议是 multi_match，后面的覆盖前面的

### TC-RMS-09: 单元测试 - 特殊协议

**操作步骤**：
```bash
cargo test -p bifrost-cli -- test_merge_forwarded_for_pushes_to_req_headers test_merge_response_for_pushes_to_res_headers test_merge_passthrough_sets_ignored_host
```

**预期结果**：
- 所有 3 个测试通过
- forwardedFor 推入 req_headers（无去重）
- responseFor 推入 res_headers（无去重）
- passthrough 设置 ignored.host = true

### TC-RMS-10: 单元测试 - 控制类协议

**操作步骤**：
```bash
cargo test -p bifrost-cli -- test_merge_tls_intercept test_merge_tls_passthrough test_merge_tls_options test_merge_sni_callback
```

**预期结果**：
- 所有 4 个测试通过
- tlsIntercept 设置 tls_intercept=Some(true)
- tlsPassthrough 设置 tls_intercept=Some(false)
- tlsOptions/sniCallback 正确赋值

### TC-RMS-11: 单元测试 - 综合场景

**操作步骤**：
```bash
cargo test -p bifrost-cli -- test_merge_forward_and_modify_coexist test_merge_multiple_accumulate_protocols test_merge_redirect_with_status_code test_merge_reqheaders_different_keys_accumulate test_merge_resheaders_different_keys_accumulate test_merge_reqheaders_same_key_first_wins test_merge_resheaders_same_key_first_wins
```

**预期结果**：
- 所有 7 个测试通过
- 转发+修改可共存：host 规则和 reqHeaders/resHeaders/reqCookies 同时生效
- 多累积型协议互不影响
- reqHeaders/resHeaders 不同 key 累积、同 key first-wins

### TC-RMS-12: E2E 测试 - 真实代理场景验证

**操作步骤**：
```bash
cargo test -p bifrost-e2e -- rule_merge_strategy
```

**预期结果**：
- 所有 15 个 E2E 测试通过
- 验证真实代理环境中：
  - host first-match-wins
  - file mock first-match-wins
  - method/ua single-match
  - statusCode 正确设置
  - reqHeaders/resHeaders 不同 key 合并
  - resBody last-wins
  - reqReplace 累积
  - resCors last-wins
  - forward + 多种修改同时生效
  - cookies 累积
  - redirect 301 正确

### TC-RMS-13: 全量单元测试回归验证

**操作步骤**：
```bash
cargo test -p bifrost-cli -- test_merge 2>&1 | tail -5
```

**预期结果**：
- 74 个 test_merge 前缀的测试全部通过
- 无失败、无 panic

## 清理步骤

1. 停止测试服务：`cargo run --bin bifrost -- stop -p 8800`
2. 删除临时数据目录：`rm -rf ./.bifrost-test`
