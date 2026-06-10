import { languages, editor, Position } from 'monaco-editor';
import type { IRange } from 'monaco-editor';

export type ReferenceType = 'value' | 'requestScript' | 'responseScript' | 'parserScript' | 'rule';
export type NavigationType = 'page' | 'editor';

export interface ReferenceLocation {
  name: string;
  type: ReferenceType;
  navigationType: NavigationType;
  uri?: string;
  line?: number;
}

export interface DynamicCompletionData {
  values: string[];
  rules: string[];
  requestScripts: string[];
  responseScripts: string[];
  referenceLocations: Map<string, ReferenceLocation>;
}

let dynamicData: DynamicCompletionData = {
  values: [],
  rules: [],
  requestScripts: [],
  responseScripts: [],
  referenceLocations: new Map(),
};

export const updateDynamicData = (data: Partial<DynamicCompletionData>) => {
  dynamicData = { ...dynamicData, ...data };
};

export const getDynamicData = (): DynamicCompletionData => dynamicData;

export type LocalVariableGetter = () => Array<{ name: string; line: number }>;
let getLocalVariables: LocalVariableGetter = () => [];

export const setLocalVariablesGetter = (getter: LocalVariableGetter) => {
  getLocalVariables = getter;
};

const createValueSuggestions = (range: IRange): languages.CompletionItem[] => {
  const globalSuggestions = dynamicData.values.map((name) => ({
    label: `{${name}}`,
    kind: languages.CompletionItemKind.Variable,
    detail: `Global Value: ${name}`,
    documentation: `Reference to global value "${name}"`,
    insertText: `{${name}}`,
    range,
    sortText: `1_${name}`,
  }));

  const localVars = getLocalVariables();
  const localSuggestions = localVars.map(({ name, line }) => ({
    label: `{${name}}`,
    kind: languages.CompletionItemKind.Variable,
    detail: `Local Variable: ${name} (line ${line})`,
    documentation: `Reference to local variable "${name}" defined at line ${line}`,
    insertText: `{${name}}`,
    range,
    sortText: `0_${name}`,
  }));

  return [...localSuggestions, ...globalSuggestions];
};

const createScriptSuggestions = (
  range: IRange,
  type: 'request' | 'response'
): languages.CompletionItem[] => {
  const scripts =
    type === 'request' ? dynamicData.requestScripts : dynamicData.responseScripts;
  const prefix = type === 'request' ? 'reqScript' : 'resScript';

  return scripts.map((name) => ({
    label: `${prefix}://${name}`,
    kind: languages.CompletionItemKind.Reference,
    detail: `${type === 'request' ? 'Request' : 'Response'} Script: ${name}`,
    documentation: `Execute ${type} script "${name}"`,
    insertText: `${prefix}://${name}`,
    range,
    sortText: `0_${name}`,
  }));
};

const fuzzyMatch = (query: string, candidate: string): boolean => {
  if (!query) return true;
  const normalizedQuery = query.toLowerCase();
  const normalizedCandidate = candidate.toLowerCase();
  if (normalizedCandidate.includes(normalizedQuery)) return true;

  let queryIndex = 0;
  for (const char of normalizedCandidate) {
    if (char === normalizedQuery[queryIndex]) {
      queryIndex += 1;
      if (queryIndex === normalizedQuery.length) return true;
    }
  }
  return false;
};

const ruleSuggestionRank = (query: string, candidate: string): string => {
  const normalizedQuery = query.toLowerCase();
  const normalizedCandidate = candidate.toLowerCase();
  if (!normalizedQuery) return `2_${candidate}`;
  if (normalizedCandidate.startsWith(normalizedQuery)) return `0_${candidate}`;
  if (normalizedCandidate.includes(normalizedQuery)) return `1_${candidate}`;
  return `2_${candidate}`;
};

export const dynamicProvider: languages.CompletionItemProvider = {
  triggerCharacters: ['{', '/', ':', '@'],
  provideCompletionItems: (
    model: editor.ITextModel,
    position: Position
  ): languages.ProviderResult<languages.CompletionList> => {
    const suggestions: languages.CompletionItem[] = [];

    if (model.isDisposed()) {
      return { suggestions };
    }

    const lineContent = model.getLineContent(position.lineNumber);
    const textBeforeCursor = lineContent.substring(0, position.column - 1);

    const word = model.getWordUntilPosition(position);
    const range: IRange = {
      startLineNumber: position.lineNumber,
      endLineNumber: position.lineNumber,
      startColumn: word.startColumn,
      endColumn: word.endColumn,
    };

    const reqScriptMatch = textBeforeCursor.match(/reqScript:\/\/([^\s]*)$/);
    const resScriptMatch = textBeforeCursor.match(/resScript:\/\/([^\s]*)$/);
    const ruleReferenceMatch = textBeforeCursor.match(/(^|\s)@([^\s#]*)$/);

    if (ruleReferenceMatch) {
      const typedText = ruleReferenceMatch[2];
      const ruleRange: IRange = {
        ...range,
        startColumn: position.column - typedText.length,
      };
      const filteredRules = dynamicData.rules.filter((name) => fuzzyMatch(typedText, name));
      return {
        suggestions: filteredRules.map((name) => ({
          label: `@${name}`,
          kind: languages.CompletionItemKind.Reference,
          detail: `Rule Reference: ${name}`,
          documentation: `Inline-expand rule "${name}" at this position`,
          insertText: name,
          range: ruleRange,
          sortText: ruleSuggestionRank(typedText, name),
        })),
      };
    }

    if (reqScriptMatch) {
      const typedText = reqScriptMatch[1];
      const scriptRange: IRange = {
        ...range,
        startColumn: position.column - typedText.length,
      };
      const filteredScripts = typedText
        ? dynamicData.requestScripts.filter((name) =>
            name.toLowerCase().includes(typedText.toLowerCase())
          )
        : dynamicData.requestScripts;
      return {
        suggestions: filteredScripts.map((name) => ({
          label: name,
          kind: languages.CompletionItemKind.Reference,
          detail: `Request Script: ${name}`,
          documentation: `Execute request script "${name}"`,
          insertText: name,
          range: scriptRange,
          sortText: `0_${name}`,
        })),
      };
    }

    if (resScriptMatch) {
      const typedText = resScriptMatch[1];
      const scriptRange: IRange = {
        ...range,
        startColumn: position.column - typedText.length,
      };
      const filteredScripts = typedText
        ? dynamicData.responseScripts.filter((name) =>
            name.toLowerCase().includes(typedText.toLowerCase())
          )
        : dynamicData.responseScripts;
      return {
        suggestions: filteredScripts.map((name) => ({
          label: name,
          kind: languages.CompletionItemKind.Reference,
          detail: `Response Script: ${name}`,
          documentation: `Execute response script "${name}"`,
          insertText: name,
          range: scriptRange,
          sortText: `0_${name}`,
        })),
      };
    }

    const valueRefMatch = textBeforeCursor.match(/\{([^}\s]*)$/);
    if (valueRefMatch) {
      const valueRange: IRange = {
        ...range,
        startColumn: position.column - valueRefMatch[0].length,
      };

      const localVars = getLocalVariables();
      const localSuggestions = localVars.map(({ name, line }) => ({
        label: `{${name}}`,
        kind: languages.CompletionItemKind.Variable,
        detail: `Local Variable: ${name} (line ${line})`,
        documentation: `Reference to local variable "${name}" defined at line ${line}`,
        insertText: `{${name}}`,
        range: valueRange,
        sortText: `0_${name}`,
      }));

      const globalSuggestions = dynamicData.values.map((name) => ({
        label: `{${name}}`,
        kind: languages.CompletionItemKind.Variable,
        detail: `Global Value: ${name}`,
        documentation: `Reference to global value "${name}"`,
        insertText: `{${name}}`,
        range: valueRange,
        sortText: `1_${name}`,
      }));

      return {
        suggestions: [...localSuggestions, ...globalSuggestions],
      };
    }

    suggestions.push(...createValueSuggestions(range));
    suggestions.push(...createScriptSuggestions(range, 'request'));
    suggestions.push(...createScriptSuggestions(range, 'response'));

    return { suggestions };
  },
};

export default dynamicProvider;
