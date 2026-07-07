export type RuleEffectStatus = "active" | "partial" | "shadowed" | "neutral";

export interface RuleLineEffect {
  lineNumber: number;
  text: string;
  status: RuleEffectStatus;
  summary: string;
  details: string[];
  coveredByLine?: number;
}

interface ParsedOperation {
  id: number;
  lineIndex: number;
  lineNumber: number;
  opIndex: number;
  pattern: string;
  patternKey: string;
  protocol: string;
  value: string;
  priority: number;
  headers: HeaderEntry[];
}

interface OperationEffect {
  status: Exclude<RuleEffectStatus, "neutral">;
  reason: string;
  details: string[];
  coveredByLine?: number;
}

interface HeaderEntry {
  name: string;
  value: string;
}

const PROTOCOL_ALIASES: Record<string, string> = {
  ignore: "passthrough",
  pathReplace: "urlReplace",
  download: "attachment",
  "http-proxy": "proxy",
  h3: "http3",
  status: "statusCode",
  hosts: "host",
  html: "htmlAppend",
  js: "jsAppend",
  reqMerge: "params",
  css: "cssAppend",
};

const KNOWN_PROTOCOLS = new Set([
  "host",
  "xhost",
  "http",
  "https",
  "ws",
  "wss",
  "proxy",
  "http3",
  "pac",
  "redirect",
  "file",
  "tpl",
  "rawfile",
  "delete",
  "skip",
  "referer",
  "auth",
  "ua",
  "urlParams",
  "params",
  "resMerge",
  "replaceStatus",
  "statusCode",
  "method",
  "cache",
  "attachment",
  "reqScript",
  "resScript",
  "decode",
  "bp",
  "reqDelay",
  "resDelay",
  "headerReplace",
  "reqSpeed",
  "resSpeed",
  "reqType",
  "resType",
  "reqCharset",
  "resCharset",
  "reqCookies",
  "resCookies",
  "forwardedFor",
  "responseFor",
  "reqCors",
  "resCors",
  "reqHeaders",
  "resHeaders",
  "trailers",
  "reqPrepend",
  "resPrepend",
  "reqBody",
  "resBody",
  "reqAppend",
  "resAppend",
  "urlReplace",
  "reqReplace",
  "resReplace",
  "cssAppend",
  "htmlAppend",
  "jsAppend",
  "cssBody",
  "htmlBody",
  "jsBody",
  "cssPrepend",
  "htmlPrepend",
  "jsPrepend",
  "dns",
  "tlsIntercept",
  "tlsPassthrough",
  "tlsOptions",
  "upstreamUnsafeSsl",
  "sniCallback",
  "devtools",
  "breakpoint",
  "passthrough",
  "tunnel",
]);

const MULTI_MATCH_PROTOCOLS = new Set([
  "trailers",
  "urlParams",
  "params",
  "headerReplace",
  "reqHeaders",
  "resHeaders",
  "reqCors",
  "resCors",
  "reqCookies",
  "resCookies",
  "reqReplace",
  "urlReplace",
  "resReplace",
  "resMerge",
  "reqBody",
  "reqPrepend",
  "resPrepend",
  "reqAppend",
  "resAppend",
  "resBody",
  "htmlAppend",
  "jsAppend",
  "cssAppend",
  "htmlBody",
  "jsBody",
  "cssBody",
  "htmlPrepend",
  "jsPrepend",
  "cssPrepend",
  "reqScript",
  "resScript",
  "decode",
  "bp",
  "delete",
  "skip",
  "devtools",
  "breakpoint",
]);

const FORWARDING_PROTOCOLS = new Set([
  "http",
  "https",
  "ws",
  "wss",
  "host",
  "xhost",
  "passthrough",
]);

const PROTOCOL_TOKEN_RE = /^([A-Za-z][A-Za-z0-9-]*):\/\/(.*)$/s;

export function analyzeRuleEffectiveness(content: string): RuleLineEffect[] {
  const sourceLines = (content.trim() ? content : "# No active rules").split(/\r?\n/);
  const operations: ParsedOperation[] = [];
  const lineOperationIds = new Map<number, number[]>();
  let inFence = false;
  let nextOperationId = 0;

  const effects: RuleLineEffect[] = sourceLines.map((line, index) => {
    const lineNumber = index + 1;
    const trimmed = line.trim();

    if (trimmed.startsWith("```")) {
      inFence = !inFence;
      return neutralLine(lineNumber, line, "Value block delimiter");
    }
    if (inFence) {
      return neutralLine(lineNumber, line, "Value block content");
    }
    if (!trimmed) {
      return neutralLine(lineNumber, line, "Blank line");
    }
    if (trimmed.startsWith("#")) {
      return neutralLine(lineNumber, line, "Comment line");
    }

    const body = stripInlineComment(line);
    const parts = splitRuleParts(body);
    if (parts.length < 2) {
      return neutralLine(lineNumber, line, "No rule operation detected");
    }

    const pattern = parts[0];
    const patternKey = normalizePatternKey(pattern);
    const priority = estimateMatcherPriority(pattern, parts);
    const ids: number[] = [];

    parts.slice(1).forEach((part, opIndex) => {
      const parsed = parseProtocolToken(part);
      if (!parsed) return;
      const operation: ParsedOperation = {
        id: nextOperationId++,
        lineIndex: index,
        lineNumber,
        opIndex,
        pattern,
        patternKey,
        protocol: parsed.protocol,
        value: parsed.value,
        priority,
        headers: parsed.protocol === "reqHeaders" ? parseHeaderEntries(parsed.value) : [],
      };
      operations.push(operation);
      ids.push(operation.id);
    });

    if (ids.length === 0) {
      return neutralLine(lineNumber, line, "No supported rule protocol detected");
    }

    lineOperationIds.set(index, ids);
    return neutralLine(lineNumber, line, "Pending analysis");
  });

  const operationEffects = buildOperationEffects(operations);

  for (const [lineIndex, operationIds] of lineOperationIds.entries()) {
    const opEffects = operationIds
      .map((id) => operationEffects.get(id))
      .filter((effect): effect is OperationEffect => Boolean(effect));
    if (opEffects.length === 0) continue;

    const status = summarizeLineStatus(opEffects);
    const coveredByLine = nearestCoveringLine(opEffects);
    const summary = summarizeLineReason(status, opEffects, coveredByLine);
    const details = Array.from(
      new Set(opEffects.flatMap((effect) => effect.details).filter(Boolean)),
    );

    effects[lineIndex] = {
      ...effects[lineIndex],
      status,
      summary,
      details,
      coveredByLine,
    };
  }

  return effects;
}

function buildOperationEffects(operations: ParsedOperation[]): Map<number, OperationEffect> {
  const effects = new Map<number, OperationEffect>();
  const bySameProtocol = groupBy(operations, (op) => `${op.patternKey}\u0000${op.protocol}`);

  for (const sameProtocolOps of bySameProtocol.values()) {
    const sorted = sortByResolverOrder(sameProtocolOps);
    if (MULTI_MATCH_PROTOCOLS.has(sorted[0]?.protocol ?? "")) {
      for (const op of sorted) {
        effects.set(op.id, activeEffect(op, "Resolver selects this multi-match protocol."));
      }
      continue;
    }

    const winner = sorted[0];
    for (const op of sorted) {
      if (op.id === winner.id) {
        effects.set(
          op.id,
          activeEffect(op, "Resolver selects the first same matcher/protocol rule."),
        );
      } else {
        effects.set(op.id, {
          status: "shadowed",
          reason: `Covered by line ${winner.lineNumber}`,
          coveredByLine: winner.lineNumber,
          details: [
            `${op.protocol} is single-match, so the resolver keeps line ${winner.lineNumber} for the same matcher.`,
            matcherDetail(op),
          ],
        });
      }
    }
  }

  applyForwardingDecisionEffects(operations, effects);
  applyRequestHeaderEffects(operations, effects);

  return effects;
}

function applyForwardingDecisionEffects(
  operations: ParsedOperation[],
  effects: Map<number, OperationEffect>,
) {
  const routeOps = operations.filter((op) => FORWARDING_PROTOCOLS.has(op.protocol));
  const byPattern = groupBy(routeOps, (op) => op.patternKey);

  for (const ops of byPattern.values()) {
    const activeRouteOps = sortByResolverOrder(
      ops.filter((op) => effects.get(op.id)?.status !== "shadowed"),
    );
    const winner = activeRouteOps[0];
    if (!winner) continue;

    for (const op of activeRouteOps.slice(1)) {
      const current = effects.get(op.id);
      if (!current) continue;
      effects.set(op.id, {
        status: "shadowed",
        reason: `Forwarding decision is already taken by line ${winner.lineNumber}`,
        coveredByLine: winner.lineNumber,
        details: [
          `${winner.protocol} on line ${winner.lineNumber} wins the forwarding decision for this matcher.`,
          `${op.protocol} on line ${op.lineNumber} is still parsed, but it cannot change forwarding after that.`,
          matcherDetail(op),
        ],
      });
    }
  }
}

function applyRequestHeaderEffects(
  operations: ParsedOperation[],
  effects: Map<number, OperationEffect>,
) {
  const headerOps = sortByResolverOrder(
    operations.filter(
      (op) => op.protocol === "reqHeaders" && effects.get(op.id)?.status !== "shadowed",
    ),
  );
  const writers = new Map<string, ParsedOperation[]>();

  for (const op of headerOps) {
    for (const header of op.headers) {
      const key = `${op.patternKey}\u0000${header.name.toLowerCase()}`;
      const list = writers.get(key) ?? [];
      list.push(op);
      writers.set(key, list);
    }
  }

  const overwrittenByOperation = new Map<number, Set<number>>();
  const overwrittenHeaderNames = new Map<number, string[]>();

  for (const [key, ops] of writers.entries()) {
    if (ops.length < 2) continue;
    const headerName = key.split("\u0000").at(-1) ?? "header";
    const winner = ops[ops.length - 1];
    for (const op of ops.slice(0, -1)) {
      const covered = overwrittenByOperation.get(op.id) ?? new Set<number>();
      covered.add(winner.lineNumber);
      overwrittenByOperation.set(op.id, covered);
      const names = overwrittenHeaderNames.get(op.id) ?? [];
      names.push(headerName);
      overwrittenHeaderNames.set(op.id, names);
    }
  }

  for (const op of headerOps) {
    if (op.headers.length === 0) continue;
    const coveredLineSet = overwrittenByOperation.get(op.id);
    if (!coveredLineSet?.size) continue;

    const headerNames = Array.from(new Set(overwrittenHeaderNames.get(op.id) ?? []));
    const coveredByLine = Math.min(...coveredLineSet);
    const allHeadersCovered = headerNames.length >= new Set(op.headers.map((h) => h.name.toLowerCase())).size;

    effects.set(op.id, {
      status: allHeadersCovered ? "shadowed" : "partial",
      reason: allHeadersCovered
        ? `Request headers are replaced by line ${coveredByLine}`
        : `Some request headers are replaced by line ${coveredByLine}`,
      coveredByLine,
      details: [
        `Header ${headerNames.join(", ")} is written again later for the same matcher.`,
        `Final request header values come from the later selected reqHeaders rule.`,
        matcherDetail(op),
      ],
    });
  }
}

function activeEffect(op: ParsedOperation, reason: string): OperationEffect {
  return {
    status: "active",
    reason,
    details: [
      `${op.protocol} is ${MULTI_MATCH_PROTOCOLS.has(op.protocol) ? "multi-match" : "single-match"}.`,
      matcherDetail(op),
    ],
  };
}

function summarizeLineStatus(opEffects: OperationEffect[]): Exclude<RuleEffectStatus, "neutral"> {
  if (opEffects.every((effect) => effect.status === "shadowed")) return "shadowed";
  if (opEffects.some((effect) => effect.status !== "active")) return "partial";
  return "active";
}

function summarizeLineReason(
  status: Exclude<RuleEffectStatus, "neutral">,
  opEffects: OperationEffect[],
  coveredByLine?: number,
): string {
  if (status === "active") {
    return "Effective: selected by the resolver and not covered by a later field-level operation.";
  }
  if (status === "partial") {
    return `Partially effective: ${opEffects.find((effect) => effect.status !== "active")?.reason ?? "some operations are covered"}.`;
  }
  return `Not effective: ${opEffects[0]?.reason ?? `covered by line ${coveredByLine}`}.`;
}

function nearestCoveringLine(opEffects: OperationEffect[]): number | undefined {
  const lines = opEffects
    .map((effect) => effect.coveredByLine)
    .filter((line): line is number => typeof line === "number");
  return lines.length > 0 ? Math.min(...lines) : undefined;
}

function neutralLine(lineNumber: number, text: string, reason: string): RuleLineEffect {
  return {
    lineNumber,
    text,
    status: "neutral",
    summary: reason,
    details: [],
  };
}

function sortByResolverOrder<T extends ParsedOperation>(ops: T[]): T[] {
  return [...ops].sort(
    (left, right) =>
      right.priority - left.priority ||
      left.lineIndex - right.lineIndex ||
      left.opIndex - right.opIndex,
  );
}

function groupBy<T>(items: T[], keyOf: (item: T) => string): Map<string, T[]> {
  const grouped = new Map<string, T[]>();
  for (const item of items) {
    const key = keyOf(item);
    const list = grouped.get(key) ?? [];
    list.push(item);
    grouped.set(key, list);
  }
  return grouped;
}

function parseProtocolToken(token: string): { protocol: string; value: string } | null {
  const match = token.match(PROTOCOL_TOKEN_RE);
  if (!match) return null;
  const rawProtocol = match[1];
  const protocol = PROTOCOL_ALIASES[rawProtocol] ?? rawProtocol;
  if (!KNOWN_PROTOCOLS.has(protocol)) return null;
  return { protocol, value: match[2] ?? "" };
}

function stripInlineComment(line: string): string {
  let quote: string | null = null;
  let escaped = false;
  for (let i = 0; i < line.length; i += 1) {
    const char = line[i];
    if (escaped) {
      escaped = false;
      continue;
    }
    if (char === "\\") {
      escaped = true;
      continue;
    }
    if (quote) {
      if (char === quote) quote = null;
      continue;
    }
    if (char === '"' || char === "'") {
      quote = char;
      continue;
    }
    if (char === "#" && (i === 0 || /\s/.test(line[i - 1]))) {
      return line.slice(0, i).trimEnd();
    }
  }
  return line;
}

function splitRuleParts(line: string): string[] {
  const parts: string[] = [];
  let current = "";
  let quote: string | null = null;
  let escaped = false;
  let parenDepth = 0;
  let braceDepth = 0;
  let bracketDepth = 0;

  const push = () => {
    if (current.trim()) parts.push(current.trim());
    current = "";
  };

  for (const char of line.trim()) {
    if (escaped) {
      current += char;
      escaped = false;
      continue;
    }
    if (char === "\\") {
      current += char;
      escaped = true;
      continue;
    }
    if (quote) {
      current += char;
      if (char === quote) quote = null;
      continue;
    }
    if (char === '"' || char === "'") {
      current += char;
      quote = char;
      continue;
    }
    if (char === "(") parenDepth += 1;
    if (char === ")" && parenDepth > 0) parenDepth -= 1;
    if (char === "{") braceDepth += 1;
    if (char === "}" && braceDepth > 0) braceDepth -= 1;
    if (char === "[") bracketDepth += 1;
    if (char === "]" && bracketDepth > 0) bracketDepth -= 1;

    if (/\s/.test(char) && parenDepth === 0 && braceDepth === 0 && bracketDepth === 0) {
      push();
      continue;
    }
    current += char;
  }
  push();
  return parts;
}

function normalizePatternKey(pattern: string): string {
  return pattern.trim().replace(/^!/, "").toLowerCase();
}

function estimateMatcherPriority(pattern: string, parts: string[]): number {
  const normalized = pattern.trim().replace(/^!/, "");
  const importantBoost = parts.some((part) => part.startsWith("lineProps://") && part.includes("important")) ? 10_000 : 0;
  if (normalized.startsWith("^")) return 80 + importantBoost;

  if (normalized.includes("*")) {
    if (normalized.includes("/")) return 60 + importantBoost;
    if (normalized.startsWith("*.") || normalized.startsWith("*")) return 55 + importantBoost;
    return 45 + importantBoost;
  }

  const urlInfo = parsePatternUrl(normalized);
  if (urlInfo) {
    let priority = 100;
    if (urlInfo.hasProtocol) priority += 5;
    if (urlInfo.hasPort) priority += 10;
    if (urlInfo.path && urlInfo.path !== "/") {
      const segments = urlInfo.path.split("/").filter(Boolean).length;
      priority += urlInfo.path.endsWith("/") ? 10 + segments : 15 + segments;
    }
    return priority + importantBoost;
  }

  if (/^(?:\d{1,3}\.){3}\d{1,3}(?::\d+)?(?:\/.*)?$/.test(normalized)) {
    return 95 + importantBoost;
  }

  return 50 + importantBoost;
}

function parsePatternUrl(pattern: string):
  | { hasProtocol: boolean; hasPort: boolean; path: string }
  | null {
  const hasProtocol = /^[A-Za-z][A-Za-z0-9+.-]*:\/\//.test(pattern);
  const candidate = hasProtocol ? pattern : `http://${pattern}`;
  try {
    const parsed = new URL(candidate);
    if (!parsed.hostname.includes(".") && parsed.hostname !== "localhost") return null;
    return {
      hasProtocol,
      hasPort: parsed.port !== "",
      path: parsed.pathname,
    };
  } catch {
    return null;
  }
}

function parseHeaderEntries(value: string): HeaderEntry[] {
  const trimmed = value.trim();
  if (!trimmed) return [];

  if (trimmed.startsWith("{") && trimmed.endsWith("}")) {
    try {
      const parsed = JSON.parse(trimmed) as unknown;
      if (parsed && typeof parsed === "object" && !Array.isArray(parsed)) {
        return Object.entries(parsed).map(([name, rawValue]) => ({
          name,
          value: String(rawValue),
        }));
      }
    } catch {
      return [];
    }
  }

  const inline = trimmed.startsWith("(") && trimmed.endsWith(")")
    ? trimmed.slice(1, -1)
    : trimmed;

  return inline
    .split(/[,\n]/)
    .map((part) => part.trim())
    .map((part) => {
      const match = part.match(/^([^:=]+)[:=](.*)$/s);
      if (!match) return null;
      return {
        name: match[1].trim(),
        value: match[2].trim(),
      };
    })
    .filter((entry): entry is HeaderEntry => Boolean(entry?.name));
}

function matcherDetail(op: ParsedOperation): string {
  return `Matcher "${op.pattern}" has estimated priority ${op.priority}.`;
}
