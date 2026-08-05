export type HeaderDiffType = "added" | "modified" | "deleted" | "unchanged";
export type HeaderChangeSource = "configured" | "protocol";

export interface HeaderDiffItem {
  key: string;
  name: string;
  value: string;
  diffType: HeaderDiffType;
  changeSource?: HeaderChangeSource;
  originalValue?: string;
}

export interface HeaderDiffSummary {
  configured: number;
  protocol: number;
}

export interface HeaderDiffResult {
  items: HeaderDiffItem[];
  summary: HeaderDiffSummary;
}

const STANDARD_HOP_BY_HOP_HEADERS = new Set([
  "connection",
  "keep-alive",
  "proxy-authenticate",
  "proxy-authorization",
  "proxy-connection",
  "te",
  "trailer",
  "transfer-encoding",
  "upgrade",
]);

const protocolManagedHeaderNames = (originalHeaders: [string, string][]) => {
  const names = new Set(STANDARD_HOP_BY_HOP_HEADERS);
  for (const [name, value] of originalHeaders) {
    if (name.toLowerCase() !== "connection") continue;
    for (const token of value.split(",")) {
      const normalized = token.trim().toLowerCase();
      if (normalized) names.add(normalized);
    }
  }
  return names;
};

const classifySource = (
  name: string,
  protocolManagedNames: Set<string>,
): HeaderChangeSource =>
  protocolManagedNames.has(name.toLowerCase()) ? "protocol" : "configured";

export const areHeadersEqual = (
  left: [string, string][] | null | undefined,
  right: [string, string][] | null | undefined,
): boolean => {
  if (left === right) return true;
  if (!left || !right) return !left && !right;
  if (left.length !== right.length) return false;
  const normalize = (headers: [string, string][]) =>
    headers
      .map(([name, value]) => [name.toLowerCase(), value] as const)
      .sort(([leftName, leftValue], [rightName, rightValue]) =>
        leftName.localeCompare(rightName) || leftValue.localeCompare(rightValue),
      );
  const normalizedLeft = normalize(left);
  const normalizedRight = normalize(right);
  return normalizedLeft.every(([leftName, leftValue], index) => {
    const [rightName, rightValue] = normalizedRight[index] ?? [];
    return leftName === rightName && leftValue === rightValue;
  });
};

export const buildHeaderDiff = (
  currentHeaders: [string, string][],
  originalHeaders: [string, string][],
): HeaderDiffResult => {
  const protocolManagedNames = protocolManagedHeaderNames(originalHeaders);
  const originalByName = new Map<
    string,
    Array<{ name: string; value: string }>
  >();
  for (const [name, value] of originalHeaders) {
    const lowerName = name.toLowerCase();
    const values = originalByName.get(lowerName) ?? [];
    values.push({ name, value });
    originalByName.set(lowerName, values);
  }

  const usedOriginalCount = new Map<string, number>();
  const currentNameCount = new Map<string, number>();
  const active = [...currentHeaders]
    .sort(([left], [right]) => left.localeCompare(right))
    .map(([name, value], index): HeaderDiffItem => {
      const lowerName = name.toLowerCase();
      currentNameCount.set(
        lowerName,
        (currentNameCount.get(lowerName) ?? 0) + 1,
      );
      const originalValues = originalByName.get(lowerName);
      if (!originalValues?.length) {
        return {
          key: `current-${index}`,
          name,
          value,
          diffType: "added",
          changeSource: classifySource(name, protocolManagedNames),
        };
      }

      const originalIndex = usedOriginalCount.get(lowerName) ?? 0;
      usedOriginalCount.set(lowerName, originalIndex + 1);
      const original = originalValues[originalIndex];
      if (!original) {
        return {
          key: `current-${index}`,
          name,
          value,
          diffType: "added",
          changeSource: classifySource(name, protocolManagedNames),
        };
      }
      if (original.value !== value) {
        return {
          key: `current-${index}`,
          name,
          value,
          diffType: "modified",
          changeSource: classifySource(name, protocolManagedNames),
          originalValue: original.value,
        };
      }
      return { key: `current-${index}`, name, value, diffType: "unchanged" };
    });

  const deleted: HeaderDiffItem[] = [];
  for (const [lowerName, originals] of originalByName) {
    const currentCount = currentNameCount.get(lowerName) ?? 0;
    for (let index = currentCount; index < originals.length; index += 1) {
      const original = originals[index];
      deleted.push({
        key: `deleted-${lowerName}-${index}`,
        name: original.name,
        value: original.value,
        diffType: "deleted",
        changeSource: classifySource(original.name, protocolManagedNames),
      });
    }
  }
  deleted.sort((left, right) => left.name.localeCompare(right.name));

  const items = [...active, ...deleted];
  const summary = items.reduce<HeaderDiffSummary>(
    (counts, item) => {
      if (item.changeSource) counts[item.changeSource] += 1;
      return counts;
    },
    { configured: 0, protocol: 0 },
  );
  return { items, summary };
};
