---
version: alpha
name: Bifrost Product Interface
description: Visual identity and interaction contract for Bifrost WebUI, desktop shell, public site, and documentation surfaces.
colors:
  primary: "#111816"
  on-primary: "#FFFFFF"
  secondary: "#13A58F"
  on-secondary: "#FFFFFF"
  tertiary: "#3578E5"
  on-tertiary: "#FFFFFF"
  accent: "#D57926"
  neutral: "#F7F8F4"
  surface: "#FFFFFF"
  surface-soft: "#F0F4F3"
  surface-dark: "#151D1A"
  text: "#111816"
  text-muted: "#5D6965"
  border: "#DCE4DF"
  border-strong: "#C5D2CB"
  success: "#52C41A"
  warning: "#FA8C16"
  danger: "#CF1322"
typography:
  display:
    fontFamily: "Inter, ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, Segoe UI, sans-serif"
    fontSize: 64px
    fontWeight: 800
    lineHeight: 0.92
    letterSpacing: 0em
  page-title:
    fontFamily: "-apple-system, BlinkMacSystemFont, Segoe UI, Roboto, Helvetica Neue, Arial, Noto Sans, sans-serif"
    fontSize: 26px
    fontWeight: 700
    lineHeight: 1.3
    letterSpacing: 0em
  section-title:
    fontFamily: "-apple-system, BlinkMacSystemFont, Segoe UI, Roboto, Helvetica Neue, Arial, Noto Sans, sans-serif"
    fontSize: 20px
    fontWeight: 700
    lineHeight: 1.35
    letterSpacing: 0em
  body:
    fontFamily: "-apple-system, BlinkMacSystemFont, Segoe UI, Roboto, Helvetica Neue, Arial, Noto Sans, sans-serif"
    fontSize: 14px
    fontWeight: 400
    lineHeight: 1.6
    letterSpacing: 0em
  caption:
    fontFamily: "-apple-system, BlinkMacSystemFont, Segoe UI, Roboto, Helvetica Neue, Arial, Noto Sans, sans-serif"
    fontSize: 12px
    fontWeight: 400
    lineHeight: 1.35
    letterSpacing: 0em
  code:
    fontFamily: "ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace"
    fontSize: 13px
    fontWeight: 400
    lineHeight: 1.7
    letterSpacing: 0em
rounded:
  xs: 3px
  sm: 4px
  md: 6px
  lg: 8px
  xl: 12px
  pill: 999px
spacing:
  xs: 4px
  sm: 8px
  md: 12px
  lg: 16px
  xl: 24px
  xxl: 32px
  page-gutter: 40px
  site-max-width: 1180px
components:
  button-primary:
    backgroundColor: "{colors.primary}"
    textColor: "{colors.on-primary}"
    typography: "{typography.body}"
    rounded: "{rounded.lg}"
    padding: 12px
    height: 46px
  button-secondary:
    backgroundColor: "{colors.surface}"
    textColor: "{colors.text}"
    typography: "{typography.body}"
    rounded: "{rounded.lg}"
    padding: 12px
    height: 46px
  tool-button:
    backgroundColor: "{colors.surface-soft}"
    textColor: "{colors.text}"
    typography: "{typography.caption}"
    rounded: "{rounded.md}"
    padding: 8px
    height: 32px
  panel:
    backgroundColor: "{colors.surface}"
    textColor: "{colors.text}"
    rounded: "{rounded.lg}"
    padding: 16px
  sidebar-item-active:
    backgroundColor: "{colors.surface-soft}"
    textColor: "{colors.primary}"
    rounded: "{rounded.md}"
    padding: 8px
  status-success:
    backgroundColor: "{colors.success}"
    textColor: "{colors.primary}"
    rounded: "{rounded.pill}"
    padding: 4px
  status-warning:
    backgroundColor: "{colors.warning}"
    textColor: "{colors.primary}"
    rounded: "{rounded.pill}"
    padding: 4px
  status-danger:
    backgroundColor: "{colors.danger}"
    textColor: "{colors.on-primary}"
    rounded: "{rounded.pill}"
    padding: 4px
  site-hero-surface:
    backgroundColor: "{colors.neutral}"
    textColor: "{colors.text}"
    rounded: "{rounded.lg}"
    padding: 24px
  dark-panel:
    backgroundColor: "{colors.surface-dark}"
    textColor: "{colors.on-primary}"
    rounded: "{rounded.lg}"
    padding: 16px
  muted-panel:
    backgroundColor: "{colors.surface}"
    textColor: "{colors.text-muted}"
    rounded: "{rounded.md}"
    padding: 12px
  divider:
    backgroundColor: "{colors.border}"
    textColor: "{colors.primary}"
    height: 1px
  divider-strong:
    backgroundColor: "{colors.border-strong}"
    textColor: "{colors.primary}"
    height: 1px
  accent-badge:
    backgroundColor: "{colors.accent}"
    textColor: "{colors.primary}"
    rounded: "{rounded.pill}"
    padding: 4px
---

## Overview

Bifrost should feel like a precise proxy workbench: calm, technical, and fast to scan. The product is built for repeated operational use by developers and coding agents, so the interface favors dense information, stable controls, and clear state over decorative storytelling.

The public site may be more editorial, but it must still feel engineered. Use the site palette from `site/home/styles.css`: warm off-white surfaces, deep ink text, teal for Bifrost identity, blue for navigational emphasis, and amber for warnings or temporal highlights.

## Colors

- **Primary (#111816):** Deep ink for product identity, primary buttons, and high-contrast text.
- **Secondary (#13A58F):** Bifrost teal for brand signals, successful proxy state, and identity moments.
- **Tertiary (#3578E5):** Operational blue for selected navigation, links, and focused technical actions.
- **Accent (#D57926):** Amber for warnings, time-sensitive state, and cautionary highlights.
- **Neutral (#F7F8F4):** Warm page foundation for site and documentation surfaces.
- **Surface (#FFFFFF):** Main workbench panels and cards in light mode.
- **Surface Dark (#151D1A):** Dark-mode panel base when a custom surface is needed beyond Ant Design tokens.

WebUI implementation should prefer Ant Design tokens such as `token.colorBgContainer`, `token.colorText`, `token.colorPrimary`, `token.colorBorder`, and `token.colorFillSecondary`. Hard-coded colors are acceptable only for protocol/status semantics already standardized in this file, syntax highlighting, or generated static site assets.

## Typography

Use native system fonts for WebUI and desktop shell controls. The public site may use Inter first, falling back to the same system stack. Monospace text should use `ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace`.

Typography should match the density of the surface. Hero-scale type is reserved for the public homepage. WebUI panels, modals, sidebars, tables, and status areas use compact titles and 12-14px supporting text.

## Layout

WebUI layouts are workbench layouts: fixed navigation, constrained toolbars, scrollable content regions, and stable split panes. Components that change state must keep stable dimensions so hover states, labels, badges, and loading text do not resize the surrounding layout.

Public site layouts use a max width around 1180px, 40px outer gutters on desktop, and 8px-radius framed previews. Documentation keeps the VitePress-style reading model with Bifrost colors rather than inventing a separate visual language.

## Elevation & Depth

Use tonal layers, borders, and modest shadows. Default WebUI depth comes from Ant Design components. Site preview panels may use a soft shadow such as `0 24px 60px rgba(18, 34, 30, 0.13)`. Avoid heavy glass effects except in desktop shell chrome where the existing implementation already uses translucent platform treatment.

## Shapes

Bifrost is mostly rectilinear: 4px for inline code and low-level cells, 6px for compact tools, 8px for cards and site panels, 12px for expanded floating overlays, and pill radius only for badges, status dots, and capsule-style indicators.

## Components

Buttons use icon-first affordances when an established icon exists. Text buttons are reserved for clear commands or navigation labels. Cards should represent individual repeated items, modals, or framed tool surfaces; do not place cards inside other cards.

Navigation items should expose selected state with both color and background. Status indicators must use explicit success/warning/danger semantics, not hue-only decoration. Data-heavy views should prefer tables, split panes, virtual lists, filters, tabs, segmented controls, and compact toolbars.

## Do's and Don'ts

- Do read this file before changing `web/`, `site/`, `docs/`, `docs-en/`, desktop shell UI, or user-facing interaction copy.
- Do keep WebUI light and dark themes equivalent; new colors should flow through Ant Design tokens or CSS variables.
- Do use teal for Bifrost identity, blue for selection/navigation, and amber/red only for state that needs attention.
- Do keep operational surfaces dense, predictable, and built for scanning.
- Do run `pnpm design:lint` after editing this file.
- Don't introduce a separate visual identity for docs, public site, desktop, and WebUI.
- Don't use decorative gradient blobs, one-note purple/blue palettes, or oversized marketing layouts inside the WebUI.
- Don't hard-code local filesystem paths in design docs or test instructions.
- Don't use large rounded cards for every section; reserve framing for actual tools, repeated items, and modals.
