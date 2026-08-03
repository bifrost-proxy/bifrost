import { editor, languages, Uri } from 'monaco-editor';

import snippet, {
  updateDynamicData,
  getDynamicData,
  dynamicProvider,
  hoverProvider,
  definitionProvider,
  codeActionProvider,
  signatureHelpProvider,
  validateRules,
  setValidationMarkers,
  clearValidationMarkers,
  createDebouncedValidator,
  setLocalVariables,
  setRuleReferenceErrors,
  executePendingNavigation,
  setLocalVariablesGetter,
} from "./snippet";
import type {
  DynamicCompletionData,
  ReferenceLocation,
  NavigateCallback,
  ParseError,
  ValidationResult,
  DebouncedValidator,
  GlobalValuesGetter,
  LocalVariableDefinition,
} from "./snippet";
import { setNavigateCallback } from "./snippet";
import theme from './theme';
import tokenizer from './tokenizer';

const LANGUAGE_BIFROST = 'bifrost';

languages.register({
  id: LANGUAGE_BIFROST,
  extensions: [`.${LANGUAGE_BIFROST}`],
});

languages.setLanguageConfiguration(LANGUAGE_BIFROST, tokenizer.config);
languages.setMonarchTokensProvider(LANGUAGE_BIFROST, tokenizer.language);

languages.registerCompletionItemProvider(LANGUAGE_BIFROST, snippet.operator);
languages.registerCompletionItemProvider(LANGUAGE_BIFROST, dynamicProvider);
languages.registerHoverProvider(LANGUAGE_BIFROST, hoverProvider);
languages.registerDefinitionProvider(LANGUAGE_BIFROST, definitionProvider);
languages.registerCodeActionProvider(LANGUAGE_BIFROST, codeActionProvider);
languages.registerSignatureHelpProvider(LANGUAGE_BIFROST, signatureHelpProvider);


editor.defineTheme(theme.dark.name, theme.dark.config);
editor.defineTheme(theme.light.name, theme.light.config);
editor.setTheme(theme.dark.name);

const originalCreate = editor.create;
editor.create = (
  domElement: HTMLElement,
  options?: editor.IStandaloneEditorConstructionOptions,
  override?: editor.IEditorOverrideServices
) => {
  const instance = originalCreate.call(
    editor,
    domElement,
    {
      language: LANGUAGE_BIFROST,
      readOnly: false,
      'semanticHighlighting.enabled': true,
      automaticLayout: true,
      smoothScrolling: true,
      links: false,
      quickSuggestions: {
        other: true,
        comments: false,
        strings: false,
      },
      suggestOnTriggerCharacters: true,
      wordBasedSuggestions: 'off',
      tabSize: 2,
      fontSize: 14,
      lineHeight: 22,
      wordWrap: 'on',
      minimap: { enabled: true },
      scrollBeyondLastLine: false,
      ...options,
    },
    override
  );

  instance.onMouseDown((e) => {
    if (e.event.metaKey || e.event.ctrlKey) {
      executePendingNavigation();
    }
  });

  return instance;
};

const originalCreateModel = editor.createModel;
editor.createModel = (
  value: string,
  language = LANGUAGE_BIFROST,
  uri?: Uri
) => originalCreateModel.call(editor, value, language, uri);

export const THEME_DARK = theme.dark.name;
export const THEME_LIGHT = theme.light.name;

export { updateDynamicData, getDynamicData, setNavigateCallback, setLocalVariables, setRuleReferenceErrors, setLocalVariablesGetter };
export { validateRules, setValidationMarkers, clearValidationMarkers, createDebouncedValidator };
export type { DynamicCompletionData, ReferenceLocation, NavigateCallback, LocalVariableDefinition };
export type { ParseError, ValidationResult, DebouncedValidator, GlobalValuesGetter };

export default editor;
