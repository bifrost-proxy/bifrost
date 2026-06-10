export interface RuleReferenceLineMatch {
  name: string;
  startColumn: number;
  endColumn: number;
}

export interface RuleReferenceCompletionContext {
  typedText: string;
  startColumn: number;
}

function isCommentDirective(trimmed: string): boolean {
  if (!trimmed.startsWith("@comment")) return false;
  const rest = trimmed.slice("@comment".length);
  return rest.length === 0 || rest.startsWith("#") || /^\s/.test(rest);
}

function inlineCommentIndex(value: string): number {
  for (let index = 0; index < value.length; index += 1) {
    if (value[index] !== "#") continue;
    if (index > 0 && /\s/.test(value[index - 1])) {
      return index;
    }
  }
  return -1;
}

export function parseRuleReferenceLine(lineContent: string): RuleReferenceLineMatch | null {
  const leadingWhitespace = lineContent.match(/^\s*/)?.[0].length ?? 0;
  const trimmed = lineContent.slice(leadingWhitespace);
  if (!trimmed.startsWith("@") || isCommentDirective(trimmed)) {
    return null;
  }

  const afterAt = trimmed.slice(1);
  const commentIndex = inlineCommentIndex(afterAt);
  const nameSegment = commentIndex >= 0 ? afterAt.slice(0, commentIndex) : afterAt;
  const nameLeadingWhitespace = nameSegment.match(/^\s*/)?.[0].length ?? 0;
  const rawName = nameSegment.trim();
  if (!rawName) return null;

  const startColumn = leadingWhitespace + 1;
  const endColumn = leadingWhitespace + 2 + nameLeadingWhitespace + rawName.length;
  return {
    name: rawName,
    startColumn,
    endColumn,
  };
}

export function findRuleReferenceAtColumn(
  lineContent: string,
  column: number,
): RuleReferenceLineMatch | null {
  const reference = parseRuleReferenceLine(lineContent);
  if (!reference) return null;
  return column >= reference.startColumn && column <= reference.endColumn ? reference : null;
}

export function getRuleReferenceCompletionContext(
  linePrefix: string,
  cursorColumn: number,
): RuleReferenceCompletionContext | null {
  const leadingWhitespace = linePrefix.match(/^\s*/)?.[0].length ?? 0;
  const trimmedPrefix = linePrefix.slice(leadingWhitespace);
  if (!trimmedPrefix.startsWith("@") || isCommentDirective(trimmedPrefix)) {
    return null;
  }

  const afterAt = trimmedPrefix.slice(1);
  if (inlineCommentIndex(afterAt) >= 0) {
    return null;
  }

  const nameLeadingWhitespace = afterAt.match(/^\s*/)?.[0].length ?? 0;
  const typedText = afterAt.slice(nameLeadingWhitespace);
  return {
    typedText,
    startColumn: Math.min(cursorColumn, leadingWhitespace + 2 + nameLeadingWhitespace),
  };
}
