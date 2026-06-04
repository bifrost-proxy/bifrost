import { describe, expect, it } from 'vitest';
import {
  BREAKPOINT_VALUE_COMPLETIONS,
  formatProtocolHover,
  getProtocolDoc,
} from './protocol-docs';

describe('rule protocol documentation', () => {
  it('documents bp parser rules with the required decode pairing', () => {
    const result = getProtocolDoc('bp');

    expect(result).toBeDefined();
    expect(result?.doc.category).toBe('script');
    expect(result?.doc.examples.join('\n')).toContain('decode://bp');
    expect(formatProtocolHover(result!)).toContain('bp://build_in_bp');
  });

  it('documents decode://bp as the parser-backed decoder', () => {
    const result = getProtocolDoc('decode');
    const hover = formatProtocolHover(result!);

    expect(result).toBeDefined();
    expect(result?.doc.valueSyntax).toContain('decode://bp');
    expect(hover).toContain('stored, displayed, and searched');
    expect(hover).toContain('bp://build_in_bp');
  });

  it('documents breakpoint rule phases and the global gate', () => {
    const result = getProtocolDoc('breakpoint');
    const hover = formatProtocolHover(result!);

    expect(result).toBeDefined();
    expect(result?.doc.examples).toContain('breakpoint://request');
    expect(result?.doc.examples).toContain('breakpoint://response');
    expect(hover).toContain('global Breakpoint switch');
    expect(hover).toContain('breakpoint://request,response');
  });

  it('provides breakpoint value completions for rule editor suggestions', () => {
    const values = BREAKPOINT_VALUE_COMPLETIONS.map((item) => item.value);

    expect(values).toEqual(
      expect.arrayContaining([
        'request',
        'req',
        'response',
        'res',
        'request,response',
        'both',
        'all',
      ]),
    );
    expect(BREAKPOINT_VALUE_COMPLETIONS[0].documentation).toContain(
      'global Breakpoint switch',
    );
  });
});
