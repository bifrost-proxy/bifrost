import { languages, editor, Position } from 'monaco-editor';
import type { IRange } from 'monaco-editor';

export type ReferenceType = 'value' | 'requestScript' | 'responseScript' | 'parserScript';
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
  requestScripts: string[];
  responseScripts: string[];
  referenceLocations: Map<string, ReferenceLocation>;
}

let dynamicData: DynamicCompletionData = {
  values: [],
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

export const dynamicProvider: languages.CompletionItemProvider = {
  triggerCharacters: ['{', '/', ':'],
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

    if (textBeforeCursor.trim().startsWith('@')) {
      return { suggestions };
    }

    const word = model.getWordUntilPosition(position);
    const range: IRange = {
      startLineNumber: position.lineNumber,
      endLineNumber: position.lineNumber,
      startColumn: word.startColumn,
      endColumn: word.endColumn,
    };

    const reqScriptMatch = textBeforeCursor.match(/reqScript:\/\/([^\s]*)$/);
    const resScriptMatch = textBeforeCursor.match(/resScript:\/\/([^\s]*)$/);

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
