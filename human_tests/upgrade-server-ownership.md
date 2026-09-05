# 升级后的 Server 归属与端口

## 目标与环境

macOS，使用本分支编译的 CLI 和 Desktop；测试仅使用 `.bifrost-e2e-upgrade-owner-*` 独立目录、动态端口，不启用系统代理或托盘，不停止正式服务。所有用例由真实 CLI/原生 Desktop 进程执行，断言来自 Admin API、runtime.json 和 Desktop 启动日志。

构建：

```bash
SKIP_FRONTEND_BUILD=1 cargo build -p bifrost-cli --bin bifrost
SKIP_FRONTEND_BUILD=1 cargo build --manifest-path desktop/src-tauri/Cargo.toml
```

## TC-USO-01 CLI 归属保留

启动隔离 CLI daemon，记录端口/PID，写入 CLI-owned 升级交接记录后打开真实 Desktop。
预期：Desktop 连接同一 PID，runtime_start_mode 为 daemon，端口不变，日志中没有 starting sidecar。

## TC-USO-02 Desktop 归属保留与自定义端口

写入 Desktop-owned 交接记录，配置另一个默认端口，再打开真实 Desktop。
预期：仅启动一个 Desktop core，使用交接记录的原端口，runtime_start_mode 为 desktop，API PID 与 runtime PID 一致。

## TC-USO-03 CLI 重启失败且端口空闲

写入 CLI-owned 交接记录但不启动 CLI，再打开真实 Desktop。
预期：显示 CLI-owned backend did not restart，未创建 Desktop core、未切换相邻端口，App 保持运行以显示错误。

## TC-USO-04 CLI 仍运行旧版本

运行真实 CLI，交接记录要求另一个版本，再打开真实 Desktop。
预期：报告 CLI 重启未完成，旧 CLI PID 保留，不停止它，不另启 Desktop core。

## 执行所有用例

```bash
python3 e2e-tests/tests/test_upgrade_server_ownership.py
```

用例使用真实进程且自动逐项断言。成功时清理目录；失败时保留路径及日志。实际执行结果记录在本次 PR 验证清单。
