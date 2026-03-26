#!/usr/bin/env bash
set -euo pipefail

PORT="${1:-9900}"
HOST="${2:-github.com}"

echo "== GitHub Actions workflow list =="
bifrost search "actions/workflow-runs" --url --host "$HOST" --limit 20 -f json-pretty

echo
echo "== GitHub Actions workflow row partial =="
bifrost search "actions/workflow-run/" --url --host "$HOST" --limit 20 -f json-pretty

echo
echo "== GitHub Actions run page =="
bifrost search "actions/runs/" --url --host "$HOST" --limit 50 -f json-pretty

echo
echo "== GitHub Actions graph partial =="
bifrost search "graph_partial" --url --host "$HOST" --limit 20 -f json-pretty

echo
echo "== GitHub Actions matrix partial =="
bifrost search "graph/matrix/" --url --host "$HOST" --limit 20 -f json-pretty

echo
echo "== GitHub Actions job page =="
bifrost search "/job/" --url --host "$HOST" --limit 50 -f json-pretty

echo
echo "== GitHub Actions step logs =="
bifrost search "/checks/" --url --host "$HOST" --limit 50 -f json-pretty
