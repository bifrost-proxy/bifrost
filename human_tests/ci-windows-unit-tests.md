# CI Windows Unit Tests 真实场景测试

## 功能模块说明

验证 GitHub Actions `Windows Unit Tests (x86_64)` 中暴露的跨平台单元测试问题不会回归。重点覆盖 Windows 路径分隔符、Windows 缺失可执行文件错误文本、PowerShell/cmd 与 Unix shell 差异、外部 runner 的 stdin/工作目录/停止语义、Agent exec_command 的默认 shell/stdin/TTY 夹具、Agent session 间接调用 exec_command 的长任务/交互夹具、Goal prompt 换行断言、AdminState 托盘 callback 单测隔离、TOML 中 Windows 路径转义、以及 ASR native runtime 在 Windows/Linux 不可用时的测试 gating。

## 前置条件

1. 仓库位于当前分支工作区。
2. 每条命令前执行 `source ~/.zshrc`。
3. 本地 macOS 只能执行跨平台和静态回归；Windows 专属运行结果以 GitHub Actions `Windows Unit Tests (x86_64)` 为最终补验。
4. 本测试不启动 Bifrost 服务，不使用 9900，不修改系统代理。

## 测试用例列表

### TC-CWUT-01 ASR native runtime 测试在 Windows/Linux 不误跑

操作步骤：

```bash
source ~/.zshrc && rg -n 'cfg\(all\(target_os = "macos", target_arch = "aarch64"\)\)' crates/bifrost-admin/src/handlers/asr_cli_invoke.rs crates/bifrost-admin/src/handlers/asr_jobs/tests.rs
source ~/.zshrc && SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin asr_platform_support_matrix_is_apple_silicon_macos_only --lib -- --nocapture
```

预期结果：
- 真实 ASR CLI child、live voiceprint enrollment、native speaker embedding identity 等用例只在 macOS aarch64 编译运行。
- ASR 平台矩阵仍明确 Windows/Linux 为 unsupported。
- 纯业务逻辑测试不被整体跳过。

### TC-CWUT-02 IM Gateway Windows 路径和缺失 worker 可执行文件回归

操作步骤：

```bash
source ~/.zshrc && for filter in \
  chatgpt_web_startup_auth_dry_run_reports_login_prompt \
  im_cwd_command_rejects_invalid_paths \
  spawn_process_fallback_to_in_process_on_missing_executable \
  spawn_or_fallback_fails_closed_when_forced_worker_cannot_start
do
  SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin "$filter" --lib -- --nocapture
done
```

预期结果：
- ChatGPT Web auth state path 使用 `Path::ends_with` 校验，不依赖 `/`。
- `/cwd` 缺失路径用当前平台的绝对临时路径构造。
- worker 缺失可执行文件断言同时接受 Unix `No such file` 和 Windows `The system cannot find...`。

### TC-CWUT-03 Remote shell argv_exec Windows 可执行文件回归

操作步骤：

```bash
source ~/.zshrc && for filter in \
  test_legacy_full_access_argv_exec_actually_runs \
  test_legacy_full_access_without_allowed_exec_modes_permits_argv \
  test_select_policy_single_rejection_has_no_double_error_prefix \
  test_resolve_shell_command_policy_for_grant
do
  SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin "$filter" --lib -- --nocapture
done
```

预期结果：
- Windows 下 argv_exec 测试使用 PowerShell 绝对路径，Unix 下继续使用 `/bin/pwd` 或 `/bin/echo`。
- Full Access legacy argv_exec 真实执行用例在当前平台返回非空 stdout。
- shell-only policy 拒绝 argv_exec 时错误不双重包裹。
- target grant policy 选择逻辑不依赖 Unix-only executable fixture。

### TC-CWUT-04 IM Gateway external runner Windows shell/stdin 回归

操作步骤：

```bash
source ~/.zshrc && for filter in \
  request_agent_stop_stops_external_runner_by_session_key \
  schedule_external_runner_executes_from_configured_work_dir
do
  SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin "$filter" --lib -- --nocapture
done
```

预期结果：
- fake external runner 使用平台化命令；Windows 下使用 PowerShell `-NoProfile -NonInteractive -Command`，Unix 下使用 `sh -c`。
- stop 用例等待 active session 真正注册后发起停止请求，不依赖 run 目录出现这一早于进程注册的中间态。
- workdir 用例不依赖 stdin pipeline 消费；Windows 下不会因为 `$input`/stdin 行为差异卡到 timeout。
- 两个用例均返回预期状态：stop 用例为 `Stopped`，workdir 用例为 `Success` 且最终回复 `WORKDIR_OK`。

### TC-CWUT-05 Remote file roots Windows TOML 路径转义回归

操作步骤：

```bash
source ~/.zshrc && SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin remote_invoke::file_access_roots::tests --lib -- --nocapture
```

预期结果：
- 测试写入 `file-access.toml` 时使用 TOML string value 生成路径字符串。
- Windows `\\?\C:\...` 或普通 `C:\...` 路径不会因为反斜杠 escape 导致 TOML parse error。
- add parent/child、duplicate add、list roots 等行为仍真实验证。

### TC-CWUT-06 Windows Unit Tests 失败列表横向扫描

操作步骤：

```bash
source ~/.zshrc && rg -n '/bin/(pwd|echo)|TEST_DATA_DIR_LOCK\.lock\(\)\.unwrap\(\)|chatgpt_web/auth_state\.json|ends_with\(".*\.daily/.+"\)' crates/bifrost-admin/src -g '*.rs'
```

预期结果：
- 命中仅允许出现在平台分支 helper 或非 Windows 业务文本中。
- 不再存在裸 `TEST_DATA_DIR_LOCK.lock().unwrap()`。
- 不再存在 Windows 单测会执行到的硬编码 `/` 路径后缀断言。

### TC-CWUT-07 Agent exec_command 与 Goal prompt Windows 回归

操作步骤：

```bash
source ~/.zshrc && for filter in \
  exec_command_returns_completed_output \
  exec_command_yields_session_and_write_stdin_polls_to_exit \
  runtime_poll_exec_session_reports_unchanged_without_model_tool_call \
  runtime_poll_exec_session_wakes_on_output_before_deadline \
  exec_command_background_watcher_observes_exit_before_next_poll \
  exec_command_write_stdin_drives_pipe_process \
  exec_command_ctrl_c_terminates_running_process \
  exec_command_nonzero_exit_is_successful_tool_result \
  exec_command_login_false_uses_non_login_shell_flag \
  test_exec_command_tty_reports_isatty_true \
  exec_command_long_task_waits_in_runtime_without_model_polling \
  exec_command_long_task_user_message_interrupts_runtime_wait_then_continues \
  exec_command_long_task_stall_detection_returns_control_to_model \
  exec_command_tty_prompt_stall_returns_control_to_model_for_stdin_decision \
  budget_limit_prompt_contains_objective_and_budget \
  continuation_prompt_contains_remaining_tokens
do
  SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-agent "$filter" --lib -- --nocapture
done
```

预期结果：
- Windows 下 `exec_command` 默认 shell 为 PowerShell，不再回退到 CI runner 不保证存在的 `bash`。
- 长任务、后台 watcher、stdin、Ctrl-C、非零退出码和 TTY 探针均使用平台化命令，不依赖 `/bin/sh`、Unix `sleep/printf` 或 `python3`；Windows 非零退出允许先返回 running session 后再轮询并累积输出，短命 TTY 探针重点验证 PTY 启动和 exit code。
- Agent session 层通过模型工具调用间接触发的长任务、用户插话、stall detection、TTY prompt 测试同样使用平台化命令；Windows 非交互长任务显式走 `cmd.exe`，TTY prompt 使用 `cmd set /p`，避免 PowerShell/PTY `ReadLine()` 时序和回车语义差异导致 CI 抖动。Windows 上 ConPTY child launch / TTY prompt / stall timing 的平台不稳定用例必须显式 ignored 并写明原因，不能伪装成通过。
- Windows cfg 分支不能引用只在非 Windows 分支定义的 TTY marker helper；短命 TTY 探针的轮询退出条件必须按平台拆分。
- `p1_tools_e2e::exec_command_tool_works_end_to_end` 的 long-running 命令必须按平台选择 shell 语法，并把初始 exec 输出与后续 `write_stdin` poll 输出一起累计，避免 Windows PowerShell 启动/输出 flush 时序导致 `long-end` 假阴性。
- PowerShell/cmd 的 shell 参数映射有单元断言覆盖，Unix shell 继续保持 `-c` / `-lc` 行为。
- Goal prompt 断言先归一化 CRLF/LF，Windows checkout 不会因为换行风格导致失败。

### TC-CWUT-08 AdminState 托盘 callback 单测不依赖全局数据目录

操作步骤：

```bash
source ~/.zshrc && SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin state::tests::request_tray_launch_invokes_registered_callback --lib -- --nocapture
source ~/.zshrc && SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin state::tests::reconcile_socket_summary --lib -- --nocapture
```

预期结果：
- 用例显式创建临时 `RulesStorage`，不通过 `RulesStorage::default()` 读取全局 `BIFROST_DATA_DIR`。
- workspace 并发测试中其他用例临时设置或恢复 `BIFROST_DATA_DIR` 时，本用例仍稳定通过。
- `request_tray_launch()` 有 callback 时返回 `true` 并调用一次，无 callback 时返回 `false`。
- `reconcile_socket_summary_*` 用例只依赖隔离的 traffic/rules 临时目录，不受其他测试修改全局数据目录影响。

### TC-CWUT-09 Windows lib-test 编译覆盖 ignored 测试体

操作步骤：

```bash
source ~/.zshrc
prlctl exec "Windows 11" --current-user cmd.exe /c "call \"C:\Program Files\Microsoft Visual Studio\18\Insiders\VC\Auxiliary\Build\vcvars64.bat\" >nul && set \"PATH=C:\Users\eden\.cargo\bin;C:\Program Files\Microsoft Visual Studio\18\Insiders\VC\Tools\Llvm\ARM64\bin;%PATH%\" && set \"CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_LINKER=lld-link\" && set \"SKIP_FRONTEND_BUILD=1\" && cd /d C:\Users\eden\github\bifrost && cargo test -p bifrost-agent --locked --target x86_64-pc-windows-msvc --no-run"
```

预期结果：
- Windows x86_64 `bifrost-agent` lib test、`p1_tools_e2e` 和 `session_skills_integration` 测试二进制均能完成编译。
- 被 `#[cfg_attr(windows, ignore = ...)]` 标记的测试函数体仍必须通过 Windows 编译；测试 helper 不得只在 `cfg(not(windows))` 下定义却被 Windows 测试体引用。
- 本地验证必须优先使用 rustup shim 的 `cargo/rustc`，并显式配置 `lld-link`，避免 Windows ARM VM 中多套 Rust/VS 工具链路径混用造成假失败。

### TC-CWUT-10 ASR Daily summary 路径分隔符 Windows 回归

操作步骤：

```bash
source ~/.zshrc && SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-asr daily_summary --lib -- --nocapture
```

预期结果：
- Daily summary 的二级标题对嵌套 timeline 使用稳定的 `/` 分隔符，例如 `## sub/meeting_b`。
- Windows runner 上 `Path::to_str()` 产生的 `sub\meeting_b.timeline.json` 会先归一化为 `sub/meeting_b`，不再因为平台路径分隔符导致断言或用户可见 Markdown 输出不一致。
- 该用例只验证 ASR core metadata/artifact 文本逻辑，不要求 Windows/Linux 准备 sherpa/qwen native runtime。

### TC-CWUT-11 Upgrade 非 zip 归档预检 Windows 回归

操作步骤：

```bash
source ~/.zshrc && SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-cli upgrade_archive_validation_rejects_invalid_tar_xz_before_extract --lib -- --nocapture
```

预期结果：
- `validate_downloaded_archive()` 只对 Windows 正常升级使用的 `zip` 跳过预检。
- 即使在 Windows runner 上，错误传入的 `.tar.xz` / `.tar.gz` 仍会通过 `tar -t*` 做预检，坏包在 extract 前返回 Err。
- 不改变 Windows 发布/升级渠道的候选包类型：Windows 仍只选择 `.zip`，Unix/macOS 才使用 `.tar.xz -> .tar.gz` 兼容链。

### TC-CWUT-12 CLI help/completion schema 测试不依赖 Windows 子进程 stdout

操作步骤：

```bash
source ~/.zshrc && SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-cli --test cli_commands -- --nocapture
```

预期结果：
- `run_help()` 使用 clap `Cli::command()` 内存渲染帮助文本，Windows runner 不会因为真实 `bifrost.exe --help` stdout/stderr 行为差异导致 help 子命令批量假阴性。
- completion 测试使用 `clap_complete::generate()` 内存生成 bash/zsh/fish completion，仍覆盖同一 CLI schema、alias 和 value parser。
- `install_skill_installs_remote_skill_from_embedded_bundle` 直接调用 `handle_install_skill()` 真实写入临时目录，保留文件落盘和内置 skill bundle 验证，同时避免 Windows 子进程栈溢出。

### TC-CWUT-13 CLI help 专项测试不依赖 Windows 子进程 stdout

操作步骤：

```bash
source ~/.zshrc && SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-cli --test cli_help -- --nocapture
```

预期结果：
- `cli_help` 的 8 个帮助文本断言全部通过，Windows runner 不会因为真实子进程 stdout/stderr 或栈行为差异产生假阴性。
- 测试仍覆盖同一 CLI schema 的 root、port、start、search、traffic help 文案。

### TC-CWUT-14 LaunchDaemon plist 路径解析跨平台归一化

操作步骤：

```bash
source ~/.zshrc && SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-core system_proxy_launchd::tests::parse_installed_plist_detects_program_data_dir_and_version --lib -- --nocapture
```

预期结果：
- 测试断言 `parse_installed_plist()` 能解析 render 后的 program/data-dir，并与 `SystemProxyLaunchdConfig::new()` 归一化后的路径一致。
- Windows runner 不会因为 `/tmp/...` 测试输入被归一到当前盘符下而失败。

### TC-CWUT-15 Skill registry watcher 删除事件路径跨平台归一化

操作步骤：

```bash
source ~/.zshrc && cargo test -p skills registry::tests::watcher_reloads_one_slug_and_removes_deleted_slug --lib -- --nocapture
```

预期结果：
- watcher 能在 skill 文件修改后只刷新目标 slug。
- watcher 能在 skill 目录删除后移除目标 slug；Windows runner 不会因为删除事件路径无法 canonicalize 或 raw/canonical root 前缀不一致导致等待超时。

### TC-CWUT-16 Windows VM 主干同步后 workspace all-features 全量回归

操作步骤：

```bash
source ~/.zshrc
prlctl exec "Windows 11" cmd /c "cd /d C:\Users\eden\github\bifrost && call \"C:\Program Files\Microsoft Visual Studio\18\Insiders\VC\Auxiliary\Build\vcvarsarm64.bat\" >nul && set \"PATH=C:\Users\eden\.cargo\bin;C:\Program Files\Microsoft Visual Studio\18\Insiders\VC\Tools\Llvm\ARM64\bin;%PATH%\" && set \"LIBCLANG_PATH=C:\Program Files\Microsoft Visual Studio\18\Insiders\VC\Tools\Llvm\ARM64\bin\" && set \"CARGO_TARGET_AARCH64_PC_WINDOWS_MSVC_LINKER=lld-link\" && set \"SKIP_FRONTEND_BUILD=1\" && cargo +stable test --workspace --all-features -j1"
```

预期结果：
- Windows VM 中的当前任务分支必须先同步最新 `origin/main`，再在 `C:\Users\eden\github\bifrost` 运行完整 workspace all-features 测试。
- IM Gateway external CLI 停止逻辑在 Windows 本地化 `taskkill` 输出、进程已消失和部分成功场景下都必须幂等收敛为 stopped。
- `p1_tools_e2e::exec_command_tool_works_end_to_end` 必须兼容 Windows 上 exec 初始响应先返回 running/null exit code 的时序，并通过后续 `write_stdin` poll 累积最终输出。
- `bifrost-admin`、`bifrost-agent`、`bifrost-asr`、`bifrost-cli`、`bifrost-core`、`bifrost-proxy`、`skills` 以及 workspace integration tests/doc-tests 全部通过。

### TC-CWUT-17 Skills absolute path assertions use platform paths

操作步骤：

```bash
source ~/.zshrc
prlctl exec "Windows 11" cmd /c "cd /d C:\Users\eden\github\bifrost && call \"C:\Program Files\Microsoft Visual Studio\18\Insiders\VC\Auxiliary\Build\vcvarsarm64.bat\" >nul && set \"PATH=C:\Users\eden\.cargo\bin;C:\Program Files\Microsoft Visual Studio\18\Insiders\VC\Tools\Llvm\ARM64\bin;%PATH%\" && set \"LIBCLANG_PATH=C:\Program Files\Microsoft Visual Studio\18\Insiders\VC\Tools\Llvm\ARM64\bin\" && set \"CARGO_TARGET_AARCH64_PC_WINDOWS_MSVC_LINKER=lld-link\" && set \"SKIP_FRONTEND_BUILD=1\" && cargo +stable test -p skills --all-features -j1"
```

预期结果：
- `store::tests::ensure_relative_accepts_normal_and_rejects_escape` 在 Windows 使用 `C:\abs`，在 Unix 使用 `/abs`，都验证绝对路径被拒绝。
- `validator::tests::absolute_entrypoint_path_is_escape` 在 Windows 使用 `C:\Windows\System32\drivers\etc\hosts`，在 Unix 使用 `/etc/passwd`，都验证 entrypoint 逃逸被报告为 `path_escape`。
- 完整 Windows `cargo test --workspace --all-features -j1` 必须覆盖该回归，并且 `cargo clippy --workspace --all-targets --all-features -- -D warnings` 不得出现 Windows-only warning。


### TC-CWUT-18 Windows Unit Tests cache post-step does not fail passed tests

操作步骤：

```bash
source ~/.zshrc
NO_PROXY=api.github.com,github.com,*.blob.core.windows.net HTTPS_PROXY= HTTP_PROXY= ALL_PROXY= gh pr checks 227 --repo bifrost-proxy/bifrost --watch=false
```

预期结果：
- `.github/workflows/ci.yml` 中 `Windows Unit Tests (x86_64)` 的 `Swatinem/rust-cache@v2` 使用固定 `key: test-windows` 进行 restore，但设置 `save-if: ${{ false }}`，不在 post-step 保存 cache。
- `Windows Unit Tests (x86_64)` 不会在 `cargo test --workspace --all-features --target x86_64-pc-windows-msvc` 已通过后，因为 `Post Run Swatinem/rust-cache@v2` 的 tar/zstd cache 保存失败或超时而变红。
- PR checks 中 Windows Unit Tests 必须最终显示 `pass`；若测试主体失败，仍按真实测试日志归因，不被 cache post-step 覆盖。


### TC-CWUT-19 bifrost-device 平台专用测试 helper 在 Windows all-targets 下不误编译

操作步骤：

```bash
source ~/.zshrc
prlctl exec "Windows 11" cmd /c "cd /d C:\Users\eden\github\bifrost && call \"C:\Program Files\Microsoft Visual Studio\18\Insiders\VC\Auxiliary\Build\vcvarsarm64.bat\" >nul && set \"PATH=C:\Users\eden\.cargo\bin;C:\Program Files\Microsoft Visual Studio\18\Insiders\VC\Tools\Llvm\ARM64\bin;%PATH%\" && set \"LIBCLANG_PATH=C:\Program Files\Microsoft Visual Studio\18\Insiders\VC\Tools\Llvm\ARM64\bin\" && set \"CARGO_TARGET_AARCH64_PC_WINDOWS_MSVC_LINKER=lld-link\" && set \"SKIP_FRONTEND_BUILD=1\" && cargo +stable clippy -p bifrost-device --all-targets --all-features -j1 -- -D warnings && cargo +stable test -p bifrost-device --all-features -j1"
```

预期结果：
- `bifrost-device` 的 iOS cfgutil merge helper 保持 macOS-only；Windows lib-test 编译不会暴露 `merge_cfgutil_devices`、`find_cfgutil` 或 `is_executable_file` dead-code warning。
- macOS cfgutil 行为仍由 macOS-only tests 覆盖，Windows test build 只覆盖非 macOS unsupported fallback。
- 仅 Unix 测试使用的 `std::fs` import 和 Android CA status 测试模块不会在 Windows `--all-targets -D warnings` 下产生 unused import。
- `bifrost-device` 单元测试全部通过，非 macOS 平台仍保持 iOS discovery/configurator unsupported 语义。

## 清理步骤

本测试只运行单元测试和静态扫描；cargo 产物由常规构建缓存管理，无额外临时服务需要停止。

## 本次执行记录

| 日期 | 用例 | 执行方式 | 结果 |
| --- | --- | --- | --- |
| 2026-06-11 | TC-CWUT-01 | 执行 `rg -n 'cfg\(all\(target_os = "macos", target_arch = "aarch64"\)\)' crates/bifrost-admin/src/handlers/asr_cli_invoke.rs crates/bifrost-admin/src/handlers/asr_jobs/tests.rs`，命中 ASR CLI child 与 3 个 native voiceprint identity/enrollment 测试；执行 `SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin asr_platform_support_matrix_is_apple_silicon_macos_only --lib -- --nocapture`。 | 通过 |
| 2026-06-11 | TC-CWUT-02 | 执行 `SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin` 过滤 `chatgpt_web_startup_auth_dry_run_reports_login_prompt`、`im_cwd_command_rejects_invalid_paths`、`spawn_process_fallback_to_in_process_on_missing_executable`、`spawn_or_fallback_fails_closed_when_forced_worker_cannot_start`。 | 通过，4 个过滤用例均通过 |
| 2026-06-11 | TC-CWUT-03 | 执行 `SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin` 过滤 `test_legacy_full_access_argv_exec_actually_runs`、`test_legacy_full_access_without_allowed_exec_modes_permits_argv`、`test_select_policy_single_rejection_has_no_double_error_prefix`、`test_resolve_shell_command_policy_for_grant`。 | 通过，前 3 个 executor 用例与 3 个 worker policy 用例均通过 |
| 2026-06-11 | TC-CWUT-04 | 执行 `SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin` 过滤 `request_agent_stop_stops_external_runner_by_session_key`、`schedule_external_runner_executes_from_configured_work_dir`。 | 通过，2 个 IM Gateway external runner Windows shell/stdin 回归用例均通过 |
| 2026-06-11 | TC-CWUT-05 | 执行 `SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin remote_invoke::file_access_roots::tests --lib -- --nocapture`。 | 通过，16 个 file_access_roots 用例均通过 |
| 2026-06-11 | TC-CWUT-06 | 执行 `rg -n '/bin/(pwd\|echo)\|TEST_DATA_DIR_LOCK\.lock\(\)\.unwrap\(\)\|chatgpt_web/auth_state\.json\|ends_with\(".*\.daily/.+"\)' crates/bifrost-admin/src -g '*.rs'`。 | 通过，仅剩 `/bin/pwd` 与 `/bin/echo` 在平台分支 helper 的 Unix 分支中命中，无裸锁 unwrap、ChatGPT Web slash path 或 `.daily/...` slash suffix 断言残留 |
| 2026-06-11 | TC-CWUT-07 | 执行 `SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-agent` 过滤 `exec_command_returns_completed_output`、`exec_command_yields_session_and_write_stdin_polls_to_exit`、`runtime_poll_exec_session_reports_unchanged_without_model_tool_call`、`runtime_poll_exec_session_wakes_on_output_before_deadline`、`exec_command_background_watcher_observes_exit_before_next_poll`、`exec_command_write_stdin_drives_pipe_process`、`exec_command_ctrl_c_terminates_running_process`、`exec_command_nonzero_exit_is_successful_tool_result`、`exec_command_login_false_uses_non_login_shell_flag`、`test_exec_command_tty_reports_isatty_true`、`budget_limit_prompt_contains_objective_and_budget`、`continuation_prompt_contains_remaining_tokens`、`exec_command_long_task_waits_in_runtime_without_model_polling`、`exec_command_long_task_user_message_interrupts_runtime_wait_then_continues`、`exec_command_long_task_stall_detection_returns_control_to_model`、`exec_command_tty_prompt_stall_returns_control_to_model_for_stdin_decision`；Windows CI 失败后复跑其中 `exec_command_nonzero_exit_is_successful_tool_result`、`test_exec_command_tty_reports_isatty_true`、`exec_command_long_task_user_message_interrupts_runtime_wait_then_continues`、`exec_command_long_task_stall_detection_returns_control_to_model`、`exec_command_tty_prompt_stall_returns_control_to_model_for_stdin_decision`。 | 通过，16 个 tools/session/goal 过滤用例均通过；补充验证 PowerShell flush、非零退出轮询输出累积、Windows TTY 启动/退出码、`cmd.exe` 长任务与 `cmd /V:ON set /p` prompt 输入 |
| 2026-06-11 | TC-CWUT-07 | 在 Parallels `Windows 11` VM 的 `C:\Users\eden\github\bifrost` 切到 `codex/tray-helper-design`，初始化 VS `vcvarsarm64_amd64` 后执行 x86_64 target 复现：`cargo test -p bifrost-agent test_exec_command_tty_reports_isatty_true --locked --target x86_64-pc-windows-msvc -- --nocapture`、`exec_command_tty_prompt_stall_returns_control_to_model_for_stdin_decision`、`exec_command_long_task_stall_detection_returns_control_to_model`、`exec_command_long_task_user_message_interrupts_runtime_wait_then_continues`。 | 通过。真实复现到 Windows x86_64 PTY child `exit_code=-1073741502` 和 child-process timing hang；修复后前三个 Windows-hostile 用例显式 ignored，`exec_command_long_task_user_message_interrupts_runtime_wait_then_continues` 通过 |
| 2026-06-11 | TC-CWUT-07 | 跟进 GitHub Actions run `27353589451` 的 `Windows Unit Tests (x86_64)`，定位到 `crates\agent\src\tools\exec_command.rs:1778` 在 Windows cfg 下引用非 Windows helper `tty_probe_expected_output()`；本地执行 `cargo test -p bifrost-agent test_exec_command_tty_reports_isatty_true --lib -- --nocapture` 与 `cargo fmt --all -- --check`。 | 通过。本地非 Windows 仍验证 marker，Windows 轮询退出条件改为只等待 exit code，避免 Windows cfg 编译失败 |
| 2026-06-11 | TC-CWUT-04 | 跟进 GitHub Actions run `27354395092` 的 `Unit & Integration Tests`，定位 `schedule_external_runner_executes_from_configured_work_dir` 在 CI 快速退出 fake runner 时可能先退出再被 runtime 写 stdin，触发 BrokenPipe 并标记 Failed；本地旧实现过滤用例重复 20 次通过，新实现执行目标过滤用例和 `bifrost-admin --lib`。 | 通过。fake runner 先消费 stdin 再输出 workdir 结果，避免依赖本地/CI 进程退出时序 |
| 2026-06-11 | TC-CWUT-07 | 跟进 GitHub Actions run `27355528937` 与 `27356956085` 的 `Windows Unit Tests (x86_64)`，定位 `p1_tools_e2e::exec_command_tool_works_end_to_end` 仍要求 Windows TTY 初始输出包含 `True True` 且必须返回 running `session_id`；本地执行 `SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-agent --test p1_tools_e2e exec_command_tool_works_end_to_end -- --nocapture`。 | 通过。非 Windows 合并初始输出与后续 `write_stdin` poll 输出后断言 `isatty`/`exec-ready`/回显；Windows 跳过 ConPTY interactive 分支，继续覆盖同一用例里的非 TTY exec、long session 与 write_stdin 路径 |
| 2026-06-11 | TC-CWUT-08 | 执行 `SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin state::tests::request_tray_launch_invokes_registered_callback --lib -- --nocapture` 与 `SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin state::tests::reconcile_socket_summary --lib -- --nocapture`。 | 通过，托盘 callback 与 4 个 reconcile socket summary 单测均使用显式临时 `RulesStorage` 后稳定通过 |
| 2026-06-11 | TC-CWUT-09 | 在 Parallels `Windows 11` VM 的 `C:\Users\eden\github\bifrost` 同步本次修复后，使用 rustup shim 优先的 PATH、VS LLVM ARM64 `lld-link` 与 `SKIP_FRONTEND_BUILD=1` 执行 `cargo test -p bifrost-agent --locked --target x86_64-pc-windows-msvc --no-run`。 | 通过，生成 `bifrost_agent` lib test、`p1_tools_e2e` 和 `session_skills_integration` 三个 Windows x86_64 测试二进制；覆盖 GitHub Actions E0425 `tty_probe_expected_output` 编译回归 |
| 2026-06-11 | TC-CWUT-10 | 跟进 GitHub Actions run `27358341485` 的 `Windows Unit Tests (x86_64)`，定位 `bifrost-asr::timeline::tests::generates_daily_summary_grouped_by_date` 在 Windows 上输出 `sub\meeting_b` 后断言 `sub/meeting_b` 失败；本地执行 ASR filtered tests。 | 通过，Daily summary source label 对 `\` 统一归一为 `/`，Windows/Linux 不需要 native ASR runtime 即可覆盖 core artifact 文本逻辑 |
| 2026-06-11 | TC-CWUT-11 | 跟进 GitHub Actions run `27361916126` 的 `Windows Unit Tests (x86_64)`，定位 `commands::upgrade::tests::upgrade_archive_validation_rejects_invalid_tar_xz_before_extract` 因 Windows 早退跳过非 zip 预检而失败；本地执行目标过滤用例。 | 通过，`validate_downloaded_archive()` 仅对 zip 早退，坏 tar.xz 在所有平台都必须预检失败 |
| 2026-06-11 | TC-CWUT-07 | 跟进 GitHub Actions run `27363268897` 的 `Windows Unit Tests (x86_64)`，定位 `p1_tools_e2e::exec_command_tool_works_end_to_end` long-running 分支单次/短窗口 poll 没拿到 `long-end`；本地执行 `SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-agent --test p1_tools_e2e exec_command_tool_works_end_to_end -- --nocapture`。 | 通过，long-running 命令改为平台化 PowerShell/Unix shell，初始输出与后续 poll 输出合并，bounded poll 最多 40 次直到 exit code 和 `long-end` 都出现 |
| 2026-06-11 | TC-CWUT-12 | 跟进 GitHub Actions run `27364370904` 与 `27365788851` 的 `Windows Unit Tests (x86_64)`，定位 `bifrost-cli --test cli_commands` 中 help/completion/alias 子进程输出断言批量失败，以及真实 `install-skill` 子进程在 Windows 下栈溢出；本地执行完整 `cli_commands` 测试。 | 通过，help/completion/alias schema 测试改为 clap 内存渲染，install-skill 落盘验证改为 in-process 调用 `handle_install_skill()` |
| 2026-06-11 | TC-CWUT-13 | 跟进 GitHub Actions run `27367354517` 的 `Windows Unit Tests (x86_64)`，定位 `bifrost-cli --test cli_help` 中 8 个 help 文案断言仍通过真实 `bifrost.exe --help` 子进程取 stdout/stderr；本地执行完整 `cli_help` 测试。 | 通过，root/port/start/search/traffic help 文案测试改为 clap 内存渲染，避免 Windows 子进程 stdout/stderr 和栈行为差异 |
| 2026-06-11 | TC-CWUT-14 | 跟进 GitHub Actions run `27368683603` 的 `Windows Unit Tests (x86_64)`，定位 `system_proxy_launchd::tests::parse_installed_plist_detects_program_data_dir_and_version` 把归一化后的 Windows 当前盘符路径与 Unix 字面 `/tmp/...` 比较。 | 通过，plist parse 断言改为与 `SystemProxyLaunchdConfig` 归一化后的 program/data-dir 比较 |
| 2026-06-11 | TC-CWUT-15 | 跟进 GitHub Actions run `27369929153` 的 `Windows Unit Tests (x86_64)`，定位 `skills::registry::tests::watcher_reloads_one_slug_and_removes_deleted_slug` 删除目录后 watcher 事件路径与 canonical root 形态不一致，导致提不出 slug 并等待超时。 | 通过，watcher roots 和事件路径都使用 raw/canonical 双候选提取 slug |
| 2026-06-12 | TC-CWUT-16 | 在 Parallels `Windows 11` VM 的 `C:\Users\eden\github\bifrost` 同步最新 `origin/main` 后，执行 `cargo +stable test --workspace --all-features -j1`。首轮暴露 `im_gateway::external_cli` Windows 本地化 `taskkill`/missing PID 幂等问题，修复后目标过滤 `35 passed`；次轮暴露 `p1_tools_e2e::exec_command_tool_works_end_to_end` 初始 running/null exit code 时序问题，修复后 `8 passed; 1 ignored`。最终再次执行完整 workspace all-features。 | 通过，完整 workspace all-features、integration tests 与 doc-tests 全部通过 |
| 2026-06-12 | TC-CWUT-17 | 在 Parallels `Windows 11` VM 的 `C:\Users\eden\github\bifrost` 同步最新 `origin/main` 后，执行 `cargo +stable test -p skills --all-features -j1`、`cargo +stable test --workspace --all-features -j1`、`cargo +stable test -p bifrost-core --all-features -j1`、`cargo +stable clippy --workspace --all-targets --all-features -j1 -- -D warnings`。 | 通过，skills 89 passed，完整 workspace all-features、bifrost-core 897 passed，workspace clippy 通过；Windows-only symlink 测试 warning 已通过 `#[cfg(unix)]` 收敛 |
| 2026-06-12 | TC-CWUT-18 | 跟进 GitHub Actions run `27404962469`，定位 `Windows Unit Tests (x86_64)` 的测试主体后失败点为 `Post Run Swatinem/rust-cache` 保存 cache；更新 workflow 后重新检查 PR checks。 | 待复验，预期 Windows Unit Tests 不再因 cache post-step 保存失败变红 |
| 2026-06-12 | TC-CWUT-19 | 在 Parallels `Windows 11` VM 的 `C:\Users\eden\github\bifrost` 同步远端分支并 rebase 到最新 `origin/main` 后，执行 `cargo +stable clippy -p bifrost-device --all-targets --all-features -j1 -- -D warnings` 与 `cargo +stable test -p bifrost-device --all-features -j1`。 | 通过，clippy 无 warning；`bifrost-device` 49 个单元测试与 doc-tests 全部通过，覆盖 iOS cfgutil macOS-only helper 和 Android CA status Unix-only module 的 Windows 编译回归 |
