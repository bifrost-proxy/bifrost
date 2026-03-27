# Bifrost 项目开发规则

## 开发需求标准流程
1. 分析现状代码与 `design/` 目录文件，明确需求范围与影响面
2. 设计技术方案：在 `design/` 下新增或更新对应模块文档（文件名：`功能模块名.md`）
3. 实现功能与必要测试：新增功能或修复 bug 后按“E2E 测试要求”执行
4. 更新文档：如涉及新功能 / API / 配置变更，同步更新相关文档（见下）
5. 项目校验：提交前必须执行 rust-project-validate，并在开发完成后至少执行一次 `cargo test --workspace --all-features`，避免 CI 才暴露工作区级失败
6. 收尾清理：清理临时数据目录，避免资源膨胀

## 技术方案（design 目录）
- 方案文档必须位于 `design/`，每个功能模块维护一个独立文件：`功能模块名.md`
- 同模块方案持续增量更新，保持与代码实现同步
- 技术方案必须包含：
  - 功能模块详细描述
  - 实现逻辑
  - 依赖项
  - 测试方案（含 e2e）
  - 校验要求（含 rust-project-validate）
  - 文档更新要求（明确需更新的文件）

## 文档更新要求

- 如果修改涉及新功能、API 变更或配置变更，需要同步更新 `README.md`
- 如果添加了新的协议，需要更新 README 中的协议列表
- 如果添加了新的 Hook，需要更新 README 中的 Hook 表格
- 如果修改了命令行参数或配置选项，需要更新相关文档说明

## 启动服务的要求

- 启动服务时必须配置临时数据目录，避免覆盖正在运行的服务数据
- 必须采用立即编译运行的方式，避免使用已编译的二进制文件
- 示例：

```bash
BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- start -p 8800 --unsafe-ssl
```

## E2E 测试要求

- 添加新功能或修复 bug 后需要创建/执行端到端测试进行验证
- 使用技能：e2e-test

## 工作区测试要求

- 开发完成后，提交前必须至少执行一次 `cargo test --workspace --all-features`
- 目的：提前发现仅在工作区聚合、全 feature 组合或 CI 路径下出现的失败，避免代码提交后才在 CI 暴露问题
- 如果该命令失败，需要先定位并处理，或在提交说明中明确标注阻塞原因与影响范围

## 日志配置规范

### 日志级别优先级（从高到低）

1. `RUST_LOG` 环境变量 - 支持精细化控制，如 `RUST_LOG=bifrost_proxy=debug,info`
2. 命令行参数 `-l/--log-level` - 仅 bifrost-cli 支持
3. 默认值 `info`

### 各入口点日志初始化规范

| 入口点      | 初始化方式                     | 说明                              |
| ----------- | ------------------------------ | --------------------------------- |
| bifrost-cli | `init_logging(&cli.log_level)` | 使用 bifrost-core 统一函数        |
| bifrost-e2e | `tracing_subscriber::fmt()`    | 从 `--verbose` 和 `RUST_LOG` 读取 |

### verbose_logging 双轨机制

项目使用两套日志控制机制：

1. **tracing 级别** - 控制全局日志输出（通过 `EnvFilter`）
2. **verbose_logging 布尔值** - 控制详细业务日志（规则匹配、请求转发等）

规则：当日志级别为 `debug` 或 `trace` 时，`verbose_logging` 自动设为 `true`

```rust
let verbose_logging = matches!(log_level.as_str(), "debug" | "trace");
```

### 新增入口点时的要求

1. 必须支持 `RUST_LOG` 环境变量优先
2. 如果有命令行参数，作为 `RUST_LOG` 未设置时的回退
3. 如果需要传递 `verbose_logging`，必须根据日志级别正确设置
4. 使用 `bifrost_core::init_logging()` 统一初始化（已支持 RUST_LOG 回退）

### 关键文件位置

- 日志初始化：`crates/bifrost-core/src/logging.rs`
- CLI 入口：`crates/bifrost-cli/src/main.rs`
- E2E 入口：`crates/bifrost-e2e/src/main.rs`
- ProxyConfig 定义：`crates/bifrost-proxy/src/server.rs`

### 日志要求

- 所有日志输出必须符合 tracing 标准，包含 `target`、`level`、`message` 等字段
- 所有日志级别必须根据 `RUST_LOG` 环境变量或 `--log-level` 参数进行控制
- 所有日志输出必须包含 `file`、`line`、`module` 等字段，方便定位问题
- 开发新需求时务必添加详细的日志，方便调试和问题定位

## 数据库表结构
如果涉及新的需求需要修改数据库表，请直接修改表协议，我们不考虑对旧数据兼容，当协议更新版本时，直接删除旧版本数据库，重建数据即可。
