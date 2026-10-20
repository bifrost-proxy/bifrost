import type { TranslationKey } from "../../../i18n";

const TAB_SEARCH_PLACEHOLDER_KEYS: Record<string, TranslationKey> = {
  elements: "devtools.search.elements",
  network: "devtools.search.network",
  cookie: "devtools.search.cookies",
  local_storage: "devtools.search.localStorage",
  session_storage: "devtools.search.sessionStorage",
  console: "devtools.search.console",
};

export function tabSearchPlaceholderKey(key: string): TranslationKey {
  return TAB_SEARCH_PLACEHOLDER_KEYS[key] ?? "devtools.search.elements";
}

export function filterBySearch<T>(items: T[], query: string, stringify: (item: T) => string): T[] {
  const needle = query.trim().toLowerCase();
  if (!needle) return items;
  return items.filter((item) => stringify(item).toLowerCase().includes(needle));
}

export function includesSearch(text: string, query: string): boolean {
  const needle = query.trim().toLowerCase();
  return Boolean(needle) && text.toLowerCase().includes(needle);
}
