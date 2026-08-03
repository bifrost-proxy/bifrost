#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT_DIR"

critical_pattern='Rules 页面置顶并保护全局 Default|Rules 编辑器在首次 syntax 请求失败后恢复协议智能提示|Values 页面完成 CRUD|Values 页面支持 bifrost-file|Scripts 页面完成创建、测试、push|加载流量列表并显示详情|Traffic toolbar exposes one global Breakpoint|AI new chat landing sends pasted images|WebKit 与 Chromium 中侧栏内容完整且小窗口可滚动'

exec pnpm --dir web exec playwright test \
  tests/ui/admin-rules-values.spec.ts \
  tests/ui/admin-scripts.spec.ts \
  tests/ui/traffic.spec.ts \
  tests/ui/breakpoint-ui.spec.ts \
  tests/ui/agent-chat.spec.ts \
  tests/ui/sidebar-webkit-compatibility.spec.ts \
  --grep "$critical_pattern" \
  --workers=1
