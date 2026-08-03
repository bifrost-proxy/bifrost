import type { editor } from 'monaco-editor';
import { beforeAll, describe, expect, it, vi } from 'vitest';

const syntaxMocks = vi.hoisted(() => ({
  fetchSyntaxInfo: vi.fn().mockRejectedValue(new Error('desktop core is not ready')),
  refreshSyntaxInfo: vi.fn(),
  getCachedSyntaxInfo: vi.fn(() => null),
}));

vi.mock('./syntaxApi', () => syntaxMocks);

import operatorProvider from './operator';

const syntaxInfo = {
  protocols: [
    {
      name: 'reqHeaders',
      category: 'request',
      description: 'Modify request headers',
      value_type: 'headers',
      example: 'example.test reqHeaders://(X-Test=1)',
      aliases: [],
    },
  ],
  template_variables: [],
  patterns: [],
  protocol_aliases: {},
  scripts: {
    request_scripts: [],
    response_scripts: [],
    decode_scripts: [],
    parser_scripts: [],
  },
  filter_specs: [],
};

function modelFor(line: string): editor.ITextModel {
  return {
    isDisposed: () => false,
    getLineContent: () => line,
    getWordUntilPosition: () => ({
      word: 'req',
      startColumn: 7,
      endColumn: 10,
    }),
  } as unknown as editor.ITextModel;
}

beforeAll(async () => {
  // Let the module-level preload finish its deliberate startup failure.
  await Promise.resolve();
  await Promise.resolve();
});

describe('Bifrost operator completion recovery', () => {
  it('reloads syntax data when the startup preload failed', async () => {
    syntaxMocks.fetchSyntaxInfo.mockReset().mockResolvedValue(syntaxInfo);

    const result = await operatorProvider.provideCompletionItems(
      modelFor('a.com req'),
      { lineNumber: 1, column: 10 } as never,
      {} as never,
      {} as never,
    );

    expect(syntaxMocks.fetchSyntaxInfo).toHaveBeenCalledTimes(1);
    expect(result?.suggestions).toEqual(
      expect.arrayContaining([
        expect.objectContaining({
          label: expect.stringContaining('reqHeaders://'),
          insertText: expect.stringContaining('reqHeaders://'),
        }),
      ]),
    );
  });

  it('does not retry syntax loading for a disposed model', async () => {
    syntaxMocks.fetchSyntaxInfo.mockClear();

    const result = await operatorProvider.provideCompletionItems(
      { isDisposed: () => true } as editor.ITextModel,
      { lineNumber: 1, column: 1 } as never,
      {} as never,
      {} as never,
    );

    expect(syntaxMocks.fetchSyntaxInfo).not.toHaveBeenCalled();
    expect(result?.suggestions).toEqual([]);
  });
});
