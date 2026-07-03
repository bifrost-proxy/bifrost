#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SITE_DIR="$ROOT_DIR/site"
TEMP_DOC="$ROOT_DIR/docs/future-docs-sync-probe.md"
TEMP_TARGET="$SITE_DIR/src/content/docs/reference/future-docs-sync-probe.md"
TEMP_DIST="$SITE_DIR/dist/reference/future-docs-sync-probe/index.html"
TEMP_EN_DOC="$ROOT_DIR/docs-en/future-english-docs-probe.md"
TEMP_EN_TARGET="$SITE_DIR/src/content/docs/en/reference/future-english-docs-probe.md"
TEMP_EN_DIST="$SITE_DIR/dist/en/reference/future-english-docs-probe/index.html"
EN_INDEX_DIST="$SITE_DIR/dist/en/reference/index.html"
EN_INSTALL_DIST="$SITE_DIR/dist/en/getting-started/installation/index.html"
EN_RULE_DIST="$SITE_DIR/dist/en/reference/rules/routing/index.html"
LEGACY_CLI_DIST="$SITE_DIR/dist/reference/getting-started/cli-quick-start/index.html"

cleanup() {
  local cleanup_status=0
  rm -f "$TEMP_DOC"
  rm -f "$TEMP_EN_DOC"
  rm -rf "$SITE_DIR/dist"
  pnpm --dir "$SITE_DIR" run docs:sync >/dev/null || cleanup_status=$?
  pnpm --dir "$SITE_DIR" run docs:verify >/dev/null || cleanup_status=$?
  if [[ "$cleanup_status" -ne 0 ]]; then
    echo "Warning: docs sync cleanup failed with status ${cleanup_status}" >&2
  fi
  return 0
}

trap cleanup EXIT

ensure_site_dependencies() {
  if [[ -x "$SITE_DIR/node_modules/.bin/astro" ]]; then
    return 0
  fi

  echo "Installing site dependencies for docs sync E2E..."
  pnpm --dir "$SITE_DIR" install --frozen-lockfile
}

if [[ -e "$TEMP_DOC" ]]; then
  echo "Temporary probe doc already exists: $TEMP_DOC" >&2
  exit 1
fi
if [[ -e "$TEMP_EN_DOC" ]]; then
  echo "Temporary English probe doc already exists: $TEMP_EN_DOC" >&2
  exit 1
fi

ensure_site_dependencies

pnpm --dir "$SITE_DIR" run docs:sync
pnpm --dir "$SITE_DIR" run docs:verify

zh_count=$(find "$ROOT_DIR/docs" -type f -name '*.md' | wc -l | tr -d ' ')
en_count=$(find "$ROOT_DIR/docs-en" -type f -name '*.md' | wc -l | tr -d ' ')
generated_en_count=$(find "$SITE_DIR/src/content/docs/en" -type f \( -name '*.md' -o -name '*.mdx' \) | wc -l | tr -d ' ')
test "$zh_count" = "$en_count"
test "$en_count" = "$generated_en_count"

node --input-type=module - "$ROOT_DIR" <<'NODE'
import fs from "node:fs";
import path from "node:path";
import { buildPagesSync } from "./site/scripts/docs-sync-lib.mjs";

const root = process.argv[2];
const pages = buildPagesSync(root);
const missing = pages.filter((page) => {
  const target = path.join(root, "site", "src", "content", "docs", page.target);
  return !fs.existsSync(target);
});

if (missing.length > 0) {
  throw new Error(`Missing generated docs targets: ${missing.map((page) => page.source).join(", ")}`);
}

console.log(`Verified ${pages.length} generated docs targets.`);
NODE

cat >"$TEMP_DOC" <<'EOF'
# Future Docs Sync Probe

This temporary document verifies that newly added docs are discovered without editing the site generator.

See [the CLI reference](./cli.md).
EOF

cat >"$TEMP_EN_DOC" <<'EOF'
# Future English Docs Probe

This temporary English document verifies that newly added English docs are discovered without editing the site generator.

See [the CLI reference](./cli.md).
EOF

pnpm --dir "$SITE_DIR" run docs:sync
pnpm --dir "$SITE_DIR" run docs:verify

test -f "$TEMP_TARGET"
grep -q '此页面由 `docs/future-docs-sync-probe.md` 自动同步生成。' "$TEMP_TARGET"
grep -q './cli/' "$TEMP_TARGET"
test -f "$TEMP_EN_TARGET"
grep -q 'This page is automatically synced from `docs-en/future-english-docs-probe.md`.' "$TEMP_EN_TARGET"
grep -q '../cli/' "$TEMP_EN_TARGET"

pnpm --dir "$SITE_DIR" run build
test -f "$TEMP_DIST"
grep -q 'Future Docs Sync Probe' "$TEMP_DIST"
test -f "$TEMP_EN_DIST"
grep -q 'Future English Docs Probe' "$TEMP_EN_DIST"
test -f "$EN_INDEX_DIST"
test -f "$EN_INSTALL_DIST"
test -f "$EN_RULE_DIST"
grep -q 'A proxy workbench for capturing traffic' "$SITE_DIR/dist/index.html"
grep -q 'href="/bifrost/docs/"' "$SITE_DIR/dist/index.html"
grep -q 'role="tablist"' "$SITE_DIR/dist/index.html"
grep -q 'data-lang="zh"' "$SITE_DIR/dist/index.html"
if grep -q '/_astro/' "$SITE_DIR/dist/index.html"; then
  echo "Static homepage must not include Astro runtime assets" >&2
  exit 1
fi
grep -q 'href="/bifrost/en/reference/"' "$SITE_DIR/dist/index.html"
test -f "$LEGACY_CLI_DIST"
grep -q '/bifrost/getting-started/cli-quick-start/' "$LEGACY_CLI_DIST"
pnpm --dir "$SITE_DIR" run site:verify-links

echo "Site docs sync E2E passed."
