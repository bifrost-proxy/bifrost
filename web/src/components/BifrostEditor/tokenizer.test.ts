import { describe, expect, it } from 'vitest';
import tokenizer from './tokenizer';
import {
  getRuleReferenceCompletionContext,
  parseRuleReferenceLine,
} from './snippet/ruleReference';

function rootTokenClasses(line: string): string[] {
  const root = tokenizer.language.tokenizer.root;
  const classes: string[] = [];
  let remaining = line;

  while (remaining.length > 0) {
    const rule = root.find((entry) => {
      if (!Array.isArray(entry)) return false;
      const pattern = entry[0] as unknown;
      if (!(pattern instanceof RegExp)) return false;
      pattern.lastIndex = 0;
      const match = pattern.exec(remaining);
      return !!match && match.index === 0 && match[0].length > 0;
    }) as unknown[] | undefined;
    if (!rule) {
      remaining = remaining.slice(1);
      continue;
    }

    const pattern = rule[0] as RegExp;
    const match = pattern.exec(remaining)!;
    const action = rule[1];
    if (Array.isArray(action)) {
      classes.push(...action.filter((token): token is string => typeof token === 'string'));
    } else if (typeof action === 'string') {
      classes.push(action);
    }
    remaining = remaining.slice(match[0].length);
  }

  return classes;
}

describe('Bifrost tokenizer', () => {
  it('keeps bp query parameters inside the protocol value before the next protocol', () => {
    const classes = rootTokenClasses('?psm=psm&method=method decode://bp');

    expect(classes).toContain('string');
    expect(classes).not.toContain('attribute.value');
  });

  it('marks at-rule references as clickable references', () => {
    const classes = rootTokenClasses('@shared-rule');

    expect(classes).toContain('reference.user');
  });

  it('matches rule references only on standalone at-rule lines', () => {
    expect(parseRuleReferenceLine('@shared-rule')?.name).toBe('shared-rule');
    expect(parseRuleReferenceLine('  @team/shared\t# reuse')?.name).toBe('team/shared');
    expect(parseRuleReferenceLine('@commented-shared')?.name).toBe('commented-shared');
    expect(parseRuleReferenceLine('@comment not a reference')).toBeNull();
    expect(parseRuleReferenceLine('# @shared-rule')).toBeNull();
    expect(parseRuleReferenceLine('example.com @shared-rule')).toBeNull();
  });

  it('only offers rule reference completion on standalone at-rule lines', () => {
    expect(getRuleReferenceCompletionContext('@sh', 4)).toMatchObject({
      typedText: 'sh',
      startColumn: 2,
    });
    expect(getRuleReferenceCompletionContext('  @te', 6)).toMatchObject({
      typedText: 'te',
      startColumn: 4,
    });
    expect(getRuleReferenceCompletionContext('# @sh', 6)).toBeNull();
    expect(getRuleReferenceCompletionContext('example.com @sh', 16)).toBeNull();
    expect(getRuleReferenceCompletionContext('@shared # comment', 18)).toBeNull();
  });
});
