#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SITE_DIR="$ROOT_DIR/site"
TEMP_DOC="$ROOT_DIR/docs/future-docs-sync-probe.md"
TEMP_TARGET="$SITE_DIR/src/content/docs/reference/future-docs-sync-probe.md"
TEMP_DIST="$SITE_DIR/dist/reference/future-docs-sync-probe/index.html"
LEGACY_CLI_DIST="$SITE_DIR/dist/reference/getting-started/cli-quick-start/index.html"

cleanup() {
  rm -f "$TEMP_DOC"
  rm -rf "$SITE_DIR/dist"
  pnpm --dir "$SITE_DIR" run docs:sync >/dev/null
  pnpm --dir "$SITE_DIR" run docs:verify >/dev/null
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

ensure_site_dependencies

pnpm --dir "$SITE_DIR" run docs:sync
pnpm --dir "$SITE_DIR" run docs:verify

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

pnpm --dir "$SITE_DIR" run docs:sync
pnpm --dir "$SITE_DIR" run docs:verify

test -f "$TEMP_TARGET"
grep -q '此页面由 `docs/future-docs-sync-probe.md` 自动同步生成。' "$TEMP_TARGET"
grep -q './cli/' "$TEMP_TARGET"

pnpm --dir "$SITE_DIR" run build
test -f "$TEMP_DIST"
grep -q 'Future Docs Sync Probe' "$TEMP_DIST"
test -f "$LEGACY_CLI_DIST"
grep -q '/bifrost/getting-started/cli-quick-start/' "$LEGACY_CLI_DIST"
pnpm --dir "$SITE_DIR" run site:verify-links

echo "Site docs sync E2E passed."
