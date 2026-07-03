# WebUI gzip static assets 设计方案

## 背景

Bifrost CLI 通过 `crates/bifrost-admin` 内嵌 Admin WebUI。发布版本会把 Vite 构建产物 `web/dist` 作为原始字节嵌入到二进制，这意味着二进制本身携带未压缩的 JS / CSS / worker / font / HTML / image 资源。这样做有两个缺点：

- 二进制体积偏大（前端资源在打包时不做压缩）。
- 首次加载时 CLI 直接把 raw bytes 回给浏览器，浏览器仍需自己解压 (`Accept-Encoding: gzip` 只影响协商，不影响 CLI 发出内容)。

本设计在 build 阶段把每个 `web/dist` 文件 gzip 一份到 `web/dist-gzip`，编译时嵌入 gzip 目录；运行时 Static 路径要求客户端支持 gzip，直接把预压缩字节回给浏览器。可以显著减小二进制体积并加快首屏。

## 用户目标验证清单

### 必须实现

- `crates/bifrost-admin/build.rs` 在完成前端 build 后自动生成 `web/dist-gzip`，每个文件用 gzip 压缩到相同相对路径（不加 `.gz` 后缀）。
- 二进制里通过 `#[folder = "../../web/dist-gzip"]` 嵌入 gzip 目录（`crates/bifrost-admin/src/static_files.rs` 第 7 行）。
- 静态请求命中时：
  - 若 `Accept-Encoding` 包含 `gzip`（非 `q=0`）→ 返回 gzip bytes，附加 `Content-Encoding: gzip`、`Vary: Accept-Encoding`、原扩展名对应的 `Content-Type` 与标准缓存头。
  - 若 `Accept-Encoding` 不含 gzip → 返回 `426 Upgrade Required`，纯文本 `Bifrost WebUI requires a gzip-capable browser. Please upgrade your browser.`。
- SPA fallback：未知 WebUI 路由映射到 `index.html`，仍以 gzip 形式返回（前提是客户端支持 gzip）。
- `mime_guess` 按原扩展名推断 `Content-Type`，因为查找 key 使用原始请求路径而非 `.gz` 后缀。

### 必须不破坏

- Admin JSON API (`/_bifrost/api/*`)、WS 推送、Sync WebSocket、Bifrost Proxy 等非 WebUI 静态路径不受 gzip 强制影响。
- `mime_guess` 的推断结果不变（gzip 只影响 body，不影响 `Content-Type`）。
- `crates/bifrost-admin/src/router.rs` 中静态路由与 API 路由的分派顺序不变。
- 开发模式下若无 `web/dist` 时的现有降级逻辑保留（`build.rs` 中 `ensure_dist_exists` + `generate_gzip_dist` 组合）。

### 必须真实验证

- Unit：`accepts_gzip_encoding_header` 覆盖多种 `Accept-Encoding` 值（`br, gzip;q=1.0`、`gzip;q=0`、`gzip;q=0.0`、缺失头）。
- Unit：`serves_index_as_gzip_static_asset`、`rejects_non_gzip_static_client`、`falls_back_to_gzip_index_for_spa_routes`。
- E2E：`e2e-tests/tests/test_webui_gzip_static_assets.sh` 用真实 CLI 二进制走 curl 验证 3 条路径。
- Human test：`human_tests/webui-static-assets.md` 覆盖 gzip 客户端、非 gzip 客户端、SPA fallback。

## 产品语义

### 只支持 gzip 客户端

Bifrost WebUI 明确不支持 `identity` 或纯 `br` 客户端；因为二进制里只带 gzip 版本，没有原始或其它编码。响应 `426 Upgrade Required` 而不是 `200` + 未压缩内容，避免解压逻辑重新嵌入到运行时。

Bifrost 目标用户是使用现代浏览器 (Chrome / Edge / Firefox / Safari / 内嵌 WebView) 的开发者与员工，`Accept-Encoding: gzip` 是默认行为，不构成兼容性风险。

### 编译时压缩、运行时零解压

- Build 阶段：`flate2::write::GzEncoder` + `Compression::default()`（约等于 level 6）。压缩发生在 `build.rs::gzip_file`。
- 运行时：`static_files.rs` 直接 clone 内嵌 bytes 到 `Response<BoxBody>`，不做 decompress / recompress。CPU 开销仅是 body clone。

### SPA fallback 也走 gzip

WebUI 是 SPA，未匹配到静态资源的路由（如 `/rules/detail/xxx`）应回 `index.html`。因为 index.html 也是 gzip 形式，fallback 分支同样要求客户端支持 gzip；否则一样 `426`。

## 技术细节

### `crates/bifrost-admin/build.rs`

关键函数：

- `ensure_dist_exists(dist_dir)`：如果 `web/dist` 不存在或没有 `index.html`，尝试通过 `pnpm build` 生成；开发模式下允许 stub。
- `generate_gzip_dist(dist_dir, gzip_dist_dir)`：
  1. 校验 `dist_dir` 存在。
  2. 若 `gzip_dist_dir` 已存在，`remove_dir_all_with_retry` 清理。
  3. 递归遍历 `dist_dir`，`gzip_directory` → `gzip_file` 生成对应 gzip 副本。
  4. 打印 `cargo:warning=Generated {} gzip frontend assets at {}`。
- `gzip_file(root_dir, source_path, gzip_dist_dir)`：
  - `output_path = gzip_dist_dir.join(relative_path)`（不加 `.gz` 后缀，见 build.rs 第 283 行）。
  - `fs::create_dir_all(output_path.parent())` 确保目录存在。
  - `GzEncoder::new(File::create(output_path), Compression::default())` → `copy(source, &mut encoder)` → `encoder.finish()`；任一失败 panic 并附源路径。

Build 触发点：

- `main()` 中根据 `SKIP_FRONTEND_BUILD` 环境变量决定是否跳过前端 build；但无论如何都会调用 `ensure_dist_exists` + `generate_gzip_dist`。
- `pnpm` 缺失时会 fallback：只要 `dist` 已存在，仍能 gzip 而不阻塞 Rust build。

### `crates/bifrost-admin/src/static_files.rs`

嵌入 & 查找：

```rust
#[derive(rust_embed::RustEmbed)]
#[folder = "../../web/dist-gzip"]
struct EmbeddedGzipAssets;
```

Serve 主流程（伪代码，与实际 impl 对齐）：

```rust
if !accepts_gzip(headers) {
    return gzip_required_response();
}
let normalized = trim_leading_slashes(path);
let mime = mime_guess::from_path(normalized).first_or_octet_stream();
if let Some(bytes) = EmbeddedGzipAssets::get(normalized) {
    return response_from_gzip(bytes, mime);
}
if let Some(bytes) = EmbeddedGzipAssets::get("index.html") {
    return response_from_gzip(bytes, "text/html; charset=utf-8");
}
NotFound
```

`response_from_gzip` 写入：

- `Content-Encoding: gzip`
- `Vary: Accept-Encoding`
- `Content-Type: <mime>`
- 静态缓存头（e.g. `Cache-Control: public, max-age=...`），保持与原有 static 行为一致。

`gzip_required_response`：

- `StatusCode::UPGRADE_REQUIRED` (426)
- `Vary: Accept-Encoding`
- Body: `"Bifrost WebUI requires a gzip-capable browser. Please upgrade your browser."`

### `accepts_gzip`

`fn accepts_gzip(headers: &HeaderMap) -> bool`：

- 读取 `ACCEPT_ENCODING`，按 `,` 拆分。
- 每一段 `encoding_allows_gzip(part)` 判定：
  - 只接受名字为 `gzip`（`eq_ignore_ascii_case`）。
  - 解析 `q=`；`q=0` / `q=0.0` 视为不允许。
  - 未指定 q 视为允许。

用例覆盖 `br, gzip;q=1.0` 等复合值，见 `static_files.rs::accepts_gzip_encoding_header` 测试。

### 依赖

- Build 依赖：`flate2` 只作为 `[build-dependencies]`，runtime crate 不引入。
- 运行时不再需要压缩库；也不引入 `br` / `zstd`。

## CLI + Web + Admin API 边界

### CLI

- 不新增 CLI 参数。
- `bifrost start` / `bifrost proxy start` 走同一个 Admin router，静态请求路径统一走 `static_files.rs`。

### Web UI

- 用户无感知：浏览器默认 `Accept-Encoding: gzip, deflate, br`，命中 gzip 分支。
- 若企业代理剥离 `Accept-Encoding`，用户会看到 `426 Upgrade Required` 提示，指向 "升级浏览器 / 保留 gzip"。

### Admin API

- API 路由 `/_bifrost/api/*` 不走 `static_files.rs`，不受 gzip 强制影响，仍按 JSON 直返。
- 只有 `/_bifrost/` 与 SPA 未知路径会走 gzip 静态。

## Sync 边界

- 完全不涉及 Sync/规则/流量 Sync；纯打包 & 静态 serving 变更。

## Phase 拆分

### Phase 1：Build 阶段生成 gzip

- 新增 `generate_gzip_dist` / `gzip_file` / `gzip_directory` / `ensure_dist_exists` 组合。
- `[build-dependencies]` 加 `flate2`。
- 保证 `pnpm build` 失败时的 fallback 不引入 panic。

### Phase 2：Runtime 只 serve gzip

- `static_files.rs` 改为 `#[folder = "../../web/dist-gzip"]`。
- 实现 `accepts_gzip` / `gzip_required_response`。
- 单元测试覆盖头解析 / 200 / 426 / SPA fallback。

### Phase 3：E2E + human_tests

- 新增 `e2e-tests/tests/test_webui_gzip_static_assets.sh`。
- 新增 `human_tests/webui-static-assets.md` 并写入 `human_tests/readme.md`。

### Phase 4：清理

- 移除任何遗留的 `web/dist` runtime embed 路径。
- 确认 `.gitignore` 不误提交 `web/dist-gzip`。

## 测试方案

### 单元测试

`crates/bifrost-admin/src/static_files.rs` 中：

- `accepts_gzip_encoding_header`：`br, gzip;q=1.0` → true；`gzip;q=0` → false；`gzip;q=0.0` → false；缺失头 → false。
- `serves_index_as_gzip_static_asset`：静态 `index.html` + gzip 头 → `200`、`Content-Encoding: gzip`、`Vary: Accept-Encoding`、`Content-Type: text/html`。
- `rejects_non_gzip_static_client`：非 gzip 请求 → `426 Upgrade Required` + 升级提示文本。
- `falls_back_to_gzip_index_for_spa_routes`：`/rules/not-a-real-asset` + gzip 头 → `200`、`Content-Encoding: gzip`、body 是 gzip index.html。
- 路径归一化仍会 trim leading slashes（既有单测保留）。

### E2E

`e2e-tests/tests/test_webui_gzip_static_assets.sh`：

- 构建或复用 `target/release/bifrost`。
- `BIFROST_DATA_DIR=$(mktemp -d)`、`--no-system-proxy`、`BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`、`BIFROST_DISABLE_TRAY=1` 启动。
- Case A：`curl -H 'Accept-Encoding: gzip' http://127.0.0.1:$PORT/_bifrost/` → `200`、`Content-Encoding: gzip`、body gunzip 后是 HTML。
- Case B：`curl -H 'Accept-Encoding: identity' http://127.0.0.1:$PORT/_bifrost/` → `426`、body 含升级提示。
- Case C：`curl -H 'Accept-Encoding: gzip' http://127.0.0.1:$PORT/_bifrost/some/spa/path` → `200`、`Content-Encoding: gzip`、body gunzip 后是 HTML（SPA fallback）。

### human_tests

`human_tests/webui-static-assets.md`：

- TC-WSA-01：Chrome / Firefox 打开 `/_bifrost/`，浏览器收到 gzip HTML。
- TC-WSA-02：`curl -H 'Accept-Encoding: identity'` 收到 426 与升级提示。
- TC-WSA-03：SPA 路径直接访问（如 `/rules/xxx`）仍拿到 gzip 后的 index.html。
- 更新 `human_tests/readme.md` 用例数量与说明。

## Review / Fix / Test 闭环

### Round 1

- 复读用户目标 + 本 doc。
- `git status --short`、`git diff`、`git diff --cached`（若有 staged）。
- 复查 `build.rs`、`static_files.rs`、E2E 脚本、design doc、human test doc：
  - 是否仍嵌入 raw `web/dist`（残留 `#[folder = "../../web/dist"]`）
  - 头字段是否齐全（`Content-Encoding` / `Vary`）
  - SPA fallback 是否遗漏
  - 非 gzip 客户端是否忘记 426
- 复跑 targeted unit 与 E2E：
  - `cargo test -p bifrost-admin static_files`
  - `bash e2e-tests/tests/test_webui_gzip_static_assets.sh`

### Round 2

- 复查 Round 1 修复后的 diff。
- 确认 `web/dist-gzip` 是 build 产物而非 source artifact（`.gitignore` 已排除）。
- 确认 `human_tests/readme.md` 的用例数量与新增行同步。
- 复跑 targeted unit 与 E2E；如 fmt/clippy 有 warning，一并修。

## 校验要求

- `cargo fmt --all -- --check`
- `cargo test -p bifrost-admin static_files`
- `bash e2e-tests/tests/test_webui_gzip_static_assets.sh`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test --workspace --all-features`
- 时间允许时收尾运行 `scripts/ci/local-ci.sh`；不允许时记录未执行风险。

## 文档更新要求

- 更新 `human_tests/webui-static-assets.md`（如未创建则新建）并同步 `human_tests/readme.md`。
- 因为纯内部打包 & 静态 serving 行为，`README` / `site/` 用户文档无需改动。
- 若未来增加 `br` / `zstd`，再补充用户可见文档。

## 风险与决策点

- **企业代理剥离 Accept-Encoding**：用户可能看到 426。文案里提示 "gzip-capable browser"，但真实原因可能是中间代理。可以在下一版补 Admin 页面手动 fallback 或提供 `--webui-serve-uncompressed` 环境开关；本版不做。
- **磁盘上出现 stale `web/dist-gzip`**：`generate_gzip_dist` 每次都会 `remove_dir_all_with_retry` 重建，避免陈旧文件泄漏；但如果 build 中断可能残留部分文件，Round 2 需要提示开发者手动删除。
- **压缩级别选择**：目前 `Compression::default()`（约 level 6）。改成更高级别 (9) 会增加 build 时间但收益不显著；改成更低级别 (1) 会加大二进制体积。保持默认。
- **`br` / `zstd` 支持**：暂不支持。如果未来需要，需要重设计 `EmbeddedAssets` 分级 + `Accept-Encoding` 协商；本 doc 不承诺。
- **CLI 二进制 `SKIP_FRONTEND_BUILD=1` 场景**：允许开发迭代跳过前端 build；此时 `web/dist` 必须已存在，否则 `generate_gzip_dist` 会 panic。文档需要提示 CI 与本地开发人员。
