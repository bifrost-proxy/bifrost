import { describe, expect, it } from 'vitest';
import { getReferenceLocation, localBpParserScriptName } from './hover';

describe('bp parser references', () => {
  it('extracts local parser script names for navigation', () => {
    expect(localBpParserScriptName('build_in_bp?protocol=thrift')).toBe('build_in_bp');
    expect(localBpParserScriptName('team/custom_parser#v1')).toBe('team/custom_parser');
  });

  it('builds a Scripts page target for local parser scripts', () => {
    expect(getReferenceLocation('build_in_bp', 'parserScript')).toMatchObject({
      navigationType: 'page',
      uri: '/scripts?type=parser&name=build_in_bp',
    });
  });

  it('does not treat remote URLs or absolute paths as local parser scripts', () => {
    expect(localBpParserScriptName('https://example.com/parser.js?sha256=abc')).toBeNull();
    expect(localBpParserScriptName('http://127.0.0.1:8080/parser.js?sha256=abc')).toBeNull();
    expect(localBpParserScriptName('file:///tmp/parser.js')).toBeNull();
    expect(localBpParserScriptName('/tmp/parser.js')).toBeNull();
    expect(localBpParserScriptName('../evil')).toBeNull();
    expect(localBpParserScriptName('team/../evil')).toBeNull();
  });
});
