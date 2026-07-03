---
title: e2e-test 快速构建启动说明
status: active
---

# e2e-test 快速构建启动说明

## 背景

Bifrost workspace 默认 `cargo build --workspace` 或 `cargo run -p bifrost-cli` 会触发 `bifrost-cli`
的 `build.rs`，在编译期把 `web/dist` 前端静态资源打包进二进制。全量前端构建（`pnpm -C web build`）
在本地约需 30–90 秒，甚至更久；对于只关心后端行为（proxy、rules、admin API、agent、traffic、sync
push 等）的 e2e 场景，这段前端构建纯属浪费。

`Makefile` 已经在 `build-backend` 与 `dev` 目标中通过 `SKIP_FRONTEND_BUILD=1` 显式跳过前端打包，
但历史上 e2e-test 技能文档没有清晰指引，导致开发者在本地反复触发前端构建，也会让 CI 上的
"cargo build 之后立刻起服务"路径出现分歧：有些 job 跳过前端，有些没跳过。

本文档定义 e2e-test 技能中"快速构建启动"的推荐用法，包括：

- 后端-only 构建命令；
- 后端 + 前端 hot reload 联调命令；
- 与 `Makefile` 目标的对应关系；
- 与真实 E2E 启动脚本（`process.sh`、`admin_client.sh`、`run_all_e2e.sh`）的边界。

## 用户目标验证清单

### 必须实现

- 在 `.agents/skills/e2e-test/SKILL.md` 与 `.agents/skills/e2e-test/01-快速构建启动.md` 中提供
  "快速构建启动（推荐）"章节，明确 `SKIP_FRONTEND_BUILD=1 cargo build --workspace` 的用法。
- 文档同时展示等价的 `make build-backend` 与 `make dev` 命令，避免开发者在两套写法间猜测。
- 快速构建启动步骤要求先编译、再用编译产物启动新进程；不允许复用旧进程做结论。
- 快速构建启动步骤要求使用临时 `BIFROST_DATA_DIR`（例如 `./.bifrost-e2e-test`）、非 9900 端口、
  显式 `--unsafe-ssl` 或按 case 加载证书。
- 文档同时说明 `pnpm -C web dev -- --backend-port <port>` 的 UI 联调启动方式，与后端 `SKIP_FRONTEND_BUILD=1` 联合使用。
- 文档说明清理规范（`rm -rf "$BIFROST_DATA_DIR"`），避免残留占用端口或数据。

### 必须不破坏

- Makefile 中 `build-backend`、`build-frontend`、`build`、`dev` 目标的行为保持不变。
- 生产构建路径（`cargo build --release` / `make build`）仍会打包前端资源，避免影响 release binary。
- 静态资源在 CI 完整构建（Actions、release job）中仍完整打包，不受 skill 文档变更影响。
- 已有 E2E helper（`e2e-tests/test_utils/process.sh` 等）不新增 `SKIP_FRONTEND_BUILD` 依赖；
  helper 仍假设调用方已完成 build 或依赖 `run_all_e2e.sh` 的 build 步骤。

### 必须真实验证

- 本地执行 `SKIP_FRONTEND_BUILD=1 cargo build --workspace` 成功产出 `target/debug/bifrost` 且未触发 `pnpm -C web build`。
- 本地执行 `make dev` 可以启动后端并在 stderr 中出现 `Proxy is up and running`，同时 `pnpm dev` 端口
  提供的 UI 能通过 `/_bifrost/api` 访问后端。
- 抽查一次真实 E2E 用例（例如 `bash e2e-tests/tests/test_rules_basic.sh` 或 `bash scripts/run_all_e2e.sh --list`），
  确认在 skill 文档定义的快速构建启动路径下测试可运行。

## 产品语义

### 快速构建启动是"迭代默认路径"

日常调试后端时，开发者应默认使用 `SKIP_FRONTEND_BUILD=1 cargo build --workspace`（或等价 `make build-backend`），
只在需要验证前端资源被打包进二进制的场景才用 `make build` 触发完整前端构建。

### `make dev` 是"UI 联调路径"

需要同时看到真实 UI 变更时，`make dev` 会在后台构建前端并启动带 hot reload 的 devserver，同时
以 `SKIP_FRONTEND_BUILD=1` 的方式启动后端；开发者通过 `pnpm -C web dev -- --backend-port <port>`
指定后端端口。

### 与 E2E helper 的关系

E2E helper（`process.sh` / `admin_client.sh` / `run_all_e2e.sh`）不负责 build，只负责启动、
探活与清理。快速构建启动的文档面向"开发者手动跑单个 case 或做本地实验"的场景，helper 则面向
"批量 CI 执行"。两者共享同一份后端二进制约定，但分工清晰。

## 技术细节

### `SKIP_FRONTEND_BUILD` 生效范围

在 `bifrost-cli/build.rs` 中检测到 `SKIP_FRONTEND_BUILD=1` 时跳过 `pnpm -C web build`。
`web/dist` 若不存在，binary 内嵌前端资源会退化为 404 或 empty；此路径只在后端-only 场景使用。

### 推荐命令

后端-only 迭代：

```bash
SKIP_FRONTEND_BUILD=1 cargo build --workspace
# 等价
make build-backend
```

后端 + 前端 hot reload：

```bash
make dev
# 等价
SKIP_FRONTEND_BUILD=1 cargo run -p bifrost-cli -- start --verbose
pnpm -C web dev -- --backend-port <port>
```

### 启动前置

- 从 `~/.bifrost/certs/` 复制证书到临时数据目录（例如 `./.bifrost-e2e-test/certs`），
  避免 CA 交互界面阻塞。
- 使用 `BIFROST_DATA_DIR=./.bifrost-e2e-test`、`-p 8890` 等非默认端口，避免与本地已运行的 Bifrost 冲突。
- `--unsafe-ssl` 用于跳过上游 TLS 校验；场景需要合法证书时应换成正常配置。

### 启动后校验

- `lsof -nP -iTCP:8890 -sTCP:LISTEN` 确认监听。
- `curl -sS http://127.0.0.1:8890/_bifrost/api/proxy/address` 确认 admin API 可用。
- 若进入 CA 交互界面，先选择并等待 `Proxy is up and running` 状态。

## CLI / 脚本 / CI 集成

- `Makefile` 保持 `build-backend`（`SKIP_FRONTEND_BUILD=1 cargo build --workspace`）与
  `dev`（`SKIP_FRONTEND_BUILD=1 cargo run -p bifrost-cli -- start --verbose`）目标。
- `scripts/run_all_e2e.sh` 在 CI 场景已使用 `SKIP_FRONTEND_BUILD=1`；本地开发者按 skill 文档执行。
- CI 单独的 `build-frontend` job 保留完整前端产物，rules E2E job 复用 `SKIP_FRONTEND_BUILD=1` 快速构建的
  CLI artifact，只关心后端行为。
- 手工验证：
  ```bash
  SKIP_FRONTEND_BUILD=1 cargo build --workspace
  BIFROST_DATA_DIR=./.bifrost-e2e-test ./target/debug/bifrost start -p 8890 --unsafe-ssl
  ```

## Sync 边界

- 快速构建启动只影响本地构建产物打包内容，不影响 Sync 客户端和服务端契约。
- 若某个 E2E 场景需要 admin UI 静态资源（例如 `test_admin_ui_bootstrap.sh`），必须显式声明
  依赖完整前端 build 或跳过快速构建启动；skill 文档在相关章节明确此约束。

## 实现切分

### Phase 1：Makefile 与 skill 骨架

- 保留 `Makefile` 中 `build-backend` / `dev` 目标定义。
- 在 `.agents/skills/e2e-test/SKILL.md` 与 `01-快速构建启动.md` 提供快速构建启动章节。

### Phase 2：脚本与 CI 对齐

- `scripts/run_all_e2e.sh` 在 CI 上使用 `SKIP_FRONTEND_BUILD=1`；rules job 依赖同一构建产物。
- 抽查 `scripts/ci/*.sh` 保持一致，不出现漏跳前端的 job。

### Phase 3：文档补齐

- skill 子文档补齐 UI 联调与清理章节。
- `human_tests/readme.md` 索引指向 skill 快速构建启动。

### Phase 4：真实回归

- 抽查一次 rules E2E 与 admin UI dev 联调，确认命令仍可运行。

## 测试方案

### 单元测试

- 无直接 Rust 单元测试。行为由 `Makefile` 目标 + skill 文档共同定义。

### E2E 测试

- 抽查 `bash e2e-tests/tests/test_rules_basic.sh` 在 `SKIP_FRONTEND_BUILD=1 cargo build --workspace`
  产出的 CLI 上运行成功。
- 抽查 `pnpm -C web dev -- --backend-port 8890` 与 `make dev` 联合启动后 UI 页面可正常访问。

### 真实场景测试 human_tests

- 更新 `human_tests/e2e-test-fast-build.md`（可与 skill 文档索引联动），覆盖：
  - TC-EFB-01：`SKIP_FRONTEND_BUILD=1 cargo build --workspace` 快速构建。
  - TC-EFB-02：`make dev` 后端 + 前端联调。
  - TC-EFB-03：`bifrost start` 启动后 `lsof` / `curl /_bifrost/api/proxy/address` 校验。
- 所有 case 使用临时 `BIFROST_DATA_DIR`、非 9900 端口、`BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`、
  `BIFROST_DISABLE_TRAY=1`、`--no-system-proxy`。

### 覆盖率与项目校验

- `cargo fmt --all -- --check`
- `bash -n Makefile` 不适用，改为抽查 `make -n build-backend` / `make -n dev` 输出与文档一致。
- `rust-project-validate`

本机 no-local-coverage 约定下不运行 `make coverage`；交付时说明豁免。

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核 skill 文档是否与 Makefile 目标一一对齐，命令是否可复制执行。
- 复核 skill 子文档是否覆盖清理步骤。
- 抽查一次 `SKIP_FRONTEND_BUILD=1 cargo build --workspace` 与 `make dev`。

### 第 2 轮

- 复核 CI 是否已在 rules job 使用同一构建路径，避免文档与 CI 分叉。
- 复核 human_tests 索引。
- 抽查一次 rules E2E 与 UI 联调。

## 风险与决策点

- **前端资源过期**：`SKIP_FRONTEND_BUILD=1` 会保留旧 `web/dist`；若开发者只关心后端行为可接受，
  UI 联调时使用 `pnpm -C web dev` 覆盖。
- **release 场景误用**：不允许在 release 构建路径使用 `SKIP_FRONTEND_BUILD`，需要通过 skill 文档
  与 Makefile 注释共同强调。
- **skill 文档漂移**：Makefile 与 skill 文档要同步更新；后续任何构建路径调整必须双写。
