---
name: design-md
description: Use when changing Bifrost WebUI, desktop shell UI, public site, docs visual style, interaction patterns, layout, copy density, colors, typography, spacing, or component styling. Reads DESIGN.md as the product design contract and validates it with @google/design.md.
---

# Bifrost DESIGN.md

This skill keeps Bifrost product interaction work consistent across agents and sessions.

## When to Use

Use this skill before modifying:

- `web/` UI, layout, theme, navigation, dashboards, tables, modals, panels, or interaction copy
- `desktop/` shell UI and desktop-specific chrome
- `site/`, `docs/`, or `docs-en/` visual style, homepage, navigation, screenshots, or docs theme
- any user-facing feature where color, density, component choice, spacing, typography, or visual hierarchy could drift

## Required Reading

1. Read `DESIGN.md` from the repository root.
2. For process and verification details, read `design/design-md-system.md`.
3. If the change touches site deployment, also follow the root `AGENTS.md` site deployment boundary.

## Working Rules

- Treat `DESIGN.md` YAML front matter as the normative design-token source.
- Treat the Markdown sections as rationale for how to apply those tokens.
- Prefer existing Ant Design tokens in `web/` and the existing CSS variables in `site/home/styles.css`.
- Keep operational UI dense, stable, and scan-friendly. Do not turn WebUI screens into marketing layouts.
- Preserve light/dark parity. If a new color is needed in WebUI, route it through Ant Design token usage, CSS variables, or a documented DESIGN.md token.
- Use icons for tool actions when an established icon exists; use text buttons only for clear commands.
- Keep card usage restrained: repeated items, modals, and genuinely framed tools.
- When editing `DESIGN.md`, run `pnpm design:lint`.

## Verification

For design-token or design-rule changes:

```bash
pnpm design:lint
```

For UI implementation changes, add the normal module tests plus human_tests coverage required by the repository. At minimum, verify light and dark theme behavior for WebUI changes and record the result in the relevant `human_tests/` document.
