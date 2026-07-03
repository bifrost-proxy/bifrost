# Daemon Certificate Check 设计方案

## 背景

`bifrost start -d`（或 `--daemon`）之前会跳过 CA 证书安装/信任检查，只在 daemon 子进程里通过 `load_tls_config()` 生成本地 CA 文件。这一路径导致：

- 异步启动看似成功——CLI 立刻返回 `Daemon started with PID …`，但 daemon 子进程从未做过证书信任检查。
- 用户第一次启用 TLS interception，或匹配到 `tls_intercept=true` 的规则时，系统/浏览器因为 Bifrost CA 从未被系统信任而失败，表现为“证书错误 / net::ERR_CERT_AUTHORITY_INVALID”。
- 前台启动 `bifrost start`（无 `-d`）有交互式 prompt 与自动信任的完整流程，daemon 却把这个门禁跳过，导致两种启动模式行为分裂。

本方案让 daemon 启动与前台启动共享同一套 **证书准备门禁**，并且保证门禁发生在 fork daemon 之前——一旦 CA 未安装或未信任、当前又无法自动或交互式确认，`bifrost start --daemon` 必须直接失败，不能报告 daemon 启动成功。

## 用户目标验证清单

### 必须实现

- `bifrost start` 与 `bifrost start --daemon` 默认都执行 CA 证书检查，不再因为 `--daemon` 跳过。
- `--skip-cert-check` 保持“显式跳过 CA 安装/信任检查”原语义，daemon 与前台一致。
- `--yes` 自动确认安装 / 信任证书，符合 CLI help 中“自动回答 yes”的语义。
- 无交互终端（`stdin` 非 TTY）+ 无 `--yes` + CA 未安装 / 未信任 → 启动直接失败，退出码非 0，输出可读的修复建议。
- 修复建议至少覆盖 3 条：
  - `bifrost start --daemon --yes`
  - 先执行 `bifrost ca install`
  - 明确知道风险时加 `--skip-cert-check`
- daemon 子进程仍通过 `load_tls_config()` 保证 CA 文件存在（用于运行时读取），但不负责交互 prompt / 系统 trust store 安装。
- 门禁失败时不落 pid 文件、不占用端口、不打印 “Daemon started with PID”。
- 修改覆盖 `desktop/src-tauri` 的 daemon 启动路径（如复用 CLI），保持行为一致。

### 必须不破坏

- 端口冲突处理、`--restart`、daemon readiness 探测、`bifrost stop` 语义。
- 前台交互式启动的 prompt UX（有 TTY 时仍走 prompt）。
- `--skip-cert-check` 在 CI / 无 CA 环境的显式绕过能力。
- Tray helper 启动命令 `build_tray_start_args`，`--skip-cert-check` 与 `--yes` 通过参数正确传递。
- Admin API `/certificate` 相关端点与 CLI `ca install` 行为。

### 必须真实验证

- 非交互模式（`stdin=/dev/null`）+ 无 `--yes` + CA 未安装 → `bifrost start --daemon` 失败，输出含 “no interactive terminal is available” + `--yes` 提示。
- 同上条件 + `--skip-cert-check` → daemon 仍能正常起来，Admin API `/api/proxy/address` 可访问。
- `--yes` 未安装 CA → 自动完成安装并成功启动 daemon。
- 有 TTY 时不带 `--yes` → 走 prompt，用户回答 y 后完成安装。

## 产品语义

### 决策矩阵

| CA 状态 | 交互 TTY | `--yes` | `--skip-cert-check` | 结果 |
|---------|:--------:|:-------:|:-------------------:|------|
| InstalledAndTrusted | 任意 | 任意 | 任意 | 直接放行 |
| InstalledNotTrusted / NotInstalled | 有 | 无 | 否 | 走交互 prompt |
| InstalledNotTrusted / NotInstalled | 无 | 是 | 否 | 自动安装 / 信任 |
| InstalledNotTrusted / NotInstalled | 无 | 无 | 是 | 显式跳过，daemon 起 |
| InstalledNotTrusted / NotInstalled | 无 | 无 | 否 | **失败**，输出 3 条修复建议 |

关键约束：daemon 启动**不允许**在“CA 未信任 + 无法确认”的状态下静默继续。前台启动同样遵循这个矩阵。

### 错误信息稳定契约

失败输出必须包含以下 substring（供 e2e 断言）：

- `no interactive terminal is available`
- `--yes`
- 一段说明用户可以先跑 `bifrost ca install` 或加 `--skip-cert-check`

失败时**不能**出现 `Daemon started with PID`。CA 材料文件（`certs/ca.crt` / `certs/ca.key`）应该已经被生成，方便用户随后手动 `bifrost ca install` 走安装流程。

### daemon 子进程职责收缩

- daemon 子进程只调用 `load_tls_config()`：确认 CA 文件存在、必要时生成，但不做安装 / 信任判定。
- 所有与用户交互 / 系统 trust store 变更相关的动作都在父进程（fork 前）完成。
- daemon 子进程不能因为 CA 未信任把自己搞死；如果父进程放行了 daemon，子进程默认信任 CA 已经就绪。

## 技术细节

### 门禁函数

抽出 `certificate_resolution` 阶段（`crates/bifrost-cli/src/commands/start.rs`）：

```rust
enum CertResolution {
    Ready,             // CA installed + trusted
    Installed,         // 已通过 --yes 或 prompt 自动信任
    Skipped,           // --skip-cert-check
    NeedManualAction,  // 缺条件，直接返回 Err
}

fn resolve_certificate(
    yes: bool,
    skip_cert_check: bool,
    is_tty: bool,
) -> Result<CertResolution> {
    if skip_cert_check { return Ok(CertResolution::Skipped); }

    let status = CertInstaller::status()?;
    if matches!(status, CertStatus::InstalledAndTrusted) {
        return Ok(CertResolution::Ready);
    }

    if yes {
        CertInstaller::install_and_trust()?;
        return Ok(CertResolution::Installed);
    }

    if is_tty {
        // 交互 prompt
        return prompt_install_trust(status);
    }

    Err(non_interactive_ca_error(status))
}
```

- `--daemon` 与前台调用同一个 `resolve_certificate`。
- 失败时返回结构化错误，`start` 命令统一转换为用户可读的 stderr。
- `resolve_certificate` 在 fork 之前调用。

### daemon 父子进程分工

- 父进程：`resolve_certificate` → 若成功继续 fork daemon；若失败输出错误 + 退出非 0。
- 子进程：`load_tls_config()` + 启动 proxy / admin。
- daemon 参数中显式带上 `--skip-cert-check` 或对应等价语义，避免子进程重新走一遍 CA 检查造成 double prompt。

### CLI 参数处理

- `--yes` 与 `--skip-cert-check` 在父进程与子进程都保留：daemon 内部起子命令时通过 `build_tray_start_args` / `build_daemon_start_args` 透传。
- `--skip-cert-check` 在父进程门禁里生效；子进程收到该参数只是保持行为幂等。
- 无 TTY 时 `--yes` 与 `--skip-cert-check` 互斥不做硬约束，`--skip-cert-check` 优先（用户明确要求跳过）。

### 依赖

- `bifrost_tls::CertInstaller`：负责判断 CA 状态、执行安装 / 信任。
- `bifrost_tls::CertStatus`：枚举 `InstalledAndTrusted` / `InstalledNotTrusted` / `NotInstalled` 等。
- `std::io::IsTerminal`：判断 `stdin` 是否为 TTY。
- 现有 `--yes` / `--skip-cert-check` CLI 参数。
- `desktop/src-tauri/src/main.rs` 中调用 `bifrost start` 的路径同步升级。

## CLI / Web / Admin API 边界

- 只影响 `bifrost start` 与相关 tray helper 启动路径；不新增任何 API 端点。
- `bifrost ca install` 独立命令行为不变；用户在门禁失败时仍可显式跑它。
- Admin API `/certificate` 系列端点保持只读语义，不参与启动门禁。

## 数据 / Sync 边界

- 不改动数据目录布局，`certs/` 位置不变。
- CA 材料属于本机安全资产，不进入 rule sync / group sync。

## 实现切分

### Phase 1：门禁函数与 CLI

- 抽 `resolve_certificate`；在 `start` 命令 fork daemon 之前调用。
- 加入 `IsTerminal` 判定与非交互错误结构化输出。
- 保持 `--yes` / `--skip-cert-check` 语义一致。
- 单测 `certificate_resolution_*` 三条覆盖矩阵三个关键格。

### Phase 2：daemon 子进程 & desktop 集成

- daemon 子进程只做 `load_tls_config()`，不做 CA 门禁。
- `build_tray_start_args` / desktop 启动路径同步传递参数。
- 单测 `build_tray_start_args_includes_yes_and_skip_cert_check_flags`。

### Phase 3：E2E 与人工回归

- 新增 `e2e-tests/tests/test_daemon_cert_check_e2e.sh` 覆盖 2 条关键场景：非交互失败、`--skip-cert-check` 成功。
- 更新 `human_tests/cli-start-stop-status.md` 与 `human_tests/readme.md`。

### Phase 4：文档

- `docs/cli.md` 与 `docs/getting-started.md` 明确“daemon 默认走 CA 门禁”。
- README 启动流程更新（若涉及）。

## 测试方案

### 单元测试

- `certificate_resolution_blocks_non_interactive_missing_ca_without_yes`：无 TTY + 无 `--yes` + CA 未安装 → `Err`。
- `certificate_resolution_auto_installs_when_yes`：`--yes` 对 `NotInstalled` / `InstalledNotTrusted` 都触发安装 / 信任。
- `certificate_resolution_prompts_when_terminal_available`：有 TTY 时仍走 prompt。
- `build_tray_start_args_includes_yes_and_skip_cert_check_flags`：tray 启动参数透传两个 flag。

### E2E

`e2e-tests/tests/test_daemon_cert_check_e2e.sh`（新增）：

- `test_daemon_blocks_missing_ca_without_tty_or_yes`：
  - `BIFROST_DATA_DIR` 临时目录、`stdin=/dev/null`、无 `--yes`。
  - 断言 exit 非 0，输出含 `no interactive terminal is available` + `--yes`，不含 `Daemon started with PID`；`certs/ca.crt` 已生成；Admin API 不可达。
- `test_daemon_skip_cert_check_still_starts`：
  - `--skip-cert-check --unsafe-ssl --no-system-proxy`。
  - 断言 exit 0，输出含 `Daemon started with PID`，Admin API `/api/proxy/address` 可达。
  - 结束后 `bifrost stop` 清理。
- 端口使用非 9900（本 e2e 用 `18892`），数据目录临时。

### 真实场景测试（human_tests/cli-start-stop-status.md）

新增 daemon CA 检查回归用例：

- TC-DCC-01：无 TTY + 无 `--yes` + CA 未安装 → daemon 启动失败，输出含修复建议 3 条。
- TC-DCC-02：`--yes` + CA 未安装 → daemon 自动安装 CA 并成功启动。
- TC-DCC-03：`--skip-cert-check` → daemon 起，Admin API 可达。
- TC-DCC-04：有 TTY 时不带 `--yes` → prompt 出现，回答 y 后成功。
- 每条 case 使用临时 `BIFROST_DATA_DIR`、非 9900 端口、`BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`、`BIFROST_DISABLE_TRAY=1`、`--no-system-proxy`。

### 校验清单

- `cargo test -p bifrost-cli certificate_resolution`
- `cargo test -p bifrost-cli build_tray_start_args`
- `bash e2e-tests/tests/test_daemon_cert_check_e2e.sh`
- `cargo test --workspace --all-features`
- 时间允许时 `cargo clippy --workspace --all-targets --all-features -- -D warnings`

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核 `start.rs`：CA 门禁是否严格在 fork daemon 之前；是否影响端口冲突、`--restart`、daemon readiness 探测。
- 复核 `ca.rs`：`--yes` / `--skip-cert-check` / prompt 语义与门禁函数的组合表。
- 复核 desktop / tray 启动路径：`build_tray_start_args` 透传两个 flag。
- 跑 CLI 单测、e2e。

### 第 2 轮

- 复查最新 diff、docs、human_tests 索引与暂存改动边界。
- 复跑受影响测试；确认 `--skip-cert-check` 与 daemon readiness 都可用。
- 抽 1 条 human_tests case 真实操作验证。

## 风险与决策点

- **门禁失败是否残留 CA 材料**：当前策略是保留（`certs/ca.crt` 已生成），方便用户后续 `bifrost ca install`。副作用：若用户永远不安装，`data_dir` 会有一份未信任的 CA。可以在错误信息里同时给出“如果不想安装可以清理 `certs/`”的补充说明。
- **`--yes` 与 `--skip-cert-check` 的组合语义**：优先 `--skip-cert-check`。原因：用户明确要跳过就不应该被 `--yes` 隐式改写为安装。
- **系统 trust store 变更权限**：macOS 需要 sudo，Linux 需要 root，Windows 需要管理员。`--yes` 在权限不足时会失败；此时门禁应把失败原因带到 stderr，避免 daemon 启动过程日志混乱。
- **desktop app 启动**：desktop 通过 tray helper 拉起 daemon，必须保证 `--yes` 或 `--skip-cert-check` 由 desktop 主进程决定，不能让用户在“不知道”的情况下静默跳过。
- **CI 环境**：CI 一般用 `--skip-cert-check`，本方案对其零影响。future 若引入 CA 自动安装到 CI 沙盒，可以复用 `--yes` 分支。
- **子进程二次门禁**：子进程默认不再检查 CA。若未来需要在子进程也做一次 sanity check，必须避免二次 prompt，可以复用 `--skip-cert-check` 语义。
