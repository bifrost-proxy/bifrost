#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
TEMPLATE="$ROOT_DIR/crates/bifrost-admin/src/handlers/asr_jobs/daily_agent_template.md"

require_text() {
  local text="$1"
  if ! grep -Fq "$text" "$TEMPLATE"; then
    echo "missing expected template text: $text" >&2
    exit 1
  fi
}

require_text "## 报告内知识沉淀模块"
require_text "## 知识沉淀输出要求"
require_text '同一份 `{{report_dir}}YYYY-MM-DD-report.md`'
require_text "### 长期想法与效率方案"
require_text "### 方向决策与判断"
require_text "### 跨天待办追踪"
require_text "### 人物与协作背景"
require_text "### 学习与资料线索"
require_text "### 生活与状态线索"
require_text "### 术语与误识别词"
require_text "资料搜索结果"
require_text "可行性分析"
require_text "方案草案"
require_text "未能联网搜索"

if grep -Fq '`knowledge/' "$TEMPLATE"; then
  echo "template must not direct Daily Agent output into knowledge/* paths" >&2
  exit 1
fi

echo "ASR Daily Agent template report-module checks passed"
