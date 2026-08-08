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
  fields: FieldEntry[];
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

interface FieldEntry {
  name: string;
  value: string;
}

interface MatcherScope {
  kind: "global" | "url" | "host" | "unknown";
  raw: string;
  protocol?: string;
  host?: string;
  port?: string;
  path: string;
  hostMatcher: HostMatcher;
  hasWildcardPath: boolean;
}

interface HostMatcher {
  kind: "any" | "exact" | "suffix" | "contains" | "unknown";
  value: string;
}

type MatcherRelation = "same" | "superset" | "subset" | "overlap" | "disjoint" | "unknown";

interface FieldCoverage {
  fieldName: string;
  lineNumber: number;
  pattern: string;
  relation: Exclude<MatcherRelation, "disjoint" | "unknown">;
  protocol: string;
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
  "resStreamScript",
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
  "resStreamScript",
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

const KEYED_OVERRIDE_PROTOCOLS = new Set([
  "reqHeaders",
  "resHeaders",
  "reqCookies",
  "resCookies",
  "urlParams",
  "params",
  "trailers",
]);

const LAST_VALUE_MULTI_PROTOCOLS = new Set([
  "reqBody",
  "resBody",
  "reqPrepend",
  "resPrepend",
  "reqAppend",
  "resAppend",
  "resMerge",
  "reqCors",
  "resCors",
  "htmlAppend",
  "jsAppend",
  "cssAppend",
  "htmlBody",
  "jsBody",
  "cssBody",
  "htmlPrepend",
  "jsPrepend",
  "cssPrepend",
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
        fields: parseOperationFields(parsed.protocol, parsed.value),
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
  applyKeyedOverrideEffects(operations, effects);
  applyLastValueMultiMatchEffects(operations, effects);

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

function applyKeyedOverrideEffects(
  operations: ParsedOperation[],
  effects: Map<number, OperationEffect>,
) {
  const fieldOps = sortByFieldOverrideOrder(
    operations.filter(
      (op) => KEYED_OVERRIDE_PROTOCOLS.has(op.protocol) && effects.get(op.id)?.status !== "shadowed",
    ),
  );
  const fullCoverage = new Map<number, FieldCoverage[]>();
  const partialCoverage = new Map<number, FieldCoverage[]>();

  for (let opIndex = 0; opIndex < fieldOps.length; opIndex += 1) {
    const op = fieldOps[opIndex];
    if (op.fields.length === 0) continue;

    for (const field of op.fields) {
      const fieldName = field.name.toLowerCase();
      for (const prior of fieldOps.slice(0, opIndex)) {
        if (prior.protocol !== op.protocol) continue;
        if (!prior.fields.some((candidate) => candidate.name.toLowerCase() === fieldName)) {
          continue;
        }

        const relation = relateMatcherScopes(prior.pattern, op.pattern);
        if (relation === "disjoint" || relation === "unknown") continue;

        const coverage: FieldCoverage = {
          fieldName,
          lineNumber: prior.lineNumber,
          pattern: prior.pattern,
          relation,
          protocol: op.protocol,
        };

        if (relation === "same" || relation === "superset") {
          const list = fullCoverage.get(op.id) ?? [];
          list.push(coverage);
          fullCoverage.set(op.id, list);
          break;
        }

        const list = partialCoverage.get(op.id) ?? [];
        list.push(coverage);
        partialCoverage.set(op.id, list);
      }
    }
  }

  for (const op of fieldOps) {
    if (op.fields.length === 0) continue;
    const full = fullCoverage.get(op.id) ?? [];
    const partial = partialCoverage.get(op.id) ?? [];
    if (full.length === 0 && partial.length === 0) continue;

    const fullyCoveredFields = new Set(full.map((coverage) => coverage.fieldName));
    const partiallyCoveredFields = new Set(partial.map((coverage) => coverage.fieldName));
    const allFieldNames = new Set(op.fields.map((field) => field.name.toLowerCase()));
    const allFieldsFullyCovered = allFieldNames.size > 0
      && Array.from(allFieldNames).every((fieldName) => fullyCoveredFields.has(fieldName));
    const coveredByLine = Math.min(
      ...[...full, ...partial].map((coverage) => coverage.lineNumber),
    );

    effects.set(op.id, {
      status: allFieldsFullyCovered ? "shadowed" : "partial",
      reason: allFieldsFullyCovered
        ? `${op.protocol} fields are replaced by line ${coveredByLine}`
        : `${op.protocol} fields are partially covered by line ${coveredByLine}`,
      coveredByLine,
      details: keyedCoverageDetails(op, full, partial, fullyCoveredFields, partiallyCoveredFields),
    });
  }
}

function applyLastValueMultiMatchEffects(
  operations: ParsedOperation[],
  effects: Map<number, OperationEffect>,
) {
  const valueOps = sortByFieldOverrideOrder(
    operations.filter(
      (op) => LAST_VALUE_MULTI_PROTOCOLS.has(op.protocol) && effects.get(op.id)?.status !== "shadowed",
    ),
  );

  for (let opIndex = 0; opIndex < valueOps.length; opIndex += 1) {
    const op = valueOps[opIndex];
    const winner = valueOps.slice(0, opIndex).find((prior) => {
      if (prior.protocol !== op.protocol) return false;
      const relation = relateMatcherScopes(prior.pattern, op.pattern);
      return relation !== "disjoint" && relation !== "unknown";
    });
    if (!winner) continue;

    const relation = relateMatcherScopes(winner.pattern, op.pattern);
    const fullyCovered = relation === "same" || relation === "superset";
    effects.set(op.id, {
      status: fullyCovered ? "shadowed" : "partial",
      reason: fullyCovered
        ? `${op.protocol} value is replaced by line ${winner.lineNumber}`
        : `${op.protocol} value is partially covered by line ${winner.lineNumber}`,
      coveredByLine: winner.lineNumber,
      details: [
        `${op.protocol} keeps the highest-priority selected value; equal-priority duplicate values are resolved by later merged rule order.`,
        describeValueCoverage(winner, relation),
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

function sortByFieldOverrideOrder<T extends ParsedOperation>(ops: T[]): T[] {
  return [...ops].sort(
    (left, right) =>
      right.priority - left.priority ||
      right.lineIndex - left.lineIndex ||
      right.opIndex - left.opIndex,
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

function relateMatcherScopes(leftPattern: string, rightPattern: string): MatcherRelation {
  const left = parseMatcherScope(leftPattern);
  const right = parseMatcherScope(rightPattern);
  if (left.kind === "unknown" || right.kind === "unknown") return "unknown";

  const protocolRelation = relateOptionalExact(left.protocol, right.protocol);
  if (protocolRelation === "disjoint") return "disjoint";

  const portRelation = relateOptionalExact(left.port, right.port);
  if (portRelation === "disjoint") return "disjoint";

  const hostRelation = relateHostMatchers(left.hostMatcher, right.hostMatcher);
  if (hostRelation === "disjoint" || hostRelation === "unknown") return hostRelation;

  const pathRelation = relatePathScopes(left, right);
  if (pathRelation === "disjoint" || pathRelation === "unknown") return pathRelation;

  const relations = [protocolRelation, portRelation, hostRelation, pathRelation];
  if (relations.every((relation) => relation === "same")) return "same";
  if (relations.every((relation) => relation === "same" || relation === "superset")) {
    return "superset";
  }
  if (relations.every((relation) => relation === "same" || relation === "subset")) {
    return "subset";
  }
  return "overlap";
}

function parseMatcherScope(pattern: string): MatcherScope {
  const raw = pattern.trim().replace(/^!/, "");
  if (!raw || raw.startsWith("^")) {
    return unknownMatcherScope(pattern);
  }

  if (raw === "*") {
    return {
      kind: "global",
      raw,
      path: "/",
      hostMatcher: { kind: "any", value: "*" },
      hasWildcardPath: false,
    };
  }

  const hasProtocol = /^[A-Za-z][A-Za-z0-9+.-]*:\/\//.test(raw);
  const candidate = hasProtocol ? raw : `http://${raw}`;
  try {
    const parsed = new URL(candidate);
    const hostMatcher = parseHostMatcher(parsed.hostname);
    if (hostMatcher.kind === "unknown") return unknownMatcherScope(pattern);

    const path = normalizeScopePath(parsed.pathname || "/");
    return {
      kind: hasProtocol ? "url" : "host",
      raw,
      protocol: hasProtocol ? parsed.protocol.replace(/:$/, "").toLowerCase() : undefined,
      host: parsed.hostname.toLowerCase(),
      port: parsed.port || undefined,
      path,
      hostMatcher,
      hasWildcardPath: path.includes("*"),
    };
  } catch {
    const hostWithPort = raw.match(/^([^/:]+)(?::(\d+))?(?:\/(.*))?$/);
    if (!hostWithPort) return unknownMatcherScope(pattern);

    const hostMatcher = parseHostMatcher(hostWithPort[1] ?? "");
    if (hostMatcher.kind === "unknown") return unknownMatcherScope(pattern);
    return {
      kind: "host",
      raw,
      host: hostWithPort[1]?.toLowerCase(),
      port: hostWithPort[2],
      path: normalizeScopePath(hostWithPort[3] ? `/${hostWithPort[3]}` : "/"),
      hostMatcher,
      hasWildcardPath: Boolean(hostWithPort[3]?.includes("*")),
    };
  }
}

function unknownMatcherScope(raw: string): MatcherScope {
  return {
    kind: "unknown",
    raw,
    path: "/",
    hostMatcher: { kind: "unknown", value: "" },
    hasWildcardPath: false,
  };
}

function parseHostMatcher(hostname: string): HostMatcher {
  const host = hostname.trim().toLowerCase();
  if (!host) return { kind: "unknown", value: "" };
  if (host === "*") return { kind: "any", value: "*" };
  if (host.includes("[") || host.includes("]")) return { kind: "unknown", value: host };
  if (host.startsWith("*.")) {
    const suffix = host.slice(1);
    return suffix.length > 1
      ? { kind: "suffix", value: suffix }
      : { kind: "unknown", value: host };
  }
  if (host.startsWith("*") && host.length > 1) {
    return { kind: "contains", value: host.slice(1) };
  }
  if (host.endsWith("*") && host.length > 1) {
    return { kind: "contains", value: host.slice(0, -1) };
  }
  if (host.includes("*")) return { kind: "unknown", value: host };
  return { kind: "exact", value: host };
}

function relateOptionalExact(left?: string, right?: string): MatcherRelation {
  if (!left && !right) return "same";
  if (!left && right) return "superset";
  if (left && !right) return "subset";
  return left === right ? "same" : "disjoint";
}

function relateHostMatchers(left: HostMatcher, right: HostMatcher): MatcherRelation {
  if (left.kind === "unknown" || right.kind === "unknown") return "unknown";
  if (left.kind === "any" && right.kind === "any") return "same";
  if (left.kind === "any") return "superset";
  if (right.kind === "any") return "subset";

  if (left.kind === "exact" && right.kind === "exact") {
    return left.value === right.value ? "same" : "disjoint";
  }

  if (left.kind === "suffix" && right.kind === "suffix") {
    if (left.value === right.value) return "same";
    if (right.value.endsWith(left.value)) return "superset";
    if (left.value.endsWith(right.value)) return "subset";
    return "disjoint";
  }

  if (left.kind === "suffix" && right.kind === "exact") {
    return hostMatchesSuffix(right.value, left.value) ? "superset" : "disjoint";
  }

  if (left.kind === "exact" && right.kind === "suffix") {
    return hostMatchesSuffix(left.value, right.value) ? "subset" : "disjoint";
  }

  if (left.kind === "contains" && right.kind === "exact") {
    return right.value.includes(left.value) ? "superset" : "disjoint";
  }

  if (left.kind === "exact" && right.kind === "contains") {
    return left.value.includes(right.value) ? "subset" : "disjoint";
  }

  if (left.kind === "contains" && right.kind === "contains") {
    if (left.value === right.value) return "same";
    if (right.value.includes(left.value)) return "superset";
    if (left.value.includes(right.value)) return "subset";
    return "overlap";
  }

  if (left.kind === "suffix" && right.kind === "contains") {
    return left.value.includes(right.value) ? "subset" : "overlap";
  }

  if (left.kind === "contains" && right.kind === "suffix") {
    return right.value.includes(left.value) ? "superset" : "overlap";
  }

  return "unknown";
}

function relatePathScopes(left: MatcherScope, right: MatcherScope): MatcherRelation {
  if (left.hasWildcardPath || right.hasWildcardPath) {
    return relateWildcardPaths(left.path, right.path);
  }

  if (left.path === right.path) return "same";
  if (pathContains(left.path, right.path)) return "superset";
  if (pathContains(right.path, left.path)) return "subset";
  return "disjoint";
}

function relateWildcardPaths(leftPath: string, rightPath: string): MatcherRelation {
  const leftPrefix = wildcardLiteralPrefix(leftPath);
  const rightPrefix = wildcardLiteralPrefix(rightPath);
  if (!leftPrefix || !rightPrefix) return "unknown";
  if (leftPrefix === rightPrefix) return "overlap";
  if (pathContains(leftPrefix, rightPrefix)) return "superset";
  if (pathContains(rightPrefix, leftPrefix)) return "subset";
  return "disjoint";
}

function wildcardLiteralPrefix(path: string): string | null {
  const starIndex = path.indexOf("*");
  const prefix = starIndex >= 0 ? path.slice(0, starIndex) : path;
  if (!prefix) return null;
  return normalizeScopePath(prefix);
}

function normalizeScopePath(path: string): string {
  const normalized = path.startsWith("/") ? path : `/${path}`;
  return normalized || "/";
}

function pathContains(leftPath: string, rightPath: string): boolean {
  if (leftPath === "/") return true;
  const left = leftPath.endsWith("/") ? leftPath : `${leftPath}/`;
  const right = rightPath.endsWith("/") ? rightPath : `${rightPath}/`;
  return right.startsWith(left);
}

function hostMatchesSuffix(host: string, suffix: string): boolean {
  return host.endsWith(suffix) || host === suffix.replace(/^\./, "");
}

function keyedCoverageDetails(
  op: ParsedOperation,
  full: FieldCoverage[],
  partial: FieldCoverage[],
  fullyCoveredFields: Set<string>,
  partiallyCoveredFields: Set<string>,
): string[] {
  const details: string[] = [];
  if (fullyCoveredFields.size > 0) {
    details.push(
      `${op.protocol} field ${Array.from(fullyCoveredFields).join(", ")} is written by a higher-priority or later selected rule.`,
    );
  }
  if (partiallyCoveredFields.size > 0) {
    details.push(
      `${op.protocol} field ${Array.from(partiallyCoveredFields).join(", ")} is overridden only for overlapping matcher traffic.`,
    );
  }

  for (const coverage of [...full, ...partial]) {
    details.push(describeFieldCoverage(coverage));
  }

  details.push(
    `${op.protocol} uses resolver priority first; when the same field is selected more than once, the later rule in the merged list wins for equal-priority matchers.`,
    matcherDetail(op),
  );
  return Array.from(new Set(details));
}

function describeFieldCoverage(coverage: FieldCoverage): string {
  if (coverage.relation === "same") {
    return `Line ${coverage.lineNumber} has the same matcher and wins ${coverage.fieldName}.`;
  }
  if (coverage.relation === "superset") {
    return `Line ${coverage.lineNumber} matcher "${coverage.pattern}" covers all traffic for this matcher and writes ${coverage.fieldName}.`;
  }
  if (coverage.relation === "subset") {
    return `Line ${coverage.lineNumber} matcher "${coverage.pattern}" covers a narrower part of this matcher, so this rule still applies outside that narrower scope.`;
  }
  return `Line ${coverage.lineNumber} matcher "${coverage.pattern}" overlaps this matcher, so this rule remains effective outside the overlap.`;
}

function describeValueCoverage(op: ParsedOperation, relation: MatcherRelation): string {
  if (relation === "same") {
    return `Line ${op.lineNumber} has the same matcher and wins this ${op.protocol} value.`;
  }
  if (relation === "superset") {
    return `Line ${op.lineNumber} matcher "${op.pattern}" covers all traffic for this matcher.`;
  }
  if (relation === "subset") {
    return `Line ${op.lineNumber} matcher "${op.pattern}" covers a narrower part of this matcher, so this rule still applies outside that narrower scope.`;
  }
  return `Line ${op.lineNumber} matcher "${op.pattern}" overlaps this matcher, so this rule remains effective outside the overlap.`;
}

function parseOperationFields(protocol: string, value: string): FieldEntry[] {
  if (!KEYED_OVERRIDE_PROTOCOLS.has(protocol)) return [];
  if (protocol === "urlParams" || protocol === "params") {
    return parseFieldEntries(value);
  }
  return parseHeaderEntries(value);
}

function parseFieldEntries(value: string): FieldEntry[] {
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
    .split(/[,&\n]/)
    .map((part) => part.trim())
    .map((part) => {
      const match = part.match(/^([^:=]+)[:=](.*)$/s);
      if (!match) return null;
      return {
        name: match[1].trim(),
        value: match[2].trim(),
      };
    })
    .filter((entry): entry is FieldEntry => Boolean(entry?.name));
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
    .split(/[,&\n]/)
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
