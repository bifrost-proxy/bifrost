# Rust Dependency Audit CI

## 功能模块说明

本模块用于在本地和 CI 中检查 Rust 依赖膨胀风险，覆盖两类问题：

- 未使用的直接依赖：通过 `cargo-udeps` 基于编译器元数据检查，避免纯字符串匹配误伤。
- 重复版本依赖：通过 `cargo-deny` 的 `bans` 检查持续暴露重复版本，但当前配置为 warning，避免对大量传递依赖做高风险的一次性强制收敛。

## 实现逻辑

1. `scripts/ci/rust-dependency-audit.sh` 统一执行依赖审计。
2. 脚本显式检查 `cargo`、`cargo-deny`、`cargo-udeps`、`rustup` 与 nightly toolchain，工具缺失时直接失败并给出错误。
3. `cargo deny check bans --hide-inclusion-graph` 使用 `deny.toml` 读取 all-features graph，并把重复版本设为 warning。
4. `cargo udeps --workspace --all-targets --all-features --locked` 使用 nightly toolchain 和 `SKIP_FRONTEND_BUILD=1`，基于真实编译图发现未使用直接依赖。
5. `.github/workflows/ci.yml` 增加 `dependency-audit` job，先安装固定版本 `cargo-deny` 与 `cargo-udeps`，再执行统一脚本。
6. `scripts/ci/local-ci.sh` 增加同一审计步骤，并提供 `--skip-deps-audit` 供本地快速验证或工具链缺失时显式跳过。

## 依赖项调整

本轮只处理工具确认且人工复核后低风险的直接依赖：

- 删除未使用的 dev-dependency：`tokio-test`、`env_logger`。
- 删除未使用的直接 dependency：`is-terminal`、`async-compression`、`bifrost-core` in `bifrost-power`。
- 把平台相关误报改为 target-specific：`netstat2` 仅非 macOS 编译，`sha1` 仅 Windows 编译。
- `bifrost-proxy` 原 `tokio-test` 只用于间接启用 `tokio/test-util`，改为显式 dev-dependency `tokio` + `test-util` feature，避免隐藏的 feature 依赖。

未在本轮强行收敛重复传递依赖版本。重复版本涉及多个上游 crate 版本链，直接统一容易改变编译图和运行行为，当前先通过 CI warning 持续观测。

## 测试方案

### 单元测试

本改动不新增 Rust 运行时代码，单元测试通过 workspace 聚合命令覆盖：

- `cargo test --workspace --all-features`

### E2E 测试

本改动不修改代理运行时、CLI 行为、WebUI 或网络协议，运行时 E2E 不适用。CI/local-ci 集成通过脚本级真实执行验证。

### 真实场景测试

对应用例文档：

- `human_tests/rust-dependency-audit-ci.md`

覆盖内容：

- 直接执行依赖审计脚本。
- 通过 `local-ci --skip-e2e` 验证审计集成。
- 通过 `local-ci --skip-e2e --skip-deps-audit` 验证显式跳过。
- 通过裁剪 `PATH` 验证工具缺失时失败且提示清晰。

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核用户目标：清理未使用依赖、检查重复依赖、接入 CI、推 PR。
- 复核 diff：Cargo manifest、lockfile、CI/local-ci、设计文档、human_tests。
- 执行脚本级验证：依赖审计脚本、shell 语法、工具缺失路径。
- 修复发现的 manifest 或脚本问题后复跑。

### 第 2 轮

- 复查第 1 轮修复后的完整 diff。
- 复查 target-specific 依赖没有误删跨平台依赖。
- 复跑依赖审计和 local-ci 相关路径。
- 确认没有需要第 3 轮的阻塞问题。

## 校验要求

- `bash scripts/ci/rust-dependency-audit.sh`
- `bash -n scripts/ci/rust-dependency-audit.sh`
- `bash -n scripts/ci/local-ci.sh`
- `cargo test --workspace --all-features`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `scripts/ci/local-ci.sh --skip-e2e`

## 文档更新要求

- 新增本设计文档。
- 新增 `human_tests/rust-dependency-audit-ci.md`。
- 保持 `human_tests/readme.md` 索引包含该用例。

## 2026-06-17 Dependabot Rust Security and Quality 修复

### 告警来源与权限边界

用户入口为 GitHub Security and quality 的 Dependabot Rust open alerts 页面。当前 `GITHUB_TOKEN` 调用 GitHub Dependabot alerts API 返回 `403 Resource not accessible by personal access token`，因此本轮以 `cargo audit` 对根 `Cargo.lock` 与 `desktop/src-tauri/Cargo.lock` 做本地 advisory-backed 复核，并把 API 权限不足作为交付证据记录。

### 已关闭的 Security 告警

根工作区与桌面 Tauri 锁文件均已通过 `cargo audit --no-fetch` 验证 `vulnerabilities=0`。

- `RUSTSEC-2026-0119` / `GHSA-q2qq-hmj6-3wpp`：`hickory-proto 0.24.4` CPU exhaustion。修复方式：`bifrost-proxy` 将 `hickory-resolver` 从 `0.24` 升级到 `0.26.1`，带入 `hickory-proto 0.26.1`。
- `RUSTSEC-2022-0013` / `CVE-2022-24713` / `GHSA-m5pq-gvj9-9vr8`：`regex 0.2.11` DoS。修复方式：vendor `sysproxy 0.3.0` 并只把 macOS target 的 `interfaces` 从 `0.0.8` 升级到 `0.0.9`，移除 `interfaces -> handlebars 0.29 -> regex 0.2` 链。
- `RUSTSEC-2022-0006` / `GHSA-9hpw-r23r-xgm5`：`thread_local 0.3.6` data race。修复方式同上，移除旧 `regex 0.2` 传递链。
- `RUSTSEC-2026-0104` / `GHSA-82j2-j2ch-gfr8`：`rustls-webpki 0.103.10` CRL parsing panic。修复方式：根与桌面锁文件均升级到 `rustls-webpki 0.103.13`。
- `RUSTSEC-2026-0098` / `GHSA-965h-392x-2mh5`：`rustls-webpki 0.103.10` URI name constraints。修复方式同上。
- `RUSTSEC-2026-0099` / `GHSA-xgp8-3hg3-c2mh`：`rustls-webpki 0.103.10` wildcard name constraints。修复方式同上.

### 已处理的 Quality 告警

- `RUSTSEC-2026-0097` / `GHSA-cq8v-f236-94qc`：根工作区 `rand 0.8.5` 与 `rand 0.9.2` unsound。修复方式：workspace `rand` 下限升到 `0.8.6`，锁文件中 `rand 0.9` 升到 `0.9.4`，`hickory` 新链使用 `rand 0.10.1`。桌面端仍有 Tauri build-time `rand 0.7.3`，见剩余项。
- `RUSTSEC-2026-0002` / `GHSA-rhfx-m35p-ff5j`：`lru 0.12.5` unsound。修复方式：`ratatui` 从 `0.29` 升级到 `0.30.1`，带入 `lru 0.18.0`。
- `RUSTSEC-2017-0008`：`serial 0.4.0` unmaintained。修复方式：`portable-pty` 从 `0.8` 升到 `0.9`，移除 `serial` 链。
- `RUSTSEC-2025-0134`：`rustls-pemfile` unmaintained。修复方式：`bifrost-tls` 改用 `rustls::pki_types::pem::PemObject` 解析 PEM，移除直接依赖。
- `RUSTSEC-2025-0141`：`bincode` unmaintained。修复方式：Traffic DB detail blob 改用 `postcard`（紧凑二进制）写入与读取，移除 `bincode` 依赖。`postcard` 以 `default-features = false` + `alloc` 引入，避免默认 `heapless-cas` feature 带入 `heapless -> atomic-polyfill`（RUSTSEC-2023-0089），保证不新增 advisory warning。

### 剩余 informational quality 告警

以下项仍由上游或平台栈传递引入，本轮不通过 ignore 绕过，也不做大范围替换：

- GTK3 / `glib 0.18.5` / `proc-macro-error 1.0.4`：来源为 Linux tray/window stack。根工作区当前路径为 `tray-icon 0.19.3`、`muda 0.15.3`、`tao 0.31.1 -> gtk 0.18.2 -> glib 0.18.5`；桌面路径为 Tauri/Wry Linux WebKitGTK 与 GTK3 stack。升级 CLI tray/window stack 无法安全关闭 `glib`，且会改变 Windows 托盘/启动行为，需要等待上游迁移或单独托盘/桌面专项改造。
- `fxhash 0.2.1`：来源为 `bm25 2.3.2`，用于 Agent tool search。当前 `bm25` 无更高兼容版本可直接升级。
- `paste 1.0.15`：来源包括 ASR `qwen3-asr/candle`、`tokenizers` 与 Linux `netstat2` 链，属于上游传递依赖。
- `proc-macro-error2 2.0.1`：来源为 `local-ip-address 0.6.13 -> neli -> getset`，已升级到当前 latest 但上游仍带入该 proc-macro。
- 桌面 `rand 0.7.3`：来源为 `tauri-utils -> kuchikiki -> selectors -> phf_codegen 0.8` build-time 链，需等待 Tauri/上游迁移或单独桌面依赖升级。
- 桌面 `unic-* 0.9.0`：来源为 Tauri/WebKitGTK 相关 Linux stack，属于桌面平台传递依赖。

### 运行时行为变更

Traffic DB 新写入的 detail blobs 从 bincode 二进制编码改为 postcard 紧凑二进制编码。数据库 schema 不变，旧 bincode blob 在读取时会按既有容错路径解析失败并返回空 detail 字段；新写入记录由 `cargo test -p bifrost-admin traffic_db --lib` 覆盖读写路径。选型理由：postcard 序列化/反序列化性能接近 bincode 2.x（实测单条 detail 字段 ser+de 约 2.4 µs，约为 JSON 的一半，体积约为 JSON 的 65%），且 postcard 活跃维护、无安全告警；bincode 2.x 因 RUSTSEC-2025-0141 覆盖全版本（no patched versions）不可用。需要说明的是，detail 序列化只占一次 SQLite 单事务 commit（约 20 µs）的个位数百分比，因此该改动对端到端写入吞吐影响很小，主要收益是统一二进制编码与体积下降。该兼容性取舍用于彻底移除 unmaintained `bincode` 依赖。

### 验证方案补充

- Security 审计：根 `cargo audit --json --no-fetch` 与桌面 `cargo audit --file desktop/src-tauri/Cargo.lock --json --no-fetch` 必须均为 `vulnerabilities=0`。
- Targeted 单测：`cargo test -p bifrost-tls` 覆盖 PEM 解析迁移；`cargo test -p bifrost-admin traffic_db --lib` 覆盖 Traffic DB postcard detail blob 读写；`cargo check -p bifrost-agent -p bifrost-admin` 覆盖 `portable-pty 0.9` 和 admin 网络接口依赖升级。
- E2E smoke：使用临时 `BIFROST_DATA_DIR`、`BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`、`BIFROST_DISABLE_TRAY=1` 和 `--no-system-proxy` 启动最新编译的 `bifrost`，验证 Admin API ready。
- Coverage 90% 门禁：收尾执行 `make coverage`；若 E2E coverage 环境不可用，按项目规则退化为 `make coverage-unit` 并记录原因。

## 2026-06-19 Dependabot Rust open alerts 复核

### 告警来源

本轮通过 GitHub Dependabot alerts API 分页读取 `bifrost-proxy/bifrost` 的 open alerts，并在本地筛选 `dependency.package.ecosystem == "rust"`。API 返回 15 条 Rust open alerts，集中在 7 个依赖包：

- 根 `Cargo.lock`：`glib`、`jsonwebtoken`、`tar`。
- `crates/bifrost-admin/Cargo.toml`：`jsonwebtoken`。
- `desktop/src-tauri/Cargo.lock`：`glib`、`openssl`、`rand`、`tauri`。

### 已修复项

- `jsonwebtoken` / `GHSA-h395-gr6q-cpjc` / `CVE-2026-25537`：`bifrost-admin` 从 `jsonwebtoken 9.3.1` 升级到 `10.4.0`。`jsonwebtoken` 10 需要显式 crypto provider，本项目只使用 HS256，选择 `aws_lc_rs` provider，避免 `rust_crypto` provider 额外引入 `rsa 0.9.10` 的 `RUSTSEC-2023-0071` timing side-channel 漏洞。
- `tar` / `GHSA-3pv8-6f4r-ffg2`：根锁文件从 `tar 0.4.45` 升级到 `0.4.46`。
- `openssl` / `GHSA-ghm9-cr32-g9qj`、`GHSA-hppc-g8h3-xhp3`、`GHSA-8c75-8mhr-p7r9`、`GHSA-xmgf-hq76-4vx2`、`GHSA-pqf5-4pqq-29f5`、`GHSA-xp3w-r5p5-63rr`、`GHSA-xv59-967r-8726`、`GHSA-phqj-4mhp-q6mq`：desktop 锁文件从 `openssl 0.10.75` 升级到 `0.10.80`。
- `tauri` / `GHSA-7gmj-67g7-phm9` / `CVE-2026-42184`：desktop `tauri` 约束提升到 patched floor `2.11.1`，锁文件解析到 `2.11.3`。
- `rand` / `GHSA-cq8v-f236-94qc`：desktop Tauri 升级移除了 `tauri-utils -> kuchikiki -> selectors -> phf_codegen 0.8 -> rand 0.7.3` 链；desktop 锁文件仅剩 `rand 0.8.6` 与 `0.9.4`。

### 未能在本轮安全关闭的项

- `glib` / `GHSA-wrw7-89jp-8q8g`：根与 desktop 锁文件仍包含 `glib 0.18.5`。根路径来自 `bifrost-cli` 的 `tray-icon 0.19.3`、`muda 0.15.3`、`tao 0.31.1 -> gtk 0.18.2 -> glib 0.18.5`；desktop 路径来自 Tauri/Wry Linux WebKitGTK 与 GTK3 stack。强行 patch 到 `glib 0.20` 会跨 gtk-rs major version，无法作为安全的锁文件修复；移除或大幅升级 `tao` / tray stack / Tauri Linux WebView 依赖会改变托盘、窗口或桌面功能边界，需单独功能迁移设计。

### Review/Fix/Test 闭环方案补充

第 1 轮复核范围：

- 用户目标：Rust Dependabot open alerts 逐项分析，能修复项全部修复，无法修复项给出上游路径和风险。
- 变更范围：根/desktop 锁文件、`bifrost-admin` JWT provider、CLI 托盘依赖 features、desktop Tauri patched floor、design/human_tests。
- 验证命令：`cargo audit` 根和 desktop、`cargo test -p bifrost-admin admin_auth --lib`、`cargo check -p bifrost-admin --all-features`、新增 human_tests 用例。

第 2 轮复核范围：

- 确认修复没有引入新的 `cargo audit vulnerabilities`，尤其验证 `jsonwebtoken` provider 不再引入 `rsa` advisory。
- 复核 `glib` 剩余路径是否确实来自上游 GTK/Tauri stack，且没有通过新增 ignore 隐藏。
- 复跑受影响测试与审计命令，并进入 `rust-project-validate`、coverage 90% 门禁与远端 CI 看护。
