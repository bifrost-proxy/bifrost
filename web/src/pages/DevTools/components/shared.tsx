import type { CSSProperties, ReactNode } from "react";

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
  background: "var(--devtools-search-bg)",
  color: "inherit",
  padding: "0 1px",
  borderRadius: 2,
};
