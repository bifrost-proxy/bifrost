# Data Dir Startup Initialization 设计方案

## 背景

Bifrost 的数据目录（`BIFROST_DATA_DIR`，默认 `$HOME/.bifrost` 或平台等价路径）承载了规则、证书、traffic、scripts、logs 等所有落盘状态。历史演进中，数据目录的子目录创建逻辑散落在多个模块——`rules` 与 `values` 在 `ConfigManager::new()` 建立，`body_cache` 在 traffic 首次写入时按需 `create_dir_all`，`scripts/*` 在脚本 loader 首次运行时创建，`replay` / `logs` 在各自入口首次访问时创建——结果是：

- 新 data dir 首次启动时，用户看到目录树按“懒创建”方式一步步长出来，很难判断某个模块是不是初始化到位。
- 老 data dir 迁移后，如果预期的子目录（例如 `body_cache`、`scripts/_sandbox`）不存在，只有对应模块触发一次实际写入才会补齐；未触发前相关 e2e / 人工回归可能出现“目录不存在”的偶发失败。
- CLI 前台、daemon、桌面端等不同启动路径分别维护 `create_dir_all` 兜底，代码重复且容易漂移。

本方案把“启动期应存在的数据目录清单”集中在 `crates/bifrost-storage/src/config_manager.rs` 里维护，并在 `ConfigManager::new()` 里通过 `init_data_dir()` 统一 `create_dir_all`，保证 **无论 data dir 是首次创建还是历史目录、无论哪种启动路径**，启动后子目录清单都完整。

## 用户目标验证清单

### 必须实现

- 在 `crates/bifrost-storage/src/config_manager.rs` 中集中维护 `expected_data_subdirs()` 常量清单。
- `ConfigManager::new(data_dir)` 里调用 `init_data_dir(&data_dir)`，`create_dir_all` 所有清单项。
- 覆盖 CLI 前台、`bifrost start --daemon`、desktop 主进程 / tray helper 等所有会实例化 `ConfigManager` 的启动路径。
- 首次创建 data dir 时在日志里打印 `Initialized data directory: <path>`；历史目录补齐时静默补齐，不打印“新建”。
- 当前纳入初始化清单的目录：
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
- 各模块自身的懒 `create_dir_all` 保留兜底，不因为集中初始化而删掉——避免运行时目录被人手删了之后模块直接崩。

### 必须不破坏

- 现有 unified config 加载 / 迁移 / 保存流程（`load_config_with_migration`、`save_config_to_file`）不变。
- `data_dir` 权限模型、文件模式（例如 `certs/` 下的私钥权限）不变。
- 已存在的子目录 / 用户创建的额外目录不被清理或改写。
- Windows 与 Linux/macOS 的路径分隔符 / 大小写敏感行为不变；`scripts/_sandbox` 在所有平台下都作为普通目录。
- `bifrost start --daemon` 与前台启动的 ConfigManager 实例化路径共享同一逻辑，避免 daemon 子进程二次初始化时报权限错。

### 必须真实验证

- 单测：用已存在的 `data_dir` 且只保留 `rules`、`body_cache` 两个子目录，`ConfigManager::new()` 之后清单里所有目录都存在。
- 单测 / 回归：`bifrost start --daemon` 在临时 `BIFROST_DATA_DIR` 下补齐所有清单目录。
- E2E：真实启动一次 daemon，`ls -1 $BIFROST_DATA_DIR` 至少包含 `scripts/_sandbox` 与 `scripts/_remote-cache/parser` 这两条“懒创建时代经常漏”的目录。

## 产品语义

### 数据目录是产品契约的一部分

`BIFROST_DATA_DIR` 里的子目录构成了产品与用户之间的隐式契约：文档、e2e 脚本、脚本引擎、traffic 中间件、备份 / 迁移工具都基于这个契约操作路径。将“启动即完备”作为契约提供，可以避免各模块因目录缺失而在运行时抛错、写日志、甚至崩溃。

### 集中入口 + 兜底两层防御

- **集中入口**：`ConfigManager::init_data_dir()` 在启动阶段一次性建齐清单。
- **模块兜底**：traffic、scripts、logs 等模块在真正使用目录前继续保留自己的 `create_dir_all`，防止运行时目录被外部删除后模块直接崩溃。

两层都存在的原因：模块兜底是运行时防御，集中入口是启动时正确性契约。用户如果在启动阶段就能看到完整目录树，可以判断部署是否正常；模块兜底则给运行时提供健壮性。

### 目录清单的产品语义

| 目录 | 语义 |
|------|------|
| `rules` | 用户规则 `.bifrost` 文件与内建规则 |
| `values` | 规则内可引用的变量 / 常量 |
| `certs` | 本地 CA、CA 材料 |
| `traffic` | traffic DB 与索引 |
| `body_cache` | traffic body 大对象磁盘 cache |
| `logs` | bifrost 主进程 / daemon / tray 日志 |
| `replay` | 请求重放临时快照 |
| `scripts` | 用户 script 根目录 |
| `scripts/request` | request 阶段脚本 |
| `scripts/response` | response 阶段脚本 |
| `scripts/decode` | body decode 脚本 |
| `scripts/parser` | 协议 parser 脚本 |
| `scripts/_remote-cache` | 远程脚本缓存根 |
| `scripts/_remote-cache/parser` | 远程 parser 脚本缓存 |
| `scripts/_sandbox` | 脚本沙盒工作目录 |

清单增删是 P1 变更：新增目录必须同步更新本文档、`init_data_dir` 与对应单测。

## 技术细节

### 关键代码结构

```rust
// crates/bifrost-storage/src/config_manager.rs
impl ConfigManager {
    pub fn new(data_dir: PathBuf) -> Result<Arc<Self>> {
        Self::init_data_dir(&data_dir)?;
        let config = Self::load_config_with_migration(&data_dir)?;
        // ... 其它字段初始化 ...
    }

    fn expected_data_subdirs() -> &'static [&'static str] {
        &[
            "rules", "values", "certs", "traffic", "body_cache",
            "logs", "replay",
            "scripts",
            "scripts/request", "scripts/response",
            "scripts/decode", "scripts/parser",
            "scripts/_remote-cache", "scripts/_remote-cache/parser",
            "scripts/_sandbox",
        ]
    }

    fn init_data_dir(dir: &Path) -> Result<()> {
        let is_new = !dir.exists();
        std::fs::create_dir_all(dir)?;
        for subdir in Self::expected_data_subdirs() {
            std::fs::create_dir_all(dir.join(subdir))?;
        }
        if is_new {
            info!("Initialized data directory: {}", dir.display());
        }
        Ok(())
    }
}
```

- `create_dir_all` 幂等，重复调用无副作用。
- `is_new` 判断只用于日志，不影响创建行为。
- 集中入口位于 `ConfigManager::new()`，是所有启动路径必经的构造函数；不需要在 CLI / desktop 里加额外调用。

### 失败语义

- `create_dir_all` 失败（例如磁盘只读 / 权限不足）会向上抛 `BifrostError::Io`，`ConfigManager::new()` 返回 Err；启动阶段直接失败，不会静默继续。
- 单个子目录失败即中断，不尝试继续创建后面的目录：把“无法初始化数据目录”作为不可继续的启动错误。
- daemon 场景：父进程构造 `ConfigManager` 时门禁失败，daemon 不会 fork；不会出现 daemon 已经启动、但目录缺失的状态。

### 模块兜底保留清单

以下模块保留自身的 `create_dir_all` 作为运行时兜底：

- traffic body cache：`bifrost-admin` 写入 body 之前。
- scripts loader：加载脚本时如果 `scripts/<kind>` 不在则重建。
- replay：`replay/` 首次写入时确认存在。
- logs：`logs/` 首次 rotate 时确认存在。

这些兜底不应该在正常启动路径下真正命中——出现命中意味着有人在运行时手动删了目录。

### 与 unified config 加载的关系

- `init_data_dir` 在 `load_config_with_migration` 之前调用，保证配置迁移时 `config.toml.bak` 备份路径所在目录存在。
- `UnifiedConfig::default_for_data_dir` 生成的默认配置仍指向清单里的路径。
- `with_data_dir(data_dir)` 只做路径回填，不影响目录创建。

## CLI / Web / Admin API 边界

- 不新增任何 CLI 命令。
- Admin API 不暴露目录初始化状态；如果需要诊断可以通过 `bifrost status` 或 `bifrost config` 输出中列出 data dir 路径，用户手动 `ls`。
- Web UI 不需要感知目录布局。

## 数据 / Sync 边界

- 初始化只在本机 `BIFROST_DATA_DIR` 下操作，不涉及远端 sync。
- 目录清单本身是代码内常量，不需要 sync。
- 若未来引入远程 data dir sync，该清单也应作为 sync 的一部分被复制到目标机器，但 sync 逻辑不属于本方案。

## 实现切分

### Phase 1：集中入口

- 在 `ConfigManager` 内新增 `expected_data_subdirs()` 与 `init_data_dir()`。
- 在 `ConfigManager::new()` 中调用 `init_data_dir` 并串到 config 加载之前。
- 单测：已存在 data dir 缺失部分子目录时全部补齐；新 data dir 首次初始化时打印日志。

### Phase 2：启动路径梳理

- 检查 CLI 前台、`bifrost start --daemon`、desktop 主进程 / tray helper 等所有构造 `ConfigManager` 的入口，确认都走同一个 `new()`。
- 移除各启动路径里显式的 `create_dir_all("scripts")` 等重复代码。
- 保留模块自身的兜底 `create_dir_all`。

### Phase 3：回归与文档

- 新增 `crates/bifrost-cli/tests/daemon_shutdown.rs` 或等价文件的启动回归：验证 daemon 在临时 data dir 下补齐清单。
- 更新 README / docs 的“数据目录结构”段落，把清单写清楚。
- `human_tests/` 相关 case（如 `human_tests/cli-start-stop-status.md`）在“首次启动”一步补一句“目录清单必须完整”。

### Phase 4：迭代增删

- 新增数据目录时按流程走：加入 `expected_data_subdirs()`、补单测、更新文档、跑一次 daemon 回归。
- 移除数据目录时评估兼容性：老版本的目录允许残留，不做主动清理。

## 测试方案

### 单元测试

- `test_missing_expected_data_subdirs_are_recreated`：已存在 data dir 只保留 `rules`、`body_cache`，`ConfigManager::new()` 之后 `expected_data_subdirs()` 中所有目录都存在。
- `test_first_time_data_dir_initialization_logs_new_dir`（可选）：新 data dir 首次初始化时日志包含 `Initialized data directory`。
- `test_init_data_dir_is_idempotent`：连续两次 `ConfigManager::new()` 不报错，目录状态不变。
- `test_init_data_dir_fails_when_path_is_read_only`（POSIX only）：只读挂载点下 `ConfigManager::new()` 返回 `Err`。

### 集成 / 回归

- `crates/bifrost-cli/tests/daemon_shutdown.rs`：`bifrost start --daemon` 在临时 `BIFROST_DATA_DIR` 下补齐清单；`bifrost stop` 后目录仍存在。
- `crates/bifrost-e2e/src/tests/body_cache.rs`：验证 body cache 目录在启动即存在。

### E2E

- 复用现有 `e2e-tests/tests/test_body_cache_sync_cleanup_admin_api.sh`：额外断言 `body_cache` 目录在首次启动即存在，不依赖任何请求写入。
- 新增或扩展一个启动脚本，断言 `scripts/_sandbox`、`scripts/_remote-cache/parser` 在首次启动后已存在（这两条曾经因懒创建缺失）。

### 真实场景测试

- 更新 `human_tests/cli-start-stop-status.md`：新增“首次启动后 `ls $BIFROST_DATA_DIR` 目录清单完整”一步。
- 使用临时 `BIFROST_DATA_DIR`、非 9900 端口、`BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`、`BIFROST_DISABLE_TRAY=1`、`--no-system-proxy`。

### 校验清单

- 先执行与启动行为相关的 E2E / 集成验证。
- 最后：
  - `cargo fmt --all -- --check`
  - `cargo clippy --all-targets --all-features -- -D warnings`
  - 按改动范围执行 `cargo test`（至少 `-p bifrost-storage`、`-p bifrost-cli`）
  - 按改动范围执行构建

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核 `expected_data_subdirs()` 是否覆盖所有 legacy 模块使用到的路径。
- 复核 `init_data_dir()` 幂等性、失败传播、日志噪声。
- 复核启动路径：CLI 前台 / daemon / desktop / tray helper 是否都走 `ConfigManager::new()`。
- 跑 `-p bifrost-storage` 单测与 daemon 启动回归。

### 第 2 轮

- 复查 `git status --short`、`git diff` 里的模块兜底代码是否被误删。
- 抽 1 条 E2E / human_tests 真实跑通，观察日志与目录清单。
- 确认新增 / 移除数据目录时的流程说明写清楚，未来演进不容易漏。

## 风险与决策点

- **模块兜底是否可以彻底删掉**：不建议。运行时目录可能被外部删除（用户手动 `rm -rf scripts/response`）；兜底保留可以让模块自愈。当前只做“启动即完备”，不改变运行时兜底。
- **目录清单增删**：清单变更是 P1 契约；PR review 必须同时更新本文档、`expected_data_subdirs()`、对应单测与 human_tests。
- **权限失败**：`create_dir_all` 失败直接阻断启动，避免半初始化的 daemon。极少数场景（例如宿主机 rootfs 只读）需要用户显式指定其它 `BIFROST_DATA_DIR`。
- **老 data dir 的自定义子目录**：本方案只增不删，用户自定义目录不受影响。
- **Windows 特殊性**：`scripts/_sandbox` 名字包含下划线，Windows 无冲突；如果未来出现 case-insensitive 冲突（例如 `Scripts` 与 `scripts`），需要在 clean 迁移里处理。
- **sync 场景**：如果引入 remote data dir sync，需要保证目标机器上先做一次 `init_data_dir`，否则可能 sync 到不存在的父路径。相关设计放到 sync 方案里，本方案不承担。
- **性能**：清单只有 15 项，每次启动 15 次 `create_dir_all` 时间可忽略；对启动关键路径无感。
