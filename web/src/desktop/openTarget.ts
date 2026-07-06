import { buildBackendUrl } from "../runtime";

const EXTERNAL_OPEN_SCHEMES = new Set([
  "http:",
  "https:",
  "mailto:",
  "bifrost:",
  "macappstore:",
]);

const BACKEND_RELATIVE_PREFIXES = ["/_bifrost/", "/api/", "/public/"];

export function resolveDesktopOpenTarget(
  rawUrl: string,
  currentHref = window.location.href,
  buildBackendTarget = buildBackendUrl,
): string | null {
  if (!rawUrl) {
    return null;
  }

  if (BACKEND_RELATIVE_PREFIXES.some((prefix) => rawUrl.startsWith(prefix))) {
    return buildBackendTarget(rawUrl);
  }

  try {
    const parsed = new URL(rawUrl, currentHref);
    const current = new URL(currentHref);
    if (parsed.protocol === current.protocol && parsed.origin === current.origin) {
      return null;
    }
    return EXTERNAL_OPEN_SCHEMES.has(parsed.protocol) ? parsed.toString() : null;
  } catch {
    return null;
  }
}
