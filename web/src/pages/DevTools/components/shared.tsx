import type { CSSProperties, ReactNode } from "react";

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

export function HighlightedText({ text, query }: { text: string; query: string }) {
  const needle = query.trim();
  if (!needle) return <>{text}</>;
  const lower = text.toLowerCase();
  const lowerNeedle = needle.toLowerCase();
  const parts: ReactNode[] = [];
  let cursor = 0;
  let index = lower.indexOf(lowerNeedle);
  while (index !== -1) {
    if (index > cursor) parts.push(text.slice(cursor, index));
    parts.push(<mark key={`${index}-${parts.length}`} style={markStyle}>{text.slice(index, index + needle.length)}</mark>);
    cursor = index + needle.length;
    index = lower.indexOf(lowerNeedle, cursor);
  }
  if (cursor < text.length) parts.push(text.slice(cursor));
  return <>{parts}</>;
}



const markStyle: CSSProperties = {
  background: "#fde68a",
  color: "inherit",
  padding: "0 1px",
  borderRadius: 2,
};
