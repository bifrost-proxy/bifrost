# CI sherpa-onnx 预编译归档缓存

## 功能模块说明

验证 GitHub Actions 在 macOS arm64 CLI 构建前预先下载并缓存 `sherpa-onnx-sys` 需要的官方预编译归档，避免 Cargo build script 在构建阶段直接访问 GitHub Release asset 并因间歇性 504 导致 CI 红灯。

## 前置条件

- 仓库根目录为当前工作目录。
- 本地无需下载真实 sherpa-onnx 归档；用 `file://` mock release 目录验证脚本行为。
- Python 3 可用于静态解析 GitHub Actions workflow YAML 文本。

## 测试用例列表

### TC-CI-SHERPA-01 本地 mock 归档准备

操作步骤：
1. 创建临时目录，按 `v1.13.2/sherpa-onnx-v1.13.2-osx-arm64-static-lib.tar.bz2` 路径放置一个可被 `tar -tjf` 读取的 mock 归档。
2. 设置 `SHERPA_ONNX_RELEASE_BASE_URL=file://<mock-release-dir>`、`SHERPA_ONNX_ARCHIVE_DIR=<tmp-cache-dir>`。
3. 执行 `bash scripts/ci/prepare-sherpa-onnx-archive.sh aarch64-apple-darwin`。
4. 再执行一次同样命令，验证第二次命中本地缓存而不是重新下载。

预期结果：
- 第一次执行输出 `Prepared sherpa-onnx archive`。
- 第二次执行输出 `Using cached sherpa-onnx archive`。
- `<tmp-cache-dir>/sherpa-onnx-v1.13.2-osx-arm64-static-lib.tar.bz2` 存在且 `tar -tjf` 成功。

### TC-CI-SHERPA-02 GitHub Actions 接线检查

操作步骤：
1. 静态读取 `.github/workflows/ci.yml`。
2. 验证 `build-cli-macos-aarch64` 包含 `Cache sherpa-onnx archive` 与 `Prepare sherpa-onnx archive` 步骤。
3. 验证 `e2e-macos-runner` 包含同样的缓存与准备步骤。
4. 静态读取 `.github/workflows/release.yml`。
5. 验证 release `Build CLI` matrix 在 `matrix.target == 'aarch64-apple-darwin'` 时执行同一缓存与准备步骤。

预期结果：
- CI workflow 中 macOS arm64 CLI build 和 macOS arm64 runner E2E 前都会准备本地 sherpa-onnx 归档。
- Release workflow 中 macOS arm64 CLI build 前也会准备本地 sherpa-onnx 归档。

## 执行记录

- 2026-06-08：执行 TC-CI-SHERPA-01，PASS。脚本通过 `file://` mock release 目录准备归档，第二次执行命中缓存。
- 2026-06-08：执行 TC-CI-SHERPA-02，PASS。静态检查确认 CI 和 Release workflow 均包含 macOS arm64 sherpa-onnx 缓存与准备步骤。

## 清理步骤

- 删除测试过程中创建的临时 mock release 目录和临时 cache 目录。
