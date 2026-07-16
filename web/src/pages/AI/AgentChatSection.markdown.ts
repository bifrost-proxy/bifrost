export type AgentChatCitationSource = {
  iconSrc: string;
  label: string;
};

type MarkdownAstNode = {
  type?: unknown;
  tagName?: unknown;
  value?: unknown;
  properties?: Record<string, unknown>;
  children?: unknown[];
};

function asMarkdownAstNode(value: unknown): MarkdownAstNode | undefined {
  return value && typeof value === "object" ? (value as MarkdownAstNode) : undefined;
}

export function isChatGptWebCitationFavicon(src?: string | null) {
  if (!src) {
    return false;
  }
  try {
    const url = new URL(src);
    return (
      url.protocol === "https:" &&
      (url.hostname === "www.google.com" || url.hostname === "google.com") &&
      url.pathname === "/s2/favicons" &&
      Boolean(url.searchParams.get("domain"))
    );
  } catch {
    return false;
  }
}

export function chatGptWebCitationSourcesFromNode(
  value: unknown,
): AgentChatCitationSource[] | null {
  const node = asMarkdownAstNode(value);
  if (node?.type !== "element" || node.tagName !== "a" || !Array.isArray(node.children)) {
    return null;
  }

  const sources: AgentChatCitationSource[] = [];
  for (const rawChild of node.children) {
    const child = asMarkdownAstNode(rawChild);
    if (!child) {
      return null;
    }
    if (child.type === "element" && child.tagName === "img") {
      const src = child.properties?.src;
      const alt = child.properties?.alt;
      if (
        typeof src !== "string" ||
        !isChatGptWebCitationFavicon(src) ||
        (typeof alt === "string" && alt.trim().length > 0)
      ) {
        return null;
      }
      sources.push({ iconSrc: src, label: "" });
      continue;
    }
    if (child.type === "text" && typeof child.value === "string" && sources.length > 0) {
      sources[sources.length - 1].label += child.value;
      continue;
    }
    return null;
  }

  const normalized = sources.map((source) => ({
    ...source,
    label: source.label.trim(),
  }));
  return normalized.length > 0 && normalized.every((source) => source.label) ? normalized : null;
}

function chatGptWebCitationMarkdownRanges(content: string) {
  const ranges: Array<{ start: number; end: number }> = [];
  const linkPattern = /\[((?:!\[\]\((?:<[^>]+>|[^)\s]+)\)[^![\]\r\n]+)+)\]\((?:<[^>]+>|[^)\s]+)(?:\s+["'][^"']*["'])?\)/g;
  const sourcePattern = /!\[\]\((<[^>]+>|[^)\s]+)\)([^![\]\r\n]+)/g;
  let linkMatch: RegExpExecArray | null;
  while ((linkMatch = linkPattern.exec(content)) !== null) {
    const label = linkMatch[1] || "";
    let cursor = 0;
    let sourceCount = 0;
    let sourceMatch: RegExpExecArray | null;
    sourcePattern.lastIndex = 0;
    while ((sourceMatch = sourcePattern.exec(label)) !== null) {
      const rawSrc = sourceMatch[1] || "";
      const src = rawSrc.startsWith("<") && rawSrc.endsWith(">")
        ? rawSrc.slice(1, -1)
        : rawSrc;
      if (
        sourceMatch.index !== cursor ||
        !isChatGptWebCitationFavicon(src) ||
        !(sourceMatch[2] || "").trim()
      ) {
        sourceCount = 0;
        break;
      }
      cursor = sourcePattern.lastIndex;
      sourceCount += 1;
    }
    if (sourceCount > 0 && cursor === label.length) {
      ranges.push({ start: linkMatch.index, end: linkMatch.index + linkMatch[0].length });
    }
  }
  return ranges;
}

export function extractPreviewableMarkdownImages(content: string) {
  const images: Array<{ alt: string; src: string }> = [];
  const citationRanges = chatGptWebCitationMarkdownRanges(content);
  const imagePattern = /!\[([^\]]*)\]\((<[^>]+>|[^)\s]+)(?:\s+["'][^"']*["'])?\)/g;
  let match: RegExpExecArray | null;
  while ((match = imagePattern.exec(content)) !== null) {
    const rawSrc = match[2] || "";
    const src = rawSrc.startsWith("<") && rawSrc.endsWith(">")
      ? rawSrc.slice(1, -1)
      : rawSrc;
    const belongsToCitation = citationRanges.some(
      (range) => match!.index >= range.start && match!.index < range.end,
    );
    if (src && !belongsToCitation) {
      images.push({ alt: match[1] || "", src });
    }
  }
  return images;
}
