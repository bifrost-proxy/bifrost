# DESIGN.md 设计一致性系统真实场景测试

## 功能模块说明

本用例验证 Bifrost 已接入 `DESIGN.md` 设计系统契约，并且后续 Agent 在产品交互迭代时能发现、读取和校验该契约。

## 前置条件

- 在仓库根目录执行命令。
- 已安装 Node.js 与 pnpm。
- 本任务不启动 Bifrost 服务，不修改系统代理，不打开桌面托盘。

## 测试用例列表

### TC-DMS-01 DESIGN.md lint 可执行

操作步骤：

1. 执行 `pnpm design:lint`。
2. 查看命令退出码。
3. 查看输出中是否存在 `errors` 汇总。

预期结果：

- 命令退出码为 0。
- `DESIGN.md` 被真实解析，未出现 broken reference、重复 section 或结构性 error。

### TC-DMS-02 design spec 可输出当前规则

操作步骤：

1. 执行 `pnpm design:spec`。
2. 检查输出前 80 行。

预期结果：

- 命令退出码为 0。
- 输出包含 `DESIGN.md` 格式说明或 linting rules，证明 `@google/design.md` 已安装且规范可读取。

### TC-DMS-03 仓库级 design-md skill 可发现

操作步骤：

1. 执行 `test -f .agents/skills/design-md/SKILL.md`。
2. 执行 `sed -n '1,40p' .agents/skills/design-md/SKILL.md`。

预期结果：

- skill 文件存在。
- frontmatter 中包含 `name: design-md`。
- description 覆盖 WebUI、桌面壳、站点、文档视觉或交互变更。

### TC-DMS-04 Agent 读取链路指向根目录 DESIGN.md

操作步骤：

1. 执行 `rg -n "DESIGN.md|design/design-md-system.md|pnpm design:lint" .agents/skills/design-md/SKILL.md DESIGN.md design/design-md-system.md`。
2. 检查匹配结果。

预期结果：

- skill 明确要求读取根目录 `DESIGN.md`。
- skill 或方案文档明确要求修改 `DESIGN.md` 后运行 `pnpm design:lint`。
- 方案文档说明 `design/design-md-system.md` 是流程和验证说明。

### TC-DMS-05 package scripts 与依赖一致

操作步骤：

1. 执行 `node -e "const p=require('./package.json'); console.log(p.scripts['design:lint']); console.log(p.devDependencies['@google/design.md']);"`。
2. 检查输出。

预期结果：

- 第一行输出 `designmd lint DESIGN.md`。
- 第二行输出非空版本范围，例如 `^0.3.0`。

## 清理步骤

无需清理。本测试只读取仓库文件并执行本地设计规范 CLI，不产生服务进程、代理配置或临时数据目录。
