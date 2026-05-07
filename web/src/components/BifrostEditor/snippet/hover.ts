import { languages, editor, Position } from 'monaco-editor';
import type { IRange } from 'monaco-editor';
import { getDynamicData, type ReferenceLocation, type ReferenceType } from './dynamic';
import { getProtocolDoc, formatProtocolHover } from './protocol-docs';

export interface ReferenceMatch {
  name: string;
  type: ReferenceType;
  range: IRange;
}

interface ProtocolMatch {
  name: string;
  range: IRange;
}

const VALUE_REF_PATTERN = /\{([\w-]+)\}/g;
const REQ_SCRIPT_PATTERN = /reqScript:\/\/([\w\-.]+)/g;
const RES_SCRIPT_PATTERN = /resScript:\/\/([\w\-.]+)/g;
const BP_SCRIPT_PATTERN = /bp:\/\/([^\s]+)/g;

export function localBpParserScriptName(rawValue: string): string | null {
  const localName = rawValue.split(/[?#]/, 1)[0];
  if (
    !localName ||
    localName.startsWith('/') ||
    localName.endsWith('/') ||
    localName.includes('//') ||
    localName.split('/').includes('..') ||
    /^[a-z][a-z0-9+.-]*:\/\//i.test(localName) ||
    !/^[A-Za-z0-9_-]+(?:\/[A-Za-z0-9_-]+)*$/.test(localName)
  ) {
    return null;
  }
  return localName;
}

function findReferenceAtPosition(
  model: editor.ITextModel,
  position: Position
): ReferenceMatch | null {
  const lineContent = model.getLineContent(position.lineNumber);
  const column = position.column;

  const patterns: { pattern: RegExp; type: ReferenceType }[] = [
    { pattern: VALUE_REF_PATTERN, type: 'value' },
    { pattern: REQ_SCRIPT_PATTERN, type: 'requestScript' },
    { pattern: RES_SCRIPT_PATTERN, type: 'responseScript' },
    { pattern: BP_SCRIPT_PATTERN, type: 'parserScript' },
  ];

  for (const { pattern, type } of patterns) {
    pattern.lastIndex = 0;
    let match;
    while ((match = pattern.exec(lineContent)) !== null) {
      const startCol = match.index + 1;
      const endCol = match.index + match[0].length + 1;

      if (column >= startCol && column <= endCol) {
        const name =
          type === 'parserScript'
            ? localBpParserScriptName(match[1])
            : match[1];
        if (!name) {
          continue;
        }

        return {
          name,
          type,
          range: {
            startLineNumber: position.lineNumber,
            endLineNumber: position.lineNumber,
            startColumn: startCol,
            endColumn: endCol,
          },
        };
      }
    }
  }

  return null;
}

const PROTOCOL_PATTERN = /(\w+):\/\//g;

function findProtocolAtPosition(
  model: editor.ITextModel,
  position: Position
): ProtocolMatch | null {
  const lineContent = model.getLineContent(position.lineNumber);
  const column = position.column;

  PROTOCOL_PATTERN.lastIndex = 0;
  let match;
  while ((match = PROTOCOL_PATTERN.exec(lineContent)) !== null) {
    const startCol = match.index + 1;
    const endCol = match.index + match[0].length + 1;

    if (column >= startCol && column <= endCol) {
      return {
        name: match[1],
        range: {
          startLineNumber: position.lineNumber,
          endLineNumber: position.lineNumber,
          startColumn: startCol,
          endColumn: endCol,
        },
      };
    }
  }

  return null;
}

export function getReferenceLocation(
  name: string,
  type: ReferenceType
): ReferenceLocation | undefined {
  const data = getDynamicData();

  const key = `${type}:${name}`;
  const location = data.referenceLocations.get(key);
  if (location) return location;

  switch (type) {
    case 'value': {
      const localVar = findLocalVariableDefinition(name);
      if (localVar) {
        return {
          name,
          type: 'value',
          navigationType: 'editor',
          line: localVar.line,
        };
      }
      return {
        name,
        type: 'value',
        navigationType: 'page',
        uri: `/values?name=${encodeURIComponent(name)}`,
      };
    }
    case 'requestScript':
      return {
        name,
        type: 'requestScript',
        navigationType: 'page',
        uri: `/scripts?type=request&name=${encodeURIComponent(name)}`,
      };
    case 'responseScript':
      return {
        name,
        type: 'responseScript',
        navigationType: 'page',
        uri: `/scripts?type=response&name=${encodeURIComponent(name)}`,
      };
    case 'parserScript':
      return {
        name,
        type: 'parserScript',
        navigationType: 'page',
        uri: `/scripts?type=parser&name=${encodeURIComponent(name)}`,
      };
  }

  return undefined;
}

function getTypeLabel(type: ReferenceType): string {
  switch (type) {
    case 'value':
      return 'Global Value';
    case 'requestScript':
      return 'Request Script';
    case 'responseScript':
      return 'Response Script';
    case 'parserScript':
      return 'Parser Script';
  }
}

export type NavigateCallback = (location: ReferenceLocation) => void;

let onNavigate: NavigateCallback | null = null;

export const setNavigateCallback = (callback: NavigateCallback | null) => {
  onNavigate = callback;
};

export const getNavigateCallback = (): NavigateCallback | null => onNavigate;

export interface LocalVariableDefinition {
  name: string;
  line: number;
}

let localVariables: LocalVariableDefinition[] = [];

export const setLocalVariables = (variables: LocalVariableDefinition[]) => {
  localVariables = variables;
};

export const getLocalVariables = (): LocalVariableDefinition[] => localVariables;

function findLocalVariableDefinition(name: string): LocalVariableDefinition | undefined {
  return localVariables.find((v) => v.name === name);
}

export const hoverProvider: languages.HoverProvider = {
  provideHover: (
    model: editor.ITextModel,
    position: Position
  ): languages.ProviderResult<languages.Hover> => {
    if (model.isDisposed()) return null;

    const protocolMatch = findProtocolAtPosition(model, position);
    if (protocolMatch) {
      const result = getProtocolDoc(protocolMatch.name);
      if (result) {
        return {
          range: protocolMatch.range,
          contents: [{ value: formatProtocolHover(result) }],
        };
      }
    }

    const reference = findReferenceAtPosition(model, position);
    if (!reference) return null;

    const location = getReferenceLocation(reference.name, reference.type);
    if (!location) return null;

    const typeLabel = getTypeLabel(reference.type);
    const actionText =
      location.navigationType === "page"
        ? "Go to definition"
        : "Jump to definition";

    const displayTypeLabel = reference.type === 'value' && location.navigationType === 'editor'
      ? 'Local Variable'
      : typeLabel;
    const hintText = '(F12 or Cmd+Click)';
    const contents = [
      {
        value: `**${displayTypeLabel}**: \`${reference.name}\`\n\n${actionText} ${hintText}`,
      },
    ];

    return {
      range: reference.range,
      contents,
    };
  },
};

let pendingNavigation: ReferenceLocation | null = null;

export const definitionProvider: languages.DefinitionProvider = {
  provideDefinition: (
    model: editor.ITextModel,
    position: Position
  ): languages.ProviderResult<languages.Definition> => {
    if (model.isDisposed()) return null;

    const reference = findReferenceAtPosition(model, position);
    if (!reference) return null;

    const location = getReferenceLocation(reference.name, reference.type);
    if (!location) return null;

    if (location.navigationType === 'editor' && location.line !== undefined) {
      pendingNavigation = null;
      return {
        uri: model.uri,
        range: {
          startLineNumber: location.line,
          startColumn: 1,
          endLineNumber: location.line,
          endColumn: 1,
        },
      };
    }

    pendingNavigation = location;
    return null;
  },
};

export const executePendingNavigation = (): boolean => {
  if (pendingNavigation && onNavigate) {
    onNavigate(pendingNavigation);
    pendingNavigation = null;
    return true;
  }
  return false;
};
