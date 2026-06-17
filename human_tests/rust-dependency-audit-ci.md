# Rust Dependency Audit CI 真实场景测试

## 功能模块说明

验证 Rust 依赖审计能力：未使用依赖通过 `cargo-udeps` 检查，重复版本依赖通过 `cargo-deny` 检查，并接入 GitHub Actions 与本地 `local-ci`。同时覆盖 Dependabot Security and quality 中 Rust 告警的本地修复回归：根工作区与桌面锁文件必须没有 Security vulnerabilities，可安全处理的 Quality 告警必须从依赖图移除，剩余 informational warning 必须有明确上游归因。

## 前置条件

1. 当前目录为仓库根目录。
2. 已安装 Rust stable 与 nightly toolchain。
3. 已安装 `cargo-deny` 与 `cargo-udeps`。
4. 已安装 `cargo-audit`。
5. 当前工作区依赖已可正常解析。

## 测试用例列表

### TC-RDA-01 直接执行 Rust 依赖审计脚本

操作步骤：

1. 执行 `bash scripts/ci/rust-dependency-audit.sh`。

预期结果：

1. 命令退出码为 0。
2. 输出包含 `Checking duplicate Rust dependencies with cargo-deny`。
3. 输出包含 `Checking unused Rust dependencies with cargo-udeps`。
4. `cargo-udeps` 输出未报告 unused dependencies。

### TC-RDA-02 local-ci 默认静态路径执行依赖审计

操作步骤：

1. 执行 `bash scripts/ci/local-ci.sh --skip-e2e`。

预期结果：

1. 命令退出码为 0。
2. Local CI Report 中包含 `Rust dependency audit`。
3. `Rust dependency audit` 结果为 PASS。

### TC-RDA-03 local-ci 可显式跳过依赖审计

操作步骤：

1. 执行 `bash scripts/ci/local-ci.sh --skip-e2e --skip-deps-audit`。

预期结果：

1. 命令退出码为 0。
2. Local CI Report 中包含 `Rust dependency audit (skipped)`。
3. 其他静态检查仍正常执行。

### TC-RDA-04 工具缺失时给出明确错误

操作步骤：

1. 执行 `PATH="/usr/bin:/bin" bash scripts/ci/rust-dependency-audit.sh`。

预期结果：

1. 命令退出码非 0。
2. stderr 输出包含 `required command not found`。
3. 错误信息包含缺失工具名称。

### TC-RDA-05 根工作区 Rust Security 告警清零

操作步骤：

1. 执行 `cargo audit --json --no-fetch > /tmp/bifrost-root-audit.json`。
2. 执行 `python3 - <<'PY'
import json
data = json.load(open('/tmp/bifrost-root-audit.json'))
print(len(data['vulnerabilities']['list']))
PY`。

预期结果：

1. `cargo audit` 命令退出码为 0。
2. Python 输出为 `0`。
3. 审计结果不再包含 `RUSTSEC-2026-0119`、`RUSTSEC-2022-0013`、`RUSTSEC-2022-0006`、`RUSTSEC-2026-0104`、`RUSTSEC-2026-0098` 或 `RUSTSEC-2026-0099`。

### TC-RDA-06 桌面 Tauri 锁文件 Rust Security 告警清零

操作步骤：

1. 执行 `cargo audit --file desktop/src-tauri/Cargo.lock --json --no-fetch > /tmp/bifrost-desktop-audit.json`。
2. 执行 `python3 - <<'PY'
import json
data = json.load(open('/tmp/bifrost-desktop-audit.json'))
print(len(data['vulnerabilities']['list']))
PY`。

预期结果：

1. `cargo audit` 命令退出码为 0。
2. Python 输出为 `0`。
3. 审计结果不再包含 `rustls-webpki 0.103.10` 相关 security advisories。

### TC-RDA-07 可安全处理的 Rust Quality 告警已从根依赖图移除

操作步骤：

1. 执行 `cargo tree -i bincode`。
2. 执行 `cargo tree -i rustls-pemfile`。
3. 执行 `cargo tree -i serial`。
4. 执行 `cargo tree -i lru@0.12.5`。
5. 执行 `cargo tree -i rand@0.8.5`。
6. 执行 `cargo tree -i rand@0.9.2`。

预期结果：

1. 每条命令均以非 0 退出或输出明确说明没有匹配 package。
2. 根依赖图中不再包含 `bincode`、`rustls-pemfile`、`serial`、`lru 0.12.5`、`rand 0.8.5` 或 `rand 0.9.2`。

### TC-RDA-08 剩余 Rust Quality warning 有明确分类

操作步骤：

1. 执行 `cargo audit --json --no-fetch > /tmp/bifrost-root-audit.json`。
2. 执行 `cargo audit --file desktop/src-tauri/Cargo.lock --json --no-fetch > /tmp/bifrost-desktop-audit.json`。
3. 打开 `design/rust-dependency-audit-ci.md` 的 `剩余 informational quality 告警` 小节。

预期结果：

1. 根工作区与桌面锁文件的 `vulnerabilities.list` 均为空。
2. 根工作区和桌面锁文件剩余 warnings 中的 GTK3/glib/proc-macro-error、`fxhash`、`paste`、`proc-macro-error2`、桌面 build-time `rand 0.7.3`、桌面 `unic-*` 均能在设计文档中找到上游来源和本轮不直接替换的原因。
3. 本轮没有新增 cargo-audit ignore 来隐藏剩余 warning。

### TC-RDA-09 TLS PEM 迁移保持证书功能可用

操作步骤：

1. 执行 `cargo test -p bifrost-tls`。

预期结果：

1. 命令退出码为 0。
2. 测试覆盖 CA 证书、私钥与安装相关 PEM 解析路径。
3. `Cargo.toml` 中不再通过 workspace 直接依赖 `rustls-pemfile`。

### TC-RDA-10 Traffic DB detail blob 改用 JSON 后读写正常

操作步骤：

1. 执行 `cargo test -p bifrost-admin traffic_db --lib`。

预期结果：

1. 命令退出码为 0。
2. Traffic DB 详情写入、读取、搜索和清理相关测试通过。
3. `bifrost-admin` 不再直接依赖 `bincode`。

### TC-RDA-11 Admin 与 Agent 依赖升级后仍可编译

操作步骤：

1. 执行 `cargo check -p bifrost-agent -p bifrost-admin`。

预期结果：

1. 命令退出码为 0。
2. `portable-pty 0.9` 与 Windows target `local-ip-address 0.6.13` 相关编译图可正常解析。
3. 输出中没有 `serial 0.4.0` 相关依赖链。

## 清理步骤

本测试不启动服务、不创建运行时数据目录，无需清理临时服务。`/tmp/bifrost-root-audit.json` 与 `/tmp/bifrost-desktop-audit.json` 可在验证完成后删除。若本地执行过程中产生 Cargo 编译缓存，可保留在标准 `target/` 目录中。
