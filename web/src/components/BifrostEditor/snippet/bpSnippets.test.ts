import { describe, expect, it } from 'vitest';
import { BUILT_IN_BP_PROTOCOL_SNIPPET, buildBpProtocolSnippets } from './bpSnippets';

describe('bp rule snippets', () => {
  it('pairs the built-in parser with decode://bp by default', () => {
    expect(buildBpProtocolSnippets([])).toEqual([BUILT_IN_BP_PROTOCOL_SNIPPET]);
  });

  it('pairs parser script suggestions with decode://bp and avoids a duplicate bare build_in_bp entry', () => {
    expect(
      buildBpProtocolSnippets([
        { name: 'build_in_bp' },
        { name: 'team/custom_parser' },
      ])
    ).toEqual([
      BUILT_IN_BP_PROTOCOL_SNIPPET,
      'bp://team/custom_parser decode://bp',
    ]);
  });
});
