# 请求体大小判断规则

## 现状结论

这项改造已经实现，HTTP 与 tunnel 两条链路都已经把“body override”和“需要读取原 body 才能处理”的情况拆开。

## 当前实现

- 请求侧：
  - `has_req_body_override = resolved_rules.req_body.is_some()`
  - 只有在没有 override 且确实需要 body 规则/脚本时，才会读取请求体。
- 响应侧：
  - `has_res_body_override = resolved_rules.res_body.is_some()`
  - 只有需要处理响应体且没有 override 时，才会走 bounded read / probe read。
- 对大体积或疑似流式 body：
  - 通过 `max_body_buffer_size` 和 `max_body_probe_size` 控制预读；
  - 超限时跳过 body 规则与脚本，改走流式转发。

## 当前语义

- `req_body` / `res_body` 这类直接替换规则，不会因为原 body 太大而失效。
- 依赖读取原 body 的 replace / prepend / append / merge / scripts 仍会在超限时被跳过。

## 适用范围

- 普通 HTTP 请求处理链路。
- HTTPS tunnel / H3 相关响应处理链路。

## 实现位置（截至 2026-06-17）

- 请求体 override 判定：
  - `crates/bifrost-proxy/src/proxy/http/handler.rs:879`（请求路径主入口）
  - `crates/bifrost-proxy/src/proxy/http/handler.rs:1882`（再处理路径）
  - `crates/bifrost-proxy/src/proxy/http/tunnel/mod.rs:2328`
- 响应体 override 判定：
  - `crates/bifrost-proxy/src/proxy/http/handler.rs:3055`
  - `crates/bifrost-proxy/src/proxy/http/tunnel/mod.rs:3621`
- 大体积 / 探测预读上限：
  - `crates/bifrost-admin/src/handlers/config.rs`、`crates/bifrost-admin/src/state.rs` 中的 `max_body_buffer_size` / `max_body_probe_size`（admin 可配置，默认 10 MiB / 64 KiB）。
- 直接替换规则的字段：`crates/bifrost-admin/src/request_rules.rs` 中的 `req_body` / `res_body`（`Option<Bytes>`）。
- 端到端验证：`e2e-tests/rules/advanced/body_size_strategy.txt` 与 `e2e-tests/tests/test_res_body_override_large.sh` 覆盖 req/res 两侧的大 body override 场景。
