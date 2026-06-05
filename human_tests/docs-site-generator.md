# Docs Site Generator / English Docs

## 功能模块说明

验证 Bifrost 面向用户的 README、`docs/` 文档和文档站点支持中文与英文双语阅读；英文文档作为独立 `README.en.md` 与 `docs-en/` 目录存在，并能同步生成站点 `/en/...` 路由。

## 前置条件

1. 当前工作目录位于仓库根目录。
2. Node.js 22+ 与 pnpm 可用。
3. 执行命令前确保 site 依赖已安装：
   ```bash
   pnpm --dir site install
   ```

## 测试用例列表

### TC-DSG-01 当前 docs 与 docs-en 全量同步

操作步骤：
1. 执行 `pnpm --dir site run docs:sync`。
2. 执行 `pnpm --dir site run docs:verify`。
3. 执行 `find docs -type f -name '*.md' | wc -l`，记录中文 docs 文档数量。
4. 执行 `find docs-en -type f -name '*.md' | wc -l`，记录英文 docs 文档数量。
5. 执行 `find site/src/content/docs/en -type f \( -name '*.md' -o -name '*.mdx' \) | wc -l`。
6. 检查 `site/src/content/docs/en/reference/index.mdx` 是否包含 `This page is automatically synced from \`docs-en/README.md\``。
7. 检查 `site/src/content/docs/en/getting-started/installation.mdx` 是否存在。
8. 检查 `site/src/content/docs/en/reference/rules/routing.md` 是否存在。

预期结果：
- `docs:sync` 成功。
- `docs:verify` 输出通过，覆盖中文与英文所有 docs 文档。
- `docs/` 与 `docs-en/` Markdown 数量一致。
- 英文生成目录存在 `/en/reference/`、`/en/getting-started/` 和 `/en/reference/rules/`。

### TC-DSG-02 README 双语切换入口

操作步骤：
1. 检查 `README.md` 是否包含 `[English](README.en.md)`。
2. 检查 `README.md` 是否包含 `[English docs](docs-en/README.md)`。
3. 检查 `README.en.md` 是否包含 `[中文](README.md)`。
4. 检查 `README.en.md` 是否包含 `[English documentation](docs-en/README.md)`。
5. 检查 `README.en.md` 的文档索引是否链接到 `docs-en/getting-started.md`、`docs-en/cli.md`、`docs-en/rules/README.md`。

预期结果：
- 中文 README 可快速切换英文 README 和英文 docs。
- 英文 README 可快速切回中文 README 和中文 docs。
- 英文 README 文档索引全部指向 `docs-en/` 独立路径。

### TC-DSG-03 docs 目录双语切换入口

操作步骤：
1. 检查 `docs/README.md` 是否包含 `[English](../docs-en/README.md)`。
2. 检查 `docs-en/README.md` 是否包含 `[中文](../docs/README.md)`。
3. 检查 `docs-en/getting-started.md` 是否包含 `[中文](../docs/getting-started.md)`。
4. 检查 `docs-en/rules/README.md` 是否包含 `[中文](../../docs/rules/README.md)`。

预期结果：
- 中文 docs 总览能进入英文 docs 总览。
- 英文 docs 每个页面顶部提供中文对应页面入口。
- 英文 rules 子目录页面的相对中文链接正确。

### TC-DSG-04 站点首页与 404 英文入口

操作步骤：
1. 执行 `pnpm --dir site run build`。
2. 检查 `site/dist/index.html` 是否包含 `English Docs`。
3. 检查 `site/dist/index.html` 是否包含 `/en/reference/`。
4. 检查 `site/dist/404.html` 是否包含 `English docs:`。
5. 检查 `site/dist/404.html` 是否包含 `/en/reference/`。

预期结果：
- 官网首页导航提供英文文档入口。
- 首页 hero 区域提供英文 Guide 入口。
- 404 页面提供英文文档恢复入口。

### TC-DSG-05 英文站点真实构建产物与内链

操作步骤：
1. 执行 `pnpm --dir site run build`。
2. 检查 `site/dist/en/reference/index.html` 是否存在。
3. 检查 `site/dist/en/getting-started/installation/index.html` 是否存在。
4. 检查 `site/dist/en/reference/rules/routing/index.html` 是否存在。
5. 执行 `pnpm --dir site run site:verify-links`。

预期结果：
- Astro/Starlight 真实构建成功。
- 英文 `/en/...` 页面生成 HTML 产物。
- 全站站内链接验证通过，无英文路径 404 或资源缺失。

### TC-DSG-06 未来新增英文文档自动纳入并清理

操作步骤：
1. 临时创建 `docs-en/future-english-docs-probe.md`，内容包含一级标题 `# Future English Docs Probe` 和链接 `[CLI](./cli.md)`。
2. 执行 `pnpm --dir site run docs:sync`。
3. 检查 `site/src/content/docs/en/reference/future-english-docs-probe.md` 是否存在。
4. 检查生成文件中是否包含 `This page is automatically synced from \`docs-en/future-english-docs-probe.md\`.`。
5. 检查生成文件中的 `./cli.md` 链接是否改写为相邻英文站点路由 `../cli/`。
6. 删除临时文档后再次执行 `pnpm --dir site run docs:sync && pnpm --dir site run docs:verify`。
7. 检查 `site/src/content/docs/en/reference/future-english-docs-probe.md` 是否不存在。

预期结果：
- 临时新增英文文档无需修改生成器清单即可生成到 `en/reference/future-english-docs-probe.md`。
- 相对 Markdown 链接被改写为英文站点路由。
- 删除临时文档并重新同步后，临时生成页面被清理，`docs:verify` 仍通过。

## 清理步骤

1. 删除 `docs-en/future-english-docs-probe.md`。
2. 删除 `site/dist`。
3. 执行 `pnpm --dir site run docs:sync && pnpm --dir site run docs:verify`。
