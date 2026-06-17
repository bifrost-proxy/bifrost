# Data Dir Startup Initialization

## 背景

现有数据目录初始化逻辑主要集中在 `ConfigManager::new()`，但部分目录仍在脚本、重放、日志等模块首次使用时才按需创建。对于已经存在的旧 `data_dir`，如果其中缺少 `body_cache`、`scripts`、`replay` 等预期目录，启动阶段不容易显式补齐，行为也分散。

## 本次调整

- 在 `crates/bifrost-storage/src/config_manager.rs` 中集中维护启动期应存在的数据目录清单。
- `ConfigManager::init_data_dir()` 在启动时统一执行 `create_dir_all`，无论 `data_dir` 是首次创建还是历史目录缺项，都自动补齐。
- 当前纳入启动初始化的目录：
  - `rules`
  - `values`
  - `certs`
  - `traffic`
  - `body_cache`
  - `logs`
  - `replay`
  - `scripts`
  - `scripts/request`
  - `scripts/response`
  - `scripts/decode`
  - `scripts/parser`
  - `scripts/_remote-cache`
  - `scripts/_remote-cache/parser`
  - `scripts/_sandbox`

## 实现逻辑

- 复用 `ConfigManager::new()` 作为统一入口，避免 CLI 前台、daemon、桌面端等不同启动路径分别维护目录初始化逻辑。
- 保留各模块自身的兜底 `create_dir_all`，但把“启动前自检并补齐数据目录”前移到配置管理层。

## 测试方案

- `crates/bifrost-storage/src/config_manager.rs`
  - 新增单测，验证已有 `data_dir` 且子目录不完整时，重新初始化会补齐所有预期目录。
- `crates/bifrost-cli/tests/daemon_shutdown.rs`
  - 新增启动回归测试，验证 `bifrost start --daemon` 会在临时数据目录下补齐预期目录。

## 校验要求

- 先执行与启动行为相关的 E2E / 集成验证。
- 最后执行 `cargo fmt --all -- --check`
- 执行 `cargo clippy --all-targets --all-features -- -D warnings`
- 按改动范围执行测试
- 执行按改动范围的构建

## 文档更新

- README 的数据目录结构需要与当前启动初始化行为保持一致。
