import { describe, expect, it } from 'vitest';
import { formatProtocolHover, getProtocolDoc } from './protocol-docs';

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
});
