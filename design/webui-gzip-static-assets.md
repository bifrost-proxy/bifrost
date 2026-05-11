# WebUI gzip static assets

## Module scope

Bifrost CLI embeds the Admin WebUI through `bifrost-admin`. The current release binary embeds the Vite `web/dist` files as raw bytes. This makes the binary carry the uncompressed JavaScript, CSS, worker, font, HTML, and image assets.

This change stores a gzip-compressed copy of each `web/dist` file in a generated `web/dist-gzip` directory and embeds that generated directory instead. Static WebUI responses require a gzip-capable client.

## Implementation logic

- `crates/bifrost-admin/build.rs` keeps the existing frontend build flow.
- After ensuring or building `web/dist`, the build script recreates `web/dist-gzip`.
- Each regular file under `web/dist` is gzip-compressed into the same relative path under `web/dist-gzip`; the output path does not add a `.gz` suffix.
- `crates/bifrost-admin/src/static_files.rs` embeds `../../web/dist-gzip`.
- Static file lookup still uses the original request path, so `mime_guess` continues to infer `Content-Type` from the original extension.
- If `Accept-Encoding` contains `gzip`, the response body is the embedded gzip bytes with:
  - `Content-Encoding: gzip`
  - `Vary: Accept-Encoding`
  - original `Content-Type`
  - normal static cache headers
- If `Accept-Encoding` does not include `gzip`, the WebUI static path returns `426 Upgrade Required` with a plain text upgrade message. The project only supports gzip-capable WebUI clients for embedded static assets.
- SPA fallback still maps unknown WebUI routes to `index.html`, served as gzip when supported.

## Dependencies

- `flate2` is used as a build dependency to generate gzip assets at compile time.
- Runtime serving does not decompress or recompress static files.

## Test plan

### Unit tests

- `accepts_gzip_encoding_header` accepts comma-separated and q-value `Accept-Encoding` values that include `gzip`.
- Static `index.html` request with gzip returns `200`, `Content-Encoding: gzip`, `Vary: Accept-Encoding`, and `text/html`.
- Non-gzip static request returns `426 Upgrade Required`.
- Unknown WebUI route with gzip falls back to gzip `index.html`.
- Existing path normalization continues to trim leading slashes.

### E2E test

Create `e2e-tests/tests/test_webui_gzip_static_assets.sh`:

- Build or reuse a fresh `target/release/bifrost`.
- Start a real Bifrost process with a temporary `BIFROST_DATA_DIR` and `--no-system-proxy`.
- `curl -H 'Accept-Encoding: gzip' /_bifrost/` should return `200`, `Content-Encoding: gzip`, and gzip bytes that decompress to HTML.
- `curl -H 'Accept-Encoding: identity' /_bifrost/` should return `426` and the upgrade message.
- `curl -H 'Accept-Encoding: gzip' /_bifrost/some/spa/path` should return gzip HTML.

### Human test

Create `human_tests/webui-static-assets.md` and index it in `human_tests/readme.md`. Execute each case immediately after writing the document:

- gzip-capable client receives compressed WebUI HTML.
- non-gzip client receives the explicit upgrade-required response.
- SPA fallback is still served to gzip-capable clients.

## Review/Fix/Test loop

### Round 1

- Re-read the user goal and this design.
- Run `git status --short`, `git diff`, and `git diff --cached` if anything is staged.
- Review `build.rs`, `static_files.rs`, E2E script, design doc, and human test doc for stale raw-dist embedding, wrong headers, missing SPA fallback, and missing non-gzip rejection.
- Run targeted unit tests and the new E2E script.

### Round 2

- Recheck the latest diff after any Round 1 fixes.
- Confirm `web/dist-gzip` is generated but not committed as a source artifact.
- Confirm `human_tests/readme.md` count and row match the new doc.
- Rerun targeted unit tests and affected E2E/human-test commands.

## Validation requirements

- `cargo fmt --all -- --check`
- targeted `cargo test -p bifrost-admin static_files`
- `bash e2e-tests/tests/test_webui_gzip_static_assets.sh`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test --workspace --all-features`
- `scripts/ci/local-ci.sh` should be run at the end if time allows; if not, record the unexecuted risk.

## Documentation updates

- Update `human_tests/webui-static-assets.md`.
- Update `human_tests/readme.md`.
- README does not need user-facing changes because this is an internal packaging and static-serving behavior change.
