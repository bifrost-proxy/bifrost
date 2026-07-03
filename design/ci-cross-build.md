# CI Cross Build 稳定化设计方案

## 背景

Bifrost 除主流 macOS/Windows 目标外，还需要在 GitHub Actions 上稳定产出 4 个非原生 Linux CLI 目标：

- `aarch64-unknown-linux-gnu`
- `armv7-unknown-linux-gnueabihf`
- `x86_64-unknown-linux-musl`
- `aarch64-unknown-linux-musl`

这四个目标都由 `cross` 工具链驱动，`cross` 会在 GitHub hosted runner 上启动自己的构建容器。历史多次红灯集中在两个阶段：

1. `cross` 在容器构建阶段调用 Docker buildx / buildkit，`ubuntu-latest` runner 上的 buildkit socket 存在偶发挂起或版本兼容问题，未进入 Rust 编译就已失败。
2. `armv7-unknown-linux-gnueabihf` 的容器 pre-build 需要 `apt-get install clang libclang-dev`，容器基础镜像里 `/etc/apt/sources.list` 有畸形主机名 `archive.archive.ubuntu.com` / `security.archive.ubuntu.com`，加上上游镜像间歇 5xx，让 apt 直接失败。

同一套目标在 `release.yml` 里也会跑一遍，PR CI 修复必须同步到 release path，避免发布路径又踩同一个坑。

## 用户目标验证清单

### 必须实现

- `.github/workflows/ci.yml` 中 4 个 cross build job (`build-linux-aarch64`、`build-linux-armv7`、`build-linux-musl-x86`、`build-linux-musl-aarch64`) 的 `cross build` step 显式设置 `CROSS_CONTAINER_ENGINE_NO_BUILDKIT: "1"`，强制走传统 docker builder。
- `.github/workflows/release.yml` 对应 cross build step 设置同一变量，保持 PR CI 与 release 一致。
- `Cross.toml` 的 `[target.armv7-unknown-linux-gnueabihf].pre-build` 在安装 `clang` / `libclang-dev` 前，用 `sed -i` 把 `archive.archive.ubuntu.com` / `security.archive.ubuntu.com` 重写回标准 `archive.ubuntu.com` / `security.ubuntu.com`（保留 HTTP 协议）。
- pre-build 通过 `apt-get -o Acquire::Retries=5 update` 与 `apt-get -o Acquire::Retries=5 install ...` 增加内建重试；首次 install 失败自动 `apt-get update` 后 `--fix-missing` 重试一次。
- 4 个 cross build step 均用 `for attempt in 1 2 3` 循环，重试之间 `sleep $((attempt * 10))` 递增等待，第 3 次仍失败才 exit 1。

### 必须不破坏

- `Cross.toml` 的 `[build.env] passthrough` 保留 `SKIP_FRONTEND_BUILD` / `CARGO_TERM_COLOR`，因为 CLI job 通过这些变量跳过前端构建。
- `SKIP_FRONTEND_BUILD=1` 与 `CARGO_TARGET_DIR=target/cross-<target>` 环境不变，不影响 desktop bundle 的产物路径。
- `Swatinem/rust-cache@v2` 的 `key: linux-<target>` 保持不变，缓存命中率不下降。
- release workflow 的非 cross 目标（macOS / Windows）不改变原有 `dtolnay/rust-toolchain@stable` + `Swatinem/rust-cache` 布局。

### 必须真实验证

- 静态检查每个 cross build step YAML，`CROSS_CONTAINER_ENGINE_NO_BUILDKIT` 精确值为 `"1"`，且循环体存在 3 次重试与递增 sleep。
- 静态检查 `Cross.toml`，`sed` 命令覆盖 `/etc/apt/sources.list` 和 `/etc/apt/sources.list.d/*.list` 两个位置，重写目标是 `archive.ubuntu.com` / `security.ubuntu.com` 而非 https 变体。
- 推送分支后观察 GitHub Actions `CI` workflow，`Linux Build (armv7)` 不再于 buildkit 阶段红灯，且 apt 安装 clang 成功。
- release workflow 触发一次 dry-run tag（或 `workflow_dispatch`）验证 release cross build 也走 no-buildkit 路径。

## 产品语义

本设计只影响 CI/CD 自动化，不新增 CLI 命令、Admin API、Web UI 或磁盘状态。产品语义如下：

- 用户视角上，`bifrost` 在四个 Linux 非原生目标上的 release binary 与之前完全一致；只是 CI 更少红灯。
- 对 Fork / 二次开发者，`Cross.toml` 变更保留 armv7 目标的默认可用性，任何在 self-hosted runner 上跑 `cross build ... --target armv7-unknown-linux-gnueabihf` 的场景都能直接继承这段 pre-build。

## 技术细节

### CI workflow (`.github/workflows/ci.yml`)

4 个 job 的公共模板：

```yaml
build-linux-aarch64:      # job id
  name: Linux Build (aarch64)
  runs-on: ubuntu-latest
  steps:
    - uses: actions/checkout@v4
    - uses: dtolnay/rust-toolchain@stable
      with: { targets: aarch64-unknown-linux-gnu }
    - name: Install cross
      uses: taiki-e/install-action@v2
      with: { tool: cross }
    - uses: Swatinem/rust-cache@v2
      with: { key: linux-aarch64 }
    - name: Build Linux CLI (aarch64)
      env:
        SKIP_FRONTEND_BUILD: "1"
        CARGO_TARGET_DIR: target/cross-aarch64
        CROSS_CONTAINER_ENGINE_NO_BUILDKIT: "1"
      run: |
        for attempt in 1 2 3; do
          echo "cross build attempt ${attempt}/3"
          if cross build -p bifrost-cli --release --target aarch64-unknown-linux-gnu; then
            exit 0
          fi
          if [ "$attempt" = "3" ]; then
            exit 1
          fi
          sleep $((attempt * 10))
        done
```

不同 job 差异：

| Job | Runner | Target | Cache key | `CARGO_TARGET_DIR` |
| --- | --- | --- | --- | --- |
| `build-linux-aarch64` | `ubuntu-latest` | `aarch64-unknown-linux-gnu` | `linux-aarch64` | `target/cross-aarch64` |
| `build-linux-armv7` | `ubuntu-latest` | `armv7-unknown-linux-gnueabihf` | `linux-armv7` | `target/cross-armv7` |
| `build-linux-musl-x86` | `ubuntu-latest` | `x86_64-unknown-linux-musl` | `linux-musl-x86_64` | `target/cross-musl-x86_64` |
| `build-linux-musl-aarch64` | `ubuntu-latest` | `aarch64-unknown-linux-musl` | `linux-musl-aarch64` | `target/cross-musl-aarch64` |

`build-linux-armv7` 的 `Install cross` step 特殊——用 `cargo install cross --git https://github.com/cross-rs/cross` 安装最新 `cross`，其余三个 job 复用 `taiki-e/install-action@v2` 的 prebuilt binary。armv7 需要 upstream 最新 pre-build 支持。

### Release workflow (`.github/workflows/release.yml`)

Release 的 Linux matrix cross build step 使用相同 `CROSS_CONTAINER_ENGINE_NO_BUILDKIT: "1"` env，位于 release 的 CLI packaging job。

### `Cross.toml`

完整内容：

```toml
[build.env]
passthrough = [
    "SKIP_FRONTEND_BUILD",
    "CARGO_TERM_COLOR",
]

[target.armv7-unknown-linux-gnueabihf]
pre-build = [
    "set -eux; sed -i -e 's|http://archive.archive.ubuntu.com/ubuntu|http://archive.ubuntu.com/ubuntu|g' -e 's|http://security.archive.ubuntu.com/ubuntu|http://security.ubuntu.com/ubuntu|g' /etc/apt/sources.list /etc/apt/sources.list.d/*.list 2>/dev/null || true; apt-get -o Acquire::Retries=5 update; apt-get -o Acquire::Retries=5 install -y --no-install-recommends clang libclang-dev || (apt-get -o Acquire::Retries=5 update && apt-get -o Acquire::Retries=5 install -y --fix-missing --no-install-recommends clang libclang-dev)",
]
```

`sed` 参数说明：

- `-i` in-place。
- 双 `-e`：一次改 `archive`, 一次改 `security`；两条 URL 都是 HTTP 而非 HTTPS，避免额外 CA 依赖。
- 目标文件是 `/etc/apt/sources.list` 与 `/etc/apt/sources.list.d/*.list`；`2>/dev/null || true` 保证 shard 内可能不存在 `sources.list.d` 时不失败。

`apt-get` 参数说明：

- `-o Acquire::Retries=5`：单个包索引 / 下载失败时最多重试 5 次。
- `--no-install-recommends`：只装必需的 clang / libclang-dev。
- fallback 分支中的 `--fix-missing`：允许 partial 包索引下继续尝试。

## CLI / Web / Admin API / Sync 边界

- CLI：无新增子命令与 flag。
- Web：无 UI 变化。
- Admin API：无新增或修改端点。
- Sync：不影响 sync payload / API 契约。

## 实现切分

### Phase 1 — CI YAML 主线

- 在 4 个 `build-linux-*` job 的 `env` 段加入 `CROSS_CONTAINER_ENGINE_NO_BUILDKIT: "1"`。
- 把 `cross build ...` 单行改成 3 次重试循环，保留原 target 与 package flag。

### Phase 2 — Release 同步

- 在 `release.yml` matrix cross build step 同步加入 `CROSS_CONTAINER_ENGINE_NO_BUILDKIT: "1"`。
- 如果 release 存在 `cross build` 单行调用，也换成同一 3 次重试循环。

### Phase 3 — Cross.toml pre-build

- 更新 `Cross.toml` 的 armv7 `pre-build`，加入 `sed` 主机名重写与 `apt-get` retry + fallback。
- 通过 `cross build -p bifrost-cli --release --target armv7-unknown-linux-gnueabihf` 本地或 self-hosted runner 验证一遍。

### Phase 4 — 文档与索引

- 更新 `human_tests/ci-cross-build.md` 与 `human_tests/readme.md`。
- 不新增 CLI/README 章节，因为无用户可见变化。

## 测试方案

### 单元测试

CI YAML / `Cross.toml` 修改无 Rust 公共函数变更，不新增 Rust 单元测试。

### 集成 & E2E 测试

- 静态检查 `.github/workflows/ci.yml`：`grep -c 'CROSS_CONTAINER_ENGINE_NO_BUILDKIT: "1"'` == 4。
- 静态检查 `.github/workflows/release.yml`：同一断言在 release cross build step 存在。
- 静态检查 `Cross.toml`：包含 `archive.ubuntu.com` / `security.ubuntu.com` / `Acquire::Retries=5` / `--fix-missing`。
- 静态检查 4 个 cross build step 中每一段 `run` 都包含 `for attempt in 1 2 3` 与 `sleep $((attempt * 10))`。
- 云端：观察 `CI` workflow 全绿，尤其 `Linux Build (armv7)` 不再在 buildkit / apt 阶段红灯。

### human_tests

- 更新 `human_tests/ci-cross-build.md`，覆盖：
  1. CI 与 release 4 个 cross build step 的 `CROSS_CONTAINER_ENGINE_NO_BUILDKIT` 断言。
  2. `Cross.toml` 主机名重写 + apt retry + fallback 断言。
  3. CI 4 段 cross build step 3 次重试循环断言。
- `human_tests/readme.md` 索引行同步更新（禁止维护全局用例数）。

## Review / Fix / Test 闭环

1. 第 1 轮：diff CI / release / Cross.toml 三处修改，跑静态断言脚本；PR 推送后观察 `CI` workflow。
2. 第 2 轮：若 armv7 apt 仍失败，追加 `sleep` 或改 mirror；若 buildkit 报错继续，检查 `cross` 版本是否需要 pin。
3. 第 3 轮：追加 release path 观察，确保 release tag 触发后 4 个 cross build 均绿。

## 风险与决策

- 决策：强制 `CROSS_CONTAINER_ENGINE_NO_BUILDKIT=1` 而不是升级 buildkit——GitHub hosted runner 无法保证 buildkit 版本，禁用是最稳的解耦方案。
- 决策：不切换到官方 `docker/build-push-action` 自建镜像——那会引入新的 registry 依赖，且 `cross` 会重复解析 target 元数据。
- 风险：Ubuntu 上游 mirror 若长期挂掉，`Acquire::Retries=5 + --fix-missing` 仍无法救活，届时需要在 `Cross.toml` 里换镜像源；本设计不预置切换，因为切镜像会让本地 debug 与 CI 行为分叉。
- 风险：3 次重试意味着最坏情况 job 时间 3 倍。目前 4 个 job 单次 <10min，可接受。

## 依赖项

- `.github/workflows/ci.yml`
- `.github/workflows/release.yml`
- `Cross.toml`
- `human_tests/ci-cross-build.md`
- `human_tests/readme.md`

## 校验要求

- `git diff --check -- .github/workflows/ci.yml .github/workflows/release.yml Cross.toml design/ci-cross-build.md human_tests/ci-cross-build.md human_tests/readme.md`
- 静态断言 4 个 cross build step + release cross build step 的 `CROSS_CONTAINER_ENGINE_NO_BUILDKIT` 与 3 次重试循环。
- GitHub Actions `CI` workflow 全绿。

## 文档更新要求

- 更新 `human_tests/ci-cross-build.md`
- 更新 `human_tests/readme.md`
