import type { ScriptListItem } from './syntaxApi';

export const BUILT_IN_BP_PROTOCOL_SNIPPET =
  'bp://build_in_bp?psm=${1:psm}&method=${2:method} decode://bp';

export const BUILT_IN_BP_VALUE_SNIPPET =
  'build_in_bp?psm=${1:psm}&method=${2:method} decode://bp';

export function buildBpProtocolSnippets(parserScripts: ScriptListItem[]): string[] {
  const parserSnippets = parserScripts
    .map((script) => `bp://${script.name} decode://bp`)
    .filter((snippet) => snippet !== 'bp://build_in_bp decode://bp');

  return [BUILT_IN_BP_PROTOCOL_SNIPPET, ...parserSnippets];
}
