export function tabSearchLabel(key: string): string {
  if (key === "local_storage") return "LocalStorage";
  if (key === "session_storage") return "SessionStorage";
  if (key === "cookie") return "Cookies";
  return key.replace(/^\w/, (value) => value.toUpperCase());
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
