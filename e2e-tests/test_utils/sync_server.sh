#!/bin/bash

sync_server_dir() {
    local script_dir
    script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
    cd "$script_dir/../.."/packages/bifrost-sync-server && pwd
}

sync_server_has_dist_entry() {
    local dir="${1:-$(sync_server_dir)}"
    [[ -f "$dir/dist/cli.js" ]]
}

sync_server_exec() {
    local dir="${1:-$(sync_server_dir)}"

    if sync_server_has_dist_entry "$dir"; then
        local candidate
        for candidate in \
            "${NODE_BIN:-}" \
            "$HOME/.local/share/mise/installs/node/22.22.0/bin/node" \
            "$HOME/.local/share/mise/installs/node/22/bin/node" \
            "$(command -v node 2>/dev/null || true)"; do
            if [[ -n "$candidate" && -x "$candidate" ]]; then
                printf '%q %q' "$candidate" "$dir/dist/cli.js"
                return 0
            fi
        done
        printf '%q %q' "node" "$dir/dist/cli.js"
        return 0
    fi

    if command -v pnpm >/dev/null 2>&1; then
        printf '%q %q %q %q' "pnpm" "exec" "tsx" "src/cli.ts"
        return 0
    fi

    printf '%q %q %q' "npx" "tsx" "src/cli.ts"
}
