# CI Cross Build

## 功能模块说明

GitHub Actions 的 Linux cross build job 使用 `cross` 构建 `aarch64-unknown-linux-gnu`、`armv7-unknown-linux-gnueabihf`、`x86_64-unknown-linux-musl` 和 `aarch64-unknown-linux-musl` CLI release binary。`armv7` job 在 GitHub hosted runner 上会触发 `cross` 构建自定义容器镜像；当 runner 的 Docker buildx/buildkit 不可用或不稳定时，`cross` 会在进入 Rust 编译前失败。

## 实现逻辑

- 所有 CI cross build step 显式设置 `CROSS_CONTAINER_ENGINE_NO_BUILDKIT=1`。
- release workflow 的 cross build step 同步设置同一变量，避免发布路径和 PR CI 配置漂移。
- `Cross.toml` 保持当前 target pre-build 命令不变；本次修复只约束 cross 的宿主容器构建方式，不改变目标容器内依赖安装逻辑。

## 依赖项

- `.github/workflows/ci.yml`
- `.github/workflows/release.yml`
- `Cross.toml`

## 测试方案

### 单元测试

本次修改为 GitHub Actions YAML 配置，不涉及 Rust 公共函数，不新增 Rust 单元测试。

### E2E 测试

- 静态检查 `.github/workflows/ci.yml` 中 4 个 `cross build` step 均设置 `CROSS_CONTAINER_ENGINE_NO_BUILDKIT: "1"`。
- 静态检查 `.github/workflows/release.yml` 的 matrix cross build step 设置 `CROSS_CONTAINER_ENGINE_NO_BUILDKIT: "1"`。
- 推送后通过 GitHub Actions `CI` workflow 验证 `Linux Build (armv7)` 不再在 Docker buildkit 阶段失败。

### 真实场景测试

- 更新 `human_tests/ci-cross-build.md`，覆盖 PR CI 与 release workflow 的 cross buildkit 禁用配置。
- 按用例执行静态检查；云端最终结果以 GitHub Actions `CI` run 全绿为准。

## 校验要求

- `git diff --check -- .github/workflows/ci.yml .github/workflows/release.yml design/ci-cross-build.md human_tests/ci-cross-build.md human_tests/readme.md`
- GitHub Actions `CI` workflow 全绿。

## 文档更新要求

- 更新 `human_tests/ci-cross-build.md`
- 更新 `human_tests/readme.md`
