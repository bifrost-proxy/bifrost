# Docs Site Generator

## 功能模块说明

验证文档站点生成器能够覆盖 `docs/` 下当前所有 Markdown 文档，并且未来新增的 `docs/**/*.md` 文档无需修改生成器清单即可同步、进入构建产物并可部署。

## 前置条件

1. 当前工作目录位于仓库根目录。
2. Node.js 与 pnpm 可用。
3. 执行命令前先初始化 shell：
   ```bash
   source ~/.zshrc
   ```

## 测试用例列表

### TC-DSG-01 当前 docs 全量同步

操作步骤：
1. 执行 `source ~/.zshrc && pnpm --dir site run docs:sync`。
2. 执行 `source ~/.zshrc && pnpm --dir site run docs:verify`。
3. 执行 `source ~/.zshrc && find docs -type f -name '*.md' | sort`，记录当前 docs 文档数量。
4. 执行 `source ~/.zshrc && find site/src/content/docs -type f \( -name '*.md' -o -name '*.mdx' \) -print0 | xargs -0 rg -n "此页面由 \`docs/"`，确认每个当前 docs 文档都有生成来源记录。

预期结果：
- `docs:sync` 成功。
- `docs:verify` 输出通过，覆盖当前所有 docs 文档。
- `site/src/content/docs/` 下存在 `reference/cli.md`、`reference/values.md`、`reference/replay.md`、`reference/architecture.md`、`reference/agent-skill.md` 等此前遗漏文档。

### TC-DSG-02 未来新增文档自动纳入

操作步骤：
1. 临时创建 `docs/future-docs-sync-probe.md`，内容包含一级标题 `# Future Docs Sync Probe` 和链接 `[CLI](./cli.md)`。
2. 执行 `source ~/.zshrc && pnpm --dir site run docs:sync`。
3. 检查 `site/src/content/docs/reference/future-docs-sync-probe.md` 是否存在。
4. 检查生成文件中是否包含 `此页面由 \`docs/future-docs-sync-probe.md\` 自动同步生成。`。
5. 检查生成文件中的 `./cli.md` 链接是否改写为站点路由 `./cli/`。
6. 删除临时文档后再次执行 `source ~/.zshrc && pnpm --dir site run docs:sync && pnpm --dir site run docs:verify`。

预期结果：
- 临时新增文档无需修改生成器清单即可生成到 `reference/future-docs-sync-probe.md`。
- 相对 Markdown 链接被改写为站点路由。
- 删除临时文档并重新同步后，临时生成页面被清理，`docs:verify` 仍通过。

### TC-DSG-03 真实部署构建路径

操作步骤：
1. 执行 `source ~/.zshrc && pnpm --dir site run build`。
2. 检查 `site/dist/reference/cli/index.html` 是否存在。
3. 检查 `site/dist/reference/values/index.html` 是否存在。
4. 检查 `site/dist/reference/rules/rule-priority/index.html` 是否存在。
5. 检查 `site/dist/reference/getting-started/cli-quick-start/index.html` 是否存在。

预期结果：
- Astro/Starlight 真实构建成功。
- 当前 docs 文档对应的 HTML 产物存在。
- 历史错误深链 `/bifrost/reference/getting-started/cli-quick-start/` 生成静态重定向入口，不再返回 404。
- 构建过程中会先执行同步校验，若未来 docs 文档漏生成则构建失败。

### TC-DSG-04 全站站内链接无缺失目标

操作步骤：
1. 执行 `source ~/.zshrc && pnpm --dir site run build`。
2. 执行 `source ~/.zshrc && pnpm --dir site run site:verify-links`。

预期结果：
- 构建后的 `site/dist/**/*.html` 中所有站内 `href` / `src` 都能解析到存在的页面或静态资源。
- 如果未来新增 docs 文档引入错误相对链接、错误绝对路径或缺失静态资源，`site:verify-links` 会失败并列出来源页面与缺失目标。

### TC-DSG-05 清理后部署产物无探针残留

操作步骤：
1. 临时创建 `docs/future-docs-sync-probe.md` 并执行 `source ~/.zshrc && pnpm --dir site run build`。
2. 确认 `site/dist/reference/future-docs-sync-probe/index.html` 存在。
3. 删除 `docs/future-docs-sync-probe.md`。
4. 再次执行 `source ~/.zshrc && pnpm --dir site run build`。
5. 检查 `site/dist/reference/future-docs-sync-probe/index.html` 是否不存在。

预期结果：
- 临时 docs 文档存在时会进入构建产物。
- 临时 docs 文档删除后，构建前清理 `site/dist`，旧 HTML 不会残留到部署产物。

## 清理步骤

1. 删除 `docs/future-docs-sync-probe.md`。
2. 删除 `site/dist`。
3. 执行 `source ~/.zshrc && pnpm --dir site run docs:sync && pnpm --dir site run docs:verify`。
